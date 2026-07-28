# Diseño: quitar el hilo-por-conexión del backend nativo

> Documento de DISEÑO, previo a cualquier código. Parte del caso medido en
> [`investigacion-p999-webserver-nativo.md`](investigacion-p999-webserver-nativo.md) §6:
> a `-c 1000` el binario nativo levanta **1002 hilos de SO y 268 MB** contra los **17 hilos y 49 MB**
> de Go, creciendo lineal a ~265 KB/conexión. No es una pendiente, es un muro: 10 000 conexiones
> pedirían ~2.6 GB solo en estructuras por hilo.

## 1. Corrección de la propuesta inicial

La opción que yo mismo recomendé se enunció como **"llevar `src/poll.rs` al runtime nativo — reusa
código probado, cero dependencias"**. Eso es cierto de `poll.rs` y **falso del problema**.

`poll.rs` son ~300 líneas autocontenidas (`wait(read_fds, write_fds, timeout_ms) -> PollResult`,
kqueue/epoll por `extern "C"`, sin deps). Portarlo es trabajo de una tarde. **Pero `poll.rs` no
resuelve lo que hay que resolver.**

El problema real es otro, y conviene enunciarlo con precisión:

| | VM | nativo |
|---|---|---|
| qué es una fibra | un **dato**: `Fiber { frames, stack, heap, … }` | una **pila de llamadas de Rust** |
| cómo se aparca en `read` | el scheduler deja de ejecutar esa estructura y sigue con otra | no se puede: el hilo está dentro de `TcpStream::read` |

La VM puede aparcar porque la ejecución está **reificada**: el estado de la fibra es un objeto que el
scheduler guarda en `io_parked` y retoma cuando `poll::wait` dice que su fd está listo. En el nativo,
el código raylang compila a funciones de Rust ordinarias y su estado vive en la pila de la máquina.
**Una pila de llamadas de Rust no se puede apartar y retomar luego** sin uno de los dos mecanismos
del §2.

Dicho de otro modo: `poll.rs` responde "¿qué fd está listo?", y esa nunca fue la pregunta difícil.
La pregunta difícil es "¿cómo suspendo a mitad de `handle_http` sin ocupar un hilo?".

## 2. Las opciones reales

### A. Transpilar a `async fn`

Cada función raylang que pueda hacer E/S se emite como `async fn`, y cada llamada a otra función así
se convierte en `.await`. El compilador de Rust genera la máquina de estados; un ejecutor pequeño
(propio, sobre `poll.rs`) multiplexa miles de conexiones sobre pocos hilos.

- ✅ Es el mecanismo estándar y seguro (sin `unsafe` propio); el estado suspendido ocupa lo que
  ocupe la máquina de estados, típicamente cientos de bytes, no 265 KB.
- ❌ **Coloreado de funciones**: `async` es contagioso hacia arriba. Como el checker no distingue hoy
  funciones "que hacen E/S", o se colorea **todo** el programa generado —con el coste de compilación
  y de rendimiento que eso implica en el camino puramente CPU— o hay que **calcular el cierre
  transitivo** de qué funciones alcanzan un builtin de E/S y colorear solo esas. Lo segundo es un
  análisis nuevo en el transpilador, y hay que resolver los indirectos: una closure guardada en una
  estructura y llamada después puede hacer E/S sin que el sitio de llamada lo sepa.
- ❌ Interactúa con lo que ya hay: `try_call` usa `catch_unwind` (que sobre un `Future` cambia de
  forma), el modelo de actores mueve heaps entre fibras, y `__RAY_POOL` tendría que convivir con el
  ejecutor o desaparecer.

### B. Corrutinas con pila propia (*stackful*)

Cada conexión recibe una pila pequeña (32-64 KiB, crecible) y el cambio de contexto se hace guardando
y restaurando registros. El código generado **no cambia**: sigue pareciendo bloqueante.

- ✅ Cero coloreado: el transpilador no se entera. `try_call`, actores y el código existente siguen
  igual.
- ✅ El aparcado es exactamente el de la VM, lo que hace la paridad casi trivial.
- ❌ **`unsafe` de cambio de contexto por arquitectura** (aarch64 + x86_64 como mínimo), o una
  dependencia nueva del tipo `corosensei`. Es un terreno donde los bugs son corrupciones de memoria,
  no excepciones.
- ❌ La memoria por conexión no baja a cientos de bytes sino a la pila mínima (~32-64 KiB). Sigue
  siendo **4-8× mejor** que los 265 KB de hoy, pero no es el orden de magnitud de (A).

### C. No tocar el modelo; reducir su coste

Ya explorada y medida (§6.1b de la investigación): la pila por hilo **no** influye (hipótesis
falsada), y `--without mimalloc` se lleva ~128 KB de los 265. Es una palanca real para el perfil
servidor, pero **no quita el muro**: 10 000 hilos siguen siendo 10 000 hilos.

## 3. Qué significa "misma semántica de concurrencia" (la pregunta de paridad)

Esto es lo que prometí fijar antes de tocar código, porque es donde la paridad cuesta trabajo real.
Aplicando los tres niveles acordados:

**Nivel 1 — inviolable.** Lo que el cambio NO puede alterar:

- **Puntos de cesión**: `recv` sobre canal vacío, `send` sobre canal acotado lleno, E/S de socket que
  bloquearía, `sleep`, `join`. Ni más ni menos: añadir un punto de cesión donde hoy no lo hay cambia
  el entrelazado observable de programas correctos.
- **FIFO de canales** y **orden causal de `print`** (la lección de M96f: un `join` que pasa-antes-que
  un `print` debe seguir viéndose en ese orden).
- **Aislamiento de heap por fibra** (modelo de actores) y semántica de movimiento en `send`.
- **Propagación de fallos**: `Task` que falla → `Failed`; `scope` que cierra → cancela hermanas.

**Nivel 2 — por defecto, renunciable con decisión registrada.** Salida byte a byte en el corpus
determinista (`tests/native_corpus.rs`, ~50 programas). Es el guardián barato de todo lo anterior y
debe correrse **antes** de cualquier medición de rendimiento, no después.

**Nivel 3 — explícitamente NO exigido.** Que el scheduler nativo sea el mismo que el de la VM. Hoy
ya no lo es (hilos de SO contra fibras M:N), y el objetivo del arco es cambiarlo — hacia el de la VM,
pero por rendimiento, no por parecido. El orden concreto de entrelazado entre fibras sin relación
causal **ya es no determinista en el nativo** y no forma parte del contrato; `--deterministic` es
quien lo fija cuando hace falta.

Esta distinción rebaja mucho el listón: no hay que replicar el scheduler de la VM, hay que **no
romper el nivel 1 y mantener verde el corpus**.

## 3b. El objetivo, MEDIDO (no estimado): la VM ya corre el modelo destino

Antes de construir un prototipo sintético conviene notar que **el modelo objetivo ya existe y se
puede medir**: la VM ejecuta el MISMO `net/webserver` sobre fibras y `poll.rs`. Mismo programa,
misma carga, loopback:

| a `-c 1000` | hilos de SO | RSS | por conexión | rps |
|---|---|---|---|---|
| nativo (hilo por conexión) | **1002** | **268 MB** | **265 KB** | ~145 000 |
| **VM (fibras + `poll.rs`)** | **13** | **40 MB** | **23 KB** | ~59 000 |
| Go `net/http` (referencia) | 17 | 49 MB | — | ~132 000 |

(VM: 19 MB a `-c 100` → 40 MB a `-c 1000`, pendiente de 23 KB/conexión; hilos **constantes** en 13,
independientes del número de conexiones.)

**Dos correcciones a lo escrito arriba, ambas a peor para mí:**

1. **La estimación de 32-64 KiB por conexión era pesimista.** El modelo de fibras, en el código
   raylang real, cuesta **23 KB** por conexión. El objetivo no es una conjetura: está medido.
2. **La descomposición del §6.1b estaba mal interpretada.** Se dijo que los ~137 KB/conexión que
   quedaban al quitar mimalloc eran "estado raylang irreducible (heap por fibra + búferes)". Falso:
   la VM tiene ese mismo estado por conexión y le cuesta 23 KB. O sea que **~91 % de los 265 KB del
   nativo son atribuibles al HILO** (arena de mimalloc, cachés por hilo del asignador del sistema,
   contabilidad del SO), no al modelo de datos de raylang.

Eso refuerza el arco: quitar el hilo no ahorra "algo", ahorra **casi todo** — y el destino ya bate a
Go en memoria (40 MB contra 49) con **13 hilos constantes** en vez de 1002 crecientes.

**La contrapartida, también medida**: la VM sostiene ~59k rps donde el nativo hace ~145k. Pero ese
2.5× es sobre todo el coste de **interpretar bytecode**, no del modelo de fibras — son dos cosas
distintas que esta medición no separa. El arco busca justamente quedarse con el modelo de memoria de
la VM y el rendimiento del nativo.

## 3c. Cuánta pila toca de verdad una conexión, MEDIDO — y el búfer que la inflaba

Era la última incógnita de la opción (B), y se acotó sin escribir una sola corrutina: se hizo
configurable el tamaño de pila de los hilos del pool (`Builder::stack_size`, parche temporal) y se
bajó bajo carga real (`plaintext`, `-c 200`, oha) hasta que el binario revienta.

| pila por hilo | 512 KiB | 128 | 64 | **56** | **48** | 32 | 16 |
|---|---|---|---|---|---|---|---|
| antes | vive | vive | vive | **vive** | **desborda** | desborda | desborda |
| después | vive | vive | vive | vive | vive | vive | **vive** |

El escalón entre 48 y 56 KiB tenía nombre y apellidos: **`let mut buf = [0u8; 65536]` en la pila**,
dentro de `__ray_socket_read`/`_read_bytes`. Cobraba dos veces:

1. Rust **inicializa el array a cero**, así que cada hilo tocaba sus 16 páginas en el primer `read` y
   quedaban residentes de por vida — RSS puro por hilo, no por trabajo.
2. Hundía la pila 64 KiB, fijando ese suelo de ~56 KiB por conexión.

Moverlo a un `thread_local!` (commit aparte, `perf(native)`) da un **beneficio inmediato en el modelo
actual**, A/B intercalado y reproducible al MB: **c=200 → 59-50 MB (−15 %)**, **c=1000 → 285-239 MB
(−16 %, o sea −46 KB por conexión)**, con el caudal igual o algo mejor. Y para el arco deja lo que
importa: **con el búfer fuera, el servidor funciona con las pilas más pequeñas que macOS concede**
(pedidas de 8 KiB; concedidas de 28, ver la auditoría de abajo).

**Auditoría de esta medición (corrige la primera versión de este apartado).** El "sobrevive a
8 KiB" no puede tomarse literal: se midió con `pthread_get_stacksize_np` qué concede macOS de
verdad, y **pedir 8 KiB concede 28 KiB** (Rust añade reserva para TLS y macOS redondea a páginas de
**16 KiB** — Apple Silicon). Así que la prueba "después" solo demuestra tocado < 28 KiB. La cota
fina la da la **bisección de antes**: con el búfer de 64 KiB en la pila, 48 KiB pedidos
(≈68 concedidos) desbordan y 56 (≈76) sobreviven → la pila tocada sin el búfer está **entre ~4 y
~12 KiB**. En una máquina de páginas de 16 KiB eso es **una página residente por pila**; en Linux
(páginas de 4 KiB), 1-3 páginas.

La cifra exacta importa menos de lo que parece: con pilas `mmap`-eadas y página de guarda, la
**reserva** virtual es gratis (solo cuentan las páginas tocadas), así que las corrutinas pueden
reservar 64-128 KiB con seguridad y pagar solo la página que toquen. El número no es de seguridad,
es de estimación de RSS.

### Lo que el prototipo sintético todavía aportaría: nada

Ya **no** hace falta para saber si el modelo de fibras baja la memoria (medido: 11.5×) ni para
dimensionar la pila de una corrutina (medido: 4-12 KiB tocados, ~1 página). Los dos números que el
prototipo iba a estimar están medidos sobre el código real, que es mejor evidencia de la que el
prototipo habría dado. **Se descarta.**

Estimación resultante para (B): **~23 KB (estado por fibra, medido en la VM) + 1 página de pila
tocada (16 KiB en macOS, 4-12 KiB en Linux) ≈ 30-40 KB por conexión.** 10 000 conexiones pasarían de
~2.6 GB y 10 000 hilos a **~300-400 MB y un puñado de hilos**.

### Verificación 2×2: los dos ahorros conocidos son aditivos (misma sesión, A/B intercalado)

| RSS a `-c 1000` | con mimalloc | `--without mimalloc` |
|---|---|---|
| búfer en la pila (antes) | 285 MB (×3 idénticos) | ~209 MB (195-212) |
| búfer thread-local (después) | **239 MB** (×3 idénticos) | **~147 MB** (141-157) |

Aditividad: 285 − 46 (búfer) − 76 (mimalloc) = 163 esperados contra ~147 medidos — consistente
dentro del ruido (el asignador del sistema es notablemente más ruidoso que mimalloc, que reproduce
al MB). El caudal no paga: la variante slim además rinde ~2 % más aquí. Dos avisos de honestidad
metodológica: (1) los **absolutos** entre sesiones bailan (el "antes" de hoy da 285 MB donde la
investigación midió 268; el ahorro de mimalloc da hoy −76 KB/conexión donde aquella midió −128) —
solo el A/B intercalado de la misma sesión es concluyente, y las conclusiones de este documento se
apoyan en esos; (2) todo esto está medido sobre `plaintext`, el handler más simple — código de
usuario más profundo tocará más pila, lo que refuerza usar reserva generosa con página de guarda.

## 4. Recomendación

**(B), corrutinas con pila propia**, y no (A), invirtiendo lo que dije antes de mirar el código.

El argumento decisivo no es el rendimiento sino **el alcance del cambio**. (A) obliga a un análisis
nuevo de coloreado en el transpilador que se propaga por todo el programa generado e interactúa con
`try_call`, los actores y el pool; es un arco de meses con riesgo repartido por toda la superficie.
(B) concentra el riesgo en una pieza pequeña, fea y aislable —el cambio de contexto— y deja intacto
todo lo demás, incluido el código raylang de `net/webserver`, que no se entera. Y el nivel 1 del §3
sale casi gratis, porque el aparcado pasa a ser el mismo concepto que en la VM.

Y con los §3b y §3c medidos, la desventaja que le apuntaba a (B) —"no baja a cientos de bytes sino a
la pila mínima"— **se desinfla**: la pila mínima real es ≤ 8 KiB, no 32-64. La memoria por conexión
baja de 265 KB a **~30 KB (≈9×)**, y los hilos de 1002 a un puñado: 10 000 conexiones pasan de
~2.6 GB y 10 000 hilos a **~300 MB y ~11 hilos**. La diferencia que quedaría con (A) —cientos de
bytes contra 8 KiB de pila— ya no cambia el orden de magnitud del resultado, y no justifica su coste
en alcance.

### Antes de comprometerse: el experimento, ya hecho

El plan era un prototipo sintético (servidor echo con corrutinas sobre `poll.rs`) para confirmar el
orden de magnitud. **Se descarta por innecesario**: los §3b y §3c midieron las dos incógnitas sobre
el código real, que es mejor evidencia. Queda una sola decisión abierta antes del arco:

**La pieza de cambio de contexto**: a mano con `asm!` por arquitectura (aarch64 + x86_64), o una
dependencia acotada tipo `corosensei`. El proyecto ya acepta deps cuando lo merecen (ring, rusqlite,
regex), y aquí el argumento a favor es fuerte: el `unsafe` de cambio de contexto escrito a mano es
justo la clase de código donde un error no se manifiesta como excepción sino como corrupción de
memoria silenciosa.

> **DECISIÓN (28 jul 2026, confirmada por el usuario): opción (B) con `corosensei`.**

## 5. Plan del arco (fases; cada una compila y se commitea sola)

La palanca de integración ya existe: los subsistemas de `ray-runtime` van tras **features** y el
default de `ray build --native` es la vía Cargo. El scheduler de fibras entra como feature
**`fibers`** (con `dep:corosensei`): mientras el arco avanza, `--without fibers` — y la vía
`rustc` pelada, que no puede traer crates — conservan el modelo de hilo-por-conexión actual como
respaldo y como grupo de control para el A/B.

- **F1 — núcleo de fibras en `ray-runtime`** (sin tocar el transpilador). Scheduler M:N:
  corrutinas `corosensei` (pila `mmap` con página de guarda, reserva generosa, residencia por
  páginas tocadas), N workers = cores, cola de listas compartida, **reactor** con kqueue/epoll
  **persistente** (a diferencia de `src/poll.rs`, que crea y destruye el poller por llamada — aquí
  el patrón oneshot por interés) + tubería de despertar CLOEXEC, y temporizadores para `sleep`.
  API: `spawn` / `park_readable(fd)` / `park_writable(fd)` / `fiber_sleep` / `yield_now` /
  `in_fiber`, con la cesión profunda vía TLS del yielder (repuesto tras cada reanudación, porque
  una fibra puede despertar en otro worker). Tests del crate: masividad, E/S aparcada, panics.
- **F2 — E/S de sockets del transpilado sobre fibras**: sockets no-bloqueantes;
  `accept`/`read`/`write` aparcan la fibra en vez del hilo; `__ray_spawn` crea fibra cuando la
  feature está activa. Corpus verde; el `plaintext` del bench como humo.
- **F3 — primitivas de concurrencia sobre fibras**: canales, `join`/`try_join`, `scope` y
  cancelación, `sleep`. ⚠️ Los thread-locals del runtime emitido (`__RAY_CANCEL`, `__SCOPES`) hoy
  suponen hilo=tarea; con fibras que pueden reanudarse en otro worker, ese estado pasa a viajar
  **con la fibra**. `__RAY_RDBUF` sí puede seguir por hilo: no se cede con el préstamo vivo.
- **F4 — TLS (rustls) sobre fibras**: `StreamOwned` bloqueante → no-bloqueante con aparcado (el
  mismo esquema que ya usa la VM).
- **F5 — paridad, medición y default**: corpus byte a byte, suite de red completa, A/B
  fibras-contra-hilos en el bench web (memoria/hilos/rps/p99.9), y la decisión de encender
  `fibers` por defecto con `--without fibers` de escape.

Nota asumida: las operaciones que siguen siendo bloqueantes de verdad (ficheros, SQLite, DNS del
sistema) bloquean un worker mientras duran, igual que hoy bloquean un hilo. Es el mismo compromiso
que acepta la VM y no forma parte de este arco.
