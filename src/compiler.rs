//! Compilador de raylang: AST → bytecode (M2).
//!
//! Recorre el AST (ya verificado por el checker) y *emite* instrucciones. Es un
//! recorrido en **post-orden**: para un nodo binario, primero compila sus hijos
//! (que dejan sus valores en la pila) y luego emite la operación, que los consume.
//!
//! ## Variables locales (M2.3)
//!
//! Cada variable se asigna a un **slot** dentro del marco de su función. El
//! compilador lleva la cuenta de los slots y resuelve cada nombre a su slot. A
//! diferencia de clox (que guarda las locales en la pila de operandos), aquí las
//! locales viven en un arreglo aparte por marco; la pila de operandos solo guarda
//! temporales. Es una simplificación didáctica: separa con claridad ambos roles.
//!
//! Como el intérprete, el compilador asume entrada verificada: confía en los tipos
//! y en que toda variable/función existe.

use std::collections::HashMap;

use crate::ast::*;
use crate::bytecode::{Chunk, CompiledFn, CompiledProgram, OpCode};
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

/// Compila un programa completo a bytecode.
pub fn compile_program(program: &Program) -> Result<CompiledProgram, CompileError> {
    // Pre-pasada: nombre de función → índice (igual que en el checker).
    let mut indices = HashMap::new();
    for (i, f) in program.functions.iter().enumerate() {
        indices.insert(f.name.clone(), i);
    }
    let main = *indices.get("main").expect("el checker garantiza 'main'");

    let mut functions = Vec::new();
    for f in &program.functions {
        functions.push(compile_function(f, &indices)?);
    }
    Ok(CompiledProgram { functions, main })
}

fn compile_function(f: &Function, indices: &HashMap<String, usize>) -> Result<CompiledFn, CompileError> {
    let mut c = FnCompiler::new(indices);
    // Los parámetros son las primeras locales (slots 0..arity).
    for p in &f.params {
        c.declare_local(&p.name);
    }
    // El cuerpo deja su valor (el retorno implícito) en la pila; lo retornamos.
    c.emit_block(&f.body)?;
    c.emit(OpCode::Return, f.line, f.col);

    Ok(CompiledFn {
        name: f.name.clone(),
        arity: f.params.len(),
        num_locals: c.max_slots,
        chunk: c.chunk,
    })
}

/// Compila una expresión suelta a un `Chunk` (sin variables ni llamadas). Se usa
/// en los tests de expresiones puras.
pub fn compile_expr(expr: &Expr) -> Result<Chunk, CompileError> {
    let empty = HashMap::new();
    let mut c = FnCompiler::new(&empty);
    c.emit_expr(expr)?;
    c.emit(OpCode::Return, expr.line, expr.col);
    Ok(c.chunk)
}

/// Una variable local activa: su nombre, su slot y la profundidad de ámbito.
struct Local {
    name: String,
    slot: usize,
    depth: usize,
}

/// Estado de compilación de una función.
struct FnCompiler<'a> {
    chunk: Chunk,
    locals: Vec<Local>,
    /// Próximo slot libre (crece al declarar, se recupera al cerrar un ámbito).
    next_slot: usize,
    /// Marca de agua: el mayor `next_slot` alcanzado = slots que necesita el marco.
    max_slots: usize,
    scope_depth: usize,
    indices: &'a HashMap<String, usize>,
}

impl<'a> FnCompiler<'a> {
    fn new(indices: &'a HashMap<String, usize>) -> Self {
        FnCompiler {
            chunk: Chunk::new(),
            locals: Vec::new(),
            next_slot: 0,
            max_slots: 0,
            scope_depth: 0,
            indices,
        }
    }

    fn emit(&mut self, op: OpCode, line: usize, col: usize) -> usize {
        self.chunk.emit(op, line, col)
    }

    // ----- Manejo de slots y ámbitos -----

    fn declare_local(&mut self, name: &str) -> usize {
        let slot = self.next_slot;
        self.next_slot += 1;
        if self.next_slot > self.max_slots {
            self.max_slots = self.next_slot;
        }
        self.locals.push(Local { name: name.to_string(), slot, depth: self.scope_depth });
        slot
    }

    fn resolve_local(&self, name: &str) -> Option<usize> {
        // De dentro hacia afuera, para que el shadowing funcione.
        self.locals.iter().rev().find(|l| l.name == name).map(|l| l.slot)
    }

    /// Abre un ámbito. Devuelve el `next_slot` a restaurar al cerrarlo.
    fn begin_scope(&mut self) -> usize {
        self.scope_depth += 1;
        self.next_slot
    }

    fn end_scope(&mut self, saved_slot: usize) {
        self.scope_depth -= 1;
        self.locals.retain(|l| l.depth <= self.scope_depth);
        // Recuperamos los slots del ámbito para reutilizarlos (los marcos
        // dimensionan según max_slots).
        self.next_slot = saved_slot;
    }

    /// Parchea un salto previamente emitido para que apunte al final actual.
    fn patch_jump(&mut self, at: usize) {
        let target = self.chunk.code.len();
        self.chunk.code[at] = match self.chunk.code[at] {
            OpCode::Jump(_) => OpCode::Jump(target),
            OpCode::JumpIfFalse(_) => OpCode::JumpIfFalse(target),
            _ => unreachable!("patch_jump sobre una instrucción que no es salto"),
        };
    }

    // ----- Emisión -----

    fn emit_block(&mut self, block: &Block) -> Result<(), CompileError> {
        let saved = self.begin_scope();
        for stmt in &block.statements {
            self.emit_stmt(stmt)?;
        }
        match &block.tail {
            Some(e) => self.emit_expr(e)?,
            None => {
                self.emit(OpCode::Unit, block.line, block.col);
            }
        }
        self.end_scope(saved);
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        let (line, col) = (stmt.line, stmt.col);
        match &stmt.kind {
            StmtKind::Let { name, value, .. } => {
                // Compilamos el inicializador ANTES de declarar (para que no se vea
                // a sí misma), luego guardamos en su slot.
                self.emit_expr(value)?;
                let slot = self.declare_local(name);
                self.emit(OpCode::SetLocal(slot), line, col);
            }
            StmtKind::Assign { target, value } => match &target.kind {
                // x = e  → guardar en el slot local.
                ExprKind::Ident(name) => {
                    self.emit_expr(value)?;
                    let slot = self.resolve_local(name).expect("el checker garantiza la variable");
                    self.emit(OpCode::SetLocal(slot), line, col);
                }
                // a[i] = e  → arreglo, índice, valor, SetIndex (consume los tres).
                ExprKind::Index { array, index } => {
                    self.emit_expr(array)?;
                    self.emit_expr(index)?;
                    self.emit_expr(value)?;
                    self.emit(OpCode::SetIndex, line, col);
                }
                _ => unreachable!("el checker garantiza un lvalue"),
            },
            StmtKind::Return { value } => {
                match value {
                    Some(e) => self.emit_expr(e)?,
                    None => {
                        self.emit(OpCode::Unit, line, col);
                    }
                }
                self.emit(OpCode::Return, line, col);
            }
            StmtKind::Expr(e) => {
                self.emit_expr(e)?;
                self.emit(OpCode::Pop, line, col); // su valor se descarta
            }
        }
        Ok(())
    }

    fn emit_expr(&mut self, expr: &Expr) -> Result<(), CompileError> {
        let (line, col) = (expr.line, expr.col);
        match &expr.kind {
            ExprKind::Int(v) => {
                let idx = self.chunk.add_constant(Value::Int(*v));
                self.emit(OpCode::Constant(idx), line, col);
            }
            ExprKind::Float(v) => {
                let idx = self.chunk.add_constant(Value::Float(*v));
                self.emit(OpCode::Constant(idx), line, col);
            }
            ExprKind::Str(s) => {
                let idx = self.chunk.add_constant(Value::Str(s.clone()));
                self.emit(OpCode::Constant(idx), line, col);
            }
            ExprKind::Bool(true) => {
                self.emit(OpCode::True, line, col);
            }
            ExprKind::Bool(false) => {
                self.emit(OpCode::False, line, col);
            }

            ExprKind::Ident(name) => {
                let slot = self.resolve_local(name).expect("el checker garantiza la variable");
                self.emit(OpCode::GetLocal(slot), line, col);
            }

            ExprKind::Unary { op, expr: inner } => {
                self.emit_expr(inner)?;
                let opc = match op {
                    UnaryOp::Neg => OpCode::Negate,
                    UnaryOp::Not => OpCode::Not,
                };
                self.emit(opc, line, col);
            }

            // Lógicos con cortocircuito (JumpIfFalse ojea la condición).
            ExprKind::Binary { op: BinaryOp::And, left, right } => {
                self.emit_expr(left)?;
                let jump = self.emit(OpCode::JumpIfFalse(0), line, col);
                self.emit(OpCode::Pop, line, col);
                self.emit_expr(right)?;
                self.patch_jump(jump);
            }
            ExprKind::Binary { op: BinaryOp::Or, left, right } => {
                self.emit_expr(left)?;
                let to_else = self.emit(OpCode::JumpIfFalse(0), line, col);
                let to_end = self.emit(OpCode::Jump(0), line, col);
                self.patch_jump(to_else);
                self.emit(OpCode::Pop, line, col);
                self.emit_expr(right)?;
                self.patch_jump(to_end);
            }

            ExprKind::Binary { op, left, right } => {
                self.emit_expr(left)?;
                self.emit_expr(right)?;
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
                self.emit(opc, line, col);
            }

            ExprKind::If { cond, then_branch, else_branch } => {
                self.emit_expr(cond)?;
                let to_else = self.emit(OpCode::JumpIfFalse(0), line, col);
                self.emit(OpCode::Pop, line, col);
                self.emit_block(then_branch)?;
                let to_end = self.emit(OpCode::Jump(0), line, col);

                self.patch_jump(to_else);
                self.emit(OpCode::Pop, line, col);
                match else_branch {
                    Some(else_e) => self.emit_expr(else_e)?,
                    None => {
                        self.emit(OpCode::Unit, line, col);
                    }
                }
                self.patch_jump(to_end);
            }

            ExprKind::While { cond, body } => {
                let loop_start = self.chunk.code.len();
                self.emit_expr(cond)?;
                let exit = self.emit(OpCode::JumpIfFalse(0), line, col);
                self.emit(OpCode::Pop, line, col); // cond true → descartarla
                self.emit_block(body)?;
                self.emit(OpCode::Pop, line, col); // descartar el valor del cuerpo
                self.emit(OpCode::Jump(loop_start), line, col); // salto hacia atrás
                self.patch_jump(exit);
                self.emit(OpCode::Pop, line, col); // cond false → descartarla
                self.emit(OpCode::Unit, line, col); // el while vale unit
            }

            ExprKind::Block(b) => self.emit_block(b)?,

            ExprKind::ArrayLit(elems) => {
                for e in elems {
                    self.emit_expr(e)?;
                }
                self.emit(OpCode::MakeArray(elems.len()), line, col);
            }

            ExprKind::Index { array, index } => {
                self.emit_expr(array)?;
                self.emit_expr(index)?;
                self.emit(OpCode::Index, line, col);
            }

            ExprKind::Call { callee, args } => {
                for arg in args {
                    self.emit_expr(arg)?;
                }
                match &callee.kind {
                    ExprKind::Ident(name) if name == "print" => self.emit(OpCode::Print, line, col),
                    ExprKind::Ident(name) if name == "len" => self.emit(OpCode::Len, line, col),
                    ExprKind::Ident(name) if name == "push" => self.emit(OpCode::Push, line, col),
                    ExprKind::Ident(name) => {
                        let idx = *self.indices.get(name).expect("el checker garantiza la función");
                        self.emit(OpCode::Call(idx, args.len()), line, col)
                    }
                    _ => unreachable!("el checker garantiza llamada por nombre"),
                };
            }
        }
        Ok(())
    }
}
