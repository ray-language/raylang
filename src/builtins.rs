//! Registro único de los **builtins** del lenguaje (limpieza post-M11, L1).
//!
//! Antes, cada builtin (`print`, `len`, `split`, `args`, …) se repetía en ~4 sitios: el checker
//! (membresía + regla de tipos), el intérprete (despacho) y el compilador (nombre → opcode). Añadir
//! uno obligaba a tocarlos todos y era fácil desincronizarlos. Aquí viven, en **una sola tabla**:
//!
//! - el **nombre** con que se invocan,
//! - el **opcode** que los implementa en la VM (el compilador lo emite),
//! - la **regla de tipado**: valida aridad y tipos de los argumentos ya comprobados y da el tipo
//!   de retorno.
//!
//! Las *implementaciones de ejecución* siguen donde corresponde (el `match` por opcode en la VM y
//! `eval_builtin` en el intérprete): son código específico de cada motor, no metadatos. Pero la
//! membresía, las firmas y el mapeo a opcode —lo duplicado y propenso a desincronizarse— están
//! centralizados aquí. Añadir un builtin "normal" es ahora: una fila en esta tabla + su opcode en
//! la VM + su caso en `eval_builtin`.
//!
//! Nota: cuatro builtins son **ad-hoc polimórficos** y no tendrían una firma raylang ordinaria
//! (`print`/`eprint` aceptan cualquier imprimible; `len` un arreglo *o* string; `to_string`
//! int/float/bool/string). Por eso la regla es una función y no una firma fija: cada uno expresa su
//! propio criterio. Es la razón por la que se eligió esta tabla en Rust frente a un `@builtin fn`.

use crate::ast::Type;
use crate::bytecode::{FsOp, FsTest, MathFn, OpCode};

/// Aplica una función matemática unaria `float -> float` (M15.1a). Helper compartido por ambos
/// motores: el resultado es determinista e idéntico en intérprete y VM, así que vive aquí (como
/// `append_to_file`) en vez de duplicarse. El dominio inválido (`sqrt(-1)`, `ln(0)`…) sigue la
/// semántica de `f64` (`NaN`/`-inf`), sin error de runtime.
pub fn apply_mathf(f: MathFn, x: f64) -> f64 {
    match f {
        MathFn::Sqrt => x.sqrt(),
        MathFn::Sin => x.sin(),
        MathFn::Cos => x.cos(),
        MathFn::Tan => x.tan(),
        MathFn::Ln => x.ln(),
        MathFn::Log10 => x.log10(),
        MathFn::Exp => x.exp(),
        MathFn::Floor => x.floor(),
        MathFn::Ceil => x.ceil(),
        MathFn::Round => x.round(),
        // M65.2: trig inversa y compañía (mismos totales IEEE: fuera de dominio → NaN).
        MathFn::Asin => x.asin(),
        MathFn::Acos => x.acos(),
        MathFn::Atan => x.atan(),
        MathFn::Log2 => x.log2(),
        MathFn::Trunc => x.trunc(),
    }
}

// --- Reloj y aleatoriedad (M15.1b) ---
//
// Estos builtins NO son deterministas → no entran al oráculo; se prueban por subproceso. Viven aquí
// (helpers compartidos) para que el intérprete y la VM usen el MISMO reloj y el MISMO flujo de RNG.

/// Milisegundos desde la época Unix (reloj de pared). Builtin `now`.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// El ancla del reloj monótono: un `Instant` fijado en la primera lectura, COMPARTIDO por las
// lecturas en ms y en ns (así `monotonic_nanos()/1_000_000` y `monotonic()` son coherentes).
fn monotonic_start() -> std::time::Instant {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *START.get_or_init(std::time::Instant::now)
}

/// Milisegundos de un reloj **monótono**: ancla un `Instant` de referencia en la primera llamada y
/// devuelve el tiempo transcurrido desde él. Sirve para medir intervalos. Builtin `monotonic`.
pub fn monotonic_millis() -> i64 {
    monotonic_start().elapsed().as_millis() as i64
}

/// Nanosegundos del mismo reloj monótono (misma ancla que `monotonic_millis` → coherentes entre
/// sí). En `i64` caben ~292 años desde el arranque del proceso. Builtin `monotonic_nanos`.
pub fn monotonic_nanos() -> i64 {
    monotonic_start().elapsed().as_nanos() as i64
}

/// Duerme el hilo `ms` milisegundos (`ms<=0` → no duerme). Builtin `sleep`.
///
/// M119: usa la espera PRECISA de `poll(2)` (ver `crate::poll::sleep_ms`), no `thread::sleep` —
/// en macOS este último se pasa varios ms por *timer coalescing* y descuadra el pacing (§72).
pub fn sleep_millis(ms: i64) {
    crate::poll::sleep_ms(ms);
}

/// El estado del PRNG del proceso. `std` no trae generador de aleatorios y la invariante es **cero
/// dependencias de Cargo**, así que llevamos uno propio: **SplitMix64**, sembrado del reloj la
/// primera vez. No es criptográfico (es para simulación/jitter/ids, no para secretos).
fn rng() -> &'static std::sync::Mutex<u64> {
    static R: std::sync::OnceLock<std::sync::Mutex<u64>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        std::sync::Mutex::new(seed ^ 0x9E37_79B9_7F4A_7C15)
    })
}

/// Avanza el generador y devuelve los siguientes 64 bits (SplitMix64).
fn next_u64() -> u64 {
    let mut state = rng().lock().expect("RNG not poisoned");
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Un `float` aleatorio en `[0, 1)` (53 bits de mantisa). Builtin `random`.
pub fn random_f64() -> f64 {
    (next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// Un entero aleatorio en `[0, n)`; `n<=0` → `0` (total, sin error de runtime). Builtin `random_int`.
pub fn random_int(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    (next_u64() % (n as u64)) as i64
}

/// M68.1: fija el estado del PRNG — misma semilla, misma secuencia (SplitMix64 es aritmética
/// entera pura → estable entre plataformas y motores). Primitivo `__random_seed`.
pub fn random_seed(n: i64) {
    *rng().lock().expect("RNG not poisoned") = n as u64;
}

/// Error de tipado de un builtin: `(índice_del_arg, mensaje)`. El índice `None` señala un error
/// general de la llamada (p. ej. aridad); `Some(i)` el argumento culpable (para ubicar el cursor).
pub type BuiltinError = (Option<usize>, String);

/// La regla de tipado de un builtin: de los tipos de los argumentos ya comprobados al tipo del
/// resultado (o un error).
pub type CheckFn = fn(&[Type]) -> Result<Type, BuiltinError>;

/// La especificación de un builtin: cómo se llama, qué opcode lo ejecuta y cómo se tipa.
pub struct Builtin {
    pub name: &'static str,
    pub opcode: OpCode,
    pub check: CheckFn,
}

/// Busca un builtin por nombre.
pub fn lookup(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

/// ¿`name` nombra un builtin?
pub fn is_builtin(name: &str) -> bool {
    lookup(name).is_some()
}

/// Los nombres de todos los builtins (incluidos los internos `__*`). Lo usa el LSP para autocompletar
/// (filtrando los `__*`, que son primitivos no destinados al usuario).
pub fn names() -> impl Iterator<Item = &'static str> {
    BUILTINS.iter().map(|b| b.name)
}

/// Una función **asociada** a un tipo incorporado (M48.1): `Tipo.fn(args)`, un namespace indexado por
/// el tipo (estilo `Vec::new()` de Rust), en vez de una función libre global. Sustituye a los antiguos
/// `map_new()`/`channel()`. Como el resultado es un tipo genérico **indeterminado** (Map/Channel), su
/// tipo lo fija el contexto esperado (como `[]`/`None`); por eso el **tipado** (result-desde-esperado)
/// vive en el checker y solo la **bajada al opcode** se lee de aquí. La tabla aporta existencia,
/// aridad, opcode y metadatos (doc/firma) para el LSP.
pub struct AssocFn {
    pub type_name: &'static str, // "Map" | "Channel"
    pub fn_name: &'static str,   // "new" | "bounded"
    pub arity: usize,            // nº de argumentos (sin receptor: no hay `self`)
    pub opcode: OpCode,          // el compilador empuja los args y emite este opcode
    pub doc: &'static str,       // hover del LSP
    pub sig: &'static str,       // signature legible: hover / signature help
}

/// Las funciones asociadas a tipos incorporados. `Map.new()`, `Channel.new()`, `Channel.bounded(n)`.
pub const ASSOC_FNS: &[AssocFn] = &[
    AssocFn {
        type_name: "Map", fn_name: "new", arity: 0, opcode: OpCode::MapNew,
        doc: "Creates an empty `Map<K, V>`. The element types are inferred from the expected type (annotate the binding if indeterminate). Keys must be hashable: int, string, char or bool.",
        sig: "Map.new() -> Map<K, V>",
    },
    AssocFn {
        type_name: "Channel", fn_name: "new", arity: 0, opcode: OpCode::ChannelNew,
        doc: "Creates an unbounded typed channel (`Channel<T>`); `send` never blocks. The element type is inferred from the expected type. Only runs on the VM.",
        sig: "Channel.new() -> Channel<T>",
    },
    AssocFn {
        type_name: "Channel", fn_name: "bounded", arity: 1, opcode: OpCode::ChannelNewBounded,
        doc: "Creates a bounded channel holding at most `n` values (`0` = synchronous rendezvous); senders block when full. The element type is inferred from the expected type. Only runs on the VM.",
        sig: "Channel.bounded(n: int) -> Channel<T>",
    },
];

/// Busca una función asociada por `(tipo, nombre)`.
pub fn assoc_lookup(type_name: &str, fn_name: &str) -> Option<&'static AssocFn> {
    ASSOC_FNS.iter().find(|a| a.type_name == type_name && a.fn_name == fn_name)
}

/// Las funciones asociadas de un tipo dado (para el completado `Tipo.` del LSP).
pub fn assoc_for_type(type_name: &str) -> impl Iterator<Item = &'static AssocFn> {
    ASSOC_FNS.iter().filter(move |a| a.type_name == type_name)
}

/// ¿El builtin, usado como **método** (`recv.f(...)`), toma argumentos más allá del receptor?
/// (M45b: para el snippet de completion — `push($0)` con args, `len()` sin ellos.) Los builtins son
/// ad-hoc polimórficos y muchos no tienen `signature()`, así que se lista el conjunto **sin args**.
pub fn method_takes_args(name: &str) -> bool {
    const NO_ARGS: &[&str] = &[
        "len", "trim", "chars", "reverse", "keys", "values", "to_upper", "to_lower", "to_string",
        "to_bytes",
    ];
    !NO_ARGS.contains(&name)
}

/// Builtins invocables como **método** (UFCS `recv.f(...)`) sobre un tipo de la categoría dada,
/// para el completion de miembros del LSP (M45). Los builtins son ad-hoc polimórficos (no tienen
/// una firma raylang uniforme que permita inferir "aplica a este tipo"), así que se listan a mano
/// por categoría. Solo los de **cara al usuario** con receptor claro; las operaciones de orden
/// superior (`map`/`filter`/`fold`/`sort`) y los envoltorios (`pop`/`position`/`get`/`remove`/
/// `index_of`) son funciones del **prelude**, no builtins, y las aporta la enumeración UFCS.
/// Categorías: `string`/`bytes`/`char`/`int`/`float`/`bool`/`array`/`map`. Tras M48.4e-3 varios de
/// estos nombres (len/push/trim/split/…) ya NO son builtins de función libre sino **métodos de los
/// traits del prelude** (`Len`/`Push`/`StrOps`/`MapOps`/…); el test-guardián
/// `methods_for_only_names_real_builtins` verifica que cada nombre sea un builtin O un método
/// conocido con `signature()`.
pub fn methods_for(category: &str) -> &'static [&'static str] {
    match category {
        "string" => &[
            "len", "trim", "split", "contains", "replace", "chars", "starts_with", "ends_with",
            "to_upper", "to_lower", "substring", "repeat", "join", "to_bytes", "to_string",
        ],
        "bytes" => &["len", "sub_bytes", "contains", "to_string"],
        "char" => &["char_code", "to_string"],
        "int" | "float" | "bool" => &["to_string"],
        "array" => &["len", "push", "reverse", "contains", "join"],
        "map" => &["len", "insert", "add_to", "contains_key", "keys", "values"],
        _ => &[],
    }
}

/// Firma legible de un builtin para el **signature help** del LSP: `(params, retorno)`. Cubre los
/// builtins de cara al usuario con **firma fija**; los ad-hoc polimórficos (int|float, `min`/`max`) se
/// muestran con un tipo representativo. `None` para los que no tienen firma fija sensata (`print`/`len`
/// aceptan cualquier valor) y los internos `__*`. El **hover** no usa esto —se calcula dinámicamente con
/// los tipos de cada llamada—; el signature help sí, porque se pide antes de que la llamada esté completa.
pub fn signature(name: &str) -> Option<(Vec<&'static str>, &'static str)> {
    Some(match name {
        // Matemáticas de un argumento float → float.
        "sqrt" | "sin" | "cos" | "tan" | "ln" | "log10" | "exp" | "floor" | "ceil" | "round" => {
            (vec!["x: float"], "float")
        }
        "pow" => (vec!["base: float", "exp: float"], "float"),
        "abs" => (vec!["x: float"], "float"), // ad-hoc: también int → int
        "min" | "max" => (vec!["a: float", "b: float"], "float"), // ad-hoc: también int
        "pi" | "e" | "random" => (vec![], "float"),
        "now" | "monotonic" | "monotonic_nanos" => (vec![], "int"),
        "sleep" => (vec!["ms: int"], "unit"),
        "random_int" => (vec!["n: int"], "int"),
        "panic" => (vec!["msg: string"], "unit"),
        // Builtins-método (M46a): firma con el receptor como primer parámetro (el completion de
        // miembros lo recorta para mostrar solo los argumentos). Tipos genéricos como texto de ayuda.
        "len" => (vec!["c"], "int"),
        "trim" => (vec!["s: string"], "string"),
        "split" => (vec!["s: string", "sep: string"], "[string]"),
        "contains" => (vec!["c", "item"], "bool"), // ad-hoc: string (subcadena) o arreglo (pertenencia)
        "replace" => (vec!["s: string", "from: string", "to: string"], "string"),
        "chars" => (vec!["s: string"], "[char]"),
        "starts_with" | "ends_with" => (vec!["s: string", "affix: string"], "bool"),
        "to_upper" | "to_lower" => (vec!["s: string"], "string"),
        "substring" => (vec!["s: string", "i: int", "j: int"], "string"),
        "repeat" => (vec!["s: string", "n: int"], "string"),
        "join" => (vec!["a: [string]", "sep: string"], "string"),
        "to_bytes" => (vec!["s: string"], "bytes"),
        "to_string" => (vec!["value"], "string"),
        "sub_bytes" => (vec!["b: bytes", "i: int", "j: int"], "bytes"),
        "char_code" => (vec!["c: char"], "int"),
        "signals" => (vec![], "Channel<int>"),
        "push" => (vec!["arr: [T]", "value: T"], "unit"),
        "reverse" => (vec!["arr: [T]"], "[T]"),
        "insert" => (vec!["m: Map<K, V>", "key: K", "value: V"], "unit"),
        "add_to" => (vec!["m: Map<K, V>", "key: K", "delta: V"], "unit"),
        "contains_key" => (vec!["m: Map<K, V>", "key: K"], "bool"),
        "keys" => (vec!["m: Map<K, V>"], "[K]"),
        "values" => (vec!["m: Map<K, V>"], "[V]"),
        _ => return None,
    })
}

/// Documentación (en inglés, cara al usuario) de un builtin, para el **hover** y el **completion**
/// del LSP. Los builtins viven en esta tabla Rust —no hay fuente raylang donde poner `///`—, así
/// que sus docs son metadatos aquí, como `signature()`. Cubre todos los de cara al usuario; `None`
/// para los primitivos internos `__*`.
pub fn doc(name: &str) -> Option<&'static str> {
    Some(match name {
        // --- Núcleo / salida ---
        "print" => "Prints a value to stdout followed by a newline. Accepts any printable value (int, float, bool, string, char, arrays, structs/enums with Show).",
        "eprint" => "Prints a value to stderr followed by a newline. Same printable values as `print`.",
        "panic" => "Aborts the program with the given message and a non-zero exit code, reporting the call position. Use for unreachable code; prefer `Result`/`Option` for expected failures.",
        "to_string" => "Converts an int, float, bool, char or string to its string representation (same formatting as `print`).",
        "len" => "Returns the length of a collection: characters of a string, elements of an array, entries of a Map, or octets of a bytes value.",
        "push" => "Appends a value to the end of an array, in place (arrays have reference semantics).",
        "args" => "Returns the command-line arguments passed to the program (after the file path) as `[string]`.",
        // --- Strings ---
        "trim" => "Returns a copy of the string with leading and trailing whitespace removed.",
        "split" => "Splits a string by a separator and returns the parts as `[string]`.",
        "contains" => "For strings: whether the substring occurs. For arrays: whether the value is an element (structural equality).",
        "replace" => "Returns a copy of the string with every occurrence of a substring replaced by another.",
        "chars" => "Returns the characters of a string as `[char]`.",
        "char_code" => "Returns the Unicode code point of a char as an int.",
        "starts_with" => "Whether the string starts with the given prefix.",
        "ends_with" => "Whether the string ends with the given suffix.",
        "to_upper" => "Returns the string converted to uppercase.",
        "to_lower" => "Returns the string converted to lowercase.",
        "substring" => "Returns the substring `[i, j)` by character index. Out-of-range indices are clamped, so it never fails at runtime.",
        "repeat" => "Returns the string repeated `n` times (`n <= 0` gives the empty string).",
        "join" => "With `(array, sep)`: joins a `[string]` into one string using the separator. With `(task)`: blocks until the task finishes and returns its value (re-raises if it failed).",
        "reverse" => "Returns a new array with the elements in reverse order.",
        // --- bytes ---
        "to_bytes" => "Encodes a string as UTF-8 and returns it as `bytes`.",
        "bytes_of" => "Builds a `bytes` value from an `[int]` of octets (each 0–255).",
        "sub_bytes" => "Returns the byte slice `[i, j)` by octet index. Out-of-range indices are clamped, so it never fails at runtime.",
        // --- Crypto (via ring) ---
        "sha256" => "Computes the SHA-256 digest of a bytes value; returns 32 bytes.",
        "sha512" => "Computes the SHA-512 digest of a bytes value; returns 64 bytes.",
        "sha1" => "Computes the SHA-1 digest of a bytes value; returns 20 bytes. Legacy algorithm: needed by some protocols (e.g. WebSocket), avoid for new designs.",
        "hmac_sha256" => "Computes the HMAC-SHA-256 of a message with the given key: `hmac_sha256(key: bytes, msg: bytes) -> bytes` (32 bytes).",
        "ed25519_verify" => "Verifies an Ed25519 signature: `ed25519_verify(pubkey: bytes, msg: bytes, sig: bytes) -> bool`.",
        // --- Map ---
        "insert" => "Inserts or updates a key/value pair in a Map, in place.",
        "add_to" => "Adds `delta` to the value at `key` (or sets it to `delta` if absent), in place — one lookup. For int/float value maps: the counting/accumulation idiom `m.add_to(k, 1)`.",
        "contains_key" => "Whether the Map contains the given key.",
        "keys" => "Returns the keys of a Map as a sorted array (deterministic order).",
        "values" => "Returns the values of a Map, in the same order as `keys()`.",
        // --- Matemáticas ---
        "sqrt" => "Square root of a float.",
        "sin" => "Sine of a float (radians).",
        "cos" => "Cosine of a float (radians).",
        "tan" => "Tangent of a float (radians).",
        "ln" => "Natural logarithm (base e) of a float.",
        "log10" => "Base-10 logarithm of a float.",
        "exp" => "e raised to the given float power.",
        "floor" => "Largest integer value not greater than the float, as float.",
        "ceil" => "Smallest integer value not less than the float, as float.",
        "round" => "Nearest integer value to the float, as float (half away from zero).",
        "pow" => "Raises `base` to the power `exp` (floats).",
        "abs" => "Absolute value. Works on int (returns int) and float (returns float).",
        "min" => "The smaller of two numbers. Works on two ints or two floats.",
        "max" => "The larger of two numbers. Works on two ints or two floats.",
        "pi" => "The constant π as a float.",
        "e" => "The constant e (Euler's number) as a float.",
        // --- Tiempo / azar ---
        "now" => "Current wall-clock time in milliseconds since the Unix epoch.",
        "monotonic" => "Monotonic clock reading in milliseconds; use for measuring durations (never goes backwards).",
        "monotonic_nanos" => "Monotonic clock reading in nanoseconds (same origin as `monotonic`); use for measuring sub-millisecond durations.",
        "sleep" => "Suspends the current fiber (or the program) for the given number of milliseconds.",
        "random" => "A pseudo-random float in `[0, 1)`.",
        "random_int" => "A pseudo-random int in `[0, n)`.",
        // --- Concurrencia (VM) ---
        "spawn" => "Starts a new concurrent task running the given closure and returns its `Task<T>` handle. Use `join(task)` to wait for its result. Requires the VM engine.",
        "scope" => "Runs the closure as a structured-concurrency scope: on return it joins every task spawned inside, cancelling siblings and re-raising the first failure.",
        "send" => "Sends a value into a channel. Blocks if the channel is bounded and full (backpressure).",
        "recv" => "Receives from a channel: blocks while it is empty and open; returns `None` once it is closed and drained.",
        "select" => "Blocks until one of the channels in the array is ready to receive and returns its index (lowest ready index; deterministic). Follow with `recv(chs[i])`.",
        "try_recv" => "Receives from a channel WITHOUT blocking: `Received.Got(v)` if a value was ready, `Received.Empty` if the channel is open but empty, `Received.Closed` if it is closed and drained.",
        "signals" => "Returns the process's OS-signal channel (SIGTERM=15, SIGINT=2, SIGWINCH=28 arrive as ints) for graceful shutdown and terminal-resize handling. A singleton; composes with `recv`/`select`. Unix only (VM and native binary).",
        "close" => "For a channel: closes it (pending values can still be received; `recv` then yields `None`). For a file handle: closes the file.",
        // --- I/O ---
        "__exists" => "Whether a file or directory exists at the given path.",
        "__local_port" => "Returns the local port a listener socket is bound to (useful with port 0 = OS-assigned).",
        _ => return None,
    })
}

/// Añade `contents` al final del archivo `path` (lo crea si no existe). Helper compartido por ambos
/// motores para el primitivo `__append_file` (M11.4b); la *impl* de ejecución no es metadato, pero
/// es idéntica en los dos motores, así que vive aquí para no duplicarse.
pub fn append_to_file(path: &str, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(contents.as_bytes())
}

/// M67: gemelo binario de `append_to_file` (primitivo `__append_file_bytes`).
pub fn append_bytes_to_file(path: &str, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(data)
}

/// M67: las operaciones de fs etiquetadas (mkdir/remove_dir/file_size/rename/copy_file), compartidas
/// por ambos motores. Devuelve el arreglo etiquetado ya montado (`["ok"(, dato)]`/`["err", msg]`) —
/// todas las cargas son strings, así cada motor solo lo convierte a su tipo de valor.
pub fn fs_tagged(op: crate::bytecode::FsOp, args: &[String]) -> Vec<String> {
    use crate::bytecode::FsOp;
    let r = match op {
        FsOp::Mkdir => std::fs::create_dir_all(&args[0]),
        // Solo directorios VACÍOS (el borrado recursivo es peligroso → a demanda).
        FsOp::RemoveDir => std::fs::remove_dir(&args[0]),
        FsOp::Rename => std::fs::rename(&args[0], &args[1]),
        FsOp::CopyFile => std::fs::copy(&args[0], &args[1]).map(|_| ()),
        FsOp::FileSize => {
            // ["ok", tamaño] (como el handle de `__open`); un directorio no tiene tamaño de archivo.
            return match std::fs::metadata(&args[0]) {
                Ok(md) if md.is_file() => vec!["ok".to_string(), md.len().to_string()],
                Ok(_) => vec!["err".to_string(), "no es un file".to_string()],
                Err(e) => vec!["err".to_string(), e.to_string()],
            };
        }
        FsOp::Stat => {
            // ["ok", kind, mode, size, mtime_ms] — metadatos SIN seguir symlinks (lstat): un
            // symlink se DETECTA (kind "symlink") en vez de seguirse a ciegas; los helpers
            // totales (is_dir/is_file/mtime) siguen resolviendo, como siempre. mode = los 12
            // bits de permiso en decimal (0o600 = 384; raylang aún no tiene literales octales).
            return match std::fs::symlink_metadata(&args[0]) {
                Ok(md) => {
                    let ft = md.file_type();
                    let kind = if ft.is_symlink() {
                        "symlink"
                    } else if ft.is_dir() {
                        "dir"
                    } else if ft.is_file() {
                        "file"
                    } else {
                        "other"
                    };
                    #[cfg(unix)]
                    let mode = {
                        use std::os::unix::fs::PermissionsExt;
                        (md.permissions().mode() & 0o7777) as u64
                    };
                    #[cfg(not(unix))]
                    let mode = 0u64;
                    let mtime = match md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {
                        Some(d) => d.as_millis().to_string(),
                        None => "0".to_string(),
                    };
                    vec!["ok".to_string(), kind.to_string(), mode.to_string(), md.len().to_string(), mtime]
                }
                Err(e) => vec!["err".to_string(), e.to_string()],
            };
        }
        FsOp::Mtime => {
            // ["ok", epoch_ms] — la última modificación en la MONEDA del tiempo (epoch-ms UTC,
            // como `now()`). Vale para archivos y directorios.
            return match std::fs::metadata(&args[0]).and_then(|md| md.modified()) {
                Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => vec!["ok".to_string(), d.as_millis().to_string()],
                    Err(_) => vec!["err".to_string(), "mtime before epoch".to_string()],
                },
                Err(e) => vec!["err".to_string(), e.to_string()],
            };
        }
    };
    match r {
        Ok(()) => vec!["ok".to_string()],
        Err(e) => vec!["err".to_string(), e.to_string()],
    }
}

/// M67: los tests totales de fs (`is_dir`/`is_file`), compartidos por ambos motores.
pub fn fs_test(t: crate::bytecode::FsTest, path: &str) -> bool {
    use crate::bytecode::FsTest;
    let p = std::path::Path::new(path);
    match t {
        FsTest::IsDir => p.is_dir(),
        FsTest::IsFile => p.is_file(),
    }
}

/// Índice de **carácter** de la primera ocurrencia de `sub` en `s` (M11.7a). Por carácter (no por
/// byte), consistente con `len`/`chars`/`s[i]`. `sub` vacío → `Some(0)`. Helper compartido por ambos
/// motores (`__index_of`).
pub fn char_index_of(s: &str, sub: &str) -> Option<usize> {
    // D1 (bench políglota, jsondeserialize): fast-path ASCII — índice de byte == índice de carácter,
    // y `str::find` va acelerado con memchr; el camino general materializaba DOS `Vec<char>` por
    // llamada (medido: VM −14% en jsondeserialize). El caso no-ASCII sigue por el camino por carácter.
    if s.is_ascii() {
        return s.find(sub);
    }
    let chars: Vec<char> = s.chars().collect();
    let sub: Vec<char> = sub.chars().collect();
    if sub.is_empty() {
        return Some(0);
    }
    if sub.len() > chars.len() {
        return None;
    }
    (0..=chars.len() - sub.len()).find(|&i| chars[i..i + sub.len()] == sub[..])
}

/// Subcadena `[i, j)` por índice de **carácter**, con *clamp* al rango válido (M11.7a): así nunca
/// falla en runtime (un `i`/`j` fuera de rango se recorta; `i > j` → `""`). Helper compartido.
pub fn substring_chars(s: &str, i: i64, j: i64) -> String {
    // D1 (bench políglota, jsondeserialize): fast-path ASCII — corte por bytes con el mismo clamp
    // (sin materializar el `Vec<char>` del string entero por llamada).
    if s.is_ascii() {
        let n = s.len() as i64;
        let lo = i.clamp(0, n);
        let hi = j.clamp(lo, n);
        return s[lo as usize..hi as usize].to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len() as i64;
    let lo = i.clamp(0, n);
    let hi = j.clamp(lo, n); // hi >= lo → rango vacío si i > j
    chars[lo as usize..hi as usize].iter().collect()
}

/// Sub-secuencia `[i, j)` de `bytes` por índice de **octeto**, con *clamp* al rango válido (M19.2): el
/// análogo de `substring_chars` para datos binarios → nunca falla en runtime. Helper compartido por
/// ambos motores (`sub_bytes`). Es lo que permite cortar cabeceras (texto) de cuerpo (binario) en HTTP.
pub fn sub_bytes_octets(b: &[u8], i: i64, j: i64) -> Vec<u8> {
    let n = b.len() as i64;
    let lo = i.clamp(0, n);
    let hi = j.clamp(lo, n); // hi >= lo → rango vacío si i > j
    b[lo as usize..hi as usize].to_vec()
}

// --- Cripto de PRODUCCIÓN vía `ring` (M43) ---
//
// Hashes de tiempo constante y auditados. A diferencia de las implementaciones en raylang puro
// (`examples/web/sha256.ray`, etc.), que se conservan como DEMOSTRACIÓN DEL LENGUAJE, estas son las que
// usa el código de producción (el paquete `net`): un hash sobre la VM interpretada no puede garantizar
// resistencia a canales laterales de temporización, requisito para tocar secretos reales. Helpers
// compartidos por ambos motores → la salida es idéntica (`ring` es determinista) y el oráculo se mantiene.

// M44a — En `wasm32` (el playground web) NO hay `ring` → la cripto no está disponible: el playground
// embarca solo el lenguaje NÚCLEO. Cada función se cfg-parte en su versión nativa (con `ring`) y un stub
// de wasm inofensivo (vacío/`None`/`false`); el *gating por checker* (M44a-4) hará que un programa que use
// cripto/red dé un error de compilación claro en el playground, así que estos stubs no se alcanzan.

/// M68.2: `n` octetos **criptográficamente seguros** (`ring::rand::SystemRandom`, el CSPRNG
/// del SO). Para tokens/salts/nonces — el SplitMix64 de `std/random` se siembra del reloj y
/// es predecible. `n <= 0` → vacío (total).
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn crypto_random_bytes(n: i64) -> Vec<u8> { ray_runtime::crypto::crypto_random_bytes(n) }
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn crypto_random_bytes(_n: i64) -> Vec<u8> { Vec::new() }

/// SHA-256 (32 octetos). El caballo de batalla de HMAC/JWT/firmas.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn sha256(data: &[u8]) -> Vec<u8> { ray_runtime::crypto::sha256(data) }
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn sha256(_data: &[u8]) -> Vec<u8> { Vec::new() }

/// SHA-512 (64 octetos).
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn sha512(data: &[u8]) -> Vec<u8> { ray_runtime::crypto::sha512(data) }
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn sha512(_data: &[u8]) -> Vec<u8> { Vec::new() }

/// SHA-1 (20 octetos). `ring` lo nombra `..._FOR_LEGACY_USE_ONLY`: roto para seguridad, se expone SOLO
/// para protocolos que aún lo exigen por diseño (p. ej. el accept-key de WebSocket, RFC 6455).
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn sha1(data: &[u8]) -> Vec<u8> { ray_runtime::crypto::sha1(data) }
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn sha1(_data: &[u8]) -> Vec<u8> { Vec::new() }

/// HMAC-SHA256 (32 octetos): MAC con clave, la base de JWT (HS256), SigV4 y muchos esquemas de auth.
/// La verificación honesta se hace **recomputando** el MAC y comparando en tiempo constante — pero eso es
/// responsabilidad de quien compara; aquí solo se produce la etiqueta.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> { ray_runtime::crypto::hmac_sha256(key, msg) }
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn hmac_sha256(_key: &[u8], _msg: &[u8]) -> Vec<u8> { Vec::new() }

// --- Ed25519 (firma de curva elíptica, M43.3) ---
//
// La semilla privada es de **exactamente 32 octetos**; `ring` falla si no. Devolvemos `Option` (→ el
// primitivo etiqueta `[]`/`[valor]` y el prelude lo envuelve): un tamaño de semilla malo es un dato
// inválido, no un ICE. `verify` es **total** (nunca falla; da `false` ante clave/firma inválidas).

/// Clave pública (32 octetos) derivada de una semilla de 32 octetos. `None` si la semilla no mide 32.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn ed25519_public_key(seed: &[u8]) -> Option<Vec<u8>> { ray_runtime::crypto::ed25519_public_key(seed) }
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn ed25519_public_key(_seed: &[u8]) -> Option<Vec<u8>> { None }

/// Firma (64 octetos) de `msg` con la semilla de 32 octetos. `None` si la semilla no mide 32. Ed25519 es
/// **determinista** (RFC 8032: el nonce se deriva por hash) → misma entrada, misma firma → el oráculo vale.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn ed25519_sign(seed: &[u8], msg: &[u8]) -> Option<Vec<u8>> { ray_runtime::crypto::ed25519_sign(seed, msg) }
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn ed25519_sign(_seed: &[u8], _msg: &[u8]) -> Option<Vec<u8>> { None }

/// Verifica que `sig` es una firma de `msg` bajo `pubkey`. Total: `false` ante cualquier entrada inválida.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn ed25519_verify(pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    ray_runtime::crypto::ed25519_verify(pubkey, msg, sig)
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn ed25519_verify(_pubkey: &[u8], _msg: &[u8], _sig: &[u8]) -> bool { false }

// --- ChaCha20-Poly1305 AEAD (cifrado autenticado, M43.4) ---
//
// La clave son 32 octetos y el nonce 12; `ring` falla si no. `seal` devuelve `texto_cifrado || etiqueta`
// (la etiqueta de 16 octetos va anexada); `open` la verifica y devuelve el texto plano, o `None` si la
// autenticación falla (dato manipulado) o los tamaños no cuadran. Ambos `Option` → primitivo `[bytes]`
// etiquetado + envoltorio en el prelude. Usamos `LessSafeKey` porque el nonce lo aporta quien llama (la
// API "segura" de `ring` gestiona el nonce por secuencia; aquí el primitivo es de más bajo nivel).

/// Cifra y autentica `plaintext` con `key` (32) y `nonce` (12), ligando `aad` (datos autenticados no
/// cifrados). Devuelve `texto_cifrado || etiqueta(16)`; `None` si `key`/`nonce` no miden lo debido.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn chacha20poly1305_seal(key: &[u8], nonce: &[u8], aad: &[u8], plaintext: &[u8]) -> Option<Vec<u8>> {
    ray_runtime::crypto::chacha20poly1305_seal(key, nonce, aad, plaintext)
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn chacha20poly1305_seal(_key: &[u8], _nonce: &[u8], _aad: &[u8], _plaintext: &[u8]) -> Option<Vec<u8>> { None }

/// Descifra y verifica `ciphertext_and_tag` (`texto_cifrado || etiqueta`) con `key`/`nonce`/`aad`. Devuelve
/// el texto plano, o `None` si la autenticación falla (manipulación) o los tamaños no cuadran.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn chacha20poly1305_open(key: &[u8], nonce: &[u8], aad: &[u8], ciphertext_and_tag: &[u8]) -> Option<Vec<u8>> {
    ray_runtime::crypto::chacha20poly1305_open(key, nonce, aad, ciphertext_and_tag)
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn chacha20poly1305_open(_key: &[u8], _nonce: &[u8], _aad: &[u8], _ciphertext_and_tag: &[u8]) -> Option<Vec<u8>> { None }

// --- Acuerdo de claves X25519 + HKDF (M114) ---
//
// La privada son **exactamente 32 octetos**, como la semilla de Ed25519 → misma forma (`Option`, primitivo
// `[bytes]` etiquetado, envoltorio en std/crypto). Detrás va `x25519-dalek` y no `ring`: ring solo entrega
// claves EFÍMERAS (sin constructor desde octetos), y aquí hacen falta clave persistible y determinismo.

/// Clave pública X25519 (32 octetos) desde una privada de 32. `None` si no mide 32.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn x25519_public_key(secret: &[u8]) -> Option<Vec<u8>> { ray_runtime::crypto::x25519_public_key(secret) }
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn x25519_public_key(_secret: &[u8]) -> Option<Vec<u8>> { None }

/// Secreto compartido X25519 (32 octetos). `None` si un tamaño no es 32 o si el resultado no es
/// contributorio (pública de orden pequeño → salida toda-ceros). Es el secreto DH CRUDO: pasa por HKDF.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn x25519_shared_secret(secret: &[u8], peer_public: &[u8]) -> Option<Vec<u8>> {
    ray_runtime::crypto::x25519_shared_secret(secret, peer_public)
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn x25519_shared_secret(_secret: &[u8], _peer_public: &[u8]) -> Option<Vec<u8>> { None }

/// HKDF-SHA256 (RFC 5869): `len` octetos derivados de `ikm`, ligando `salt` e `info`. `None` fuera de
/// `1..=8160`. Es lo que convierte un secreto DH en claves usables — y lo que las SEPARA por `info`.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], len: i64) -> Option<Vec<u8>> {
    ray_runtime::crypto::hkdf_sha256(salt, ikm, info, len)
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn hkdf_sha256(_salt: &[u8], _ikm: &[u8], _info: &[u8], _len: i64) -> Option<Vec<u8>> { None }

/// Compara dos `bytes` en tiempo constante. Total: `false` ante longitudes distintas.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool { ray_runtime::crypto::constant_time_eq(a, b) }
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn constant_time_eq(_a: &[u8], _b: &[u8]) -> bool { false }

// --- I/O con buffering: registro de archivos abiertos (M11.8) ---
//
// Un handle de archivo es un `int`: NO hay un nuevo tipo de valor ni se toca el GC. Los archivos
// abiertos viven en un almacén de **proceso** del host (como el de `args`), compartido por ambos
// motores. La lectura es **bufferizada** (`BufReader`), que es el grano fino del *streaming*: abrir
// una vez y leer/escribir por partes sin recargar todo el archivo.

/// Un recurso abierto: un archivo (lectura bufferizada o escritura) o un socket TCP (M15.2). Los
/// sockets reusan **el mismo registro** que los archivos para que `close(h)` (que solo quita del
/// mapa) cierre cualquiera de los dos sin saber de cuál se trata.
enum OpenHandle {
    Reader(std::io::BufReader<std::fs::File>),
    Writer(std::fs::File),
    Tcp(std::net::TcpStream),
    Listener(std::net::TcpListener),
    /// M19.4: una conexión TLS (cliente o servidor, rustls). Guarda la sesión + el socket juntos (la
    /// sesión es una máquina de estados mutable que no se puede clonar, a diferencia de `Tcp`). El
    /// intérprete la usa bloqueante (`rustls::Stream`); la VM, no bloqueante con cesión (M19.4b).
    /// M44a: no existe en `wasm32` (sin `rustls`) → el playground web no tiene TLS.
    #[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
    Tls(Box<TlsConn>),
    /// M20.8: un socket UDP (sin conexión). Se enlaza con `udp_bind` y se usa con `udp_send_to`/
    /// `udp_recv_from` (cada datagrama lleva su remitente). En el mismo registro de handles.
    Udp(std::net::UdpSocket),
    /// M53.3: una conexión SQLite embebida (rusqlite). En el mismo registro: `close(h)` la quita del
    /// mapa y el `Drop` de `Connection` cierra la base (statements ya finalizados: nunca escapan del
    /// helper). No existe en `wasm32` (rusqlite compila C) ni sin la feature `sqlite` (M89: build slim).
    #[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
    Sqlite(rusqlite::Connection),
    /// M100 v2: el extremo de LECTURA de un pipe de un proceso hijo, ya NO-bloqueante. Las bombas de
    /// `std/process` lo leen con `__socket_read_bytes`: el WouldBlock aparca la fibra en el poller
    /// exactamente como un socket (el `IoParked` de la VM es fd+deadline, nada socket-específico).
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    Pipe(std::fs::File),
    /// M100 v3: el extremo de ESCRITURA del stdin de un hijo VIVO (no-bloqueante), de
    /// `Cmd.stdin_pipe()`. Se escribe con `__proc_write` —que reusa el camino de escritura parcial
    /// de los sockets, así un pipe lleno APARCA la fibra por interés de escritura en vez de girar—
    /// y `close(h)` lo cierra: ese cierre ES el EOF que ve el hijo.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    PipeW(std::fs::File),
    /// M100 v2: un proceso hijo lanzado con `__proc_spawn`. Vive en el registro (no un pid crudo en
    /// el borde) para que `try_wait`/`kill` operen sobre el `Child` real — sin carreras de reuso de
    /// pid. `close(h)` lo quita del mapa SIN matar ni cosechar (el `Drop` de `Child` no hace nada);
    /// la cosecha es de `__proc_try_wait` y la estructura la pone `std/process` (bombas + wait).
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    Child(std::process::Child),
    /// M115.4: un watch de filesystem vivo (eventos de kernel vía notify, ray-runtime). En el
    /// registro común: `close(h)` lo quita y el `Drop` del watcher detiene sus hilos. La fibra
    /// aparca por el fd de su self-pipe (`FsWatcher::fd`), como un socket.
    #[cfg(all(feature = "watch", unix, not(target_arch = "wasm32")))]
    Watch(ray_runtime::watch::FsWatcher),
}

/// Una conexión TLS: la sesión rustls (cliente **o** servidor, vía el enum unificado `Connection`) +
/// su socket TCP subyacente. M44a: solo en targets no-wasm (usa `rustls`).
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
struct TlsConn {
    conn: rustls::Connection,
    sock: std::net::TcpStream,
}

/// El registro de archivos abiertos: un contador para los handles y el mapa handle → archivo.
struct FileRegistry {
    next: i64,
    open: std::collections::HashMap<i64, OpenHandle>,
}

fn registry() -> &'static std::sync::Mutex<FileRegistry> {
    static R: std::sync::OnceLock<std::sync::Mutex<FileRegistry>> = std::sync::OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(FileRegistry { next: 1, open: std::collections::HashMap::new() }))
}

/// Abre `path` en el modo dado (`"r"` lectura, `"w"` escritura/trunca, `"a"` añade) y devuelve un
/// handle (M11.8). Compartido por ambos motores (`__open`).
pub fn open_file(path: &str, mode: &str) -> Result<i64, String> {
    let handle = match mode {
        "r" => std::fs::File::open(path).map(|f| OpenHandle::Reader(std::io::BufReader::new(f))),
        "w" => std::fs::File::create(path).map(OpenHandle::Writer),
        "a" => std::fs::OpenOptions::new().create(true).append(true).open(path).map(OpenHandle::Writer),
        _ => return Err(format!("invalid open mode: '{}' (use \"r\", \"w\" or \"a\")", mode)),
    }
    .map_err(|e| e.to_string())?;
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, handle);
    Ok(id)
}

/// Lee la siguiente línea (sin el `\n`) del handle; `None` en EOF, error o handle no-lector (M11.8).
pub fn read_line_handle(h: i64) -> Option<String> {
    use std::io::BufRead;
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Reader(r)) => {
            let mut line = String::new();
            match r.read_line(&mut line) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
            }
        }
        _ => None,
    }
}

/// M113: lee hasta `max` octetos del handle (lector) desde su posición actual. `Ok(Some(datos))`
/// —exactamente `max` salvo cerca del final—, `Ok(None)` en EOF, `Err` en error o handle
/// no-lector. Compartido por ambos motores (`__read_bytes_handle`). Se lee con `take` +
/// `read_to_end`: la memoria es lo LEÍDO (un `max` desorbitado no pre-reserva nada) y no hay
/// lecturas cortas de por medio (importa en protocolos de chunks de tamaño fijo).
pub fn read_bytes_handle(h: i64, max: i64) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    if max <= 0 {
        return Err("read_bytes expects max > 0".to_string());
    }
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Reader(r)) => {
            let mut buf = Vec::new();
            match (&mut *r).take(max as u64).read_to_end(&mut buf) {
                Ok(0) => Ok(None),
                Ok(_) => Ok(Some(buf)),
                Err(e) => Err(e.to_string()),
            }
        }
        Some(_) => Err("the handle is not a file open for reading".to_string()),
        None => Err(format!("invalid file handle: {}", h)),
    }
}

/// M113: mueve la posición del handle de archivo (lector o escritor) a `pos` octetos desde el
/// inicio; devuelve la nueva posición. En un lector con buffer, `seek` descarta el buffer
/// (semántica de `BufReader`). (`__seek_handle`.)
pub fn seek_handle(h: i64, pos: i64) -> Result<i64, String> {
    use std::io::Seek;
    if pos < 0 {
        return Err("seek expects pos >= 0".to_string());
    }
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Reader(r)) => {
            r.seek(std::io::SeekFrom::Start(pos as u64)).map(|p| p as i64).map_err(|e| e.to_string())
        }
        Some(OpenHandle::Writer(f)) => {
            f.seek(std::io::SeekFrom::Start(pos as u64)).map(|p| p as i64).map_err(|e| e.to_string())
        }
        Some(_) => Err("the handle is not a file".to_string()),
        None => Err(format!("invalid file handle: {}", h)),
    }
}

/// Escribe `s` en el handle; `Ok(nº de caracteres)` o `Err(mensaje)` (M11.8).
/// M107.1 (std/io): escribe a stdout SIN salto de línea, por el MISMO lock que `print` (orden
/// consistente al intercalarlos). stdout va line-buffered → sin '\n' los datos pueden quedarse en
/// el buffer hasta `stdout_flush` (o el fin del proceso). En wasm acumula en el buffer del playground.
pub fn stdout_write(s: &str) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Write;
        std::io::stdout().lock().write_all(s.as_bytes()).map_err(|e| e.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        crate::wasm::push_stdout_raw(s);
        Ok(())
    }
}

/// M107.1 (std/io): como `stdout_write`, a stderr (sin buffer en Rust → visible al instante).
pub fn stderr_write(s: &str) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Write;
        std::io::stderr().lock().write_all(s.as_bytes()).map_err(|e| e.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        crate::wasm::push_stdout_raw(s); // el playground muestra stdout y stderr juntos
        Ok(())
    }
}

/// M107.1 (std/io): bytes crudos a stdout (secuencias de escape de terminal, salida binaria).
pub fn stdout_write_bytes(b: &[u8]) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Write;
        std::io::stdout().lock().write_all(b).map_err(|e| e.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        crate::wasm::push_stdout_raw(&String::from_utf8_lossy(b));
        Ok(())
    }
}

/// M107.1 (std/io): vacía el buffer de stdout.
pub fn stdout_flush() -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Write;
        std::io::stdout().lock().flush().map_err(|e| e.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    Ok(())
}

pub fn write_handle(h: i64, s: &str) -> Result<usize, String> {
    use std::io::Write;
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Writer(f)) => f.write_all(s.as_bytes()).map(|_| s.chars().count()).map_err(|e| e.to_string()),
        Some(OpenHandle::Reader(_)) => Err("the handle is open for reading, not writing".to_string()),
        Some(OpenHandle::Tcp(_)) => Err("the handle is a socket; use socket_write".to_string()),
        Some(OpenHandle::Listener(_)) => Err("the handle is a listening socket, not writable".to_string()),
        #[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
        Some(OpenHandle::Tls(_)) => Err("the handle is a TLS connection; use socket_write".to_string()),
        Some(OpenHandle::Udp(_)) => Err("the handle is a UDP socket; use udp_send_to".to_string()),
        #[cfg(not(target_arch = "wasm32"))]
        #[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
        Some(OpenHandle::Sqlite(_)) => Err("the handle is a SQLite connection; use db/sqlite".to_string()),
        // M100 v2: los handles de proceso no son escribibles (el stdin de v2 sigue siendo
        // escribir-y-cerrar en el spawn; un stdin por canal sería v3).
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        Some(OpenHandle::Pipe(_)) => Err("the handle is a child process pipe; read it with socket_read_bytes".to_string()),
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        Some(OpenHandle::PipeW(_)) => Err("the handle is a child's stdin; write it with proc_write".to_string()),
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        Some(OpenHandle::Child(_)) => Err("the handle is a child process; it is not writable".to_string()),
        #[cfg(all(feature = "watch", unix, not(target_arch = "wasm32")))]
        Some(OpenHandle::Watch(_)) => Err("the handle is a filesystem watch; it is not writable".to_string()),
        None => Err(format!("invalid file handle: {}", h)),
    }
}

/// Escribe octetos crudos en la posición actual del handle (M115.1). Gemelo binario de
/// `write_handle`; mismos mensajes que el helper nativo `__ray_write_bytes` (paridad).
pub fn write_bytes_handle(h: i64, data: &[u8]) -> Result<usize, String> {
    use std::io::Write;
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Writer(f)) => f.write_all(data).map(|_| data.len()).map_err(|e| e.to_string()),
        Some(OpenHandle::Reader(_)) => Err("the handle is open for reading, not writing".to_string()),
        Some(_) => Err("the handle is not a file open for writing".to_string()),
        None => Err(format!("invalid file handle: {}", h)),
    }
}

/// Vuelca los búferes y fuerza el archivo a almacenamiento estable — fsync (M115.1). Solo sobre
/// handles de escritura: es la pieza de durabilidad (WAL/AOF); en un lector no tiene sentido.
pub fn sync_handle(h: i64) -> Result<(), String> {
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Writer(f)) => f.sync_all().map_err(|e| e.to_string()),
        Some(OpenHandle::Reader(_)) => Err("the handle is open for reading, not writing".to_string()),
        Some(_) => Err("the handle is not a file open for writing".to_string()),
        None => Err(format!("invalid file handle: {}", h)),
    }
}

/// Cambia los bits de permiso del archivo — chmod, solo los 12 bits bajos (M115.3). En una
/// plataforma no-unix devuelve error (los permisos POSIX no existen ahí).
pub fn chmod_path(path: &str, mode: i64) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode((mode as u32) & 0o7777))
            .map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Err("chmod is not supported on this platform".to_string())
    }
}

/// Intenta el candado consultivo EXCLUSIVO del archivo sin bloquear — flock (M115.2).
/// `Ok(true)` = adquirido; `Ok(false)` = lo tiene otra open file description (otro proceso u otro
/// handle de este mismo proceso). Consultivo: solo protege frente a procesos que también lo piden.
/// Un lock BLOQUEANTE queda fuera a propósito: bloquearía el hilo entero (todas las fibras).
pub fn try_lock_handle(h: i64) -> Result<bool, String> {
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Writer(f)) => try_lock_file(f),
        Some(OpenHandle::Reader(r)) => try_lock_file(r.get_ref()),
        Some(_) => Err("the handle is not a file".to_string()),
        None => Err(format!("invalid file handle: {}", h)),
    }
}

fn try_lock_file(f: &std::fs::File) -> Result<bool, String> {
    match f.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e.to_string()),
    }
}

/// Suelta el candado consultivo del archivo (M115.2). Sin candado previo también es `Ok` (así se
/// comporta flock); `close(h)` lo suelta igualmente al morir la open file description.
pub fn unlock_handle(h: i64) -> Result<(), String> {
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Writer(f)) => f.unlock().map_err(|e| e.to_string()),
        Some(OpenHandle::Reader(r)) => r.get_ref().unlock().map_err(|e| e.to_string()),
        Some(_) => Err("the handle is not a file".to_string()),
        None => Err(format!("invalid file handle: {}", h)),
    }
}

// --- M115.4: watch de filesystem por eventos de kernel (feature `watch`; slim/wasm → stub). ---
#[cfg(any(not(all(feature = "watch", unix)), target_arch = "wasm32"))]
const WATCH_UNAVAILABLE: &str = if cfg!(target_arch = "wasm32") {
    "fs.watch is not available in the web playground (wasm)"
} else {
    "this binary was built without filesystem watch support (rebuild with the 'watch' feature)"
};

/// Abre un watch sobre la ruta (directorio → recursivo) y lo registra; `Ok(handle)`.
#[cfg(all(feature = "watch", unix, not(target_arch = "wasm32")))]
pub fn watch_open(path: &str) -> Result<i64, String> {
    let w = ray_runtime::watch::watch(path)?;
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, OpenHandle::Watch(w));
    Ok(id)
}
#[cfg(any(not(all(feature = "watch", unix)), target_arch = "wasm32"))]
pub fn watch_open(_path: &str) -> Result<i64, String> {
    Err(WATCH_UNAVAILABLE.to_string())
}

/// Recupera el watcher del handle o el error apropiado (factoriza fd/try_next/blocking).
#[cfg(all(feature = "watch", unix, not(target_arch = "wasm32")))]
fn watch_of(reg: &mut FileRegistry, h: i64) -> Result<&ray_runtime::watch::FsWatcher, String> {
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Watch(w)) => Ok(w),
        Some(_) => Err("the handle is not a watch handle".to_string()),
        None => Err(format!("invalid handle: {}", h)),
    }
}

/// El fd por el que aparca la fibra de la VM (self-pipe del watcher).
#[cfg(all(feature = "watch", unix, not(target_arch = "wasm32")))]
pub fn watch_fd(h: i64) -> Result<i32, String> {
    let mut reg = registry().lock().unwrap();
    watch_of(&mut reg, h).map(|w| w.fd())
}
#[cfg(any(not(all(feature = "watch", unix)), target_arch = "wasm32"))]
pub fn watch_fd(_h: i64) -> Result<i32, String> {
    Err(WATCH_UNAVAILABLE.to_string())
}

/// El siguiente evento si ya hay uno, sin bloquear: `Ok(Some((kind, path)))` / `Ok(None)`.
#[cfg(all(feature = "watch", unix, not(target_arch = "wasm32")))]
pub fn watch_try_next(h: i64) -> Result<Option<(String, String)>, String> {
    let mut reg = registry().lock().unwrap();
    watch_of(&mut reg, h).map(|w| w.try_next())
}
#[cfg(any(not(all(feature = "watch", unix)), target_arch = "wasm32"))]
pub fn watch_try_next(_h: i64) -> Result<Option<(String, String)>, String> {
    Err(WATCH_UNAVAILABLE.to_string())
}

/// El siguiente evento BLOQUEANDO el hilo (el intérprete, oráculo de desarrollo): sondea la cola
/// y aparca en poll(2) sobre el fd del watcher — nunca retiene el lock del registro mientras
/// espera. `ms <= 0` = sin plazo; `Ok(None)` = plazo vencido.
#[cfg(all(feature = "watch", unix, not(target_arch = "wasm32")))]
pub fn watch_next_blocking(h: i64, ms: i64) -> Result<Option<(String, String)>, String> {
    let deadline = if ms > 0 {
        Some(std::time::Instant::now() + std::time::Duration::from_millis(ms as u64))
    } else {
        None
    };
    loop {
        let fd = {
            let mut reg = registry().lock().unwrap();
            let w = watch_of(&mut reg, h)?;
            if let Some(ev) = w.try_next() {
                return Ok(Some(ev));
            }
            w.fd()
        };
        let wait = match deadline {
            None => 200, // re-sondea el registro (un close concurrente no debe colgar esto)
            Some(d) => {
                let rem = d.saturating_duration_since(std::time::Instant::now()).as_millis() as i64;
                if rem <= 0 {
                    return Ok(None);
                }
                rem.min(200) as i32
            }
        };
        watch_mod::fd_ready(fd, wait);
    }
}
#[cfg(any(not(all(feature = "watch", unix)), target_arch = "wasm32"))]
pub fn watch_next_blocking(_h: i64, _ms: i64) -> Result<Option<(String, String)>, String> {
    Err(WATCH_UNAVAILABLE.to_string())
}

#[cfg(all(feature = "watch", unix, not(target_arch = "wasm32")))]
mod watch_mod {
    unsafe extern "C" {
        fn poll(fds: *mut PollFd, nfds: u64, timeout_ms: i32) -> i32;
    }
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    const POLLIN: i16 = 0x0001;

    /// ¿Hay lectura pendiente en `fd` dentro de `timeout_ms`? (Solo espera; el dato lo drena el
    /// watcher.)
    pub(super) fn fd_ready(fd: i32, timeout_ms: i32) -> bool {
        let mut pfd = PollFd { fd, events: POLLIN, revents: 0 };
        // SAFETY: un solo PollFd bien formado; poll no retiene el puntero tras volver.
        unsafe { poll(&mut pfd, 1, timeout_ms) > 0 }
    }
}

/// Cierra el handle (lo quita del registro; el `Drop` del archivo/socket libera el recurso) (M11.8).
pub fn close_handle(h: i64) {
    registry().lock().unwrap().open.remove(&h);
    // M56.4: limpia el estado de timeout del socket (los ids no se reusan; es solo higiene).
    read_timeouts().lock().unwrap().remove(&h);
    read_expired().lock().unwrap().remove(&h);
}

// M89.2: ¿este binario trae cripto/TLS (ring/rustls)? Los motores y el CLI lo consultan para
// dar un error CLARO (nunca un hash vacío ni una verificación que "pasa" en silencio).
pub fn net_tls_available() -> bool {
    cfg!(all(feature = "net-tls", not(target_arch = "wasm32")))
}

/// El mensaje del stub/guardia, por motivo: el playground web (wasm) o un binario slim (M89.2).
pub const NET_TLS_UNAVAILABLE: &str = if cfg!(target_arch = "wasm32") {
    "crypto/TLS not available in the web playground (wasm)"
} else {
    "this binary was built without crypto/TLS support (rebuild with the 'net-tls' feature)"
};

// --- SQLite embebido (M53.3, vía rusqlite) ---
//
// Como la cripto de M43 (`ring`): territorio donde envolver la librería C a mano (dobles punteros,
// lifetimes de statements, destructores de bind) es peor ingeniería que delegar en el binding maduro.
// El handle es un `int` en el MISMO registro que archivos/sockets → `close(h)` cierra la conexión.
// El ciclo prepare→bind→step→finalize ocurre ENTERO dentro de cada helper (el statement nunca escapa)
// → sin use-after-finalize posible. Las celdas se devuelven como texto (INTEGER/REAL → repr decimal,
// NULL → "", BLOB → hex), consistente con la API `[[string]]` de `db/mysql` y `db/postgres`.
// Nota: a diferencia de los sockets (que clonan el descriptor), la conexión no es clonable → el lock
// del registro se retiene durante la consulta. Aceptable: es I/O local, y serializa entre fibras.

/// Convierte una celda SQLite a su representación de texto para la API `[[string]]`.
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
fn sqlite_value_str(v: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => String::new(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Blob(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
    }
}

/// Abre (o crea) la base en `path` (`":memory:"` = en memoria) y devuelve un handle (M53.3).
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub fn sqlite_open(path: &str) -> Result<i64, String> {
    let conn = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, OpenHandle::Sqlite(conn));
    Ok(id)
}
// El mensaje del stub distingue el motivo: el playground web (wasm) o un binario slim (M89).
#[cfg(any(not(feature = "sqlite"), target_arch = "wasm32"))]
const SQLITE_UNAVAILABLE: &str = if cfg!(target_arch = "wasm32") {
    "SQLite not available in the web playground (wasm)"
} else {
    "this binary was built without SQLite support (rebuild with the 'sqlite' feature)"
};
#[cfg(any(not(feature = "sqlite"), target_arch = "wasm32"))]
pub fn sqlite_open(_path: &str) -> Result<i64, String> { Err(SQLITE_UNAVAILABLE.to_string()) }

/// Recupera la conexión del handle o el error apropiado. Factoriza la validación de exec/query.
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
fn sqlite_conn(reg: &mut FileRegistry, h: i64) -> Result<&mut rusqlite::Connection, String> {
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Sqlite(conn)) => Ok(conn),
        Some(_) => Err("the handle is not a SQLite connection".to_string()),
        None => Err("invalid or already closed handle".to_string()),
    }
}

/// Ejecuta una sentencia sin filas (INSERT/UPDATE/DDL/BEGIN/…) con parámetros posicionales (`?1`…)
/// enlazados como texto; devuelve el número de filas afectadas (M53.3).
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub fn sqlite_exec(h: i64, sql: &str, params: &[String]) -> Result<i64, String> {
    let mut reg = registry().lock().unwrap();
    let conn = sqlite_conn(&mut reg, h)?;
    conn.execute(sql, rusqlite::params_from_iter(params.iter()))
        .map(|n| n as i64)
        .map_err(|e| e.to_string())
}
#[cfg(any(not(feature = "sqlite"), target_arch = "wasm32"))]
pub fn sqlite_exec(_h: i64, _sql: &str, _params: &[String]) -> Result<i64, String> { Err(SQLITE_UNAVAILABLE.to_string()) }

/// Ejecuta una consulta con filas; devuelve `(ncols, celdas)` con las celdas aplanadas fila a fila
/// (el envoltorio raylang reconstruye el `[[string]]`) (M53.3).
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub fn sqlite_query(h: i64, sql: &str, params: &[String]) -> Result<(usize, Vec<String>), String> {
    let mut reg = registry().lock().unwrap();
    let conn = sqlite_conn(&mut reg, h)?;
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let ncols = stmt.column_count();
    let mut rows = stmt.query(rusqlite::params_from_iter(params.iter())).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                // Copiar cada celda ANTES de avanzar: el texto de un ValueRef solo vive hasta el
                // siguiente paso del statement.
                for i in 0..ncols {
                    out.push(sqlite_value_str(row.get_ref(i).map_err(|e| e.to_string())?));
                }
            }
            Ok(None) => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok((ncols, out))
}
#[cfg(any(not(feature = "sqlite"), target_arch = "wasm32"))]
pub fn sqlite_query(_h: i64, _sql: &str, _params: &[String]) -> Result<(usize, Vec<String>), String> { Err(SQLITE_UNAVAILABLE.to_string()) }

// --- Cliente TCP (M15.2) ---
//
// Sobre `std::net::TcpStream`, cero deps. El handle es un `int` y vive en el MISMO registro que los
// archivos. Para no retener el `Mutex` del registro durante un I/O **bloqueante**, los helpers de
// lectura/escritura **clonan** el stream (`try_clone` = `dup` del descriptor) y sueltan el lock antes.

/// Conecta a `host:port` (resuelve el nombre vía `std::net`) y devuelve un handle (M15.2).
pub fn tcp_connect(host: &str, port: i64) -> Result<i64, String> {
    let stream = std::net::TcpStream::connect((host, port as u16)).map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true); // Nagle+delayed-ACK = stalls fijos de 40-100 ms (M96b)
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, OpenHandle::Tcp(stream));
    Ok(id)
}

/// Error estable de un connect que agota su plazo (M122), hermano de `READ_TIMEOUT_MSG`.
pub const CONNECT_TIMEOUT_MSG: &str = "connect timeout";

/// La dirección del peer (`"ip:puerto"`, IPv6 con corchetes) de una conexión TCP o TLS (M123).
/// La pieza que le faltaba a cualquier servidor real: rate-limit por IP, X-Forwarded-For, logs de
/// acceso con origen (hallazgo de raygate, IDEAS §65.1).
pub fn peer_addr(h: i64) -> Result<String, String> {
    let reg = registry().lock().unwrap();
    match reg.open.get(&h) {
        Some(OpenHandle::Tcp(s)) => s.peer_addr().map(|a| a.to_string()).map_err(|e| e.to_string()),
        #[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
        Some(OpenHandle::Tls(tc)) => tc.sock.peer_addr().map(|a| a.to_string()).map_err(|e| e.to_string()),
        Some(_) => Err(format!("handle {} is not a TCP/TLS socket", h)),
        None => Err(format!("invalid handle: {}", h)),
    }
}

/// Como [`tcp_connect`], con PLAZO (M122): un host que no responde al SYN (firewall que descarta,
/// ruta negra) retenía la conexión ~75 s (el timeout del SO); con `ms` el intento falla con el
/// error estable "connect timeout". `ms <= 0` = sin plazo (idéntico a `tcp_connect`). La espera es
/// **acotada pero bloqueante** (`TcpStream::connect_timeout` del std); la resolución del nombre
/// (getaddrinfo) va aparte y no entra en el plazo.
pub fn tcp_connect_timeout(host: &str, port: i64, ms: i64) -> Result<i64, String> {
    if ms <= 0 {
        return tcp_connect(host, port);
    }
    use std::net::ToSocketAddrs;
    let addr = (host, port as u16)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| format!("could not resolve host '{}'", host))?;
    let stream = match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(ms as u64)) {
        Ok(s) => s,
        Err(ref e)
            if e.kind() == std::io::ErrorKind::TimedOut
                || e.kind() == std::io::ErrorKind::WouldBlock =>
        {
            return Err(CONNECT_TIMEOUT_MSG.to_string());
        }
        Err(e) => return Err(e.to_string()),
    };
    let _ = stream.set_nodelay(true); // mismo trato que tcp_connect (M96b)
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, OpenHandle::Tcp(stream));
    Ok(id)
}

/// Saca un clon del stream del handle `h` (suelta el lock antes del I/O bloqueante), o un error si
/// el handle no es un socket.
fn socket_clone(h: i64) -> Result<std::net::TcpStream, String> {
    let reg = registry().lock().unwrap();
    match reg.open.get(&h) {
        Some(OpenHandle::Tcp(s)) => s.try_clone().map_err(|e| e.to_string()),
        Some(_) => Err(format!("handle {} is not a socket", h)),
        None => Err(format!("invalid handle: {}", h)),
    }
}

/// Hace **una** lectura del socket (hasta 64 KiB) y devuelve lo leído como `string` (UTF-8 *lossy*);
/// `""` indica EOF (el otro extremo cerró). Bloquea hasta que haya datos (M15.2).
pub fn socket_read(h: i64) -> Result<String, String> {
    use std::io::Read;
    let mut stream = socket_clone(h)?;
    let mut buf = [0u8; 65536];
    match stream.read(&mut buf) {
        Ok(n) => Ok(String::from_utf8_lossy(&buf[..n]).into_owned()),
        // M56.4: con SO_RCVTIMEO puesto (motor bloqueante), la espera vencida llega como
        // WouldBlock/TimedOut (según SO) → el mismo mensaje que en la VM.
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
            Err(READ_TIMEOUT_MSG.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Escribe `s` completo en el socket; `Ok(nº de bytes)` o `Err(mensaje)` (M15.2).
///
/// Bucle de escritura manual (no `write_all`) para tolerar sockets **no bloqueantes** (M15.5): en un
/// socket bloqueante `write` nunca da `WouldBlock` y esto equivale a `write_all`; en uno no bloqueante,
/// gira (`yield_now`) hasta poder escribir. La escritura NO es punto de cesión del scheduler (cargas
/// reales —líneas, respuestas cortas— nunca giran; una escritura gigante a un peer que no lee sí).
pub fn socket_write(h: i64, s: &str) -> Result<usize, String> {
    socket_write_raw(h, s.as_bytes())
}

/// Núcleo de la escritura: escribe `bytes` completos en el socket. Tolera sockets no bloqueantes
/// (gira en `WouldBlock`). Lo usan `socket_write` (M15.2) y `socket_write_bytes` (M16.1c).
pub fn socket_write_raw(h: i64, bytes: &[u8]) -> Result<usize, String> {
    use std::io::Write;
    // M19.4: un handle TLS se cifra por la bomba TLS (sobre socket bloqueante, no gira). TCP normal si no.
    if is_tls_handle(h) {
        return tls_write_nb(h, bytes);
    }
    // M100 v3: el stdin de un hijo vivo. Camino del INTÉRPRETE (sin scheduler): gira en WouldBlock
    // hasta colocarlo todo, como el TCP de arriba.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    if is_child_stdin(h) {
        let mut off = 0;
        while off < bytes.len() {
            match pipe_write_once(h, &bytes[off..]) {
                Ok(0) => std::thread::yield_now(),
                Ok(n) => off += n,
                Err(e) => return Err(e),
            }
        }
        return Ok(bytes.len());
    }
    let mut stream = socket_clone(h)?;
    let mut off = 0;
    while off < bytes.len() {
        match stream.write(&bytes[off..]) {
            Ok(0) => return Err("the connection closed during the write".to_string()),
            Ok(n) => off += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::yield_now(),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(bytes.len())
}

/// Escritura **parcial no bloqueante** (VM): escribe lo que quepa en el buffer de envío del socket y
/// devuelve cuántos octetos entraron (`Ok(n)`). `n == bytes.len()` → completa; `n < len` → el buffer se
/// llenó (`WouldBlock`) y el resto (`bytes[n..]`) hay que reintentarlo cuando el socket sea **escribible**
/// (el scheduler aparca la fibra con interés de escritura, M19.4b post — cesión en `socket_write`).
pub fn socket_write_nb(h: i64, bytes: &[u8]) -> Result<usize, String> {
    use std::io::Write;
    // M100 v3: stdin de un hijo → escritura parcial sobre el pipe; el resto lo re-intenta la VM
    // tras aparcar la fibra por interés de escritura (`park_write`/`finish_parked_write`, que
    // llaman aquí de nuevo). Mismo contrato que el socket: `Ok(n)` con `n < len` = se llenó.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    if is_child_stdin(h) {
        let mut off = 0;
        while off < bytes.len() {
            match pipe_write_once(h, &bytes[off..]) {
                Ok(0) => break, // WouldBlock: el pipe está lleno
                Ok(n) => off += n,
                Err(e) => return Err(e),
            }
        }
        return Ok(off);
    }
    let mut stream = socket_clone(h)?;
    let mut off = 0;
    while off < bytes.len() {
        match stream.write(&bytes[off..]) {
            Ok(0) => return Err("the connection closed during the write".to_string()),
            Ok(n) => off += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(off)
}

/// M100 v3: ¿el handle es el stdin (escribible) de un hijo vivo?
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn is_child_stdin(h: i64) -> bool {
    matches!(registry().lock().unwrap().open.get(&h), Some(OpenHandle::PipeW(_)))
}

/// M100 v3: UN intento de escritura sobre el stdin del hijo (fd no-bloqueante). `Ok(0)` = el pipe
/// está lleno (`WouldBlock`); `Err` = el hijo cerró su stdin o murió (EPIPE) — el error que un
/// cliente de sesión (MCP/LSP) necesita ver, no un silencio.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn pipe_write_once(h: i64, bytes: &[u8]) -> Result<usize, String> {
    use std::io::Write;
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::PipeW(f)) => match f.write(bytes) {
            Ok(n) => Ok(n),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                Ok(0)
            }
            Err(e) => Err(e.to_string()),
        },
        _ => Err(format!("invalid child stdin handle: {}", h)),
    }
}

// --- TLS (M19.4) ---
//
// La ÚNICA parte del runtime con una dependencia externa (`rustls`, decisión §28.4). Una conexión TLS
// (cliente o servidor) vive en el MISMO registro de handles (`OpenHandle::Tls`), así que `close(h)` la
// cierra igual que un socket o un archivo, y `socket_read_bytes`/`socket_write_bytes` la manejan (se
// desvían a los caminos TLS). Dos modos de I/O sobre la MISMA sesión rustls:
//   - **Bloqueante** (intérprete, sin scheduler): `rustls::Stream` sobre el socket bloqueante.
//   - **No bloqueante con cesión** (VM, M19.4b): se conduce la máquina de estados a mano (`read_tls`/
//     `write_tls`/`process_new_packets`) sobre un socket no bloqueante; si haría falta LEER del peer y
//     bloquearía, se devuelve "WouldBlock" y la VM **aparca la fibra** en el fd (como un socket normal).
//     Las escrituras (handshake/datos) caben casi siempre en el buffer de envío del SO; en el raro
//     `WouldBlock` de escritura se gira (`yield_now`), porque el poller de M17 solo notifica lectura.

/// La configuración de cliente TLS (raíces de Mozilla vía `webpki-roots` + `SSL_CERT_FILE`). Verifica
/// el certificado del servidor como un navegador. Se construye una vez y se comparte.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
fn tls_client_config() -> std::sync::Arc<rustls::ClientConfig> {
    static C: std::sync::OnceLock<std::sync::Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        // Igual que curl/OpenSSL: `SSL_CERT_FILE` añade certificados de confianza extra (una CA propia,
        // un proxy corporativo, o —en las pruebas— una CA autofirmada local). Se ignoran los inválidos.
        if let Ok(path) = std::env::var("SSL_CERT_FILE") {
            use rustls::pki_types::pem::PemObject;
            if let Ok(certs) = rustls::pki_types::CertificateDer::pem_file_iter(&path) {
                for cert in certs.flatten() {
                    let _ = roots.add(cert);
                }
            }
        }
        let cfg = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        std::sync::Arc::new(cfg)
    })
    .clone()
}

/// Abre una conexión TLS de cliente a `host:port` (handshake en la primera I/O); el `host` valida el
/// certificado (SNI). Builtin `__tls_connect` (M19.4a).
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn tls_connect(host: &str, port: i64) -> Result<i64, String> {
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| format!("invalid server name for TLS: {host}"))?;
    let client = rustls::ClientConnection::new(tls_client_config(), server_name)
        .map_err(|e| e.to_string())?;
    let sock = std::net::TcpStream::connect((host, port as u16)).map_err(|e| e.to_string())?;
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, OpenHandle::Tls(Box::new(TlsConn { conn: rustls::Connection::Client(client), sock })));
    Ok(id)
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn tls_connect(_host: &str, _port: i64) -> Result<i64, String> { Err(NET_TLS_UNAVAILABLE.to_string()) }

/// M124: el resumen del certificado del PEER de una conexión TLS — "expira en N días" es EL check
/// que todo operador quiere (raywatch, IDEAS §70.1). `tls_connect` deja el handshake para la
/// primera I/O, así que aquí se CONDUCE si sigue pendiente (acotado a 10 s): en la VM el socket es
/// no bloqueante → WouldBlock espera readiness con el poller; en el intérprete bloquea.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn tls_peer_cert(h: i64) -> Result<ray_runtime::x509::CertSummary, String> {
    use std::os::fd::AsRawFd;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Tls(tc)) => {
            while tc.conn.is_handshaking() {
                match tc.conn.complete_io(&mut tc.sock) {
                    Ok(_) => {}
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        if std::time::Instant::now() >= deadline {
                            return Err("TLS handshake timeout".to_string());
                        }
                        let fd = tc.sock.as_raw_fd();
                        let (rd, wr): (&[i32], &[i32]) =
                            if tc.conn.wants_write() { (&[], &[fd]) } else { (&[fd], &[]) };
                        let _ = crate::poll::wait(rd, wr, 1000);
                    }
                    Err(e) => return Err(e.to_string()),
                }
            }
            let der = tc
                .conn
                .peer_certificates()
                .and_then(|c| c.first())
                .map(|c| c.to_vec())
                .ok_or_else(|| "the peer presented no certificate".to_string())?;
            ray_runtime::x509::cert_summary(&der)
        }
        Some(_) => Err(format!("handle {} is not a TLS connection", h)),
        None => Err(format!("invalid handle: {}", h)),
    }
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn tls_peer_cert(_h: i64) -> Result<ray_runtime::x509::CertSummary, String> {
    Err(NET_TLS_UNAVAILABLE.to_string())
}

/// M31.2a: conexión TLS de cliente ofreciendo **ALPN `h2`** (HTTP/2). Conecta, **completa el handshake**
/// (bloqueante) y exige que el servidor negocie `h2`; si no, error. Devuelve el handle (reusa el mismo
/// registro/rutas de I/O que `tls_connect`). Builtin `__tls_connect_h2`.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn tls_connect_h2(host: &str, port: i64) -> Result<i64, String> {
    // Config propia (la cacheada no lleva ALPN); reusa el mismo almacén de raíces + SSL_CERT_FILE.
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Ok(path) = std::env::var("SSL_CERT_FILE") {
        use rustls::pki_types::pem::PemObject;
        if let Ok(certs) = rustls::pki_types::CertificateDer::pem_file_iter(&path) {
            for cert in certs.flatten() {
                let _ = roots.add(cert);
            }
        }
    }
    let mut cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec()];

    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| format!("invalid server name for TLS: {host}"))?;
    let mut client = rustls::ClientConnection::new(std::sync::Arc::new(cfg), server_name)
        .map_err(|e| e.to_string())?;
    let mut sock = std::net::TcpStream::connect((host, port as u16)).map_err(|e| e.to_string())?;
    // Handshake bloqueante hasta terminar (para poder consultar el ALPN negociado).
    while client.is_handshaking() {
        client.complete_io(&mut sock).map_err(|e| e.to_string())?;
    }
    match client.alpn_protocol() {
        Some(p) if p == b"h2" => {}
        _ => return Err("the server did not negotiate HTTP/2 (ALPN 'h2')".to_string()),
    }
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, OpenHandle::Tls(Box::new(TlsConn { conn: rustls::Connection::Client(client), sock })));
    Ok(id)
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn tls_connect_h2(_host: &str, _port: i64) -> Result<i64, String> { Err(NET_TLS_UNAVAILABLE.to_string()) }

/// Construye una configuración de servidor TLS a partir de los PEM de la cadena de certificados y la
/// clave privada (M19.4b). Cada servidor puede tener su propio certificado, así que NO se cachea.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
fn tls_server_config(cert_pem: &str, key_pem: &str) -> Result<std::sync::Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::pem::PemObject;
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls::pki_types::CertificateDer::pem_slice_iter(cert_pem.as_bytes())
            .collect::<Result<_, _>>()
            .map_err(|e| format!("invalid certificate: {e}"))?;
    if certs.is_empty() {
        return Err("the PEM contains no certificate".to_string());
    }
    let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .map_err(|e| format!("invalid private key: {e}"))?;
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    Ok(std::sync::Arc::new(cfg))
}

/// Convierte una conexión TCP ya aceptada (handle `h`, `OpenHandle::Tcp`) en una conexión TLS de
/// **servidor** con el certificado/clave dados (M19.4b). Reusa el MISMO handle (saca el socket del
/// registro y lo reinserta envuelto). El handshake ocurre en la primera I/O. Builtin `__tls_accept`.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn tls_accept(h: i64, cert_pem: &str, key_pem: &str) -> Result<i64, String> {
    let config = tls_server_config(cert_pem, key_pem)?;
    let server = rustls::ServerConnection::new(config).map_err(|e| e.to_string())?;
    let mut reg = registry().lock().unwrap();
    let sock = match reg.open.remove(&h) {
        Some(OpenHandle::Tcp(s)) => s,
        Some(other) => { reg.open.insert(h, other); return Err(format!("handle {h} is not an accepted TCP socket")); }
        None => return Err(format!("invalid handle: {h}")),
    };
    reg.open.insert(h, OpenHandle::Tls(Box::new(TlsConn { conn: rustls::Connection::Server(server), sock })));
    Ok(h)
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn tls_accept(_h: i64, _cert_pem: &str, _key_pem: &str) -> Result<i64, String> { Err(NET_TLS_UNAVAILABLE.to_string()) }

/// Envuelve un socket TCP plano YA CONECTADO (handle `h`) en una sesión TLS de **cliente** —
/// el simétrico de `tls_accept`, para STARTTLS (Postgres sslRequest, MySQL caching_sha2
/// full-path, SMTP…). Verifica el certificado del servidor contra `host` con la misma config
/// (raíces Mozilla + SSL_CERT_FILE) que `tls_connect`. **Reusa el mismo handle**: el I/O
/// existente se desvía solo a TLS vía `is_tls_handle`; el modo (no) bloqueante del socket se
/// conserva (en la VM ya es no bloqueante → el handshake lo conduce el primer I/O, cediendo).
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn tls_upgrade(h: i64, host: &str) -> Result<i64, String> {
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| format!("invalid server name for TLS: {host}"))?;
    let client = rustls::ClientConnection::new(tls_client_config(), server_name)
        .map_err(|e| e.to_string())?;
    let mut reg = registry().lock().unwrap();
    let sock = match reg.open.remove(&h) {
        Some(OpenHandle::Tcp(s)) => s,
        Some(other) => { reg.open.insert(h, other); return Err(format!("handle {h} is not a plain TCP socket")); }
        None => return Err(format!("invalid handle: {h}")),
    };
    reg.open.insert(h, OpenHandle::Tls(Box::new(TlsConn { conn: rustls::Connection::Client(client), sock })));
    Ok(h)
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn tls_upgrade(_h: i64, _host: &str) -> Result<i64, String> { Err(NET_TLS_UNAVAILABLE.to_string()) }

/// ¿El handle `h` es una conexión TLS? Lo consultan los caminos de socket para desviarse al I/O TLS.
/// M44a: en wasm nunca hay handles TLS (no se pueden crear) → siempre `false`, y los caminos TLS de
/// `socket_write_raw`/`socket_read_bytes_blocking`/la VM quedan muertos.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn is_tls_handle(h: i64) -> bool {
    matches!(registry().lock().unwrap().open.get(&h), Some(OpenHandle::Tls(_)))
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn is_tls_handle(_h: i64) -> bool { false }

/// Pone el socket subyacente de una conexión TLS en modo no bloqueante (lo hace la VM tras connect/
/// accept, para que el I/O TLS pueda ceder la fibra). M19.4b.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn tls_set_nonblocking(h: i64) -> Result<(), String> {
    let reg = registry().lock().unwrap();
    match reg.open.get(&h) {
        Some(OpenHandle::Tls(tc)) => tc.sock.set_nonblocking(true).map_err(|e| e.to_string()),
        _ => Err(format!("handle {h} is not a TLS connection")),
    }
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn tls_set_nonblocking(_h: i64) -> Result<(), String> { Err(NET_TLS_UNAVAILABLE.to_string()) }

/// Drena las escrituras TLS pendientes (handshake/datos) al socket no bloqueante. Gira en `WouldBlock`
/// (el buffer de envío rara vez se llena con tramas pequeñas; el poller de M17 solo notifica lectura).
/// M44a: solo no-wasm (toma `&mut TlsConn`, que no existe en wasm).
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
fn tls_flush_writes(tc: &mut TlsConn) -> Result<(), String> {
    while tc.conn.wants_write() {
        match tc.conn.write_tls(&mut tc.sock) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::yield_now(),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

/// Lectura TLS **no bloqueante** (VM, M19.4b): conduce el handshake/transporte y devuelve datos de
/// aplicación. `Ok(Some(data))` = datos (vacío en cierre limpio), `Ok(None)` = bloquearía leyendo del
/// peer (la VM aparca la fibra en el fd), `Err` en fallo de protocolo.
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn tls_read_nb(h: i64) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    // M56.4: la espera de esta lectura venció (marcada por el scheduler) → error de timeout.
    if take_read_expired(h) {
        return Err(READ_TIMEOUT_MSG.to_string());
    }
    let mut reg = registry().lock().unwrap();
    let tc = match reg.open.get_mut(&h) {
        Some(OpenHandle::Tls(tc)) => tc,
        _ => return Err(format!("handle {h} is not a TLS connection")),
    };
    loop {
        // 1) Enviar lo pendiente (ServerHello, datos…) antes de esperar al peer; si no, deadlock.
        tls_flush_writes(tc)?;
        // 2) ¿Hay ya texto plano descifrado disponible?
        let mut buf = [0u8; 65536];
        match tc.conn.reader().read(&mut buf) {
            Ok(0) => return Ok(Some(Vec::new())),            // close_notify → EOF limpio
            Ok(n) => return Ok(Some(buf[..n].to_vec())),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {} // no hay texto plano todavía
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(Some(Vec::new())),
            Err(e) => return Err(e.to_string()),
        }
        // 3) Necesitamos más registros del peer: leer del socket (no bloqueante).
        match tc.conn.read_tls(&mut tc.sock) {
            Ok(0) => return Ok(Some(Vec::new())),            // el peer cerró el TCP
            Ok(_) => {
                tc.conn.process_new_packets().map_err(|e| e.to_string())?;
                // tras procesar puede haber nuevas escrituras (handshake) o texto plano → reitera.
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None), // aparcar en el fd
            Err(e) => return Err(e.to_string()),
        }
    }
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn tls_read_nb(_h: i64) -> Result<Option<Vec<u8>>, String> { Err(NET_TLS_UNAVAILABLE.to_string()) }

/// Escritura TLS **no bloqueante** (VM, M19.4b): cifra `bytes` y los drena al socket. Las escrituras
/// rara vez bloquean (tramas pequeñas); se completan en el sitio (girando en el raro `WouldBlock`).
#[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
pub fn tls_write_nb(h: i64, bytes: &[u8]) -> Result<usize, String> {
    use std::io::Write;
    let mut reg = registry().lock().unwrap();
    let tc = match reg.open.get_mut(&h) {
        Some(OpenHandle::Tls(tc)) => tc,
        _ => return Err(format!("handle {h} is not a TLS connection")),
    };
    // Antes de cifrar datos de aplicación, asegúrate de que el handshake terminó (drena sus registros).
    tls_flush_writes(tc)?;
    tc.conn.writer().write_all(bytes).map_err(|e| e.to_string())?;
    tls_flush_writes(tc)?;
    Ok(bytes.len())
}
#[cfg(any(not(feature = "net-tls"), target_arch = "wasm32"))]
pub fn tls_write_nb(_h: i64, _bytes: &[u8]) -> Result<usize, String> { Err(NET_TLS_UNAVAILABLE.to_string()) }

// --- I/O binaria (M16.1c) ---

/// Lee un archivo entero como octetos crudos. Builtin `read_file_bytes`.
pub fn read_file_bytes(path: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Escribe octetos crudos a un archivo (lo crea/sobrescribe). Builtin `write_file_bytes`.
pub fn write_file_bytes(path: &str, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)
}

/// Lectura binaria **no bloqueante** del socket (VM, M16.1c): `Ok(Some(datos))` (o `Some(vacío)` en
/// EOF), `Ok(None)` si aún no hay datos (`WouldBlock` → la VM aparca), `Err` en error real.
pub fn socket_read_bytes_nb(h: i64) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    // M56.4: la espera de esta lectura venció (marcada por el scheduler) → error de timeout.
    if take_read_expired(h) {
        return Err(READ_TIMEOUT_MSG.to_string());
    }
    // M100 v2: un pipe de proceso se lee AQUÍ mismo, bajo el lock (la lectura es no-bloqueante por
    // construcción → nunca retiene el registro; clonar, como con Tcp, no hace falta).
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let mut reg = registry().lock().unwrap();
        if let Some(OpenHandle::Pipe(f)) = reg.open.get_mut(&h) {
            let mut buf = [0u8; 65536];
            return match f.read(&mut buf) {
                Ok(n) => Ok(Some(buf[..n].to_vec())),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(None),
                Err(e) => Err(e.to_string()),
            };
        }
    }
    let mut stream = socket_clone(h)?;
    let mut buf = [0u8; 65536];
    match stream.read(&mut buf) {
        Ok(n) => Ok(Some(buf[..n].to_vec())),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Lectura binaria **bloqueante** del socket (intérprete, M16.1c): una lectura; `Ok(datos)` (vacío en
/// EOF) o `Err`.
pub fn socket_read_bytes_blocking(h: i64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    // M19.4: un handle TLS se lee por la bomba TLS. Sobre el socket bloqueante del intérprete, `read_tls`
    // bloquea (nunca da WouldBlock), así que `tls_read_nb` actúa como lectura bloqueante — salvo con
    // SO_RCVTIMEO puesto (M56.4): entonces un `Ok(None)` solo puede significar que la espera venció.
    if is_tls_handle(h) {
        return match tls_read_nb(h)? {
            Some(data) => Ok(data),
            None => Err(READ_TIMEOUT_MSG.to_string()),
        };
    }
    // M100 v2: un pipe de proceso (fd no-bloqueante por construcción) se lee aquí BLOQUEANDO:
    // reintento en WouldBlock con cesión de 1 ms, SOLTANDO el lock entre intentos. Es el camino
    // del oráculo secuencial; la VM aparca la fibra por la variante nb.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    loop {
        {
            let mut reg = registry().lock().unwrap();
            match reg.open.get_mut(&h) {
                Some(OpenHandle::Pipe(f)) => {
                    let mut buf = [0u8; 65536];
                    match f.read(&mut buf) {
                        Ok(n) => return Ok(buf[..n].to_vec()),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
                _ => break,
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let mut stream = socket_clone(h)?;
    let mut buf = [0u8; 65536];
    match stream.read(&mut buf) {
        Ok(n) => Ok(buf[..n].to_vec()),
        // M56.4: con SO_RCVTIMEO puesto (set_read_timeout, motor bloqueante), la espera vencida
        // llega como WouldBlock/TimedOut (según SO) → el mismo mensaje que en la VM.
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
            Err(READ_TIMEOUT_MSG.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

// --- Sockets no bloqueantes para el scheduler de la VM (M15.5) ---
//
// El intérprete usa los sockets BLOQUEANTES de arriba (un solo hilo). La VM los voltea a NO bloqueantes
// con `set_nonblocking` y usa estos helpers, que devuelven `Ok(None)` para señalar `WouldBlock` (la VM
// aparca la fibra y reintenta). Así `tcp_accept`/`socket_read` ceden al scheduler en vez de bloquear.

// --- Timeout de lectura de sockets (M56.4) ---
//
// `__socket_set_read_timeout(h, ms)` fija cuánto puede esperar UNA lectura del socket `h` (ms <= 0
// lo quita). Dos mecanismos según el motor:
//   - **VM** (sockets no bloqueantes): el timeout vive en `read_timeouts()`; al aparcar una fibra
//     por lectura, el scheduler calcula su *deadline* (`read_deadline`) y el poller espera como
//     mucho hasta el más próximo. Al vencer, marca el handle (`mark_read_timeout`) y despierta la
//     fibra: la lectura re-ejecutada consume la marca (`take_read_expired`) y devuelve el error.
//   - **Intérprete** (sockets bloqueantes): se aplica el `SO_RCVTIMEO` real del SO; la lectura
//     bloqueante mapea `WouldBlock`/`TimedOut` al mismo mensaje.
// En plataformas sin poller (busy-poll de respaldo) el timeout de la VM no vence (cada re-aparcado
// renueva el deadline): degradación documentada, macOS/Linux tienen poller real.

/// El mensaje del timeout de lectura (idéntico en ambos motores).
pub const READ_TIMEOUT_MSG: &str = "read timeout";

/// handle → timeout de lectura en ms (los sockets sin entrada no tienen timeout).
fn read_timeouts() -> &'static std::sync::Mutex<std::collections::HashMap<i64, u64>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<i64, u64>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// handles cuya espera de lectura venció (marcados por el scheduler, consumidos por la lectura).
fn read_expired() -> &'static std::sync::Mutex<std::collections::HashSet<i64>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<i64>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// Fija (ms > 0) o quita (ms <= 0) el timeout de lectura del socket `h`. Total: un handle que no
/// es un socket se ignora. Builtin `__socket_set_read_timeout`.
pub fn socket_set_read_timeout(h: i64, ms: i64) {
    if ms > 0 {
        read_timeouts().lock().unwrap().insert(h, ms as u64);
    } else {
        read_timeouts().lock().unwrap().remove(&h);
        read_expired().lock().unwrap().remove(&h);
    }
    // El SO_RCVTIMEO real, para los sockets BLOQUEANTES del intérprete (en los no bloqueantes de
    // la VM no aplica; ponerlo es inofensivo).
    let dur = if ms > 0 { Some(std::time::Duration::from_millis(ms as u64)) } else { None };
    let reg = registry().lock().unwrap();
    match reg.open.get(&h) {
        Some(OpenHandle::Tcp(s)) => {
            let _ = s.set_read_timeout(dur);
        }
        #[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
        Some(OpenHandle::Tls(tc)) => {
            let _ = tc.sock.set_read_timeout(dur);
        }
        // M121: el timeout de lectura aplica también a UDP (`udp_recv_from` vence con
        // "read timeout"); en la VM el SO_RCVTIMEO es inerte (socket no bloqueante) y manda
        // `read_timeouts()`, como en TCP.
        Some(OpenHandle::Udp(s)) => {
            let _ = s.set_read_timeout(dur);
        }
        _ => {}
    }
}

/// El deadline absoluto de la próxima lectura del handle, si tiene timeout (lo consulta la VM al
/// aparcar una fibra por lectura).
pub fn read_deadline(h: i64) -> Option<std::time::Instant> {
    read_timeouts()
        .lock()
        .unwrap()
        .get(&h)
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(*ms))
}

/// Marca que la espera de lectura del handle venció (la pone el scheduler al expirar el deadline).
pub fn mark_read_timeout(h: i64) {
    read_expired().lock().unwrap().insert(h);
}

/// M107.2: consume la marca de timeout del PSEUDO-HANDLE de stdin (la mira el opcode
/// `StdinReadTimeout` re-ejecutado al despertar). Envoltorio público de `take_read_expired`.
pub fn take_read_timeout(h: i64) -> bool {
    take_read_expired(h)
}

/// Consume la marca de timeout del handle (la mira la lectura re-ejecutada al despertar).
fn take_read_expired(h: i64) -> bool {
    read_expired().lock().unwrap().remove(&h)
}

/// Pone el socket (conexión o escucha) del handle `h` en modo **no bloqueante** (M15.5). Lo llama la VM
/// tras crear el socket; el intérprete nunca, así que sus sockets siguen bloqueantes.
pub fn set_nonblocking(h: i64) -> Result<(), String> {
    let reg = registry().lock().unwrap();
    match reg.open.get(&h) {
        Some(OpenHandle::Tcp(s)) => s.set_nonblocking(true).map_err(|e| e.to_string()),
        Some(OpenHandle::Listener(l)) => l.set_nonblocking(true).map_err(|e| e.to_string()),
        Some(OpenHandle::Udp(s)) => s.set_nonblocking(true).map_err(|e| e.to_string()),
        _ => Err(format!("handle {} is not a socket", h)),
    }
}

/// M17: el descriptor de archivo crudo (`RawFd`, un `i32` en Unix) del socket detrás del handle, para
/// registrarlo en el poller (`kqueue`/`epoll`). `None` si el handle no es un socket o la plataforma no
/// es Unix (allí el scheduler cae al busy-poll de M15.5, que no necesita fds).
#[cfg(unix)]
pub fn raw_fd(h: i64) -> Option<i32> {
    use std::os::unix::io::AsRawFd;
    let reg = registry().lock().unwrap();
    match reg.open.get(&h) {
        Some(OpenHandle::Tcp(s)) => Some(s.as_raw_fd()),
        Some(OpenHandle::Listener(l)) => Some(l.as_raw_fd()),
        // M19.4b: el fd del socket subyacente de una conexión TLS, para aparcar la fibra en el poller.
        #[cfg(all(feature = "net-tls", not(target_arch = "wasm32")))]
        Some(OpenHandle::Tls(tc)) => Some(tc.sock.as_raw_fd()),
        Some(OpenHandle::Udp(s)) => Some(s.as_raw_fd()), // M20.11: cesión de udp_recv_from
        // M100 v2: el fd del pipe de un hijo, para que la bomba aparque la fibra en el poller.
        Some(OpenHandle::Pipe(f)) => Some(f.as_raw_fd()),
        // M100 v3: el fd del stdin de un hijo vivo, para aparcar por interés de ESCRITURA.
        Some(OpenHandle::PipeW(f)) => Some(f.as_raw_fd()),
        _ => None,
    }
}
#[cfg(not(unix))]
pub fn raw_fd(_h: i64) -> Option<i32> {
    None
}

/// Lectura **no bloqueante**: `Ok(Some(datos))` (o `Some("")` en EOF), `Ok(None)` si aún no hay datos
/// (`WouldBlock` → la VM aparca), `Err` en error real (M15.5).
pub fn socket_read_nb(h: i64) -> Result<Option<String>, String> {
    use std::io::Read;
    // M56.4: la espera de esta lectura venció (marcada por el scheduler) → error de timeout.
    if take_read_expired(h) {
        return Err(READ_TIMEOUT_MSG.to_string());
    }
    let mut stream = socket_clone(h)?;
    let mut buf = [0u8; 65536];
    match stream.read(&mut buf) {
        Ok(n) => Ok(Some(String::from_utf8_lossy(&buf[..n]).into_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Accept **no bloqueante**: `Ok(Some(handle))` con la conexión (ya puesta en no bloqueante),
/// `Ok(None)` si no hay ninguna pendiente (`WouldBlock`), `Err` en error real (M15.5).
pub fn tcp_accept_nb(h: i64) -> Result<Option<i64>, String> {
    let listener = {
        let reg = registry().lock().unwrap();
        match reg.open.get(&h) {
            Some(OpenHandle::Listener(l)) => l.try_clone().map_err(|e| e.to_string())?,
            Some(_) => return Err(format!("handle {} is not a listening socket", h)),
            None => return Err(format!("invalid handle: {}", h)),
        }
    };
    match listener.accept() {
        Ok((stream, _)) => {
            let _ = stream.set_nodelay(true); // Nagle+delayed-ACK (M96b)
            stream.set_nonblocking(true).map_err(|e| e.to_string())?;
            let mut reg = registry().lock().unwrap();
            let id = reg.next;
            reg.next += 1;
            reg.open.insert(id, OpenHandle::Tcp(stream));
            Ok(Some(id))
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

// --- Servidor TCP (M15.3) ---

/// Hace *bind* + *listen* en `host:port` (con `port=0` el SO asigna un puerto efímero) y devuelve un
/// handle de escucha (M15.3).
pub fn tcp_listen(host: &str, port: i64) -> Result<i64, String> {
    let listener = adopt_or_bind(host, port)?;
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, OpenHandle::Listener(listener));
    Ok(id)
}

/// M92.3 (socket-activation para `ray dev`): si el proceso heredó un socket de escucha del supervisor
/// (`RAY_LISTEN_FD` + `RAY_LISTEN_ADDR` == `host:port`), lo **ADOPTA** (`from_raw_fd`) en vez de hacer
/// `bind` → el socket lo retiene el supervisor entre reinicios (cero conexiones rechazadas, cero re-bind).
/// La adopción es una sola vez por proceso (un `AtomicBool`; un segundo `tcp_listen` del mismo addr hace
/// bind normal → `EADDRINUSE`, correcto). Sin herencia, o en no-unix, es el `bind` de siempre.
fn adopt_or_bind(host: &str, port: i64) -> Result<std::net::TcpListener, String> {
    #[cfg(unix)]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static ADOPTED: AtomicBool = AtomicBool::new(false);
        if let (Ok(fd_s), Ok(addr)) = (std::env::var("RAY_LISTEN_FD"), std::env::var("RAY_LISTEN_ADDR"))
            && addr == format!("{host}:{port}")
            && let Ok(fd) = fd_s.parse::<i32>()
            && ADOPTED
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            use std::os::unix::io::FromRawFd;
            // SAFETY: el fd lo dup2-eó el supervisor a este número (RAY_LISTEN_FD) antes del exec, es un
            // socket de escucha válido, y `ADOPTED` garantiza que solo un `TcpListener` toma su propiedad.
            return Ok(unsafe { std::net::TcpListener::from_raw_fd(fd) });
        }
    }
    std::net::TcpListener::bind((host, port as u16)).map_err(|e| e.to_string())
}

/// Bloquea hasta una conexión entrante en el handle de escucha `h` y devuelve un handle de conexión
/// (un socket normal). Clona el listener para no retener el lock durante el `accept()` bloqueante (M15.3).
pub fn tcp_accept(h: i64) -> Result<i64, String> {
    let listener = {
        let reg = registry().lock().unwrap();
        match reg.open.get(&h) {
            Some(OpenHandle::Listener(l)) => l.try_clone().map_err(|e| e.to_string())?,
            Some(_) => return Err(format!("handle {} is not a listening socket", h)),
            None => return Err(format!("invalid handle: {}", h)),
        }
    };
    let (stream, _addr) = listener.accept().map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true); // Nagle+delayed-ACK (M96b)
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, OpenHandle::Tcp(stream));
    Ok(id)
}

/// El puerto local de un socket de escucha o de conexión; `0` si el handle no es un socket o falla.
/// Útil para descubrir el puerto efímero tras `tcp_listen(host, 0)` (M15.3). Total.
pub fn local_port(h: i64) -> i64 {
    let reg = registry().lock().unwrap();
    match reg.open.get(&h) {
        Some(OpenHandle::Listener(l)) => l.local_addr().map(|a| a.port() as i64).unwrap_or(0),
        Some(OpenHandle::Tcp(s)) => s.local_addr().map(|a| a.port() as i64).unwrap_or(0),
        Some(OpenHandle::Udp(s)) => s.local_addr().map(|a| a.port() as i64).unwrap_or(0),
        _ => 0,
    }
}

// --- UDP (M20.8) ---
//
// Sockets sin conexión sobre `std::net::UdpSocket`, cero deps. El handle vive en el mismo registro.
// A diferencia de TCP, cada datagrama lleva su remitente → `udp_recv_from` devuelve (host, puerto,
// datos). La cesión cooperativa llegó en M20.11 (VM: no-bloqueante + aparcado de fibra) y F4
// (nativo con fibras); el intérprete (oráculo mono-hilo) usa el camino bloqueante. M121: el
// timeout de lectura por handle (`net.set_read_timeout`, M56.4) aplica también a UDP en los tres
// motores — una espera vencida devuelve `Err("read timeout")`, como en TCP.

/// Enlaza un socket UDP a `host:port` (port=0 → efímero, consultable con `local_port`) (M20.8).
pub fn udp_bind(host: &str, port: i64) -> Result<i64, String> {
    let sock = std::net::UdpSocket::bind((host, port as u16)).map_err(|e| e.to_string())?;
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, OpenHandle::Udp(sock));
    Ok(id)
}

/// Envía `data` al destino `host:port` desde el socket UDP `h`; `Ok(nº de octetos enviados)` (M20.8).
pub fn udp_send_to(h: i64, host: &str, port: i64, data: &[u8]) -> Result<usize, String> {
    let reg = registry().lock().unwrap();
    match reg.open.get(&h) {
        Some(OpenHandle::Udp(s)) => s.send_to(data, (host, port as u16)).map_err(|e| e.to_string()),
        Some(_) => Err(format!("handle {} is not a UDP socket", h)),
        None => Err(format!("invalid handle: {}", h)),
    }
}

/// Recibe un datagrama del socket UDP `h` (bloqueante); `Ok((host, puerto, datos))` del remitente
/// (M20.8). Clona el socket para no retener el lock del registro durante la espera (como en TCP).
pub fn udp_recv_from(h: i64) -> Result<(String, i64, Vec<u8>), String> {
    let sock = {
        let reg = registry().lock().unwrap();
        match reg.open.get(&h) {
            Some(OpenHandle::Udp(s)) => s.try_clone().map_err(|e| e.to_string())?,
            Some(_) => return Err(format!("handle {} is not a UDP socket", h)),
            None => return Err(format!("invalid handle: {}", h)),
        }
    };
    let mut buf = vec![0u8; 65536]; // un datagrama UDP cabe de sobra en 64 KiB
    let (n, addr) = match sock.recv_from(&mut buf) {
        Ok(r) => r,
        // M121: con SO_RCVTIMEO puesto (set_read_timeout, motor bloqueante), la espera vencida
        // llega como WouldBlock/TimedOut (según SO) → el mismo mensaje estable que en TCP/VM.
        Err(ref e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            return Err(READ_TIMEOUT_MSG.to_string());
        }
        Err(e) => return Err(e.to_string()),
    };
    buf.truncate(n);
    Ok((addr.ip().to_string(), addr.port() as i64, buf))
}

/// Variante NO bloqueante de `udp_recv_from` para la VM (M20.11): `Ok(None)` si no hay datagrama listo
/// (`WouldBlock`) → la VM aparca la fibra en el fd y reintenta, como `socket_read_bytes_nb`.
pub fn udp_recv_from_nb(h: i64) -> Result<Option<(String, i64, Vec<u8>)>, String> {
    // M121/M56.4: la espera de esta lectura venció (marcada por el scheduler al expirar el
    // deadline del aparcado) → error de timeout, como en las lecturas TCP/TLS.
    if take_read_expired(h) {
        return Err(READ_TIMEOUT_MSG.to_string());
    }
    let sock = {
        let reg = registry().lock().unwrap();
        match reg.open.get(&h) {
            Some(OpenHandle::Udp(s)) => s.try_clone().map_err(|e| e.to_string())?,
            Some(_) => return Err(format!("handle {} is not a UDP socket", h)),
            None => return Err(format!("invalid handle: {}", h)),
        }
    };
    let mut buf = vec![0u8; 65536];
    match sock.recv_from(&mut buf) {
        Ok((n, addr)) => {
            buf.truncate(n);
            Ok(Some((addr.ip().to_string(), addr.port() as i64, buf)))
        }
        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Lista los nombres de las entradas de un directorio (M11.7c). Helper compartido por ambos motores
/// (`__list_dir`). Ordenados para que el resultado sea **determinista** (el sistema no garantiza orden).
pub fn list_dir(path: &str) -> std::io::Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

/// Repite `s` `n` veces (`n <= 0` → `""`) (M11.7a). Helper compartido.
pub fn repeat_str(s: &str, n: i64) -> String {
    if n <= 0 {
        String::new()
    } else {
        s.repeat(n as usize)
    }
}

// --- Helpers de las reglas ---

/// Error de aridad "espera N argumento(s), se le pasaron M".
fn arity(a: &[Type], n: usize, name: &str, detail: &str) -> Result<(), BuiltinError> {
    if a.len() != n {
        let plural = if n == 1 { "argument" } else { "arguments" };
        return Err((None, format!("{} expects {} {}{}, received {}", name, n, plural, detail, a.len())));
    }
    Ok(())
}

/// Error de aridad para builtins sin argumentos.
fn nullary(a: &[Type], name: &str) -> Result<(), BuiltinError> {
    if !a.is_empty() {
        return Err((None, format!("{} expects no arguments, received {}", name, a.len())));
    }
    Ok(())
}

/// ¿Es un tipo que `print`/`eprint` saben imprimir? (Coincide con `is_printable` del checker.)
fn printable(t: &Type) -> bool {
    matches!(
        t,
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Array(_)
            | Type::Struct(_, _) | Type::Fn(_, _) | Type::Enum(_, _) | Type::Var(_)
            | Type::Bytes // diferido de M16: se imprime en hexadecimal (ver `bytes_to_hex`)
            | Type::UInt(_) // jul 2026: decimal sin signo (ambos motores ya lo formateaban; la SPEC §10 lo prometía)
    )
}

/// Representación textual de `bytes`: los octetos en hexadecimal continuo en minúsculas (p. ej.
/// `b"Hi\xff"` → `"4869ff"`). Es la forma honesta para datos binarios (no son texto) y casa con las
/// convenciones de digests. La comparten `print`/`to_string` en ambos motores (oráculo). M16 (diferido).
pub fn bytes_to_hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        s.push_str(&format!("{:02x}", byte));
    }
    s
}

/// Regla de tipado de una función matemática unaria `float -> float` (M15.1a).
fn mathf_check(a: &[Type], name: &str) -> Result<Type, BuiltinError> {
    arity(a, 1, name, "")?;
    if a[0] != Type::Float {
        return Err((Some(0), format!("{} expects a float, not {}", name, a[0])));
    }
    Ok(Type::Float)
}

// M49.1b: `numeric_unary_check`/`numeric_binary_check` (regla ad-hoc de abs/min/max) se eliminaron al
// mover esas funciones a `std/math` (puras en raylang, tipadas por sus bounds `Signed`/`Ord`).

/// La tabla. El orden no importa (la búsqueda es por nombre).
static BUILTINS: &[Builtin] = &[
    // print(x) -> unit: imprime un imprimible a stdout.
    Builtin { name: "print", opcode: OpCode::Print, check: |a| {
        arity(a, 1, "print", "")?;
        if !printable(&a[0]) { return Err((Some(0), format!("print cannot print a {}", a[0]))); }
        Ok(Type::Unit)
    } },
    // M48.4: `__len` — primitivo interno de `len`, al que baja el trait `Len` (`impl Len for [T]` etc.
    // → `__len(self)`). Mismo opcode que `len`; oculto (`__`). Sobrevive al retiro de `len` (M48.4e).
    Builtin { name: "__len", opcode: OpCode::Len, check: |a| {
        arity(a, 1, "__len", "")?;
        if !matches!(a[0], Type::Array(_) | Type::String | Type::Map(_, _) | Type::Bytes) {
            return Err((Some(0), format!("__len expects an array, a string, a Map or bytes, not {}", a[0])));
        }
        Ok(Type::Int)
    } },
    // M48.4b: `__push` — primitivo interno de `push`, al que baja `impl<T> Push<T> for [T]`.
    Builtin { name: "__push", opcode: OpCode::Push, check: |a| {
        arity(a, 2, "__push", " (array, value)")?;
        let elem = match &a[0] {
            Type::Array(e) => (**e).clone(),
            other => return Err((Some(0), format!("__push expects an array as first argument, not {}", other))),
        };
        if a[1] != elem {
            return Err((Some(1), format!("__push: the array is of {} but got {}", elem, a[1])));
        }
        Ok(Type::Unit)
    } },
    // to_string(x) -> string (M11.1a): representación textual de un primitivo imprimible.
    Builtin { name: "to_string", opcode: OpCode::ToString, check: |a| {
        arity(a, 1, "to_string", "")?;
        if !matches!(a[0], Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Bytes | Type::UInt(_)) {
            return Err((Some(0), format!("to_string only converts int/float/bool/string/char/bytes/u*, not {}", a[0])));
        }
        Ok(Type::String)
    } },
    // M48.4b: `__contains` — primitivo interno de `contains`, al que bajan los impls de `Contains<T>`
    // (subcadena en string, pertenencia en arreglo). Bytes queda fuera (el builtin tampoco lo cubre).
    Builtin { name: "__contains", opcode: OpCode::Contains, check: |a| {
        arity(a, 2, "__contains", " (string/array, value)")?;
        match &a[0] {
            Type::String => {
                if a[1] != Type::String { return Err((Some(1), format!("__contains expects a string as substring, not {}", a[1]))); }
            }
            Type::Array(elem) => {
                if a[1] != **elem { return Err((Some(1), format!("__contains: the array is of {} but looking for {}", elem, a[1]))); }
            }
            _ => return Err((Some(0), format!("__contains expects a string or an array, not {}", a[0]))),
        }
        Ok(Type::Bool)
    } },
    // char_code(c) -> int (M40.3a): el code point Unicode del carácter. Habilita hashear strings/chars
    // en raylang (para `Hash`) y ordenar por code point.
    Builtin { name: "char_code", opcode: OpCode::CharCode, check: |a| {
        arity(a, 1, "char_code", "")?;
        if a[0] != Type::Char { return Err((Some(0), format!("char_code expects a char, not {}", a[0]))); }
        Ok(Type::Int)
    } },
    // M43: hashes de producción vía `ring` (bytes -> bytes). Ver el bloque de helpers arriba.
    // M68.2: aleatoriedad criptográfica (CSPRNG del SO vía ring) — para tokens/salts/nonces.
    Builtin { name: "__crypto_random_bytes", opcode: OpCode::CryptoRandomBytes, check: |a| {
        arity(a, 1, "__crypto_random_bytes", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__crypto_random_bytes expects an int (number of bytes), not {}", a[0]))); }
        Ok(Type::Bytes)
    } },
    // M126: hasher INCREMENTAL (new → update* → final; final consume el handle). Devuelven
    // arreglos etiquetados que std/crypto envuelve en Result.
    Builtin { name: "__hasher_new", opcode: OpCode::HasherNew, check: |a| {
        arity(a, 1, "__hasher_new", " (algorithm)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__hasher_new expects a string (the algorithm), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__hasher_update", opcode: OpCode::HasherUpdate, check: |a| {
        arity(a, 2, "__hasher_update", " (handle, chunk)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__hasher_update expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("__hasher_update expects bytes (the chunk), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__hasher_final", opcode: OpCode::HasherFinal, check: |a| {
        arity(a, 1, "__hasher_final", " (handle)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__hasher_final expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    Builtin { name: "__sha256", opcode: OpCode::Sha256, check: |a| {
        arity(a, 1, "sha256", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("sha256 expects bytes, not {}", a[0]))); }
        Ok(Type::Bytes)
    } },
    Builtin { name: "__sha512", opcode: OpCode::Sha512, check: |a| {
        arity(a, 1, "sha512", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("sha512 expects bytes, not {}", a[0]))); }
        Ok(Type::Bytes)
    } },
    Builtin { name: "__sha1", opcode: OpCode::Sha1, check: |a| {
        arity(a, 1, "sha1", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("sha1 expects bytes, not {}", a[0]))); }
        Ok(Type::Bytes)
    } },
    // M43.2: HMAC-SHA256 (clave, mensaje) -> etiqueta de 32 octetos.
    Builtin { name: "__hmac_sha256", opcode: OpCode::HmacSha256, check: |a| {
        arity(a, 2, "hmac_sha256", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("hmac_sha256 expects bytes (key), not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("hmac_sha256 expects bytes (message), not {}", a[1]))); }
        Ok(Type::Bytes)
    } },
    // M43.3: Ed25519. Los fallibles (semilla de 32 octetos) son primitivos `[bytes]` etiquetados
    // (vacío/único); el prelude los envuelve en Option<bytes>. `verify` es total → bool directo.
    Builtin { name: "__ed25519_public_key", opcode: OpCode::Ed25519PublicKey, check: |a| {
        arity(a, 1, "__ed25519_public_key", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("ed25519_public_key expects bytes (seed), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    Builtin { name: "__ed25519_sign", opcode: OpCode::Ed25519Sign, check: |a| {
        arity(a, 2, "__ed25519_sign", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("ed25519_sign expects bytes (seed), not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("ed25519_sign expects bytes (message), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    Builtin { name: "__ed25519_verify", opcode: OpCode::Ed25519Verify, check: |a| {
        arity(a, 3, "ed25519_verify", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("ed25519_verify expects bytes (public key), not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("ed25519_verify expects bytes (message), not {}", a[1]))); }
        if a[2] != Type::Bytes { return Err((Some(2), format!("ed25519_verify expects bytes (signature), not {}", a[2]))); }
        Ok(Type::Bool)
    } },
    // M43.4: ChaCha20-Poly1305 AEAD. seal/open (clave, nonce, aad, dato) -> [bytes] etiquetado; el
    // prelude → Option<bytes> (None si tamaños malos o —en open— falla la autenticación).
    Builtin { name: "__chacha20poly1305_seal", opcode: OpCode::ChaChaPolySeal, check: |a| {
        arity(a, 4, "__chacha20poly1305_seal", "")?;
        for (i, etiqueta) in ["clave", "nonce", "aad", "text plano"].iter().enumerate() {
            if a[i] != Type::Bytes { return Err((Some(i), format!("chacha20poly1305_seal expects bytes ({etiqueta}), not {}", a[i]))); }
        }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    Builtin { name: "__chacha20poly1305_open", opcode: OpCode::ChaChaPolyOpen, check: |a| {
        arity(a, 4, "__chacha20poly1305_open", "")?;
        for (i, etiqueta) in ["clave", "nonce", "aad", "text cifrado"].iter().enumerate() {
            if a[i] != Type::Bytes { return Err((Some(i), format!("chacha20poly1305_open expects bytes ({etiqueta}), not {}", a[i]))); }
        }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // M114: acuerdo de claves X25519 + HKDF. Los fallibles son `[bytes]` etiquetado (el prelude →
    // Option<bytes>); `constant_time_eq` es total → bool directo.
    Builtin { name: "__x25519_public_key", opcode: OpCode::X25519PublicKey, check: |a| {
        arity(a, 1, "__x25519_public_key", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("x25519_public_key expects bytes (secret key), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    Builtin { name: "__x25519_shared_secret", opcode: OpCode::X25519SharedSecret, check: |a| {
        arity(a, 2, "__x25519_shared_secret", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("x25519_shared_secret expects bytes (secret key), not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("x25519_shared_secret expects bytes (peer public key), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    Builtin { name: "__hkdf_sha256", opcode: OpCode::HkdfSha256, check: |a| {
        arity(a, 4, "__hkdf_sha256", "")?;
        for (i, etiqueta) in ["salt", "input key material", "info"].iter().enumerate() {
            if a[i] != Type::Bytes { return Err((Some(i), format!("hkdf_sha256 expects bytes ({etiqueta}), not {}", a[i]))); }
        }
        if a[3] != Type::Int { return Err((Some(3), format!("hkdf_sha256 expects an int (output length), not {}", a[3]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    Builtin { name: "__constant_time_eq", opcode: OpCode::ConstantTimeEq, check: |a| {
        arity(a, 2, "constant_time_eq", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("constant_time_eq expects bytes, not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("constant_time_eq expects bytes, not {}", a[1]))); }
        Ok(Type::Bool)
    } },
    // __char_from_code(n) -> [char] (diferido JSON-1): [] si n no es un code point válido.
    // El prelude → char_from_code -> Option<char>. El inverso de char_code.
    Builtin { name: "__char_from_code", opcode: OpCode::CharFromCode, check: |a| {
        arity(a, 1, "__char_from_code", " (the code point)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__char_from_code expects an int, not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Char)))
    } },
    // --- Bits de float (M54.1): primitivos totales; `std/math` → float_bits/float_from_bits. ---
    Builtin { name: "__float_bits", opcode: OpCode::FloatBits, check: |a| {
        arity(a, 1, "__float_bits", " (the float)")?;
        if a[0] != Type::Float { return Err((Some(0), format!("__float_bits expects a float, not {}", a[0]))); }
        Ok(Type::Int)
    } },
    Builtin { name: "__float_from_bits", opcode: OpCode::FloatFromBits, check: |a| {
        arity(a, 1, "__float_from_bits", " (the 64 bits as int)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__float_from_bits expects an int, not {}", a[0]))); }
        Ok(Type::Float)
    } },
    // --- SQLite embebido (M53.3): primitivos con arreglo etiquetado; `db/sqlite` → Result. ---
    // __sqlite_open(path) -> [string]: ["ok", handle] o ["err", msg].
    Builtin { name: "__sqlite_open", opcode: OpCode::SqliteOpen, check: |a| {
        arity(a, 1, "__sqlite_open", " (the database path)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__sqlite_open expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __sqlite_exec(h, sql, params) -> [string]: ["ok", n_afectadas] o ["err", msg].
    Builtin { name: "__sqlite_exec", opcode: OpCode::SqliteExec, check: |a| {
        arity(a, 3, "__sqlite_exec", " (handle, sql, params)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__sqlite_exec expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__sqlite_exec expects a string (the SQL), not {}", a[1]))); }
        if a[2] != Type::Array(Box::new(Type::String)) { return Err((Some(2), format!("__sqlite_exec expects a [string] (the parameters), not {}", a[2]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __sqlite_query(h, sql, params) -> [string]: ["ok", ncols, celdas…] o ["err", msg].
    Builtin { name: "__sqlite_query", opcode: OpCode::SqliteQuery, check: |a| {
        arity(a, 3, "__sqlite_query", " (handle, sql, params)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__sqlite_query expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__sqlite_query expects a string (the SQL), not {}", a[1]))); }
        if a[2] != Type::Array(Box::new(Type::String)) { return Err((Some(2), format!("__sqlite_query expects a [string] (the parameters), not {}", a[2]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __from_utf8(b) -> [string] (M16.1b): ["ok", s] o ["err", msg]. El prelude → Result<string,string>.
    Builtin { name: "__from_utf8", opcode: OpCode::FromUtf8, check: |a| {
        arity(a, 1, "__from_utf8", "")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("__from_utf8 expects bytes, not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // --- I/O binaria (M16.1c). Lecturas → [bytes] etiquetado; escrituras → [string]. ---
    // __read_file_bytes(ruta) -> [bytes]: [b"ok", datos] o [b"err", msg]. El prelude → Result<bytes,string>.
    Builtin { name: "__read_file_bytes", opcode: OpCode::ReadFileBytes, check: |a| {
        arity(a, 1, "__read_file_bytes", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__read_file_bytes expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // __write_file_bytes(ruta, datos) -> [string]: ["ok"] o ["err", msg]. El prelude → Result<int,string>.
    Builtin { name: "__write_file_bytes", opcode: OpCode::WriteFileBytes, check: |a| {
        arity(a, 2, "__write_file_bytes", " (path, data)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__write_file_bytes expects a string (the path), not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("__write_file_bytes expects bytes (the data), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __socket_read_bytes(h) -> [bytes]: [b"ok", datos] o [b"err", msg]. El prelude → Result<bytes,string>.
    Builtin { name: "__socket_read_bytes", opcode: OpCode::SocketReadBytes, check: |a| {
        arity(a, 1, "__socket_read_bytes", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__socket_read_bytes expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // __socket_write_bytes(h, datos) -> [string]: ["ok", ""] o ["err", msg]. El prelude → Result<int,string>.
    Builtin { name: "__socket_write_bytes", opcode: OpCode::SocketWriteBytes, check: |a| {
        arity(a, 2, "__socket_write_bytes", " (handle, data)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__socket_write_bytes expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("__socket_write_bytes expects bytes (the data), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // M48.4e-1: primitivos internos de `StrOps`/`BytesOps` (mismos opcodes que los builtins públicos,
    // ocultos). Los cuerpos de sus impls (M48.4d) los llaman; sobreviven al retiro de los públicos (e-3).
    Builtin { name: "__trim", opcode: OpCode::Trim, check: |a| {
        arity(a, 1, "__trim", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__trim expects a string, not {}", a[0]))); }
        Ok(Type::String)
    } },
    Builtin { name: "__split", opcode: OpCode::Split, check: |a| {
        arity(a, 2, "__split", " (string, separador)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__split expects a string, not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__split expects a string as separator, not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__replace", opcode: OpCode::Replace, check: |a| {
        arity(a, 3, "__replace", " (string, from, to)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__replace expects a string, not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__replace expects a string in 'from', not {}", a[1]))); }
        if a[2] != Type::String { return Err((Some(2), format!("__replace expects a string in 'to', not {}", a[2]))); }
        Ok(Type::String)
    } },
    Builtin { name: "__chars", opcode: OpCode::Chars, check: |a| {
        arity(a, 1, "__chars", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__chars expects a string, not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Char)))
    } },
    Builtin { name: "__starts_with", opcode: OpCode::StartsWith, check: |a| {
        arity(a, 2, "__starts_with", " (string, prefijo)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__starts_with expects a string, not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__starts_with expects a string as prefix, not {}", a[1]))); }
        Ok(Type::Bool)
    } },
    Builtin { name: "__ends_with", opcode: OpCode::EndsWith, check: |a| {
        arity(a, 2, "__ends_with", " (string, sufijo)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__ends_with expects a string, not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__ends_with expects a string as suffix, not {}", a[1]))); }
        Ok(Type::Bool)
    } },
    Builtin { name: "__to_upper", opcode: OpCode::ToUpper, check: |a| {
        arity(a, 1, "__to_upper", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__to_upper expects a string, not {}", a[0]))); }
        Ok(Type::String)
    } },
    Builtin { name: "__to_lower", opcode: OpCode::ToLower, check: |a| {
        arity(a, 1, "__to_lower", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__to_lower expects a string, not {}", a[0]))); }
        Ok(Type::String)
    } },
    Builtin { name: "__substring", opcode: OpCode::Substring, check: |a| {
        arity(a, 3, "__substring", " (string, start, fin)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__substring expects a string, not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__substring expects an int as start, not {}", a[1]))); }
        if a[2] != Type::Int { return Err((Some(2), format!("__substring expects an int as end, not {}", a[2]))); }
        Ok(Type::String)
    } },
    Builtin { name: "__repeat", opcode: OpCode::Repeat, check: |a| {
        arity(a, 2, "__repeat", " (string, veces)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__repeat expects a string, not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__repeat expects an int as the repeat count, not {}", a[1]))); }
        Ok(Type::String)
    } },
    Builtin { name: "__to_bytes", opcode: OpCode::ToBytes, check: |a| {
        arity(a, 1, "__to_bytes", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__to_bytes expects a string, not {}", a[0]))); }
        Ok(Type::Bytes)
    } },
    Builtin { name: "__sub_bytes", opcode: OpCode::SubBytes, check: |a| {
        arity(a, 3, "__sub_bytes", " (bytes, start, fin)")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("__sub_bytes expects bytes, not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__sub_bytes expects an int as start, not {}", a[1]))); }
        if a[2] != Type::Int { return Err((Some(2), format!("__sub_bytes expects an int as end, not {}", a[2]))); }
        Ok(Type::Bytes)
    } },
    // bytes_of(xs) -> bytes (M19.3c): construye bytes a partir de un [int] (cada elemento se trunca a
    // octeto con `& 255`). Es el **dual del indexado** `b[i]` (que ya lee un octeto como int, M16.1a):
    // permite *construir* datos binarios octeto a octeto (tramas de WebSocket, cabeceras).
    Builtin { name: "bytes_of", opcode: OpCode::BytesOf, check: |a| {
        arity(a, 1, "bytes_of", " (array of int)")?;
        match &a[0] {
            Type::Array(el) if **el == Type::Int => Ok(Type::Bytes),
            _ => Err((Some(0), format!("bytes_of expects an [int], not {}", a[0]))),
        }
    } },
    // __index_of(s, sub) -> [int] (M11.7a): [] o [i] (índice de carácter). El prelude → Option<int>.
    Builtin { name: "__index_of", opcode: OpCode::IndexOf, check: |a| {
        arity(a, 2, "__index_of", " (string, subcadena)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__index_of expects a string as first argument, not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__index_of expects a string as substring, not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::Int)))
    } },
    // join(arr, sep) -> string (M11.7a): une un [string] con el separador `sep`.
    // join **ad-hoc polimórfico** (como `close`): `join(arr: [string], sep) -> string` (M11.7a) o
    // `join(t: Task<T>) -> T` (M12.3, une una tarea). raylang no tiene sobrecarga, así que un único
    // builtin que ramifica por el tipo del primer argumento; el compilador elige el opcode por la aridad.
    Builtin { name: "join", opcode: OpCode::Join, check: |a| {
        if matches!(a.first(), Some(Type::Task(_))) {
            arity(a, 1, "join", " (one Task)")?;
            match &a[0] {
                Type::Task(t) => return Ok((**t).clone()),
                _ => unreachable!(),
            }
        }
        arity(a, 2, "join", " (array of string, separator)")?;
        if a[0] != Type::Array(Box::new(Type::String)) {
            return Err((Some(0), format!("join expects a [string] or a Task as first argument, not {}", a[0])));
        }
        if a[1] != Type::String { return Err((Some(1), format!("join expects a string as separator, not {}", a[1]))); }
        Ok(Type::String)
    } },
    // M48.4b: `__reverse` — primitivo interno de `reverse`, al que baja `impl<T> Reverse for [T]`.
    Builtin { name: "__reverse", opcode: OpCode::Reverse, check: |a| {
        arity(a, 1, "__reverse", "")?;
        match &a[0] {
            Type::Array(_) => Ok(a[0].clone()),
            other => Err((Some(0), format!("__reverse expects an array, not {}", other))),
        }
    } },
    // __pop(a) -> [T] (M11.7b): muta `a` quitando el último; [] si vacío, [x] si no. Prelude → Option<T>.
    Builtin { name: "__pop", opcode: OpCode::ArrayPop, check: |a| {
        arity(a, 1, "__pop", "")?;
        match &a[0] {
            Type::Array(elem) => Ok(Type::Array(elem.clone())),
            other => Err((Some(0), format!("__pop expects an array, not {}", other))),
        }
    } },
    // __position(a, x) -> [int] (M11.7b): [] o [i] (índice de la 1ª ocurrencia). Prelude → Option<int>.
    Builtin { name: "__position", opcode: OpCode::Position, check: |a| {
        arity(a, 2, "__position", " (array, value)")?;
        match &a[0] {
            Type::Array(elem) => {
                if a[1] != **elem { return Err((Some(1), format!("__position: the array is of {} but looking for {}", elem, a[1]))); }
            }
            other => return Err((Some(0), format!("__position expects an array, not {}", other))),
        }
        Ok(Type::Array(Box::new(Type::Int)))
    } },
    // --- Mapas Map<K,V> (M13.1) ---
    // map_new() -> Map<K,V>: mapa vacío. Su tipo es INDETERMINADO (como `[]`/`None`): lo fija el
    // tipo esperado en `check_expr_expected`. Por eso esta regla (sin tipo esperado) es un error;
    // el camino normal lo intercepta antes de llegar aquí.
    // `Map.new()` (M48.1): construir un Map vacío es una **función asociada** (`ASSOC_FNS`), no un
    // builtin de función libre. El opcode `MapNew` sigue; lo emite el compilador para la asociada.

    // --- Concurrencia: CSP sobre la VM (M12.1). Solo la VM las ejecuta; el intérprete da error limpio. ---
    // spawn(f: fn() -> T) -> Task<T>: lanza f (sin parámetros) como green thread y devuelve su handle
    // (M12.3; en M12.1/M12.2 devolvía unit y el handle no existía).
    Builtin { name: "spawn", opcode: OpCode::Spawn, check: |a| {
        arity(a, 1, "spawn", " (a function with no parameters)")?;
        match &a[0] {
            Type::Fn(params, ret) if params.is_empty() => Ok(Type::Task(ret.clone())),
            Type::Fn(_, _) => Err((Some(0), "spawn requires a function WITHOUT parameters (fn() -> T)".into())),
            other => Err((Some(0), format!("spawn expects a function, not {}", other))),
        }
    } },
    // __task_failed(t) -> [string] (M56.5): bloquea hasta que la tarea termine; [] si acabó bien,
    // [msg] si falló. El fallo como valor (sin re-lanzar, a diferencia de join); el prelude lo
    // envuelve en try_join(t) -> Result<T, string>.
    // M97.2: llama a la closure en la MISMA fibra y devuelve `[]` (bien) o `[msg]` (falló). El
    // valor de retorno NO viaja por aquí: el prelude pasa una closure que empuja el resultado a un
    // array capturado, así este primitivo se queda con la firma mínima (`fn()` → `[string]`) y no
    // necesita construir un enum genérico desde el runtime.
    Builtin { name: "__try_call", opcode: OpCode::TryCall, check: |a| {
        arity(a, 1, "__try_call", " (one function of no arguments)")?;
        match &a[0] {
            Type::Fn(params, _) if params.is_empty() => Ok(Type::Array(Box::new(Type::String))),
            other => Err((Some(0), format!("__try_call expects a function of no arguments, not {}", other))),
        }
    } },
    Builtin { name: "__task_failed", opcode: OpCode::TaskFailed, check: |a| {
        arity(a, 1, "__task_failed", " (one Task)")?;
        match &a[0] {
            Type::Task(_) => Ok(Type::Array(Box::new(Type::String))),
            other => Err((Some(0), format!("__task_failed expects a Task, not {}", other))),
        }
    } },
    // select(chs: [Channel<T>]) -> int: bloquea hasta que algún canal de la lista esté listo para recibir
    // y devuelve el índice del primero listo (M12.4). Luego recv(chs[i]) toma el valor.
    Builtin { name: "select", opcode: OpCode::Select, check: |a| {
        arity(a, 1, "select", " (an array of channels)")?;
        match &a[0] {
            Type::Array(el) if matches!(&**el, Type::Channel(_)) => Ok(Type::Int),
            other => Err((Some(0), format!("select expects a [Channel<T>], not {}", other))),
        }
    } },
    // M116: try_recv(ch) -> Received<T>: recepción NO bloqueante (Got(v)/Empty/Closed). El retorno
    // es el enum del prelude parametrizado con el elemento del canal.
    Builtin { name: "try_recv", opcode: OpCode::ChanTryRecv, check: |a| {
        arity(a, 1, "try_recv", " (a channel)")?;
        match &a[0] {
            Type::Channel(el) => Ok(Type::Enum("Received".to_string(), vec![(**el).clone()])),
            other => Err((Some(0), format!("try_recv expects a Channel<T>, not {}", other))),
        }
    } },
    // M116.1: __select_timeout(chs, ms) -> [int] (0 o 1 elem): el índice listo, o vacío si vence el
    // plazo. El prelude lo envuelve en Option<int> (select_timeout). Primitivo interno (con `__`).
    Builtin { name: "__select_timeout", opcode: OpCode::SelectTimeout, check: |a| {
        arity(a, 2, "__select_timeout", " (an array of channels, timeout ms)")?;
        match &a[0] {
            Type::Array(el) if matches!(&**el, Type::Channel(_)) => {}
            other => return Err((Some(0), format!("__select_timeout expects a [Channel<T>], not {}", other))),
        }
        if a[1] != Type::Int { return Err((Some(1), format!("__select_timeout expects an int (the timeout in ms), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::Int)))
    } },
    // scope(body: fn() -> R) -> R: corre body; al volver, une todas las tareas lanzadas dentro y propaga
    // un fallo si lo hubo (M12.3 structured concurrency). El compilador lo baja con ScopeBegin/ScopeEnd.
    Builtin { name: "scope", opcode: OpCode::ScopeBegin, check: |a| {
        arity(a, 1, "scope", " (a function with no parameters)")?;
        match &a[0] {
            Type::Fn(params, ret) if params.is_empty() => Ok((**ret).clone()),
            Type::Fn(_, _) => Err((Some(0), "scope requires a function WITHOUT parameters (fn() -> R)".into())),
            other => Err((Some(0), format!("scope expects a function, not {}", other))),
        }
    } },
    // `Channel.new()` / `Channel.bounded(n)` (M48.1): crear un canal es una **función asociada**
    // (`ASSOC_FNS`), no un builtin `channel`. Los opcodes `ChannelNew`/`ChannelNewBounded` siguen; los
    // emite el compilador para la asociada (no acotado / acotado a la capacidad `n: int ≥ 0`).
    // send(ch, v) -> unit: envía v por el canal ch.
    Builtin { name: "send", opcode: OpCode::ChanSend, check: |a| {
        arity(a, 2, "send", " (channel, value)")?;
        let et = match &a[0] {
            Type::Channel(t) => (**t).clone(),
            other => return Err((Some(0), format!("send expects a Channel as first argument, not {}", other))),
        };
        if a[1] != et { return Err((Some(1), format!("send: the channel is of {} but got {}", et, a[1]))); }
        Ok(Type::Unit)
    } },
    // __recv(ch) -> [T]: recibe (primitivo). [v] si hay valor, [] si cerrado+vacío; bloquea si vacío+abierto.
    // El prelude lo envuelve en recv(ch) -> Option<T>.
    Builtin { name: "__recv", opcode: OpCode::ChanRecv, check: |a| {
        arity(a, 1, "__recv", " (canal)")?;
        match &a[0] {
            Type::Channel(t) => Ok(Type::Array(Box::new((**t).clone()))),
            other => Err((Some(0), format!("__recv expects a Channel, not {}", other))),
        }
    } },
    // close: ad-hoc polimórfico (cerrar un recurso). Un Channel (M12.1) → unit; un handle de archivo
    // (int, M11.8) → int. Una sola entrada `close` (más abajo) lo cubre; NO se duplica aquí (raylang no
    // tiene sobrecarga). El canal se cierra con `close(ch)` igual que un handle con `close(h)`.

    // M48.4c: primitivos internos de los métodos de `MapOps` (mismos opcodes que los públicos, ocultos).
    Builtin { name: "__insert", opcode: OpCode::MapInsert, check: |a| {
        arity(a, 3, "__insert", " (map, key, value)")?;
        let (kt, vt) = match &a[0] {
            Type::Map(k, v) => ((**k).clone(), (**v).clone()),
            other => return Err((Some(0), format!("__insert expects a Map as first argument, not {}", other))),
        };
        if a[1] != kt { return Err((Some(1), format!("__insert: the Map key is {} but got {}", kt, a[1]))); }
        if a[2] != vt { return Err((Some(2), format!("__insert: the Map value is {} but got {}", vt, a[2]))); }
        Ok(Type::Unit)
    } },
    Builtin { name: "__contains_key", opcode: OpCode::MapContainsKey, check: |a| {
        arity(a, 2, "__contains_key", " (map, key)")?;
        let kt = match &a[0] {
            Type::Map(k, _) => (**k).clone(),
            other => return Err((Some(0), format!("__contains_key expects a Map as first argument, not {}", other))),
        };
        if a[1] != kt { return Err((Some(1), format!("__contains_key: the Map key is {} but got {}", kt, a[1]))); }
        Ok(Type::Bool)
    } },
    Builtin { name: "__keys", opcode: OpCode::MapKeys, check: |a| {
        arity(a, 1, "__keys", " (mapa)")?;
        match &a[0] {
            Type::Map(k, _) => Ok(Type::Array(k.clone())),
            other => Err((Some(0), format!("__keys expects a Map, not {}", other))),
        }
    } },
    Builtin { name: "__values", opcode: OpCode::MapValues, check: |a| {
        arity(a, 1, "__values", " (mapa)")?;
        match &a[0] {
            Type::Map(_, v) => Ok(Type::Array(v.clone())),
            other => Err((Some(0), format!("__values expects a Map, not {}", other))),
        }
    } },
    // __map_get(m, k) -> [V]: [] si la clave no está, [v] si está. El prelude → Option<V>.
    Builtin { name: "__map_get", opcode: OpCode::MapGet, check: |a| {
        arity(a, 2, "__map_get", " (map, key)")?;
        let (kt, vt) = match &a[0] {
            Type::Map(k, v) => ((**k).clone(), (**v).clone()),
            other => return Err((Some(0), format!("__map_get expects a Map as first argument, not {}", other))),
        };
        if a[1] != kt { return Err((Some(1), format!("__map_get: the Map key is {} but got {}", kt, a[1]))); }
        Ok(Type::Array(Box::new(vt)))
    } },
    // __get_or(m, k, default) -> V (P0.2, perf): valor asociado a k, o `default` si no está. SIN alocar
    // (opcode `MapGetOr`), a diferencia de `get(m, k).unwrap_or(d)`. El prelude expone `get_or`.
    Builtin { name: "__get_or", opcode: OpCode::MapGetOr, check: |a| {
        arity(a, 3, "__get_or", " (map, key, default)")?;
        let (kt, vt) = match &a[0] {
            Type::Map(k, v) => ((**k).clone(), (**v).clone()),
            other => return Err((Some(0), format!("__get_or expects a Map as first argument, not {}", other))),
        };
        if a[1] != kt { return Err((Some(1), format!("__get_or: the Map key is {} but got {}", kt, a[1]))); }
        if a[2] != vt { return Err((Some(2), format!("__get_or: the Map value is {} but got {}", vt, a[2]))); }
        Ok(vt)
    } },
    // add_to(m, k, delta) -> unit (P0.3, perf): `m[k] += delta` (o `= delta` si no está) en UN lookup
    // (opcode `MapAdd`, entry-API), frente a `insert(k, get_or(k,0)+delta)` que busca dos veces. Ad-hoc
    // sobre valores numéricos (int|float), como `+`; público (UFCS `m.add_to(k, d)`), sin envoltorio de
    // prelude (el constraint int|float no se expresa en una firma genérica).
    Builtin { name: "add_to", opcode: OpCode::MapAdd, check: |a| {
        arity(a, 3, "add_to", " (map, key, delta)")?;
        let (kt, vt) = match &a[0] {
            Type::Map(k, v) => ((**k).clone(), (**v).clone()),
            other => return Err((Some(0), format!("add_to expects a Map as first argument, not {}", other))),
        };
        if vt != Type::Int && vt != Type::Float {
            return Err((Some(0), format!("add_to requires a Map with int or float values, not {}", vt)));
        }
        if a[1] != kt { return Err((Some(1), format!("add_to: the Map key is {} but got {}", kt, a[1]))); }
        if a[2] != vt { return Err((Some(2), format!("add_to: the delta is {} but the Map value is {}", a[2], vt))); }
        Ok(Type::Unit)
    } },
    // __map_remove(m, k) -> [V] (M13.1b): quita k del mapa; [] si no estaba, [v] si sí. Prelude → Option.
    Builtin { name: "__map_remove", opcode: OpCode::MapRemove, check: |a| {
        arity(a, 2, "__map_remove", " (map, key)")?;
        let (kt, vt) = match &a[0] {
            Type::Map(k, v) => ((**k).clone(), (**v).clone()),
            other => return Err((Some(0), format!("__map_remove expects a Map as first argument, not {}", other))),
        };
        if a[1] != kt { return Err((Some(1), format!("__map_remove: the Map key is {} but got {}", kt, a[1]))); }
        Ok(Type::Array(Box::new(vt)))
    } },
    // --- Matemáticas (M15.1a) ---
    // Funciones unarias float -> float, todas bajo el opcode parametrizado MathF(MathFn).
    Builtin { name: "__sqrt",  opcode: OpCode::MathF(MathFn::Sqrt),  check: |a| mathf_check(a, "sqrt") },
    Builtin { name: "__sin",   opcode: OpCode::MathF(MathFn::Sin),   check: |a| mathf_check(a, "sin") },
    Builtin { name: "__cos",   opcode: OpCode::MathF(MathFn::Cos),   check: |a| mathf_check(a, "cos") },
    Builtin { name: "__tan",   opcode: OpCode::MathF(MathFn::Tan),   check: |a| mathf_check(a, "tan") },
    Builtin { name: "__ln",    opcode: OpCode::MathF(MathFn::Ln),    check: |a| mathf_check(a, "ln") },
    Builtin { name: "__log10", opcode: OpCode::MathF(MathFn::Log10), check: |a| mathf_check(a, "log10") },
    Builtin { name: "__exp",   opcode: OpCode::MathF(MathFn::Exp),   check: |a| mathf_check(a, "exp") },
    Builtin { name: "__floor", opcode: OpCode::MathF(MathFn::Floor), check: |a| mathf_check(a, "floor") },
    Builtin { name: "__ceil",  opcode: OpCode::MathF(MathFn::Ceil),  check: |a| mathf_check(a, "ceil") },
    Builtin { name: "__round", opcode: OpCode::MathF(MathFn::Round), check: |a| mathf_check(a, "round") },
    // M65.2: trig inversa y compañía (unarias → mismo opcode parametrizado).
    Builtin { name: "__asin",  opcode: OpCode::MathF(MathFn::Asin),  check: |a| mathf_check(a, "asin") },
    Builtin { name: "__acos",  opcode: OpCode::MathF(MathFn::Acos),  check: |a| mathf_check(a, "acos") },
    Builtin { name: "__atan",  opcode: OpCode::MathF(MathFn::Atan),  check: |a| mathf_check(a, "atan") },
    Builtin { name: "__log2",  opcode: OpCode::MathF(MathFn::Log2),  check: |a| mathf_check(a, "log2") },
    Builtin { name: "__trunc", opcode: OpCode::MathF(MathFn::Trunc), check: |a| mathf_check(a, "trunc") },
    // pow(base, exp) -> float.
    Builtin { name: "__pow", opcode: OpCode::Pow, check: |a| {
        arity(a, 2, "pow", " (base, exponente)")?;
        if a[0] != Type::Float { return Err((Some(0), format!("pow expects a float, not {}", a[0]))); }
        if a[1] != Type::Float { return Err((Some(1), format!("pow expects a float, not {}", a[1]))); }
        Ok(Type::Float)
    } },
    // M65.2: atan2(y, x) -> float (binaria, como pow).
    Builtin { name: "__atan2", opcode: OpCode::Atan2, check: |a| {
        arity(a, 2, "atan2", " (y, x)")?;
        if a[0] != Type::Float { return Err((Some(0), format!("atan2 expects a float, not {}", a[0]))); }
        if a[1] != Type::Float { return Err((Some(1), format!("atan2 expects a float, not {}", a[1]))); }
        Ok(Type::Float)
    } },
    // M49.1b: abs/min/max/pi/e se movieron a `std/math` como funciones puras en raylang (abs vía el
    // trait `Signed`; min/max vía `Ord`; pi/e nularias) → ya no son builtins ni tienen opcode.

    // --- Reloj (M15.1b) y aleatoriedad → M49.2: `std/time` (now/monotonic/sleep) y `std/random`. Aquí
    // solo los primitivos internos `__now`/`__monotonic`/`__sleep`/`__random`/`__random_int`. ---
    // --- FFI (M41 + revisión jul 2026): errno del hilo, envuelto por `std/ffi.errno()`. ---
    Builtin { name: "__ffi_errno", opcode: OpCode::FfiErrno, check: |a| { nullary(a, "__ffi_errno")?; Ok(Type::Int) } },

    Builtin { name: "__now",       opcode: OpCode::Now,       check: |a| { nullary(a, "__now")?; Ok(Type::Int) } },
    Builtin { name: "__monotonic", opcode: OpCode::Monotonic, check: |a| { nullary(a, "__monotonic")?; Ok(Type::Int) } },
    Builtin { name: "__monotonic_nanos", opcode: OpCode::MonotonicNanos, check: |a| { nullary(a, "__monotonic_nanos")?; Ok(Type::Int) } },
    Builtin { name: "__random",  opcode: OpCode::Random,    check: |a| { nullary(a, "__random")?; Ok(Type::Float) } },
    Builtin { name: "__sleep", opcode: OpCode::Sleep, check: |a| {
        arity(a, 1, "__sleep", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__sleep expects an int (ms), not {}", a[0]))); }
        Ok(Type::Unit)
    } },
    // M68.1: fija la semilla del PRNG (reproducibilidad; std/random.seed).
    Builtin { name: "__random_seed", opcode: OpCode::RandomSeed, check: |a| {
        arity(a, 1, "__random_seed", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__random_seed expects an int (the seed), not {}", a[0]))); }
        Ok(Type::Unit)
    } },
    Builtin { name: "__random_int", opcode: OpCode::RandomInt, check: |a| {
        arity(a, 1, "__random_int", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__random_int expects an int, not {}", a[0]))); }
        Ok(Type::Int)
    } },

    // panic(msg) -> unit (M13.2a): aborta la ejecución con `msg`. Lo usan `assert`/`assert_eq` del
    // prelude; es el único primitivo de runtime de M13.2 (el resto vive en raylang). Diverge (nunca
    // retorna), lo que aprovecha el análisis de divergencia del checker.
    Builtin { name: "panic", opcode: OpCode::Panic, check: |a| {
        arity(a, 1, "panic", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("panic expects a string, not {}", a[0]))); }
        Ok(Type::Unit)
    } },
    // eprint(x) -> unit (M11.2a): como print, pero a stderr.
    Builtin { name: "eprint", opcode: OpCode::EPrint, check: |a| {
        arity(a, 1, "eprint", "")?;
        if !printable(&a[0]) { return Err((Some(0), format!("eprint cannot print a {}", a[0]))); }
        Ok(Type::Unit)
    } },
    // __parse_int(s) -> [int] (M11.2a): [] si no parsea, [n] si sí. El prelude → Option<int>.
    // V2 (bench políglota): primitivo INTERNO de concatenación n-aria de strings. No lo escribe el
    // usuario: lo genera el checker (`lower_concat`) aplanando cadenas de `+`/interpolación. El
    // opcode de la fila es un PLACEHOLDER: la aridad es variable y el compilador emite
    // `ConcatN(argc)` por special-case (como `channel` por aridad). La regla tipa por si acaso
    // alguien lo escribe a mano: ≥2 strings → string (semántica idéntica a la cadena de `+`).
    Builtin { name: "__concat", opcode: OpCode::ConcatN(0), check: |a| {
        if a.len() < 2 {
            return Err((None, format!("__concat expects at least 2 arguments, got {}", a.len())));
        }
        for (i, t) in a.iter().enumerate() {
            if *t != Type::String {
                return Err((Some(i), format!("__concat expects strings, not {t}")));
            }
        }
        Ok(Type::String)
    } },
    // V5 (bench políglota): sort NATIVO para arreglos de primitivos. Interno: lo genera el checker
    // (`lower_sort_prim`) reescribiendo el `sort` del prelude cuando resuelve con el `impl Ord` de
    // un primitivo del prelude. La regla tipa por si alguien lo escribe a mano. `float` excluido
    // (su `<` con NaN no forma orden total; sigue por el merge sort del prelude).
    Builtin { name: "__sort_prim", opcode: OpCode::SortPrim, check: |a| {
        arity(a, 1, "__sort_prim", "")?;
        match &a[0] {
            Type::Array(t) if matches!(**t, Type::Int | Type::String | Type::Char) => Ok(a[0].clone()),
            other => Err((Some(0), format!("__sort_prim expects [int], [string] or [char], not {other}"))),
        }
    } },
    // D3 (jsondeserialize): formas FUSIONADAS de `<wrapper>(…).unwrap_or(d)` — las genera el
    // checker (`lower_prelude_fusions`) cuando el wrapper del prelude y el `unwrap_or` de
    // `OptionOps` son los del prelude (sin overrides). Reglas por si se escriben a mano.
    Builtin { name: "__index_of_or", opcode: OpCode::IndexOfOr, check: |a| {
        arity(a, 3, "__index_of_or", "")?;
        for (i, t) in a.iter().take(2).enumerate() {
            if *t != Type::String { return Err((Some(i), format!("__index_of_or expects a string, not {t}"))); }
        }
        if a[2] != Type::Int { return Err((Some(2), format!("__index_of_or expects an int default, not {}", a[2]))); }
        Ok(Type::Int)
    } },
    Builtin { name: "__parse_int_or", opcode: OpCode::ParseIntOr, check: |a| {
        arity(a, 2, "__parse_int_or", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__parse_int_or expects a string, not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__parse_int_or expects an int default, not {}", a[1]))); }
        Ok(Type::Int)
    } },
    Builtin { name: "__parse_int", opcode: OpCode::ParseInt, check: |a| {
        arity(a, 1, "__parse_int", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__parse_int expects a string, not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Int)))
    } },
    // __parse_float(s) -> [float] (M14): [] si no parsea, [f] si sí. El prelude → Option<float>.
    Builtin { name: "__parse_float", opcode: OpCode::ParseFloat, check: |a| {
        arity(a, 1, "__parse_float", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__parse_float expects a string, not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Float)))
    } },
    // __read_line() -> [string] (M11.2a): [] en EOF, [linea] si no. El prelude → Option<string>.
    Builtin { name: "__read_line", opcode: OpCode::ReadLine, check: |a| {
        nullary(a, "__read_line")?;
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __env(s) -> [string] (M11.2b): [] si no existe, [valor] si sí. El prelude → Option<string>.
    Builtin { name: "__env", opcode: OpCode::Env, check: |a| {
        arity(a, 1, "__env", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__env expects a string, not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // args() -> [string] (M11.2b): argumentos de la línea de comandos del programa.
    Builtin { name: "args", opcode: OpCode::Args, check: |a| {
        nullary(a, "args")?;
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // M88.1: signals() -> Channel<int> — el canal de señales del SO (SIGTERM/SIGINT).
    // Singleton del proceso; compone con recv/select como cualquier canal. Solo VM.
    Builtin { name: "signals", opcode: OpCode::Signals, check: |a| {
        nullary(a, "signals")?;
        Ok(Type::Channel(Box::new(Type::Int)))
    } },
    // __read_file(path) -> [string] (M11.2c): ["ok", contenido] o ["err", msg]. Prelude → Result.
    Builtin { name: "__read_file", opcode: OpCode::ReadFile, check: |a| {
        arity(a, 1, "__read_file", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__read_file expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __write_file(path, contenido) -> [string] (M11.2c): ["ok"] o ["err", msg]. Prelude → Result.
    Builtin { name: "__write_file", opcode: OpCode::WriteFile, check: |a| {
        arity(a, 2, "__write_file", " (path, contenido)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__write_file expects a string (the path), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__write_file expects a string (the contents), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __remove_file(ruta) -> [string] (M11.7c): ["ok"] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__remove_file", opcode: OpCode::RemoveFile, check: |a| {
        arity(a, 1, "__remove_file", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__remove_file expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __list_dir(ruta) -> [string] (M11.7c): ["ok", n0, …] o ["err", msg]. Prelude → Result<[string],…>.
    Builtin { name: "__list_dir", opcode: OpCode::ListDir, check: |a| {
        arity(a, 1, "__list_dir", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__list_dir expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // M67: directorios y metadatos. Las etiquetadas → [string] (["ok"(, dato)]/["err", msg]);
    // los tests → bool. std/fs las envuelve en Result/bool.
    Builtin { name: "__mkdir", opcode: OpCode::FsTagged(FsOp::Mkdir), check: |a| {
        arity(a, 1, "__mkdir", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__mkdir expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__remove_dir", opcode: OpCode::FsTagged(FsOp::RemoveDir), check: |a| {
        arity(a, 1, "__remove_dir", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__remove_dir expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__file_size", opcode: OpCode::FsTagged(FsOp::FileSize), check: |a| {
        arity(a, 1, "__file_size", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__file_size expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __stat(path) -> [string] (M115.3): ["ok", kind, mode, size, mtime_ms] o ["err", msg] — SIN
    // seguir symlinks (lstat). std/fs → Result<Stat, string>.
    Builtin { name: "__stat", opcode: OpCode::FsTagged(FsOp::Stat), check: |a| {
        arity(a, 1, "__stat", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__stat expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __chmod(path, mode) -> [string] (M115.3): ["ok"] o ["err", msg]. std/fs → Result<int,string>.
    Builtin { name: "__chmod", opcode: OpCode::Chmod, check: |a| {
        arity(a, 2, "__chmod", " (path, mode)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__chmod expects a string (the path), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__chmod expects an int (the mode bits), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__mtime", opcode: OpCode::FsTagged(FsOp::Mtime), check: |a| {
        arity(a, 1, "__mtime", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__mtime expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__rename", opcode: OpCode::FsTagged(FsOp::Rename), check: |a| {
        arity(a, 2, "__rename", " (origen, target)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__rename expects a string (the source), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__rename expects a string (the target), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__copy_file", opcode: OpCode::FsTagged(FsOp::CopyFile), check: |a| {
        arity(a, 2, "__copy_file", " (origen, target)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__copy_file expects a string (the source), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__copy_file expects a string (the target), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__is_dir", opcode: OpCode::FsTest(FsTest::IsDir), check: |a| {
        arity(a, 1, "__is_dir", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__is_dir expects a string (the path), not {}", a[0]))); }
        Ok(Type::Bool)
    } },
    Builtin { name: "__is_file", opcode: OpCode::FsTest(FsTest::IsFile), check: |a| {
        arity(a, 1, "__is_file", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__is_file expects a string (the path), not {}", a[0]))); }
        Ok(Type::Bool)
    } },
    Builtin { name: "__append_file_bytes", opcode: OpCode::AppendFileBytes, check: |a| {
        arity(a, 2, "__append_file_bytes", " (path, data)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__append_file_bytes expects a string (the path), not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("__append_file_bytes expects bytes (the data), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __open(ruta, modo) -> [string] (M11.8): ["ok", handle] o ["err", msg]. Prelude → Result<int,…>.
    Builtin { name: "__open", opcode: OpCode::Open, check: |a| {
        arity(a, 2, "__open", " (path, mode)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__open expects a string (the path), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__open expects a string (the mode), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __read_line_handle(h) -> [string] (M11.8): [] (EOF) o [linea]. Prelude → Option<string>.
    Builtin { name: "__read_line_handle", opcode: OpCode::ReadLineHandle, check: |a| {
        arity(a, 1, "__read_line_handle", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__read_line_handle expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __read_bytes_handle(h, max) -> [bytes] (M113): [b"ok", datos] | [b"eof"] | [b"err", msg].
    // std/fs → Result<Option<bytes>, string>.
    Builtin { name: "__read_bytes_handle", opcode: OpCode::ReadBytesHandle, check: |a| {
        arity(a, 2, "__read_bytes_handle", " (handle, max)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__read_bytes_handle expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__read_bytes_handle expects an int (the max), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // __seek_handle(h, pos) -> [string] (M113): ["ok", nueva_pos] o ["err", msg]. std/fs → Result<int,string>.
    Builtin { name: "__seek_handle", opcode: OpCode::SeekHandle, check: |a| {
        arity(a, 2, "__seek_handle", " (handle, pos)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__seek_handle expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__seek_handle expects an int (the position), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __write_handle(h, s) -> [string] (M11.8): ["ok"] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__write_handle", opcode: OpCode::WriteHandle, check: |a| {
        arity(a, 2, "__write_handle", " (handle, contenido)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__write_handle expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__write_handle expects a string (the content), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __write_bytes_handle(h, data) -> [string] (M115.1): ["ok"] o ["err", msg]. std/fs → Result<int,string>.
    Builtin { name: "__write_bytes_handle", opcode: OpCode::WriteBytesHandle, check: |a| {
        arity(a, 2, "__write_bytes_handle", " (handle, datos)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__write_bytes_handle expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("__write_bytes_handle expects bytes (the data), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __sync_handle(h) -> [string] (M115.1): ["ok"] o ["err", msg]. std/fs → Result<int,string>.
    Builtin { name: "__sync_handle", opcode: OpCode::SyncHandle, check: |a| {
        arity(a, 1, "__sync_handle", " (handle)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__sync_handle expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __try_lock_handle(h) -> [string] (M115.2): ["ok","1"/"0"] o ["err", msg]. std/fs → Result<bool,string>.
    Builtin { name: "__try_lock_handle", opcode: OpCode::TryLockHandle, check: |a| {
        arity(a, 1, "__try_lock_handle", " (handle)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__try_lock_handle expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __unlock_handle(h) -> [string] (M115.2): ["ok"] o ["err", msg]. std/fs → Result<int,string>.
    Builtin { name: "__unlock_handle", opcode: OpCode::UnlockHandle, check: |a| {
        arity(a, 1, "__unlock_handle", " (handle)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__unlock_handle expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __watch(path) -> [string] (M115.4): ["ok", handle] o ["err", msg]. std/fs → Result<int,string>.
    Builtin { name: "__watch", opcode: OpCode::WatchOpen, check: |a| {
        arity(a, 1, "__watch", " (path)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__watch expects a string (the path), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __watch_next(h, ms) -> [string] (M115.4): ["ok", kind, path] / ["timeout"] / ["err", msg].
    // ms <= 0 = sin plazo. En la VM APARCA la fibra (self-pipe del watcher, patrón StdinRead).
    Builtin { name: "__watch_next", opcode: OpCode::WatchNext, check: |a| {
        arity(a, 2, "__watch_next", " (handle, timeout ms)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__watch_next expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__watch_next expects an int (the timeout in ms), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // --- std/io (M107.1): stdout/stderr sin salto de línea + flush. Primitivos con arreglo
    // etiquetado ["ok"]/["err", msg]; el módulo std/io los envuelve en Result<int,string>. ---
    Builtin { name: "__stdout_write", opcode: OpCode::StdoutWrite, check: |a| {
        arity(a, 1, "__stdout_write", " (content)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__stdout_write expects a string, not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__stderr_write", opcode: OpCode::StderrWrite, check: |a| {
        arity(a, 1, "__stderr_write", " (content)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__stderr_write expects a string, not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__stdout_write_bytes", opcode: OpCode::StdoutWriteBytes, check: |a| {
        arity(a, 1, "__stdout_write_bytes", " (content)")?;
        if a[0] != Type::Bytes { return Err((Some(0), format!("__stdout_write_bytes expects bytes, not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    Builtin { name: "__stdout_flush", opcode: OpCode::StdoutFlush, check: |a| {
        nullary(a, "__stdout_flush")?;
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // --- std/io (M107.2): lectura de stdin POR BYTES. En la VM aparca la fibra (poller sobre el
    // fd 0, patrón SocketRead); el arreglo devuelto usa tags en bytes (como __proc_try_wait). ---
    // __stdin_read(max) -> [bytes]: [datos] (1..=max octetos) o [] (EOF).
    Builtin { name: "__stdin_read", opcode: OpCode::StdinRead, check: |a| {
        arity(a, 1, "__stdin_read", " (max bytes)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__stdin_read expects an int (max bytes), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // __stdin_read_timeout(max, ms) -> [bytes]: [b"data", datos] | [b"eof"] | [b"timeout"].
    Builtin { name: "__stdin_read_timeout", opcode: OpCode::StdinReadTimeout, check: |a| {
        arity(a, 2, "__stdin_read_timeout", " (max bytes, timeout ms)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__stdin_read_timeout expects an int (max bytes), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__stdin_read_timeout expects an int (the timeout in ms), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // --- std/term (M107.3): terminal — isatty, tamaño, modo crudo. ---
    // __term_is_tty(fd) -> bool: ¿el fd (0/1/2) es una terminal?
    Builtin { name: "__term_is_tty", opcode: OpCode::TermIsTty, check: |a| {
        arity(a, 1, "__term_is_tty", " (fd)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__term_is_tty expects an int (the fd), not {}", a[0]))); }
        Ok(Type::Bool)
    } },
    // __term_size() -> [int]: [cols, rows], o [] si no hay terminal.
    Builtin { name: "__term_size", opcode: OpCode::TermSize, check: |a| {
        nullary(a, "__term_size")?;
        Ok(Type::Array(Box::new(Type::Int)))
    } },
    // __term_raw_on() -> [string]: ["ok"] o ["err", msg]. Guarda el termios y activa el modo crudo.
    Builtin { name: "__term_raw_on", opcode: OpCode::TermRawOn, check: |a| {
        nullary(a, "__term_raw_on")?;
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __term_raw_off() -> [string]: ["ok"] o ["err", msg]. Restaura el termios guardado (no-op si no hay).
    Builtin { name: "__term_raw_off", opcode: OpCode::TermRawOff, check: |a| {
        nullary(a, "__term_raw_off")?;
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // --- Cliente TCP (M15.2): primitivos con arreglo etiquetado; el prelude → Result. ---
    // __tcp_connect(host, port) -> [string]: ["ok", handle] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__tcp_connect", opcode: OpCode::TcpConnect, check: |a| {
        arity(a, 2, "__tcp_connect", " (host, port)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__tcp_connect expects a string (the host), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__tcp_connect expects an int (the port), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __tcp_connect_timeout(host, port, ms) -> [string] (M122): como __tcp_connect con PLAZO — el
    // intento que agote `ms` falla con el error estable "connect timeout". std/net → Result<int,string>.
    Builtin { name: "__tcp_connect_timeout", opcode: OpCode::TcpConnectTimeout, check: |a| {
        arity(a, 3, "__tcp_connect_timeout", " (host, port, ms)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__tcp_connect_timeout expects a string (the host), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__tcp_connect_timeout expects an int (the port), not {}", a[1]))); }
        if a[2] != Type::Int { return Err((Some(2), format!("__tcp_connect_timeout expects an int (ms), not {}", a[2]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __peer_addr(h) -> [string] (M123): ["ok", "ip:puerto"] o ["err", msg] — la dirección del peer
    // de una conexión TCP/TLS. std/net → Result<string, string>.
    Builtin { name: "__peer_addr", opcode: OpCode::PeerAddr, check: |a| {
        arity(a, 1, "__peer_addr", " (handle)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__peer_addr expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __tls_peer_cert(h) -> [string] (M124): ["ok", subject, issuer, not_before_ms, not_after_ms,
    // san...] o ["err", msg]. std/net lo envuelve en el struct PeerCert (patrón stat M115.3).
    Builtin { name: "__tls_peer_cert", opcode: OpCode::TlsPeerCert, check: |a| {
        arity(a, 1, "__tls_peer_cert", " (handle)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__tls_peer_cert expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __tls_connect(host, puerto) -> [string] (M19.4a): ["ok", handle] o ["err", msg]. Prelude →
    // Result<int,string>. Igual que __tcp_connect pero cifra con TLS (rustls); el handle se lee/escribe
    // con socket_read_bytes/socket_write_bytes (que desvían a TLS) y se cierra con close.
    Builtin { name: "__tls_connect", opcode: OpCode::TlsConnect, check: |a| {
        arity(a, 2, "__tls_connect", " (host, port)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__tls_connect expects a string (the host), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__tls_connect expects an int (the port), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __tls_connect_h2(host, puerto) -> [string] (M31.2a): como __tls_connect pero ofreciendo ALPN 'h2';
    // exige que el servidor negocie HTTP/2. ["ok", handle] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__tls_connect_h2", opcode: OpCode::TlsConnectH2, check: |a| {
        arity(a, 2, "__tls_connect_h2", " (host, port)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__tls_connect_h2 expects a string (the host), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__tls_connect_h2 expects an int (the port), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __tls_accept(handle, cert, clave) -> [string] (M19.4b): envuelve un socket TCP ya aceptado en una
    // sesión TLS de servidor con el certificado/clave PEM dados. ["ok", handle] o ["err", msg]. Prelude
    // → Result<int,string>. El mismo handle se lee/escribe con socket_read_bytes/socket_write_bytes.
    Builtin { name: "__tls_accept", opcode: OpCode::TlsAccept, check: |a| {
        arity(a, 3, "__tls_accept", " (handle, cert, key)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__tls_accept expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__tls_accept expects a string (the PEM certificate), not {}", a[1]))); }
        if a[2] != Type::String { return Err((Some(2), format!("__tls_accept expects a string (the PEM key), not {}", a[2]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __tls_upgrade(h, host) -> [string] (diferido TLS): STARTTLS de cliente sobre un TCP plano;
    // ["ok", handle] (el MISMO handle) o ["err", msg]. std/net → tls_upgrade.
    Builtin { name: "__tls_upgrade", opcode: OpCode::TlsUpgrade, check: |a| {
        arity(a, 2, "__tls_upgrade", " (handle, host)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__tls_upgrade expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__tls_upgrade expects a string (the host), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __socket_read(h) -> [string]: ["ok", datos] o ["err", msg]. Prelude → Result<string,string>.
    Builtin { name: "__socket_read", opcode: OpCode::SocketRead, check: |a| {
        arity(a, 1, "__socket_read", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__socket_read expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __socket_write(h, s) -> [string]: ["ok", ""] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__socket_write", opcode: OpCode::SocketWrite, check: |a| {
        arity(a, 2, "__socket_write", " (handle, contenido)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__socket_write expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__socket_write expects a string (the content), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // --- Servidor TCP (M15.3) ---
    // __tcp_listen(host, port) -> [string]: ["ok", handle] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__tcp_listen", opcode: OpCode::TcpListen, check: |a| {
        arity(a, 2, "__tcp_listen", " (host, port)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__tcp_listen expects a string (the host), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__tcp_listen expects an int (the port), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __tcp_accept(listener) -> [string]: ["ok", handle] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__tcp_accept", opcode: OpCode::TcpAccept, check: |a| {
        arity(a, 1, "__tcp_accept", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__tcp_accept expects an int (the listening handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __local_port(h) -> int (M15.3; M50.3: __x): el puerto local del socket (0 si no aplica). Total.
    // Envoltorio net.local_port en std/net.
    Builtin { name: "__local_port", opcode: OpCode::LocalPort, check: |a| {
        arity(a, 1, "__local_port", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("local_port expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Int)
    } },
    // __socket_set_read_timeout(h, ms) -> unit (M56.4): fija (ms > 0) o quita (ms <= 0) el timeout de
    // lectura del socket. Total (un handle que no es socket se ignora). Envoltorio net.set_read_timeout.
    Builtin { name: "__socket_set_read_timeout", opcode: OpCode::SocketSetReadTimeout, check: |a| {
        arity(a, 2, "__socket_set_read_timeout", " (handle, ms)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__socket_set_read_timeout expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__socket_set_read_timeout expects an int (ms), not {}", a[1]))); }
        Ok(Type::Unit)
    } },
    // --- UDP (M20.8) ---
    // __udp_bind(host, port) -> [string]: ["ok", handle] o ["err", msg]. Lib udp.ray → Result<int,string>.
    Builtin { name: "__udp_bind", opcode: OpCode::UdpBind, check: |a| {
        arity(a, 2, "__udp_bind", " (host, port)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__udp_bind expects a string (the host), not {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__udp_bind expects an int (the port), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __udp_send_to(h, host, port, datos) -> [string]: ["ok", n] o ["err", msg]. Lib → Result<int,string>.
    Builtin { name: "__udp_send_to", opcode: OpCode::UdpSendTo, check: |a| {
        arity(a, 4, "__udp_send_to", " (handle, host, port, data)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__udp_send_to expects an int (the handle), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__udp_send_to expects a string (the host), not {}", a[1]))); }
        if a[2] != Type::Int { return Err((Some(2), format!("__udp_send_to expects an int (the port), not {}", a[2]))); }
        if a[3] != Type::Bytes { return Err((Some(3), format!("__udp_send_to expects bytes (the data), not {}", a[3]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __udp_recv_from(h) -> [bytes]: [b"ok", host, puerto, datos] o [b"err", msg] (todo en bytes, homogéneo).
    // Lib → Result<Packet,string> con Packet{host,port,data}.
    Builtin { name: "__udp_recv_from", opcode: OpCode::UdpRecvFrom, check: |a| {
        arity(a, 1, "__udp_recv_from", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__udp_recv_from expects an int (the handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // close: ad-hoc polimórfico. close(h: int) -> int (M11.8 archivo / M15.2 socket, devuelve 0) o
    // close(ch: Channel<T>) -> unit (M12.1, cierra un canal). El opcode Close ramifica en runtime.
    Builtin { name: "close", opcode: OpCode::Close, check: |a| {
        arity(a, 1, "close", "")?;
        match &a[0] {
            Type::Int => Ok(Type::Int),
            Type::Channel(_) => Ok(Type::Unit),
            other => Err((Some(0), format!("close expects a handle (int) or a Channel, not {}", other))),
        }
    } },
    // __exists(ruta) -> bool (M11.4b; M50.1 lo renombra a __x): ¿existe la ruta? Total. Envoltorio fs.exists.
    Builtin { name: "__exists", opcode: OpCode::Exists, check: |a| {
        arity(a, 1, "__exists", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("exists expects a string (the path), not {}", a[0]))); }
        Ok(Type::Bool)
    } },
    // __append_file(path, contenido) -> [string] (M11.4b): ["ok"] o ["err", msg]. Prelude → Result.
    Builtin { name: "__append_file", opcode: OpCode::AppendFile, check: |a| {
        arity(a, 2, "__append_file", " (path, contenido)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__append_file expects a string (the path), not {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__append_file expects a string (the content), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __run(program, args, dir, env, env_clear, stdin, has_stdin, timeout_ms, max_output,
    // merge_output) -> [bytes] (M100, IDEAS §53.8): las opciones van APLANADAS (el borde de builtins
    // solo ve escalares y arreglos); el resultado es el arreglo etiquetado de `run_encoded`.
    // `std/process` (fase 1c) es quien arma el builder y decodifica a Result<Output, string>.
    Builtin { name: "__run", opcode: OpCode::Run, check: |a| {
        arity(a, 10, "__run", " (program, args, dir, env, env_clear, stdin, has_stdin, timeout_ms, max_output, merge_output)")?;
        let str_arr = Type::Array(Box::new(Type::String));
        if a[0] != Type::String { return Err((Some(0), format!("__run expects a string (the program), not {}", a[0]))); }
        if a[1] != str_arr { return Err((Some(1), format!("__run expects a [string] (the arguments), not {}", a[1]))); }
        if a[2] != Type::String { return Err((Some(2), format!("__run expects a string (the directory, \"\" = inherited), not {}", a[2]))); }
        if a[3] != str_arr { return Err((Some(3), format!("__run expects a [string] (the flattened env pairs), not {}", a[3]))); }
        if a[4] != Type::Bool { return Err((Some(4), format!("__run expects a bool (env_clear), not {}", a[4]))); }
        if a[5] != Type::Bytes { return Err((Some(5), format!("__run expects bytes (the stdin data), not {}", a[5]))); }
        if a[6] != Type::Bool { return Err((Some(6), format!("__run expects a bool (has_stdin), not {}", a[6]))); }
        if a[7] != Type::Int { return Err((Some(7), format!("__run expects an int (the timeout in ms), not {}", a[7]))); }
        if a[8] != Type::Int { return Err((Some(8), format!("__run expects an int (max_output), not {}", a[8]))); }
        if a[9] != Type::Bool { return Err((Some(9), format!("__run expects a bool (merge_output), not {}", a[9]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // __proc_write(h, datos) -> [string] (M100 v3): escribe en el stdin de un hijo VIVO. Alias con
    // opcode COMPARTIDO con `__socket_write_bytes` (como `__proc_read` con SocketReadBytes): el
    // camino de escritura ya despacha por tipo de handle, y así el pipe lleno APARCA la fibra por
    // interés de escritura sin un opcode ni un aparcado propios. ["ok", ""] o ["err", msg].
    Builtin { name: "__proc_write", opcode: OpCode::SocketWriteBytes, check: |a| {
        arity(a, 2, "__proc_write", " (handle, data)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__proc_write expects an int (the stdin handle), not {}", a[0]))); }
        if a[1] != Type::Bytes { return Err((Some(1), format!("__proc_write expects bytes (the data), not {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __proc_spawn(program, args, dir, env, env_clear, stdin, has_stdin, stdin_open, merge_output) -> [bytes]
    // (M100 v2/v3): lanza en modo STREAMING → [b"ok", h_child, h_in, h_out, h_err] o
    // [b"err", msg]. Sin timeout_ms/max_output: el streaming no los tiene (canal acotado = tope).
    Builtin { name: "__proc_spawn", opcode: OpCode::ProcSpawn, check: |a| {
        arity(a, 9, "__proc_spawn", " (program, args, dir, env, env_clear, stdin, has_stdin, stdin_open, merge_output)")?;
        let str_arr = Type::Array(Box::new(Type::String));
        if a[0] != Type::String { return Err((Some(0), format!("__proc_spawn expects a string (the program), not {}", a[0]))); }
        if a[1] != str_arr { return Err((Some(1), format!("__proc_spawn expects a [string] (the arguments), not {}", a[1]))); }
        if a[2] != Type::String { return Err((Some(2), format!("__proc_spawn expects a string (the directory, \"\" = inherited), not {}", a[2]))); }
        if a[3] != str_arr { return Err((Some(3), format!("__proc_spawn expects a [string] (the flattened env pairs), not {}", a[3]))); }
        if a[4] != Type::Bool { return Err((Some(4), format!("__proc_spawn expects a bool (env_clear), not {}", a[4]))); }
        if a[5] != Type::Bytes { return Err((Some(5), format!("__proc_spawn expects bytes (the stdin data), not {}", a[5]))); }
        if a[6] != Type::Bool { return Err((Some(6), format!("__proc_spawn expects a bool (has_stdin), not {}", a[6]))); }
        if a[7] != Type::Bool { return Err((Some(7), format!("__proc_spawn expects a bool (stdin_open), not {}", a[7]))); }
        if a[8] != Type::Bool { return Err((Some(8), format!("__proc_spawn expects a bool (merge_output), not {}", a[8]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // __proc_read(h) -> [bytes] (M100 v2): una lectura del pipe del hijo — [b"ok", datos] (vacío =
    // EOF) o [b"err", msg]. REUSA el opcode SocketReadBytes (la VM ya sabe leer un handle Pipe y
    // aparcar la fibra); el nombre propio existe para que el NATIVO emita su lector de pipes
    // (__ray_proc_read) sin pasar por la ruta de sockets.
    Builtin { name: "__proc_read", opcode: OpCode::SocketReadBytes, check: |a| {
        arity(a, 1, "__proc_read", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__proc_read expects an int (the pipe handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // __proc_try_wait(h) -> [bytes] (M100 v2): ["running"] | ["code"|"signal", n] | ["err", msg].
    Builtin { name: "__proc_try_wait", opcode: OpCode::ProcTryWait, check: |a| {
        arity(a, 1, "__proc_try_wait", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__proc_try_wait expects an int (the child handle), not {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Bytes)))
    } },
    // __proc_kill(h, force) -> unit (M100 v2): SIGTERM/SIGKILL al GRUPO; no-op si ya cosechado.
    Builtin { name: "__proc_kill", opcode: OpCode::ProcKill, check: |a| {
        arity(a, 2, "__proc_kill", " (handle, force)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__proc_kill expects an int (the child handle), not {}", a[0]))); }
        if a[1] != Type::Bool { return Err((Some(1), format!("__proc_kill expects a bool (force = SIGKILL), not {}", a[1]))); }
        Ok(Type::Unit)
    } },
];


// ---------------------------------------------------------------------------
// M107.2 — lectura de stdin POR BYTES (std/io.read), sin tocar los flags del fd.
//
// Dos piezas, ambas `extern "C"` a mano (precedente de src/poll.rs, cero crates):
// - `poll(2)` con timeout 0 responde "¿hay algo que leer YA?" sin voltear stdin a O_NONBLOCK
//   (el fd 0 comparte la open file description con el shell padre: cambiarle los flags sería
//   grosero y se filtraría al terminal). Con eso la VM decide aparcar la fibra o leer.
// - `read(2)` CRUDO sobre el fd 0, sin pasar por el BufReader de `std::io::stdin()`: si se
//   leyera bufferizado, el poll del fd mentiría (los datos estarían en el buffer de Rust, no
//   en el fd). Consecuencia documentada: mezclar `input()`/`__read_line` (bufferizados) con
//   `io.read` (crudo) en el mismo programa no está soportado.
//
// El PSEUDO-HANDLE 0 identifica a stdin en la maquinaria de timeouts del scheduler
// (`mark_read_timeout`/`take_read_expired`): el registro de handles reparte desde 1, así que
// 0 nunca colisiona. Un solo lector a la vez (documentado): el opcode re-consulta `poll` al
// despertar y se re-aparca si otro consumió los datos, así que en M:1 nunca bloquea.
// ---------------------------------------------------------------------------

/// El pseudo-handle de stdin para `mark_read_timeout`/`take_read_expired` (los handles reales
/// del registro empiezan en 1).
pub const STDIN_PSEUDO_HANDLE: i64 = 0;

#[cfg(all(unix, not(target_arch = "wasm32")))]
mod stdin_host {
    unsafe extern "C" {
        fn poll(fds: *mut PollFd, nfds: u64, timeout_ms: i32) -> i32;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    }
    // (nfds_t es u32 en macOS y u64 en Linux; con nfds=1 el registro coincide en ambos.)

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    const POLLIN: i16 = 0x1;

    /// ¿Se puede leer de stdin sin bloquear (datos o EOF)? Espera hasta `timeout_ms` (0 = sondeo).
    pub fn ready(timeout_ms: i32) -> bool {
        let mut pfd = PollFd { fd: 0, events: POLLIN, revents: 0 };
        // SAFETY: un solo PollFd bien formado; poll no retiene el puntero tras volver.
        let r = unsafe { poll(&mut pfd, 1, timeout_ms) };
        r > 0 // >0 = el fd tiene evento (POLLIN o POLLHUP: ambos significan "read no bloquea")
    }

    /// `read(2)` crudo del fd 0: `Ok(octetos)` (vacío = EOF) o `Err(mensaje)`. Bloquea si no hay
    /// datos — el llamador consulta `ready` primero (la VM aparca la fibra en el poller).
    pub fn read_bytes(max: usize) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; max];
        loop {
            // SAFETY: buf vive durante la llamada y n == buf.len().
            let n = unsafe { read(0, buf.as_mut_ptr(), buf.len()) };
            if n >= 0 {
                buf.truncate(n as usize);
                return Ok(buf);
            }
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue; // EINTR: reintenta (una señal atendida no es un error de lectura)
            }
            return Err(e.to_string());
        }
    }
}

/// ¿Hay algo que leer YA en stdin (datos o EOF)? En plataformas sin `poll(2)` responde `true`
/// (la lectura bloquea, el comportamiento pre-M107; en wasm no hay stdin → "EOF listo").
pub fn stdin_ready(timeout_ms: i32) -> bool {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        stdin_host::ready(timeout_ms)
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        let _ = timeout_ms;
        true
    }
}

/// Lee hasta `max` octetos de stdin (crudo, sin buffer): `Ok(vec)` — vacío = EOF — o `Err(msg)`.
/// `max` se acota a 1..=1 MiB.
pub fn stdin_read(max: i64) -> Result<Vec<u8>, String> {
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))] // wasm: sin stdin, max no aplica
    let max = (max.max(1) as usize).min(1 << 20);
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        stdin_host::read_bytes(max)
    }
    #[cfg(all(not(unix), not(target_arch = "wasm32")))]
    {
        // Sin poll(2) (Windows): lectura bloqueante por std (el fallback honesto, como el
        // busy-poll del scheduler en esas plataformas).
        use std::io::Read;
        let mut buf = vec![0u8; max];
        let n = std::io::stdin().lock().read(&mut buf).map_err(|e| e.to_string())?;
        buf.truncate(n);
        Ok(buf)
    }
    #[cfg(target_arch = "wasm32")]
    {
        Ok(Vec::new()) // el playground no tiene stdin → EOF
    }
}

// ---------------------------------------------------------------------------
// M107.3 — terminal (std/term): isatty, tamaño y modo crudo. `extern "C"` a mano
// (precedente de src/poll.rs, cero crates), con dos trucos que evitan el detalle por SO:
//
// - El `termios` se maneja como BUFFER OPACO de 128 bytes alineados: nunca tocamos sus campos
//   (macOS 72 bytes, glibc ~60 — 128 cubre a ambos con margen), así que el layout da igual;
//   `cfmakeraw(3)` (libc en macOS y glibc) rellena los flags del modo crudo por nosotros —
//   sin reproducir la tabla de constantes ICANON/ECHO/… que difiere por plataforma.
// - `atexit(3)` registra la restauración: cubre la salida normal Y `std::process::exit` (los
//   tres caminos del CLI). Lo que NO cubre: una señal fatal o kill -9 — como cualquier
//   programa de terminal; `reset` lo arregla (documentado en MANUAL).
//
// `ioctl` es VARIÁDICA (el mismo gotcha de `fcntl` en arm64): se declara `...`.
// ---------------------------------------------------------------------------

#[cfg(all(unix, not(target_arch = "wasm32")))]
mod term_host {
    use std::sync::atomic::{AtomicBool, Ordering};

    unsafe extern "C" {
        fn isatty(fd: i32) -> i32;
        fn tcgetattr(fd: i32, t: *mut u8) -> i32;
        fn tcsetattr(fd: i32, act: i32, t: *const u8) -> i32;
        fn cfmakeraw(t: *mut u8);
        fn ioctl(fd: i32, req: u64, ...) -> i32;
        fn atexit(f: extern "C" fn()) -> i32;
    }

    /// TCSAFLUSH: aplica tras drenar la salida y descarta la entrada pendiente (2 en macOS y Linux).
    const TCSAFLUSH: i32 = 2;
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    const TIOCGWINSZ: u64 = 0x4008_7468;
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
    const TIOCGWINSZ: u64 = 0x5413;

    #[repr(C)]
    struct WinSize {
        rows: u16,
        cols: u16,
        xpixel: u16,
        ypixel: u16,
    }

    /// El termios ORIGINAL de stdin, guardado al entrar al modo crudo (buffer opaco, ver arriba).
    /// Protegido por `SAVED`: solo se lee para restaurar si el raw_on lo rellenó.
    static mut ORIGINAL: [u8; 128] = [0; 128];
    static SAVED: AtomicBool = AtomicBool::new(false);
    static ATEXIT_ARMED: AtomicBool = AtomicBool::new(false);

    pub fn is_tty(fd: i32) -> bool {
        // SAFETY: isatty solo consulta el fd.
        unsafe { isatty(fd) == 1 }
    }

    /// (cols, rows) del terminal, probando stdout → stdin → stderr; `None` si ninguno es tty.
    pub fn size() -> Option<(i64, i64)> {
        for fd in [1, 0, 2] {
            let mut ws = WinSize { rows: 0, cols: 0, xpixel: 0, ypixel: 0 };
            // SAFETY: TIOCGWINSZ escribe un WinSize bien formado; no retiene el puntero.
            if unsafe { ioctl(fd, TIOCGWINSZ, &mut ws as *mut WinSize) } == 0 && ws.cols > 0 {
                return Some((ws.cols as i64, ws.rows as i64));
            }
        }
        None
    }

    /// Restaura el termios guardado (la registra `atexit`; también la llama `raw_off`).
    extern "C" fn restore() {
        if SAVED.load(Ordering::Acquire) {
            // SAFETY: ORIGINAL se escribió completo antes de publicar SAVED (Release).
            unsafe { tcsetattr(0, TCSAFLUSH, std::ptr::addr_of!(ORIGINAL) as *const u8) };
        }
    }

    pub fn raw_on() -> Result<(), String> {
        let mut cur = [0u8; 128];
        // SAFETY: buffers de 128 bytes, mayores que cualquier termios de las plataformas soportadas.
        unsafe {
            if tcgetattr(0, cur.as_mut_ptr()) != 0 {
                return Err(format!("stdin is not a terminal: {}", std::io::Error::last_os_error()));
            }
            // Guarda el original UNA vez (si raw_on se llama dos veces, el original no se pisa
            // con un termios ya crudo).
            if !SAVED.load(Ordering::Acquire) {
                std::ptr::copy_nonoverlapping(cur.as_ptr(), std::ptr::addr_of_mut!(ORIGINAL) as *mut u8, 128);
                SAVED.store(true, Ordering::Release);
            }
            if !ATEXIT_ARMED.swap(true, Ordering::AcqRel) {
                atexit(restore);
            }
            cfmakeraw(cur.as_mut_ptr());
            if tcsetattr(0, TCSAFLUSH, cur.as_ptr()) != 0 {
                return Err(format!("could not enter raw mode: {}", std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }

    pub fn raw_off() -> Result<(), String> {
        if !SAVED.load(Ordering::Acquire) {
            return Ok(()); // nunca se entró al modo crudo: no-op
        }
        // SAFETY: ORIGINAL completo (ver restore).
        if unsafe { tcsetattr(0, TCSAFLUSH, std::ptr::addr_of!(ORIGINAL) as *const u8) } != 0 {
            return Err(format!("could not restore the terminal: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

/// ¿El fd (0/1/2) es una terminal? Fuera de unix/wasm: false.
pub fn term_is_tty(fd: i64) -> bool {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        term_host::is_tty(fd as i32)
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        let _ = fd;
        false
    }
}

/// (cols, rows) del terminal; `None` si no hay tty (o plataforma sin soporte).
pub fn term_size() -> Option<(i64, i64)> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        term_host::size()
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        None
    }
}

/// Activa el modo crudo del terminal (stdin). La restauración queda registrada con `atexit`.
pub fn term_raw_on() -> Result<(), String> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        term_host::raw_on()
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        Err("raw mode is not supported on this platform".to_string())
    }
}

/// Restaura el terminal al estado previo al primer `term_raw_on` (no-op si nunca se entró).
pub fn term_raw_off() -> Result<(), String> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        term_host::raw_off()
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        Err("raw mode is not supported on this platform".to_string())
    }
}

// ---------------------------------------------------------------------------
// M88.1 — señales del SO (SIGTERM/SIGINT) para el apagado ordenado de servicios.
//
// El truco clásico del **self-pipe**: el handler (async-signal-safe: solo un `write`)
// escribe el número de señal en un pipe; el scheduler de la VM registra el extremo de
// lectura en su poller y drena/entrega al canal `signals()`. `extern "C"` sin crates
// (el precedente de `src/poll.rs`, M17): `pipe`/`write`/`read`/`fcntl`/`signal` viven
// en libc/libSystem, siempre enlazadas.
// ---------------------------------------------------------------------------
#[cfg(all(unix, not(target_arch = "wasm32")))]
mod signals_host {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    unsafe extern "C" {
        fn pipe(fds: *mut i32) -> i32;
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        // OJO: fcntl es VARIÁDICA — declararla con aridad fija es UB en arm64 (los
        // varargs van por la pila en la convención de Apple). La declaración variádica
        // hace que Rust emita la llamada correcta.
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn signal(sig: i32, handler: usize) -> usize;
    }

    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    /// M107.4: cambio de tamaño del terminal (28 en macOS/BSD Y Linux — la rara coincidencia).
    const SIGWINCH: i32 = 28;
    const F_SETFL: i32 = 4;
    const F_SETFD: i32 = 2;
    const FD_CLOEXEC: i32 = 1;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const O_NONBLOCK: i32 = 0x0004;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const O_NONBLOCK: i32 = 0o4000;

    static PIPE_W: AtomicI32 = AtomicI32::new(-1);
    /// Bandera barata que el scheduler consulta en cada conmutación de fibra.
    pub static PENDING: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_signal(sig: i32) {
        // Async-signal-safe: solo write + stores atómicos.
        let b = sig as u8;
        let w = PIPE_W.load(Ordering::Relaxed);
        if w >= 0 {
            unsafe {
                let _ = write(w, &b, 1);
            }
        }
        PENDING.store(true, Ordering::Release);
    }

    /// Crea el pipe, instala los handlers (SIGTERM+SIGINT) y devuelve el fd de LECTURA
    /// (no bloqueante), que la VM registra en su poller.
    pub fn install() -> Result<i32, String> {
        let mut fds = [0i32; 2];
        if unsafe { pipe(fds.as_mut_ptr()) } != 0 {
            return Err("could not create the signal pipe".into());
        }
        unsafe {
            let _ = fcntl(fds[0], F_SETFL, O_NONBLOCK);
            let _ = fcntl(fds[1], F_SETFL, O_NONBLOCK);
            // CLOEXEC (auditoría jul 2026, IDEAS §53.4): `pipe(2)` crea los fds SIN FD_CLOEXEC, y
            // `F_SETFL` toca los flags de ESTADO (O_NONBLOCK), no los del DESCRIPTOR — sin esto, el
            // par de fds del self-pipe se filtra a cualquier hijo que se lance por exec. Hoy no hay
            // vía de fuga (el `exec` del proyecto es de nivel CLI y la VM no lanza procesos), pero
            // el día que exista sería una fuga silenciosa: un hijo con el extremo de ESCRITURA
            // abierto impide para siempre el EOF del pipe. `pipe2(O_CLOEXEC)` sería atómico pero no
            // existe en macOS; el `fcntl` posterior es la vía portable (sin carrera real: `install`
            // corre una sola vez, temprano, y aquí nadie hace fork).
            let _ = fcntl(fds[0], F_SETFD, FD_CLOEXEC);
            let _ = fcntl(fds[1], F_SETFD, FD_CLOEXEC);
        }
        PIPE_W.store(fds[1], Ordering::Release);
        unsafe {
            signal(SIGTERM, on_signal as *const () as usize);
            signal(SIGINT, on_signal as *const () as usize);
            // M107.4: SIGWINCH — con `select` sobre signals() + term.size(), una TUI se
            // re-maqueta al redimensionar la ventana.
            signal(SIGWINCH, on_signal as *const () as usize);
        }
        Ok(fds[0])
    }

    /// Drena UN octeto del pipe (el número de señal), o None si no hay más.
    pub fn read_one(fd: i32) -> Option<i32> {
        let mut b = 0u8;
        let n = unsafe { read(fd, &mut b, 1) };
        if n == 1 { Some(b as i32) } else { None }
    }
}

/// M88.1: instala la fontanería de señales y devuelve el fd de lectura del self-pipe.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn signals_install() -> Result<i32, String> {
    signals_host::install()
}
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
pub fn signals_install() -> Result<i32, String> {
    Err("signals() is not supported on this platform".into())
}

/// M88.1: ¿hay señales pendientes de entregar? (bandera barata para el scheduler).
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn signals_pending() -> bool {
    signals_host::PENDING.swap(false, std::sync::atomic::Ordering::AcqRel)
}
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
pub fn signals_pending() -> bool { false }

/// M88.1: drena un número de señal del self-pipe, o None si está vacío.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn signals_read_one(fd: i32) -> Option<i32> {
    signals_host::read_one(fd)
}
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
pub fn signals_read_one(_fd: i32) -> Option<i32> { None }

// --- Ejecución de procesos del SO (M100, IDEAS §53.8) ---
//
// La implementación vive en `ray_runtime::process` (feature `process`, siempre activa para este
// binario): el MISMO código para la VM/intérprete y para el binario transpilado (`__ray_run`) =
// paridad byte-idéntica por construcción, como crypto/tls/sqlite. Aquí solo el reexport y el stub
// de wasm (el playground no trae ray-runtime; `run` responde con el Err honesto de plataforma).
#[cfg(not(target_arch = "wasm32"))]
pub use ray_runtime::process::{run, run_encoded, run_opts_from_flat, RunOpts, RunOutput};

/// Stub de wasm: la firma completa, con el `Err` honesto de plataforma en `run_encoded` (los
/// motores no distinguen — es la misma codificación etiquetada del camino real).
#[cfg(target_arch = "wasm32")]
pub struct RunOpts;
#[cfg(target_arch = "wasm32")]
#[allow(clippy::too_many_arguments)] // espejo exacto de la firma real (ver ray_runtime::process)
pub fn run_opts_from_flat(
    _dir: &str,
    _env_flat: Vec<String>,
    _env_clear: bool,
    _stdin: &[u8],
    _has_stdin: bool,
    _timeout_ms: i64,
    _max_output: i64,
    _merge_output: bool,
) -> RunOpts {
    RunOpts
}
#[cfg(target_arch = "wasm32")]
pub fn run_encoded(program: &str, _args: &[String], _opts: &RunOpts) -> Vec<Vec<u8>> {
    vec![
        b"err".to_vec(),
        format!("{program}: running OS processes is not supported on this platform").into_bytes(),
    ]
}

// --- Streaming de procesos (M100 v2, IDEAS §53.9): los primitivos de host ---
//
// El registro de handles vive AQUÍ (no en ray-runtime): estos helpers son el pegamento entre
// `ray_runtime::process::spawn_streamed/try_wait/kill_group` y los handles que ven los dos motores
// del binario `ray`. El gemelo del nativo se emite en el preámbulo (fase 2c).

/// Lanza el hijo en modo streaming y lo registra: `Ok((h_child, h_out, h_err))` (`h_err = -1` con
/// merge: todo llega por `h_out`) o `Err(msg)`. Los pipes quedan como handles `Pipe`
/// no-bloqueantes: las bombas de `std/process` los leen con `__proc_read` (aparcan la fibra) y los
/// cierran con `close(h)`. La VM usa esta variante cruda para además ATAR `h_child` al scope
/// activo (fase 2e); el intérprete usa la codificada.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn proc_spawn_handles(program: &str, args: &[String], opts: &RunOpts) -> Result<(i64, i64, i64, i64), String> {
    let s = ray_runtime::process::spawn_streamed(program, args, opts)?;
    let mut reg = registry().lock().unwrap();
    let put = |reg: &mut FileRegistry, h: OpenHandle| -> i64 {
        let id = reg.next;
        reg.next += 1;
        reg.open.insert(id, h);
        id
    };
    let h_child = put(&mut reg, OpenHandle::Child(s.child));
    let h_in = s.stdin.map_or(-1, |f| put(&mut reg, OpenHandle::PipeW(f)));
    let h_out = s.out.map_or(-1, |f| put(&mut reg, OpenHandle::Pipe(f)));
    let h_err = s.err.map_or(-1, |f| put(&mut reg, OpenHandle::Pipe(f)));
    Ok((h_child, h_in, h_out, h_err))
}

#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
pub fn proc_spawn_handles(program: &str, _args: &[String], _opts: &RunOpts) -> Result<(i64, i64, i64, i64), String> {
    Err(format!("{program}: running OS processes is not supported on this platform"))
}

/// El resultado de `proc_spawn_handles`, aplanado al arreglo etiquetado del builtin `__proc_spawn`:
/// `[b"ok", h_child, h_out, h_err]` o `[b"err", msg]`.
pub fn proc_spawn_encode(r: Result<(i64, i64, i64, i64), String>) -> Vec<Vec<u8>> {
    match r {
        Ok((h_child, h_in, h_out, h_err)) => vec![
            b"ok".to_vec(),
            h_child.to_string().into_bytes(),
            h_in.to_string().into_bytes(),
            h_out.to_string().into_bytes(),
            h_err.to_string().into_bytes(),
        ],
        Err(e) => vec![b"err".to_vec(), e.into_bytes()],
    }
}

/// Spawn + codificación en un paso (el camino del intérprete, que no tiene scopes que atar).
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn proc_spawn_encoded(program: &str, args: &[String], opts: &RunOpts) -> Vec<Vec<u8>> {
    proc_spawn_encode(proc_spawn_handles(program, args, opts))
}

/// Cosecha ESTRUCTURAL (fase 2e): `SIGKILL` al GRUPO del hijo del handle + `wait` (tras el KILL
/// retorna enseguida) + eliminación del registro. Total e idempotente: un handle ya cosechado por
/// `__proc_try_wait` es un no-op — el registro ES el desatado. La llaman los ganchos de
/// cancelación/cierre de scope de la VM y el `__RayScopeChild` del nativo.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn proc_kill_and_reap(h: i64) {
    let mut reg = registry().lock().unwrap();
    let Some(OpenHandle::Child(child)) = reg.open.get_mut(&h) else { return };
    ray_runtime::process::kill_group(child.id() as i32, true);
    let _ = child.wait();
    reg.open.remove(&h);
}
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
pub fn proc_kill_and_reap(_h: i64) {}

/// `waitpid(WNOHANG)` del hijo del handle: `[b"running"]`, `[b"code", n]` / `[b"signal", n]`
/// (cosechado — y el handle se ELIMINA bajo el mismo lock: un `__proc_kill` posterior es no-op,
/// nunca una señal a un pid REUSADO por otro proceso) o `[b"err", msg]` (handle inválido).
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn proc_try_wait_encoded(h: i64) -> Vec<Vec<u8>> {
    let mut reg = registry().lock().unwrap();
    let Some(OpenHandle::Child(child)) = reg.open.get_mut(&h) else {
        return vec![b"err".to_vec(), format!("handle {h} is not a child process").into_bytes()];
    };
    let result = match ray_runtime::process::try_wait(child) {
        Ok(None) => return vec![b"running".to_vec()],
        Ok(Some(Ok(c))) => vec![b"code".to_vec(), c.to_string().into_bytes()],
        Ok(Some(Err(s))) => vec![b"signal".to_vec(), s.to_string().into_bytes()],
        Err(e) => vec![b"err".to_vec(), e.into_bytes()],
    };
    reg.open.remove(&h); // cosechado (o irrecuperable): fuera del registro
    result
}

/// `SIGTERM` (o `SIGKILL` con `force`) al GRUPO del hijo del handle. Total e idempotente: un
/// handle ya cosechado/cerrado es un no-op (matar a un muerto no es un error).
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn proc_kill(h: i64, force: bool) {
    let reg = registry().lock().unwrap();
    if let Some(OpenHandle::Child(child)) = reg.open.get(&h) {
        ray_runtime::process::kill_group(child.id() as i32, force);
    }
}

/// Stubs de plataforma (Windows/wasm): el mismo `Err` honesto que `run`.
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
pub fn proc_spawn_encoded(program: &str, _args: &[String], _opts: &RunOpts) -> Vec<Vec<u8>> {
    vec![
        b"err".to_vec(),
        format!("{program}: running OS processes is not supported on this platform").into_bytes(),
    ]
}
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
pub fn proc_try_wait_encoded(h: i64) -> Vec<Vec<u8>> {
    vec![b"err".to_vec(), format!("handle {h} is not a child process").into_bytes()]
}
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
pub fn proc_kill(_h: i64, _force: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership() {
        assert!(is_builtin("print"));
        assert!(is_builtin("args"));
        assert!(!is_builtin("noexiste"));
        assert!(!is_builtin("map")); // map/filter/fold son del prelude, no builtins
    }

    #[test]
    fn every_user_builtin_has_doc() {
        // La documentación (en inglés) es parte del contrato de la tabla: cada builtin de cara
        // al usuario (sin prefijo `__`) debe tener su entrada en `doc()`; añadir un builtin sin
        // documentarlo rompe este test. Los internos `__*` no la necesitan.
        let without_doc: Vec<&str> = names()
            .filter(|n| !n.starts_with("__") && doc(n).is_none())
            .collect();
        assert!(without_doc.is_empty(), "builtins sin doc(): {without_doc:?}");
        assert!(doc("__parse_int").is_none(), "los internos no llevan doc");
        assert!(doc("noexiste").is_none());
    }

    #[test]
    fn methods_for_only_names_real_builtins() {
        // M45: cada nombre listado por categoría debe ser un builtin real (evita que un
        // rename de builtin deje `methods_for` ofreciendo un método inexistente).
        for cat in ["string", "bytes", "char", "int", "float", "bool", "array", "map"] {
            for m in methods_for(cat) {
                assert!(is_builtin(m) || signature(m).is_some(), "methods_for({cat:?}) nombra '{m}', what no es builtin ni método conocido con signature()");
            }
        }
        assert!(methods_for("noexiste").is_empty());
    }

    #[test]
    fn todo_builtin_method_has_signature() {
        // M46a: cada builtin ofrecible como método debe tener firma (para el detalle del popup).
        for cat in ["string", "bytes", "char", "int", "float", "bool", "array", "map"] {
            for m in methods_for(cat) {
                assert!(signature(m).is_some(), "methods_for({cat:?}) nombra '{m}' sin signature()");
            }
        }
    }

    #[test]
    fn rule_ok_and_errors() {
        // M48.4e-3: `split` público se retiró; su gemelo interno `__split` conserva la misma regla.
        let split = lookup("__split").unwrap();
        // Firma correcta → tipo de retorno.
        assert_eq!((split.check)(&[Type::String, Type::String]), Ok(Type::Array(Box::new(Type::String))));
        // Aridad mal → error general (índice None: lo ubica el sitio de llamada).
        assert!(matches!((split.check)(&[Type::String]), Err((None, _))));
        // Tipo de un arg mal → error con el índice del argumento culpable.
        assert!(matches!((split.check)(&[Type::Int, Type::String]), Err((Some(0), _))));
    }

    /// M53.3: los helpers de SQLite (rusqlite) — abrir en memoria, exec con parámetros, query con
    /// celdas aplanadas (NULL → ""), error SQL como valor, y close vía el registro común.
    /// M89: solo con la feature `sqlite` (el build slim compila los stubs).
    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_opens_executes_and_queries() {
        let h = sqlite_open(":memory:").unwrap();
        sqlite_exec(h, "CREATE TABLE t (id INTEGER, name TEXT, nota REAL)", &[]).unwrap();
        let n = sqlite_exec(h, "INSERT INTO t VALUES (?1, ?2, ?3)", &["1".into(), "ada".into(), "9.5".into()]).unwrap();
        assert_eq!(n, 1);
        sqlite_exec(h, "INSERT INTO t (id, name) VALUES (?1, ?2)", &["2".into(), "grace".into()]).unwrap();
        let (ncols, cells) = sqlite_query(h, "SELECT id, name, nota FROM t ORDER BY id", &[]).unwrap();
        assert_eq!(ncols, 3);
        assert_eq!(cells, vec!["1", "ada", "9.5", "2", "grace", ""]); // NULL → ""
        // Error SQL = valor, no panic; handle cerrado = error claro.
        assert!(sqlite_query(h, "SELECT * FROM no_existe", &[]).unwrap_err().contains("no_existe"));
        close_handle(h);
        assert!(sqlite_exec(h, "SELECT 1", &[]).unwrap_err().contains("invalid or already closed"));
    }

    #[test]
    fn push_is_homogeneous() {
        // M48.4e-3: `push` público se retiró; su gemelo interno `__push` conserva la misma regla.
        let push = lookup("__push").unwrap();
        let xs_int = Type::Array(Box::new(Type::Int));
        assert_eq!((push.check)(&[xs_int.clone(), Type::Int]), Ok(Type::Unit));
        assert!(matches!((push.check)(&[xs_int, Type::String]), Err((Some(1), _))));
    }

    // D1 (jsondeserialize): el fast-path ASCII de index_of/substring debe ser INDISTINGUIBLE del
    // camino por carácter. Ambos motores comparten estos helpers → el oráculo VM↔intérprete no
    // cazaría una divergencia aquí; este test fija el contrato por-carácter directamente.
    #[test]
    fn index_of_and_substring_ascii_fast_path_matches_char_semantics() {
        // ASCII: el fast-path (bytes) da los mismos índices que la semántica por carácter.
        assert_eq!(char_index_of("{\"id\":7,\"name\":\"u\"}", ",\"name\":\""), Some(7));
        assert_eq!(char_index_of("abc", ""), Some(0));
        assert_eq!(char_index_of("abc", "zz"), None);
        assert_eq!(substring_chars("hello world", 6, 11), "world");
        assert_eq!(substring_chars("abc", -5, 99), "abc"); // clamp
        assert_eq!(substring_chars("abc", 2, 1), ""); // i > j → vacío
        // No-ASCII: sigue el camino por carácter — los índices son de CARÁCTER, no de byte.
        assert_eq!(char_index_of("añô€x", "€x"), Some(3));
        assert_eq!(substring_chars("añô€x", 1, 4), "ñô€");
        assert_eq!(char_index_of("añô", "z"), None);
    }
}

