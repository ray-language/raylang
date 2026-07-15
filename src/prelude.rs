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

/// Banda de líneas del prelude. Sus posiciones `(línea, col)` se desplazan a partir de esta base,
/// muy por encima de cualquier programa de usuario realista. Motivo: varias lowerings del checker
/// (uint literals M28.3b, UFCS/diccionarios/`dyn` de M9) indexan por `(línea, col)` sobre el
/// programa **fusionado**; el loader ya da a cada módulo de usuario una banda disjunta
/// (`shift_program`), pero el prelude se inyecta DESPUÉS con sus líneas propias (1..). Sin
/// desplazarlo, un literal `u64` de un módulo desplazado puede caer en la misma `(línea, col)` que
/// un literal `int` del prelude → la lowering por posición lo envuelve por error (p. ej. corrompía
/// `string#hash`). Con la banda alta, las posiciones del prelude son globalmente únicas.
pub const LINE_BASE: usize = 1_000_000_000;

/// Parsea el prelude UNA vez por proceso y cachea el AST (antes, cada accessor re-lexeaba y
/// re-parseaba el fuente entero → 5 pasadas completas por `check()`, en cada arranque del CLI y
/// en cada tecleo del LSP). Los accessors CLONAN del caché: clonar el AST es mucho más barato
/// que re-parsearlo, y los llamadores necesitan copias propias (las mutan al inyectar).
fn parsed() -> &'static crate::ast::Program {
    static P: std::sync::OnceLock<crate::ast::Program> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let tokens = crate::lexer::lex(SOURCE).unwrap_or_else(|e| crate::ice!("el prelude no lexea: {e}"));
        let mut program = crate::parser::parse(tokens)
            .unwrap_or_else(|e| crate::ice!("el prelude no parsea: {e}"));
        // Desplaza el prelude a su banda disjunta (ver LINE_BASE): posiciones globalmente únicas
        // en el programa fusionado, para que las lowerings por posición no lo confundan con módulos
        // de usuario desplazados.
        crate::loader::shift_program(&mut program, LINE_BASE);
        program
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_prelude_va_en_su_banda_de_lineas_disjunta() {
        // Guarda el fix de la colisión de posiciones: el prelude se inyecta en el programa fusionado
        // y varias lowerings del checker (uint literals, UFCS/dicts/`dyn`) indexan por (línea, col).
        // Debe vivir por encima de LINE_BASE para no chocar con módulos de usuario desplazados.
        for f in functions() {
            assert!(f.line >= LINE_BASE, "fn '{}' del prelude en línea {} < LINE_BASE", f.name, f.line);
        }
        // Los métodos de impl (p. ej. `string#hash`) también: su cuerpo lleva los literales que
        // se corrompían al colisionar.
        for imp in impls() {
            for m in &imp.methods {
                assert!(m.line >= LINE_BASE, "método '{}' del prelude en línea {} < LINE_BASE", m.name, m.line);
            }
        }
    }
}
