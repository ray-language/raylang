# Investigación: el p99.9 del webserver nativo bajo carga sostenida

> Nota de alcance: investigación puntual con una propuesta al final. Continúa la línea de
> [`investigacion-p99-framework-web.md`](investigacion-p99-framework-web.md) y
> [`investigacion-overhead-framework-express.md`](investigacion-overhead-framework-express.md),
> pero con una diferencia importante de contexto: aquellas midieron raylang **contra sí mismo** en
> loopback; esta parte de una comparación **externa** (`benchmarks/web/`, generador en otra
> máquina) que aísla un rasgo concreto. Si la propuesta se implementa, su crónica va a
> `PERFORMANCE.md`.

## 1. El hallazgo que la motiva

`benchmarks/web/` (27 jul 2026, servidor M3 Pro ↔ generador M4 por Thunderbolt bridge, `-c 100`,
3 repeticiones por escalón) mide el escalón **pelado**: `net/webserver` compilado a nativo contra
Rust hyper, Go `net/http` y `node:http`. A 120 000 rps sostenidos:

| | p50 | p99 | p99 MAD | **p99.9** |
|---|---|---|---|---|
| raylang (`net/webserver`, nativo) | **0.65 ms** | **1.86 ms** | ±0.04 | **6.59 ms** |
| Go `net/http` | 0.79 ms | 2.11 ms | ±0.01 | **2.63 ms** |

raylang gana la mediana, la p99 (ventanas de mediana ± 2·MAD disjuntas) y el techo de throughput
(~129 500 contra ~120 400 rps). **Pierde la cola profunda por 2.5×**, y se reprodujo en las dos
sesiones remotas (7.14 y 6.59 ms) → es un rasgo, no ruido. Pregunta: ¿de dónde sale?

## 2. Método

`sample` de macOS (el mismo instrumento de las investigaciones anteriores) sobre el binario nativo
mientras el generador remoto lo satura a 120k rps, 18 s de muestreo. Luego, clasificación de los
198 bloques de hilo del `Call graph` por lo que está haciendo cada uno.

⚠️ **Salvedad del instrumento**: `sample` suspende hilos para recorrer pilas, y con 198 hilos eso
es caro — durante el muestreo el throughput cayó de 120k a 61k rps y la p99.9 subió a 12.4 ms. Así
que el perfil sirve para ver **la estructura** (cuántos hilos hay y dónde están), no para atribuir
tiempos absolutos. La estructura es lo que buscábamos.

## 3. Hallazgo — 198 hilos de SO para 100 conexiones, en 11 cores

| hilos | qué hacen |
|---|---|
| 101 | sirviendo una conexión: `handle_http` → `read_with_deadline` → `TcpStream::read` → `__recvfrom` (lectura **bloqueante**) |
| 96 | workers del pool **aparcados**: `mpmc::Channel::recv` → `park_timeout` → `_dispatch_semaphore_wait_slow` → `semaphore_timedwait_trap` |
| 1 | el hilo principal, bloqueado en `TcpListener::accept` |
| **198** | **total** |

Dos cosas, y la segunda es la interesante:

**(a) El backend nativo es thread-per-connection bloqueante.** Un hilo de SO por conexión viva,
bloqueado en `read`. La VM, del MISMO fuente, hace otra cosa completamente distinta: fibras M:N con
readiness por kqueue/epoll (`src/poll.rs`). No es una diferencia de constante, es de modelo.

**(b) Cada petición cruza DOS hilos de SO.** `handle_http` (`packages/net/webserver.ray:834-835`)
hace, dentro del bucle de keep-alive:

```raylang
let t = spawn(fn() -> Response { handler(req) });
match (try_join(t)) { ... }
```

En el nativo ese `spawn` va a `__RAY_POOL` → un hilo de SO real. Así que **por petición** hay: un
`send` a un canal, el despertar de un worker aparcado (semáforo → syscall), la ejecución del
handler en ESE hilo, el envío del resultado de vuelta, y el `join` del hilo de la conexión. Los 96
workers aparcados del perfil son precisamente esa maquinaria en reposo.

Y el `spawn` no está ahí por concurrencia — el comentario del propio código lo dice: es
**aislamiento de panic** (M56.5), para que un handler que revienta responda 500 sin tumbar el
servidor.

## 4. Por qué esto produce cola y no mediana

Es la firma exacta del síntoma. 198 hilos ejecutables sobre 11 cores: quien decide qué corre es el
scheduler del SO. En el caso común el worker despierta y encuentra core libre → la mediana no se
entera (0.65 ms, la mejor de las cuatro implementaciones). En la fracción de casos en que el
despertar coincide con los cores ocupados, la petición espera en la run queue del kernel — y eso
es justo lo que muestrean p99.9 y p99.99.

Los rivales no pagan este peaje:

- **Go** sirve la misma carga con ~11 hilos de SO y goroutines; el equivalente del handoff ocurre
  en espacio de usuario, sin syscall ni despertar de semáforo.
- **hyper** no tiene handoff en absoluto: el handler corre **inline** en el mismo worker de tokio
  que leyó el socket.

Coherente además con que raylang gane el throughput: el trabajo por petición es eficiente (de ahí
p50 y p99 mejores que Go); lo que se paga es la **varianza** de meter al scheduler del SO en el
camino de cada petición.

## 5. Propuesta — implementar `try_call` (M97.2) y quitar el `spawn` por petición

La solución **ya está diseñada en el backlog**: `IDEAS.md` §49, **M97.2**, con el plan fijado y
clasificado como "aditivo, no bloquea nada". Esta investigación no propone algo nuevo: le da a
M97.2 una **justificación de rendimiento** que no tenía (estaba planteada como ergonomía —
el "recover" del proyecto).

```
try_call(f: fn() -> T) -> Result<T, string>     // recuperación en la MISMA fibra, sin spawn
```

Y el cambio en `handle_http`:

```raylang
match (try_call(fn() -> Response { handler(req) })) { ... }   // en vez de spawn + try_join
```

Lo que desaparece, por petición: un `send` al canal del pool, un despertar de semáforo (syscall),
dos cambios de hilo y el `join`. Y el censo de hilos del §3 baja de ~198 a ~101 (los 96 workers
aparcados dejan de existir para este camino).

Por motor, según el plan de §49 —que ya contempla los tres— y por qué encaja aquí:

- **nativo**: el cuerpo bajo `catch_unwind`, con `__ray_rt_err` haciendo `panic!` en vez de
  `exit(70)` dentro de ese dynamic scope. Es exactamente el mecanismo que hace innecesario el hilo
  extra: misma garantía panic→500, cero handoff.
- **VM**: desenrollar los marcos de la fibra hasta un marcador. También gana (se ahorra un `spawn`
  de fibra por petición), aunque ahí el coste era mucho menor.
- **intérprete**: interceptar `Flow::Error`, trivial — y con esto el webserver gana **oráculo
  VM↔intérprete completo** en este camino, que hoy no tiene porque `spawn` no corre en el
  intérprete.

### Riesgos, ya identificados en §49

- **El sharp edge documentado**: `try_call` recupera con el heap de la propia fibra posiblemente a
  medio mutar (el mismo trade-off que `catch_unwind` de Rust). Para el webserver es aceptable y
  conviene ser explícito: tras recuperar se responde 500 y **se cierra la conexión** (que es lo que
  el código ya hace hoy), así que no se sigue usando estado sospechoso.
- **M97.4 (recursos huérfanos en unwind)**: §49 lo difiere "solo si 97.2 lo destapa como dolor
  real". En este caso concreto el único recurso en juego es el handle de la conexión, y lo posee el
  llamador (`handle_http`), no el handler — así que este caso de uso **no** lo destapa. Vale
  registrarlo como dato para esa decisión.
- **Paridad de mensajes**: el 500 y cualquier texto de error deben quedar byte-idénticos entre los
  tres motores; el corpus nativo (`tests/native_corpus.rs`) es el guardián, y la lección de la
  ronda M96f fue correrlo **antes** de medir rendimiento, no después.

### Implementado (27 jul 2026) — y qué quedó demostrado y qué no

M97.2 está hecho en los tres motores y `handle_http` ya usa `try_call`. Resultados, separando lo
confirmado de lo que sigue en el aire:

**✅ El mecanismo desapareció.** Censo de hilos de SO del mismo binario bajo la misma carga (120k
rps, `-c 100`, generador remoto): **198 → 97**. Los 96 workers del pool aparcados ya no existen para
este camino; queda un hilo por conexión más el `accept`, que es el ítem del §6.

**✅ La mediana mejora, medida y consistente.** A/B del binario ANTES vs DESPUÉS en loopback a 80k
rps, con los dos servidores vivos a la vez en puertos distintos y patrón ABBA×2 (8 corridas):

| | p50 | p99 | p99.9 |
|---|---|---|---|
| antes (`spawn`+`try_join`) | 0.73 · 0.73 · 0.74 · 0.74 | ~1.77 | 2.72–2.81 |
| después (`try_call`) | **0.62 · 0.63 · 0.62 · 0.63** | ~1.77 | 2.64–2.81 |

**p50 ~15 % mejor (0.735 → 0.625 ms)** con los rangos completamente disjuntos, 4 de 4 contra 4 de 4
— exactamente lo que se espera al quitar un handoff de hilo por petición. p99 y p99.9 quedan
idénticos **en este régimen**.

**❓ La hipótesis del p99.9 sigue SIN validar.** Y hay que decirlo claro: la cola profunda que motivó
todo esto (6.59 ms contra 2.63 de Go) apareció a **120k rps con el generador remoto**, y ese régimen
no se pudo volver a medir — el enlace Thunderbolt se cayó a mitad de la sesión (la IP del bridge
desapareció de las dos máquinas). En loopback a 80k la p99.9 ya era de 2.7 ms *antes* del cambio, o
sea que ese banco no tiene resolución para la señal que se busca. Lo que se sabe es que el mecanismo
identificado ya no está; **falta comprobar que la cola lo seguía**.

Pendiente inmediato, en cuanto vuelva el enlace:

```sh
cd benchmarks/web && ./webbench.py --bind 10.0.0.10 --generator-host <gen> -i ~/.ssh/id_bench \
                                  --only ray,go --rates 80000,120000,160000 --reps 5
```

Criterio: p99.9 de raylang a 120k hacia el entorno de Go (~2.6 ms) desde los 6.59 ms medidos, con
las ventanas de mediana ± 2·MAD como juez. Si NO baja, la conclusión honesta es que el handoff de
hilo era un coste real de mediana pero no el mecanismo de la cola, y el siguiente sospechoso es el
ítem del §6 (un hilo bloqueante por conexión).

**Efecto colateral del cambio, en el transpilador**: `handle_http` perdió su `spawn` y con él la
marca que hace viajar su parámetro-handler como genérico monomorfizado, mientras `loop_iter_server`
—que sí spawnea por conexión y le pasa el handler— seguía marcado → `expected Rc<dyn Fn…>, found
type parameter __F5`. Faltaba la propagación **hacia delante** de esas marcas (un `__F` no se
convierte solo a `Rc<dyn Fn>`), que además es lo semánticamente correcto: ese handler sigue cruzando
a la fibra de la conexión, así que necesita los mismos bounds. Añadida en
`src/transpile/analysis.rs`.

### Cómo validarlo

El banco ya está montado para esto, y es la primera vez que se puede validar contra un tercero en
vez de contra uno mismo:

```sh
cd benchmarks/web && ./webbench.py --bind 10.0.0.10 --generator-host <gen> -i ~/.ssh/id_bench \
                                  --only ray,go --rates 80000,120000,160000 --reps 5
```

Criterio de éxito: la p99.9 de raylang a 120k baja de ~6.6 ms hacia el entorno de Go (~2.6 ms),
con las ventanas de mediana ± 2·MAD como juez. Criterio de no-regresión: p50, p99 y techo no
empeoran (hoy raylang gana los tres).

## 6. El segundo ítem, más grande: thread-per-connection

Quitar el `spawn` por petición deja ~101 hilos (uno por conexión) donde Go usa ~11. Eso es el
hallazgo (a) del §3 y es una decisión de arquitectura del backend nativo, no un detalle: el mismo
fuente corre en la VM sobre fibras y kqueue, y en nativo sobre hilos bloqueantes. A concurrencias
mucho mayores que 100 esto volverá a aparecer, y con más fuerza.

No lo propongo ahora: es un arco, no un fix, y el §5 es la parte barata que ataca el mecanismo
medido. Pero conviene tenerlo escrito para no re-descubrirlo, y **medirlo antes de diseñarlo**:
repetir el banco con `-c 500` y `-c 1000` diría cuánto duele y a partir de dónde.

## 7. ¿Ayudaría hyper a raylang?

Pregunta legítima, porque hay precedente: el backend nativo ya usa **crates reales** para
subsistemas donde escribirlo en raylang no aportaba — `rustls` (TLS), `rusqlite` (SQLite), y sobre
todo el **crate `regex` como motor del nativo** (R5), con la implementación raylang de `std/regex`
como fallback y para la VM. Ese patrón funcionó y dejó el bench de regex nativo por delante de Go.

Mi lectura es que **hyper vale mucho más como instrumento de medida que como dependencia**, por
tres razones:

1. **La medición dice que no hace falta.** El problema no está en parsear HTTP ni en construir
   respuestas: en esas partes raylang ya bate a Go (p50 0.65 vs 0.79, p99 1.86 vs 2.11) y sostiene
   más throughput. Lo que falla es **una** decisión estructural — el handoff de hilo por petición —
   y quitarla cuesta un builtin ya planificado. Importar hyper para arreglar eso sería sustituir la
   casa por no arreglar una puerta.
2. **No es un drop-in como `regex`.** El crate `regex` es una función pura: entra un patrón, sale
   un match, y el contrato es fácil de igualar. hyper trae **tokio**, un runtime async completo, que
   tendría que coexistir con el modelo de concurrencia propio de raylang (`__RAY_POOL`, actores de
   heap aislado, fibras de la VM). Eso no es enlazar un subsistema, es tener dos schedulers en el
   mismo proceso.
3. **Rompería el espejo VM↔nativo en la superficie más visible.** `net/webserver` expone
   `Request`/`Response`/`Limits`/SSE/keep-alive/chunked/estáticos, y el proyecto exige salida
   **byte-idéntica** entre motores (`tests/native_corpus.rs`). Con hyper por debajo en nativo y
   raylang puro en la VM, cada detalle —orden y capitalización de cabeceras, framing, textos de
   error, comportamiento en timeout— pasa a ser una fuente de divergencia permanente. El caso de
   `regex` ya obligó a "validación raylang + dialecto traducido" para mantener la paridad; en HTTP
   esa superficie es un orden de magnitud mayor.

Donde sí lo veo defendible, como discusión aparte y con su propia medición: **HTTP/2**.
`packages/net/http2.ray` + `hpack.ray` son ~330 líneas a mano, y ahí hyper (con `h2`) sí aporta
algo que no es "lo mismo pero más rápido". Pero eso es una decisión de alcance de la stdlib, no una
respuesta a esta cola.

Y hay un uso de hyper que ya está dando valor y conviene mantener: **como techo del banco**. Es
quien nos dice cuánto hardware queda en la mesa (y quien, de paso, destapó que
`--latency-correction` fabricaba colas y que el generador topa antes que el servidor).
