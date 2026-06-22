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

use crate::ast::{EnumDef, Function};

/// El código fuente del prelude. Se parsea una vez; sus enums y funciones se anteponen
/// a los del programa del usuario.
pub const SOURCE: &str = r#"
enum Option<T> { Some(T), None }
enum Result<T, E> { Ok(T), Err(E) }

// Aplica `f` a cada elemento, devolviendo un arreglo nuevo con los resultados.
fn map<T, U>(xs: [T], f: fn(T) -> U) -> [U] {
    var out: [U] = [];
    var i: int = 0;
    while (i < len(xs)) {
        push(out, f(xs[i]));
        i = i + 1;
    }
    out
}

// Conserva los elementos para los que `pred` es verdadero, en un arreglo nuevo.
fn filter<T>(xs: [T], pred: fn(T) -> bool) -> [T] {
    var out: [T] = [];
    var i: int = 0;
    while (i < len(xs)) {
        let x: T = xs[i];
        if (pred(x)) { push(out, x); }
        i = i + 1;
    }
    out
}

// Reduce el arreglo a un único valor, acumulando de izquierda a derecha desde `init`.
fn fold<T, A>(xs: [T], init: A, f: fn(A, T) -> A) -> A {
    var acc: A = init;
    var i: int = 0;
    while (i < len(xs)) {
        acc = f(acc, xs[i]);
        i = i + 1;
    }
    acc
}
"#;

/// Parsea el prelude una vez. El `expect` no puede fallar: el fuente es una constante
/// conocida y válida.
fn parse() -> crate::ast::Program {
    let tokens = crate::lexer::lex(SOURCE).expect("el prelude lexea");
    crate::parser::parse(tokens).expect("el prelude parsea")
}

/// Los enums del prelude (`Option`/`Result`), ya parseados.
pub fn enums() -> Vec<EnumDef> {
    parse().enums
}

/// Las funciones del prelude (`map`/`filter`/`fold`), ya parseadas.
pub fn functions() -> Vec<Function> {
    parse().functions
}
