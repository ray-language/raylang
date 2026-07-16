# El transpilador nativo (`ray build --native`) — estado actual y rumbo de las dependencias externas

> Documento de referencia del **arco P2.b** (jul 2026). Complementa a `PERFORMANCE.md` (crónica
> fase a fase, con mediciones) fijando: (1) una foto de la implementación actual, (2) el análisis
> de la pregunta de diseño sobre dependencias de Cargo en el binario transpilado, (3) el **diseño
> acordado: el crate `ray-runtime`** (§4), y (4) el plan por pasos para retomarlo en sesiones
> futuras (§5).

## 1. Qué es

El transpilador (`src/transpile.rs`) es el **tercer backend** de raylang: traduce el programa
**ya chequeado** a código Rust y lo compila a un **binario nativo**. El modelo es
**dev = VM / deploy = nativo** (como el ciclo dev/release de Rust): la VM da el arranque
instantáneo y el REPL; el binario nativo da la velocidad (24–61× sobre la VM; fib gana a node
5.4×). El contrato de corrección es el mismo del proyecto: **salida byte-idéntica a la VM**,
verificada con oráculos, nunca asumida.

```
fuente → [loader] → [checker] → AST bajado → [transpile.rs] → .rs → rustc → binario
```

## 2. Implementación actual

### 2.1 El CLI (`src/cli.rs::build_native`, ~línea 416)

- `ray build --native [-o salida] [--release]`. Re-carga y re-chequea la entrada, llama a
  `transpile::transpile(&program)`, escribe el Rust a un temporal
  (`$TMP/ray_native_<stem>_<pid>.rs`, PID para builds concurrentes) y lo compila con **`rustc`
  directamente** (sin Cargo).
- **Tiers medidos** (PERFORMANCE.md fase 33): por defecto `-O` (opt-level=2, ~0.2 s, portable);
  `--release` = `opt-level=3 + lto=fat + codegen-units=1 + target-cpu=native` (~10 % extra solo
  en cargas de asignación/Map, 9× de tiempo de compilación, binario no portable). PGO descartado
  por medición.

### 2.2 El transpilador (`src/transpile.rs`, ~4.300 líneas)

**Modelo de valores** (espejo del intérprete): escalares *unboxed* (`i64`/`f64`/`bool`/`char`),
`string → Rc<str>`, `bytes → Rc<[u8]>`, arreglos → `Rc<RefCell<Vec<T>>>`, `Map` →
`Rc<RefCell<HashMap>>`, structs → `Rc<RefCell<S>>`, enums → `Rc<E>`, `Option`/`Result` nativos,
closures → `Rc<dyn Fn>`, `ptr` → `i64`. La semántica de valor de raylang sobre la de movimiento
de Rust se resuelve **clonando al leer** (bump de refcount, O(1)). Traits por **erasure** (los
métodos ya llegan bajados como funciones `Tipo#metodo` → `mangle` los hace identificadores Rust);
bounds `T: Eq`/`T: Show` por **paso de diccionarios**; `print`/`to_string` vía un trait propio
`RayShow` (con impls para `Rc<RefCell<…>>`, Map, tuplas).

**El mecanismo clave para lo que sigue — emisión bajo demanda del runtime**: el struct
`Transpiler` lleva flags `needs_handles` / `needs_concurrency` / `needs_signals` /
`needs_time_rng` / `needs_net` que se **activan al toparse el uso** (un `open`, un `spawn`, un
`tcp_connect`…) y gatean la emisión de cada bloque de runtime auxiliar (registro de handles,
canales con hilos de SO, handler de señales, `Instant` global, helpers de sockets). Un programa
que no usa un subsistema **no arrastra su código**.

**Honestidad sobre el alcance**: todo nodo fuera del subconjunto devuelve `Err` claro. Desde la
fase 45, una función *no-main* que no transpila se emite como **stub que panica** con su firma
declarada (`panic!("'f' no está soportada en el binario nativo…")`): el binario compila, y si el
flujo real nunca alcanza el código no soportado, corre idéntico a la VM (fue el mayor desbloqueo:
web-demos 24 → 38 byte-idénticos).

**Otras piezas estructurales**:

- **Prelude en banda de líneas disjunta** (`prelude::LINE_BASE = 1e9`): las lowerings del checker
  indexan por `(línea, col)` sobre el programa fusionado; el prelude desplazado no colisiona con
  módulos de usuario (fix de la corrupción de `string#hash`/poly1305, fase 42).
- **Closures con estado** (B1): análisis de "celdas" — una variable mutable capturada por un
  closure baja a `Rc<RefCell<T>>` con lectura `.borrow().clone()` y escritura por temporal.
- **Iteradores** (B2): `for x in it` baja a un `loop`/`match` sobre `next(it)`; el protocolo
  `Iter` del prelude (`map`/`filter`/`enumerate`/`zip`…) se emite como código normal (la
  inferencia genérica `unify`/`subst_type` recorre Struct/Enum/Tuple con argumentos).
- **FFI** (fase 49): `extern "lib" { … }` emite el bloque `#[link(name = "lib")] extern "C"` +
  un wrapper que marshala (string→`CString`, retorno `int` = **`c_int` de 32 bits con extensión
  de signo** — el gotcha de ABI que convertía el EOF de `fgetc` en bucle infinito).

### 2.3 Cobertura y verificación

- **Lenguaje completo del corpus**: 37/37 ejemplos deterministas byte-idénticos (escalares,
  strings, arreglos, Map, structs/enums/match, Option/Result/`?` + From, closures, genéricos,
  traits/`dyn`, tuplas, operator overloading, `@derive`, u8/u32/u64, multi-módulo/cápsulas → un
  binario).
- **I/O completa de `std/fs`** (texto/binaria/handles/dirs/stdin/env/args), `std/time`,
  `std/random`, `std/math`, sockets **TCP y UDP** de `std/net`.
- **Concurrencia CSP con hilos de SO reales**: spawn/canales (acotados)/`Task`/`join`/`scope`/
  `select`/`signals` (canales de tipos inmutables: int/float/bool/char/string/bytes).
- **FFI a C** (libm/libc verificados byte-idénticos).
- **38 de ~46 web-demos deterministas byte-idénticos**; 605 tests lib + 19 de integración
  `build_native` en `tests/cli_cli.rs`.

### 2.4 Fuera del alcance hoy, y POR QUÉ

Dos categorías muy distintas:

1. **Diferidos de ingeniería** (proyectos con tradeoffs, no huecos): canales de tipos mutables
   (struct/arreglo/Map cruzando hilos), `spawn` de función nombrada, cancelación M12.5,
   `try_join`, `print` de `ptr`, structs/arreglos por FFI. De la fase 45 quedan además tres
   web-demos caídos por bugs concretos del transpilador (no de crates): `metrics_server_demo`
   (canal de struct no-Send), `udp_yield_demo` (`spawn` de fn nombrada), `framework`/`webserver`
   (captura `t` en spawn + `RayShow` de un `[fn]`).
2. **El techo de los crates** — la razón de este documento: **TLS (`rustls`), criptografía de
   producción (`ring`) y bases de datos (`rusqlite`, y los clientes que cuelgan de TLS)**. La VM
   los tiene (M43, M53.3) porque el binario `ray` es un proyecto Cargo. El transpilado **no**,
   y la causa raíz no está en el transpilador: es que `build_native` compila con **`rustc`
   pelado**, y un `rustc` suelto no puede resolver dependencias de crates.io (eso lo hace Cargo).
   Los `__tls_*`, `ring::*`, `sqlite_*` hoy quedan como stubs que panican si se alcanzan.

## 3. La pregunta de diseño: dependencias externas en el binario transpilado

**Planteamiento** (Roberto, 14 jul 2026): no hay problema en que el binario transpilado lleve
dependencias de Cargo y así quitar las limitaciones. Dos opciones sobre la mesa:

- **Opción 1 — flags sustractivos**: `build --native` enlaza todo por defecto, con flags para
  desactivar dependencias selectivamente.
- **Opción 2 — transpilador inteligente**: detectar y añadir una dependencia externa **solo
  cuando el programa la necesita**.

### 3.1 El prerrequisito común

Ambas opciones exigen lo mismo primero: cuando haya al menos un crate, `build_native` debe dejar
de invocar `rustc` pelado y **generar un mini-proyecto Cargo** (un `Cargo.toml` + `src/main.rs`
con el Rust emitido) y compilar con `cargo build`. Es un cambio de *fontanería del CLI*, no del
transpilador. Consecuencias a asumir: primera compilación más lenta (descarga/compila los crates;
mitigable con `CARGO_TARGET_DIR` compartido como caché), y necesidad de red la primera vez (o
vendoring).

### 3.2 Análisis comparativo

| | Opción 1 (flags sustractivos) | Opción 2 (bajo demanda) |
|---|---|---|
| Default | Todo enlazado siempre | Solo lo que el programa usa |
| «Hola mundo» | Arrastra rustls+ring+rusqlite: build lento, binario gordo | **Cero crates → sigue el camino `rustc` pelado actual** (~0.2 s) |
| Carga cognitiva | El usuario debe saber qué apagar | Ninguna |
| Encaje con el código | Ninguno | **Es el diseño que ya existe**: los flags `needs_*` |
| Riesgo | Regresión de DX para el 90 % de programas | La detección debe ser precisa (pero el transpilador ya visita cada llamada) |

El argumento decisivo es el último de la columna derecha: el transpilador **ya funciona bajo
demanda** para su propio runtime (§2.2). `needs_net` no enlaza un crate pero es exactamente la
misma idea: *detectar el uso → emitir/enlazar solo entonces*. Extenderlo a crates es la
continuación natural del diseño, no un mecanismo nuevo. La opción 1 en solitario, además,
contradice la prioridad DX del proyecto: penaliza por defecto a los programas que no tocan esos
subsistemas.

### 3.3 Decisión (acordada con Roberto, 15 jul 2026): **bajo demanda (núcleo) + camino rápido preservado + flags como override**

1. **Detección bajo demanda** (el núcleo, opción 2): nuevos flags `needs_rt_*` que se activan al
   interceptar la feature correspondiente. Se materializan como **features de Cargo de un único
   crate `ray-runtime`** (el diseño detallado en §4) — NO como una lista de crates que el
   transpilador conoce por su cuenta.
2. **Camino rápido de cero-deps intacto**: si ningún `needs_rt_*` se activa —el caso de TODO
   lo cubierto hasta hoy—, se sigue compilando con `rustc` pelado. Cero regresión de velocidad
   de build ni de requisitos (sin red, sin Cargo) para el caso común.
3. **Flags encima, como escape hatch** (lo útil de la opción 1): p. ej. `--without tls` fuerza
   el comportamiento actual (stub que panica) aunque se detecte el uso — para builds herméticos,
   cross-compile complicado o depuración. Se implementan apagando el mismo flag `needs_rt_*`,
   así que cuestan casi nada una vez existe el punto 1.

**Nota sobre los stubs**: la detección debe distinguir *uso alcanzable* de *referencia muerta*.
Hoy un `import http` arrastra `tls_connect` como stub aunque el programa hable HTTP plano
(fase 45). La regla: un uso que hoy acaba en **stub** activa el `needs_rt_*` (el crate hace que
el stub se vuelva implementación real); `--without <dep>` lo devuelve a stub. Así el mismo
programa compila siempre, con o sin la dep — solo cambia si el camino TLS funciona o panica.

## 4. El diseño: el crate `ray-runtime`

### 4.1 La motivación — dos debilidades de la versión ingenua

La versión ingenua del punto 3.3 (el `Cargo.toml` generado lista `rustls`/`ring`/`rusqlite`
directamente y el transpilador emite el glue inline en el `.rs`) tiene dos problemas serios:

1. **Mantenimiento del glue inline**. El runtime que hoy se emite inline es std puro (sockets,
   hilos, HashMap) y cabe en strings. Pero un handshake TLS con rustls o el manejo de statements
   de rusqlite son *cientos* de líneas no triviales; emitirlas como literales dentro de
   `transpile.rs` sería código Rust serio viviendo en strings — sin rustfmt, sin tests propios,
   duplicando lo que la VM ya tiene en sus builtins.
2. **Deriva de versiones**. «Las mismas versiones que la VM» como constantes en el transpilador
   es un contrato que se rompe en silencio: alguien sube `ring` en el `Cargo.toml` del proyecto
   y olvida el literal en `transpile.rs` → la paridad con el oráculo deja de ser por construcción.

### 4.2 El punto de partida: ese código YA existe y ya tiene la forma correcta

Hoy, en `src/builtins.rs`, la VM implementa los builtins con-crate como **funciones Rust puras
con firmas de tipos simples** (no tocan `Value` ni el GC — eso lo hace el opcode que las llama):

```rust
// src/builtins.rs — HOY (todo esto ya existe tal cual)
pub fn sha256(data: &[u8]) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA256, data).as_ref().to_vec()
}
pub fn sqlite_open(path: &str) -> Result<i64, String> { /* rusqlite + registro de handles */ }
pub fn tls_connect(host: &str, port: i64) -> Result<i64, String> { /* rustls + registro */ }
```

Y ya están tras features de Cargo (`net-tls`, `sqlite` — M89 build slim, M44a wasm), con **doble
definición**: la real y un fallback "unavailable" cuando la feature está apagada:

```rust
#[cfg(feature = "sqlite")]
pub fn sqlite_open(path: &str) -> Result<i64, String> { /* real */ }
#[cfg(not(feature = "sqlite"))]
pub fn sqlite_open(_path: &str) -> Result<i64, String> { Err(SQLITE_UNAVAILABLE.to_string()) }
```

Esta es exactamente la interfaz que un binario transpilado necesita llamar. El problema es solo
*dónde vive*: dentro del crate `raylang`, inaccesible para un `.rs` generado. **El crate
`ray-runtime` no hay que escribirlo — hay que extraerlo.**

### 4.3 La extracción: workspace de dos miembros

```
raylang/
├── Cargo.toml            # workspace = ["crates/ray-runtime", "."]
├── crates/ray-runtime/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs        # pub mod crypto; pub mod tls; pub mod sqlite;
│       ├── crypto.rs     # sha256/sha512/sha1/hmac/ed25519/chacha20poly1305/crypto_rand  (ring)
│       ├── tls.rs        # tls_connect/accept/upgrade/read/write + TlsConn + su registro de handles  (rustls)
│       └── sqlite.rs     # sqlite_open/exec/query + su registro  (rusqlite)
└── src/                  # el compilador/VM, como hoy
```

El `Cargo.toml` del runtime es **la única fuente de verdad de versiones** — se lleva las líneas
que hoy están en el raíz:

```toml
# crates/ray-runtime/Cargo.toml
[package]
name = "ray-runtime"
version = "0.1.0"          # versionado junto a raylang

[features]
default = []
tls    = ["dep:rustls", "dep:webpki-roots", "dep:rustls-pki-types", "dep:ring"]
crypto = ["dep:ring"]
sqlite = ["dep:rusqlite"]

[dependencies]
rustls   = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"], optional = true }
ring     = { version = "0.17", optional = true }
rusqlite = { version = "0.32", features = ["bundled"], optional = true }
# … webpki-roots, rustls-pki-types (las mismas líneas del Cargo.toml raíz actual)
```

El binario `ray` pasa a consumirlo — sus features actuales (`net-tls`, `sqlite`, el build slim
M89) se convierten en *reenvíos*:

```toml
# Cargo.toml raíz
[dependencies]
ray-runtime = { path = "crates/ray-runtime" }
[features]
net-tls = ["ray-runtime/tls"]
sqlite  = ["ray-runtime/sqlite"]
```

En `src/builtins.rs` cada función extraída queda como delegación de una línea (o directamente
`pub use ray_runtime::crypto::sha256;`). **La VM no cambia de comportamiento en absoluto**:
mismo código, otra ruta de módulo. El patrón `#[cfg(feature)]`/fallback existente se muda con
las funciones. Verificación de la extracción: si los 605 tests lib + corpus pasan tras moverla,
la VM quedó intacta.

### 4.4 Qué emite el transpilador: antes y después

Tomando `examples/db/sqlite_demo.ray`:

```text
// raylang
let db = sqlite_open("demo.db")?;
sqlite_exec(db, "insert into t values (?1)", ["hola"])?;
```

**Hoy** el transpilador no conoce `sqlite_open` → cae en `emit_stub` (fase 45):

```rust
// .rs generado HOY
fn sqlite_open(path: Rc<str>) -> Result<i64, Rc<str>> {
    panic!("'sqlite_open' no está soportada en el binario nativo (transpilación a Rust)")
}
```

**Después**, `emit_call` intercepta el builtin (igual que ya intercepta `std::net::tcp_connect`
→ `std::net` de Rust), activa el flag y emite **una llamada, no una implementación**. El
marshalling es trivial porque las firmas del runtime ya son de tipos simples:

```rust
// .rs generado DESPUÉS
fn sqlite_open(path: Rc<str>) -> Result<i64, Rc<str>> {
    ray_runtime::sqlite::sqlite_open(&path).map_err(|e| Rc::from(e.as_str()))
}
```

En el struct `Transpiler`, tres flags nuevos junto a los existentes:

```rust
needs_net: bool,        // hoy: emite helpers std::net inline (sigue igual, es std puro)
needs_rt_tls: bool,     // nuevo → feature "tls" de ray-runtime
needs_rt_crypto: bool,  // nuevo → feature "crypto"
needs_rt_sqlite: bool,  // nuevo → feature "sqlite"
```

**Frontera inline-vs-crate (importante)**: los `needs_*` actuales NO cambian. Todo lo que es std
puro (canales con hilos, handles de archivo, sockets TCP/UDP, señales) se sigue emitiendo inline,
porque el camino rápido `rustc` pelado no puede depender del crate. `ray-runtime` contiene
*exclusivamente* lo que requiere crates externos. Coste asumido y deliberado: TLS lleva su propio
registro de handles dentro del crate, separado del de archivos emitido inline (duplicación
pequeña; unificarlos acoplaría el camino rápido al crate).

### 4.5 Qué genera `build_native`

```rust
// pseudo-código del nuevo build_native
let (rust, features) = transpile::transpile(&program)?;   // features: las needs_rt_* activadas
if features.is_empty() {
    rustc_pelado(rust);                                    // el camino de HOY, intacto: ~0.2 s
} else {
    cargo_project(rust, &features);                        // el camino nuevo
}
```

`cargo_project` materializa un proyecto temporal:

```
$TMP/ray_native_demo_<pid>/
├── Cargo.toml
├── src/main.rs           # el Rust transpilado
└── ray-runtime/          # las fuentes del runtime, incrustadas (§4.6)
```

```toml
# Cargo.toml generado
[package]
name = "demo"
[dependencies]
ray-runtime = { path = "./ray-runtime", features = ["sqlite"] }  # SOLO las detectadas
[profile.release]         # el tier --release actual, como perfil de cargo
opt-level = 3
lto = "fat"
codegen-units = 1
```

y corre `cargo build` con un **`CARGO_TARGET_DIR` compartido** (p. ej. `~/.ray/native-cache/`):
rustls/ring compilan **una vez por máquina**; los builds siguientes solo compilan el `main.rs`
(~1 s, no minutos). `target-cpu=native` del tier `--release` va vía `RUSTFLAGS` (no es clave de
perfil).

### 4.6 Distribución del runtime: **incrustar las fuentes en el binario `ray`** (decidido)

¿De dónde sale `ray-runtime/` en la máquina de un usuario que solo instaló el binario `ray`?
Es el punto de diseño no obvio. Tres opciones evaluadas:

1. **Incrustar las fuentes en el binario** (`include_str!` de los ~4-5 archivos del crate,
   escritos al materializar el proyecto generado) — igual que ya se incrusta `prelude.ray`.
   **← ELEGIDA**: garantiza que el runtime es *exactamente* el de la versión de `ray` que
   ejecutas (paridad con el oráculo por construcción), funciona offline salvo la primera
   descarga de rustls/ring de crates.io, y no requiere publicar nada.
2. Publicar `ray-runtime` en crates.io y depender por versión — DESCARTADA: más "estándar" pero
   introduce el desfase binario-instalado vs crate-publicado, justo la deriva que queríamos matar.
3. Ruta de instalación (`~/.ray/runtime/`) — DESCARTADA: más piezas que se desincronizan.

### 4.7 Alternativas de arquitectura descartadas (para no re-litigar)

- **rlib/dylib precompilada junto a `ray`** (permitiría `rustc --extern` sin cargo, builds
  siempre de 0.2 s): el formato rlib NO es estable entre versiones de rustc (obligaría a fijar
  el toolchain del usuario), y una dylib rompe el binario estático de deploy.
- **"Siempre cargo", sin camino rápido** (menos matriz de mantenimiento): el camino `rustc`
  pelado ya existe, cuesta poco conservarlo, y evita cold-start/red/cargo para el 90 % de
  programas.
- **Emitir el glue de los crates inline en el `.rs` generado**: ver §4.1 — infierno de
  mantenimiento + deriva de versiones.

### 4.8 Por qué este diseño cumple el contrato del proyecto

- **El oráculo byte-idéntico se vuelve estructural.** El test
  `build_native_sqlite_coincide_con_la_vm` compara dos ejecuciones que atraviesan *la misma
  función Rust* (`ray_runtime::sqlite::sqlite_query`). No puede haber divergencia de semántica
  SQL, de mensajes de error ni de versiones — es una sola implementación con dos llamadores.
- **Los mensajes de error cara-al-usuario** (por convención en inglés y byte-idénticos entre
  motores) viven una vez, en el crate.
- **`--without <dep>` sale gratis**: apaga `needs_rt_*` → la función vuelve a caer en
  `emit_stub` → el binario-que-panica de hoy. El build slim M89 de la propia VM ya demuestra el
  patrón feature-apagada→fallback en este mismo código.
- **Escala fila-a-fila**, como todo el arco: el siguiente crate (p. ej. un cliente postgres
  nativo algún día) es un módulo + una feature en el runtime, una interceptación en `emit_call`,
  un test de oráculo.
- **Coste real**: el refactor del Paso 0 — mover ~600–800 líneas de `builtins.rs` al workspace
  member y re-exportar. Mecánico, verificable con la suite existente, y de paso ordena el lado
  VM: la frontera "lenguaje vs runtime-con-deps" queda explícita en la estructura del repo.

## 5. Plan por pasos (para retomar en sesiones futuras)

> Secuencia acordada el 15 jul 2026. Método invariante del arco: un paso a la vez, oráculo
> byte-idéntico contra la VM + test de integración `build_native_*` + entrada en PERFORMANCE.md
> por paso (incl. medir el coste de la primera build con cargo y el tamaño del binario).

- **Paso previo — los flecos que NO son de crates** (lista §2.4.1, fase 45): canal de struct de
  `metrics_server`, `spawn` de función nombrada (`udp_yield`), captura `t` en spawn + `RayShow`
  de `[fn]` (`framework`/`webserver`). Son fixes dentro del transpilador actual, sin cambio de
  arquitectura, y condicionan los demos que TLS desbloquearía — `webserver` necesita *ambas*
  cosas. Hacerlos primero evita que el estreno de TLS quede deslucido por bugs preexistentes.
- **Paso 0 — la extracción + la fontanería**:
  1. Crear el workspace y extraer `crypto.rs`/`tls.rs`/`sqlite.rs` de `src/builtins.rs` a
     `crates/ray-runtime` (§4.3); el binario `ray` delega/re-exporta. Gate: 605 tests lib +
     corpus 37/37 intactos (la VM no cambió).
  2. `build_native` bifurca: sin features → `rustc` pelado (como hoy); con features → proyecto
     Cargo generado con las fuentes incrustadas del runtime (§4.5–4.6) + caché de target
     compartido. Gate: test de que el hola-mundo sigue por la vía rápida + test sintético de que
     un programa marcado compila vía cargo.
- **Paso 1 — `rustls` (feature `tls`)**: el crate de mayor valor — desbloquea `https`,
  `webserver`, `https_server` y los clientes que cuelgan de TLS (postgres/mysql/redis-TLS).
  Interceptar los `__tls_*` en `emit_call` → `ray_runtime::tls::*`, activar `needs_rt_tls`.
  Verificación: los web-demos hoy limitados por el techo TLS.
- **Paso 2 — `rusqlite` (feature `sqlite`)**: superficie pequeña, resultados deterministas →
  oráculo directo con `examples/db/sqlite_demo.ray`.
- **Paso 3 — `ring` (feature `crypto`)**: sha/hmac/ed25519/chacha de producción (M43), donde los
  ejemplos lo pidan. (Nota: `ring` ya entra en el árbol con `tls`; este paso solo expone los
  builtins de crypto.)
- **Después (opcional)**: `--without <dep>` como flag de CLI (§3.3.3) — trivial una vez existe
  el mecanismo.

**Estado (15 jul 2026)**: diseño acordado y cerrado; **implementación NO iniciada** — Roberto
indicará cuándo arrancar (por el Paso previo o el Paso 0).
