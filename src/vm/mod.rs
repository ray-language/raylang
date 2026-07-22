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

use crate::bytecode::{CastTarget, Chunk, CmpOp, CompiledEnum, CompiledFn, CompiledProgram, OpCode, UpvalueSource};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::gc::{Handle, Heap, HeapValue, Obj, TaskState, VmChannel, VmClosure, VmEnum, VmStruct, VmTask};
use crate::runtime::{EnumInstance, MapKey, RuntimeError, StructInstance, Value};

mod values;
mod transfer;
mod sched;
use values::*;
use transfer::*;
use sched::*;

/// Límite de marcos para detectar recursión infinita en vez de colgarse. Es el
/// **mismo** que el del intérprete (`runtime::MAX_CALL_DEPTH`, M13.3a) para que
/// ambos motores coincidan en la frontera: un programa que recurre justo al límite
/// da el mismo veredicto en los dos.
const MAX_FRAMES: usize = crate::runtime::MAX_CALL_DEPTH;

/// M38.3b paso 3: cuando un worker no encuentra fibra lista pero otro sigue ejecutando (`running > 0`),
/// espera con un *busy-poll* de baja frecuencia (en vez de una `Condvar`, que exigiría pasar el guard del
/// `Mutex` por valor y es más propensa a *lost-wakeups*). Este es el intervalo entre reintentos: corto para
/// baja latencia al despertar, pero no cero para no quemar un núcleo. Sólo se alcanza cuando un worker está
/// ocioso con trabajo pendiente en otro hilo (raro en cargas bien paralelizadas). Con N=1 nunca se usa.
const SPIN_SLEEP_US: u64 = 50;

/// M38.4: bandera global de **modo determinista** (`--deterministic`). La fija la CLI (proceso-local, como
/// `set_program_args`) antes de ejecutar; fuerza el scheduler **M:1 reproducible** (un hilo, orden FIFO) para
/// tests y para el oráculo, aunque el default sea multicore. `Relaxed` basta: se escribe una vez al arranque,
/// antes de lanzar hilos worker.
static FORCE_DETERMINISTIC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// M38.4: activa/desactiva el modo determinista (`--deterministic`). La CLI la llama al parsear el flag.
pub fn set_deterministic(v: bool) {
    FORCE_DETERMINISTIC.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// M38.4: nº de hilos worker del scheduler M:N. **El multicore es el default** (§46.4); lo determinista es
/// opt-in. Reglas, en orden:
/// 1. `--deterministic` (o el default de los tests) → **1** (M:1 reproducible; el oráculo/M12 lo exige).
/// 2. `RAYLANG_THREADS=N` explícito → **N** (override; `=1` = forzar determinista).
/// 3. El programa **no usa `spawn`** → **1**: sólo existe la fibra `main`, el multicore no aporta nada y
///    evitamos el coste de lanzar hilos (la inmensa mayoría de programas, incluidos todos los del oráculo).
/// 4. Concurrente y sin override → **`available_parallelism()`** (multicore por defecto).
///
/// El resultado se clampa al rango 1..=256.
fn num_workers(program: &CompiledProgram) -> usize {
    // M44a: en `wasm32` (el playground web) no hay hilos del SO → siempre 1 (nunca se invoca `thread::
    // scope`). `available_parallelism()` ya daría `Err`→1 allí, pero lo forzamos por robustez.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = program;
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if FORCE_DETERMINISTIC.load(std::sync::atomic::Ordering::Relaxed) {
            return 1;
        }
        if let Ok(s) = std::env::var("RAYLANG_THREADS") {
            return s.trim().parse::<usize>().unwrap_or(1).clamp(1, 256);
        }
        if !program_uses_spawn(program) {
            return 1;
        }
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 256)
    }
}

/// M38.4: ¿el programa contiene el opcode `Spawn`? Si no, sólo hay la fibra `main` → el scheduler M:N no
/// aporta nada y `num_workers` devuelve 1 (sin lanzar hilos). Un escaneo único y barato del bytecode.
/// M44a: solo lo usa la rama no-wasm de `num_workers` (en wasm siempre es 1).
#[cfg(not(target_arch = "wasm32"))]
fn program_uses_spawn(program: &CompiledProgram) -> bool {
    program.functions.iter().any(|f| f.chunk.code.iter().any(|op| matches!(op, OpCode::Spawn | OpCode::SpawnDiscard)))
}

/// M38.3b paso 3: una referencia al programa compilado **compartible entre hilos worker**. `CompiledProgram`
/// contiene `Value` (las constantes del chunk) que usa `Rc` → es `!Send`/`!Sync` **por tipo**. Pero durante
/// la ejecución el programa es **inmutable** y el único acceso a sus constantes (`const_to_heap`) sólo LEE:
/// copia los escalares y clona el `Vec` interno de `Bytes` **por deref** (nunca clona ni dropea un `Rc`, así
/// que ningún refcount se toca) → compartir esta referencia inmutable entre hilos es sano. El wrapper lo
/// afirma con `unsafe impl`; sólo cruza el borde del `thread::spawn` (dentro del worker se usa como `&`).
#[derive(Clone, Copy)]
struct ProgRef<'a>(&'a CompiledProgram);
// SAFETY: el programa es inmutable durante la ejecución y sus constantes sólo se leen (sin tocar refcounts
// de `Rc`), luego el `!Sync` de `Value` no puede provocar carreras. Ver la doc de `ProgRef`.
unsafe impl Send for ProgRef<'_> {}
unsafe impl Sync for ProgRef<'_> {}

/// Ejecuta un programa compilado (empezando por `main`) y devuelve su resultado.
pub fn run_program(program: &CompiledProgram) -> Result<Value, RuntimeError> {
    let mut vm = Vm::new(program);
    let result = vm.run()?;
    vm.print_gc_stats_if_requested(); // M37.1
    Ok(to_value(&vm.cur.heap, &program.enums, &result))
}

/// Como [`run_program`], pero con **límites de recursos** para embeber raylang confinado (un bucle
/// infinito o una entrada maliciosa no cuelgan ni agotan la memoria del anfitrión):
/// - `fuel` (M42.1): presupuesto de **instrucciones**; al superarlo, aborta en vez de correr sin fin.
/// - `heap_cap` (M42.2): tope de **objetos vivos**; al rebasarlo (tras un GC), aborta.
///
/// `None` en ambos = sin límite (idéntico a `run_program`).
pub fn run_program_with_limit(
    program: &CompiledProgram,
    fuel: Option<u64>,
    heap_cap: Option<usize>,
) -> Result<Value, RuntimeError> {
    let mut vm = Vm::new(program);
    if let Some(f) = fuel {
        vm.fuel = f;
    }
    if let Some(n) = heap_cap {
        vm.cur.heap.set_max_live(n);
    }
    let result = vm.run()?;
    vm.print_gc_stats_if_requested(); // M37.1
    Ok(to_value(&vm.cur.heap, &program.enums, &result))
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
        externs: Vec::new(),
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
    /// Profundidad de la pila de operandos de la fibra al entrar (tras sacar los argumentos).
    /// `Return` trunca a esta base: un `Return` emitido por `?` en mitad de una expresión deja
    /// operandos pendientes por encima, y sin truncar corrompen la pila del llamador (M64.1).
    stack_base: usize,
}

struct Vm<'a> {
    program: &'a CompiledProgram,
    /// M38.3b: la **fibra en curso** (su ejecución: marcos, pila, heap, scopes, task, is_main). Es
    /// thread-local (en M:N cada worker tiene la suya); al conmutar se salva en `Shared` y se carga la
    /// siguiente. Antes eran campos sueltos del `Vm`; unificarlos en un `Fiber` hace la conmutación un
    /// simple swap y prepara el split exec/shared que exige el lock de M38.3.
    cur: Fiber,
    /// M38.3a: el **estado compartido del scheduler** (colas de fibras + almacenes de canales/tareas),
    /// agrupado para el paso a M:N (M38.3b lo envolverá en `Arc<Mutex<Shared>>` — con los heaps ya aislados
    /// en M38.1, es lo ÚNICO que N hilos compartirían; el GC no, cada heap es thread-local). Aquí sigue
    /// siendo propiedad directa del `Vm` (single-thread, sin lock) → comportamiento idéntico.
    /// M38.3b paso 2: el estado compartido del scheduler tras un `Mutex` (envuelto en `Arc` para que en el
    /// paso 3 lo compartan N hilos). Aquí (single-thread) el lock nunca tiene contención. Cada operación del
    /// scheduler bloquea UNA vez (`self.sched()`) y pasa el guard a la cadena de helpers (que ya toman
    /// `&mut Shared`), evitando la reentrancia (el `Mutex` no es reentrante).
    shared: Arc<Mutex<Shared>>,
    /// Opt.2: pool de `Vec<Local>` reutilizables. Cada llamada necesita un arreglo de locales; en vez de
    /// asignar/liberar uno por llamada (millones en recursión), reciclamos los de los marcos que retornan.
    /// NO es raíz del GC (sus contenidos son basura entre reciclar y reusar; `new_locals` los reconstruye).
    locals_pool: Vec<Vec<Local>>,
    /// M42.1: **fuel** — presupuesto de instrucciones restante (límite de recursos para embeber raylang
    /// como lenguaje de scripts confinado). Decrece una por instrucción; al llegar a 0 se aborta con un
    /// error limpio. `u64::MAX` = **sin límite** (el default): nunca se agota en la práctica, así que el
    /// coste es un decremento + comparación por instrucción, sin ramas.
    fuel: u64,
    /// M37.1: **instrumentación de pausas del GC**. Cuenta de recolecciones y la pausa máxima (ns) de una
    /// sola recolección stop-the-world. Sirve para MEDIR el objetivo de M37 (pausas acotadas, <1 ms) antes
    /// de decidir si el barrido/marcado incremental compensan. Coste: un `Instant` por recolección (raro).
    /// Se imprimen a stderr al terminar si `RAYLANG_GC_STATS` está en el entorno.
    gc_count: u64,
    gc_max_pause_ns: u128,
    gc_total_pause_ns: u128,
    /// M38.3b paso 3: señal **local al worker** de que debe detenerse porque el programa ya terminó (otro
    /// worker fijó `Shared.outcome`). El lazo la comprueba en su cima y retorna. Con N=1 nunca se activa.
    stop: bool,
}

impl<'a> Vm<'a> {
    fn new(program: &'a CompiledProgram) -> Self {
        Vm {
            program,
            cur: Fiber {
                frames: Vec::new(),
                stack: Vec::new(),
                heap: Heap::new(),
                is_main: true,
                task: None,
                scopes: Vec::new(),
            },
            shared: Arc::new(Mutex::new(Shared::default())),
            locals_pool: Vec::new(),
            fuel: u64::MAX, // sin límite por defecto
            gc_count: 0,
            gc_max_pause_ns: 0,
            gc_total_pause_ns: 0,
            stop: false,
        }
    }

    /// M38.3b paso 3: un **contexto de ejecución worker** para el scheduler M:N. Comparte `program`
    /// (inmutable) y `shared` (`Arc<Mutex<Shared>>`, el estado del scheduler) con los demás workers, pero su
    /// estado de ejecución (`cur`/`locals_pool`/`fuel`/stats-GC) es **thread-local**: cada worker recolecta
    /// sólo su heap y no comparte marcos/pila. Arranca sin fibra en curso (`cur` vacío); la toma de `ready`.
    fn worker(program: &'a CompiledProgram, shared: Arc<Mutex<Shared>>) -> Self {
        Vm {
            program,
            cur: Fiber::default(), // sin fibra aún; `poll_next` cargará la primera de `ready`
            shared,
            locals_pool: Vec::new(),
            fuel: u64::MAX,
            gc_count: 0,
            gc_max_pause_ns: 0,
            gc_total_pause_ns: 0,
            stop: false,
        }
    }

    /// M38.3b paso 2: bloquea el estado compartido del scheduler. Una operación del scheduler llama esto
    /// UNA vez y usa el guard (`&mut *sh`) para toda su cadena de helpers → sin reentrancia ni deadlock.
    fn sched(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().expect("the scheduler Mutex should not be poisoned") // ice-ok: invariante
    }

    /// M38.3b paso 3: **orquestador** del scheduler M:N. Arma la fibra de `main`, la encola en `ready` y
    /// lanza `num_workers(program)` hilos worker (o corre single-thread si es 1 → scheduler determinista).
    /// Cada worker ejecuta fibras de la cola compartida hasta que `main` retorna (semántica Go) o hay un
    /// error fatal; el primero en terminar fija `Shared.outcome`, que los demás ven y se detienen. El
    /// resultado del programa es ese `outcome`.
    fn run(&mut self) -> Result<HeapValue, RuntimeError> {
        // Marco inicial: main, con su arreglo de locales (sin argumentos). Se encola como una fibra más en
        // `ready`; un worker la tomará (con N=1, este mismo Vm). La fibra de main **reutiliza el heap de
        // `self.cur`** (no un `Heap::new()`): `run_program_with_limit` fija ahí el tope de heap (M42.2) antes
        // de `run()`, y hay que conservarlo.
        let main = self.program.main;
        let locals = self.new_locals(main);
        let mut main_fiber = std::mem::take(&mut self.cur); // is_main: true, heap con el tope preconfigurado
        main_fiber.frames.push(CallFrame { function: main, ip: 0, locals, upvalues: Vec::new(), stack_base: 0 });
        self.sched().ready.push_back(main_fiber);

        let n = num_workers(self.program);
        if n == 1 {
            // Single-thread determinista (default): este Vm ES el único worker. `poll_next` toma main de
            // `ready`; comportamiento idéntico a antes de M38.3b (con N=1 el `running` oscila 1↔0 y nunca se
            // espera en el busy-poll).
            self.run_worker();
        } else {
            // M:N: N hilos worker comparten `program` (inmutable, vía `ProgRef`) y `shared`. Cada uno con su
            // estado de ejecución (heap/marcos) thread-local. `thread::scope` garantiza que se unen antes de
            // salir → los préstamos de `self`/`program` viven lo suficiente (sin `'static`).
            let prog = ProgRef(self.program);
            let shared = Arc::clone(&self.shared);
            std::thread::scope(|s| {
                for _ in 0..n {
                    let shared = Arc::clone(&shared);
                    // Pila grande por worker (paridad con `with_big_stack`): la VM es iterativa, pero
                    // `format_value`/`collect`/`transfer_value` recurren sobre la pila de Rust en estructuras
                    // profundas. 256 MiB como el hilo principal del binario.
                    std::thread::Builder::new()
                        .stack_size(256 * 1024 * 1024)
                        .spawn_scoped(s, move || {
                            // Rebind del `ProgRef` ENTERO (no `prog.0`): fuerza la captura de la struct
                            // `Send`, no del `&CompiledProgram` disjunto (captura disjunta de la ed. 2021).
                            let prog = prog;
                            let mut w = Vm::worker(prog.0, shared);
                            w.run_worker();
                        })
                        .expect("could not launch the worker thread"); // ice-ok: fallo del SO al crear hilo
                }
            });
        }
        // Todos los workers unidos: `outcome` debe estar fijado (main terminó o hubo un fatal). Si por algún
        // camino quedó vacío, el programa no produjo nada → unit.
        self.sched().outcome.take().unwrap_or(Ok(HeapValue::Unit))
    }

    /// M38.3b paso 3: el bucle de ejecución de fibras (antes el cuerpo de `run`). Ejecuta la fibra en
    /// `self.cur`; al bloquear/terminar una fibra `poll_next` carga la siguiente y el bucle sigue. Retorna
    /// cuando `main` termina (su valor), hay un fatal (Err) o `self.stop` (otro worker apagó → Unit ignorado).
    fn run_loop(&mut self) -> Result<HeapValue, RuntimeError> {
        // El programa es inmutable y vive tanto como la VM; copiamos su referencia a un local (Opt.1). Así
        // el `match` de cada instrucción la toma **prestada** del programa (no de `self`), y el cuerpo puede
        // mutar `self` sin que el préstamo choque — eliminando el clon de la instrucción por iteración.
        let program = self.program;

        loop {
            // M38.3b: si otro worker apagó el programa, deténte (el valor Unit se ignora: `outcome` ya está).
            if self.stop {
                return Ok(HeapValue::Unit);
            }
            // --- Punto seguro del GC ---
            if self.cur.heap.should_collect() {
                self.collect();
                // M42.2: tope de heap. Si tras recolectar siguen vivos más objetos de los permitidos,
                // el programa realmente necesita más memoria de la presupuestada → aborta limpio.
                if self.cur.heap.over_cap() {
                    let fi = self.cur.frames.len() - 1;
                    let func = self.cur.frames[fi].function;
                    let ip = self.cur.frames[fi].ip;
                    let (l, c) = program.functions[func].chunk.lines.get(ip).copied().unwrap_or((0, 0));
                    return Err(runtime_error(l, c, "memory limit exhausted (heap cap)"));
                }
            }

            let fi = self.cur.frames.len() - 1;
            let func = self.cur.frames[fi].function;
            let ip = self.cur.frames[fi].ip;

            // M42.1: fuel. Sin límite (`u64::MAX`) nunca dispara; con límite, aborta al agotarse. La
            // posición es la de la instrucción en curso (para el diagnóstico).
            if self.fuel == 0 {
                let (l, c) = program.functions[func].chunk.lines.get(ip).copied().unwrap_or((0, 0));
                return Err(runtime_error(l, c, "instruction limit exhausted (fuel)"));
            }
            self.fuel -= 1;

            // Robustez: si se acabó el chunk sin Return (no debería), retorna unit.
            if ip >= program.functions[func].chunk.code.len() {
                if let Some(frame) = self.cur.frames.pop() {
                    self.cur.stack.truncate(frame.stack_base);
                    self.recycle_locals(frame.locals); // Opt.2
                }
                if self.cur.frames.is_empty() {
                    match self.on_fiber_done(HeapValue::Unit)? {
                        Some(v) => return Ok(v),
                        None => continue, // era una fibra spawn: el scheduler ya cargó la siguiente (o `stop`)
                    }
                }
                self.cur.stack.push(HeapValue::Unit);
                continue;
            }

            // La instrucción se toma PRESTADA del programa (Opt.1: sin clonar). `instr` vive lo que
            // `program` (toda la VM), así que no estorba a las mutaciones de `self` del cuerpo.
            let instr = &program.functions[func].chunk.code[ip];
            // Opt.7: la posición `(línea, col)` NO se lee por instrucción —el camino caliente
            // (locales/constantes/aritmética/saltos) nunca la usa, solo los sitios de error o de cesión—.
            // Se resuelve **bajo demanda** con `pos!()`, leyendo `lines[ip]` solo donde hace falta.
            macro_rules! pos { () => {{ let p = program.functions[func].chunk.lines[ip]; (p.0, p.1) }} }
            self.cur.frames[fi].ip = ip + 1; // avance por defecto; los saltos lo cambian

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
                        // -i64::MIN desborda (M34, SPEC §8): error, como la aritmética binaria.
                        HeapValue::Int(n) => HeapValue::Int(n.checked_neg().ok_or_else(|| {
                            let (l, c) = pos!();
                            runtime_error(l, c, "arithmetic overflow on int")
                        })?),
                        HeapValue::Float(x) => HeapValue::Float(-x),
                        _ => unreachable!("the checker guarantees a number"),
                    });
                }
                OpCode::Not => {
                    let v = self.pop();
                    self.push(match v {
                        HeapValue::Bool(b) => HeapValue::Bool(!b),
                        _ => unreachable!("the checker guarantees a bool"),
                    });
                }
                OpCode::BitNot => {
                    let v = self.pop();
                    self.push(match v {
                        HeapValue::Int(n) => HeapValue::Int(!n), // M19.3a: complemento a uno
                        HeapValue::UInt(n, w) => uint_heap(!n, w), // M28.3: NOT sobre uint (enmascarado)
                        _ => unreachable!("the checker guarantees an int"),
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
                | OpCode::GreaterEqual
                | OpCode::BitAnd
                | OpCode::BitOr
                | OpCode::BitXor
                | OpCode::Shl
                | OpCode::Shr) => {
                    let right = self.pop();
                    let left = self.pop();
                    // Opt.4: fast-path entero. En la inmensa mayoría de programas (bucles, recursión
                    // aritmética) ambos operandos son `Int`; resolverlo aquí evita el doble match y la
                    // llamada a `apply_binary` (que rematchea opcode + ~30 combinaciones de tipos).
                    // Medido (mejor de 5, release): fib(35) -5%, bucle aritmético 10M -6%. Semántica
                    // idéntica al camino general — incluido el desbordamiento como error (M34, SPEC §8).
                    // (Opt.1/Opt.2 ya aplicadas; Opt.3 = `Rc<str>`, evaluada y descartada → por eso esto es Opt.4.)
                    if let (HeapValue::Int(a), HeapValue::Int(b)) = (&left, &right) {
                        let (a, b) = (*a, *b);
                        let ovf = || {
                            let (l, c) = pos!();
                            runtime_error(l, c, "arithmetic overflow on int")
                        };
                        let r = match bin {
                            OpCode::Add => HeapValue::Int(a.checked_add(b).ok_or_else(ovf)?),
                            OpCode::Sub => HeapValue::Int(a.checked_sub(b).ok_or_else(ovf)?),
                            OpCode::Mul => HeapValue::Int(a.checked_mul(b).ok_or_else(ovf)?),
                            OpCode::Div => {
                                if b == 0 {
                                    return Err(runtime_error(pos!().0, pos!().1, "integer division by zero"));
                                }
                                HeapValue::Int(a.checked_div(b).ok_or_else(ovf)?)
                            }
                            OpCode::Rem => {
                                if b == 0 {
                                    return Err(runtime_error(pos!().0, pos!().1, "modulo by zero"));
                                }
                                HeapValue::Int(a.checked_rem(b).ok_or_else(ovf)?)
                            }
                            OpCode::Less => HeapValue::Bool(a < b),
                            OpCode::LessEqual => HeapValue::Bool(a <= b),
                            OpCode::Greater => HeapValue::Bool(a > b),
                            OpCode::GreaterEqual => HeapValue::Bool(a >= b),
                            OpCode::Equal => HeapValue::Bool(a == b),
                            OpCode::NotEqual => HeapValue::Bool(a != b),
                            // Bit a bit (M19.3a): mismos `wrapping_*` que el intérprete.
                            OpCode::BitAnd => HeapValue::Int(a & b),
                            OpCode::BitOr => HeapValue::Int(a | b),
                            OpCode::BitXor => HeapValue::Int(a ^ b),
                            OpCode::Shl => HeapValue::Int(a.wrapping_shl(b as u32)),
                            OpCode::Shr => HeapValue::Int(a.wrapping_shr(b as u32)),
                            _ => unreachable!("the `bin` group only holds binary operators"),
                        };
                        self.push(r);
                    }
                    // M11.7b: `+` sobre dos arreglos (objetos del heap) los concatena en uno nuevo.
                    // El checker garantiza que dos `Obj` con `Add` son arreglos (strings son inline).
                    else if let (OpCode::Add, HeapValue::Obj(l), HeapValue::Obj(r)) = (bin, &left, &right) {
                        let (l, r) = (*l, *r);
                        let mut elems = self.as_array(l).clone();
                        elems.extend(self.as_array(r).iter().cloned());
                        let h = self.cur.heap.allocate(Obj::Array(elems));
                        self.push(HeapValue::Obj(h));
                    } else {
                        let result = self.apply_binary(bin, left, right, pos!().0, pos!().1)?;
                        self.push(result);
                    }
                }

                OpCode::Jump(target) => {
                    self.cur.frames[fi].ip = *target;
                }
                OpCode::JumpIfFalse(target) => {
                    if matches!(self.peek(), HeapValue::Bool(false)) {
                        self.cur.frames[fi].ip = *target;
                    }
                }

                OpCode::GetLocal(slot) => {
                    let v = self.get_local(fi, *slot);
                    self.push(v);
                }
                // M36.1: superinstrucciones — dos empujes en una iteración del lazo.
                OpCode::GetLocalLocal(s, t) => {
                    let a = self.get_local(fi, *s);
                    let b = self.get_local(fi, *t);
                    self.push(a);
                    self.push(b);
                }
                OpCode::GetLocalConst(s, c) => {
                    let a = self.get_local(fi, *s);
                    let b = const_to_heap(&self.program.functions[func].chunk.constants[*c]);
                    self.push(a);
                    self.push(b);
                }
                OpCode::SetLocal(slot) => {
                    let v = self.pop();
                    self.set_local(fi, *slot, v);
                }
                // P0.6 (ronda 3): la guarda entera `local op const` de if/while en UNA instrucción.
                // Semántica idéntica a [GetLocalConst(s,c), CmpJump(op,t)]: compara local[s] con
                // const[c] y, si es falso, salta a t — sin apilar/sacar los operandos.
                OpCode::GetLocalConstCmpJump(s, c, op, target) => {
                    let left = self.get_local(fi, *s);
                    let right = const_to_heap(&self.program.functions[func].chunk.constants[*c]);
                    let res = if let (HeapValue::Int(a), HeapValue::Int(b)) = (&left, &right) {
                        match op {
                            CmpOp::Less => a < b,
                            CmpOp::LessEqual => a <= b,
                            CmpOp::Greater => a > b,
                            CmpOp::GreaterEqual => a >= b,
                            CmpOp::Equal => a == b,
                            CmpOp::NotEqual => a != b,
                        }
                    } else {
                        let legacy = match op {
                            CmpOp::Less => &OpCode::Less,
                            CmpOp::LessEqual => &OpCode::LessEqual,
                            CmpOp::Greater => &OpCode::Greater,
                            CmpOp::GreaterEqual => &OpCode::GreaterEqual,
                            CmpOp::Equal => &OpCode::Equal,
                            CmpOp::NotEqual => &OpCode::NotEqual,
                        };
                        match self.apply_binary(legacy, left, right, pos!().0, pos!().1)? {
                            HeapValue::Bool(b) => b,
                            _ => unreachable!("a comparison produces bool"),
                        }
                    };
                    if !res {
                        self.cur.frames[fi].ip = *target;
                    }
                }
                // A4 (ronda 2): la guarda de if/while en UNA instrucción. Semántica idéntica a
                // [Cmp, JumpIfFalse(t), Pop]: saca ambos operandos, compara, y si es falso salta
                // (el destino ya viene ajustado tras el Pop del lado else). El bool nunca se apila.
                OpCode::CmpJump(op, target) => {
                    let right = self.pop();
                    let left = self.pop();
                    let res = if let (HeapValue::Int(a), HeapValue::Int(b)) = (&left, &right) {
                        // Fast-path entero (el caso dominante: i < n, x == 0, …).
                        match op {
                            CmpOp::Less => a < b,
                            CmpOp::LessEqual => a <= b,
                            CmpOp::Greater => a > b,
                            CmpOp::GreaterEqual => a >= b,
                            CmpOp::Equal => a == b,
                            CmpOp::NotEqual => a != b,
                        }
                    } else {
                        let legacy = match op {
                            CmpOp::Less => &OpCode::Less,
                            CmpOp::LessEqual => &OpCode::LessEqual,
                            CmpOp::Greater => &OpCode::Greater,
                            CmpOp::GreaterEqual => &OpCode::GreaterEqual,
                            CmpOp::Equal => &OpCode::Equal,
                            CmpOp::NotEqual => &OpCode::NotEqual,
                        };
                        match self.apply_binary(legacy, left, right, pos!().0, pos!().1)? {
                            HeapValue::Bool(b) => b,
                            _ => unreachable!("a comparison produces bool"),
                        }
                    };
                    if !res {
                        self.cur.frames[fi].ip = *target;
                    }
                }
                // A4 (ronda 2): local[s] + const / local[s] - const, en una instrucción.
                OpCode::AddLocalConst(s, c) => {
                    let left = self.get_local(fi, *s);
                    let right = const_to_heap(&self.program.functions[func].chunk.constants[*c]);
                    let r = if let (HeapValue::Int(a), HeapValue::Int(b)) = (&left, &right) {
                        HeapValue::Int(a.checked_add(*b).ok_or_else(|| {
                            let (l, c2) = pos!();
                            runtime_error(l, c2, "arithmetic overflow on int")
                        })?)
                    } else {
                        self.apply_binary(&OpCode::Add, left, right, pos!().0, pos!().1)?
                    };
                    self.push(r);
                }
                OpCode::SubLocalConst(s, c) => {
                    let left = self.get_local(fi, *s);
                    let right = const_to_heap(&self.program.functions[func].chunk.constants[*c]);
                    let r = if let (HeapValue::Int(a), HeapValue::Int(b)) = (&left, &right) {
                        HeapValue::Int(a.checked_sub(*b).ok_or_else(|| {
                            let (l, c2) = pos!();
                            runtime_error(l, c2, "arithmetic overflow on int")
                        })?)
                    } else {
                        self.apply_binary(&OpCode::Sub, left, right, pos!().0, pos!().1)?
                    };
                    self.push(r);
                }
                OpCode::InitLocal(slot) => {
                    // Declaración: si el slot está boxeado, estrena celda (shadowing
                    // seguro); si no, guarda el valor directamente.
                    let v = self.pop();
                    let boxed = self.program.functions[func].captured.get(*slot).copied().unwrap_or(false);
                    self.cur.frames[fi].locals[*slot] = if boxed {
                        Local::Boxed(self.cur.heap.allocate(Obj::Cell(v)))
                    } else {
                        Local::Plain(v)
                    };
                }
                OpCode::GetUpvalue(i) => {
                    let h = self.cur.frames[fi].upvalues[*i];
                    let v = self.cell_get(h);
                    self.push(v);
                }
                OpCode::SetUpvalue(i) => {
                    let v = self.pop();
                    let h = self.cur.frames[fi].upvalues[*i];
                    self.cell_set(h, v);
                }

                OpCode::Print => {
                    let v = self.pop();
                    crate::host_print(&format_value(&self.cur.heap, &self.program.enums, &v));
                    self.push(HeapValue::Unit);
                }

                // --- FFI (M41): llamada a una función C ---
                OpCode::CallExtern(idx, argc) => {
                    let desc = &program.externs[*idx];
                    let mut fargs = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        fargs.push(self.pop());
                    }
                    fargs.reverse(); // se sacaron en orden inverso
                    // Los `HeapValue` retenidos en `fargs` viven durante la llamada; los `FfiVal` de
                    // string/bytes toman prestado su buffer inline (M41.2).
                    let mut cargs = Vec::with_capacity(*argc);
                    for v in &fargs {
                        cargs.push(match v {
                            HeapValue::Int(n) => crate::ffi::FfiVal::Int(*n),
                            HeapValue::UInt(v, _) => crate::ffi::FfiVal::Int(*v as i64), // M41.4: u64 → 64 bits
                            HeapValue::Float(f) => crate::ffi::FfiVal::Float(*f),
                            HeapValue::Bool(b) => crate::ffi::FfiVal::Int(*b as i64),
                            HeapValue::Str(s) => crate::ffi::FfiVal::Str(s.as_str()),
                            HeapValue::Bytes(b) => crate::ffi::FfiVal::Bytes(b.as_slice()),
                            HeapValue::Ptr(p) => crate::ffi::FfiVal::Int(*p), // M41.4b
                            _ => return Err(runtime_error(pos!().0, pos!().1,
                                "non-marshalable argument at the FFI boundary")),
                        });
                    }
                    let r = crate::ffi::call(desc, &cargs)
                        .map_err(|m| runtime_error(pos!().0, pos!().1, &m))?;
                    let val = match r {
                        crate::ffi::FfiRet::Int(n) if desc.ret_kind == crate::ffi::CKind::Bool => HeapValue::Bool(n != 0),
                        crate::ffi::FfiRet::Int(n) if desc.ret_kind == crate::ffi::CKind::U64 => HeapValue::UInt(n as u64, 64),
                        crate::ffi::FfiRet::Int(n) => HeapValue::Int(n),
                        crate::ffi::FfiRet::Float(f) => HeapValue::Float(f),
                        crate::ffi::FfiRet::Unit => HeapValue::Unit,
                        // M41.3: char* → Option<bytes>/Option<string> (construido como el enum del prelude).
                        crate::ffi::FfiRet::OptBytes(opt) => {
                            let (variant, payload): (&str, Vec<HeapValue>) = match opt {
                                None => ("None", vec![]),
                                Some(bytes) => {
                                    let inner = if desc.ret_kind == crate::ffi::CKind::OptStr {
                                        match String::from_utf8(bytes) {
                                            Ok(s) => HeapValue::Str(s),
                                            Err(_) => return Err(runtime_error(pos!().0, pos!().1,
                                                "the C function returned bytes that are not valid UTF-8 (declare Option<bytes> to receive them raw)")),
                                        }
                                    } else {
                                        HeapValue::Bytes(bytes)
                                    };
                                    ("Some", vec![inner])
                                }
                            };
                            let (eid, tag) = option_variant(&program.enums, variant).ok_or_else(||
                                runtime_error(pos!().0, pos!().1, "the prelude Option enum is not available for the FFI return_val"))?;
                            let h = self.cur.heap.allocate(Obj::Enum(VmEnum { enum_id: eid, tag, payload }));
                            HeapValue::Obj(h)
                        }
                        // M41.4b: puntero opaco.
                        crate::ffi::FfiRet::Ptr(p) => HeapValue::Ptr(p),
                        crate::ffi::FfiRet::OptPtr(opt) => {
                            let (variant, payload): (&str, Vec<HeapValue>) = match opt {
                                None => ("None", vec![]),
                                Some(p) => ("Some", vec![HeapValue::Ptr(p)]),
                            };
                            let (eid, tag) = option_variant(&program.enums, variant).ok_or_else(||
                                runtime_error(pos!().0, pos!().1, "the prelude Option enum is not available for the FFI return_val"))?;
                            let h = self.cur.heap.allocate(Obj::Enum(VmEnum { enum_id: eid, tag, payload }));
                            HeapValue::Obj(h)
                        }
                    };
                    self.push(val);
                }

                // --- Arreglos (M3) ---
                OpCode::MakeArray(n) => {
                    let mut elems = Vec::with_capacity(*n);
                    for _ in 0..*n {
                        elems.push(self.pop());
                    }
                    elems.reverse(); // se sacaron en orden inverso
                    // M98.5: un literal con TODOS los elementos int nace especializado (8 B/elem).
                    let h = self.cur.heap.allocate(specialize_array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Index => {
                    let i = self.pop_int();
                    match self.pop() {
                        HeapValue::Obj(h) => {
                            // M98.5: camino nativo del IntArray (sin degradar).
                            if let Obj::IntArray(v) = self.cur.heap.get(h) {
                                let idx = bounds_check(i, v.len(), pos!().0, pos!().1)?;
                                let n = v[idx];
                                self.push(HeapValue::Int(n));
                            } else {
                                let idx = {
                                    let arr = self.as_array(h);
                                    bounds_check(i, arr.len(), pos!().0, pos!().1)?
                                };
                                let v = self.as_array(h)[idx].clone();
                                self.push(v);
                            }
                        }
                        // M11.4c-2: indexar un string → el carácter en esa posición.
                        // M90.6 (superset de Opt.16): sin materializar los chars (antes un
                        // `Vec<char>` COMPLETO por acceso): un string ASCII indexa el byte en
                        // O(1); uno no-ASCII escanea hasta `i` sin asignar. El conteo total
                        // solo se paga al errar.
                        HeapValue::Str(s) => {
                            let c = if s.is_ascii() {
                                let idx = bounds_check(i, s.len(), pos!().0, pos!().1)?;
                                s.as_bytes()[idx] as char
                            } else {
                                match usize::try_from(i).ok().and_then(|idx| s.chars().nth(idx)) {
                                    Some(c) => c,
                                    None => {
                                        bounds_check(i, s.chars().count(), pos!().0, pos!().1)?;
                                        unreachable!("nth failed ⇒ index out of range")
                                    }
                                }
                            };
                            self.push(HeapValue::Char(c));
                        }
                        // M16.1a: indexar bytes → el octeto como int.
                        HeapValue::Bytes(b) => {
                            let idx = bounds_check(i, b.len(), pos!().0, pos!().1)?;
                            self.push(HeapValue::Int(b[idx] as i64));
                        }
                        _ => unreachable!("the checker guarantees an array, string or bytes"),
                    }
                }
                OpCode::SetIndex => {
                    let v = self.pop();
                    let i = self.pop_int();
                    let h = self.pop_obj();
                    // M98.5: camino nativo del IntArray si el valor es Int (el checker lo garantiza
                    // para [int]; cualquier otra cosa degrada y sigue por el genérico).
                    if let (Obj::IntArray(xs), HeapValue::Int(n)) = (self.cur.heap.get_mut(h), &v) {
                        let idx = bounds_check(i, xs.len(), pos!().0, pos!().1)?;
                        xs[idx] = *n;
                    } else {
                        let idx = bounds_check(i, self.as_array(h).len(), pos!().0, pos!().1)?;
                        self.as_array_mut(h)[idx] = v;
                    }
                }
                OpCode::Len => {
                    // M11.1a: len de arreglo o string; M13.1: len de Map (nº de entradas).
                    let len = match self.pop() {
                        // M90.6: ASCII → nº de chars == nº de bytes (O(1) tras el is_ascii vectorizado).
                        HeapValue::Str(s) => {
                            if s.is_ascii() { s.len() as i64 } else { s.chars().count() as i64 }
                        }
                        // M16.1a: len de bytes = nº de octetos.
                        HeapValue::Bytes(b) => b.len() as i64,
                        HeapValue::Obj(h) => match self.cur.heap.get(h) {
                            Obj::Array(v) => v.len() as i64,
                            Obj::IntArray(v) => v.len() as i64, // M98.5
                            Obj::Map(m) => m.len() as i64,
                            _ => unreachable!("the checker guarantees an array or Map"),
                        },
                        _ => unreachable!("the checker guarantees an array, string, Map or bytes"),
                    };
                    self.push(HeapValue::Int(len));
                }
                // M27.4: conversión numérica `as` (según el valor en runtime + destino).
                OpCode::Cast(target) => {
                    let v = self.pop();
                    let out = match (&v, target) {
                        (HeapValue::Int(n), CastTarget::Float) => HeapValue::Float(*n as f64),
                        (HeapValue::Float(f), CastTarget::Int) => HeapValue::Int(*f as i64),
                        (HeapValue::Char(c), CastTarget::Int) => HeapValue::Int(*c as i64),
                        (HeapValue::Int(n), CastTarget::Char) => {
                            match u32::try_from(*n).ok().and_then(char::from_u32) {
                                Some(c) => HeapValue::Char(c),
                                None => {
                                    return Err(runtime_error(pos!().0, pos!().1,
                                        &format!("{} is not a valid Unicode character for 'as char'", n)));
                                }
                            }
                        }
                        // M28.3: conversiones de/hacia enteros sin signo con tamaño (enmascaran al ancho).
                        (HeapValue::Int(n), CastTarget::UInt(w)) => uint_heap(*n as u64, *w),
                        (HeapValue::UInt(n, _), CastTarget::Int) => HeapValue::Int(*n as i64),
                        (HeapValue::UInt(n, _), CastTarget::UInt(w)) => uint_heap(*n, *w),
                        (HeapValue::UInt(n, _), CastTarget::Float) => HeapValue::Float(*n as f64),
                        (HeapValue::Float(f), CastTarget::UInt(w)) => uint_heap(*f as i64 as u64, *w),
                        (HeapValue::Char(c), CastTarget::UInt(w)) => uint_heap(*c as u64, *w),
                        _ => v, // identidad
                    };
                    self.push(out);
                }
                // --- Mapas Map<K,V> (M13.1) ---
                OpCode::MapNew => {
                    let h = self.cur.heap.allocate(Obj::Map(Default::default()));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::MapInsert => {
                    let v = self.pop();
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    match self.cur.heap.get_mut(h) {
                        Obj::Map(m) => { m.insert(k, v); }
                        _ => unreachable!("the checker guarantees a Map"),
                    }
                    self.push(HeapValue::Unit);
                }
                OpCode::MapContainsKey => {
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let present = match self.cur.heap.get(h) {
                        Obj::Map(m) => m.contains_key(&k),
                        _ => unreachable!("the checker guarantees a Map"),
                    };
                    self.push(HeapValue::Bool(present));
                }
                OpCode::MapGet => {
                    // Primitivo: [] o [v]; el prelude lo envuelve en Option<V>.
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let elems = match self.cur.heap.get(h) {
                        Obj::Map(m) => match m.get(&k) {
                            Some(v) => vec![v.clone()],
                            None => vec![],
                        },
                        _ => unreachable!("the checker guarantees a Map"),
                    };
                    let arr = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::MapGetOr => {
                    // P0.2: get-or-default SIN alocar. Args apilados (map, key, default) → cima = default.
                    let d = self.pop();
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let v = match self.cur.heap.get(h) {
                        Obj::Map(m) => m.get(&k).cloned().unwrap_or(d),
                        _ => unreachable!("the checker guarantees a Map"),
                    };
                    self.push(v);
                }
                OpCode::MapAdd => {
                    // P0.3: upsert acumulativo en UN lookup (entry-API). Args (map, key, delta) → cima = delta.
                    use std::collections::hash_map::Entry;
                    let delta = self.pop();
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let (l, c) = pos!();
                    let ovf = || runtime_error(l, c, "arithmetic overflow on int");
                    match self.cur.heap.get_mut(h) {
                        Obj::Map(m) => match m.entry(k) {
                            Entry::Occupied(mut e) => {
                                let nv = match (e.get(), &delta) {
                                    (HeapValue::Int(a), HeapValue::Int(b)) => HeapValue::Int(a.checked_add(*b).ok_or_else(ovf)?),
                                    (HeapValue::Float(a), HeapValue::Float(b)) => HeapValue::Float(a + b),
                                    _ => unreachable!("the checker guarantees int/float map value + matching delta"),
                                };
                                e.insert(nv);
                            }
                            // Ausente: m[k] = delta (0 + delta).
                            Entry::Vacant(e) => { e.insert(delta); }
                        },
                        _ => unreachable!("the checker guarantees a Map"),
                    }
                    self.push(HeapValue::Unit);
                }
                OpCode::MapRemove => {
                    // M13.1b: quita la clave; [] o [v]. El prelude → Option<V>.
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let elems = match self.cur.heap.get_mut(h) {
                        Obj::Map(m) => match m.remove(&k) {
                            Some(v) => vec![v],
                            None => vec![],
                        },
                        _ => unreachable!("the checker guarantees a Map"),
                    };
                    let arr = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::MapKeys => {
                    // M13.1b: claves ordenadas (determinista).
                    let h = self.pop_obj();
                    let mut ks: Vec<MapKey> = match self.cur.heap.get(h) {
                        Obj::Map(m) => m.keys().cloned().collect(),
                        _ => unreachable!("the checker guarantees a Map"),
                    };
                    ks.sort();
                    let elems: Vec<HeapValue> = ks.iter().map(key_to_heap).collect();
                    let arr = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::MapValues => {
                    // M13.1b: valores en orden de clave ordenada (casa con keys).
                    let h = self.pop_obj();
                    let elems: Vec<HeapValue> = match self.cur.heap.get(h) {
                        Obj::Map(m) => {
                            let mut pairs: Vec<(&MapKey, &HeapValue)> = m.iter().collect();
                            pairs.sort_by(|a, b| a.0.cmp(b.0));
                            pairs.iter().map(|(_, v)| (*v).clone()).collect()
                        }
                        _ => unreachable!("the checker guarantees a Map"),
                    };
                    let arr = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::Push => {
                    let v = self.pop();
                    let h = self.pop_obj();
                    // M98.5: caminos nativos — push de Int sobre IntArray, y PROMOCIÓN del arreglo
                    // genérico VACÍO al recibir su primer Int (el patrón `var xs = []; … push`).
                    // Cualquier otra combinación degrada (si hace falta) y sigue por el genérico.
                    match (self.cur.heap.get_mut(h), &v) {
                        (Obj::IntArray(xs), HeapValue::Int(n)) => xs.push(*n),
                        (Obj::Array(xs), HeapValue::Int(n)) if xs.is_empty() => {
                            *self.cur.heap.get_mut(h) = Obj::IntArray(vec![*n]);
                        }
                        _ => self.as_array_mut(h).push(v),
                    }
                    self.push(HeapValue::Unit);
                }

                // --- Concurrencia: CSP sobre la VM (M12.1) ---
                OpCode::Spawn | OpCode::SpawnDiscard => {
                    // Saca el valor-función; crea una fibra nueva que lo ejecuta (0 args), le asigna una
                    // Task<T> (M12.3) y la encola. Si hay un scope activo, adscribe la tarea a él.
                    // M98.1: `SpawnDiscard` (fire-and-forget fuera de scope) NO aloja Task — no hay
                    // quién la consuma y la entrada quedaría retenida para siempre (la fuga de M98).
                    let discard = matches!(instr, OpCode::SpawnDiscard);
                    let (fn_idx, upvalues) = match self.pop() {
                        HeapValue::Function(i) => (i, Vec::new()),
                        HeapValue::Obj(h) => match self.cur.heap.get(h) {
                            Obj::Closure(c) => (c.index, c.upvalues.clone()),
                            _ => unreachable!("the checker guarantees a function"),
                        },
                        _ => unreachable!("the checker guarantees a function"),
                    };
                    // M38.1b-2: la fibra hija tiene su PROPIO heap; las capturas (upvalues) del closure
                    // viven en el heap del spawner → se transfieren al heap nuevo (aislamiento por actores).
                    // Esto es thread-local (usa `self.cur.heap`/`self.locals_pool`) → sin lock.
                    let mut new_heap = Heap::new();
                    let mut remap = HashMap::new();
                    let upvalues: Vec<Handle> = upvalues.iter()
                        .map(|&up| transfer_obj(&self.cur.heap, &mut new_heap, up, &mut remap))
                        .collect();
                    let locals = self.new_locals(fn_idx);
                    let frame = CallFrame { function: fn_idx, ip: 0, locals, upvalues, stack_base: 0 };
                    // M38.3b paso 3: alojar la Task y encolar la fibra hija en UN solo lock (bajo M:N real,
                    // dos `self.sched()` —len y push— tendrían un TOCTOU en el id de la tarea).
                    // M98.1: fire-and-forget fuera de scope → fibra sin Task (nada que retener).
                    // Dentro de un scope, SpawnDiscard aloja igual (el scope la rastrea y consume).
                    let needs_task = !discard || !self.cur.scopes.is_empty();
                    let task = {
                        let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                        let task = if needs_task {
                            Some(sh.alloc_task()) // M98.1: slot con generación (reusa libres)
                        } else {
                            None
                        };
                        sh.ready.push_back(Fiber {
                            frames: vec![frame], stack: Vec::new(), heap: new_heap, is_main: false,
                            task, scopes: Vec::new(),
                        });
                        task
                    };
                    if let (Some(task), Some(scope)) = (task, self.cur.scopes.last_mut()) {
                        scope.children.push(task); // M12.3: adscribe la tarea al scope activo
                    }
                    if discard {
                        self.push(HeapValue::Unit); // el Pop que sigue lo descarta
                    } else {
                        self.push(HeapValue::Task(task.expect("Spawn always allocates")));
                    }
                }
                OpCode::Signals => {
                    // M88.1: el canal de señales del SO — SINGLETON del proceso (la primera
                    // llamada crea el canal + instala el self-pipe y los handlers; las demás
                    // devuelven el mismo canal). El fd entra al poller vía io_wait.
                    let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                    if sh.signal_chan.is_none() {
                        let fd = match crate::builtins::signals_install() {
                            Ok(fd) => fd,
                            Err(e) => return Err(runtime_error(pos!().0, pos!().1, &e)),
                        };
                        let id = sh.alloc_channel(None); // M98.3: slot con generación
                        sh.signal_chan = Some(id);
                        sh.signal_fd = fd;
                    }
                    let id = sh.signal_chan.expect("just created");
                    drop(sh);
                    self.push(HeapValue::Channel(id));
                }
                OpCode::ChannelNew => {
                    // channel() sin argumentos → canal NO acotado (cap = None). M38.1b: en el host.
                    // M38.3b paso 3: id + push en UN solo lock (TOCTOU bajo M:N real).
                    let id = {
                        let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                        let id = sh.alloc_channel(None); // M98.3: slot con generación
                        id
                    };
                    self.push(HeapValue::Channel(id));
                }
                OpCode::ChannelNewBounded => {
                    // channel(n) → canal acotado a la capacidad n ≥ 0 (n = 0 rendezvous), M12.2.
                    let n = match self.pop() {
                        HeapValue::Int(n) => n,
                        _ => unreachable!("the checker guarantees an int"),
                    };
                    if n < 0 {
                        return Err(runtime_error(pos!().0, pos!().1, "a channel capacity cannot be negative"));
                    }
                    let id = {
                        let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                        let id = sh.alloc_channel(Some(n as usize)); // M98.3
                        id
                    };
                    self.push(HeapValue::Channel(id));
                }
                OpCode::ChanSend => {
                    let v = self.pop();
                    let h = self.pop_channel();
                    // M38.3b paso 2: un ÚNICO lock por handler (lock-once). Se bloquea `self.shared`
                    // DIRECTAMENTE (no vía `self.sched()`, que tomaría `&self` entero) para que el guard
                    // preste solo el campo `self.shared`; así `self.cur.*` (campo disjunto) sigue accesible
                    // bajo el guard sostenido.
                    let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                    // M98.3: un handle stale (canal liberado = cerrado y drenado) responde como cerrado.
                    let (closed, len, cap) = match sh.chan(h) {
                        Some(c) => (c.closed, c.queue.len(), c.cap),
                        None => (true, 0, None),
                    };
                    if closed {
                        return Err(runtime_error(pos!().0, pos!().1, "send on a closed channel"));
                    }
                    // (1) ¿Hay un receptor bloqueado en este canal? Entrégaselo directo (rendezvous) y
                    // despiértalo (el primero, FIFO → determinista).
                    if let Some(pos) = sh.parked.iter().position(
                        |p| p.on == h && matches!(p.waiting, Waiting::Recv))
                    {
                        let parked = sh.parked.remove(pos);
                        Self::wake_recv(&self.cur, &mut sh, parked.fiber, vec![v]);
                        self.cur.stack.push(HeapValue::Unit);
                    } else if cap.is_none() || len < cap.unwrap() {
                        // (2) Hay hueco (no acotado, o len < cap) → encola y sigue. M38.1b-2: el valor se
                        // transfiere del heap de la fibra al heap del canal (en tránsito). (El canal está
                        // vivo: si fuera stale habríamos errado arriba como cerrado.)
                        let ch = sh.chan_mut(h).expect("live: stale handles error above as closed");
                        let mut ch_heap = std::mem::take(&mut ch.heap);
                        let v2 = transfer_value(&self.cur.heap, &mut ch_heap, &v, &mut HashMap::new());
                        let ch = sh.chan_mut(h).expect("live: stale handles error above as closed");
                        ch.heap = ch_heap;
                        ch.queue.push_back(v2);
                        Self::wake_select_waiters(&mut sh, h); // M12.4: el canal ya tiene valor → listo para un select
                        self.cur.stack.push(HeapValue::Unit);
                    } else {
                        // (3) Cola llena (acotado) → BLOQUEAR al emisor (backpressure, M12.2). Guarda la
                        // fibra con el valor pendiente; al despertarla, `wake_sender` le deja unit (el
                        // resultado de `send`) en la pila y continúa tras el ChanSend.
                        let fiber = Self::take_current_fiber(&mut self.cur);
                        sh.parked.push(Parked { on: h, fiber, waiting: Waiting::Send(v) });
                        // M12.4: un emisor bloqueado vuelve al canal "listo" para un select (un recv lo
                        // tomaría); despierta a los selectores que lo esperan.
                        Self::wake_select_waiters(&mut sh, h);
                        sh.running -= 1; // este emisor se bloqueó → worker ocioso
                        drop(sh);
                        let (l, c2) = pos!();
                        if !self.poll_next(l, c2)? { self.stop = true; }
                    }
                }
                OpCode::ChanRecv => {
                    let h = self.pop_channel();
                    // M38.3b paso 2: lock-once (ver ChanSend).
                    let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                    // M98.3: un handle stale (canal liberado = cerrado y drenado) responde como
                    // cerrado y vacío → None ([]).
                    if sh.chan(h).is_none() {
                        drop(sh);
                        let arr = self.cur.heap.allocate(Obj::Array(Vec::new()));
                        self.cur.stack.push(HeapValue::Obj(arr));
                        return Ok(None);
                    }
                    // (1) ¿Valor en la cola? Sácalo; al liberar un hueco, si hay un emisor bloqueado en este
                    // canal, su valor entra a la cola (ya hay sitio) y se le despierta.
                    let from_queue = sh.chan_mut(h).expect("just checked live").queue.pop_front();
                    if let Some(v) = from_queue {
                        // M38.1b-2: el valor viene del heap del canal → se transfiere al heap del receptor.
                        // Si la cola queda vacía, el heap del canal se limpia (nadie referencia sus objetos).
                        let ch = sh.chan_mut(h).expect("just checked live");
                        let ch_heap = std::mem::take(&mut ch.heap);
                        let v2 = transfer_value(&ch_heap, &mut self.cur.heap, &v, &mut HashMap::new());
                        let ch = sh.chan_mut(h).expect("just checked live");
                        if !ch.queue.is_empty() {
                            ch.heap = ch_heap; // aún hay valores en tránsito → conserva el heap
                        } // si no, `ch_heap` se descarta (limpieza)
                        // M98.3: este recv acaba de DRENAR un canal ya cerrado → libéralo (nadie puede
                        // volver a encolar: send sobre cerrado es error; los recv futuros ven el handle
                        // stale y devuelven None, indistinguible de cerrado+vacío).
                        if ch.closed && ch.queue.is_empty() {
                            sh.free_channel(h);
                        } else {
                            Self::wake_blocked_sender(&mut sh, h);
                        }
                        let arr = self.cur.heap.allocate(Obj::Array(vec![v2]));
                        self.cur.stack.push(HeapValue::Obj(arr));
                        return Ok(None);
                    }
                    // (2) Cola vacía: ¿hay un emisor bloqueado? (canal lleno con cap > 0, o rendezvous
                    // cap = 0). Toma su valor directo y despiértalo.
                    if let Some(pos) = sh.parked.iter().position(
                        |p| p.on == h && matches!(p.waiting, Waiting::Send(_)))
                    {
                        let parked = sh.parked.remove(pos);
                        let sv = match parked.waiting {
                            Waiting::Send(sv) => sv,
                            _ => unreachable!(),
                        };
                        // M38.1b-2: el valor del emisor bloqueado vive en el heap de SU fibra (aparcada) →
                        // se transfiere al heap del receptor antes de despertar al emisor.
                        let sv2 = transfer_value(&parked.fiber.heap, &mut self.cur.heap, &sv, &mut HashMap::new());
                        Self::wake_sender(&mut sh, parked.fiber);
                        let arr = self.cur.heap.allocate(Obj::Array(vec![sv2]));
                        self.cur.stack.push(HeapValue::Obj(arr));
                        return Ok(None);
                    }
                    // (3) Cola vacía y sin emisores: cerrado → None ([]); abierto → bloquear (Recv).
                    let closed = sh.chan(h).expect("just checked live").closed;
                    if closed {
                        // M98.3: cerrado y vacío → liberable (los recv futuros ven stale → mismo None).
                        sh.free_channel(h);
                        let arr = self.cur.heap.allocate(Obj::Array(Vec::new()));
                        self.cur.stack.push(HeapValue::Obj(arr));
                    } else {
                        // Bloquear: guardar la fibra actual (ip ya apunta tras el ChanRecv → al
                        // despertarla, el `wake_recv` le deja el `[T]` en la pila y continúa) y conmutar.
                        let fiber = Self::take_current_fiber(&mut self.cur);
                        sh.parked.push(Parked { on: h, fiber, waiting: Waiting::Recv });
                        sh.running -= 1; // este receptor se bloqueó → worker ocioso
                        drop(sh);
                        let (l, c2) = pos!();
                        if !self.poll_next(l, c2)? { self.stop = true; }
                    }
                }
                OpCode::TaskJoin => {
                    // Une una tarea (M12.3): si terminó, su valor; si falló, re-lanza; si pendiente, bloquea.
                    // M38.3b paso 3: UN solo guard a través de leer-estado + aparcar (bajo M:N real, leer
                    // Pending y luego aparcar en dos locks separados perdería el wake si la tarea completa en
                    // medio → cuelgue).
                    let t = self.pop_task();
                    let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                    // M98.1: `join` CONSUME la tarea (libera su slot y el heap del resultado). Un handle
                    // stale (ya consumida por otro join/try_join, o por el cierre de su scope) → error.
                    let outcome = match sh.task(t) {
                        None => {
                            drop(sh);
                            return Err(runtime_error(pos!().0, pos!().1, TASK_CONSUMED));
                        }
                        Some(vt) => match &vt.state {
                            TaskState::Done(v) => Some(Ok(v.clone())),
                            TaskState::Failed(msg) => Some(Err(msg.clone())),
                            TaskState::Pending => None,
                        },
                    };
                    match outcome {
                        Some(Ok(v)) => {
                            // M38.1b-2: el valor de Done vive en el heap de la tarea → al heap del que
                            // la une. M98.1: se consume el slot; su heap se suelta al salir del bloque.
                            let vt = sh.take_task(t).expect("just read as Done");
                            let v2 = transfer_value(&vt.heap, &mut self.cur.heap, &v, &mut HashMap::new());
                            drop(sh);
                            self.push(v2);
                        }
                        Some(Err(msg)) => {
                            sh.take_task(t); // consumida también al re-lanzar
                            drop(sh);
                            return Err(runtime_error(pos!().0, pos!().1, &msg));
                        }
                        None => {
                            // Bloquear: re-empuja el id (lo sacamos arriba) y rebobina el ip al
                            // TaskJoin, para que al despertar (con la tarea ya Done/Failed) lo re-ejecute.
                            self.cur.stack.push(HeapValue::Task(t)); // no `self.push` (guard sostenido)
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            sh.parked.push(Parked { on: t, fiber, waiting: Waiting::Join });
                            sh.running -= 1;
                            drop(sh);
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                OpCode::TaskFailed => {
                    // __task_failed (M56.5): espera a que la tarea termine y empuja `[]` (bien) o
                    // `[msg]` (falló) — el fallo como VALOR, sin re-lanzar. Es la base de `try_join`
                    // del prelude (que reusa `join` para el valor, ya sin bloquear). Mismo esquema de
                    // guard único + park que TaskJoin.
                    let t = self.pop_task();
                    let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                    let outcome = match sh.task(t) {
                        None => {
                            drop(sh);
                            return Err(runtime_error(pos!().0, pos!().1, TASK_CONSUMED));
                        }
                        Some(vt) => match &vt.state {
                            TaskState::Done(_) => Some(None),
                            TaskState::Failed(msg) => Some(Some(msg.clone())),
                            TaskState::Pending => None,
                        },
                    };
                    match outcome {
                        Some(failed) => {
                            // M97.1/M98.1: observar un fallo lo CONSUME (libera el slot) → queda
                            // manejado: el ScopeEnd del scope dueño lo salta (handle stale), ni
                            // cancela hermanas ni re-lanza. En Done NO se consume: el envoltorio
                            // `try_join` del prelude hace `join(t)` a continuación para el valor.
                            if failed.is_some() {
                                sh.take_task(t);
                            }
                            drop(sh);
                            let elems = match failed {
                                None => Vec::new(),
                                Some(msg) => vec![HeapValue::Str(msg)],
                            };
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        None => {
                            // Bloquear hasta que termine, como TaskJoin (re-empuja el handle y rebobina).
                            self.cur.stack.push(HeapValue::Task(t)); // no `self.push` (guard sostenido)
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            sh.parked.push(Parked { on: t, fiber, waiting: Waiting::Join });
                            sh.running -= 1;
                            drop(sh);
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                OpCode::ScopeBegin => {
                    // Abre un scope (M12.3): las tareas spawneadas mientras esté activo se le adscriben.
                    self.cur.scopes.push(ScopeFrame { children: Vec::new() });
                }
                OpCode::ScopeEnd => {
                    // Cierra el scope: el valor del cuerpo (R) ya está en la pila.
                    // M38.3b paso 3: UN solo guard a través de comprobar-fallo/pendiente + aparcar (como
                    // TaskJoin: evita perder el wake si una hija completa entre el chequeo y el park).
                    let children: Vec<usize> =
                        self.cur.scopes.last().expect("ScopeEnd without ScopeBegin").children.clone();
                    let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                    // (1) ¿Alguna hija FALLÓ? Cancela a las hermanas que sigan pendientes y propaga el fallo
                    // ORIGINAL de inmediato, sin esperar a las demás (M12.5: cancelación de hermanas).
                    // M97.1/M98.1: un fallo ya OBSERVADO con `try_join` fue CONSUMIDO (slot liberado) →
                    // el handle es stale y `sh.task` da None → se salta (manejado, ni cancela ni re-lanza).
                    let failure = children.iter().find_map(|&c| match sh.task(c).map(|vt| &vt.state) {
                        Some(TaskState::Failed(msg)) => Some(msg.clone()),
                        _ => None,
                    });
                    if let Some(msg) = failure {
                        for &c in &children {
                            Self::cancel_task(&mut sh, c); // ignora las no-pendientes (la que falló, las Done)
                        }
                        // M98.1: el scope CONSUME a sus hijas al cerrar (también en el camino de fallo;
                        // las canceladas-aún-corriendo escriben luego sobre un handle stale y se ignora).
                        for &c in &children {
                            sh.take_task(c);
                        }
                        drop(sh);
                        self.cur.scopes.pop();
                        return Err(runtime_error(pos!().0, pos!().1, &msg));
                    }
                    // (2) ¿Alguna pendiente? Rebobina a ScopeEnd y bloquéate (al despertar re-escanea).
                    let pending = children.iter().copied().find(|&c|
                        matches!(sh.task(c).map(|vt| &vt.state), Some(TaskState::Pending)));
                    if let Some(c) = pending {
                        self.cur.frames.last_mut().unwrap().ip -= 1;
                        let fiber = Self::take_current_fiber(&mut self.cur);
                        sh.parked.push(Parked { on: c, fiber, waiting: Waiting::Join });
                        sh.running -= 1;
                        drop(sh);
                        let (l, c2) = pos!();
                        if !self.poll_next(l, c2)? { self.stop = true; }
                    } else {
                        // (3) Todas terminaron con éxito: desapila el scope. M98.1: el scope es el DUEÑO
                        // de sus hijas → consume las que nadie unió (fire-and-forget), liberando su slot
                        // y el heap del resultado descartado. Un `join` posterior sobre un handle que
                        // escapó del scope da el error TASK_CONSUMED (las hijas no sobreviven al scope).
                        for &c in &children {
                            sh.take_task(c);
                        }
                        drop(sh);
                        self.cur.scopes.pop();
                    }
                }
                OpCode::Select => {
                    // Espera a que algún canal de la lista esté listo para recibir; devuelve su índice
                    // (el menor, determinista). Si ninguno lo está, bloquea esperando al conjunto (M12.4).
                    let arr = self.pop_obj();
                    let chans: Vec<usize> = match self.cur.heap.get(arr) {
                        Obj::Array(elems) => elems.iter().filter_map(|v| match v {
                            HeapValue::Channel(id) => Some(*id),
                            _ => None,
                        }).collect(),
                        _ => unreachable!("the checker guarantees an array of channels"),
                    };
                    // M38.3b paso 3: un ÚNICO guard sostenido a través del escaneo Y el park. Es reentrante-
                    // seguro (un solo lock, sin re-entrar `self.sched()`) y —clave bajo M:N real— atómico:
                    // escanear "ninguno listo" y aparcar deben ser indivisibles, o un canal que se vuelve
                    // listo entre medias dispararía `wake_select_waiters` antes de que estemos aparcados →
                    // wake perdido → cuelgue.
                    let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                    let mut ready_idx = None;
                    for (i, &c) in chans.iter().enumerate() {
                        // M98.3: un handle stale (canal liberado) responde como cerrado → listo,
                        // igual que el canal cerrado de siempre (gotcha documentado de select).
                        let buffered_or_closed = match sh.chan(c) {
                            Some(ch) => !ch.queue.is_empty() || ch.closed,
                            None => true,
                        };
                        let has_sender = sh.parked.iter()
                            .any(|p| p.on == c && matches!(p.waiting, Waiting::Send(_)));
                        if buffered_or_closed || has_sender {
                            ready_idx = Some(i);
                            break;
                        }
                    }
                    match ready_idx {
                        Some(i) => {
                            drop(sh);
                            self.push(HeapValue::Int(i as i64));
                        }
                        None => {
                            // Ninguno listo: re-empuja el arreglo (lo sacamos arriba), rebobina el ip al
                            // Select y aparca esperando al conjunto (el handle del arreglo va en `on`).
                            self.cur.stack.push(HeapValue::Obj(arr)); // no `self.push` (guard sostenido)
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            sh.parked.push(Parked { on: arr, fiber, waiting: Waiting::Select });
                            sh.running -= 1;
                            drop(sh);
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }

                // --- Stdlib de string (M11.1) ---
                OpCode::ToString => {
                    // Representación textual (la misma que `print`): coincide con el `Display`
                    // que usa el intérprete en `to_string`.
                    let v = self.pop();
                    let s = format_value(&self.cur.heap, &self.program.enums, &v);
                    self.push(HeapValue::Str(s));
                }
                OpCode::ConcatN(n) => {
                    // V2 (bench políglota): concatenación n-aria — un solo String con la capacidad
                    // EXACTA (frente a n−1 intermedios de la cadena de `Add`). Se opera sobre el
                    // tramo superior de la pila sin sacar los valores (cero Vec temporal).
                    let n = *n;
                    let start = self.cur.stack.len() - n;
                    let total: usize = self.cur.stack[start..].iter().map(|v| match v {
                        HeapValue::Str(s) => s.len(),
                        _ => unreachable!("the checker guarantees strings"),
                    }).sum();
                    let mut out = String::with_capacity(total);
                    for v in &self.cur.stack[start..] {
                        if let HeapValue::Str(s) = v { out.push_str(s); }
                    }
                    self.cur.stack.truncate(start);
                    self.push(HeapValue::Str(out));
                }
                OpCode::Trim => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Str(s.trim().to_string())),
                    _ => unreachable!("the checker guarantees a string"),
                },
                OpCode::Split => {
                    // El separador está encima del string (orden de los argumentos).
                    let sep = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(sep)) = (s, sep) else {
                        unreachable!("the checker guarantees two strings");
                    };
                    let parts: Vec<HeapValue> =
                        s.split(sep.as_str()).map(|p| HeapValue::Str(p.to_string())).collect();
                    // El arreglo es un objeto del heap; los Str son inline, sin handles que rootear.
                    let h = self.cur.heap.allocate(Obj::Array(parts));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Chars => {
                    let s = match self.pop() {
                        HeapValue::Str(s) => s,
                        _ => unreachable!("the checker guarantees a string"),
                    };
                    let cs: Vec<HeapValue> = s.chars().map(HeapValue::Char).collect();
                    // El arreglo es un objeto del heap; los Char son inline, sin handles que rootear.
                    let h = self.cur.heap.allocate(Obj::Array(cs));
                    self.push(HeapValue::Obj(h));
                }
                // M40.3a: el code point Unicode de un char → int.
                OpCode::CharCode => {
                    let c = match self.pop() {
                        HeapValue::Char(c) => c,
                        _ => unreachable!("the checker guarantees a char"),
                    };
                    self.push(HeapValue::Int(c as i64));
                }
                // M16.1b: los octetos UTF-8 del string → bytes (inline, no objeto del heap).
                OpCode::ToBytes => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Bytes(s.into_bytes())),
                    _ => unreachable!("the checker guarantees a string"),
                },
                // M89.2: guardia — la cripto de ring en un binario sin la feature 'net-tls'
                // ABORTA con un error claro (nunca un hash vacío ni una firma que falla en
                // silencio). El TLS no va aquí: ya es falible (Result) y degrada como Err-valor
                // desde su stub (el programa puede hacer fallback, como con sqlite). Con la
                // feature activa el guard es false constante y el compilador elimina el brazo.
                OpCode::CryptoRandomBytes | OpCode::Sha256 | OpCode::Sha512 | OpCode::Sha1
                | OpCode::HmacSha256 | OpCode::Ed25519PublicKey | OpCode::Ed25519Sign
                | OpCode::Ed25519Verify | OpCode::ChaChaPolySeal | OpCode::ChaChaPolyOpen
                    if !crate::builtins::net_tls_available() =>
                {
                    let (l, c) = pos!();
                    return Err(runtime_error(l, c, crate::builtins::NET_TLS_UNAVAILABLE));
                }
                // M43: hashes de producción vía `ring` (helpers compartidos con el intérprete).
                // M68.2: aleatoriedad criptográfica (CSPRNG del SO).
                OpCode::CryptoRandomBytes => match self.pop() {
                    HeapValue::Int(n) => self.push(HeapValue::Bytes(crate::builtins::crypto_random_bytes(n).into())),
                    _ => unreachable!("the checker guarantees an int"),
                },
                OpCode::Sha256 => match self.pop() {
                    HeapValue::Bytes(b) => self.push(HeapValue::Bytes(crate::builtins::sha256(&b))),
                    _ => unreachable!("the checker guarantees bytes"),
                },
                OpCode::Sha512 => match self.pop() {
                    HeapValue::Bytes(b) => self.push(HeapValue::Bytes(crate::builtins::sha512(&b))),
                    _ => unreachable!("the checker guarantees bytes"),
                },
                OpCode::Sha1 => match self.pop() {
                    HeapValue::Bytes(b) => self.push(HeapValue::Bytes(crate::builtins::sha1(&b))),
                    _ => unreachable!("the checker guarantees bytes"),
                },
                OpCode::HmacSha256 => {
                    let m = self.pop();
                    let k = self.pop();
                    let (HeapValue::Bytes(k), HeapValue::Bytes(m)) = (k, m) else {
                        unreachable!("the checker guarantees bytes, bytes");
                    };
                    self.push(HeapValue::Bytes(crate::builtins::hmac_sha256(&k, &m)));
                }
                // M43.3: Ed25519. Los fallibles empujan `[bytes]` etiquetado; `verify` empuja un bool.
                OpCode::Ed25519PublicKey => {
                    let seed = match self.pop() {
                        HeapValue::Bytes(b) => b,
                        _ => unreachable!("the checker guarantees bytes"),
                    };
                    let elems = match crate::builtins::ed25519_public_key(&seed) {
                        Some(pk) => vec![HeapValue::Bytes(pk)],
                        None => vec![],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Ed25519Sign => {
                    let msg = self.pop();
                    let seed = self.pop();
                    let (HeapValue::Bytes(seed), HeapValue::Bytes(msg)) = (seed, msg) else {
                        unreachable!("the checker guarantees bytes, bytes");
                    };
                    let elems = match crate::builtins::ed25519_sign(&seed, &msg) {
                        Some(sig) => vec![HeapValue::Bytes(sig)],
                        None => vec![],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Ed25519Verify => {
                    let sig = self.pop();
                    let msg = self.pop();
                    let pk = self.pop();
                    let (HeapValue::Bytes(pk), HeapValue::Bytes(msg), HeapValue::Bytes(sig)) = (pk, msg, sig) else {
                        unreachable!("the checker guarantees bytes, bytes, bytes");
                    };
                    self.push(HeapValue::Bool(crate::builtins::ed25519_verify(&pk, &msg, &sig)));
                }
                // M43.4: ChaCha20-Poly1305 AEAD. Pop en orden inverso (dato, aad, nonce, clave).
                op @ (OpCode::ChaChaPolySeal | OpCode::ChaChaPolyOpen) => {
                    let data = self.pop();
                    let aad = self.pop();
                    let nonce = self.pop();
                    let key = self.pop();
                    let (HeapValue::Bytes(key), HeapValue::Bytes(nonce), HeapValue::Bytes(aad), HeapValue::Bytes(data)) =
                        (key, nonce, aad, data)
                    else {
                        unreachable!("the checker guarantees four bytes");
                    };
                    let res = if matches!(op, OpCode::ChaChaPolySeal) {
                        crate::builtins::chacha20poly1305_seal(&key, &nonce, &aad, &data)
                    } else {
                        crate::builtins::chacha20poly1305_open(&key, &nonce, &aad, &data)
                    };
                    let elems = match res {
                        Some(out) => vec![HeapValue::Bytes(out)],
                        None => vec![],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M16.1b: decodifica bytes como UTF-8 → arreglo etiquetado; el prelude → Result.
                OpCode::FromUtf8 => {
                    let b = match self.pop() {
                        HeapValue::Bytes(b) => b,
                        _ => unreachable!("the checker guarantees bytes"),
                    };
                    let elems = match String::from_utf8(b) {
                        Ok(s) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(s)],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }

                // --- I/O binaria (M16.1c) ---
                // Lecturas → [bytes] etiquetado (tag en bytes para que el arreglo sea homogéneo).
                OpCode::ReadFileBytes => {
                    let path = match self.pop() {
                        HeapValue::Str(s) => s,
                        _ => unreachable!("the checker guarantees a string"),
                    };
                    let elems = match crate::builtins::read_file_bytes(&path) {
                        Ok(data) => vec![HeapValue::Bytes(b"ok".to_vec()), HeapValue::Bytes(data)],
                        Err(e) => vec![HeapValue::Bytes(b"err".to_vec()), HeapValue::Bytes(e.to_string().into_bytes())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::WriteFileBytes => {
                    let data = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Bytes(data)) = (path, data) else {
                        unreachable!("the checker guarantees string, bytes");
                    };
                    let elems = match crate::builtins::write_file_bytes(&path, &data) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M16.1c: lectura binaria del socket; cede al scheduler en WouldBlock (como SocketRead, M15.5).
                OpCode::SocketReadBytes => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("the checker guarantees an int"),
                    };
                    // M19.4b: una conexión TLS se lee con su bomba no bloqueante (conduce el handshake/
                    // descifrado); si bloquearía leyendo del peer, se aparca la fibra en el fd subyacente,
                    // igual que un socket normal. El intérprete usa el camino bloqueante (no llega aquí).
                    if crate::builtins::is_tls_handle(handle) {
                        match crate::builtins::tls_read_nb(handle) {
                            Ok(Some(data)) => {
                                let elems = vec![HeapValue::Bytes(b"ok".to_vec()), HeapValue::Bytes(data)];
                                let h = self.cur.heap.allocate(Obj::Array(elems));
                                self.push(HeapValue::Obj(h));
                            }
                            Err(e) => {
                                let elems = vec![HeapValue::Bytes(b"err".to_vec()), HeapValue::Bytes(e.into_bytes())];
                                let h = self.cur.heap.allocate(Obj::Array(elems));
                                self.push(HeapValue::Obj(h));
                            }
                            Ok(None) => {
                                self.push(HeapValue::Int(handle));
                                self.cur.frames.last_mut().unwrap().ip -= 1;
                                let fiber = Self::take_current_fiber(&mut self.cur);
                                let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
                                {
                                    let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                                    sh.io_parked.push(IoParked { fd, fiber, pending_write: None, handle, deadline: crate::builtins::read_deadline(handle) });
                                    sh.running -= 1; // aparcada por E/S → worker ocioso
                                }
                                let (l, c2) = pos!();
                                if !self.poll_next(l, c2)? { self.stop = true; }
                            }
                        }
                        return Ok(None);
                    }
                    match crate::builtins::socket_read_bytes_nb(handle) {
                        Ok(Some(data)) => {
                            let elems = vec![HeapValue::Bytes(b"ok".to_vec()), HeapValue::Bytes(data)];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Bytes(b"err".to_vec()), HeapValue::Bytes(e.into_bytes())];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(None) => {
                            self.push(HeapValue::Int(handle));
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            // M17: guarda el fd del socket para que el scheduler lo registre en el poller.
                            let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
                            {
                                let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                                sh.io_parked.push(IoParked { fd, fiber, pending_write: None, handle, deadline: crate::builtins::read_deadline(handle) });
                                sh.running -= 1; // aparcada por E/S → worker ocioso
                            }
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                OpCode::SocketWriteBytes => {
                    let data = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Bytes(data)) = (handle, data) else {
                        unreachable!("the checker guarantees int, bytes");
                    };
                    if crate::builtins::is_tls_handle(handle) {
                        // M19.4b: las escrituras TLS cifran por su propia bomba (busy-spin en el raro bloqueo).
                        let elems = match crate::builtins::tls_write_nb(handle, &data) {
                            Ok(_) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(String::new())],
                            Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                        };
                        let h = self.cur.heap.allocate(Obj::Array(elems));
                        self.push(HeapValue::Obj(h));
                    } else {
                        // TCP plano: escritura parcial; si el buffer se llena, CEDE la fibra hasta que el
                        // socket sea escribible (en vez de girar) → no acapara el hilo del scheduler.
                        match crate::builtins::socket_write_nb(handle, &data) {
                            Ok(n) if n == data.len() => {
                                let elems = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(String::new())];
                                let h = self.cur.heap.allocate(Obj::Array(elems));
                                self.push(HeapValue::Obj(h));
                            }
                            Ok(n) => {
                                let (line, col) = pos!();
                                self.park_write(handle, data[n..].to_vec(), line, col)?;
                                return Ok(None);
                            }
                            Err(e) => {
                                let elems = vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)];
                                let h = self.cur.heap.allocate(Obj::Array(elems));
                                self.push(HeapValue::Obj(h));
                            }
                        }
                    }
                }
                OpCode::Contains => {
                    // El valor buscado está encima del contenedor (orden de los argumentos).
                    let x = self.pop();
                    let cont = self.pop();
                    let res = match (&cont, &x) {
                        (HeapValue::Str(s), HeapValue::Str(sub)) => s.contains(sub.as_str()),
                        // M11.7b: arreglo → pertenencia por igualdad estructural.
                        (HeapValue::Obj(h), _) => {
                            self.cur.heap.degrade_int_array(*h); // M98.5 (préstamo inmutable después)
                            match self.cur.heap.get(*h) {
                                Obj::Array(v) => v.iter().any(|e| values_equal(&self.cur.heap, e, &x)),
                                _ => unreachable!("the checker guarantees an array"),
                            }
                        }
                        _ => unreachable!("the checker guarantees string+string or array+element"),
                    };
                    self.push(HeapValue::Bool(res));
                }
                OpCode::Replace => {
                    // Orden de los argumentos en la pila: s, from, a → se sacan en orden inverso.
                    let a = self.pop();
                    let from = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(from), HeapValue::Str(a)) = (s, from, a) else {
                        unreachable!("the checker guarantees three strings");
                    };
                    self.push(HeapValue::Str(s.replace(from.as_str(), a.as_str())));
                }

                // --- Más string (M11.7a) ---
                OpCode::StartsWith => {
                    let p = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(p)) = (s, p) else {
                        unreachable!("the checker guarantees two strings");
                    };
                    self.push(HeapValue::Bool(s.starts_with(p.as_str())));
                }
                OpCode::EndsWith => {
                    let p = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(p)) = (s, p) else {
                        unreachable!("the checker guarantees two strings");
                    };
                    self.push(HeapValue::Bool(s.ends_with(p.as_str())));
                }
                OpCode::ToUpper => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Str(s.to_uppercase())),
                    _ => unreachable!("the checker guarantees a string"),
                },
                OpCode::ToLower => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Str(s.to_lowercase())),
                    _ => unreachable!("the checker guarantees a string"),
                },
                OpCode::Substring => {
                    // Orden en la pila: s, i, j → se sacan en inverso.
                    let j = self.pop();
                    let i = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Int(i), HeapValue::Int(j)) = (s, i, j) else {
                        unreachable!("the checker guarantees string, int, int");
                    };
                    self.push(HeapValue::Str(crate::builtins::substring_chars(&s, i, j)));
                }
                // M19.2: sub-secuencia de bytes por octeto (con clamp). Orden en la pila: b, i, j.
                OpCode::SubBytes => {
                    let j = self.pop();
                    let i = self.pop();
                    let b = self.pop();
                    let (HeapValue::Bytes(b), HeapValue::Int(i), HeapValue::Int(j)) = (b, i, j) else {
                        unreachable!("the checker guarantees bytes, int, int");
                    };
                    self.push(HeapValue::Bytes(crate::builtins::sub_bytes_octets(&b, i, j)));
                }
                // M19.3c: construye bytes a partir de un [int] (objeto del heap), truncando a octeto.
                OpCode::BytesOf => {
                    let HeapValue::Obj(h) = self.pop() else {
                        unreachable!("the checker guarantees an array");
                    };
                    let octets: Vec<u8> = self.as_array(h).iter().map(|v| match v {
                        HeapValue::Int(n) => (*n & 0xff) as u8,
                        _ => unreachable!("the checker guarantees [int]"),
                    }).collect();
                    self.push(HeapValue::Bytes(octets));
                }
                OpCode::Repeat => {
                    let n = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Int(n)) = (s, n) else {
                        unreachable!("the checker guarantees string, int");
                    };
                    self.push(HeapValue::Str(crate::builtins::repeat_str(&s, n)));
                }
                OpCode::IndexOf => {
                    // Primitivo: [] o [i] (índice de carácter). El prelude → Option<int>.
                    let sub = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(sub)) = (s, sub) else {
                        unreachable!("the checker guarantees two strings");
                    };
                    let elems = match crate::builtins::char_index_of(&s, &sub) {
                        Some(i) => vec![HeapValue::Int(i as i64)],
                        None => vec![],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Join => {
                    // Orden en la pila: arr, sep → se saca sep primero.
                    let sep = self.pop();
                    let arr = self.pop();
                    let (HeapValue::Obj(h), HeapValue::Str(sep)) = (arr, sep) else {
                        unreachable!("the checker guarantees [string], string");
                    };
                    // V1 (bench políglota): unir SIN clonar cada elemento. Antes: `Vec<String>` intermedio
                    // (un clon por elemento, con el arreglo aún vivo → en jsonserialize ~27 MB de pico
                    // transitorio) + `join`. Ahora: suma de longitudes → un `String` preasignado exacto →
                    // escribir los `&str` directo del heap. Mismo resultado, N clones menos y −14% de pico
                    // medido en jsonserialize (88→76 MB).
                    let out = {
                        let elems = self.as_array(h);
                        let total: usize = elems.iter().map(|v| match v {
                            HeapValue::Str(s) => s.len(),
                            _ => unreachable!("the checker guarantees [string]"),
                        }).sum::<usize>() + sep.len() * elems.len().saturating_sub(1);
                        let mut out = String::with_capacity(total);
                        for (i, v) in elems.iter().enumerate() {
                            if i > 0 { out.push_str(sep.as_str()); }
                            if let HeapValue::Str(s) = v { out.push_str(s); }
                        }
                        out
                    };
                    self.push(HeapValue::Str(out));
                }

                // --- Más arreglos (M11.7b) ---
                OpCode::Reverse => {
                    let h = self.pop_obj();
                    let mut elems = self.as_array(h).clone();
                    elems.reverse();
                    let nh = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(nh));
                }
                OpCode::ArrayPop => {
                    // Muta el arreglo quitando el último; devuelve [] o [x]. Prelude → Option<T>.
                    let h = self.pop_obj();
                    // M98.5: camino nativo del IntArray.
                    let popped = if let Obj::IntArray(xs) = self.cur.heap.get_mut(h) {
                        xs.pop().map(HeapValue::Int)
                    } else {
                        self.as_array_mut(h).pop()
                    };
                    let elems = popped.map(|v| vec![v]).unwrap_or_default();
                    let nh = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(nh));
                }
                OpCode::Position => {
                    let x = self.pop();
                    let h = self.pop_obj();
                    self.cur.heap.degrade_int_array(h); // M98.5 (préstamo inmutable después)
                    let idx = match self.cur.heap.get(h) {
                        Obj::Array(v) => v.iter().position(|e| values_equal(&self.cur.heap, e, &x)),
                        _ => unreachable!("the checker guarantees an array"),
                    };
                    let elems = idx.map(|i| vec![HeapValue::Int(i as i64)]).unwrap_or_default();
                    let nh = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(nh));
                }

                // --- I/O y API de runtime (M11.2) ---
                // M13.2a: aborta con el mensaje en la posición de la llamada (igual que el
                // intérprete, que lo intercepta en `eval_call`). El mensaje es el string en la cima.
                OpCode::Panic => {
                    let msg = match self.pop() {
                        HeapValue::Str(s) => s,
                        _ => unreachable!("the checker guarantees a string"),
                    };
                    return Err(runtime_error(pos!().0, pos!().1, &msg));
                }
                OpCode::EPrint => {
                    let v = self.pop();
                    crate::host_eprint(&format_value(&self.cur.heap, &self.program.enums, &v));
                    self.push(HeapValue::Unit);
                }
                OpCode::ParseInt => {
                    // Primitivo: [] o [n]; el prelude lo envuelve en Option<int>.
                    let elems = match self.pop() {
                        HeapValue::Str(s) => match s.trim().parse::<i64>() {
                            Ok(n) => vec![HeapValue::Int(n)],
                            Err(_) => vec![],
                        },
                        _ => unreachable!("the checker guarantees a string"),
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ParseFloat => {
                    // M14: [] o [f]; el prelude lo envuelve en Option<float>.
                    let elems = match self.pop() {
                        HeapValue::Str(s) => match s.trim().parse::<f64>() {
                            Ok(f) => vec![HeapValue::Float(f)],
                            Err(_) => vec![],
                        },
                        _ => unreachable!("the checker guarantees a string"),
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ReadLine => {
                    // Primitivo: [] en EOF, [linea] si no (sin el '\n'). El prelude → Option<string>.
                    let mut line = String::new();
                    let elems = match std::io::stdin().read_line(&mut line) {
                        Ok(0) | Err(_) => vec![],
                        Ok(_) => vec![HeapValue::Str(line.trim_end_matches(['\n', '\r']).to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Env => {
                    // Primitivo: [] si no existe, [valor] si sí. El prelude → Option<string>.
                    let elems = match self.pop() {
                        HeapValue::Str(name) => match std::env::var(name.as_str()) {
                            Ok(v) => vec![HeapValue::Str(v)],
                            Err(_) => vec![],
                        },
                        _ => unreachable!("the checker guarantees a string"),
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Args => {
                    // Argumentos del programa (del almacén de proceso); arreglo de strings.
                    let items: Vec<HeapValue> = crate::runtime::program_args()
                        .iter()
                        .map(|a| HeapValue::Str(a.clone()))
                        .collect();
                    let h = self.cur.heap.allocate(Obj::Array(items));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ReadFile => {
                    // Arreglo etiquetado ["ok", contenido] o ["err", msg]. El prelude → Result.
                    let elems = match self.pop() {
                        HeapValue::Str(path) => match std::fs::read_to_string(path.as_str()) {
                            Ok(c) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(c)],
                            Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                        },
                        _ => unreachable!("the checker guarantees a string"),
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::WriteFile => {
                    // El contenido está encima de la ruta (orden de los argumentos).
                    let contents = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Str(contents)) = (path, contents) else {
                        unreachable!("the checker guarantees two strings");
                    };
                    let elems = match std::fs::write(path.as_str(), contents.as_str()) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Exists => match self.pop() {
                    HeapValue::Str(path) => self.push(HeapValue::Bool(std::path::Path::new(path.as_str()).exists())),
                    _ => unreachable!("the checker guarantees a string"),
                },
                OpCode::AppendFile => {
                    // El contenido está encima de la ruta (orden de los argumentos).
                    let contents = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Str(contents)) = (path, contents) else {
                        unreachable!("the checker guarantees two strings");
                    };
                    let elems = match crate::builtins::append_to_file(path.as_str(), contents.as_str()) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::RemoveFile => {
                    let path = match self.pop() {
                        HeapValue::Str(p) => p,
                        _ => unreachable!("the checker guarantees a string"),
                    };
                    let elems = match std::fs::remove_file(&path) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M67: operaciones de fs etiquetadas — un solo brazo parametrizado (como MathF);
                // saca op.argc() strings y delega en el helper compartido con el intérprete.
                OpCode::FsTagged(op) => {
                    let mut args = vec![String::new(); op.argc()];
                    for i in (0..op.argc()).rev() {
                        args[i] = match self.pop() {
                            HeapValue::Str(s) => s,
                            _ => unreachable!("the checker guarantees strings"),
                        };
                    }
                    let elems = crate::builtins::fs_tagged(*op, &args).into_iter().map(HeapValue::Str).collect();
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M67: tests totales de fs → bool (como Exists).
                OpCode::FsTest(t) => {
                    let path = match self.pop() {
                        HeapValue::Str(p) => p,
                        _ => unreachable!("the checker guarantees a string"),
                    };
                    self.push(HeapValue::Bool(crate::builtins::fs_test(*t, &path)));
                }
                // M67: append binario → ["ok"]/["err", msg].
                OpCode::AppendFileBytes => {
                    let data = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Bytes(data)) = (path, data) else {
                        unreachable!("the checker guarantees string, bytes");
                    };
                    let elems = match crate::builtins::append_bytes_to_file(&path, &data) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ListDir => {
                    let path = match self.pop() {
                        HeapValue::Str(p) => p,
                        _ => unreachable!("the checker guarantees a string"),
                    };
                    let elems = match crate::builtins::list_dir(&path) {
                        Ok(names) => {
                            let mut v = vec![HeapValue::Str("ok".to_string())];
                            v.extend(names.into_iter().map(HeapValue::Str));
                            v
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }

                // Diferido JSON-1: code point → [char] de 0/1 (vacío si inválido).
                OpCode::CharFromCode => {
                    let HeapValue::Int(n) = self.pop() else { unreachable!("the checker guarantees an int") };
                    // El guard de rango evita que un int enorme haga wrap al castear a u32.
                    let elems = if (0..=0x10FFFF).contains(&n) {
                        char::from_u32(n as u32).map(|c| vec![HeapValue::Char(c)]).unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }

                // --- Bits de float (M54.1): totales, sin heap. ---
                OpCode::FloatBits => {
                    let HeapValue::Float(f) = self.pop() else { unreachable!("the checker guarantees a float") };
                    self.push(HeapValue::Int(f.to_bits() as i64));
                }
                OpCode::FloatFromBits => {
                    let HeapValue::Int(n) = self.pop() else { unreachable!("the checker guarantees an int") };
                    self.push(HeapValue::Float(f64::from_bits(n as u64)));
                }

                // --- SQLite embebido (M53.3): arreglo etiquetado, como los primitivos de I/O. ---
                OpCode::SqliteOpen => {
                    let path = self.pop();
                    let HeapValue::Str(path) = path else {
                        unreachable!("the checker guarantees a string");
                    };
                    let elems = match crate::builtins::sqlite_open(&path) {
                        Ok(h) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::SqliteExec | OpCode::SqliteQuery => {
                    // Orden en la pila: handle, sql, params → se saca params primero.
                    let ps = self.pop();
                    let sql = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(sql), HeapValue::Obj(ph)) = (handle, sql, ps) else {
                        unreachable!("the checker guarantees int, string, [string]");
                    };
                    let params: Vec<String> = self.as_array(ph).iter().map(|v| match v {
                        HeapValue::Str(s) => s.clone(),
                        _ => unreachable!("the checker guarantees [string]"),
                    }).collect();
                    let elems = if matches!(instr, OpCode::SqliteExec) {
                        match crate::builtins::sqlite_exec(handle, &sql, &params) {
                            Ok(n) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(n.to_string())],
                            Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                        }
                    } else {
                        match crate::builtins::sqlite_query(handle, &sql, &params) {
                            Ok((ncols, cells)) => {
                                let mut v = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(ncols.to_string())];
                                v.extend(cells.into_iter().map(HeapValue::Str));
                                v
                            }
                            Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                        }
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }

                // --- I/O con buffering: handles de archivo (M11.8) ---
                OpCode::Open => {
                    let mode = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Str(mode)) = (path, mode) else {
                        unreachable!("the checker guarantees two strings");
                    };
                    let elems = match crate::builtins::open_file(&path, &mode) {
                        Ok(h) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ReadLineHandle => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("the checker guarantees an int"),
                    };
                    let elems = crate::builtins::read_line_handle(handle).map(|l| vec![HeapValue::Str(l)]).unwrap_or_default();
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::WriteHandle => {
                    let s = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(s)) = (handle, s) else {
                        unreachable!("the checker guarantees int, string");
                    };
                    let elems = match crate::builtins::write_handle(handle, &s) {
                        Ok(_) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // --- Cliente TCP (M15.2): arreglo etiquetado en el heap; el prelude → Result. ---
                OpCode::TcpConnect => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("the checker guarantees string, int");
                    };
                    let elems = match crate::builtins::tcp_connect(&host, port) {
                        Ok(h) => {
                            // M15.5: la VM usa sockets NO bloqueantes → socket_read cede al scheduler.
                            let _ = crate::builtins::set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M20.8: UDP. Bloqueante en ambos motores por ahora (la cesión cooperativa queda diferida).
                OpCode::UdpBind => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("the checker guarantees string, int");
                    };
                    let elems = match crate::builtins::udp_bind(&host, port) {
                        Ok(h) => {
                            // M20.11: la VM usa el socket NO bloqueante → udp_recv_from cede al scheduler.
                            let _ = crate::builtins::set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::UdpSendTo => {
                    let data = self.pop();
                    let port = self.pop();
                    let host = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(host), HeapValue::Int(port), HeapValue::Bytes(data)) =
                        (handle, host, port, data)
                    else {
                        unreachable!("the checker guarantees int, string, int, bytes");
                    };
                    let elems = match crate::builtins::udp_send_to(handle, &host, port, &data) {
                        Ok(n) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(n.to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M20.11: recv UDP no bloqueante; cede la fibra al scheduler en WouldBlock (como SocketReadBytes).
                OpCode::UdpRecvFrom => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("the checker guarantees an int"),
                    };
                    match crate::builtins::udp_recv_from_nb(handle) {
                        Ok(Some((host, port, data))) => {
                            let elems = vec![
                                HeapValue::Bytes(b"ok".to_vec()),
                                HeapValue::Bytes(host.into_bytes()),
                                HeapValue::Bytes(port.to_string().into_bytes()),
                                HeapValue::Bytes(data),
                            ];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Bytes(b"err".to_vec()), HeapValue::Bytes(e.into_bytes())];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(None) => {
                            // No hay datagrama: re-ejecuta el opcode al despertar (re-empuja el handle).
                            self.push(HeapValue::Int(handle));
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
                            {
                                let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                                sh.io_parked.push(IoParked { fd, fiber, pending_write: None, handle, deadline: crate::builtins::read_deadline(handle) });
                                sh.running -= 1; // aparcada por E/S → worker ocioso
                            }
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                // M19.4b: conexión TLS de cliente. La VM pone el socket en no bloqueante para que el I/O
                // TLS (SocketReadBytes) pueda ceder la fibra, como con un socket plano.
                OpCode::TlsConnect => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("the checker guarantees string, int");
                    };
                    let elems = match crate::builtins::tls_connect(&host, port) {
                        Ok(h) => {
                            let _ = crate::builtins::tls_set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M31.2a: conexión TLS con ALPN h2 (el handshake ya se completó de forma bloqueante en el
                // builtin; tras él se pone no bloqueante para el framing con cesión de fibras).
                OpCode::TlsConnectH2 => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("the checker guarantees string, int");
                    };
                    let elems = match crate::builtins::tls_connect_h2(&host, port) {
                        Ok(h) => {
                            let _ = crate::builtins::tls_set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M19.4b: envuelve un socket aceptado (ya no bloqueante) en una sesión TLS de servidor.
                // El handshake lo conduce el primer SocketReadBytes, cediendo la fibra si bloquea.
                OpCode::TlsAccept => {
                    let key = self.pop();
                    let cert = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(cert), HeapValue::Str(key)) = (handle, cert, key) else {
                        unreachable!("the checker guarantees int, string, string");
                    };
                    let elems = match crate::builtins::tls_accept(handle, &cert, &key) {
                        Ok(h) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // Diferido TLS: STARTTLS de cliente — envuelve un TCP plano (ya no bloqueante en la
                // VM) en una sesión TLS de cliente; el handshake lo conduce el primer I/O, cediendo.
                OpCode::TlsUpgrade => {
                    let host = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(host)) = (handle, host) else {
                        unreachable!("the checker guarantees int, string");
                    };
                    let elems = match crate::builtins::tls_upgrade(handle, &host) {
                        Ok(h) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::SocketRead => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("the checker guarantees an int"),
                    };
                    // M15.5: lectura no bloqueante. WouldBlock (Ok(None)) → aparcar la fibra y reintentar.
                    match crate::builtins::socket_read_nb(handle) {
                        Ok(Some(s)) => {
                            let elems = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(s)];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(None) => {
                            // Re-empuja el handle y rebobina al SocketRead: al despertar lo re-ejecuta.
                            self.push(HeapValue::Int(handle));
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            // M17: guarda el fd del socket para que el scheduler lo registre en el poller.
                            let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
                            {
                                let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                                sh.io_parked.push(IoParked { fd, fiber, pending_write: None, handle, deadline: crate::builtins::read_deadline(handle) });
                                sh.running -= 1; // aparcada por E/S → worker ocioso
                            }
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                OpCode::SocketWrite => {
                    let s = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(s)) = (handle, s) else {
                        unreachable!("the checker guarantees int, string");
                    };
                    // Cesión en `socket_write` (como SocketWriteBytes): escritura parcial de los octetos
                    // UTF-8; si el buffer se llena, cede la fibra (el resto pendiente vive en bytes, lo que
                    // evita reconstruir un string roto a mitad de carácter multibyte).
                    let bytes = s.into_bytes();
                    match crate::builtins::socket_write_nb(handle, &bytes) {
                        Ok(n) if n == bytes.len() => {
                            let elems = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(String::new())];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(n) => {
                            let (line, col) = pos!();
                            self.park_write(handle, bytes[n..].to_vec(), line, col)?;
                            return Ok(None);
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                    }
                }
                // --- Servidor TCP (M15.3) ---
                OpCode::TcpListen => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("the checker guarantees string, int");
                    };
                    let elems = match crate::builtins::tcp_listen(&host, port) {
                        Ok(h) => {
                            // M15.5: escucha NO bloqueante → tcp_accept cede al scheduler.
                            let _ = crate::builtins::set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::TcpAccept => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("the checker guarantees an int"),
                    };
                    // M15.5: accept no bloqueante. WouldBlock (Ok(None)) → aparcar y reintentar.
                    match crate::builtins::tcp_accept_nb(handle) {
                        Ok(Some(c)) => {
                            let elems = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(c.to_string())];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(None) => {
                            self.push(HeapValue::Int(handle));
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            // M17: guarda el fd del socket para que el scheduler lo registre en el poller.
                            let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
                            {
                                let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                                sh.io_parked.push(IoParked { fd, fiber, pending_write: None, handle, deadline: crate::builtins::read_deadline(handle) });
                                sh.running -= 1; // aparcada por E/S → worker ocioso
                            }
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                OpCode::LocalPort => match self.pop() {
                    HeapValue::Int(h) => self.push(HeapValue::Int(crate::builtins::local_port(h))),
                    _ => unreachable!("the checker guarantees an int"),
                },
                // M56.4: timeout de lectura del socket. En la VM el efecto real lo aplica el
                // scheduler (deadline al aparcar la fibra); aquí solo se registra.
                OpCode::SocketSetReadTimeout => {
                    let ms = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Int(ms)) = (handle, ms) else {
                        unreachable!("the checker guarantees int, int");
                    };
                    crate::builtins::socket_set_read_timeout(handle, ms);
                    self.push(HeapValue::Unit);
                }
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
                        HeapValue::Channel(ch) => {
                            // M38.3b paso 2: un ÚNICO lock para todo el cierre. El `if ... && matches!(...)`
                            // hacía dos `self.sched()` en la misma condición con guards solapados → doble-lock
                            // del Mutex no reentrante = DEADLOCK cuando el canal tenía un receptor aparcado.
                            let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                            // M98.3: close sobre un handle stale (canal liberado = ya cerrado y drenado)
                            // es no-op, igual que el doble close de siempre (idempotente).
                            if sh.chan(ch).is_none() {
                                drop(sh);
                                self.cur.stack.push(HeapValue::Unit);
                                return Ok(None);
                            }
                            if sh.parked.iter().any(
                                |p| p.on == ch && matches!(p.waiting, Waiting::Send(_)))
                            {
                                return Err(runtime_error(pos!().0, pos!().1,
                                    "close on a channel with a blocked sender"));
                            }
                            sh.chan_mut(ch).expect("just checked live").closed = true;
                            let mut i = 0;
                            while i < sh.parked.len() {
                                if sh.parked[i].on == ch && matches!(sh.parked[i].waiting, Waiting::Recv) {
                                    let parked = sh.parked.remove(i);
                                    Self::wake_recv(&self.cur, &mut sh, parked.fiber, Vec::new());
                                } else {
                                    i += 1;
                                }
                            }
                            Self::wake_select_waiters(&mut sh, ch); // M12.4: un canal cerrado está "listo" para un select
                            // M98.3: cerrado y ya drenado (cola vacía) → liberable de inmediato; si aún
                            // hay valores en tránsito, lo libera el recv que lo vacíe.
                            if sh.chan(ch).expect("just checked live").queue.is_empty() {
                                sh.free_channel(ch);
                            }
                            self.cur.stack.push(HeapValue::Unit);
                        }
                        _ => unreachable!("the checker guarantees a handle (int) or a Channel"),
                    }
                }

                // --- Matemáticas (M15.1a) ---
                // Una sola rama para las 10 funciones float -> float; delega en el helper compartido
                // con el intérprete (mismo cálculo → oráculo cuadra, incl. NaN/inf).
                OpCode::MathF(f) => match self.pop() {
                    HeapValue::Float(x) => self.push(HeapValue::Float(crate::builtins::apply_mathf(*f, x))),
                    _ => unreachable!("the checker guarantees a float"),
                },
                OpCode::Pow => {
                    let exp = self.pop();
                    let base = self.pop();
                    let (HeapValue::Float(base), HeapValue::Float(exp)) = (base, exp) else {
                        unreachable!("the checker guarantees two floats");
                    };
                    self.push(HeapValue::Float(base.powf(exp)));
                }
                // M65.2: atan2(y, x) — mismo f64::atan2 que el intérprete → el oráculo cuadra.
                OpCode::Atan2 => {
                    let x = self.pop();
                    let y = self.pop();
                    let (HeapValue::Float(y), HeapValue::Float(x)) = (y, x) else {
                        unreachable!("the checker guarantees two floats");
                    };
                    self.push(HeapValue::Float(y.atan2(x)));
                }
                // M49.1b: abs/min/max/pi/e ya no tienen opcode (funciones puras en `std/math`).

                // --- Reloj y aleatoriedad (M15.1b): delegan en los helpers compartidos. ---
                OpCode::Now => self.push(HeapValue::Int(crate::builtins::now_millis())),
                OpCode::Monotonic => self.push(HeapValue::Int(crate::builtins::monotonic_millis())),
                OpCode::Sleep => match self.pop() {
                    HeapValue::Int(ms) => {
                        // M57.2: dormir es COOPERATIVO — la fibra se aparca con un deadline sin fd
                        // (la maquinaria de M56.4) y las demás siguen corriendo. Antes:
                        // `thread::sleep` bloqueaba el worker entero (en M:1, todas las fibras).
                        // El resultado (unit) se empuja ANTES de aparcar y el ip NO se rebobina:
                        // al despertar, la fibra continúa tras el sleep (no lo re-ejecuta).
                        self.push(HeapValue::Unit);
                        if ms > 0 {
                            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms as u64);
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            {
                                let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                                sh.io_parked.push(IoParked { fd: -1, fiber, pending_write: None, handle: -1, deadline: Some(deadline) });
                                sh.running -= 1; // aparcada durmiendo → worker ocioso
                            }
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                    _ => unreachable!("the checker guarantees an int"),
                },
                OpCode::Random => self.push(HeapValue::Float(crate::builtins::random_f64())),
                OpCode::RandomInt => match self.pop() {
                    HeapValue::Int(n) => self.push(HeapValue::Int(crate::builtins::random_int(n))),
                    _ => unreachable!("the checker guarantees an int"),
                },
                // M68.1: fija la semilla del PRNG (reproducibilidad).
                OpCode::RandomSeed => match self.pop() {
                    HeapValue::Int(n) => {
                        crate::builtins::random_seed(n);
                        self.push(HeapValue::Unit);
                    }
                    _ => unreachable!("the checker guarantees an int"),
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
                    let h = self.cur.heap.allocate(Obj::Struct(VmStruct { name: sname, fields }));
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
                    let h = self.cur.heap.allocate(Obj::Enum(VmEnum { enum_id: *enum_id, tag: *tag, payload }));
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
                    return Err(runtime_error(pos!().0, pos!().1, "no match branch matched (should not happen)"));
                }
                OpCode::GetField(name) => {
                    let h = self.pop_obj();
                    let v = self.as_struct(h).fields.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
                        .expect("the checker guarantees the field");
                    self.push(v);
                }
                OpCode::SetField(name) => {
                    let v = self.pop();
                    let h = self.pop_obj();
                    let s = self.as_struct_mut(h);
                    let slot = s.fields.iter_mut().find(|(n, _)| n == name).expect("the checker guarantees the field");
                    slot.1 = v;
                }

                OpCode::Call(idx, argc) => {
                    if self.cur.frames.len() >= MAX_FRAMES {
                        return Err(runtime_error(pos!().0, pos!().1, "stack overflow (recursion too deep)"));
                    }
                    let mut locals = self.new_locals(*idx);
                    for i in (0..*argc).rev() {
                        let v = self.pop();
                        self.put_arg(&mut locals, i, v);
                    }
                    self.cur.frames.push(CallFrame {
                        function: *idx, ip: 0, locals, upvalues: Vec::new(), stack_base: self.cur.stack.len(),
                    });
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
                    self.cur.frames[fi].function = *idx;
                    self.cur.frames[fi].ip = 0;
                    let old = std::mem::replace(&mut self.cur.frames[fi].locals, locals);
                    self.recycle_locals(old); // Opt.2: la llamada en cola reemplaza las locales → recicla
                    self.cur.frames[fi].upvalues = Vec::new();
                }

                // --- Funciones de primera clase (M4.1) ---
                OpCode::Function(idx) => self.push(HeapValue::Function(*idx)),
                OpCode::CallValue(argc) => {
                    if self.cur.frames.len() >= MAX_FRAMES {
                        return Err(runtime_error(pos!().0, pos!().1, "stack overflow (recursion too deep)"));
                    }
                    let mut args_rev = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        args_rev.push(self.pop());
                    }
                    let (fn_idx, upvalues) = match self.pop() {
                        HeapValue::Function(i) => (i, Vec::new()),
                        HeapValue::Obj(h) => match self.cur.heap.get(h) {
                            Obj::Closure(c) => (c.index, c.upvalues.clone()),
                            _ => unreachable!("the checker guarantees a function"),
                        },
                        _ => unreachable!("the checker guarantees a function"),
                    };
                    let mut locals = self.new_locals(fn_idx);
                    for (j, val) in args_rev.into_iter().enumerate() {
                        self.put_arg(&mut locals, *argc - 1 - j, val);
                    }
                    self.cur.frames.push(CallFrame {
                        function: fn_idx, ip: 0, locals, upvalues, stack_base: self.cur.stack.len(),
                    });
                }
                // M13.3b: llamada indirecta en cola — reutiliza el marco actual.
                OpCode::TailCallValue(argc) => {
                    let mut args_rev = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        args_rev.push(self.pop());
                    }
                    let (fn_idx, upvalues) = match self.pop() {
                        HeapValue::Function(i) => (i, Vec::new()),
                        HeapValue::Obj(h) => match self.cur.heap.get(h) {
                            Obj::Closure(c) => (c.index, c.upvalues.clone()),
                            _ => unreachable!("the checker guarantees a function"),
                        },
                        _ => unreachable!("the checker guarantees a function"),
                    };
                    let mut locals = self.new_locals(fn_idx);
                    for (j, val) in args_rev.into_iter().enumerate() {
                        self.put_arg(&mut locals, *argc - 1 - j, val);
                    }
                    self.cur.frames[fi].function = fn_idx;
                    self.cur.frames[fi].ip = 0;
                    let old = std::mem::replace(&mut self.cur.frames[fi].locals, locals);
                    self.recycle_locals(old); // Opt.2
                    self.cur.frames[fi].upvalues = upvalues;
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
                            UpvalueSource::Local(slot) => match &self.cur.frames[fi].locals[slot] {
                                Local::Boxed(h) => *h,
                                Local::Plain(_) => unreachable!("a captured local must be boxed"),
                            },
                            UpvalueSource::Upvalue(u) => self.cur.frames[fi].upvalues[u],
                        };
                        upvalues.push(cell);
                    }
                    let h = self.cur.heap.allocate(Obj::Closure(VmClosure { index: *idx, upvalues }));
                    self.push(HeapValue::Obj(h));
                }

                OpCode::Return => {
                    let result = self.pop();
                    if let Some(frame) = self.cur.frames.pop() {
                        // El `Return` que baja `?` ocurre en mitad de una expresión: los operandos
                        // pendientes de este marco quedan por encima de su base y hay que descartarlos,
                        // o desalinean los argumentos de la siguiente llamada del llamador (M64.1).
                        self.cur.stack.truncate(frame.stack_base);
                        self.recycle_locals(frame.locals); // Opt.2: el marco se descarta → recicla sus locales
                    }
                    if self.cur.frames.is_empty() {
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
                Err(mut e) => {
                    // M79: la traza de llamadas se compone AQUÍ, donde los marcos siguen
                    // intactos (el `Err` no los desenrolla) — coste cero en el camino
                    // caliente. `is_empty` respeta una traza ya adjunta (no hay hoy, pero
                    // es la misma disciplina que el intérprete).
                    if e.trace.is_empty() {
                        e.trace = Self::build_trace(&self.cur.frames, program, e.line, e.col);
                    }
                    // Propagación de fallos (M12.3): el error de la fibra HIJA en curso no aborta el
                    // programa; se captura en su `Task` (`Failed`) y se planifica la siguiente. Abortan los
                    // de `main` y los del scheduler (frames vacíos = la fibra ya se aparcó/terminó → el
                    // error es un deadlock, no un fallo de la fibra actual).
                    if self.cur.frames.is_empty() || self.cur.is_main {
                        return Err(e);
                    }
                    self.fail_current_fiber(e)?;
                }
            }
        }
    }

    /// M79: compone la traza de llamadas a partir de los marcos vivos de la fibra en
    /// curso. La entrada 0 es el marco más interno (su nombre + la posición del error);
    /// cada llamador aporta su nombre + la posición de su llamada en vuelo: su `ip`
    /// guardado ya apunta TRAS el `Call` (el avance por defecto ocurre antes de
    /// ejecutar), así que la llamada es `lines[ip - 1]`. Solo se llama al capturar un
    /// error — cero coste en el camino caliente.
    fn build_trace(
        frames: &[CallFrame],
        program: &CompiledProgram,
        err_line: usize,
        err_col: usize,
    ) -> Vec<crate::runtime::TraceFrame> {
        let n = frames.len();
        let mut trace = Vec::with_capacity(n);
        if n == 0 {
            return trace;
        }
        trace.push(crate::runtime::TraceFrame {
            name: program.functions[frames[n - 1].function].name.clone(),
            line: err_line,
            col: err_col,
        });
        for f in frames[..n - 1].iter().rev() {
            let (line, col) = f
                .ip
                .checked_sub(1)
                .and_then(|i| program.functions[f.function].chunk.lines.get(i).copied())
                .unwrap_or((0, 0));
            trace.push(crate::runtime::TraceFrame {
                name: program.functions[f.function].name.clone(),
                line,
                col,
            });
        }
        trace
    }

    // ----- Recolección de basura (mark-and-sweep) -----

    /// Recolecta: marca desde las raíces (pila + locales + upvalues de los marcos),
    /// propaga y barre. Solo se llama en puntos seguros del bucle.
    fn collect(&mut self) {
        // M37.1: cronometramos la pausa stop-the-world para medir el objetivo de M37 (pausas acotadas).
        // M44a: `Instant::now()` PANIQUEA en `wasm32-unknown-unknown` (sin reloj) → en wasm no se cronometra
        // (las stats del GC solo se imprimen con `RAYLANG_GC_STATS`, que no aplica en el playground).
        #[cfg(not(target_arch = "wasm32"))]
        let start = std::time::Instant::now();
        // Reunimos las raíces (handles) primero, para no tomar prestado `self.cur.stack`
        // y `self.cur.heap` a la vez. M12.1: además de la fibra en ejecución, rooteamos TODAS las fibras
        // (listas y bloqueadas) y los canales que esperan.
        // M38.1b-2: **heap-por-fibra**. Este `collect` recolecta SOLO el heap de la fibra en curso, cuyas
        // únicas raíces son sus propios marcos y pila (invariante de aislamiento: ningún objeto de este
        // heap lo alcanza otra fibra ni un canal/tarea — los valores que cruzan se TRANSFIEREN). Las demás
        // fibras tienen su propio heap (se recolecta cuando cada una corre); los valores en tránsito viven
        // en el heap del canal/tarea. Así la pausa la acota el tamaño del heap de una sola fibra.
        let mut roots: Vec<Handle> = Vec::new();
        gather_roots(&self.cur.frames, &self.cur.stack, &mut roots);

        for h in roots {
            self.cur.heap.mark(h);
        }
        self.cur.heap.trace();
        self.cur.heap.sweep();
        self.gc_count += 1;
        // M37.1: registra la pausa (una sola recolección stop-the-world). Solo fuera de wasm (ver arriba).
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dt = start.elapsed().as_nanos();
            self.gc_total_pause_ns += dt;
            if dt > self.gc_max_pause_ns {
                self.gc_max_pause_ns = dt;
            }
        }
    }

    /// M37.1: imprime las estadísticas de pausas del GC a stderr si `RAYLANG_GC_STATS` está en el entorno.
    /// Instrumentación de medición (arco B / M37): cuántas recolecciones y la pausa máxima/media.
    fn print_gc_stats_if_requested(&self) {
        if std::env::var_os("RAYLANG_GC_STATS").is_none() {
            return;
        }
        let media = if self.gc_count > 0 { self.gc_total_pause_ns / self.gc_count as u128 } else { 0 };
        eprintln!(
            "[gc] recolecciones={} pausa_max={:.3}ms pausa_media={:.3}ms total={:.3}ms",
            self.gc_count,
            self.gc_max_pause_ns as f64 / 1e6,
            media as f64 / 1e6,
            self.gc_total_pause_ns as f64 / 1e6,
        );
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
                let cell = self.cur.heap.allocate(Obj::Cell(HeapValue::Unit));
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
        match &self.cur.frames[fi].locals[slot] {
            Local::Plain(v) => v.clone(),
            Local::Boxed(h) => self.cell_get(*h),
        }
    }

    fn set_local(&mut self, fi: usize, slot: usize, v: HeapValue) {
        match &self.cur.frames[fi].locals[slot] {
            Local::Boxed(h) => {
                let h = *h;
                self.cell_set(h, v);
            }
            Local::Plain(_) => self.cur.frames[fi].locals[slot] = Local::Plain(v),
        }
    }

    fn cell_get(&self, h: Handle) -> HeapValue {
        match self.cur.heap.get(h) {
            Obj::Cell(v) => v.clone(),
            _ => unreachable!("expected a cell"),
        }
    }

    fn cell_set(&mut self, h: Handle, v: HeapValue) {
        match self.cur.heap.get_mut(h) {
            Obj::Cell(slot) => *slot = v,
            _ => unreachable!("expected a cell"),
        }
    }

    // ----- Acceso a objetos del heap -----

    // M98.5: los dos embudos DEGRADAN un `IntArray` a genérico antes de prestarlo — así toda
    // operación no especializada (contains/reverse/concat/…) sigue funcionando sin tocarla.
    // Las calientes (push/index/set/len/pop) manejan `IntArray` nativo y no pasan por aquí.
    fn as_array(&mut self, h: Handle) -> &Vec<HeapValue> {
        self.cur.heap.degrade_int_array(h);
        match self.cur.heap.get(h) {
            Obj::Array(v) => v,
            _ => unreachable!("the checker guarantees an array"),
        }
    }

    fn as_array_mut(&mut self, h: Handle) -> &mut Vec<HeapValue> {
        self.cur.heap.degrade_int_array(h);
        match self.cur.heap.get_mut(h) {
            Obj::Array(v) => v,
            _ => unreachable!("the checker guarantees an array"),
        }
    }

    fn as_struct(&self, h: Handle) -> &VmStruct {
        match self.cur.heap.get(h) {
            Obj::Struct(s) => s,
            _ => unreachable!("the checker guarantees a struct"),
        }
    }

    fn as_struct_mut(&mut self, h: Handle) -> &mut VmStruct {
        match self.cur.heap.get_mut(h) {
            Obj::Struct(s) => s,
            _ => unreachable!("the checker guarantees a struct"),
        }
    }

    fn as_enum(&self, h: Handle) -> &VmEnum {
        match self.cur.heap.get(h) {
            Obj::Enum(e) => e,
            _ => unreachable!("the checker guarantees an enum"),
        }
    }

    // ----- Pila de operandos -----

    fn push(&mut self, v: HeapValue) {
        self.cur.stack.push(v);
    }

    fn pop(&mut self) -> HeapValue {
        self.cur.stack.pop().expect("empty stack: malformed bytecode")
    }

    fn peek(&self) -> &HeapValue {
        self.cur.stack.last().expect("empty stack: malformed bytecode")
    }

    fn pop_int(&mut self) -> i64 {
        match self.pop() {
            HeapValue::Int(n) => n,
            _ => unreachable!("the checker guarantees an int"),
        }
    }

    fn pop_obj(&mut self) -> Handle {
        match self.pop() {
            HeapValue::Obj(h) => h,
            _ => unreachable!("the checker guarantees an object"),
        }
    }

    /// M38.1b: saca un id de canal (el checker garantiza un `Channel<T>`).
    fn pop_channel(&mut self) -> usize {
        match self.pop() {
            HeapValue::Channel(id) => id,
            _ => unreachable!("the checker guarantees a channel"),
        }
    }

    /// M38.1b: saca un id de tarea (el checker garantiza un `Task<T>`).
    fn pop_task(&mut self) -> usize {
        match self.pop() {
            HeapValue::Task(id) => id,
            _ => unreachable!("the checker guarantees a task"),
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
            Equal => return Ok(Bool(values_equal(&self.cur.heap, &left, &right))),
            NotEqual => return Ok(Bool(!values_equal(&self.cur.heap, &left, &right))),
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
            // Desbordamiento de int = error de ejecución (M34, SPEC §8), como en el intérprete.
            (Add, Int(a), Int(b)) => Int(a.checked_add(b).ok_or_else(|| runtime_error(line, col, "arithmetic overflow on int"))?),
            (Sub, Int(a), Int(b)) => Int(a.checked_sub(b).ok_or_else(|| runtime_error(line, col, "arithmetic overflow on int"))?),
            (Mul, Int(a), Int(b)) => Int(a.checked_mul(b).ok_or_else(|| runtime_error(line, col, "arithmetic overflow on int"))?),
            (Div, Int(a), Int(b)) => {
                if b == 0 {
                    return Err(runtime_error(line, col, "integer division by zero"));
                }
                Int(a.checked_div(b).ok_or_else(|| runtime_error(line, col, "arithmetic overflow on int"))?)
            }
            (Rem, Int(a), Int(b)) => {
                if b == 0 {
                    return Err(runtime_error(line, col, "modulo by zero"));
                }
                Int(a.checked_rem(b).ok_or_else(|| runtime_error(line, col, "arithmetic overflow on int"))?)
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
            // M28.3: enteros sin signo con tamaño. Mismo ancho garantizado por el checker; wrapping
            // dentro del ancho (`crate::runtime::uint_mask`), idéntico al intérprete.
            (Add, UInt(a, w), UInt(b, _)) => uint_heap(a.wrapping_add(b), w),
            (Sub, UInt(a, w), UInt(b, _)) => uint_heap(a.wrapping_sub(b), w),
            (Mul, UInt(a, w), UInt(b, _)) => uint_heap(a.wrapping_mul(b), w),
            (Div, UInt(a, w), UInt(b, _)) => {
                if b == 0 { return Err(runtime_error(line, col, "integer division by zero")); }
                uint_heap(a / b, w)
            }
            (Rem, UInt(a, w), UInt(b, _)) => {
                if b == 0 { return Err(runtime_error(line, col, "modulo by zero")); }
                uint_heap(a % b, w)
            }
            (Less, UInt(a, _), UInt(b, _)) => Bool(a < b),
            (LessEqual, UInt(a, _), UInt(b, _)) => Bool(a <= b),
            (Greater, UInt(a, _), UInt(b, _)) => Bool(a > b),
            (GreaterEqual, UInt(a, _), UInt(b, _)) => Bool(a >= b),
            (BitAnd, UInt(a, w), UInt(b, _)) => uint_heap(a & b, w),
            (BitOr, UInt(a, w), UInt(b, _)) => uint_heap(a | b, w),
            (BitXor, UInt(a, w), UInt(b, _)) => uint_heap(a ^ b, w),
            (Shl, UInt(a, w), UInt(b, _)) => uint_heap(a.wrapping_shl(b as u32), w),
            (Shr, UInt(a, w), UInt(b, _)) => uint_heap(a.wrapping_shr(b as u32), w),
            _ => unreachable!("operator/operand combination that the checker should have rejected"),
        })
    }
}

fn runtime_error(line: usize, col: usize, msg: &str) -> RuntimeError {
    RuntimeError { msg: msg.to_string(), line, col, trace: Vec::new() }
}

/// Localiza la variante `variant` del enum `Option` del prelude en la tabla compilada, devolviendo
/// `(enum_id, tag)` para armar un `VmEnum`. Lo usa el retorno FFI `char*` → `Option` (M41.3).
fn option_variant(enums: &[crate::bytecode::CompiledEnum], variant: &str) -> Option<(usize, usize)> {
    let ei = enums.iter().position(|e| e.name == "Option")?;
    let tag = enums[ei].variants.iter().position(|v| v.name == variant)?;
    Some((ei, tag))
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
// M38.1b-2: `gather_fiber_roots` se eliminó — con heap-por-fibra, `collect` solo rootea el heap de la
// fibra en curso (sus marcos/pila); las demás fibras recolectan su propio heap cuando corren.

/// Comprueba que `i` es un índice válido en `0..len`; si no, error de ejecución.
fn bounds_check(i: i64, len: usize, line: usize, col: usize) -> Result<usize, RuntimeError> {
    if i < 0 || (i as usize) >= len {
        return Err(runtime_error(line, col, &format!("index {} out of range (length {})", i, len)));
    }
    Ok(i as usize)
}

#[cfg(test)]
mod tests;
