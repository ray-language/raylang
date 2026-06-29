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

// Orden total (M11.7d): `self < otro`. Lo usa `sort`. Los primitivos lo implementan vía el
// operador `<` (extendido a string/char en M11.7d); un tipo del usuario lo implementa a mano.
trait Ord {
    fn menor(self, otro: Self) -> bool;
}
impl Ord for int { fn menor(self, otro: int) -> bool { self < otro } }
impl Ord for float { fn menor(self, otro: float) -> bool { self < otro } }
impl Ord for string { fn menor(self, otro: string) -> bool { self < otro } }
impl Ord for char { fn menor(self, otro: char) -> bool { self < otro } }

// Eq/Show para los primitivos (M13.2a): los habilita `assert_eq` (que pide `T: Eq + Show`) y, en
// general, cualquier genérico acotado por Eq/Show sobre un primitivo. Vía `==` y `to_string`, que
// ya operan sobre int/float/bool/string/char. (Un tipo del usuario los obtiene con `@derive`.)
impl Eq for int { fn igual(self, otro: int) -> bool { self == otro } }
impl Eq for float { fn igual(self, otro: float) -> bool { self == otro } }
impl Eq for string { fn igual(self, otro: string) -> bool { self == otro } }
impl Eq for bool { fn igual(self, otro: bool) -> bool { self == otro } }
impl Eq for char { fn igual(self, otro: char) -> bool { self == otro } }
impl Show for int { fn mostrar(self) -> string { to_string(self) } }
impl Show for float { fn mostrar(self) -> string { to_string(self) } }
impl Show for string { fn mostrar(self) -> string { to_string(self) } }
impl Show for bool { fn mostrar(self) -> string { to_string(self) } }
impl Show for char { fn mostrar(self) -> string { to_string(self) } }

// Ordena ascendente, devolviendo un arreglo NUEVO (insertion sort). `T` debe implementar `Ord`;
// el bound se baja a paso de diccionarios (M9.2), así que `sort` es front-end puro (cero opcodes).
fn sort<T: Ord>(a: [T]) -> [T] {
    var out: [T] = [];
    var i: int = 0;
    while (i < len(a)) {
        let x: T = a[i];
        push(out, x);
        var j: int = len(out) - 1;
        while (j > 0 && x.menor(out[j - 1])) {
            out[j] = out[j - 1];
            j = j - 1;
        }
        out[j] = x;
        i = i + 1;
    }
    out
}

// --- Mapas Map<K,V> (M13.1) ---
// `get` envuelve el primitivo `__map_get` (que devuelve [V]) en un Option, como los demás
// envoltorios. Las claves son hashables (int/string/char/bool); el checker lo garantiza al
// instanciar K. `map_new`/`insert`/`contains_key`/`len` son builtins (operan directo).

// Valor asociado a la clave `k`, o None si no está.
fn get<K, V>(m: Map<K, V>, k: K) -> Option<V> {
    let r = __map_get(m, k);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M13.1b: quita la clave `k` del mapa y devuelve su valor (None si no estaba).
fn remove<K, V>(m: Map<K, V>, k: K) -> Option<V> {
    let r = __map_remove(m, k);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// --- Aserciones (M13.2a) ---
// Sobre el primitivo `panic` (el único toque de runtime). No hay sobrecarga, así que en vez de
// `assert(cond)` y `assert(cond, msg)` se ofrece `assert(cond)` (mensaje genérico), `assert_eq`
// (mensaje detallado con los valores) y, para un mensaje a medida, `panic("...")` directo.

// Falla con un mensaje genérico si la condición no se cumple.
fn assert(cond: bool) {
    if (!cond) { panic("aserción falló"); }
}

// Falla mostrando ambos valores si no son iguales. `T` debe ser Eq (comparar) y Show (mostrar);
// los bounds se bajan a diccionarios (M9.2), así que esto es front-end puro sobre `panic`.
fn assert_eq<T: Eq + Show>(a: T, b: T) {
    if (!a.igual(b)) {
        panic("assert_eq falló: " + a.mostrar() + " != " + b.mostrar());
    }
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

// Parsea un flotante; None si el texto no es un flotante válido (M14).
fn parse_float(s: string) -> Option<float> {
    let r = __parse_float(s);
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

// M12.1 (concurrencia): recibe del canal. Some(v) si llega un valor; None si el canal está cerrado y
// vacío. Envuelve el primitivo __recv (que devuelve [T]) en un Option, como input/parse_int. Solo la VM.
fn recv<T>(ch: Channel<T>) -> Option<T> {
    let r = __recv(ch);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
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

// M16.1b: decodifica bytes como UTF-8; Ok(string) u Err(mensaje) si no es válido.
fn from_utf8(b: bytes) -> Result<string, string> {
    let r = __from_utf8(b);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}

// M16.1c: I/O binaria. Las lecturas devuelven [bytes] etiquetado (tag en bytes para arreglo
// homogéneo); el mensaje de error viene como bytes y se decoda con from_utf8.
fn read_file_bytes(ruta: string) -> Result<bytes, string> {
    let r = __read_file_bytes(ruta);
    if (r[0] == b"ok") {
        Result.Ok(r[1])
    } else {
        match (from_utf8(r[1])) {
            Result.Ok(m) => Result.Err(m),
            Result.Err(e) => Result.Err("error de E/S"),
        }
    }
}

fn write_file_bytes(ruta: string, datos: bytes) -> Result<int, string> {
    let r = __write_file_bytes(ruta, datos);
    if (r[0] == "ok") { Result.Ok(len(datos)) } else { Result.Err(r[1]) }
}

fn socket_read_bytes(h: int) -> Result<bytes, string> {
    let r = __socket_read_bytes(h);
    if (r[0] == b"ok") {
        Result.Ok(r[1])
    } else {
        match (from_utf8(r[1])) {
            Result.Ok(m) => Result.Err(m),
            Result.Err(e) => Result.Err("error de socket"),
        }
    }
}

fn socket_write_bytes(h: int, datos: bytes) -> Result<int, string> {
    let r = __socket_write_bytes(h, datos);
    if (r[0] == "ok") { Result.Ok(len(datos)) } else { Result.Err(r[1]) }
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

// M11.7c: borra un archivo; Ok(0) u Err(mensaje del sistema).
fn remove_file(ruta: string) -> Result<int, string> {
    let r = __remove_file(ruta);
    if (r[0] == "ok") { Result.Ok(0) } else { Result.Err(r[1]) }
}

// M11.7c: nombres de las entradas de un directorio (ordenados); Ok([nombres]) u Err(mensaje).
// El primitivo devuelve ["ok", n0, n1, …] o ["err", msg]; aquí se reconstruye el [string].
fn list_dir(ruta: string) -> Result<[string], string> {
    let r = __list_dir(ruta);
    if (r[0] == "ok") {
        var nombres: [string] = [];
        var i = 1;
        while (i < len(r)) { push(nombres, r[i]); i = i + 1; }
        Result.Ok(nombres)
    } else {
        Result.Err(r[1])
    }
}

// --- I/O con buffering: handles de archivo (M11.8). open/read_line/write/close. ---

// Abre un archivo (modo "r"/"w"/"a") y devuelve un handle (int); Err(mensaje) si falla.
fn open(ruta: string, modo: string) -> Result<int, string> {
    let r = __open(ruta, modo);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// Lee la siguiente línea del handle (sin el salto); None en EOF (o handle no-lector).
fn read_line(h: int) -> Option<string> {
    let r = __read_line_handle(h);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Escribe en el handle; Ok(nº de caracteres) u Err(mensaje).
fn write(h: int, s: string) -> Result<int, string> {
    let r = __write_handle(h, s);
    if (r[0] == "ok") { Result.Ok(len(s)) } else { Result.Err(r[1]) }
}

// --- Cliente TCP (M15.2). Sobre los primitivos __tcp_connect/__socket_read/__socket_write. ---

// Conecta a host:port (resuelve el nombre); Ok(handle) u Err(mensaje).
fn tcp_connect(host: string, port: int) -> Result<int, string> {
    let r = __tcp_connect(host, port);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// M19.4a: conecta por TLS a host:port (verifica el certificado del servidor); Ok(handle) u Err. El
// handle se lee/escribe con socket_read_bytes/socket_write_bytes (que desvían a TLS) y se cierra con close.
fn tls_connect(host: string, port: int) -> Result<int, string> {
    let r = __tls_connect(host, port);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// M19.4b: envuelve un socket TCP ya aceptado (handle de tcp_accept) en una sesión TLS de servidor con
// el certificado y la clave en PEM; Ok(handle) u Err. Reusa el handle. Solo VM (servidor concurrente).
fn tls_accept(handle: int, cert: string, key: string) -> Result<int, string> {
    let r = __tls_accept(handle, cert, key);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// Hace una lectura del socket; Ok(datos) ("" = EOF) u Err(mensaje).
fn socket_read(h: int) -> Result<string, string> {
    let r = __socket_read(h);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}

// Escribe en el socket; Ok(nº de bytes) u Err(mensaje).
fn socket_write(h: int, s: string) -> Result<int, string> {
    let r = __socket_write(h, s);
    if (r[0] == "ok") { Result.Ok(len(s)) } else { Result.Err(r[1]) }
}

// --- Servidor TCP (M15.3). Sobre __tcp_listen/__tcp_accept. ---

// Escucha en host:port (port=0 → puerto efímero); Ok(handle de escucha) u Err(mensaje).
fn tcp_listen(host: string, port: int) -> Result<int, string> {
    let r = __tcp_listen(host, port);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// Bloquea hasta una conexión; Ok(handle de conexión) u Err(mensaje).
fn tcp_accept(listener: int) -> Result<int, string> {
    let r = __tcp_accept(listener);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
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

/// Los traits del prelude (`Eq`/`Show`/`Ord`), ya parseados (M10.1).
pub fn traits() -> Vec<TraitDef> {
    parse().traits
}

/// Los `impl` del prelude (M11.7d: `Ord` para int/float/string/char), ya parseados.
pub fn impls() -> Vec<crate::ast::ImplBlock> {
    parse().impls
}
