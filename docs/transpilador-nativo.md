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
3. **Flags encima, como escape hatch** (lo útil de la opción 1). **HECHO** (commit `383d831`):
   `ray build --native --without crypto,tls,sqlite` fuerza el stub-que-panica aunque se detecte
   el uso. Se aplica **durante** la transpilación (las guardas de los arms de interceptación
   consultan un set `exclude` → el subsistema excluido cae en `_ =>` → la función stubbea), NO
   solo en el `Cargo.toml` generado — así el binario compila por la **vía rápida `rustc`** (sin
   cargo/red) si no queda otro subsistema con-crate. Vive en el `ray` CLI (arg efímero, como
   `--release`), no en el proyecto generado ni en el binario. Medido: cripto + `--without crypto`
   compila en ~0.15 s (vs cargo compilando ring). Casos de uso: builds herméticos/cross-compile/
   policy, y programas que **referencian** un crate pero no lo ejecutan (el escenario Fase 45).
   Diferido opcional: leer una política estable del `ray.toml` del proyecto (mismo mecanismo).

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

- **Paso previo — los flecos que NO son de crates** (lista §2.4.1, fase 45). **PARCIALMENTE HECHO**
  (Fase 50, 15 jul — commit `1d60046`). Tres fixes cerrados dentro del transpilador, sin cambio de
  arquitectura: (a) leak de scope al caer a stub (un `Task t` fantasma se filtraba y corrompía el
  `spawn` siguiente); (b) `RayShow` de un tipo-función anidado en un contenedor (`[fn]`/`Map`/tupla) →
  `impl` concreto que renderiza `<fn>`; (c) `spawn`/`scope` de una función **nombrada** de aridad 0
  (`spawn(worker)`), no solo un literal. Resultado: `udp_yield` **compila**; los dos bugs nombrados de
  `framework`/`webserver` (captura `t` + `RayShow [fn]`) quedan **corregidos**.
  - **Queda fuera del Paso previo** (reclasificado al modelo de concurrencia thread-safe, mismo cubo
    que §2.4.1): `framework`/`webserver` topan ahora con un `Rc<dyn Fn>` no-`Send` capturado por
    `spawn` (los closures `preparar`/`atender_conn` de `bucle_servidor`), y `metrics_server` con un
    canal de struct no-`Send`. Ambos exigen la repr `Arc<dyn Fn + Send + Sync>` / valores thread-safe
    para cruzar hilos → proyecto propio, no un fleco. (Nota: `webserver`/`https_server` además tienen
    el techo TLS del Paso 1.)
- **Paso 0 — la extracción + la fontanería**. **HECHO** (Fase 51 — commits `d199b93` + `cf8a8e5`).
  - **Hallazgo que reordenó la extracción**: los handles TLS y SQLite comparten el `enum
    OpenHandle` con archivos/TCP/UDP (registro común; `close(h)` no distingue) → extraerlos es un
    *split de registro* (diseño propio de sus Pasos), NO una extracción mecánica. **Crypto es
    puro** (10 funciones sin estado, único uso directo de `ring::`) → se extrajo limpio y validó
    el mecanismo con riesgo cero. Por eso el Paso 0 = **solo crypto**; TLS/SQLite se extraen en
    sus Pasos, con su registro.
  - **0a** (`d199b93`): workspace + `crates/ray-runtime` con módulo `crypto` (feature `crypto`,
    ring 0.17); `builtins.rs` delega (una línea/función) conservando su gating cfg; `ring` deja
    de declararse directo en raylang (llega vía ray-runtime + transitivo de rustls). Verificado:
    default + slim + wasm-sin-ring (cargo tree); 605 tests lib intactos.
  - **0b** (`cf8a8e5`): `build_native` bifurca (sin features → `rustc` pelado; con features →
    proyecto Cargo con `ray-runtime` **incrustado** vía `include_str!` + `CARGO_TARGET_DIR`
    compartido). `transpile()` → `Transpiled { source, rt_features }`; el transpilador intercepta
    los primitivos de cripto → `ray_runtime::crypto::*` y activa `needs_rt_crypto`. **Vertical
    slice**: `std/crypto` (sha/hmac/ed25519/chacha) transpila a nativo por primera vez,
    byte-idéntico a la VM. Tests `build_native_crypto_de_produccion_via_ray_runtime` +
    `build_native_sin_crate_externo_usa_la_via_rapida`.
- **Paso 1 — `rustls` (feature `tls`)**. **HECHO** (Fase 52 — commit `2759650`). TLS cliente +
  servidor en el binario transpilado. Estrenó el **split del registro de handles** del Paso 0:
  el binario nativo usa hilos reales → I/O TLS **bloqueante** (`ray_runtime::tls::TlsStream` sobre
  `rustls::StreamOwned`, mucho más simple que el pump no-bloqueante de la VM); el `__RayHandle`
  inline gana una variante `Tls` tras un `Arc<Mutex<TlsStream>>` propio (lock por-conexión → no
  serializa/deadlockea conexiones concurrentes). Intercepta `__tls_connect`/`_h2`/`accept`/`upgrade`;
  `socket_read_bytes`/`socket_write` despachan a TLS como la VM. Verificado (conductual, red no
  determinista): cliente nativo ↔ servidor VM y servidor nativo ↔ cliente VM (`eco: hola tls`, con
  los fixtures `tests/fixtures/tls_*.pem`). Diferido: los web-demos completos (`webserver`/`https`)
  siguen topando además con el `Rc<dyn Fn>` no-`Send` en spawn (Paso previo, modelo de concurrencia).
- **Paso 2 — `rusqlite` (feature `sqlite`)**. **HECHO** (Fase 53 — commit `2dccb8a`). SQLite embebido
  (bundled) en el binario transpilado; conexión propia (no muta un handle TCP) → I/O local reteniendo
  el lock global (sin `Arc<Mutex>` por-conexión). Determinista → **oráculo por byte-identidad**: el demo
  real `examples/db/sqlite_demo.ray` (`:memory:`) transpila a nativo idéntico a la VM. De paso arregló un
  **bug preexistente de `type_of(match)`** (un brazo divergente hacía que un `var x = match {…}` con
  struct no se clonara al leer → move error): `arm_type`/`pattern_binding_types` saltan brazos
  divergentes y resuelven el tipo del binding desde el escrutinio.
- **Paso 3 — `ring`-extra (feature `crypto`)**: la crypto ya transpila (Paso 0). Este paso solo
  aplicaría si aparecen builtins de crypto avanzada aún no cubiertos; hoy no hay pendientes.
- **Después (opcional)**: `--without <dep>` como flag de CLI (§3.3.3) — trivial una vez existe
  el mecanismo.

**Estado (15 jul 2026)**: **Paso previo**, **Paso 0** (crypto/ring), **Paso 1** (`rustls` TLS, cliente +
servidor) y **Paso 2** (`rusqlite` DB) **HECHOS**. **El techo de los crates está quitado**: crypto + TLS +
DB transpilan a nativo, bajo demanda, con paridad por construcción (mismo código que la VM). El diseño
`ray-runtime` escala fila-a-fila para cualquier crate futuro. El escape hatch `--without` está **hecho**
(§3.3.3). No quedan pasos obligatorios; `ring`-extra solo si aparecen builtins de crypto avanzada no
cubiertos (hoy ninguno). Diferido opcional: leer la exclusión de una política estable en `ray.toml`.

## 6. Auditoría del backend nativo (16 jul 2026) — hallazgos, prioridades y esfuerzo

> Revisión a fondo de `src/transpile.rs` (~4.760 líneas), `src/cli.rs::build_native`,
> `crates/ray-runtime/` y los tests `build_native_*` de `tests/cli_cli.rs`, contrastando puntos
> clave contra la VM (`vm.rs`). Veredicto general: **el diseño es sólido** (bifurcación
> rustc-pelado/cargo limpia y testeada por ambos lados, runtime incrustado con `include_str!` que
> garantiza paridad de versión binario↔runtime, deuda declarada honestamente en §2.4) — la
> debilidad está en (a) varias **divergencias silenciosas VM↔nativo en programas válidos**,
> (b) la **capa de verificación** (más débil de lo que §2.3 promete) y (c) flecos de DX.
> Nada estructural que re-litigar.

> **Estado (16 jul 2026): LOTES URGENTE y VERIFICACIÓN COMPLETOS.** Resueltos con test: **H1**
> (CI verde; el error nombra el origen del typo), **H2** (rendezvous ya no se deadlockea, handshake
> síncrono), **H3** (el override del usuario gana sobre el builtin), **H4/H8** (izado de índice/
> receptor/RHS en asignaciones autoreferentes + orden de la VM), **H5** (keywords de Rust como
> identificadores vía raw idents; campos/variantes/constantes/params-de-closure enrutados por
> `mangle`; temporales al prefijo reservado `__rt_*`), **H14** (caché en `~/.ray/native-cache/`),
> **H15** (limpieza de `.rs`/proyecto Cargo tras un build ok), **H10** (corpus automatizado:
> `tests/native_corpus.rs`, 50 ejemplos deterministas nativo≡VM), **H11** (guardia de la tabla
> `BUILTINS`: `NATIVE_TRACKED_BUILTINS` + test que la fuerza), **H12** (CI: cachea
> `~/.ray/native-cache`, verifica rustc, corpus nocturno), **H9** (infiere args de tipo en literales
> genéricos anidados), **H17** (mensajes del transpilador/build a inglés, sin jerga `spike:`), **H18**
> (escape de nombres FFI + float inf/NaN), **H7** (aviso de funciones stubbeadas al compilar; la
> divergencia muda de cancelación queda documentada). **Estructural:** **H13** (error-paths de
> SQLite/TLS byte-idénticos nativo≡VM), **H20** (`--target` cross-compile + `Cargo.lock` cacheado),
> **H19** (medido: el nativo ya gana a la VM en código idiomático; fast-path ASCII en `len` aplicado).
> **AUDITORÍA CERRADA (17 jul 2026): los 21 hallazgos resueltos.** Los tres que quedaban se
> cerraron en orden: **H6** (16 jul: paridad de errores de ejecución por defecto + `--fast`),
> **H21** (17 jul: port de scheduler completo, N1–N5) y **H16** (17 jul: `type_of` e
> `is_handled_builtin` consultan la tabla `BUILTINS`; las ramas de `emit_call` son implementación
> por motor, no duplicación). Los ítems marcados abajo con ✅ ya están hechos.

> **Revisión post-lote (16 jul 2026)**: un análisis crítico de los cuatro lotes cerró seis huecos que
> los fixes originales dejaron (patrón común: cada fix cubría los sitios enumerados, no la clase):
> **H3-bis** (guarda defensiva `builtins::lookup` en `shadows_builtin` + test de las tres resoluciones
> — se verificó que la divergencia espejo temida NO puede ocurrir: el checker prohíbe redefinir un
> builtin de tabla), **H4-bis** (la clase del doble borrow vivía también en `insert`/`add_to`/`remove`
> de Map → izado `__rt_*`), **H14-bis** (la carrera del stem: el pkg de la caché lleva un hash de la
> ruta canónica), **H12-bis** (el corpus corre en CADA push, no nocturno — el schedule solo evalúa
> `main` y no habría corrido hasta el merge; skip sin rustc = fallo duro bajo CI), **H5-bis** (`gen`
> keyword + los dos caminos compilaban con ediciones DISTINTAS, 2015 vs 2024 → ambos a `--edition
> 2024`), y el lote barato: mensajes de éxito del build a inglés, expects FFI generados alineados con
> el texto de la VM, corpus +4 entradas (multi-módulo `modulos`/`capsula`/`proyecto` — antes el loader
> →transpile estaba a ciegas — y `sqlite_demo` — antes el camino Cargo no se cubría), caso rustls sin
> red (server name inválido) en el test H13, y `docs/build.md` actualizado (`--target`, lock, caché).
> **Deuda anotada, diferida a sabiendas**: `ASSOC_FNS` fuera de la guardia H11; reproducibilidad del
> lock solo por máquina (H20); CI solo Linux x86_64.

> **Slice de canales (16 jul 2026, post-revisión)**: los tres bugs de corrección del runtime de
> canales embebido, que vivían solo como nota dentro del diferido H21, están CERRADOS: (1) el
> handshake rendezvous pasa de "cola vacía" a **generación de consumo** (`taken`) — con ≥2 emisores,
> A podía despertar con el valor de B en cola y re-dormirse para siempre aunque el suyo ya se
> consumió; (2) **`send` sobre canal cerrado** aborta con el texto de la VM (`send on a closed
> channel`; antes: descarte silencioso); (3) **`close` con emisor bloqueado** aborta en el sitio del
> close (`close on a channel with a blocked sender`, contador `senders`; antes: return silencioso del
> emisor y su valor quedaba consumible). Los panics llevan el MISMO texto que el error de ejecución de
> la VM; el exit code (101 vs 70) queda con H6. Tests: multi-emisor ×10, send-cerrado y
> close-bloqueado en `tests/cli_cli.rs`.

> **Port H21 N1/N2/N5 (17 jul 2026)**: tres piezas grandes del port de scheduler, HECHAS.
> **N1 — contención de fallos**: el error de ejecución viaja como panic con payload (`__RayErr`);
> el `catch_unwind` de cada tarea lo captura en su Task (`Err(msg)`, como el `Failed` de la VM) y el
> proceso solo muere cuando el fallo llega a `main` sin observarse; `join` re-lanza, el scope propaga.
> **N2 — `try_join`**: el fallo como VALOR (`Result<T, string>`), sobre `wait()`;
> `NATIVE_STUBBED_BUILTINS` queda VACÍO. **N5 — valores de heap y funciones cruzando hilos**:
> (a) structs/enums/Map/arrays/tuplas cruzan canales/Tasks/capturas de spawn por DEEP COPY — la repr
> Send universal `__RaySend` + conversores `__to_send_N`/`__from_send_N` generados bajo demanda
> (semántica de heap aislado M38: lo que cruza se copia; los canales son el conducto); las capturas
> del closure de spawn se convierten fuera y se reconstruyen dentro (una `var`-celda se re-crea como
> celda local → mutación aislada, como la VM). (b) un param de tipo fn que cruza un spawn (directa o
> transitivamente — punto fijo sobre el grafo de llamadas) se emite como GENÉRICO de Rust
> (`__F: Fn(..) + Send + Sync + Clone`): una función NOMBRADA o un closure de capturas enviables
> sirven de handler a través de la cadena serve→loop→handle→spawn; las capturas de heap de esos
> closures también cruzan por deep copy (reconstrucción por llamada). **Resultado**: el patrón
> webserver COMPLETO transpila sin stubs — el demo SSR (webserver + templates + handler puro) corre
> NATIVO byte-idéntico a la VM (`/`, `/lang/rust`, 404).
>
> **Port H21 N3/N4 (17 jul 2026): HECHOS — port de scheduler COMPLETO.**
> **N3 — cancelación de hermanas (M12.5)**: cada Task lleva un token `Arc<AtomicBool>`; el hilo hijo
> ESCRIBE su resultado al terminar (push + condvar, ya no `JoinHandle.join`) y el scope, al salir,
> espera a sus hijas SIN orden fijo: si alguna falló, cancela a las pendientes y propaga el fallo
> ORIGINAL de inmediato (antes: unión en orden de registro → un fallo podía colgar para siempre
> detrás de una hermana bloqueada). La cancelación es COOPERATIVA como en la VM (que solo cancela en
> los yields M:1): toda espera bloqueante (send/recv/join/select/scope) usa `wait_timeout` de 10 ms +
> chequeo del token y aborta deshaciendo su rastro (contador `senders`, valor en cola del rendezvous);
> código que corre sin bloquearse no se interrumpe (divergencia menor documentada). Transitiva: una
> hija que falla con scopes sin cerrar (unwinding) cancela a sus nietos. Texto de cancelación
> idéntico a la VM (`task cancelled (a sibling failed)`).
> **N4 — select sin busy-wait**: condvar GLOBAL de actividad (generación monótona `__RAY_ACT_*`);
> `send`/`close`/fin-de-tarea la notifican y `select`/salida-del-scope esperan en ella (la generación
> se lee ANTES de escanear → un send concurrente no se pierde). Antes: poll de 50µs; ahora 0.00s de
> CPU esperando. Orden de locks canal/tarea → actividad, sin ciclos.
> **Bonus — bug REAL de la VM destapado por el port**: `ScopeEnd` aparca sobre la PRIMERA hija
> pendiente y `fail_current_fiber` solo despertaba a los joiners de la tarea que falló → si fallaba
> OTRA hermana, el scope nunca re-escaneaba: **deadlock en vez de propagar** (violaba M12.5; el test
> existente pasaba porque registraba la que falla primero). Fix en tándem en `src/vm.rs`:
> `wake_all_join_waiters` al fallar una tarea (despertar espurio seguro: re-escanean y se re-aparcan)
> + `cancel_task` despierta a los joiners de la cancelada. Test de regresión con el orden inverso en
> `tests/concurrency_cli.rs`. **El port H21 está CERRADO.**

### 6.1 P0 — Roto en el momento de la auditoría

**H1. ✅ RESUELTO. CI rojo en `main`: assert desincronizado con su mensaje.** El commit `e6bb6b7` cambió el
mensaje de `src/cli.rs:608` de `"unknown subsystem in --without"` a `"unknown subsystem to
exclude"` (necesario porque la exclusión ahora une CLI + `ray.toml` y el origen ya no es siempre
el flag) pero no actualizó el assert de `tests/cli_cli.rs:551`. Verificado ejecutándolo:
`build_native_without_rechaza_un_subsistema_desconocido` **falla hoy** → `cargo test` completo
(el CI, `.github/workflows/ci.yml`) está rojo en `main`. *Fix de una línea.* Al arreglarlo,
conviene que el error diga de dónde vino el nombre inválido (flag vs manifiesto): un typo en un
`ray.toml` versionado afecta a todo el equipo. **Esfuerzo: ~5 min** (el fix; +30 min si se
distingue el origen).

**H2. ✅ RESUELTO. Deadlock en canales rendezvous (`channel(0)`).** En el runtime de canales embebido
(`src/transpile.rs:843-894`), `send` espera en la condvar mientras `q.len() >= cap` (con `cap=0`,
siempre cierto) y `recv` espera mientras `q.is_empty()` — **no existe la entrega directa
emisor→receptor** que la VM sí hace (M12.2, caso 1 de `send`). Ambos hilos duermen para siempre:
un programa rendezvous válido que corre en la VM se cuelga en nativo. Además `__ray_select` solo
considera listo `!q.is_empty() || closed` — no ve "emisor bloqueado", que en la VM cuenta como
canal listo (M12.4). *Fix:* implementar el handshake de entrega directa (un slot de rendezvous o
un contador de emisores esperando que `recv` consuma) + que `select` lo consulte; testear los
tres casos (cap=0, acotado lleno, `select` sobre emisor bloqueado). **Esfuerzo: ~1 día** (el
runtime es código-como-string, cada iteración recompila el binario de prueba).

### 6.2 P1 — Divergencias silenciosas en programas válidos

**H3. ✅ RESUELTO. Los builtins interceptados PISAN los overrides del usuario.** `skip_fn_def`
(`transpile.rs:140`) emite correctamente la *definición* de un `sort`/`trim`/`get`/`map`
redefinido por el usuario (línea < `LINE_BASE` = no-prelude), pero el sitio de llamada en
`emit_call` (`transpile.rs:2537+`) matchea el nombre del builtin **antes** de mirar
`self.funcs` → `sort(xs)` llama al `__ray_sort` ascendente aunque el usuario definiera su propio
`sort` descendente. Solo `get_or` tiene guarda (por aridad, `transpile.rs:2751`). En la VM el
override gana (M7.3); en nativo gana el builtin, **en silencio**. *Fix:* anteponer
`self.funcs.contains_key(name) && !viene_del_prelude` al match de interceptación + test por
builtin sobreescribible. **Esfuerzo: 2–3 h** (la guarda es pequeña; lo laborioso es el test).

**H4. ✅ RESUELTO. Doble borrow de RefCell en asignación indexada.** El RHS se iza a un temporal `__rhs`
(`transpile.rs:1278-1285`), pero el **índice** se evalúa dentro del `borrow_mut()`:
`a[a.len()-1] = v` genera `a.borrow_mut()[a.borrow().len()-1] = …` → panic de RefCell en runtime
donde la VM funciona. `push` (`transpile.rs:2679`) iza el valor pero tiene el mismo agujero si el
receptor es una expresión que borrowea la misma colección. *Fix:* izar también el índice (y el
receptor en `push`) a temporales, igual que ya se hace con el RHS. **Esfuerzo: 1–2 h.**

**H5. ✅ RESUELTO. Identificadores raylang legales que generan Rust inválido o valores equivocados.**
`mangle` (`transpile.rs:35-42`) solo trata `self`; no cubre:
- **Keywords de Rust** (`type`, `loop`, `move`, `ref`, `mod`, `use`, `where`, `crate`,
  `unsafe`, …) como variables (`transpile.rs:1219`), params (`:1010`) y sobre todo **campos de
  struct, que nunca se manglan** (`:570`, `:1766`, `:1292`) → rustc rechaza el fuente generado.
- **Colisiones con temporales sintéticos**: un local del usuario llamado `__v`/`__it`/`__c`/
  `__rhs` es shadowed por los temporales que emite el transpilador (`:2679`, `:1279`, `:1721`) →
  **valor equivocado en runtime**, ni siquiera error de compilación.
- **Constantes de módulo**: la definición emite `fn {c.name}()` SIN manglar
  (`transpile.rs:600`) mientras el uso sí mangla (`:1575`) → un `pub let` en un módulo importado
  genera `fn geo::PI()`, Rust ilegal.
*Fix barato:* raw identifiers (`r#type`) o sufijo en `mangle` + aplicarlo a campos y constantes;
prefijo imposible `__ray_tmp_` para todos los temporales sintéticos. **Esfuerzo: 2–4 h**
(mecánico pero toca muchos sitios de emisión; la constante de módulo cae junto, ~30 min).

**H6. ✅ RESUELTO (16 jul 2026, enfoque 1 del menú: paridad por defecto + `--fast` opt-out).**
La aritmética de `int` es **CHECKED por defecto** (helpers inline `__ray_add/sub/mul/div/mod/neg`,
mismos textos que la VM: `arithmetic overflow on int`, `integer division by zero`, `modulo by zero`);
`panic`/`assert`/`assert_eq` abortan con los mensajes de los cuerpos del prelude; TODO error de
ejecución sale por `__ray_rt_err` → **`runtime error: <msg>` + exit 70** (sin posición: el nativo no
lleva el AST — paridad de código y de mensaje, no de `L:C`). La cola de panics de Rust (índice fuera
de rango, expects FFI) se captura con `catch_unwind` en `main` → **también exit 70** (texto de Rust).
**`--fast`**: wrapping (renuncia deliberada al check de overflow); div/mod por cero SIGUEN chequeados
(Rust lo hace gratis). **Medición** (release, aarch64): checked cuesta ~2× en un bucle de puro int
(0,50s vs 0,24s / 500M iter), ~20 % en fib(42) recursivo (0,85s vs 0,71s), ~0 en código idiomático
(el corpus de 54 no se mueve). Se eligió paridad por defecto porque el overflow silencioso era el
único riesgo de corrección real del backend; quien quiera el último tramo, `--fast` explícito.
Test: `build_native_errores_de_ejecucion_exit_70_como_la_vm` (7 casos, exit + mensaje ≡ VM) y
`build_native_fast_envuelve_overflow_pero_chequea_div_cero`. — *Ficha original:* La VM hace
`checked_add` → `"arithmetic overflow on int"` + **exit 70** (`vm.rs:579,632`; `cli.rs:338`);
el nativo (rustc con `-O` en ambos tiers, `cli.rs:685-689`) **envuelve en silencio**. Div-por-cero
e `i64::MIN / -1` panican en Rust (mensaje inglés de Rust, **exit 101**) vs error raylang +
exit 70. `panic`/`assert`/`assert_eq` (`transpile.rs:2993-3009`) bajan a `panic!` de Rust:
`thread 'main' panicked …` + exit 101, no el formato ni el exit de la VM. Está documentado como
"fiel sin desbordamiento" (§1), pero es **LA divergencia byte-conductual sistemática** del
backend. *Fix:* aritmética checked en la emisión de operadores (mensaje byte-idéntico + exit 70,
p. ej. vía un panic hook propio o salida de proceso controlada) — y **medir** el coste (checked
resta algo del 24–61×; quizá un flag `--unchecked` para el tier release). **Esfuerzo: ~1 día**
(mecánico en la emisión, pero hay que cuadrar mensajes byte-idénticos y benchmarkear).
> **Diferido para analizar en el futuro** por el tradeoff paridad-vs-rendimiento (el rendimiento es el
> objetivo nº 1 del proyecto). **Impacto actual** (evaluado 16 jul): (1) el **overflow silencioso** es el
> único riesgo de CORRECCIÓN real — un programa que rebase `i64` (~9,2·10¹⁸: factoriales, hashing,
> acumulados grandes) da un resultado ERRÓNEO en nativo donde la VM aborta limpio; poco común pero es un
> footgun silencioso. (2) div-por-cero y `panic`/`assert` fallan RUIDOSAMENTE en ambos (solo difieren el
> código —101 vs 70— y el texto), impacto menor salvo scripts que discriminen por `exit == 70`. (3) Los
> 50 ejemplos del corpus (H10) NO rebasan → cero impacto en la cobertura actual. Cuando se retome, el
> menú de enfoques (paridad+`--fast` / solo-exit-code / solo-panic / documentar) quedó en la conversación.

**H7. ✅ RESUELTO (parcial: aviso de stubs). Semántica no implementada que NO se rechaza en compilación.** Dos formas:
- **Divergencia muda**: la **cancelación de hermanas M12.5** y `try_join` no están implementadas
  en nativo (deuda declarada en §2.4), pero un programa que dependa de ellas **compila sin aviso
  y se comporta distinto** (las hermanas siguen corriendo). Peor que un stub que panica.
- **Stub silencioso**: la degradación a `panic!("… no está soportada")` (`transpile.rs:625-648`)
  solo se ve con `RAYLANG_TRANSPILE_DEBUG`; el build dice "ok" y el binario muere en runtime.
*Fix:* detectar y **rechazar en compilación** (o al menos warning prominente) los usos de
cancelación/`try_join`; imprimir SIEMPRE un resumen "N funciones degradadas a stub: …" al
compilar. **Esfuerzo: ~0,5 día** (es detectar y reportar, no implementar).
> **HECHO — el stub silencioso**: `build_native` ahora AVISA de cada función stubbeada (nombre + motivo)
> al compilar (`Transpiled.stubbed`), no solo con `RAYLANG_TRANSPILE_DEBUG`. Un uso de `try_join` cae en
> ese aviso. **QUEDA — la divergencia muda de cancelación**: la semántica automática de cancelación de
> hermanas (M12.5) es un comportamiento del scheduler, no una llamada detectable en un punto → no se
> rechaza estáticamente; sigue documentada como límite del backend nativo (§2.4). Un futuro análisis
> estático (¿un `scope` con hijos que puedan fallar?) podría avisar, pero es incierto y de bajo ROI.

**H8. ✅ RESUELTO (con H4). Orden de evaluación divergente en varios puntos.** `push(a, v)` evalúa `v` antes que `a`
(`transpile.rs:2679`); la asignación indexada evalúa RHS antes que target/índice (`:1279`); el
`Assign` a campo igual (`:1288`). La VM evalúa izquierda→derecha; con expresiones con efectos
(llamadas que mutan) el nativo observa otro orden. *Fix:* izar operandos a temporales en orden
fuente (compone con H4). **Esfuerzo: incluido en H4** si se hace junto (+1 h de tests).

**H9. ✅ RESUELTO. `type_of` de literales genéricos descarta los args de tipo.** `type_of(StructLit/EnumLit)`
devuelve `Type::Struct(name, vec![])` (`transpile.rs:3435`, `:3448`) → con un literal genérico
(`Par { a: 1, b: true }`), `enum_subst` recibe args vacíos y el tipo del campo queda como el
param sin sustituir → clasificación heap/escalar errónea o degradación a stub en código genérico
válido. *Fix:* propagar los args de tipo que el checker ya resolvió. **Esfuerzo: 2–4 h.**

### 6.3 P1 — Capa de verificación (lo que evita que todo lo anterior vuelva a pasar)

**H10. ✅ RESUELTO. El claim "37/37 ejemplos byte-idénticos" (§2.3) NO está automatizado.** No existe ningún
test que itere `examples/` por el camino nativo (grep de `read_dir`/corpus en `tests/`: nada).
Hay ~28 tests `build_native_*` en `tests/cli_cli.rs` sobre programas concretos (fib,
multi-módulo, env/args, CSP, TLS×2, crypto, sqlite, json, protobuf, iteradores, FFI, TCP/UDP…) —
buena selección, pero la cobertura "lenguaje completo" es un claim manual que se descompasará en
silencio. *Fix:* test corpus que itera los ejemplos deterministas nativo↔VM, probablemente
`#[ignore]` por lento (como los metacirculares), corrido en CI nightly o bajo demanda.
**Esfuerzo: 0,5–1 día.**

**H11. ✅ RESUELTO. Sin guardia contra la "triple implementación" de builtins.** `transpile.rs` **no consulta
la tabla `BUILTINS`** de `src/builtins.rs` (cero referencias): un builtin nuevo añadido a
checker/VM/intérprete cae en nativo en `emit_stub` (panic en runtime) o en
`Err("spike: … no soportada")` sin que ningún test lo detecte. *Fix:* test que recorre `BUILTINS`
y exige que cada fila esté (a) interceptada por el transpilador o (b) en una lista explícita de
omitidos-conscientes; requiere exponer del transpilador "qué nombres intercepto".
**Esfuerzo: 0,5–1 día.** — Este hallazgo es el síntoma de **H16** (duplicación interna).

**H12. ✅ RESUELTO. Tests nativos que se saltan en silencio + caché de CI incompleta.** Cada test
`build_native_*` hace `if rustc/cargo no disponible → return` con un `eprintln`
(`cli_cli.rs:89-92`): en una máquina sin toolchain pasan todos "en verde" sin ejecutar nada.
Además los 5 tests del camino Cargo (TLS×2, crypto, sqlite) compilan ring+rustls+rusqlite-bundled
(SQLite en C) en cada CI limpio: la caché del CI (`ci.yml:31-36`) guarda `~/.cargo` y `target`
pero **no** `$TMP/ray_native_cache` → minutos de compilación por run. *Fix:* en CI, fallar (o
skip explícito reportado) si falta rustc; añadir `ray_native_cache` a la caché.
**Esfuerzo: 1–2 h.**

**H13. ✅ RESUELTO. TLS/sqlite: un solo escenario feliz cada uno, ningún camino de error.** TLS: un eco
cliente↔servidor (`cli_cli.rs:346,382`); sqlite: un demo `:memory:` (`cli_cli.rs:457`). La
promesa "mensajes byte-idénticos porque es el mismo código" (§4.8) es precisamente lo que habría
que verificar en los caminos de error (certificado inválido, handshake fallido, SQL malformado):
el marshalling `map_err` del borde puede alterar el mensaje. **Esfuerzo: 1–2 días.**

### 6.4 P2 — DX, robustez y rendimiento del código generado

**H14. ✅ RESUELTO. La caché de builds vive en `temp_dir()`, contradiciendo este mismo doc.** §3.3 decide
`~/.ray/native-cache/`, pero `cli.rs:759` usa `std::env::temp_dir().join("ray_native_cache")` —
macOS purga `/tmp` (3 días sin uso) y Linux al reboot → la promesa "ring compila una vez por
máquina" se rompe periódicamente sin explicación. Carrera menor: el binario producido se copia de
`target/debug/<pkg>` donde `pkg` es solo el *stem* (`cli.rs:770`) — dos builds concurrentes de
programas distintos con el mismo stem comparten esa ruta (el proyecto temporal lleva PID; la
salida en la caché compartida, no). **Esfuerzo: ~30 min** (mover la caché; +1 h si se
desambigua el stem, p. ej. hash de la ruta).

**H15. ✅ RESUELTO. Artefactos temporales sin limpiar.** Ni el `.rs` temporal (`cli.rs:679`) ni el proyecto
Cargo generado (`cli.rs:726`) se borran tras compilar — se acumulan en `$TMP` uno por PID. Choca
con la política del proyecto de cero fugas de artefactos. **Esfuerzo: ~30 min.**

**H16. ✅ RESUELTO (17 jul 2026, alcance acotado). Duplicación interna del registro de builtins
(deuda estructural).** El conocimiento de
los builtins está repetido a mano en ≥4 sitios de `transpile.rs` — `emit_call` (~735 líneas),
`type_of` (~330 líneas de match paralelo), `is_handled_builtin` (`:98`) e `is_prelude_impl`
(`:47`) — sin apoyarse en la tabla `BUILTINS` (la limpieza L1 hizo exactamente esto para
checker/VM/intérprete). Cada builtin nuevo exige 3–4 ediciones sincronizadas; H3 y la guarda
ad-hoc de `get_or` son síntomas. Al menos la regla `check` de L1 debería aportar el tipo de
retorno y matar el `type_of` paralelo.
> **Resolución (la sugerencia del propio hallazgo):** `type_of` ahora consulta la regla `check` de
> la tabla `BUILTINS` como caso general — por nombre exacto (público `join`/`spawn`/… o primitivo
> `__sha256`/… tal como llega el sitio) y por nombre pelado para métodos manglados (`int#to_string`)
> — con dos guardas (una definición de USUARIO o un closure local ganan; si la regla no casa, cae al
> camino manual). Eso mató ~25 brazos del match paralelo, incluidos los polimórficos (`join`
> Task-vs-string, `close` canal-vs-handle, `spawn`/`scope` — habilitado por un brazo `Func` nuevo
> que tipa el literal de función) y TODOS los de cripto/TLS/SQLite/UDP. `is_handled_builtin` deriva
> los públicos de `builtins::lookup` en vez de repetirlos. Quedan a mano, a sabiendas: los WRAPPERS
> del prelude que reenvasan `[T]` → Option/Result (get/recv/parse_int/try_join/…, no son filas de la
> tabla) y los métodos manglados cuyo primitivo cambia de nombre (`#len` → `__len`). Las ramas de
> **`emit_call` NO son duplicación**: son la *implementación* por motor (como el match de opcodes de
> la VM y el `eval_builtin` del intérprete, que L1 dejó como código a propósito). La guardia H11
> (`NATIVE_TRACKED_BUILTINS`) sigue siendo el checklist de clasificación.

**H17. ✅ RESUELTO. Mensajes en español y jerga interna de cara al usuario.** Contra la convención del
proyecto (diagnósticos en INGLÉS): el panic del stub `"'f' no está soportada en el binario
nativo…"` (`transpile.rs:1048`), `"native build: no se pudo ejecutar rustc (¿está en el
PATH?)"` (`cli.rs:701`), `"cargo falló (código N)"` (`cli.rs:779`), y los rechazos `"spike: …"`
(~40 sitios) — jerga del spike de julio que llega al usuario final. *Fix:* traducción por lotes
(`tools/spanglish.py` ya existe) + renombrar el prefijo `spike:`. **Esfuerzo: 2–3 h.**

**H18. ✅ RESUELTO. Robustez menor del codegen.** (a) Inyección de Rust vía el nombre de librería FFI:
`#[link(name = "{}")]` interpola `e.lib` sin escapar (`transpile.rs:1075`; ídem `#[link_name]`
`:1085`) — no es frontera de seguridad (el usuario compila su propio código) pero merece
validación o `{:?}`. (b) `ExprKind::Float` con `{:?}f64` (`:1509`) emite `inff64`/`NaNf64`
(Rust inválido) para un literal `1e999`. El escaping de strings (`{:?}` en `:1512`,
`push_fmt_literal` `:3961`) sí es correcto. **Esfuerzo: 1–2 h.**

**H19. ✅ MEDIDO — mayormente un no-problema; 1 win aplicado, el resto diferido.** Rendimiento del
código generado.
- Strings por carácter O(n) por operación → bucles O(n²): `s[i]` → `.chars().nth(i)`
  (`transpile.rs:1693`), `len(s)` → `.chars().count()` (`:2670`) recomputados en cada iteración
  de un `while i < s.len() { s[i] }`; `substring` colecta un `Vec<char>` completo por llamada
  (`:2648`), `__ray_index_of` dos (`:493`).
- `for x in arr` clona el `Vec` entero al entrar (`:1402`, `.borrow().clone()`) incluso cuando el
  cuerpo no muta; `filter` clona cada elemento dos veces
  (`.cloned().filter(|__x| __f(__x.clone()))`, `:3027`). Un análisis "el cuerpo no muta el
  arreglo" permitiría iterar el borrow.
*Método:* cada mejora con benchmark antes/después, estilo arco P. **Esfuerzo: 2–4 días.**
> **Medido (16 jul).** La premisa "margen fácil para batir a la VM" resultó **mayormente falsa**: el
> nativo YA aplasta a la VM en código idiomático. Benchmarks (release, aarch64): iteración de arreglo
> `for x in arr` con clone del Vec → **nativo 0,29 s vs VM 4,1 s (~14×)**, o sea el clone NO es cuello;
> `for c in s` idiomático (100k×100) → **nativo 0,5 s vs VM sin terminar en 2 min**. El ÚNICO caso donde
> el nativo PIERDE (~6×) es el **anti-patrón** `while i < s.len() { s[i] }` (indexado de string en bucle,
> O(n²)) — y ahí la VM también es O(n²), solo con constante menor (`is_ascii` SIMD vs `chars().count()`
> decodificado). **HECHO**: se copió el fast-path ASCII de la VM a `len` de string (`is_ascii()` → `.len()`,
> si no `.chars().count()`); correcto (ASCII y no-ASCII casan con la VM), coste cero. **DIFERIDO** (bajo
> ROI): cerrar del todo el anti-patrón `s[i]` exige cambiar la representación de string (`Rc<str>` →
> indexable por char), un cambio grande y arriesgado que solo beneficia código no-idiomático; los clones
> de `for`/`filter` no son cuello (el nativo gana igual) → no valen el análisis de mutación.

**H20. ✅ RESUELTO. Portabilidad y reproducibilidad no declaradas.** No hay `--target` (cross-compilation);
`--release` fija `target-cpu=native` (binario no portable, documentado) sin alternativa "release
portable"; el proyecto Cargo generado no fija `Cargo.lock` (las deps de `ray-runtime` son rangos
`0.23`/`0.17`) → dos máquinas pueden resolver versiones distintas de rustls, erosionando la
"paridad por construcción" **entre instalaciones**. `docs/build.md` no menciona ninguna de las
dos cosas. **Esfuerzo: 1–2 días.**

**H21. ✅ RESUELTO (17 jul 2026, port N1–N5; ver la nota "Port H21" arriba). Flecos de concurrencia
conocidos (contexto, no acción inmediata).** Deuda ya declarada en
§2.4 que la auditoría confirma: canales/Task solo de primitivos+string/bytes (`send_type`
`transpile.rs:3740`); `spawn`/`scope` solo con literal anónimo o función nombrada nularia — no un
closure en variable (`:2926-2942`); `Rc<dyn Fn>` no-`Send` bloquea `webserver`/`framework`/
`metrics_server`; guardas de `match` (`:1987`) y patrones de struct (`:2079`) fuera del
subconjunto; `signals()` solo Unix (`:897-901`). Semántica distinta documentable: scheduling con
hilos reales vs VM determinista (el oráculo CSP lo mitiga corriendo solo salidas estables,
`cli_cli.rs:225` — es decir, solo se testea el subconjunto determinista); `__ray_select` es poll
con sleep de 50µs (busy-wait); **`send` sobre canal cerrado se descarta en silencio**
(`transpile.rs:846`) donde la VM tiene semántica propia — este último sí merece fix junto a H2.
La implementación real de cancelación M12.5/`try_join` en hilos reales es el ítem más difícil de
toda la lista: **3–5 días**, diferible si H7 los rechaza en compilación.
> **Resuelto en tres tandas (la parte de CONCURRENCIA quedó cerrada):** el slice de canales
> (16 jul: rendezvous multi-emisor, `send` sobre cerrado y `close` con emisor bloqueado ≡ VM) y el
> port de scheduler N1–N5 (17 jul: contención de fallos, `try_join`, cancelación M12.5, select por
> condvar sin busy-wait, y heap/funciones cruzando hilos — `Rc<dyn Fn>`/webserver incluidos). Los
> flecos que SIGUEN siendo límites documentados del backend (§2.4), fuera del alcance de la
> concurrencia: guardas de `match` y patrones de struct fuera del subconjunto, `signals()` solo
> Unix, scheduling con hilos reales vs VM determinista (se testea el subconjunto determinista), y
> la cancelación cooperativa solo en puntos bloqueantes (la VM cancela en los yields M:1).

### 6.5 Plan de ataque sugerido (por lotes)

| Lote | Hallazgos | Esfuerzo | Resultado |
|------|-----------|----------|-----------|
| **Urgente** | H1, H2, H3, H4+H8, H5, H14, H15 | **~3 días** | Sin bugs conocidos en programas válidos; sin fugas |
| **Verificación** | H10, H11, H12 | **~2 días** | Red contra regresiones futuras |
| **Pulido** | H6, H7, H9, H17, H18 | **~2 días** | Contrato de comportamiento cerrado; mensajes según convención |
| **Estructural (opcional, priorizable pieza a pieza)** | H16, H19, H13, H20, H21 | **~2 semanas** | Desduplicación, rendimiento, error-paths, portabilidad, concurrencia plena |

Orden natural del lote urgente: H1 → H4 → H5 → H3 → H2 → H14/H15. Los tres primeros lotes
(~1 semana) cubren todo lo *necesario*; el cuarto es inversión incremental tipo arco.
