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
    Sha256,
    Sha512,
    Sha1,
    /// M43.2: HMAC-SHA256. Saca `msg` y `key` (bytes); empuja la etiqueta de 32 octetos.
    HmacSha256,
    /// M43.3: Ed25519. Los dos primeros empujan `[bytes]` etiquetado (`[]`/`[valor]`; el prelude →
    /// `Option<bytes>`); `verify` empuja un `bool` (total).
    Ed25519PublicKey,
    Ed25519Sign,
    Ed25519Verify,
    /// M43.4: ChaCha20-Poly1305 AEAD. Sacan 4 `bytes` (clave, nonce, aad, dato); empujan `[bytes]`
    /// etiquetado (el prelude → `Option<bytes>`).
    ChaChaPolySeal,
    ChaChaPolyOpen,

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
    /// Saca un `Task<T>`; **une** la tarea: si terminó, empuja su valor; si falló (panic), re-lanza ese
    /// fallo; si sigue pendiente, **bloquea** la fibra hasta que termine (M12.3). Builtin `join` de 1
    /// argumento (el de 2 args es el `Join` de strings); el compilador elige por aridad. Solo VM.
    TaskJoin,
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

    // --- Reloj y aleatoriedad (M15.1b) ---
    /// No saca nada; empuja los milisegundos desde la época Unix (`int`). Builtin `now`.
    Now,
    /// No saca nada; empuja los milisegundos de un reloj monótono (`int`). Builtin `monotonic`.
    Monotonic,
    /// Saca `ms` (int); duerme el hilo ese tiempo y empuja unit. Builtin `sleep`.
    Sleep,
    /// No saca nada; empuja un `float` en `[0, 1)`. Builtin `random`.
    Random,
    /// Saca `n` (int); empuja un entero en `[0, n)` (`n<=0` → `0`). Builtin `random_int`.
    RandomInt,

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
    /// Saca `port` (int) y `host` (string); enlaza un socket UDP y empuja un `[string]` etiquetado.
    /// Primitivo `__udp_bind`; la lib udp.ray → `Result<int,string>` (M20.8).
    UdpBind,
    /// Saca `datos` (bytes), `port` (int), `host` (string) y el handle (int); envía un datagrama y
    /// empuja un `[string]` etiquetado con los octetos enviados. Primitivo `__udp_send_to` (M20.8).
    UdpSendTo,
    /// Saca un handle (int); bloquea hasta recibir un datagrama y empuja un `[bytes]` etiquetado
    /// (`[b"ok", host, puerto, datos]`/`[b"err", msg]`). Primitivo `__udp_recv_from` (M20.8).
    UdpRecvFrom,

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

    /// Termina la ejecución del chunk; el valor de retorno es la cima de la pila.
    Return,
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
    fn add_constant_deduplica() {
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
