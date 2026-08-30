# raylang — Backlog de features y su impacto en el diseño

> Registro de ideas que NO entran ahora pero queremos considerar a futuro. Para
> cada una anotamos: **impacto** en el diseño actual, **cuándo** podría llegar, la
> **decisión/recomendación** (si la hay) y la **restricción** que debemos respetar
> hoy para no bloquearla.
>
> Las features ya comprometidas (tipos suma, genéricos, `Result`/`?`, UFCS,
> pipelines, stdlib) viven en [DESIGN.md](DESIGN.md) §2 y §10, no aquí.

## Resumen de impacto

> ⚠️ **Esta tabla es un registro histórico de clasificación**, no el estado del proyecto. Se escribió
> "tras M26" y cada fila anota el impacto que una idea **tenía sobre el diseño de entonces**, junto con
> lo que se decidió. Casi todas están ya ✅ COMPLETO (el proyecto va por **M100**): consérvala para
> saber *por qué* algo se aceptó, se difirió o se descartó, no para saber qué existe hoy — eso está en
> [SPEC.md](SPEC.md) (lo normativo), [REFERENCE.md](REFERENCE.md) (el catálogo) y
> [CHANGELOG.md](CHANGELOG.md) (lo entregado).
>
> Las ideas **vivas** (las que aún no se han hecho) están en las secciones numeradas de más abajo, no
> en esta tabla.

| Idea | ¿Dónde pega? | Cuándo | Estado |
|------|--------------|--------|--------|
| Concurrencia (goroutines / async / suspend) | **Arquitectura de la VM** | **M12** | ✅ **COMPLETO** (DESIGN §21): **CSP sobre la VM** — green threads cooperativos M:1, canales tipados, structured concurrency; data-race freedom **vía CSP** (no ownership); scheduler determinista; intérprete = oráculo secuencial. Surface: `spawn(closure)->Task<T>`, `channel()`/`channel(n)`/`send`/`recv->Option<T>`/`close`, `join`, `scope`, `select` (builtins). Sub-fases: ✅ **M12.1** slice CSP (spawn + canales no acotados + scheduler determinista; solo VM, intérprete da error limpio; `close` ad-hoc polimórfico con el de handles; GC multi-raíz) · ✅ **M12.2** acotados/backpressure (`channel(n)`, `n≥0`; `n=0` rendezvous; `send` se vuelve punto de yield al llenarse la cola; `recv` despierta al emisor bloqueado; `VmChannel.cap`; `Waiting::Recv`/`Send(v)`; el valor del emisor aparcado es raíz del GC) · ✅ **M12.3** structured concurrency (`Task<T>`+`join(t)->T`+`scope(fn()->R)->R`; `spawn` pasa a devolver `Task<T>`; el scope posee las tareas lanzadas dentro y las une al salir; propagación del fallo de una hija vía captura en la `Task` y re-lanzado en `join`/`ScopeEnd`; estado por fibra `task`/`scopes`; GC multi-raíz; diferido: cancelación de hermanas) · ✅ **M12.4** `select(chs: [Channel<T>]) -> int` (bloquea hasta que un canal esté listo para recibir; devuelve el índice del primero listo, determinista; `recv(chs[i])` toma el valor; `Waiting::Select`, `wake_select_waiters`; solo VM) · ✅ **M12.5** cancelación de hermanas (semántica, sin superficie: al fallar una tarea del `scope`, se cancelan las hermanas pendientes —`cancel_task` recursivo: las saca de ready/parked y cancela nietos— y se propaga el fallo original; `ScopeEnd` cancela en vez de esperar; `fail_current_fiber` cancela los hijos de una fibra-hija que falla; cooperativa, no preemptiva). **M12 COMPLETO** (diferido: cancelación preemptiva, `Selected<T>` índice+valor, select de send, `cancel(t)` explícito). Diferido: algebraic effects (intérprete a pila explícita), M:N paralelo (GC thread-safe). Descartado: ownership/regiones |
| **raylang de producción** (cambio de norte) | Todo el runtime | **M33–M43** | ✅ **COMPLETO** — los cuatro arcos ejecutados: A estabilidad (spans/no-ICE/SPEC/un motor) · B rendimiento + M:N por actores · C ecosistema (`ray` + paquetes + `std/` + FFI) · D endurecimiento + 1.0. El contrato vigente que salió de ahí es **[PRODUCTION.md](PRODUCTION.md)** (crónica en DESIGN §37). De lo que quedó fuera de la 1.0: el **backend nativo** acabó haciéndose (y es el destino de despliegue); macros, effects y reflection siguen aquí sin hacer |
| Null safety | Sistema de tipos | hecho | ✅ no hay `null` (`Option<T>`, M6) |
| Introspección / reflection | Modelo de valores de la VM | post-M11 | 💤 puerta abierta (los valores cargan tipo en runtime) |
| Structs vs interfaces/**traits** | Sistema de tipos / polimorfismo | **M9** | 📌 recomendación fijada (traits estilo Rust) |
| Hot code reloading | Indirección de llamadas en la VM | tardío | 💤 acomodable |
| Visibilidad (`pub` vs mayúscula) | Sistema de módulos | **M11** | 📌 recomendación fijada (`pub` explícito) |
| **Módulos por directorios** (`import geo/formas/circulo;`) | Loader + parser de `import` | **M11.5** | ✅ separador `/` fijado; **solo leaf-binding** + `as`; prohibido el acceso por ruta en expresiones (ambiguo con `/` y mala práctica); rutas absolutas desde la raíz. Diferido: imports relativos, `pub` granular |
| **Aislamiento de módulos** (`mod.ray` = cápsula) | Loader (resolución + aristas) | **M11.6** | ✅ estrategia "cápsula": `mod.ray` vuelve un directorio direccionable (`import geo;`) y **encapsula** su subárbol; reexport `pub from … import …` (-a) + enforcement del borde (-b); descartados `internal/`-Go y `mod x;`/`pub(crate)`-Rust |
| **Redes + base moderna** (sockets / HTTP / JSON · reloj/RNG/math) | Builtins (transporte/base) + librería raylang (protocolos) | **M15** | 🚧 DESIGN §24. Dirección fijada: **transporte = builtins** (sockets TCP/UDP sobre `std::net`, cero deps, molde de handles de M11.8); **protocolos (HTTP/URL/JSON) = librería en raylang** con `import`; **carga útil = `string`** por ahora (bytes diferido); **bloqueante primero** (async sobre el scheduler de M12 = capstone M15.5). ✅ **M15.1a** matemáticas (`sqrt`/`pow`/`floor`/`ceil`/`round`/`abs`/`min`/`max`/trig/`ln`/`log10`/`exp`/`pi`/`e`; opcode parametrizado `MathF(MathFn)`; determinista → oráculo). ✅ **M15.1b** reloj/RNG (`now`/`monotonic`/`sleep`/`random`/`random_int`; PRNG **SplitMix64** propio sembrado del reloj, cero deps; no determinista → pruebas de propiedades por subproceso). ✅ **M15.2** cliente TCP (`tcp_connect`/`socket_read`/`socket_write` sobre `std::net::TcpStream`; carga útil `string`; lectura por trozos; el handle reusa el registro de archivos de M11.8 → `close` extendido a sockets; helpers clonan el stream para no retener el lock en I/O bloqueante; subproceso vs. servidor de juguete en Rust). ✅ **M15.3** servidor TCP (`tcp_listen`/`tcp_accept`/`local_port` sobre `std::net::TcpListener`; `OpenHandle::Listener` en el mismo registro; `accept` clona el listener para no retener el lock; servidor **secuencial bloqueante** —una conexión a la vez en M:1, el concurrente real es M15.5—; subproceso con el `.ray` como servidor). **Transporte TCP completo.** ✅ **M15.4a** JSON **como librería en raylang** (`examples/web/json.ray`: `parse`/`stringify` de descenso recursivo; objetos = `Map<string,Json>` → salida canónica con claves ordenadas; errores como `Result`; **cero runtime**, puro front-end + stdlib). Materializa "protocolos/libs en el propio lenguaje". Limitación: escapes `\uXXXX` no soportados (pediría un builtin code-point→char). Probado por subproceso (golden) en ambos motores. ✅ **M15.4b** HTTP **como librería en raylang** (`examples/web/http.ray`: `fetch`/`request`/`header` + parseo de URL y de respuesta, sobre los builtins TCP de M15.2; solo `http://`, `Connection: close` + leer-hasta-EOF; cabeceras en `Map` con clave en minúsculas; **cero runtime**). Atajo `fetch` (no `get`: chocaría con el `get` de Map, raylang no tiene sobrecarga). **Compone con `json`** (un GET cuyo cuerpo se parsea con la librería JSON) → showcase de librerías de raylang componiéndose. Probado vs. servidor HTTP de juguete en Rust, ambos motores. **M15.4 (protocolos en raylang) COMPLETO.** ✅ **M15.5** (capstone) sockets no bloqueantes integrados con el scheduler de M12: `tcp_accept`/`socket_read` **ceden la fibra** (la VM voltea sus sockets a no bloqueantes; en `WouldBlock` aparca en `io_parked` y el scheduler hace **busy-poll cooperativo** —duerme ~1 ms y re-encola— cuando nadie está listo; cero deps, sin `epoll`). Reusa los opcodes `SocketRead`/`TcpAccept` (solo cambia su ejecución en la VM); GC rootea `io_parked`; `cancel_task` también. El intérprete sigue con sockets bloqueantes (un hilo). Con `spawn` → **servidor concurrente** sobre un hilo (test de ordenación: el 2.º cliente recibe su eco antes de que el 1.º envíe). **Solo VM.** Diferidos de M15.5 ya resueltos: `epoll`/`kqueue` (M17), `bytes` (M16), TLS (M19.4), y la **cesión en `socket_write`** (post-M19: escritura parcial no bloqueante + aparcado por interés de **escritura** en el poller; ya no gira). **M15 COMPLETO.** Sin gestor de paquetes (las "libs externas" son archivos/cápsulas del proyecto) |
| **Tipo `bytes`** (datos binarios) | Nuevo tipo en todo el pipeline (como `char`) + I/O binaria | **M16** | 🚧 DESIGN §25. Secuencia **inmutable** de octetos, hermano de `string` (inline en la VM, `Rc<Vec<u8>>` en el intérprete; no toca el GC). Cierra la deuda de M15 (carga útil binaria correcta) y cimenta TLS (M17) y el backend nativo (M18). ✅ **M16.1a** el tipo: `Type::Bytes` + keyword `bytes`; literal **`b"..."`** con escapes de string + **`\xNN`** (octeto arbitrario); `len(bytes)`, indexar `b[i] -> int`, `==` estructural; oráculo (incl. UTF-8 multibyte y bytes nulos). `print(bytes)`/`to_string(bytes)` → hex (post-M19, `bytes_to_hex`; oráculo). ✅ **M16.1b** string-interop: `to_bytes(s) -> bytes` (codifica UTF-8, builtin), `from_utf8(b) -> Result<string,string>` (decodifica; primitivo `__from_utf8` etiquetado + envoltorio en el prelude) y concatenación `b1 + b2` (se extiende `Add` en checker + ambos motores); oráculo (round-trip, UTF-8 inválido → Err). ✅ **M16.1c** I/O binaria: `read_file_bytes`/`write_file_bytes` (disco) y `socket_read_bytes`/`socket_write_bytes` (red); cierra la deuda binaria de M15 (octetos crudos intactos, incl. `\x00`/`\xff`). Gotcha: el arreglo `[T]` es homogéneo → las **lecturas** devuelven `[bytes]` con el tag también en bytes (`[b"ok", datos]`/`[b"err", msg_utf8]`); las escrituras siguen con `[string]`. `socket_read_bytes` cede al scheduler como `socket_read` (M15.5). Probado por subproceso, ambos motores; ejemplo `binario.ray`. **M16 COMPLETO.** ✅ **`bytes` como clave de Map** (post-M19: `MapKey::Bytes`, hashable/Ord como un string; oráculo). Diferido: mutabilidad |
| **`epoll`/`kqueue`** (readiness real de E/S) | Poller del SO en `src/poll.rs` (FFI propio) + scheduler de la VM | **M17** | ✅ DESIGN §26. Sustituye el **busy-poll de M15.5** (dormir 1 ms + re-encolar todas) por **notificación de readiness del SO**: el scheduler se **bloquea** hasta que algún socket esté listo y despierta **solo** las fibras de esos fds. **Invariante cero-deps mantenida**: en vez del crate `libc`, se declaran los `extern "C"` (`kqueue`/`kevent` macOS/BSD, `epoll_*` Linux, `close`) — viven en libSystem/libc, siempre enlazados; `unsafe` **acotado** a `src/poll.rs`. Los fds salen de `std` (`AsRawFd`). `io_parked` pasa a llevar el `fd` por fibra. **Fallback honesto** al busy-poll en plataformas sin poller (Windows) o EINTR → siempre hay progreso. Bloqueo infinito (sin timeout): esperar a la red es correcto y no quema CPU. **Cero cambios observables** → la garantía es la **regresión** (tests de red concurrente verdes, ahora vía `kqueue` real). Solo VM. ✅ cesión en `socket_write` (post-M19: escritura parcial + aparcado por interés de escritura en el poller —`wait(read_fds, write_fds)`—). Diferido: registro persistente del poller, edge-triggered, `bytes`/bitops en el toolchain auto-alojado |
| **La capa web** (servidor HTTP async + SSE · HTTP en bytes · WebSockets `ws://` · TLS) | Librerías raylang sobre sockets/scheduler (cero runtime) · cómputo cripto vs. cero-deps | **M19** | 🚧 DESIGN §28. Construye la capa de aplicación sobre el transporte (M15) + concurrencia (M15.5/M17) + `bytes` (M16). Filosofía de M15: protocolos = librería en raylang. ✅ **M19.1** servidor web async + SSE (`webserver.ray`: `Request`/`Response`, `read_request`/`send_response`, `serve`/`serve_raw` concurrentes, SSE vía `text/event-stream` — cero runtime nuevo) · ✅ **M19.2** HTTP en `bytes` (builtin **`sub_bytes`** —único toque de runtime— + cliente/servidor con cuerpo `bytes`, cabeceras texto; `bytes_response`/`body_text`/`request_text`; round-trip binario `\x00`/`\xff` intacto) · ✅ **M19.3** WebSockets `ws://` **COMPLETO**: M19.3a operadores **bit a bit** `& | ^ ~ << >>` (único toque de lenguaje; precedencia C; `>>` partido en el parser para genéricos anidados; oráculo) + M19.3b **SHA-1+base64 en raylang** (cero runtime; vectores RFC 3174/4648/6455) + M19.3c handshake/framing/echo server (`websocket.ray`/`websocket_echo.ray`; builtin `bytes_of`; e2e en el test) · 🚧 **M19.4** TLS/SSL — **DECIDIDO con el usuario: excepción de cero-deps con `rustls`** (1.ª dependencia de Cargo; excepción consciente y acotada al dominio TLS, donde "hazlo a mano" es irresponsable). Sub-fases: ✅ **M19.4a** cliente TLS bloqueante + `https://` (`tls_connect` + `OpenHandle::Tls` en el registro de sockets → `socket_read_bytes`/`socket_write_bytes`/`close` desvían a TLS, `http.ray` habla https transparente; `webpki-roots` + `SSL_CERT_FILE`; test determinista con servidor TLS local en `tests/tls_cli.rs`); ✅ **M19.4b** servidor TLS + `wss://` (`tls_accept`; rustls conducido a mano sobre el enum `Connection`, **integrado con el scheduler no bloqueante** —aparca la fibra al bloquear leyendo, mismo `io_parked`/poller de M15.5/M17—; la misma bomba sirve a ambos motores —sobre socket bloqueante `read_tls` bloquea—; `wss_echo.ray`; e2e en `tests/tls_cli.rs`). **M19.4 + M19 COMPLETOS.** Único punto donde se rompe cero-deps: TLS (excepción consciente) |
| **Cripto, identidad y clientes cloud** (SHA-256/HMAC · JWT/UUID · URL/cookies · tiempo · Redis · HTTP robusto · UDP) | Librerías raylang sobre M19 (cero runtime) salvo UDP (3 builtins) | **M20** | 🚧 DESIGN §29. La capa que un servicio cloud/distribuido necesita; filosofía M15/M19 (protocolos = librería raylang). Librerías cripto = cómputo puro determinista → verificadas contra vectores estándar (NIST/RFC/openssl/Python) por ambos motores. ✅ **M20.1** SHA-256 (`sha256.ray`, gemelo de SHA-1) · ✅ **M20.2** HMAC-SHA256 + base64url + hex (`hmac.ray`/`hex.ray`; RFC 2104/4648) · ✅ **M20.3** JWT HS256 + UUID v4 (`jwt.ray`/`uuid.ray`; verify en tiempo casi constante; JWT idéntico byte a byte a la referencia) · ✅ **M20.4** URL percent-encoding + query + cookies (`url.ray`/`cookie.ray`; setters `with_*` por UFCS cross-module) · ✅ **M20.5** fechas/horas UTC (`time.ray`; algoritmo de Hinnant, ISO 8601/RFC 1123/duraciones; cero runtime, lo cubre el oráculo de self-hosting) · ✅ **M20.6** cliente Redis RESP2 (`redis.ray`; e2e vs. servidor de juguete) · ✅ **M20.7** HTTP robusto (`request_with`/`fetch_follow`/chunked en `http.ray`) · ✅ **M20.8** UDP (único toque de runtime: `OpenHandle::Udp` + 3 builtins/opcodes en ambos motores; `udp.ray` con `Packet{host,port,data}`) · ✅ **M20.9** AWS Signature V4 (`sigv4.ray`; vector oficial get-vanilla) · ✅ **M20.10** gzip/deflate: descompresor DEFLATE/INFLATE en raylang puro (`inflate.ray`, port de puff.c; 3 tipos de bloque + CRC-32) + integración `Content-Encoding: gzip` en `http.ray` · ✅ **M20.11** cesión cooperativa de `udp_recv_from` en la VM (aparca la fibra en el fd, como TCP) · ✅ **M20.12** encoder DEFLATE (`deflate.ray`; LZ77 con cadenas de hash + Huffman fijo; gzip/zlib; verificado por round-trip con inflate.ray + compatibilidad con Python). **M20 COMPLETO** |
| **Cliente DNS** (resolución sobre UDP) | Librería raylang sobre los sockets UDP de M20 | **M22** | ✅ DESIGN §31. `dns.ray` (RFC 1035): resuelve **A/AAAA/MX/CNAME/TXT** por UDP; `query`/`query_a`/`query_aaaa`/`query_mx`/`query_cname`/`query_txt` → `enum Record`. La pieza difícil = **compresión de nombres** (punteros `0xC0`; `read_name` los sigue y devuelve la posición siguiente; también en el RDATA de MX/CNAME) + IPv6 canónica con `::` (RFC 5952) + character-strings de TXT. Verificado e2e contra un servidor DNS de juguete (los 5 tipos según QTYPE, con compresión) por ambos motores + comprobado contra DNS real (8.8.8.8). Diferido: SOA/PTR, TCP fallback, caché por TTL |
| **Observabilidad** (logging estructurado + métricas Prometheus) | Librerías raylang puras (cero runtime) | **M21** | ✅ DESIGN §30. ✅ **M21.1** logging estructurado en JSON (`log.ray`; niveles, campos tipados, escapado, builder encadenable por UFCS; verificado con Python `json.loads`) · ✅ **M21.2** métricas Prometheus (`metrics.ray`; counters/gauges/histogramas con labels, formato de exposición de texto; verificado por validación estructural en Python) · ✅ **M21.3** endpoint `/metrics` real (`metrics_server_demo.ray`; monta metrics sobre webserver, Registry compartido capturado en el handler, escrapeable por Prometheus) · ✅ **M21.4** histogramas con labels (familia de series por conjunto de labels, `le` fusionado, creación canónica en la 1.ª observación; verificado por cumulatividad por grupo en Python). Las libs puras las cubre el oráculo de self-hosting. **M21 COMPLETO** |
| **Habilitadores de self-hosting** (`Map<K,V>`, `assert`/test, recursión profunda) | Runtime + GC (Map) · runner (test) · hilo/límites (recursión) | **M13** | ✅ **completo** (DESIGN §22): **M13.1** `Map<K,V>` heap obj en ambos motores · **M13.2** `panic`/`assert`/`assert_eq` + runner aislado por prueba (`@test` unit/bool, filtro) · **M13.3** pila grande (hilo worker) + límite de marcos con error limpio + **TCO en ambos motores** (no quedó diferido). Genérica vía `Hash` sigue diferida |
| **Self-hosting** (raylang en raylang) | Capstone: lexer/parser/checker/intérprete/loader en raylang | **M14** | ✅ **LOGRADO — meta-circularidad** (DESIGN §23): el compilador entero escrito en raylang corre **sobre el intérprete auto-alojado** (lex/parse/check + run-on-run idénticos a Rust). Decisiones: intérprete (no VM), checker = validador, resolución en runtime (= *erasure* gratis). Oráculo Rust (texto canónico para front-end, conductual para back-end) |
| **VM auto-alojada** (compilador→bytecode + VM en raylang) | Back-end alternativo en raylang | **M14.5** (opcional) | 💤 diferido: el M2 de este módulo. El intérprete auto-alojado es el oráculo, igual que en Rust M1→M2 |
| **Tooling de editor** (coloreado / LSP) | Front-end (reutiliza el checker) | **M10** | ✅ coloreado (VSCode/Sublime) + **LSP completo**: diagnósticos, hover/def, find-references, rename, completion (de archivo + **de miembros** `recv.`, M45), signature help (M10.2b–f). Clientes VSCode/Sublime/Neovim/Helix |
| **Anotaciones** (`@test`, `@derive`, …) | Parser + fase que las consume | **M10** | ✅ conjunto cerrado: `@test` + runner, `@derive(Eq, Show)` (genera el `impl`). `@delegate`/macros de usuario → diferidos |
| **`@derive(ToJson)`** (serialización JSON derivada) | Checker (`generate_derives`, como Eq/Show) | **M93.5** | ✅ **HECHO** (structs no genéricos; campos primitivos/ToJson anidados; enums/arrays/Map diferidos con error claro; trait en `std/json` desde M93.4 + builder `obj().field(...)`). Extensiones pendientes a demanda: enums (representación por decidir), `[T]`/`Map`/`Option` como campo (pedirían impls genéricos `impl<T: ToJson> ToJson for [T]`) |
| **Nativo: pool de hilos para `__ray_spawn`** | Preámbulo del transpilador (runtime de concurrencia) | **M96** | ✅ **HECHO** (DESIGN §87): thread-cache creciente (pool fijo = deadlock con fibras de conexión bloqueadas) con protocolo sin pérdida y reset del estado thread-local por tarea. Medido (`wrk -t4 -c500`, framework `/yo`): 18,0k → **58,3k req/s (3.24×)**, p50 14.9→2.4 ms, p99 1.24s→231 ms, timeouts 172→0. Semántica intacta (cli_cli 88/88, corpus nativo byte-idéntico) |
| **Nativo: cola p99 bajo alta concurrencia** | — | **RESUELTA (M96b) / metodología** | ✅ causa cazada con trazas de latencia por lock: `try_clone()` = syscall `dup()` DENTRO de la sección crítica del registro (158 stalls de hasta 112 ms en 12 s → convoyes). Fix: `Arc<TcpStream>` (clonar = ns) → mutexwait 23k→3k muestras, **94.4k→107.1k req/s (+13.5%)**, p50 235 µs. La cola restante bajo wrk es SATURACIÓN (open-loop a plena CPU = mide profundidad de cola); para SLO real, medir a tasa fija (wrk2 -R). Secuela anotada: el MISMO patrón try_clone-bajo-lock vive en la VM (`builtins.rs::socket_clone`) — aplicar Arc allí cuando toque perf de VM |
| **Framework: coste por petición** (logging, no el rebuild) | net/log + std/time (formateo/stdout por petición) | **investigada (M96b), reorientada** | 📌 la teoría original (rebuild de App + regex por petición) resultó FALSA al descomponer (18 jul): sin `GET_re` = 61.5k (≈igual que completo 62.4k); sin `log_requests` = **80.8k** → el LOGGING es el 31% del gap (now_utc + to_iso8601 + JSON + print-con-lock POR PETICIÓN); el rebuild+enrutado solo ~14% (80.8k vs 94.4k pelado). Palancas: cachear el timestamp formateado (granularidad 1 s), buffer de stdout, emit más barato. El handler-por-conexión (catch_unwind) baja de prioridad |
| **Backticks** (`` `…${expr}…` ``: string sin escapar `"`) | Lexer + fmt + gramáticas de editores | **M95** | ✅ **HECHO** (jul 2026): delimitador alterno donde `"` es literal, multilínea, misma interpolación `${}` (M27.3), mismo token `Str`/`InterpStr` (el resto del pipeline ni se entera); el fmt preserva el delimitador oliendo el fuente; demo `examples/basics/plantillas.ray` (byte-idéntico VM↔nativo). Diferido: el espejo del lexer auto-alojado (mismo estado que la interpolación M27.3 — el corpus los excluye) |
| **API de runtime / I/O** (`args`, `input`, `env`) | Builtins / stdlib | **M11** | ✅ `args`/`input`/`read_int`/`env`/`eprint` + I/O de archivos (`read_file`/`write_file`/`exists`/`append_file`/handles con buffering). `main` sin parámetros |
| **stdlib** (orden superior / string / I/O / arreglos) | prelude + builtins | **M7/M11** | ✅ `map`/`filter`/`fold` (M7.3) + string completa (M11.1/4/7a) + arreglos (`+`/`reverse`/`pop`/`contains`/`position`, M11.7b) + `sort`+`Ord` (M11.7d). Registro único de builtins (L1) |
| **stdlib importable** (math/tiempo/cripto → módulos `std/…`) | Contenido: builtins → `std/*.ray` (cero maquinaria) | **M49** | 📌 **PLAN FIJADO** ([docs/M49-stdlib-importable.md](docs/M49-stdlib-importable.md)). Continúa M48 (descongestionar el namespace de **valores**): saca las familias matemática/tiempo/cripto del global a módulos importables (`import std/math; math.sqrt(x)`), dejando globales solo lo universal (`print`/`panic`/`assert`) y **core la concurrencia** (atada al modelo de ejecución). **Cero maquinaria nueva**: reusa la std **embebida** (M40.5, `src/stdlib.rs`+`include_str!`) + el patrón `__x`+envoltorio (como la I/O). **Empieza por `std/math`** (mayor liberación de nombres —`min`/`max`/`abs`/`round`— + ya está a medias). Decisiones recomendadas (a confirmar): `min`/`max` genéricos sobre `Ord` y `abs` sobre un trait nuevo `Signed` → **puros en raylang, sin opcode** (poda `Abs`/`Min`/`Max`); `pi`/`e` → `const PI`/`const E` (poda `Pi`/`E`); `random`/`random_int` → `std/random` (no deterministas, aparte del `math` puro); corte en seco con el **reescritor AST** de M48.4e (+ auto-`import`). Sub-fases: 49.1 `std/math` (a: float; b: abs/min/max+consts) · 49.2 `std/time`+`std/random` · 49.3 `std/crypto`. Verificación: oráculo (deterministas) + subproceso (RNG/tiempo). **Restricción hoy**: no bloquea nada (el embedding + wrappers ya existen; la migración es mecánica) |
| **stdlib importable II** (fs/collections/net → `std/…`) | Contenido: prelude/builtins → `std/*.ray` (molde M49) | **M50** | 📌 **PLAN FIJADO** ([docs/M50-stdlib-fs-collections-net.md](docs/M50-stdlib-fs-collections-net.md)). Cierra la descongestión del namespace de valores (tras M48/M49): saca del prelude global los 3 grupos grandes que quedan → **`std/fs`** (read_file/write_file/…/open/exists; disco = opt-in, *capability hint*), **`std/collections`** (Set/Deque/StringBuilder, puras), **`std/net`** (tcp/tls/socket/udp). Se quedan globales los esenciales (`Option`/`Result`/`?`, map/filter/fold, print/eprint/panic/assert) + `close` (ad-hoc); stdin/`env` a decisión aparte. Mecanismo M49 (`__x`+envoltorio, migración dirigida por errores). Alcance tratable: collections ~2 archivos, net ~15 (ninguno embebido usa red), fs moderado. Collections en **submódulos** `std/collections/set`·`deque`·`stringbuilder` (leaf-binding M11.5: `import std/collections/set; set.new()` — agrupa Y sin prefijo redundante; sin maquinaria nueva). Verificación: oráculo (collections) + subproceso (fs/net). Sub-fases 50.1 fs · 50.2 collections · 50.3 net |
| **Ecosistema de paquetes** (registro central + política de tiers) | CLI (`ray add`/`publish`) + índice git + gobernanza de `std/` vs paquetes | **M51** | 📌 **DISEÑO FIJADO** (DESIGN §53 política de tiers, §54 registro). Dos piezas: (a) **política de tiers** (gobernanza, ya explícita): `std/` embebida (universal/ligera/estable) vs paquetes `packages/*` (nicho/pesado/API propia) vs `examples/` (demos); criterios de colocación + pipeline de promoción `examples/→std|paquete`. (b) **Registro central** = cierra la brecha nº1 de PRODUCTION.md ("flexible en el lenguaje, ❌ en el ecosistema"): **índice respaldado por git** (repo `nombre → git URL + versiones + hash`, sin servidor propio, reusa toda la maquinaria de M39c: cápsula/lock/transitivas/MVS), `ray.toml` **por nombre** (`foo = "1.2.0"`), `ray add`/`ray publish`/`ray yank`. Prereq: **rangos semver de verdad** (diferido de M39c). Fases: ✅ **51a** leer índice+`ray add`+rangos semver (`src/index.rs`: `VersionReq` exacta/caret/tilde/`*`, lector `<index>/<name>.toml`, `resolve`/`latest`; `deps::ensure` resuelve por nombre vía índice y delega en git+lock; `ray add` con `manifest::upsert_dependency`; índice por `RAY_INDEX`/`[registry] index`; tests offline `tests/registry_cli.rs`) · ✅ **51b** `ray publish` (valida name+version+parseo · `deps::hash_package` · `index::append_version` inmutable · spec git de `--repo` o derivada de `origin`+tag `v<ver>`; tests offline con bare repo) · ✅ **51c** índice remoto por git (clonado/cacheado en `.ray-deps/.index`) + lock-pinning (reproducibilidad de caret) + `ray update` (re-resuelve + `git pull`) + `ray yank`/`--undo`. **M51 COMPLETO** (tests offline en `tests/registry_cli.rs`, 11 casos; cero runtime). Diferido: UI/búsqueda web, firmas de publicación, mirrors, namespaces con dueño |
| **Identificadores en inglés** (deuda: nombres mezclados es/en) | Rename transversal (Rust `src/` + core raylang) | **diferida (tras M49)** | 📌 **REGLA FIJADA + PLAN** ([docs/limpieza-nombres-en-ingles.md](docs/limpieza-nombres-en-ingles.md), regla en CLAUDE.md § Convenciones). Los **identificadores** (funciones/métodos/variables/params/tipos/campos) deben ir en **inglés**; comentarios/`///` en español. El código antiguo mezcla ambos (`cargar`/`analizar`/`nombre_fachada`/`receptor`/`otro`…). Tres tiers por riesgo: **A** Rust `src/` interno (~66+ fns + vars, NO rompe) · **B** core raylang interno (selfhost/prelude/std vars privadas, NO rompe; `std/` ya casi todo inglés) · **C** ⚠️ **INCOMPATIBLE**: los métodos de trait user-facing `Eq.igual`/`Show.mostrar`/`Ord.menor` → inglés (toca cada `impl`+llamada del corpus + `@derive` + reescritor AST + self-hosted + docs; fase aparte, la última). Código **nuevo ya en inglés**. Se hace **tras cerrar los puntos pendientes** (M49.2/49.3). Verificación: suite completa (A/B) + oráculo/self-hosting byte-idéntico (C) |
| **Optimización de la VM** | `bytecode`/`compiler`/`vm` | transversal **(activo)** | 🚧 DESIGN §27, registro medido en §11. Foco tras **aparcar M18** (backend nativo) por decisión del usuario. Principio: **incremental y midiendo** — banco `benchmarks/` (`bench.sh`+hyperfine o `measure.py` sin deps) y se conserva solo lo que supera el ruido (~3–5 %), oráculo VM↔intérprete intacto. Opt.1/Opt.2 ✅ (pase previo); Opt.3 `Rc<str>` ❌. ✅ **Opt.4** fast-path entero en ops binarias (fib −5 %, bucle −6 %); ✅ **Opt.7** posición `(línea,col)` perezosa con `pos!()` (quita la lectura de `lines[ip]` por instrucción del camino caliente → **fib −7 %, loop −9 %, arrays −8 %**, consistente; señal destapada con mejor-de-15); Opt.5 (`new_locals`)/Opt.6 (safepoint GC)/Opt.8 (`children()` con buffer reusado, dentro del ruido incluso con `gcnested.ray`)/LTO ❌ descartados. ✅ **Opt.9** dedup de constantes (M29.3, por memoria) · ❌ **Opt.10** OpCode 24 B / `HeapValue` 16 B (medido sin efecto: no limitan el fetch) · ✅ **Opt.11** arranque 3.9→3.0 ms · ✅ **Opt.12** plegado de constantes (jul 2026, neutro; por calidad) · ✅ **Opt.13** umbral del GC amortizado por trabajo trazado (jul 2026: `iter` 1M **6.8 s → 0.4 s, 17×**) |
| **Backend nativo** (bootstrap sin Rust) | codegen a máquina/asm/C/Rust | **M18** | 💤 **aparcado** (decisión del usuario, 2026-06): no perseguir lo nativo/sin-toolchain por ahora; el esfuerzo va a la optimización de la VM. Opciones barajadas: asm (as+ld), máquina directa, C, transpilar a Rust→rustc. Se retoma más adelante |
| **Asperezas de M3** | Parser + checker | hecho | ✅ `[]` en campo de struct (M6.2) y coma final en arreglos (limpieza) resueltos |
| **Ergonomía del lenguaje I** (tuplas · `for`/iteradores · interpolación · casts · `const`) | Lexer + parser + checker + ambos motores + self-hosting | **M27** | 🚧 DESIGN §36. La deuda ergonómica que destaparon las librerías M15–M26. ✅ **M27.1** tuplas (`Type::Tuple`, `t.0`, `let (a,b)=…`; erasure a arreglos) · ✅ **M27.2** `for`/iteradores (rango `a..b`, arreglo, string→char, `Map`→tupla `(k,v)`; `StmtKind::For` ejecutado directamente en ambos motores); **M27.3** interpolación `"…{expr}…"` (desugar a `+ to_string`, puro léxico); **M27.4** casts `x as int`/`as float` (reusa `as`); **M27.5** `const` de nivel superior |
| **Ergonomía del lenguaje II** (operadores · `?`+From/Into · enteros con tamaño) | Sistema de tipos / traits + modelo numérico | **M28** | ✅ **COMPLETO** (DESIGN §36: 28.1 traits `Add`/`Sub`/`Mul`/`Div`/`Neg` en el prelude, el checker baja `a + b` a `a.add(b)`; 28.2 `?` convierte el error vía `impl From<E1> for E2` —método `convert` (renombrado de `desde` en la limpieza ES→EN; `from` es keyword)—; 28.3 `u8`/`u32`/`u64` con wrapping, casts `as` y literal polimórfico, oráculo `uint_literal_oraculo`). Esta fila decía 🚧 por desactualización, detectada en la auditoría de diferidos de jul 2026. **M28.1** sobrecarga de operadores vía traits (`Add`/`Ord`/`PartialEq`…; hoy *special-cased*; puede unificar `@derive(Eq)`); **M28.2** `?` con conversión de error (`From`/`Into` → enums de error propios en vez de `string`); **M28.3** enteros con tamaño/unsigned (`u8`/`u32`/`u64`; el más invasivo; mata el `& 0xFFFFFFFF` de la cripto; puede quedar acotado sin promoción implícita) |
| **Tooling** (regex · formateador · optimización VM) | Motor propio / cliente externo (reusa parser) / VM | **M29** | 🚧 DESIGN §36. **M29.1** regex (ausencia más llamativa de la stdlib; motor Thompson NFA, librería raylang o builtin-asistido); **M29.2** formateador `rayfmt` (pretty-printer canónico del AST, idempotente, sin config); **M29.3** retomar optimización VM (§27: dedup constantes, peephole, `HeapValue` 32→16 B) |
| **Cripto avanzada** (cifrado + firma asimétrica) | Librería raylang (cómputo) | **M30** | 🚧 DESIGN §36. Hoy hay hashing/HMAC pero **no cifrado**. **M30.1** simétrica ChaCha20-Poly1305/AES-GCM (vectores RFC 8439); **M30.2** asimétrica Ed25519 (RFC 8032; ejercita bignum/`u64`); **M30.3** JWT RS256/ES256 sobre lo anterior |
| **Cerrar gRPC** (transporte HTTP/2 vivo) | Librería raylang sobre TLS+ALPN | **M31** | 🚧 DESIGN §36. Los diferidos grandes de M26. **M31.1** HPACK-Huffman (tabla 257 del RFC 7541 Ap. B; vectores C.4/C.6); **M31.2** transporte vivo (preface + SETTINGS + streams sobre TLS con ALPN `h2` — requiere exponer ALPN en `tls_connect`); **M31.3** cliente gRPC e2e |
| **Clientes y formatos** (PostgreSQL · TOML/CSV · plantillas) | Librería raylang | **M32** | 🚧 DESIGN §36. **M32.1** cliente PostgreSQL (protocolo wire + SCRAM-SHA-256, reusa M20); **M32.2** TOML/YAML/CSV; **M32.3** motor de plantillas HTML sobre M27 |
| **Webserver de producción** (límites/timeouts · query · TLS · keep-alive · cookies · estáticos) | Librería `packages/net/webserver.ray` (casi todo) + 2 toques de runtime aditivos (deadline de E/S en el scheduler, `try_join`) | **M56** | ✅ **COMPLETO** (DESIGN §60, detalle en §17 abajo): 56.1 frontera de seguridad (Limits: cabeceras/cuerpo/conexiones vía semáforo `Channel.bounded`) · 56.2 query string separada + percent-decoding (`std/url.percent_decode`) · 56.3 `serve_tls` · 56.4 timeouts de lectura (`net.set_read_timeout`, deadline en `io_parked`) · 56.5 `try_join` + panic del handler→500 sin fugas · 56.6 keep-alive HTTP/1.1 (el framework se sube gratis) · 56.7 `set_cookie: [string]`+`with_cookie` (decisión con el usuario) · 56.8 chunked entrante + `static_response` con saneo + HEAD sin cuerpo · 56.9 estáticos de producción (`static_mount(prefix, dir, req)`: prefijo de URL + 405 + **ETag/304** por tamaño+mtime —builtin `fs.mtime` nuevo—; `mime_of` pub con ~26 tipos). Diferidos menores en DESIGN §60 (de estáticos: `Range`/206, streaming por trozos, precomprimidos `.gz`) |
| **Framework web `packages/web`** (estilo Express, promoción de examples) | Librería raylang pura sobre `net/webserver` + `net/log` | **M93** | ✅ **COMPLETO** (DESIGN §85, rama `feature/packages-web`): promovido de `examples/web/framework.ray` y re-basado en el webserver de producción; API express-parity — `static_files` (M56.9, ETag/304), `not_found` custom, `PATCH`+`route` genérico, `header`/`html`/`redirect`, `log_requests` (JSON por petición), `listen_tls`/`listen_graceful`/`listen_limits`. Consumidores: `examples/web/framework/` (proyecto con ray.toml) + `tests/framework_cli.rs` sobre el paquete; guía en `docs/web-framework.md`. Diferido: middleware por-ruta/grupos, body parsers tipados, sesiones/CSRF |
| **Memoria** (fuga del almacén de tareas VM + trampa de paridad del pool nativo + coste 32 B/elem) | VM (`vm.rs`/`gc.rs`: free-list de tasks/channels) + runtime nativo (`transpile.rs`: pool) + banco | **M98** | 📌 **PLAN FIJADO** ([docs/investigacion-uso-de-memoria.md](docs/investigacion-uso-de-memoria.md) §8, investigación 20 jul 2026). Huella de arranque = la mejor de la mesa (nativo 1.5 MB, VM 6.8 vs node 49/php 26/python 15); **dos bugs de producción**: (1) la VM **fuga ~1 KB/request** (tasks/channels nunca liberan; `Done(v)` retenido → webserver 924 MB en 30 s) · (2) el nativo **crashea** con churn secuencial de tareas (trampa de paridad del round-robin M96e → hilo nuevo por spawn → EAGAIN). Fases: ✅ **98.1** join consume + free-list con generación + `SpawnDiscard` (VM; task_churn 123.8→6.9 MB, webserver 924 MB→31 MB plano; espec DESIGN §21.7) · ✅ **98.2** sondear shards antes de crear hilo (nativo; churn 20k de crash→2.1 MB) · ✅ **98.3** canales ídem (liberados al cerrar+drenar; stale ≡ cerrado+vacío, cero semántica nueva; chan_churn 45.4→7.0 MB) · ✅ **98.4** RSS en el banco (gate de memoria en regress.py: ru_maxrss vía os.wait4, umbral 15%, micros task_churn/chan_churn/arr_while commiteados; ruido medido ±0.0%) · ✅ **98.5** `Obj::IntArray` (storage strategy con degradación a genérico; arr_1M 69→22.1 MB −68%, iter −73%, CPU neutra-o-mejor en A/B de misma sesión; HeapValue-16B descartado sin re-medir). **ARCO M98 COMPLETO** |
| **Concurrencia del webserver NATIVO: un hilo de SO por conexión** | Runtime nativo (`transpile/`): llevarle el `poll.rs` de la VM (kqueue/epoll) o equivalente | **sin hito** | 📌 **MEDIDO, decisión de diseño PENDIENTE** ([docs/investigacion-p999-webserver-nativo.md](docs/investigacion-p999-webserver-nativo.md) §6.1-6.2, 27 jul 2026). El mismo fuente raylang corre sobre DOS modelos: la VM usa fibras M:N con readiness (`src/poll.rs`), el nativo **un hilo de SO bloqueante por conexión**. Barrido de concurrencia con generador remoto: raylang **166.7k→159.2k→154.3k→145.2k rps** de `-c 100` a `-c 1000` (**−13 %**, el único que DEGRADA; Go pasa de 119.6k a ~137k y se mantiene, hyper plano ≥197k). Sigue ganando a Go en throughput y p50 en TODAS las concurrencias, así que **el problema no es velocidad**. Lo que decide es la MEMORIA: a `-c 1000`, **1002 hilos y 268 MB** contra los **17 hilos y 49 MB** de Go (5.5×), creciendo lineal a ~265 KB/conexión (29 MB a `-c 100`). Eso es un **muro, no una pendiente**: 10 000 conexiones pedirían 10 000 hilos y ~2.6 GB solo en pilas, y el SO se niega antes — Go y Rust lo cruzan sin enterarse. Duele además en el activo que el proyecto presume (arco M98, huella de arranque la mejor de la mesa). Opciones por coherencia: (1) **llevar `src/poll.rs` al runtime nativo** — reusa código probado, cero deps nuevas; (2) event loop a mano con sockets no bloqueantes — (1) sin reusar lo que existe; (3) tokio — mete un segundo scheduler junto a `__RAY_POOL` y los actores de heap aislado (mismo argumento que descartó hyper). **Composición de la memoria MEDIDA (28 jul, §6.1b)**: la pila por hilo NO es (2 MiB/512 KiB/128 KiB dan los mismos 268 MB — se reserva virtual y solo cuentan páginas tocadas; hipótesis FALSADA, sin dejar código); **mimalloc explica ~128 KB de los 265** (una arena por hilo: sin él, 172 MB en vez de 268 a `-c 1000`), y los otros ~137 KB son estado raylang por conexión (heap por fibra + búferes). Matiz nuevo: **mimalloc es mejor en huella base y peor al escalar** (ahorra 20 MB a `-c 100`, cuesta 96 MB a `-c 1000`; cruce ~250 conexiones) → `--without mimalloc` es palanca real para el perfil servidor, el default sigue bien para el caso común. Refuerza el arco: pasar a fibras se lleva el hilo Y su arena, o sea los 265 KB completos. **No arrancado**: es un arco con impacto en el modelo de ejecución del backend nativo, no un fix; el caso medido ya está, falta la decisión |
| **Recuperación de errores fatales** (panic → valor, estilo `recover` de Go) | Builtin nuevo (`try_call`) + doc; la base ya existe (`try_join` M56.5, tres motores) | **M97** | 📌 **PLAN FIJADO** (§49 abajo): 97.1 documentar `try_join` como el "recover" del proyecto (hoy 0 menciones en el MANUAL) + fijar semántica con la cancelación de hermanas · 97.2 `try_call` (misma fibra; intérprete trivial → oráculo completo; VM desenrolla marcos al marcador; nativo `catch_unwind`) · 97.3 supervisión de actores (librería pura sobre spawn+try_join) · 97.4 💤 limpieza en unwind (defer/with_file), solo si 97.2 lo destapa. Aditivo, no bloquea nada |
| **Ejecución de comandos del SO** (`run("git", ["status"])`) | Builtin + `std/process` + feature de Cargo; toca los TRES motores | 💤 **sin hito** | 💤 **APARCADA con diseño hecho** (§53 abajo, jul 2026). Decisión: **no antes de la 1.0**. No hay demanda —cero entradas previas en este archivo, ningún paquete bloqueado— y el FFI (M41) ya es válvula de escape para quien lo necesite hoy. M34 congeló la API con semver: meterla ahora sería congelar superficie sin uso real, y la API de procesos es de las que nadie acierta a la primera (Python tardó 15 años en `subprocess.run`; Node arrastra `spawn`/`exec`/`execFile`/`fork` + `Sync`). El diseño queda escrito para no rehacerlo. **Desacoplado y sí urgente**: la auditoría CLOEXEC (§53.4) |

---

## 1. Concurrencia — goroutines / async-await / suspend

**La de mayor sombra de diseño de todo el lote.** No toca M1 (el intérprete es
mono-hilo), pero **condiciona la arquitectura de la VM en M2.**

Son tres respuestas a la misma pregunta —"¿cómo se suspende y reanuda trabajo?"— y
representan filosofías que compiten:

- **Goroutines + channels (estilo Go)**: green threads con planificador M:N,
  estilo bloqueante, **sin "color" de funciones**. Necesita un *scheduler* y un
  stack por goroutine (coroutines *stackful*). Conceptualmente simple de usar.
- **async/await + futures (estilo Rust/JS)**: coroutines *stackless*
  (transformación a máquina de estados / CPS). Funciones "coloreadas"
  (`async fn`). Más maquinaria de tipos, gran lección de transformación de código.
- **suspend functions (estilo Kotlin)**: coroutines stackless con `suspend`; punto
  medio.

> **Restricción para no bloquear (clave para M2).** Si en M2 construimos la VM
> usando el stack nativo de Rust para los marcos del lenguaje, suspender se vuelve
> casi imposible. Si queremos concurrencia, la VM debe tener su **propio stack
> explícito** (los frames del lenguaje viven en estructuras nuestras, no en el
> stack de Rust). Esa es la decisión de arquitectura que esto impone *cuando
> diseñemos la VM*, no antes.

**Estado tras M2**: ✅ la restricción **se respetó**. La VM (`src/vm.rs`) ejecuta sobre
un stack de operandos y una **pila de marcos explícita** (`frames: Vec<CallFrame>`,
`stack`), en un bucle iterativo — no usa el stack de Rust para los marcos del lenguaje.
Eso es justo lo que permitió, en M12, **guardar una fibra a medias y reanudarla**.

**Estado tras M12**: ✅ **COMPLETO** (DESIGN §21). Se eligió **CSP / green threads
cooperativos M:1** (no async/await, no function coloring) — *stackful* en la práctica: una
fibra es el par `(frames, stack)` de la VM, que se guarda en el scheduler al ceder. Las
cinco sub-fases (slice CSP · backpressure · structured concurrency · `select` · cancelación
de hermanas) están en la fila de la tabla de arriba y en DESIGN §21.2–§21.6. Vive **solo en
la VM** (el intérprete da error limpio y sigue siendo el oráculo secuencial); el scheduler
es **determinista**, así que los programas concurrentes se prueban contra salida exacta.
**Diferidos** (puertas abiertas, no agujeros): M:N paralelo (GC thread-safe), cancelación
preemptiva, algebraic effects (§21.1, rama de producción).

## 2. Null safety

**Ya decidido, implícitamente, al elegir `Option<T>`.** raylang **no tiene
`null`**: la ausencia de valor se modela con `Option<T>` (M6), y el checker obliga
a manejar el caso `None` antes de usar el valor. Es el enfoque de Rust/Swift/Kotlin
moderno.

- Impacto en M1: ninguno (no hay `null` de todos modos).
- Solo lo registramos como **principio**: en raylang nunca habrá referencias nulas.

## 3. Introspección / reflection

Inspeccionar tipos y valores en tiempo de ejecución.

- **Impacto**: medio en el *modelo de valores* de la VM (los valores deben
  conservar metadata de tipo); bajo en el intérprete, donde los valores ya cargan
  su tipo de forma natural.
- **Restricción**: ninguna urgente. Solo evitar un value model en la VM que borre
  toda la información de tipos en runtime.
- **Cuándo**: tardío (post-M6, una vez existan structs/enums que valga la pena
  reflejar).

## 4. Structs vs interfaces / traits

¿Solo structs (datos), o también un mecanismo de abstracción/polimorfismo?

- **Impacto**: en el sistema de tipos (M5–M6). Define la historia de polimorfismo y
  de despacho de métodos. Interactúa con genéricos (los *límites* de un genérico,
  `T: Trait`) y con UFCS.
- **Recomendación**: **structs (datos) + traits estilo Rust** (comportamiento), no
  clases con herencia. Despacho estático por defecto; *trait objects* (despacho
  dinámico) como opción posterior. Encaja limpio con UFCS y genéricos.
- **Cuándo**: decidir al llegar a M5/M6. No afecta M1.
- **Es la primitiva de desacople.** Programar contra un trait (no contra un tipo
  concreto) es lo que permite *delegar/intercambiar implementaciones*. Las
  anotaciones (§9) no desacoplan por sí solas: a lo sumo generan el reenvío
  (`@delegate`/`by` estilo Kotlin) encima de un trait. La DI por reflexión
  (estilo Spring) queda **fuera del camino principal** (runtime + reflexión; el
  trait logra el mismo desacople sin magia y con seguridad de tipos).

## 5. Hot code reloading

Reemplazar código mientras el programa corre.

- **Impacto**: en la arquitectura de runtime — favorece que las llamadas a función
  se resuelvan vía una **tabla mutable** (no punteros fijos), para poder sustituir
  una función en caliente. Aislado y de bajo riesgo.
- **Restricción**: menor; basta tenerlo en mente al diseñar la tabla de funciones
  de la VM (M2).
- **Cuándo**: muy tardío / opcional. Es más tooling que lenguaje.

## 6. Visibilidad (encapsulamiento)

`pub` explícito vs. exportar por mayúscula inicial (estilo Go) vs. otro.

- **Impacto**: pertenece al **sistema de módulos**, que M1 no tiene (un solo
  archivo). Sin módulos, no hay nada que ocultar.
- **Recomendación**: `pub` explícito, en vez de acoplar la visibilidad a la
  capitalización del identificador (la convención de Go mezcla *naming* con
  semántica y es discutible). Pero es cuestión de gusto, a confirmar.
- **Cuándo**: cuando introduzcamos módulos (aún no planificado). No afecta M1.

## 7. Self-hosting (raylang escrito en raylang) — ✅ LOGRADO (M14)

El examen definitivo: que el compilador de raylang esté escrito en **raylang mismo**, y
corra sobre sí mismo (**meta-circularidad**). **Conseguido en M14** (DESIGN §23). Detalle
completo en el libro (parte M14) y en `selfhost/`.

**Qué se logró.** El pipeline entero —lexer, parser, checker, intérprete y loader— vive en
`selfhost/*.ray`, escrito en raylang, y produce **lo mismo que la implementación de Rust**
sobre el código real del proyecto (incluido el suyo propio). Verificado eslabón a eslabón
con Rust como **oráculo**: el lexer se lexea a sí mismo, el parser se parsea, el checker da
el mismo veredicto, y el intérprete ejecuta con el mismo `stdout`+exit. El cierre total es
**run-on-run**: `run.ray` corriendo `run.ray` corriendo un programa → el back-end también.

**Tres decisiones que lo hicieron viable** (no un bootstrap clásico):
1. **Intérprete, no VM** (el back-end auto-alojado es tree-walking; mismo orden M1→M2).
2. **El checker es un validador** (produce el veredicto, NO el lowering de M9).
3. **Resolución en runtime**, no lowering: el intérprete despacha por la etiqueta del valor
   → `dyn`/bounds/genéricos son **no-ops** (el *erasure* ocurre solo). El intérprete
   "cabalga sobre el host": `Value` es un enum de raylang, las celdas de closure son un
   struct, sin GC ni celdas propias.

**Oráculo** (estrategia de todo M14): la misma entrada por ambos pipelines (Rust vs raylang),
y se comparan — **texto canónico** (tokens, AST como S-expr, veredicto del checker) para el
front-end, y **conductual** (stdout + código de salida) para el back-end.

**Habilitadores que lo precedieron**: M13 (`Map`, `panic`/`assert`+test, recursión profunda
+ TCO) lo volvió práctico; M14.6 cerró la stdlib que el compilador usa (`Map`/string/`parse_*`/
`assert`/`sort`/I/O/`pop`/concat de arreglos); M14.7 añadió el loader y la consistencia de
`args()`.

**Diferidos (no necesarios para la meta-circularidad lograda):**
- **VM auto-alojada (M14.5)**: el compilador-a-bytecode + VM en raylang, con el intérprete
  auto-alojado como oráculo. Es el M2 de este módulo; opcional.
- **Loader auto-alojado**: `import M;` calificado, módulos por directorios y cápsulas
  (`mod.ray`), reexports. El loader actual solo cubre `from M import …` (lo que usa el
  compilador). No hace falta *position-shifting* (el checker no baja por posición).
- **Resto de I/O** en el self-hosting: stdin (`input`/`read_line`), `env`, handles de
  archivo —no los usa el compilador, y `args()`/`read_file` ya bastan—.
- **Claves de `Map` genéricas** (vía un trait `Hash` con diccionarios): hoy solo primitivos.
- **Operadores bitwise (`& | ^ ~ << >>`) + `bytes` en el toolchain auto-alojado**: el
  lexer/parser de `selfhost/` quedaron **congelados en M14.1**, antes de que M19.3a añadiera
  los operadores bit a bit al lexer de Rust (para WebSockets/cripto) y de que M16 añadiera el
  tipo `bytes`. Confirmado empíricamente por el modo **`audit`** de `tools/spanglish.py`
  (cruza el tokenizador Python contra el lexer REAL auto-alojado vía `selfhost/lex_dump.ray`):
  **15 de 34 `packages/*.ray` no lexean** con el lexer auto-alojado —`lex error … expected
  '&&'/'||' (did you forget a '&'/'|'?)`, `unexpected character '^'`—, justo los de red/cripto
  que usan bitops (hpack, scram, hashing, mongo, mysql, postgres, http2, …). **No bloquea la
  meta-circularidad** (el compilador de raylang no usa bitops), pero el lexer/parser
  auto-alojado va **por detrás** del de Rust en paridad de lenguaje. Cerrarlo = port mecánico
  de esos tokens + su precedencia (como M19.3a en Rust) al lexer/parser de `selfhost/`, y el
  tipo `bytes` en el pipeline auto-alojado. (Nota ya entrevista en la fila de M17.)

## 8. Tooling de editor (coloreado y validación)

Soporte de los archivos `.ray` en editores. Tiene dos mitades muy distintas:

- **Coloreado (syntax highlighting)** — ✅ **hecho** para VSCode en
  `editors/vscode/` (gramática TextMate). Es **por-editor**: cada editor tiene su
  formato (TextMate en VSCode; tree-sitter en Neovim/Zed/Helix). Es una reescritura
  en regex de las reglas léxicas de `DESIGN.md` §3, independiente del lexer en Rust.

- **Validación / lint en vivo (diagnostics)** — ✅ **hecho** (M10.2). Un **Language
  Server (LSP)** dentro del propio binario (`raylang --lsp`), que **reutiliza el checker**.
  Decisión clave: **cero dependencias de Cargo** → JSON-RPC + *framing* a mano (`mod json`
  en `src/lsp.rs`), no `tower-lsp`. Es un **cliente externo** como el REPL/runner: no toca
  el core. Funciona en todos los editores; VSCode trae un cliente propio (`editors/vscode/`,
  con npm del lado del editor), Sublime/Neovim/Helix lo usan declarándolo.
  - Capacidades (M10.2b–f): diagnósticos, **hover** e **ir-a-definición** (de variables,
    funciones y tipos), **find-references**, **rename**, **completion** (de archivo y por
    ámbito) y **signature help**. El checker pasó de *validador* a *consultable*
    (`semantic_index` recolecta un `SemanticIndex` antes de cualquier lowering).
  - **M45** ✅ **completion de miembros** (`recv.`, DESIGN §47) + **de imports** (`from M import …`
    símbolos `pub`; `import …` rutas de módulo con encapsulación, DESIGN §47.2). El de miembros: campos/métodos/builtins/UFCS del tipo del
    receptor; DESIGN §47). Repara la fuente con un centinela (`recv.__raycomplete__;`) y consulta
    `checker::member_completion`; incluye los **builtins de string/array/map** y el orden superior
    del prelude (`map`/`filter`/`fold`/`sort`). Diferido: docs `///` de métodos de impl del usuario,
    receptores que son expresiones (`f(x).`), UFCS del usuario sobre primitivos.
  - **Completion type-aware tras `|>`** (pipeline) — el `|>` no tiene tratamiento propio en la
    completion: sin un `.` delante, cae al camino **de archivo** (ofrece TODAS las funciones, no
    filtradas). Idea: en `x |> ` filtrar a las funciones libres cuyo **primer parámetro** acepte el
    tipo del operando izquierdo (`x`), igual que `member_completion_items` hace para el `.`. Reusa la
    inferencia del checker sobre el operando. Impacto **bajo** (cliente LSP, front-end puro; cero
    runtime). Ergonomía, no corrección.
  - ~~Diferido: hover/def de **métodos**~~ → ✅ **M10.2g/h** (DESIGN: hover de campos y métodos vía
    `field_name_pos` del parser + ir-a-definición cruzando archivos); esta fila quedó desactualizada.

## 9. Anotaciones (`@test`, `@derive`, …)

Metadatos adheridos a declaraciones (`@nombre` o `@nombre(args)` antes de una
función/tipo/campo). El eje que define la complejidad es **quién las consume**.

**Dirección decidida: empezar por anotaciones *integradas* (conjunto cerrado que
el compilador conoce).** Es la primera aproximación: barata, didáctica y de buen
rendimiento. Candidatas:

- `@test` — marca funciones de prueba; base de un framework de tests para `.ray`
  (el win que motiva arrancar por aquí).
- `@deprecated("...")` — el checker advierte al usarla.
- `@inline` — pista para la VM (M2+).
- `@builtin` / `@extern` — la implementación vive en el host (Rust). Permitiría
  **limpiar deuda**: `print` dejaría de ser un caso especial y sería
  `@builtin fn print(...)`.
- `@derive(Eq, Show)` — autogenera igualdad/impresión para `struct`/`enum`. Su caso
  de uso natural aparece **cuando existan structs/enums** (M3/M5), que es lo que las
  motiva de verdad.
- `@delegate` / keyword `by` — autogenera el **reenvío** de los métodos de un trait
  a un campo (`struct App impl Saludo by saludo`). Es *sugar* sobre traits (§4): la
  anotación genera el reenvío, pero el desacople lo da el trait, no la anotación.
  La inyección de dependencias por anotación (`@inject` + contenedor/reflexión)
  queda **fuera del camino principal**.

**Lo que NO hacemos por ahora:**

- **Anotaciones definidas por el usuario que "hacen algo"** = un **sistema de
  macros / metaprogramación** (transformar o generar código). Es de lo más difícil
  del diseño de lenguajes (higiene, fases, manipular el propio AST; conecta con
  reflection §3 y con self-hosting §7). Queda como **capstone de muy largo plazo**,
  opcional, con su propio hito.
- **Retención en runtime + reflexión** (estilo Java `@Retention(RUNTIME)`): atada al
  ítem de introspección §3.

**Estado (M10):** ✅ hecho para el conjunto cerrado. El parser consume `@nombre[(args)]`;
**`@test`** (con runner) y **`@derive(Eq, Show)`** (genera el `impl`, que M9 baja) están
implementados. `@delegate`/`by` y las anotaciones de usuario que "hacen algo" (macros) siguen
diferidos (capstone de muy largo plazo).

## 10. API de runtime / I/O (cómo raylang habla con el exterior)

Hoy raylang tiene un único cable hacia afuera: `print` (stdout) y el código de
salida que devuelve `main`. Para escribir apps de verdad (CLI, interactivas) hace
falta una **API de runtime**: funciones que expongan lo que el host (Rust) ya tiene.

**Decisión de diseño: los argumentos y la I/O NO van en la firma de `main`.**
`main` queda como `main() -> int` (punto de entrada + código de salida). El acceso
al exterior se hace por **funciones builtin/stdlib**, estilo Go (`os.Args`) y
Python (`sys.argv`) — no estilo C (`main(argc, argv)`). Razón: no especializa la
firma de `main`, y la capacidad queda disponible en *cualquier* función, no solo en
la entrada. Encaja con cómo ya funciona `print` (un builtin).

**Estado (M11):** ✅ implementado como builtins (la I/O falible devuelve `Option`/`Result`;
el runtime no sabe de ellos —los arma el prelude en raylang—). Superficie:

- `args() -> [string]` — argumentos de la línea de comandos. ✅
- `input()`/`read_int()` (stdin), `eprint(...)` (stderr), `env(nombre)`. ✅
- I/O de archivos: `read_file`/`write_file`/`append_file`/`exists`/`remove_file`/`list_dir`
  + handles con buffering (`open`/`read_line`/`write`/`close`, M11.8). ✅

**Matiz de orden** (dos capacidades distintas):

- **Interactivo (stdin)**: un builtin de lectura solo necesita strings/enteros, que
  ya existen → **podría llegar relativamente pronto**.
- **Por argumentos (argv)**: `args()` devuelve `[string]`, así que necesita
  **arreglos (M3)** + indexar + `len`. Es de la época de la stdlib (**M7**).

**Impacto en el diseño actual:** ninguno; es puramente aditivo (más builtins). Solo
fija la decisión de mantener `main` sin parámetros.

## 11. Optimización de la VM

**Línea base (M2):** la VM corre ~3x más rápido que el intérprete en `fib(32)`
(`benchmarks/bench.sh`), con mucha menos varianza. El ~3x es el techo de la
arquitectura *actual*, no de la idea: ambos motores comparten el mismo `Value` (que
se clona) y la misma `apply_binary`. Estas optimizaciones, de mayor a menor impacto
esperado, llevarían la VM bastante más allá. Medir cada cambio con el benchmark.

- **Evitar clones de `Value`** (victoria fácil). Hoy `GetLocal` y `Constant` hacen
  `.clone()`. Para `int/float/bool` (que son `Copy`) el clon es barato, pero para
  `string` copia el `String` entero. Pasar a un `Value` con strings compartidos
  (`Rc<str>`) abarata el clon a incrementar un contador.
- **Locales en la pila de operandos** (estilo clox). Hoy las locales viven en un
  `Vec` aparte por marco. Ponerlas en la propia pila de operandos (con un *base
  pointer* por marco) evita una indirección y un arreglo extra por llamada.
- **Despacho más rápido.** El bucle hace `match` sobre `OpCode` clonado por
  instrucción. Opciones: no clonar la instrucción (resolver el préstamo de otra
  forma), *direct threading* / *computed goto* (no disponible en Rust estable de
  forma directa; se aproxima con un `match` bien ordenado o tablas de saltos).
- **Bytecode empaquetado en bytes.** Pasar de `Vec<OpCode>` (un enum por
  instrucción) a bytes mejora la densidad de caché —el sentido original de
  "bytecode". Cuesta legibilidad; es una optimización tardía.
- **Constantes deduplicadas.** `add_constant` hoy siempre agrega; deduplicar
  reduce la tabla y mejora la localidad.
- **Peephole / plegado de constantes** en el compilador (`1 + 2` → `3`), e
  *inline caching* para llamadas. Más avanzado.

**Impacto en el diseño:** ninguno en el lenguaje; es trabajo interno de la VM. No
bloquea nada y se hace de forma incremental, midiendo con `benchmarks/`.

**Progreso (medido en `fib(32)`, release, hyperfine):**
- **Baseline**: VM 735 ms (intérprete 1.76 s; 2.40× más rápida).
- **Opt.1 — no clonar la instrucción por iteración** ✅: el bucle copia el `&CompiledProgram` a un
  local (`let program = self.program`) y toma la instrucción **prestada** (`&program…code[ip]`) en vez
  de clonarla; el préstamo es del programa (inmutable, vive como la VM), no de `self`, así que el cuerpo
  sigue pudiendo mutar `self`. → **685 ms** (~7%; 2.59×).
- **Opt.2 — pool de locales por llamada** ✅: cada llamada necesitaba un `Vec<Local>` nuevo (millones en
  recursión). Ahora un **pool/free-list** (`locals_pool`) recicla los `Vec` de los marcos que retornan
  (Return, fin de chunk, llamada en cola): `new_locals` saca del pool y reconstruye; `recycle_locals`
  devuelve (acotado a 256). **No es raíz del GC** (contenido basura entre reciclar y reusar; se `clear()`+
  reconstruye, nunca se lee lo viejo → seguro). → **552 ms** (~19%; **3.22×**, 25% acumulado vs baseline).
- **Opt.3 — `Rc<str>` para strings** ❌ **evaluado y DESCARTADO** (medido): se cambió `HeapValue::Str`
  de `String` a `Rc<str>` para abaratar el clon (bump de contador). Resultado en `benchmarks/strings.ray`
  (string-heavy): **79 → 77 ms** (~2.6%, dentro del ruido) — porque los builtins que producen strings
  (`to_upper`/`split`/…) devuelven `String` y el `.into()` a `Rc<str>` **copia** los bytes, anulando el
  clon más barato. Peor aún: en `fib` (que NO toca strings) **552 → 609 ms (−10%)**: cambiar el layout de
  `HeapValue` desplazó el codegen de LLVM del gigantesco `run()` y empeoró el bucle aritmético. Net
  negativo → **revertido**. Lección: `Rc<str>` solo gana en código *clone-heavy* (leer el mismo string
  muchas veces sin construir, p. ej. `self.src` del lexer auto-alojado); no compensa el coste de
  construcción ni el riesgo de codegen. Reconsiderar solo con un perfil que muestre el clon de strings
  dominando (string interning sería una alternativa con menos churn de construcción).
- **Opt.4 — fast-path entero en ops binarias** ✅ (jun 2026, `measure.py` mejor-de-5): en el brazo de
  operaciones binarias del lazo, si ambos operandos son `Int` (caso dominante en bucles/recursión) se
  resuelve la operación **en el sitio**, evitando el doble match (`bin @ (...)` + el rematcheo de opcode y
  ~30 tipos dentro de `apply_binary`) y la llamada a `apply_binary`. Semántica idéntica → oráculo intacto.
  **fib(35) −5 %, bucle 10M −6 %** (`arrays` sin cambio, no es aritmético).
- **Opt.5 — `new_locals` sin branch para funciones sin capturas** ❌ **descartado** (medido dentro del
  ruido): las funciones calientes (fib) tienen pocos locales, el branch estaba bien predicho.
- **Opt.6 — safepoint del GC amortizado** ❌ **descartado**: techo ~2-3 % pero rompe el modo estrés del GC
  (colectar en cada punto seguro caza raíces faltantes); capturarlo bien exige mover el safepoint a los
  sitios de asignación + back-edges → riesgo sobre el test sagrado del GC, no compensa por ~2-3 %.
- **Opt.7 — posición `(línea,col)` perezosa** ✅ (jun 2026, `measure.py` mejor-de-**15**): el lazo de
  despacho leía `chunk.lines[ip]` por instrucción, pero el camino caliente (locales/constantes/aritmética/
  saltos) no la usa —solo error/cesión—. Ahora un macro `pos!()` la lee bajo demanda → una lectura menos
  por iteración. **fib −7 %, loop −9 %, arrays −8 %** (consistente: toda instrucción pasa por el lazo).
  **Lección de medición**: el efecto (~8 %) quedaba enmascarado con mejor-de-5 (la baseline saltaba ±4 %);
  mejor-de-15 lo destapó. Oráculo + tests de posición de error intactos (la posición se calcula igual).
- **Opt.8 — `children()` del GC con buffer reusado** ❌ **descartado** (dentro del ruido, incluso con el
  benchmark nuevo `gcnested.ray`): el `trace` corre infrecuente (umbral ×2) y un `Vec` pequeño no es el
  cuello; para arreglos de `int` los hijos son primitivos → ya devolvía un `Vec` vacío (que no asigna).
- **LTO + `codegen-units=1`** ❌ **descartado** (medido: igual o peor que el perfil por defecto).
- **M29.3 (jul 2026, `measure.py` mejor-de-15).** Se retomó el transversal con la baseline fib(35) 2.18 s /
  loop 1.04 s / arrays 0.196 s / gcnested 0.312 s.
  - **Opt.9 — dedup de constantes** ✅ **conservado por MEMORIA, no velocidad**: `add_constant` reutiliza el
    índice de una constante idéntica ya presente (búsqueda lineal, solo en compilación). Los literales se
    repiten muchísimo (`0`/`1`/`2`, nombres de campo, strings) → el pool encoge. **Velocidad neutra** (dentro
    del ruido: es una lectura por índice en runtime, da igual cuántas constantes haya), pero es la optimización
    estándar de todo VM de bytecode y **no tiene contrapartida** (solo elimina duplicados) → se mantiene como
    mejora de calidad/memoria. Test `add_constant_deduplica`.
  - **Opt.10 — `OpCode` 32→24 B (boxear `GetField`/`SetField`)** ❌ **medido y DESCARTADO**: eran las únicas
    variantes con `String` inline; boxearlas a `Box<str>` baja `OpCode` a 24 B (tope `(usize,usize)`=16 B) →
    stream de código 25 % más denso. **Sin efecto** (fib incluso +2 %, resto plano): estos benchmarks NO están
    limitados por el *fetch*/caché (los chunks caben de sobra en L1), sino por el trabajo real (llamadas,
    aritmética, GC). Confirma que reducir `HeapValue` 32→16 (boxear `Str`/`Bytes`, alta cirugía ~119 sitios)
    tampoco pagaría en estos casos → **no se intentó**. Revertido.
  - Conclusión: **los levers de tamaño (OpCode/HeapValue) no mueven estos benchmarks**; las ganancias fáciles
    (Opt.1/2/4/7) ya están exprimidas. El salto restante sería algorítmico (locales en la pila estilo clox,
    reducir el coste de llamada/GC), refactor grande de ROI decreciente. **M29.3 cerrado** con el dedup + este
    registro. La VM sigue a ~3.2× del intérprete.
- **Opt.11 — ARRANQUE (jul 2026)**: benchmark del usuario (hyperfine hello-world vs 7 lenguajes de
  scripting) situó a `ray run` en 3.9 ms (3.º tras lua 1.6/perl 2.7, por delante de bun/python/node).
  Desglose medido: piso del binario 1.9 ms + pipeline 2.3 ms, TODO el pipeline dentro de `check()`.
  **Bug real encontrado**: los accessors de `src/prelude.rs` (`enums()`/`structs()`/`functions()`/
  `traits()`/`impls()`) re-lexeaban y re-parseaban el prelude ENTERO cada uno → **5 parses completos
  por check** (en cada arranque del CLI y en cada tecleo del LSP). Fix: un solo parse cacheado en
  `OnceLock` + accessors que clonan (−0.85 ms). Más `strip = "symbols"` en el perfil release (6.5→5.8
  MB). **Resultado: 3.9 → 3.0 ms** (−23 %; pipeline 2.3→1.2 ms). El LSP gana lo mismo por tecleo.
  Reparto restante del pipeline: prepare ~540 µs (clonado del prelude + mangling de impls) +
  check_program ~365 µs + bajadas/compile ~230 µs — reducirlo más exigiría un prelude pre-chequeado
  compartido (arquitectural, ROI decreciente). El piso (1.8 ms vs 0.9 de `/usr/bin/true`) es dyld/init
  de un binario de 5.8 MB (lua es ~300 KB); achicarlo = quitar rusqlite bundled, no vale la pena.
- **REGRESIÓN detectada (revisión jul 2026, `measure.py` mejor-de-15): arrays +38 % / gcnested +39 %**
  (fib/loop intactos). **Bisecada a M48.4** (builtins de contenedor → métodos de trait): `a.push(i)`/`a.len()`
  ya no bajan al opcode directo (`Push`/`Len`) sino al método del trait (`impl<T> Push<T> for [T]`), cuyo
  cuerpo es un *forwarder* de una línea a `__push`/`__len` → cada push/len paga **una llamada VM completa
  (marco + call + return) para ejecutar UN opcode**. M38 exonerado (medido ≈ baseline en `233532f`).
  - ✅ **M52 — inlining de forwarders triviales, COMPLETO** (jul 2026): pase `inline_forwarders` en el
    checker (paso 8 de `check`, tras todo el lowering → beneficia a AMBOS motores, oráculo intacto): si el
    destino de una llamada es un método **manglado** (`Tipo#metodo`; un local no puede llamarse así) cuyo
    cuerpo es **exactamente una llamada a builtin pasando sus params en orden** (`fn push(self, x) {
    __push(self, x) }`), el sitio se reescribe a la llamada al builtin (`__push(a, i)`) → la VM emite el
    opcode directo y el intérprete se ahorra el marco. El forwarder NO se elimina (sigue referenciable como
    valor: vtables de `dyn`, diccionarios). **Guarda de sonoridad**: el compilador resuelve variable-local
    antes que builtin → si el programa liga una variable con el nombre de un builtin objetivo (`let __push
    = …`, legal), ese builtin se excluye del inlining (aproximación programa-completo, coste cero en la
    práctica). **Medido (`measure.py` mejor-de-15): arrays 0.250 → 0.184 s, gcnested 0.403 → 0.291 s —
    regresión de M48.4 recuperada al completo** (≈ baseline pre-M48.4; fib/loop sin cambio). Baseline
    actualizada. Los traits siguen siendo la superficie del lenguaje; solo desaparece del código generado.
- **Opt.12 — plegado de constantes** ✅ (jul 2026) **conservado por CALIDAD, no velocidad** (precedente
  Opt.9): `const_fold` en `emit_expr` (compilador, solo VM) evalúa en compilación las (sub)expresiones de
  literales int/float/bool con operaciones TOTALES (`1 + 2 * 3` → una constante). Lo que puede trapear
  NO se pliega (división/módulo por 0, overflow del int checked → quedan para runtime con su posición) →
  semántica idéntica, oráculo intacto (test `plegado_de_constantes`). Medido: neutro (los benchmarks no
  tienen constantes plegables en caliente); chunks más cortos sin contrapartida.
- **Opt.13 — umbral del GC amortizado por TRABAJO trazado** ✅ (jul 2026, la ganancia GRANDE del arco):
  cierra la idea aparte de §22 (el `next()` lazy a ~6 µs/elemento) con un diagnóstico distinto al anotado.
  **No era el closure ni el Option en heap** (~0.31 µs/paso, medido con arreglo de 1k): era el **pacing del
  GC** — el umbral por conteo (`live*2`, mínimo 64) con POCOS objetos vivos pero un contenedor GRANDE
  dispara el GC cada ~50 asignaciones y cada recolección re-escanea el contenedor entero (`children()` de
  un `[int]` de 1M recorre 1M elementos aunque no haya handles). `for x in xs.iter()` sobre 1M: **18.500
  GCs × 1M elementos ≈ 6.8 s**. Fix en `gc.rs`: `trace()` contabiliza el trabajo (elementos escaneados,
  `trace_cost`) y `sweep()` fija `next_gc = max(live*2, live + trabajo/4, 64)` → tras un GC que costó W se
  permiten ≥ W/4 asignaciones → coste amortizado O(1)/asignación. **Medido: `iter.ray` 6.77 → 0.40 s
  (17×)**; banco general neutro (±1.5 %); pausas gcpause idénticas (8.2→8.5 ms max, media 1.1: con heap
  vivo grande `live*2` sigue mandando); gcpause_concurrent 0 GCs igual. Contrapartida consciente: más
  basura transitoria entre GCs (espacio por tiempo; hasta W/4 objetos ≈ decenas de MB con un arreglo de
  1M); el tope `max_live` (M42.2) y el modo estrés siguen intactos. Benchmark nuevo `benchmarks/iter.ray`.

## 12. Asperezas de M3

Dos límites pequeños del front-end que afloraron al escribir ejemplos con arreglos
y structs (`examples/data/pila.ray`, `examples/data/inventario.ray`). No son bugs —el
lenguaje es consistente— sino refinamientos de ergonomía.

- **Coma final en literales de arreglo.** ✅ **Resuelto** (fase de limpieza post-M8).
  `[1, 2, 3,]` ya se acepta, como la coma final en los campos de un `struct`
  (`array_literal` corta el bucle si tras una coma viene `]`).

- **Inferencia del `[]` vacío en posición de campo.** ✅ **Resuelto en M6.2.** El
  **chequeo bidireccional** (`check_expr_expected`) propaga el tipo esperado del campo
  hacia la expresión, así que `Pila { datos: [], tope: 0 }` ya tipa sin un `let`
  intermedio. Era, como se anticipó aquí, un primer caso del trabajo de inferencia que
  M6.2/M8 generalizaron.

**Impacto**: bajo y aditivo; ningún cambio de semántica del lenguaje, solo acepta
más programas que hoy se rechazan. No bloquea nada.

---

## 13. Ecosistema de paquetes (registro central + política de tiers)

Cómo se distribuyen las **librerías** de raylang y cómo se decide dónde vive cada una. El gestor de
paquetes (M39c) ya resuelve dependencias por **git** y por **ruta**; faltan dos piezas, diseñadas en
**DESIGN §53 (política de tiers)** y **§54 (registro central, M51)**.

### 13.1 Política de tiers (gobernanza) — ya explícita

Toda capacidad fuera del núcleo vive en uno de tres tiers, decididos por **universalidad · peso e
independencia · estabilidad de API · superficie de seguridad**:

- **`std/` embebida** — universal, ligera, estable; en el binario base (`import std/math;`). API atada al
  versionado del lenguaje.
- **paquetes `packages/*`** — nicho, pesado o dependiente de sockets/TLS; vía `ray.toml`. API con su propio
  semver. Hoy: `packages/net`.
- **`examples/`** — demos y material pedagógico; no importables como librería.

**Pipeline de promoción**: `examples/` (prototipo) → si madura → `std/` (universal) o `packages/*` (nicho).
Regla de seguridad ya vigente: cripto que toca secretos reales → paquete respaldado por `ring` (`net/crypto`),
no la impl pura embebida (que se queda como demo del lenguaje).

### 13.2 Registro central (M51) — la brecha nº1 del ecosistema

Hoy "instalar" = escribir la URL git exacta. Falta **instalar por nombre** (`ray add foo`) contra un
**índice** y **publicar** (`ray publish`). Es la brecha que `PRODUCTION.md` (Parte I §2) marca como
"flexible en el lenguaje, ❌ en el ecosistema".

**Decisión central: índice respaldado por git, sin servidor propio** (coherente con "cero deps de Cargo /
*shell out* a git / tests offline"). El índice es un **repo git** que mapea `nombre → (git URL, versiones,
hash)`; todo lo demás (descarga, cápsula, lock, transitivas, MVS) es la **maquinaria existente de M39c**.
Descartado el índice hospedado (contradice el "sin servidor"; su valor —búsqueda, cuentas, firmas— es
ortogonal y se añade después sobre el mismo índice git).

- **`ray.toml` por nombre**: `foo = "1.2.0"` (sin prefijo `git+`/`path:`) → resuelve por el índice.
- **Subcomandos**: `ray add`, `ray publish` (valida+hashea+añade entrada inmutable), `ray update`, `ray yank`.
- **Prereq**: rangos semver de verdad (diferido de M39c; el índice mapea un nombre a muchas versiones).
- **Fases**: 51a leer índice + `ray add` + rangos · 51b `ray publish` · 51c índice remoto + `update`/`yank`
  · **51d endurecimiento** ✅ (revisión jul 2026, DESIGN §54.5): nombres de paquete validados (un nombre
  `../../x` de una transitiva no confiable escapaba de la caché), el **hash del índice se verifica** al
  descargar (antes era decorativo; cierra el TOFU del lock → el índice es raíz de confianza), `ray publish`
  valida+hashea el **tag** publicado (no el working tree), e índice remoto pinneado que se re-clona si su
  spec cambia (antes quedaba obsoleto en silencio)
  · **51e cierre de límites** ✅ (DESIGN §54.5): `ray publish` corre el **check semántico completo** del
  clon del tag (resuelve sus deps; `check_all_modulo`, sin exigir `main`); **pre-releases** de verdad
  (`1.0.0-rc1`: orden semver §11 + regla de cargo — solo casan si el requisito las menciona; `latest`/`*`
  eligen finales); y **aviso de dependency confusion** cuando una transitiva declara su propio
  `[registry] index`
  · **51f ergonomía** ✅ (DESIGN §54.5): **`ray remove`** (inversa de add: manifiesto + lock re-resuelto +
  caché borrada solo si nadie más la usa) y **`ray search [patrón]`** (lista el índice con la versión
  instalable más alta); limpieza: el semver se extrae de `index.rs` a **`src/semver.rs`** (lo consumen
  índice, resolutor y CLI). **M51 COMPLETO, revisión de diseño cerrada** (queda diferido: multi-índice,
  firmas de publicación — §54.7).

**Impacto**: **medio-alto en adopción, cero en runtime y en el lenguaje** — es CLI + resolución en el
front-end; los motores nunca ven un paquete. Es aditivo (git/`path:` siguen). Diferido: UI/búsqueda web,
firmas de publicación (sobre el hash existente), mirrors/proxy, namespaces con dueño.

---

## 14. Clientes de bases de datos (MySQL · PostgreSQL · SQLite) — M53, PLAN

Análisis de factibilidad (jul 2026). Punto de partida: `packages/net` ya tiene **PostgreSQL** (SCRAM +
protocolo simple, `pg_query`) y redis; el patrón de verificación (servidor de juguete en Rust + oráculo
ambos motores, `tests/postgres_cli.rs`) está probado; `std/` trae TCP/TLS + SHA1/SHA256/HMAC.

- **Factibilidad**: Postgres = **evolución** de lo existente. MySQL = **factible en raylang puro**
  (handshake v10; `mysql_native_password` = SHA1 ✓, caching_sha2 fast-path = SHA256 ✓, full-path sobre
  `tls_connect` ✓; `COM_QUERY` texto). SQLite = **factible SOLO vía FFI** (no es red: librería C
  embebida; `extern "sqlite3"` resuelve `libsqlite3` del sistema) con UN bloqueador: los *out-params*
  de doble puntero (`sqlite3_open(path, sqlite3**)`) que el FFI de M41 no marshalea → extensión
  acotada `out ptr` (M41.5, la ruta recomendada) o el hack `malloc(8)`+`memcpy`+puntero-como-`u64`
  (spike, no diseño). Reimplementar el formato de archivo en raylang puro: descartado (solo-lectura y
  enorme).
- **Ubicación**: nuevo paquete **`packages/db`** (SQLite no encaja en `net`; API uniforme para los
  tres: `connect`/`query(conn, sql, params) -> Result<[[string]]>`/`exec`/`close`, tipado-a-texto v1).
  `db` → path-dep a `net` (scram); `net/postgres` se conserva (compat).
- **Fases**: **M53.1** MySQL ✅ **COMPLETO** (DESIGN §55.1: `db/mysql.ray` — handshake v10 +
  mysql_native_password completa + caching_sha2 fast-path + AuthSwitchRequest + COM_QUERY texto;
  `connect/query/exec/disconnect`; toy server con auth precomputada + oráculo ambos motores en
  `tests/mysql_cli.rs`) · **M53.2** Postgres v2 ✅ **COMPLETO**
  (DESIGN §55.2: `db/postgres.ray` — conexión persistente + SCRAM + protocolo extendido Parse/Bind/
  Describe/Execute/Sync → parámetros `$1`/`$2` en texto/anti-inyección + todas las filas + transacciones
  vía SQL; reusa `net/scram` como cápsula hermana; toy server con eco de params + oráculo ambos motores en
  `tests/postgres_v2_cli.rs`) · **M53.3** ✅ **COMPLETO — REFORMULADO** (jul 2026, giro a foco
  producción): en vez de extender el FFI con out-params, **builtins `__sqlite_open/exec/query` sobre
  `rusqlite` (bundled)** — patrón `ring`/M43: el binding maduro resuelve dobles punteros, lifetimes de
  statements y destructores de bind; SQLite compilado dentro del binario (cero deps del sistema, sin
  test condicionado); la conexión vive en el registro común de handles (`close(h)` la cierra); stubs
  wasm (DESIGN §55.3) · **M53.4** ✅ **COMPLETO** (`db/sqlite.ray`: `connect(path)`/`query`/`exec` con
  `?N` posicionales/`disconnect`, celdas texto NULL→""; test determinista `:memory:` ambos motores en
  `tests/sqlite_cli.rs`; demo `examples/db/sqlite_demo.ray`) · **M53.5** opcional: libro + ejemplo CRUD
  integrador. **M53 COMPLETO** (los tres clientes).
- **Impacto**: 53.1/53.2 cero compilador (librería pura). 53.3 reformulado: 3 opcodes + impls por motor
  (mecánico, patrón M11.4); cero cambios de checker/superficie. Todo aditivo.
- **Diferido — FFI out-params (ex-M53.3, cierra el diferido de M41)**: la extensión que vuelve al FFI
  útil para APIs C con dobles punteros (`f(&handle)`); superficie por decidir (retorno extra en tupla
  vs. slot explícito `out ptr`). Sin fecha: hacerla cuando aparezca la **segunda** librería C que la
  necesite, con un caso de uso real guiando el diseño (SQLite ya no la necesita).
- **Diferido — FFI v2 con `libffi`** (análisis jul 2026, tras la revisión del FFI): el crate `libffi`
  construye el *call frame* en runtime para CUALQUIER firma → mataría el catálogo combinatorio de
  moldes (aridad libre, structs por valor) y su API de **closures** permitiría pasar closures de
  raylang como callbacks a C (qsort, event loops) — la carencia grande real. Costo: `libffi-sys`
  compila la libffi de C con autotools (build más frágil que rusqlite, molesto en MSVC). Mismo
  gatillo que los out-params: la segunda librería C real. Endurecimiento ya hecho (jul 2026): carga
  migrada a `libloading` (arregla Windows: dlopen no existía en MSVC → el binario no linkeaba) +
  catálogo de aridad 3 completado (faltaban 5 de 8 combinaciones). Pendiente conocido: las
  **variádicas** (printf) transmutan "bien" pero son UB en arm64 (la ABI difiere) — sin detección
  posible desde la firma; documentado como fuera de contrato.
- **Revisión FFI bajo fibras (30 jul 2026)** — con las fibras por default en el binario nativo
  (fijadas a su worker, sin work-stealing), una extern C bloqueante vara al worker entero. Hallazgos
  y estado:
  - ✅ **`extern "lib" blocking { … }`** (DESIGN §90): descarga al pool bloqueante de
    `ray_runtime::fibers::run_blocking` + fibra aparcada; inerte donde no hay scheduler que proteger.
  - **Pendiente — descarga en la VM**: la VM también es M:N y sufre el mismo varamiento; aparcar la
    fibra de la VM durante la llamada exige integrar la finalización con `poll.rs` (una tubería de
    completado o similar). Hacerlo cuando haya un caso real de FFI bloqueante intensivo sobre la VM.
  - ✅ **Paridad de aridad VM↔nativo** (30 jul 2026): catálogo de la VM generado por macro
    cartesiana hasta `ffi::MAX_ARITY` (= 6, cubre `mmap`/`sendto`/`recvfrom`; 127 firmas) +
    diagnóstico del checker por encima del límite — un extern fuera de rango ya no compila en
    ningún motor (antes: nativo OK, VM reventaba en runtime). DESIGN §90.
  - ✅ **Pila de fibra y código C** (30 jul 2026): con externs declaradas, el main emitido fija
    `set_default_fiber_stack_kib(1024)` (1 MiB, reserva virtual) antes de la primera fibra;
    precedencia `RAY_FIBER_STACK_KIB` > default programático > 128 KiB. Documentado en la sección
    FFI de REFERENCE/MANUAL. DESIGN §90.
  - ✅ **`std/ffi.errno()`** (30 jul 2026): builtin `__ffi_errno` + módulo `std/ffi`; mismo lector
    por plataforma en los tres motores, y `run_blocking` repone en el worker el errno del hilo del
    pool (la regla "leer inmediatamente" vale igual con `blocking`). DESIGN §90.
  - Nota positiva documentada: la **fijación** hace seguras las C-libs con afinidad de hilo usadas
    desde una sola fibra.

---

## 14b. SSR: std/template optimizado + templates compilados — M55

- **Fase 1 ✅ COMPLETA (jul 2026)** — `std/template` optimizado para SSR (motivado por: es la pieza de
  server-side rendering junto a `net/webserver`). Medido con una página de 21 KB / 500 filas:
  **4.6 → 1.3 ms por render (3.5×)**. Cambios: (1) **API de dos niveles** `compile(tpl) -> Result<
  Template, string>` + `render(t, ctx) -> string` (SSR: compilar al arrancar, renderizar por request;
  `render_template` queda como azúcar); (2) el tokenizador extrae tramos con `substring` por índices
  (antes carácter a carácter con `+`, O(n²)); (3) render/`render_val` emiten PARTES en `[string]`
  compartido + `join` final; (4) `escape_html` sobre los builtins NATIVOS (`contains`×5 como fast
  path del texto limpio + `replace`×5) — lección medida: **indexar `s[i]` en bucle es O(len) por
  acceso** (la VM colecta los chars en cada Index) → un escaneo así es cuadrático; o se toma
  `s.chars()` una vez o se delega en builtins; (5) el binding del `{% for %}` se crea una vez y se
  MUTA por iteración (antes copiaba el contexto entero por vuelta); (6) **`{% elif %}`** (desazucara
  a if anidado en la rama else; `SeqResult.stop_tag` conserva la condición). Golden extendido
  (`tests/template_cli.rs`, ambos motores).
- **Fase 2 ✅ COMPLETA — M55 TEMPLATES COMPILADOS** (DESIGN §59): `ray templ` compila `.ray.html`
  (firma inline `{% params nombre: tipo, … %}`) a `pub fn render_<stem>(…) -> string` en un módulo
  generado al lado (commiteable). Expresiones raylang verbatim en `{{ }}`/`{% if %}`/`{% for %}`;
  typo en una variable = error de compilación (probado); **0.6 ms por render** (2× sobre el motor
  runtime optimizado, 7.7× sobre el original). Diferido: include/layouts, regeneración en `ray
  build`, `{% let %}`.
- Diferido de fase 1: `{% include %}`/parciales (pide diseño de resolución: mapa de parciales en
  compile vs. filesystem), filtros. ~~`s[i]` O(1) en la VM~~ → ✅ **M90.6** (en AMBOS motores, sin
  cachear ni tocar la representación —la Opt.3 `Rc<str>` ya se midió y revirtió—: se elimina el
  `Vec<char>` completo que se asignaba POR ACCESO; ASCII indexa el byte en O(1) —también `len`—,
  no-ASCII escanea hasta `i` sin asignar; bucle `s[i]` sobre 64k chars: 37,9 s → 1,16 s, ~33×).

## 15. Cliente MongoDB — M54, PLAN

Análisis de factibilidad (jul 2026): **raylang puro, tier 2** (`packages/db/mongo.ray`), cero cambios
de compilador salvo el habilitador de bits de float (M54.1a, ya hecho). Punto fuerte de partida: la
autenticación de MongoDB moderno es **SCRAM-SHA-256 vía SASL** (`saslStart`/`saslContinue`) — el mismo
mecanismo que ya usa `db/postgres` → `net/scram` se reusa tal cual. El wire (**OP_MSG**: cabecera 16
bytes LE + flags + un documento BSON) es más simple que el de MySQL.

- **Superficie elegida**: `enum Bson` recursivo (no JSON strings: no hay parser JSON en el ecosistema
  y JSON pierde tipos — int64/double/ObjectId/binario). Un puente JSON queda como fase posterior si el
  ecosistema lo pide.
- **Cuidados fijados**: `_id` lo asigna el **servidor** (generar ObjectId en cliente exige
  aleatoriedad+tiempo → rompería el determinismo de los tests); `find` v1 usa `firstBatch` (cursores
  `getMore` diferidos); sin compresión (`OP_COMPRESSED` diferido); comando `hello` moderno.
- **Fases**: **M54.1** ✅ **COMPLETO** (DESIGN §56.1: (a) builtins `__float_bits`/`__float_from_bits`
  + `float_bits`/`float_from_bits` en std/math — habilitador del double IEEE 754, sirve también a
  protobuf; (b) `packages/db/bson.ray`: `enum Bson` (Double/Str/Doc/Arr/Bin/ObjectId/Bool/Null/Int) +
  `encode`/`decode` con errores como valores + `dump`; oráculo `tests/bson_cli.rs` contra los vectores
  canónicos de bsonspec.org + round-trip exacto) · **M54.2** ✅ **COMPLETO** (DESIGN §56.2:
  `db/mongo.ray` — framing OP_MSG + `run_command` + `connect` (`hello` + saslStart/saslContinue con
  `net/scram` reusado tal cual, verificación de la firma del servidor) + `disconnect`; toy server
  OP_MSG con las constantes SCRAM precomputadas de postgres en `tests/mongo_cli.rs`) · **M54.3** ✅
  **COMPLETO** (DESIGN §56.3: `insert`/`find` (firstBatch)/`update` ($set explícito)/`delete` sobre
  `run_command`; filtros = documentos BSON → anti-inyección por construcción; toy server extendido +
  demo `examples/db/mongo_demo.ray`). **M54 COMPLETO** (el paquete `db` cubre los 4: MySQL, PostgreSQL,
  SQLite, MongoDB). Post-M54 cerrados: getMore ✅ (DESIGN §56.5), puente Json↔Bson ✅ (§16), y el
  hilo TLS COMPLETO ✅ (primitivo `tls_upgrade` §57 + `postgres.connect_tls` §57.1 +
  `mysql.connect_tls` con full-path de caching_sha2 §57.2).
- **Impacto**: todo aditivo; BSON es el grueso (comparable al protobuf de M25 en naturaleza).
- **Diferidos del arco DB** (consolidado, jul 2026 — ninguno bloquea; el paquete `db` está
  funcionalmente completo para los 4 motores, con transporte cifrado donde importa):
  - **MySQL**: protocolo binario ✅ **CERRADO** (DESIGN §55.5: `query`/`exec` ganan `params` —
    prepare/execute/close de una ronda, fila binaria decodificada por tipo; con `[]`, texto como
    siempre). Quedan: sentencias con estado (cachear stmt_id), tipos binarios en los parámetros
    (hoy texto), full-path de caching_sha2 **sin** TLS (RSA; con `connect_tls` pierde casi todo el
    sentido), BIGINT UNSIGNED ≥ 2^63 (se muestra envuelto).
  - **Postgres**: parámetros binarios/tipados, sentencias preparadas con estado (hoy anónimas, una
    por ronda), COPY, `sslmode` negociable estilo libpq (hoy: `connect` = nunca TLS /
    `connect_tls` = obligatorio).
  - **SQLite**: `last_insert_rowid` ✅ (raylang puro, DESIGN §56.6) y WAL ✅ (ya posible:
    `query(c, "PRAGMA journal_mode=WAL", [])`). Queda: tipos nativos (celdas no-texto).
  - **MongoDB**: Date/Timestamp ✅ y `connect_tls` ✅ (DESIGN §56.6). Quedan: Decimal128 (error
    claro al decodificar), `batchSize` configurable + `killCursors`, compresión OP_COMPRESSED,
    Extended JSON riguroso ($oid/$numberLong) en el puente (§16).

---

## 16. JSON — huecos pendientes de `std/json`

`std/json` EXISTE y está completo en lo esencial (M15.4a, embebido desde `examples/web/json.ray`:
`enum Json` + `parse -> Result` + `stringify` canónico; lo usa `net/oauth2`). Huecos detectados
(jul 2026, al analizar la superficie del cliente MongoDB):

- ✅ **Escapes `\uXXXX`** — CERRADO (jul 2026): primitivo `__char_from_code -> [char]` (opcode
  `CharFromCode`; guard de rango contra el wrap del cast a u32) + `char_from_code -> Option<char>`
  en el prelude (el inverso de `char_code`) + los escapes en `std/json` con **pares surrogate**
  (astrales) y errores como valores (surrogate suelto, par incompleto, dígito no hex). Oráculo
  `char_from_code_oraculo` + test `escapes_unicode` en `tests/json_cli.rs`.
- **`JNum` es solo `float`**: fiel a JSON, pero un int64 > 2^53 pierde precisión. Irrelevante para
  APIs web; importa para un puente con BSON (abajo). Cambiarlo rompería a los usuarios del enum →
  decidir solo si el puente lo exige.
- ✅ **Pretty-print + helpers de acceso** — CERRADO (**M90.3**): `stringify_pretty(j, indent)`
  (multilínea con sangría, claves ordenadas como `stringify`, hojas compactas) y navegación sin
  `match` anidado: `member`/`at` (bajan un nivel → `Option<Json>`), `as_string`/`as_float`/`as_int`
  (integral, `3.5 → None`)/`as_bool`/`as_array`/`as_object`/`is_null` (extraen el payload), y los
  combinados `get_string`/`get_float`/`get_int`/`get_bool`/`get_array`/`get_object` (campo tipado
  de un objeto). UFCS: `j.get_string("nombre")`. Tests golden en `json_cli.rs`.
- ✅ **Puente `Json ↔ Bson`** — CERRADO (jul 2026): `bson.from_json` (número JSON → `Double`; las
  claves salen ordenadas, el objeto es Map), `bson.to_json` (degradación documentada: `Int` →
  número con pérdida > 2^53, `ObjectId`/`Bin` → hex, orden de campos perdido) y **`doc_from_json(s)
  -> Result<[Field], string>`** — la ruta ergonómica para filtros de mongo (`mongo.find(c, coll,
  bson.doc_from_json("{...}")?)`); el tope debe ser un objeto. Test `bson_puente_json` (compone con
  los escapes `\uXXXX`). Diferido: Extended JSON riguroso ($oid/$numberLong) si algún consumidor lo
  exige.

---

## 17. Webserver de producción — M56, PLAN (revisión jul 2026)

Revisión completa de la implementación (detalle y sub-fases en DESIGN §60). El **núcleo es
sólido**: la parte difícil (fibras aparcadas por fd, poller kqueue/epoll real con fallback,
escritura parcial con interés de escritura, pool M:N multicore de M38 con heaps aislados) vive en
el runtime genérico y está testeada (`webserver_cli`/`framework_cli`/`metrics_server_cli` +
concurrencia). HTTP es **librería pura** (`packages/net/webserver.ray` con docs `///` y
`html_response`; espejo histórico en `examples/web/webserver.ray` que usan tests y framework) →
casi todo el endurecimiento es raylang, barato y sin tocar el oráculo.

**Clasificación de impacto por hallazgo:**

| Hallazgo | ¿Dónde pega? | Impacto | Sub-fase |
|---|---|---|---|
| Sin límites de cabeceras/cuerpo; O(n²) del escaneo; cuerpo truncado silencioso; accept-loop muere al 1er error; sin tope de conexiones | Solo `webserver.ray` | **Ninguno** en runtime/API (límites con default + variante configurable; semáforo = `channel(n)` de M12.2) | **56.1** (primera: mayor ganancia de seguridad, cero decisiones abiertas) |
| Query pegada a `req.path` (el enrutado del framework NO casa con `?x=1`); sin percent-decoding | `Request` (campo nuevo `query`) + framework/demos | **Cambio semántico deliberado** de `path` (queda sin query); mecánico en consumidores | **56.2** |
| Sin `serve_tls` (https de servidor) | Solo `webserver.ray` (patrón ya probado en `wss_echo.ray`) | Ninguno | **56.3** |
| Sin timeouts (slowloris; fibra aparcada para siempre) | **Runtime**: deadline en `io_parked` + timeout en `poll::wait` | Acotado a la VM (el poller ya bloquea; añadir timeout no cambia semántica sin deadline). Diseño fino al llegar | **56.4** |
| Panic del handler fuga el fd (`close` no corre; no hay `recover`) | **Runtime**: builtin `try_join(t) -> Result<T,string>` (variante de `join` que no re-lanza) + `atender` en tarea | Un builtin pequeño, alineado con "errores como valores"; útil más allá del webserver | **56.5** |
| Siempre `Connection: close` (un handshake TCP por petición) | Solo `webserver.ray` (bucle por conexión honrando `Connection`) | Ninguno (framing por Content-Length ya correcto); cuidar `serve_raw`/SSE | **56.6** |
| `Map<string,string>` impide 2 `Set-Cookie`; pisa repetidas entrantes | **API de `Response`** (¿lista `[(k,v)]` o campo extra?) | **Rompe API** → decisión de diseño con el usuario | **56.7** |
| Chunked entrante, `serve_static` (con saneo `..`), HEAD sin cuerpo, `status_text` incompleto | Solo `webserver.ray` (`status_text` adelantado a 56.1) | Ninguno | **56.8** |

**Restricción hoy**: nada bloquea a nadie; solo 56.7 rompe API (decidir antes de 1.0/M44 para no
publicar una firma que haya que romper). 56.4/56.5 son los únicos toques de runtime y ambos son
aditivos.

---

## 18. Tiempo y fechas — M57, PLAN (revisión jul 2026)

Revisión completa del manejo de tiempo (tras cerrar M56). **El modelo de fondo es sano y no se
toca**: la moneda universal es `int` = **epoch-ms UTC** (`time.now()`), `monotonic()` para
intervalos (separación correcta y documentada), y `DateTime` es una *vista civil* para
formatear/parsear — no un instante (no lleva offset). Solo UTC por diseño (M20.5); sin leap
seconds (= tiempo Unix); pre-1970 no soportado (documentado).

**Aristas detectadas:**

| Arista | ¿Dónde pega? | Impacto | Sub-fase |
|---|---|---|---|
| `parse_iso8601` solo acepta la forma exacta `…Z`: un offset `+02:00` o `.123Z` (JSON de cualquier API) no parsea; campos no numéricos → **0 en silencio**; sin validar rangos | `net/time` (librería pura) | Corrección latente para todo consumidor de JSON | **57.1** (junto a la promoción a `std/time`) |
| Las fechas civiles viven en el paquete `net` pero las usan net/log, net/sigv4 y db/bson (cruce de paquetes) y no son "web" | Promoción `net/time` → **`std/time`** (política §53: universal/ligera/estable); `net/time` queda como reexport (`pub from`) | Compat total vía reexport | **57.1** |
| `sleep` bloquea el WORKER (`thread::sleep`, no cede): en M:1 congela todas las fibras; sin timer de fibra no hay timeout-de-handler (diferido M56.5), retries ni cron | **Runtime**: aparcar con deadline SIN fd (la maquinaria de M56.4) | Toque de VM acotado; el intérprete sigue bloqueante (documentado) | **57.2** |
| `uuid_v4` es aleatorio puro; falta **UUID v7** (timestamp ordenable — claves de DB, trazas) | `std/uuid` (junto al v4; usa `time.now()`) | Ninguno (aditivo) | **57.3** |
| `monotonic` es por-proceso (origen arbitrario): no comparable entre procesos ni persistible | Solo documentación (es correcto para su propósito) | — | 57.1 (doc) |
| Hora local (`__local_offset_millis` del SO, sin base IANA) | `std/time` (primitivo pequeño) | Aditivo | diferido a demanda |
| Webserver sin cabecera `Date:` (SHOULD de la RFC) | `net/webserver` | Ninguno | diferido (extra menor) |

**Candidatos a PACKAGE (tier 2, cuando haya demanda):** `tz` (zonas IANA — pesado, API propia;
viable en raylang puro leyendo los TZif de `/usr/share/zoneinfo` vía std/fs, cero deps; embeber
tzdata sería una decisión estilo ring) · `net/ntp` (cliente SNTP sobre net/udp: mide la deriva del
reloj sin tocar el del SO) · `cron` (expresiones + timers recurrentes, sobre el sleep de fibra de
57.2) · `dist` (relojes lógicos Lamport/HLC — solo si multi-nodo real).

**Restricción hoy**: nada bloquea. 57.2 es el único toque de runtime (aditivo, mismo esquema de
deadlines de M56.4). La promoción de 57.1 usa reexports → cero rotura.

---

## 19. Clientes web de producción (WebSocket · HTTP/1.1 · HTTP/2) — M58, PLAN (revisión jul 2026)

Revisión de `net/websocket[_client]`, `net/http` y `net/http2[_client]`+`net/hpack`+`net/grpc_client`
(detalle en DESIGN §62). Patrón común: **la criptografía y el framing (lo difícil) están bien y
verificados contra vectores RFC**; lo que falta es la robustez de red (buffering, timeouts, flow
control) que el SERVIDOR ya recibió en M56. Todo librería pura, cero runtime.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **WS: "una trama = una lectura"** — trama partida = OOB (muere la fibra), tramas juntas = la 2ª se descarta; sin ping→pong (peers estrictos cierran), sin close-handshake, sin límite de payload (longitud declarada de 2⁶⁰ = asignación sin tope), sin fragmentación | El hueco ESTRUCTURAL; rompe bajo condiciones de red reales | **58.1**: lector bufferizado `WsConn`+`read_frame`/`read_message` (rompe la API del cliente: `connect` pasa de devolver handle a `WsConn` — necesita estado entre lecturas) |
| HTTP/1.1: sin timeouts (cuelga para siempre); `Host` SIN puerto (bug de interop con vhosts `:8080`; el cliente WS sí lo pone); `Accept-Encoding` nunca se envía (el gunzip es código muerto en la práctica); cuerpo de petición `string` (sin binario); Content-Length de respuesta ignorado (truncado = aceptado en silencio) | Asimetría con el servidor M56 | **58.2** (decisión menor: cuerpo `bytes` vía firma nueva o `request_bytes`) |
| HTTP/2: **sin WINDOW_UPDATE** → respuesta > ~64 KiB (ventana inicial 65535) CUELGA para siempre; PING sin ACK (el servidor corta en transferencias largas); RST_STREAM ignorado (lee hasta EOF en vez de fallar) | Límite duro real de `http2_get`/`grpc_call` | **58.3** |
| Menores: WS `extract_key` case-sensitive; `recv_text` ignora el opcode; HTTP `absolutizar` con Location relativa sin `/`; cabeceras de respuesta pierden Set-Cookie múltiples; h2 sin CONTINUATION/END_HEADERS; `Http2Response` sin headers | — | dentro de su sub-fase o diferido |

**Diferidos** (a demanda): ~~keep-alive/pool del cliente HTTP~~ → ✅ **M90.2**: conexión persistente
explícita (`struct Conn` + `connect`/`conn_request`/`conn_request_bytes`/`conn_close`, patrón `Rd` del
RPC): delimita cada respuesta por Content-Length/chunked incremental (sin EOF) guardando los sobrantes,
honra `Connection: close`, reconecta perezoso y reintenta UNA vez transparente en la carrera del
keep-alive ocioso (el servidor cerró sin entregar ningún octeto). La API one-shot (`fetch`/`request*`)
sigue con `Connection: close`. Quedan a demanda: multiplexado h2 real, fragmentación WS de ENVÍO (la de
recepción entra en 58.1), y un pool multi-conexión sobre `Conn` si aparece un consumidor concurrente.

---

## 20. Librerías de datos de la std (json · regex · base64 · protobuf · csv · hex · url · StringBuilder) — M59, PLAN (revisión jul 2026)

Revisión de las 8 librerías de datos (detalle en DESIGN §63). Patrón común: la lógica dura
(NFA de Thompson, surrogates, RFC 4180) está bien y con tests golden; los huecos son de
**conformidad de RFC en los bordes**, **rigor de validación** y **rendimiento O(n²)**. Todo
librería pura, cero runtime.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **JSON no conforme a RFC 8259 en escapes**: el parse rechaza `\b`/`\f` (escapes LEGALES → rechaza JSON válido de terceros) y `quote` no escapa `\b`/`\f` ni los controles < 0x20 (un string con U+0001 dentro se serializa CRUDO → **emite JSON inválido**) | Corrección pura; interop real | **59.1** |
| **regex panica con patrón malformado** (5 sitios: `(` sin cerrar, `\` final…) — la ÚNICA de las 8 que viola "errores como valores"; además el patrón se RECOMPILA en cada llamada (`find_all`/`replace_all` incluidos) | Un patrón de usuario/config inválido tumba la fibra | **59.2**: `compile(pat) -> Result<Regex, string>` + las funciones actuales delegando (compat) |
| **base64 decode laxo**: ignora todo tras el primer `=` (`"QQ==basura"` → Ok) y no valida los bits sobrantes de la cola (dos encodings distintos → mismo payload) | Maleabilidad de representación bajo JWT/SCRAM | **59.3** (decode estricto) |
| **protobuf `encode_varint` con negativo = corrupción SILENCIOSA** (el bucle `v >= 128` no entra y emite `v & 127` mal); el diferido "sin negativos" está documentado pero debería ser `Err`, no bytes corruptos | Corrupción de wire sin aviso | **59.4** |
| **O(n²) transversal**: json/csv/hex/base64 construyen resultados con `s = s + …` por carácter — el `StringBuilder` (M40.3c) existe exactamente para esto y NINGUNA lo usa (son anteriores) | Cuadrático real en payloads de MBs | **59.5** (midiendo antes/después) |
| Menores: json `parse_number` laxo (`+1`, `01`) y sin límite de profundidad (lo corta `MAX_CALL_DEPTH`, indocumentado); csv separador fijo `,` (sin `;`/TSV) y basura tras comilla de cierre tolerada; regex sin grupos de captura/`{n,m}`/lazy; StringBuilder sin `clear()`; url `parse_query` last-wins con claves repetidas (documentado) | — | dentro de su sub-fase o diferido |
| **Decisión de API aparte (¿pre-1.0?)**: hex/base64/PbWriter hablan `[int]`, no `bytes` (pre-M16) — conversiones por doquier en jwt/scram/crypto y sin garantía 0..255; unificar rompe API de ~6 consumidores | Consistencia del ecosistema | **RESUELTA → M60** (DESIGN §64): hex/base64 a `bytes`, shims `*_octets` de net/crypto retirados, consumidores sobre `std/crypto` directo |

**Verificados sin hallazgo**: floats de json round-trip limpios (`42.0` → `"42"`, sin notación
científica), surrogates `\uXXXX`, salida canónica (claves ordenadas); url ya endurecida en M56.2.
**Bonus de lenguaje** (fuera del arco): el lexer NO soporta literales float con exponente (`1e21`
es error de sintaxis) → ✅ **RESUELTO en M80** (§40).

---

## 21. El prelude (revisión jul 2026) — M61, PLAN

Revisión de `src/prelude.rs` (detalle en DESIGN §65). La arquitectura está bien (todo librería
raylang, erasure, envoltorios [T]→Option uniformes, iteradores correctos, parse cacheado); los
hallazgos son dos defectos verificados con el binario y ergonomía faltante.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **`Hash` DESBORDA y revienta**: `h = h*31 + c` con el `int` checked (trap, no wrapping) → `.hash()` de un string ≥ ~12 chars panica → **`Set<string>` inutilizable con claves reales**; el combinador de `@derive(Hash)` igual (un campo int grande revienta) | Bug que revienta en runtime | **61.1**: acumular con máscara de 32 bits (`& 4294967295`) en el impl del prelude Y en `generate_derives` |
| **`sort` es insertion sort O(n²)**: 20k ints = **23 s** (medido, release) | El json-O(n²) de M59.5 pero en sort | **61.2**: merge sort en raylang puro (estable; `std/sort.merge` ya existe), midiendo |
| **Option/Result sin ergonomía**: no hay `unwrap_or`/`is_some`/`is_none`/`expect`/`ok_or` — todo es `match` (los packages están llenos de `match … Some(x) => x`); `bytes` sin `Eq`/`Show` (`assert_eq` sobre bytes no tipa, aunque `==` ya funciona); `[T]` sin `Eq` | La mejora de ergonomía más rentable pendiente | **61.3** (funciones libres genéricas + impls de una línea) |
| **DX de assert**: un `assert` fallido reporta la posición DEL PRELUDE (579:9), no el sitio del usuario — con varios asserts no sabes cuál falló | Inherente a "prelude = funciones ordinarias"; el fix real es posición-del-llamador o mini stack trace (toca runtime) | **idea aparte** (diseño de runtime; no entra en M61) → **M79** (stack trace, §39) |
| Menores: ~~`sum` solo `Iter<int>` (sin float)~~ (cerrado: `sum_float` ya existía en el prelude); ~~`Ord` para `bool`/`bytes` ausente~~ → ✅ **M90.5** (`false < true`; bytes lexicográfico) | — | ✅ |

**Verificados sin hallazgo**: envoltorios de I/O/Map correctos; `try_join` sin carrera; iteradores
(map/filter/take/skip/zip/enumerate/fold/collect) con semántica correcta y one-shot como Rust
(mismo caveat de consumo asimétrico en `zip`); insertion sort ESTABLE (el merge sort debe conservarlo);
arranque con parse cacheado (`OnceLock`).

---

## 22. Iteradores (revisión jul 2026) — M62, PLAN

Revisión del trait `Iterator`/`Iter` del prelude + la bajada del `for` (detalle en DESIGN §66).
Correctitud IMPECABLE (pereza, bordes, zip, iteradores de usuario con adaptadores heredados,
motores idénticos); el problema es de RENDIMIENTO y está medido (1M elementos, release).

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **M40.6 hizo lento el camino ergonómico**: las funciones libres eager `map`/`filter`/`fold` delegan en la maquinaria perezosa → `xs.map(f).fold(…)` = **36 441 ms** vs 107 ms del while (340×); como bucles directos: **317 ms** (115× de mejora disponible, semántica idéntica) | El código real usa `xs.map(f)`, no cadenas lazy | **62.1**: revertir las libres a bucles directos; lo lazy queda para `iter().…` |
| Semántica sutil sin documentar: `for x in xs` CONGELA `len` pero `for x in xs.iter()` es vista VIVA (mutar durante la iteración diverge entre formas); aliasing one-shot; `zip` pierde el elemento ya consumido del lado largo | Sorpresas evitables | **62.2** (libro m40 + gotcha en DESIGN; cero código) |
| **El `next()` cuesta ~6 µs/elemento** (medido: `for x in xs.iter()` pelado = 6 191 ms/1M): llamada a closure + ALOCACIÓN de un `Option` en el heap del GC + match por paso; cada adaptador apila otro tanto | Techo estructural del throughput lazy | ✅ **RESUELTO en Opt.13** (§11): el diagnóstico real era el **pacing del GC** (umbral por conteo re-escaneando el arreglo grande cada ~50 asignaciones), no el closure/Option (~0.31 µs/paso). Umbral amortizado por trabajo trazado → **6.8 → 0.4 s (17×)** |
| Faltan terminales comunes: `any`/`all`/`count` (3 líneas c/u sobre `next`/`fold`); ~~`find`/`chain`/`min`/`max` a demanda~~ → ✅ **M90.5**: `find` (terminal, corta en el primero) y `chain` (perezoso, `other: Iter<T>` como `zip`) como métodos por defecto del trait; `min<T: Ord>`/`max<T: Ord>` funciones libres (como `sum`/`sort`) → UFCS `it.min()`; y `impl Ord for bool` (false < true) y `bytes` (lexicográfico). Oráculo `find_chain_min_max_oraculo` | Ergonomía menor | opcional en 62.1 o diferido |

**Verificados sin hallazgo**: pereza con orden intercalado; `take` corta el consumo del origen;
`skip`/`take` con bordes (negativo, más allá del final); `enumerate` + `for` con tupla; iterador
de usuario hereda los adaptadores por defecto y funciona con `for`; la bajada `for_iter_sites`
por posición. El literal de struct en la cabecera del `for` cae en la ambigüedad
literal-vs-bloque (consistente con if/while; error claro).

---

## 23. std/toml (revisión jul 2026) — M63, PLAN

Revisión de `std/toml` (detalle en DESIGN §67). Contexto: **`ray.toml` NO usa este parser** (el
CLI tiene su lector TOML mínimo en Rust, `src/manifest.rs` — circular si no) → librería de
usuario, no ruta crítica del toolchain. Estructura sana (parser sobre `[char]`, errores como
valores, subconjunto documentado); los huecos, verificados con sondas:

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **Corrupción silenciosa de strings**: `"caf\u00E9"` → `"cafu00E9"` (el escape desconocido traga la barra y deja el resto); `\q` ilegal → se acepta; faltan `\b \f \uXXXX \UXXXXXXXX` (la clase M59.1 otra vez) | TOML legal corrompido sin aviso | **63.1**: escapes completos, desconocido = `Err`; + strings literales `'...'` (core TOML: rutas Windows, regex) |
| Números TOML legales rechazados: `1_000` (separadores `_`, lo MÁS común en configs), `inf`/`nan`; hex/octal/bin (menor) | Configs reales no parsean | **63.2** |
| Laxitudes que la spec prohíbe: `a = 1 b = 2` en una línea (exige salto tras el valor); clave duplicada aceptada y **`toml_get` devuelve la PRIMERA** (se espera error o last-wins); `[]` cabecera vacía resetea a raíz en silencio | Sorpresas silenciosas | **63.3**: duplicada = Err, `[]` = Err, salto obligatorio tras el valor |
| Menores: O(n²) por carácter (configs pequeños — impacto bajo); control chars crudos en strings; `toml_show` no escapa (debug, documentado) | — | dentro de su sub-fase o diferido |

**Diferidos que siguen** (documentados en el propio módulo): inline tables `{…}`, arrays de
tablas `[[…]]`, fechas, strings multilínea `"""…"""` — a demanda. Nota de raydoc pendiente: el
manifiesto usa otro parser.

---

## 24. Compresión (inflate/deflate/huffman) — M64, PLAN (revisión jul 2026)

Revisión del trío (detalle en DESIGN §68). `deflate` (encoder, datos propios) sólido; `huffman`
HPACK excelente (trie + errores como valores + relleno EOS validado). El problema es `inflate`,
que decodifica datos EXTERNOS (el gzip transparente del cliente HTTP, activado en M58.2):

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **Input corrupto/truncado = CRASH, no Err**: `read_bit` indexa sin límites ("asume datos suficientes"); ídem `stored_block`, el bucle FNAME de gunzip (`while (data[pos] != 0)` sin tope) y el salto FEXTRA (xlen del atacante). Verificado: `inflate_raw(b"")` → "índice fuera de rango", muere el programa | Una respuesta gzip corrupta tumba la fibra (misma clase que WS pre-M58.1) | **64.1**: bounds → `Err`; + NLEN verificado, FDICT rechazado, hlit/hdist acotados, repeticiones 16/17/18 sin rebasar |
| **Bomba de descompresión sin tope**: gzip diminuto → expansión sin cota (LZ77 sobre sí mismo); ISIZE no sirve (atacante); el cliente HTTP descomprime automático | Agotamiento de memoria remoto | **64.2**: `gunzip_limit`/`inflate_raw_limit` (patrón `read_message_limit` del WS) + el cliente HTTP los usa |
| Menores: árboles sobre-suscritos aceptados (puff valida Kraft; aquí basura en vez de Err, sin OOB); `huffman_decode` reconstruye el trie (~5k nodos) POR string de cabecera h2; ~~crc32 bit a bit~~ (✅ M90.4: tabla de 256 entradas construida por llamada —sin estado de módulo—, umbral < 256 octetos sigue bit a bit; ~4,5× medido en la VM); huffman/hpack hablan `[int]` entre sí (par interno); `prev` de deflate O(n) vs anillo 32K | — | **64.3** o diferido |

---

## 25. std/math (revisión jul 2026) — M65, PLAN

Revisión en frío (detalle en DESIGN §69). Lo verificado sano: dominios float totales IEEE
(sqrt(-1)→NaN, ln(0)→-inf, sin traps), `round` ties-away-from-zero como documenta, `gcd`/`lcm`
(divide antes de multiplicar)/`is_prime` correctos, `float_bits` total.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **`ipow` revienta con resultados que CABEN**: la exponenciación binaria hace el cuadrado final `b = b*b` que ya no necesita; con el int checked, `ipow(2, 40)` (=1.1e12) trap por 2^64 | Cualquier potencia con base²^⌈log e⌉ > 2^63 aunque el resultado quepa | **65.1**: saltar el cuadrado cuando `e` llega a 1 |
| **`min`/`max` contradicen su doc en empates**: doc "Ties return `a`", código devuelve `b` (observable con tipos de usuario vía `impl Ord`) | Sorpresa semántica; `max` no-estable | **65.1**: invertir la comparación (empate → `a`) |
| **Falta trig inversa y compañía**: sin `asin`/`acos`/`atan`/**`atan2`** (no se puede recuperar un ángulo de coordenadas), `log2`, `trunc` | Hueco de superficie para geometría/gráficos | **65.2**: 6 builtins mecánicos (fila en BUILTINS + opcode + impl por motor, patrón M11.4) |
| `clamp` solo-int (min/max ya genéricas `Ord`) | Asimetría menor | **65.3**: `clamp<T: Ord>` (retrocompatible) + docs de frontera (`factorial(≥21)`/`ipow` desbordante = trap del int checked) |

Aparte (ya fichado en §21): la posición del trap de `factorial`/`ipow` apunta dentro de
std/math, no al llamador — diferido general de posición-del-llamador → **M79** (stack trace, §39).

---

## 26. std/text (revisión jul 2026) — M66, PLAN

Revisión en frío (detalle en DESIGN §70). Corrección SANA (reverse por carácter con UTF-8
astral, capitalize no-ASCII, count no-solapado, pads por carácter). Módulo casi sin
consumidores reales (solo el smoke de cli_cli) → cambiar es barato.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **`reverse` y `count` O(n²)**: reverse concatena `out + char` en bucle (cada `+` copia el acumulado; 100k chars = 167 ms); count re-materializa el resto con `substring` por ocurrencia — y `substring` es O(i), el gotcha de M59.5 (100k chars / 10k ocurrencias = 879 ms) | Texto grande inutilizable | **M66**: acumular en `[string]`+`join` / buscar sobre `[char]` con offset |
| **`words` solo separa por espacio**: `words("a\tb\nc")` = 1 palabra (doc-honesto pero contradice el split_whitespace universal) | Sorpresa semántica | **M66**: whitespace = espacio/tab/`\n`/`\r` |
| **Falta `lines(s)`** (partir por `\n` tratando `\r\n`) — toml/csv/http la hand-rollean | La utilidad más pedida ausente | **M66**: `lines` nueva |

---

## 27. std/fs (revisión jul 2026) — M67, PLAN

Revisión en frío (detalle en DESIGN §71). Sano: errores como valores en todo (mensajes del
sistema propagados), list_dir determinista, exists total, handles con buffering, I/O binaria.
El hallazgo: **el módulo es solo-archivos; los directorios son de solo lectura** (verificado
a ambos niveles: ni envoltorio ni primitivo del host).

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **No hay `mkdir`**: desde raylang no se puede crear un directorio (write_file a un dir inexistente = Err sin remedio) | Scaffolders/cachés/sitios estáticos bloqueados | **M67** |
| **No hay `is_dir`/`is_file`**: list_dir da solo nombres → no se puede recorrer un árbol | Recorrido imposible | **M67** |
| **No hay `rename`**: la escritura atómica (temp + rename) es imposible | Toda escritura es ventana de corrupción | **M67** |
| Sin `copy_file`/`remove_dir`/`file_size`; `append_file_bytes` ausente (append solo-texto) | Kit incompleto | **M67** |

M67 de una pieza: 8 primitivos mecánicos (patrón M11.4; helpers compartidos en builtins.rs)
+ envoltorios Result en std/fs + integración por subproceso en io_cli. `remove_dir` solo
vacío (el recursivo es peligroso → a demanda). `write_file` sigue devolviendo caracteres
(documentado, coherente con `len()`).

---

## 28. std/random y aleatoriedad criptográfica (revisión jul 2026) — M68, PLAN

Revisión en frío (detalle en DESIGN §72). Sano: SplitMix64 canónico (53 bits limpios para el
float), Mutex de proceso (seguro bajo M:N), `below(n<=0)`→0 total, honestidad "no criptográfico"
en host/módulo/uuid. El sesgo de módulo de `below` (~n/2^64) es inmedible → no se toca.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **Sin `seed(n)`**: sembrado del reloj, no fijable — simulaciones irreproducibles, semillas no compartibles, tests de aleatoriedad no deterministas | Reproducibilidad imposible | **68.1**: primitivo `__random_seed` + `seed()` |
| **Falta el kit sobre `below`**: `between(lo,hi)`, `choice(arr)`, `shuffle(arr)` (Fisher-Yates) | Superficie mínima | **68.1**: raylang puro, cero opcodes; con `seed` → goldens deterministas |
| **Sin aleatoriedad criptográfica en NINGUNA parte**: ring disponible (M43) pero nadie expone SystemRandom — tokens de sesión/salts/nonces solo pueden salir del PRNG de reloj (predecible) | Secretos predecibles | **68.2**: `crypto.random_bytes(n)` (`__crypto_random_bytes` sobre ring::rand) + docs de uuid/websocket apuntando a ella |

---

## 29. Cliente Redis (revisión jul 2026) — M69, PLAN

Revisión en frío de `net/redis` (detalle en DESIGN §73). Estructura sana (errores como
valores, framing sobre buffer, recursión para arrays, toy server determinista en el test).

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **El framing RESP cuenta CARACTERES, no octetos** (todo en `string`): `encode_command(["SET","k","ñ"])` declara `$1` pero envía 2 octetos (verificado); `read_n` corta por caracteres el `$n` en octetos del servidor | **Desincronización del protocolo** con cualquier valor no-ASCII (basta una ñ); valores binarios imposibles | **M69**: framing interno 100% `bytes` (la migración M60 que no llegó aquí) |
| `int_or` tolerante (0 si falla): `$basura`/`:abc` → 0 en silencio | Enmascara errores de protocolo (clase toml pre-M63) | **M69**: `Err` claro |
| Sin tope de bulk: `$<gigante>` de un servidor roto acumula sin cota | Agotamiento de memoria (clase M64.2) | **M69**: tope generoso + `Err` |

API pública conservada: `command(c, args: [string])` codifica a bytes por dentro y
`Reply.Str` sigue siendo `string` (decodifica UTF-8; `?` si no) — cero ruptura para texto.

---

## 30. Observabilidad: log + metrics (revisión jul 2026) — M70, PLAN

Revisión en frío de `net/log` y `net/metrics` (detalle en DESIGN §74). Sano: log con orden
de claves fijo y `render(e, ts)` determinista/testeable; metrics con labels ordenados,
salida determinista, histograma cumulativo correcto y modelo lineal honestamente documentado.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **log: `json_escape` no cubre controles < 0x20** (solo \n/\t/\r): un mensaje con BEL/ESC/\x00 emite **JSON inválido** (verificado con sonda) | El agregador rechaza justo las entradas interesantes (las de datos raros); clase M59.1 | **M70**: controles → `\uXXXX` (RFC 8259) |
| metrics: el texto de `# HELP` no se escapa (`\\`/`\n` obligatorios en el formato de exposición) | Un help con salto de línea rompe el scrape entero | **M70** |
| metrics: sin chequeo de tipo — `observe_l` sobre un counter / `inc` sobre un histograma crean una serie espuria que corrompe la exposición en silencio | Corrupción silenciosa (ya hay panic para no-registrada; mismo trato) | **M70** |

---

## 31. Cookies seguras + sigv4 (revisión jul 2026) — M71, PLAN

Revisión en frío de `net/cookie`, `net/sigv4`, `net/oauth2` (detalle en DESIGN §75). oauth2
sano (errores como valores, state como responsabilidad del llamador, maneja el JSON de error
de OAuth). sigv4 sólido en lo estructural.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **cookie: `set_cookie` no valida nombre ni Path** → un `\r\n` inyecta cabeceras (HTTP response splitting; verificado con sonda: `Set-Cookie: admin=true` inyectado). El VALOR sí se url-codifica (protegido) | Vulnerabilidad de inyección si el nombre/path viene de datos externos | **M71** |
| **cookie: falta `SameSite`** (Strict/Lax/None), el atributo anti-CSRF de facto (HttpOnly/Secure sí están) | Hueco de superficie de seguridad | **M71** |
| sigv4: los valores de cabecera canónica no colapsan espacios internos (SigV4 exige secuencias→1; solo hace trim) → firma inválida con cabeceras de doble espacio | Bug de FIRMA (403 de AWS), no seguridad | **M71 (menor)** o diferir |
| sigv4: el path no se URI-encodea (correcto S3, incorrecto el resto) | Corrección de firma no-S3 | DIFERIDO documentado |

---

## 32. Cliente DNS (revisión jul 2026) — M72, PLAN

Revisión en frío de `net/dns`, `net/dns_cache`, `net/udp` (detalle en DESIGN §76). udp sano
(solo traduce el arreglo etiquetado a Result/Packet). dns_cache sano (TTL respetado, búsqueda
lineal aceptable para cachés pequeñas). El parser DNS procesa datos EXTERNOS y tiene la misma
clase de problemas que inflate pre-M64, más un vector de spoofing clásico.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **Cero bounds en el parser**: `be16`/`be32`/`read_name`/`format_ipv4`/`format_ipv6`/`read_txt` indexan sin comprobar. Verificado: respuesta truncada → crash "índice fuera de rango" | Una respuesta corrupta/truncada tumba la fibra (clase inflate pre-M64) | **72.1**: bounds → `Err`, nunca crash |
| **`read_name`: bucle infinito con punteros de compresión maliciosos** — un puntero 0xC0 que cicla nunca termina. Verificado: timeout | DoS (respuesta de 20 octetos cuelga el resolver) | **72.1**: límite de saltos de puntero |
| **ID de transacción FIJO (4660)** + no se valida el ID de la respuesta contra la consulta | Spoofing/cache-poisoning trivial (ahora hay `crypto.random_bytes`, M68.2) | **72.2**: txid aleatorio + validación |

---

## 33. Cliente gRPC / HTTP/2 (revisión jul 2026) — M73, PLAN

Revisión en frío de `net/grpc_client` (detalle en DESIGN §77). Sano: framing HTTP/2 (procesa
solo frames completos), flow control, ACK de PING, RST/GOAWAY=Err con causa (M58.3), errores
como valores, HPACK.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **`body`/`buf` crecen sin cota**: frames DATA indefinidos → agotamiento de memoria; `frame_size` acepta un length de hasta 16 MB (24 bits) sin validar contra el max-frame de 16 KB | Bomba de memoria remota (clase M64.2/read_message_limit) | **M73** |
| **`tuvo_grpc_status` variable MUERTA**: se computa pero nunca se lee → un servidor que NO manda `grpc-status` en los trailers pasa como `Ok(grpc_status: 0)`, indistinguible de OK | Error de protocolo silenciado | **M73** |

El mismo tope de acumulación aplica al **`http2_client.ray`** hermano (comparte el patrón
body=body+payload sin cota) → se arregla en paralelo.

---

## 34. JWT — HS256 y EdDSA (revisión jul 2026) — M74, PLAN

Revisión en frío de `net/jwt` (HS256) y `net/jwt_eddsa` (EdDSA). Sano y bien pensado: el orden
correcto (verificar la firma ANTES de decodificar/usar el payload), comparación de firmas en
tiempo casi constante (`const_eq`, recorre toda la longitud sin cortar en el primer byte),
base64url estricto (M59.3: rechaza bits sobrantes no nulos), y —clave— **ambos verificadores
recomputan siempre con su algoritmo hardcodeado** (HS256 / EdDSA), así que un `alg:none` o un
token de otro algoritmo YA fallan la comparación de firma (verificado: `alg:none` forjado →
`Err`). No había agujero vivo.

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **El campo `alg` del header NUNCA se valida**: se usa el header verbatim en el input de firma pero nunca se decodifica ni se comprueba. La clase de defecto canónica de JWT (raíz de CVE tras CVE: alg-confusion, alg:none). Hoy no explotable (recompute hardcodeado, fail-closed), pero **frágil**: un refactor que leyera el `alg` del header lo volvería catastrófico, y un token de otro propósito/algoritmo se acepta si por casualidad es HS256/EdDSA válido | Fragilidad de seguridad / defensa en profundidad | **M74** |

Fix (defensa en profundidad): decodificar el header (base64url → UTF-8 → JSON con `std/json`) y
exigir que `alg` sea exactamente el esperado (`"HS256"` / `"EdDSA"`), rechazando con mensaje claro
antes de dar por buena la firma. Espejos `packages/net` ↔ `examples/web` juntos (jwt + jwt_eddsa).
Diferido (documentado, no defecto): `jwt_eddsa_sign` con un seed de longitud errónea emite una
firma vacía (degradación honesta, no puede fallar limpio sin cambiar el tipo de retorno);
`exp`/`nbf` siguen siendo política de la aplicación sobre el JSON devuelto.

---

## 35. SCRAM-SHA-256 (revisión jul 2026) — M75, PLAN

Revisión en frío de `net/scram` (el mecanismo de auth que reusan `db/postgres`, `net/postgres` y
`db/mongo`). Bien pensado el núcleo: `bytes` de punta a punta, PBKDF2 correcto (INT(1) BE, U1 y
XOR acumulado), `scram_verify` en tiempo constante (OR-acumulación + chequeo de longitud), orden
correcto. Pero cuatro huecos frente a RFC 5802/7677:

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **El nombre de usuario NO se escapa** (`scram_first` concatena `"n=" + username` verbatim): RFC 5802 §5.1 exige `,` → `=2C` y `=` → `=3D`. Un usuario con `,`/`=` corrompe el mensaje o **inyecta atributos** (`r=`, `n=`) | Inyección en el protocolo (clase cookie M71) | **M75** |
| **El nonce del servidor NO se verifica**: RFC 5802 §5.1 (MUST) exige que el `r=` del server-first EMPIECE por el nonce del cliente; `scram_final` lo extrae pero nunca lo comprueba → server-first no ligado a la sesión (replay/MITM) | Salta un MUST de la RFC (replay/MITM) | **M75** |
| **El recuento de iteraciones no tiene tope**: `i` lo fija el servidor; un valor enorme haría girar PBKDF2 sin fin en el cliente | Bomba de CPU remota (clase M64.2) | **M75** |
| **`scram_verify` acepta `server_sig` vacío**: si `scram_final` no corrió/falló, la firma esperada es `b""`; un servidor que mande `v=` (base64 vacío) casa con longitud 0 → `true` | Verificación falsa-positiva defensiva | **M75** |

Fix: `escape_saslname` (=3D primero, luego =2C) en `scram_first`; verificar `rnonce.starts_with(
client_nonce)` (el nonce del cliente vive en `client_first_bare`); acotar `1 <= i <= 10_000_000`
(no se impone el mínimo 4096 de la RFC para no romper el toy-server a i=64); guarda de `server_sig`
vacío. Regresión rápida (`scram_reject_demo`, retorna antes del PBKDF2) por ambos motores; el vector
RFC 7677 (`#[ignore]`) sigue byte-idéntico. Espejos `packages/net` ↔ `examples/web` juntos.

---

## 36. Clientes de BD — MongoDB + PostgreSQL (revisión jul 2026) — M76, PLAN

Revisión en frío de los parsers de wire de `db/bson`, `db/mongo`, `db/postgres` y `net/postgres`
(legacy). La misma clase de defectos que Redis (M69)/DNS (M72): datos del servidor sin validar →
crash, bomba de memoria o desincronía. Sano: `bson.decode` acota TODOS sus reads contra `b.len()`
(sin OOB ni bomba de asignación), sin bucle infinito. Pero:

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **`bson.decode` recurre sin límite de profundidad**: doc/arreglo anidados (0x03/0x04) → `rd_doc`↔`rd_value` agotan la pila. Un BSON de ~4.8 KB (600 niveles) tumba al cliente con "desbordamiento de pila" en vez de `Err` | DoS por respuesta pequeña (clase salto-DNS M72) | **M76** |
| **`mongo.read_msg` sin tope de tamaño**: el `total` del header (hasta ~4.29e9) no se acota → un servidor malicioso declara un `total` gigante y el bucle acumula octetos del socket sin fin | Bomba de memoria remota (clase Redis M69) | **M76** |
| **`postgres` (db+net) `read_msg` sin tope**: idéntico a mongo (`mlen` del header sin cota → acumulación sin fin); además `mlen < 4` no se valida → `total < 5` deja `sub_bytes(5, total)` con fin < inicio | Bomba de memoria + crash por longitud inválida | **M76** |
| **`postgres.parse_datarow` NO detecta NULL**: el marcador -1 (0xFFFFFFFF) se lee con `be32` (sin signo) como 4294967295 → la comprobación `vlen < 0` falla → `sub_bytes(pos, pos+4.29e9)` **revienta el cliente ante CUALQUIER columna NULL** (bug vivo, no solo servidor malicioso). Sin chequeos de límites del payload | Crash en uso normal (NULL es ubicuo) + OOB por payload truncado | **M76** |

Fix: tope de profundidad (`max_depth`=200, supera el ~100 de MongoDB) en `bson`; tope de mensaje
(`max_message`=64 MiB) en `mongo`/`postgres` (rechazo al leer la cabecera, antes de acumular);
`mlen >= 4`; `parse_datarow` reinterpreta la longitud como int32 CON SIGNO (−1 → NULL → ""), valida
los límites del payload y **falla como valor** (`Result`). Regresión: BSON de 600 niveles = `Err`
(bson_cli), y una columna NULL del toy-server postgres = "" (postgres_v2_cli). Sin espejos (los
paquetes `db`/`net` no son embebidos). **Diferido a M77**: `mysql` (`lenc_int` lee `p[i+1..i+8]` sin
chequear límites → OOB en paquete truncado; el caso de 8 octetos puede desbordar el i64; sin bomba
ilimitada porque el largo de paquete son 3 octetos, ≤ 16 MB) y `sqlite` (fichero LOCAL → otro modelo
de amenaza).

---

## 37. Clientes de BD — MySQL + SQLite (revisión jul 2026) — M77, CIERRA el cluster db

Cierra la revisión en frío de `packages/db` (tras M76). **`sqlite` es SANO**: no parsea binario no
confiable (rusqlite lo hace en C, memory-safe y probado en batalla), parámetros enlazados aparte
(anti-inyección), errores como valores; los únicos `p[i]` operan sobre el arreglo etiquetado del
primitivo del HOST (confiable). Nada que arreglar. **`mysql`** era la excepción del cluster:

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **Lecturas OOB pervasivas ante paquetes malformados del servidor**: ~27 accesos `p[i]` sin chequeo de límites en ambas rutas de fila (texto COM_QUERY + binaria preparada), el handshake, `col_type_flags`, `int_le`, `dec_*`, y el bucle length-encoded de `bin_cell` (`while j < ln.0` con `ln.0` hasta 2^64) → un servidor malicioso/roto o un paquete truncado **tumban al cliente con un trap** ("índice fuera de rango"). El caso de 8 octetos de `lenc_int` además puede desbordar el i64 | DoS por servidor malicioso (trapea seguro, sin corrupción/RCE; acotado a 16 MB por el largo de paquete de 3 octetos) | **M77** |

Fix (refactor de endurecimiento, elegido con el usuario): accesor `at(p, i) -> Result` con chequeo
de límites; los ~10 helpers de parseo (`lenc_int`/`nul_str`/`int_le`/`int_cell`/`dec_date`/
`dec_datetime`/`dec_time`/`col_type_flags`/`bin_cell`) pasan a `Result` y propagan con `?` por ambas
rutas de fila + el handshake; `lenc_int` de 8 octetos se arma por mitades y rechaza longitudes
absurdas (>= 2^32) → sin overflow ni OOB; `read_packet` rechaza un paquete de carga vacía (cierra los
`p[0]` de golpe); `stmt_prepare` valida los 12 octetos fijos del OK. Regresión: una fila con un
length-encoded truncado = `Err`, no crash (`mysql_cli`, rama TRUNC). El NULL binario ya se manejaba
(bitmap) → no había bug vivo como el de Postgres. **Cluster db CERRADO** (M76 mongo/postgres + M77
mysql/sqlite). Sin espejos (los paquetes `db` no son embebidos).

---

## 38. HPACK — decodificación robusta (revisión jul 2026) — M78

Revisión en frío de `net/hpack` (compresión de cabeceras HTTP/2, RFC 7541). El `decode` procesa
bloques del PEER (no confiables), lo consumen `http2_client`/`grpc_client`. Sano: la tabla estática
+ dinámica con evicción por tamaño, `table_lookup` acotado, el codificador solo emite literales
crudos válidos. Pero el decodificador tenía la superficie clásica de bombas de HPACK:

| Hallazgo | Impacto | Sub-fase |
|---|---|---|
| **`dec_int` sin límites ni tope**: la lectura de continuación `data[p]` no chequea `p < len` (entero truncado → OOB), y `shift` crece 7 por octeto sin cota → un bloque de octetos 0xFF lleva `shift` a millones y `(b & 127) << shift` **desborda el i64** (trap). RFC 7541 §5.1 exige limitar el tamaño del entero | DoS por entero truncado/gigante (trap) | **M78** |
| **`dec_str` con longitud sin validar**: `sub_bytes(lr.next, lr.next + lr.value)` con `lr.value` del peer → OOB si excede el bloque; además leía `data[pos]` sin chequear el fin | OOB por string sobredimensionado (trap) | **M78** |
| **Actualización de tamaño de tabla sin tope**: `set_max_size(h, ir.value)` acepta cualquier valor; RFC 7541 §6.3 lo limita al tamaño anunciado por SETTINGS (4096). Sin tope, un peer sube el máximo y la tabla dinámica crece **sin evicción a lo largo de la conexión** | Bomba de memoria de la tabla dinámica | **M78** |

Fix: `dec_int` pasa a `Result`, chequea `p < len` en cada continuación y corta con error si `shift >
28` (entero fuera de cualquier uso legítimo, antes de desbordar); `dec_str` valida `pos < len` y
`lr.next + lr.value <= len` antes de `sub_bytes`; la actualización de tamaño rechaza `> 4096`. Los
llamadores de `dec_int` en `decode` propagan con `?`. Regresión: entero truncado, string
sobredimensionado, size-update > 4096 y bomba de varint = `Err`, no crash (`cli_cli`,
`paquete_net_hpack_decode_malformado`); el round-trip legítimo y el e2e HTTP/2 siguen verdes.
Espejos `packages/net` ↔ `examples/web` juntos. El Huffman de decodificación sigue diferido
(rechazado con error claro, como antes).

---

## 40. Diferidos de lenguaje (jul 2026) — arco M80+

Cierre de los diferidos de lenguaje acumulados, uno por hito, en orden de coste/beneficio:

| Diferido | Alcance | Hito |
|---|---|---|
| **Literales float con exponente** (`1e21`, `1.5e-3`, `2E+10`; venía de §20). Lexer de Rust + lexer auto-alojado EN ESPEJO (oráculo byte-idéntico) + SPEC/DESIGN §3.4. Guarda conservadora: `e` solo con dígito (o signo+dígito) detrás. Bonus: 2 exclusiones pre-existentes que faltaban en el corpus del parser auto-alojado (log.ray bitops M70, redis.ray bytes M69) | lexer ×2 + docs normativos | ✅ **M80** |
| **Regex II**: grupos de captura (`captures`/`captures_str`, `(?:)` no captura), `{n,m}` (expansión en el parser, tope 512), lazy (`*?` `+?` `??` `{n,m}?`). Motor subido de Thompson a **Pike VM** (hilos con ranuras + prioridad) → semántica **leftmost-first** (Perl); goldens previos intactos. DESIGN §84 | `examples/stdlib/regex` (librería pura) | ✅ **M81** |
| **Claves de usuario en mapas** (trait `Hash`; venía de M13). **Decisión (con el usuario): `std/collections/dict`** — `Dict<K: Hash+Eq, V>` en raylang puro (buckets+rehash, hermano del Set), cero runtime; el `Map` builtin queda para claves primitivas (rápido, `keys()` ordenadas). Regla práctica documentada en el módulo | `std/collections/dict` | ✅ **M82** |
| **Bytes mutable** (venía de M16) | sigue **a demanda** (decidido con el usuario, jul 2026): sin consumidor real; la señal sería alguien construyendo bytes por concatenación en caliente → entonces un `BytesBuilder` en std/collections (como StringBuilder con el O(n²) de strings), NO mutar el modelo de valores | 💤 |

---

## 41. Registro multi-publicador — análisis de diseño (jul 2026) — M83/M84

Análisis de los diferidos de M51 (§54.7: UI web, firmas, mirrors, namespaces). **La base
condiciona todo**: el lock verifica sha256 (→ quien SIRVE los bytes no importa, solo quien
nombra el hash), el historial git del índice es un log append-only (cuasi-transparencia
gratis), y el parser del índice ignora claves desconocidas DENTRO de `[versión]` (extensible)
pero **erraría con claves a nivel de archivo** → los metadatos de paquete van en un archivo
**sidecar**, nunca en el TOML existente.

| Pieza | Diseño fijado | Disparador | Hito |
|---|---|---|---|
| **Manual del flujo de publicación** — documentar de punta a punta lo que YA existe: crear un paquete (`ray.toml`, cara pública), versionado semver + pre-releases, `ray publish` (check semántico, tag, hash), `ray add`/`search`/`remove`/`update`/`yank`, índice propio (`RAY_INDEX`/`[registry] index`), el lock y sus garantías. En el MANUAL (§ nueva "Publicar un paquete") o un `PUBLISH.md` dedicado + capítulo del libro | necesario YA (la maquinaria existe desde M51 y no está contada en ningún sitio) | ✅ **M83a** (`PUBLISH.md`: paquete/cara, índice, publish paso a paso con sus 5 validaciones, tabla de rangos+pre-releases, yank, garantías Y límites honestos —sin owners/firmas aún, apunta a M83b/c—, receta de punta a punta; enlazado de MANUAL §11 y README) |
| **Namespaces con dueño** — la propiedad cabalga sobre git hosting (sin cuentas propias): publicar para terceros = **PR al repo del índice**; sidecar `<nombre>.owners.toml` (dueños + claves públicas; la PRIMERA publicación reclama el nombre); enforcement = check de CI del repo del índice (`ray index-verify`: el PR solo toca paquetes cuyo owners incluye al autor) + branch protection. Descartado: scopes sintácticos `@user/pkg` (tocan gramática/imports/manifiesto; el owners-file da lo mismo) | abrir el índice a terceros | ✅ **M83b** (`read/write_owners` en index.rs — reclamación TOFU en la primera publicación firmada, pisar = Err; `ray index-verify` audita; la mitad PR-autor queda en el hosting) |
| **Firmas de publicación** — Ed25519 sobre `(nombre, versión, hash)` (`ring` ya enlazado M43; verificable incluso en raylang puro M30.2), NO sigstore (servicios externos, contra la filosofía). `ray publish --sign` añade `sig = "ed25519:…"` en `[versión]` (los clientes viejos la ignoran); la pubkey vive en el owners.toml de M83b, fijada por **TOFU** en la primera publicación (patrón del hash del índice M51d); el log de transparencia es el historial git. Firmas y namespaces son la MISMA feature de confianza → un solo hito | junto con M83b (paquete "confianza multi-publicador") | ✅ **M83c** (`ray keygen` + `publish --sign` — Ed25519 sobre `nombre@versión:hash` reusando builtins::ed25519_* de M43; verificación en `resolve_pinned` ANTES de descargar: firma inválida corta, sin-firma-con-dueño avisa, firma-sin-dueño = Err; 3 tests en registry_cli 23/23; PUBLISH.md §6bis) |
| **UI/búsqueda web** — sitio ESTÁTICO generado desde el índice (TOMLs + READMEs → HTML + búsqueda client-side), publicado por una Action del repo del índice en Pages; sin servidor ni BD. Oportunidad de dogfooding: el generador EN raylang (templates M55 + std/toml + std/fs + json) | la ola de hosting de M44 (libro+playground+SPEC; la UI es el 4º inquilino) | ✅ **M84** (`tools/registry_site.ray` — raylang puro sobre std/fs+StringBuilder, mini-parser propio del formato del índice —std/toml anidaría `[1.2.0]` por los puntos—, insignias retirada/firmada + dueño de M83, búsqueda client-side; salida determinista, golden byte-idéntico ambos motores en `tests/registry_site_cli.rs`; snippet de Pages en PUBLISH.md. Fuera de examples/ para no entrar al corpus selfhost) |
| **Mirrors de paquetes** — el hash ya hace los mirrors *trustless* (solo hace falta disponibilidad). Mirror del ÍNDICE ya existe (`RAY_INDEX` a cualquier clon). Falta solo la regla de reescritura `[registry] mirror = "prefijo"` (~30 líneas en `deps::ensure`; el hash verifica igual). OJO: un mirror NO es otro índice (mismo índice, otra URL) — mantener la distinción evita reabrir la dependency-confusion mitigada en M51e | a demanda (CI tras firewall / caída del hosting) | ✅ **M90.1**: `[registry] mirror = "prefijo"` / `RAY_MIRROR` reescribe la descarga a `prefijo/<url-sin-esquema>` (estilo proxy de Go), con fallback a la URL original si el mirror falla; el lock/MVS siguen viendo la URL original (el mirror es transporte, no identidad). Tests offline en `registry_cli` |

**Nada de esto es pre-1.0** salvo M83a (documentación de lo existente). Único "hazlo ya"
negativo: NO poner metadatos a nivel de archivo en los TOML del índice (romperían clientes
viejos); sidecar cuando llegue.

---

## 42. Librerías de tiempo II — hora local y compañía (jul 2026) — M85+

Análisis de los candidatos diferidos de M57 (§18). Base: `std/time` UTC (Hinnant, ISO 8601,
**RFC 1123 ya existe**), `sleep` cooperativo de fibra (M57.2), `net/udp`, `bytes`.

| Pieza | Diseño fijado | Cuándo | Hito |
|---|---|---|---|
| **`Date:` del webserver** (SHOULD RFC 7231) | `time.to_rfc1123(time.now_utc())` en `send_response_keep`, ambos espejos; los tests usan `contains` → solo se añade la aserción de presencia+formato | ya | ✅ **M85a** |
| **`packages/tz`** — hora local IANA | Parser **TZif v2** en raylang puro leyendo `/usr/share/zoneinfo` vía std/fs (cero deps; formato binario tamaño-DNS). Decisiones: (1) **la ambigüedad DST es API**: `to_utc(civil)` devuelve `enum LocalResult { Single(int), Ambiguous(int,int), Gap }` (estilo chrono; errores como valores); (2) zona del sistema sin primitivo: `env("TZ")` y fallback leer `/etc/localtime` POR CONTENIDO (es un TZif válido); (3) Windows honesto: `load` → Err claro, UTC sigue. Fixtures TZif commiteados + goldens de transiciones DST (determinista, ambos motores). Tier 2 (política §53). Embeber tzdata = decisión estilo ring, solo a demanda | siguiente | ✅ **M85** (`packages/tz/tz.ray`, ~370 líneas; `tests/tz_cli.rs` golden con fixtures Madrid/NY/UTC: invierno/verano, gap, solape con abreviaturas, round-trip, errores; ✅ **M85b**: footer TZ-string —parser POSIX `STD off DST[off],Mm.w.d[/t],Mm.w.d[/t]` + resolución por reglas perpetuas tras la última transición; golden 2100 incl. gap/solape en territorio del footer) |
| **`cron`** | dos mitades: `next_after(expr, civil) -> civil` PURO (goldens) + runner sobre spawn+sleep cooperativo. v1 **solo UTC** (la trampa: cron local correcto necesita tz — gap → siguiente hora válida, solape → solo la primera); tz-aware tras M85 | tras tz (o antes, UTC-only) | ✅ **M86** (`packages/cron/cron.ray`; next_after salta por campos —mes→día→hora→minuto—, quirk vixie DOM/DOW=OR, alias `@…`, imposible=Err a ~5 años; golden `tests/cron_cli.rs` ambos motores. Diferido M86b: cron en hora local sobre tz) |
| **`ntp`** (SNTP v4 sobre net/udp, ~150 líneas, toy-server como DNS) | especificado | a demanda | ✅ **M90.7** (`packages/net/ntp.ray`: `query(host, port) -> Result<NtpResult, string>` con hora del servidor + offset + delay en ms Unix y stratum; cálculo clásico de 4 marcas t1–t4; rechaza kiss-of-death, modo foráneo, transmit a cero, respuesta corta; bloqueante como `udp.recv_from`. Toy-server determinista en `tests/ntp_cli.rs`, ambos motores) |
| **`dist`** (HLC ~40 líneas sobre now()) | especificado | multi-nodo real | 💤 |

---

## 43. DX del checker: diagnóstico del gotcha §55 (jul 2026) — M87

El punto de fricción de gramática MÁS recurrente (mordió 3 veces en una sola sesión
escribiendo raylang nuevo, y es la clase de error del compromiso literal-vs-bloque): una
cola que empieza con `(` o `[` tras un if/while/match-sentencia se parsea como LLAMADA /
INDEXACIÓN de su valor, y el error genérico ("no se puede llamar un valor de tipo unit")
despista. **M87** ✅: cuando el callee/indexado es una expresión de BLOQUE (if/match/
while/bloque), el error lleva la pista ("…se parsea como llamada a su valor — sepárala
con 'return' o 'let'"), **byte-idéntico en ambos checkers** (Rust + auto-alojado, con
casos nuevos en el corpus del oráculo). Cero semántica: solo el mensaje.

---

## 44. Microservicios nativos (distribuido-ready v1) — arco M88

**Objetivo (fijado con el usuario, jul 2026)**: que raylang sirva DESDE EL PRIMER DÍA para
microservicios y su comunicación, de forma nativa — **sin salirse del dominio del
lenguaje**. Base ya existente: transporte completo no bloqueante, webserver endurecido
(M56), gRPC/h2 cliente, DNS, protobuf/json, cripto, clientes de BD, log+metrics, actores
M:N y el modo determinista (simulation testing gratis).

**FUERA del arco a propósito** (dominios ajenos o a demanda): consenso/Raft (coordinación
= usa los clientes de Redis/Postgres: SET NX+TTL, advisory locks — patrón a documentar),
service discovery más allá de DNS, mTLS (diferido), orquestación, ntp/dist (§42).

| Pieza | Diseño | Hito |
|---|---|---|
| **Señales del SO + apagado ordenado** — el hueco nº1 (hoy un servicio no puede drenar al recibir SIGTERM) y el ÚNICO toque de runtime del arco. `signals() -> Channel<int>` (builtin, solo VM como toda la concurrencia): un canal que recibe SIGTERM/SIGINT — compone NATIVO con `select` (drenar el canal de trabajo O apagar). Host: handler `extern "C"` (precedente poll.rs, cero deps) async-signal-safe con **self-pipe** (escribe el signo; el fd del pipe se registra en el poller → despierta el scheduler incluso con todo aparcado; el EINTR ya se maneja de M17); el scheduler drena el pipe y hace send al canal. Fibras aparcadas en el canal de señales NO cuentan como deadlock (esperan al exterior). GC: el canal es raíz como cualquier canal. Intérprete: error limpio (como spawn). | ✅ **88.1** (`signals()` builtin+opcode; self-pipe con fcntl VARIÁDICO —declararla de aridad fija era UB en arm64, el bug de la sesión—; entrega en poll_next por bandera atómica + fd en el poller de io_wait; todo-aparcado-con-señales ≠ deadlock; 4 tests en `signals_cli` incl. el patrón select(trabajo, señales); ejemplo `senales.ray`). ✅ **88.1b**: apagado ordenado del webserver — `serve_graceful(host, port, drain_ms, handler)` (sobre `signals()`) y la forma general `serve_shutdown[_limits](…, stop: Channel<int>, drain_ms, …)` (testeable sin señales). `bucle_servidor_drenable`: el accept en una BOMBA (fibra) que entrega por un canal `conns` → el bucle principal hace `select(stop, conns, done)` (imposible sobre el accept bloqueante); contador de activos LOCAL a la fibra del serve + canal `done` (modelo de actores M38: cero estado compartido); al apagar cierra el listener (clientes nuevos: rechazo) y drena con plazo (canal `plazo` por timer-fibra); la bomba muere en silencio observando un canal `apagado` CERRADO (el gotcha de M12.4 a favor). 3 tests en `webserver_shutdown_cli` (stop por canal, SIGTERM drena la en-vuelo, el plazo no espera para siempre) |
| **Kit de resiliencia** `std/resilience` — retry con backoff exponencial + jitter (sobre sleep cooperativo + random), circuit breaker, helper de deadline por operación. Raylang puro, genérico sobre `fn() -> Result<T,E>` | ✅ **88.2** (`std/resilience` embebida: `policy`/`retry` —primer `Ok` o ÚLTIMO `Err`, backoff duplicando con tope + jitter uniforme—; `breaker`/`guard`/`is_open` —fail-fast con `err_open` del llamador porque `E` es suyo; semiabierto al vencer el cooldown—; `Deadline` como PRESUPUESTO consultable (`remaining`/`expired`, sin preempción: se aplica de verdad en la E/S vía `net.set_read_timeout`). 6 tests en `resilience_cli`, ambos motores) |
| **Tracing distribuido** — propagación W3C `traceparent` (cliente http + webserver: Request gana el trace-id entrante o genera uno; el cliente lo propaga) + correlación en `net/log` (campo trace_id). Librería pura | ✅ **88.3** (`net/trace`: `Trace`+`new_trace`/`child`/`traceparent`/`parse_traceparent` estricto (2-32-16-2, minúscula, versión 00, sin todo-ceros)/`from_headers`; ids del PRNG sembrable —identifican, no autentican, como uuid_v4—. `webserver.trace_of(req)` adopta o crea; `http.request_traced`/`fetch_traced` propagan un span HIJO por salto (copia el Map de cabeceras: no muta el del llamador); `log.with_trace` estampa `trace_id` tras `service` (omitido si "" → retrocompatible). Tests `trace_cli`: golden relacional ambos motores + e2e por sockets: el traceparent VIAJA) |
| **RPC raylang↔raylang** `packages/rpc` — framing con prefijo de longitud sobre TCP (payload protobuf o json), una fibra por conexión, request/response con id + deadline; servidor `rpc.serve(puerto, handler)` y cliente `rpc.call`. La "comunicación nativa" entre servicios sin el peso de h2-servidor (interop externo entrante: HTTP/1.1+JSON del webserver, que ya está) | ✅ **88.4** (`packages/rpc/rpc.ray`, raylang puro sobre std/net+std/json. Wire: frame 4B BE + sobre JSON `{"id","method","params"[,"deadline_ms","traceparent"]}` → `{"id","ok"\|"err"}`; payload protobuf DIFERIDO (el framing no cambia). Servidor: fibra por conexión, peticiones secuenciales por conexión, handler en su tarea (try_join → panic responde err), apagado ordenado DE SERIE (un solo bucle drenable: `serve` = `serve_shutdown` con canal que nunca llega; `serve_graceful` sobre signals()), `Limits.max_frame_bytes` 10 MiB. Cliente: `connect`/`call`/`call_deadline` (read-timeout; tras timeout la conexión queda desincronizada → reconectar)/`call_full` (traceparent W3C como string OPACO — rpc no depende del paquete net)/`disconnect`; id correlado y validado. Gotcha del checker: el esperado no cruza al interior de una TUPLA ni de un match sin anotar → `let x: Result<Json,string> = …`. Tests `rpc_cli`: DOS procesos `ray run` (valida el wire), batería golden + dos clientes concurrentes). **ARCO M88 COMPLETO** (88.1 señales + 88.1b apagado ordenado + 88.2 resiliencia + 88.3 tracing + 88.4 RPC) |

---

## 46. Operador ternario — considerado y DESCARTADO (jul 2026, decisión con el usuario)

Propuesta: `cond ? a : b` como azúcar del if. **Descartado** por:

1. **El if YA es expresión** (§0, orientación a expresiones): `let x = if (c) { a } else { b };`
   cubre el 100% de los casos — el ternario sería un segundo dialecto para lo mismo (misma
   razón por la que Rust/Kotlin lo rechazaron).
2. **Conflicto duro con `?`**: el token está ocupado por la propagación postfija (Try, M6.3).
   La desambiguación "si tras `?` arranca expresión, es ternario" NO funciona: `x? - 1` hoy es
   `(x?) - 1` y `-` también arranca expresión → misparse; resolverlo exige backtracking o
   lookahead no acotado ("¿hay un `:` más adelante?") en DOS parsers de descenso recursivo
   (Rust + selfhost) con errores byte-idénticos.
3. **Costo doble del self-hosting**: cada producción se paga en ambos front-ends + SPEC + fmt +
   LSP + resaltados, a perpetuidad — no se justifica para un alias.
4. **Dos formas canónicas** de lo mismo (contra la línea del proyecto: sin sobrecarga, una forma).

**Alternativa clasificada (💤 a demanda): if-expresión sin llaves para ramas simples** —
`if (c) a else b`. Precedente YA en el lenguaje (señalado por el usuario): los brazos de
`match` son expresiones simples sin llaves (`Option.Some(n) => n,`); esto alinearía el `if`
con lo que `match` ya hace, sin token nuevo ni conflicto con `?`. Matices a resolver si se
hace: dangling-else, interacción con el gotcha §55, formateador, y el mismo costo doble de
gramática (por eso queda a demanda, no comprometida).

## 47. Features de compilación — el binario slim (arco M89, jul 2026)

**Contexto (decisión de arquitectura, con el usuario)**: se evaluó separar el proyecto estilo
Rust (VM / gestor de paquetes / LSP como binarios o repos aparte) vs mantener el monolito.
**Decisión: mantener el binario único auto-contenido** — es una feature deliberada (M40.5,
prioridad DX; el modelo Deno/Zig/Go, no el de Rust, cuyo split fue histórico y le costó que
rust-analyzer reimplementara el front-end; aquí el LSP reusa el checker REAL y el version-skew
es irrepresentable). La separación lógica ya existe por disciplina (patrón "cliente externo"
desde M8) → la opción de partir en crates queda abierta y barata. Disparadores para revisitar:
compilación >30 s, consumidor real de embedding, colaborador regular. El paso intermedio con
mejor costo/beneficio: **features de Cargo** (precedentes: `interp`, y el build wasm que ya
excluye ring/rustls/rusqlite por target).

| Feature | Qué excluye | Estado |
|---|---|---|
| **`sqlite`** (default ON) | rusqlite bundled (el C de SQLite entero). Slim: `cargo build --release --no-default-features --features interp`. Un programa con `db/sqlite` contra un binario slim recibe **Err como valor** en `connect` ("este binario se compiló sin soporte de SQLite (recompila con la feature 'sqlite')"); el checker acepta el programa (la tabla BUILTINS se registra siempre; solo los stubs cambian — mismo molde que el playground wasm). Medido: **6,09 → 4,41 MB (−28%)**. El check `--no-default-features` del CI ya ejercita el build sin sqlite (guardia anti-bitrot gratis) | ✅ **89.1** |
| **`net-tls`** (rustls+ring+webpki: TLS + cripto de producción) | Mapeo previo hecho: TODO ring/rustls vive en builtins.rs (vm.rs no lo toca); el hash de integridad del lockfile es Rust puro (src/sha256.rs) → el gestor de paquetes con deps de ruta/git SIGUE funcionando en slim. Diseño por fallibilidad: lo FALIBLE degrada como **Err-valor** (tls_connect/accept/upgrade → el programa puede hacer fallback, como sqlite); lo INFALIBLE aborta con error claro vía guardia en ambos motores (sha/hmac/ed25519/chacha/random_bytes: NUNCA un hash vacío ni una firma que falla en silencio — el gotcha de los stubs wasm que devolvían Vec::new()); el CLI corta explícito (keygen/publish --sign/index-verify) y resolver un paquete FIRMADO da error honesto ("este binario no puede verificar firmas"), fail-closed. Traza M79 reposiciona al sitio del usuario, idéntica en ambos motores. **Slim total (sin sqlite ni net-tls): 6,09 → 2,90 MB (−52%)** | ✅ **89.2** |
| **`ffi`** (libloading + extern) | carga de librerías nativas — la propiedad "este binario NO puede cargar código nativo" es valiosa en contenedores endurecidos | ✅ **89.3** (la más simple: `ffi.rs` ya estaba partido para wasm y los motores ya tratan el Err de `ffi::call` como error de ejecución con posición → solo el cfg compuesto en la maquinaria de carga + stub con mensaje por motivo + libloading opcional. Un `extern fn` CHEQUEA igual en slim (el descriptor es puro); llamarlo → "este binario se compiló sin soporte de FFI (recompila con la feature 'ffi'): 'sqrt'". Slim total final: **2,88 MB**). **ARCO M89 COMPLETO** (89.1 sqlite + 89.2 net-tls + 89.3 ffi; binario default idéntico, slim −53%) |

Fuera del arco: multi-binario `ray-run` (solo si un deploy real lo pide; las features ya dan el
80%), workspace de crates (ver disparadores arriba).

## 48. Desarrollo con hot reload (`ray dev`) — arco M92 (jul 2026, diseño fijado con el usuario)

**Premisa medida que ordena todo el diseño**: el pipeline completo compila en MILISEGUNDOS y el
binario arranca en ~3 ms (M55) → *el hot reload no es un problema de runtime, es un problema de
no perder el listener*. Y los handles de I/O ya viven en el **host** (registro de proceso), no en
el heap del invitado → una instancia nueva del programa en el MISMO proceso puede heredar el
socket de escucha. Tres caminos analizados:

| Camino | Qué | Veredicto |
|---|---|---|
| **A. `ray dev` = watcher + restart** (fase 1) | Supervisor con polling de mtimes (~200 ms; portable, cero deps — mismo mecanismo que la regen de templates) sobre `.ray`/`.ray.html`/`ray.toml`; ante un cambio, SIGTERM al hijo (compone con `serve_graceful`: drena) y re-run. Reload percibido ~20–50 ms. Templates gratis (la regen de `ray run` ya existe) | ✅ **M92.1** |
| **B. Hot swap DENTRO de la VM** (estilo Erlang/Dart: generaciones de código, safepoints, solo-cuerpos) | Única opción que preserva el heap del invitado, pero: marcos vivos con ips viejos, formas de struct cambiadas rompen el heap, VM-only sin oráculo, y el restart ya cuesta ms → máximo coste para mínimo delta | 💤 aparcado (como §5); solo si aparece estado en memoria caro de reconstruir |
| **C. Swap de programa in-process conservando el listener** (era la fase 2) | Modo dev del webserver: el proceso retiene el handle de escucha (host-side), y ante un cambio recompila (ms) y despacha las peticiones siguientes contra el programa NUEVO — cero conexiones caídas, cero re-bind, downtime ≈ una compilación. Estado del invitado se RESETEA por reload (decisión: limpio y documentado). Es un **cliente externo** más (REPL/runner/LSP), cero cambios en la VM. | 💤 **aparcado** (re-análisis 16 jul, abajo): el teardown in-process es el problema irresuelto |

### Re-análisis (16 jul 2026, tras M92.1 en producción) — dos hechos del código que cambian el mapa

1. **A favor de C**: el registro de handles es **global de proceso** (`OnceLock<Mutex<FileRegistry>>`,
   `builtins.rs`) → una segunda instancia corrida en el mismo proceso vería *literalmente el mismo
   handle* del listener. La fontanería de "retener el listener" existe gratis.
2. **En contra de C, y decisivo**: cuando se fijó este diseño la VM era M:1 cooperativa en un hilo.
   Desde **M38 es M:N multicore con hilos de SO reales**, la cancelación es **cooperativa, no
   preemptiva** (diferido de M12.5), y no hay `catch_unwind` alrededor del invitado. Para recargar
   in-process hay que PARAR la instancia vieja: no existe forma fiable de detener N workers con
   fibras en vuelo (una fibra colgada = supervisor colgado = **listener perdido igualmente**), y un
   panic desmonta el proceso supervisor entero. **El teardown por proceso (el kernel como mecanismo
   de cancelación) es hoy la única parada fiable** → C tal como estaba diseñado queda aparcado.

**Opciones adicionales evaluadas** (con su relación coste/beneficio):

| Opción | Qué preserva | Complejidad | Veredicto |
|---|---|---|---|
| **A+. Endurecer el watch+restart** | — | **baja** | ✅ **M92.2** (check-before-restart + debounce): (1) ✅ **chequear ANTES de reiniciar** — `ray build <entry>` primero (ms), solo reiniciar en verde; en rojo imprimir el diagnóstico y dejar el programa viejo corriendo (ya no mata un servidor que funciona por un cambio roto). (2) ✅ **debounce** ~120 ms (coalesce guardado+formateador). (3) 💤 drenado graceful en Windows (hoy kill duro; unix-only por diseño, como todo el manejo de señales del proyecto). (4) 💤 latencia vía `kqueue EVFILT_VNODE` — Linux pediría inotify → el polling de ~200 ms se conserva (portable, cero deps) |
| **D. Herencia de fd** (socket-activation estilo systemd) | el listener | **media** | ✅ **M92.3**: el SUPERVISOR pre-abre y RETIENE el socket (`--port N`/`--listen host:port`/`[dev] listen` en ray.toml) y lo pasa a cada hijo (`pre_exec` dup2 al fd 3 + `RAY_LISTEN_FD`/`RAY_LISTEN_ADDR`); `tcp_listen` (builtins) ADOPTA con `from_raw_fd` si el env matchea (una vez, guardado por `AtomicBool`) → el mismo programa corre idéntico en dev y prod. Durante el reinicio el socket nunca se cierra: el kernel encola en el backlog → **cero conexiones rechazadas, cero re-bind**, conservando el aislamiento por proceso (SIGTERM+drenado como antes). Gotcha cazado: si el fd del listener ya era 3 (primer libre tras stdio), `dup2(3,3)` es no-op que NO limpia CLOEXEC → fix con `fcntl(F_SETFD,0)` explícito. Unix; no-unix cae al re-bind por reinicio. Test rigoroso: si el 2.º hijo no re-adoptara, su bind chocaría (EADDRINUSE) con el socket retenido |
| **E. `SO_REUSEPORT`** (blue/green local) | el puerto (solape de procesos) | media | descartada para dev: `std` no lo expone (setsockopt por FFI, factible), semántica de reparto difiere macOS/Linux, y durante el solape DOS versiones sirven a la vez (confuso en dev). Es la herramienta de *deploy* sin downtime, no de dev; D es más simple y suficiente |
| **F. Live-reload del navegador (SSE)** | — (UX) | baja-media | ✅ **M92.4**: hub SSE en el SUPERVISOR (puerto lateral, solo en sesión web con `--port`), un servidor SSE mínimo en Rust que emite `data: reload` a los navegadores en cada reinicio VÁLIDO (compone con check-before-restart). Vive en el supervisor porque es lo único vivo entre reinicios. El webserver, viendo `RAY_DEV_RELOAD`, inyecta el `<script>EventSource(...).onmessage=location.reload()</script>` antes de `</body>` en las respuestas text/html (Content-Length recalculado; no-op en producción). NO reemplaza el SSE del paquete (ese es de la app; este es solo-dev, lateral). Diferido: inyección para CUALQUIER programa (necesitaría un proxy, en tensión con la herencia de fd de D) — hoy solo el paquete webserver |
| **G. Estado del invitado entre reloads** | estado app | — | ~~NO construir maquinaria: documentar el patrón "estado de dev en SQLite de archivo"~~ **REABIERTA 21 jul 2026** (decisión con el usuario): para sesiones/config —el caso que hace diferenciador el DX del reload— SQLite es mucho; se construye un **KV persistido en raylang puro** (ver G-rev abajo). SQLite queda como el patrón para estado *de verdad* (relacional, consultas) |

**Contexto del ecosistema**: Go (air), Node (nodemon), Rails = watch+restart → M92.1 ya es el
estándar de la industria. Flutter/Vite hacen hot reload real porque su runtime lo soporta (clase B).
El nicho diferencial barato de raylang es D: *restart tan rápido (arranque ~3 ms) que, con el
listener retenido, se percibe como hot reload*.

Fases (revisadas 16 jul): **92.1** ✅ watcher+restart+drenado · **92.2** ✅ A+ (check-before-restart +
debounce; Windows-graceful aparcado) · **92.3** ✅ D (herencia de fd: `--port`/`--listen`/`[dev] listen`)
+ ✅ G (patrón de estado sqlite-de-archivo, documentado en el MANUAL) · **92.4** ✅ F (live-reload SSE
desde el supervisor). **ARCO M92 COMPLETO** (A+ + D + F + G; B y C aparcados con criterio — C revive solo
si la VM gana cancelación preemptiva/teardown fiable).

### G-rev — estado persistido en raylang puro (reabierta 21 jul 2026)

**Motivación**: el diferenciador de DX del reload es *no perder la sesión ni la config* al editar un
handler. La respuesta original (G: "usa sqlite de archivo") funciona pero es pesada para ese caso:
una DB entera para un puñado de pares clave/valor. Todas las piezas para hacerlo en raylang puro ya
existen: `bytes` + `bytes_of`/`sub_bytes`/`to_bytes`/`from_utf8`, `fs.read_file_bytes`/
`write_file_bytes`/`rename` (→ escritura atómica temp+rename), `Map<string, bytes>` con `keys()`
ordenadas (→ serialización determinista gratis).

**Diseño fijado**:
- **G.1 — `std/kv`** ✅ (21 jul, PR #43): `Store` = `Map<string, bytes>` respaldado por archivo.
  Formato binario propio versionado (magic + count + entradas `[u32 klen | clave utf8 | u32 vlen |
  valor]`, LE). API: `kv.open(path) -> Result<Store, string>` (archivo ausente → vacío; corrupto →
  error honesto) + **métodos del trait `StoreOps`** (`s.get(k)`/`set`/`delete`/`keys`/`count` +
  azúcar `get_string`/`set_string`, y `s.save() -> Result<int, string>` **atómico**: escribe
  `path.tmp` + `rename`). Por trait y no funciones libres: una libre `get(Store,…)` ganaría al
  builtin `get` del Map en UFCS (rompía `self.data.get(k)` dentro del módulo, cazado por el LSP);
  el despacho por receptor lo esquiva (precedente: `Matcher` de regex, M59.2) y los métodos SÍ
  cruzan módulos. Cadencia dev: cargar al boot,
  `save()` tras el drenado graceful (un write por reload, cero coste en caliente); quien quiera
  más, save-on-mutation. Módulo puro (ambos motores, sin primitivos nuevos).
- **G.2 — compartición multicore + session store** ✅ (21 jul, mismo arco): (a) **actor CSP** en
  `std/kv` — `SharedStore` (handle = canal de peticiones; patrón del actor de métricas M38),
  `kv.share(store)`/`kv.open_shared(path)`/`kv.stop(sh)`, y — la pieza elegante — **`impl
  StoreOps for SharedStore`**: la MISMA API sirve local y compartido (`sh.get(k)`, `sh.save()`);
  coherencia por FIFO, azúcar de strings del lado del cliente, `kv.empty(path)` para stores que
  arrancan limpios. Solo VM (spawn/canales). (b) **sesiones del framework web** —
  `sessions(path) -> Result<Sessions,string>` (bajo `RAY_DEV_RELOAD` carga del archivo y persiste
  tras cada escritura, best-effort; en producción memoria pura, cero disco), `session_of(ctx,res)`
  (cookie `ray_session` uuid v4, `Path=/; HttpOnly`, cacheada en locals) y `session_get/put/
  delete(sess, ctx, res, key[, val])` — "editas el handler y sigues logueado" verificado
  (tests/session_cli.rs: la sesión sobrevive al restart). Bonus: destapó el bug §52 (UFCS interno
  de módulo vs namespacing) — 3 sitios de framework.ray arreglados a llamadas libres.

**Impacto** (clasificación): 💤 acomodable — aditivo puro (módulo std + paquete); no toca VM ni
checker; el riesgo de drift de esquema entre reloads lo esquiva el KV de bytes por diseño
(deserializar es problema de la app; un valor viejo ilegible se descarta).

## 49. Recuperación de errores fatales (panic → valor, estilo `recover` de Go) — arco M97 (jul 2026)

**La pregunta**: ¿puede un programa raylang sobrevivir a un error fatal (`panic`, división por cero,
índice fuera de rango, overflow) y seguir corriendo, como hace Go con `panic`/`recover`?

**Estado real (auditado 19 jul 2026): el 80% YA EXISTE, en la frontera de tarea.** La infraestructura
se construyó por partes y nunca se nombró como "recover":

- **M12.3**: el error de una fibra hija NO aborta el proceso — se captura en su `Task` como
  `Failed(msg)` y se re-lanza recién en el `join`/`ScopeEnd` que lo observa. `fail_current_fiber`
  (`vm.rs`) captura **cualquier** `RuntimeError` (no solo `panic` explícito), con traza (M79).
- **M56.5**: **`try_join(t: Task<T>) -> Result<T, string>`** — el `join` que observa el fallo **sin
  re-lanzar**: el error fatal como valor. Existe en la VM (primitivo `__task_failed` + envoltorio del
  prelude) **y en el nativo** (H21-N2, sobre la contención de fallos de N1). El webserver lo usa:
  un handler que revienta → 500 + log, el servidor sigue (el caso `net/http` de Go, ya en producción).
- **M38** (heap por fibra/actor): recuperar es *sano* — el heap del actor fallido se descarta entero
  (estilo Erlang), sin estado compartido a medio mutar.

En esto raylang está **por delante de Go**: una goroutine sin recover mata el proceso; una tarea
raylang no. Lo que falta no es la maquinaria, es superficie, doc y el caso misma-fibra.

**Decisión de diseño (norte)**: NO copiar `panic`/`recover`/`defer` (raylang no tiene `defer` y la
magia posicional de `recover` es lo menos elegante de Go). La forma raylang es **el fallo como
`Result` en una frontera explícita** — compone con `?`/`match`/UFCS y respeta "errores como valores".

**Fases**:

- ✅ **M97.1 — nombrar y documentar lo que existe** (COMPLETA, 20 jul 2026): la verificación destapó
  que `try_join` dentro de un `scope` era INÚTIL (observabas el fallo y `ScopeEnd` lo re-lanzaba igual,
  cancelando hermanas). **Semántica fijada con el usuario: un fallo observado es un fallo manejado**
  (estilo errgroup de Go, no el fail-fast incondicional de coroutineScope de Kotlin) — flag `observed`
  en `VmTask` (lo pone el opcode `TaskFailed`), `ScopeEnd` salta los observados; `join` re-lanza
  siempre; el no-observado conserva M12.5 íntegra. Paridad en el nativo (`wait_observed`). Tests VM +
  nativo con salida exacta; espec en DESIGN §21.6 (refinamiento); MANUAL §15 "Recuperación de errores
  fatales" (patrón `spawn`+`try_join`, reglas, batch tolerante, el webserver como caso real).
- ✅ **M97.2 — `try_call(f: fn() -> T) -> Result<T, string>`** (COMPLETA, 27 jul 2026): recuperación
  en la MISMA fibra, sin `spawn`; el recover general, en los TRES motores. Implementado tal como se
  había planeado: primitivo `__try_call(f: fn()) -> [string]` (`[]` bien / `[msg]` falló — el mismo
  contrato que `__task_failed`) + envoltorio `try_call` en el prelude. **El valor NO viaja por el
  primitivo**: el envoltorio le pasa una closure que empuja el resultado a un array capturado, así el
  primitivo se queda con la firma mínima y no hay que construir un enum genérico desde el runtime.
  Por motor: **intérprete** captura el `Flow::Error` de `call_index`; **VM** pila de `TryMarker` por
  fibra (marcos/pila/scopes al entrar) — el `Return` que devuelve los marcos a la altura del marcador
  entrega `[]`, y el manejador de errores del bucle desenrolla hasta él (`unwind_to_try_marker`, con
  la misma cancelación de hijas huérfanas que `fail_current_fiber`); **nativo** `catch_unwind` en el
  mismo hilo. **Dos decisiones que la implementación obligó a tomar**: (1) el nativo captura
  CUALQUIER panic, no solo `__RayErr`, porque un índice fuera de rango allí es el bounds check de
  Rust — capturar solo `__RayErr` habría hecho DIVERGIR el flujo de control entre motores, que es la
  línea que no se cruza; el texto del mensaje sí difiere en esa clase de error (divergencia
  preexistente, documentada en MANUAL §15); (2) el hook de panic calla dentro de un `try_call`
  (contador thread-local) — un fallo que se va a recuperar no debe imprimir "thread panicked at". El
  marcador gana al `return Err` que aborta en `main`: si no, `try_call` no serviría donde más se usa.
  Tests: `tests/recover_cli.rs` (6, casi todos de ORÁCULO intérprete≡VM≡nativo — lo que
  `spawn`+`try_join` nunca pudo tener) + corpus nativo completo verde.
- ✅ **RESULTADO de aplicarlo al webserver (27 jul 2026, validado con generador remoto y 5
  repeticiones)**: `handle_http` pasa de `spawn`+`try_join` a `try_call` → hilos de SO con 100
  conexiones **198→97**; a 120k rps **p50 0.65→0.54 ms, p99 1.86→1.15, p99.9 6.59→1.88 (3.5×)**; techo
  **~129 500→~165 600 rps**. raylang deja de empatar con Go y le gana en las CUATRO métricas, con
  **1.33× su throughput sostenido bajo SLO** (160k contra 120k) y ventanas de p99 disjuntas por mucho.
  La cola profunda pasa de ser 2.5× PEOR que la de Go a ser mejor (1.88 vs 2.64 ms).
- **La justificación de RENDIMIENTO que lo movió de "planificado" a "hecho", medida contra terceros** (27 jul 2026, [docs/investigacion-p999-webserver-nativo.md](docs/investigacion-p999-webserver-nativo.md)):
  el `spawn`+`try_join` por petición de `handle_http` —que está ahí SOLO por el aislamiento de
  panic de M56.5— hace que en el nativo **cada petición cruce dos hilos de SO** (send al canal del
  pool + despertar de semáforo + join). Censo del perfil bajo 120k rps con `-c 100`: **198 hilos de
  SO** (101 sirviendo conexión + 96 workers aparcados + accept) sobre 11 cores. Es el mecanismo de
  la cola profunda: `benchmarks/web/` mide raylang **mejor que Go en p50 (0.65 vs 0.79), p99 (1.86
  vs 2.11) y techo (129.5k vs 120.4k)** pero **2.5× PEOR en p99.9 (6.59 vs 2.63 ms)** — el trabajo
  por petición es eficiente, lo que se paga es meter al scheduler del SO en cada petición. Con
  `try_call` el handoff desaparece y el censo baja a ~101. Validación ya montada: `webbench.py
  --only ray,go --reps 5`. Por motor: **intérprete** trivial (interceptar `Flow::Error`) — y a
  diferencia de `try_join` (solo-VM, porque `spawn` no corre en el intérprete), `try_call` tendría
  **oráculo VM↔intérprete completo**; **VM**: desenrollar los marcos de la fibra hasta el marcador
  (guardar `frames.len()`/altura de pila al entrar, restaurar al fallar — la mecánica que
  `fail_current_fiber` ya hace con la fibra entera, acotada a un tramo); **nativo**: el cuerpo bajo
  `catch_unwind` y `__ray_rt_err` haciendo `panic!` en vez de `exit(70)` dentro de ese dynamic scope
  (paridad de mensaje byte-idéntica, como siempre). ⚠️ Sharp edge a documentar: recupera con el heap
  de la PROPIA fibra posiblemente a medio mutar (mismo trade-off que `catch_unwind` de Rust); para
  aislamiento real, `spawn`+`try_join`.
- **M97.3 — supervisión de actores** (estilo Erlang/OTP, sobre M38): `supervise(f)` / política de
  reinicio (N reintentos, backoff) para workers de larga vida. Probablemente **librería raylang pura**
  (`packages/`): `loop { match (try_join(spawn(f))) { Err → log+reintentar } }` ya casi se escribe
  solo; la fase es el paquete + el patrón documentado, no runtime nuevo.
- **M97.4 (diferido, solo si 97.2 lo destapa como dolor real) — limpieza en unwind**: sin `defer`, un
  `try_call` que recupera deja recursos huérfanos (handles de archivo/socket viven en el registro
  global del host → fugan hasta el fin del proceso). Opciones: `defer`/`ensure` (gramática nueva) o
  recursos con ámbito (`with_file(ruta, fn(h) {...})`, librería pura). No construir hasta tener el
  caso de uso real.

**Impacto en el diseño actual**: aditivo, no bloquea nada. 97.1 es doc; 97.2 es un builtin (fila en
`BUILTINS` + opcode + impl por motor, patrón M11.4); 97.3 librería. **Restricción hoy**: ninguna —
la única decisión que conviene fijar pronto es la semántica de 97.1 (try_join vs cancelación de
hermanas), porque el webserver ya depende de ella en producción.

## 39. Posición-del-llamador — stack trace de runtime (jul 2026) — M79

El diferido de DX más transversal ([[§21]]/[[§25]]): `assert` fallido reporta la posición del
prelude; el trap de `factorial`/`ipow` apunta dentro de `std/math`. **Decisión (con el usuario):
mini stack trace general** en `RuntimeError` (no el intrinsic `@track_caller`, que solo cubre
funciones anotadas y un nivel). Diseño completo en DESIGN §83.

| Pieza | Coste | Sub-fase |
|---|---|---|
| `RuntimeError.trace` + captura en ambos motores (VM: al capturar el Err, frames intactos, coste cero en caliente; intérprete: pila explícita en `call_body`, TCO renombra la cima) + oráculo de trazas | Runtime acotado; `Display` NO cambia (oráculos/runner/selfhost intactos) | ✅ **79a** |
| Presentación en `cli.rs`: `en <fn> (<módulo> L:C)` / `desde …`, localización por bandas, fuera-de-banda = prelude, truncado 6+…+5, solo con ≥2 marcos | Solo cliente | ✅ **79b** |

| Reposición de la cabecera (y el `^`) al **primer marco de usuario** cuando el error cae en prelude/std — la mitad "intrinsic", barata sobre la traza (DESIGN §83.3) | Solo cliente | ✅ **79c** |

**M79 COMPLETO (a+b+c)** (546 lib + `stack_trace_oraculo` + 8 en `errors_cli`). Diferido:
la traza no cruza `Task::Failed`; saltar también paquetes/deps en la reposición; nombres
manglados tal cual.

Diferido: transportar la traza a través de `Task::Failed` (hoy solo cruza el mensaje); nombres
"bonitos" para métodos manglados (`Tipo#metodo` se muestra tal cual, honesto).

## 45. Optimización de la VM ronda 2 — análisis post-M88 (jul 2026)

**Datos** (best-of-15, M3 Pro; perfil `sample` con símbolos sobre release): fib35 1.97 s ·
loop10M 0.96 s · arrays 0.179 s · gcnested 0.282 s. Del CPU real de fib35: `run_loop` (match)
~53%, **preámbulo por instrucción ~20%** (`run_worker`: stop + should_collect + 3 lecturas de
marco + fuel + bounds + fetch + write-back de ip + cierre M12.3), `push` ~9% y `get_local` ~5%
(**no inlineadas** — símbolos propios en el perfil), coste por llamada (`new_locals`/`put_arg`/
`recycle`) ~8%, `const_to_heap` ~3%.

**Trabas estructurales** (acotan el espacio): (1) el safepoint del GC DEBE estar en frontera de
instrucción (temporales de Rust sin rootear a mitad de brazo → no se puede mover el check a
`allocate`); (2) oráculo + goldens = re-validación total ante cualquier backend nuevo; (3) las
trazas M79 exigen `frames`/`ip` coherentes en todo punto de error → registerizar `ip` obliga a
sincronizar en errores/cesiones; (4) `Rc` en `HeapValue` es `!Send` y la fibra migra entre
workers (M38) → strings compartidos = `Arc<str>` o handle del heap, no `Rc`; (5) fuel/
--deterministic/tope de heap viven en el lazo; (6) §27: solo sobrevive lo que supera el ruido
3-5%; (7) el nicho es SERVICIOS (I/O-bound) → el techo de valor de acelerar aritmética es
limitado.

| Opción | Qué | Esperado | Estado |
|---|---|---|---|
| **A2 Opt.14** inlines calientes | `#[inline(always)]` en push/pop/get_local/set_local/put_arg + reserve de la pila | 3-8% | ❌ **DESCARTADA por medición** (A/B espalda-con-espalda: fib +1,4%, resto ruido — presión de registros en el run_loop gigante; el codegen ya elegía bien) |
| **A5 Opt.15** constantes precomputadas | tabla `Vec<HeapValue>` en el Vm; `Constant` = clone directo | 1-3% | ❌ **DESCARTADA por medición** (A/B plano: el 3% del perfil era CONSTRUIR el HeapValue, que el clone paga igual; y añadía estado duplicado por worker) |
| **A6 Opt.16** `s[i]` sin collect | `Index` string hacía `chars().collect::<Vec<_>>()` POR ACCESO (alloc O(n)); `nth(i)` la elimina | grande solo string-heavy | ✅ **HECHA** (micro-bench patrón lexer: 0,22 → 0,04 s, **5,5×**; banco general neutro; ambos motores) |
| **A1 Opt.17** registerizar ip/marco-tope | cachear `(func, ip, chunk)` en locales del lazo; write-back solo en llamadas/saltos-de-fibra/errores | 10-20% | ❌ **DESCARTADA por medición, premisa refutada**. Implementada completa (whitelist rápida + protocolo legado para aparcados/TailCall; TODA la batería + self-hosting + metacircular verdes → la corrección no fue el problema). A/B: loop −3,3% pero fib **+6,5%**; experimento de atribución (reload por instrucción, sin whitelist): **+9-15% en todo el banco** → leer/escribir `frames[fi]` por instrucción es ~GRATIS en el M3 (store-forwarding+L1) y cualquier rama/reload añadido pierde. El ~20% de `run_worker` del perfil son las ramas fijas (stop/gc/fuel/bounds) + fontanería del outcome, NO las lecturas de marco |
| **A3** PGO | profile-generate → banco → profile-use; reordena el match gigante | 5-15% | ✅ **HECHA** (`tools/pgo.sh`: instrumentado → entrenamiento con banco+strings+iter+parse-selfhost+concurrencia → merge → build final en `target/release`). Medido intercalado best-of-15: **fib −5,2% · loop −5,4% · arrays −8,7% · gcnested −6,0%** — consistente, sobre el ruido; encaja con la atribución de Opt.17 (reordena las ramas fijas del lazo). Para cortar releases; el ciclo de dev sigue con cargo a secas (RUSTFLAGS invalida la caché). **Nota (13 jul 2026)**: el delta vs el plano depende del LAYOUT que le tocó al plano ese día — tras el renombrado ES→EN el plano cayó casi óptimo por azar y el delta bajó a ~0-4%, con el tiempo ABSOLUTO del PGO idéntico (fib ~1,59 s); la métrica estable es el absoluto, PGO es la *garantía* del layout bueno. `pgo.sh` acepta `--slim`/`--features` (compone con el arco M89); guía completa de builds en `docs/build.md` |
| **A4** superinstrucciones ronda 2 | elegidas por HISTOGRAMA dinámico de pares (instrumentación temporal, revertida): la guarda de todo if/while era `[GetLocalConst, Cmp, JumpIfFalse, Pop]` (30M en fib) y la asignación-sentencia emitía `[Unit, Pop]` (10M en loop) | 5-15% en bucles | ✅ **HECHA — el win grande de la ronda** (pase `fuse_round2`): `[Unit,Pop]` eliminado · `[Cmp,JumpIfFalse(t),Pop]` con `code[t]==Pop` → `CmpJump(op,t+1)` · `[GetLocalConst,Add\|Sub]` → `Add/SubLocalConst` (posición del error = la del Add/Sub → byte-idéntico). A/B: **fib −18,9% · loop −24,8% · arrays −27,5% · gcnested −26,7%**. Batería + selfhost + metacircular verdes |
| **B1** locales en pila (clox) | híbrido: solo fn sin capturas (`captured` ya existe); marco = base en la pila de la fibra | 10-20% call-heavy | clasificada |
| **B2** structs por índice | `GetField(String)` → `GetFieldIdx` (el checker anota pre-erasure); instancia `(struct_id, Vec<HeapValue>)` sin nombres repetidos | el estructural con mejor ROI para servicios | clasificada |
| **B3** strings compartidos | revivir Opt.3 como `Arc<str>` o string-como-Obj (traba 4); re-medir con strings.ray, no fib | data-dependent | clasificada |
| **B4** throughput de canales | locks por canal / Condvar vs busy-poll 50µs; solo con contención real (send_heavy) | actor-heavy | a demanda |
| **C1** bytecode de registros | elimina el tráfico de pila (~20%); reescritura compiler+peepholes+revalidación total | 20-40% | 💤 solo si compite en CPU |
| **C2** JIT (cranelift; deps permitidas) | method-JIT numérico con deopt | 5-20× aritmética | 💤 NO recomendado: root-maps, fibras en código JIT, 2 backends × trazas/fuel/deterministic; el nicho es I/O-bound |
| **C3** GC generacional | pausas YA resueltas (0.12 ms, heap-por-fibra); sweep O(slots) no asoma en perfil | — | 💤 sin síntoma |

**Veredicto del paquete barato**: 1 de 3 sobrevive (Opt.16); las micro del preámbulo ya están exprimidas — lo que queda ahí es ESTRUCTURAL. **Veredicto de Opt.17**: cierra DEFINITIVAMENTE el tier "cachear estado del marco" — en Apple Silicon esos accesos son gratis; la única palanca que reduce el impuesto por instrucción es ejecutar MENOS instrucciones (A4 superinstrucciones) o reordenar las ramas fijas (A3 PGO).

**CIERRE DE LA RONDA 2 (jul 2026)**: sobreviven **Opt.16** (s[i] 5,5×), **A3 PGO** (−5 a −9%,
`tools/pgo.sh`) y **A4 superinstrucciones** (−19 a −28%). **Acumulado vs la baseline de
arranque: fib −25,5% · loop −28,0% · arrays −32,4% · gcnested −32,9%** (con PGO encima; la
baseline.json guarda el release PLANO post-A4). La moraleja que deja la ronda, confirmada dos
veces (Opt.17 refutada, A4 ganadora): en Apple Silicon los accesos a memoria caliente son
gratis y las ramas fijas del lazo son el impuesto — solo paga ejecutar MENOS instrucciones
(fusiones por histograma) o reordenar ramas por frecuencia real (PGO). Siguientes palancas si
alguna vez hacen falta: más fusiones del histograma (Call/Return-heavy), B1/B2 estructurales. **Secuencia**: → Opt.17
(registerizar ip) → PGO → superinstrucciones con histograma → reevaluar B/C. Branch:
`feature/opt-vm-ronda2`.

### 45.1 Ronda 3 — diagnóstico ante benchmark externo multi-lenguaje (14 jul 2026)

**Detonante**: benchmark del usuario contra 7 lenguajes (node/php/lua/python/ruby/perl) en
`fibrec` (fib recursivo) y `loopsum` (bucle 10M con `*`/`%`). Perfil de ray: **arranque
excelente** (~3 ms, top-3, casi como lua — binario nativo, sin warm-up de VM) pero **cómputo
lento** (fibrec 12,5× tras node; loopsum 8,3× tras php). Importante: esos números son el estado
**post-A4+PGO+Opt.16** (las palancas baratas YA aterrizadas), no código sin optimizar.

**Baseline fresco** (`measure.py` best-of-15, release plano sin PGO, M3): fib35 **1,67 s** ·
loop10M **0,755 s** · arrays2000x **0,134 s** · gcnested **0,211 s**. Consistente con el CIERRE
de la ronda 2 (fib ~1,59 s con PGO encima).

**Lectura estratégica** (honesta):
- El benchmark mide justo lo que raylang hace PEOR (aritmética/llamada puras) y lo que MENOS
  importa a su nicho (traba 7: servicios I/O-bound). Node gana fibrec por **JIT** — inalcanzable
  sin JIT (C2, 💤 no recomendado). El objetivo realista NO es alcanzar a node, sino **cerrar el
  hueco con lua/php/ruby en cómputo** (2-4× → ~1,5-2×).
- Las micro del preámbulo están EXPRIMIDAS y su ataque directo está REFUTADO (Opt.17). No
  reabrir ese tier.

**Palancas restantes, priorizadas** (todas oráculo-safe salvo C1):
1. **A4' — fusiones de histograma Call/Return-heavy** (la indicada por el propio cierre de r2).
   fibrec es call-heavy y las fusiones de r2 apuntaron a guardas if/while + asignación, NO a
   `Call`/`Return`. Instrumentar el histograma de pares sobre fib/gcd/selfhost (call-heavy),
   fusionar los pares calientes de llamada/retorno. **Mismo patrón probado** (−19 a −28% en r2),
   bajo riesgo, revalidación por oráculo+goldens. → **primer candidato**.
2. **B1 — locales en pila estilo clox** (10-20% call-heavy). Ataca el ~8% de `new_locals`/
   `put_arg`/`recycle` + framing de llamada que domina fibrec. Híbrido: solo fn sin capturas
   (`captured` ya existe). Más invasivo que A4' pero el ROI estructural más directo para fib.
3. **B2 — structs por índice** (`GetFieldIdx`). NO ayuda a fib/loop, pero es el estructural con
   mejor ROI para el **nicho real** (servicios que manosean structs). Si el objetivo es el
   producto y no el benchmark, sube de prioridad sobre B1.
4. **C1 — bytecode de registros** (20-40%): la palanca grande, pero reescritura de
   compiler+peepholes + revalidación total. Solo si raylang ha de competir en CPU de verdad.

**Recomendación**: si el objetivo es responder al benchmark → **A4' (Call/Return) primero**,
luego **B1**. Si el objetivo es el producto → **B2** antes que nada (el benchmark es un mal
proxy del nicho). Secuencia r3 sugerida: A4' → medir → B1 → medir → decidir B2/C1. Branch
propuesta: `feature/opt-vm-ronda3`.

## 50. Runtime embebido del transpilador: `include_str!` para los bloques estáticos (jul 2026)

**Qué**: en `src/transpile/runtime.rs`, el preámbulo de runtime que el backend nativo emite en el
`.rs` generado vive como strings (`out.push_str`/`concat!`). Los bloques **100% estáticos y
autocontenidos** (helpers de string/Map, `RayShow`, manejo de errores/panic) podrían migrar a
archivos aparte (p. ej. `src/transpile/runtime/*.rs.txt`) embebidos con `include_str!` — mismo
patrón que la stdlib embebida (`src/stdlib.rs`). Ganancia: edición con highlighting y diffs más
legibles, sin perder el binario único (el texto se incrusta en compile-time).

**Por qué strings hoy (y qué NO puede migrar)**: (1) el resultado debe ser un solo `.rs`
autocontenido (camino `rustc` pelado, docs/transpilador-nativo.md §4.5) — resuelto igual por
`include_str!`; (2) buena parte del texto es **condicional/interpolado** (`fast` vs checked,
bloques por `needs_*`, variantes `Tls`/`Sqlite` del registro de handles vía `format!`) — eso es
template, no archivo, y se queda como código; (3) los fragmentos **no parsean como Rust
independiente** (referencian `__RayErr`/`__RaySend`/nombres generados) — de ahí la extensión
`.txt`, para que rust-analyzer/cargo no los marquen en rojo.

**Impacto**: solo mantenibilidad; cero cambio de comportamiento (el output debe quedar
byte-idéntico — verificable con `tests/native_corpus.rs`). **Prioridad: baja**; hacerlo si se
vuelve a trabajar a fondo en `runtime.rs`. Coste: partir el runtime en dos regímenes
(archivos para lo estático, `format!` para lo condicional).

## 51. raylang para LLMs — contexto destilado + MCP (jul 2026)

**Problema**: un LLM no tiene raylang en su pretraining → *pattern-matchea* desde Rust/Go y
alucina (`match x {` sin paréntesis, `null`, `mut` en vez de `var`, sobrecarga, métodos de
string inexistentes). La SPEC no lo arregla: es **normativa** (446 líneas, EBNF, optimizada
para conformidad humana), cara en contexto, y los modelos aprenden sintaxis nueva de
**ejemplos contrastivos**, no de gramática formal. Además, aunque el contexto sea perfecto, el
modelo se equivocará — y sin bucle de verificación no se entera hasta que el humano ejecuta.

Dos piezas **complementarias** (no alternativas), en orden:

### Pieza A — contexto estático: "raylang para LLMs" destilado (~2-4k tokens, estilo llms.txt)

**✅ HECHA (21 jul 2026): `llms.txt` en la raíz del repo** (~190 líneas, inglés — es de cara al
modelo). Secciones: delta contra Rust · formas canónicas (del catálogo de snippets del LSP) ·
errores-como-valores · semántica (referencia, indeterminados, sin aritmética de char, `&`/`|`
vs `==`) · módulos · concurrencia CSP · mapa de stdlib + nombres congelados (SPEC §10) · tabla
de "error que provocarás → mensaje EXACTO → fix" · el bucle `ray run/build/test/fmt/doc`.
**Método anti-alucinación: el propio doc se VERIFICÓ contra el binario** — todos los ejemplos
compilados y corridos por ambos motores, y cada mensaje de la tabla provocado y cotejado
literal; cazó 5 errores del borrador (`return` no es expresión de brazo de match, `@derive(Ord)`
no existe, 2 mensajes mal citados, módulo duplicado en la lista). Bonus: SPEC §11 estaba
desactualizada (cabeceras en español; el binario emite las inglesas desde la regla del 21 jul)
— corregida. Cuando exista la pieza B (MCP), este archivo es el *resource* que servirá.

- La **delta contra Rust/Go**: "se parece a Rust PERO: `match (x) {` con paréntesis, `var` no
  `mut`, sin sobrecarga, sin `null`, UFCS universal, `let` inmutable/`var` mutable, firmas
  explícitas…".
- Los **errores típicos** con el mensaje EXACTO del checker (el modelo reconoce sus fallos).
- 10-15 **mini-ejemplos idiomáticos** destilados de `examples/` (el oro real ya existe).
- El **catálogo de snippets de construcciones del LSP** (jul 2026, rama
  `feature/lsp-code-snippets`: 26 snippets en `code_block_snippets()`, `src/lsp/features.rs`)
  comparte inventario con este destilado — mismas construcciones, mismos gotchas fijados
  (paréntesis en `if`/`match`, variantes calificadas `Option.Some`, `Channel.new()` anotado,
  `if let` sin paréntesis). Reusarlo como esqueleto de la sección de sintaxis.
- Las **decisiones de nombres congeladas** (SPEC §10: `index_of` vs `position`, `fetch` no
  `get`, `bytes_of` vs `to_bytes`…).
- Usos: CLAUDE.md/skill de Claude Code, `llms.txt` publicado, y el *resource* que serviría el
  MCP de la pieza B. **Coste: una tarde. Hacer PRIMERO.**

### Pieza B — el MCP: el bucle de feedback (el ROI grande)

**✅ HECHA (21 jul 2026): `ray mcp`** — subcomando del propio binario (`src/mcp.rs`, ~300
líneas), cliente 100% externo como LSP/REPL/runner: cero cambios en el core, **cero deps** (MCP
es JSON-RPC 2.0 por stdio delimitado por línea; el JSON reusa `lsp::json`, ahora `pub(crate)`).
Las 5 tools de la tabla; `ray_check/run/test/fmt` van por **subproceso del propio binario**
(`current_exe`): aislamiento por proceso, el stdout del invitado no toca el canal MCP, y el
confinamiento previsto — fuel 100M + heap 1M (M42) + plazo de pared 10 s con kill (lo que no
consume fuel: red/stdin) + salida truncada 64 KiB + `--deterministic`. `ray_doc` en-proceso
(registro de builtins: `signature()` + `doc()`). Resource `raylang://llms.txt` = la pieza A
embebida (`include_str!`). Un diagnóstico del compilador es resultado normal (isError=false);
isError=true solo para fallos del envoltorio. Guía + config de clientes: `docs/mcp.md`. Tests:
3 unitarios en memoria + `tests/mcp_cli.rs` e2e (handshake, 5 tools, stdin pipeado, bomba de
bucle cortada por FUEL, resource). **ARCO §51 COMPLETO (A + B).**

Ventaja inusual: **el tooling ya existe entero** — el servidor MCP es un envoltorio fino
(~200 líneas) sobre el binario `ray`. El bucle escribir→verificar→corregir convierte la
alucinación en iteración; los mensajes de error de raylang son idóneos (posicionados,
multi-error hasta 20 vía M33c, byte-idénticos). Es "el LSP para agentes" — encaja con la
prioridad DX (el tooling es parte del alcance).

| Tool MCP | Sobre qué | Qué le da al LLM |
|---|---|---|
| `ray_check(code)` | lex→parse→check (lo del LSP) | diagnósticos exactos con posición, hasta 20 errores — autocorrección sin ejecutar |
| `ray_run(code, stdin?)` | `ray run` (VM) | stdout + exit code — verificación conductual |
| `ray_test(code)` | runner `@test` | pasa/falla por test |
| `ray_fmt(code)` | `ray fmt` | código canónico |
| `ray_doc(symbol)` | raydoc/builtins | firma y doc — mata la alucinación de API |

**Seguridad**: `ray_run` ejecuta código arbitrario → correrlo con el **fuel** (M42.1) + límite
de heap ya existentes (exactamente el caso "embeber raylang confinado" para el que se
diseñaron).

**Fuera de alcance por ahora**: fine-tuning y eval-set formal (prematuro hasta medir cuánto
rinde A+B; si algún día, el eval reusa el runner `@test`: prompt → `.ray` → ¿compila/corre?).

## 52. UFCS interno de un módulo se rompe con el namespacing (bug latente del loader, jul 2026)

**Descubierto en G.2** (sesiones del framework): dentro de `packages/web/framework.ray`, `cors`
hacía `c.header_of(…)`/`r.status(204).header(…)` — llamadas UFCS a funciones del PROPIO módulo.
El sitio UFCS (`Call(Field)`) se resuelve en el **checker** por el nombre pelado (`header_of`)
contra el programa fusionado; pero el loader ya renombró la función a `web::framework::header_of`
y el **Resolver no reescribe los nombres de campo de un `Call(Field)`** (no puede sin tipos: no
distingue campo-de-struct de UFCS). Resultado: el sitio interno solo compila si la ENTRADA
casualmente trae el nombre pelado al programa (p. ej. `from web/framework import header_of`) —
**acoplamiento accidental a qué importa el consumidor**; el demo del framework importaba todo y
lo tapaba (los tests pasaban). Con un consumidor mínimo, error: `no field or function 'header_of'
applicable to web::framework::Ctx`.

**Mitigación aplicada (G.2)**: dentro de un módulo, las llamadas a funciones propias van
**libres** (`header_of(c, …)`, `text(res, …)`) — al ser `EIdent`, el Resolver las reescribe
siempre. Arreglados los 3 sitios de `framework.ray` (cors ×2, 405/404 ×2) con nota en el código.
Es la misma pauta ya documentada para el caso cross-módulo ("UFCS no cruza a módulos importados").

**Fix real ✅ HECHO (21 jul 2026, mismo día)**: el loader publica `Program.module_bands`
(inicio de banda → prefijo de funciones; las bandas disjuntas ya existían por L3) y el checker
resuelve el paso función-libre de UFCS en orden **builtin/local → `prefijo::nombre` por la banda
del sitio → pelado → alias from-import** (`module_local_fn` en `check_ufcs`; `name_is_callable`
absorbida). Semántica ganada: ámbito léxico real — con homónimas en módulo y entrada, cada sitio
usa la de SU módulo (cubre también privadas del módulo); archivo único sin bandas → idéntico a
antes. DESIGN §16 documenta el orden completo. Tests: 3 nuevos en `tests/modules_cli.rs` (repro
sin imports, prioridad módulo-vs-entrada con directorio, fallback al prelude). Alternativa
descartada: que el Resolver reescriba nombres de `Call(Field)` (rompería campo-de-struct
homónimo, que gana por diseño). La regla de estilo para paquetes ya no es necesaria (las
llamadas libres de framework.ray se conservan — equivalentes y válidas). **Diferido**: el espejo
selfhost no conoce módulos con namespacing (su loader M14.7 es más simple) → fuera del corpus.

---

## 53. Ejecución de comandos del SO — ✅ EJECUTADA como M100 (jul 2026)

Lanzar procesos del sistema (`git`, `ffmpeg`, `rustc`) desde raylang. Se diseñó a fondo en una
sesión de julio de 2026, se difirió, se revisó (§53.7) y finalmente **se construyó**: el contrato
v1 (§53.8) y el streaming v2 (§53.9) están **ejecutados** y son hoy `std/process` (crónica en
DESIGN §89, superficie en REFERENCE.md §10). Lo que sigue es el registro del diseño y de cómo
llegó a fijarse — incluidas las conclusiones que la segunda mirada invirtió.

### 53.1 Por qué se aparca

- **No hay demanda.** Cero entradas previas en este archivo; ningún paquete ni ejemplo bloqueado.
  En un backlog que clasifica hasta PTY, mirrors del registro y namespaces con dueño, la ausencia
  es evidencia, no descuido.
- **Hay válvula de escape.** El FFI con ABI C (M41, `dlopen`/`dlsym`) permite declarar `system()` o
  `posix_spawn` como `extern` hoy mismo. Es incómodo a propósito: si alguien se toma esa molestia,
  aparece la demanda que ahora falta.
- **Calendario.** M44 (distribución) es el único hito que queda para la 1.0, y **M34 ya congeló la
  API con semver**. Meter `exec` ahora congela superficie recién nacida y sin uso. La API de
  procesos es de las que nadie acierta a la primera: Python tardó quince años en llegar a
  `subprocess.run` y arrastra `os.system`/`popen`/`call`/`check_output`; Node lleva
  `spawn`/`exec`/`execFile`/`fork` con sus variantes `Sync` como verruga permanente. Ninguno pudo
  quitarlo después.
- **El self-hosting NO la pide.** `selfhost/` es el **front-end** (lexer/parser/checker/compiler/
  interpreter/loader) y nada de eso lanza procesos. Quien sí lo haría es la *toolchain* (`ray
  publish`→`git`, `ray build --native`→`rustc`/`cargo`, `ray dev`→se relanza a sí mismo), pero
  self-hostear el CLI no es objetivo declarado.

**Qué la reabre** (cualquiera de las tres): un servicio real en raylang que lo necesite · decidir
self-hostear la toolchain, no solo el front-end · alguien que llegue con el FFI puesto y la queja
de que es intragable.

### 53.2 La restricción que manda el diseño: TRES motores

raylang corre en **intérprete** (oráculo secuencial), **VM** (producto) y **nativo transpilado**,
con paridad byte a byte exigida. El propio código ya trazó la línea que decide el reparto:

> `"concurrency (spawn/channel/send/recv/join/scope/select) requires the VM; the interpreter is
> only the sequential oracle"` — `src/interpreter.rs`

| Capa | Motores | Por qué |
|---|---|---|
| `run()` — bloqueante, captura acotada | los **tres** | llamada secuencial; el oráculo la valida |
| streaming por canales | **solo VM** | necesita aparcar fibras; mismo precedente que `spawn` |

**Empezar por lo bloqueante.** Si se empieza por el streaming, el 90% de los casos de uso se queda
sin oráculo de desarrollo.

### 53.3 La API

```raylang
enum Exit { Code(int), Signal(int) }

struct Output { exit: Exit, stdout: bytes, stderr: bytes, truncated: bool }
```

- **`Exit` es un enum, no un int.** El lenguaje tiene `enum`+`match`; aplanar una muerte por señal
  a `128+sig` es la convención del shell, no una verdad, y aquí no cuesta nada evitarla.
- **Salir con código ≠ error.** El `Result` distingue *no se pudo lanzar* (ENOENT/EACCES) de *lanzó
  y terminó*; que `grep` devuelva 1 es un dato. Norte del proyecto (DESIGN §0: errores como
  valores). Encima, un `check()` para el caso ergonómico.
- **`bytes`, no `string`.** El tipo ya existe (`Type::Bytes`, M16.1a). Un nombre de fichero POSIX es
  cualquier secuencia de octetos sin NUL; forzar UTF-8 en el borde es una mentira que explota tarde.
- **Tope de salida por defecto** (~16 MB) con `truncated: bool`. raylang apunta a servidores: ese
  `run()` acabará ejecutándose con input semi-controlado, y un `Vec` sin límite es vía de OOM.
- **Sin shell, ni siquiera opt-in en v1.** Quien quiera una tubería escribe `run("sh", ["-c", …])`
  explícitamente — honesto y visible en el código.
- **Streaming (v2, solo-VM): un `Channel<bytes>` ACOTADO**, no un tipo nuevo de concurrencia. El
  `send` que bloquea con el canal lleno **es** la contrapresión, y se propaga sola al pipe y de ahí
  al hijo. El orden causal ("el estado siempre tras todos los datos") sale **estructural**: el canal
  cierra en EOF → `recv` da `None` → entonces `join`. No hay que prometerlo en la spec.
- Intercalar stdout/stderr en orden real exige `dup2` de ambos al MISMO pipe (modo `Merge`
  explícito). Mezclar dos canales en userspace da orden arbitrario: los buffers del kernel son
  independientes.

### 53.4 Auditoría CLOEXEC — HECHA (27 jul 2026)

Se auditó **todo sitio del proyecto que crea un fd fuera de `std`** (que sí pone CLOEXEC por
defecto). Resultado: **dos fds sin CLOEXEC, ambos nuestros, ambos corregidos**; el resto limpio.

| Sitio | Veredicto |
|---|---|
| **Self-pipe de señales** (`builtins.rs`, M88.1) | ❌ **FUGA** → corregida. `pipe(2)` crea los fds sin `FD_CLOEXEC` y el código solo llamaba a `F_SETFL` (flags de ESTADO: `O_NONBLOCK`), nunca a `F_SETFD` (flags del DESCRIPTOR). Fix: `fcntl(fd, F_SETFD, FD_CLOEXEC)` en ambos extremos (`pipe2` sería atómico pero no existe en macOS) |
| **`epoll_create1(0)`** (`poll.rs`, M17) | ❌ **FUGA** (Linux) → corregida con `EPOLL_CLOEXEC`. Ventana estrecha (el epoll nace y muere dentro de `wait`), pero el flag es gratis |
| **`kqueue()`** (`poll.rs`) | ✅ limpio **por construcción**: kqueue(2) garantiza que el descriptor NO se hereda por `fork` |
| **SQLite** (`rusqlite`/`libsqlite3-sys` bundled) | ✅ limpio, verificado en la fuente vendorizada: `robust_open` usa `osOpen(z, f\|O_CLOEXEC, m2)` con fallback a `FD_CLOEXEC` por `fcntl` |
| **`ray dev`: `pre_exec`+`dup2` al fd 3** (`cli.rs`, M92.3) | ✅ **correcto**. El `fcntl(F_SETFD, 0)` que limpia CLOEXEC corre DENTRO de `pre_exec` — o sea en el hijo, tras el `fork` y antes del `exec` — así que el fd del supervisor conserva su CLOEXEC. La herencia es deliberada y acotada al hijo |
| **Sockets / ficheros / handles** (`std::net`, `std::fs`) | ✅ limpio: Rust pone CLOEXEC al crear |
| **FFI (`dlopen`/`dlsym`)** | ✅ sin fds propios. Lo que abra una función foránea queda fuera de nuestro control — documentado, no auditable |

**Exposición real HOY: ninguna.** Las dos fugas eran **latentes**, no vivas: los únicos `exec` del
proyecto son de nivel CLI (`git` desde `deps.rs`, `rustc`/`cargo` desde `build --native`, el
re-lanzamiento de `ray` en `mcp.rs` y `dev`), y ninguno de esos procesos sostiene el self-pipe (lo
crea la VM, que no lanza procesos) ni un epoll. El supervisor de `ray dev` tiene su propio handler
de señales **sin** self-pipe (reenvía SIGTERM al hijo directamente). Se corrigieron igual: el día
que exista `exec` habrían sido fugas silenciosas, y un hijo con el extremo de ESCRITURA del
self-pipe abierto impide para siempre el EOF de ese pipe.

**Pendiente para cuando llegue `exec`**: el hijo de `ray dev` adopta el listener en el fd 3 con
`from_raw_fd`, y ese fd tiene `FD_CLOEXEC` a 0 **dentro del hijo** (necesario para que sobreviva al
exec). Si ese programa llegara a lanzar procesos, heredarían el socket de escucha → hay que
re-poner CLOEXEC tras la adopción en `tcp_listen`.

**No verificado**: el camino de `epoll` no se compiló (no hay target de Linux instalado en la
máquina de desarrollo); el cambio es un `const` + su paso como argumento, y lo cubre el CI.

### 53.5 Implementación, cuando toque

- **El bloqueo del pool es el problema propio de raylang.** Desde M38 el scheduler es multicore por
  defecto con `available_parallelism()` hilos: un `run()` bloqueante dentro de una fibra secuestra
  un hilo del pool, y ocho concurrentes lo dejan seco. Ningún runtime single-threaded tiene este
  problema. Solución sin maquinaria nueva: **aparcar la fibra en los fds del hijo** vía `src/poll.rs`
  (M17) — donde `IoParked` **ya lleva `deadline`**, así que los timeouts no son código nuevo. En
  intérprete y nativo, bloquear está bien (el intérprete no tiene fibras; el nativo tiene hilos).
- **SIGCHLD por self-pipe, reusando M88.1**, no `pidfd`/`EVFILT_PROC`. El patrón ya está en el árbol
  y probado (handler async-signal-safe → pipe → fd registrado en el mismo `poll::wait` que los
  sockets). Objeción previsible ("secuestra la señal globalmente"): el proyecto **ya** secuestra
  SIGTERM y SIGINT. Además `poll::wait()` toma y devuelve **fds** con `EVFILT_READ`/`WRITE` fijos, y
  `EVFILT_PROC` identifica por *pid* → no cabe en esa firma sin ensancharla.
- **Feature de Cargo, no sistema de permisos.** raylang no tiene capabilities y no conviene
  inventarlas aquí: tiene el precedente de `sqlite`/`net-tls`/`ffi` (arco M89), donde el binario slim
  da un `Err` claro. "Este binario no puede lanzar procesos" verificable por ausencia de código es
  más fuerte que un flag de runtime.
- **Reparto por tiers** (política de DESIGN §53): el primitivo es **builtin** (syscalls del host + impl en el
  transpilador nativo), el builder y la ergonomía van en **`std/process` escrito en raylang**.
- **Grupos de proceso desde el día uno**, aunque parezcan prematuros: en cuanto exista `timeout`,
  matar solo al PID directo deja vivos a los nietos de un `sh -c "a | b"`, y añadirlo después
  **cambia la semántica** de programas ya escritos. Escalera de apagado: cerrar stdin (deja hacer
  flush) → SIGTERM al grupo → SIGKILL al grupo.
- **Fuera de v1**: PTY (subsistema propio: cambia el buffering de los hijos), Windows (el CI compila
  `ray` para Windows → ahí `run` da un `Err` honesto, precedente de `packages/tz`), y cualquier
  abstracción de supervisión de procesos (`Task<T>` + `Channel` ya cubren lo que haría falta).

### 53.6 Orden de ataque, si se reabre

**(0)** auditoría CLOEXEC (independiente, hacer ya) · **(1)** SIGCHLD por self-pipe sobre M88.1 ·
**(2)** `run()` + `std/process` en los tres motores con golden VM≡nativo · **(3)** streaming sobre
canal acotado, solo-VM.

### 53.7 SEGUNDA MIRADA (28 jul 2026, post-arco de fibras) — dos conclusiones se INVIERTEN

Releído con ojos frescos tras cerrar el arco de concurrencia nativa (fibras M:N por default en el
nativo, #71-#79). Los pilares de API del §53.3 SOBREVIVEN enteros (Exit como enum, bytes, tope de
salida, sin shell, salir-con-código ≠ error, grupos de proceso desde el día uno). Pero la
restricción que ORGANIZABA el diseño — la asimetría VM-fibras / nativo-hilos — ya no existe, y
con ella caen dos conclusiones y aparecen cuatro piezas nuevas.

**Lo que se invierte:**

1. **"En el nativo, bloquear está bien (tiene hilos)" — FALSO hoy, y al revés.** El nativo tiene
   14 workers FIJOS: un `run()` bloqueante dentro de una fibra secuestra un worker, y 14
   shell-outs concurrentes congelan el binario entero (la lección del livelock de connects, ya
   pagada). `run()` DEBE aparcar la fibra también en el nativo. La buena noticia: la maquinaria
   ya existe y está en producción — pipes del hijo son fds → `wait_readable` del reactor;
   timeout → `wait_readable_timeout` (deadline en el park); espera de salida → pulso del
   self-pipe de SIGCHLD como fd + `waitpid(pid, WNOHANG)` en bucle (el despertar-de-todos del
   reactor es spurious-safe por diseño). Coste de implementación: una fracción del que el §53
   presupuestaba, porque el arco ya construyó el 80%.
2. **"Streaming solo-VM" — OBSOLETO.** La razón era la asimetría de motores. Hoy ambos motores de
   producto tienen fibras: el streaming por `Channel<bytes>` acotado va a **VM + nativo con
   paridad byte a byte** (el intérprete lo rechaza con su mensaje propio, precedente exacto de
   `spawn`). El 100% de la feature queda bajo oráculo o bajo paridad-entre-productos.

**Lo nuevo que la mirada fresca añade (no estaba en el §53):**

3. **`Command::process_group(0)` de Rust std (estable), no `pre_exec`.** Verificado en esta
   máquina: crea el grupo del hijo SIN closure `pre_exec`. Importa el doble ahora: el binario
   nativo es un proceso de 14+ hilos con mimalloc — un `fork` con `pre_exec` solo puede ejecutar
   código async-signal-safe entre fork y exec (un lock del asignador tomado por otro hilo =
   deadlock del hijo), y evitar `pre_exec` deja a std usar su camino `posix_spawn` (más rápido y
   sin esa clase de bug entera). La escalera de apagado del §53.5 (stdin → TERM al grupo → KILL
   al grupo) queda igual, pero su mecanismo de creación de grupo es este.
4. **stdin = /dev/null por defecto.** El §53 no lo decía. Un hijo que hereda el stdin del
   servidor puede colgarse leyéndolo (o leer lo que no debe). `.stdin(bytes)` escribe y cierra;
   heredar jamás. Es la opción moderna-y-segura para un lenguaje de servidores.
5. **Cosecha de zombis sin destructores.** raylang no tiene drop: en v1, `run()` SIEMPRE cosecha
   (incluso tras timeout: escalera + `waitpid` final). En v2 (streaming), el `Proc` es un hijo de
   scope como las `Task` (M97.1: consumido-o-cosechado-al-cerrar-el-scope) — la cancelación
   estructural del scope dispara la escalera de apagado. Eso es concurrencia estructurada DE
   PROCESOS, que ni Go ni Python ofrecen de serie: la ventaja de diseñar esto DESPUÉS del arco.
6. **El pulso de 10 ms ya existe** (F3, WaitList): la espera de salida del hijo hereda gratis la
   cadencia de cancelación cooperativa (H21-N3) — una fibra esperando un hijo y cancelada por su
   scope nota la cancelación en ≤10 ms y ejecuta la escalera. En el diseño del §53 esto habría
   sido código nuevo.

**Lo que NO cambia:** la cautela de API del §53.1 (nadie acierta una API de procesos a la
primera; M34 congeló con semver) sigue siendo el mejor argumento para una v1 MÍNIMA: `run()` +
builder acotado, streaming detrás, PTY jamás en v1. Y el reparto por tiers (primitivo builtin,
ergonomía en `std/process` raylang) sigue intacto. La feature de Cargo como gating también —
con un matiz nuevo: exec no trae NINGÚN crate (std::process basta), así que la exclusión es
puramente de política ("este binario no puede lanzar procesos"), igual de verificable.

**Demanda:** el disparador del §53.1 era "un servicio real que lo necesite / que el dueño
reabra el tema". Este apartado existe porque se reabrió. El hueco además cambió de naturaleza:
con el arco cerrado, raylang se posiciona como lenguaje de SERVICIOS de producción (92-93% de
hyper/axum), y los servicios reales lanzan procesos (git, ffmpeg, migraciones, backups).

### 53.8 CONTRATO de la v1 (fijado 28 jul 2026, aprobado por el dueño) — **v1 EJECUTADA**

> **Estado (28 jul 2026)**: el punto (1) del orden de ataque está COMPLETO (fases 1a–1d, rama
> `feat/process-exec-v1`; crónica en DESIGN.md §89): primitivo en `ray_runtime::process`, builtin
> `__run` en los tres motores, `std/process` con la superficie exacta de abajo, golden triple
> automatizado. Interinato consciente: VM y nativo bloquean el hilo del worker durante `run()`
> (como SQLite) — el aparcado de la fibra necesita un park multi-fd que aún no existe; es la
> siguiente fase de M100. El punto (2) (streaming v2) sigue sin ejecutar.

Superficie EXACTA. Dos entradas y nada más; `stream()` llega en v2. Cada línea esquiva un error
documentado de otro lenguaje (tabla al final).

```raylang
enum Exit { Code(int), Signal(int) }

struct Output {
    exit: Exit,
    stdout: bytes,
    stderr: bytes,
    timed_out: bool,   // la escalera actuó; exit será Signal(15|9)
    truncated: bool,   // se alcanzó max_output; lo capturado es el PREFIJO
}

// (1) El caso del 90 %:
process.run(program: string, args: [string]) -> Result<Output, string>

// (2) El builder, para todo lo demás:
process.cmd(program: string, args: [string]) -> Cmd
Cmd.dir(path: string) -> Cmd
Cmd.env(key: string, value: string) -> Cmd
Cmd.env_clear() -> Cmd
Cmd.stdin(data: bytes) -> Cmd          // se escribe y SE CIERRA
Cmd.timeout_ms(ms: int) -> Cmd
Cmd.max_output(bytes: int) -> Cmd      // default ~16 MB
Cmd.merge_output() -> Cmd              // dup2 al MISMO pipe → orden real del kernel
Cmd.run() -> Result<Output, string>
```

**Invariantes del contrato** (lo que el `Result`/los campos PROMETEN):

- `Err` significa **no se pudo lanzar** (ENOENT/EACCES/dir inválido). Un hijo que corrió y salió
  siempre es `Ok`, aunque su código sea ≠ 0 o muriera por señal.
- **Sin shell**, ni opt-in: `argv` tipado. Una tubería se escribe `run("sh", ["-c", …])`, visible.
- **stdin = `/dev/null`** si no se llama a `.stdin(…)`. Heredar el stdin del proceso: NUNCA.
- **Ambos pipes se drenan concurrentemente** aparcando fibras (VM y nativo) o con `poll(2)` en el
  intérprete → el deadlock clásico "wait antes de leer" es imposible por construcción; el usuario
  no ordena nada.
- **Timeout ⇒ NO es `Err`**: `timed_out: true` con el `Output` PARCIAL (diagnóstico con los datos).
  La escalera es: cerrar stdin → `SIGTERM` al **grupo** → margen → `SIGKILL` al **grupo**.
- **El grupo se crea con `Command::process_group(0)` de std** (sin `pre_exec`: en un proceso de
  14 hilos con mimalloc, el código entre `fork` y `exec` debe ser async-signal-safe).
- **`run()` SIEMPRE cosecha** (raylang no tiene destructores): también tras timeout.
- Salida capturada con **tope** y `truncated` explícito; nunca un `Vec` sin límite.
- `bytes` en todo el borde; decodificar es decisión del llamador (`from_utf8`).
- **Reparto por tiers**: primitivo `__run` builtin (host + transpilador), ergonomía en
  `std/process` escrito en raylang. Gating por `--without process` (política, no crate).

**v2 (fuera de este contrato, diseñado para no romperlo)**: `Cmd.stream() -> Proc` con
`Proc { out: Channel<bytes>, err: Channel<bytes> }` y `Proc.wait() -> Exit`; el canal ACOTADO es la
contrapresión; `Proc` es **hijo de scope** (cosechado o cancelado estructuralmente, como `Task`) →
concurrencia estructurada de procesos. VM **y nativo** (ambos tienen fibras), no solo-VM.

**Errores ajenos que cada decisión esquiva** (la razón de ser del contrato):

| Error histórico | Quién lo pagó | Decisión |
|---|---|---|
| Shell por defecto → inyección | `os.system`, `child_process.exec` | sin shell, ni opt-in |
| API multiplicada | Python `call/check_call/check_output/run/Popen`, Node `exec/execFile/spawn/fork`×`Sync` | dos entradas (+`stream` en v2) |
| `128+señal` aplana la muerte | POSIX shell y sus imitadores | `Exit` enum |
| Deadlock del pipe lleno | Go `cmd.Wait`, Python `Popen.wait` | drenaje concurrente; el usuario no ordena |
| Timeout ausente o añadido después | Python (13 años), Go (mata solo al hijo directo) | `timeout_ms` + escalera al GRUPO desde el día uno |
| Timeout = excepción que esconde el parcial | `subprocess.TimeoutExpired` | `timed_out` EN el Output |
| Captura sin límite → OOM | Rust `output()`, Python | tope + `truncated` |
| `maxBuffer` mata en silencio | Node | trunca y lo DICE |
| Texto forzado en el borde | Python 2/3, encodings de Node | `bytes` siempre |
| Exit ≠ 0 como excepción | `check_output`, `execSync` | salir con código no es error |
| stdin heredado → hijo colgado | casi todos | `/dev/null` por defecto |
| fds del padre filtrados | clásico universal | CLOEXEC auditado (§53.4) |
| `fork` en multihilo → deadlock del asignador | cualquiera con `pre_exec` | `process_group(0)`, sin `pre_exec` |
| Zombis sin `wait` | C, Node `detached` | `run()` cosecha; v2: hijo de scope |
| stdout/stderr con orden inventado | quien fusiona en userspace | `merge_output()` = un pipe (`dup2`) |

**Orden de ataque revisado** (sustituye al del §53.6):
**(1)** `run()` en los tres motores — interp bloquea (oráculo), VM y nativo APARCAN la fibra;
`Exit`/`Output`, tope, timeout con escalera, `process_group(0)`, stdin=/dev/null; golden
intérprete≡VM≡nativo (comandos deterministas: `sh -c 'echo hi; exit 3'`, muerte por señal) ·
**(2)** streaming VM+NATIVO por `Channel<bytes>` acotado (modo `Merge` incluido) con paridad
entre productos; `Proc` como hijo de scope · **(3)** menores: `env_clear`, Windows honesto,
rlimits si alguien los pide.

### 53.9 DISEÑO de la v2 (streaming), fijado 29 jul 2026 — **EJECUTADA COMPLETA** (fases 2a–2e,
29 jul; crónica en DESIGN §89 — la 2e resultó NO invasiva: kill-list en ScopeFrame en la VM y el
trait __RayScopeChild reutilizado en el nativo)

La superficie ya estaba en el contrato (§53.8): `Cmd.stream() -> Proc` con
`Proc { out: Channel<bytes>, err: Channel<bytes> }` y `Proc.wait() -> Exit`; canal ACOTADO =
contrapresión; `Proc` hijo de scope. Lo que se fija aquí es el CÓMO, tras auditar la maquinaria:

**La decisión central: las BOMBAS se escriben EN raylang.** `stream()` (en `std/process`) spawnea
una fibra por flujo que hace `loop { read(handle) → send(canal) }` y cierra el canal en EOF. Todo
lo difícil ya existe y se reusa tal cual:

- **Contrapresión**: `send` sobre `Channel.bounded(n)` lleno APARCA la fibra de la bomba
  (`vm/mod.rs` ChanSend / `__RayChan` nativo con cap) → la bomba deja de leer → el pipe del SO se
  llena → el hijo se bloquea en su write. La cadena entera es el diseño; no hay código nuevo.
- **Aparcado por fd**: la lectura no-bloqueante de un pipe aparca la fibra igual que un socket
  (VM: `IoParked` es fd+deadline, `poll::wait` acepta cualquier fd; nativo: `wait_readable`).
  **El interinato del "park multi-fd" de la v1 se DISUELVE**: cada bomba espera UN fd.
- **Estructura**: las bombas son `spawn` ordinarios → hijas del scope del llamador (join implícito
  en `ScopeEnd`, canceladas si una hermana falla). El precedente de `spawn` dentro de la std es
  `std/kv.share` (actor) y el webserver.

**Host nuevo, mínimo** (por motor: registro de la VM en `builtins.rs` + su gemelo emitido del
nativo, como se hizo con Udp/Sqlite):

1. Variante **`OpenHandle::Pipe(File)`** + brazo en `raw_fd` + brazo en la lectura no-bloqueante
   (reusa el camino de `__socket_read_bytes`; `close(h)` genérico ya funciona).
2. **`__proc_spawn(program, args, dir, env, env_clear, stdin, has_stdin, merge) -> [bytes]`**:
   lanza (mismo `Command` de la v1: process_group, /dev/null, merge por dup) y devuelve
   `[b"ok", pid, h_out, h_err]` con los pipes YA no-bloqueantes y registrados (con merge, h_err
   es -1 y el canal `err` nace cerrado). El stdin sigue siendo escribir-y-cerrar en el spawn
   (misma limitación documentada que la v1; un stdin por canal sería v3).
3. **`__proc_try_wait(pid) -> [bytes]`**: `waitpid(WNOHANG)` → `[b"running"]` o
   `[b"code"|b"signal", valor]`. `Proc.wait()` en raylang: bucle try_wait + `time.sleep(2)`
   (cooperativo en ambos motores). En el camino feliz no itera: se llama tras drenar los canales,
   y EOF de ambos pipes ≈ el hijo ya salió.
4. **`__proc_kill(pid) -> unit`**: SIGTERM/SIGKILL al GRUPO (para timeout/cancelación).

**Reparto de opciones del builder**: `stream()` honra `dir/env/env_clear/stdin/merge_output`.
`max_output` NO aplica (la contrapresión del canal ES el tope) y `timeout_ms` NO aplica en v2
(el llamador compone: `deadline` de std/resilience + `kill()`); documentado en el módulo.

**Cosecha y cancelación estructural, por fases**: la fase base entrega `Proc.wait()` explícito +
las bombas como hijas de scope (el scope no termina con bombas vivas). La cancelación que MATA al
grupo del hijo (hermana falla → `__proc_kill` + cosecha) necesita un gancho de host en
`cancel_task` — va en fase propia y si resulta invasiva se documenta como pendiente (el zombi es
finito: se cosecha al morir el programa).

**Intérprete**: fuera, como todo spawn/canal (mensaje limpio ya existente); el oráculo de la v2 es
VM≡nativo (golden con los mismos comandos deterministas de la v1).

**Orden de ataque v2**: **(2a)** host: `OpenHandle::Pipe` + `__proc_spawn`/`__proc_try_wait`/
`__proc_kill` en VM (+ tests unitarios) · **(2b)** `std/process.stream()` + `Proc` + bombas en
raylang; ejemplo determinista · **(2c)** gemelo nativo (registro emitido + helpers `__ray_proc_*`)
· **(2d)** golden VM≡nativo + docs (MANUAL/REFERENCIA/llms.txt/DESIGN) · **(2e)** cancelación
estructural con kill al grupo, si el gancho no es invasivo.

### 53.10 DISEÑO de la v3 (stdin escribible sobre un hijo VIVO), fijado 21 ago 2026 — impacto: MEDIO

**El caso real que lo desbloquea** (el criterio de promoción de este archivo): un **cliente MCP**
escrito en raylang. Un servidor MCP por stdio es un hijo JSON-RPC **vivo**: se le escriben
peticiones y se leen respuestas indefinidamente. Hoy es **imposible** — `spawn_streamed` crea el
pipe de stdin solo con `.stdin(bytes)`, lo escribe ENTERO y lo cierra antes de devolver (el hijo
ve EOF), `Proc` expone solo `out`/`err`, y `write_handle` rechaza los handles de proceso
(`src/builtins.rs`: "un stdin por canal sería v3"). No es un caso periférico: cliente MCP,
cliente LSP, drivers de REPL y toda herramienta interactiva caen en el mismo hueco.

**Superficie** (métodos, NO un canal — corrección sobre la nota original de v2): un canal es
simétrico y bonito (`send(p.stdin, b)`), pero **se traga los errores**: si el hijo muere a mitad
de sesión, un `send` a un canal no tiene dónde decir EPIPE, y para un cliente MCP ese es EL error
que importa. El norte del lenguaje es errores como valores →

- `Cmd.stdin_pipe()` — el stdin del hijo será un pipe ABIERTO (excluyente con `.stdin(data)`, que
  sigue siendo escribir-y-cerrar). Sin ninguno de los dos, `/dev/null` (invariante intacto: jamás
  se hereda el stdin del padre).
- `Proc.write(data: bytes) -> Result<int, string>` — escribe TODO el dato (EPIPE = el hijo cerró
  o murió → `Err`, visible).
- `Proc.close_stdin()` — el EOF explícito es parte del protocolo de muchos hijos (`sort`, `wc`),
  así que es superficie, no un efecto colateral del `Drop`.

**Implementación** (la fontanería está al 90 %; el cambio es *no cerrar* el pipe):
`RunOpts.stdin_open` + `SpawnedChild.stdin` (extremo de escritura, NO-bloqueante) ·
`OpenHandle::PipeW` en el registro (`close(h)` ya lo cierra → EOF gratis) · `__proc_write`
**reusa el opcode `SocketWriteBytes`** (como `__proc_read` reusa `SocketReadBytes`: cero opcodes
nuevos) → en la VM la contrapresión de un pipe lleno **reusa `park_write`** (aparcado por interés
de ESCRITURA, ya existente); en el nativo con fibras, `wait_writable` del reactor; el intérprete,
bloqueante.

**Invariantes que NO cambian**: sin shell, argv tipado, stdin nunca heredado, `Proc` sigue siendo
hijo de scope (un hijo con stdin abierto que nadie cierra muere con su scope, como hoy).

## 54. Carrera de builds nativos concurrentes del MISMO fuente (jul 2026, impacto: BAJO)

Dos `ray build --native` simultáneos del mismo archivo comparten el pkg de la caché Cargo (H14: el
nombre sale de la ruta canónica) → compiten por `~/.ray/native-cache/<profile>/<pkg>` y uno puede
copiar el binario mientras el otro lo reemplaza ("could not copy the binary"). Cazada al correr en
paralelo los dos corpus nativos (plano y `--fibers`), que compilan los mismos ejemplos; los tests
se serializaron con un mutex (`tests/native_corpus.rs`). El caso real (dos terminales/CI jobs
compilando el mismo fuente a la vez en una máquina) es raro y el fallo es ruidoso, no silencioso:
clasificación BAJO. Arreglo natural si se reabre: copiar con nombre temporal + rename atómico, o
un flock por pkg alrededor del `cargo build` + copia.

---

## 55. Constructores de tamaño — `kb`/`mb`/`gb` vía UFCS — ✅ EJECUTADA (jul 2026)

El hermano de los constructores de duración de `std/time` (DESIGN §91): `64.kb()` → bytes, como
funciones ordinarias vía UFCS, sin sintaxis nueva. Las dos decisiones que arrastraba, tomadas el
30 jul 2026: (1) **convención binaria `kb/mb/gb = 1024ⁿ`** documentada en grande (la lectura real
en código de sistemas: buffers, límites de memoria; se descartaron los nombres pedantes
`kib/mib/gib`); (2) **el hogar: `std/units`, PLANO** — sin submódulo `units/size` de arranque
(YAGNI: las duraciones ya viven en `std/time` y el catálogo realista de unidades restantes es
corto; un directorio con un único submódulo sería ceremonia). Si algún día llega otra familia
grande, entra como `std/units/<familia>` hermana o se reexporta al estilo `net/time` → `std/time`.
Vetos sintácticos anotados: ni el módulo ni un submódulo pueden llamarse `bytes` (palabra reservada
de tipo — `import ...bytes;` muere en el parser). Entregada junto a las duraciones (PR #98):
`std/units.ray` + `tests/units_cli.rs` (UFCS y forma calificada, ambos motores).

---

## 56. LSP: completion en posición de from-import — ✅ EJECUTADA (jul 2026)

Detectado al verificar `std/units`: tras `from std/M import ` el LSP devolvía `[]` — para TODOS
los módulos de la stdlib. El diagnóstico cambió al abrir el código: la feature YA existía (M45c,
`ImportCtx::Symbols`), pero `module_pub_symbols` resolvía el módulo **solo por disco**
(`resolve_module_path` + `read_to_string`) → los módulos **embebidos** (`std/…`, sin archivo fuera
del repo) devolvían `None` y la lista salía vacía. Era un BUG de resolución, no un hueco de
feature. Arreglo: helper `module_source` (stdlib embebida primero, disco después — el mismo orden
que el loader) usado en `module_pub_symbols`, en la clasificación de re-exports y en la de
from-imports del documento; además `loader::available_modules` ofrece también las rutas embebidas
(→ `from std/` completa `std/units` incluso en un buffer sin proyecto). El hueco picaba justo
donde más se escribe: la forma UFCS de los constructores de unidades exige el import sin
calificar. Tests: unitarios (`completion_of_from_import_symbols_of_embedded_std`,
`completion_of_module_path_includes_embedded_std`) + integración por protocolo real (`lsp_cli`).

**2ª tanda (mismo día): el 4º sitio.** Al probar el **signature help** con los constructores
apareció el mismo bug en `SigCtx::new` — el BFS del cierre de imports resolvía solo por disco →
las funciones de la stdlib embebida no tenían firma que mostrar (`units.kb(` y `64.kb(` daban
null). Mismo arreglo (`module_source` en el BFS); con él quedan alineados los CUATRO resolutores
de fuente de módulo del LSP. Test: `signature_help_of_embedded_std_functions` (calificada + UFCS
con receptor recortado).

---

## 57. Playground web: autocompletado e imports de la stdlib (jul 2026, impacto: BAJO — DX)

Detectado al "probar el autocompletado en el playground": no existe — el editor es un `<textarea>`
plano (sin Monaco/CodeMirror ni puente con el LSP). Dos features separables, por orden de valor:
(1) **imports de la stdlib embebida**: el pipeline wasm es sin loader a propósito (un solo archivo,
núcleo), pero `stdlib::embedded` es puro (sin fs) — un merge mínimo de los `std/…` importados
haría funcionar `from std/time import seconds` en el navegador, donde hoy el from-import se ignora
en silencio y el error aparece en el USO (confuso); (2) **autocompletado**: exigiría un editor de
verdad (CodeMirror ~vendorizable) + exponer una función de completion del wasm (el LSP entero por
stdio no aplica en el navegador; la lógica de `lsp/features.rs` sí es reusable). Contexto: la
CADENA DE BUILD del playground se reparó en esta fecha (aHash/getrandom, handles de 32 bits, stub
de procesos — CHANGELOG, PR #102) y la guarda CI del target wasm quedó AÑADIDA (paso "Build wasm32
(playground)" en ci.yml + fila en PRODUCTION §4): lo pendiente de esta idea son solo las dos
features (imports de la stdlib embebida; autocompletado con editor de verdad).

---

## 58. Tests de producción: runner `@test` sobre el loader (jul 2026, impacto: ALTO — DX/producción) — arco M101

Auditoría (30 jul 2026): `@test` era un runner de **un archivo** heredado del self-hosting (M13.2) —
insuficiente para un proyecto real. Hallazgos, por severidad:

1. **El runner no pasaba por el loader** (`test_runner::run` lexeaba el fuente crudo): un archivo de
   tests con `import` fallaba con `name 'math' not declared` mientras `ray run` lo resolvía. Solo se
   podía testear código del mismo archivo → invalida el caso central (testear tus módulos, o usar
   `import std/...` en un test). **La brecha nº 1.**
2. **Sin descubrimiento a nivel proyecto**: un archivo por invocación; nada de barrer los módulos del
   proyecto ni una convención `tests/`.
3. **Exit code = `failures & 0xFF`**: 256 fallos → exit 0 → CI en verde (y >125 colisiona con los
   códigos de señal del shell).
4. **Fallos sin ubicación** (`assert_eq failed: 4 != 5`, sin archivo:línea) — contradice el principio
   "todo error reporta ubicación".
5. Coste O(n²) al escalar (re-check del programa completo por prueba) y faltan comodidades
   (`--list`, skip, should-panic, salida JUnit/JSON, paralelismo).

**Arco M101 (1–4; ejecutado):** el runner carga vía `loader::load_with_deps` (imports resuelven,
tests recolectados de TODOS los módulos fusionados con nombre calificado `math.t`, el main sintético
se construye como AST — un nombre global `math::t` no lexea — y esquiva la visibilidad `pub`
llamando por nombre global post-resolver); descubrimiento `tests/*.ray` como suites de integración
(cada una una entrada, con la raíz de `src/` como raíz extra de módulos); exit **0/1** (65/66 se
conservan); fallos con `at módulo:línea:col` (reposicionado al primer marco de usuario, estilo M79c)
y duración por prueba. **Diferido** (5): re-check incremental por prueba, `--list`/skip/should-panic,
salida machine-readable, ejecución paralela de la batería — a demanda cuando haya baterías grandes.

---

## 59. Diagnósticos de templates traducidos al `.ray.html` — ✅ EJECUTADA (jul 2026) — fase A2 de M102

Con M102 (templates compilados **en memoria** por el loader; DESIGN §93) el `.ray` generado ya no
existía en disco, pero un error de **tipos**/runtime dentro del módulo generado reportaba la línea
del generado, que el usuario no puede abrir. **Ejecutada el mismo día (30 jul)**: el loader guarda
por módulo-template su `TemplateOrigin` (fuente del `.ray.html` + line map de
`templ::generate_with_map`, el mismo que ya usaba el LSP), y `locate` — la vía única de
presentación de diagnósticos (CLI y test runner) — traduce línea, fuente y cursor: todo error de
un módulo-template (checker, runtime con traza, lex/parse del generado, imports de un include
roto) apunta a la línea real del template, a nivel de línea (col 1, subrayado completo — la
columna del generado no existe en el template). Detalle en DESIGN §93.

---

## 60. std/term + std/io — terminal real (ago 2026, impacto: MEDIO — runtime en ambos motores) — arco M107, PLAN

**El caso.** Construir una TUI real en raylang (raycode) hoy exige FFI a `getchar`, abrir
`/dev/stdout` en append para escribir sin salto, y `tput cols < /dev/tty` para el tamaño. Todo eso
son huecos de la stdlib, no del lenguaje. El arco los cierra con dos módulos (`std/io`, `std/term`)
sobre builtins, **sin FFI de usuario y sin dependencias nuevas**.

**Lo que el código ya tiene (y el plan aprovecha).** (a) La VM aparca fibras por fd en kqueue/epoll
(`IoParked` + patrón `SocketRead`: rebobinar ip, aparcar, re-ejecutar) — una lectura de stdin que
aparca NO pide `extern blocking` ni bloquea la VM. (b) El nativo expone `wait_readable(fd)` en
`ray_runtime::fibers`; sin fibras (`--without fibers`), un read bloqueante en su hilo es correcto.
(c) `signals()` ya existe TAMBIÉN en el nativo (`__ray_signals`, self-pipe) — la doc "VM only" está
rancia; añadir SIGWINCH (28 en macOS y Linux) es pequeño. (d) `IoParked` ya tiene `deadline`
(M56.4) → lectura con timeout gratis.

**Fases (un PR cada una, en este orden):**

- **M107.0 — bug: `unit` escrito en posición de tipo.** No hay token de tipo `unit`
  (`parse_type_inner`): `-> unit` llega como `Struct("unit")` y `resolve_type`
  (`checker/core.rs:1164`) no lo mapea → `extern fn free(p: ptr) -> unit` se rechaza con un mensaje
  que lo da por válido, y `fn f() -> unit` tampoco compila. SPEC lista `unit` entre los tipos → bug
  de implementación. Fix: `Struct("unit", [])` → `Type::Unit` en `resolve_type`, **en tándem con el
  espejo `selfhost/checker.ray`** y tests en ambos.
- **M107.1 — `std/io`: escribir sin salto + flush.** Builtins `__stdout_write(s)`,
  `__stderr_write(s)`, `__stdout_write_bytes(b)`, `__stdout_flush()` (convención `[string]`
  ok/err, como `__write_handle`); módulo `std/io` con `write/ewrite/write_bytes/flush ->
  Result`. Los cuatro consumidores del registro (checker/VM/interp/nativo) + entrada en
  `NATIVE_TRACKED_BUILTINS` con test que los EJECUTA (lección de los seis huecos).
- **M107.2 — lectura de stdin por bytes, que aparca.** `__stdin_read(max) -> bytes` (`b""` = EOF) y
  `__stdin_read_timeout(max, ms) -> [bytes]`. VM: patrón `SocketRead` sobre fd 0 — aparcar hasta
  readiness y LEER DESPUÉS (sin `O_NONBLOCK`: stdin comparte la open file description con el shell
  padre; volteársela sería grosero). Un solo lector a la vez (documentado). Interp: read bloqueante
  (oráculo M:1, correcto). Nativo: `wait_readable(0)` + read; sin fibras, read directo. wasm: error
  limpio. **El test que importa**: hijo con stdin=pipe alimentado con retardo; una fibra ticker
  imprime mientras main espera el byte → los ticks salen ANTES del byte (prueba el aparcamiento
  real), y paridad nativa del mismo programa.
- **M107.3 — `std/term`.** Builtins `__term_is_tty(fd)`, `__term_size() -> [int]` ([] si no tty;
  `ioctl(TIOCGWINSZ)` sobre `/dev/tty` con fallback a fd 1) y `__term_raw_on()/__term_raw_off()`
  (termios guardado en static; equivalente de `cfmakeraw` a mano). **Cero deps**: `extern "C"`
  declarados a mano (patrón `src/poll.rs`), structs termios `repr(C)` por SO (macOS ≠ Linux) →
  inventario de `unsafe` en SECURITY.md. Módulo: `is_tty()`, `size() -> Option<(int, int)>`,
  `raw(f: fn() -> T) -> Result<T, string>` (enciende, corre `f` bajo `try_call`, restaura SIEMPRE;
  red de seguridad extra: restaurar al salir la VM/binario y en el panic hook; `kill -9` queda
  documentado → `reset`), y `read_key() -> Option<Key>` con
  `enum Key { Char(char), Enter, Tab, Backspace, Esc, Up, Down, Left, Right, Home, End, PageUp,
  PageDown, Delete, Ctrl(char), F(int) }` — decodificador de secuencias ESC + UTF-8 multibyte en
  **raylang puro** → testeable por `@test` alimentando bytes, sin tty. Ejemplo `examples/term/`
  (visor con repintado). CI sin tty: paths no-tty + decoder puro; termios = smoke manual.
- **M107.4 — SIGWINCH en `signals()`.** Añadir 28 al self-pipe de la VM y del nativo; corregir la
  doc rancia ("VM only"). Con esto: `select` sobre `signals()` + `term.size()` = re-maquetado al
  redimensionar.
- **M107.5 — Buffer mutable en la frontera FFI (CLASIFICAR, no ejecutar en este arco).** El
  síntoma (un `read(0, buf, 8)` C que "no escribe") es el modelo, no un bug: `bytes` es inmutable y
  compartido (`Rc<[u8]>`); que C escribiera ahí sería corrupción. El arreglo real es un tipo
  `Buffer` (objeto de heap mutable con storage estable, pasable como `void*`): SPEC + checker +
  ambos motores + nativo + interacción con heap-por-fibra (cruza `spawn` por copia, coherente).
  Impacto: MEDIO-ALTO. **No lo necesita std/term** (que va por builtins); lo necesitan APIs C con
  out-params. Milestone propio si el caso real aparece.

**Fuera del arco** (backlog aparte): streaming/chunked del cliente `http` — ✅ **EJECUTADO como
M108** (ago 2026): `net/http.stream*` + cliente `net/sse` (crónica en DESIGN §99).

**Docs por fase**: SPEC (superficie estable: módulos y builtins nuevos, SIGWINCH), REFERENCE +
MANUAL, CHANGELOG "Sin publicar", SECURITY.md (unsafe de termios/ioctl/isatty), DESIGN (crónica al
cerrar el arco).

## 61. Valores-función thread-safe en el nativo — fn dentro de un dato cruzando fibras (ago 2026, impacto: MEDIO-ALTO — solo backend nativo)

**El caso** (destapado por un programa real, ago 2026): un valor **con funciones dentro** (campo
`fn` de un struct, payload de enum, elemento de array/Map/tupla) que cruza una fibra —canal,
captura de `spawn`, `Task`—. **No es un hueco del lenguaje**: el checker no lo restringe (no hay
noción de "enviable") y la **VM lo soporta** — su modelo de closure es *índice de función +
upvalues* y `vm/transfer.rs` copia `Obj::Closure` en profundidad entre heaps (M38), verificado en
vivo. Es una **divergencia del backend nativo**, hoy degradada a fallo limpio y guiado:

- fn **suelta** capturada por `spawn` → error en transpilación ("cannot cross threads… yet"),
  salvo por la vía N5c (abajo).
- fn **dentro de un compuesto** que cruza → el conversor `__to_send_N` del tipo entero se emite
  como stub que **panica en runtime** ("value of a type holding functions cannot cross a thread
  boundary… rebuild it inside the fiber").

**Lo que SÍ está resuelto** (H21-N5c): un **parámetro** de tipo fn que atraviesa un `spawn`
(directa o transitivamente, punto fijo sobre el grafo de llamadas) se emite como genérico Rust
(`__F: Fn + Send + Sync + Clone`) — así corre el patrón webserver completo en nativo. Lo que
falta es la fn **almacenada en un dato**.

**Por qué es un proyecto propio y no un fleco** (`docs/transpilador-nativo.md`, "modelo de
concurrencia thread-safe"): en el nativo una closure es una closure de Rust tras `Rc<dyn Fn>`
(no-`Send`), y en el punto de cruce el tipo `fn(int)->int` ya está **borrado** — no dice qué
closure es ni qué capturó, así que no hay reconstrucción genérica posible al otro lado. Dos vías:

1. **Repr thread-safe universal**: toda fn-valor pasa a `Arc<dyn Fn + Send + Sync>` con capturas
   copiables. Coste: cambio de repr pervasivo + posible impacto en los caminos calientes de
   closures (medir; el contrato de PERFORMANCE manda).
2. **Defuncionalización en el cruce**: replicar el modelo índice+upvalues de la VM — un registro
   de constructores de closures (id de forma + capturas convertidas a `__RaySend`) y un caso
   `Type::Fn` en el árbol de conversión. Más quirúrgico, pero exige rastrear qué closures pueden
   llegar a cada sitio de cruce (análisis estático o tabla global de formas).

**Semántica ya fijada** (no re-decidir): lo que cruza se **copia** (heap aislado M38); las
capturas mutables se re-crean como celdas locales → mutación aislada, como la VM y como ya hace
N5c con los params genéricos.

**Interacciones**: el contrato byte-idéntico VM≡nativo de PRODUCTION.md (esta es hoy su excepción
documentada); `web/framework` (`listen_app` existe precisamente como workaround: reconstruir el
valor dentro de la fibra); si algún día hay serialización de closures (no planeada), compartiría
la defuncionalización. **Workaround vigente**: reconstruir el valor con funciones dentro de la
fibra receptora (pasar los DATOS y rehacer las fns allí).

## 62. Acuerdo de claves — X25519/ECDH + HKDF en std/crypto — ✅ EJECUTADA como M114 (ago 2026, impacto: ALTO — superficie estable de cripto, dependencia nueva)

**El caso** (reporte de un usuario construyendo un p2p tipo IRC, ago 2026): `std/crypto` tiene
**identidad** (Ed25519) y **cifrado autenticado** (ChaCha20-Poly1305), pero **no tiene con qué
acordar la clave**. Sin X25519 no hay secreto compartido, así que las dos piezas que sí están no
se pueden unir: el usuario solo puede cifrar con claves precompartidas fuera de banda.

Un canal cifrado entre pares necesita **cuatro** piezas; hoy hay dos:

| pieza | primitiva | estado |
|---|---|---|
| identidad / autenticación | Ed25519 | ✅ M43.3 |
| **acuerdo de claves** | **X25519 (ECDH)** | ❌ |
| **derivación** (secreto DH → claves) | **HKDF-SHA256** | ❌ |
| cifrado autenticado | ChaCha20-Poly1305 | ✅ M43.4 |

HKDF **no es opcional**: la salida cruda de un DH no es una clave uniforme y jamás debe usarse
directa como clave AEAD. Si se entrega X25519 sin HKDF, el usuario hará exactamente eso.

**El bloqueo técnico — `ring` NO puede darlo con la forma que el proyecto necesita.** Verificado
sobre `ring` 0.17.14: `agreement::EphemeralPrivateKey` solo se construye con `generate(alg, rng)`;
no hay constructor desde octetos, `bytes()` es `#[cfg(test)]` + `#[deprecated]`, `agree_ephemeral`
**consume** la clave, y el truco de pasar un RNG determinista es imposible porque el trait
`rand::SecureRandom` está **sellado** (`sealed::SecureRandom`). Consecuencias, ambas fatales:

1. **No se puede persistir** la clave privada larga de un nodo — justo lo que un p2p necesita
   (la identidad sobrevive al reinicio).
2. **No es determinista** → rompe el oráculo byte-idéntico VM≡nativo, que es invariante del
   proyecto (PRODUCTION.md): no habría forma de probarlo en el corpus dorado.

Ojo con la formulación: `ring` **sí** trae X25519 como algoritmo (`agreement::X25519`) — lo que no
trae es esta FORMA de clave. Una API **"solo efímera"** (generar, acordar, tirar) evitaría la
dependencia y es **defendible**: la identidad la llevaría Ed25519 firmando el transcript, que es la
forma de Noise y de Signal. Se descartó por la API, no por la criptografía — obliga a una superficie
asimétrica (handle de sesión opaco donde Ed25519 tiene semilla→clave) y a un test relacional en vez
de vectores oficiales.

**Propuesta: `x25519-dalek` 3.0** bajo la feature `crypto` ya existente. Cae de lleno en la
cláusula de `SECURITY.md` ("una dependencia entra solo cuando hacerlo a mano sería peor
ingeniería": criptografía de curva elíptica en tiempo constante lo es). Árbol medido con
`--no-default-features --features static_secrets`: `curve25519-dalek` 5.0 + `subtle` + `cfg-if` +
`rand_core` — cuatro crates pequeños, sin `std` obligatorio, la base de todo Noise/WireGuard en
Rust. `StaticSecret::from([u8;32])` da la forma **semilla → determinista** que ya usa Ed25519.
Verificado contra los vectores de RFC 7748 §6.1 antes de proponerlo.

HKDF **no** trae dependencia: `ring::hkdf` ya está disponible.

**Superficie propuesta** (misma forma que Ed25519: semilla de 32 octetos, `Option` ante tamaño malo):

```ray
pub fn x25519_public_key(secret: bytes) -> Option<bytes>                       // 32 -> 32
pub fn x25519_shared_secret(secret: bytes, peer_public: bytes) -> Option<bytes> // 32,32 -> 32
pub fn hkdf_sha256(salt: bytes, ikm: bytes, info: bytes, len: int) -> Option<bytes>
pub fn constant_time_eq(a: bytes, b: bytes) -> bool
```

- **`x25519_shared_secret` devuelve `None` si el resultado no es contributorio** (clave pública de
  orden pequeño → salida toda-ceros). Es la comprobación que impide seguir con una clave nula, y
  `was_contributory()` la da hecha. Decisión: `None`, no ceros — el tipo obliga a mirarlo.
- **`constant_time_eq`**: hoy comparar dos `bytes` en raylang (`==`) NO es de tiempo constante;
  cualquier comparación de etiquetas/MAC escrita por el usuario filtra por temporización.
- HKDF con `salt` vacío = el `HashLen` ceros del RFC 5869, como manda el estándar.

**Lo que hay que documentar junto con la primitiva** (si no, el usuario se dispara en el pie —
esta es la mitad del valor del arco): la receta de canal seguro entre pares — X25519 efímero por
sesión, HKDF sobre el secreto con el **transcript** como `info`, **dos claves direccionales**
(`info` distinto por sentido) para que ningún nonce se reutilice entre los dos extremos, nonce =
contador por dirección, y una firma Ed25519 sobre el transcript para **ligar la identidad** al
intercambio (si no, hay man-in-the-middle: X25519 solo no autentica). Va a `MANUAL.md`.

**Alcance del arco**: `crates/ray-runtime/src/crypto.rs` (+ stubs sin la feature), registro de
builtins (4 opcodes), intérprete + VM + `transpile/`, `std/crypto.ray`, `SECURITY.md` (tabla de
dependencias + inventario), REFERENCE/MANUAL/SPEC, vectores de RFC 7748 §6.1 y RFC 5869 en el
corpus de 3 motores. Sin la feature `crypto` (build slim/wasm), stubs inofensivos como el resto.

**Interacciones**: es la pieza que faltaba para un Noise/handshake escrito **en raylang**; si
algún día hay un `std/noise`, se apoya exactamente en estas cuatro funciones.

**EJECUTADA como M114** (ago 2026, DESIGN §107): las cuatro funciones entregadas con la superficie de
arriba, en los tres motores byte-idénticos, con los vectores de RFC 7748 §6.1 y RFC 5869 A.1/A.3 en
`tests/key_agreement_cli.rs` y la receta de canal seguro en `MANUAL.md` §13 +
`examples/stdlib/key_agreement.ray`. Entró `subtle` además de `x25519-dalek` (para
`constant_time_eq`; `ring::constant_time` está deprecado para uso externo) — ya venía en el árbol
como transitiva de `curve25519-dalek`.


## 63. Hallazgos de raylogs — sort de floats roto en nativo, tail -f, regex con nombres, CSV streaming (ago 2026)

Dogfood de `ray-apps/raylogs` (analizador de logs en streaming: stdin/archivo → parse
JSON/CSV/regex → filtros → count-by/percentiles). Cuatro necesidades confirmadas con caso
concreto, en orden de impacto:

1. **[EJECUTADA — PR #140: `__ray_sort_float` (merge sort con `<`, paridad NaN)] BUG — `sort([float])` compila en la VM pero rompe el build nativo** (impacto: ALTO —
   corrección entre motores). El transpilador emite `__ray_sort<T: Ord + Clone>` y `f64` no es
   `Ord` en Rust → `error[E0277]` en el `cargo build` del usuario, con el error de Rust crudo como
   diagnóstico. Es una violación del contrato "los tres motores byte-idénticos": el mismo programa
   corre en `ray run` y no compila en `--native`. Repro: `let ys = sort([3.5, 1.2]);` +
   `ray build --native`. Arreglo natural: `total_cmp` para el caso float (o un bound propio
   `RayOrd` que f64 implemente). raylogs lo esquiva con un mergesort de floats propio
   (`src/agg.ray`), que además costó cero en el benchmark (200k floats dentro de 0.62 s totales).

2. **`tail -f` / watch de fs** (impacto: MEDIO — ya anotado en §2 transversal de IDEAS-APPS):
   `--follow` se compone con `fs.read_bytes` a EOF + `time.sleep(200)` y funciona, pero es la
   cuarta app que sondea (ray dev, raycode-dev, y lo que raysync/raysite necesitarán). La pieza
   mínima útil no es inotify completo: un `fs.poll_append(h)` o watch de mtime bastaría.

3. **`std/regex` sin grupos con nombre** `(?P<name>...)` (impacto: MEDIO — ergonomía): para
   extraer campos de logs los índices g1..gN obligan a un side-channel (raylogs: flag
   `--fields ip,method,status`). `captures_str` ya devuelve las ranuras; falta solo el parseo del
   nombre y un mapa nombre→índice en `Regex`.

4. **`std/csv` no es incremental** (impacto: BAJO): `parse_csv` traga el documento entero; un
   lector por líneas pierde los campos entrecomillados con `\n` dentro (raylogs lo documenta como
   fuera de v1). La forma streaming sería un parser push (chunk de bytes → filas completas).

**Señal de perf** (para PERFORMANCE.md): 200k líneas JSON parse+count-by — VM 16.5 s vs nativo
0.62 s (27×); regex 5 grupos — VM 8.75 s vs nativo 0.21 s (42×). El nativo queda al nivel de awk
(0.48 s con extracción cruda por split, sin parse JSON real). El hotspot de la VM en este workload
es el parser de `std/json` interpretado (mismo patrón que el 55× de `std/deflate` en store, §7
transversal de IDEAS-APPS: los parsers de la stdlib en el intérprete son candidatos a builtin).


## 64. Hallazgos de rayrelay — close cross-fibra roto en nativo, RefCell en constructores de variante, half-close, select (ago 2026)

Dogfood de `ray-apps/rayrelay` (rendezvous + relay ciego TCP para takeit/msg, con actor CSP,
traspaso de sockets entre fibras por canal, STUN-lite UDP y métricas). Es la primera app que opera
sockets de larga vida con fibras en el backend NATIVO, y destapó dos bugs de corrección entre
motores más varios huecos de expresividad. Repros mínimos en `ray-apps/rayrelay/docs/repros/`.

1. **[EJECUTADA — PR #140: `__ray_close` hace `shutdown(Both)` y el bucle de lectura re-verifica el handle al despertar/EOF] BUG — nativo: `close(h)` de un socket con un lector aparcado es un no-op silencioso**
   (impacto: ALTO — corrección entre motores, servicios de larga vida). Matriz medida:
   - misma fibra, sin lector: ✅ ambos motores (FIN llega, handle inválido después).
   - cross-fibra, socket OCIOSO: ✅ ambos motores.
   - cross-fibra con un lector APARCADO en `socket_read_bytes(h)`: **VM ✅** (el lector despierta
     con `Err(invalid handle)` y el peer recibe FIN) / **nativo ❌**: el close no surte efecto —
     ni FIN al peer, ni invalidación del handle; el lector sigue recibiendo `Err("read timeout")`
     para siempre (`closed_read.ray`), y sin timeout configurado queda colgado eternamente
     (`close_wake.ray`). El idiom estándar de proxies/relays "cierro el socket para despertar al
     pump del otro sentido" no es portable. Workaround de rayrelay: cada pump cierra su PROPIO src
     al salir (same-fiber) + suelo de read timeout para que el segundo pump se autolimpie.
2. **[EJECUTADA — PR #140: los args de literales compuestos con 2+ exprs se izan a temporales `let __rt_a{i}`] BUG — nativo: `Variant(b.campo, f(b), …)` con `f` mutando `b` panica con "RefCell already
   borrowed"** (impacto: ALTO — crash de runtime en código válido). El transpilador mantiene vivo
   el borrow del acceso a campo mientras evalúa el siguiente argumento del constructor de variante;
   si ese argumento llama a una función que muta el struct (borrow_mut), panic. En la VM funciona.
   Repro: `borrow_repro2.ray` (send(ch, Msg.Claim(b.n, steal_tag(b), reply))); la variante con
   llamada a función libre y struct-literal NO reproduce — es específico del constructor de
   variante. Workaround: izar los argumentos a locales. Emparenta con §61 (thread-safety del
   nativo), pero este es determinista, no una carrera.
3. **Half-close: no hay `shutdown(SHUT_WR)`** (impacto: MEDIO). Sin él no se puede expresar el
   idiom netcat "aviso EOF de escritura y dreno hasta el FIN del peer"; el cliente de rayrelay usa
   un periodo de gracia de 2 s tras EOF de stdin como aproximación. Cualquier protocolo que
   termina por half-close (HTTP/1.0, pipes estilo nc) lo necesita.
4. **Composición canal↔socket: falta `try_recv` (o `recv_timeout`) y `select` heterogéneo**
   (impacto: MEDIO). `select` bloquea, solo acepta `[Channel<T>]` del MISMO T, y no hay recv no
   bloqueante → "espera datos del socket O una orden de control" en una fibra no se puede escribir;
   rayrelay lo rodea con fibras lectoras que vierten a canales del mismo tipo y un timer que envía
   `bytes` vacíos para poder entrar al select. Un `select` con timeout o sobre tipos mixtos
   (enum-ificado) cubriría el 90%.
   - **[EJECUTADA — M116 (DESIGN §112) `try_recv` + M116.1 (DESIGN §113) `select_timeout`]** recv no
     bloqueante (`Got`/`Empty`/`Closed`) y select con plazo (`Some(i)`/`None`; `ms=0` = poll),
     event-driven en los dos motores. **PENDIENTE menor**: `select` sobre tipos MIXTOS (hoy se
     enum-ifica en userland: un `Channel<MiEnum>` y se despacha por variante).
5. **[EJECUTADA — M121, DESIGN §118: `net.set_read_timeout` aplica a UDP; la nota "bloquea todas
   las fibras" resultó RANCIA (VM cede desde M20.11, nativo-fibras desde F4)] UDP**: confirmada la
   restricción documentada (recv bloquea TODAS las fibras en ambos motores
   → el respondedor STUN debe ser proceso aparte), y además `recv_from` no tiene timeout (un
   datagrama perdido cuelga al cliente `probe` sin remedio; `timeout_err.ray` muestra que TCP sí
   lo tiene y su error es el string estable `"read timeout"` — un enum de error tipado sería
   mejor contrato).
6. Menores: no hay `exit(code)` (terminar el proceso desde una fibra auxiliar obliga a
   reestructurar para que main decida); el error de read-timeout solo se distingue por string.

**Lo que funcionó bien de verdad** (vale la pena decirlo): el patrón actor + canales-en-mensajes
— incluidos canales DENTRO de variantes de enum y handles de socket cruzando fibras — es
byte-idéntico en VM y nativo bajo carga concurrente (`refcell_repro.ray`/`refcell_repro2.ray`
pasan en ambos); `signals()` compone limpio para apagado; y el par `net/metrics` + `net/log` deja
un servicio operable (scrape Prometheus + JSON lines) sin fricción.

## 65. Hallazgos de raygate — remote addr del Request, listeners zombis en ray test, [[toml]], forma de guard (ago 2026)

Dogfood de `ray-apps/raygate` (API gateway: rutas TOML, rate limit, breaker, JWT, retry+deadline,
proxy streaming, métricas, trace propagado). Es la app que ejercita webserver + cliente http A LA
VEZ y `std/resilience` completo. Cinco necesidades y una corrección de documentación:

1. **[EJECUTADA — M123, DESIGN §120: `Request.remote` + `remote_ip(req)` + `net.peer_addr(h)`] `webserver.Request` no expone la dirección remota del cliente** (impacto: MEDIO-ALTO —
   cualquier servidor real la necesita). Sin ella no hay rate limit por IP, ni `X-Forwarded-For`,
   ni logs de acceso con origen. Superficie natural: un campo `remote: string` (o `peer_addr(req)`)
   rellenado por el bucle de servicio.
2. **`ray test` deja listeners zombis entre tests** (impacto: MEDIO — DX de tests con servidores).
   El runner descarta las fibras del `@test` anterior pero los sockets de ESCUCHA del SO
   sobreviven: el siguiente test los ve aceptar conexiones que nadie atiende (read timeout en vez
   de connection refused). Un boot de servidores compartido entre tests se envenena; tampoco hay
   `var` top-level para un "boot once". Workaround: todo el E2E en UN `@test`. Arreglo natural:
   cerrar los handles vivos del test anterior al aislarlo (los listeners incluidos), o un hook de
   setup/teardown por archivo.
3. **`std/toml` sin arrays de tablas `[[route]]`** (ya documentado como diferido; aquí confirmado
   con el caso concreto: la config de un gateway/proxy es EL uso canónico de `[[...]]`). raygate
   usa tablas con nombre `[route.api]` como rodeo aceptable.
4. **La forma de `resilience.guard` no compone con el patrón actor** (impacto: BAJO-MEDIO —
   API-shape). `guard(b, err, f)` exige que `f` corra en la fibra dueña del breaker; en cuanto el
   estado vive en un actor (lo obligado con fibras de heap aislado) hay que reimplementar las
   transiciones a mano sobre los campos del struct. Un par explícito `admit(b) -> bool` /
   `report(b, ok)` sería la primitiva componible (guard puede quedarse como azúcar). De regalo:
   el campo `abierto_hasta` del `Breaker` es spanglish en superficie pública.
5. `jwt_verify` deja `exp`/`nbf` como política del llamador (decisión documentada y razonable),
   pero todo gateway la reescribe igual: candidato a `jwt_verify_claims(secret, token, now_ms)`.
6. **Docs desactualizadas (positivo)**: `webserver.serve` dice "VM only", pero el gateway completo
   — accept concurrente, streaming chunked (`stream_response`), señales, fibras — funciona
   compilado a NATIVO y rinde ~5.5k req/s en el hop completo local (VM ~4.9k; generador de carga
   co-alojado, cota inferior). Y `stream_response` + `http.stream_with` componen un proxy
   streaming real con contrapresión (canal acotado): primer byte en 2 ms con un upstream que
   tarda 500 ms — el "¿puede el handler streamear?" de IDEAS-APPS §1.1 tiene respuesta: SÍ.

## 66. Hallazgos de rayq — fsync y file locks ausentes de std/fs, rename validado, rpc estrenado (ago 2026)

Dogfood de `ray-apps/rayq` (broker de colas persistente at-least-once: WAL por cola, visibility
timeout, backoff, DLQ, compactación, worker de procesos). Es la app que convierte "escribir
archivos" en "ser una base de datos" — exactamente el territorio que IDEAS-APPS §1.2 predijo.

1. **[EJECUTADA — M115.1, DESIGN §108: `fs.sync(h)`] `std/fs` no tiene `fsync`/flush** (impacto: ALTO — el techo de durabilidad de todo el
   lenguaje). Un append (`fs.write(h, …)`) llega al page cache del SO y ahí se queda hasta que el
   kernel quiera: durable ante crash del PROCESO (verificado con `kill -9` a mitad de carga: el
   replay recupera exactamente lo no-ackeado), NO ante corte de luz. Sin `fs.sync(h)` (o un modo
   `open(path, "as")` con fsync-on-write) ningún programa raylang puede prometer durabilidad
   real. Es LA pieza que falta para rayq/raykv/cualquier motor de almacenamiento.
2. **[EJECUTADA — M115.2, DESIGN §109: `fs.try_lock(h)`/`fs.unlock(h)`] No hay file locks** (impacto: MEDIO-ALTO). Dos brokers sobre el mismo directorio
   intercalan appends y doble-entregan sin ningún aviso; el patrón estándar (flock sobre un
   `LOCK` file al arrancar) no se puede expresar. Candidato: `fs.lock_exclusive(h) -> Result`.
3. **`fs.rename` existe y es el reemplazo atómico que promete** (positivo): la compactación
   entera de rayq (reescribir a `.tmp` + rename encima + reabrir handle) funciona a la primera;
   verificado en caliente con 2000 acks → log de 0 bytes sin perder el handle. Matiz aprendido:
   tras el rename, el handle de append viejo apunta al inode ANTIGUO — hay que cerrar y reabrir
   (documentarlo en fs.rename evitaría un footgun clásico). `truncate` no hizo falta gracias a
   este patrón.
4. **`packages/rpc` estrenado y funcionó a la primera** (positivo): serve_graceful, ids
   correlados, Err de handler → Err del cliente de punta a punta, una fibra por conexión.
   ~6.8k push/s (cliente único secuencial, broker nativo; cada push con append a disco) y
   ~7.3k RPC/s en el drenado. Su README dice "Solo VM (fibras)" y es la TERCERA nota "VM only"
   desactualizada (webserver §65, ahora rpc): el broker nativo sirve RPC perfectamente —
   toca barrer esas notas de una vez.
5. Patrón que salió gratis: ids `uuid_v7` + Map (claves ordenadas) = el replay del WAL
   reconstruye la cola en orden FIFO sin ordenar nada explícitamente.

## 67. Hallazgos de raytop — ancho de celdas, escapes \x/\u, literales hex; el patrón TUI validado (ago 2026)

Dogfood de `ray-apps/raytop` (monitor de procesos de pantalla completa: alt-screen, redibujado
diferencial, orden/filtro/scroll, resize en vivo, muestreo vía `ps`). Es el escalón de terminal
que IDEAS-APPS §1.3 señalaba — y el veredicto es mejor de lo esperado: el estruje encontró
carencias de ERGONOMÍA, no de runtime.

1. **[EJECUTADA — M117, DESIGN §114: `term.width`/`char_width`/`fit`/`fit_right`] No hay ancho de celdas Unicode** (impacto: MEDIO — todo TUI lo necesita; predicho por el
   catálogo §2.3). Alinear columnas con CJK/kana/emoji exige un wcwidth; raytop trae el suyo
   (`src/width.ray`, ~40 líneas de rangos) y es el candidato directo a `term.width(s: string) ->
   int` (+ `term.fit(s, cells)`), junto al decode que ya existe. Sin él, cada TUI copiará la
   misma tabla de rangos.
2. **[EJECUTADA — M118, DESIGN §115: escapes `\0`/`\xNN`/`\u{H…H}` en string y char] Los literales string no admiten `\x`/`\u`** (impacto: BAJO-MEDIO — ergonomía repetida):
   todo escape ANSI se construye con `char_from_code(27)` + concatenación. raycode ya lo
   sufría (su ui.ray lo comenta) y raytop lo repite — segunda app con el mismo helper. O un
   escape `\u{1b}` en el lexer, o un módulo `term/style` con las secuencias hechas.
3. **[EJECUTADA — M118, DESIGN §115: `0x`/`0o`/`0b` con la base preservada por `ray fmt`] No hay literales hexadecimales** (`0x1F300` no lexea; impacto: BAJO): las tablas de rangos
   Unicode/bits quedan en decimal, incontrastables con cualquier spec. `0x` en el lexer es
   barato y paga en todo código de protocolos.
4. **Lo VALIDADO** (positivo, cierra preguntas abiertas del catálogo): (a) el patrón
   tecla-o-plazo (`io.read_timeout` + `term.decode` + ESC-suelto-25ms) escala del line editor
   de raycode al TUI de pantalla completa SIN cambios; (b) `term.raw` restaura el terminal
   siempre, incluso saliendo con el alt-screen activo; (c) el sondeo de `term.size()` por vuelta
   aguanta perfectamente a 1 s (SIGWINCH-por-`signals()` queda como refinamiento, no necesidad);
   (d) perf: frame completo (filtro+sort+layout de 749 procesos, 120×40) en 0.29 ms nativo /
   5.2 ms VM — el render jamás es el cuello (lo es `ps`, ~60-70 ms). Verificado bajo pty real
   en ambos motores, con el diff repintando exactamente las líneas cambiadas.

## 68. Hallazgos de raykv — return-en-spawn rompe el nativo, fs.write solo string; 86% de Redis real (ago 2026)

Dogfood de `ray-apps/raykv` (servidor RESP2 compatible redis-cli/redis-benchmark/net-redis, con
AOF lógica y pub/sub). El benchmark honesto que IDEAS-APPS §1.8 pedía, y otro bug de motor.

1. **[EJECUTADA — PR #140: el cuerpo literal se emite como IIFE `(|| {…})()` y la conversión Send se aplica al resultado] BUG — nativo: un `return;` dentro de una clausura `spawn` no compila** (impacto: MEDIO-ALTO
   — patrón común). El transpilador emite el cierre con el `return` que fuerza tipo `()` y luego
   remata el cuerpo con `__RaySend::U` → E0308 en el cargo del usuario. Repro mínimo: `spawn(fn()
   { while (true) { match (x) { A => { return; }, … } } });`. Workaround: bandera de salida.
   Tercer bug del transpilador (con §63 sort-float y §64 RefCell-en-variante); los tres son
   "código válido en VM que no compila o panica en nativo" — el contrato tres-motores necesita
   un harness de fuzzing/differential propio más que arreglos puntuales.
2. **[EJECUTADA — M115.1, DESIGN §108: `fs.write_bytes(h, data)`] `fs.write(handle, string)` no tiene gemelo binario** (impacto: MEDIO): sockets tienen
   `socket_write_bytes` pero fs no tiene `write_bytes(h, bytes)` (solo `append_file_bytes` por
   RUTA, que no compone con un handle abierto ni con seek). Consecuencia real: la AOF de raykv
   no puede persistir valores binarios (v1 acepta solo UTF-8) y cualquier formato binario en
   disco (RDB, WAL binario, delta de bloques de raysync) está bloqueado igual. La simetría
   `fs.write_bytes(h, b)` es la pieza.
3. **El benchmark de oro para PERFORMANCE.md**: `redis-benchmark -t set,get -n 50000 -c 50`,
   misma máquina — raykv NATIVO 124k SET / 133k GET rps (p50 0.23 ms) vs Redis 7 real 145k/154k
   (p50 0.18 ms): **~86% de Redis**, con parser RESP incremental en raylang puro y un round-trip
   de canal por comando (actor del keyspace). La VM sola: 81k/84k (~55% de Redis). La AOF
   sin fsync cuesta <1%. Conclusión: el camino socket→bytes→actor→Map del nativo es de clase
   producción; el runtime no es la excusa.

## 69. Hallazgos de raysync — watch de fs (4ª vez), metadatos, hasher incremental; la cripto vuela (ago 2026)

Dogfood de `ray-apps/raysync` (sync unidireccional cifrado con delta por bloques fijos de 64 KiB,
reconstrucción verificada + rename, `--watch`, `--delete`).

1. **[EJECUTADA — M115.4, DESIGN §111: `fs.watch` + `next_event[_timeout]` por eventos de kernel] Watch de filesystem — CUARTA app sondeando mtimes** (ray dev, raycode-dev, raylogs
   `--follow`, ahora raysync `--watch`). El caso está sobre-demostrado; la pieza mínima
   (watch de mtime por árbol, o kqueue/inotify detrás de una API de eventos) paga en cuatro
   sitios ya escritos.
2. **[EJECUTADA — M115.3, DESIGN §110: `fs.stat(path)` lstat con kind/mode/size/mtime_ms] `fs` sin metadatos**: ni permisos ni symlinks (solo `is_dir`/`is_file`/`file_size`/`mtime`)
   → un sync fiel (modo rsync -a) es inexpresable; los symlinks ni siquiera se pueden DETECTAR
   (se siguen o se ignoran a ciegas). Candidato: `fs.stat(path) -> {kind, mode, mtime, size}`.
3. **[EJECUTADA — M126, DESIGN §123: `sha256_init`/`sha512_init` + `hash_update` + `hash_final`]** **Sin hasher incremental en `std/crypto`** — tercera app copiando el patrón takeit de
   sha256 encadenado por chunks para hashear archivos grandes sin cargarlos enteros. Un
   `sha256_init/update/final` (o un `crypto.Hasher`) elimina la variante casera y su
   incompatibilidad mutua (cada app elige su seed/encadenado).
4. La ausencia de `fs.write_bytes(handle)` (§68) definió el diseño: el delta NO puede escribir
   bloques in-place; reconstruye a `.tmp` con `append_file_bytes` + rename — que resultó MEJOR
   (atómico y verificable antes de pisar), pero fue obligado, no elegido.
5. **Perf (positivo)**: 50 MB fríos cifrados (ChaCha20-Poly1305) + hasheados en ambos lados en
   0.17 s nativo por localhost; push sin cambios 53 ms; 1 byte cambiado en 30 MB → 64 KiB al
   wire, resultado byte-idéntico. La cripto de `ring` y el camino fs streaming no son cuello.

## 70. Hallazgos de raywatch — el certificado del peer es invisible, dns hereda el UDP bloqueante (ago 2026)

Dogfood de `ray-apps/raywatch` (monitor: checks http/tcp/tls/redis/dns con fibra por check,
SQLite, dashboard SSE, webhooks al cambiar de estado).

1. **[EJECUTADA — M124, DESIGN §121: `net.tls_peer_cert(h) -> PeerCert{subject, issuer, not_before_ms, not_after_ms, san}`]** **No se puede leer el certificado del peer TLS** — predicción del catálogo confirmada:
   `tls_connect` devuelve solo el handle. El handshake de rustls ya valida cadena y fechas (así
   que "conecta por TLS" sí es un check honesto), pero **"expira en N días"** — el check de TLS
   que todo operador quiere — es inexpresable. Superficie candidata:
   `net.tls_peer_cert(h) -> {not_after_ms, subject, san, issuer}` (rustls ya tiene los DER a
   mano en el handshake).
2. **[EJECUTADA — M121, DESIGN §118: dns acota su espera a 5 s → `Err("recv: read timeout")`, no cuelgue] `net/dns` hereda el UDP bloqueante de §64 y lo agrava**: `query_a` hace `recv_from` sin
   timeout → cada consulta DNS congela TODAS las fibras del proceso mientras dura, y un paquete
   perdido cuelga el monitor ENTERO para siempre. En un monitor la diferencia entre "check
   lento" y "proceso muerto". El arreglo de fondo es el de §64 (UDP async + timeout); mientras,
   net/dns debería al menos documentarlo en su cabecera.
3. **[EJECUTADA — M122, DESIGN §119: `net.tcp_connect_timeout(host, port, ms)` → `"connect timeout"`]** `tcp_connect` sin timeout de conexión (queda el del SO, ~75 s en macOS): un host que tira
   SYNs bloquea la fibra del check mucho más que su `timeout_ms` configurado. Candidato:
   `tcp_connect_timeout(host, port, ms)`.
4. **Positivo**: fibra-por-check escala sin drama (la pregunta del catálogo §1.6); SSE se sirve
   con `stream_response` sin soporte dedicado; `db/sqlite` + actor + webhooks-en-fibra componen
   limpio e idéntico en VM y nativo.

## 71. Hallazgos de raymail + raysite + raypass — STARTTLS validado, markdown seguro, entrada oculta, chmod (ago 2026)

Tres apps del eje "texto y cripto" en una tanda: `raymail` (SMTP real + MIME + sink),
`raysite` (generador estático sobre `markdown.to_html`) y `raypass` (bóveda de secretos CLI).

**Validaciones (positivo, cierran preguntas del catálogo):**
- **`tls_upgrade` funciona a la primera en su estreno** (raymail): EHLO → STARTTLS → upgrade
  in-place del handle → EHLO → MAIL contra `smtp.gmail.com:587` real; el 530 de auth llega A
  TRAVÉS de la sesión TLS. STARTTLS deja de ser superficie sin dogfood.
- **El modelo de seguridad de `markdown.to_html` aguanta su primera app** (raysite): el HTML
  embebido en un post sale ESCAPADO (verificado con `<script>` en tests) — "seguro por diseño"
  sin sanitizador externo, tal como promete su doc.
- **La pila M114 compone** (raypass): X25519 efímero + HKDF + AEAD = sealed box en ~40 líneas;
  manipulación = fallo de autenticación. Y el patrón temp+`fs.rename` da atomicidad por tercera
  vez (rayq, raysync, raypass).

**Carencias confirmadas:**
1. **[EJECUTADA — M125, DESIGN §122: `term.read_hidden(prompt)` + el núcleo puro `hidden_feed`; de paso arregla el backspace-por-byte que rompía UTF-8]** **Entrada oculta de passphrase = artesanía sobre raw** (raypass; predicho por §1.12): ~30
   líneas de raw + byte a byte + backspace + decode que toda herramienta repetirá. Candidata:
   `term.read_hidden(prompt) -> Result<string, _>`.
2. **[EJECUTADA — M115.3, DESIGN §110: `fs.chmod(path, mode)`] Sin `fs.chmod`/permisos** (raypass): una bóveda de secretos queda con el umask del proceso
   y no puede restringirse a 600. Misma familia que el `fs.stat` de §69 — la superficie de
   metadatos de fs es EL hueco transversal de esta tanda (watch §69, stat §69, chmod aquí).
3. **Zeroización inexpresable** (raypass): secretos en strings del GC sin borrado garantizado.
   Decisión de diseño consciente, pero conviene dejarla escrita en SECURITY.md del lenguaje.
4. **Codificaciones de correo a mano** (raymail; predicho por §1.7): RFC 2047 encoded-words,
   plegado a 78, base64 a 76 columnas, dot-stuffing — ninguna difícil, todas fáciles de hacer
   sutilmente mal → candidatas a un `std/mail` o al menos ejemplos canónicos.
5. **Sin normalización Unicode** (raysite): el slugify translitera a mano las vocales del
   castellano; NFD/NFKD no existen. Y **quinta app sondeando mtimes** (raysite serve).
6. **[EJECUTADA — M118, DESIGN §115: `'\0'` y `"\0"` ya lexean]** Menor (raymail): `'\0'` no es expresable como literal de char — el NUL de AUTH PLAIN se
   construye con `char_from_code(0)` (la misma familia que \x/\u de §67).

## 72. Hallazgos de raybot + raycall + raygame — websocket y M88 validados, sleep se pasa 6–10 ms (ago 2026)

La última tanda del catálogo de IDEAS-APPS (14 de 14 construidas). Tres apps de ejes distintos:
`raybot` (conexión websocket de larga vida), `raycall` (microservicios M88 completos) y
`raygame` (latencia de frame dura).

**Validaciones:**
- **`websocket_client` + `net/websocket` (lado servidor) funcionan a la primera en su estreno
  conjunto** (raybot): handshake, framing enmascarado, ping/pong automático en `read_message`.
  El test E2E fuerza CAÍDAS del gateway tras cada dispatch y el bot reconecta gen 1→2→3 con
  re-IDENTIFY y su contador SQLite intacto (1, 2, 3 a través de las caídas).
- **El arco M88 compone entero** (raycall): front HTTP → orders RPC → inventory RPC con el
  traceparent entrante adoptado, un span HIJO por salto (el test verifica mismo trace_id /
  span distinto en los TRES servicios — visible en un solo curl), deadline en cascada por
  `rpc.Req.deadline_ms` (el almacén lento muere en el salto correcto → 504), logs JSON con
  trace_id, y Err de handler → status HTTP honesto (409/502/504). Sin fricción alguna.
- **El patrón tecla-o-plazo aguanta 30 fps** (raygame): frame completo (lógica+layout+diff)
  en 21 µs nativo / 92 µs VM — 0.06% del presupuesto de 33 ms.

**Carencias:**
1. **[EJECUTADA — M119, DESIGN §116: `time.sleep` preciso vía `poll(2)`] `time.sleep` se pasa +6–10 ms consistentemente en AMBOS motores** (raygame; la predicción
   de §1.14 con número): `sleep(33)` duerme 39–40 ms de media y 43–44 ms en el peor caso.
   Dormir el presupuesto entero da ~25 fps, no 30. Workaround validado: reloj absoluto
   (`next += 33`) + `io.read_timeout` como única espera. Candidatos: afinar el timer del
   scheduler o documentar el patrón como canónico en MANUAL (juegos, pacing, muestreadores).
   → RESUELTO: la causa era `std::thread::sleep` (el `nanosleep` de macOS se pasa por *timer
   coalescing*); `poll(2)` con cero fds honra el timeout ajustado. Ambos motores a ~34 ms para
   `sleep(33)` (~1 ms de overshoot). El reloj absoluto sigue siendo la práctica para pacing
   sin deriva (documentado en MANUAL + el `///` de `sleep`).
2. **[EJECUTADA — M127, DESIGN §124: `rpc.pool(host, port, size)` + `pool_call*` — el canal acotado como cola, marcado perezoso, reconexión automática]** **El cliente `packages/rpc` es secuencial por conexión** (raycall): handlers concurrentes no
   pueden compartir uno → conexión por llamada. El hueco de producción de rpc es un pool o
   multiplexación por id (el streaming diferido de su README apunta ahí).
3. Sin `try_recv`/select-timeout, matar una fibra dormida sigue sin poderse (raybot): el patrón
   generación-en-el-canal deja una fibra de heartbeat huérfana latiendo por cada reconexión —
   inofensivo pero acumulativo en procesos de semanas. Refuerza §64.
4. **[EJECUTADA — M133, DESIGN §130]** `grpc_client` queda como la ÚNICA superficie de red del
   paquete sin dogfood (necesita un servicio gRPC externo real). → Dogfood contra grpc-go real:
   dos bugs cazados (HPACK sin Huffman; trailers-only roto), ambos arreglados con guardas.

**Cierre del catálogo**: con esta tanda, las 14 apps de IDEAS-APPS están construidas (§§63–72).
Los temas transversales que dejaron **están todos ejecutados y mergeados** (ago 2026):
- **fs de producción** — `fs.sync` (§66.1, M115.1), `fs.try_lock`/`unlock` (§66.2, M115.2),
  `fs.stat` (§69.2, M115.3), `fs.chmod` (§71.2, M115.3), `fs.watch` por eventos de kernel
  (§69.1, M115.4), `fs.write_bytes`-en-handle (§68.2, M115.1).
- **Los 4 bugs de divergencia VM/nativo** — sort-float (§63.1), close-con-lector (§64.1),
  RefCell-en-variante (§64.2), return-en-spawn (§68.1): los cuatro cerrados en el PR #140.
- **Concurrencia con-plazo** — `try_recv` (§64.4, M116) y `select_timeout` (§64.4, M116.1).
- **Terminal y tiempo** — `term.width`/`char_width`/`fit` (§67.1, M117), escapes `\0`/`\x`/`\u`
  (§67.2/§71.6, M118), literales hex/octal/binario (§67.3, M118) y `time.sleep` preciso
  (§72.1, M119).

**[EJECUTADA — M120, DESIGN §117]** El **harness diferencial VM/nativo** que §68 pedía existe:
`tests/native_differential.rs` — programas generados por clase de interacción, 3 motores byte a
byte, bisección automática, semillas reproducibles; humo en cada `cargo test`, campaña en cada
push, nocturna con presupuesto alto. Validación inmediata: su primera corrida cazó y dejó
corregidos TRES bugs nuevos del backend nativo (print de u*, genéricos acotados `T: Ord`, método
dyn en posición de argumento) — los tres de la clase §63/§64/§68 y ninguno cubierto por el corpus.

**[EJECUTADA — M121, DESIGN §118]** El timeout UDP/dns (§64.5/§70.2): `net.set_read_timeout`
aplica a UDP en los tres motores (espera vencida = `Err("read timeout")`) y `net/dns` acota su
respuesta a 5 s. La nota "UDP bloquea todas las fibras" resultó rancia (VM cede desde M20.11,
nativo-fibras desde F4) — el hueco real era solo el timeout.

**[EJECUTADA — M122, DESIGN §119]** `tcp_connect_timeout` (§70.3): el connect con plazo — el
intento vencido falla con `"connect timeout"` en vez de los ~75 s del SO.

**[EJECUTADAS — M123/M124, DESIGN §120/§121]** `Request.remote` + `net.peer_addr` (§65.1) y
`net.tls_peer_cert` (§70.1): la dirección del cliente y el certificado del peer.

**[EJECUTADA — M125, DESIGN §122]** `term.read_hidden` (§71.1): la passphrase sin eco, con núcleo
puro probeable sin tty.

**[EJECUTADA — M126, DESIGN §123]** El hasher incremental (§69.3): sha256_init/sha512_init +
hash_update + hash_final, digest idéntico al de una pasada, estado en el runtime compartido.

**[EJECUTADA — M127, DESIGN §124]** El pool de `packages/rpc` (§72.2): `pool(host, port, size)` +
`pool_call*` + `pool_close` — el canal acotado como cola del pool, marcado perezoso, checkout que
aparca (backpressure) y reconexión automática tras un fallo. (Se descartó multiplexar por id sobre
una conexión: el servidor procesa en serie por conexión — no compraría paralelismo.)

**[EJECUTADAS — M128, DESIGN §125]** El lote de menores de std: regex con grupos con nombre
(§63.3: `(?P<name>)`/`(?<name>)` + `group_names` + `captures_map`, la vía acelerada intacta), csv
incremental (§63.4: `parser`/`feed`/`finish`, `parse_csv` reescrito encima), `[[toml]]` (§65.3:
aplanado como `ruta.N.clave` + `toml_array_len`) y `jwt_verify_claims` (§65.5: firma + `exp`/`nbf`
en una llamada). De pasada, los errores de regex/csv pasan a inglés.

**[EJECUTADAS — M129, DESIGN §126]** El lote B: `admit(b)`/`report(b, ok)` en resilience (§65.4;
`guard` queda como azúcar; `abierto_hasta`/`hasta` → `open_until`/`until`), `ray test` drena TODOS
los handles del SO al aislar cada `@test` (§65.2; listeners incluidos, los hijos no se matan) y
`webserver.gzip(req, resp)` + `accepts_gzip` (negociación de Accept-Encoding, explícita por
handler: comprimir cuesta CPU en la VM).

**[EJECUTADAS — M130, DESIGN §127]** El lote C (motores): `net.shutdown_write(h)` — half-close
`shutdown(SHUT_WR)` (§64.3; el idiom netcat expresable al fin, solo TCP, errores estables en los
cuatro sabores) y `exit(code)` (§64.6; builtin núcleo que diverge como panic, termina el proceso
desde cualquier fibra, `try_call` no lo captura; SPEC y espejo selfhost en tándem).

**[EJECUTADAS — M131, DESIGN §128]** El lote D: `std/text` gana NFC/NFD/NFKC/NFKD (§71.5; crate
unicode-normalization vía ray-runtime, feature `unicode` por defecto/por-uso) y `net/mail` las
codificaciones de correo (§71.4; RFC 2047/5322/2045/5321 en raylang puro). El ítem "hora local
§42" resultó YA ejecutado (M85, packages/tz: TZif v2 + system() + DST como API) — la lista lo
arrastraba rancio.

**[EJECUTADO — M132, DESIGN §129]** El barrido final de spanglish: TODOS los errores de la
stdlib embebida en inglés (~110 strings: json, toml, inflate, base64, hex, url, huffman,
template, protobuf, time, fs), goldens en tándem; los paquetes Tier 2 ya estaban limpios. De
regalo cazó un bug real de M130: `exit(code)` nativo perdía la salida pendiente (flusheaba el
buffer equivocado; ahora drena el hilo escritor con `__ray_flush_prints()`).

**[EJECUTADA — M133, DESIGN §130]** El dogfood de gRPC (§72.4): `grpc_client` contra grpc-go
REAL (fixture Go reproducible, codec crudo sin protoc; `tests/grpc_real_cli.rs`, `#[ignore]` por
el toolchain de Go). Cazó dos bugs en la primera llamada: el decoder HPACK sin literales Huffman
(grpc-go comprime siempre; la tabla YA vivía en std/huffman — puente de 15 líneas + vectores
§C.4 como guarda) y la respuesta trailers-only de un error rota en `grpc_unframe(b"")`.

**Con esto, el backlog COMPLETO del dogfood de 14 apps está ejecutado — hitos y menores — la
superficie entera del lenguaje habla inglés, y TODA la superficie de red del paquete net tiene
dogfood.** Queda solo el VPS 24/7 de rayrelay (validación de operación, no de superficie).

## 73. El manejador de paquetes en escenario real — asperezas del estreno (ago 2026) — M134

Round-trip real contra github.com/ray-language (cápsula `greeting` + índice `ray-index`,
DESIGN §131): el flujo entero funciona; el bug del publish (identidad de cápsula) se arregló en
el propio M134. Quedan clasificadas dos mejoras:

1. **Shallow clone / clone parcial de deps** (impacto: MEDIO — coste de red y disco): cada
   dependencia clona el repo ENTERO (~4 s y decenas de MB para un paquete de 3 archivos si vive
   en un monorepo grande). `--depth 1` no compone con checkout de SHA arbitrario (el lockfile
   fija commits); diseño candidato: `clone --filter=blob:none` + checkout, o depth para tags y
   fallback a full para SHAs.
2. **Normalizar github-ssh→https al publicar** (impacto: BAJO — UX): sin `--repo`, la URL del
   índice sale del `origin` del publicador (ssh si empuja por ssh) y el consumidor anónimo no
   puede clonarla. Candidatos: reescritura `git@github.com:` → `https://github.com/` con aviso,
   o un warning duro al publicar URL no-anónima. (El error de un repo privado por https también
   podría explicarse mejor que el "could not read Username" crudo de git.)

## 74. El sitio del lenguaje — landing + SPEC + book + playground publicados (ago 2026)

**Problema**: raylang 1.3.x se publica (Releases, extensión VSCode) pero no tiene sitio web:
la SPEC, el book y el playground existen solo en el repo (el ítem "Libro y sitio publicados"
de RELEASE-1.0.md es lo único del lanzamiento aún pendiente). **Impacto: MEDIO** (visibilidad
y adopción; cero riesgo de bloqueo de features — es tooling/distribución, no lenguaje).

Plan por fases, con dogfooding como principio (el sitio lo genera raylang mismo, como el sitio
del registro de M84):

- **Fase 1 — ✅ HECHA (26 ago 2026): `site/`**, el generador estático en raylang puro
  (`site/site.ray` + templates nativos `.ray.html` del mismo directorio), con dogfooding
  doble: layout con herencia + landing + shell de la SPEC con `{{& body }}`, y la SPEC
  renderizada con `std/markdown`. Diseño con el **manual de marca**
  (`assets/branding/raylang-brand.pdf`): paleta océano, Space Grotesk/JetBrains Mono
  **embebidas** (las woff2 del playground → sitio 100% autocontenido, cero CDNs), símbolo en
  la nav, Manta en la banda final, modo claro/oscuro. La landing suma: muestra de código con
  efecto de tipeo (con fallback `noscript` y `prefers-reduced-motion`), sección de agentes
  LLM (llms.txt + `ray mcp`), el **playground WASM embebido** (iframe lazy + copia completa
  bajo `playground/`), nueve tarjetas de features y el ecosistema (apps de la organización +
  packages/ + registro). Salida determinista, byte-idéntica por ambos motores
  (`tests/site_cli.rs`). Previsualización local: servir la salida con cualquier estático
  (`python3 -m http.server -d <salida>`; el iframe del playground prefiere http a file://).
- **Fase 1b — ✅ HECHA (26 ago 2026): el playground con EDITOR REAL y el LSP embebido.**
  El despacho del Language Server se extrajo del bucle stdio (`lsp::handle_message`,
  independiente del transporte) y se exporta desde el wasm (`src/wasm.rs::lsp`: un mensaje
  JSON-RPC por llamada → el array de mensajes emitidos). El editor pasa del overlay
  textarea+pre a **CodeMirror 6** (`playground/editor/` → `editor.bundle.js` vía npm/esbuild;
  artefacto NO versionado, como el wasm — `build.sh` produce ambos): diagnósticos en vivo,
  **autocompletado** (símbolos + builtins + snippets), hover con tipos, **signature help**
  (tooltip CM propio con el parámetro activo) y **formateo** LSP (`ray fmt`), tema de marca.
  De paso se destapó y arregló que el build wasm32 llevaba roto desde M100 v3/M124/M126/M131
  (usos de ray_runtime sin guarda `cfg`). La guarda de CI EXISTÍA (IDEAS §57) pero vivía al
  final del job de tests: cualquier rojo anterior la dejaba sin ejecutar (y main llevaba en
  rojo por un enlace de docs) → ✅ promovida a job propio que corre siempre, en paralelo.
  La landing suma la sección de editores (VSCode marketplace, Sublime, Neovim/Helix).
- **Fase 1c — ✅ HECHA (26 ago 2026): benchmarks en el sitio.** `site/bench_chart.ray` parsea
  la tabla de resultados de `benchmarks/poly/README.md` con `std/markdown` (`Block.Table`) y
  genera barras CSS (raylang nativo = 1× vs rustc/go/node, tope 4×) — dogfooding triple: el
  sitio, sus templates y ahora sus gráficas los produce el lenguaje, con una sola fuente de
  verdad (re-medir el banco regenera todo). Banda "Medido, no prometido" en la landing +
  `bench.html` con el README completo renderizado; la fecha/hardware sale del encabezado
  "## Resultados (…)" del propio banco.
- **Fase 2 — ensamblado**: el sitio completo = landing + SPEC + book (`mdbook build`) +
  playground (WASM) bajo un mismo árbol de salida; enlaces internos de SPEC.md (DESIGN.md,
  MANUAL.md…) resueltos o neutralizados.
- **Fase 3 — ✅ HECHA (26 ago 2026): deploy a GitHub Pages.** `.github/workflows/pages.yml`:
  en cada push a main construye el playground (wasm release + wasm-opt -Oz + bundle CodeMirror),
  genera el sitio con `site/site.ray`, ASEVERA el sitio completo (wasm y bundle presentes — un
  deploy degradado no pasa en silencio) y publica con upload-pages-artifact/deploy-pages.
  El usuario activó Settings → Pages → Source: GitHub Actions. URL:
  https://ray-language.github.io/raylang/. Se eligió push-a-main (sitio = documentación viva);
  las releases conservan su workflow propio.

## 75. Linguist: highlighting de `.ray` en GitHub — bloqueado por base de usuarios (ago 2026)

**Qué es.** PR a [github-linguist/linguist](https://github.com/github-linguist/linguist) para que
GitHub coloree los `.ray` y los cuente en las estadísticas de lenguaje. El prerequisito técnico ya
está: la gramática TextMate canónica vive en `ray-language/raylang-grammar` (pública), y los
`examples/` del monorepo sirven como samples de código real (tutoriales/"hello world" no los
aceptan). El PR en sí es ~1 día: entrada en `languages.yml`, `script/add-grammar`, samples,
`script/update-ids`, y la búsqueda de evidencia en el template.

**Por qué está bloqueado (política verificada el 27 ago 2026, sin cambios).** Linguist rechaza
lenguajes nuevos sin uso real: *"we do not accept PRs for very new or hobby languages, and will
close any such PRs"*. El umbral vigente:

- **≥ 2.000 archivos `.ray` indexados en el último año** en GitHub público, **excluyendo forks**
  (el umbral de 200 es solo para filenames únicos por repo, tipo `Makefile` — no es nuestro caso).
- **Distribución entre usuarios distintos**, revisada a mano: filtran al dueño del lenguaje con
  `-user:ray-language`, así que **nuestros repos (monorepo, espejos, apps) no cuentan**.

**Estado medido (27 ago 2026):** ~418 archivos con `extension:ray` y contenido raylang en la
búsqueda de código — lejos del umbral. El camino es la base de usuarios (sitio, releases,
extensiones de editor, paquetes), no hay atajo legítimo (`.gitattributes` solo reasigna entre
lenguajes ya existentes).

**Disparador.** Re-medir cada pocos meses: `extension:ray -user:ray-language` (excluyendo forks)
en la búsqueda de GitHub; al acercarse a 2.000 con distribución sana, ejecutar el PR con el repo
ya preparado.

**Impacto:** ninguno sobre el lenguaje (todo externo). **Valor:** visibilidad — cada repo `.ray`
de terceros se vería como raylang en GitHub, y de paso el propio linguist referencia la gramática.

## 76. DX de `ray dev` — adyacentes tras el watcher por eventos (ago 2026)

Con el watcher de `ray dev` migrado a eventos de kernel (M139, DESIGN §137), quedan
clasificadas las dos mejoras vecinas que se evaluaron y NO entraron en ese arco:

- **`ray test --watch`** — ✅ HECHA completa: simple (M140, DESIGN §138) y **selectiva** (M141,
  DESIGN §139): re-corre solo las suites cuyo grafo de imports alcanza el cambio, con grafo
  calculado fresco por el loader en cada cambio (sin estado que invalidar) y vías de escape a
  correr-todo (ray.toml, borrados, desconocidos, suite rota).
- **Recarga de templates sin reinicio** (impacto MEDIO): hoy editar un `.ray.html` reinicia el
  proceso; los templates ya se compilan en memoria al cargar, así que un modo dev del webserver
  podría recompilar solo el template tocado y conservar el estado del proceso. Toca el borde
  loader/webserver (invalidación por archivo) y solo vale en dev — cuidar que el modo
  producción quede intacto y byte-idéntico.
- **Hot swap en la VM viva** (impacto ALTO, futuro lejano): reemplazar funciones en caliente
  exige soporte del checker y la VM (tabla de funciones, closures capturados, fibras vivas) y
  difumina la byte-identidad de los tres motores. El beneficio marginal sobre restart+drenado+
  socket-activation no lo justifica hoy; se anota para que ninguna decisión lo bloquee por
  accidente (la tabla de funciones de la VM ya es indirecta — buen augurio).

## 77. `ray build --native -o` sobrescribe in-place → SIGKILL en macOS (ago 2026)

Hallado en `ray-apps/rallyx` (27 ago 2026): recompilar con `ray build --native src/main.ray
-o rallyx --release` sobre un binario `rallyx` YA existente produce un binario que macOS mata
al instante con SIGKILL (exit 137) — incluso `./rallyx --help`. Causa conocida de la
plataforma: sobrescribir el archivo de un binario firmado (mismo inode) invalida la caché de
firma ad-hoc del kernel (`CODESIGNING`), que mata el proceso al exec. El workaround es
`rm -f rallyx` antes de recompilar (o compilar a nombre temporal + `mv`, que reemplaza el
inode). Propuesta: que `ray build` haga **unlink del output antes de escribir** (o escriba a
`<out>.tmp` y renombre — `fs.rename` ya es atomic replace, §63); coste trivial, elimina un
fallo silencioso y desconcertante del ciclo edit-build-run en macOS. Repro: compilar dos
veces seguidas al mismo `-o` y ejecutar.

## 77. `std/image` — decodificar PNG sobre el inflate existente (ago 2026) — ✅ HECHA (M144, DESIGN §141)

Reporte desde un juego de demostración: "cargo un sprite.png" no se puede hoy. El diagnóstico
del reporte ("falta DEFLATE") era FALSO — `std/inflate` ya trae DEFLATE completo + envoltorios
zlib/gzip endurecidos (M64: input corrupto = `Err`, tope anti-bomba), así que el IDAT de un PNG
se descomprime hoy con `zlib_inflate`. La lección de discoverabilidad (buscó "zlib" y no lo
encontró) quedó saldada en REFERENCE §std/inflate.

**Lo que falta de verdad**: el parser PNG encima — `std/image` con `decode_png(bytes) ->
Result<Image, string>` (`Image { width, height, pixels: bytes /* RGBA8 */ }`):

- Chunks: IHDR/PLTE/IDAT (concatenados)/IEND/tRNS; CRC con el `crc32` ya existente.
- Des-filtrado por scanline: None/Sub/Up/Average/Paeth.
- Tipos de color 0/2/3/4/6 y profundidades 1/2/4/8 (16 → convertir a 8); interlace Adam7
  puede diferirse a v2 (raro en assets de juego) devolviendo `Err` claro.
- Raylang puro, cero runtime nuevo (mismo espíritu que inflate/markdown): 3 motores gratis.
- Vectores de prueba: la PngSuite de Willem van Schaik (el estándar de facto).

**Impacto**: MEDIO-ALTO en valor (sprites para juegos/TUIs gráficas — con §78 —, thumbnails,
raysite), BAJO en riesgo (módulo hoja). Encode PNG (con `std/deflate`) es la continuación
natural pero va después: decode desbloquea el caso real.

## 78. Terminal gráfico: capacidades y píxeles de celda en `std/term` (ago 2026) — ✅ HECHA (M143, DESIGN §140)

Del mismo reporte, las dos piezas para elegir "peldaño" gráfico (¿truecolor? ¿sixel? ¿kitty?)
y escalar imágenes al layout:

- **`term.cell_pixels()` (aperitivo, casi gratis)**: `TIOCGWINSZ` ya se lee en
  `builtins::term_host::size()` y su `WinSize` **ya trae `ws_xpixel`/`ws_ypixel` — hoy se
  descartan**. Exponer el tamaño en píxeles (superficie a decidir: `term.size_px() ->
  Option<(int, int)>` del área total, y el de celda = área/filas·columnas) es plomería mínima.
  Gotcha conocido: muchos terminales reportan 0 (no soportan) → `Option`/`None`, jamás un 0
  que divida.
- **`term.capabilities()` (el equivalente de `term.width` de M117)**: emitir queries (DA1
  `ESC [ c`, XTGETTCAP de kitty, `COLORTERM`/`TERM` como pista barata) y leer la respuesta con
  plazo — hoy se puede a mano con `term.raw` + `io.read_timeout`, y esa es justo la señal de
  que le toca a la stdlib (cada TUI lo reimplementará peor). Con su miga: terminales que no
  contestan (plazo corto y degradar), respuestas variopintas, y NO emitir queries si stdout no
  es tty. Forma sugerida: struct con lo decidible (`truecolor`, `sixel`, `kitty_graphics`,
  `colors_256`) y todo `false` como degradación segura.

**Impacto**: BAJO en riesgo (aditivo en `std/term`); el orden natural es píxeles → capacidades
→ (con §77) imágenes en terminal.

## 79b. `std/audio` v2 — afinado de latencia (ago 2026, tras el dogfood de rallyx)

Diferidos de M145 (la adenda de DESIGN §142 tiene el contexto): **hint de latencia en `open`**
(elegir el tamaño del anillo/buffers: un juego rítmico quiere <30 ms, una radio tolera 200) y
**`audio.played_ms(h)`** (la posición real de reproducción — AudioQueueGetCurrentTime /
snd_pcm_delay — para sincronizar visuales con el audio). Impacto BAJO (aditivos); ejecutar
cuando un dogfood los pida con caso concreto.

## 79. `std/audio` — salida PCM al dispositivo (ago 2026) — ✅ HECHA (M145, DESIGN §142; la decisión cpal-vs-externs se volteó a EXTERNS al implementar: cpal exigía headers de ALSA en build)

Segundo reporte del juego de demostración (tras §77/§78). De las cuatro piezas pedidas, DOS
partían de premisas falsas — verificado contra el código y corregido aquí para el registro:

- **"No hay forma de mantener un hijo vivo empujándole samples" — FALSO desde M100 v3**:
  `process.cmd(...).stdin_pipe().stream()` + `Proc.write(bytes)` (+ `close_stdin()` = EOF)
  alimentan un `ffplay -f s16le -i -` vivo, y `write` APARCA LA FIBRA si el pipe se llena →
  contrapresión = *pacing* gratis contra el consumo real. La música reactiva/síntesis en vivo
  funciona HOY por esta vía (REFERENCE §std/process, "Sesión persistente").
- **"Sin FFI no puedes bindear CoreAudio" — raylang tiene FFI desde M41.** Lo cierto de fondo:
  el FFI no soporta **callbacks C→raylang**, y CoreAudio es *pull* (render callbacks) → en
  macOS no es bindeable por el usuario; en Linux ALSA es *push* (`snd_pcm_writei`) y SÍ lo es.
  Esa asimetría es el mejor argumento del módulo dedicado. (Callbacks en FFI = arco de
  LENGUAJE aparte, impacto ALTO, no un prerequisito de esto.)

**Lo que falta de verdad**: `std/audio`, el análogo de `term.*` para sonido — primera clase,
sin proceso externo ni latencia de spawn:

- Superficie push mínima: `open(sample_rate, channels) -> Result<int, string>` ·
  `write(h, samples: bytes) -> Result<int, string>` (PCM entrelazado; formato a decidir:
  s16le como mínimo común) · `drain(h)` · `close(h)`.
- **Contrapresión por aparcado de fibra** (el patrón de todo el I/O de la VM): `write` con el
  buffer del dispositivo lleno aparca — el pacing del juego sale del propio dispositivo, sin
  relojes.
- **Decisión de diseño abierta — la dependencia**: `cpal` (el estándar Rust: CoreAudio/ALSA/
  WASAPI de una, precedente de foco-producción como ring/rusqlite/notify) tras feature
  excluible `--without audio`, vs. externs a mano por plataforma (cero deps, doble
  mantenimiento, Windows cuesta arriba). Inclinación: cpal. En nativo, vía `ray-runtime`
  enlazado bajo demanda, como TLS/sqlite.
- Interp = oráculo: misma superficie, puede ser bloqueante simple.

**Decoders MP3/OGG**: mismo cajón que PNG (§77) — ports puros grandes (minimp3/vorbis),
clasificados LEJOS; WAV no necesita nada (cabecera + PCM a mano) y `stdin_pipe`→ffplay ya
decodifica cualquier formato mientras tanto.

**Impacto**: MEDIO en valor (juegos/TUIs con sonido, alertas, el arcade del reporte), MEDIO en
esfuerzo (runtime con-crate en 3 motores), BAJO en riesgo de diseño (superficie push pequeña).

## 80. Apps de escritorio nativas — el "Tauri de raylang" (ago 2026)

**Qué es.** Apps de escritorio multiplataforma: webview NATIVO del SO (no Chromium embebido) +
el backend en raylang + bundling. El 70% ya existe — Tauri es esencialmente *webview + backend
Rust + bundling*, y raylang ya tiene el backend entero: webserver (streaming/WS/SSE), templates
`.ray.html` tipados, sesiones, sqlite/fs/crypto, canales/fibras (actores = eventos de UI), y el
binario único de `ray build --native`.

**Fase 0 — funciona HOY, cero código nuevo** ✅ (28 ago 2026): binario que levanta el webserver
en `127.0.0.1` con puerto libre del SO (`tcp_listen(_, 0)` + `local_port`) y abre el navegador
del sistema cuando el servidor ya acepta (sondeo con `tcp_connect_timeout`); salir desde la UI
con `POST /quit` → `exit(0)` en fibra aparte. Documentado como patrón en MANUAL §14 + ejemplo
ejecutable `examples/web/desktop/` (verificado VM y nativo).

**Los cinco huecos reales, en orden de dureza:**

1. **`std/ui`: ventana + webview nativo** — el arco duro (~std/audio ×3). NO es FFI-able por el
   usuario (exige callbacks C→raylang, la limitación conocida): subsistema del runtime como
   audio/watch. La lección de cpal (M145) aplica: `wry` exige headers GTK/WebKit en BUILD en
   Linux → la vía coherente es A MANO — WKWebView por mensajes objc en macOS (frameworks
   siempre presentes, enlace limpio), WebKitGTK por `dlopen` en Linux, Windows DIFERIDO honesto
   (COM a mano es brutal; ahí se re-evaluaría un crate).
2. **Contrato del hilo principal**: AppKit/GTK exigen poseer el main thread y su loop. Diseño
   que encaja: `ui.run()` captura el hilo principal y los eventos llegan por un **`Channel`**
   (precedente exacto de `signals()`: self-pipe → canal → la fibra aparca). La app vive en
   fibras; la UI es un actor más — cero conceptos nuevos.
3. **IPC JS↔raylang — el atajo grande: NO inventar nada.** El webview carga del webserver
   embebido y habla fetch/WS con los handlers de `packages/web`: el framework ES el puente
   (127.0.0.1 + puerto aleatorio + token, el hardening estándar). Scheme custom (`app://`)
   después, si hace falta offline/sin-puerto.
4. **Assets embebidos**: los templates ya compilan al binario, y el patrón autocontenido
   existe (verificado en store: imágenes SVG GENERADAS en código + templates); pero los
   estáticos de archivo (el `dist` de Astro de store, css, fuentes) se sirven DESDE DISCO vía
   `static_files_cached` — deliberado allí ("editable sin recompilar"), insuficiente para el
   binario-único de una app de escritorio → `ray build --native --embed assets/` (útil mucho
   más allá de UI). El webserver ganaría un `static_embedded` que sirva de ese espacio.
5. **`ray bundle`**: `.app` con icono/Info.plist (barato: es una estructura de carpetas),
   `.desktop`/AppImage; firmado/notarización después. Tooling puro.

**Faseo**: F0 patrón navegador (documentar) → F1 `std/ui` macOS (ventana + webview + `eval_js`
+ canal de eventos) → F2 Linux por dlopen + embed + `ray bundle` → F3 menús/diálogos/tray/
updater.

**F1 ✅ (29 ago 2026, M146, DESIGN §143)**: `std/ui` entregada — open/eval_js/eventos/close,
tres motores byte-idénticos, headless para CI, ejemplo `examples/web/desktop_window/`. La
bifurcación (b) se resolvió con un tercero: NO hay `ui.run()` — el runtime captura el hilo 1
transparente (VM: estaba ocioso en el `join()`; nativo: el main emitido mueve el programa a un
hilo del SO — jamás a una fibra: payload de pánico, pila e `in_fiber()`). Diferidos de v1 a
dogfood: eval con retorno (ABI de blocks), más kinds de evento (resize/focus), scheme `app://`.

**F2 ✅ (29 ago 2026, M147/M147c/M147d, DESIGN §144)**: `std/embed` (+ `static_embedded` +
`ray dev` recarga assets sin reiniciar), `ray bundle` (.app/.desktop) y el backend GTK3+WebKitGTK
por dlopen — el contrato de std/ui en los DOS sistemas. ⚠️ PENDIENTE honesto: la ventana GTK
real no se ha visto en pantalla (validada por compilación cruzada + headless + negativo de CI);
primer usuario con desktop Linux = el dogfood.

**F3 v1 ✅ (29 ago 2026, M148, DESIGN §145)**: menús (estándar automático — el fix del
portapapeles del webview — + custom por datos con eventos `"menu"`+tag) y diálogos de archivo
(open/folder/save, modales, `RAY_UI_PICK` en headless). QUIRÚRGICO por doctrina — DIFERIDOS a
dogfood: **tray** (NSStatusItem barato, sin caso de uso aún), **notificaciones**
(UNUserNotificationCenter exige bundle+autorización → encaja tras `ray bundle` cuando un
dogfood lo pida), **updater de apps** (arco propio), **aceleradores de teclado GTK**
(GtkAccelGroup), **⌘Q como evento** (hoy `terminate:` sale directo, estándar de macOS; un
app-delegate lo convertiría en evento interceptable). Siguiente: §80b (móvil) cuando toque.

**DECISIÓN FIJADA (28 ago 2026, con el usuario): el camino es WEBVIEW, sin ambigüedad.** Bindear
toolkits nativos completos (AppKit/UIKit/GTK/WinUI) o construir renderer propio eclipsaría al
lenguaje y quedaría a medio hacer — no es opción para el tamaño del proyecto. Lo "nativo" se
invierte donde de verdad se SIENTE: menús, atajos del SO, diálogos de archivo, tray,
notificaciones, dock (superficies chicas, bindeables a mano — el patrón objc/dlopen de audio).
`std/ui` es una PRIMITIVA ventana+webview, no un framework: si un dogfood exige un widget
nativo concreto, se añade quirúrgicamente, no por doctrina.

**Bifurcaciones de diseño anotadas** (no decidir en solitario al ejecutar): (a) a-mano vs
wry/tao — la inclinación es a-mano por la lección cpal, pero Windows puede forzar el crate;
(b) el contrato exacto del main thread en el runtime de fibras (¿`ui.run` no retorna? ¿main
migra a fibra?); (c) IPC por webserver propio vs scheme custom (v1: webserver).

**Impacto**: ALTO en valor (el pitch "apps de escritorio en un binario con el framework web del
propio lenguaje como UI" es diferenciador real), ALTO en esfuerzo (F1 es el mayor subsistema de
runtime desde las fibras). Ninguna decisión pendiente lo bloquea; F4 (assets/bundle) son
independientes y pueden adelantarse.

**§80b — Móvil (iOS/Android), la extensión sobre los MISMOS cimientos** (el camino que Tauri
2.0 validó: mismo core, móvil encima). A favor desde ya: el nativo compila en principio a
`aarch64-apple-ios`/`aarch64-linux-android` (AOT puro — sin choque con la prohibición de JIT de
Apple; kqueue ES Darwin, epoll en Android; rustls/rusqlite/corosensei compilan); el webview de
iOS es WKWebView — la MISMA API que macOS, la F1 de escritorio paga el 80% del peaje de iOS; el
IPC por webserver embebido está permitido en ambas plataformas; `--embed` pasa de conveniencia
a REQUISITO (las stores prohíben descargar código/UI). Lo genuinamente nuevo: (1) el
transpilador debe poder emitir LIBRERÍA (`staticlib`/`cdylib` con entrada invocable), no un
`main` — una app móvil es un shell UIKit/Activity que llama a tu código; (2) shells por
plataforma generados por `ray bundle --ios/--android` (plantilla Xcode / Gradle+JNI); (3) el
puente de ciclo de vida (background/foreground/memoria) por el mismo canal de eventos; (4) la
muralla operativa (firmas, provisioning, emuladores, review). Android WebView vía JNI es la
única pieza de UI sin precedente en el arco. ORDEN: escritorio primero (feedback en segundos,
sin firmas) → iOS (reusa objc/WKWebView de F1) → Android.

## Cómo usar este archivo

- Cuando una idea madure y se comprometa, se **mueve** a [DESIGN.md](DESIGN.md) con su hito, y lo
  que quede aquí es su clasificación de impacto.
- Cuando aparezca una idea nueva, se **agrega aquí** con su clasificación de impacto, no
  directamente al diseño.
- Antes de cada arco grande, revisar este archivo: puede que alguna decisión "tardía" deba
  adelantarse por una restricción de arquitectura.
- El **orden de las secciones numeradas es cronológico** (por cuándo se clasificó la idea), no por
  importancia ni por estado: una sección puede estar ya ✅ ejecutada. Lo entregado se lee en
  [CHANGELOG.md](CHANGELOG.md); lo vigente, en [SPEC.md](SPEC.md) y [REFERENCE.md](REFERENCE.md).
