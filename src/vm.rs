//! La máquina virtual (VM) de raylang (M2, con GC en M4.3).
//!
//! Ejecuta bytecode sobre una **pila de operandos** y una **pila de marcos de
//! llamada** explícita (no la pila de Rust). Reificar los marcos así es lo que
//! mantiene abierta la puerta a la concurrencia (ver IDEAS.md §1) y, ahora, lo que
//! hace **enumerables las raíces** del recolector de basura.
//!
//! ## Memoria (M4.3)
//!
//! Los datos compuestos (arreglos, structs, closures y celdas) viven en un **heap
//! propio** (`gc::Heap`) y se referencian por *handle*. Un recolector
//! **mark-and-sweep** los libera —incluidos los ciclos, que el `Rc` del intérprete
//! no puede—. El intérprete se queda con `Rc` y hace de oráculo.
//!
//! El GC se dispara solo en **puntos seguros**: al inicio del bucle de
//! instrucciones, cuando todos los valores vivos están en la pila o los marcos (no
//! hay temporales a medio ensamblar en variables de Rust). Así marcar desde la pila
//! y los marcos es correcto sin más cuidado.

use std::cell::RefCell;
use std::rc::Rc;

use crate::bytecode::{Chunk, CompiledEnum, CompiledFn, CompiledProgram, OpCode, UpvalueSource};
use std::collections::{HashMap, VecDeque};

use crate::gc::{Handle, Heap, HeapValue, Obj, TaskState, VmChannel, VmClosure, VmEnum, VmStruct, VmTask};
use crate::interpreter::{EnumInstance, MapKey, RuntimeError, StructInstance, Value};

/// Límite de marcos para detectar recursión infinita en vez de colgarse. Es el
/// **mismo** que el del intérprete (`interpreter::MAX_CALL_DEPTH`, M13.3a) para que
/// ambos motores coincidan en la frontera: un programa que recurre justo al límite
/// da el mismo veredicto en los dos.
const MAX_FRAMES: usize = crate::interpreter::MAX_CALL_DEPTH;

/// Ejecuta un programa compilado (empezando por `main`) y devuelve su resultado.
pub fn run_program(program: &CompiledProgram) -> Result<Value, RuntimeError> {
    let mut vm = Vm::new(program);
    let result = vm.run()?;
    Ok(to_value(&vm.heap, &program.enums, &result))
}

/// Ejecuta un `Chunk` suelto (una expresión compilada). Lo envuelve como una
/// función sin parámetros ni locales. Se usa en los tests de expresiones.
pub fn run(chunk: &Chunk) -> Result<Value, RuntimeError> {
    let program = CompiledProgram {
        functions: vec![CompiledFn {
            name: "<expr>".to_string(),
            arity: 0,
            num_locals: 0,
            captured: Vec::new(),
            upvalues: Vec::new(),
            chunk: chunk.clone(),
        }],
        structs: Vec::new(),
        enums: Vec::new(),
        main: 0,
    };
    run_program(&program)
}

/// Un slot local. Normalmente guarda el valor directamente (`Plain`); si la variable
/// es capturada por una closure, vive **boxeada** en una celda del heap (`Boxed`),
/// referenciada por handle, para que la closure y el dueño la compartan (M4.2/M4.3).
enum Local {
    Plain(HeapValue),
    Boxed(Handle),
}

struct CallFrame {
    function: usize,
    ip: usize,
    locals: Vec<Local>,
    /// Upvalues de la closure en ejecución (handles a celdas); vacío si no lo es.
    upvalues: Vec<Handle>,
}

/// Una **fibra** (green thread, M12.1): el estado suspendido de una tarea — su pila de marcos y su pila de
/// operandos. La fibra en ejecución vive en los campos `frames`/`stack` de la VM; las demás esperan aquí.
struct Fiber {
    frames: Vec<CallFrame>,
    stack: Vec<HeapValue>,
    is_main: bool,
    /// M12.3: la `Task` que esta fibra debe rellenar al terminar (`None` para `main`).
    task: Option<Handle>,
    /// M12.3: pila de scopes activos en esta fibra (structured concurrency); las tareas que lance
    /// mientras un scope esté activo quedan adscritas al más interno.
    scopes: Vec<ScopeFrame>,
}

/// Un scope activo (M12.3): la lista de tareas lanzadas mientras estuvo en la cima de la pila de la
/// fibra. Al cerrarse (`ScopeEnd`), el scope une a todas (las espera) y propaga el primer fallo.
struct ScopeFrame {
    children: Vec<Handle>,
}

/// Qué espera una fibra **bloqueada** (el handle por el que espera va en `Parked.on`):
/// - `Recv`: bloqueada en `recv` (canal vacío y abierto) → despierta cuando alguien envía o lo cierra.
/// - `Send(v)`: bloqueada en `send` (canal acotado y lleno) → despierta cuando un `recv` libera un hueco;
///   sostiene el valor `v` que aún no ha podido entregar (es una raíz del GC).
/// - `Join`: bloqueada en `join`/`ScopeEnd` esperando a una **tarea** (M12.3); al completarse la tarea se
///   la despierta y re-ejecuta el opcode (que rebobinó su `ip`).
/// - `Select`: bloqueada en `select` esperando a que CUALQUIERA de un conjunto de canales esté listo
///   (M12.4); `Parked.on` es el handle del **arreglo** de canales. Al despertar re-ejecuta el `select`.
enum Waiting {
    Recv,
    Send(HeapValue),
    Join,
    Select,
}

/// Una fibra **bloqueada**, con el handle por el que espera (`on`: un canal para Recv/Send, una tarea para
/// Join) y qué espera.
struct Parked {
    on: Handle,
    fiber: Fiber,
    waiting: Waiting,
}

struct Vm<'a> {
    program: &'a CompiledProgram,
    frames: Vec<CallFrame>,
    stack: Vec<HeapValue>,
    heap: Heap,
    // --- Scheduler cooperativo M:1 (M12.1) ---
    /// Fibras listas para ejecutar, en orden FIFO (scheduler determinista).
    ready: VecDeque<Fiber>,
    /// Fibras bloqueadas en `recv`/`send`/`join`, con el handle (canal o tarea) que esperan.
    parked: Vec<Parked>,
    /// M15.5: fibras aparcadas esperando **E/S de red** (`accept`/`read` que dieron `WouldBlock`). No
    /// llevan handle de GC (un socket es un `int` del registro del host, no un objeto del heap). El
    /// scheduler las re-encola cuando no hay nadie listo (busy-poll cooperativo).
    io_parked: Vec<Fiber>,
    /// ¿La fibra en ejecución es la principal (`main`)? Su retorno termina el programa (semántica Go).
    current_is_main: bool,
    /// M12.3: la `Task` que la fibra en ejecución debe rellenar al terminar (`None` para `main`).
    current_task: Option<Handle>,
    /// M12.3: pila de scopes activos de la fibra en ejecución (espejo del `Fiber.scopes`).
    scopes: Vec<ScopeFrame>,
    /// Opt.2: pool de `Vec<Local>` reutilizables. Cada llamada necesita un arreglo de locales; en vez de
    /// asignar/liberar uno por llamada (millones en recursión), reciclamos los de los marcos que retornan.
    /// NO es raíz del GC (sus contenidos son basura entre reciclar y reusar; `new_locals` los reconstruye).
    locals_pool: Vec<Vec<Local>>,
}

impl<'a> Vm<'a> {
    fn new(program: &'a CompiledProgram) -> Self {
        Vm {
            program,
            frames: Vec::new(),
            stack: Vec::new(),
            heap: Heap::new(),
            ready: VecDeque::new(),
            parked: Vec::new(),
            io_parked: Vec::new(),
            current_is_main: true,
            current_task: None,
            scopes: Vec::new(),
            locals_pool: Vec::new(),
        }
    }

    fn run(&mut self) -> Result<HeapValue, RuntimeError> {
        // Marco inicial: main, con su arreglo de locales (sin argumentos).
        let main = self.program.main;
        let locals = self.new_locals(main);
        self.frames.push(CallFrame { function: main, ip: 0, locals, upvalues: Vec::new() });

        // El programa es inmutable y vive tanto como la VM; copiamos su referencia a un local (Opt.1). Así
        // el `match` de cada instrucción la toma **prestada** del programa (no de `self`), y el cuerpo puede
        // mutar `self` sin que el préstamo choque — eliminando el clon de la instrucción por iteración.
        let program = self.program;

        loop {
            // --- Punto seguro del GC ---
            if self.heap.should_collect() {
                self.collect();
            }

            let fi = self.frames.len() - 1;
            let func = self.frames[fi].function;
            let ip = self.frames[fi].ip;

            // Robustez: si se acabó el chunk sin Return (no debería), retorna unit.
            if ip >= program.functions[func].chunk.code.len() {
                if let Some(frame) = self.frames.pop() {
                    self.recycle_locals(frame.locals); // Opt.2
                }
                if self.frames.is_empty() {
                    match self.on_fiber_done(HeapValue::Unit)? {
                        Some(v) => return Ok(v),
                        None => continue, // era una fibra spawn: el scheduler ya cargó la siguiente
                    }
                }
                self.stack.push(HeapValue::Unit);
                continue;
            }

            // La instrucción y su posición se toman PRESTADAS del programa (Opt.1: sin clonar). `instr`
            // vive lo que `program` (toda la VM), así que no estorba a las mutaciones de `self` del cuerpo.
            let instr = &program.functions[func].chunk.code[ip];
            let (line, col) = program.functions[func].chunk.lines[ip];
            self.frames[fi].ip = ip + 1; // avance por defecto; los saltos lo cambian

            // M12.3: ejecutamos la instrucción dentro de un cierre que devuelve `Ok(Some(v))` (fin del
            // programa), `Ok(None)` (seguir) o `Err` (fallo). Así el bucle puede CAPTURAR el error de una
            // fibra hija (propagación structured concurrency) en vez de abortar siempre.
            let outcome: Result<Option<HeapValue>, RuntimeError> = (|| {
            match instr {
                OpCode::Constant(idx) => {
                    let v = const_to_heap(&self.program.functions[func].chunk.constants[*idx]);
                    self.push(v);
                }
                OpCode::True => self.push(HeapValue::Bool(true)),
                OpCode::False => self.push(HeapValue::Bool(false)),
                OpCode::Unit => self.push(HeapValue::Unit),
                OpCode::Pop => {
                    self.pop();
                }

                OpCode::Negate => {
                    let v = self.pop();
                    self.push(match v {
                        HeapValue::Int(n) => HeapValue::Int(-n),
                        HeapValue::Float(x) => HeapValue::Float(-x),
                        _ => unreachable!("el checker garantiza un número"),
                    });
                }
                OpCode::Not => {
                    let v = self.pop();
                    self.push(match v {
                        HeapValue::Bool(b) => HeapValue::Bool(!b),
                        _ => unreachable!("el checker garantiza un bool"),
                    });
                }

                bin @ (OpCode::Add
                | OpCode::Sub
                | OpCode::Mul
                | OpCode::Div
                | OpCode::Rem
                | OpCode::Equal
                | OpCode::NotEqual
                | OpCode::Less
                | OpCode::LessEqual
                | OpCode::Greater
                | OpCode::GreaterEqual) => {
                    let right = self.pop();
                    let left = self.pop();
                    // M11.7b: `+` sobre dos arreglos (objetos del heap) los concatena en uno nuevo.
                    // El checker garantiza que dos `Obj` con `Add` son arreglos (strings son inline).
                    if let (OpCode::Add, HeapValue::Obj(l), HeapValue::Obj(r)) = (bin, &left, &right) {
                        let (l, r) = (*l, *r);
                        let mut elems = self.as_array(l).clone();
                        elems.extend(self.as_array(r).iter().cloned());
                        let h = self.heap.allocate(Obj::Array(elems));
                        self.push(HeapValue::Obj(h));
                    } else {
                        let result = self.apply_binary(bin, left, right, line, col)?;
                        self.push(result);
                    }
                }

                OpCode::Jump(target) => {
                    self.frames[fi].ip = *target;
                }
                OpCode::JumpIfFalse(target) => {
                    if matches!(self.peek(), HeapValue::Bool(false)) {
                        self.frames[fi].ip = *target;
                    }
                }

                OpCode::GetLocal(slot) => {
                    let v = self.get_local(fi, *slot);
                    self.push(v);
                }
                OpCode::SetLocal(slot) => {
                    let v = self.pop();
                    self.set_local(fi, *slot, v);
                }
                OpCode::InitLocal(slot) => {
                    // Declaración: si el slot está boxeado, estrena celda (shadowing
                    // seguro); si no, guarda el valor directamente.
                    let v = self.pop();
                    let boxed = self.program.functions[func].captured.get(*slot).copied().unwrap_or(false);
                    self.frames[fi].locals[*slot] = if boxed {
                        Local::Boxed(self.heap.allocate(Obj::Cell(v)))
                    } else {
                        Local::Plain(v)
                    };
                }
                OpCode::GetUpvalue(i) => {
                    let h = self.frames[fi].upvalues[*i];
                    let v = self.cell_get(h);
                    self.push(v);
                }
                OpCode::SetUpvalue(i) => {
                    let v = self.pop();
                    let h = self.frames[fi].upvalues[*i];
                    self.cell_set(h, v);
                }

                OpCode::Print => {
                    let v = self.pop();
                    println!("{}", format_value(&self.heap, &self.program.enums, &v));
                    self.push(HeapValue::Unit);
                }

                // --- Arreglos (M3) ---
                OpCode::MakeArray(n) => {
                    let mut elems = Vec::with_capacity(*n);
                    for _ in 0..*n {
                        elems.push(self.pop());
                    }
                    elems.reverse(); // se sacaron en orden inverso
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Index => {
                    let i = self.pop_int();
                    match self.pop() {
                        HeapValue::Obj(h) => {
                            let idx = {
                                let arr = self.as_array(h);
                                bounds_check(i, arr.len(), line, col)?
                            };
                            let v = self.as_array(h)[idx].clone();
                            self.push(v);
                        }
                        // M11.4c-2: indexar un string → el carácter en esa posición.
                        HeapValue::Str(s) => {
                            let chars: Vec<char> = s.chars().collect();
                            let idx = bounds_check(i, chars.len(), line, col)?;
                            self.push(HeapValue::Char(chars[idx]));
                        }
                        // M16.1a: indexar bytes → el octeto como int.
                        HeapValue::Bytes(b) => {
                            let idx = bounds_check(i, b.len(), line, col)?;
                            self.push(HeapValue::Int(b[idx] as i64));
                        }
                        _ => unreachable!("el checker garantiza un arreglo, string o bytes"),
                    }
                }
                OpCode::SetIndex => {
                    let v = self.pop();
                    let i = self.pop_int();
                    let h = self.pop_obj();
                    let idx = bounds_check(i, self.as_array(h).len(), line, col)?;
                    self.as_array_mut(h)[idx] = v;
                }
                OpCode::Len => {
                    // M11.1a: len de arreglo o string; M13.1: len de Map (nº de entradas).
                    let len = match self.pop() {
                        HeapValue::Str(s) => s.chars().count() as i64,
                        // M16.1a: len de bytes = nº de octetos.
                        HeapValue::Bytes(b) => b.len() as i64,
                        HeapValue::Obj(h) => match self.heap.get(h) {
                            Obj::Array(v) => v.len() as i64,
                            Obj::Map(m) => m.len() as i64,
                            _ => unreachable!("el checker garantiza un arreglo o Map"),
                        },
                        _ => unreachable!("el checker garantiza un arreglo, string, Map o bytes"),
                    };
                    self.push(HeapValue::Int(len));
                }
                // --- Mapas Map<K,V> (M13.1) ---
                OpCode::MapNew => {
                    let h = self.heap.allocate(Obj::Map(HashMap::new()));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::MapInsert => {
                    let v = self.pop();
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    match self.heap.get_mut(h) {
                        Obj::Map(m) => { m.insert(k, v); }
                        _ => unreachable!("el checker garantiza un Map"),
                    }
                    self.push(HeapValue::Unit);
                }
                OpCode::MapContainsKey => {
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let presente = match self.heap.get(h) {
                        Obj::Map(m) => m.contains_key(&k),
                        _ => unreachable!("el checker garantiza un Map"),
                    };
                    self.push(HeapValue::Bool(presente));
                }
                OpCode::MapGet => {
                    // Primitivo: [] o [v]; el prelude lo envuelve en Option<V>.
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let elems = match self.heap.get(h) {
                        Obj::Map(m) => match m.get(&k) {
                            Some(v) => vec![v.clone()],
                            None => vec![],
                        },
                        _ => unreachable!("el checker garantiza un Map"),
                    };
                    let arr = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::MapRemove => {
                    // M13.1b: quita la clave; [] o [v]. El prelude → Option<V>.
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let elems = match self.heap.get_mut(h) {
                        Obj::Map(m) => match m.remove(&k) {
                            Some(v) => vec![v],
                            None => vec![],
                        },
                        _ => unreachable!("el checker garantiza un Map"),
                    };
                    let arr = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::MapKeys => {
                    // M13.1b: claves ordenadas (determinista).
                    let h = self.pop_obj();
                    let mut ks: Vec<MapKey> = match self.heap.get(h) {
                        Obj::Map(m) => m.keys().cloned().collect(),
                        _ => unreachable!("el checker garantiza un Map"),
                    };
                    ks.sort();
                    let elems: Vec<HeapValue> = ks.iter().map(key_to_heap).collect();
                    let arr = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::MapValues => {
                    // M13.1b: valores en orden de clave ordenada (casa con keys).
                    let h = self.pop_obj();
                    let elems: Vec<HeapValue> = match self.heap.get(h) {
                        Obj::Map(m) => {
                            let mut pares: Vec<(&MapKey, &HeapValue)> = m.iter().collect();
                            pares.sort_by(|a, b| a.0.cmp(b.0));
                            pares.iter().map(|(_, v)| (*v).clone()).collect()
                        }
                        _ => unreachable!("el checker garantiza un Map"),
                    };
                    let arr = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::Push => {
                    let v = self.pop();
                    let h = self.pop_obj();
                    self.as_array_mut(h).push(v);
                    self.push(HeapValue::Unit);
                }

                // --- Concurrencia: CSP sobre la VM (M12.1) ---
                OpCode::Spawn => {
                    // Saca el valor-función; crea una fibra nueva que lo ejecuta (0 args), le asigna una
                    // Task<T> (M12.3) y la encola. Si hay un scope activo, adscribe la tarea a él.
                    let (fn_idx, upvalues) = match self.pop() {
                        HeapValue::Function(i) => (i, Vec::new()),
                        HeapValue::Obj(h) => match self.heap.get(h) {
                            Obj::Closure(c) => (c.index, c.upvalues.clone()),
                            _ => unreachable!("el checker garantiza una función"),
                        },
                        _ => unreachable!("el checker garantiza una función"),
                    };
                    let task = self.heap.allocate(Obj::Task(VmTask { state: TaskState::Pending }));
                    if let Some(scope) = self.scopes.last_mut() {
                        scope.children.push(task);
                    }
                    let locals = self.new_locals(fn_idx);
                    let frame = CallFrame { function: fn_idx, ip: 0, locals, upvalues };
                    self.ready.push_back(Fiber {
                        frames: vec![frame], stack: Vec::new(), is_main: false,
                        task: Some(task), scopes: Vec::new(),
                    });
                    self.push(HeapValue::Obj(task)); // el Task<T> es el resultado de spawn
                }
                OpCode::ChannelNew => {
                    // channel() sin argumentos → canal NO acotado (cap = None).
                    let h = self.heap.allocate(Obj::Channel(VmChannel {
                        queue: VecDeque::new(), closed: false, cap: None,
                    }));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ChannelNewBounded => {
                    // channel(n) → canal acotado a la capacidad n ≥ 0 (n = 0 rendezvous), M12.2.
                    let n = match self.pop() {
                        HeapValue::Int(n) => n,
                        _ => unreachable!("el checker garantiza un int"),
                    };
                    if n < 0 {
                        return Err(runtime_error(line, col, "la capacidad de un canal no puede ser negativa"));
                    }
                    let h = self.heap.allocate(Obj::Channel(VmChannel {
                        queue: VecDeque::new(), closed: false, cap: Some(n as usize),
                    }));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ChanSend => {
                    let v = self.pop();
                    let h = self.pop_obj();
                    let (closed, len, cap) = match self.heap.get(h) {
                        Obj::Channel(c) => (c.closed, c.queue.len(), c.cap),
                        _ => unreachable!("el checker garantiza un Channel"),
                    };
                    if closed {
                        return Err(runtime_error(line, col, "send sobre un canal cerrado"));
                    }
                    // (1) ¿Hay un receptor bloqueado en este canal? Entrégaselo directo (rendezvous) y
                    // despiértalo (el primero, FIFO → determinista).
                    if let Some(pos) = self.parked.iter().position(
                        |p| p.on == h && matches!(p.waiting, Waiting::Recv))
                    {
                        let parked = self.parked.remove(pos);
                        self.wake_recv(parked.fiber, vec![v]);
                        self.push(HeapValue::Unit);
                    } else if cap.is_none() || len < cap.unwrap() {
                        // (2) Hay hueco (no acotado, o len < cap) → encola y sigue.
                        match self.heap.get_mut(h) {
                            Obj::Channel(c) => c.queue.push_back(v),
                            _ => unreachable!("el checker garantiza un Channel"),
                        }
                        self.wake_select_waiters(h); // M12.4: el canal ya tiene valor → listo para un select
                        self.push(HeapValue::Unit);
                    } else {
                        // (3) Cola llena (acotado) → BLOQUEAR al emisor (backpressure, M12.2). Guarda la
                        // fibra con el valor pendiente; al despertarla, `wake_sender` le deja unit (el
                        // resultado de `send`) en la pila y continúa tras el ChanSend.
                        let fiber = self.take_current_fiber();
                        self.parked.push(Parked { on: h, fiber, waiting: Waiting::Send(v) });
                        // M12.4: un emisor bloqueado vuelve al canal "listo" para un select (un recv lo
                        // tomaría); despierta a los selectores que lo esperan.
                        self.wake_select_waiters(h);
                        self.schedule_next(line, col)?;
                    }
                }
                OpCode::ChanRecv => {
                    let h = self.pop_obj();
                    // (1) ¿Valor en la cola? Sácalo; al liberar un hueco, si hay un emisor bloqueado en este
                    // canal, su valor entra a la cola (ya hay sitio) y se le despierta.
                    let from_queue = match self.heap.get_mut(h) {
                        Obj::Channel(c) => c.queue.pop_front(),
                        _ => unreachable!("el checker garantiza un Channel"),
                    };
                    if let Some(v) = from_queue {
                        self.wake_blocked_sender(h);
                        let arr = self.heap.allocate(Obj::Array(vec![v]));
                        self.push(HeapValue::Obj(arr));
                        return Ok(None);
                    }
                    // (2) Cola vacía: ¿hay un emisor bloqueado? (canal lleno con cap > 0, o rendezvous
                    // cap = 0). Toma su valor directo y despiértalo.
                    if let Some(pos) = self.parked.iter().position(
                        |p| p.on == h && matches!(p.waiting, Waiting::Send(_)))
                    {
                        let parked = self.parked.remove(pos);
                        let sv = match parked.waiting {
                            Waiting::Send(sv) => sv,
                            _ => unreachable!(),
                        };
                        self.wake_sender(parked.fiber);
                        let arr = self.heap.allocate(Obj::Array(vec![sv]));
                        self.push(HeapValue::Obj(arr));
                        return Ok(None);
                    }
                    // (3) Cola vacía y sin emisores: cerrado → None ([]); abierto → bloquear (Recv).
                    let closed = match self.heap.get(h) {
                        Obj::Channel(c) => c.closed,
                        _ => unreachable!("el checker garantiza un Channel"),
                    };
                    if closed {
                        let arr = self.heap.allocate(Obj::Array(Vec::new()));
                        self.push(HeapValue::Obj(arr));
                    } else {
                        // Bloquear: guardar la fibra actual (ip ya apunta tras el ChanRecv → al
                        // despertarla, el `wake_recv` le deja el `[T]` en la pila y continúa) y conmutar.
                        let fiber = self.take_current_fiber();
                        self.parked.push(Parked { on: h, fiber, waiting: Waiting::Recv });
                        self.schedule_next(line, col)?;
                    }
                }
                OpCode::TaskJoin => {
                    // Une una tarea (M12.3): si terminó, su valor; si falló, re-lanza; si pendiente, bloquea.
                    let t = self.pop_obj();
                    let outcome = match self.heap.get(t) {
                        Obj::Task(task) => match &task.state {
                            TaskState::Done(v) => Some(Ok(v.clone())),
                            TaskState::Failed(msg) => Some(Err(msg.clone())),
                            TaskState::Pending => None,
                        },
                        _ => unreachable!("el checker garantiza una Task"),
                    };
                    match outcome {
                        Some(Ok(v)) => self.push(v),
                        Some(Err(msg)) => return Err(runtime_error(line, col, &msg)),
                        None => {
                            // Bloquear: re-empuja el handle (lo sacamos arriba) y rebobina el ip al
                            // TaskJoin, para que al despertar (con la tarea ya Done/Failed) lo re-ejecute.
                            self.push(HeapValue::Obj(t));
                            self.frames.last_mut().unwrap().ip -= 1;
                            let fiber = self.take_current_fiber();
                            self.parked.push(Parked { on: t, fiber, waiting: Waiting::Join });
                            self.schedule_next(line, col)?;
                        }
                    }
                }
                OpCode::ScopeBegin => {
                    // Abre un scope (M12.3): las tareas spawneadas mientras esté activo se le adscriben.
                    self.scopes.push(ScopeFrame { children: Vec::new() });
                }
                OpCode::ScopeEnd => {
                    // Cierra el scope: el valor del cuerpo (R) ya está en la pila.
                    let children: Vec<Handle> =
                        self.scopes.last().expect("ScopeEnd sin ScopeBegin").children.clone();
                    // (1) ¿Alguna hija FALLÓ? Cancela a las hermanas que sigan pendientes y propaga el fallo
                    // ORIGINAL de inmediato, sin esperar a las demás (M12.5: cancelación de hermanas).
                    let failure = children.iter().find_map(|&c| match self.heap.get(c) {
                        Obj::Task(VmTask { state: TaskState::Failed(msg) }) => Some(msg.clone()),
                        _ => None,
                    });
                    if let Some(msg) = failure {
                        for &c in &children {
                            self.cancel_task(c); // ignora las no-pendientes (la que falló, las Done)
                        }
                        self.scopes.pop();
                        return Err(runtime_error(line, col, &msg));
                    }
                    // (2) ¿Alguna pendiente? Rebobina a ScopeEnd y bloquéate (al despertar re-escanea).
                    let pending = children.iter().copied().find(|&c| matches!(
                        self.heap.get(c), Obj::Task(t) if matches!(t.state, TaskState::Pending)));
                    if let Some(c) = pending {
                        self.frames.last_mut().unwrap().ip -= 1;
                        let fiber = self.take_current_fiber();
                        self.parked.push(Parked { on: c, fiber, waiting: Waiting::Join });
                        self.schedule_next(line, col)?;
                    } else {
                        // (3) Todas terminaron con éxito: desapila el scope.
                        self.scopes.pop();
                    }
                }
                OpCode::Select => {
                    // Espera a que algún canal de la lista esté listo para recibir; devuelve su índice
                    // (el menor, determinista). Si ninguno lo está, bloquea esperando al conjunto (M12.4).
                    let arr = self.pop_obj();
                    let chans: Vec<Handle> = match self.heap.get(arr) {
                        Obj::Array(elems) => elems.iter().filter_map(|v| v.handle()).collect(),
                        _ => unreachable!("el checker garantiza un arreglo de canales"),
                    };
                    let mut ready_idx = None;
                    for (i, &c) in chans.iter().enumerate() {
                        let buffered_or_closed = match self.heap.get(c) {
                            Obj::Channel(ch) => !ch.queue.is_empty() || ch.closed,
                            _ => unreachable!("el checker garantiza un Channel"),
                        };
                        let has_sender = self.parked.iter()
                            .any(|p| p.on == c && matches!(p.waiting, Waiting::Send(_)));
                        if buffered_or_closed || has_sender {
                            ready_idx = Some(i);
                            break;
                        }
                    }
                    match ready_idx {
                        Some(i) => self.push(HeapValue::Int(i as i64)),
                        None => {
                            // Ninguno listo: re-empuja el arreglo (lo sacamos arriba), rebobina el ip al
                            // Select y aparca esperando al conjunto (el handle del arreglo va en `on`).
                            self.push(HeapValue::Obj(arr));
                            self.frames.last_mut().unwrap().ip -= 1;
                            let fiber = self.take_current_fiber();
                            self.parked.push(Parked { on: arr, fiber, waiting: Waiting::Select });
                            self.schedule_next(line, col)?;
                        }
                    }
                }

                // --- Stdlib de string (M11.1) ---
                OpCode::ToString => {
                    // Representación textual (la misma que `print`): coincide con el `Display`
                    // que usa el intérprete en `to_string`.
                    let v = self.pop();
                    let s = format_value(&self.heap, &self.program.enums, &v);
                    self.push(HeapValue::Str(s));
                }
                OpCode::Trim => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Str(s.trim().to_string())),
                    _ => unreachable!("el checker garantiza un string"),
                },
                OpCode::Split => {
                    // El separador está encima del string (orden de los argumentos).
                    let sep = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(sep)) = (s, sep) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    let parts: Vec<HeapValue> =
                        s.split(sep.as_str()).map(|p| HeapValue::Str(p.to_string())).collect();
                    // El arreglo es un objeto del heap; los Str son inline, sin handles que rootear.
                    let h = self.heap.allocate(Obj::Array(parts));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Chars => {
                    let s = match self.pop() {
                        HeapValue::Str(s) => s,
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let cs: Vec<HeapValue> = s.chars().map(HeapValue::Char).collect();
                    // El arreglo es un objeto del heap; los Char son inline, sin handles que rootear.
                    let h = self.heap.allocate(Obj::Array(cs));
                    self.push(HeapValue::Obj(h));
                }
                // M16.1b: los octetos UTF-8 del string → bytes (inline, no objeto del heap).
                OpCode::ToBytes => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Bytes(s.into_bytes())),
                    _ => unreachable!("el checker garantiza un string"),
                },
                // M16.1b: decodifica bytes como UTF-8 → arreglo etiquetado; el prelude → Result.
                OpCode::FromUtf8 => {
                    let b = match self.pop() {
                        HeapValue::Bytes(b) => b,
                        _ => unreachable!("el checker garantiza bytes"),
                    };
                    let elems = match String::from_utf8(b) {
                        Ok(s) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(s)],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Contains => {
                    // El valor buscado está encima del contenedor (orden de los argumentos).
                    let x = self.pop();
                    let cont = self.pop();
                    let res = match (&cont, &x) {
                        (HeapValue::Str(s), HeapValue::Str(sub)) => s.contains(sub.as_str()),
                        // M11.7b: arreglo → pertenencia por igualdad estructural.
                        (HeapValue::Obj(h), _) => {
                            self.as_array(*h).iter().any(|e| values_equal(&self.heap, e, &x))
                        }
                        _ => unreachable!("el checker garantiza string+string o arreglo+elemento"),
                    };
                    self.push(HeapValue::Bool(res));
                }
                OpCode::Replace => {
                    // Orden de los argumentos en la pila: s, de, a → se sacan en orden inverso.
                    let a = self.pop();
                    let de = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(de), HeapValue::Str(a)) = (s, de, a) else {
                        unreachable!("el checker garantiza tres strings");
                    };
                    self.push(HeapValue::Str(s.replace(de.as_str(), a.as_str())));
                }

                // --- Más string (M11.7a) ---
                OpCode::StartsWith => {
                    let p = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(p)) = (s, p) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    self.push(HeapValue::Bool(s.starts_with(p.as_str())));
                }
                OpCode::EndsWith => {
                    let p = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(p)) = (s, p) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    self.push(HeapValue::Bool(s.ends_with(p.as_str())));
                }
                OpCode::ToUpper => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Str(s.to_uppercase())),
                    _ => unreachable!("el checker garantiza un string"),
                },
                OpCode::ToLower => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Str(s.to_lowercase())),
                    _ => unreachable!("el checker garantiza un string"),
                },
                OpCode::Substring => {
                    // Orden en la pila: s, i, j → se sacan en inverso.
                    let j = self.pop();
                    let i = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Int(i), HeapValue::Int(j)) = (s, i, j) else {
                        unreachable!("el checker garantiza string, int, int");
                    };
                    self.push(HeapValue::Str(crate::builtins::substring_chars(&s, i, j)));
                }
                OpCode::Repeat => {
                    let n = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Int(n)) = (s, n) else {
                        unreachable!("el checker garantiza string, int");
                    };
                    self.push(HeapValue::Str(crate::builtins::repeat_str(&s, n)));
                }
                OpCode::IndexOf => {
                    // Primitivo: [] o [i] (índice de carácter). El prelude → Option<int>.
                    let sub = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(sub)) = (s, sub) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    let elems = match crate::builtins::char_index_of(&s, &sub) {
                        Some(i) => vec![HeapValue::Int(i as i64)],
                        None => vec![],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Join => {
                    // Orden en la pila: arr, sep → se saca sep primero.
                    let sep = self.pop();
                    let arr = self.pop();
                    let (HeapValue::Obj(h), HeapValue::Str(sep)) = (arr, sep) else {
                        unreachable!("el checker garantiza [string], string");
                    };
                    let parts: Vec<String> = self.as_array(h).iter().map(|v| match v {
                        HeapValue::Str(s) => s.clone(),
                        _ => unreachable!("el checker garantiza [string]"),
                    }).collect();
                    self.push(HeapValue::Str(parts.join(sep.as_str())));
                }

                // --- Más arreglos (M11.7b) ---
                OpCode::Reverse => {
                    let h = self.pop_obj();
                    let mut elems = self.as_array(h).clone();
                    elems.reverse();
                    let nh = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(nh));
                }
                OpCode::ArrayPop => {
                    // Muta el arreglo quitando el último; devuelve [] o [x]. Prelude → Option<T>.
                    let h = self.pop_obj();
                    let popped = self.as_array_mut(h).pop();
                    let elems = popped.map(|v| vec![v]).unwrap_or_default();
                    let nh = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(nh));
                }
                OpCode::Position => {
                    let x = self.pop();
                    let h = self.pop_obj();
                    let idx = self.as_array(h).iter().position(|e| values_equal(&self.heap, e, &x));
                    let elems = idx.map(|i| vec![HeapValue::Int(i as i64)]).unwrap_or_default();
                    let nh = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(nh));
                }

                // --- I/O y API de runtime (M11.2) ---
                // M13.2a: aborta con el mensaje en la posición de la llamada (igual que el
                // intérprete, que lo intercepta en `eval_call`). El mensaje es el string en la cima.
                OpCode::Panic => {
                    let msg = match self.pop() {
                        HeapValue::Str(s) => s,
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    return Err(runtime_error(line, col, &msg));
                }
                OpCode::EPrint => {
                    let v = self.pop();
                    eprintln!("{}", format_value(&self.heap, &self.program.enums, &v));
                    self.push(HeapValue::Unit);
                }
                OpCode::ParseInt => {
                    // Primitivo: [] o [n]; el prelude lo envuelve en Option<int>.
                    let elems = match self.pop() {
                        HeapValue::Str(s) => match s.trim().parse::<i64>() {
                            Ok(n) => vec![HeapValue::Int(n)],
                            Err(_) => vec![],
                        },
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ParseFloat => {
                    // M14: [] o [f]; el prelude lo envuelve en Option<float>.
                    let elems = match self.pop() {
                        HeapValue::Str(s) => match s.trim().parse::<f64>() {
                            Ok(f) => vec![HeapValue::Float(f)],
                            Err(_) => vec![],
                        },
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ReadLine => {
                    // Primitivo: [] en EOF, [linea] si no (sin el '\n'). El prelude → Option<string>.
                    let mut line = String::new();
                    let elems = match std::io::stdin().read_line(&mut line) {
                        Ok(0) | Err(_) => vec![],
                        Ok(_) => vec![HeapValue::Str(line.trim_end_matches(['\n', '\r']).to_string())],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Env => {
                    // Primitivo: [] si no existe, [valor] si sí. El prelude → Option<string>.
                    let elems = match self.pop() {
                        HeapValue::Str(name) => match std::env::var(name.as_str()) {
                            Ok(v) => vec![HeapValue::Str(v)],
                            Err(_) => vec![],
                        },
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Args => {
                    // Argumentos del programa (del almacén de proceso); arreglo de strings.
                    let items: Vec<HeapValue> = crate::interpreter::program_args()
                        .iter()
                        .map(|a| HeapValue::Str(a.clone()))
                        .collect();
                    let h = self.heap.allocate(Obj::Array(items));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ReadFile => {
                    // Arreglo etiquetado ["ok", contenido] o ["err", msg]. El prelude → Result.
                    let elems = match self.pop() {
                        HeapValue::Str(path) => match std::fs::read_to_string(path.as_str()) {
                            Ok(c) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(c)],
                            Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                        },
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::WriteFile => {
                    // El contenido está encima de la ruta (orden de los argumentos).
                    let contents = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Str(contents)) = (path, contents) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    let elems = match std::fs::write(path.as_str(), contents.as_str()) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Exists => match self.pop() {
                    HeapValue::Str(path) => self.push(HeapValue::Bool(std::path::Path::new(path.as_str()).exists())),
                    _ => unreachable!("el checker garantiza un string"),
                },
                OpCode::AppendFile => {
                    // El contenido está encima de la ruta (orden de los argumentos).
                    let contents = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Str(contents)) = (path, contents) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    let elems = match crate::builtins::append_to_file(path.as_str(), contents.as_str()) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::RemoveFile => {
                    let path = match self.pop() {
                        HeapValue::Str(p) => p,
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let elems = match std::fs::remove_file(&path) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ListDir => {
                    let path = match self.pop() {
                        HeapValue::Str(p) => p,
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let elems = match crate::builtins::list_dir(&path) {
                        Ok(nombres) => {
                            let mut v = vec![HeapValue::Str("ok".to_string())];
                            v.extend(nombres.into_iter().map(HeapValue::Str));
                            v
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }

                // --- I/O con buffering: handles de archivo (M11.8) ---
                OpCode::Open => {
                    let mode = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Str(mode)) = (path, mode) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    let elems = match crate::builtins::open_file(&path, &mode) {
                        Ok(h) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ReadLineHandle => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("el checker garantiza un int"),
                    };
                    let elems = crate::builtins::read_line_handle(handle).map(|l| vec![HeapValue::Str(l)]).unwrap_or_default();
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::WriteHandle => {
                    let s = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(s)) = (handle, s) else {
                        unreachable!("el checker garantiza int, string");
                    };
                    let elems = match crate::builtins::write_handle(handle, &s) {
                        Ok(_) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // --- Cliente TCP (M15.2): arreglo etiquetado en el heap; el prelude → Result. ---
                OpCode::TcpConnect => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("el checker garantiza string, int");
                    };
                    let elems = match crate::builtins::tcp_connect(&host, port) {
                        Ok(h) => {
                            // M15.5: la VM usa sockets NO bloqueantes → socket_read cede al scheduler.
                            let _ = crate::builtins::set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::SocketRead => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("el checker garantiza un int"),
                    };
                    // M15.5: lectura no bloqueante. WouldBlock (Ok(None)) → aparcar la fibra y reintentar.
                    match crate::builtins::socket_read_nb(handle) {
                        Ok(Some(s)) => {
                            let elems = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(s)];
                            let h = self.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)];
                            let h = self.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(None) => {
                            // Re-empuja el handle y rebobina al SocketRead: al despertar lo re-ejecuta.
                            self.push(HeapValue::Int(handle));
                            self.frames.last_mut().unwrap().ip -= 1;
                            let fiber = self.take_current_fiber();
                            self.io_parked.push(fiber);
                            self.schedule_next(line, col)?;
                        }
                    }
                }
                OpCode::SocketWrite => {
                    let s = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(s)) = (handle, s) else {
                        unreachable!("el checker garantiza int, string");
                    };
                    let elems = match crate::builtins::socket_write(handle, &s) {
                        Ok(_) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(String::new())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // --- Servidor TCP (M15.3) ---
                OpCode::TcpListen => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("el checker garantiza string, int");
                    };
                    let elems = match crate::builtins::tcp_listen(&host, port) {
                        Ok(h) => {
                            // M15.5: escucha NO bloqueante → tcp_accept cede al scheduler.
                            let _ = crate::builtins::set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::TcpAccept => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("el checker garantiza un int"),
                    };
                    // M15.5: accept no bloqueante. WouldBlock (Ok(None)) → aparcar y reintentar.
                    match crate::builtins::tcp_accept_nb(handle) {
                        Ok(Some(c)) => {
                            let elems = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(c.to_string())];
                            let h = self.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)];
                            let h = self.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(None) => {
                            self.push(HeapValue::Int(handle));
                            self.frames.last_mut().unwrap().ip -= 1;
                            let fiber = self.take_current_fiber();
                            self.io_parked.push(fiber);
                            self.schedule_next(line, col)?;
                        }
                    }
                }
                OpCode::LocalPort => match self.pop() {
                    HeapValue::Int(h) => self.push(HeapValue::Int(crate::builtins::local_port(h))),
                    _ => unreachable!("el checker garantiza un int"),
                },
                OpCode::Close => {
                    // Ad-hoc polimórfico: un handle de archivo (int, M11.8) o un canal (M12.1).
                    match self.pop() {
                        HeapValue::Int(h) => {
                            crate::builtins::close_handle(h);
                            self.push(HeapValue::Int(0));
                        }
                        // Cerrar un canal: marcarlo cerrado y despertar a TODOS sus receptores bloqueados
                        // (recibirán [] → None). Devuelve unit. M12.2: cerrar un canal con un EMISOR
                        // bloqueado es un error de programa (alguien todavía esperaba enviar) → error de
                        // ejecución en el sitio del `close` (determinista, a diferencia de "panic en otra
                        // fibra").
                        HeapValue::Obj(ch) => {
                            if self.parked.iter().any(
                                |p| p.on == ch && matches!(p.waiting, Waiting::Send(_)))
                            {
                                return Err(runtime_error(line, col,
                                    "close sobre un canal con un emisor bloqueado"));
                            }
                            match self.heap.get_mut(ch) {
                                Obj::Channel(c) => c.closed = true,
                                _ => unreachable!("el checker garantiza un handle o un Channel"),
                            }
                            let mut i = 0;
                            while i < self.parked.len() {
                                if self.parked[i].on == ch && matches!(self.parked[i].waiting, Waiting::Recv) {
                                    let parked = self.parked.remove(i);
                                    self.wake_recv(parked.fiber, Vec::new());
                                } else {
                                    i += 1;
                                }
                            }
                            self.wake_select_waiters(ch); // M12.4: un canal cerrado está "listo" para un select
                            self.push(HeapValue::Unit);
                        }
                        _ => unreachable!("el checker garantiza un handle (int) o un Channel"),
                    }
                }

                // --- Matemáticas (M15.1a) ---
                // Una sola rama para las 10 funciones float -> float; delega en el helper compartido
                // con el intérprete (mismo cálculo → oráculo cuadra, incl. NaN/inf).
                OpCode::MathF(f) => match self.pop() {
                    HeapValue::Float(x) => self.push(HeapValue::Float(crate::builtins::apply_mathf(*f, x))),
                    _ => unreachable!("el checker garantiza un float"),
                },
                OpCode::Pow => {
                    let exp = self.pop();
                    let base = self.pop();
                    let (HeapValue::Float(base), HeapValue::Float(exp)) = (base, exp) else {
                        unreachable!("el checker garantiza dos floats");
                    };
                    self.push(HeapValue::Float(base.powf(exp)));
                }
                OpCode::Abs => match self.pop() {
                    HeapValue::Int(x) => self.push(HeapValue::Int(x.abs())),
                    HeapValue::Float(x) => self.push(HeapValue::Float(x.abs())),
                    _ => unreachable!("el checker garantiza int o float"),
                },
                OpCode::Min => {
                    let b = self.pop();
                    let a = self.pop();
                    match (a, b) {
                        (HeapValue::Int(a), HeapValue::Int(b)) => self.push(HeapValue::Int(a.min(b))),
                        (HeapValue::Float(a), HeapValue::Float(b)) => self.push(HeapValue::Float(a.min(b))),
                        _ => unreachable!("el checker garantiza dos números del mismo tipo"),
                    }
                }
                OpCode::Max => {
                    let b = self.pop();
                    let a = self.pop();
                    match (a, b) {
                        (HeapValue::Int(a), HeapValue::Int(b)) => self.push(HeapValue::Int(a.max(b))),
                        (HeapValue::Float(a), HeapValue::Float(b)) => self.push(HeapValue::Float(a.max(b))),
                        _ => unreachable!("el checker garantiza dos números del mismo tipo"),
                    }
                }
                OpCode::Pi => self.push(HeapValue::Float(std::f64::consts::PI)),
                OpCode::E => self.push(HeapValue::Float(std::f64::consts::E)),

                // --- Reloj y aleatoriedad (M15.1b): delegan en los helpers compartidos. ---
                OpCode::Now => self.push(HeapValue::Int(crate::builtins::now_millis())),
                OpCode::Monotonic => self.push(HeapValue::Int(crate::builtins::monotonic_millis())),
                OpCode::Sleep => match self.pop() {
                    HeapValue::Int(ms) => {
                        crate::builtins::sleep_millis(ms);
                        self.push(HeapValue::Unit);
                    }
                    _ => unreachable!("el checker garantiza un int"),
                },
                OpCode::Random => self.push(HeapValue::Float(crate::builtins::random_f64())),
                OpCode::RandomInt => match self.pop() {
                    HeapValue::Int(n) => self.push(HeapValue::Int(crate::builtins::random_int(n))),
                    _ => unreachable!("el checker garantiza un int"),
                },

                // --- Structs (M3.2) ---
                OpCode::MakeStruct(idx) => {
                    let sname = self.program.structs[*idx].name.clone();
                    let field_names: Vec<String> = self.program.structs[*idx].fields.clone();
                    let mut values = Vec::with_capacity(field_names.len());
                    for _ in 0..field_names.len() {
                        values.push(self.pop());
                    }
                    values.reverse(); // orden de declaración
                    let fields: Vec<(String, HeapValue)> = field_names.into_iter().zip(values).collect();
                    let h = self.heap.allocate(Obj::Struct(VmStruct { name: sname, fields }));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::MakeEnum(enum_id, tag) => {
                    // La aridad la da la tabla; sacamos ese tanto de payload.
                    let arity = self.program.enums[*enum_id].variants[*tag].arity;
                    let mut payload = Vec::with_capacity(arity);
                    for _ in 0..arity {
                        payload.push(self.pop());
                    }
                    payload.reverse(); // orden de declaración
                    let h = self.heap.allocate(Obj::Enum(VmEnum { enum_id: *enum_id, tag: *tag, payload }));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::EnumTagEq(tag) => {
                    let h = self.pop_obj();
                    let matches = self.as_enum(h).tag == *tag;
                    self.push(HeapValue::Bool(matches));
                }
                OpCode::GetEnumField(i) => {
                    let h = self.pop_obj();
                    let v = self.as_enum(h).payload[*i].clone();
                    self.push(v);
                }
                OpCode::MatchFail => {
                    return Err(runtime_error(line, col, "ningún brazo del match casó (no debería ocurrir)"));
                }
                OpCode::GetField(name) => {
                    let h = self.pop_obj();
                    let v = self.as_struct(h).fields.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
                        .expect("el checker garantiza el campo");
                    self.push(v);
                }
                OpCode::SetField(name) => {
                    let v = self.pop();
                    let h = self.pop_obj();
                    let s = self.as_struct_mut(h);
                    let slot = s.fields.iter_mut().find(|(n, _)| n == name).expect("el checker garantiza el campo");
                    slot.1 = v;
                }

                OpCode::Call(idx, argc) => {
                    if self.frames.len() >= MAX_FRAMES {
                        return Err(runtime_error(line, col, "desbordamiento de pila (recursión demasiado profunda)"));
                    }
                    let mut locals = self.new_locals(*idx);
                    for i in (0..*argc).rev() {
                        let v = self.pop();
                        self.put_arg(&mut locals, i, v);
                    }
                    self.frames.push(CallFrame { function: *idx, ip: 0, locals, upvalues: Vec::new() });
                }
                // M13.3b: llamada en cola — REUTILIZA el marco actual (no crece la pila de marcos).
                // En posición de cola, el valor de esta llamada es el de la función actual, así que
                // el resultado caerá en la misma posición de la pila. No hay límite que comprobar:
                // ese es justo el punto (recursión de cola en O(1) marcos).
                OpCode::TailCall(idx, argc) => {
                    let mut locals = self.new_locals(*idx);
                    for i in (0..*argc).rev() {
                        let v = self.pop();
                        self.put_arg(&mut locals, i, v);
                    }
                    self.frames[fi].function = *idx;
                    self.frames[fi].ip = 0;
                    let old = std::mem::replace(&mut self.frames[fi].locals, locals);
                    self.recycle_locals(old); // Opt.2: la llamada en cola reemplaza las locales → recicla
                    self.frames[fi].upvalues = Vec::new();
                }

                // --- Funciones de primera clase (M4.1) ---
                OpCode::Function(idx) => self.push(HeapValue::Function(*idx)),
                OpCode::CallValue(argc) => {
                    if self.frames.len() >= MAX_FRAMES {
                        return Err(runtime_error(line, col, "desbordamiento de pila (recursión demasiado profunda)"));
                    }
                    let mut args_rev = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        args_rev.push(self.pop());
                    }
                    let (fn_idx, upvalues) = match self.pop() {
                        HeapValue::Function(i) => (i, Vec::new()),
                        HeapValue::Obj(h) => match self.heap.get(h) {
                            Obj::Closure(c) => (c.index, c.upvalues.clone()),
                            _ => unreachable!("el checker garantiza una función"),
                        },
                        _ => unreachable!("el checker garantiza una función"),
                    };
                    let mut locals = self.new_locals(fn_idx);
                    for (j, val) in args_rev.into_iter().enumerate() {
                        self.put_arg(&mut locals, *argc - 1 - j, val);
                    }
                    self.frames.push(CallFrame { function: fn_idx, ip: 0, locals, upvalues });
                }
                // M13.3b: llamada indirecta en cola — reutiliza el marco actual.
                OpCode::TailCallValue(argc) => {
                    let mut args_rev = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        args_rev.push(self.pop());
                    }
                    let (fn_idx, upvalues) = match self.pop() {
                        HeapValue::Function(i) => (i, Vec::new()),
                        HeapValue::Obj(h) => match self.heap.get(h) {
                            Obj::Closure(c) => (c.index, c.upvalues.clone()),
                            _ => unreachable!("el checker garantiza una función"),
                        },
                        _ => unreachable!("el checker garantiza una función"),
                    };
                    let mut locals = self.new_locals(fn_idx);
                    for (j, val) in args_rev.into_iter().enumerate() {
                        self.put_arg(&mut locals, *argc - 1 - j, val);
                    }
                    self.frames[fi].function = fn_idx;
                    self.frames[fi].ip = 0;
                    let old = std::mem::replace(&mut self.frames[fi].locals, locals);
                    self.recycle_locals(old); // Opt.2
                    self.frames[fi].upvalues = upvalues;
                }

                // --- Closures (M4.2) ---
                OpCode::Closure(idx) => {
                    // Armamos el arreglo de upvalues tomando las celdas que indica la
                    // función, del marco actual (un local boxeado, o un upvalue propio
                    // para la captura transitiva).
                    let descs = self.program.functions[*idx].upvalues.clone();
                    let mut upvalues = Vec::with_capacity(descs.len());
                    for d in &descs {
                        let cell = match d.source {
                            UpvalueSource::Local(slot) => match &self.frames[fi].locals[slot] {
                                Local::Boxed(h) => *h,
                                Local::Plain(_) => unreachable!("un local capturado debe estar boxeado"),
                            },
                            UpvalueSource::Upvalue(u) => self.frames[fi].upvalues[u],
                        };
                        upvalues.push(cell);
                    }
                    let h = self.heap.allocate(Obj::Closure(VmClosure { index: *idx, upvalues }));
                    self.push(HeapValue::Obj(h));
                }

                OpCode::Return => {
                    let result = self.pop();
                    if let Some(frame) = self.frames.pop() {
                        self.recycle_locals(frame.locals); // Opt.2: el marco se descarta → recicla sus locales
                    }
                    if self.frames.is_empty() {
                        // La fibra terminó: si es main → fin del programa; si es spawn → siguiente fibra.
                        match self.on_fiber_done(result)? {
                            Some(v) => return Ok(Some(v)),
                            None => return Ok(None),
                        }
                    }
                    self.push(result); // entregamos el valor al llamador
                }
            }
            Ok(None)
            })();

            match outcome {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => {}
                Err(e) => {
                    // Propagación de fallos (M12.3): el error de la fibra HIJA en curso no aborta el
                    // programa; se captura en su `Task` (`Failed`) y se planifica la siguiente. Abortan los
                    // de `main` y los del scheduler (frames vacíos = la fibra ya se aparcó/terminó → el
                    // error es un deadlock, no un fallo de la fibra actual).
                    if self.frames.is_empty() || self.current_is_main {
                        return Err(e);
                    }
                    self.fail_current_fiber(e)?;
                }
            }
        }
    }

    // ----- Scheduler de fibras (M12.1 / M12.3) -----

    /// Empaqueta la fibra en ejecución (vaciando los campos de la VM) para aparcarla o descartarla. M12.3:
    /// además de `frames`/`stack`/`is_main`, lleva su `Task` y su pila de scopes.
    fn take_current_fiber(&mut self) -> Fiber {
        Fiber {
            frames: std::mem::take(&mut self.frames),
            stack: std::mem::take(&mut self.stack),
            is_main: self.current_is_main,
            task: self.current_task.take(),
            scopes: std::mem::take(&mut self.scopes),
        }
    }

    /// Una fibra terminó **con éxito**. Si es `main` → fin del programa (su valor; semántica Go). Si es una
    /// fibra `spawn` → escribe el resultado en su `Task` (M12.3), despierta a los que la unen y planifica la
    /// siguiente. Devuelve `Some(v)` si el programa termina, `None` si ya cargó otra fibra.
    fn on_fiber_done(&mut self, result: HeapValue) -> Result<Option<HeapValue>, RuntimeError> {
        if self.current_is_main {
            return Ok(Some(result));
        }
        if let Some(task) = self.current_task.take() {
            if let Obj::Task(t) = self.heap.get_mut(task) {
                t.state = TaskState::Done(result);
            }
            self.wake_task_waiters(task);
        }
        self.scopes.clear();
        self.schedule_next(0, 0)?;
        Ok(None)
    }

    /// Una fibra hija **falló** (M12.3): guarda el fallo en su `Task` como `Failed`, despierta a los que la
    /// unen (que lo re-lanzarán) y planifica la siguiente. El error solo se pierde si la tarea no la une
    /// nadie ni la posee un `scope`.
    fn fail_current_fiber(&mut self, e: RuntimeError) -> Result<(), RuntimeError> {
        if let Some(task) = self.current_task.take() {
            let msg = e.msg.clone(); // solo el mensaje; el join que lo observe le pone su propia posición
            if let Obj::Task(t) = self.heap.get_mut(task) {
                t.state = TaskState::Failed(msg);
            }
            self.wake_task_waiters(task);
        }
        // M12.5: si esta fibra poseía tareas (scopes activos cuyo cuerpo hizo panic), cancélalas en vez de
        // dejarlas huérfanas. (En `main` el programa aborta, así que esto importa para fibras hijas.)
        let orphans: Vec<Handle> = self.scopes.iter().flat_map(|s| s.children.iter().copied()).collect();
        for c in orphans {
            self.cancel_task(c);
        }
        self.frames.clear();
        self.stack.clear();
        self.scopes.clear();
        self.schedule_next(e.line, e.col)
    }

    /// Carga la siguiente fibra lista en los campos de ejecución de la VM. Si no hay ninguna lista pero sí
    /// fibras bloqueadas → **deadlock** (nadie puede desbloquearlas).
    fn schedule_next(&mut self, line: usize, col: usize) -> Result<(), RuntimeError> {
        // M15.5: si no hay nadie listo pero sí fibras esperando E/S de red, hacemos una **espera de E/S**:
        // dormimos un poco (no quemar CPU) y re-encolamos TODAS las aparcadas en E/S para que reintenten su
        // operación (las que sigan sin estar listas se volverán a aparcar). Busy-poll cooperativo, sin deps.
        if self.ready.is_empty() && !self.io_parked.is_empty() {
            crate::builtins::sleep_millis(1);
            for f in self.io_parked.drain(..) {
                self.ready.push_back(f);
            }
        }
        if let Some(next) = self.ready.pop_front() {
            self.frames = next.frames;
            self.stack = next.stack;
            self.current_is_main = next.is_main;
            self.current_task = next.task;
            self.scopes = next.scopes;
            Ok(())
        } else if !self.parked.is_empty() {
            Err(runtime_error(line, col, "deadlock: todas las fibras están bloqueadas esperando un canal o una tarea"))
        } else {
            Err(runtime_error(line, col, "no hay fibras ejecutables"))
        }
    }

    /// Despierta a todas las fibras aparcadas en `join` sobre `task` (M12.3): al re-ejecutar su `Join`/
    /// `ScopeEnd` verán la tarea ya terminada (Done/Failed). No empuja nada (el opcode rebobinó su `ip`).
    fn wake_task_waiters(&mut self, task: Handle) {
        let mut i = 0;
        while i < self.parked.len() {
            if self.parked[i].on == task && matches!(self.parked[i].waiting, Waiting::Join) {
                let parked = self.parked.remove(i);
                self.ready.push_back(parked.fiber);
            } else {
                i += 1;
            }
        }
    }

    /// Despierta a los `select` aparcados cuyo arreglo de canales contiene `chan`, porque acaba de pasar a
    /// estar listo para recibir (M12.4). Re-ejecutarán el `select` y verán el canal listo (o, si otro lo
    /// consumió antes, se volverán a bloquear). No empuja nada (el opcode rebobinó su `ip`).
    fn wake_select_waiters(&mut self, chan: Handle) {
        let mut i = 0;
        while i < self.parked.len() {
            let on = self.parked[i].on;
            let is_select = matches!(self.parked[i].waiting, Waiting::Select);
            let contains = is_select && match self.heap.get(on) {
                Obj::Array(elems) => elems.iter().any(|v| v.handle() == Some(chan)),
                _ => false,
            };
            if contains {
                let parked = self.parked.remove(i);
                self.ready.push_back(parked.fiber);
            } else {
                i += 1;
            }
        }
    }

    /// Cancela una tarea **pendiente** (M12.5, structured concurrency): la marca `Failed`, **saca** su fibra
    /// de `ready`/`parked` (no se reanudará nunca; sus marcos/locales los reclama el GC) y cancela
    /// **recursivamente** los hijos de los scopes de esa fibra (cancelación transitiva: una hermana
    /// cancelada que era dueña de un scope no deja nietos huérfanos). Si la tarea ya terminó, no hace nada.
    /// Es trivial porque el scheduler es cooperativo M:1: una fibra solo corre en los puntos de yield, así
    /// que "cancelar" = "retirar de las colas". No es preemptiva: no interrumpe código que corra sin ceder.
    fn cancel_task(&mut self, task: Handle) {
        match self.heap.get_mut(task) {
            Obj::Task(t) if matches!(t.state, TaskState::Pending) => {
                t.state = TaskState::Failed("tarea cancelada (una hermana falló)".to_string());
            }
            _ => return, // no es una tarea, o ya terminó (Done/Failed) → nada que cancelar
        }
        let mut grandchildren: Vec<Handle> = Vec::new();
        if let Some(pos) = self.ready.iter().position(|f| f.task == Some(task)) {
            let f = self.ready.remove(pos).unwrap();
            for s in &f.scopes {
                grandchildren.extend(s.children.iter().copied());
            }
        } else if let Some(pos) = self.parked.iter().position(|p| p.fiber.task == Some(task)) {
            let p = self.parked.remove(pos);
            for s in &p.fiber.scopes {
                grandchildren.extend(s.children.iter().copied());
            }
        } else if let Some(pos) = self.io_parked.iter().position(|f| f.task == Some(task)) {
            // M15.5: la fibra cancelada podría estar esperando E/S de red.
            let f = self.io_parked.remove(pos);
            for s in &f.scopes {
                grandchildren.extend(s.children.iter().copied());
            }
        }
        for g in grandchildren {
            self.cancel_task(g);
        }
    }

    /// Despierta una fibra bloqueada en `recv`: le deja `values` (envuelto en el `[T]` que devuelve el
    /// primitivo `__recv`) en su pila de operandos y la encola como lista. `[v]` la entrega un `send`; `[]`
    /// (vacío → `None`) la entrega un `close`.
    fn wake_recv(&mut self, mut fiber: Fiber, values: Vec<HeapValue>) {
        let arr = self.heap.allocate(Obj::Array(values));
        fiber.stack.push(HeapValue::Obj(arr));
        self.ready.push_back(fiber);
    }

    /// Despierta una fibra bloqueada en `send` (M12.2): su `send` ya quedó atrás (el `ip` apunta tras el
    /// ChanSend), así que solo le deja **unit** (el resultado de `send`) en la pila y la encola.
    fn wake_sender(&mut self, mut fiber: Fiber) {
        fiber.stack.push(HeapValue::Unit);
        self.ready.push_back(fiber);
    }

    /// Tras un `recv` que liberó un hueco: si hay un **emisor bloqueado** en `chan` (cola llena, M12.2),
    /// mete su valor pendiente en la cola (ahora hay sitio) y lo despierta. FIFO → el primero que se
    /// bloqueó despierta antes.
    fn wake_blocked_sender(&mut self, chan: Handle) {
        if let Some(pos) = self.parked.iter().position(
            |p| p.on == chan && matches!(p.waiting, Waiting::Send(_)))
        {
            let parked = self.parked.remove(pos);
            let sv = match parked.waiting {
                Waiting::Send(sv) => sv,
                _ => unreachable!(),
            };
            match self.heap.get_mut(chan) {
                Obj::Channel(c) => c.queue.push_back(sv),
                _ => unreachable!("el checker garantiza un Channel"),
            }
            self.wake_sender(parked.fiber);
        }
    }

    // ----- Recolección de basura (mark-and-sweep) -----

    /// Recolecta: marca desde las raíces (pila + locales + upvalues de los marcos),
    /// propaga y barre. Solo se llama en puntos seguros del bucle.
    fn collect(&mut self) {
        // Reunimos las raíces (handles) primero, para no tomar prestado `self.stack`
        // y `self.heap` a la vez. M12.1: además de la fibra en ejecución, rooteamos TODAS las fibras
        // (listas y bloqueadas) y los canales que esperan.
        let mut roots: Vec<Handle> = Vec::new();
        gather_roots(&self.frames, &self.stack, &mut roots);
        // M12.3: la Task y los scopes de la fibra en curso.
        roots.extend(self.current_task);
        for s in &self.scopes {
            roots.extend(s.children.iter().copied());
        }
        for f in &self.ready {
            gather_fiber_roots(f, &mut roots);
        }
        // M15.5: las fibras aparcadas esperando E/S también deben sobrevivir al GC.
        for f in &self.io_parked {
            gather_fiber_roots(f, &mut roots);
        }
        for p in &self.parked {
            roots.push(p.on); // el canal/tarea que espera la fibra debe sobrevivir aunque solo lo referencie ella
            // M12.2: un emisor bloqueado sostiene un valor pendiente que aún no entró al canal → es raíz.
            if let Waiting::Send(v) = &p.waiting {
                roots.extend(v.handle()); // `Option<Handle>` es iterable: añade el handle si lo hay
            }
            gather_fiber_roots(&p.fiber, &mut roots);
        }

        for h in roots {
            self.heap.mark(h);
        }
        self.heap.trace();
        self.heap.sweep();
    }

    // ----- Locales (con boxing) -----

    /// Crea el arreglo de locales de un marco nuevo: cada slot capturado nace
    /// **boxeado** (su celda en el heap), los demás como `Plain(Unit)`.
    fn new_locals(&mut self, fn_idx: usize) -> Vec<Local> {
        let n = self.program.functions[fn_idx].num_locals;
        // Opt.2: reusa un `Vec` del pool (conserva su capacidad) en vez de asignar uno nuevo. Lo vaciamos
        // y lo reconstruimos entero, así no se lee ninguna basura que arrastrara del uso anterior.
        let mut locals = self.locals_pool.pop().unwrap_or_default();
        locals.clear();
        for s in 0..n {
            if self.program.functions[fn_idx].captured.get(s).copied().unwrap_or(false) {
                let cell = self.heap.allocate(Obj::Cell(HeapValue::Unit));
                locals.push(Local::Boxed(cell));
            } else {
                locals.push(Local::Plain(HeapValue::Unit));
            }
        }
        locals
    }

    /// Opt.2: devuelve al pool el arreglo de locales de un marco que se descarta (Return, llamada en cola,
    /// fin de chunk). Acotado para no crecer sin límite; el GC no lo traza (contenido basura hasta reusar).
    fn recycle_locals(&mut self, locals: Vec<Local>) {
        if self.locals_pool.len() < 256 {
            self.locals_pool.push(locals);
        }
    }

    /// Coloca un argumento en un slot recién creado (respeta el boxing).
    fn put_arg(&mut self, locals: &mut [Local], slot: usize, v: HeapValue) {
        match &locals[slot] {
            Local::Boxed(h) => self.cell_set(*h, v),
            Local::Plain(_) => locals[slot] = Local::Plain(v),
        }
    }

    fn get_local(&self, fi: usize, slot: usize) -> HeapValue {
        match &self.frames[fi].locals[slot] {
            Local::Plain(v) => v.clone(),
            Local::Boxed(h) => self.cell_get(*h),
        }
    }

    fn set_local(&mut self, fi: usize, slot: usize, v: HeapValue) {
        match &self.frames[fi].locals[slot] {
            Local::Boxed(h) => {
                let h = *h;
                self.cell_set(h, v);
            }
            Local::Plain(_) => self.frames[fi].locals[slot] = Local::Plain(v),
        }
    }

    fn cell_get(&self, h: Handle) -> HeapValue {
        match self.heap.get(h) {
            Obj::Cell(v) => v.clone(),
            _ => unreachable!("se esperaba una celda"),
        }
    }

    fn cell_set(&mut self, h: Handle, v: HeapValue) {
        match self.heap.get_mut(h) {
            Obj::Cell(slot) => *slot = v,
            _ => unreachable!("se esperaba una celda"),
        }
    }

    // ----- Acceso a objetos del heap -----

    fn as_array(&self, h: Handle) -> &Vec<HeapValue> {
        match self.heap.get(h) {
            Obj::Array(v) => v,
            _ => unreachable!("el checker garantiza un arreglo"),
        }
    }

    fn as_array_mut(&mut self, h: Handle) -> &mut Vec<HeapValue> {
        match self.heap.get_mut(h) {
            Obj::Array(v) => v,
            _ => unreachable!("el checker garantiza un arreglo"),
        }
    }

    fn as_struct(&self, h: Handle) -> &VmStruct {
        match self.heap.get(h) {
            Obj::Struct(s) => s,
            _ => unreachable!("el checker garantiza un struct"),
        }
    }

    fn as_struct_mut(&mut self, h: Handle) -> &mut VmStruct {
        match self.heap.get_mut(h) {
            Obj::Struct(s) => s,
            _ => unreachable!("el checker garantiza un struct"),
        }
    }

    fn as_enum(&self, h: Handle) -> &VmEnum {
        match self.heap.get(h) {
            Obj::Enum(e) => e,
            _ => unreachable!("el checker garantiza un enum"),
        }
    }

    // ----- Pila de operandos -----

    fn push(&mut self, v: HeapValue) {
        self.stack.push(v);
    }

    fn pop(&mut self) -> HeapValue {
        self.stack.pop().expect("pila vacía: bytecode mal formado")
    }

    fn peek(&self) -> &HeapValue {
        self.stack.last().expect("pila vacía: bytecode mal formado")
    }

    fn pop_int(&mut self) -> i64 {
        match self.pop() {
            HeapValue::Int(n) => n,
            _ => unreachable!("el checker garantiza un int"),
        }
    }

    fn pop_obj(&mut self) -> Handle {
        match self.pop() {
            HeapValue::Obj(h) => h,
            _ => unreachable!("el checker garantiza un objeto"),
        }
    }

    /// Aplica un operador binario. Misma semántica que el intérprete de M1 (esa es la
    /// idea del oráculo: deben coincidir). La igualdad es **estructural** para los
    /// compuestos, por lo que necesita el heap.
    fn apply_binary(&self, op: &OpCode, left: HeapValue, right: HeapValue, line: usize, col: usize) -> Result<HeapValue, RuntimeError> {
        use HeapValue::*;
        use OpCode::*;
        // Igualdad: estructural, mirando el heap.
        match op {
            Equal => return Ok(Bool(values_equal(&self.heap, &left, &right))),
            NotEqual => return Ok(Bool(!values_equal(&self.heap, &left, &right))),
            _ => {}
        }
        Ok(match (op, left, right) {
            // M11.1a: `+` concatena dos strings.
            (Add, Str(a), Str(b)) => Str(a + &b),
            // M16.1b: `+` concatena dos bytes (inline, no son objetos del heap → van por aquí).
            (Add, Bytes(a), Bytes(b)) => {
                let mut v = a;
                v.extend_from_slice(&b);
                Bytes(v)
            }
            (Add, Int(a), Int(b)) => Int(a + b),
            (Sub, Int(a), Int(b)) => Int(a - b),
            (Mul, Int(a), Int(b)) => Int(a * b),
            (Div, Int(a), Int(b)) => {
                if b == 0 {
                    return Err(runtime_error(line, col, "división entera por cero"));
                }
                Int(a / b)
            }
            (Rem, Int(a), Int(b)) => {
                if b == 0 {
                    return Err(runtime_error(line, col, "módulo por cero"));
                }
                Int(a % b)
            }
            (Add, Float(a), Float(b)) => Float(a + b),
            (Sub, Float(a), Float(b)) => Float(a - b),
            (Mul, Float(a), Float(b)) => Float(a * b),
            (Div, Float(a), Float(b)) => Float(a / b),
            (Rem, Float(a), Float(b)) => Float(a % b),
            (Less, Int(a), Int(b)) => Bool(a < b),
            (LessEqual, Int(a), Int(b)) => Bool(a <= b),
            (Greater, Int(a), Int(b)) => Bool(a > b),
            (GreaterEqual, Int(a), Int(b)) => Bool(a >= b),
            (Less, Float(a), Float(b)) => Bool(a < b),
            (LessEqual, Float(a), Float(b)) => Bool(a <= b),
            (Greater, Float(a), Float(b)) => Bool(a > b),
            (GreaterEqual, Float(a), Float(b)) => Bool(a >= b),
            // M11.7d: orden de strings (lexicográfico) y char (por code point).
            (Less, Str(a), Str(b)) => Bool(a < b),
            (LessEqual, Str(a), Str(b)) => Bool(a <= b),
            (Greater, Str(a), Str(b)) => Bool(a > b),
            (GreaterEqual, Str(a), Str(b)) => Bool(a >= b),
            (Less, Char(a), Char(b)) => Bool(a < b),
            (LessEqual, Char(a), Char(b)) => Bool(a <= b),
            (Greater, Char(a), Char(b)) => Bool(a > b),
            (GreaterEqual, Char(a), Char(b)) => Bool(a >= b),
            _ => unreachable!("combinación operador/operandos que el checker debió rechazar"),
        })
    }
}

/// Convierte una constante del chunk (un `Value` del intérprete, siempre primitivo)
/// al valor de la VM.
fn const_to_heap(v: &Value) -> HeapValue {
    match v {
        Value::Int(n) => HeapValue::Int(*n),
        Value::Float(x) => HeapValue::Float(*x),
        Value::Bool(b) => HeapValue::Bool(*b),
        Value::Str(s) => HeapValue::Str(s.clone()),
        Value::Char(c) => HeapValue::Char(*c),
        Value::Bytes(b) => HeapValue::Bytes((**b).clone()),
        Value::Unit => HeapValue::Unit,
        _ => unreachable!("las constantes del chunk son primitivas"),
    }
}

/// Igualdad estructural entre valores de la VM (mira el heap). Las funciones y
/// closures se comparan por identidad (el checker prohíbe `==` sobre ellas).
fn values_equal(heap: &Heap, a: &HeapValue, b: &HeapValue) -> bool {
    use HeapValue as H;
    match (a, b) {
        (H::Int(x), H::Int(y)) => x == y,
        (H::Float(x), H::Float(y)) => x == y,
        (H::Bool(x), H::Bool(y)) => x == y,
        (H::Str(x), H::Str(y)) => x == y,
        (H::Char(x), H::Char(y)) => x == y,
        (H::Bytes(x), H::Bytes(y)) => x == y,
        (H::Unit, H::Unit) => true,
        (H::Function(x), H::Function(y)) => x == y,
        (H::Obj(x), H::Obj(y)) => match (heap.get(*x), heap.get(*y)) {
            (Obj::Array(va), Obj::Array(vb)) => {
                va.len() == vb.len() && va.iter().zip(vb).all(|(p, q)| values_equal(heap, p, q))
            }
            (Obj::Struct(sa), Obj::Struct(sb)) => {
                sa.name == sb.name
                    && sa.fields.len() == sb.fields.len()
                    && sa.fields.iter().zip(&sb.fields).all(|((n1, v1), (n2, v2))| n1 == n2 && values_equal(heap, v1, v2))
            }
            // Closures: identidad (mismo handle).
            (Obj::Closure(_), Obj::Closure(_)) => x == y,
            _ => false,
        },
        _ => false,
    }
}

/// Resuelve `(enum_id, tag)` de un enum a `(nombre_enum, nombre_variante)` usando la
/// tabla de enums del programa.
fn enum_names<'a>(enums: &'a [CompiledEnum], enum_id: usize, tag: usize) -> (&'a str, &'a str) {
    let e = &enums[enum_id];
    (e.name.as_str(), e.variants[tag].name.as_str())
}

/// Formatea un valor de la VM como texto (siguiendo handles en el heap). Debe
/// coincidir con el `Display` del `Value` del intérprete, para que `print` sea igual.
fn format_value(heap: &Heap, enums: &[CompiledEnum], v: &HeapValue) -> String {
    match v {
        HeapValue::Int(n) => n.to_string(),
        HeapValue::Float(x) => x.to_string(),
        HeapValue::Bool(b) => b.to_string(),
        HeapValue::Str(s) => s.clone(),
        HeapValue::Char(c) => c.to_string(),
        HeapValue::Bytes(b) => format!("bytes[{}]", b.len()),
        HeapValue::Unit => "()".to_string(),
        HeapValue::Function(_) => "<fn>".to_string(),
        HeapValue::Obj(h) => match heap.get(*h) {
            Obj::Array(elems) => {
                let parts: Vec<String> = elems.iter().map(|e| format_value(heap, enums, e)).collect();
                format!("[{}]", parts.join(", "))
            }
            Obj::Struct(s) => {
                let parts: Vec<String> = s.fields.iter().map(|(n, v)| format!("{}: {}", n, format_value(heap, enums, v))).collect();
                format!("{} {{ {} }}", s.name, parts.join(", "))
            }
            Obj::Enum(e) => {
                let (ename, vname) = enum_names(enums, e.enum_id, e.tag);
                if e.payload.is_empty() {
                    format!("{}.{}", ename, vname)
                } else {
                    let parts: Vec<String> = e.payload.iter().map(|v| format_value(heap, enums, v)).collect();
                    format!("{}.{}({})", ename, vname, parts.join(", "))
                }
            }
            Obj::Closure(_) => "<fn>".to_string(),
            Obj::Cell(_) => "<cell>".to_string(), // no debería imprimirse directamente
            // M13.1: el print de un Map está diferido; se ordena por clave (determinista).
            Obj::Map(m) => {
                let mut parts: Vec<String> = m.iter()
                    .map(|(k, v)| format!("{}: {}", k.to_value(), format_value(heap, enums, v)))
                    .collect();
                parts.sort();
                format!("Map{{{}}}", parts.join(", "))
            }
            // M12.1: un canal no tiene representación textual significativa (no se inspecciona).
            Obj::Channel(_) => "<channel>".to_string(),
            // M12.3: una tarea tampoco (se une con `join`, no se imprime).
            Obj::Task(_) => "<task>".to_string(),
        },
    }
}

/// Convierte un valor de la VM al `Value` del intérprete (para el resultado final y
/// el oráculo). Los compuestos se reconstruyen siguiendo el heap.
fn to_value(heap: &Heap, enums: &[CompiledEnum], v: &HeapValue) -> Value {
    match v {
        HeapValue::Int(n) => Value::Int(*n),
        HeapValue::Float(x) => Value::Float(*x),
        HeapValue::Bool(b) => Value::Bool(*b),
        HeapValue::Str(s) => Value::Str(s.clone()),
        HeapValue::Char(c) => Value::Char(*c),
        HeapValue::Bytes(b) => Value::Bytes(Rc::new(b.clone())),
        HeapValue::Unit => Value::Unit,
        HeapValue::Function(i) => Value::Function(*i),
        HeapValue::Obj(h) => match heap.get(*h) {
            Obj::Array(elems) => {
                let v: Vec<Value> = elems.iter().map(|e| to_value(heap, enums, e)).collect();
                Value::Array(Rc::new(RefCell::new(v)))
            }
            Obj::Struct(s) => {
                let fields: Vec<(String, Value)> = s.fields.iter().map(|(n, v)| (n.clone(), to_value(heap, enums, v))).collect();
                Value::Struct(Rc::new(RefCell::new(StructInstance { name: s.name.clone(), fields })))
            }
            Obj::Enum(e) => {
                let (ename, vname) = enum_names(enums, e.enum_id, e.tag);
                let payload: Vec<Value> = e.payload.iter().map(|v| to_value(heap, enums, v)).collect();
                Value::Enum(Rc::new(EnumInstance {
                    enum_name: ename.to_string(),
                    variant: vname.to_string(),
                    payload,
                }))
            }
            // Una closure como resultado: la representamos como función (su identidad
            // no se observa; se imprime <fn>).
            Obj::Closure(c) => Value::Function(c.index),
            Obj::Cell(inner) => to_value(heap, enums, inner),
            // M13.1: reconstruye el Map del intérprete (igual igualdad estructural → oráculo).
            Obj::Map(m) => {
                let mut hm: HashMap<MapKey, Value> = HashMap::with_capacity(m.len());
                for (k, val) in m {
                    hm.insert(k.clone(), to_value(heap, enums, val));
                }
                Value::Map(Rc::new(RefCell::new(hm)))
            }
            // M12.1: un canal vive solo en la VM y nunca es el resultado del programa (main devuelve
            // int/unit) ni cruza al intérprete (no hay oráculo concurrente) → no necesita representación.
            Obj::Channel(_) => unreachable!("un canal nunca es el resultado del programa"),
            // M12.3: una tarea tampoco es el resultado del programa.
            Obj::Task(_) => unreachable!("una tarea nunca es el resultado del programa"),
        },
    }
}

fn runtime_error(line: usize, col: usize, msg: &str) -> RuntimeError {
    RuntimeError { msg: msg.to_string(), line, col }
}

/// Reúne las raíces del GC (handles) de una fibra: los valores en su pila de operandos y, por cada marco,
/// sus locales (los `Boxed` son celdas del heap) y sus upvalues. Compartida por la fibra en ejecución y
/// las suspendidas (M12.1). Función libre para no tomar prestado `self` entero durante la recolección.
fn gather_roots(frames: &[CallFrame], stack: &[HeapValue], roots: &mut Vec<Handle>) {
    for v in stack {
        if let Some(h) = v.handle() {
            roots.push(h);
        }
    }
    for frame in frames {
        for slot in &frame.locals {
            match slot {
                Local::Plain(v) => {
                    if let Some(h) = v.handle() {
                        roots.push(h);
                    }
                }
                Local::Boxed(h) => roots.push(*h),
            }
        }
        roots.extend(frame.upvalues.iter().copied());
    }
}

/// Reúne las raíces de una fibra suspendida o lista (M12.3): su pila/marcos, su `Task` y los hijos de sus
/// scopes activos (las tareas que aún no ha unido).
fn gather_fiber_roots(f: &Fiber, roots: &mut Vec<Handle>) {
    gather_roots(&f.frames, &f.stack, roots);
    roots.extend(f.task);
    for s in &f.scopes {
        roots.extend(s.children.iter().copied());
    }
}

/// Convierte un valor de la VM en una clave de Map (M13.1). El checker garantiza el tipo.
fn heap_to_key(v: &HeapValue) -> MapKey {
    match v {
        HeapValue::Int(n) => MapKey::Int(*n),
        HeapValue::Str(s) => MapKey::Str(s.clone()),
        HeapValue::Char(c) => MapKey::Char(*c),
        HeapValue::Bool(b) => MapKey::Bool(*b),
        _ => unreachable!("el checker garantiza una clave hashable (int/string/char/bool)"),
    }
}

/// Reconstruye el valor de la VM a partir de una clave de Map (para `keys`, M13.1b).
fn key_to_heap(k: &MapKey) -> HeapValue {
    match k {
        MapKey::Int(n) => HeapValue::Int(*n),
        MapKey::Str(s) => HeapValue::Str(s.clone()),
        MapKey::Char(c) => HeapValue::Char(*c),
        MapKey::Bool(b) => HeapValue::Bool(*b),
    }
}

/// Comprueba que `i` es un índice válido en `0..len`; si no, error de ejecución.
fn bounds_check(i: i64, len: usize, line: usize, col: usize) -> Result<usize, RuntimeError> {
    if i < 0 || (i as usize) >= len {
        return Err(runtime_error(line, col, &format!("índice {} fuera de rango (longitud {})", i, len)));
    }
    Ok(i as usize)
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Expr;
    use crate::compiler::{compile_expr, compile_program};

    fn expr_of(src: &str) -> Expr {
        let prog_src = format!("fn v() {{ {} }}", src);
        let tokens = crate::lexer::lex(&prog_src).expect("lex ok");
        let prog = crate::parser::parse(tokens).expect("parse ok");
        *prog.functions[0].body.tail.clone().expect("expresión en posición tail")
    }

    fn run_vm(src: &str) -> Value {
        let chunk = compile_expr(&expr_of(src)).expect("compila");
        run(&chunk).expect("ejecuta sin error")
    }

    /// Oráculo a nivel de expresión (int): VM vs intérprete.
    fn oracle_int(src: &str) {
        let prog_src = format!("fn main() -> int {{ {} }}", src);
        let tokens = crate::lexer::lex(&prog_src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("intérprete ok");
        let vm = run_vm(src);
        assert_eq!(interp, vm, "VM y intérprete difieren en `{}`", src);
    }

    /// **El oráculo a nivel de programa completo**: compila y ejecuta el programa
    /// en la VM y en el intérprete, y exige que el resultado coincida.
    fn oracle_program(src: &str) {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("intérprete ok");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect("vm ok");
        assert_eq!(interp, vm, "VM y intérprete difieren");
    }

    /// Ejecuta un programa en la VM con el GC en **modo estrés** (recolecta en cada
    /// punto seguro) y exige que el resultado coincida con el intérprete. Es la
    /// prueba clave del GC: si una raíz faltara, un valor vivo se liberaría y el
    /// resultado cambiaría o reventaría.
    fn oracle_stress(src: &str) {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("intérprete ok");
        let compiled = compile_program(&prog).expect("compila");
        let mut vm = Vm::new(&compiled);
        vm.heap.stress = true;
        let result = vm.run().expect("vm ok");
        let vm_result = to_value(&vm.heap, &compiled.enums, &result);
        assert_eq!(interp, vm_result, "VM (estrés) y intérprete difieren en:\n{}", src);
    }

    // ----- M2.1 / M2.2: expresiones -----

    #[test]
    fn aritmetica_coincide_con_el_interprete() {
        oracle_int("1 + 2 * 3");
        oracle_int("(1 + 2) * 3");
        oracle_int("10 - 2 - 3");
        oracle_int("17 % 5");
        oracle_int("-5 + 3");
        oracle_int("2 * 3 * 4 - 10 / 2");
    }

    #[test]
    fn comparaciones_y_bools() {
        assert_eq!(run_vm("3 < 5"), Value::Bool(true));
        assert_eq!(run_vm("3 == 5"), Value::Bool(false));
        assert_eq!(run_vm("!(2 > 1)"), Value::Bool(false));
        assert_eq!(run_vm("true"), Value::Bool(true));
    }

    #[test]
    fn flotantes() {
        assert_eq!(run_vm("1.0 / 2.0"), Value::Float(0.5));
        assert_eq!(run_vm("1.5 + 1.5"), Value::Float(3.0));
    }

    #[test]
    fn division_por_cero_es_error() {
        let chunk = compile_expr(&expr_of("10 / 0")).unwrap();
        assert!(run(&chunk).unwrap_err().msg.contains("división"));
    }

    #[test]
    fn if_como_expresion_coincide_con_el_interprete() {
        oracle_int("if (3 < 5) { 10 } else { 20 }");
        oracle_int("if (3 > 5) { 10 } else { 20 }");
        oracle_int("if (1 < 2) { if (2 < 3) { 1 } else { 2 } } else { 3 }");
        oracle_int("if (1 < 2 && 3 < 4) { 7 } else { 8 }");
    }

    #[test]
    fn if_sin_else_es_unit() {
        assert_eq!(run_vm("if (true) { }"), Value::Unit);
        assert_eq!(run_vm("if (false) { }"), Value::Unit);
    }

    #[test]
    fn logicos_y_su_cortocircuito() {
        assert_eq!(run_vm("true && true"), Value::Bool(true));
        assert_eq!(run_vm("true && false"), Value::Bool(false));
        assert_eq!(run_vm("false || true"), Value::Bool(true));
        assert_eq!(run_vm("false && (1 / 0 == 0)"), Value::Bool(false));
        assert_eq!(run_vm("true || (1 / 0 == 0)"), Value::Bool(true));
    }

    #[test]
    fn bloque_con_sentencias_y_valor_final() {
        assert_eq!(run_vm("{ 1; 2; 3 }"), Value::Int(3));
        assert_eq!(run_vm("{ 1; }"), Value::Unit);
    }

    // ----- M2.3: programas completos (variables, while, llamadas) -----

    #[test]
    fn recursion_fibonacci() {
        oracle_program(
            "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }
             fn main() -> int { fib(10) }",
        );
    }

    #[test]
    fn factorial_con_while_y_mutacion() {
        oracle_program(
            "fn main() -> int {
                var n: int = 5; var f: int = 1;
                while (n > 1) { f = f * n; n = n - 1; }
                f
             }",
        );
    }

    #[test]
    fn retorno_temprano() {
        oracle_program(
            "fn signo(x: int) -> int { if (x < 0) { return -1; } if (x > 0) { return 1; } 0 }
             fn main() -> int { signo(-7) + signo(0) + signo(42) }",
        );
    }

    #[test]
    fn gcd_recursivo() {
        oracle_program(
            "fn gcd(a: int, b: int) -> int { if (b == 0) { a } else { gcd(b, a % b) } }
             fn main() -> int { gcd(1071, 462) }",
        );
    }

    /// M13.3a: recursión infinita → ambos motores cortan con el MISMO error de
    /// desbordamiento, en vez de colgarse o reventar la pila. Es el oráculo del
    /// límite compartido (`MAX_CALL_DEPTH` == `MAX_FRAMES`). Corre dentro del hilo de
    /// pila grande para que el intérprete alcance el tope sin desbordar la pila del
    /// hilo de test (que es pequeña por defecto). **La recursión es NO de cola**
    /// (`1 + bucle(...)`): la de cola, con el TCO de M13.3b, sería un bucle infinito
    /// legítimo (O(1) marcos) y nunca desbordaría —ese es justo el punto del TCO—.
    #[test]
    fn overflow_recursion_oraculo() {
        let (interp_msg, vm_msg) = crate::with_big_stack(|| {
            let src = "fn bucle(n: int) -> int { 1 + bucle(n + 1) }
                       fn main() -> int { bucle(0) }";
            let tokens = crate::lexer::lex(src).expect("lex ok");
            let mut prog = crate::parser::parse(tokens).expect("parse ok");
            crate::checker::check(&mut prog).expect("check ok");
            let interp = crate::interpreter::run(&prog).expect_err("el intérprete debe errar");
            let compiled = compile_program(&prog).expect("compila");
            let vm = run_program(&compiled).expect_err("la VM debe errar");
            (interp.msg, vm.msg)
        });
        assert!(interp_msg.contains("desbordamiento de pila"), "intérprete: {interp_msg}");
        assert!(vm_msg.contains("desbordamiento de pila"), "vm: {vm_msg}");
        // Ambos motores reportan exactamente el mismo mensaje.
        assert_eq!(interp_msg, vm_msg, "los dos motores difieren en el mensaje");
    }

    /// M13.2a: aserciones que pasan no alteran el resultado (oráculo normal).
    #[test]
    fn assert_pasa_oraculo() {
        oracle_program(
            "fn main() -> int {
                assert(1 + 1 == 2);
                assert_eq(2 * 3, 6);
                assert_eq(\"ab\", \"a\" + \"b\");
                42
             }",
        );
    }

    /// M13.2a: `panic` / `assert_eq` que falla → ambos motores cortan con el MISMO mensaje.
    #[test]
    fn panic_y_assert_falla_oraculo() {
        for (src, esperado) in [
            ("fn main() -> int { panic(\"boom\"); 0 }", "boom"),
            ("fn main() -> int { assert_eq(2 + 2, 5); 0 }", "assert_eq falló: 4 != 5"),
            ("fn main() -> int { assert(false); 0 }", "aserción falló"),
        ] {
            let tokens = crate::lexer::lex(src).expect("lex ok");
            let mut prog = crate::parser::parse(tokens).expect("parse ok");
            crate::checker::check(&mut prog).expect("check ok");
            let interp = crate::interpreter::run(&prog).expect_err("el intérprete debe errar");
            let compiled = compile_program(&prog).expect("compila");
            let vm = run_program(&compiled).expect_err("la VM debe errar");
            assert_eq!(interp.msg, esperado, "intérprete: {}", src);
            assert_eq!(vm.msg, esperado, "vm: {}", src);
        }
    }

    /// M15.1a: la stdlib de matemáticas en el oráculo. Las funciones float se enrutan a `int` por la
    /// comparación de floats del propio lenguaje (NaN != NaN impediría comparar `Value::Float`
    /// directamente); abs/min/max sobre `int` devuelven `int`. El último caso fija la semántica de
    /// **borde** de `f64`: `sqrt(-1.0)` da `NaN` en ambos motores → `NaN == NaN` es `false` → `0`.
    #[test]
    fn matematicas_oraculo() {
        // Polimórficas sobre int → resultado int directo.
        oracle_int("abs(-7)");
        oracle_int("abs(7)");
        oracle_int("min(3, 8)");
        oracle_int("max(3, 8)");
        // Funciones float, verificadas por igualdad (ambos motores calculan idéntico).
        oracle_int("if (sqrt(16.0) == 4.0) { 1 } else { 0 }");
        oracle_int("if (pow(2.0, 10.0) == 1024.0) { 1 } else { 0 }");
        oracle_int("if (floor(3.7) == 3.0) { 1 } else { 0 }");
        oracle_int("if (ceil(3.2) == 4.0) { 1 } else { 0 }");
        oracle_int("if (round(2.5) == 3.0) { 1 } else { 0 }");
        oracle_int("if (abs(-2.5) == 2.5) { 1 } else { 0 }");
        oracle_int("if (min(1.5, 9.0) == 1.5) { 1 } else { 0 }");
        oracle_int("if (max(1.5, 9.0) == 9.0) { 1 } else { 0 }");
        oracle_int("if (sin(0.0) == 0.0) { 1 } else { 0 }");
        oracle_int("if (cos(0.0) == 1.0) { 1 } else { 0 }");
        oracle_int("if (ln(e()) == 1.0) { 1 } else { 0 }");
        oracle_int("if (log10(1000.0) == 3.0) { 1 } else { 0 }");
        oracle_int("if (exp(0.0) == 1.0) { 1 } else { 0 }");
        oracle_int("if (pi() > 3.14) { 1 } else { 0 }");
        // Borde: NaN se comporta igual en ambos motores (NaN != NaN → la rama else).
        oracle_int("if (sqrt(0.0 - 1.0) == sqrt(0.0 - 1.0)) { 1 } else { 0 }");
    }

    /// M16.1a: el tipo `bytes` en el oráculo. Literal `b"..."` (con `\xNN`), `len`, indexar (→int) e
    /// igualdad. Se enruta a `int` (el booleano de `==` vía `if`) porque `print(bytes)` está diferido.
    #[test]
    fn bytes_oraculo() {
        oracle_int("len(b\"AB\")");                    // 2
        oracle_int("len(b\"hola\")");                  // 4
        oracle_int("b\"\\x00\\xff\"[1]");              // 255
        oracle_int("b\"AB\\x00\"[0]");                 // 65
        oracle_int("b\"AB\\x00\"[2]");                 // 0
        oracle_int("len(b\"\")");                      // 0 (vacío)
        // Igualdad estructural (misma secuencia / distinta) → 1/0.
        oracle_int("if (b\"AB\\xff\" == b\"AB\\xff\") { 1 } else { 0 }");
        oracle_int("if (b\"AB\" == b\"AC\") { 1 } else { 0 }");
        oracle_int("if (b\"AB\" == b\"ABC\") { 1 } else { 0 }");
        // Los caracteres no-ASCII se codifican como UTF-8 (á = 2 octetos).
        oracle_int("len(b\"á\")");                     // 2
        // M16.1b: to_bytes (builtin) + concatenación (opcode Add).
        oracle_int("len(to_bytes(\"hola, mundo\"))");                   // 11
        oracle_int("len(to_bytes(\"á\"))");                            // 2 (UTF-8)
        oracle_int("len(to_bytes(\"AB\") + to_bytes(\"CD\"))");        // 4
        oracle_int("if (to_bytes(\"AB\") == b\"AB\") { 1 } else { 0 }");
        oracle_int("if (to_bytes(\"A\") + to_bytes(\"B\") == b\"AB\") { 1 } else { 0 }");
    }

    /// M16.1b: `from_utf8` es un envoltorio del **prelude** (no un opcode), así que se prueba con el
    /// oráculo a nivel de programa completo (que inyecta el prelude), no con expresiones sueltas.
    #[test]
    fn bytes_from_utf8_oraculo() {
        // Round-trip válido: decodifica y mide la longitud del string.
        oracle_program("fn main() -> int { match (from_utf8(b\"hola\")) { Result.Ok(s) => len(s), Result.Err(e) => -1, } }");
        // UTF-8 inválido → Err → 0.
        oracle_program("fn main() -> int { match (from_utf8(b\"\\xff\\xfe\")) { Result.Ok(s) => 1, Result.Err(e) => 0, } }");
        // to_bytes ∘ from_utf8 es identidad sobre texto válido.
        oracle_program("fn main() -> int { match (from_utf8(to_bytes(\"raylang\"))) { Result.Ok(s) => len(s), Result.Err(e) => -1, } }");
    }

    /// M13.1: Map en el oráculo. Las operaciones básicas dan el mismo resultado en ambos motores.
    #[test]
    fn map_basico_oraculo() {
        oracle_program(
            "fn main() -> int {
                let m: Map<string, int> = map_new();
                insert(m, \"a\", 1);
                insert(m, \"b\", 2);
                insert(m, \"a\", 10);
                let total = match (m.get(\"a\")) { Option.Some(v) => v, Option.None => 0 };
                total + len(m)
             }",
        );
    }

    /// M13.1: el Map asigna en el heap y guarda valores → estrés del GC (recolecta en cada paso).
    /// Si una raíz faltara, los valores guardados se liberarían y el resultado cambiaría.
    #[test]
    fn map_estres_gc_oraculo() {
        oracle_stress(
            "fn celda(n: int) -> [int] { [n, n * 2] }
             fn main() -> int {
                let m: Map<int, [int]> = map_new();
                var i = 0;
                while (i < 30) { insert(m, i, celda(i)); i = i + 1; }
                var suma = 0;
                var j = 0;
                while (j < 30) {
                    match (m.get(j)) {
                        Option.Some(par) => { suma = suma + par[0] + par[1]; },
                        Option.None => { suma = suma - 1; },
                    }
                    j = j + 1;
                }
                suma + len(m)
             }",
        );
    }

    /// M13.1: claves de distintos tipos primitivos hashables.
    #[test]
    fn map_claves_variadas_oraculo() {
        oracle_program(
            "fn main() -> int {
                let porInt: Map<int, int> = map_new();
                insert(porInt, 7, 70);
                let porChar: Map<char, int> = map_new();
                insert(porChar, 'z', 100);
                let porBool: Map<bool, int> = map_new();
                insert(porBool, true, 1);
                insert(porBool, false, 2);
                let a = match (porInt.get(7)) { Option.Some(v) => v, Option.None => 0 };
                let b = match (porChar.get('z')) { Option.Some(v) => v, Option.None => 0 };
                let c = match (porBool.get(true)) { Option.Some(v) => v, Option.None => 0 };
                a + b + c + len(porBool)
             }",
        );
    }

    /// M13.1b: keys (ordenadas) + values (en orden de clave) + remove, en el oráculo.
    #[test]
    fn map_keys_values_remove_oraculo() {
        oracle_program(
            "fn suma(a: [int]) -> int { var s = 0; var i = 0; while (i < len(a)) { s = s + a[i]; i = i + 1; } s }
             fn main() -> int {
                let m: Map<int, int> = map_new();
                insert(m, 3, 30);
                insert(m, 1, 10);
                insert(m, 2, 20);
                let ks = keys(m);              // [1, 2, 3]
                let vs = values(m);            // [10, 20, 30]
                let quitado = match (remove(m, 2)) { Option.Some(v) => v, Option.None => 0 };
                ks[0] * 100 + ks[2] + suma(vs) + quitado + len(m)
             }",
        );
    }

    /// M13.1b: keys/values asignan arreglos en el heap → estrés del GC.
    #[test]
    fn map_keys_values_estres_gc_oraculo() {
        oracle_stress(
            "fn suma(a: [int]) -> int { var s = 0; var i = 0; while (i < len(a)) { s = s + a[i]; i = i + 1; } s }
             fn main() -> int {
                let m: Map<int, int> = map_new();
                var i = 0;
                while (i < 25) { insert(m, i, i * i); i = i + 1; }
                let total = suma(values(m)) + suma(keys(m));
                var quitados = 0;
                var j = 0;
                while (j < 25) {
                    match (remove(m, j)) {
                        Option.Some(v) => { quitados = quitados + 1; },
                        Option.None => {},
                    }
                    j = j + 2;
                }
                total + quitados + len(m)
             }",
        );
    }

    /// M13.3b: recursión de cola PROFUNDA (más allá de MAX_FRAMES) funciona en ambos motores
    /// gracias al TCO, y coinciden. Sin TCO, ambos cortarían en 1024 con desbordamiento.
    #[test]
    fn tco_recursion_de_cola_profunda_oraculo() {
        // 5000 > MAX_FRAMES (1024): solo pasa si la llamada en cola reutiliza el marco.
        oracle_program(
            "fn cuenta(n: int, acc: int) -> int {
                if (n == 0) { acc } else { cuenta(n - 1, acc + 1) }
             }
             fn main() -> int { cuenta(5000, 0) }",
        );
    }

    /// M13.3b: recursión mutua en cola + `return` en cola, también profunda.
    #[test]
    fn tco_mutua_y_return_en_cola_oraculo() {
        oracle_program(
            "fn par(n: int) -> bool { if (n == 0) { true } else { return impar(n - 1); } }
             fn impar(n: int) -> bool { if (n == 0) { false } else { par(n - 1) } }
             fn main() -> int { if (par(4000)) { 1 } else { 0 } }",
        );
    }

    /// M13.3b: una llamada que NO está en cola (su valor se usa en `n + ...`) sigue recurriendo de
    /// verdad —el TCO no debe convertirla— y da el mismo resultado en ambos motores. La profundidad
    /// es modesta porque el intérprete recurre sobre la pila de Rust (el hilo de test es pequeño; el
    /// binario real corre con pila grande, M13.3a). Que la recursión de cola SÍ se optimiza lo
    /// prueban `tco_recursion_de_cola_profunda_oraculo` (5000) y `tco_mutua_*` (4000).
    #[test]
    fn tco_no_aplica_a_llamada_no_en_cola_oraculo() {
        oracle_program(
            "fn suma_hasta(n: int) -> int { if (n == 0) { 0 } else { n + suma_hasta(n - 1) } }
             fn main() -> int { suma_hasta(30) }",
        );
    }

    #[test]
    fn variables_locales_y_shadowing() {
        oracle_program("fn main() -> int { let x: int = 1; { let x: int = 99; } x }");
        oracle_program(
            "fn main() -> int { var s: int = 0; var i: int = 0; while (i < 5) { s = s + i; i = i + 1; } s }",
        );
    }

    #[test]
    fn programa_con_print() {
        oracle_program("fn main() -> int { print(42); print(true); 0 }");
    }

    // ----- M3.1: arreglos -----

    #[test]
    fn arreglos_indexar_len_y_suma() {
        oracle_program("fn main() -> int { let a: [int] = [10, 20, 30]; a[0] + a[2] }");
        oracle_program("fn main() -> int { let a: [int] = [1, 2, 3, 4]; len(a) }");
    }

    #[test]
    fn arreglos_mutacion_y_push() {
        oracle_program("fn main() -> int { var a: [int] = [1, 2, 3]; a[1] = 99; a[1] }");
        oracle_program(
            "fn main() -> int { let a: [int] = []; push(a, 5); push(a, 7); a[0] + a[1] }",
        );
    }

    #[test]
    fn arreglos_son_por_referencia() {
        oracle_program("fn main() -> int { let a: [int] = [1, 2, 3]; let b: [int] = a; b[0] = 9; a[0] }");
    }

    #[test]
    fn suma_de_un_arreglo_con_while() {
        oracle_program(
            "fn suma(a: [int]) -> int {
                var s: int = 0; var i: int = 0;
                while (i < len(a)) { s = s + a[i]; i = i + 1; }
                s
             }
             fn main() -> int { suma([5, 10, 15, 20]) }",
        );
    }

    #[test]
    fn indice_fuera_de_rango_es_error() {
        let prog_src = "fn main() -> int { let a: [int] = [1, 2]; a[5] }";
        let tokens = crate::lexer::lex(prog_src).unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        let compiled = compile_program(&prog).unwrap();
        assert!(run_program(&compiled).unwrap_err().msg.contains("fuera de rango"));
    }

    // ----- M3.2: structs -----

    #[test]
    fn structs_acceso_y_orden_de_campos() {
        oracle_program("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 3, y: 4 }; p.x + p.y }");
        oracle_program("struct P { x: int, y: int } fn main() -> int { let p: P = P { y: 4, x: 3 }; p.x - p.y }");
    }

    #[test]
    fn structs_mutacion_de_campo() {
        oracle_program("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 1, y: 2 }; p.x = 9; p.x + p.y }");
    }

    #[test]
    fn structs_son_por_referencia() {
        oracle_program("struct C { v: int } fn main() -> int { let a: C = C { v: 1 }; let b: C = a; b.v = 9; a.v }");
    }

    #[test]
    fn structs_anidados_y_con_arreglos() {
        oracle_program(
            "struct P { x: int, y: int }
             struct L { a: P, b: P }
             fn dx(l: L) -> int { l.b.x - l.a.x }
             fn main() -> int { dx(L { a: P { x: 1, y: 0 }, b: P { x: 5, y: 0 } }) }",
        );
        oracle_program(
            "struct Pila { datos: [int] }
             fn main() -> int { let s: Pila = Pila { datos: [10, 20] }; push(s.datos, 30); s.datos[2] }",
        );
    }

    // ----- M4.1: funciones de primera clase -----

    #[test]
    fn funcion_anonima_en_variable() {
        oracle_program("fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x * x }; f(9) }");
    }

    #[test]
    fn de_orden_superior_recibe_funcion() {
        oracle_program(
            "fn aplicar(f: fn(int) -> int, x: int) -> int { f(x) }
             fn main() -> int { aplicar(fn(n: int) -> int { n + 1 }, 41) }",
        );
    }

    #[test]
    fn nombre_de_funcion_como_valor() {
        oracle_program(
            "fn inc(n: int) -> int { n + 1 }
             fn aplicar(f: fn(int) -> int, x: int) -> int { f(x) }
             fn main() -> int { aplicar(inc, 10) }",
        );
    }

    #[test]
    fn devolver_una_funcion() {
        oracle_program(
            "fn elegir(b: bool) -> fn(int) -> int {
                 if (b) { fn(n: int) -> int { n + n } } else { fn(n: int) -> int { n * n } }
             }
             fn main() -> int { let f: fn(int) -> int = elegir(true); f(21) }",
        );
    }

    #[test]
    fn llamar_un_literal_de_funcion_directo() {
        oracle_program("fn main() -> int { (fn(x: int) -> int { x + x })(21) }");
    }

    #[test]
    fn variable_tapa_a_funcion_global() {
        oracle_program(
            "fn f(x: int) -> int { x * 100 }
             fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x + 1 }; f(41) }",
        );
    }

    #[test]
    fn mapear_sobre_arreglo_con_funcion() {
        oracle_program(
            "fn mapear(a: [int], f: fn(int) -> int) {
                 var i: int = 0;
                 while (i < len(a)) { a[i] = f(a[i]); i = i + 1; }
             }
             fn main() -> int {
                 var xs: [int] = [1, 2, 3, 4];
                 mapear(xs, fn(n: int) -> int { n * n });
                 xs[0] + xs[1] + xs[2] + xs[3]
             }",
        );
    }

    // ----- M4.2: closures (captura de entorno) -----

    #[test]
    fn closure_captura_un_let() {
        oracle_program(
            "fn main() -> int {
                 let base: int = 1000;
                 let f: fn(int) -> int = fn(d: int) -> int { base + d };
                 f(7)
             }",
        );
    }

    #[test]
    fn contador_con_estado_mutable() {
        oracle_program(
            "fn contador() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
             fn main() -> int { let c: fn() -> int = contador(); c(); c(); c() }",
        );
    }

    #[test]
    fn instancias_de_closure_son_independientes() {
        oracle_program(
            "fn contador() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
             fn main() -> int {
                 let a: fn() -> int = contador();
                 let b: fn() -> int = contador();
                 a(); a(); a();   // n de a -> 3
                 b();             // n de b -> 1 (su propia celda, independiente)
                 a() + b()        // a()->4, b()->2 => 6
             }",
        );
    }

    #[test]
    fn captura_transitiva_dos_niveles() {
        oracle_program(
            "fn sumador(x: int) -> fn(int) -> int { fn(y: int) -> int { x + y } }
             fn main() -> int { let add5: fn(int) -> int = sumador(5); add5(10) + add5(100) }",
        );
    }

    #[test]
    fn closures_hermanas_comparten_celda() {
        oracle_program(
            "struct Par { inc: fn(), get: fn() -> int }
             fn hacer() -> Par {
                 var n: int = 0;
                 Par { inc: fn() { n = n + 1; }, get: fn() -> int { n } }
             }
             fn main() -> int { let p: Par = hacer(); p.inc(); p.inc(); p.inc(); p.get() }",
        );
    }

    #[test]
    fn closure_en_arreglo_y_orden_superior() {
        oracle_program(
            "fn aplica_dos(f: fn(int) -> int, x: int) -> int { f(f(x)) }
             fn main() -> int {
                 let k: int = 3;
                 aplica_dos(fn(n: int) -> int { n + k }, 10)
             }",
        );
    }

    // ----- M5.1: enums (tipos suma) y construcción -----

    #[test]
    fn enum_construccion_oraculo() {
        // Ambos motores construyen variantes (con y sin payload) y coinciden en el
        // resultado. El payload se evalúa en orden antes de MakeEnum.
        oracle_program(
            "enum E { A(int, int), B }
             fn main() -> int { let x: E = E.A(2, 3); let y: E = E.B; print(x); print(y); 0 }",
        );
    }

    #[test]
    fn enum_recursivo_oraculo() {
        oracle_program(
            "enum Lista { Cons(int, Lista), Nil }
             fn main() -> int { let xs: Lista = Lista.Cons(1, Lista.Cons(2, Lista.Nil)); print(xs); 0 }",
        );
    }

    #[test]
    fn derive_show_oraculo() {
        // `@derive(Show)` genera `mostrar` (front-end → impls normales): el intérprete y la VM
        // deben producir la **misma** cadena. Se compara vía `len` (el oráculo mira el retorno).
        oracle_program(
            "@derive(Show)
             enum Color { Rojo, RGB(int, int, int) }
             @derive(Show)
             struct Punto { x: int, y: int }
             fn main() -> int {
                 let p = Punto { x: 3, y: 40 };
                 print(p.mostrar());
                 print(Color.RGB(1, 2, 3).mostrar());
                 len(p.mostrar()) + len(Color.RGB(1, 2, 3).mostrar())
             }",
        );
    }

    #[test]
    fn enums_en_modo_estres() {
        // Construir enums (incl. recursivos) con el GC recolectando en cada punto
        // seguro: si el trazado del payload faltara, un valor vivo se liberaría.
        oracle_stress(
            "enum Lista { Cons(int, Lista), Nil }
             fn construir(n: int) -> Lista {
                 if (n == 0) { Lista.Nil } else { Lista.Cons(n, construir(n - 1)) }
             }
             fn main() -> int { let xs: Lista = construir(20); print(xs); 0 }",
        );
    }

    #[test]
    fn el_gc_libera_enums_inalcanzables() {
        // Cada llamada construye una lista enlazada que queda inalcanzable al
        // retornar. El mark-and-sweep debe barrer esos objetos de enum: el heap
        // queda acotado en vez de crecer sin parar.
        let src = r#"
            enum Lista { Cons(int, Lista), Nil }
            fn construir(n: int) -> Lista {
                if (n == 0) { Lista.Nil } else { Lista.Cons(n, construir(n - 1)) }
            }
            fn main() -> int {
                var i: int = 0;
                while (i < 50) { let xs: Lista = construir(10); i = i + 1; }
                0
            }
        "#;
        let tokens = crate::lexer::lex(src).unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        let compiled = compile_program(&prog).unwrap();
        let mut vm = Vm::new(&compiled);
        vm.run().expect("vm ok");
        // Sin GC habría ~550 objetos vivos; con barrido, muy pocos.
        assert!(vm.heap.live() < 80, "el heap no se acotó: {} objetos vivos", vm.heap.live());
    }

    // ----- M5.3: match en la VM (oráculo VM<->intérprete) -----

    #[test]
    fn match_recorrido_oraculo() {
        // Recorrer un enum recursivo con match: longitud y suma, en ambos motores.
        oracle_program(
            "enum Lista { Cons(int, Lista), Nil }
             fn longitud(xs: Lista) -> int { match (xs) { Lista.Cons(_, t) => 1 + longitud(t), Lista.Nil => 0 } }
             fn suma(xs: Lista) -> int { match (xs) { Lista.Cons(h, t) => h + suma(t), Lista.Nil => 0 } }
             fn main() -> int {
                 let xs: Lista = Lista.Cons(10, Lista.Cons(20, Lista.Cons(30, Lista.Nil)));
                 longitud(xs) * 100 + suma(xs)
             }",
        );
    }

    #[test]
    fn match_selecciona_brazo_oraculo() {
        // Variantes con distinta aridad de payload; cada brazo liga lo suyo.
        oracle_program(
            "enum Figura { Circulo(int), Rect(int, int), Punto }
             fn area(f: Figura) -> int {
                 match (f) { Figura.Circulo(r) => 3 * r * r, Figura.Rect(w, h) => w * h, Figura.Punto => 0 }
             }
             fn main() -> int { area(Figura.Rect(4, 5)) + area(Figura.Circulo(2)) + area(Figura.Punto) }",
        );
    }

    #[test]
    fn match_comodin_y_binding_oraculo() {
        // Comodín `_` (dentro de variante y suelto) y binding catch-all.
        oracle_program(
            "enum E { Uno, Dos, Otro }
             fn n(e: E) -> int { match (e) { E.Uno => 1, otro => 99 } }
             fn main() -> int { n(E.Uno) * 100 + n(E.Dos) }",
        );
    }

    #[test]
    fn match_en_modo_estres() {
        // La prueba clave de M5.3: con el GC recolectando en CADA punto seguro, el
        // escrutinio guardado en el local temporal y el payload extraído deben seguir
        // rooteados. Si faltara una raíz, recorrer la lista reventaría o cambiaría.
        oracle_stress(
            "enum Lista { Cons(int, Lista), Nil }
             fn construir(n: int) -> Lista { if (n == 0) { Lista.Nil } else { Lista.Cons(n, construir(n - 1)) } }
             fn suma(xs: Lista) -> int { match (xs) { Lista.Cons(h, t) => h + suma(t), Lista.Nil => 0 } }
             fn main() -> int { suma(construir(15)) }",
        );
    }

    #[test]
    fn match_binding_capturado_por_closure_oraculo() {
        // Interacción fina: un binding de match capturado por una closure debe
        // BOXEARSE (vivir en una celda). InitLocal sobre el slot del binding lo
        // maneja, igual que con un `let`. Ambos motores deben coincidir.
        oracle_program(
            "enum E { A(int), B(int), C }
             fn sumador(e: E) -> fn(int) -> int {
                 match (e) {
                     E.A(n) => fn(x: int) -> int { x + n },
                     E.B(n) => fn(x: int) -> int { x * n },
                     E.C    => fn(x: int) -> int { x },
                 }
             }
             fn main() -> int {
                 let f: fn(int) -> int = sumador(E.A(10));
                 let g: fn(int) -> int = sumador(E.B(3));
                 f(5) + g(5)
             }",
        );
    }

    #[test]
    fn match_anidado_en_expresiones_oraculo() {
        // match como expresión: su valor alimenta otra operación, y el cuerpo de un
        // brazo construye otra variante (resolución dentro del brazo).
        oracle_program(
            "enum Sem { Rojo, Verde }
             fn opuesto(s: Sem) -> Sem { match (s) { Sem.Rojo => Sem.Verde, Sem.Verde => Sem.Rojo } }
             fn a_int(s: Sem) -> int { match (s) { Sem.Rojo => 0, Sem.Verde => 1 } }
             fn main() -> int { a_int(opuesto(Sem.Rojo)) + a_int(opuesto(Sem.Verde)) * 10 }",
        );
    }

    // ----- M6.1: funciones genéricas (erasure: ambos motores coinciden) -----

    #[test]
    fn generica_identidad_oraculo() {
        // Con borrado de tipos, una función genérica solo mueve valores: el resultado
        // debe coincidir en intérprete y VM sin que el runtime sepa nada de T.
        oracle_program(
            "fn identidad<T>(x: T) -> T { x }
             fn main() -> int { let b: bool = identidad(true); let n: int = identidad(7); if (b) { n } else { 0 } }",
        );
    }

    #[test]
    fn generica_de_orden_superior_oraculo() {
        oracle_program(
            "fn aplicar<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }
             fn doble(n: int) -> int { n * 2 }
             fn main() -> int { aplicar(doble, 21) }",
        );
    }

    #[test]
    fn generica_sobre_arreglos_oraculo() {
        oracle_program(
            "fn par<T>(a: T, b: T) -> [T] { [a, b] }
             fn main() -> int { let xs: [int] = par(10, 32); xs[0] + xs[1] }",
        );
    }

    // ----- M6.2: tipos genéricos del usuario (erasure: ambos motores coinciden) -----

    #[test]
    fn enum_generico_oraculo() {
        oracle_program(
            "enum Caja<T> { Llena(T), Vacia }
             fn val(c: Caja<int>, def: int) -> int { match (c) { Caja.Llena(v) => v, Caja.Vacia => def } }
             fn main() -> int {
                 let a: Caja<int> = Caja.Llena(7);
                 let b: Caja<int> = Caja.Vacia;
                 val(a, 0) + val(b, 35)
             }",
        );
    }

    #[test]
    fn struct_generico_oraculo() {
        oracle_program(
            "struct Par<A, B> { primero: A, segundo: B }
             fn main() -> int {
                 let p: Par<int, bool> = Par { primero: 10, segundo: true };
                 if (p.segundo) { p.primero } else { 0 }
             }",
        );
    }

    // ----- M6.3: Option/Result y el operador ? (oráculo) -----

    #[test]
    fn try_result_oraculo() {
        oracle_program(
            "fn d(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } }
             fn calc(x: int, y: int, z: int) -> Result<int, string> { let q1: int = d(x, y)?; let q2: int = d(q1, z)?; Result.Ok(q1 + q2) }
             fn desemp(r: Result<int, string>) -> int { match (r) { Result.Ok(v) => v, Result.Err(_) => -1 } }
             fn main() -> int { desemp(calc(100, 5, 2)) * 100 + desemp(calc(100, 0, 2)) }",
        );
    }

    #[test]
    fn try_option_oraculo() {
        oracle_program(
            "fn primero(xs: [int]) -> Option<int> { if (len(xs) == 0) { Option.None } else { Option.Some(xs[0]) } }
             fn mas_uno(xs: [int]) -> Option<int> { let v: int = primero(xs)?; Option.Some(v + 1) }
             fn desemp(o: Option<int>) -> int { match (o) { Option.Some(v) => v, Option.None => -99 } }
             fn main() -> int { desemp(mas_uno([41])) * 100 + desemp(mas_uno([])) }",
        );
    }

    #[test]
    fn try_en_modo_estres() {
        // El ? construye/propaga valores de enum (Result) bajo el GC en cada punto
        // seguro: el escrutinio del ? vive en su local temporal y queda rooteado.
        oracle_stress(
            "fn d(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } }
             fn cadena(n: int) -> Result<int, string> { let a: int = d(n, 2)?; let b: int = d(a, 1)?; Result.Ok(a + b) }
             fn desemp(r: Result<int, string>) -> int { match (r) { Result.Ok(v) => v, Result.Err(_) => -1 } }
             fn main() -> int { desemp(cadena(40)) }",
        );
    }

    #[test]
    fn enum_generico_recursivo_en_estres() {
        // Lista genérica construida con un tipo concreto, recorrida con match, bajo el
        // GC en modo estrés: los valores de enum genérico se trazan como cualquier enum.
        oracle_stress(
            "enum Lista<T> { Cons(T, Lista<T>), Nil }
             fn suma(xs: Lista<int>) -> int { match (xs) { Lista.Cons(h, t) => h + suma(t), Lista.Nil => 0 } }
             fn construir(n: int) -> Lista<int> { if (n == 0) { Lista.Nil } else { Lista.Cons(n, construir(n - 1)) } }
             fn main() -> int { suma(construir(15)) }",
        );
    }

    // ----- M4.3: recolección de basura -----

    #[test]
    fn el_gc_no_rompe_programas_en_modo_estres() {
        // Si el GC liberara algo vivo (raíz faltante), estos resultados cambiarían.
        oracle_stress("fn fib(n: int) -> int { if (n < 2) { n } else { fib(n-1) + fib(n-2) } } fn main() -> int { fib(12) }");
        oracle_stress(
            "fn main() -> int {
                 var xs: [int] = [];
                 var i: int = 0;
                 while (i < 30) { push(xs, i * i); i = i + 1; }
                 var s: int = 0; var j: int = 0;
                 while (j < len(xs)) { s = s + xs[j]; j = j + 1; }
                 s
             }",
        );
        oracle_stress(
            "struct P { x: int, y: int }
             fn main() -> int { var p: P = P { x: 1, y: 2 }; p.x = 10; p.x + p.y }",
        );
        oracle_stress(
            "fn contador() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
             fn main() -> int { let c: fn() -> int = contador(); c(); c(); c(); c() }",
        );
    }

    #[test]
    fn el_gc_libera_ciclos() {
        // Cada 'make_cycle' crea un ciclo (celda <-> closure) que queda inalcanzable
        // al retornar. Con conteo de referencias se filtrarían (~200 objetos); el
        // mark-and-sweep los libera, así que el heap queda acotado.
        let src = r#"
            fn make_cycle() {
                var f: fn() = fn() {};
                f = fn() { f(); };
            }
            fn main() -> int {
                var i: int = 0;
                while (i < 100) { make_cycle(); i = i + 1; }
                0
            }
        "#;
        let tokens = crate::lexer::lex(src).unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        let compiled = compile_program(&prog).unwrap();
        let mut vm = Vm::new(&compiled);
        vm.run().expect("vm ok");
        // Sin GC habría ~200 objetos vivos; con mark-and-sweep, muy pocos.
        assert!(vm.heap.live() < 80, "el heap no se acotó: {} objetos vivos", vm.heap.live());
    }

    // ----- M7.1: UFCS (azúcar de front-end; ambos motores ven la llamada ya bajada) -----

    #[test]
    fn ufcs_oraculo() {
        // Función del usuario y builtin (len) usados como métodos.
        oracle_program(r#"
            fn suma(a: int, b: int) -> int { a + b }
            fn main() -> int {
                let xs: [int] = [1, 2, 3, 4];
                let n: int = xs.len();      // len(xs) = 4
                let v: int = 10;
                v.suma(n)                    // suma(10, 4) = 14
            }
        "#);
    }

    #[test]
    fn ufcs_encadenado_oraculo() {
        oracle_program(r#"
            fn doble(x: int) -> int { x * 2 }
            fn inc(x: int) -> int { x + 1 }
            fn main() -> int {
                let v: int = 5;
                v.doble().inc().doble()      // doble(inc(doble(5))) = 22
            }
        "#);
    }

    #[test]
    fn ufcs_sobre_struct_oraculo() {
        // 'norma1' no es campo de Punto -> UFCS; 'p.x' sigue siendo acceso a campo.
        oracle_program(r#"
            struct Punto { x: int, y: int }
            fn norma1(p: Punto) -> int { p.x + p.y }
            fn main() -> int {
                let p: Punto = Punto { x: 7, y: 6 };
                p.norma1() + p.x             // 13 + 7 = 20
            }
        "#);
    }

    #[test]
    fn ufcs_campo_funcion_oraculo() {
        // 'op' ES un campo de tipo función: c.op(x) llama al campo, no es UFCS.
        oracle_program(r#"
            struct Caja { op: fn(int) -> int }
            fn main() -> int {
                let c: Caja = Caja { op: fn(x: int) -> int { x + 100 } };
                c.op(41)                     // (c.op)(41) = 141
            }
        "#);
    }

    #[test]
    fn ufcs_en_modo_estres() {
        // El receptor y los argumentos viven en el heap: el GC en estrés no debe
        // romper la llamada UFCS bajada.
        oracle_stress(r#"
            fn cabeza(xs: [int]) -> int { xs[0] }
            fn cola_suma(xs: [int]) -> int {
                var s: int = 0;
                var i: int = 1;
                while (i < len(xs)) { s = s + xs[i]; i = i + 1; }
                s
            }
            fn main() -> int {
                let xs: [int] = [10, 20, 30, 40];
                xs.cabeza() + xs.cola_suma()   // 10 + 90 = 100
            }
        "#);
    }

    // ----- M7.2: pipelines (azúcar de parser; ambos motores ven la llamada bajada) -----

    #[test]
    fn pipeline_oraculo() {
        oracle_program(r#"
            fn doble(x: int) -> int { x * 2 }
            fn inc(x: int) -> int { x + 1 }
            fn suma(a: int, b: int) -> int { a + b }
            fn main() -> int {
                let v: int = 5;
                let a: int = v |> doble |> inc;   // inc(doble(5)) = 11
                let b: int = v |> suma(100);       // suma(5, 100) = 105
                a + b                               // 116
            }
        "#);
    }

    #[test]
    fn pipeline_y_ufcs_oraculo() {
        // `.f()` (UFCS) y `|> f` (pipeline) componen sobre el mismo valor.
        oracle_program(r#"
            fn doble(x: int) -> int { x * 2 }
            fn inc(x: int) -> int { x + 1 }
            fn main() -> int {
                let v: int = 5;
                v.doble() |> inc |> doble           // doble(inc(doble(5))) = 22
            }
        "#);
    }

    #[test]
    fn pipeline_en_modo_estres() {
        // El valor que fluye por el pipeline es un arreglo en el heap.
        oracle_stress(r#"
            fn suma_todo(xs: [int]) -> int {
                var s: int = 0;
                var i: int = 0;
                while (i < len(xs)) { s = s + xs[i]; i = i + 1; }
                s
            }
            fn con_extra(xs: [int], x: int) -> [int] { push(xs, x); xs }
            fn main() -> int {
                let xs: [int] = [1, 2, 3];
                xs |> con_extra(4) |> suma_todo     // suma_todo(con_extra(xs, 4)) = 10
            }
        "#);
    }

    // ----- M7.3: stdlib (prelude map/filter/fold escrito en raylang) -----

    #[test]
    fn prelude_map_filter_fold_oraculo() {
        oracle_program(r#"
            fn doble(x: int) -> int { x * 2 }
            fn par(x: int) -> bool { x % 2 == 0 }
            fn suma(a: int, b: int) -> int { a + b }
            fn main() -> int {
                let xs: [int] = [1, 2, 3, 4, 5];
                let ys: [int] = xs.map(doble).filter(par);  // [2,4,6,8,10]
                ys.fold(0, suma)                             // 30
            }
        "#);
    }

    #[test]
    fn prelude_pipeline_oraculo() {
        // El mismo cálculo, en estilo pipeline.
        oracle_program(r#"
            fn doble(x: int) -> int { x * 2 }
            fn par(x: int) -> bool { x % 2 == 0 }
            fn suma(a: int, b: int) -> int { a + b }
            fn main() -> int {
                let xs: [int] = [1, 2, 3, 4, 5];
                xs |> filter(par) |> map(doble) |> fold(0, suma)  // [2,4]->[4,8]->12
            }
        "#);
    }

    #[test]
    fn prelude_con_closures_oraculo() {
        // map/fold con funciones anónimas inline.
        oracle_program(r#"
            fn main() -> int {
                let xs: [int] = [1, 2, 3, 4];
                let cuadrados: [int] = xs |> map(fn(x: int) -> int { x * x });  // [1,4,9,16]
                cuadrados.fold(0, fn(a: int, x: int) -> int { a + x })           // 30
            }
        "#);
    }

    #[test]
    fn prelude_en_modo_estres() {
        // map y filter alojan arreglos nuevos en el heap: el GC en estrés debe
        // mantenerlos vivos durante toda la cadena.
        oracle_stress(r#"
            fn inc(x: int) -> int { x + 1 }
            fn pos(x: int) -> bool { x > 3 }
            fn suma(a: int, b: int) -> int { a + b }
            fn main() -> int {
                let xs: [int] = [1, 2, 3, 4, 5, 6];
                xs.map(inc).filter(pos).fold(0, suma)   // [2..7]->[4,5,6,7]->22
            }
        "#);
    }

    // ----- M8.1: inferencia local (solo checker; el runtime no cambia) -----

    #[test]
    fn inferencia_local_oraculo() {
        // Variables inferidas (int, [int], struct, enum genérico) deben dar el mismo
        // resultado en ambos motores: la inferencia se borra antes de ejecutar.
        oracle_program(r#"
            struct Punto { x: int, y: int }
            enum Caja<T> { Llena(T), Vacia }
            fn doble(x: int) -> int { x * 2 }
            fn main() -> int {
                let x = 3;
                let xs = [10, 20, 30];
                let p = Punto { x: 7, y: 6 };
                let c = Caja.Llena(5);
                var total = 0;
                total = total + x.doble();
                let dentro = match (c) { Caja.Llena(v) => v, Caja.Vacia => 0 };
                total + xs[0] + p.x + p.y + dentro   // 6 + 10 + 7 + 6 + 5 = 34
            }
        "#);
    }

    // ----- M9.1: traits (erasure; ambos motores ven funciones y llamadas ordinarias) -----

    #[test]
    fn traits_despacho_estatico_oraculo() {
        // Un trait implementado para un struct, un enum y un primitivo: los métodos se
        // bajan a funciones mangladas y las llamadas por punto a llamadas ordinarias,
        // así que la VM y el intérprete deben coincidir sin tocar el runtime.
        oracle_program(r#"
            trait Valor { fn valor(self) -> int; }
            struct Punto { x: int, y: int }
            enum Moneda { Cara, Cruz }
            impl Valor for Punto { fn valor(self) -> int { self.x + self.y } }
            impl Valor for Moneda {
                fn valor(self) -> int { match (self) { Moneda.Cara => 1, Moneda.Cruz => 0 } }
            }
            impl Valor for int { fn valor(self) -> int { self } }
            fn main() -> int {
                let p = Punto { x: 3, y: 4 };
                p.valor() + Moneda.Cara.valor() + 10.valor()   // 7 + 1 + 10 = 18
            }
        "#);
    }

    #[test]
    fn traits_self_y_metodos_internos_oraculo() {
        // `Self` en el retorno, parámetros extra, y un método que llama a otro del mismo
        // impl (`self.sumar(self)`): bajo estrés del GC para validar las raíces.
        oracle_stress(r#"
            trait Punteable {
                fn sumar(self, otro: Punto) -> Punto;
                fn doble(self) -> Self;
                fn norma(self) -> int;
            }
            struct Punto { x: int, y: int }
            impl Punteable for Punto {
                fn sumar(self, otro: Punto) -> Punto { Punto { x: self.x + otro.x, y: self.y + otro.y } }
                fn doble(self) -> Self { self.sumar(self) }
                fn norma(self) -> int { self.x * self.x + self.y * self.y }
            }
            fn main() -> int {
                let p = Punto { x: 3, y: 4 };
                p.doble().norma()   // (6,8) -> 36 + 64 = 100
            }
        "#);
    }

    // ----- M9.2: bounds vía paso de diccionarios -----

    #[test]
    fn bounds_diccionarios_oraculo() {
        // Genérico acotado sobre struct y primitivo + reenvío entre genéricos. Los
        // diccionarios son valores función; ambos motores deben coincidir.
        oracle_program(r#"
            trait Valor { fn valor(self) -> int; }
            struct Punto { x: int, y: int }
            impl Valor for Punto { fn valor(self) -> int { self.x + self.y } }
            impl Valor for int { fn valor(self) -> int { self } }
            fn doble_valor<T: Valor>(x: T) -> int { x.valor() + x.valor() }
            fn suma_tres<T: Valor>(a: T, b: T, c: T) -> int {
                doble_valor(a) + b.valor() + c.valor()   // reenvío del diccionario
            }
            fn main() -> int {
                let p = Punto { x: 3, y: 4 };
                doble_valor(p) + doble_valor(10) + suma_tres(p, p, p)   // 14 + 20 + 28 = 62
            }
        "#);
    }

    #[test]
    fn bounds_multiples_oraculo() {
        // T: A + B — dos diccionarios. Bajo estrés del GC.
        oracle_stress(r#"
            trait Nombre { fn largo(self) -> int; }
            trait Doble { fn doble(self) -> int; }
            struct Cosa { n: int }
            impl Nombre for Cosa { fn largo(self) -> int { self.n } }
            impl Doble for Cosa { fn doble(self) -> int { self.n + self.n } }
            fn usar<T: Nombre + Doble>(x: T) -> int { x.largo() + x.doble() }
            fn main() -> int {
                let c = Cosa { n: 5 };
                usar(c)   // 5 + 10 = 15
            }
        "#);
    }

    // ----- M9.2b: impls genéricos -----

    #[test]
    fn impl_generico_sin_bounds_oraculo() {
        // `impl<T> Trait for Caja<T>` cuyo método no usa T: el método manglado es genérico
        // pero sin diccionarios. Despacha igual para Caja<int> y Caja<string>.
        oracle_program(r#"
            struct Caja<T> { contenido: T }
            trait Contar { fn contar(self) -> int; }
            impl<T> Contar for Caja<T> { fn contar(self) -> int { 1 } }
            fn main() -> int {
                let c = Caja { contenido: 42 };
                let s = Caja { contenido: "hola" };
                c.contar() + s.contar()   // 1 + 1 = 2
            }
        "#);
    }

    #[test]
    fn impl_generico_acotado_llamada_directa_oraculo() {
        // `impl<T: Mostrable> Mostrable for Caja<T>`: el cuerpo usa T.mostrar() (vía el
        // diccionario interno). Llamada directa sobre Caja<int> → el dict interno es el de
        // int (plano). Es M9.2b-1: el caso anidado (pasar Caja a otro genérico) es -2.
        oracle_stress(r#"
            struct Caja<T> { contenido: T }
            trait Medir { fn medir(self) -> int; }
            impl Medir for int { fn medir(self) -> int { self } }
            impl<T: Medir> Medir for Caja<T> { fn medir(self) -> int { self.contenido.medir() + 1 } }
            fn main() -> int {
                let c = Caja { contenido: 41 };
                c.medir()   // 41 + 1 = 42
            }
        "#);
    }

    #[test]
    fn impl_generico_diccionario_anidado_oraculo() {
        // M9.2b-2: pasar un Caja<int> a otro genérico acotado. El diccionario de Caja<int> es
        // un **closure** que captura el de int. Ambos motores deben coincidir.
        oracle_program(r#"
            struct Caja<T> { contenido: T }
            trait Medir { fn medir(self) -> int; }
            impl Medir for int { fn medir(self) -> int { self } }
            impl<T: Medir> Medir for Caja<T> { fn medir(self) -> int { self.contenido.medir() + 1 } }
            fn medir_dos<X: Medir>(a: X, b: X) -> int { a.medir() + b.medir() }
            fn main() -> int {
                let c = Caja { contenido: 10 };
                medir_dos(c, c)   // (10+1) * 2 = 22
            }
        "#);
    }

    #[test]
    fn impl_generico_anidado_profundo_estres() {
        // Caja<Caja<int>>: un diccionario anidado que contiene otro. Bajo estrés del GC,
        // porque los closures-diccionario son objetos del heap (sus raíces deben trazarse).
        oracle_stress(r#"
            struct Caja<T> { contenido: T }
            trait Medir { fn medir(self) -> int; }
            impl Medir for int { fn medir(self) -> int { self } }
            impl<T: Medir> Medir for Caja<T> { fn medir(self) -> int { self.contenido.medir() + 1 } }
            fn medir_uno<X: Medir>(x: X) -> int { x.medir() }
            fn main() -> int {
                let c2 = Caja { contenido: Caja { contenido: 100 } };
                c2.medir() + medir_uno(c2)   // 102 + 102 = 204
            }
        "#);
    }

    // ----- M11.1: stdlib de string -----

    #[test]
    fn string_concat_len_oraculo() {
        // Concatenación con `+`, len de string y to_string; el resultado es un int.
        oracle_program(r#"
            fn main() -> int {
                let s = "hola, " + "mundo";       // concat
                let etiqueta = "n=" + to_string(len(s));
                print(etiqueta);                   // n=11
                len(s) + len("123")               // 11 + 3 = 14
            }
        "#);
    }

    #[test]
    fn string_to_string_de_varios_tipos_oraculo() {
        oracle_program(r#"
            fn main() -> int {
                print(to_string(42));      // 42
                print(to_string(true));    // true
                print(to_string("ya"));    // ya (identidad)
                len(to_string(true)) + len(to_string(false))   // 4 + 5 = 9
            }
        "#);
    }

    #[test]
    fn string_ufcs_oraculo() {
        // UFCS sobre los builtins de string (s.len(), n.to_string()).
        oracle_program(r#"
            fn main() -> int {
                let s = "raylang";
                print(s.len().to_string());   // 7
                s.len()
            }
        "#);
    }

    #[test]
    fn string_trim_split_oraculo() {
        oracle_program(r#"
            fn main() -> int {
                let limpio = trim("  hola  ");
                print("[" + limpio + "]");        // [hola]
                let campos = split("a,bb,ccc", ",");
                print(campos[1]);                  // bb
                len(campos) + len(limpio)          // 3 + 4 = 7
            }
        "#);
    }

    #[test]
    fn char_tipo_oraculo() {
        // M11.4c-1: literal de char, anotación, ==, to_string, y @derive(Eq, Show) con campo char.
        oracle_program(r#"
            @derive(Eq, Show)
            struct Tecla { c: char, repetida: bool }
            fn clase(c: char) -> int {
                if (c == 'a') { 1 } else { if (c == '\n') { 2 } else { 0 } }
            }
            fn main() -> int {
                let c: char = 'z';
                print(c);                              // z
                print(to_string('x') + "!");           // x!
                print('a' == 'a');                     // true
                let t = Tecla { c: 'q', repetida: false };
                print(t.mostrar());                    // Tecla { c: q, repetida: false }
                print(t.igual(Tecla { c: 'q', repetida: false }));  // true
                clase('a') + clase('\n') + clase('z')  // 1 + 2 + 0 = 3
            }
        "#);
    }

    #[test]
    fn char_indexar_y_chars_oraculo() {
        // M11.4c-2: s[i] -> char, chars(s) -> [char] (asigna heap → estrés del GC).
        oracle_stress(r#"
            fn cuenta(s: string, c: char) -> int {
                var n = 0;
                var i = 0;
                while (i < len(s)) {
                    if (s[i] == c) { n = n + 1; }
                    i = i + 1;
                }
                n
            }
            fn main() -> int {
                let s = "racecar";
                print(s[0]);                       // r
                print(s[3]);                       // e
                let cs = chars(s);
                print(cs[1]);                      // a
                print(len(cs));                    // 7
                cuenta(s, 'r') + cuenta(s, 'c') + len(chars("hola"))  // 2 + 2 + 4 = 8
            }
        "#);
    }

    #[test]
    fn string_contains_replace_oraculo() {
        // contains -> bool; replace asigna un string nuevo (heap en la VM). Oráculo + estrés del GC.
        oracle_stress(r#"
            fn main() -> int {
                let s = "hola mundo, hola raylang";
                print(s.contains("mundo"));            // true
                print(s.contains("python"));           // false
                let r = s.replace("hola", "HOLA");
                print(r);                              // HOLA mundo, HOLA raylang
                print("a.b.c".replace(".", "/"));      // a/b/c
                if (s.contains("raylang")) { len(r) } else { 0 }  // 24
            }
        "#);
    }

    #[test]
    fn string_stdlib_m117_oraculo() {
        // M11.7a: starts_with/ends_with (bool); to_upper/to_lower/substring/repeat/join asignan
        // string nuevo (heap en la VM); index_of construye Option en el prelude. Oráculo + estrés GC.
        oracle_stress(r#"
            fn pos(o: Option<int>, def: int) -> int {
                match (o) { Option.Some(i) => i, Option.None => def, }
            }
            fn main() -> int {
                let s = "Hola, Mundo";
                print(s.starts_with("Hola"));      // true
                print(s.ends_with("xyz"));         // false
                print(s.to_upper());               // HOLA, MUNDO
                print(s.to_lower());               // hola, mundo
                print(s.substring(0, 4));          // Hola
                print(s.substring(6, 100));        // Mundo (clamp)
                print("ab".repeat(3));             // ababab
                print("".repeat(5));               // (vacío)
                let partes = ["a", "b", "c"];
                print(join(partes, "-"));          // a-b-c
                print(pos(index_of(s, "Mundo"), 0 - 1));   // 6
                print(pos(index_of(s, "zzz"), 0 - 1));      // -1
                len(s.substring(6, 11)) + pos(index_of(s, "Mundo"), 0)  // 5 + 6 = 11
            }
        "#);
    }

    #[test]
    fn array_stdlib_m117b_oraculo() {
        // M11.7b: concat (a+b), reverse, pop (muta + Option), contains, position. reverse/pop/concat
        // asignan en el heap → estrés del GC; pop construye Option en el prelude.
        oracle_stress(r#"
            fn idx(o: Option<int>, def: int) -> int {
                match (o) { Option.Some(i) => i, Option.None => def, }
            }
            fn ult(o: Option<int>, def: int) -> int {
                match (o) { Option.Some(x) => x, Option.None => def, }
            }
            fn main() -> int {
                let a = [1, 2, 3];
                let b = [4, 5];
                let c = a + b;                      // [1,2,3,4,5]
                print(len(c));                      // 5
                let r = reverse(c);                 // [5,4,3,2,1]
                print(r[0]);                        // 5
                print(c.contains(4));               // true
                print(c.contains(99));              // false
                print(idx(position(c, 3), 0 - 1));  // 2
                print(idx(position(c, 99), 0 - 1)); // -1
                let v = [10, 20, 30];
                let x = ult(pop(v), 0);             // 30, y v queda [10,20]
                print(len(v));                      // 2
                x + len(c) + r[1]                   // 30 + 5 + 4 = 39
            }
        "#);
    }

    #[test]
    fn sort_ord_oraculo() {
        // M11.7d: sort<T: Ord> (bound → diccionarios M9.2) sobre primitivos y un tipo de usuario
        // que implementa Ord. Asigna arreglos en el heap → estrés del GC.
        oracle_stress(r#"
            struct Caja { peso: int }
            impl Ord for Caja {
                fn menor(self, otro: Caja) -> bool { self.peso < otro.peso }
            }
            fn main() -> int {
                let xs = sort([3, 1, 4, 1, 5, 9, 2, 6]);
                print(xs[0]); print(xs[7]);             // 1 ... 9
                let cs = sort(['c', 'a', 'b']);
                print(cs[0]);                            // a
                let cajas = sort([Caja { peso: 30 }, Caja { peso: 10 }, Caja { peso: 20 }]);
                print(cajas[0].peso);                    // 10
                print(cajas[2].peso);                    // 30
                xs[0] + xs[7] + cajas[0].peso            // 1 + 9 + 10 = 20
            }
        "#);
    }

    #[test]
    fn string_split_estres_gc() {
        // split asigna un arreglo (objeto del heap). Bajo estrés del GC: si una raíz faltara,
        // el arreglo recién creado se liberaría y el resultado cambiaría.
        oracle_stress(r#"
            fn main() -> int {
                let partes = "uno:dos:tres:cuatro".trim().split(":");
                let total = len(partes) + len(partes[0]) + len(partes[3]);
                print(partes[2]);                  // tres
                total                              // 4 + 3 + 6 = 13
            }
        "#);
    }

    #[test]
    fn parse_int_oraculo() {
        // parse_int es determinista (no toca stdin) → oráculo VM↔intérprete. Construye Option
        // en el prelude (raylang); el resultado debe coincidir en ambos motores.
        oracle_program(r#"
            fn valor(o: Option<int>, def: int) -> int {
                match (o) {
                    Option.Some(n) => n,
                    Option.None => def,
                }
            }
            fn main() -> int {
                let a = valor(parse_int("42"), 0);        // 42
                let b = valor(parse_int("  -7 "), 0);     // -7 (trim)
                let c = valor(parse_int("xyz"), 100);     // 100 (None)
                a + b + c                                 // 135
            }
        "#);
    }

    #[test]
    fn parse_float_oraculo() {
        // M14: parse_float, como parse_int, es determinista → oráculo. El formateo de float es
        // el mismo f64 de Rust en ambos motores, así que los valores coinciden.
        oracle_program(r#"
            fn main() -> int {
                let ok = match (parse_float("3.14")) { Option.Some(f) => f, Option.None => 0.0 };
                let no = match (parse_float("hola")) { Option.Some(_) => 1, Option.None => 0 };
                let ent = match (parse_float("42")) { Option.Some(f) => f, Option.None => 0.0 };
                // 3.14*100 = 314, 42.0 → 42; no=0. Resultado 314 + 42 + 0 = 356.
                let a: int = if (ok * 100.0 == 314.0) { 314 } else { -1 };
                let b: int = if (ent == 42.0) { 42 } else { -1 };
                a + b + no
            }
        "#);
    }

    #[test]
    fn args_y_env_oraculo() {
        // En el proceso de test no se fijan args (→ []) y la variable no existe (→ None): ambos
        // motores deben coincidir. (El comportamiento "real" se prueba por subproceso en io_cli.)
        oracle_program(r#"
            fn main() -> int {
                let n = len(args());                       // 0
                let e = match (env("RAYLANG_NO_EXISTE_XYZ")) {
                    Option.Some(_) => 1,
                    Option.None => 0,
                };
                n + e                                      // 0
            }
        "#);
    }

    #[test]
    fn read_file_inexistente_es_err_oraculo() {
        // Leer un archivo inexistente es determinista (misma llamada a std::fs en ambos motores) →
        // oráculo. Construye Result en el prelude vía el arreglo etiquetado; debe coincidir.
        oracle_program(r#"
            fn main() -> int {
                match (read_file("/raylang_no_existe_xyz_123.txt")) {
                    Result.Ok(_) => 0,
                    Result.Err(_) => 1,
                }
            }
        "#);
    }

    #[test]
    fn parse_int_option_construido_en_el_heap_estres_gc() {
        // El [int] del primitivo y el Option que arma el prelude son objetos del heap. Bajo
        // estrés del GC: si una raíz faltara, el valor vivo se liberaría.
        oracle_stress(r#"
            fn main() -> int {
                let xs = ["1", "2", "no", "4"];
                var suma = 0;
                var i = 0;
                while (i < len(xs)) {
                    match (parse_int(xs[i])) {
                        Option.Some(n) => { suma = suma + n; },
                        Option.None => {},
                    }
                    i = i + 1;
                }
                suma                               // 1 + 2 + 4 = 7
            }
        "#);
    }

    // ----- M9.3a: métodos por defecto -----

    #[test]
    fn metodos_por_defecto_oraculo() {
        // Defecto heredado, defecto que llama a otro método, y redefinición. El método
        // sintetizado es una función ordinaria: ambos motores deben coincidir.
        oracle_program(r#"
            trait Valor {
                fn base(self) -> int;
                fn doble(self) -> int { self.base() + self.base() }   // defecto usa otro
                fn diez(self) -> int { 10 }                            // defecto constante
            }
            struct A { n: int }
            impl Valor for A { fn base(self) -> int { self.n } }       // hereda doble y diez
            struct B { n: int }
            impl Valor for B {
                fn base(self) -> int { self.n }
                fn doble(self) -> int { self.n * 100 }                 // redefine doble
            }
            fn main() -> int {
                let a = A { n: 3 };
                let b = B { n: 4 };
                a.doble() + a.diez() + b.doble() + b.diez()   // 6 + 10 + 400 + 10 = 426
            }
        "#);
    }

    #[test]
    fn metodos_por_defecto_via_bound_oraculo() {
        // Un método por defecto invocado desde un genérico acotado (M9.2 + M9.3a).
        oracle_stress(r#"
            trait Saludo {
                fn nombre(self) -> int;
                fn doble_nombre(self) -> int { self.nombre() + self.nombre() }
            }
            struct P { v: int }
            impl Saludo for P { fn nombre(self) -> int { self.v } }
            fn usar<T: Saludo>(x: T) -> int { x.doble_nombre() }
            fn main() -> int { let p = P { v: 21 }; usar(p) }   // 42
        "#);
    }

    // ----- M9.3b: trait objects (despacho dinámico) -----

    #[test]
    fn trait_objects_despacho_dinamico_oraculo() {
        // Arreglo heterogéneo de trait objects + despacho por valor. El trait object se
        // realiza como un struct sintetizado (la vtable); ambos motores deben coincidir.
        oracle_program(r#"
            trait Figura { fn area(self) -> int; }
            struct Cuadrado { lado: int }
            impl Figura for Cuadrado { fn area(self) -> int { self.lado * self.lado } }
            struct Rect { ancho: int, alto: int }
            impl Figura for Rect { fn area(self) -> int { self.ancho * self.alto } }
            fn total(xs: [dyn Figura]) -> int {
                var s = 0; var i = 0;
                while (i < len(xs)) { s = s + xs[i].area(); i = i + 1; }
                s
            }
            fn main() -> int {
                let figuras: [dyn Figura] = [Cuadrado{lado:3}, Rect{ancho:4,alto:5}, Cuadrado{lado:2}];
                total(figuras)   // 9 + 20 + 4 = 33
            }
        "#);
    }

    #[test]
    fn dyn_multi_trait_oraculo() {
        // M9.5a: `dyn A + B` — un objeto que satisface dos traits; despacho a métodos de ambos.
        // El orden del conjunto es canónico (dyn Nombre + Area == dyn Area + Nombre).
        oracle_program(r#"
            trait Area { fn area(self) -> int; }
            trait Nombre { fn nombre(self) -> string; }
            struct Cuadrado { lado: int }
            impl Area for Cuadrado { fn area(self) -> int { self.lado * self.lado } }
            impl Nombre for Cuadrado { fn nombre(self) -> string { "cuad" } }
            struct Circ { r: int }
            impl Area for Circ { fn area(self) -> int { 3 * self.r * self.r } }
            impl Nombre for Circ { fn nombre(self) -> string { "circ" } }
            fn describe(x: dyn Nombre + Area) -> int { len(x.nombre()) + x.area() }
            fn main() -> int {
                let xs: [dyn Area + Nombre] = [Cuadrado{lado:4}, Circ{r:2}];
                var s = 0; var i = 0;
                while (i < len(xs)) { s = s + describe(xs[i]); i = i + 1; }
                // (4 + 16) + (4 + 12) = 20 + 16 = 36
                s
            }
        "#);
    }

    #[test]
    fn dyn_upcasting_oraculo() {
        // M9.5b: upcasting `dyn A + B` -> `dyn A` (olvidar traits, S2 ⊆ S1). Se reconstruye el
        // struct menor proyectando los campos del mayor.
        oracle_program(r#"
            trait Area { fn area(self) -> int; }
            trait Nombre { fn nombre(self) -> string; }
            struct Cuadrado { lado: int }
            impl Area for Cuadrado { fn area(self) -> int { self.lado * self.lado } }
            impl Nombre for Cuadrado { fn nombre(self) -> string { "cuad" } }
            fn solo_area(a: dyn Area) -> int { a.area() }
            fn main() -> int {
                let ab: dyn Area + Nombre = Cuadrado { lado: 5 };
                let v1 = solo_area(ab);        // upcast en el argumento: 25
                let a: dyn Area = ab;          // upcast en el let
                v1 + a.area()                  // 25 + 25 = 50
            }
        "#);
    }

    #[test]
    fn dyn_sobre_impl_generico_oraculo() {
        // M9.4: coercionar a `dyn Trait` un tipo cuyo impl es genérico acotado (Caja<T>): la vtable
        // lleva un closure anidado (como un diccionario), no el método manglado plano. Incluye
        // anidamiento Caja<Caja<N>> y un impl concreto en el mismo arreglo heterogéneo.
        oracle_program(r#"
            trait Mostrar { fn mostrar(self) -> string; }
            struct N { x: int }
            impl Mostrar for N { fn mostrar(self) -> string { "N" } }
            struct Caja<T> { v: T }
            impl<T: Mostrar> Mostrar for Caja<T> {
                fn mostrar(self) -> string { "Caja(" + self.v.mostrar() + ")" }
            }
            fn describe(d: dyn Mostrar) -> string { d.mostrar() }
            fn main() -> int {
                let xs: [dyn Mostrar] = [N{x:1}, Caja{v:N{x:2}}, Caja{v:Caja{v:N{x:3}}}];
                var total = 0; var i = 0;
                while (i < len(xs)) { total = total + len(describe(xs[i])); i = i + 1; }
                // len("N")=1, len("Caja(N)")=7, len("Caja(Caja(N))")=13 -> 21
                total
            }
        "#);
    }

    #[test]
    fn defecto_con_self_heredado_por_dos_impls() {
        // Regresión: un método por defecto que llama a `self.m()` y es heredado por DOS
        // impls. Cada cuerpo clonado debe resolver a SUS métodos (no compartir destino).
        oracle_program(r#"
            trait Animal {
                fn sonido(self) -> int;
                fn doble_sonido(self) -> int { self.sonido() + self.sonido() }   // defecto
            }
            struct Perro { v: int }
            impl Animal for Perro { fn sonido(self) -> int { self.v } }            // hereda
            struct Gato { v: int }
            impl Animal for Gato { fn sonido(self) -> int { self.v * 10 } }        // hereda
            fn main() -> int {
                let p = Perro { v: 3 };
                let g = Gato { v: 4 };
                p.doble_sonido() + g.doble_sonido()   // (3+3) + (40+40) = 6 + 80 = 86
            }
        "#);
    }

    #[test]
    fn trait_objects_estres_gc() {
        // El struct sintetizado (vtable) y el dato viven en el heap de la VM: el GC debe
        // trazar ambos. Bajo estrés (recolecta en cada punto seguro), un fallo de raíz
        // cambiaría el resultado o reventaría.
        oracle_stress(r#"
            trait Valor { fn valor(self) -> int; fn doble(self) -> int { self.valor() + self.valor() } }
            struct A { n: int }
            impl Valor for A { fn valor(self) -> int { self.n } }
            struct B { n: int }
            impl Valor for B { fn valor(self) -> int { self.n + 1 } fn doble(self) -> int { self.n } }
            fn usar(x: dyn Valor) -> int { x.valor() + x.doble() }
            fn main() -> int {
                let a: dyn Valor = A { n: 10 };
                let b: dyn Valor = B { n: 20 };
                usar(a) + usar(b)   // (10+20) + (21+20) = 30 + 41 = 71
            }
        "#);
    }

    // ----- M10.1: @derive(Eq) -----

    #[test]
    fn derive_eq_oraculo() {
        // El impl generado por @derive(Eq) baja a una función ordinaria (M9): ambos motores
        // deben coincidir, para struct, enum unit y enum con payload.
        oracle_program(r#"
            @derive(Eq)
            struct Punto { x: int, y: int }
            @derive(Eq)
            enum Color { Rojo, Verde, Azul }
            @derive(Eq)
            enum Forma { Circulo(int), Rect(int, int) }
            fn b2i(b: bool) -> int { if (b) { 1 } else { 0 } }
            fn main() -> int {
                let p = Punto { x: 1, y: 2 };
                let q = Punto { x: 1, y: 2 };
                let r = Punto { x: 9, y: 2 };
                let e1 = b2i(p.igual(q)) + b2i(p.igual(r));               // 1 + 0
                let e2 = b2i(Color.Verde.igual(Color.Verde)) + b2i(Color.Rojo.igual(Color.Azul)); // 1 + 0
                let f = Forma.Rect(3, 4);
                let e3 = b2i(f.igual(Forma.Rect(3, 4))) + b2i(f.igual(Forma.Circulo(3)));         // 1 + 0
                e1 + e2 + e3   // 3
            }
        "#);
    }
}
