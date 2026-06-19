//! Compilador de raylang: AST → bytecode (M2).
//!
//! Recorre el AST (ya verificado por el checker) y *emite* instrucciones en un
//! `Chunk`. Es un recorrido en **post-orden**: para un nodo binario, primero
//! compila sus dos hijos (que dejan sus valores en la pila) y luego emite la
//! operación, que los consume. Ese orden es lo que hace que la VM de pila
//! funcione.
//!
//! M2.1 compila el subconjunto de **expresiones** sin control de flujo: literales,
//! aritmética, comparación y unarios. El control de flujo (`if`/`while`/`&&`/`||`),
//! las variables y las llamadas llegan en M2.2 y M2.3.
//!
//! Como el intérprete, el compilador asume que el AST ya pasó el checker: confía en
//! que los tipos son correctos y no re-verifica.

use crate::ast::{BinaryOp, Expr, ExprKind, UnaryOp};
use crate::bytecode::{Chunk, OpCode};
use crate::interpreter::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error de compilación en {}:{}: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for CompileError {}

/// Compila una expresión a un `Chunk` que, al ejecutarse, deja su valor en la cima
/// de la pila y termina con `Return`.
pub fn compile_expr(expr: &Expr) -> Result<Chunk, CompileError> {
    let mut chunk = Chunk::new();
    emit_expr(&mut chunk, expr)?;
    chunk.emit(OpCode::Return, expr.line, expr.col);
    Ok(chunk)
}

fn emit_expr(chunk: &mut Chunk, expr: &Expr) -> Result<(), CompileError> {
    let (line, col) = (expr.line, expr.col);
    match &expr.kind {
        // --- Literales: empujar una constante (o un opcode dedicado para bools) ---
        ExprKind::Int(v) => {
            let idx = chunk.add_constant(Value::Int(*v));
            chunk.emit(OpCode::Constant(idx), line, col);
        }
        ExprKind::Float(v) => {
            let idx = chunk.add_constant(Value::Float(*v));
            chunk.emit(OpCode::Constant(idx), line, col);
        }
        ExprKind::Str(s) => {
            let idx = chunk.add_constant(Value::Str(s.clone()));
            chunk.emit(OpCode::Constant(idx), line, col);
        }
        ExprKind::Bool(true) => {
            chunk.emit(OpCode::True, line, col);
        }
        ExprKind::Bool(false) => {
            chunk.emit(OpCode::False, line, col);
        }

        // --- Unarios: compila el operando, luego emite la operación ---
        ExprKind::Unary { op, expr: inner } => {
            emit_expr(chunk, inner)?;
            let opc = match op {
                UnaryOp::Neg => OpCode::Negate,
                UnaryOp::Not => OpCode::Not,
            };
            chunk.emit(opc, line, col);
        }

        // --- Binarios: compila izquierda, derecha, luego la operación ---
        ExprKind::Binary { op, left, right } => {
            // && y || hacen cortocircuito → necesitan saltos → M2.2.
            if matches!(op, BinaryOp::And | BinaryOp::Or) {
                return Err(error(line, col, "los operadores lógicos (&&/||) se compilan en M2.2 (requieren saltos)"));
            }
            emit_expr(chunk, left)?;
            emit_expr(chunk, right)?;
            let opc = match op {
                BinaryOp::Add => OpCode::Add,
                BinaryOp::Sub => OpCode::Sub,
                BinaryOp::Mul => OpCode::Mul,
                BinaryOp::Div => OpCode::Div,
                BinaryOp::Rem => OpCode::Rem,
                BinaryOp::Eq => OpCode::Equal,
                BinaryOp::Ne => OpCode::NotEqual,
                BinaryOp::Lt => OpCode::Less,
                BinaryOp::Le => OpCode::LessEqual,
                BinaryOp::Gt => OpCode::Greater,
                BinaryOp::Ge => OpCode::GreaterEqual,
                BinaryOp::And | BinaryOp::Or => unreachable!("cubiertos arriba"),
            };
            chunk.emit(opc, line, col);
        }

        // --- Lo que aún no se compila en M2.1 ---
        ExprKind::Ident(_) => {
            return Err(error(line, col, "las variables se compilan en M2.3"))
        }
        ExprKind::Call { .. } => {
            return Err(error(line, col, "las llamadas se compilan en M2.3"))
        }
        ExprKind::If { .. } | ExprKind::While { .. } | ExprKind::Block(_) => {
            return Err(error(line, col, "el control de flujo se compila en M2.2"))
        }
    }
    Ok(())
}

fn error(line: usize, col: usize, msg: &str) -> CompileError {
    CompileError { msg: msg.to_string(), line, col }
}
