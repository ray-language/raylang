//! El **prelude** de raylang (M6.3, M7.3).
//!
//! El prelude es la **biblioteca estándar** mínima, **escrita en el propio raylang** y
//! inyectada en cada programa antes de verificarlo. No hay nada incrustado en el
//! compilador: son definiciones normales que el checker, el intérprete y la VM tratan
//! como cualquier otra.
//!
//! - **Tipos (M6.3):** `Option<T>` y `Result<T, E>`, enums genéricos. El lenguaje no
//!   los trata especial salvo por el operador `?`. Que el modelo de errores sea "solo
//!   librería" es deliberado: el mismo mecanismo permite al usuario definir su propio
//!   `Either<A, B>` con el mismo poder.
//! - **Funciones de orden superior (M7.3):** `map`, `filter`, `fold`. Son la prueba de
//!   que con los genéricos (M6), los closures (M4) y los builtins `len`/`push` ya se
//!   puede escribir librería útil **dentro del lenguaje**, sin tocar el runtime. Lucen
//!   con UFCS (`xs.map(f)`) y pipelines (`xs |> map(f)`).

use crate::ast::{EnumDef, Function, StructDef, TraitDef};

/// El código fuente del prelude. Se parsea una vez; sus enums y funciones se anteponen
/// a los del programa del usuario.
pub const SOURCE: &str = include_str!("prelude.ray");

/// Parsea el prelude UNA vez por proceso y cachea el AST (antes, cada accessor re-lexeaba y
/// re-parseaba el fuente entero → 5 pasadas completas por `check()`, en cada arranque del CLI y
/// en cada tecleo del LSP). Los accessors CLONAN del caché: clonar el AST es mucho más barato
/// que re-parsearlo, y los llamadores necesitan copias propias (las mutan al inyectar).
fn parsed() -> &'static crate::ast::Program {
    static P: std::sync::OnceLock<crate::ast::Program> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let tokens = crate::lexer::lex(SOURCE).unwrap_or_else(|e| crate::ice!("el prelude no lexea: {e}"));
        crate::parser::parse(tokens).unwrap_or_else(|e| crate::ice!("el prelude no parsea: {e}"))
    })
}

/// Los enums del prelude (`Option`/`Result`), ya parseados.
pub fn enums() -> Vec<EnumDef> {
    parsed().enums.clone()
}

/// Los structs del prelude (`ArrayIter`/`RangeIter` para `.iter()`/`range`, M40.2b), ya parseados.
pub fn structs() -> Vec<StructDef> {
    parsed().structs.clone()
}

/// Las funciones del prelude (`map`/`filter`/`fold`), ya parseadas.
pub fn functions() -> Vec<Function> {
    parsed().functions.clone()
}

/// Los traits del prelude (`Eq`/`Show`/`Ord`), ya parseados (M10.1).
pub fn traits() -> Vec<TraitDef> {
    parsed().traits.clone()
}

/// Los `impl` del prelude (M11.7d: `Ord` para int/float/string/char), ya parseados.
pub fn impls() -> Vec<crate::ast::ImplBlock> {
    parsed().impls.clone()
}
