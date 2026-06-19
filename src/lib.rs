//! raylang — la librería del compilador/intérprete.
//!
//! Aquí viven las fases del pipeline (DESIGN.md §2). Con M1 el pipeline está
//! completo:
//!
//!   fuente → [lexer] → tokens → [parser] → AST → [checker] → [interpreter] → ejecución
//!
//! El binario (`src/main.rs`) es un cliente delgado de esta librería.

pub mod ast;
pub mod checker;
pub mod interpreter;
pub mod lexer;
pub mod parser;
pub mod token;
