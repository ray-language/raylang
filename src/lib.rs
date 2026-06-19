//! raylang — la librería del compilador/intérprete.
//!
//! Aquí viven las fases del pipeline (DESIGN.md §2). De momento solo el front-end
//! empieza a tomar forma:
//!
//!   fuente → [lexer] → tokens → … (el resto de fases vendrá después)
//!
//! El binario (`src/main.rs`) será un cliente delgado de esta librería.

pub mod lexer;
pub mod token;
