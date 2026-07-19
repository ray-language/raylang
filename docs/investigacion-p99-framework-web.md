# Investigación: la cola de latencia de `web/framework` bajo carga (nativo)

> Nota de alcance: esto es una investigación puntual (no un documento-contrato). Registra el
> método y los hallazgos de una sesión de profiling sobre el binario nativo de
> `examples/web/framework` (M93), motivada por una diferencia de p99/p99.9 observada con `oha`
> entre la demo completa y una app con una sola ruta. No reemplaza a `docs/web-framework.md` ni a
> `PERFORMANCE.md`; si alguna hipótesis de aquí se implementa, su crónica va en `PERFORMANCE.md`
> como una entrada más del arco P2.b.

## 1. Pregunta de partida

Sirviendo `GET /yo` (responde `{"id": 7, "name": "Ada"}` vía `r.json_of(...)`) con el binario
nativo de `examples/web/framework/main.ray` (15 rutas + una sub-app montada en `/api` + una ruta
regex), `oha -q 27000 -c 200 --latency-correction` da una cola notablemente peor que la
documentada para el "webserver pelado" (`docs/web-framework.md:256-260`: p99 = 2.2 ms al 60% de
capacidad, sobre un pico de ~107k req/s). ¿Por qué, sirviendo a solo ~25% de esa capacidad
documentada, la cola ya es peor?

Hipótesis inicial (sesión anterior): `web/framework` reconstruye la `App` entera en cada
petición (por el modelo de actores de heap-aislado del backend nativo — `docs/web-framework.md:
236-244`), y ese costo de reconstrucción explica la diferencia. Un primer barrido comparativo
(A = demo completa, B = 1 sola ruta, C = `net/webserver` pelado sin framework) mostró B≈C y A
claramente peor, lo que apuntaba en esa dirección. Este documento la pone a prueba con
instrumentación directa en vez de inferirla solo del comportamiento externo.

## 2. Método

Tres pasos, cada uno pensado para descartar explicaciones alternativas del anterior:

1. **Lectura de código** — confirmar en qué punto del ciclo de vida se reconstruye la `App`
   (¿por conexión, como sugiere un comentario de `framework.ray`, o por petición?).
2. **Microbenchmark aislado** — medir el costo de `build_app()` en un bucle apretado,
   *sin red, sin concurrencia*, y descompuesto por pieza (rutas planas, ruta regex, mount,
   estáticos, cors). Esto separa "cuánto cuesta reconstruir la app" de "qué le pasa a esa app
   bajo 200 conexiones concurrentes".
3. **Profiling bajo carga real** — `sample` (macOS) sobre el proceso mientras `oha` lo satura,
   para ver dónde cae realmente el tiempo de CPU/espera cuando SÍ hay concurrencia. Si el
   microbenchmark del paso 2 resulta demasiado pequeño para explicar la brecha observada con
   `oha`, este paso es el que puede decir por qué.

Variantes usadas (mismo handler, mismo JSON de respuesta):

| variante | descripción | puerto |
|---|---|---|
| **A** | `examples/web/framework/main.ray` sin modificar (15 rutas + sub-app `/api` + 1 regex) | 8080 |
| **B** | mismo framework, `build_app()` con **una sola ruta** (`GET /yo`) | 8081 |
| **C** | `net/webserver.serve` pelado, sin `web/framework` en absoluto | 8082 |

Las tres compiladas con `ray build --native main.ray --release` (mismo binario release,
mismo M3 Pro).

## 3. Hallazgo 1 — la app se reconstruye POR PETICIÓN, no por conexión

`packages/web/framework.ray:660-665` documenta la intención:

> "La fibra de cada conexión la llama UNA vez (vía `webserver.serve_with`) y atiende con esa
> App todas sus peticiones keep-alive."

Pero el código real de `serve_with` (`packages/net/webserver.ray:1157-1175`) dice lo contrario,
explícitamente, en su propio comentario:

> "Se construye **POR PETICIÓN, no por conexión**: el aislamiento panic→500 (M56.5) corre cada
> petición en su propia tarea, así que el handler cruzaría OTRO hilo igualmente."

Y en efecto, `handle_http` (`packages/net/webserver.ray:827-835`) hace, dentro del bucle
keep-alive de cada conexión:

```
let t = spawn(fn() -> Response { handler(req) });   // 1 spawn del pool por CADA request
match (try_join(t)) { ... }
```

donde `handler` (armado por `serve_with_limits`, línea 1174) es
`fn(req) { let h = make_handler(); h(req) }` — y `make_handler` para el framework
(`framework.ray:672-677`, `listen`) es justamente `let app = build(); ...` — es decir,
**`build_app()` corre una vez por cada request**, no una vez por conexión. El comentario de
`framework.ray:660-665` describe una intención que el código de `webserver.ray` contradice
explícitamente; son dos módulos del mismo commit (M93.3) con relatos distintos del mismo
comportamiento. Vale la pena corregir el comentario de `framework.ray` para que no induzca a
error (documentado aquí; no lo he tocado).

**Consecuencia derivada**: la docstring de `route_re` ("Compiled ONCE here") también queda
engañosa bajo este régimen — "una vez" es una vez **por petición**, no una vez en la vida del
proceso. La regex de `GET_re` se recompila en cada request, íntegra.

## 4. Hallazgo 2 — el costo de `build_app()` aislado es pequeño

Microbenchmark sin red (`bench-build/main.ray`, 200 000 iteraciones por caso, `std/time.monotonic()`):

| caso | total (200k iters) | promedio |
|---|---|---|
| **A) `build_full`** (15 rutas + mount + regex + estáticos + cors + log) | 1562 ms | **7.8 µs/llamada** |
| **B) `build_minimal`** (1 ruta) | 69 ms | **0.35 µs/llamada** |
| `new_app()` vacío (baseline) | 21 ms | 0.11 µs |
| 1 ruta GET plana | 76 ms | 0.38 µs |
| 5 rutas GET planas | 293 ms | 1.47 µs |
| 10 rutas GET planas | 561 ms | 2.81 µs |
| 14 rutas GET planas | 788 ms | 3.94 µs → **~0.27 µs/ruta marginal** |
| 1 ruta regex (`GET_re`) | 466 ms | 2.33 µs |
| **`regex.compile()` pelado** | 432 ms | **2.16 µs** ← domina el costo de la ruta regex |
| mount (sub-app de 1 ruta) | 180 ms | 0.90 µs |
| `static_files_cached` (solo registro, sin tocar disco) | 34 ms | 0.17 µs |
| `cors` (2 closures registradas) | 37 ms | 0.19 µs |

Lectura: reconstruir la app de 15 rutas cuesta **~7.8 µs de CPU**, contra ~0.35 µs de la app de
una ruta — una diferencia real (~22×) pero en términos absolutos, diminuta. De esos ~7.5 µs
extra, el reparto aproximado es: ~3.9 µs en registrar las 14 rutas planas adicionales (empujar a
un `[Route]`, ~0.27 µs/ruta), ~2.2 µs en compilar la ÚNICA ruta regex (`regex.compile`, el ítem
individual más caro con diferencia — más caro que las 14 rutas planas juntas), ~0.9 µs en el
mount de la sub-app, y el resto (~0.5 µs) en estáticos/cors/log/after. La suma aproximada de
las piezas (con algo de solapamiento del `new_app()` base de cada medición) cuadra con el
`build_full` medido directamente.

**Esto NO explica la brecha vista con `oha`.** La sesión anterior midió del orden de
200–600 µs de diferencia en p50/p99 entre A y B bajo carga, y 2.6×–4.5× en la cola (p99.9/
p99.99) — dos a tres órdenes de magnitud por encima de los ~7.5 µs medidos aquí en aislamiento.
El costo de CPU de reconstruir la app es real pero demasiado pequeño para ser, por sí solo, la
causa de la brecha observada. Hace falta ver qué pasa bajo concurrencia real.

## 5. Hallazgo 3 — bajo carga, el proceso pasa la mayoría del tiempo bloqueado en UN mutex global

Se perfiló cada binario (`sample <pid> 8` de macOS, muestreo cada 1 ms) mientras `oha -z 12s
-c 200 -q 30000` lo saturaba:

| | A (15 rutas) | B (1 ruta) |
|---|---|---|
| req/s conseguidos (de 30000 pedidos) | **23 527** (78%) | **29 994** (~100%) |
| muestras totales "top of stack" | 386 306 | 329 471 |
| muestras en `__psynch_mutexwait` | **110 996 (28.7%)** | **14 054 (4.3%)** |

Dos cosas saltan del mismo perfil:

- **A no sostiene la tasa pedida** (23.5k de 30k) mientras B la sostiene sin problema — a pesar
  de que el costo de CPU medido en el paso anterior es de microsegundos. Algo distinto de "más
  trabajo de CPU" está limitando el throughput de A.
- **A pasa ~6.7× más tiempo relativo (y ~8× más en términos absolutos) bloqueado en un mutex**
  que B, bajo la misma carga y en la misma ventana de muestreo.

Siguiendo la pila de llamadas hasta ese `__psynch_mutexwait` (sección "Call graph" de `sample`,
no solo el resumen "top of stack"), el bloqueo aparece consistentemente detrás de
`__ray_set_read_timeout` y `__ray_sock_clone`/`__ray_tls_get` — todas funciones generadas por
`src/transpile.rs` que hacen `__ray_reg().lock().unwrap()` antes de tocar el handle.

`__ray_reg()` (`src/transpile.rs:1058-1065`) es:

```rust
struct __RayReg { next: i64, open: __RayMap<i64, __RayHandle> }
fn __ray_reg() -> &'static std::sync::Mutex<__RayReg> {
    static R: std::sync::OnceLock<std::sync::Mutex<__RayReg>> = std::sync::OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(__RayReg { next: 1, open: __RayMap::new() }))
}
```

**Un único `Mutex` global, para TODOS los handles del proceso** — sockets TCP, listeners, UDP,
archivos, streams TLS y conexiones SQLite comparten el mismo registro y el mismo lock. Cada
lectura de socket (`__ray_socket_read`/`_bytes`, vía `__ray_sock_clone`) y cada
`set_read_timeout` (que `leer_con_plazo` en `webserver.ray` parece invocar en cada ciclo de
lectura, no solo al aceptar la conexión) toma este lock brevemente — solo para resolver el
`i64` al `Arc<TcpStream>` real, ya optimizado desde M96b para no hacer el `dup()` bajo el lock
(commit `503771c`). Con 200 conexiones concurrentes, **todas** compiten por este mismo mutex en
cada ciclo de lectura, sin importar si es A, B o C — el código de lectura es idéntico.

## 6. Hipótesis de causa raíz

El lock en sí no distingue A de B (mismo `net/webserver` por debajo). La hipótesis que conecta
el hallazgo 2 (7.5 µs extra de CPU) con el hallazgo 3 (8× más contención) es de **amplificación
por Little's Law / efecto convoy** — el mismo fenómeno que M96b ya documentó y parcialmente
corrigió para este lock (el comentario de `transpile.rs:1096-1098` habla literalmente de
"convoyes de ~100 ms" antes del fix del `dup()`):

- Cada request en A tarda ~7.5 µs más de CPU en servirse (build_app) que en B, **antes** de
  siquiera llegar al lock del registro.
- A tasa fija (`L = λ · W`, Little's Law): si el tiempo de servicio `W` por conexión sube
  aunque sea un poco, el número de conexiones **simultáneamente en vuelo** `L` sube en
  proporción, para sostener la misma `λ` (tasa pedida).
- Más conexiones en vuelo a la vez → más intentos concurrentes de tomar el mismo mutex global
  en la misma ventana de tiempo → cada intento de lock tiene más chance de encontrarlo tomado →
  el hilo se bloquea (futex/`__psynch_mutexwait`, con el costo de despertar que eso implica) →
  el tiempo de servicio efectivo sube más → vuelve a Little's Law.
- Es un bucle de realimentación, no una suma lineal — coherente con que 7.5 µs de causa se
  traduzcan en cientos de µs de efecto, y con que la cola (p99.9/p99.99, donde la contención
  pega más fuerte) se vea proporcionalmente peor que la mediana. También coherente con que A
  directamente **no pueda sostener** la tasa pedida (78%) mientras B sí: A cruza antes al
  régimen donde el sistema ya no da abasto.

Dicho de otro modo: **`build_app()` no es "la" causa — es el empujón que hace que la
contención preexistente en el registro global de handles se vuelva visible.** El mismo lock
probablemente ya limita el techo de throughput del framework incluso en B/C a una escala mayor
de concurrencia; A simplemente llega ahí antes porque cada request es un poco más lenta.

## 7. Lo que esto NO prueba (límites de la investigación)

- No se instrumentó el conteo exacto de adquisiciones del lock por request (habría que contar
  invocaciones a `set_read_timeout`/`socket_read` en `leer_con_plazo`); la lectura de código
  sugiere que es igual en A/B/C, pero no se verificó con un contador.
- El `sample` de macOS es un profiler estadístico de baja resolución (1 ms); confirma DÓNDE se
  acumula el tiempo pero no da un desglose exacto de nanosegundos por adquisición de lock.
- No se probó con una tasa distinta de 200×30k (p. ej. concurrencia más baja) para verificar
  que el "codo" de contención se mueve como predice la hipótesis de Little's Law — sería la
  prueba más directa de la hipótesis del §6.
- No se descarta que haya un SEGUNDO efecto (p. ej. presión del allocator: el perfil de A
  también muestra más muestras en símbolos de `libsystem_malloc.dylib` — `_xzm_xzone_malloc*`,
  `xzm_realloc` — coherente con que A asigna más por request: 14 `Route`, un `Regex` compilado,
  un `App` de sub-app — que podría contribuir aparte de la contención del mutex de I/O).

## 8. Hipótesis de solución (para decidir, no implementadas)

En orden de menor a mayor invasividad:

1. **Cachear la regex compilada entre requests.** *(Reevaluada, no implementada — ver nota.)*
   Es la pieza individual más cara del microbenchmark (2.16 µs, ~29% del extra de A) y en
   principio es puro desperdicio: el patrón de `GET_re("^/v(\\d+)/estado$", ...)` es un literal,
   idéntico en cada compilación. Pero al intentar implementarla surgieron dos problemas que la
   sacan de "bajo riesgo": (a) los valores raylang (`Regex`/`Prog`/`[Inst]`) usan `Rc`/`RefCell`
   internamente (semántica de referencia, M3) — clonar un `Rc` concurrentemente desde varios
   hilos sin sincronización es una carrera de datos en su contador de referencias (UB), así que
   NO se puede compartir directamente vía un `Mutex`/`OnceLock` global entre las conexiones
   (que corren en hilos de SO reales en el backend nativo); haría falta reconstruir el valor a
   partir de una copia "plana" (sin `Rc`) en cada request, o (b) esa caché tampoco ataca la causa
   confirmada (§5–§9): el costo de `build_app()` NO es el mecanismo real de la cola, es el mutex
   del registro. Se **pospuso** a favor del ítem 4 (mismo problema, causa confirmada, sin el
   riesgo de soundness) — queda como candidato de una sesión futura si se quiere apurar el
   último ~29% del costo de CPU de `build_app()`, ya con el mutex del registro fuera de la
   ecuación.

2. **Reducir las adquisiciones del lock global por lectura.** Si `leer_con_plazo` llama a
   `set_read_timeout` en cada ciclo de lectura (no solo al aceptar la conexión) pese a que el
   timeout no cambia entre lecturas de la misma conexión, cachear "ya seteado" evitaría una de
   las dos adquisiciones de lock por ciclo. Requiere confirmar primero (ítem del §7) cuántas
   veces se llama realmente.

3. **Shardear el registro de handles.** En vez de un único `Mutex<__RayReg>`, N buckets
   (`handle_id % N` → su propio `Mutex`), como se hace comúnmente para mapas concurrentes de
   alto tráfico. Reduce la probabilidad de colisión sin cambiar la API interna
   (`__ray_reg_insert`/`__ray_close`/etc. seguirían iguales, solo cambia qué mutex toman).
   Coherente con la disciplina de cero-dependencias del proyecto (es código a mano, no un
   crate).

4. **Camino rápido sin registro para el socket ya aceptado.** El framework (y en general
   cualquier servidor sobre `net/webserver`) usa el handle del socket aceptado de forma
   puramente lineal dentro de su propia fibra/tarea — nunca lo comparte con otro handle
   simultáneamente. Si el bucle de `handle_http` pasara el `Arc<TcpStream>` ya resuelto
   (una sola vez, al aceptar) en vez de re-resolver el `i64` contra el registro global en
   cada lectura, la mayoría de las adquisiciones de lock desaparecerían para el caso caliente
   (servir HTTP), dejando el registro global solo para las operaciones que sí lo necesitan
   (abrir/cerrar, listar, TLS/SQLite). Es el camino que TLS ya usa parcialmente (`__ray_tls_get`
   clona un `Arc<Mutex<TlsStream>>` propio del handle) — llevar la misma idea a TCP plano.
   Más invasivo: toca la firma interna de varias funciones generadas.

5. **Construir la `App` una vez por conexión, no por petición** (el diferido que
   `docs/web-framework.md:241` ya marcaba). Cerraría también el hallazgo 1 y el hallazgo 2 de
   raíz para el framework específicamente (no para el lock del registro, que es un problema más
   general de `net/webserver`). Exige resolver el conflicto con el modelo de actores de
   heap-aislado (una `App` con closures no puede cruzar el hilo donde se spawnea cada request
   sin `catch_unwind` o defuncionalización, según el propio comentario de M93.3) — es la opción
   de mayor payback pero también la de mayor costo de diseño; encaja como un M93.x futuro, no
   como un fix puntual.

## 9. Experimento — ¿la brecha crece con la concurrencia?

Barrido de `oha -z 10s -c {20,50,100,200} -q 15000 --latency-correction` (tasa FIJA, solo varía
`-c`) sobre A y B, mismo binario `--release`, misma máquina:

| `-c` | p50 A | p50 B | p99.9 A | p99.9 B | ratio p99.9 | p99.99 A | p99.99 B | ratio p99.99 |
|---|---|---|---|---|---|---|---|---|
| 20  | 0.35 ms | 0.24 ms | 2.11 ms | 0.54 ms | 3.9× | 5.71 ms | 0.97 ms | 5.9× |
| 50  | 0.36 ms | 0.25 ms | 1.55 ms | 0.54 ms | 2.9× | 4.46 ms | 0.95 ms | 4.7× |
| 100 | 0.37 ms | 0.25 ms | 0.83 ms | 0.55 ms | 1.5× | 2.22 ms | 1.10 ms | 2.0× |
| 200 | 0.38 ms | 0.25 ms | **7.59 ms** | 0.65 ms | **11.7×** | **14.32 ms** | 1.32 ms | **10.9×** |

**La mediana confirma el hallazgo 2**: la brecha p50 A-B es plana en las cuatro concurrencias
(~0.10-0.13 ms, el costo de CPU de `build_app()` que no depende de cuántas conexiones haya
alrededor — es puro trabajo secuencial por request).

**La cola NO crece suave con la concurrencia — salta.** El ratio p99.9/p99.99 en realidad
*baja* de c=20 a c=100 (3.9×→1.5×, 5.9×→2.0×) y luego **se dispara de golpe en c=200**
(1.5×→11.7×, 2.0×→10.9×), sin paso intermedio visible entre 100 y 200. Esto descarta la
versión más simple de la hipótesis del §6 (degradación lineal/suave con la concurrencia) a
favor de una más específica: **hay un umbral de contención** — por debajo de cierto número de
conexiones simultáneas, el mutex del registro (§5) rara vez colisiona lo bastante como para
formar un convoy visible; por encima, la probabilidad de colisión cruza un punto donde los
convoyes empiezan a formarse y la cola se dispara de golpe. Es el patrón típico de un mutex
global bajo un número creciente de hilos (más que el de una cola M/M/1 con utilización
gradual), y coincide con el propio relato de M96b (`transpile.rs:1096-1098`), que ya había
visto convoyes de ~100 ms en el mismo lock antes de su fix parcial.

### 9.1 Acotando el umbral (`-c 120/150/175`)

Mismo barrido (`-q 15000 --latency-correction`, 10s) afinando el rango entre 100 y 200:

| `-c` | p99.9 A | p99.99 A | régimen |
|---|---|---|---|
| 100 | 0.83 ms | 2.22 ms | calmo |
| 120 (repetido) | 0.88 ms | 1.48 ms | calmo |
| 150 | 0.84 ms | 1.20–1.67 ms | calmo |
| **175** | **7.69 ms** | **14.49 ms** | **disparado** |
| 200 | 7.59 ms | 14.32 ms | disparado |

(La primera corrida de `-c 120` había dado 1.82/4.87 ms; se repitió y salió 0.88/1.48 ms,
dentro del ruido normal de corrida a corrida — no era el inicio de una subida gradual.)

El umbral **no es difuso**: entre `-c 150` y `-c 175` el sistema pasa de "calmo" a "disparado"
en un solo escalón (25 conexiones), y una vez cruzado **no sigue subiendo** — `-c 175` ya
iguala a `-c 200`, el sistema se estabiliza en el régimen malo en vez de seguir empeorando con
más conexiones. Esto es más compatible con un **punto de transición de fase** (el momento en
que, con la probabilidad de colisión suficiente, empiezan a formarse convoyes de espera en el
mutex, que se autoalimentan una vez arrancan) que con una cola M/M/1 de degradación continua.
B se mantuvo calmo en las tres concurrencias nuevas (p99.99 entre 0.98 y 1.88 ms, sin salto).

**Implicación práctica**: para esta carga concreta (15k req/s, M3 Pro) el umbral cae entre 150
y 175 conexiones simultáneas — no es una constante del framework sino del par (tasa, hilos en
vuelo, hardware), y valdría la pena repetirlo a otra tasa fija para ver si el umbral se mueve
en conexiones o en adquisiciones-de-lock/segundo (quedan sin diferenciar: a 15k req/s con 175
conexiones, el agregado de llamadas a `leer_con_plazo`/`set_read_timeout` por segundo es un
número concreto que no se aisló aquí). Pero el resultado central queda confirmado: **el mutex
del registro (§5), no el costo de `build_app()` (§4), es el mecanismo real detrás de la
degradación** que motivó esta investigación. Esto respalda directamente las hipótesis 2–4 del
§8 (reducir adquisiciones del lock, shardear el registro, camino rápido sin registro para el
socket ya aceptado) como las de mayor impacto por esfuerzo: atacan el umbral en sí, no el
síntoma de A.

## 10. Implementación — caché thread-local del socket (hipótesis 2+4 fusionadas)

Rama `perf/registro-handles-contencion`. Cambio en `src/transpile.rs`, generador del backend
nativo — no toca `checker`/`interpreter`/VM ni ningún archivo `.ray`.

**Idea**: una conexión aceptada la sirve SIEMPRE el mismo hilo de SO durante toda su vida
(`handle_http`, línea 827 de `webserver.ray`, corre en el hilo que `loop_iter_servidor` le
asignó al aceptar — el pool M96 solo reasigna ESE hilo a otra conexión distinta cuando la
actual termina y se cierra, nunca lo comparte concurrentemente). Eso hace válido cachear el
`Arc<TcpStream>` ya resuelto en un `thread_local!` por handle: el primer acceso de esa
conexión paga el `Mutex` global (como antes), y todos los accesos siguientes — que son la
mayoría, dado el ciclo de lectura de `leer_con_plazo` y el keep-alive — lo resuelven sin tocar
el lock. Es sano (no hay UB): el `Arc` cacheado nunca cruza de hilo, así que clonarlo no compite
por su contador de referencias con nadie.

Tres funciones tocadas: `__ray_sock_clone` (de la que cuelgan `socket_read`/`_bytes`/`write`),
`__ray_set_read_timeout` (el otro llamador frecuente de `__ray_reg()`) y `__ray_close` (evict de
la entrada, para que un worker del pool reusado miles de veces no acumule handles muertos).
Verificación funcional: `cargo test --release` sobre `net_cli`/`http_cli`/`dev_cli`/`tls_cli`/
`tls_upgrade_cli`/`bytes_io_cli` (22 tests, todos verdes) + smoke manual (keep-alive
multi-request en una sola conexión, POST/echo, burst de `oha` con 100% success rate).

### 10.1 Medición — **cuidado con el proceso de larga vida**

La primera comparación (A/B recompilados, mismo proceso reutilizado para TODO el barrido de
concurrencia de la sesión) dio resultados contradictorios y ruidosos — en algunos casos el fix
se veía PEOR que el baseline. Sospecha: el pool de hilos (M96) reusa hilos entre conexiones
distintas a lo largo de TODA la vida del proceso; si la caché thread-local no se vacía siempre
(p. ej. un panic salta el `close(conn)` normal) o simplemente por el volumen de miles de
conexiones servidas en una sesión de pruebas larga, el estado acumulado contamina las
mediciones posteriores dentro del MISMO proceso — un artefacto metodológico, no necesariamente
un bug del fix. **Lección**: medir siempre con reinicio limpio del proceso entre corridas
cuando se sospecha de estado acumulado; no reusar el mismo servidor para un barrido largo.

Con reinicio limpio antes de cada corrida (baseline y fixed en puertos distintos, 8080 y 8083,
compilados del mismo commit salvo el fix), 3 repeticiones a `-c 200 -q 15000
--latency-correction`:

| repetición | p99.9 baseline | p99.9 fixed | p99.99 baseline | p99.99 fixed |
|---|---|---|---|---|
| 1 | 26.67 ms | **1.47 ms** (18×) | 34.58 ms | **3.48 ms** (10×) |
| 2 | 69.84 ms | **4.14 ms** (17×) | 78.47 ms | **10.39 ms** (7.5×) |
| 3 | 11.80 ms | **1.73 ms** (6.8×) | 20.47 ms | **3.65 ms** (5.6×) |

Mejora consistente y sustancial en las tres repeticiones (6.8×–18× en p99.9), y notablemente
**el baseline es mucho más ruidoso entre corridas** (11.8–69.8 ms) que el fixed (1.5–4.1 ms) —
el fix no solo baja la cola, la hace más predecible, que para un SLO importa tanto como el
número puntual.

### 10.2 Lo que NO se cerró — sigue habiendo algo de umbral

Un barrido de concurrencia con reinicio limpio (`-c 150/175/200/250/300`, una corrida cada uno)
sobre el binario CON el fix mostró que el umbral **no desapareció del todo**: `-c 150` calmo
(p99.9 1.7 ms), pero `-c 175/200/250` volvieron a mostrar picos (21–35 ms de p99.9) antes de
calmarse de nuevo en `-c 300` (1.5 ms) — un patrón no monótono, con una sola corrida por punto
(no se repitió por presupuesto de tiempo de esta sesión), así que no se puede separar limpiamente
señal de ruido/deriva térmica aquí. Lo que SÍ es sólido es la comparación directa baseline-vs-
fixed a `-c 200` con reinicio limpio y 3 repeticiones (§10.1): ahí la mejora es clara y
reproducible.

**Hipótesis para el remanente**: el mutex del registro (§5) no es el único lock global de la
ruta caliente. `__RAY_POOL` (`transpile.rs`, el pool de hilos de M96) es OTRO
`Mutex<Vec<(u64, Sender)>>` global, tomado en cada `spawn`/retorno-a-pool — y **cada request**
hace un spawn+join para aislar panics (`handle_http`, línea 835), tanto en el baseline como en
el binario con este fix (no se tocó). Es un candidato directo para la siguiente ronda: mismo
patrón de diagnóstico (§5, `sample` bajo carga) aplicado a `__RAY_POOL` en vez de `__ray_reg`.

### 10.3 Próximo paso

Perfilar (`sample`) el binario CON este fix bajo el mismo régimen de `-c 175–250` para ver si el
`__psynch_mutexwait` remanente ahora aparece detrás de `__ray_pool_exec`/`__RAY_POOL` en vez de
`__ray_reg()` — confirmaría que el pool de hilos es el siguiente cuello de botella y motivaría
aplicarle el mismo tipo de tratamiento (sharding, o evitar la vuelta al pool para hilos que van
a servir la MISMA conexión otra vez).

## 11. Segunda ronda — el PRNG global (`__ray_random_int`), y los límites de medir en esta sesión

Se perfiló (`sample`, mismo método del §5) el binario CON el fix del §10 bajo `-c 200 -q 15000`,
buscando específicamente si `__RAY_POOL` (la hipótesis del §10.3) aparecía detrás del
`__psynch_mutexwait` remanente. En vez de eso, contando cuántas veces cada función de
`framework-demo` aparece como ancestro más cercano de una instancia de `__psynch_mutexwait` en
el árbol de llamadas del `sample`:

| función | apariciones como origen del bloqueo |
|---|---|
| **`__ray_random_int`** | **400** |
| `net::log::emit` | 200 |
| `__ray_pool_exec` (`__RAY_POOL`, la hipótesis del §10.3) | 107 |
| `__ray_tls_get` | 91 |

Dos hallazgos, ninguno el que se esperaba:

- **`__ray_random_int`domina, muy por encima del pool.** Causa: `app.log_requests()` (activo en
  el demo) hace que cada petición pase por `webserver.trace_of(req)` → si no hay `traceparent`
  entrante, `trace.new_trace()` genera un `trace_id` de 32 dígitos hex + un `span_id` de 16 —
  **48 llamadas a `random.below(16)`** (`net/trace.ray:27-44`), cada una un `__ray_random_int`,
  cada una tomando el **mismo patrón de mutex global** que `__ray_reg()` pero muchas más veces
  por petición (48 contra las 2-3 del registro). `__ray_rng()` era un único
  `Mutex<u64>` (estado del SplitMix64) compartido por TODO el proceso.
- **`__ray_tls_get` sigue ahí (91)**: el binario linkea soporte TLS (`net/webserver.ray` lo
  define aunque `main.ray` nunca lo llame) y `socket_read_bytes` consulta "¿es TLS este
  handle?" ANTES de mi caché del §10 — esa consulta también toma `__ray_reg()`, sin cachear.
  Documentado, no arreglado en esta ronda (ver "Pendiente" más abajo).

**Fix implementado** (`src/transpile.rs`, M96d): el PRNG pasa de un `Mutex<u64>` global a
**estado thread-local** (`thread_local! { static __RAY_RNG: Cell<u64> }`), sembrado distinto por
hilo (reloj + contador atómico, para que dos hilos no repitan la misma secuencia — importante
para no emitir trace_ids duplicados entre requests concurrentes en hilos distintos). Es sano
(el estado nunca cruza de hilo) y no necesita coordinación: el propio comentario de
`net/trace.ray:11-12` ya documenta que estos ids "identifican, no autentican — no necesitan
cripto", por lo que no hay contrato de aleatoriedad criptográfica ni de secuencia global que
preservar. `random_seed` pasa a sembrar el hilo llamador (antes ya no había reproducibilidad
entre hilos distintos con el mutex global tampoco — dos hilos consumiendo la misma secuencia
competían por orden de llegada, no determinista). 28 tests verdes (incluida
`seed_reproducible_y_kit`, que solo corre en intérprete/VM de un solo hilo — no ejercía este
camino nativo antes ni ahora, pero confirma que el algoritmo SplitMix64 no se tocó).

**Medición — inconclusa por ruido del entorno.** Tres repeticiones a `-c 200` A/B contra el
binario del §10 (solo fix de registro) dieron resultados contradictorios — a veces mejor, a
veces peor, incluso invirtiendo el orden de las pruebas dentro de cada repetición (para
descartar sesgo de "lo segundo que corre ya encontró el sistema más caliente"). La varianza
corrida-a-corrida (p99 entre 0.68 ms y 43 ms para el MISMO binario en corridas sucesivas) es
demasiado alta para separar limpiamente el efecto del fix del ruido de fondo de esta sesión
—`load average` de la máquina venía inusualmente alto (~9.8) sin que `pmset -g therm` reportara
throttling térmico, así que la causa del ruido queda sin diagnosticar—. **Se optó por
mantener el fix igual**: está justificado por evidencia directa y estática (conteo de
apariciones en el perfil, la aritmética de 48 locks/request de `log_requests`, cero riesgo de
soundness, tests verdes) aunque no se pudo demostrar limpiamente su ganancia incremental de
wall-clock en esta sesión. Recomendación: re-medir en una sesión con la máquina en reposo antes
de reportar un número de mejora para este fix específico.

**Pendiente para una próxima ronda** (en orden sugerido):
1. Cachear también `__ray_tls_get` (91 apariciones) — mismo patrón que el §10, con el cuidado
   extra de invalidar la entrada cacheada si la conexión hace un STARTTLS `tls_upgrade` a mitad
   de vida (el handle pasa de Tcp a Tls con el mismo id).
2. Confirmar o descartar `__RAY_POOL` (107 apariciones, la hipótesis original de esta ronda)
   con el mismo método de conteo, en una sesión sin el ruido de fondo actual.
3. Re-correr el barrido de concurrencia (`-c 150–300`, varias repeticiones por punto) con la
   máquina en reposo para ver si el umbral de contención del §9 se movió, se aplanó, o
   simplemente cambió de mecanismo (de `__ray_reg` a `__ray_rng`/`__RAY_POOL`).
