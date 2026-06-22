//! El **prelude** de raylang (M6.3).
//!
//! `Option<T>` y `Result<T, E>` no son tipos incrustados en el compilador: son enums
//! genéricos **escritos en raylang**, que se inyectan en cada programa antes de
//! verificarlo. El lenguaje no los trata especial salvo por el operador `?`, que sabe
//! desempaquetar/propagar `Ok`/`Err` y `Some`/`None`.
//!
//! Que el modelo de errores del lenguaje sea "solo librería" es deliberado: el mismo
//! mecanismo de genéricos + enums permite al usuario definir su propio `Either<A, B>`
//! con el mismo poder.

use crate::ast::EnumDef;

/// El código fuente del prelude. Se parsea una vez y sus enums se anteponen a los del
/// programa del usuario.
pub const SOURCE: &str = r#"
enum Option<T> { Some(T), None }
enum Result<T, E> { Ok(T), Err(E) }
"#;

/// Los enums del prelude, ya parseados. El `expect` no puede fallar: el fuente es una
/// constante conocida y válida.
pub fn enums() -> Vec<EnumDef> {
    let tokens = crate::lexer::lex(SOURCE).expect("el prelude lexea");
    let program = crate::parser::parse(tokens).expect("el prelude parsea");
    program.enums
}
