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
| **Ecosistema de paquetes** (registro central + política de tiers) | CLI (`ray add`/`publish`) + índice git + gobernanza de `std/` vs paquetes | **M51** | 📌 **DISEÑO FIJADO** (DESIGN §53 política de tiers, §54 registro). Dos piezas: (a) **política de tiers** (gobernanza, ya explícita): `std/` embebida (universal/ligera/estable) vs paquetes `packages/*` (nicho/pesado/API propia) vs `examples/` (demos); criterios de colocación + pipeline de promoción `examples/→std|paquete`. (b) **Registro central** = cierra la brecha nº1 de PRODUCCION.md ("flexible en el lenguaje, ❌ en el ecosistema"): **índice respaldado por git** (repo `nombre → git URL + versiones + hash`, sin servidor propio, reusa toda la maquinaria de M39c: cápsula/lock/transitivas/MVS), `ray.toml` **por nombre** (`foo = "1.2.0"`), `ray add`/`ray publish`/`ray yank`. Prereq: **rangos semver de verdad** (diferido de M39c). Fases: ✅ **51a** leer índice+`ray add`+rangos semver (`src/index.rs`: `VersionReq` exacta/caret/tilde/`*`, lector `<index>/<name>.toml`, `resolve`/`latest`; `deps::ensure` resuelve por nombre vía índice y delega en git+lock; `ray add` con `manifest::upsert_dependency`; índice por `RAY_INDEX`/`[registry] index`; tests offline `tests/registry_cli.rs`) · ✅ **51b** `ray publish` (valida name+version+parseo · `deps::hash_package` · `index::append_version` inmutable · spec git de `--repo` o derivada de `origin`+tag `v<ver>`; tests offline con bare repo) · ✅ **51c** índice remoto por git (clonado/cacheado en `.ray-deps/.index`) + lock-pinning (reproducibilidad de caret) + `ray update` (re-resuelve + `git pull`) + `ray yank`/`--undo`. **M51 COMPLETO** (tests offline en `tests/registry_cli.rs`, 11 casos; cero runtime). Diferido: UI/búsqueda web, firmas de publicación, mirrors, namespaces con dueño |
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
| **Webserver de producción** (límites/timeouts · query · TLS · keep-alive · cookies · estáticos) | Librería `packages/net/webserver.ray` (casi todo) + 2 toques de runtime aditivos (deadline de E/S en el scheduler, `try_join`) | **M56** | ✅ **COMPLETO** (DESIGN §60, detalle en §17 abajo): 56.1 frontera de seguridad (Limits: cabeceras/cuerpo/conexiones vía semáforo `Channel.bounded`) · 56.2 query string separada + percent-decoding (`std/url.percent_decode`) · 56.3 `serve_tls` · 56.4 timeouts de lectura (`net.set_read_timeout`, deadline en `io_parked`) · 56.5 `try_join` + panic del handler→500 sin fugas · 56.6 keep-alive HTTP/1.1 (el framework se sube gratis) · 56.7 `set_cookie: [string]`+`with_cookie` (decisión con el usuario) · 56.8 chunked entrante + `static_response` con saneo + HEAD sin cuerpo. Diferidos menores en DESIGN §60 |

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
**índice** y **publicar** (`ray publish`). Es la brecha que `PRODUCCION.md` (Parte I §2) marca como
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
  compile vs. filesystem), filtros, `s[i]` O(1) en la VM (cachear los chars del string — optimización
  del runtime que beneficiaría a todo el ecosistema, no solo templates).

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
- **Menores**: sin pretty-print (solo compacto); sin helpers de acceso (navegar es a `match` puro).
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

**Diferidos** (a demanda): keep-alive/pool del cliente HTTP (la delimitación por Content-Length lo
habilita), multiplexado h2 real, fragmentación WS de ENVÍO (la de recepción entra en 58.1).

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
es error de sintaxis) — anotar como idea aparte si algún consumidor lo pide.

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
| **DX de assert**: un `assert` fallido reporta la posición DEL PRELUDE (579:9), no el sitio del usuario — con varios asserts no sabes cuál falló | Inherente a "prelude = funciones ordinarias"; el fix real es posición-del-llamador o mini stack trace (toca runtime) | **idea aparte** (diseño de runtime; no entra en M61) |
| Menores: `sum` solo `Iter<int>` (sin float); `Ord` para `bool`/`bytes` ausente | — | dentro de 61.3 o diferido |

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
| **El `next()` cuesta ~6 µs/elemento** (medido: `for x in xs.iter()` pelado = 6 191 ms/1M): llamada a closure + ALOCACIÓN de un `Option` en el heap del GC + match por paso; cada adaptador apila otro tanto | Techo estructural del throughput lazy | **idea aparte** (runtime: `Option` sin alocación o devirtualizar `step`; cuando importe de verdad) |
| Faltan terminales comunes: `any`/`all`/`count` (3 líneas c/u sobre `next`/`fold`); `find`/`chain`/`min`/`max` a demanda | Ergonomía menor | opcional en 62.1 o diferido |

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

## Cómo usar este archivo

- Cuando una idea madure y se comprometa, se **mueve** a [DESIGN.md](DESIGN.md)
  (hoja de ruta §2 o norte de diseño §10) con su hito.
- Cuando aparezca una idea nueva, se **agrega aquí** con su clasificación de
  impacto, no directamente al diseño.
- Antes de cada hito grande (sobre todo **M2**), revisar este archivo: puede que
  alguna decisión "tardía" deba adelantarse por una restricción de arquitectura.
