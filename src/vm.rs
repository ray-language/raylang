//! La máquina virtual (VM) de raylang (M2).
//!
//! Ejecuta un `Chunk` de bytecode sobre una **pila de operandos**. El modelo es una
//! *stack machine*: las instrucciones sacan sus operandos de la cima de la pila y
//! empujan sus resultados. Por ejemplo, `1 + 2 * 3` compila a:
//!
//! ```text
//! Constant 1      ; pila: [1]
//! Constant 2      ; pila: [1, 2]
//! Constant 3      ; pila: [1, 2, 3]
//! Mul             ; pila: [1, 6]
//! Add             ; pila: [7]
//! Return          ; devuelve 7
//! ```
//!
//! M2.1 ejecuta código en línea recta (sin saltos), así que recorremos las
//! instrucciones en orden. En M2.2 introduciremos un *instruction pointer* (`ip`)
//! manipulable para los saltos.
//!
//! Como el compilador asume entrada verificada por el checker, las combinaciones de
//! tipos imposibles se marcan con `unreachable!` (serían un bug nuestro, no del
//! programa). Solo la división por cero es un error de ejecución legítimo.

use crate::bytecode::{Chunk, OpCode};
use crate::interpreter::{RuntimeError, Value};

/// Ejecuta un chunk y devuelve el valor que quede como resultado.
pub fn run(chunk: &Chunk) -> Result<Value, RuntimeError> {
    Vm::new().run(chunk)
}

struct Vm {
    stack: Vec<Value>,
}

impl Vm {
    fn new() -> Self {
        Vm { stack: Vec::new() }
    }

    fn run(&mut self, chunk: &Chunk) -> Result<Value, RuntimeError> {
        // El instruction pointer (ip) es un índice manipulable: los saltos lo
        // mueven. Por eso es un bucle `while` con `ip` explícito, no un `for`.
        let mut ip = 0;
        while ip < chunk.code.len() {
            let (line, col) = chunk.lines[ip];
            match &chunk.code[ip] {
                OpCode::Constant(idx) => self.push(chunk.constants[*idx].clone()),
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

                // Operadores binarios: sacan derecha y luego izquierda (la derecha
                // está en la cima porque se compiló al final).
                op @ (OpCode::Add
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
                    let result = apply_binary(op, left, right, line, col)?;
                    self.push(result);
                }

                // Saltos: mueven el ip. `continue` evita el `ip += 1` del final.
                OpCode::Jump(target) => {
                    ip = *target;
                    continue;
                }
                OpCode::JumpIfFalse(target) => {
                    // Ojea la condición sin sacarla (el compilador emite el Pop).
                    if matches!(self.peek(), Value::Bool(false)) {
                        ip = *target;
                        continue;
                    }
                }

                OpCode::Return => return Ok(self.pop()),
            }
            ip += 1;
        }
        // Si el chunk no terminó en Return (no debería ocurrir), devolvemos lo que
        // haya en la cima, o Unit.
        Ok(self.stack.pop().unwrap_or(Value::Unit))
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
        // Aritmética entera.
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
        // Aritmética flotante.
        (Add, Float(a), Float(b)) => Float(a + b),
        (Sub, Float(a), Float(b)) => Float(a - b),
        (Mul, Float(a), Float(b)) => Float(a * b),
        (Div, Float(a), Float(b)) => Float(a / b),
        (Rem, Float(a), Float(b)) => Float(a % b),
        // Orden.
        (Less, Int(a), Int(b)) => Bool(a < b),
        (LessEqual, Int(a), Int(b)) => Bool(a <= b),
        (Greater, Int(a), Int(b)) => Bool(a > b),
        (GreaterEqual, Int(a), Int(b)) => Bool(a >= b),
        (Less, Float(a), Float(b)) => Bool(a < b),
        (LessEqual, Float(a), Float(b)) => Bool(a <= b),
        (Greater, Float(a), Float(b)) => Bool(a > b),
        (GreaterEqual, Float(a), Float(b)) => Bool(a >= b),
        // Igualdad (mismo tipo, garantizado por el checker).
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
    use crate::compiler::compile_expr;

    /// Extrae el AST de una expresión suelta (la pone en posición de valor de un
    /// bloque y la saca). No corre el checker: solo necesitamos el árbol.
    fn expr_of(src: &str) -> Expr {
        let prog_src = format!("fn v() {{ {} }}", src);
        let tokens = crate::lexer::lex(&prog_src).expect("lex ok");
        let prog = crate::parser::parse(tokens).expect("parse ok");
        *prog.functions[0].body.tail.clone().expect("expresión en posición tail")
    }

    /// Compila y ejecuta una expresión en la VM.
    fn run_vm(src: &str) -> Value {
        let chunk = compile_expr(&expr_of(src)).expect("compila");
        run(&chunk).expect("ejecuta sin error")
    }

    /// **El oráculo**: para una expresión que devuelve int, compara el resultado de
    /// la VM con el del intérprete de M1. Deben coincidir.
    fn oracle_int(src: &str) {
        // Intérprete: lo envolvemos como retorno de main y lo ejecutamos.
        let prog_src = format!("fn main() -> int {{ {} }}", src);
        let tokens = crate::lexer::lex(&prog_src).expect("lex ok");
        let prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("intérprete ok");

        // VM: compilamos la misma expresión y la ejecutamos.
        let vm = run_vm(src);

        assert_eq!(interp, vm, "VM y intérprete difieren en `{}`", src);
    }

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
        assert_eq!(run_vm("3 != 5"), Value::Bool(true));
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
        let e = run(&chunk).unwrap_err();
        assert!(e.msg.contains("división"));
    }

    #[test]
    fn el_desensamblador_muestra_las_instrucciones() {
        let chunk = compile_expr(&expr_of("1 + 2")).unwrap();
        let text = chunk.disassemble("test");
        // Debe contener las dos constantes, el Add y el Return.
        assert!(text.contains("Constant"));
        assert!(text.contains("Add"));
        assert!(text.contains("Return"));
    }

    // ----- M2.2: control de flujo -----

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
        assert_eq!(run_vm("false || false"), Value::Bool(false));

        // Cortocircuito: la división por cero de la derecha NUNCA se ejecuta.
        assert_eq!(run_vm("false && (1 / 0 == 0)"), Value::Bool(false));
        assert_eq!(run_vm("true || (1 / 0 == 0)"), Value::Bool(true));
    }

    #[test]
    fn bloque_con_sentencias_y_valor_final() {
        // Las sentencias-de-expresión se ejecutan y descartan; el valor final manda.
        assert_eq!(run_vm("{ 1; 2; 3 }"), Value::Int(3));
        assert_eq!(run_vm("{ 1 + 1 }"), Value::Int(2));
        // Bloque sin valor final → unit.
        assert_eq!(run_vm("{ 1; }"), Value::Unit);
    }
}
