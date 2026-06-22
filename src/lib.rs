//! raylang — la librería del compilador/intérprete.
//!
//! Fases del pipeline (DESIGN.md §2). El front-end se comparte; el backend de
//! ejecución tiene dos caminos:
//!
//!   fuente → [lexer] → [parser] → [checker] → ┬─ [interpreter] ──────── (M1)
//!                                             └─ [compiler] → [vm] ───── (M2)
//!
//! El binario (`src/main.rs`) es un cliente delgado de esta librería.

pub mod ast;
pub mod bytecode;
pub mod checker;
pub mod compiler;
pub mod diagnostic;
pub mod gc;
pub mod interpreter;
pub mod lexer;
pub mod parser;
pub mod prelude;
pub mod repl;
pub mod token;
pub mod vm;
