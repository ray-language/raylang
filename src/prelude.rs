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

use crate::ast::{EnumDef, Function, TraitDef};

/// El código fuente del prelude. Se parsea una vez; sus enums y funciones se anteponen
/// a los del programa del usuario.
pub const SOURCE: &str = r#"
enum Option<T> { Some(T), None }
enum Result<T, E> { Ok(T), Err(E) }

// Igualdad estructural (M10.1). `@derive(Eq)` genera el `impl` para un struct/enum.
// Usa `Self` en posición de argumento, así que no es invocable sobre un `dyn Eq`
// (object safety): se compara entre valores concretos, `a.igual(b)`.
trait Eq {
    fn igual(self, otro: Self) -> bool;
}

// Representación textual (limpieza post-M11, L2). `@derive(Show)` genera el `impl` para un
// struct/enum. No usa `Self` fuera del receptor, así que sí es object-safe (`dyn Show`).
trait Show {
    fn mostrar(self) -> string;
}

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

// --- I/O (M11.2): envoltorios sobre primitivos builtin que devuelven [T] (vacío/único) ---
// El runtime no sabe de Option: los primitivos devuelven un arreglo de 0 o 1 elementos y aquí,
// en raylang, se traducen a Option con Some/None corrientes (el patrón de la stdlib, M7.3).

// Parsea un entero; None si el texto no es un entero válido.
fn parse_int(s: string) -> Option<int> {
    let r = __parse_int(s);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Lee una línea de stdin (sin el salto de línea); None en fin de entrada (EOF).
fn input() -> Option<string> {
    let r = __read_line();
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Lee una línea y la parsea como entero; None en EOF o si no es un entero.
fn read_int() -> Option<int> {
    let s = input()?;
    parse_int(s)
}

// Valor de una variable de entorno; None si no está definida.
fn env(nombre: string) -> Option<string> {
    let r = __env(nombre);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7a: índice (de carácter) de la primera ocurrencia de `sub` en `s`; None si no aparece.
fn index_of(s: string, sub: string) -> Option<int> {
    let r = __index_of(s, sub);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7b: quita y devuelve el último elemento del arreglo (lo muta); None si está vacío.
fn pop<T>(a: [T]) -> Option<T> {
    let r = __pop(a);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7b: índice de la primera ocurrencia de `x` en el arreglo; None si no aparece.
fn position<T>(a: [T], x: T) -> Option<int> {
    let r = __position(a, x);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// --- Archivos (M11.2c): el primitivo devuelve un arreglo ETIQUETADO (primer elemento "ok"/"err");
// aquí se traduce a Result. Así el runtime tampoco sabe de Result (como con Option). ---

// Lee el archivo completo; Ok(contenido) u Err(mensaje del sistema).
fn read_file(ruta: string) -> Result<string, string> {
    let r = __read_file(ruta);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}

// Escribe el contenido en el archivo (lo crea/sobrescribe); Ok(nº de caracteres) u Err(mensaje).
fn write_file(ruta: string, contenido: string) -> Result<int, string> {
    let r = __write_file(ruta, contenido);
    if (r[0] == "ok") { Result.Ok(len(contenido)) } else { Result.Err(r[1]) }
}

// Añade el contenido al final del archivo (lo crea si no existe); Ok(nº de caracteres) u Err(mensaje).
fn append_file(ruta: string, contenido: string) -> Result<int, string> {
    let r = __append_file(ruta, contenido);
    if (r[0] == "ok") { Result.Ok(len(contenido)) } else { Result.Err(r[1]) }
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

/// Los traits del prelude (`Eq`), ya parseados (M10.1).
pub fn traits() -> Vec<TraitDef> {
    parse().traits
}
