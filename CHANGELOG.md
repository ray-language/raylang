# Changelog

Todas las versiones notables de raylang. El formato sigue el espíritu de
[Keep a Changelog](https://keepachangelog.com/) y el versionado es
[SemVer](https://semver.org/) (la versión del lenguaje y la de la stdlib van juntas; ver `SPEC.md` §12).

## Sin publicar

Todo lo que ha entrado en `main` desde la 1.0.0 (jul 2026). El eje del periodo: un **tercer motor**
(el binario nativo), un salto de **rendimiento** medido arco por arco, y la capa de aplicación
(framework web, procesos del SO, herramientas de desarrollo).

### Añadido — un tercer motor

- **Compilación a binario nativo** (`ray build --native`, arco P2.b): **transpila el programa a Rust** y
  lo compila a un ejecutable de código máquina — modelo *dev = VM / deploy = nativo*. Byte-idéntico a la
  VM (verificado con un corpus de paridad) y **3–4× más rápido que ella en cargas de servicio, 28–57× en
  cómputo puro**. En el banco poliglota (29 jul 2026) **gana a node en 9 de los 10 programas de cómputo**,
  a `rustc -O` en cinco (empatando en otros dos) y a Go en cinco, y arranca en 1,80 ms — el más rápido de la mesa; en tiempo ×
  memoria queda #2 de 10 lenguajes en 10 de los 12 programas. Cubre el lenguaje completo (genéricos,
  traits, `dyn`, tuplas, closures
  con captura mutable, iteradores) + `std/fs`, sockets TCP/UDP, TLS, SQLite, procesos, FFI y toda la
  concurrencia.
- **Concurrencia nativa sobre fibras M:N** (arco F, `docs/diseno-concurrencia-nativa.md`): scheduler de
  corrutinas de pila propia (`corosensei`) + reactor `kqueue`/`epoll`, **el default** desde F-cierre
  (`--without fibers` recupera el hilo-por-tarea). Cubre sockets, TLS, UDP, `spawn`, `select`,
  cancelación de hermanas y esperas con plazo. Frente al hilo-por-conexión: de ~265 KB a ~21 KB por
  conexión y 350× menos CPU en esperas ociosas.
- **Crates de producción bajo demanda**: TLS (`rustls`), criptografía (`ring`), SQLite (`rusqlite`) y la
  regex acelerada se enlazan **solo cuando el programa los usa** (proyecto Cargo generado sobre el crate
  compartido `crates/ray-runtime`, del que también depende la VM → paridad por construcción).
  `mimalloc`, `ahash`, las fibras y los procesos van **por defecto**.
- **Control del build nativo**: `-o`, `--release` (opt3+lto+target-cpu=native), `--fast` (aritmética
  envolvente), `--target` (cross-compilation, con `Cargo.lock` reproducible) y
  `--without crypto,tls,sqlite,regex,mimalloc,ahash,fibers,process` — o `[native] without = […]` en
  `ray.toml` como política estable del proyecto.

### Añadido — lenguaje y stdlib

- **`try_call(f) -> Result<T, string>`** (M97): recuperación de un `panic`/error de ejecución **en la
  misma fibra**, el fallo como valor. En los tres motores. `try_join` hace lo propio con una tarea.
- **Cadenas plantilla con backticks** (M95): `` `…` `` es multilínea y admite `"` literal, con la misma
  interpolación que cualquier cadena.
- **`std/process`** (M100): ejecución de procesos del SO **sin shell** (argv tipado). `run` para el caso
  simple, un builder (`dir`/`env`/`stdin`/`timeout_ms`/`max_output`/`merge_output`) y **streaming** por
  canales acotados con contrapresión. `Err` solo significa "no se pudo lanzar"; el plazo devuelve la
  salida **parcial** con `timed_out`. El hijo corre en su propio grupo de procesos y es **hijo de
  scope**: una hermana que falla lo mata y lo cosecha, y uno que nadie esperó no sobrevive al scope.
- **`std/kv`** (G.1/G.2): estado clave-valor persistente en raylang puro, con un `SharedStore` por actor
  CSP — sobrevive al hot reload.
- **`std/collections/dict`** (M82): mapas con claves de **usuario** vía `Hash` + `Eq`.
- **`std/resilience`** (M88.2): reintentos con backoff y jitter, *circuit breaker* y plazos.
- **Señales del SO** (M88.1): `signals() -> Channel<int>` para apagado ordenado, y `serve_graceful` en el
  servidor web.
- **Stack trace de errores de ejecución** (M79) con la posición del llamador, en ambos motores.
- **Más superficie**: literales float con exponente (M80), `@derive(ToJson)` (M93.5), `monotonic_nanos`,
  `random.seed/between/choice/shuffle`, `crypto.random_bytes`, UUID v7, directorios y metadatos en
  `std/fs`, escapes `\uXXXX` en `std/json`, `find`/`chain`/`min`/`max` en iteradores, `clamp` genérica y
  trigonometría inversa en `std/math`.
- **Regex (M81/M59.2)**: Pike VM con grupos de captura, `{n,m}` y cuantificadores perezosos;
  `compile -> Result` y el trait `Matcher`.

### Añadido — ecosistema y aplicaciones

- **Paquete `web`** (M93): framework de aplicación estilo Express sobre el servidor HTTP/1.1 —
  enrutado (parámetros, catch-all, `mount`, rutas regex, 405 + `Allow`), middleware componible, contexto
  (`header_of`/`cookie_of`/`form`/`json_body`), presets de CORS, `Cache-Control`, trace-id en el log y
  respuestas JSON tipadas vía `ToJson`. Compila también a binario nativo.
- **Servidor web de producción** (M56): límites anti-DoS, timeouts de lectura (anti-slowloris),
  keep-alive, HTTPS de servidor, `chunked` entrante, HEAD, estáticos con saneo, `static_mount` y caché
  ETag/304, varias cookies por respuesta y apagado ordenado.
- **Templates compilados `.ray.html`** (M55): `{% %}` con composición (`import`/`include`/`extends`/
  `block`/`let`), formateador propio y soporte completo de editor (completion, hover, references,
  rename, outline). `run`/`build`/`test` regeneran solos los desactualizados.
- **`ray dev`** (M92): modo desarrollo con watcher, *check-before-restart* (un error a medio escribir no
  tira el servidor que funciona), debounce, confirmación por contenido, retención del socket entre
  reinicios y **live-reload** del navegador por SSE.
- **`ray mcp`** (IDEAS §51): servidor MCP con las tools `check`/`run`/`test`/`fmt`/`doc` para agentes
  LLM, ejecutando el código confinado (fuel + heap + plazo). Junto a `llms.txt`, el contexto destilado
  del lenguaje.
- **Registro de paquetes multi-publicador** (M83/M84/M90.1): dueños de nombre y firmas Ed25519
  (`ray registry publish --sign`, `keygen`, `verify`), UI web estática generada **en raylang**, mirrors
  (`[registry] mirror`/`RAY_MIRROR`) y `ray remove`/`ray search`.
- **Clientes de base de datos** (`packages/db`, M53–M54, M76–M77): PostgreSQL (protocolo extendido,
  TLS), MySQL (prepared statements binarios, TLS, caching_sha2), SQLite (sobre `rusqlite`) y MongoDB
  (OP_MSG, SCRAM, BSON en raylang puro, cursores).
- **Más red**: RPC raylang↔raylang (`packages/rpc`, M88.4), tracing distribuido W3C (M88.3), keep-alive
  del cliente HTTP (M90.2), `tls_upgrade` (STARTTLS de cliente), cliente SNTP (M90.7), HTTP/2 con flow
  control y `grpc-status` (M58.3), lector de tramas WebSocket robusto (M58.1).
- **Tiempo local y planificación**: `packages/tz` (IANA/TZif, incluido el footer de reglas DST
  perpetuas) y `packages/cron` (expresiones cron y timers, UTC y hora local).
- **Builds a medida**: features `sqlite`, `net-tls` y `ffi` (activas por defecto) → un build *slim*
  ocupa un 53% menos y no puede cargar código nativo; PGO del binario de release (`tools/pgo.sh`);
  `Makefile` con todos los comandos del proyecto.

### Rendimiento

Cada cifra está medida y contada en [`PERFORMANCE.md`](PERFORMANCE.md).

- **VM (arcos P0/A/D/V/MM/TA)**: `Map` sin alocar en el camino caliente (aHash, `get_or`, `add_to`),
  allocador `mimalloc`, superinstrucciones guiadas por histograma (−19 a −28% en todo el banco), PGO
  (−5 a −9%), opcode `ConcatN` (jsonserialize −27%), fusión del envoltorio `Option` (jsondeserialize
  −52/−59%), fast-paths ASCII, `s[i]` sin materializar los chars (~33× en bucles), fast-path flotante y
  fusión de indexado (matrixmul −35%), structs sin metadatos y `Slot` de 88→48 B (treealloc −15%),
  arreglos homogéneos de ints (−68% de RSS) y GC con umbral amortizado por trabajo trazado (17× en un
  `iter` perezoso de 1M).
- **Binario nativo (arcos N/R/M96/SN/F)**: `mimalloc` y aHash en el transpilado (wordcount/logparse
  −40%, −8,5% extra), `join` y `concat` sin recopia, `for` sin clonar, `std/regex` sobre el crate
  `regex` (570 → 71 ms), pool de hilos shardeado y `print` sin lock global (18k → 58k req/s antes de
  las fibras), y el reactor de fibras a **cero asignaciones por ciclo**.
- **Framework web**: **~188k req/s** de techo — 93% de axum, p50/p99.9 empatadas (0,48/1,05 ms frente a
  0,47/1,04) y 1,5× Go+chi, sirviendo con 14 hilos y ~21 KB por conexión.
- **Banco poliglota** importado al repo (`benchmarks/poly/`) y banco de **carga web** con generador
  remoto, medianas y MAD; gate de regresión de **memoria** (pico de RSS) además del de tiempo.

### Cambiado

- **Todos los mensajes que el lenguaje entrega al usuario están en inglés** (compilador, runtime,
  tooling y stdlib), incluidos los espejos del compilador auto-alojado. Los comentarios del código
  siguen en español.
- **La CLI se agrupa en subcomandos** (M99): `ray registry publish/yank/keygen/verify` y
  `ray build --templates-only` sustituyen a los comandos sueltos anteriores (la interfaz legada por
  flags se conserva).
- El código del compilador se reorganizó en módulos-directorio (`vm/`, `transpile/`, `checker/`,
  `lsp/`), documentado en `docs/organizacion-codigo.md`.

### Corregido

- Contención de fallos por tarea y `try_join` en el backend nativo; un fallo observado con `try_join`
  cuenta como manejado por el scope (M97.1).
- El almacén de tareas y el de canales liberan al consumirse/cerrarse (M98.1–M98.3): fugas de memoria en
  servicios de larga vida.
- Errores de ejecución del binario nativo con **exit 70** y mensajes idénticos a la VM (H6).
- Tope de salida anti-bomba al descomprimir (M64.2) y endurecimiento de parseo en HPACK, DNS, JWT,
  SCRAM y los clientes de BD.
- Diagnóstico dedicado para el gotcha del checker de DESIGN §55, en ambos checkers (M87).

## 1.0.0 — 2026-07-03

Primera versión estable. raylang pasó de un lenguaje de juguete (un lexer + un intérprete tree-walking) a un
lenguaje **auto-alojado**, con una VM de bytecode como motor de producto, concurrencia multicore por actores,
un ecosistema de herramientas (`ray`, gestor de paquetes, LSP, formateador, doc) y un playground en el
navegador — manteniendo la invariante de **casi cero dependencias** (solo TLS/criptografía vía `rustls`/`ring`).

### El lenguaje

- **Estáticamente tipado, orientado a expresiones** (`if`/bloques/`match` producen valor; retorno implícito),
  sintaxis de llaves. `let` inmutable / `var` mutable; **sin `null`**.
- **Errores como valores**: `Option<T>`/`Result<T,E>` + el operador `?`.
- **Tipos suma** (`enum`) y **pattern matching** exhaustivo (`match`), con guardas, `if let`, patrones
  anidados y de struct.
- **Genéricos** (funciones y tipos, con inferencia y *erasure*) y **traits** con despacho estático, *bounds*
  (paso de diccionarios), impls genéricos, métodos por defecto y **trait objects** (`dyn A + B`, con
  upcasting).
- **UFCS** (`recv.f(args)`) y **pipelines** (`x |> f(a)`); closures con captura; **inferencia local**.
- **Protocolo `Iterator`** perezoso (`map`/`filter`/`take`/`skip`/`zip`/`enumerate` + `fold`/`collect`/`sum`)
  sobre el que se re-fundan las operaciones ansiosas.
- **Datos**: arreglos `[T]`, structs, `Map<K,V>`, `Set<T>`, `Deque<T>`, `char`, `bytes`, enteros con signo/
  tamaño y operadores bit a bit.
- **Módulos** multi-archivo por directorios, con cápsulas (`mod.ray`), `pub`, `import`/`from … import`,
  re-exports y tipos por módulo.
- **Anotaciones** (`@test`, `@derive(Eq, Show, Hash)`).

### El runtime

- **VM de bytecode** (pila y marcos explícitos) como **motor de producto**, con **GC mark-and-sweep**; el
  **intérprete** queda como oráculo de validación cruzada en desarrollo.
- **TCO** (recursión de cola en O(1) de pila) en ambos motores; recursión profunda robusta.
- **Confinamiento opcional** para embeber raylang: `--fuel` (límite de instrucciones) y `--heap` (tope de
  objetos vivos).

### Concurrencia

- Modelo **CSP → actores con aislamiento de heap**: `spawn`/canales tipados (`channel`/`send`/`recv`/
  `close`/`select`), *structured concurrency* (`scope`/`join`) y cancelación.
- **Scheduler M:N multicore real** (pool de hilos; ~3,84× en 4 tareas), con `--deterministic` para salida
  reproducible. *Data-race freedom* por construcción (heap por fibra; los canales transfieren la propiedad).

### Auto-alojado (self-hosting)

- El **lexer, parser, checker, intérprete y VM de raylang están escritos en raylang** (`selfhost/`), validados
  contra el toolchain de Rust como oráculo. **Meta-circularidad**: el compilador auto-alojado se ejecuta a sí
  mismo sobre el intérprete y la VM auto-alojados.

### Ecosistema y herramientas

- **CLI `ray`**: `new/run/build/test/fmt/doc/lsp/repl/version`.
- **Gestor de paquetes**: manifiesto `ray.toml`, lockfile `ray.lock` con hashes SHA-256 (supply-chain),
  dependencias git / ruta local / transitivas.
- **stdlib `std/`** embebida (matemáticas, texto, orden, colecciones, codificación, hashing…) + un paquete
  `net` (HTTP/HTTP2, DNS, WebSocket, TLS, gRPC, Postgres, Redis, OAuth2…) como paquete adicional.
- **LSP** (diagnósticos, hover, ir-a-definición, referencias, rename, completado, signature help),
  **formateador**, **raydoc**, y clientes de editor (VSCode, Sublime, Neovim/Helix).
- **FFI** con ABI C (`extern "lib" { fn … }`, sin `libffi`).

### Seguridad y calidad

- **Compilador sin pánicos**: toda entrada → error con posición o ICE reportable; **fuzzing continuo** del
  front-end (0 crashes). Política de seguridad en `SECURITY.md`.
- **Criptografía de producción** vía `ring` (SHA/HMAC/Ed25519/AEAD); las implementaciones puras en raylang
  quedan como demostración del lenguaje.
- Casi cero dependencias de Cargo (única excepción consciente: TLS/`ring`), auditadas en CI (`cargo audit`).

### Playground web

- La VM compilada a **WebAssembly** (`wasm32`, **sin `wasm-bindgen`**) → raylang corre **en el navegador**
  (`playground/`). Alcance: lenguaje núcleo (sin red/cripto/FFI/hilos).

### Distribución

- Instalador `curl | sh` (`install.sh`) y CI de releases con binarios por plataforma (macOS, Linux, Windows).
- Licencia **MIT OR Apache-2.0**.
