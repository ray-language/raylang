# raylang — El contrato de producción

> Qué significa, en concreto, que raylang esté enfocado a **producción real**: los ejes que se
> mantienen, las invariantes que no se negocian, el estado medido y las guardas que lo sostienen.
> Es un documento **vigente** (se actualiza con el proyecto), no una crónica: el porqué histórico
> de cada decisión vive en [DESIGN.md](DESIGN.md) y las mediciones en
> [PERFORMANCE.md](PERFORMANCE.md). El plan que llevó hasta aquí (arcos A–D, M33–M43) está
> ejecutado y resumido en el apéndice.

---

## 1. Los ejes

raylang se juzga contra seis ejes. Los cinco primeros vienen del cambio de norte de julio de 2026;
el sexto se añadió el 14 de julio, cuando el rendimiento pasó a ser el objetivo nº 1.

| Eje | Qué significa aquí |
|---|---|
| **Rendimiento** | competir con Node y Go en cargas de servicio, no en un microbenchmark elegido a mano. Se persigue **midiendo**: banco poliglota + banco de carga web + gates de regresión de tiempo y de memoria |
| **Moderno** | lo que un programador de 2026 espera el primer día: tipos suma y pattern matching exhaustivo, genéricos con bounds, traits con métodos por defecto y trait objects, `Option`/`Result`/`?` sin `null`, iteradores perezosos, UFCS y pipelines, módulos con cápsulas |
| **Flexible** | el ecosistema crece **sin tocar el compilador**: paquetes con registro y firmas, FFI con ABI C, y una stdlib que se escribe en el propio lenguaje |
| **Ligero** | binario único, arranque de milisegundos, dependencias contadas, y builds a medida (*slim*, `--without`) para contenedores y embedding |
| **Seguro** | memory safety, sin `null`, sin data races por construcción, compilador sin pánicos, confinamiento opcional y cadena de suministro verificada. El detalle es [SECURITY.md](SECURITY.md) |
| **Elegante** | la propiedad arquitectónica: núcleo pequeño, todo lo demás encima (erasure, azúcar de front-end, builtins registrados una sola vez). Cada fase declara qué **no** va a engordar |

## 2. Las invariantes (no negociables)

1. **La SPEC manda.** [SPEC.md](SPEC.md) es normativo: define qué programa es válido y qué hace.
   Un desacuerdo entre la SPEC y la implementación es un bug de una de las dos y se resuelve
   explícitamente; cambiar el lenguaje empieza por cambiar la SPEC.
2. **Tres motores, un comportamiento.** VM (producto), binario nativo (despliegue) e intérprete
   (oráculo) producen salida **idéntica** para todo programa determinista, y el nativo lo hace
   **byte a byte**. Las excepciones están enumeradas en la SPEC, no se descubren por sorpresa.
3. **Una fase a la vez, con tests y su commit.** Nada entra sin su prueba y sin poder compilar por
   sí solo. Los commits son *Conventional Commits* en español.
4. **Medir antes de conservar.** Ninguna optimización se queda si no supera el ruido en el banco.
   Las descartadas se registran igual (el ledger de IDEAS.md y PERFORMANCE.md), para no volver a
   intentarlas a ciegas.
5. **Dependencias justificadas, una a una.** Entra un crate cuando hacerlo a mano sería peor
   ingeniería o cuando la mejora está medida; nunca por comodidad. La lista completa, con su
   alcance y su feature, está en [SECURITY.md](SECURITY.md#política-de-dependencias).
6. **Errores con posición, siempre.** Todo token y todo nodo lleva `(línea, columna)`; ningún
   diagnóstico se emite sin ubicación. Es un principio, no un extra.
7. **Lo que el lenguaje entrega al usuario va en inglés** (diagnósticos, LSP, CLI); los
   identificadores también. Los comentarios del código van en español. Lo vigila un test de CI.

## 3. Estado medido (jul 2026)

| Métrica | Valor |
|---|---|
| Núcleo Rust | ~53.100 líneas en `src/` + ~2.800 en `crates/ray-runtime` |
| Código raylang | 268 archivos `.ray` (173 ejemplos, 15 módulos `std/`, 35 de paquetes, 13 del compilador auto-alojado) |
| Tests | **703** unitarios + **123** archivos de integración (~28.300 líneas), con oráculo VM↔intérprete y corpus de paridad del binario nativo |
| Dependencias de Cargo | 7 externas directas en el binario `ray` (`ahash`, `mimalloc`, `rustls` + 2 satélites, `rusqlite`, `libloading`) y 4 más en `ray-runtime` (`ring`, `regex`, `corosensei`, y las mismas tras features). Todas justificadas, casi todas tras una feature (ver SECURITY.md) |
| Higiene | ~90 avisos de clippy (55 son `collapsible_if`; ninguno es error) y 29 `TODO` anotados |
| `unsafe` | acotado a una docena de archivos, cada bloque con su invariante `SAFETY` documentada e inventariado en SECURITY.md |
| Motores | VM (producto) · binario nativo (despliegue, byte-idéntico) · intérprete (oráculo) |
| Concurrencia | actores con heap aislado sobre scheduler **M:N multicore** (VM) y fibras M:N (nativo) |

## 4. Las guardas

Lo que impide que el contrato se erosione en silencio; todo corre en CI:

| Guarda | Qué protege |
|---|---|
| Oráculo VM↔intérprete | que un cambio de runtime no altere la semántica |
| Corpus de paridad nativo | que el binario nativo siga siendo byte-idéntico a la VM |
| `tests/native_differential.rs` (M120) | que las **interacciones de features** que los ejemplos idiomáticos no ejercitan tampoco diverjan: programas GENERADOS (builtin×tipo, mutación-en-constructor, return-en-closure, valores cruzando fibras…), 3 motores byte a byte, bisección automática al divergir (humo en cada `cargo test` + campaña en cada push + nocturna con presupuesto alto) |
| `tests/fuzz_frontend.rs` | que ninguna entrada haga *panic* al compilador (campaña continua + nocturna) |
| `tests/ice_policy.rs` | que los fallos internos vayan por `ice!()` y no por `unwrap` suelto |
| `tests/naming_policy.rs` | que no se cuele *spanglish* en los identificadores |
| `tests/fmt_policy.rs` | que todo `.ray` versionado siga siendo un punto fijo de `ray fmt` — y, por separado, que el formateador **converja** (formatear dos veces = formatear una) |
| `tests/perf_regression.rs` | que ningún cambio degrade el banco más de un 5% |
| Gate de memoria del banco | que no crezca el pico de RSS sin darse cuenta |
| `cargo clippy --all-targets` + `cargo audit` | lints y avisos de seguridad de las dependencias |
| Build wasm32 del playground | que el target del playground web no vuelva a romperse en silencio (una dep o un `usize` de 64 bits asumido lo rompen sin que ningún otro paso lo vea) |
| Meta-circularidad del `selfhost/` | que el lenguaje siga siendo capaz de compilarse a sí mismo |

## 5. Qué NO se promete

Ser honesto sobre el borde es parte del contrato:

- **No hay auditoría externa.** El proyecto lo mantiene una sola persona; las garantías son reales
  y verificadas en CI, pero nadie de fuera las ha revisado.
- **El confinamiento (`--fuel`/`--heap`) es de la VM.** El binario nativo no lo tiene, y `--fast`
  renuncia a la detección de desbordamientos a propósito.
- **El FFI es la frontera insegura**, por definición y por diseño.
- **Los paquetes (`net`, `db`, `web`, `rpc`…) versionan aparte** de la SPEC: su superficie puede
  moverse más rápido que la del lenguaje.
- **Windows va por detrás de macOS/Linux** (ver [Windows](#windows) abajo).

### Windows

El inventario completo de deudas, con la API de Windows que cierra cada hueco, su tamaño y el
orden de ataque, vive en [`docs/windows.md`](docs/windows.md) (M167).

Desde M165 el camino de instalación es de primera: `install.ps1` (`irm … | iex`), `ray upgrade`
con el zip de la release, y un job de CI en `windows-latest` que construye el binario, corre la
VM y ejecuta el instalador REAL contra la última release; `release.yml` prueba el instalador
contra el zip recién subido. Lo que funciona: la toolchain entera (`ray new/run/build/test/fmt/doc/
lsp/repl/mcp`), la VM y el binario nativo, la red (sockets, TLS, HTTP/1.1 y 2, WebSocket, DNS,
clientes de BD), `std/fs`, `std/json`/`toml`/`regex`/`crypto`, el framework web, `ray dev` (desde M172 con
reinicio drenado, sin huérfanos y con socket-activation, como en unix) y, desde M173, `std/term`
y `std/io` por la Console API (TUIs, `read_hidden`, `read_key`, stdin sin bloquear la VM). Los huecos, todos con
`Err` honesto de plataforma en vez de fallo silencioso:

| Superficie | Estado en Windows |
|---|---|
| `ray dev` / `ray test --watch` | **funcionan** (M172): el reinicio manda `CTRL_BREAK` al grupo del hijo (`serve_graceful` drena), un Job Object mata al hijo si el supervisor muere, `--port` retiene el socket entre reinicios (handle heredable); el watcher va por eventos de kernel (`ReadDirectoryChangesW`, M181) |
| `std/process` | **funciona** (M175): `run`/`cmd`/`stream`/`stdin_pipe` con Job Object por hijo y escalera `CTRL_BREAK` → `TerminateJobObject`; `Exit.Signal` mapeado (9/15/2); la escritura al stdin del hijo es bloqueante |
| `std/term` (`is_tty`, `size`, `raw`, `read_key`, `read_hidden`, `capabilities`) | **funcionan** (M173) por la Console API; `size_px`/`cell_px` → `None` (la API no expone píxeles) |
| `std/io` (`read`, `read_timeout`, `stdin_ready`) | **funcionan** (M173): disponibilidad real en consola y pipes, la fibra aparca (no la VM), el plazo vence |
| `signals()` | **funciona** (M168): Ctrl-C/Break → 2, cierre/logoff/apagado → 15 vía `SetConsoleCtrlHandler`; sin SIGWINCH (28) hasta el arco de terminal |
| `fs.chmod` | no soportado (permisos POSIX) |
| `fs.watch` | **funciona** (M181): eventos de kernel por `ReadDirectoryChangesW` (notify) con puente por cola compartida; la fibra de la VM aparca sin fd y despierta solo con evento en cola |
| FFI a `"c"`/`"m"` | resuelve a `ucrtbase.dll`/`msvcrt.dll` (M165); librerías propias por nombre `.dll` |
| Paquetes (`ray add`, `ray.lock`) | funcionan (M166): clones con LF forzado y hash insensible a CRLF |
| Poller de red | **`WSAPoll`** (M174): readiness real, sin busy-poll; `read_timeout` de sockets vence; sueño fino por *waitable timer*; UDP sin el reset 10054 |
| Fibras en el binario nativo | **funcionan en x86_64** (M182): scheduler M:N sobre un reactor `WSAPoll` persistente; pipes, consola y watch por el pool bloqueante. En ARM64, `--without fibers` automático (corosensei no tiene backend AArch64-Windows) |
| `std/ui` | **funciona** (M179): ventana Win32 + WebView2 (`webview2-com`, el único crate del port), menús, diálogos, puente IPC; headless (M177) en pruebas y CI |
| `std/audio` | **funciona** (M178): WASAPI en modo compartido, s16le con conversión del motor; `audio.write` bloquea el hilo con el búfer lleno (pipe anónimo, sin aparcar la fibra) |
| `ray bundle` | **funciona** (M180): `<name><name>.exe` (subsistema WINDOWS, icono y VERSIONINFO embebidos por `UpdateResourceW`) + `<name>.lnk`; sin Authenticode en v1 |
| Build arm64 | **publicada** (M185): `raylang-aarch64-pc-windows-msvc.zip`, compilada nativa en el runner `windows-11-arm`; `install.ps1` y `ray upgrade` la eligen por la arquitectura del sistema |

---

## Apéndice — el plan que trajo hasta aquí (arcos A–D, M33–M43)

El análisis original (8 jul 2026) identificó siete brechas hacia producción y las agrupó en cuatro
arcos. **Los cuatro están ejecutados**; se resumen aquí para dar contexto a las decisiones que
todavía se ven en el código. El detalle razonado, fase a fase, está en DESIGN.md (§37 y siguientes)
y en el historial de git.

| Arco | Contenido | Resultado |
|---|---|---|
| **A — estabilidad** (M33–M35) | spans y multi-error, compilador sin ICEs + fuzzing, SPEC normativa y versionado, un solo motor de producto | ✅ El front-end no hace *panic* con ninguna entrada; la VM es el producto y el intérprete el oráculo |
| **B — rendimiento y paralelismo** (M36–M38) | optimización de la VM, GC de producción, multicore | ✅ Superinstrucciones y arcos de optimización medidos; el GC se resolvió con **heap por fibra**; scheduler M:N multicore con speedup real (3,84× en 4 tareas) |
| **C — ecosistema** (M39–M41) | CLI `ray` + gestor de paquetes, stdlib versionada, FFI | ✅ `ray` con subcomandos, `ray.toml`/`ray.lock` con hashes, registro con firmas, `std/` embebida y FFI con ABI C |
| **D — endurecimiento** (M42–M43) | política de overflow, cripto de producción, límites de recursos, fuzzing y `cargo audit`, distribución | ✅ `int` *checked*, cripto vía `ring`, `--fuel`/`--heap`, CI completa, instalador y workflow de release |

Dos apuestas del plan original cambiaron de forma al ejecutarse, y merece la pena decirlo:

- **El GC generacional de M37 no se construyó**: el aislamiento de heap por fibra (M38) hizo el
  problema pequeño, que es mejor solución que una más sofisticada.
- **El backend nativo estaba fuera de la 1.0** ("investigación post-1.0") y acabó siendo el mayor
  salto de rendimiento del proyecto: hoy es el destino de despliegue recomendado.

Lo que el plan dejó explícitamente fuera —macros de usuario, algebraic effects, reflection— sigue
fuera, anotado en [IDEAS.md](IDEAS.md).
