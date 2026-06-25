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
use crate::bytecode::{MathFn, OpCode};

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

/// Milisegundos de un reloj **monótono**: ancla un `Instant` de referencia en la primera llamada y
/// devuelve el tiempo transcurrido desde él. Sirve para medir intervalos. Builtin `monotonic`.
pub fn monotonic_millis() -> i64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as i64
}

/// Duerme el hilo `ms` milisegundos (`ms<=0` → no duerme). Builtin `sleep`.
pub fn sleep_millis(ms: i64) {
    if ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
    }
}

/// El estado del PRNG del proceso. `std` no trae generador de aleatorios y la invariante es **cero
/// dependencias de Cargo**, así que llevamos uno propio: **SplitMix64**, sembrado del reloj la
/// primera vez. No es criptográfico (es para simulación/jitter/ids, no para secretos).
fn rng() -> &'static std::sync::Mutex<u64> {
    static R: std::sync::OnceLock<std::sync::Mutex<u64>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let semilla = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        std::sync::Mutex::new(semilla ^ 0x9E37_79B9_7F4A_7C15)
    })
}

/// Avanza el generador y devuelve los siguientes 64 bits (SplitMix64).
fn next_u64() -> u64 {
    let mut estado = rng().lock().expect("RNG no envenenado");
    *estado = estado.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *estado;
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

/// Añade `contents` al final del archivo `path` (lo crea si no existe). Helper compartido por ambos
/// motores para el primitivo `__append_file` (M11.4b); la *impl* de ejecución no es metadato, pero
/// es idéntica en los dos motores, así que vive aquí para no duplicarse.
pub fn append_to_file(path: &str, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(contents.as_bytes())
}

/// Índice de **carácter** de la primera ocurrencia de `sub` en `s` (M11.7a). Por carácter (no por
/// byte), consistente con `len`/`chars`/`s[i]`. `sub` vacío → `Some(0)`. Helper compartido por ambos
/// motores (`__index_of`).
pub fn char_index_of(s: &str, sub: &str) -> Option<usize> {
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
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len() as i64;
    let lo = i.clamp(0, n);
    let hi = j.clamp(lo, n); // hi >= lo → rango vacío si i > j
    chars[lo as usize..hi as usize].iter().collect()
}

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
        _ => return Err(format!("modo de apertura inválido: '{}' (usa \"r\", \"w\" o \"a\")", mode)),
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

/// Escribe `s` en el handle; `Ok(nº de caracteres)` o `Err(mensaje)` (M11.8).
pub fn write_handle(h: i64, s: &str) -> Result<usize, String> {
    use std::io::Write;
    let mut reg = registry().lock().unwrap();
    match reg.open.get_mut(&h) {
        Some(OpenHandle::Writer(f)) => f.write_all(s.as_bytes()).map(|_| s.chars().count()).map_err(|e| e.to_string()),
        Some(OpenHandle::Reader(_)) => Err("el handle está abierto para lectura, no escritura".to_string()),
        Some(OpenHandle::Tcp(_)) => Err("el handle es un socket; usa socket_write".to_string()),
        Some(OpenHandle::Listener(_)) => Err("el handle es un socket de escucha, no escribible".to_string()),
        None => Err(format!("handle de archivo inválido: {}", h)),
    }
}

/// Cierra el handle (lo quita del registro; el `Drop` del archivo/socket libera el recurso) (M11.8).
pub fn close_handle(h: i64) {
    registry().lock().unwrap().open.remove(&h);
}

// --- Cliente TCP (M15.2) ---
//
// Sobre `std::net::TcpStream`, cero deps. El handle es un `int` y vive en el MISMO registro que los
// archivos. Para no retener el `Mutex` del registro durante un I/O **bloqueante**, los helpers de
// lectura/escritura **clonan** el stream (`try_clone` = `dup` del descriptor) y sueltan el lock antes.

/// Conecta a `host:port` (resuelve el nombre vía `std::net`) y devuelve un handle (M15.2).
pub fn tcp_connect(host: &str, port: i64) -> Result<i64, String> {
    let stream = std::net::TcpStream::connect((host, port as u16)).map_err(|e| e.to_string())?;
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
        Some(_) => Err(format!("el handle {} no es un socket", h)),
        None => Err(format!("handle inválido: {}", h)),
    }
}

/// Hace **una** lectura del socket (hasta 64 KiB) y devuelve lo leído como `string` (UTF-8 *lossy*);
/// `""` indica EOF (el otro extremo cerró). Bloquea hasta que haya datos (M15.2).
pub fn socket_read(h: i64) -> Result<String, String> {
    use std::io::Read;
    let mut stream = socket_clone(h)?;
    let mut buf = [0u8; 65536];
    let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}

/// Escribe `s` completo en el socket; `Ok(nº de bytes)` o `Err(mensaje)` (M15.2).
///
/// Bucle de escritura manual (no `write_all`) para tolerar sockets **no bloqueantes** (M15.5): en un
/// socket bloqueante `write` nunca da `WouldBlock` y esto equivale a `write_all`; en uno no bloqueante,
/// gira (`yield_now`) hasta poder escribir. La escritura NO es punto de cesión del scheduler (cargas
/// reales —líneas, respuestas cortas— nunca giran; una escritura gigante a un peer que no lee sí).
pub fn socket_write(h: i64, s: &str) -> Result<usize, String> {
    use std::io::Write;
    let mut stream = socket_clone(h)?;
    let bytes = s.as_bytes();
    let mut off = 0;
    while off < bytes.len() {
        match stream.write(&bytes[off..]) {
            Ok(0) => return Err("la conexión se cerró durante la escritura".to_string()),
            Ok(n) => off += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::yield_now(),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(bytes.len())
}

// --- Sockets no bloqueantes para el scheduler de la VM (M15.5) ---
//
// El intérprete usa los sockets BLOQUEANTES de arriba (un solo hilo). La VM los voltea a NO bloqueantes
// con `set_nonblocking` y usa estos helpers, que devuelven `Ok(None)` para señalar `WouldBlock` (la VM
// aparca la fibra y reintenta). Así `tcp_accept`/`socket_read` ceden al scheduler en vez de bloquear.

/// Pone el socket (conexión o escucha) del handle `h` en modo **no bloqueante** (M15.5). Lo llama la VM
/// tras crear el socket; el intérprete nunca, así que sus sockets siguen bloqueantes.
pub fn set_nonblocking(h: i64) -> Result<(), String> {
    let reg = registry().lock().unwrap();
    match reg.open.get(&h) {
        Some(OpenHandle::Tcp(s)) => s.set_nonblocking(true).map_err(|e| e.to_string()),
        Some(OpenHandle::Listener(l)) => l.set_nonblocking(true).map_err(|e| e.to_string()),
        _ => Err(format!("el handle {} no es un socket", h)),
    }
}

/// Lectura **no bloqueante**: `Ok(Some(datos))` (o `Some("")` en EOF), `Ok(None)` si aún no hay datos
/// (`WouldBlock` → la VM aparca), `Err` en error real (M15.5).
pub fn socket_read_nb(h: i64) -> Result<Option<String>, String> {
    use std::io::Read;
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
            Some(_) => return Err(format!("el handle {} no es un socket de escucha", h)),
            None => return Err(format!("handle inválido: {}", h)),
        }
    };
    match listener.accept() {
        Ok((stream, _)) => {
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
    let listener = std::net::TcpListener::bind((host, port as u16)).map_err(|e| e.to_string())?;
    let mut reg = registry().lock().unwrap();
    let id = reg.next;
    reg.next += 1;
    reg.open.insert(id, OpenHandle::Listener(listener));
    Ok(id)
}

/// Bloquea hasta una conexión entrante en el handle de escucha `h` y devuelve un handle de conexión
/// (un socket normal). Clona el listener para no retener el lock durante el `accept()` bloqueante (M15.3).
pub fn tcp_accept(h: i64) -> Result<i64, String> {
    let listener = {
        let reg = registry().lock().unwrap();
        match reg.open.get(&h) {
            Some(OpenHandle::Listener(l)) => l.try_clone().map_err(|e| e.to_string())?,
            Some(_) => return Err(format!("el handle {} no es un socket de escucha", h)),
            None => return Err(format!("handle inválido: {}", h)),
        }
    };
    let (stream, _addr) = listener.accept().map_err(|e| e.to_string())?;
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
        _ => 0,
    }
}

/// Lista los nombres de las entradas de un directorio (M11.7c). Helper compartido por ambos motores
/// (`__list_dir`). Ordenados para que el resultado sea **determinista** (el sistema no garantiza orden).
pub fn list_dir(path: &str) -> std::io::Result<Vec<String>> {
    let mut nombres: Vec<String> = std::fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    nombres.sort();
    Ok(nombres)
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
fn arity(a: &[Type], n: usize, nombre: &str, detalle: &str) -> Result<(), BuiltinError> {
    if a.len() != n {
        let plural = if n == 1 { "argumento" } else { "argumentos" };
        return Err((None, format!("{} espera {} {}{}, se le pasaron {}", nombre, n, plural, detalle, a.len())));
    }
    Ok(())
}

/// Error de aridad para builtins sin argumentos.
fn nullary(a: &[Type], nombre: &str) -> Result<(), BuiltinError> {
    if !a.is_empty() {
        return Err((None, format!("{} no espera argumentos, se le pasaron {}", nombre, a.len())));
    }
    Ok(())
}

/// ¿Es un tipo que `print`/`eprint` saben imprimir? (Coincide con `is_printable` del checker.)
fn printable(t: &Type) -> bool {
    matches!(
        t,
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Array(_)
            | Type::Struct(_, _) | Type::Fn(_, _) | Type::Enum(_, _) | Type::Var(_)
    )
}

/// Regla de tipado de una función matemática unaria `float -> float` (M15.1a).
fn mathf_check(a: &[Type], nombre: &str) -> Result<Type, BuiltinError> {
    arity(a, 1, nombre, "")?;
    if a[0] != Type::Float {
        return Err((Some(0), format!("{} espera un float, no {}", nombre, a[0])));
    }
    Ok(Type::Float)
}

/// Regla de tipado de un builtin numérico **ad-hoc polimórfico** unario (`abs`): `int -> int` o
/// `float -> float`. Conserva el tipo numérico del argumento.
fn numeric_unary_check(a: &[Type], nombre: &str) -> Result<Type, BuiltinError> {
    arity(a, 1, nombre, "")?;
    match a[0] {
        Type::Int => Ok(Type::Int),
        Type::Float => Ok(Type::Float),
        _ => Err((Some(0), format!("{} espera un int o un float, no {}", nombre, a[0]))),
    }
}

/// Regla de tipado de un builtin numérico ad-hoc polimórfico binario (`min`/`max`): ambos
/// argumentos del mismo tipo numérico (`int` o `float`); devuelve ese tipo.
fn numeric_binary_check(a: &[Type], nombre: &str) -> Result<Type, BuiltinError> {
    arity(a, 2, nombre, "")?;
    if !matches!(a[0], Type::Int | Type::Float) {
        return Err((Some(0), format!("{} espera un int o un float, no {}", nombre, a[0])));
    }
    if a[1] != a[0] {
        return Err((Some(1), format!("{}: ambos argumentos deben ser del mismo tipo ({} vs {})", nombre, a[0], a[1])));
    }
    Ok(a[0].clone())
}

/// La tabla. El orden no importa (la búsqueda es por nombre).
static BUILTINS: &[Builtin] = &[
    // print(x) -> unit: imprime un imprimible a stdout.
    Builtin { name: "print", opcode: OpCode::Print, check: |a| {
        arity(a, 1, "print", "")?;
        if !printable(&a[0]) { return Err((Some(0), format!("print no puede imprimir un {}", a[0]))); }
        Ok(Type::Unit)
    } },
    // len(a) -> int: longitud de un arreglo, un string (M11.1a: nº de caracteres) o un Map (M13.1).
    Builtin { name: "len", opcode: OpCode::Len, check: |a| {
        arity(a, 1, "len", "")?;
        if !matches!(a[0], Type::Array(_) | Type::String | Type::Map(_, _)) {
            return Err((Some(0), format!("len espera un arreglo, un string o un Map, no {}", a[0])));
        }
        Ok(Type::Int)
    } },
    // push(a, x) -> unit: agrega x al final del arreglo a (lo muta).
    Builtin { name: "push", opcode: OpCode::Push, check: |a| {
        arity(a, 2, "push", " (arreglo, valor)")?;
        let elem = match &a[0] {
            Type::Array(e) => (**e).clone(),
            other => return Err((Some(0), format!("push espera un arreglo como primer argumento, no {}", other))),
        };
        if a[1] != elem {
            return Err((Some(1), format!("push: el arreglo es de {} pero se empuja {}", elem, a[1])));
        }
        Ok(Type::Unit)
    } },
    // to_string(x) -> string (M11.1a): representación textual de un primitivo imprimible.
    Builtin { name: "to_string", opcode: OpCode::ToString, check: |a| {
        arity(a, 1, "to_string", "")?;
        if !matches!(a[0], Type::Int | Type::Float | Type::Bool | Type::String | Type::Char) {
            return Err((Some(0), format!("to_string solo convierte int/float/bool/string/char, no {}", a[0])));
        }
        Ok(Type::String)
    } },
    // trim(s) -> string (M11.1b): quita el espacio en blanco de los extremos.
    Builtin { name: "trim", opcode: OpCode::Trim, check: |a| {
        arity(a, 1, "trim", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("trim espera un string, no {}", a[0]))); }
        Ok(Type::String)
    } },
    // split(s, sep) -> [string] (M11.1b): parte s por el separador sep.
    Builtin { name: "split", opcode: OpCode::Split, check: |a| {
        arity(a, 2, "split", " (string, separador)")?;
        if a[0] != Type::String { return Err((Some(0), format!("split espera un string como primer argumento, no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("split espera un string como separador, no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // contains(x, y) -> bool: ad-hoc polimórfico. String: ¿s contiene la subcadena sub? (M11.4a).
    // Arreglo: ¿el arreglo contiene el elemento x (por igualdad estructural)? (M11.7b).
    Builtin { name: "contains", opcode: OpCode::Contains, check: |a| {
        arity(a, 2, "contains", " (string/arreglo, valor)")?;
        match &a[0] {
            Type::String => {
                if a[1] != Type::String { return Err((Some(1), format!("contains espera un string como subcadena, no {}", a[1]))); }
            }
            Type::Array(elem) => {
                if a[1] != **elem { return Err((Some(1), format!("contains: el arreglo es de {} pero se busca {}", elem, a[1]))); }
            }
            _ => return Err((Some(0), format!("contains espera un string o un arreglo, no {}", a[0]))),
        }
        Ok(Type::Bool)
    } },
    // replace(s, de, a) -> string (M11.4a): reemplaza todas las ocurrencias de `de` por `a`.
    Builtin { name: "replace", opcode: OpCode::Replace, check: |a| {
        arity(a, 3, "replace", " (string, de, a)")?;
        if a[0] != Type::String { return Err((Some(0), format!("replace espera un string como primer argumento, no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("replace espera un string en 'de', no {}", a[1]))); }
        if a[2] != Type::String { return Err((Some(2), format!("replace espera un string en 'a', no {}", a[2]))); }
        Ok(Type::String)
    } },
    // chars(s) -> [char] (M11.4c-2): los caracteres del string, en orden.
    Builtin { name: "chars", opcode: OpCode::Chars, check: |a| {
        arity(a, 1, "chars", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("chars espera un string, no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Char)))
    } },
    // starts_with(s, pre) -> bool (M11.7a): ¿`s` empieza con `pre`?
    Builtin { name: "starts_with", opcode: OpCode::StartsWith, check: |a| {
        arity(a, 2, "starts_with", " (string, prefijo)")?;
        if a[0] != Type::String { return Err((Some(0), format!("starts_with espera un string como primer argumento, no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("starts_with espera un string como prefijo, no {}", a[1]))); }
        Ok(Type::Bool)
    } },
    // ends_with(s, suf) -> bool (M11.7a): ¿`s` termina con `suf`?
    Builtin { name: "ends_with", opcode: OpCode::EndsWith, check: |a| {
        arity(a, 2, "ends_with", " (string, sufijo)")?;
        if a[0] != Type::String { return Err((Some(0), format!("ends_with espera un string como primer argumento, no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("ends_with espera un string como sufijo, no {}", a[1]))); }
        Ok(Type::Bool)
    } },
    // to_upper(s) -> string (M11.7a): en MAYÚSCULAS.
    Builtin { name: "to_upper", opcode: OpCode::ToUpper, check: |a| {
        arity(a, 1, "to_upper", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("to_upper espera un string, no {}", a[0]))); }
        Ok(Type::String)
    } },
    // to_lower(s) -> string (M11.7a): en minúsculas.
    Builtin { name: "to_lower", opcode: OpCode::ToLower, check: |a| {
        arity(a, 1, "to_lower", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("to_lower espera un string, no {}", a[0]))); }
        Ok(Type::String)
    } },
    // substring(s, i, j) -> string (M11.7a): subcadena [i, j) por índice de carácter (con clamp).
    Builtin { name: "substring", opcode: OpCode::Substring, check: |a| {
        arity(a, 3, "substring", " (string, inicio, fin)")?;
        if a[0] != Type::String { return Err((Some(0), format!("substring espera un string como primer argumento, no {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("substring espera un int como inicio, no {}", a[1]))); }
        if a[2] != Type::Int { return Err((Some(2), format!("substring espera un int como fin, no {}", a[2]))); }
        Ok(Type::String)
    } },
    // repeat(s, n) -> string (M11.7a): `s` repetido `n` veces (`n<=0` → "").
    Builtin { name: "repeat", opcode: OpCode::Repeat, check: |a| {
        arity(a, 2, "repeat", " (string, veces)")?;
        if a[0] != Type::String { return Err((Some(0), format!("repeat espera un string como primer argumento, no {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("repeat espera un int como nº de veces, no {}", a[1]))); }
        Ok(Type::String)
    } },
    // __index_of(s, sub) -> [int] (M11.7a): [] o [i] (índice de carácter). El prelude → Option<int>.
    Builtin { name: "__index_of", opcode: OpCode::IndexOf, check: |a| {
        arity(a, 2, "__index_of", " (string, subcadena)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__index_of espera un string como primer argumento, no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__index_of espera un string como subcadena, no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::Int)))
    } },
    // join(arr, sep) -> string (M11.7a): une un [string] con el separador `sep`.
    // join **ad-hoc polimórfico** (como `close`): `join(arr: [string], sep) -> string` (M11.7a) o
    // `join(t: Task<T>) -> T` (M12.3, une una tarea). raylang no tiene sobrecarga, así que un único
    // builtin que ramifica por el tipo del primer argumento; el compilador elige el opcode por la aridad.
    Builtin { name: "join", opcode: OpCode::Join, check: |a| {
        if matches!(a.first(), Some(Type::Task(_))) {
            arity(a, 1, "join", " (una Task)")?;
            match &a[0] {
                Type::Task(t) => return Ok((**t).clone()),
                _ => unreachable!(),
            }
        }
        arity(a, 2, "join", " (arreglo de string, separador)")?;
        if a[0] != Type::Array(Box::new(Type::String)) {
            return Err((Some(0), format!("join espera un [string] o una Task como primer argumento, no {}", a[0])));
        }
        if a[1] != Type::String { return Err((Some(1), format!("join espera un string como separador, no {}", a[1]))); }
        Ok(Type::String)
    } },
    // reverse(a) -> [T] (M11.7b): arreglo nuevo con los elementos en orden inverso.
    Builtin { name: "reverse", opcode: OpCode::Reverse, check: |a| {
        arity(a, 1, "reverse", "")?;
        match &a[0] {
            Type::Array(_) => Ok(a[0].clone()),
            other => Err((Some(0), format!("reverse espera un arreglo, no {}", other))),
        }
    } },
    // __pop(a) -> [T] (M11.7b): muta `a` quitando el último; [] si vacío, [x] si no. Prelude → Option<T>.
    Builtin { name: "__pop", opcode: OpCode::ArrayPop, check: |a| {
        arity(a, 1, "__pop", "")?;
        match &a[0] {
            Type::Array(elem) => Ok(Type::Array(elem.clone())),
            other => Err((Some(0), format!("__pop espera un arreglo, no {}", other))),
        }
    } },
    // __position(a, x) -> [int] (M11.7b): [] o [i] (índice de la 1ª ocurrencia). Prelude → Option<int>.
    Builtin { name: "__position", opcode: OpCode::Position, check: |a| {
        arity(a, 2, "__position", " (arreglo, valor)")?;
        match &a[0] {
            Type::Array(elem) => {
                if a[1] != **elem { return Err((Some(1), format!("__position: el arreglo es de {} pero se busca {}", elem, a[1]))); }
            }
            other => return Err((Some(0), format!("__position espera un arreglo, no {}", other))),
        }
        Ok(Type::Array(Box::new(Type::Int)))
    } },
    // --- Mapas Map<K,V> (M13.1) ---
    // map_new() -> Map<K,V>: mapa vacío. Su tipo es INDETERMINADO (como `[]`/`None`): lo fija el
    // tipo esperado en `check_expr_expected`. Por eso esta regla (sin tipo esperado) es un error;
    // el camino normal lo intercepta antes de llegar aquí.
    Builtin { name: "map_new", opcode: OpCode::MapNew, check: |a| {
        arity(a, 0, "map_new", "")?;
        Err((None, "no se puede inferir el tipo de map_new; anótalo, p. ej. 'let m: Map<string, int> = map_new()'".into()))
    } },

    // --- Concurrencia: CSP sobre la VM (M12.1). Solo la VM las ejecuta; el intérprete da error limpio. ---
    // spawn(f: fn() -> T) -> Task<T>: lanza f (sin parámetros) como green thread y devuelve su handle
    // (M12.3; en M12.1/M12.2 devolvía unit y el handle no existía).
    Builtin { name: "spawn", opcode: OpCode::Spawn, check: |a| {
        arity(a, 1, "spawn", " (una función sin parámetros)")?;
        match &a[0] {
            Type::Fn(params, ret) if params.is_empty() => Ok(Type::Task(ret.clone())),
            Type::Fn(_, _) => Err((Some(0), "spawn requiere una función SIN parámetros (fn() -> T)".into())),
            other => Err((Some(0), format!("spawn espera una función, no {}", other))),
        }
    } },
    // select(chs: [Channel<T>]) -> int: bloquea hasta que algún canal de la lista esté listo para recibir
    // y devuelve el índice del primero listo (M12.4). Luego recv(chs[i]) toma el valor.
    Builtin { name: "select", opcode: OpCode::Select, check: |a| {
        arity(a, 1, "select", " (un arreglo de canales)")?;
        match &a[0] {
            Type::Array(el) if matches!(&**el, Type::Channel(_)) => Ok(Type::Int),
            other => Err((Some(0), format!("select espera un [Channel<T>], no {}", other))),
        }
    } },
    // scope(body: fn() -> R) -> R: corre body; al volver, une todas las tareas lanzadas dentro y propaga
    // un fallo si lo hubo (M12.3 structured concurrency). El compilador lo baja con ScopeBegin/ScopeEnd.
    Builtin { name: "scope", opcode: OpCode::ScopeBegin, check: |a| {
        arity(a, 1, "scope", " (una función sin parámetros)")?;
        match &a[0] {
            Type::Fn(params, ret) if params.is_empty() => Ok((**ret).clone()),
            Type::Fn(_, _) => Err((Some(0), "scope requiere una función SIN parámetros (fn() -> R)".into())),
            other => Err((Some(0), format!("scope espera una función, no {}", other))),
        }
    } },
    // channel() / channel(n) -> Channel<T>: crea un canal (no acotado, o acotado a la capacidad n: int ≥ 0,
    // M12.2). Indeterminado en el tipo de elemento (como map_new): el camino con tipo esperado lo
    // intercepta; aquí solo validamos la aridad/capacidad y damos el error de inferencia.
    Builtin { name: "channel", opcode: OpCode::ChannelNew, check: |a| {
        if a.len() > 1 {
            return Err((Some(1), "channel recibe a lo sumo un argumento (la capacidad)".into()));
        }
        if matches!(a.first(), Some(t) if !matches!(t, Type::Int)) {
            return Err((Some(0), format!("la capacidad de channel debe ser int, no {}", a[0])));
        }
        Err((None, "no se puede inferir el tipo de channel; anótalo, p. ej. 'let c: Channel<int> = channel()'".into()))
    } },
    // send(ch, v) -> unit: envía v por el canal ch.
    Builtin { name: "send", opcode: OpCode::ChanSend, check: |a| {
        arity(a, 2, "send", " (canal, valor)")?;
        let et = match &a[0] {
            Type::Channel(t) => (**t).clone(),
            other => return Err((Some(0), format!("send espera un Channel como primer argumento, no {}", other))),
        };
        if a[1] != et { return Err((Some(1), format!("send: el canal es de {} pero se pasó {}", et, a[1]))); }
        Ok(Type::Unit)
    } },
    // __recv(ch) -> [T]: recibe (primitivo). [v] si hay valor, [] si cerrado+vacío; bloquea si vacío+abierto.
    // El prelude lo envuelve en recv(ch) -> Option<T>.
    Builtin { name: "__recv", opcode: OpCode::ChanRecv, check: |a| {
        arity(a, 1, "__recv", " (canal)")?;
        match &a[0] {
            Type::Channel(t) => Ok(Type::Array(Box::new((**t).clone()))),
            other => Err((Some(0), format!("__recv espera un Channel, no {}", other))),
        }
    } },
    // close: ad-hoc polimórfico (cerrar un recurso). Un Channel (M12.1) → unit; un handle de archivo
    // (int, M11.8) → int. Una sola entrada `close` (más abajo) lo cubre; NO se duplica aquí (raylang no
    // tiene sobrecarga). El canal se cierra con `close(ch)` igual que un handle con `close(h)`.

    // insert(m, k, v) -> unit: inserta/actualiza la clave k con el valor v en el mapa m (lo muta).
    Builtin { name: "insert", opcode: OpCode::MapInsert, check: |a| {
        arity(a, 3, "insert", " (mapa, clave, valor)")?;
        let (kt, vt) = match &a[0] {
            Type::Map(k, v) => ((**k).clone(), (**v).clone()),
            other => return Err((Some(0), format!("insert espera un Map como primer argumento, no {}", other))),
        };
        if a[1] != kt { return Err((Some(1), format!("insert: la clave del Map es {} pero se pasó {}", kt, a[1]))); }
        if a[2] != vt { return Err((Some(2), format!("insert: el valor del Map es {} pero se pasó {}", vt, a[2]))); }
        Ok(Type::Unit)
    } },
    // contains_key(m, k) -> bool: ¿está la clave k en el mapa m?
    Builtin { name: "contains_key", opcode: OpCode::MapContainsKey, check: |a| {
        arity(a, 2, "contains_key", " (mapa, clave)")?;
        let kt = match &a[0] {
            Type::Map(k, _) => (**k).clone(),
            other => return Err((Some(0), format!("contains_key espera un Map como primer argumento, no {}", other))),
        };
        if a[1] != kt { return Err((Some(1), format!("contains_key: la clave del Map es {} pero se pasó {}", kt, a[1]))); }
        Ok(Type::Bool)
    } },
    // __map_get(m, k) -> [V]: [] si la clave no está, [v] si está. El prelude → Option<V>.
    Builtin { name: "__map_get", opcode: OpCode::MapGet, check: |a| {
        arity(a, 2, "__map_get", " (mapa, clave)")?;
        let (kt, vt) = match &a[0] {
            Type::Map(k, v) => ((**k).clone(), (**v).clone()),
            other => return Err((Some(0), format!("__map_get espera un Map como primer argumento, no {}", other))),
        };
        if a[1] != kt { return Err((Some(1), format!("__map_get: la clave del Map es {} pero se pasó {}", kt, a[1]))); }
        Ok(Type::Array(Box::new(vt)))
    } },
    // __map_remove(m, k) -> [V] (M13.1b): quita k del mapa; [] si no estaba, [v] si sí. Prelude → Option.
    Builtin { name: "__map_remove", opcode: OpCode::MapRemove, check: |a| {
        arity(a, 2, "__map_remove", " (mapa, clave)")?;
        let (kt, vt) = match &a[0] {
            Type::Map(k, v) => ((**k).clone(), (**v).clone()),
            other => return Err((Some(0), format!("__map_remove espera un Map como primer argumento, no {}", other))),
        };
        if a[1] != kt { return Err((Some(1), format!("__map_remove: la clave del Map es {} pero se pasó {}", kt, a[1]))); }
        Ok(Type::Array(Box::new(vt)))
    } },
    // keys(m) -> [K] (M13.1b): las claves del mapa, ordenadas (determinista).
    Builtin { name: "keys", opcode: OpCode::MapKeys, check: |a| {
        arity(a, 1, "keys", " (mapa)")?;
        match &a[0] {
            Type::Map(k, _) => Ok(Type::Array(k.clone())),
            other => Err((Some(0), format!("keys espera un Map, no {}", other))),
        }
    } },
    // values(m) -> [V] (M13.1b): los valores, en orden de clave ordenada (casa con keys).
    Builtin { name: "values", opcode: OpCode::MapValues, check: |a| {
        arity(a, 1, "values", " (mapa)")?;
        match &a[0] {
            Type::Map(_, v) => Ok(Type::Array(v.clone())),
            other => Err((Some(0), format!("values espera un Map, no {}", other))),
        }
    } },

    // --- Matemáticas (M15.1a) ---
    // Funciones unarias float -> float, todas bajo el opcode parametrizado MathF(MathFn).
    Builtin { name: "sqrt",  opcode: OpCode::MathF(MathFn::Sqrt),  check: |a| mathf_check(a, "sqrt") },
    Builtin { name: "sin",   opcode: OpCode::MathF(MathFn::Sin),   check: |a| mathf_check(a, "sin") },
    Builtin { name: "cos",   opcode: OpCode::MathF(MathFn::Cos),   check: |a| mathf_check(a, "cos") },
    Builtin { name: "tan",   opcode: OpCode::MathF(MathFn::Tan),   check: |a| mathf_check(a, "tan") },
    Builtin { name: "ln",    opcode: OpCode::MathF(MathFn::Ln),    check: |a| mathf_check(a, "ln") },
    Builtin { name: "log10", opcode: OpCode::MathF(MathFn::Log10), check: |a| mathf_check(a, "log10") },
    Builtin { name: "exp",   opcode: OpCode::MathF(MathFn::Exp),   check: |a| mathf_check(a, "exp") },
    Builtin { name: "floor", opcode: OpCode::MathF(MathFn::Floor), check: |a| mathf_check(a, "floor") },
    Builtin { name: "ceil",  opcode: OpCode::MathF(MathFn::Ceil),  check: |a| mathf_check(a, "ceil") },
    Builtin { name: "round", opcode: OpCode::MathF(MathFn::Round), check: |a| mathf_check(a, "round") },
    // pow(base, exp) -> float.
    Builtin { name: "pow", opcode: OpCode::Pow, check: |a| {
        arity(a, 2, "pow", " (base, exponente)")?;
        if a[0] != Type::Float { return Err((Some(0), format!("pow espera un float, no {}", a[0]))); }
        if a[1] != Type::Float { return Err((Some(1), format!("pow espera un float, no {}", a[1]))); }
        Ok(Type::Float)
    } },
    // abs(x): int -> int / float -> float (ad-hoc polimórfico).
    Builtin { name: "abs", opcode: OpCode::Abs, check: |a| numeric_unary_check(a, "abs") },
    // min/max(a, b): mismo tipo numérico (ad-hoc polimórfico).
    Builtin { name: "min", opcode: OpCode::Min, check: |a| numeric_binary_check(a, "min") },
    Builtin { name: "max", opcode: OpCode::Max, check: |a| numeric_binary_check(a, "max") },
    // Constantes π y e (Euler).
    Builtin { name: "pi", opcode: OpCode::Pi, check: |a| { nullary(a, "pi")?; Ok(Type::Float) } },
    Builtin { name: "e",  opcode: OpCode::E,  check: |a| { nullary(a, "e")?; Ok(Type::Float) } },

    // --- Reloj y aleatoriedad (M15.1b) ---
    Builtin { name: "now",       opcode: OpCode::Now,       check: |a| { nullary(a, "now")?; Ok(Type::Int) } },
    Builtin { name: "monotonic", opcode: OpCode::Monotonic, check: |a| { nullary(a, "monotonic")?; Ok(Type::Int) } },
    Builtin { name: "random",    opcode: OpCode::Random,    check: |a| { nullary(a, "random")?; Ok(Type::Float) } },
    Builtin { name: "sleep", opcode: OpCode::Sleep, check: |a| {
        arity(a, 1, "sleep", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("sleep espera un int (ms), no {}", a[0]))); }
        Ok(Type::Unit)
    } },
    Builtin { name: "random_int", opcode: OpCode::RandomInt, check: |a| {
        arity(a, 1, "random_int", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("random_int espera un int, no {}", a[0]))); }
        Ok(Type::Int)
    } },

    // panic(msg) -> unit (M13.2a): aborta la ejecución con `msg`. Lo usan `assert`/`assert_eq` del
    // prelude; es el único primitivo de runtime de M13.2 (el resto vive en raylang). Diverge (nunca
    // retorna), lo que aprovecha el análisis de divergencia del checker.
    Builtin { name: "panic", opcode: OpCode::Panic, check: |a| {
        arity(a, 1, "panic", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("panic espera un string, no {}", a[0]))); }
        Ok(Type::Unit)
    } },
    // eprint(x) -> unit (M11.2a): como print, pero a stderr.
    Builtin { name: "eprint", opcode: OpCode::EPrint, check: |a| {
        arity(a, 1, "eprint", "")?;
        if !printable(&a[0]) { return Err((Some(0), format!("eprint no puede imprimir un {}", a[0]))); }
        Ok(Type::Unit)
    } },
    // __parse_int(s) -> [int] (M11.2a): [] si no parsea, [n] si sí. El prelude → Option<int>.
    Builtin { name: "__parse_int", opcode: OpCode::ParseInt, check: |a| {
        arity(a, 1, "__parse_int", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__parse_int espera un string, no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Int)))
    } },
    // __parse_float(s) -> [float] (M14): [] si no parsea, [f] si sí. El prelude → Option<float>.
    Builtin { name: "__parse_float", opcode: OpCode::ParseFloat, check: |a| {
        arity(a, 1, "__parse_float", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__parse_float espera un string, no {}", a[0]))); }
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
        if a[0] != Type::String { return Err((Some(0), format!("__env espera un string, no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // args() -> [string] (M11.2b): argumentos de la línea de comandos del programa.
    Builtin { name: "args", opcode: OpCode::Args, check: |a| {
        nullary(a, "args")?;
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __read_file(path) -> [string] (M11.2c): ["ok", contenido] o ["err", msg]. Prelude → Result.
    Builtin { name: "__read_file", opcode: OpCode::ReadFile, check: |a| {
        arity(a, 1, "__read_file", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__read_file espera un string (la ruta), no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __write_file(path, contenido) -> [string] (M11.2c): ["ok"] o ["err", msg]. Prelude → Result.
    Builtin { name: "__write_file", opcode: OpCode::WriteFile, check: |a| {
        arity(a, 2, "__write_file", " (ruta, contenido)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__write_file espera un string (la ruta), no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__write_file espera un string (el contenido), no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __remove_file(ruta) -> [string] (M11.7c): ["ok"] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__remove_file", opcode: OpCode::RemoveFile, check: |a| {
        arity(a, 1, "__remove_file", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__remove_file espera un string (la ruta), no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __list_dir(ruta) -> [string] (M11.7c): ["ok", n0, …] o ["err", msg]. Prelude → Result<[string],…>.
    Builtin { name: "__list_dir", opcode: OpCode::ListDir, check: |a| {
        arity(a, 1, "__list_dir", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__list_dir espera un string (la ruta), no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __open(ruta, modo) -> [string] (M11.8): ["ok", handle] o ["err", msg]. Prelude → Result<int,…>.
    Builtin { name: "__open", opcode: OpCode::Open, check: |a| {
        arity(a, 2, "__open", " (ruta, modo)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__open espera un string (la ruta), no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__open espera un string (el modo), no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __read_line_handle(h) -> [string] (M11.8): [] (EOF) o [linea]. Prelude → Option<string>.
    Builtin { name: "__read_line_handle", opcode: OpCode::ReadLineHandle, check: |a| {
        arity(a, 1, "__read_line_handle", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__read_line_handle espera un int (el handle), no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __write_handle(h, s) -> [string] (M11.8): ["ok"] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__write_handle", opcode: OpCode::WriteHandle, check: |a| {
        arity(a, 2, "__write_handle", " (handle, contenido)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__write_handle espera un int (el handle), no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__write_handle espera un string (el contenido), no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // --- Cliente TCP (M15.2): primitivos con arreglo etiquetado; el prelude → Result. ---
    // __tcp_connect(host, port) -> [string]: ["ok", handle] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__tcp_connect", opcode: OpCode::TcpConnect, check: |a| {
        arity(a, 2, "__tcp_connect", " (host, puerto)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__tcp_connect espera un string (el host), no {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__tcp_connect espera un int (el puerto), no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __socket_read(h) -> [string]: ["ok", datos] o ["err", msg]. Prelude → Result<string,string>.
    Builtin { name: "__socket_read", opcode: OpCode::SocketRead, check: |a| {
        arity(a, 1, "__socket_read", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__socket_read espera un int (el handle), no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __socket_write(h, s) -> [string]: ["ok", ""] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__socket_write", opcode: OpCode::SocketWrite, check: |a| {
        arity(a, 2, "__socket_write", " (handle, contenido)")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__socket_write espera un int (el handle), no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__socket_write espera un string (el contenido), no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // --- Servidor TCP (M15.3) ---
    // __tcp_listen(host, port) -> [string]: ["ok", handle] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__tcp_listen", opcode: OpCode::TcpListen, check: |a| {
        arity(a, 2, "__tcp_listen", " (host, puerto)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__tcp_listen espera un string (el host), no {}", a[0]))); }
        if a[1] != Type::Int { return Err((Some(1), format!("__tcp_listen espera un int (el puerto), no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __tcp_accept(listener) -> [string]: ["ok", handle] o ["err", msg]. Prelude → Result<int,string>.
    Builtin { name: "__tcp_accept", opcode: OpCode::TcpAccept, check: |a| {
        arity(a, 1, "__tcp_accept", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("__tcp_accept espera un int (el handle de escucha), no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // local_port(h) -> int (M15.3): el puerto local del socket (0 si no aplica). Total.
    Builtin { name: "local_port", opcode: OpCode::LocalPort, check: |a| {
        arity(a, 1, "local_port", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("local_port espera un int (el handle), no {}", a[0]))); }
        Ok(Type::Int)
    } },
    // close: ad-hoc polimórfico. close(h: int) -> int (M11.8 archivo / M15.2 socket, devuelve 0) o
    // close(ch: Channel<T>) -> unit (M12.1, cierra un canal). El opcode Close ramifica en runtime.
    Builtin { name: "close", opcode: OpCode::Close, check: |a| {
        arity(a, 1, "close", "")?;
        match &a[0] {
            Type::Int => Ok(Type::Int),
            Type::Channel(_) => Ok(Type::Unit),
            other => Err((Some(0), format!("close espera un handle (int) o un Channel, no {}", other))),
        }
    } },
    // exists(ruta) -> bool (M11.4b): ¿existe la ruta? Total (no falla).
    Builtin { name: "exists", opcode: OpCode::Exists, check: |a| {
        arity(a, 1, "exists", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("exists espera un string (la ruta), no {}", a[0]))); }
        Ok(Type::Bool)
    } },
    // __append_file(path, contenido) -> [string] (M11.4b): ["ok"] o ["err", msg]. Prelude → Result.
    Builtin { name: "__append_file", opcode: OpCode::AppendFile, check: |a| {
        arity(a, 2, "__append_file", " (ruta, contenido)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__append_file espera un string (la ruta), no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__append_file espera un string (el contenido), no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membresia() {
        assert!(is_builtin("print"));
        assert!(is_builtin("args"));
        assert!(!is_builtin("noexiste"));
        assert!(!is_builtin("map")); // map/filter/fold son del prelude, no builtins
    }

    #[test]
    fn regla_ok_y_errores() {
        let split = lookup("split").unwrap();
        // Firma correcta → tipo de retorno.
        assert_eq!((split.check)(&[Type::String, Type::String]), Ok(Type::Array(Box::new(Type::String))));
        // Aridad mal → error general (índice None: lo ubica el sitio de llamada).
        assert!(matches!((split.check)(&[Type::String]), Err((None, _))));
        // Tipo de un arg mal → error con el índice del argumento culpable.
        assert!(matches!((split.check)(&[Type::Int, Type::String]), Err((Some(0), _))));
    }

    #[test]
    fn push_es_homogeneo() {
        let push = lookup("push").unwrap();
        let xs_int = Type::Array(Box::new(Type::Int));
        assert_eq!((push.check)(&[xs_int.clone(), Type::Int]), Ok(Type::Unit));
        assert!(matches!((push.check)(&[xs_int, Type::String]), Err((Some(1), _))));
    }
}
