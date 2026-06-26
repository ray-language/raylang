# `epoll` y `kqueue`: readiness del SO

El servidor concurrente de M15.5 ya funcionaba: muchas conexiones a la vez sobre un solo hilo, con las
fibras del scheduler de M12. Pero tenía un corazón humilde. Cuando ninguna fibra estaba lista para correr
pero varias esperaban datos de la red, el scheduler hacía un **busy-poll**: dormía ~1 ms y **reintentaba
todas** las fibras aparcadas, a ver si alguna ya tenía datos. Las que seguían sin tenerlos, a dormir otra
vez.

Funciona, es cero-dependencias y es didáctico. Pero paga dos precios: **latencia** (hasta ~1 ms de
retraso aunque los datos lleguen al instante) y **CPU** (despierta y revisa los N sockets cada
milisegundo aunque ninguno esté listo). M17 lo sustituye por lo que hacen los servidores de verdad:
**preguntarle al sistema operativo** qué socket está listo, y **bloquearse** hasta que alguno lo esté.

## La tensión: readiness vs. cero dependencias

Cada sistema operativo tiene su API de *readiness*: **`kqueue`** en macOS y los BSD, **`epoll`** en
Linux. Le das una lista de descriptores y te bloquea hasta que al menos uno tenga datos, devolviéndote
cuáles. Es exactamente lo que necesitamos.

El problema es que la `std` de Rust **no las expone**, y la invariante del proyecto es **cero
dependencias de Cargo** — nada de `libc`, nada de `mio`. ¿Cómo se llama a un syscall sin el crate que
trae sus declaraciones?

La respuesta es honesta y mínima: **declararlas nosotros**. En `src/poll.rs`, un puñado de bloques
`unsafe extern "C"` declaran `kqueue`/`kevent` (macOS/BSD), `epoll_create1`/`epoll_ctl`/`epoll_wait`
(Linux) y `close`. Esas funciones viven en libSystem (macOS) / libc (Linux), que **siempre están
enlazadas** a cualquier binario — no son una dependencia de Cargo, solo unas firmas de FFI con `unsafe`
**acotado** a este módulo. Los descriptores salen de `std` vía `AsRawFd::as_raw_fd()`. Es la misma
filosofía que el JSON-RPC del LSP (M10.2) o el PRNG de M15: lo que `std` no da y no queremos como
dependencia, se escribe a mano, pequeño y contenido.

```
wait_readable(fds, timeout_ms) -> Ready(listos) | Unsupported
```

Una función con tres ramas por plataforma (kqueue, epoll, y un *fallback* para Windows u otras). En
macOS, una sola llamada a `kevent` registra los descriptores y espera → un syscall.

## El cambio en el scheduler

Antes, `io_parked` era una lista de fibras a secas. Ahora cada una lleva su `fd` (obtenido del registro
de handles al aparcar). El bucle de planificación, cuando no hay nadie listo:

1. Reúne los `fd` de todas las fibras en E/S.
2. Llama a `wait_readable` con timeout **infinito** — y se **bloquea** en el kernel. Esperar a la red es
   correcto: no hay nada más que hacer y no se quema CPU.
3. Despierta **solo** las fibras cuyos descriptores quedaron listos; las demás siguen aparcadas.

Si la plataforma no tiene poller (`Unsupported`) o la espera se interrumpe (EINTR), cae al busy-poll de
M15.5 — así **siempre hay progreso**, pase lo que pase. Cero opcodes nuevos; el resto del runtime,
intacto.

## Probarlo sin poder verlo

M17 no cambia **nada** observable: el mismo programa produce la misma salida, en el mismo orden
determinista. Solo cambia *cuándo* despiertan las fibras y cuánta CPU se gasta esperando. Eso convierte
la prueba en un ejercicio de **regresión**: los tests del servidor concurrente de M15.5 siguen verdes,
pero ahora recorriendo el camino de `kqueue` real en macOS. Que el servidor concurrente atienda a dos
clientes fuera de orden **demuestra** que el readiness del SO desbloquea las fibras correctas — y lo hace
sin el 1 ms de antes.

Es una optimización del tipo más satisfactorio: el código de arriba (las fibras, el `spawn`, el servidor
de eco) no se enteró de nada. Cambió el motor, no la carretera.
