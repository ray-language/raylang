//! La máquina virtual (VM) de raylang (M2).
//!
//! Ejecuta bytecode sobre una **pila de operandos** y una **pila de marcos de
//! llamada** explícita (no la pila de Rust). Reificar los marcos así es lo que
//! mantiene abierta la puerta a la concurrencia (ver IDEAS.md §1).
//!
//! ## Modelo de ejecución
//!
//! - **Pila de operandos** (`stack`): valores temporales que las instrucciones
//!   consumen y producen. Es compartida por todos los marcos.
//! - **Pila de marcos** (`frames`): cada llamada empuja un `CallFrame` con su
//!   `ip` (instruction pointer) y su propio arreglo de **slots locales**.
//!
//! Una llamada (`Call`) saca los argumentos de la pila de operandos y los coloca
//! como las primeras locales del nuevo marco. Un `Return` saca el valor de
//! retorno, descarta el marco, y lo empuja a la pila para el llamador.

use std::cell::RefCell;
use std::rc::Rc;

use crate::bytecode::{Chunk, CompiledFn, CompiledProgram, OpCode, UpvalueSource};
use crate::interpreter::{Cell, Closure, RuntimeError, StructInstance, Value};

/// Límite de marcos para detectar recursión infinita en vez de colgarse.
const MAX_FRAMES: usize = 1024;

/// Ejecuta un programa compilado (empezando por `main`) y devuelve su resultado.
pub fn run_program(program: &CompiledProgram) -> Result<Value, RuntimeError> {
    Vm::new(program).run()
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
        main: 0,
    };
    run_program(&program)
}

/// Un slot local. Normalmente guarda el valor directamente (`Plain`); si la variable
/// es capturada por una closure, vive **boxeada** en una celda compartida (`Boxed`)
/// para que la closure y el dueño compartan la misma referencia (M4.2).
enum Local {
    Plain(Value),
    Boxed(Cell),
}

struct CallFrame {
    function: usize,
    ip: usize,
    locals: Vec<Local>,
    /// Upvalues de la closure en ejecución (vacío si no es una closure). (M4.2)
    upvalues: Vec<(String, Cell)>,
}

struct Vm<'a> {
    program: &'a CompiledProgram,
    frames: Vec<CallFrame>,
    stack: Vec<Value>,
}

impl<'a> Vm<'a> {
    fn new(program: &'a CompiledProgram) -> Self {
        Vm { program, frames: Vec::new(), stack: Vec::new() }
    }

    fn run(&mut self) -> Result<Value, RuntimeError> {
        // Marco inicial: main, con su arreglo de locales (sin argumentos).
        let main = self.program.main;
        let locals = self.new_locals(main);
        self.frames.push(CallFrame { function: main, ip: 0, locals, upvalues: Vec::new() });

        loop {
            let fi = self.frames.len() - 1;
            let func = self.frames[fi].function;
            let ip = self.frames[fi].ip;

            // Robustez: si se acabó el chunk sin Return (no debería), retorna unit.
            if ip >= self.program.functions[func].chunk.code.len() {
                self.frames.pop();
                if self.frames.is_empty() {
                    return Ok(Value::Unit);
                }
                self.stack.push(Value::Unit);
                continue;
            }

            // Clonamos la instrucción y su posición para soltar el préstamo de
            // `self.program` antes de mutar `self`.
            let op = self.program.functions[func].chunk.code[ip].clone();
            let (line, col) = self.program.functions[func].chunk.lines[ip];
            self.frames[fi].ip = ip + 1; // avance por defecto; los saltos lo cambian

            match &op {
                OpCode::Constant(idx) => {
                    let v = self.program.functions[func].chunk.constants[*idx].clone();
                    self.push(v);
                }
                OpCode::True => self.push(Value::Bool(true)),
                OpCode::False => self.push(Value::Bool(false)),
                OpCode::Unit => self.push(Value::Unit),
                OpCode::Pop => {
                    self.pop();
                }

                OpCode::Negate => {
                    let v = self.pop();
                    self.push(match v {
                        Value::Int(n) => Value::Int(-n),
                        Value::Float(x) => Value::Float(-x),
                        _ => unreachable!("el checker garantiza un número"),
                    });
                }
                OpCode::Not => {
                    let v = self.pop();
                    self.push(match v {
                        Value::Bool(b) => Value::Bool(!b),
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
                    let result = apply_binary(bin, left, right, line, col)?;
                    self.push(result);
                }

                OpCode::Jump(target) => {
                    self.frames[fi].ip = *target;
                }
                OpCode::JumpIfFalse(target) => {
                    if matches!(self.peek(), Value::Bool(false)) {
                        self.frames[fi].ip = *target;
                    }
                }

                OpCode::GetLocal(slot) => {
                    let v = get_slot(&self.frames[fi].locals, *slot);
                    self.push(v);
                }
                OpCode::SetLocal(slot) => {
                    let v = self.pop();
                    set_slot(&mut self.frames[fi].locals, *slot, v);
                }
                OpCode::InitLocal(slot) => {
                    // Declaración: si el slot está boxeado, estrena celda (shadowing
                    // seguro); si no, guarda el valor directamente.
                    let v = self.pop();
                    let boxed = self.program.functions[func].captured.get(*slot).copied().unwrap_or(false);
                    self.frames[fi].locals[*slot] = if boxed {
                        Local::Boxed(Rc::new(RefCell::new(v)))
                    } else {
                        Local::Plain(v)
                    };
                }
                OpCode::GetUpvalue(i) => {
                    let v = self.frames[fi].upvalues[*i].1.borrow().clone();
                    self.push(v);
                }
                OpCode::SetUpvalue(i) => {
                    let v = self.pop();
                    *self.frames[fi].upvalues[*i].1.borrow_mut() = v;
                }

                OpCode::Print => {
                    let v = self.pop();
                    println!("{}", v);
                    self.push(Value::Unit);
                }

                // --- Arreglos (M3) ---
                OpCode::MakeArray(n) => {
                    let mut elems = Vec::with_capacity(*n);
                    for _ in 0..*n {
                        elems.push(self.pop());
                    }
                    elems.reverse(); // se sacaron en orden inverso
                    self.push(Value::Array(Rc::new(RefCell::new(elems))));
                }
                OpCode::Index => {
                    let i = self.pop_int();
                    let rc = self.pop_array();
                    let len = rc.borrow().len();
                    let idx = bounds_check(i, len, line, col)?;
                    let v = rc.borrow()[idx].clone();
                    self.push(v);
                }
                OpCode::SetIndex => {
                    let v = self.pop();
                    let i = self.pop_int();
                    let rc = self.pop_array();
                    let len = rc.borrow().len();
                    let idx = bounds_check(i, len, line, col)?;
                    rc.borrow_mut()[idx] = v;
                }
                OpCode::Len => {
                    let rc = self.pop_array();
                    let len = rc.borrow().len() as i64;
                    self.push(Value::Int(len));
                }
                OpCode::Push => {
                    let v = self.pop();
                    let rc = self.pop_array();
                    rc.borrow_mut().push(v);
                    self.push(Value::Unit);
                }

                // --- Structs (M3.2) ---
                OpCode::MakeStruct(idx) => {
                    // Clonamos nombre y campos para soltar el préstamo de program.
                    let sname = self.program.structs[*idx].name.clone();
                    let field_names: Vec<String> = self.program.structs[*idx].fields.clone();
                    let mut values = Vec::with_capacity(field_names.len());
                    for _ in 0..field_names.len() {
                        values.push(self.pop());
                    }
                    values.reverse(); // orden de declaración
                    let fields: Vec<(String, Value)> = field_names.into_iter().zip(values).collect();
                    self.push(Value::Struct(Rc::new(RefCell::new(StructInstance { name: sname, fields }))));
                }
                OpCode::GetField(name) => {
                    let rc = self.pop_struct();
                    let v = rc.borrow().fields.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
                        .expect("el checker garantiza el campo");
                    self.push(v);
                }
                OpCode::SetField(name) => {
                    let v = self.pop();
                    let rc = self.pop_struct();
                    let mut s = rc.borrow_mut();
                    let slot = s.fields.iter_mut().find(|(n, _)| n == name).expect("el checker garantiza el campo");
                    slot.1 = v;
                }

                OpCode::Call(idx, argc) => {
                    if self.frames.len() >= MAX_FRAMES {
                        return Err(runtime_error(line, col, "desbordamiento de pila (recursión demasiado profunda)"));
                    }
                    // Los argumentos están en la cima: los movemos a las primeras
                    // locales del nuevo marco (param 0 = primer argumento).
                    let mut locals = self.new_locals(*idx);
                    for i in (0..*argc).rev() {
                        let v = self.pop();
                        set_slot(&mut locals, i, v);
                    }
                    self.frames.push(CallFrame { function: *idx, ip: 0, locals, upvalues: Vec::new() });
                }

                // --- Funciones de primera clase (M4.1) ---
                OpCode::Function(idx) => self.push(Value::Function(*idx)),
                OpCode::CallValue(argc) => {
                    if self.frames.len() >= MAX_FRAMES {
                        return Err(runtime_error(line, col, "desbordamiento de pila (recursión demasiado profunda)"));
                    }
                    // En la pila: [valor-función, arg0, ..., arg{argc-1}]. Sacamos
                    // primero los argumentos, y debajo queda el valor-función.
                    let mut args_rev = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        args_rev.push(self.pop());
                    }
                    let (fn_idx, upvalues) = match self.pop() {
                        Value::Function(i) => (i, Vec::new()),
                        Value::Closure(c) => (c.index, c.upvalues.clone()),
                        _ => unreachable!("el checker garantiza una función"),
                    };
                    let mut locals = self.new_locals(fn_idx);
                    // args_rev está en orden inverso: el último que se sacó es arg0.
                    for (j, val) in args_rev.into_iter().enumerate() {
                        set_slot(&mut locals, *argc - 1 - j, val);
                    }
                    self.frames.push(CallFrame { function: fn_idx, ip: 0, locals, upvalues });
                }

                // --- Closures (M4.2) ---
                OpCode::Closure(idx) => {
                    // Construimos el arreglo de upvalues tomando las celdas que indica
                    // la función, del marco actual (un local boxeado, o un upvalue
                    // propio para la captura transitiva).
                    let descs = self.program.functions[*idx].upvalues.clone();
                    let mut upvalues = Vec::with_capacity(descs.len());
                    for d in &descs {
                        let cell = match d.source {
                            UpvalueSource::Local(slot) => match &self.frames[fi].locals[slot] {
                                Local::Boxed(c) => c.clone(),
                                Local::Plain(_) => unreachable!("un local capturado debe estar boxeado"),
                            },
                            UpvalueSource::Upvalue(u) => self.frames[fi].upvalues[u].1.clone(),
                        };
                        upvalues.push((d.name.clone(), cell));
                    }
                    self.push(Value::Closure(Rc::new(Closure { index: *idx, upvalues })));
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

    /// Crea el arreglo de locales de un marco nuevo: cada slot capturado nace
    /// **boxeado** (en su celda), los demás como `Plain(Unit)`.
    fn new_locals(&self, fn_idx: usize) -> Vec<Local> {
        let cf = &self.program.functions[fn_idx];
        (0..cf.num_locals)
            .map(|s| {
                if cf.captured.get(s).copied().unwrap_or(false) {
                    Local::Boxed(Rc::new(RefCell::new(Value::Unit)))
                } else {
                    Local::Plain(Value::Unit)
                }
            })
            .collect()
    }

    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }

    fn pop(&mut self) -> Value {
        self.stack.pop().expect("pila vacía: bytecode mal formado")
    }

    fn peek(&self) -> &Value {
        self.stack.last().expect("pila vacía: bytecode mal formado")
    }

    fn pop_int(&mut self) -> i64 {
        match self.pop() {
            Value::Int(n) => n,
            _ => unreachable!("el checker garantiza un int"),
        }
    }

    fn pop_array(&mut self) -> Rc<RefCell<Vec<Value>>> {
        match self.pop() {
            Value::Array(rc) => rc,
            _ => unreachable!("el checker garantiza un arreglo"),
        }
    }

    fn pop_struct(&mut self) -> Rc<RefCell<StructInstance>> {
        match self.pop() {
            Value::Struct(rc) => rc,
            _ => unreachable!("el checker garantiza un struct"),
        }
    }
}

/// Aplica un operador binario. Misma semántica que el intérprete de M1 (esa es la
/// idea del oráculo: deben coincidir).
fn apply_binary(op: &OpCode, left: Value, right: Value, line: usize, col: usize) -> Result<Value, RuntimeError> {
    use OpCode::*;
    use Value::*;
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
        (Equal, a, b) => Bool(a == b),
        (NotEqual, a, b) => Bool(a != b),
        _ => unreachable!("combinación operador/operandos que el checker debió rechazar"),
    })
}

/// Lee un slot local, deshaciendo el boxing si lo hay.
fn get_slot(locals: &[Local], slot: usize) -> Value {
    match &locals[slot] {
        Local::Plain(v) => v.clone(),
        Local::Boxed(cell) => cell.borrow().clone(),
    }
}

/// Escribe un slot local **respetando** el boxing: si está boxeado, muta su celda
/// (visible para las closures que la capturaron); si no, reemplaza el valor.
fn set_slot(locals: &mut [Local], slot: usize, v: Value) {
    if let Local::Boxed(cell) = &locals[slot] {
        *cell.borrow_mut() = v;
    } else {
        locals[slot] = Local::Plain(v);
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
        let prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("intérprete ok");
        let vm = run_vm(src);
        assert_eq!(interp, vm, "VM y intérprete difieren en `{}`", src);
    }

    /// **El oráculo a nivel de programa completo**: compila y ejecuta el programa
    /// en la VM y en el intérprete, y exige que el resultado coincida.
    fn oracle_program(src: &str) {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("intérprete ok");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect("vm ok");
        assert_eq!(interp, vm, "VM y intérprete difieren");
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
        // print va a stdout; se compara el valor de retorno de main.
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
        // 'b = a' comparte el arreglo: mutar b se ve en a (aliasing).
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
        let prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&prog).unwrap();
        let compiled = compile_program(&prog).unwrap();
        assert!(run_program(&compiled).unwrap_err().msg.contains("fuera de rango"));
    }

    // ----- M3.2: structs -----

    #[test]
    fn structs_acceso_y_orden_de_campos() {
        oracle_program("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 3, y: 4 }; p.x + p.y }");
        // El orden del literal no afecta el resultado.
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
        // 'f' local (una función) tapa a la global 'f': la llamada es indirecta.
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
        // La celda 'n' sobrevive a 'contador' y persiste entre llamadas.
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
        // 'inc' y 'get' capturan la MISMA 'n': mutar por una se ve por la otra.
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
}
