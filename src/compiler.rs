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

use crate::ast::{BinaryOp, Block, Expr, ExprKind, Stmt, StmtKind, UnaryOp};
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

        // --- Lógicos: cortocircuito con saltos (M2.2) ---
        // `JumpIfFalse` ojea (no saca) la condición, así que el valor que decide
        // queda en la pila como resultado cuando hay cortocircuito.
        ExprKind::Binary { op: BinaryOp::And, left, right } => {
            // a && b:  si a es false, deja 'a' y salta al final; si no, descarta
            // 'a' y evalúa b (su valor manda).
            emit_expr(chunk, left)?;
            let jump = chunk.emit(OpCode::JumpIfFalse(0), line, col);
            chunk.emit(OpCode::Pop, line, col);
            emit_expr(chunk, right)?;
            patch_jump(chunk, jump, chunk.code.len());
        }
        ExprKind::Binary { op: BinaryOp::Or, left, right } => {
            // a || b:  si a es false, ve a evaluar b; si no, corto y deja 'a' (true).
            emit_expr(chunk, left)?;
            let to_else = chunk.emit(OpCode::JumpIfFalse(0), line, col);
            let to_end = chunk.emit(OpCode::Jump(0), line, col);
            patch_jump(chunk, to_else, chunk.code.len());
            chunk.emit(OpCode::Pop, line, col);
            emit_expr(chunk, right)?;
            patch_jump(chunk, to_end, chunk.code.len());
        }

        // --- Resto de binarios: compila izquierda, derecha, luego la operación ---
        ExprKind::Binary { op, left, right } => {
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

        // --- if como expresión (M2.2) ---
        ExprKind::If { cond, then_branch, else_branch } => {
            // [cond]
            // JumpIfFalse -> else
            // Pop                 (descarta la condición, rama then)
            // [then]
            // Jump -> end
            // else: Pop           (descarta la condición, rama else)
            //       [else] | Unit
            // end:
            emit_expr(chunk, cond)?;
            let to_else = chunk.emit(OpCode::JumpIfFalse(0), line, col);
            chunk.emit(OpCode::Pop, line, col);
            emit_block(chunk, then_branch)?;
            let to_end = chunk.emit(OpCode::Jump(0), line, col);

            patch_jump(chunk, to_else, chunk.code.len());
            chunk.emit(OpCode::Pop, line, col);
            match else_branch {
                Some(else_e) => emit_expr(chunk, else_e)?,
                None => {
                    chunk.emit(OpCode::Unit, line, col);
                }
            }
            patch_jump(chunk, to_end, chunk.code.len());
        }

        // --- bloque como expresión (M2.2) ---
        ExprKind::Block(b) => emit_block(chunk, b)?,

        // --- Lo que aún no se compila (M2.3) ---
        ExprKind::Ident(_) => return Err(error(line, col, "las variables se compilan en M2.3")),
        ExprKind::Call { .. } => return Err(error(line, col, "las llamadas se compilan en M2.3")),
        ExprKind::While { .. } => return Err(error(line, col, "el while se compila en M2.3 (necesita variables)")),
    }
    Ok(())
}

/// Compila un bloque: sus sentencias (que no dejan valor neto) y luego su valor
/// final (el `tail`, o `Unit` si no hay). Garantiza dejar **exactamente un valor**
/// en la pila, igual que cualquier expresión.
fn emit_block(chunk: &mut Chunk, block: &Block) -> Result<(), CompileError> {
    for stmt in &block.statements {
        emit_stmt(chunk, stmt)?;
    }
    match &block.tail {
        Some(e) => emit_expr(chunk, e)?,
        None => {
            chunk.emit(OpCode::Unit, block.line, block.col);
        }
    }
    Ok(())
}

/// Compila una sentencia. Las sentencias se ejecutan por su efecto: una
/// sentencia-de-expresión deja un valor que descartamos con `Pop`.
fn emit_stmt(chunk: &mut Chunk, stmt: &Stmt) -> Result<(), CompileError> {
    match &stmt.kind {
        StmtKind::Expr(e) => {
            emit_expr(chunk, e)?;
            chunk.emit(OpCode::Pop, stmt.line, stmt.col);
        }
        StmtKind::Let { .. } | StmtKind::Assign { .. } => {
            return Err(error(stmt.line, stmt.col, "las variables se compilan en M2.3"))
        }
        StmtKind::Return { .. } => {
            return Err(error(stmt.line, stmt.col, "return se compila en M2.3"))
        }
    }
    Ok(())
}

/// Parchea un salto ya emitido para que apunte a `target`. Es el "backpatching":
/// emitimos el salto con destino provisional y lo corregimos cuando sabemos a
/// dónde va.
fn patch_jump(chunk: &mut Chunk, at: usize, target: usize) {
    chunk.code[at] = match chunk.code[at] {
        OpCode::Jump(_) => OpCode::Jump(target),
        OpCode::JumpIfFalse(_) => OpCode::JumpIfFalse(target),
        _ => unreachable!("patch_jump sobre una instrucción que no es salto"),
    };
}

fn error(line: usize, col: usize, msg: &str) -> CompileError {
    CompileError { msg: msg.to_string(), line, col }
}
