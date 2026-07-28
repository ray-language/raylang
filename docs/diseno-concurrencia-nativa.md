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

### Lo que el prototipo sintético todavía aportaría (y lo que ya no)

Ya **no** hace falta para saber si el modelo de fibras baja la memoria: está medido, 11.5×. Queda
una sola incógnita propia de la opción (B): **cuánta pila TOCA de verdad una corrutina**, que es lo
que se sumaría a esos 23 KB. Hay una forma barata de acotarlo sin escribir corrutinas: bajar la pila
de los hilos actuales a 32 KiB y 16 KiB y ver a partir de qué tamaño el binario revienta — eso da
directamente cuánta pila usa el código de `net/webserver`. Con la hipótesis 1 ya se probó que a
128 KiB funciona sin cambiar el RSS, así que el techo está por debajo de eso.

Estimación resultante para (B): **~23 KB + pila tocada**, probablemente 30-40 KB por conexión. 10 000
conexiones pasarían de ~2.6 GB y 10 000 hilos a ~300-400 MB y un puñado de hilos.

## 4. Recomendación

**(B), corrutinas con pila propia**, y no (A), invirtiendo lo que dije antes de mirar el código.

El argumento decisivo no es el rendimiento sino **el alcance del cambio**. (A) obliga a un análisis
nuevo de coloreado en el transpilador que se propaga por todo el programa generado e interactúa con
`try_call`, los actores y el pool; es un arco de meses con riesgo repartido por toda la superficie.
(B) concentra el riesgo en una pieza pequeña, fea y aislable —el cambio de contexto— y deja intacto
todo lo demás, incluido el código raylang de `net/webserver`, que no se entera. Y el nivel 1 del §3
sale casi gratis, porque el aparcado pasa a ser el mismo concepto que en la VM.

La memoria estimada baja de 265 KB a ~32-64 KiB por conexión (**4-8×**), y los hilos de 1002 a un
puñado. No es el orden de magnitud de (A), pero **quita el muro**: 10 000 conexiones pasan de ~2.6 GB
y 10 000 hilos a ~300-600 MB y ~11 hilos.

### Antes de comprometerse: un experimento que decide

Estimar no basta, y el precedente de esta misma semana lo demuestra (la hipótesis de la pila se
falsó en 20 minutos). Antes del arco completo:

1. **Prototipo mínimo fuera de raylang**: un servidor echo en Rust con corrutinas de pila propia
   sobre `poll.rs`, 1000 conexiones, midiendo hilos y RSS. Confirma o refuta el orden de magnitud
   estimado sin tocar el transpilador.
2. Si confirma, **decidir la pieza de cambio de contexto**: a mano con `asm!` por arquitectura, o
   una dependencia acotada. El proyecto ya acepta deps cuando lo merecen (ring, rusqlite, regex),
   y aquí el argumento a favor es fuerte: el `unsafe` de cambio de contexto escrito a mano es
   justo la clase de código donde un error no se manifiesta como excepción sino como corrupción.

Solo después, el arco sobre el runtime nativo.
