# raylang en Windows — el contrato y las deudas

> Estado a 4 de septiembre de 2026 (v1.5.2 + M172–M175). Este documento es el inventario **vivo** de lo que
> raylang hace y no hace en Windows: qué funciona, qué degrada con un `Err` honesto, qué falla
> de forma opaca, la API de Windows que cierra cada hueco, su tamaño y el orden de ataque.
> Lo alimentan dos fuentes: la auditoría del código (todo `cfg(unix)` y su cadena de llamadas)
> y el **censo** de los ejemplos en el runner de Windows (`.github/workflows/windows-census.yml`,
> `tools/windows_census.py`). Cuando un hueco se cierra, se tacha aquí y se actualiza la tabla
> de `PRODUCTION.md` §Windows.

## 1. En una frase

**La toolchain, el compilador, la VM, la red con su poller, los paquetes, las señales, `ray dev`, el
terminal, stdin y los procesos hijos funcionan en Windows; lo que falta son los arcos de escritorio
y audio** (y el reactor IOCP de las fibras del binario nativo). La VM es uniformemente honesta (cada
hueco es un `Err` con mensaje) y el transpilador rechaza antes de generar lo que no compila (W2).

## 2. Qué se verifica hoy en CI (`build · smoke (windows)`)

Desde M165/M166, cada PR corre en `windows-latest`:

- Build de `ray.exe` con todos los subsistemas (TLS/ring, SQLite, mimalloc, regex, FFI).
- La VM ejecuta `fib.ray` y `ray fmt` formatea.
- 458 tests del front-end y la VM (lexer, parser, checker, compilador, VM, fmt), incluido el
  oráculo FFI (`"c"`/`"m"` → `ucrtbase.dll`).
- Un proyecto con dependencias del índice bajo `core.autocrlf=true`: `ray new` + `ray add net` +
  `ray build` dos veces (resolución + verificación del lock).
- El instalador real: `install.ps1` contra la última release, `ray version`, `ray run` y
  `ray upgrade --check`.
- `signals()` (M168), la red sin poller (M170) y `ray dev` (M172: drenado por `CTRL_BREAK`, Job
  Object y socket-activation — `tests/dev_cli.rs` + los unitarios de `dev_host`).
- `std/term` y `std/io` (M173): las suites `term_cli` e `io_cli` enteras, con un test bajo una consola
  REAL nueva (`cmd /c start /wait /min`): `is_tty`, `size`, `raw` y un `read_timeout` que vence; VM y nativo.
- El poller (M174): la readiness de un listener en `poll::tests` y `a_socket_read_timeout_expires`.
- `std/process` (M175): `process_windows_cli` — el contrato de `run` con `cmd` y la sesión con stdin
  abierto, en VM, intérprete y nativo; el gate de W2 pasa a comprobar `std/ui`.
- `release.yml` instala el zip recién subido antes de dar la release por buena.

Verificado además por un usuario real (2 sep 2026): instalación, `ray run` con `web`/`net`/`db`,
SQLite (`data/store.db` con 24 productos) y el arranque del servidor HTTP.

## 3. Las deudas, por subsistema

Leyenda de **hoy**: `Err` = falla con mensaje de plataforma; `silencioso` = degrada sin avisar;
`no compila` = el binario nativo no se puede construir. **Tamaño**: S (horas), M (días), L (arco).

### 3.1 Señales — `signals()` · ✅ **cerrada en M168** (DESIGN §160)

| | |
|---|---|
| Hoy (VM) | `Err("signals() is not supported on this platform")` al crear el handle (`src/builtins.rs`, `signals_install`). |
| Hoy (nativo) | **No compila**: el runtime emitido declara `pipe`/`signal` sin `cfg(unix)` (`src/transpile/runtime.rs`, bloque `needs_signals`). |
| Superficie que arrastra | `webserver.serve_graceful` y `serve_with_graceful` (`packages/net`), `web.listen_graceful` (`packages/web`), `select` sobre `signals()` para SIGWINCH en TUIs. **Es el fallo de la app `store`**: el punto de entrada recomendado para producción no arranca en Windows. |
| Cierra con | `SetConsoleCtrlHandler` (CTRL_C → 2, CTRL_CLOSE/CTRL_SHUTDOWN → 15) escribiendo en el mismo self-pipe que consume el scheduler; SIGWINCH ≈ `WINDOW_BUFFER_SIZE_EVENT` de `ReadConsoleInput`. |
| Tamaño | **M** en la VM (el self-pipe ya existe; cambia el productor) + **S** para gatear/portar la emisión nativa. |
| **Hecho (M168)** | Exactamente eso: `SetConsoleCtrlHandler` encola y levanta la bandera `PENDING`; `install()` devuelve `-1` (sin fd) y `io_wait` duerme a cuantos de 10 ms cuando solo espera señales; el handler retiene su hilo 4 s ante el cierre. El nativo emite variante unix y Windows, y `ray build --native` apaga las fibras también por host. Sin SIGWINCH (W4). |

### 3.2 `ray dev` y `ray test --watch` · ✅ **cerrada en M172** salvo el watcher (DESIGN §164)

| | |
|---|---|
| Reinicio | ~~Sin SIGTERM: `terminate_gracefully` es `cfg(unix)`; en Windows es `TerminateProcess` directo → un servidor con `serve_graceful` no drena; las peticiones en vuelo se pierden en cada guardado.~~ |
| Huérfanos | ~~`install_cleanup_on_death` es `cfg(unix)` → matar `ray dev` por pid deja al hijo vivo reteniendo el puerto.~~ |
| Socket-activation | ~~`--port`/`--listen` se ignora con aviso (`dup2` + `pre_exec` + `RAY_LISTEN_FD` son unix) → cada reinicio re-bindea: ventana de "connection refused" y carreras `WSAEADDRINUSE`.~~ |
| Watcher | Cae a polling de mtimes (~200 ms): el crate `notify` sí soporta `ReadDirectoryChangesW`, pero el puente self-pipe de `ray_runtime::watch` es unix. **Sigue abierto** (W5: depende de 3.6). |
| Cierra con | `CREATE_NEW_PROCESS_GROUP` + `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT)` para el reinicio con drenado; Job Objects (`AssignProcessToJobObject` + kill-on-close) para huérfanos; handle heredable (o `WSADuplicateSocket`) para pasar el listener; puente por evento/IOCP para el watcher. |
| Tamaño | reinicio **S–M** · huérfanos **S** · socket-activation **M** · watcher **M** (depende de 3.6). |
| **Hecho (M172)** | La capa de SO del supervisor vive en `src/dev_host.rs` con las dos variantes. El hijo se lanza con `CREATE_NEW_PROCESS_GROUP` y el reinicio le manda `CTRL_BREAK` (el handler de M168 lo entrega como `2`; `serve_graceful` drena; 3 s y escala a `TerminateProcess`). Un Job Object con `KILL_ON_JOB_CLOSE` arrastra al hijo si el supervisor muere de cualquier forma (Ctrl-C, cierre de la ventana, kill por pid), y el handler de consola del supervisor reenvía `CTRL_BREAK` antes de salir para que además drene. `--port`/`--listen`: el listener se marca heredable (`SetHandleInformation`), su valor viaja en `RAY_LISTEN_FD` y el hijo lo adopta con `from_raw_socket` (validado con `local_addr`; si no sirve, `bind` normal), quitándole la herencia para sus propios hijos. `tests/dev_cli.rs` corre en las tres plataformas: drenado en el reinicio, socket retenido entre reinicios y (Windows) el hijo muere con el supervisor. |

### 3.3 Terminal — `std/term` · ✅ **cerrada en M173** salvo `size_px` (DESIGN §165)

| | |
|---|---|
| Hoy (VM) | ~~`is_tty` → **`false` siempre** (silencioso: apaga colores, y `read_hidden` falla con "stdin is not a terminal" en una consola real); `size`/`size_px`/`cell_px` → `None`; `raw`/`read_key`/`capabilities` y los gráficos kitty → `Err("raw mode is not supported on this platform")`.~~ Lo puro (`width`, `char_width`, `fit`) funciona. |
| Hoy (nativo) | Paridad exacta con la VM (los brazos `windows` existen). |
| Superficie | Toda TUI; `read_hidden` (passphrases); detección de color. |
| Cierra con | `GetConsoleMode`/`SetConsoleMode` con `ENABLE_VIRTUAL_TERMINAL_INPUT` + `ENABLE_VIRTUAL_TERMINAL_PROCESSING` (raw = quitar `LINE_INPUT`/`ECHO_INPUT`/`PROCESSED_INPUT`); `GetConsoleScreenBufferInfo` para el tamaño; `GetFileType`+`GetConsoleMode` para `is_tty`. Windows 10 1511+ entiende las secuencias ANSI que `std/term` ya emite. |
| Tamaño | **M** (isatty + size + raw es S; `read_key` depende de 3.4). |
| **Hecho (M173)** | Exactamente eso, en `src/builtins.rs` (`term_host` Windows) y en el runtime nativo (`RT_WIN_TERM`): `is_tty` por `IsTerminal` (consola y pty de MSYS), `size` = la VENTANA de `GetConsoleScreenBufferInfo`, `raw` = stdin sin LINE/ECHO/PROCESSED_INPUT + VT input (las flechas llegan como ESC-secuencias, Ctrl-C como 0x03) y stdout con VT processing + sin auto-CR (`\n` como con OPOST apagado); modos restaurados en `raw_off` y por `atexit`; `attrs_fingerprint` = los dos modos (así `ray dev` reconoce una TUI). VT processing se activa además en la primera consulta de `is_tty`/`size` (conhost no lo trae; Windows Terminal sí). **Queda**: `size_px`/`cell_px` → `None` (la Console API no expone píxeles) y SIGWINCH (W7 o nunca). |

### 3.4 stdin — `std/io` · ✅ **cerrada en M173** (DESIGN §165)

| | |
|---|---|
| Hoy | ~~`stdin_ready` → **`true` siempre** (miente); `io.read` bloquea el **hilo entero de la VM** (todas las fibras), no la fibra; `io.read_timeout` **ignora el plazo**. Nativo: misma degradación, explícita (`let _ = ms`).~~ |
| Superficie | `read_key` (el ESC de 25 ms decodifica mal las flechas), REPLs, servidores dirigidos por stdin, `ray mcp` y `ray lsp` sobre stdio. |
| Cierra con | `WaitForSingleObject`/`GetNumberOfConsoleInputEvents` (consola), `PeekNamedPipe` (pipe), lecturas overlapped (archivo). |
| Tamaño | **M**. |
| **Hecho (M173)** | `stdin_host` Windows: disponibilidad real por tipo de stdin — consola: `PeekConsoleInputW` busca una tecla pulsada con carácter (y en modo línea, un Enter: hasta entonces `ReadConsole` no entrega nada); pipe: `PeekNamedPipe` (octetos o extremo cerrado); archivo/NUL: siempre. Sin `WaitForSingleObject` (el handle de consola se señala por key-up, ratón y foco que nunca se leen): la espera con plazo sondea a 5 ms. Lectura CRUDA (`ReadConsoleW` → UTF-8 con resto para no partir un carácter; `ReadFile` para pipes), sin el `BufReader` de std. Y en el scheduler, la fibra aparcada en stdin deja de despertarse a ciegas en el respaldo sin poller (cada reintento renovaba su plazo y `read_timeout` no vencía nunca): se despierta solo con datos, y su deadline expira. Pendiente relacionado (W5): los `read_timeout` de SOCKETS siguen renovándose en el respaldo sin poller. |

### 3.5 Procesos — `std/process` · ✅ **cerrada en M175** (DESIGN §167)

| | |
|---|---|
| Hoy (VM) | `Err("<program>: running OS processes is not supported on this platform")` en `run`/`cmd`/`stream`/`stdin_pipe`. |
| Hoy (nativo) | **No compila**: el runtime emite llamadas a `ray_runtime::process` y el módulo entero es `cfg(unix)`. |
| Superficie | `process.run`/`cmd`, sesiones persistentes (MCP/LSP hijos), pipelines `sh -c`, drivers de build. |
| Cierra con | Lo portable ya lo es (`std::process::Command`, pipes, env, cwd). Lo unix: `process_group(0)` → Job Objects; `poll(2)` + `O_NONBLOCK` del drenado → `PeekNamedPipe`/overlapped; `kill(-pid)` → `TerminateJobObject`; `Exit.Signal` no tiene análogo (documentar el mapeo). |
| Tamaño | **L**. |
| **Hecho (M175)** | Exactamente el mapeo de "Cierra con", en `crates/ray-runtime/src/process.rs` (variante Windows, compartida por VM y nativo): grupo = `CREATE_NEW_PROCESS_GROUP` + un Job Object por hijo con kill-on-close (los nietos de `cmd /c "a \| b"` mueren con el grupo); escalera del timeout y de `kill(force=false)` = `CTRL_BREAK` al grupo → 500 ms → `TerminateJobObject`; `run` drena los dos pipes con un hilo por flujo (los pipes anónimos no tienen modo no bloqueante) y el streaming de la VM consulta `PeekNamedPipe` antes de leer (la fibra aparca por el respaldo sin fd, M170). **Mapeo de `Exit.Signal`**: `Signal(9)` si lo terminó el job, `Signal(15)` si el peldaño suave tuvo que forzarse, `Signal(2)` si murió por el `CTRL_BREAK` (`STATUS_CONTROL_C_EXIT`); el resto, `Code(n)`. El gate de M169 deja de listar `process`. `tests/process_windows_cli.rs`: el contrato de `run` con `cmd` y la sesión con stdin abierto (con `ray` como hijo: los filtros de Windows bufferizan bajo un pipe), en VM, intérprete y nativo. Límite conocido: la escritura al stdin del hijo es bloqueante (sin aparcar la fibra). |

### 3.6 Poller de red y scheduler · ✅ **poller, sueño fino y UDP cerrados en M174** (DESIGN §166); IOCP nativo queda para W7

| | |
|---|---|
| Hoy (VM) | Sin kqueue/epoll: `raw_fd` → `None` y el scheduler cae al busy-poll de M15.5 (1 ms de sueño + re-encolar). Funciona; más CPU y peor p99 bajo carga. El handshake TLS espera con `sleep(20 ms)` en vez de `poll`. |
| Hoy (nativo) | Sin reactor: `ray build --native --target *windows*` apaga las fibras con aviso (hilo-por-tarea). ⚠️ El apagado mira solo `--target`: un build nativo **en un host Windows sin `--target`** deja las fibras encendidas e intenta compilar corosensei + kqueue/epoll → fallo de compilación. **S** de arreglar (`cfg!(windows)` cuando no hay target). |
| `sleep_ms` | `thread::sleep` en vez de `poll(NULL,0,ms)`: con el tick por defecto de 15,6 ms la precisión del pacing de juegos/audio y de `time.sleep_ms` cae. |
| **Esperas de red sin fd** ✅ M170 | Las sondas (`windows-probe.yml`) descartaron IPv6 (`tcp_connect` hacia fuera: 25 ms) y cazaron la causa real: en no-unix `raw_fd` es `None`, y `io_wait` tomaba por DURMIENTE (fd −1) toda fibra aparcada por `WouldBlock` — sin deadline, `sleep(0)` y a girar sin despertarla jamás. **Todo servidor colgaba en el primer `accept`** (el par local del censo imprimía el puerto y moría ahí; `webserver` + `http.fetch` locales igual), y de rebote `tcp_cliente`/`http_demo`. Arreglo: E/S aparcada sin fd (`handle >= 0`) → busy-poll cooperativo de 1 ms y reintento. **Segunda mitad**: las operaciones clonan el socket (`try_clone`) y en Windows el clon nace bloqueante (`WSADuplicateSocket` no hereda `FIONBIO`; en unix el fd duplicado sí comparte `O_NONBLOCK`) → el `accept` clonado bloqueaba al único worker; los clones re-aplican el modo. Test `net_no_poller_cli` en las tres plataformas. Pendiente W5: el mismo busy-poll con readiness real (wepoll). |
| **UDP y el reset 10054 (censo)** | En Windows, un ICMP "port unreachable" previo hace que el siguiente `recv` UDP falle con `WSAECONNRESET` (10054): `udp_demo`, `dns_cache_demo` y `udp_timeout_demo` lo muestran (Linux simplemente espera). Es un comportamiento documentado de Winsock; se desactiva con `WSAIoctl(SIO_UDP_CONNRESET, FALSE)` al crear el socket. **S**. `dns_demo` además **cuelga** (el `recv` UDP bloqueante que ya anotó IDEAS §70). |
| Cierra con | `wepoll` (ABI epoll sobre IOCP/AFD, encaja en la forma de `src/poll.rs`) para la VM; IOCP nativo a largo plazo, que además desbloquea las fibras; `CreateWaitableTimerEx(HIGH_RESOLUTION)` o `timeBeginPeriod(1)` para el sueño fino. |
| Tamaño | wepoll **M** · IOCP para fibras **L** · sueño fino **S**. |
| **Hecho (M174)** | `WSAPoll` (ws2_32, la forma exacta de `poll(2)`: sin crates) como backend Windows de `src/poll.rs`; `raw_fd` devuelve el SOCKET y el scheduler aparca las fibras de red en el poller de verdad — se acabó el busy-poll de 1 ms para sockets, y los `read_timeout` de sockets VENCEN (aparcadas en el poller, sus deadlines expiran; antes cada reintento los renovaba). El pseudo-fd de stdin no es un socket: el backend lo sondea aparte a 5 ms. El handshake TLS espera por el poller también en Windows. `sleep_ms` = *waitable timer* de alta resolución (Windows 10 1803+; `thread::sleep` de respaldo): fuera el tick de 15,6 ms. UDP: `SIO_UDP_CONNRESET = FALSE` al crear el socket (adiós 10054). **Queda**: el reactor IOCP para las fibras del binario nativo (W7). |

### 3.7 `fs.chmod` y `stat().mode`

`chmod` → `Err` honesto; `stat().mode` → **`0` siempre** (silencioso: `mode & 0o111` dice "no
ejecutable"). Nativo en paridad. No hay equivalente limpio: documentar; opcionalmente mapear
`u+w` a `FILE_ATTRIBUTE_READONLY` y derivar `x` de la extensión. **S**.

### 3.8 Escritorio y audio — `std/ui`, `std/audio`, `ray bundle`

| | |
|---|---|
| Hoy (VM) | `Err` honesto en cada llamada (`UI_UNAVAILABLE`/`AUDIO_UNAVAILABLE`); `ray bundle` → "no bundle format for this platform". |
| Hoy (nativo) | **No compila** (emite `ray_runtime::ui`/`audio` y los módulos son `cfg(unix)`). |
| Cierra con | WebView2 (COM) + `CreateWindowExW`, o adoptar `wry`; WASAPI para audio; `.exe` + acceso directo o MSIX para el bundle. Decisión registrada en IDEAS §80: "Windows diferido honesto". |
| Tamaño | **L** cada uno. Paso intermedio **S**: dejar alcanzable `RAY_UI_BACKEND=headless` en Windows para que los tests no sean ciegos. |

### 3.9 Menores (S cada uno)

| Dónde | Hoy | Arreglo |
|---|---|---|
| `key_path` (`ray publish --sign`) | ✅ M169: `HOME` → `USERPROFILE` | — |
| `raise_fd_limit` | no-op (`cfg(unix)`) | N/A en Windows: documentar |
| `packages/tz` | `load()` → `Err` (no hay `/usr/share/zoneinfo`); UTC funciona | tzdata embebida o registro + `windowsZones` de CLDR (**M**) |
| Ejemplos que asumen `/tmp` | `examples/io/binario.ray` escribe en `/tmp/…` → "The system cannot find the path specified" (censo) | usar el directorio temporal del sistema (`time`/`fs` no lo exponen: candidato a `fs.temp_dir()`) |
| FFI `libm` | `pow`/`sqrt` de `ucrtbase` redondean distinto: `3` donde glibc da `3.0000000000000004` (censo, `examples/ffi/libm.ray`) | no es bug: precisión de la CRT; el oráculo FFI VM↔nativo sigue valiendo (misma CRT en ambos) |
| `ray upgrade` en ARM64 | exit 69 (no hay asset) | publicar `aarch64-pc-windows-msvc` (IDEAS §84) |
| FFI | **funciona**: `_errno`, `libloading::os::windows`, `"c"`/`"m"` → `ucrtbase.dll` | — |

## 4. La asimetría VM ↔ nativo

Es el hallazgo transversal de la auditoría. La VM devuelve `Err` en todo hueco; el transpilador,
en cambio, emitía Rust **que no compila** en Windows cuando el programa usaba cualquiera de estas
cinco superficies: `signals()`, `std/process`, `fs.watch`, `std/audio`, `std/ui` (hoy solo
`std/audio` y `std/ui`: señales desde M168, procesos desde M175, `watch` se excluye solo). El usuario ve
un backtrace de `rustc`, no el mensaje del lenguaje.

El arreglo barato e independiente de cualquier port: una **comprobación pre-transpilación** —
si el target efectivo es `*-pc-windows-*` y el programa activa alguno de los flags
(`needs_rt_process`, `needs_rt_watch`, `needs_rt_audio`, `needs_rt_ui`; `signals` ya compila
desde M168), `ray build --native` falla con el mismo mensaje que daría la VM en runtime.
✅ **Hecho en M169**: `native_unsupported_on_windows` en `src/cli.rs`, exit 69 y sin binario;
el job de Windows de CI lo prueba con un programa que usa `std/process`. Matiz de M173: `watch` se
excluye SOLO en targets Windows (el transpilador emite todas las funciones de los módulos importados y
`fs.watch` vive en `std/fs`, que importa casi todo programa: el gate rechazaba programas que jamás
vigilan nada); excluido, `fs.watch` devuelve el `Err` de la VM en vez de impedir el binario.

## 5. Probablemente funciona, sin verificar

Fuera de la red de CI actual:

1. **Rutas con `\`**: los módulos usan `/` por regla del lenguaje; la frontera módulo → ruta de
   archivo (`ray run C:\proj\src\main.ray`, `Manifest::find` subiendo directorios) no tiene test.
2. **Colores ANSI en consola**: ✅ M173 — `std/term` activa `ENABLE_VIRTUAL_TERMINAL_PROCESSING` en la
   primera consulta de `is_tty`/`size` y al entrar en `raw` (Windows Terminal ya lo trae; conhost no).
   Sin verificar a ojo en conhost heredado.
3. **UTF-8 en consola**: sin `SetConsoleOutputCP(CP_UTF8)`; los mensajes con acentos pueden salir
   mal en páginas de código heredadas. **Demostrado por el propio censo**: el primer run murió con
   `UnicodeEncodeError: 'charmap' codec can't encode character '\u2192'` al imprimir una flecha
   desde Python en el runner (cp1252). `ray` escribe bytes UTF-8 crudos; con `chcp 65001` o
   Windows Terminal se ven bien, en `cmd.exe` heredado no.
4. **Liberación del puerto** entre reinicios de `ray dev` (TIME_WAIT sin `SO_REUSEADDR` equivalente).
5. **TLS** (ring/rustls con `webpki-roots`): compila, ningún handshake corre en Windows en CI.
6. **SQLite** (`rusqlite` bundled): compila; sin test en Windows (WAL, unidades de red).
7. **`ray build --native` en host Windows**: CI nunca lo ejecuta (ver 3.6 y §4).
8. **`ray mcp` / `ray lsp`** sobre stdio: 3.4 está cerrada (M173: `stdin_ready` real por `PeekNamedPipe`);
   sin prueba de extremo a extremo en Windows todavía.
9. **URIs del LSP en Windows** · ✅ M176: `path_to_uri` emite `file:///C:/Users/…` (barras hacia
   delante, `%20`) y `uri_to_path` acepta la forma de VS Code (`file:///c%3A/…`); los 5 tests de
   `lsp::tests` corren ya en el job de Windows (DESIGN §168).
10. **BOM UTF-8** · ✅ M176: un BOM inicial se ignora y no ocupa columna (SPEC §1; en medio sigue
    siendo un carácter inesperado), en el lexer de Rust y en el autoalojado.

## 6. Orden de ataque

| Fase | Qué | Tamaño | Desbloquea |
|---|---|---|---|
| **W1** | `signals()` vía `SetConsoleCtrlHandler` + gate de la emisión nativa; mientras llega, `serve_graceful` degrada a `serve` con aviso cuando no hay señales | M + S | `serve_graceful`, `web.listen_graceful` (**la app `store`**), apagado limpio de cualquier servidor |
| **W2** ✅ | Comprobación pre-transpilación (§4, M169); fibras apagadas por host (M168); `key_path` con `USERPROFILE` (M169) | S | errores honestos en el nativo; `ray build --native` en Windows |
| **W3** ✅ | `ray dev`: `CREATE_NEW_PROCESS_GROUP` + `CTRL_BREAK`, Job Objects para huérfanos, socket-activation por handle heredable (M172) | S–M | ciclo edit-run con drenado; sin puertos secuestrados |
| **W4** ✅ | `std/term` por Console API (isatty, size, raw) + `std/io` readiness (`PeekNamedPipe`/eventos de consola) (M173) | M + M | TUIs, `read_hidden`, color correcto, `read_key`, `ray mcp`/`lsp` sin bloquear la VM |
| **W5** ✅ | `WSAPoll` en `src/poll.rs` + sueño fino + `SIO_UDP_CONNRESET` (M174) | M + S | p99 de servidores bajo carga; pacing de juegos; `read_timeout` de sockets |
| **W6** ✅ | `std/process` con `CreateProcess` + pipes + Job Objects (M175) | L | MCP/LSP hijos, pipelines |
| **W7** | IOCP para fibras nativas · WebView2 · WASAPI | L × 3 | fibras en el nativo; escritorio y audio |

Al margen: Scoop (bucket propio), winget a demanda y la build `aarch64-pc-windows-msvc` (IDEAS §84).

## 7. Censo de los ejemplos en Windows

Primer censo: 2 de septiembre de 2026, `windows-census.yml` run 33706948973, sobre `main` en
`census/windows` (v1.5.1 + el fix de CRLF; **antes** de M168). 129 ejemplos con `main`, cada uno
ejecutado en Linux (referencia) y en Windows con `ray run`, stdin cerrado y plazo de 45 s. La
comparación es entre plataformas: un `main` que devuelve un entero distinto de cero a propósito
cuenta como OK si Windows devuelve el mismo.

| Estado | Ejemplos | Qué significa |
|---|---|---|
| OK | 104 | mismo código de salida y mismo stdout |
| OK-CRLF | 1 | `plantillas.ray`: idéntico salvo CRLF — lee una plantilla del repo, que el checkout con `autocrlf` convirtió |
| INTERACTIVO | 9 | servidores que exceden el plazo en AMBOS (`webserver_demo`, `framework`, `ssr`, `tcp_servidor`, `websocket_echo`…): validar a mano, no dicen nada de Windows |
| CUELGA-LINUX | 3 | `senales.ray` (espera una señal: en Linux cuelga a propósito, en Windows fallaba antes de M168), `udp_demo` y `dns_cache_demo` (Linux espera un datagrama que no llega; Windows recibe el reset 10054 y sale) |
| CODIGO-DISTINTO | 4 | ver abajo |
| CUELGA-WIN | 2 | ver abajo |
| DIFIERE | 6 | ver abajo |

**Los 12 que importan**, con su causa:

| Ejemplo | Resultado en Windows | Causa | Deuda |
|---|---|---|---|
| `stdlib/process_session.ray`, `process_stream.ray` | exit 1: "running OS processes is not supported" | `std/process` es unix | 3.5 (W6) |
| `stdlib/process_run.ray` | stdout sin las líneas de los hijos | ídem | 3.5 (W6) |
| `web/http_demo.ray` | `error reading: read timeout` | esperas de red sin fd nunca despertaban | ✅ M170 (3.6) |
| `net/tcp_cliente.ray` | cuelga (>45 s) | ídem | ✅ M170 (3.6) |
| `web/dns_demo.ray` | cuelga | `recv` UDP bloqueante + 10054 | 3.6 (S) |
| `web/udp_timeout_demo.ray` | exit 0 vs 1: `recv err: 10054` donde Linux da `send err: EINVAL` | semántica UDP de Winsock | 3.6 (S) |
| `io/binario.ray` | faltan "escritos 9 octetos" y "round-trip OK": `/tmp` no existe | el ejemplo asume `/tmp` | 3.9 |
| `ffi/libm.ray` | `3` vs `3.0000000000000004` | precisión de `ucrtbase` vs glibc | no es bug |
| `io/reloj_aleatorio.ray` | dados y `random` distintos | aleatorio por diseño | — |
| `concurrency/select.ray` | `200` en otra línea | orden de llegada bajo multicore | — (`--deterministic` lo fija) |
| `web/desktop_window/main.ray` | mismo error de `ui` sin el "listening on port N" | Linux imprime el puerto antes de fallar; Windows falla antes | 3.8 |

Lectura: **descontando lo esperado (procesos, señales pre-M168, aleatoriedad, puertos), el único
hueco que el censo descubrió y la auditoría de código no tenía es el de las esperas de red** — las
sondas lo redujeron a un bug del scheduler (M170), no del transporte: ningún servidor podía
aceptar una conexión en Windows. El censo no lo vio directamente porque los servidores son
INTERACTIVO (exceden el plazo en ambas plataformas); lo delató el cliente. El censo se relanza
con `gh workflow run windows-census.yml` o empujando a una rama `census/**`; al cerrar una deuda,
esta tabla se actualiza con el run que lo demuestre.

## 8. Mientras tanto: escribir raylang portable hoy

- **Servidores**: `serve_graceful` no arranca en Windows hasta W1. `signals()` devuelve un
  `Channel<int>` y en Windows es un **error de runtime**, no un `Err`: se captura con `try_call`
  (M97), que convierte el fallo en valor. Portable hoy:
  `match (try_call(fn() -> Channel<int> { signals() })) { Result.Ok(s) => webserver.serve_shutdown(host, port, s, drain_ms, h), Result.Err(_) => webserver.serve(host, port, h) }`
  — el mismo binario drena en unix y sirve sin drenado en Windows. W1 mete exactamente esta
  degradación dentro de `serve_graceful`, con aviso en stderr, para que nadie tenga que escribirla.
- **Terminal**: consultar `term.is_tty` y `term.capabilities()` antes de dibujar, y tener camino
  sin color/sin raw; hoy en Windows ambos dicen "no".
- **Procesos**: tratar `process.run` como opcional (`Result`) y no como infraestructura del programa.
- **Rutas**: separar siempre con `/` (Windows lo acepta en todas las APIs de archivo); nunca
  concatenar `\` a mano.
- **Nativo**: `ray build --native` en Windows compila todo lo que la VM soporta ahí, señales
  incluidas; procesos, watch, ui y audio se rechazan con mensaje antes de generar nada (W2).
