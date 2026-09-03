# raylang en Windows — el contrato y las deudas

> Estado a 2 de septiembre de 2026 (v1.5.1). Este documento es el inventario **vivo** de lo que
> raylang hace y no hace en Windows: qué funciona, qué degrada con un `Err` honesto, qué falla
> de forma opaca, la API de Windows que cierra cada hueco, su tamaño y el orden de ataque.
> Lo alimentan dos fuentes: la auditoría del código (todo `cfg(unix)` y su cadena de llamadas)
> y el **censo** de los ejemplos en el runner de Windows (`.github/workflows/windows-census.yml`,
> `tools/windows_census.py`). Cuando un hueco se cierra, se tacha aquí y se actualiza la tabla
> de `PRODUCTION.md` §Windows.

## 1. En una frase

**La toolchain, el compilador, la VM, la red y los paquetes funcionan en Windows; lo que falta es
la capa de sistema operativo** — procesos hijos, señales, terminal en modo crudo, el poller de
red eficiente, y los arcos de escritorio y audio. La VM es uniformemente honesta (cada hueco es
un `Err` con mensaje); el transpilador nativo no siempre: para cinco superficies emite Rust que
no compila en Windows.

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
- `release.yml` instala el zip recién subido antes de dar la release por buena.

Verificado además por un usuario real (2 sep 2026): instalación, `ray run` con `web`/`net`/`db`,
SQLite (`data/store.db` con 24 productos) y el arranque del servidor HTTP.

## 3. Las deudas, por subsistema

Leyenda de **hoy**: `Err` = falla con mensaje de plataforma; `silencioso` = degrada sin avisar;
`no compila` = el binario nativo no se puede construir. **Tamaño**: S (horas), M (días), L (arco).

### 3.1 Señales — `signals()` · **la primera en cerrar**

| | |
|---|---|
| Hoy (VM) | `Err("signals() is not supported on this platform")` al crear el handle (`src/builtins.rs`, `signals_install`). |
| Hoy (nativo) | **No compila**: el runtime emitido declara `pipe`/`signal` sin `cfg(unix)` (`src/transpile/runtime.rs`, bloque `needs_signals`). |
| Superficie que arrastra | `webserver.serve_graceful` y `serve_with_graceful` (`packages/net`), `web.listen_graceful` (`packages/web`), `select` sobre `signals()` para SIGWINCH en TUIs. **Es el fallo de la app `store`**: el punto de entrada recomendado para producción no arranca en Windows. |
| Cierra con | `SetConsoleCtrlHandler` (CTRL_C → 2, CTRL_CLOSE/CTRL_SHUTDOWN → 15) escribiendo en el mismo self-pipe que consume el scheduler; SIGWINCH ≈ `WINDOW_BUFFER_SIZE_EVENT` de `ReadConsoleInput`. |
| Tamaño | **M** en la VM (el self-pipe ya existe; cambia el productor) + **S** para gatear/portar la emisión nativa. |

### 3.2 `ray dev` y `ray test --watch` — degradados, documentados

| | |
|---|---|
| Reinicio | Sin SIGTERM: `terminate_gracefully` es `cfg(unix)`; en Windows es `TerminateProcess` directo → un servidor con `serve_graceful` no drena; las peticiones en vuelo se pierden en cada guardado. |
| Huérfanos | `install_cleanup_on_death` es `cfg(unix)` → matar `ray dev` por pid deja al hijo vivo reteniendo el puerto. |
| Socket-activation | `--port`/`--listen` se ignora con aviso (`dup2` + `pre_exec` + `RAY_LISTEN_FD` son unix) → cada reinicio re-bindea: ventana de "connection refused" y carreras `WSAEADDRINUSE`. |
| Watcher | Cae a polling de mtimes (~200 ms): el crate `notify` sí soporta `ReadDirectoryChangesW`, pero el puente self-pipe de `ray_runtime::watch` es unix. |
| Cierra con | `CREATE_NEW_PROCESS_GROUP` + `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT)` para el reinicio con drenado; Job Objects (`AssignProcessToJobObject` + kill-on-close) para huérfanos; `WSADuplicateSocket` para pasar el listener; puente por evento/IOCP para el watcher. |
| Tamaño | reinicio **S–M** · huérfanos **S** · socket-activation **M** · watcher **M** (depende de 3.6). |

### 3.3 Terminal — `std/term`

| | |
|---|---|
| Hoy (VM) | `is_tty` → **`false` siempre** (silencioso: apaga colores, y `read_hidden` falla con "stdin is not a terminal" en una consola real); `size`/`size_px`/`cell_px` → `None`; `raw`/`read_key`/`capabilities` y los gráficos kitty → `Err("raw mode is not supported on this platform")`. Lo puro (`width`, `char_width`, `fit`) funciona. |
| Hoy (nativo) | Paridad exacta con la VM (los brazos `not(unix)` existen). |
| Superficie | Toda TUI; `read_hidden` (passphrases); detección de color. |
| Cierra con | `GetConsoleMode`/`SetConsoleMode` con `ENABLE_VIRTUAL_TERMINAL_INPUT` + `ENABLE_VIRTUAL_TERMINAL_PROCESSING` (raw = quitar `LINE_INPUT`/`ECHO_INPUT`/`PROCESSED_INPUT`); `GetConsoleScreenBufferInfo` para el tamaño; `GetFileType`+`GetConsoleMode` para `is_tty`. Windows 10 1511+ entiende las secuencias ANSI que `std/term` ya emite. |
| Tamaño | **M** (isatty + size + raw es S; `read_key` depende de 3.4). |

### 3.4 stdin — `std/io`

| | |
|---|---|
| Hoy | `stdin_ready` → **`true` siempre** (miente); `io.read` bloquea el **hilo entero de la VM** (todas las fibras), no la fibra; `io.read_timeout` **ignora el plazo**. Nativo: misma degradación, explícita (`let _ = ms`). |
| Superficie | `read_key` (el ESC de 25 ms decodifica mal las flechas), REPLs, servidores dirigidos por stdin, `ray mcp` y `ray lsp` sobre stdio. |
| Cierra con | `WaitForSingleObject`/`GetNumberOfConsoleInputEvents` (consola), `PeekNamedPipe` (pipe), lecturas overlapped (archivo). |
| Tamaño | **M**. |

### 3.5 Procesos — `std/process`

| | |
|---|---|
| Hoy (VM) | `Err("<program>: running OS processes is not supported on this platform")` en `run`/`cmd`/`stream`/`stdin_pipe`. |
| Hoy (nativo) | **No compila**: el runtime emite llamadas a `ray_runtime::process` y el módulo entero es `cfg(unix)`. |
| Superficie | `process.run`/`cmd`, sesiones persistentes (MCP/LSP hijos), pipelines `sh -c`, drivers de build. |
| Cierra con | Lo portable ya lo es (`std::process::Command`, pipes, env, cwd). Lo unix: `process_group(0)` → Job Objects; `poll(2)` + `O_NONBLOCK` del drenado → `PeekNamedPipe`/overlapped; `kill(-pid)` → `TerminateJobObject`; `Exit.Signal` no tiene análogo (documentar el mapeo). |
| Tamaño | **L**. |

### 3.6 Poller de red y scheduler

| | |
|---|---|
| Hoy (VM) | Sin kqueue/epoll: `raw_fd` → `None` y el scheduler cae al busy-poll de M15.5 (1 ms de sueño + re-encolar). Funciona; más CPU y peor p99 bajo carga. El handshake TLS espera con `sleep(20 ms)` en vez de `poll`. |
| Hoy (nativo) | Sin reactor: `ray build --native --target *windows*` apaga las fibras con aviso (hilo-por-tarea). ⚠️ El apagado mira solo `--target`: un build nativo **en un host Windows sin `--target`** deja las fibras encendidas e intenta compilar corosensei + kqueue/epoll → fallo de compilación. **S** de arreglar (`cfg!(windows)` cuando no hay target). |
| `sleep_ms` | `thread::sleep` en vez de `poll(NULL,0,ms)`: con el tick por defecto de 15,6 ms la precisión del pacing de juegos/audio y de `time.sleep_ms` cae. |
| Cierra con | `wepoll` (ABI epoll sobre IOCP/AFD, encaja en la forma de `src/poll.rs`) para la VM; IOCP nativo a largo plazo, que además desbloquea las fibras; `CreateWaitableTimerEx(HIGH_RESOLUTION)` o `timeBeginPeriod(1)` para el sueño fino. |
| Tamaño | wepoll **M** · IOCP para fibras **L** · sueño fino **S**. |

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
| `key_path` (`ray publish --sign`) | solo `HOME` → sin `USERPROFILE` la clave va a `./.ray/publish.key` | `HOME` → `USERPROFILE`, como ya hace `native_cache_dir` |
| `raise_fd_limit` | no-op (`cfg(unix)`) | N/A en Windows: documentar |
| `packages/tz` | `load()` → `Err` (no hay `/usr/share/zoneinfo`); UTC funciona | tzdata embebida o registro + `windowsZones` de CLDR (**M**) |
| `ray upgrade` en ARM64 | exit 69 (no hay asset) | publicar `aarch64-pc-windows-msvc` (IDEAS §84) |
| FFI | **funciona**: `_errno`, `libloading::os::windows`, `"c"`/`"m"` → `ucrtbase.dll` | — |

## 4. La asimetría VM ↔ nativo

Es el hallazgo transversal de la auditoría. La VM devuelve `Err` en todo hueco; el transpilador,
en cambio, emite Rust **que no compila** en Windows cuando el programa usa cualquiera de estas
cinco superficies: `signals()`, `std/process`, `fs.watch`, `std/audio`, `std/ui`. El usuario ve
un backtrace de `rustc`, no el mensaje del lenguaje.

El arreglo barato e independiente de cualquier port: una **comprobación pre-transpilación** —
si el target efectivo es `*-pc-windows-*` y el programa activa alguno de los cinco flags
(`needs_signals`, `needs_rt_process`, `needs_rt_watch`, `needs_rt_audio`, `needs_rt_ui`),
`ray build --native` falla con el mismo mensaje que daría la VM en runtime. **S**, y
prerrequisito para que este documento sea veraz también para el binario nativo.

## 5. Probablemente funciona, sin verificar

Fuera de la red de CI actual:

1. **Rutas con `\`**: los módulos usan `/` por regla del lenguaje; la frontera módulo → ruta de
   archivo (`ray run C:\proj\src\main.ray`, `Manifest::find` subiendo directorios) no tiene test.
2. **Colores ANSI en consola**: nada activa `ENABLE_VIRTUAL_TERMINAL_PROCESSING`; Windows Terminal
   lo trae por defecto, conhost no. Sumado a `is_tty → false`, el color en `cmd.exe` es incógnita.
3. **UTF-8 en consola**: sin `SetConsoleOutputCP(CP_UTF8)`; los mensajes con acentos pueden salir
   mal en páginas de código heredadas. **Demostrado por el propio censo**: el primer run murió con
   `UnicodeEncodeError: 'charmap' codec can't encode character '\u2192'` al imprimir una flecha
   desde Python en el runner (cp1252). `ray` escribe bytes UTF-8 crudos; con `chcp 65001` o
   Windows Terminal se ven bien, en `cmd.exe` heredado no.
4. **Liberación del puerto** entre reinicios de `ray dev` (TIME_WAIT sin `SO_REUSEADDR` equivalente).
5. **TLS** (ring/rustls con `webpki-roots`): compila, ningún handshake corre en Windows en CI.
6. **SQLite** (`rusqlite` bundled): compila; sin test en Windows (WAL, unidades de red).
7. **`ray build --native` en host Windows**: CI nunca lo ejecuta (ver 3.6 y §4).
8. **`ray mcp` / `ray lsp`** sobre stdio: dependen de 3.4.

## 6. Orden de ataque

| Fase | Qué | Tamaño | Desbloquea |
|---|---|---|---|
| **W1** | `signals()` vía `SetConsoleCtrlHandler` + gate de la emisión nativa; mientras llega, `serve_graceful` degrada a `serve` con aviso cuando no hay señales | M + S | `serve_graceful`, `web.listen_graceful` (**la app `store`**), apagado limpio de cualquier servidor |
| **W2** | Comprobación pre-transpilación (§4); fibras apagadas por host y no solo por `--target`; `key_path` con `USERPROFILE` | S | errores honestos en el nativo; `ray build --native` en Windows |
| **W3** | `ray dev`: `CREATE_NEW_PROCESS_GROUP` + `CTRL_BREAK`, Job Objects para huérfanos | S–M | ciclo edit-run con drenado; sin puertos secuestrados |
| **W4** | `std/term` por Console API (isatty, size, raw) + `std/io` readiness (`PeekNamedPipe`/eventos de consola) | M + M | TUIs, `read_hidden`, color correcto, `read_key`, `ray mcp`/`lsp` sin bloquear la VM |
| **W5** | `wepoll` en `src/poll.rs` + sueño fino | M + S | p99 de servidores bajo carga; pacing de juegos |
| **W6** | `std/process` con `CreateProcess` + pipes + Job Objects | L | MCP/LSP hijos, pipelines |
| **W7** | IOCP para fibras nativas · WebView2 · WASAPI | L × 3 | fibras en el nativo; escritorio y audio |

Al margen: Scoop (bucket propio), winget a demanda y la build `aarch64-pc-windows-msvc` (IDEAS §84).

## 7. Censo de los ejemplos en Windows

_Se rellena con la salida de `windows-census.yml` (resumen del job). Categorías: FALLA (Linux ok,
Windows no), CUELGA, DIFIERE (stdout distinto), OK-CRLF, FALLA-AMBOS, INTERACTIVO (servidores y
TUIs que exceden el plazo en ambos: validar a mano), OK._

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
- **Nativo**: hasta W2, compilar en Windows solo programas sin señales/procesos/watch/ui/audio.
