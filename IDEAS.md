# raylang — Backlog de features y su impacto en el diseño

> Registro de ideas que NO entran ahora pero queremos considerar a futuro. Para
> cada una anotamos: **impacto** en el diseño actual, **cuándo** podría llegar, la
> **decisión/recomendación** (si la hay) y la **restricción** que debemos respetar
> hoy para no bloquearla.
>
> Las features ya comprometidas (tipos suma, genéricos, `Result`/`?`, UFCS,
> pipelines, stdlib) viven en [DESIGN.md](DESIGN.md) §2 y §10, no aquí.

## Resumen de impacto

> **Estado tras M26** (núcleo M1–M14 completo con **meta-circularidad** y **concurrencia CSP**; luego el
> gran arco de **librerías aplicadas** M15–M26: red/cloud/cripto/compresión/observabilidad —DNS(7 tipos+
> caché)/HTTP(S)/WebSocket(ws+wss)/TLS/Redis/UDP/OAuth2/protobuf/HTTP2-framing+HPACK, más logging+métricas—,
> todo como librería en el propio lenguaje). La columna *Cuándo* refleja la hoja de ruta (ver
> [DESIGN.md](DESIGN.md) §2 y el **plan post-M26 en §36**). **Siguiente arco (M27–M32)**: volver a la
> **ergonomía del lenguaje** (tuplas, `for`/iteradores, interpolación… lo que destaparon las librerías) →
> **tooling** (regex, formateador, optimización VM) → **más librerías** (cripto avanzada, cerrar gRPC,
> PostgreSQL). Detalle y orden razonado en §36.

| Idea | ¿Dónde pega? | Cuándo | Estado |
|------|--------------|--------|--------|
| Concurrencia (goroutines / async / suspend) | **Arquitectura de la VM** | **M12** | ✅ **COMPLETO** (DESIGN §21): **CSP sobre la VM** — green threads cooperativos M:1, canales tipados, structured concurrency; data-race freedom **vía CSP** (no ownership); scheduler determinista; intérprete = oráculo secuencial. Surface: `spawn(closure)->Task<T>`, `channel()`/`channel(n)`/`send`/`recv->Option<T>`/`close`, `join`, `scope`, `select` (builtins). Sub-fases: ✅ **M12.1** slice CSP (spawn + canales no acotados + scheduler determinista; solo VM, intérprete da error limpio; `close` ad-hoc polimórfico con el de handles; GC multi-raíz) · ✅ **M12.2** acotados/backpressure (`channel(n)`, `n≥0`; `n=0` rendezvous; `send` se vuelve punto de yield al llenarse la cola; `recv` despierta al emisor bloqueado; `VmChannel.cap`; `Waiting::Recv`/`Send(v)`; el valor del emisor aparcado es raíz del GC) · ✅ **M12.3** structured concurrency (`Task<T>`+`join(t)->T`+`scope(fn()->R)->R`; `spawn` pasa a devolver `Task<T>`; el scope posee las tareas lanzadas dentro y las une al salir; propagación del fallo de una hija vía captura en la `Task` y re-lanzado en `join`/`ScopeEnd`; estado por fibra `task`/`scopes`; GC multi-raíz; diferido: cancelación de hermanas) · ✅ **M12.4** `select(chs: [Channel<T>]) -> int` (bloquea hasta que un canal esté listo para recibir; devuelve el índice del primero listo, determinista; `recv(chs[i])` toma el valor; `Waiting::Select`, `wake_select_waiters`; solo VM) · ✅ **M12.5** cancelación de hermanas (semántica, sin superficie: al fallar una tarea del `scope`, se cancelan las hermanas pendientes —`cancel_task` recursivo: las saca de ready/parked y cancela nietos— y se propaga el fallo original; `ScopeEnd` cancela en vez de esperar; `fail_current_fiber` cancela los hijos de una fibra-hija que falla; cooperativa, no preemptiva). **M12 COMPLETO** (diferido: cancelación preemptiva, `Selected<T>` índice+valor, select de send, `cancel(t)` explícito). Diferido: algebraic effects (intérprete a pila explícita), M:N paralelo (GC thread-safe). Descartado: ownership/regiones |
| **raylang de producción** (cambio de norte) | Todo el runtime | **rama `feature/improvements`** | 🚧 **PLAN FIJADO** — análisis a fondo + plan M33–M43 en **[PRODUCCION.md](PRODUCCION.md)** (DESIGN §37). Arcos: A estabilidad (spans/no-ICE/SPEC/un motor) · B rendimiento+M:N por actores · C ecosistema (`ray`+paquetes+std/+FFI) · D endurecimiento+1.0. Fuera de 1.0: JIT/nativo, macros, effects, reflection (siguen aquí) |
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
| **API de runtime / I/O** (`args`, `input`, `env`) | Builtins / stdlib | **M11** | ✅ `args`/`input`/`read_int`/`env`/`eprint` + I/O de archivos (`read_file`/`write_file`/`exists`/`append_file`/handles con buffering). `main` sin parámetros |
| **stdlib** (orden superior / string / I/O / arreglos) | prelude + builtins | **M7/M11** | ✅ `map`/`filter`/`fold` (M7.3) + string completa (M11.1/4/7a) + arreglos (`+`/`reverse`/`pop`/`contains`/`position`, M11.7b) + `sort`+`Ord` (M11.7d). Registro único de builtins (L1) |
| **stdlib importable** (math/tiempo/cripto → módulos `std/…`) | Contenido: builtins → `std/*.ray` (cero maquinaria) | **M49** | 📌 **PLAN FIJADO** ([docs/M49-stdlib-importable.md](docs/M49-stdlib-importable.md)). Continúa M48 (descongestionar el namespace de **valores**): saca las familias matemática/tiempo/cripto del global a módulos importables (`import std/math; math.sqrt(x)`), dejando globales solo lo universal (`print`/`panic`/`assert`) y **core la concurrencia** (atada al modelo de ejecución). **Cero maquinaria nueva**: reusa la std **embebida** (M40.5, `src/stdlib.rs`+`include_str!`) + el patrón `__x`+envoltorio (como la I/O). **Empieza por `std/math`** (mayor liberación de nombres —`min`/`max`/`abs`/`round`— + ya está a medias). Decisiones recomendadas (a confirmar): `min`/`max` genéricos sobre `Ord` y `abs` sobre un trait nuevo `Signed` → **puros en raylang, sin opcode** (poda `Abs`/`Min`/`Max`); `pi`/`e` → `const PI`/`const E` (poda `Pi`/`E`); `random`/`random_int` → `std/random` (no deterministas, aparte del `math` puro); corte en seco con el **reescritor AST** de M48.4e (+ auto-`import`). Sub-fases: 49.1 `std/math` (a: float; b: abs/min/max+consts) · 49.2 `std/time`+`std/random` · 49.3 `std/crypto`. Verificación: oráculo (deterministas) + subproceso (RNG/tiempo). **Restricción hoy**: no bloquea nada (el embedding + wrappers ya existen; la migración es mecánica) |
| **stdlib importable II** (fs/collections/net → `std/…`) | Contenido: prelude/builtins → `std/*.ray` (molde M49) | **M50** | 📌 **PLAN FIJADO** ([docs/M50-stdlib-fs-collections-net.md](docs/M50-stdlib-fs-collections-net.md)). Cierra la descongestión del namespace de valores (tras M48/M49): saca del prelude global los 3 grupos grandes que quedan → **`std/fs`** (read_file/write_file/…/open/exists; disco = opt-in, *capability hint*), **`std/collections`** (Set/Deque/StringBuilder, puras), **`std/net`** (tcp/tls/socket/udp). Se quedan globales los esenciales (`Option`/`Result`/`?`, map/filter/fold, print/eprint/panic/assert) + `close` (ad-hoc); stdin/`env` a decisión aparte. Mecanismo M49 (`__x`+envoltorio, migración dirigida por errores). Alcance tratable: collections ~2 archivos, net ~15 (ninguno embebido usa red), fs moderado. Collections en **submódulos** `std/collections/set`·`deque`·`stringbuilder` (leaf-binding M11.5: `import std/collections/set; set.new()` — agrupa Y sin prefijo redundante; sin maquinaria nueva). Verificación: oráculo (collections) + subproceso (fs/net). Sub-fases 50.1 fs · 50.2 collections · 50.3 net |
| **Identificadores en inglés** (deuda: nombres mezclados es/en) | Rename transversal (Rust `src/` + core raylang) | **diferida (tras M49)** | 📌 **REGLA FIJADA + PLAN** ([docs/limpieza-nombres-en-ingles.md](docs/limpieza-nombres-en-ingles.md), regla en CLAUDE.md § Convenciones). Los **identificadores** (funciones/métodos/variables/params/tipos/campos) deben ir en **inglés**; comentarios/`///` en español. El código antiguo mezcla ambos (`cargar`/`analizar`/`nombre_fachada`/`receptor`/`otro`…). Tres tiers por riesgo: **A** Rust `src/` interno (~66+ fns + vars, NO rompe) · **B** core raylang interno (selfhost/prelude/std vars privadas, NO rompe; `std/` ya casi todo inglés) · **C** ⚠️ **INCOMPATIBLE**: los métodos de trait user-facing `Eq.igual`/`Show.mostrar`/`Ord.menor` → inglés (toca cada `impl`+llamada del corpus + `@derive` + reescritor AST + self-hosted + docs; fase aparte, la última). Código **nuevo ya en inglés**. Se hace **tras cerrar los puntos pendientes** (M49.2/49.3). Verificación: suite completa (A/B) + oráculo/self-hosting byte-idéntico (C) |
| **Optimización de la VM** | `bytecode`/`compiler`/`vm` | transversal **(activo)** | 🚧 DESIGN §27, registro medido en §11. Foco tras **aparcar M18** (backend nativo) por decisión del usuario. Principio: **incremental y midiendo** — banco `benchmarks/` (`bench.sh`+hyperfine o `measure.py` sin deps) y se conserva solo lo que supera el ruido (~3–5 %), oráculo VM↔intérprete intacto. Opt.1/Opt.2 ✅ (pase previo); Opt.3 `Rc<str>` ❌. ✅ **Opt.4** fast-path entero en ops binarias (fib −5 %, bucle −6 %); ✅ **Opt.7** posición `(línea,col)` perezosa con `pos!()` (quita la lectura de `lines[ip]` por instrucción del camino caliente → **fib −7 %, loop −9 %, arrays −8 %**, consistente; señal destapada con mejor-de-15); Opt.5 (`new_locals`)/Opt.6 (safepoint GC)/Opt.8 (`children()` con buffer reusado, dentro del ruido incluso con `gcnested.ray`)/LTO ❌ descartados. Pendiente: dedup constantes, peephole/plegado, `HeapValue` 32→16 B |
| **Backend nativo** (bootstrap sin Rust) | codegen a máquina/asm/C/Rust | **M18** | 💤 **aparcado** (decisión del usuario, 2026-06): no perseguir lo nativo/sin-toolchain por ahora; el esfuerzo va a la optimización de la VM. Opciones barajadas: asm (as+ld), máquina directa, C, transpilar a Rust→rustc. Se retoma más adelante |
| **Asperezas de M3** | Parser + checker | hecho | ✅ `[]` en campo de struct (M6.2) y coma final en arreglos (limpieza) resueltos |
| **Ergonomía del lenguaje I** (tuplas · `for`/iteradores · interpolación · casts · `const`) | Lexer + parser + checker + ambos motores + self-hosting | **M27** | 🚧 DESIGN §36. La deuda ergonómica que destaparon las librerías M15–M26. ✅ **M27.1** tuplas (`Type::Tuple`, `t.0`, `let (a,b)=…`; erasure a arreglos) · ✅ **M27.2** `for`/iteradores (rango `a..b`, arreglo, string→char, `Map`→tupla `(k,v)`; `StmtKind::For` ejecutado directamente en ambos motores); **M27.3** interpolación `"…{expr}…"` (desugar a `+ to_string`, puro léxico); **M27.4** casts `x as int`/`as float` (reusa `as`); **M27.5** `const` de nivel superior |
| **Ergonomía del lenguaje II** (operadores · `?`+From/Into · enteros con tamaño) | Sistema de tipos / traits + modelo numérico | **M28** | 🚧 DESIGN §36. **M28.1** sobrecarga de operadores vía traits (`Add`/`Ord`/`PartialEq`…; hoy *special-cased*; puede unificar `@derive(Eq)`); **M28.2** `?` con conversión de error (`From`/`Into` → enums de error propios en vez de `string`); **M28.3** enteros con tamaño/unsigned (`u8`/`u32`/`u64`; el más invasivo; mata el `& 0xFFFFFFFF` de la cripto; puede quedar acotado sin promoción implícita) |
| **Tooling** (regex · formateador · optimización VM) | Motor propio / cliente externo (reusa parser) / VM | **M29** | 🚧 DESIGN §36. **M29.1** regex (ausencia más llamativa de la stdlib; motor Thompson NFA, librería raylang o builtin-asistido); **M29.2** formateador `rayfmt` (pretty-printer canónico del AST, idempotente, sin config); **M29.3** retomar optimización VM (§27: dedup constantes, peephole, `HeapValue` 32→16 B) |
| **Cripto avanzada** (cifrado + firma asimétrica) | Librería raylang (cómputo) | **M30** | 🚧 DESIGN §36. Hoy hay hashing/HMAC pero **no cifrado**. **M30.1** simétrica ChaCha20-Poly1305/AES-GCM (vectores RFC 8439); **M30.2** asimétrica Ed25519 (RFC 8032; ejercita bignum/`u64`); **M30.3** JWT RS256/ES256 sobre lo anterior |
| **Cerrar gRPC** (transporte HTTP/2 vivo) | Librería raylang sobre TLS+ALPN | **M31** | 🚧 DESIGN §36. Los diferidos grandes de M26. **M31.1** HPACK-Huffman (tabla 257 del RFC 7541 Ap. B; vectores C.4/C.6); **M31.2** transporte vivo (preface + SETTINGS + streams sobre TLS con ALPN `h2` — requiere exponer ALPN en `tls_connect`); **M31.3** cliente gRPC e2e |
| **Clientes y formatos** (PostgreSQL · TOML/CSV · plantillas) | Librería raylang | **M32** | 🚧 DESIGN §36. **M32.1** cliente PostgreSQL (protocolo wire + SCRAM-SHA-256, reusa M20); **M32.2** TOML/YAML/CSV; **M32.3** motor de plantillas HTML sobre M27 |

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
  - Diferido: hover/def de **métodos** (comparten `(línea,col)` con el receptor, sin spans).

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

## Cómo usar este archivo

- Cuando una idea madure y se comprometa, se **mueve** a [DESIGN.md](DESIGN.md)
  (hoja de ruta §2 o norte de diseño §10) con su hito.
- Cuando aparezca una idea nueva, se **agrega aquí** con su clasificación de
  impacto, no directamente al diseño.
- Antes de cada hito grande (sobre todo **M2**), revisar este archivo: puede que
  alguna decisión "tardía" deba adelantarse por una restricción de arquitectura.
