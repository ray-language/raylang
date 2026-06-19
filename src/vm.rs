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

use crate::bytecode::{Chunk, CompiledFn, CompiledProgram, OpCode};
use crate::interpreter::{RuntimeError, Value};

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
            chunk: chunk.clone(),
        }],
        main: 0,
    };
    run_program(&program)
}

struct CallFrame {
    function: usize,
    ip: usize,
    locals: Vec<Value>,
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
        let locals = vec![Value::Unit; self.program.functions[main].num_locals];
        self.frames.push(CallFrame { function: main, ip: 0, locals });

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
                    let v = self.frames[fi].locals[*slot].clone();
                    self.push(v);
                }
                OpCode::SetLocal(slot) => {
                    let v = self.pop();
                    self.frames[fi].locals[*slot] = v;
                }

                OpCode::Print => {
                    let v = self.pop();
                    println!("{}", v);
                    self.push(Value::Unit);
                }

                OpCode::Call(idx, argc) => {
                    if self.frames.len() >= MAX_FRAMES {
                        return Err(runtime_error(line, col, "desbordamiento de pila (recursión demasiado profunda)"));
                    }
                    // Los argumentos están en la cima: los movemos a las primeras
                    // locales del nuevo marco (param 0 = primer argumento).
                    let mut locals = vec![Value::Unit; self.program.functions[*idx].num_locals];
                    for i in (0..*argc).rev() {
                        locals[i] = self.pop();
                    }
                    self.frames.push(CallFrame { function: *idx, ip: 0, locals });
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

    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }

    fn pop(&mut self) -> Value {
        self.stack.pop().expect("pila vacía: bytecode mal formado")
    }

    fn peek(&self) -> &Value {
        self.stack.last().expect("pila vacía: bytecode mal formado")
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

fn runtime_error(line: usize, col: usize, msg: &str) -> RuntimeError {
    RuntimeError { msg: msg.to_string(), line, col }
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
}
