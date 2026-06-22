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
use crate::gc::{Handle, Heap, HeapValue, Obj, VmClosure, VmEnum, VmStruct};
use crate::interpreter::{EnumInstance, RuntimeError, StructInstance, Value};

/// Límite de marcos para detectar recursión infinita en vez de colgarse.
const MAX_FRAMES: usize = 1024;

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

struct Vm<'a> {
    program: &'a CompiledProgram,
    frames: Vec<CallFrame>,
    stack: Vec<HeapValue>,
    heap: Heap,
}

impl<'a> Vm<'a> {
    fn new(program: &'a CompiledProgram) -> Self {
        Vm { program, frames: Vec::new(), stack: Vec::new(), heap: Heap::new() }
    }

    fn run(&mut self) -> Result<HeapValue, RuntimeError> {
        // Marco inicial: main, con su arreglo de locales (sin argumentos).
        let main = self.program.main;
        let locals = self.new_locals(main);
        self.frames.push(CallFrame { function: main, ip: 0, locals, upvalues: Vec::new() });

        loop {
            // --- Punto seguro del GC ---
            if self.heap.should_collect() {
                self.collect();
            }

            let fi = self.frames.len() - 1;
            let func = self.frames[fi].function;
            let ip = self.frames[fi].ip;

            // Robustez: si se acabó el chunk sin Return (no debería), retorna unit.
            if ip >= self.program.functions[func].chunk.code.len() {
                self.frames.pop();
                if self.frames.is_empty() {
                    return Ok(HeapValue::Unit);
                }
                self.stack.push(HeapValue::Unit);
                continue;
            }

            // Clonamos la instrucción y su posición para soltar el préstamo de
            // `self.program` antes de mutar `self`.
            let op = self.program.functions[func].chunk.code[ip].clone();
            let (line, col) = self.program.functions[func].chunk.lines[ip];
            self.frames[fi].ip = ip + 1; // avance por defecto; los saltos lo cambian

            match &op {
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
                    let result = self.apply_binary(bin, left, right, line, col)?;
                    self.push(result);
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
                    let h = self.pop_obj();
                    let idx = {
                        let arr = self.as_array(h);
                        bounds_check(i, arr.len(), line, col)?
                    };
                    let v = self.as_array(h)[idx].clone();
                    self.push(v);
                }
                OpCode::SetIndex => {
                    let v = self.pop();
                    let i = self.pop_int();
                    let h = self.pop_obj();
                    let idx = bounds_check(i, self.as_array(h).len(), line, col)?;
                    self.as_array_mut(h)[idx] = v;
                }
                OpCode::Len => {
                    let h = self.pop_obj();
                    let len = self.as_array(h).len() as i64;
                    self.push(HeapValue::Int(len));
                }
                OpCode::Push => {
                    let v = self.pop();
                    let h = self.pop_obj();
                    self.as_array_mut(h).push(v);
                    self.push(HeapValue::Unit);
                }

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
                    self.frames.pop();
                    if self.frames.is_empty() {
                        return Ok(result); // retornó main: fin del programa
                    }
                    self.push(result); // entregamos el valor al llamador
                }
            }
        }
    }

    // ----- Recolección de basura (mark-and-sweep) -----

    /// Recolecta: marca desde las raíces (pila + locales + upvalues de los marcos),
    /// propaga y barre. Solo se llama en puntos seguros del bucle.
    fn collect(&mut self) {
        // Reunimos las raíces (handles) primero, para no tomar prestado `self.stack`
        // y `self.heap` a la vez.
        let mut roots: Vec<Handle> = Vec::new();
        for v in &self.stack {
            if let Some(h) = v.handle() {
                roots.push(h);
            }
        }
        for frame in &self.frames {
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
        (0..n)
            .map(|s| {
                if self.program.functions[fn_idx].captured.get(s).copied().unwrap_or(false) {
                    Local::Boxed(self.heap.allocate(Obj::Cell(HeapValue::Unit)))
                } else {
                    Local::Plain(HeapValue::Unit)
                }
            })
            .collect()
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
        },
    }
}

fn runtime_error(line: usize, col: usize, msg: &str) -> RuntimeError {
    RuntimeError { msg: msg.to_string(), line, col }
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
}
