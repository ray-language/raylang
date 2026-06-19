//! raylang — la librería del compilador/intérprete.
//!
//! Fases del pipeline (DESIGN.md §2). Front-end:
//!
//!   fuente → [lexer] → tokens → [parser] → AST → [checker] → … (el intérprete vendrá después)
//!
//! El binario (`src/main.rs`) es un cliente delgado de esta librería.

pub mod ast;
pub mod checker;
pub mod lexer;
pub mod parser;
pub mod token;
