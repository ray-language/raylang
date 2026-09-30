# Diseño: hot reload del programa en el teléfono (arco M330)

Origen: `RAYLANG-FINDINGS.md` de ray-apps, #50 y #106 (ray808). El nivel 1 ya existía (la
página se recarga; con `[frontend]`, Vite sirve el frontend al webview del teléfono). Lo que no se
recargaba era el **programa raylang**: va compilado dentro de la app como librería nativa
(`--lib`: cdylib en Android, estática en iOS), y cada cambio en un `.ray` exigía recompilar la
librería, regenerar el proyecto e instalar. En el Mac, `ray dev` reinicia en milisegundos; en el
teléfono el ciclo duraba minutos. Es el dolor que empuja a muchos equipos a abandonar un stack
multiplataforma, así que el requisito fijado con el usuario es **fidelidad al dispositivo desde el
primer día**: archivos, llavero, red local, audio y permisos son los del teléfono, no los del Mac.

## La idea: la misma app, otra librería

Tres hechos del código deciden el diseño:

1. La crate de la toolchain ya se compila como librería (`crate-type = ["cdylib", "rlib"]`) y el
   wasm32 del playground demuestra que compilador y VM no dependen del sistema operativo.
2. La VM usa el **mismo** `ray_runtime::ui`, `audio`, `crypto` y `watch` que el binario nativo.
   El shell móvil no habla con el programa: habla con `ray-runtime` por una interfaz C de cinco
   funciones (`ray_ui_set_handlers`, `ray_ui_push_event`, `ray_ui_scheme_*`,
   `ray_ui_shell_capabilities`, `ray_start`), y ya existe el backend `Win::Shell` en `ui.rs`, con
   la feature `ui-shell` para probarlo en escritorio sin teléfono.
3. La librería que genera el transpilador exporta un único símbolo propio: `ray_start`.

Por tanto, el shell iOS/Android generado **no cambia**. Lo que cambia es qué librería enlaza: la
del programa compilado (release) o una **librería de desarrollo** que es la toolchain entera
(loader, checker, VM, runtime) compilada para el teléfono, cuyo `ray_start` en vez de ejecutar un
programa fijo abre un bucle «recibe código, ejecútalo, reinicia». Como es el mismo runtime en el
dispositivo, todo es real; y como la VM y el nativo son byte-idénticos por contrato (harness
diferencial), lo que se prueba en caliente es lo que después se compila.

## Piezas

- **Transporte de código: fuente, no bytecode.** No hay serialización de bytecode y no hace
  falta. `ray dev --device` vigila el proyecto como siempre y, ante un cambio, ejecuta el
  check-before-restart (milisegundos). Si no compila, no manda nada y el programa del teléfono
  sigue vivo, con el diagnóstico en la terminal del Mac. Si compila, envía por WebSocket un
  snapshot diferencial: `.ray`, `ray.toml`, `.ray.html`, assets embebidos y `.ray-deps` ya
  resueltos por git en el Mac. El teléfono lo escribe en su sandbox y llama a `loader::load` →
  `check` → `vm::run_program`. Dependencias resueltas offline; sin git en el teléfono.
- **Reinicio cooperativo dentro del proceso (D1, esta fase).** `ray dev` de escritorio reinicia
  el proceso; iOS no permite que una app se relance sola. La VM gana una *parada cooperativa*
  pedida desde fuera (`vm::stop::request_stop`) y un reset del runtime (`builtins::runtime_reset`)
  que deja el proceso como recién arrancado para el siguiente programa. Detalle abajo.
- **Descubrimiento y seguridad.** `ray dev --device` escucha en la LAN en un puerto alto
  aleatorio e imprime la URL con un token y un código QR; el shell de desarrollo la lee al primer
  arranque y la recuerda. Solo LAN, solo con token; la librería de desarrollo jamás entra en un
  build de release (bundle id con sufijo `.dev` para convivir con la app real).
- **Lo que vuelve al Mac.** `print`, errores de runtime y diagnósticos del teléfono viajan por el
  mismo socket a la terminal de `ray dev`. Devtools: Safari Web Inspector y Chrome remoto ya
  funcionan con el shell. El frontend en Vite sigue con su nivel 1.
- **Distribución de la librería de desarrollo.** Compilar la toolchain para `aarch64-apple-ios`
  y `aarch64-linux-android` en frío es lento; se publica como asset de cada release (como
  `ray-runtime-vendor.tar.gz`) y `ray bundle --ios --dev` la descarga por versión. En el
  teléfono se apagan `ffi` (dlopen) y `process` (iOS no permite `fork`), con el error claro de
  siempre.

## Fases

| Fase | Contenido | Verificable sin teléfono |
|---|---|---|
| D1 ✅ | Parada cooperativa de la VM + reset del runtime | Sí: `tests/vm_stop.rs` |
| D2 ✅ | `src/devlink.rs`: protocolo de snapshot por TCP, `ray dev --device`, `ray dev-client`, entrada C `ray_dev_start` | Sí: `tests/devlink_cli.rs` (anfitrión + cliente headless, cambio, cambio que no compila) |
| D3 ✅ (iOS) | `ray bundle --ios --dev`: librería de desarrollo, página de emparejamiento, `.ray-dev` por proyecto, bundle id `.dev` | Parcial: `tests/devlink_pair.rs` (emparejamiento headless); el humo real en el iPhone |
| D3b ✅ | Android (`--android --dev`, cdylib con los símbolos JNI), `ray dev-lib` y el asset prebuilt por release | Parcial: unidad del fuente generado; humo en el emulador |
| D4 | Consola remota, reconexión, snapshot diferencial, pulido de DX | Sí |

Decisiones tomadas: librería de desarrollo prebuilt como asset de release; si el reinicio
cooperativo no converge en unos segundos, la librería hace `exit(0)` y el usuario toca el icono
(al arrancar se reconecta y recibe el último snapshot); D1 cubre solo lo que el móvil necesita
(dar a `ray dev` de escritorio el reinicio en proceso queda para después).

## D1 por dentro: la parada cooperativa

Reusa la fontanería que ya tenía el scheduler de la VM:

- **Bandera atómica + self-pipe** (`src/vm/stop.rs`), el patrón del canal de señales (M88.1). El
  extremo de lectura del pipe entra **siempre** al conjunto del poller de `io_wait`, así que una
  fibra aparcada en red, en `sleep` o en un `select_timeout` despierta en cuanto alguien pide la
  parada. Sin fd (Windows, wasm) el poller ya sondea a cuantos cortos y consulta la bandera.
- **Puntos de comprobación.** `poll_next` (cada conmutación de fibra) fija `outcome = Err(STOP)`,
  la señal de apagado global de M38.3b que detiene a todos los workers. Para un bucle que nunca
  conmuta, la comprobación va en los **saltos hacia atrás** (`Jump` atrás e `IncJump`, el cierre
  fusionado de los bucles contados): una cuenta atrás por worker lee la bandera cada 4096
  vueltas. El despacho por instrucción queda intacto — instrumentarlo (una máscara sobre el
  `fuel`, o el `fuel` por trozos con recarga fuera de línea) costaba un 4–5 % en un bucle denso,
  medido; el contador en los saltos no se distingue del ruido (ver DESIGN §312).
- **Error distinguido.** `run_program` devuelve `Err` con `RuntimeError::is_stop() == true`:
  quien embebe la VM lo separa de un crash. Una parada pedida sin programa en marcha se descarta
  al arrancar el siguiente (`stop::clear` en `run_program`): no se guarda para él.
- **Reset del runtime** (`builtins::runtime_reset`): `close_all_handles` (M129, el aislamiento de
  `ray test`: sockets, listeners, TLS, SQLite, pipes, watches, archivos y las salidas de audio, cuyo
  alimentador ve el EOF y se retira), `ui::reset_for_restart` (cierra ventanas de escritorio;
  olvida las filas del shell móvil y de mesa SIN avisar al shell — su webview sigue en pantalla y
  el `ui.open` del programa nuevo lo recarga, sin parpadeo — y vacía la cola de eventos para que
  un `closed` del programa viejo no llegue al nuevo) y vuelta al dominio de handles principal.
  Fibras, canales y tareas caen con la VM. El contador de ids no se resetea: los handles no se
  reusan, como entre tests.

Las pruebas (`tests/vm_stop.rs`, en su propio proceso porque la bandera es global) cubren: un
bucle que nunca conmuta, una fibra dormida, una fibra aparcada en `accept` sin plazo, una tarea
girando en otro worker, correr otro programa en el mismo proceso tras parada + reset, y una parada
pedida sin programa en marcha.

## D2 por dentro: el enlace (`src/devlink.rs`)

- **Marcos** `[u32 BE len][u8 kind][payload]` sobre TCP, `std::net` puro. Del dispositivo:
  `HELLO` (token, nombre, versión de raylang) y `STATUS` (un octeto de estado + texto). Del
  anfitrión: `SNAPSHOT` (entradas `[u32][ruta][u64][bytes]`, rutas relativas con `/`). Un
  `SNAPSHOT` siempre reinicia. Sin JSON ni WebSocket: los dos extremos son nuestros.
- **Anfitrión** (`ray dev --device`): escucha en `0.0.0.0:0` (puerto alto aleatorio; la IP de
  LAN se obtiene con un `connect` UDP que no envía nada), imprime `ray-dev://ip:puerto/token`,
  y reusa de `ray dev` el vigilante de kernel, el debounce, la confirmación por hash y el
  check-before-restart (`ray build`). Publica el snapshot al conectar un dispositivo y en cada
  cambio que compila; los `STATUS` de cada dispositivo salen en la terminal.
- **Snapshot**: `ray.toml`/`ray.lock`, todo `.ray`/`.ray.html` fuera de `target`, `node_modules`
  y ocultos (un `.ray` generado de un `.ray.html` hermano no viaja), los assets de `[native]
  embed`/`[frontend] dist` y `.ray-deps/` entero sin sus `.git` ni `.index`. Tope 64 MB.
- **Dispositivo** (`run_device`): conecta (reintento cada segundo; al perder el enlace el
  programa sigue y se reconecta), escribe el snapshot en `<dir>/project` borrando lo que ya no
  viene, y en un hilo propio: `Manifest` → entrada → `loader::load_with_deps` con las raíces de
  `.ray-deps` del snapshot (nunca git ni índice) → `check` → `compile` → `vm::run_program`.
  Antes de cada snapshot: `request_stop` (D1), espera al hilo hasta 5 s y `runtime_reset`; si
  no para a tiempo, `exit(0)` (la decisión de diseño: el shell arranca limpio y reconecta).
- **Entrada C** `ray_dev_start(url, dir)` para el shell móvil: como `ray_start`, retorna 0 con el
  enlace corriendo en su hilo. Cómo recibe el shell la URL (QR/emparejamiento) es D3.
- **Pendiente para D4**: snapshot diferencial, `print`/`eprint` del dispositivo hacia la
  terminal del anfitrión (hoy van a su stdout/logcat), dependencias `path = …` fuera de la raíz
  (no viajan en el snapshot).

## D3 por dentro: el shell de desarrollo (iOS)

- **La librería de desarrollo es un reemplazo directo.** `build_dev_lib` genera un proyecto
  Cargo mínimo (`crate-type = ["staticlib"]`, dependencia `raylang` por ruta al árbol de la
  toolchain con las features del móvil: sin `interp` ni `ffi`) cuyo `ray_start` es
  `devlink::start_from_shell(None, None)`. Los `ray_ui_*` que el shell referencia llegan de
  `ray-runtime` a través del rlib, y `nm -gU` lo confirma: el `.a` exporta exactamente los
  símbolos del contrato del shell más `ray_dev_start`. El proyecto Xcode es el de siempre
  (`bundle_ios::write_project` sin cambios) con nombre `<app>-dev` y bundle id `<id>.dev`.
- **Emparejamiento sin tocar el shell.** La librería monta su página en `ray://app/__raydev/
  pair.html` (`ui::scheme::mount_bytes`) y la abre como una ventana más; la página manda la
  URL con `window.ray.send`, que llega como un evento `message` por la cola de `ui`. La URL
  se guarda en `Application Support/ray-dev/link.url`; a partir de ahí `run_device_until`
  con un plazo de 20 s sin respuesta devuelve a la página con la URL rellenada.
- **Puerto y token por proyecto.** `ray dev --device` guarda `port=`/`token=` en `.ray-dev`
  (oculto: fuera del snapshot y de `scan_sources`) y vuelve a escuchar ahí; si el puerto está
  ocupado avisa y toma otro. Sin esto, cada sesión pedía emparejar de nuevo.
- **Android (D3b)**: el cdylib de desarrollo define él mismo los símbolos JNI que el shell
  resuelve por nombre (`JNI_OnLoad`, `Java_org_raylang_shell_RayBridge_*`), delegando en
  `ray_runtime::ui::android_*` exactamente como el transpilador (M156); por eso el proyecto
  Cargo generado declara `ray-runtime` también como dependencia directa. El shell pone
  `HOME` en el directorio de datos de la app, así `default_dir()` cae en `$HOME/.ray-dev`.
- **La librería prebuilt (D3b)**: `build_dev_lib` prueba `RAY_DEV_LIB`, luego el árbol de
  fuentes (`CARGO_MANIFEST_DIR` horneado al compilar `ray`), y si no existe descarga
  `ray-dev-lib-<target>.tar.gz` de la release de su versión a `~/.ray/dev-lib/<versión>/`
  (`curl` + `tar`, como `ray upgrade`). El job `dev-lib` de `release.yml` construye los cuatro
  targets móviles con `ray dev-lib` desde el mismo checkout.

