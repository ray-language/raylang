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
use crate::bytecode::OpCode;

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

/// Un archivo abierto: lectura bufferizada o escritura.
enum OpenHandle {
    Reader(std::io::BufReader<std::fs::File>),
    Writer(std::fs::File),
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
        None => Err(format!("handle de archivo inválido: {}", h)),
    }
}

/// Cierra el handle (lo quita del registro; el `Drop` del archivo vuelca lo pendiente) (M11.8).
pub fn close_handle(h: i64) {
    registry().lock().unwrap().open.remove(&h);
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
    Builtin { name: "join", opcode: OpCode::Join, check: |a| {
        arity(a, 2, "join", " (arreglo de string, separador)")?;
        if a[0] != Type::Array(Box::new(Type::String)) {
            return Err((Some(0), format!("join espera un [string] como primer argumento, no {}", a[0])));
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
    // close(h) -> int (M11.8): cierra el handle; devuelve 0 (total).
    Builtin { name: "close", opcode: OpCode::Close, check: |a| {
        arity(a, 1, "close", "")?;
        if a[0] != Type::Int { return Err((Some(0), format!("close espera un int (el handle), no {}", a[0]))); }
        Ok(Type::Int)
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
