//! raylang — la librería del compilador/intérprete.
//!
//! Fases del pipeline (DESIGN.md §2). Front-end en construcción:
//!
//!   fuente → [lexer] → tokens → [parser] → AST → … (checker e intérprete vendrán después)
//!
//! El binario (`src/main.rs`) es un cliente delgado de esta librería.

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod token;
