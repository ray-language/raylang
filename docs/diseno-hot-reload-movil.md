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
| D1 | Parada cooperativa de la VM + reset del runtime | Sí: `tests/vm_stop.rs` |
| D2 | `ray_start` de desarrollo en la crate + protocolo de sync + `ray dev --device` | Sí: el mismo cliente compilado para macOS con `ui-shell` |
| D3 | `ray bundle --ios --dev` / `--android --dev`, QR, bundle id `.dev`, asset prebuilt | Parcial: generación de proyectos en CI; el humo real en el dispositivo |
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
