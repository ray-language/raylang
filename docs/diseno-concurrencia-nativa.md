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
que acepta la VM y no forma parte de este arco. (Post-arco, jul 2026: para el FFI de **usuario** —
llamadas C arbitrarias — ese compromiso ya no se impone: `extern "lib" blocking { … }` descarga la
llamada a un pool bloqueante vía `fibers::run_blocking` y la fibra espera aparcada; DESIGN §90.)

## 6. F2, EJECUTADA (28 jul 2026) — resultados y lecciones

`ray build --native --fibers` funciona de punta a punta. Paridad: los **7 tests dedicados**
(`tests/native_fibers_cli.rs`: spawn/join, canales acotados y rendezvous, scope+cancelación,
try_call en fibras, sleep, servidor TCP auto-hablándose, read-timeout) y el **corpus completo**
(~50 programas, `native_corpus -- --ignored`, variante `--fibers`) son **byte-idénticos a la VM**.
La vía sin `--fibers` no cambia (corpus plano verde).

Medido sobre el `plaintext` del bench, A/B intercalado ×3, misma sesión, `-c 1000` loopback:

| | hilos | RSS (mimalloc) | RSS (slim) | rps |
|---|---|---|---|---|
| hilo-por-conexión | **1002** | 239 MB | ~147 MB | ~106.4k |
| **fibras (--fibers)** | **14** | ~209 MB | **~110 MB** | ~93k (−13 %) |

- **El muro cayó**: 14 hilos constantes (idle: 2 MB de RSS). El objetivo estructural del arco.
- El RSS restante era **retención de churn del asignador** (~100-200 B/petición retenidos; `leaks`
  reportaba 0 fugas; `vmmap` lo situaba en las arenas) — RESUELTO en F5, ver §7.
- TLS sigue bloqueante con fibras (F4); UDP también. `connect` bloqueante (acotado por el SO).

**Dos lecciones de corrección que redefinen el diseño (pagadas con sangre de release-only):**

1. **Las fibras quedan FIJADAS a su worker (sin migración, sin robo de trabajo).** Con opt3+LTO,
   LLVM cachea la dirección de un thread-local a través del cambio de contexto (el asm de
   corosensei no le dice que el hilo puede cambiar): una fibra que migraba escribía en los TLS del
   worker ANTIGUO — yielder nulo, `RefCell already borrowed` en `__RAY_RDBUF`, `in_try` con
   overflow, SIGBUS. Solo en release; el test de estrés de la vía del reactor lo caza. Es el
   hazard clásico de las corrutinas stackful compiladas (Go lo evita porque su compilador conoce
   los puntos de cesión). La fijación lo elimina de raíz: cualquier TLS cacheado sigue siendo del
   hilo correcto. Consecuencia: **el robo de trabajo queda PROHIBIDO, no pendiente.**
2. **Con fijación, ninguna espera dentro de una fibra puede bloquear el hilo**: una hermana fijada
   al mismo worker no correría jamás (interbloqueo). Todas las esperas del runtime emitido pasan
   por `__ray_cv_wait`, que con fibras suelta el lock, CEDE el turno y re-toma (F3 lo hará por
   lista de esperas, sin ceder en bucle); el `JoinHandle::join` del crate igual. Y las syscalls
   bloqueantes en fibra deben ser ACOTADAS (la lección del livelock de connects > backlog).

Y una del reactor: el read-timeout del webserver (10 s) aparca CADA lectura con deadline; el
readiness casi siempre gana y el temporizador huérfano vivía 10 s — a 100k rps, ~1M de entradas y
un barrido O(n) por ciclo. Ahora: min-heap + cancelación explícita + compactación (memoria
O(vivos), siguiente-plazo O(log n)).

## 7. F5, EJECUTADA (28 jul 2026) — el reactor a cero asignaciones: fibras gana TODO

La "retención del asignador" del §6 tenía una causa concreta y era NUESTRA: el reactor asignaba
**cuatro buffers O(fds-aparcados) POR CICLO** (changelist, eventlist y las dos listas de interés)
— a `-c 1000` y ~15k ciclos/s, ~500 MB/s de churn de asignación cuya marca de agua quedaba
retenida (meseta de 163-217 MB). F5 lo lleva a **cero asignaciones por ciclo en régimen**:

1. **Armado incremental**: solo se registran los parks nuevos; los oneshot no disparados siguen
   armados en el kqueue/epoll. El `poke` del close (byte etiquetado en la tubería) dispara el
   único re-armado global — O(n) solo en closes, donde el diseño anterior lo pagaba SIEMPRE.
2. **Buffers persistentes** en el Poller y buzón por swap (ping-pong, sin frees cruzados de
   hilos); un byte de despertar por lote, no por op.
3. **Entradas de `fds` persistentes** (capacidad reutilizada; acotado por el pico de fds vivos).
4. **Despertares agrupados por worker** (un lock por cola y ciclo).

Medido en loopback (`plaintext`, A/B intercalado ×3, `-c 1000`, misma sesión):

| | hilos | RSS bajo carga | RSS tras carga | rps |
|---|---|---|---|---|
| hilo-por-conexión | 1002 | 239 MB | 239 (retiene) | ~105.7k |
| fibras F2 | 14 | ~217 MB | ~217 (retiene) | ~100.1k |
| **fibras F5** | **14** | **23 MB** | **8 MB** | **~112k** |

72× menos hilos, 10× menos memoria bajo carga que el modelo de hilos — 2× menos que Go (49 MB),
1.7× menos que la VM (40 MB) —, +6 % de rps, y la memoria **vuelve** al cerrar. Los ~23 MB bajo
carga son ~21 KB/conexión con todo incluido, clavado en la estimación del §3c.

### §7b — Validación en RED REAL (generador remoto Mac mini M4 por Thunderbolt)

Corrida oficial del banco (`webbench.py`, plaintext, `-c 100`, 10 s × 3 reps/escalón, escalera
hasta 280k, SLO p99 ≤ 10 ms, enlace verificado sano antes y después; `ray-fib` entra al banco
como implementación permanente — el MISMO `main.ray` compilado con `--fibers`):

| | sostenida bajo SLO | techo | p50 | p99 | p99.9 |
|---|---|---|---|---|---|
| hyper | 200k (líder) | **~206k** (por fin visible) | 0.45 | 0.73 | 0.96 |
| **ray-fib** | **160k** | **~190.5k** | **0.47** | 0.73 | 1.05 |
| ray (hilos) | 160k | ~164k | 0.59 | 0.73 | 1.17 |
| go | 120k | ~121.5k | 0.77 | 2.16 | 2.71 |
| node | 40k | ~60k | 0.72 | 1.73 | 2.68 |

En red real las lecturas aparcan DE VERDAD (en loopback los datos solían estar ya en el búfer) —
y aun así **fibras supera al modelo de hilos en todo**: techo +16 % (190.5k contra 164k; el
sostenido empata a 160k porque el siguiente escalón de la escalera era 200k), p50 −20 %, p99.9
−10 %, sirviendo 160k reales con **14 hilos / 8 MB** contra 102 hilos / 24 MB del modelo de hilos
(a `-c 100`; el muro de 1002 hilos aparece a `-c 1000`). Contra Go: **1.57× su techo**; contra
hyper: **92 % de su techo**.

Gotcha de banco pagado esta semana: una corrida con el generador DORMIBLE no vale — el Mac mini
entró en reposo a mitad de la primera escalera y `ray-fib`/`go` dieron techos falsos (32k/90k)
mientras el resto salía normal (la rotación repartió el daño de forma desigual); con el reposo
desactivado volvieron a 190k/125k. El protocolo incorpora: desactivar el reposo del generador y
verificar el enlace con ping antes y después de cada corrida.

Pendiente del arco: F3 (esperas de fibra por lista — los canales calientes no están en el hot
path del webserver, no mueve este bench), **F4 (TLS/UDP sobre fibras)**, y el DEFAULT: con estos
números la única razón para no encender `fibers` por defecto es que TLS/UDP siguen bloqueantes →
primero F4, luego el default.

## 8. F4, EJECUTADA (28 jul 2026) — TLS y UDP sobre fibras: el arco queda completo

Lo que faltaba para que `--fibers` cubra TODA la superficie de red:

- **TLS** (`ray_runtime::tls`): variantes `read_wait`/`write_all_wait` (feature `fibers`) que en
  `WouldBlock` esperan readiness y reintentan — en fibra APARCAN, fuera hacen poll(2). La
  **dirección** de la espera sale de la sesión rustls (`wants_write`): el handshake alterna
  lecturas y escrituras, y aparcar por lectura cuando toca escribir interbloquearía. El timeout de
  lectura (M56.4) va por el mismo camino y vence con `"read timeout"` byte-idéntico a la VM. Los
  sockets de `connect`/`connect_h2` pasan a no-bloqueantes al crearse (los de accept/upgrade ya lo
  eran por F2); el handshake eager de `connect_h2` (ALPN) sigue bloqueante-acotado.
- **La lectura TLS aparca DENTRO del despacho** → dos reglas nuevas con dientes: el búfer no puede
  ser `__RAY_RDBUF` (su préstamo no cruza la cesión: otra fibra del MISMO worker lo pediría y
  reventaría el RefCell) → va en el ctx (`tls_buf`, `mem::take` antes / restaurar después; cero
  asignaciones en régimen); y el `MutexGuard` de la sesión SÍ cruza el park — sólido SOLO por la
  fijación (el guard nunca cambia de hilo) y porque la sesión es fiber-privada.
- **La caché "¿es TLS este handle?" viaja en el ctx** (como la de sockets: el estado por-conexión
  muere con su fibra dueña, no con un hilo).
- **UDP**: bind no-bloqueante; `recv_from` aparca la fibra hasta el datagrama (la cesión M20.11 de
  la VM); `send_to` aparca en el raro búfer-lleno.

Paridad: 9/9 en `native_fibers_cli` (los 2 nuevos: TLS self-talk — handshake+I/O aparcando por
ambos lados, cliente en main por la vía poll — y ping-pong UDP) y los DOS corpus completos verdes
(plano y `--fibers`, ahora serializados: compilan los mismos ejemplos contra la misma caché Cargo
y en paralelo se pisaban el artefacto — carrera preexistente anotada en IDEAS §54).

**Con F4, la única fase restante es F3 (esperas por lista, optimización sin efecto en el bench) y
la DECISIÓN DE DEFAULT queda desbloqueada**: no queda ninguna superficie de red donde `--fibers`
se comporte peor que el modelo de hilos.

## 9. DEFAULT activado (28 jul 2026) — el arco se cierra

Con F4 completa y el banco en red real a favor en todas las métricas, **`fibers` es el default de
`ray build --native`** (decisión del usuario): la feature va siempre-on como mimalloc/aHash,
`--without fibers` recupera el hilo-por-tarea (que sigue soportado, con su corpus propio como
respaldo y única vía en Windows, donde el reactor kqueue/epoll no existe y el modo se apaga solo
con aviso). `--fibers` se acepta por compatibilidad; combinarlo con el escape es error. El banco
conserva ambos modelos (`ray` = default/fibras, `ray-thr` = respaldo) mientras convivan.

Queda F3 (esperas de fibra por lista de esperas, sin ceder en bucle) como optimización de fondo
sin efecto en el bench del webserver, y los pendientes menores anotados: connect no-bloqueante
del cliente, pool de pilas de corrutina, sharding del buzón del reactor si el bench lo pide.

## 10. Post-cierre: el FRAMEWORK con el default nuevo (28 jul 2026, escalón json en red real)

La incógnita que dejó el arco: el framework con hilos tenía el techo CLAVADO en el del pelado
(165.6k vs 165.5k — "el cuello está debajo de la capa de framework"). Con fibras por default:

| json (framework) | sostenida | techo | p50 | p99.9 |
|---|---|---|---|---|
| axum | 200k (líder) | **~202k** (por fin visible) | 0.47 | 1.04 |
| **ray (fibras)** | **160k** | **~188k** | **0.48** | **1.05** |
| ray-thr (hilos) | 160k | ~161.6k | 0.60 | 1.17 |
| chi | 120k | ~124.8k | 0.65 | 2.77 |
| express | 40k | ~40k | 2.44 | 5.28 |

- **El techo del framework SIGUIÓ al del pelado**: 161.6k (hilos) → 188k (fibras), +16 % — y queda
  a ~1 % del plaintext (190.5k). El diagnóstico "el cuello está debajo" era correcto y el arco lo
  quitó: la capa de framework sigue costando ~0-1 %.
- **Contra axum (el framework de referencia de Rust): 93 % de su techo**, con p50 y p99.9 en
  EMPATE estadístico (0.48/1.05 contra 0.47/1.04). Contra chi (Go): **1.51×**.
- El techo de axum por fin es visible (~202k, falla el escalón de 240k) — casi idéntico al de
  hyper pelado (~206k): en Rust la capa axum también sale ~gratis.
- `ray-thr` como grupo de control interno reprodujo el techo histórico del modelo de hilos
  (161.6k ≈ 160-165k de las corridas previas): la sesión es comparable.

## 11. F3, EJECUTADA (28 jul 2026) — esperas por lista: el arco queda SIN interinos

La última pieza: las esperas de condición de una fibra (canales, `join`, `select`, salida de
scope) dejan el interino de F2 (soltar-ceder-retomar en bucle, que quemaba un worker por
esperador ocioso) y pasan a una **lista de esperas** (`WaitList` en `ray_runtime::fibers`):

- Protocolo anti despertar-perdido: el esperador lee la GENERACIÓN con el lock de su condición
  tomado (`prepare`), lo suelta y suspende; el worker, al registrar, re-lee la generación — si
  cambió en la ventana, re-encola en vez de dormir. `wake_all` avanza la generación y encola cada
  fibra en su worker de origen.
- **Cancelación (H21-N3) intacta**: cada espera lleva un pulso de 10 ms (temporizador del
  reactor) — la MISMA cadencia con la que el modelo de hilos notaba la cancelación vía
  `wait_timeout(10ms)` — pero con la fibra aparcada de verdad entre pulsos.
- El runtime emitido unifica los sitios (send/recv/close/wait/select/scope) sobre una tríada
  `__RaySync<T>` = (Mutex, Condvar, WaitList) con helpers `__ray_cv_wait`/`__ray_notify`: las
  cadenas de los sitios son IDÉNTICAS en ambos modos; solo cambian los helpers y el alias. El
  hilo `main` sigue esperando por la condvar (ambas vías se notifican).

Medido (6 fibras esperando canales ociosos durante 3 s):

| | CPU (user+sys) |
|---|---|
| F2 (ceder-en-bucle) | 14.0 s |
| **F3 (lista de esperas)** | **0.04 s** (~350×) |

El webserver no se mueve (A/B intercalado: ~111k rps, 8 MB — idéntico), como se predijo: sus
canales no están en el hot path. Paridad intacta: 9/9 de fibras, AMBOS corpus, cli_cli 94/94.

**El arco no tiene ya piezas interinas.** Pendientes menores (no bloqueantes): connect
no-bloqueante del cliente, pool de pilas de corrutina, sharding del buzón del reactor.

## 12. Windows (M182, sep 2026) — el reactor `WSAPoll` y el límite de corosensei

El port a Windows (`docs/windows.md` 3.6, DESIGN §174) no cambió el modelo: el scheduler es de
*readiness* y la forma que encaja en Windows es `WSAPoll` — persistente sobre la lista de
intereses armados, oneshot al dispararse, con un socket UDP conectado a sí mismo como tubería
de despertar. IOCP (completion) habría exigido I/O solapada en todo el runtime emitido. Lo que
no es socket (pipes de procesos, consola, watch) va al pool bloqueante (`run_blocking`) con la
fibra aparcada.

El límite es de corosensei: sin backend para AArch64-Windows, las fibras son solo x86_64 en
Windows; en ARM64 `ray build` cae al hilo-por-tarea con el motivo en el aviso. Verificado
cross-compilando desde una VM ARM64 y ejecutando bajo la emulación x64 de Windows 11 (salida
byte-idéntica a la VM en el servidor TCP que se habla a sí mismo y en los canales); el runner
x86_64 de CI corre `tests/native_fibers_cli.rs`.
