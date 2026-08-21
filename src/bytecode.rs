//! Bytecode de raylang: el lenguaje intermedio que ejecuta la VM (M2).
//!
//! En vez de recorrer el AST en cada evaluación (como el intérprete de M1), lo
//! **compilamos una vez** a una secuencia de instrucciones simples y planas, y la
//! VM las ejecuta sobre una pila. Esa secuencia, junto a sus constantes, vive en un
//! `Chunk`.
//!
//! ## Nota de representación
//!
//! Una VM "de verdad" (como la de Lua o la de CPython) empaqueta las instrucciones
//! en **bytes** para densidad de caché. Aquí usamos un `enum` por instrucción
//! (`Vec<OpCode>`): es lo idiomático en Rust y muchísimo más claro para aprender,
//! a costa de algo de densidad. Empaquetar a bytes sería una optimización posterior.

use crate::runtime::Value;

/// M67: las operaciones de fs etiquetadas del opcode `FsTagged` (la aridad la da `argc`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FsOp {
    Mkdir,
    RemoveDir,
    FileSize,
    Rename,
    CopyFile,
    Mtime,
}

impl FsOp {
    /// Nº de argumentos string (rutas) que saca de la pila.
    pub fn argc(self) -> usize {
        match self {
            FsOp::Mkdir | FsOp::RemoveDir | FsOp::FileSize | FsOp::Mtime => 1,
            FsOp::Rename | FsOp::CopyFile => 2,
        }
    }
}

/// M67: los tests totales de fs del opcode `FsTest`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FsTest {
    IsDir,
    IsFile,
}

/// Las funciones matemáticas unarias `float -> float` (M15.1a). Van todas bajo el mismo opcode
/// `OpCode::MathF(MathFn)`: el opcode dice "aplica una función matemática" y este enum **cuál**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MathFn {
    Sqrt,
    Sin,
    Cos,
    Tan,
    Ln,
    Log10,
    Exp,
    Floor,
    Ceil,
    Round,
    // M65.2: trig inversa y compañía.
    Asin,
    Acos,
    Atan,
    Log2,
    Trunc,
}

/// Una instrucción de la VM. Las que llevan operando (como `Constant`) lo guardan
/// inline.
/// El tipo destino de un `as` (M27.4). El opcode `Cast` convierte según el valor en runtime.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CastTarget {
    Int,
    Float,
    Char,
    UInt(u8), // M28.3: u8/u32/u64 (el u8 es el ancho en bits)
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpCode {
    /// Empuja `constants[idx]` a la pila.
    Constant(usize),
    /// Empuja un booleano literal.
    True,
    False,

    // Aritmética: sacan 2 operandos, empujan 1 resultado.
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// Niega el número en la cima (`-x`).
    Negate,
    /// Niega el booleano en la cima (`!b`).
    Not,

    // Bit a bit (M19.3a): operandos int. Las binarias sacan 2 y empujan 1; `BitNot` saca 1.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    /// Complemento a uno del int en la cima (`~x`).
    BitNot,

    // Comparación: sacan 2, empujan un bool.
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,

    // --- Pila y control de flujo ---
    /// Descarta el valor de la cima.
    Pop,
    /// Empuja el valor unit `()`.
    Unit,
    /// Salta incondicionalmente a la instrucción en el índice dado.
    Jump(usize),
    /// Si la cima es `false`, salta al índice dado. **No** saca la condición de la
    /// pila (la "ojea"); el compilador emite un `Pop` explícito donde corresponde.
    /// Esto es lo que permite el cortocircuito de `&&`/`||`.
    JumpIfFalse(usize),

    // --- Variables locales y llamadas (M2.3) ---
    /// Empuja a la pila el valor del slot local `slot` del marco actual.
    GetLocal(usize),
    /// Saca la cima y la guarda en el slot local `slot` del marco actual.
    SetLocal(usize),
    /// **Superinstrucción** (M36.1): empuja `local[s]` y luego `local[t]` en una sola instrucción.
    /// Fusiona el par `GetLocal(s); GetLocal(t)` que produce el compilador para cargar los dos operandos
    /// de un binario `a op b` → una iteración del lazo de despacho en vez de dos. Semántica idéntica.
    GetLocalLocal(usize, usize),
    /// **Superinstrucción** (M36.1): empuja `local[s]` y luego `constants[c]`. Fusiona `GetLocal(s);
    /// Constant(c)` (el patrón de `x op <literal>`, `i < N`, …). Semántica idéntica.
    GetLocalConst(usize, usize),
    /// A4 (ronda 2): comparación + salto-si-falso + pop, en UNA instrucción. Fusiona el
    /// patrón de TODA guarda de `if`/`while` (`[Cmp, JumpIfFalse(t), Pop]` con `code[t] == Pop`):
    /// saca los dos operandos, compara, y si es falso salta a `t+1` (tras el Pop del lado else,
    /// que la fusión ya consumió conceptualmente — el bool nunca llega a la pila).
    CmpJump(CmpOp, usize),
    /// P0.6 (ronda 3): la guarda ENTERA de `if`/`while` sobre `local op const` en una instrucción
    /// (`[GetLocalConst(slot, const), CmpJump(op, target)]`): lee `local[slot]` y `const[const]`,
    /// compara, y si es falso salta a `target` — sin apilar ni sacar los operandos. Es el par de
    /// opcodes más ejecutado en fib/bucles (la guarda `n < 2`, `i < N`). Campos: `(slot, const, op, target)`.
    GetLocalConstCmpJump(usize, usize, CmpOp, usize),
    /// MM2 (ronda 4, bench matrixmul): indexación con base e índice LOCALES en una instrucción
    /// (`[GetLocalLocal(s, t), Index]` → `IndexLL(s, t)`): empuja `local[s][local[t]]`. Es la forma
    /// `a[i]` — el patrón dominante de los bucles numéricos sobre arreglos. Misma semántica que
    /// `Index` (arrays/IntArray/string/bytes, bounds).
    IndexLL(usize, usize),
    /// MM2 (ronda 4): `[GetLocal(t), Index]` → `IndexLocal(t)`: saca la base de la pila e indexa por
    /// `local[t]`. Es el segundo nivel de `a[i][k]` (la base ya está en la pila) y el `x[k]` suelto.
    IndexLocal(usize),
    /// V9 (ronda 5): la guarda entera de `if`/`while` sobre DOS locales en una instrucción
    /// (`[GetLocalLocal(a, b), CmpJump(op, target)]`): compara `local[a] op local[b]` y si es
    /// falso salta a `target`, sin apilar ni sacar operandos. Es la guarda `i < n` con tope en
    /// VARIABLE — el patrón de todo for-range cuyo límite no es constante (la ronda 3 solo
    /// cubría `local op const`).
    LocalLocalCmpJump(usize, usize, CmpOp, usize),
    /// V9 (ronda 5): `local[s] = local[s] + const[c]` sin pasar por la pila
    /// (`[AddLocalConst(s, c), SetLocal(s)]`): el incremento de todo bucle contado.
    IncLocalConst(usize, usize),
    /// V9 (ronda 5): el CIERRE completo de un bucle contado en una instrucción
    /// (`[AddLocalConst(s, c), SetLocal(s), Jump(target)]`): incrementa el índice y salta
    /// atrás a la guarda. Junto a las guardas fusionadas deja el overhead de bucle en 1+1
    /// instrucciones por iteración.
    IncJump(usize, usize, usize),
    /// A4 (ronda 2): `local[slot] + const` en una instrucción (`[GetLocalConst, Add]`).
    AddLocalConst(usize, usize),
    /// A4 (ronda 2): `local[slot] - const` en una instrucción (`[GetLocalConst, Sub]`).
    SubLocalConst(usize, usize),
    /// **Declara** un slot local (M4.2): saca la cima y la guarda inicializando el
    /// slot. Distinto de `SetLocal` porque si el slot está *boxeado* (capturado por
    /// una closure), crea una **celda nueva** — cada declaración estrena celda, lo
    /// que hace seguro el shadowing.
    InitLocal(usize),
    /// Llama a `functions[idx]` tomando `argc` argumentos de la pila.
    Call(usize, usize),
    /// **Llamada en cola** (M13.3b): como `Call`, pero **reutiliza el marco actual** en vez de
    /// apilar uno nuevo (recursión de cola en O(1) marcos). Lo emite un *peephole* del compilador
    /// cuando una llamada va seguida —directa o vía saltos— de un `Return`.
    TailCall(usize, usize),
    /// **Llamada a función externa** (M41, FFI): `CallExtern(extern_idx, argc)`. Saca `argc`
    /// argumentos de la pila, los marshala, llama a la función C descrita por `externs[extern_idx]`
    /// (`dlopen`/`dlsym` + transmutación de firma) y empuja el resultado. La frontera insegura.
    CallExtern(usize, usize),
    /// Builtin `__ffi_errno` (std/ffi.errno): empuja el `errno` del hilo actual como int — el
    /// motivo del último fallo de una extern C estilo POSIX. Leer inmediatamente tras la llamada.
    FfiErrno,
    /// Builtin `print`: saca un valor, lo imprime, y empuja unit.
    Print,

    // --- Funciones de primera clase (M4.1) ---
    /// Empuja un valor-función: `functions[idx]` como dato (sin llamarla). Solo para
    /// funciones **sin** captura.
    Function(usize),
    /// Llamada indirecta: en la pila están el valor-función y luego `argc`
    /// argumentos encima. Saca los argumentos y la función, y empuja un marco.
    CallValue(usize),
    /// **Llamada indirecta en cola** (M13.3b): como `CallValue`, pero reutiliza el marco actual.
    TailCallValue(usize),

    // --- Closures (M4.2) ---
    /// Construye una closure de `functions[idx]`: arma su arreglo de upvalues
    /// tomando las celdas que indica `functions[idx].upvalues` del marco actual, y
    /// empuja el valor closure.
    Closure(usize),
    /// Empuja el valor del upvalue `i` de la closure en ejecución (lee su celda).
    GetUpvalue(usize),
    /// Saca la cima y la escribe en el upvalue `i` (muta su celda compartida).
    SetUpvalue(usize),

    // --- Arreglos (M3) ---
    /// Saca `n` valores de la pila y construye un arreglo con ellos (en orden);
    /// empuja el arreglo.
    MakeArray(usize),
    /// Saca el índice y el arreglo; empuja el elemento (chequea límites).
    Index,
    /// Saca valor, índice y arreglo; asigna `arreglo[índice] = valor`.
    SetIndex,
    /// Saca un arreglo **o string**; empuja su longitud (int). Builtin `len`. (M11.1a: string
    /// → nº de caracteres.)
    Len,
    /// M27.4: conversión numérica `as`. Saca un valor y empuja su conversión al tipo destino (según el
    /// tipo del valor en runtime: int↔float, char↔int).
    Cast(CastTarget),
    /// Saca valor y arreglo; agrega el valor al final; empuja unit. Builtin `push`.
    Push,

    // --- Stdlib de string (M11.1) ---
    /// Saca un valor primitivo; empuja su representación textual (string). Builtin `to_string`.
    ToString,
    /// V2 (bench políglota): concatenación **n-aria** de strings. Saca `n` strings y empuja su
    /// concatenación, construida UNA vez con la capacidad exacta (suma de longitudes) — frente a la
    /// cadena de `Add` par a par, que creaba n−1 strings intermedios. Lo emite el compilador para el
    /// primitivo interno `__concat(a, b, …)`, que genera el checker (`lower_concat`) aplanando las
    /// cadenas de `+`/interpolación de strings.
    ConcatN(usize),
    /// V5 (bench políglota): ordena un arreglo de PRIMITIVOS (int/string/char) con el sort de Rust
    /// y empuja un arreglo NUEVO (como el `sort` del prelude, que no muta). Lo emite el compilador
    /// para `__sort_prim(a)`, que genera el checker (`lower_sort_prim`) cuando el `sort` genérico
    /// resuelve con el `impl Ord` de un primitivo del prelude (sin overrides del usuario).
    SortPrim,
    /// D3 (jsondeserialize): `index_of(s, sub).unwrap_or(d)` FUSIONADO — saca `d`, `sub`, `s` y
    /// empuja el índice (por carácter) o `d`. Cero arreglo etiquetado, cero Option, cero marcos de
    /// wrapper (el camino sin fusión pagaba ~3 allocs de heap + 2 llamadas por uso). Lo genera el
    /// checker (`lower_prelude_fusions`) cuando wrapper y `unwrap_or` son los del prelude.
    IndexOfOr,
    /// D3: `parse_int(s).unwrap_or(d)` fusionado — saca `d` y `s`, empuja el int parseado
    /// (`trim().parse::<i64>()`, la misma semántica que el primitivo `__parse_int`) o `d`.
    ParseIntOr,
    /// Saca un string; empuja el mismo sin espacio en blanco en los extremos. Builtin `trim`.
    Trim,
    /// Saca el separador y el string; empuja un arreglo de strings con los trozos. Builtin
    /// `split`. (El arreglo es un objeto del heap → lo traza el GC.)
    Split,
    /// Saca la subcadena y el string; empuja un `bool`: ¿el string la contiene? Builtin `contains`.
    Contains,
    /// Saca `a`, `de` y el string; empuja el string con **todas** las ocurrencias de `de`
    /// reemplazadas por `a`. Builtin `replace`. (El string nuevo es un objeto del heap en la VM.)
    Replace,
    /// Saca un string; empuja un arreglo `[char]` con sus caracteres. Builtin `chars` (M11.4c-2).
    /// (El arreglo es un objeto del heap → lo traza el GC.)
    Chars,
    /// Saca un char; empuja su code point Unicode como `int`. Builtin `char_code` (M40.3a).
    CharCode,
    /// Saca un string; empuja sus octetos UTF-8 como `bytes`. Builtin `to_bytes` (M16.1b).
    ToBytes,
    /// Saca `bytes`; empuja un `[string]` etiquetado (`["ok", s]`/`["err", msg]`) según decodifique
    /// como UTF-8. Primitivo `__from_utf8`; el prelude → `Result<string, string>` (M16.1b).
    FromUtf8,
    /// M43: hashes de PRODUCCIÓN vía `ring` (bytes -> bytes). Sacan `bytes`, empujan el digest.
    /// M68.2: saca un `int` (n) y empuja `n` octetos criptográficamente seguros (`bytes`;
    /// CSPRNG del SO vía ring). Primitivo `__crypto_random_bytes` (→ `crypto.random_bytes`).
    CryptoRandomBytes,
    Sha256,
    Sha512,
    Sha1,
    /// M43.2: HMAC-SHA256. Saca `msg` y `key` (bytes); empuja la etiqueta de 32 octetos.
    HmacSha256,
    /// M43.3: Ed25519. Los dos primeros empujan `[bytes]` etiquetado (`[]`/`[valor]`; el prelude →
    /// `Option<bytes>`); `verify` empuja un `bool` (total).
    /// M88.1: signals() — el canal de señales del SO (singleton; solo VM).
    Signals,
    Ed25519PublicKey,
    Ed25519Sign,
    Ed25519Verify,
    /// M43.4: ChaCha20-Poly1305 AEAD. Sacan 4 `bytes` (clave, nonce, aad, dato); empujan `[bytes]`
    /// etiquetado (el prelude → `Option<bytes>`).
    ChaChaPolySeal,
    ChaChaPolyOpen,

    /// Diferido TLS (STARTTLS): envuelve un socket TCP plano YA CONECTADO en una sesión TLS de
    /// CLIENTE (el simétrico de `TlsAccept`). Saca host (string) y handle (int); empuja
    /// `["ok", handle]`/`["err", msg]`. Reusa el mismo handle → el I/O existente se desvía solo a
    /// TLS. Primitivo `__tls_upgrade`; `std/net` → `tls_upgrade`. Habilita STARTTLS (Postgres
    /// sslRequest, MySQL caching_sha2 full-path, SMTP…).
    TlsUpgrade,
    /// Diferido JSON-1: el inverso de `char_code`. Saca un `int`; empuja `[char]` de 0/1 elementos
    /// (vacío si no es un code point válido: surrogates D800–DFFF o fuera de rango). Primitivo
    /// `__char_from_code`; el prelude → `char_from_code -> Option<char>`. Habilita los escapes
    /// `\uXXXX` de `std/json`.
    CharFromCode,
    /// M54.1: bits IEEE 754 de un float. Saca un `float`; empuja sus 64 bits como `int` (dos
    /// complemento). Primitivo `__float_bits`; `std/math` → `float_bits`. Lo pide el `double` de
    /// BSON (y serviría a protobuf).
    FloatBits,
    /// M54.1: el inverso — saca un `int` (los 64 bits) y empuja el `float`. Total (cualquier patrón
    /// de bits es un f64 válido, incl. NaN/Inf). Primitivo `__float_from_bits`.
    FloatFromBits,

    // --- SQLite embebido (M53.3, vía rusqlite). El handle es un `int` en el registro de proceso
    // (como archivos/sockets); `close(h)` lo cierra. Resultados como arreglo etiquetado. ---
    /// Saca la ruta (string; `":memory:"` = en memoria); empuja `["ok", handle]`/`["err", msg]`.
    /// Primitivo `__sqlite_open`; el paquete `db/sqlite` → `Result<Conn, string>`.
    SqliteOpen,
    /// Saca params (`[string]`), sql (string) y handle (int); ejecuta una sentencia sin filas y
    /// empuja `["ok", n_afectadas]`/`["err", msg]`. Primitivo `__sqlite_exec`.
    SqliteExec,
    /// Como `SqliteExec` pero para consultas con filas: empuja `["ok", ncols, v0, v1, …]` (las celdas
    /// aplanadas por fila; NULL = "") o `["err", msg]`. Primitivo `__sqlite_query`.
    SqliteQuery,

    // --- I/O binaria (M16.1c). Lecturas → [bytes] etiquetado; escrituras → [string]. ---
    /// Saca la ruta (string); lee el archivo y empuja `[bytes]` (`[b"ok", datos]`/`[b"err", msg]`).
    /// Primitivo `__read_file_bytes`; el prelude → `Result<bytes, string>`.
    ReadFileBytes,
    /// Saca `datos` (bytes) y la ruta (string); escribe el archivo y empuja `[string]` etiquetado.
    /// Primitivo `__write_file_bytes`; el prelude → `Result<int, string>`.
    WriteFileBytes,
    /// Saca un handle (int); lee del socket (no bloqueante en la VM → cede al scheduler como
    /// `SocketRead`) y empuja `[bytes]` etiquetado. Primitivo `__socket_read_bytes`.
    SocketReadBytes,
    /// Saca `datos` (bytes) y el handle (int); escribe en el socket y empuja `[string]` etiquetado.
    /// Primitivo `__socket_write_bytes`; el prelude → `Result<int, string>`.
    SocketWriteBytes,

    // --- Más string (M11.7a) ---
    /// Saca el prefijo y el string; empuja un `bool`: ¿empieza con él? Builtin `starts_with`.
    StartsWith,
    /// Saca el sufijo y el string; empuja un `bool`: ¿termina con él? Builtin `ends_with`.
    EndsWith,
    /// Saca un string; empuja el mismo en MAYÚSCULAS. Builtin `to_upper`. (String nuevo en el heap.)
    ToUpper,
    /// Saca un string; empuja el mismo en minúsculas. Builtin `to_lower`. (String nuevo en el heap.)
    ToLower,
    /// Saca `j`, `i` y el string; empuja la subcadena `[i, j)` por índice de **carácter** (con
    /// *clamp* al rango válido). Builtin `substring`. (String nuevo en el heap.)
    Substring,
    /// Saca `j`, `i` y los `bytes`; empuja la sub-secuencia `[i, j)` por índice de **octeto** (con
    /// *clamp*). Builtin `sub_bytes` (M19.2). Análogo binario de `Substring`; lo usa el HTTP sobre bytes.
    SubBytes,
    /// Saca un `[int]` (objeto del heap); empuja `bytes` con cada elemento truncado a octeto
    /// (`& 255`). Builtin `bytes_of` (M19.3c); dual del indexado, para construir tramas/cabeceras.
    BytesOf,
    /// Saca `n` y el string; empuja el string repetido `n` veces (`n<=0` → `""`). Builtin `repeat`.
    Repeat,
    /// Saca la subcadena y el string; empuja un `[int]` con **0 o 1** elementos: el índice de
    /// **carácter** de la primera ocurrencia, o vacío. Primitivo `__index_of`; el prelude → `Option`.
    IndexOf,
    /// Saca el separador y un arreglo `[string]`; empuja un string con los trozos unidos por el
    /// separador. Builtin `join`. (String nuevo en el heap.)
    Join,

    // --- Más arreglos (M11.7b) ---
    /// Saca un arreglo; **lo muta** quitando el último elemento y empuja un `[T]` con **0 o 1**
    /// elementos (el quitado). Primitivo `__pop`; el prelude → `Option<T>`.
    ArrayPop,
    /// Saca un arreglo; empuja uno nuevo con los elementos en orden inverso. Builtin `reverse`.
    /// (El arreglo es un objeto del heap → lo traza el GC.)
    Reverse,
    /// Saca `x` y un arreglo; empuja un `[int]` con **0 o 1** elementos: el índice de la primera
    /// ocurrencia de `x` (por igualdad estructural), o vacío. Primitivo `__position`; prelude → `Option`.
    Position,

    // --- Mapas Map<K,V> (M13.1) ---
    /// No saca nada; asigna un **mapa vacío** en el heap y lo empuja. Builtin `map_new`.
    MapNew,
    /// Saca valor, clave y mapa; **inserta** (clave→valor) mutando el mapa, y empuja unit.
    /// Builtin `insert`.
    MapInsert,
    /// Saca clave y mapa; empuja un arreglo `[V]` con **0 o 1** elementos (el valor asociado,
    /// o vacío). Primitivo `__map_get`; el prelude lo envuelve en `Option<V>`.
    MapGet,
    /// Saca `default`, clave y mapa; empuja el valor asociado a la clave, o `default` si no está —
    /// **sin alocar** (a diferencia de `MapGet`+`Option`+`unwrap_or`, que aloca el arreglo y el enum).
    /// Primitivo `__get_or`; el prelude expone `get_or(m, k, default) -> V` (P0.2, perf).
    MapGetOr,
    /// Saca `delta`, clave y mapa; hace `m[k] += delta` (o `m[k] = delta` si la clave no está) en un
    /// **único lookup** vía la *entry-API* — a diferencia de `get_or(k,0)+insert(k,...)`, que hashea y
    /// busca dos veces y clona la clave dos veces. Valor int o float (checked como `+`). Builtin
    /// `add_to(m, k, delta) -> unit`; el idioma de conteo/acumulación de servicios (P0.3, perf).
    MapAdd,
    /// Saca clave y mapa; empuja un `bool`: si la clave está presente. Builtin `contains_key`.
    MapContainsKey,
    /// Saca clave y mapa; **quita** la clave (mutando el mapa) y empuja un arreglo `[V]` con **0 o
    /// 1** elementos (el valor que había, o vacío). Primitivo `__map_remove`; el prelude → `Option`.
    MapRemove,
    /// Saca un mapa; empuja un arreglo `[K]` con sus **claves**, **ordenadas** (determinista).
    /// Builtin `keys`.
    MapKeys,
    /// Saca un mapa; empuja un arreglo `[V]` con sus **valores**, en orden de **clave ordenada**
    /// (para que case posición a posición con `keys` y sea determinista). Builtin `values`.
    MapValues,

    // --- Concurrencia: CSP sobre la VM (M12.1) ---
    /// Saca un **valor-función** (`fn() -> T`); crea una **fibra** (green thread) nueva que lo ejecuta y
    /// la encola en el scheduler, y empuja un **`Task<T>`** (M12.3; en M12.1/M12.2 era unit). Si hay un
    /// `scope` activo en la fibra que llama, adscribe la tarea a él. Builtin `spawn`. Solo la VM.
    Spawn,
    /// M98.1: `Spawn` **fire-and-forget** — un `spawn(f);` como sentencia cuyo handle se descarta
    /// (peephole `[Spawn, Pop]` → `[SpawnDiscard, Pop]`). Fuera de un `scope` no aloja entrada en el
    /// almacén de tareas (la fibra corre con `task: None` → nada que retener ni liberar); dentro de un
    /// scope se comporta como `Spawn` (el scope necesita rastrear a la hija y la consume al cerrar).
    /// Empuja `unit` (el `Pop` que sigue lo tira). Solo la VM.
    SpawnDiscard,
    /// Saca un `Task<T>`; **une** la tarea: si terminó, empuja su valor; si falló (panic), re-lanza ese
    /// fallo; si sigue pendiente, **bloquea** la fibra hasta que termine (M12.3). Builtin `join` de 1
    /// argumento (el de 2 args es el `Join` de strings); el compilador elige por aridad. Solo VM.
    TaskJoin,
    /// Saca un `Task<T>`; espera a que la tarea termine y empuja un `[string]`: `[]` si acabó bien,
    /// `[msg]` si falló — el fallo como VALOR, sin re-lanzar (M56.5). Primitivo `__task_failed`; el
    /// prelude lo envuelve en `try_join(t) -> Result<T, string>` (que reusa `join` para el valor). Solo VM.
    TaskFailed,
    /// M97.2: saca una closure `fn()` y la llama en la **MISMA fibra**, con un marcador de
    /// recuperación. Al volver bien empuja `[]`; si el cuerpo falla, se desenrollan los marcos hasta
    /// el marcador y empuja `[msg]` — el fallo como VALOR, sin `spawn` ni cambio de hilo. Primitivo
    /// `__try_call`; el prelude lo envuelve en `try_call(f) -> Result<T, string>`. Los TRES motores
    /// (a diferencia de `TaskFailed`, que es solo-VM porque `spawn` no corre en el intérprete).
    TryCall,
    /// Saca un **arreglo de canales** `[Channel<T>]`; espera a que **alguno** esté listo para recibir (cola
    /// no vacía, emisor bloqueado, o cerrado) y empuja el **índice** (int) del primero listo. Si ninguno
    /// lo está, **bloquea** la fibra hasta que alguno lo esté (M12.4). Builtin `select`. Solo VM.
    Select,
    /// Apila un **marco de scope** (vacío) en la fibra actual: las tareas `spawn`eadas mientras esté
    /// activo quedan adscritas a él (M12.3 structured concurrency). El compilador lo intercala antes de la
    /// llamada al cuerpo de `scope`. No toca la pila de operandos. Solo VM.
    ScopeBegin,
    /// Desapila el marco de scope: **espera a todas** sus tareas (las une una a una, bloqueando si hace
    /// falta) y, al estar todas, propaga el primer fallo o deja el valor del cuerpo en la pila (M12.3). El
    /// compilador lo intercala tras la llamada al cuerpo de `scope`. Solo VM.
    ScopeEnd,
    /// No saca nada; asigna un **canal no acotado** en el heap y empuja su handle. Builtin `channel()`
    /// sin argumentos (la cola crece sin límite). Solo VM.
    ChannelNew,
    /// Saca un `int` (la capacidad ≥ 0); asigna un **canal acotado** a esa capacidad y empuja su handle.
    /// Builtin `channel(n)` (M12.2; `n = 0` rendezvous). El compilador elige entre este y `ChannelNew`
    /// según haya argumento. Solo VM.
    ChannelNewBounded,
    /// Saca valor y canal; **envía** el valor (lo entrega a un receptor bloqueado, lo encola si hay hueco,
    /// o **bloquea** al emisor si la cola está llena → backpressure, M12.2) y empuja unit. Error si el
    /// canal está cerrado. Builtin `send`. Solo VM.
    ChanSend,
    /// Saca un canal; empuja un arreglo `[T]` con **0 o 1** elementos (el valor recibido, o vacío si el
    /// canal está cerrado y vacío). Si está vacío y abierto, **bloquea** la fibra; al recibir despierta a
    /// un emisor bloqueado (M12.2). Primitivo `__recv`; el prelude lo envuelve en `Option<T>`. Solo VM.
    ChanRecv,
    // (cerrar un canal reusa el opcode `Close` de M11.8, ad-hoc polimórfico: handle de archivo o canal.)

    // --- Aserciones (M13.2a) ---
    /// Saca un string (el mensaje) y **aborta** la ejecución con un error de runtime que lo lleva,
    /// en la posición de la llamada. Builtin `panic`; el prelude lo usa para `assert`/`assert_eq`.
    Panic,

    // --- I/O y API de runtime (M11.2) ---
    /// Saca un valor primitivo; lo escribe a **stderr** y empuja unit. Builtin `eprint`.
    EPrint,
    /// Saca un string; empuja un arreglo `[int]` con **0 o 1** elementos: el entero parseado,
    /// o vacío si no parsea. Primitivo `__parse_int`; el prelude lo envuelve en `Option<int>`.
    ParseInt,
    /// Saca un string; empuja un arreglo `[float]` con **0 o 1** elementos: el flotante parseado,
    /// o vacío si no parsea. Primitivo `__parse_float`; el prelude → `Option<float>` (M14, lo pide
    /// el lexer auto-alojado).
    ParseFloat,
    /// No saca nada; lee una línea de **stdin** (sin el `\n`) y empuja un `[string]` con **0 o
    /// 1** elementos: vacío en EOF. Primitivo `__read_line`; el prelude lo envuelve en `Option`.
    ReadLine,
    /// Saca un string (el nombre); empuja un `[string]` con **0 o 1** elementos: el valor de la
    /// variable de entorno, o vacío si no existe. Primitivo `__env`; el prelude → `Option<string>`.
    Env,
    /// No saca nada; empuja un `[string]` con los **argumentos del programa** (sin el binario ni
    /// las flags de raylang). Builtin `args`. Los args vienen de un almacén de proceso.
    Args,
    /// Saca un string (la ruta); empuja un `[string]` **etiquetado**: `["ok", contenido]` si se
    /// pudo leer, `["err", mensaje]` si no. Primitivo `__read_file`; el prelude → `Result`.
    ReadFile,
    /// Saca el contenido y la ruta; escribe el archivo y empuja un `[string]` etiquetado: `["ok"]`
    /// si se pudo, `["err", mensaje]` si no. Primitivo `__write_file`; el prelude → `Result`.
    WriteFile,
    /// Saca una ruta; empuja un `bool`: ¿existe esa ruta? Builtin `exists` (total, no falla).
    Exists,
    /// Saca el contenido y la ruta; **añade** al final del archivo (lo crea si no existe) y empuja un
    /// `[string]` etiquetado `["ok"]`/`["err", mensaje]`. Primitivo `__append_file`; el prelude → `Result`.
    AppendFile,
    /// Saca una ruta; **borra** el archivo y empuja un `[string]` etiquetado `["ok"]`/`["err", msg]`.
    /// Primitivo `__remove_file` (M11.7c); el prelude → `Result<int,string>`.
    RemoveFile,
    /// Saca una ruta; empuja un `[string]` etiquetado: `["ok", n0, n1, …]` con los nombres del
    /// directorio, o `["err", msg]`. Primitivo `__list_dir` (M11.7c); el prelude → `Result<[string],…>`.
    ListDir,
    /// M67: operación de fs etiquetada — saca `op.argc()` strings (rutas) y empuja el `[string]`
    /// etiquetado `["ok"(, dato)]`/`["err", msg]`. **Un opcode parametrizado** (como `MathF`) para
    /// las cinco: mkdir/remove_dir/file_size/rename/copy_file; delega en `builtins::fs_tagged`.
    FsTagged(FsOp),
    /// M67: test total de fs — saca una ruta y empuja un `bool` (is_dir/is_file, como `Exists`).
    FsTest(FsTest),
    /// M67: saca los datos (`bytes`) y la ruta; **añade** al final del archivo y empuja el
    /// `[string]` etiquetado. Primitivo `__append_file_bytes` (gemelo binario de `AppendFile`).
    AppendFileBytes,

    // --- I/O con buffering: handles de archivo (M11.8) ---
    /// Saca el modo y la ruta; abre el archivo y empuja un `[string]` etiquetado `["ok", handle]`
    /// (handle como decimal) o `["err", msg]`. Primitivo `__open`; el prelude → `Result<int,string>`.
    Open,
    /// Saca un handle (int); lee la siguiente línea (sin `\n`) y empuja un `[string]` con **0 o 1**
    /// elementos (vacío en EOF/handle inválido). Primitivo `__read_line_handle`; prelude → `Option`.
    ReadLineHandle,
    /// Saca el contenido y un handle (int); escribe y empuja un `[string]` etiquetado
    /// `["ok"]`/`["err", msg]`. Primitivo `__write_handle`; el prelude → `Result<int,string>`.
    WriteHandle,
    /// M107.1 (std/io): escribe el string a stdout SIN salto de línea → `["ok"]`/`["err", msg]`.
    /// Primitivo `__stdout_write`; `std/io` → `Result<int,string>`.
    StdoutWrite,
    /// M107.1 (std/io): como `StdoutWrite`, a stderr. Primitivo `__stderr_write`.
    StderrWrite,
    /// M107.1 (std/io): escribe bytes crudos a stdout (secuencias de escape, salida binaria) →
    /// `["ok"]`/`["err", msg]`. Primitivo `__stdout_write_bytes`.
    StdoutWriteBytes,
    /// M107.1 (std/io): vacía el buffer de stdout → `["ok"]`/`["err", msg]`. Primitivo
    /// `__stdout_flush`; sin él, un `write` sin salto puede no verse hasta el fin del proceso
    /// (stdout va line-buffered).
    StdoutFlush,
    /// M107.2 (std/io): lee hasta `max` octetos de stdin → `[datos]` o `[]` (EOF). En la VM
    /// APARCA la fibra si no hay nada que leer (poller sobre el fd 0, patrón `SocketRead`).
    /// Primitivo `__stdin_read`; `std/io` → `Option<bytes>`.
    StdinRead,
    /// M107.2 (std/io): como `StdinRead` con plazo → `[b"data", datos]`/`[b"eof"]`/`[b"timeout"]`.
    /// Primitivo `__stdin_read_timeout`; el deadline reusa la maquinaria de M56.4 con el
    /// pseudo-handle 0 de stdin.
    StdinReadTimeout,
    /// M107.3 (std/term): ¿el fd (0/1/2) es una terminal? Primitivo `__term_is_tty`.
    TermIsTty,
    /// M107.3 (std/term): tamaño del terminal → `[cols, rows]` o `[]`. Primitivo `__term_size`.
    TermSize,
    /// M107.3 (std/term): entra al modo crudo (termios) → `["ok"]`/`["err", msg]`; la restauración
    /// queda registrada con `atexit`. Primitivo `__term_raw_on`.
    TermRawOn,
    /// M107.3 (std/term): restaura el terminal → `["ok"]`/`["err", msg]`. Primitivo `__term_raw_off`.
    TermRawOff,
    /// Saca un handle (int); cierra el archivo (lo quita del registro) y empuja `0`. Builtin `close`.
    Close,

    // --- Matemáticas (M15.1a) ---
    /// Saca un `float`; empuja `f(x)` donde `f` es la función `MathFn`. Builtins `sqrt`/`sin`/`cos`/
    /// `tan`/`ln`/`log10`/`exp`/`floor`/`ceil`/`round`. **Un solo opcode parametrizado** (en vez de
    /// uno por función) para no inflar el `match` de la VM; delega en `builtins::apply_mathf`.
    MathF(MathFn),
    /// Saca el exponente y la base (`float`, `float`); empuja `base.powf(exp)`. Primitivo `__pow`
    /// (envuelto por `math.pow`). M49.1b: `abs`/`min`/`max`/`pi`/`e` dejaron de tener opcode (son
    /// funciones puras en `std/math`).
    Pow,
    /// M65.2: saca `x` y `y` (`float`, `float`); empuja `atan2(y, x)` — el ángulo de (x, y) en
    /// (-π, π]. Binaria como `Pow` (no cabe en `MathF`). Primitivo `__atan2` (→ `math.atan2`).
    Atan2,

    // --- Reloj y aleatoriedad (M15.1b) ---
    /// No saca nada; empuja los milisegundos desde la época Unix (`int`). Builtin `now`.
    Now,
    /// No saca nada; empuja los milisegundos de un reloj monótono (`int`). Builtin `monotonic`.
    Monotonic,
    /// No saca nada; empuja los nanosegundos del mismo reloj monótono (`int`, misma ancla que
    /// `Monotonic`). Builtin `monotonic_nanos`.
    MonotonicNanos,
    /// Saca `ms` (int); duerme el hilo ese tiempo y empuja unit. Builtin `sleep`.
    Sleep,
    /// No saca nada; empuja un `float` en `[0, 1)`. Builtin `random`.
    Random,
    /// Saca `n` (int); empuja un entero en `[0, n)` (`n<=0` → `0`). Builtin `random_int`.
    RandomInt,
    /// M68.1: saca un `int` (la semilla) y fija el estado del PRNG; empuja unit. Primitivo
    /// `__random_seed` (→ `random.seed`): misma semilla, misma secuencia (reproducibilidad).
    RandomSeed,

    // --- Cliente TCP (M15.2) ---
    /// Saca `port` (int) y `host` (string); conecta y empuja un `[string]` etiquetado
    /// (`["ok", handle]`/`["err", msg]`). Primitivo `__tcp_connect`; el prelude → `Result<int,string>`.
    TcpConnect,
    /// Saca `port` (int) y `host` (string); abre una conexión **TLS** (rustls) y empuja un `[string]`
    /// etiquetado. Primitivo `__tls_connect` (M19.4a); el prelude → `Result<int,string>`. El handle se
    /// lee/escribe con los mismos `socket_*` (desvían a TLS) y se cierra con `close`.
    TlsConnect,
    /// Como `TlsConnect` pero ofreciendo **ALPN `h2`** (HTTP/2) y exigiendo que el servidor lo negocie
    /// (M31.2a). Primitivo `__tls_connect_h2`; el prelude → `Result<int,string>`.
    TlsConnectH2,
    /// Saca la clave, el cert (strings) y el handle (int); envuelve un socket TCP aceptado en una sesión
    /// **TLS de servidor** y empuja un `[string]` etiquetado. Primitivo `__tls_accept` (M19.4b); prelude
    /// → `Result<int,string>`. Habilita servir `https`/`wss`.
    TlsAccept,
    /// Saca un handle (int); hace una lectura del socket y empuja un `[string]` etiquetado
    /// (`["ok", datos]`/`["err", msg]`). Primitivo `__socket_read`; el prelude → `Result<string,string>`.
    SocketRead,
    /// Saca `s` (string) y el handle (int); escribe y empuja un `[string]` etiquetado
    /// (`["ok", ""]`/`["err", msg]`). Primitivo `__socket_write`; el prelude → `Result<int,string>`.
    SocketWrite,

    // --- Servidor TCP (M15.3) ---
    /// Saca `port` (int) y `host` (string); hace bind+listen y empuja un `[string]` etiquetado
    /// (`["ok", handle]`/`["err", msg]`). Primitivo `__tcp_listen`; el prelude → `Result<int,string>`.
    TcpListen,
    /// Saca un handle de escucha (int); bloquea hasta una conexión y empuja un `[string]` etiquetado
    /// (`["ok", handle]`/`["err", msg]`). Primitivo `__tcp_accept`; el prelude → `Result<int,string>`.
    TcpAccept,
    /// Saca un handle (int); empuja su puerto local (`int`, `0` si no es un socket). Builtin `local_port`.
    LocalPort,
    /// Saca `ms` (int) y el handle (int); fija (ms > 0) o quita (ms <= 0) el timeout de lectura del
    /// socket y empuja `unit`. Total. Primitivo `__socket_set_read_timeout` (M56.4); envoltorio
    /// `net.set_read_timeout` en std/net.
    SocketSetReadTimeout,
    /// Saca `port` (int) y `host` (string); enlaza un socket UDP y empuja un `[string]` etiquetado.
    /// Primitivo `__udp_bind`; la lib udp.ray → `Result<int,string>` (M20.8).
    UdpBind,
    /// Saca `datos` (bytes), `port` (int), `host` (string) y el handle (int); envía un datagrama y
    /// empuja un `[string]` etiquetado con los octetos enviados. Primitivo `__udp_send_to` (M20.8).
    UdpSendTo,
    /// Saca un handle (int); bloquea hasta recibir un datagrama y empuja un `[bytes]` etiquetado
    /// (`[b"ok", host, puerto, datos]`/`[b"err", msg]`). Primitivo `__udp_recv_from` (M20.8).
    UdpRecvFrom,

    // --- Procesos del SO (M100, IDEAS §53.8) ---
    /// Saca (en orden inverso al empuje) merge_output (bool), max_output (int), timeout_ms (int),
    /// has_stdin (bool), stdin (bytes), env_clear (bool), env ([string], pares clave/valor
    /// aplanados), dir (string, "" = heredado), args ([string]) y program (string); ejecuta el
    /// proceso y empuja un `[bytes]` etiquetado (ver `builtins::run_encoded`):
    /// `[b"ok", b"code"|b"signal", valor, timed_out, truncated, stdout, stderr]` / `[b"err", msg]`.
    /// Primitivo `__run`; la ergonomía (`Cmd`/`Output`/`Exit`) vive en `std/process` (fase 1c).
    Run,
    /// M100 v2: saca (en orden inverso) merge_output (bool), has_stdin (bool), stdin (bytes),
    /// env_clear (bool), env ([string]), dir (string), args ([string]) y program (string); lanza el
    /// hijo en modo STREAMING (pipes registrados como handles no-bloqueantes) y empuja
    /// `[b"ok", h_child, h_out, h_err]` (`h_err = -1` con merge) / `[b"err", msg]`.
    /// Primitivo `__proc_spawn`; las bombas viven en `std/process` (IDEAS §53.9).
    ProcSpawn,
    /// Saca el handle del hijo (int); `waitpid(WNOHANG)` y empuja `[b"running"]`,
    /// `[b"code"|b"signal", n]` (cosechado; el handle se elimina) o `[b"err", msg]`.
    /// Primitivo `__proc_try_wait`.
    ProcTryWait,
    /// Saca `force` (bool) y el handle del hijo (int); SIGTERM/SIGKILL al GRUPO y empuja unit.
    /// Total e idempotente (handle cosechado = no-op). Primitivo `__proc_kill`.
    ProcKill,

    // --- Structs (M3.2) ---
    /// Construye el struct definido en `structs[idx]`: saca tantos valores como
    /// campos tenga (estaban en orden de declaración) y empuja el struct.
    MakeStruct(usize),
    /// Saca un struct; empuja el valor de su campo (buscado por nombre).
    GetField(String),
    /// Saca valor y struct; asigna `struct.campo = valor` (por nombre).
    SetField(String),

    // --- Enums (M5) ---
    /// Construye una variante de enum: `(enum_id, variant_id)` indexan
    /// `program.enums`. Saca de la pila tantos valores como aridad tenga la variante
    /// (el payload, en orden) y empuja el valor de enum.
    MakeEnum(usize, usize),

    // --- match (M5.3) ---
    /// Saca un enum de la pila y empuja `Bool(tag == arg)`: ¿es esta la variante?
    /// La cadena de brazos compara con esto y salta con `JumpIfFalse`.
    EnumTagEq(usize),
    /// Saca un enum y empuja el valor en la posición `i` de su payload (para ligar
    /// los sub-patrones de un brazo).
    GetEnumField(usize),
    /// El `match` no casó ningún brazo: error de ejecución. Es un **trap defensivo**:
    /// el checker garantiza exhaustividad, así que es inalcanzable en programas
    /// válidos.
    MatchFail,

    // --- MM4: kernel de bucle con deopt ---
    /// El PRODUCTO PUNTO `for k in lo..hi { acc = acc + a[k] * b[k]; }` como un solo opcode,
    /// emitido justo ANTES de la guarda del bucle (que sigue emitido completo detrás). En
    /// runtime, si los locales tienen la forma rápida (acc Float, a/b FloatArray, k/end Int,
    /// `0 <= k` y `end <= len` de ambos), ejecuta el bucle entero en Rust —misma secuencia
    /// mul→add, mismo orden: resultado bit a bit idéntico— , deja `k = end` y salta a `exit`
    /// (justo después del bucle). Si NO (arrays degradados/de ints, locales boxeados, rango
    /// que se saldría), NO hace nada y cae al bucle interpretado de siempre: la semántica de
    /// errores (índice fuera de rango, overflow de int) la da el bytecode normal — deopt, no
    /// una segunda implementación.
    DotRange { acc: usize, a: usize, b: usize, k: usize, end: usize, exit: usize },

    // --- R7: std/regex sobre el crate `regex` (feature `regex`) ---
    /// El CUERPO completo de una de las 7 funciones internas `run_*` de std/regex: lee sus
    /// argumentos de los locales del marco (slot 0 = el `Prog`, cuyo campo `pat` retiene el
    /// patrón FUENTE ya validado; slot 1 = el texto; slot 2 = el reemplazo en `replace_all`),
    /// ejecuta el crate `regex` vía ray-runtime (la MISMA traducción de dialecto que el binario
    /// nativo, R5) y empuja el resultado. Le sigue siempre un `Return`. `opt` trae los índices
    /// `(enum_id, tag_some, tag_none)` de `Option`, resueltos al compilar, para construir los
    /// retornos `Option<...>` sin buscar por nombre en caliente.
    RegexNative { f: RegexNativeFn, opt: (usize, usize, usize) },

    /// Termina la ejecución del chunk; el valor de retorno es la cima de la pila.
    Return,
}

/// R7: cuál de las 7 `run_*` de std/regex despacha una `RegexNative` (misma lista que
/// intercepta el transpilador nativo desde R5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RegexNativeFn {
    Full,
    Search,
    Find,
    FindAll,
    ReplaceAll,
    Captures,
    CapturesStr,
}

/// A4: el comparador de una `CmpJump` (los seis de la familia de comparación).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmpOp {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
}

/// Un bloque de bytecode compilado: las instrucciones, la tabla de constantes, y
/// la posición fuente de cada instrucción (para errores con ubicación).
#[derive(Debug, Default, Clone)]
pub struct Chunk {
    pub code: Vec<OpCode>,
    pub constants: Vec<Value>,
    /// `lines[i]` es la `(línea, columna)` de la instrucción `code[i]`. Paralela a
    /// `code`. Es el equivalente a la "line table" de un compilador real: el
    /// bytecode pierde el texto fuente, pero conservamos de dónde vino cada
    /// instrucción para poder reportar errores de ejecución con su ubicación.
    pub lines: Vec<(usize, usize)>,
}

impl Chunk {
    pub fn new() -> Self {
        Chunk::default()
    }

    /// Emite una instrucción con su posición fuente. Devuelve su índice (útil para
    /// el parcheo de saltos en M2.2).
    pub fn emit(&mut self, op: OpCode, line: usize, col: usize) -> usize {
        self.code.push(op);
        self.lines.push((line, col));
        self.code.len() - 1
    }

    /// Registra una constante y devuelve su índice. M29.3: **dedup** — si ya hay una constante
    /// idéntica en el pool, reutiliza su índice en vez de añadir un duplicado. Los literales se
    /// repiten muchísimo (los enteros pequeños `0`/`1`/`2`, strings, nombres de campo), así que el
    /// pool encoge notablemente. La búsqueda es lineal, pero solo ocurre en **compilación** (una vez).
    pub fn add_constant(&mut self, value: Value) -> usize {
        if let Some(i) = self.constants.iter().position(|c| c == &value) {
            return i;
        }
        self.constants.push(value);
        self.constants.len() - 1
    }

    /// Desensambla el chunk a texto legible. Herramienta para *ver* el bytecode
    /// mientras aprendemos y depuramos.
    pub fn disassemble(&self, name: &str) -> String {
        let mut out = format!("== {} ==\n", name);
        for (i, op) in self.code.iter().enumerate() {
            let (line, col) = self.lines[i];
            // Para Constant mostramos también el valor al que apunta el índice.
            let detail = match op {
                OpCode::Constant(idx) => format!("Constant   {} -> {}", idx, self.constants[*idx]),
                other => format!("{:?}", other),
            };
            out.push_str(&format!("{:04}  {:>3}:{:<3}  {}\n", i, line, col, detail));
        }
        out
    }
}

/// De dónde sale la celda de un upvalue al construir una closure (M4.2). La
/// resolución la hace el compilador al estilo clox.
#[derive(Debug, Clone, PartialEq)]
pub enum UpvalueSource {
    /// Una variable **local** del marco que crea la closure, en este slot.
    Local(usize),
    /// Un **upvalue** del marco que crea la closure (captura transitiva), en este
    /// índice de su propio arreglo de upvalues.
    Upvalue(usize),
}

/// Un upvalue de una función: su nombre (para el intérprete/depuración) y de dónde
/// tomar su celda en el marco que la cierra.
#[derive(Debug, Clone, PartialEq)]
pub struct UpvalueRef {
    pub name: String,
    pub source: UpvalueSource,
}

/// Una función compilada a bytecode.
#[derive(Debug)]
pub struct CompiledFn {
    pub name: String,
    pub arity: usize,
    /// Tamaño del arreglo de slots locales que necesita un marco de esta función.
    pub num_locals: usize,
    /// `captured[s] == true` si el slot local `s` es capturado por alguna closure
    /// anidada y, por tanto, debe **boxearse** (vivir en una celda) (M4.2).
    pub captured: Vec<bool>,
    /// V9: ¿algún slot capturado? Precomputado al compilar — el camino de llamada de la VM
    /// decide con UN bool (lo común: ninguno → locales y argumentos van directos, sin mirar
    /// `captured` slot a slot).
    pub has_captured: bool,
    /// Los upvalues de esta función: cómo construir su entorno al crearla (M4.2).
    pub upvalues: Vec<UpvalueRef>,
    pub chunk: Chunk,
}

/// La definición de un struct, compilada: su nombre y sus campos en orden.
#[derive(Debug)]
pub struct CompiledStruct {
    pub name: String,
    pub fields: Vec<String>,
}

/// La definición de un enum, compilada: su nombre y sus variantes **en orden**. El
/// índice de una variante en `variants` es su *tag* (lo usará el `match` de M5.3).
#[derive(Debug)]
pub struct CompiledEnum {
    pub name: String,
    pub variants: Vec<CompiledVariant>,
}

/// Una variante compilada: su nombre y su aridad (cuántos valores de payload lleva).
#[derive(Debug)]
pub struct CompiledVariant {
    pub name: String,
    pub arity: usize,
}

/// Un programa compilado: sus structs, enums, funciones (indexadas) y el índice de
/// `main`.
#[derive(Debug)]
pub struct CompiledProgram {
    pub functions: Vec<CompiledFn>,
    pub structs: Vec<CompiledStruct>,
    pub enums: Vec<CompiledEnum>,
    pub main: usize,
    /// Descriptores de las funciones externas (M41, FFI), indexados por el `CallExtern(idx, _)`.
    pub externs: Vec<crate::ffi::ExternDesc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Value;

    /// M29.3: `add_constant` deduplica — una constante idéntica reutiliza su índice.
    #[test]
    fn add_constant_deduplicates() {
        let mut c = Chunk::new();
        let i0 = c.add_constant(Value::Int(7));
        let i1 = c.add_constant(Value::Int(7)); // duplicado → mismo índice
        let i2 = c.add_constant(Value::Int(9)); // distinto → índice nuevo
        let i3 = c.add_constant(Value::Str("a".into()));
        let i4 = c.add_constant(Value::Str("a".into())); // duplicado
        assert_eq!(i0, i1);
        assert_ne!(i0, i2);
        assert_eq!(i3, i4);
        assert_eq!(c.constants.len(), 3); // 7, 9, "a" — sin duplicados
    }
}
