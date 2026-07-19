# Investigación: el costo de la "azúcar" del framework `web/framework` (post M96c-g)

> Nota de alcance: continuación directa de
> [`docs/investigacion-p99-framework-web.md`](investigacion-p99-framework-web.md) (rama
> `perf/registro-handles-contencion`, fusionada). Esa investigación resolvió la contención de
> mutexes globales del runtime nativo (M96c-g); con eso fuera de la ecuación, esta retoma la
> pregunta original — cuánto cuesta la capa de conveniencia del framework tipo Express frente al
> `net/webserver` pelado — que antes quedaba enmascarada por la contención.

## 1. Punto de partida

Con M96c-g mergeados (registro de handles, PRNG, pool de hilos, `print`, chequeo TLS — todos
cacheados/sin lock global), se repitió la comparación original A (framework completo, 15 rutas) /
B (framework, 1 ruta) / C (`net/webserver` pelado) para ver cuánto de la brecha sigue viva.

## 2. Metodología

Igual que la investigación anterior: `oha -z 8s -c 200 -q 15000 --latency-correction`, reinicio
limpio de cada binario antes de cada corrida, patrón ABBA para cancelar sesgo de orden, máquina
en reposo (verificado con `uptime`/`top` antes de medir). Luego profiling con `sample` de macOS
bajo carga sostenida, y microbenchmarks aislados (sin red) de las piezas que el profile señaló.

## 3. Hallazgo 1 — la brecha de mediana es ahora clara y consistente (antes, el ruido de los locks la tapaba)

| variante | p50 (promedio de 2 corridas) |
|---|---|
| **A** — framework completo (15 rutas + sub-app + regex + `log_requests()`) | ~0.80 ms |
| **B** — framework, 1 sola ruta (sin `log_requests()`) | ~0.76 ms |
| **C** — `net/webserver` pelado | ~0.69 ms |

Descompuesto:
- **B − C ≈ 72 µs**: costo de la MAQUINARIA del framework en sí, aun con una sola ruta —
  `dispatch`/`route_request`, construir `Ctx`/`Res`, `Map.new()` para headers/locals,
  `json_of`/`ToJson`, `build_response`.
- **A − B ≈ 41 µs**: costo de reconstruir 14 rutas más, el mount de la sub-app, la regex, Y
  (importante) el `log_requests()` que A tiene activado y B no.

A diferencia de la investigación anterior (§4 de `investigacion-p99-framework-web.md`), esta vez
la brecha de MEDIANA es nítida y repetible entre corridas — antes esos mismos ~7-8 µs de
`build_app()` quedaban invisibles al lado de la contención de mutexes (cientos de µs); ahora que
esa contención no está, se ve la letra chica.

## 4. Hallazgo 2 — el profile ya no tiene locks; el trabajo real es reconstrucción + tracing/logging

`sample` bajo `-c 200 -q 15000` sobre A: `__psynch_mutexwait` cayó de **110 996** (medición
original, pre-M96c) a **188** — confirma que la contención está resuelta. Contando apariciones de
funciones raylang en el árbol de llamadas (proxy de "cuánto tiempo se ve esto en la pila"):

| función | apariciones |
|---|---|
| `build_app` | 1351 |
| `web::framework::handle` | 1209 |
| `net::webserver::handle_http` | 1126 |
| `net::webserver::send_response_keep` | 1021 |
| **`net::log::emit`** | 984 |
| `net::webserver::read_request_limits` | 698 |
| **`net::log::json_escape`** | 560 |
| `web::framework::route` | 538 |
| **`std::regex::compile_re`** | 524 |
| `web::framework::GET` | 507 |
| **`net::trace::random_hex`** | 413 |
| **`net::trace::new_trace`** | 363 |
| `web::framework::split_path` | 308 |
| `web::framework::dispatch` | 276 |
| `std::regex::parse_concat` | 187 |
| `web::framework::route_request` | 169 |
| `std::regex::parse_alt` | 163 |
| `User#to_json` | 92 |

Dos familias de costo real (ninguna es un lock, todo es cómputo puro):
1. **Reconstrucción de la tabla de rutas** — `build_app`/`route`/`GET`/`compile_re`/
   `parse_concat`/`parse_alt`/`split_path`: exactamente lo que ya se había medido en la
   investigación anterior (§4), ahora sin nada que lo tape.
2. **Tracing + logging estructurado** — `random_hex`/`new_trace`/`log::emit`/`json_escape`: NO
   estaba en el radar de la investigación anterior (esa se centró en el LOCK del PRNG y de
   `Stdout`, ya resueltos por M96d/M96f — pero el trabajo de CÓMPUTO detrás de generar un
   trace_id y renderizar la línea JSON nunca se había medido en microbenchmark aislado).

## 5. Hallazgo 3 — microbenchmark aislado: el logging pesa MÁS que reconstruir la app

200 000 iteraciones, sin red, `std/time.monotonic()` (mismo harness de la investigación
anterior, extendido):

| pieza | promedio |
|---|---|
| `build_full()` (15 rutas+mount+regex+static+cors+log) | 7.0 µs |
| `build_minimal()` (1 ruta) | 0.3 µs |
| `trace.new_trace()` (32+16 dígitos hex = 48 `random.below`) | 2.4 µs |
| **`log`: `new_trace` + `with_trace` + 4 `field*` + `render`** | **7.7 µs** |
| `json_of` (`ToJson` + `jsonlib.stringify`) de `/yo` | 0.5 µs |

**La línea de log completa (7.7 µs) cuesta más que reconstruir las 15 rutas (7.0 µs).** Con
`log_requests()` activo (como en A), el costo de CPU por petición es aproximadamente:

```
build_app()        ~7.0 µs
trace + log render  ~7.7 µs   (new_trace ya incluido en el número de arriba: 2.4 de los 7.7)
json_of             ~0.5 µs
────────────────────────────
total conocido      ~15.2 µs
```

Contra los ~41 µs de brecha wall-clock medidos entre A y B (§3) — cerca de un tercio del gap se
explica con estas dos piezas solas; el resto es ruido de medición y overhead no aislado
(scheduling, variación de I/O). Aun así, es la primera vez que se cuantifica el logging como un
costo de CPU real y no solo como "una línea de más en el profile".

## 6. Hipótesis de solución

En orden de menor a mayor invasividad — a diferencia de la ronda anterior (locks, todo interno al
runtime nativo, invisible al programa raylang), acá las opciones más grandes SÍ tocan diseño de
lenguaje/stdlib, así que se marcan explícitamente cuáles necesitan una decisión antes de
implementarse.

1. **Optimizar la construcción de strings en `net/trace.ray`/`net/log.ray`** (bajo riesgo, sin
   tocar runtime ni lenguaje). `random_hex(n)` arma un `[string]` de UN CARÁCTER cada uno y hace
   `join(parts, "")` — 32+16 asignaciones de string de 1 char más una concatenación final, en vez
   de construir directo un solo buffer. `log::render` concatena con `out = out + ...` repetidas
   veces (cada `+` de string en raylang es una nueva asignación de heap, no in-place) — con 4+
   campos y las cabeceras, son varias asignaciones intermedias descartadas. Ambas son
   optimizaciones de ALGORITMO puro, en código 100% raylang, sin ningún cambio de runtime — el
   tipo de fix más seguro de esta lista.

2. **Cachear la regex compilada — ahora SÍ es seguro, con el alcance correcto.** La ronda
   anterior (`investigacion-p99-framework-web.md` §8, ítem 1) descartó esto por un problema de
   *soundness*: un `Regex` raylang usa `Rc` internamente, y compartirlo entre hilos de verdad
   (una caché GLOBAL) es una carrera de datos en su contador de referencias. Pero acá el patrón
   es distinto: si la caché es **thread-local** (no cruza de hilo jamás, como M96c/M96g), no hay
   ningún problema de soundness — el primer request que un hilo del pool atiende paga el
   `compile()`, los siguientes (el pool reusa hilos entre miles de requests, M96) lo encuentran
   cacheado. Esto SÍ necesita una pieza nueva: raylang no tiene hoy ninguna forma de expresar
   "guardame esto por hilo" desde código raylang — hace falta un builtin nuevo (candidato:
   algo como `once_per_thread<T>(f: fn() -> T) -> T`, ver ítem 3).

3. **La apuesta más grande: memoizar `build_app()` entero, por hilo — necesita una decisión de
   diseño.** Es la generalización del ítem 2: en vez de cachear solo la regex, cachear la `App`
   COMPLETA (rutas, middlewares, mounts, todo) la primera vez que un hilo del pool la construye, y
   reusarla en cada request siguiente que ESE MISMO hilo atienda — sin importar de qué conexión
   venga (el pool ya reusa hilos entre conexiones distintas). Elimina de un saque el `build_app()`
   completo (~7 µs) para la inmensa mayoría de los requests. Es seguro exactamente por la misma
   razón que M96c/g: nunca cruza de hilo. **Pero necesita un builtin nuevo de lenguaje** (algo
   como `once_per_thread`), lo cual implica:
   - Checker: nueva regla de tipos para un builtin genérico.
   - Nativo: `thread_local!` + `OnceCell` — trivial, mismo patrón que M96c/g.
   - **VM: semántica DISTINTA, no es "no-op safe by default".** La VM usa fibras M:1 (muchas
     fibras concurrentes sobre el MISMO hilo de SO) — cachear "por hilo de SO" en la VM
     compartiría la App entre fibras que lógicamente son conexiones DISTINTAS y sí corren
     concurrentemente entre sí (aunque cooperativamente) → violaría el aislamiento que la VM
     hoy garantiza. La VM tendría que tratar `once_per_thread` como "llamar siempre" (no cachear
     nada) — lo cual es CORRECTO (la VM no tiene el problema de creación de hilos que esto
     ataca) pero exige tenerlo claro para no introducir un bug de aislamiento por fibra.
   - Es la que más impacto tiene (elimina el 100% de `build_app()`, no solo la regex), pero
     también la única de esta lista que agrega superficie nueva al LENGUAJE (no solo al runtime
     interno) — vale la pena decidirla explícitamente antes de tocar código, como se hizo con el
     fork de `log::emit` en la ronda anterior.

4. **Reducir el propio costo de generar el trace_id.** Alternativa más chica al ítem 1 para el
   tracing específicamente: en vez de 48 llamadas a `random.below(16)` (una por dígito hex), un
   PRNG que genere bytes de a 8 (`u64`) y los formatee directo a hex de a 16 bits por llamada
   reduciría las llamadas al generador de 48 a 6 — pero toca `std/random`/`net/trace`, más código
   que el ítem 1 y con menor beneficio que atacar el logging completo.

## 7. Plan

Empezar por el ítem 1 (seguro, sin decisiones de diseño pendientes, ataca la pieza más cara del
microbenchmark — el logging), medir, documentar. Ítem 2 (regex thread-local) requiere primero
resolver si se justifica el builtin nuevo — se plantea junto con el ítem 3 como una sola decisión
de diseño a consultar, ya que comparten la misma pieza de lenguaje (`once_per_thread` o similar).

## 8. Ítem 1 implementado — mejora real, pero por debajo del piso de medición de `oha`

`net/log.ray`'s `render()` pasó de `out = out + …` repetido a `parts.push(...)` + `join` (mismo
patrón que ya usaba `json_escape` en el mismo archivo, con un comentario propio advirtiendo el
O(n²) — `render` simplemente no lo había recibido en su momento).

**Microbenchmark aislado** (200 000 iteraciones): `log: new_trace + with_trace + 4 fields +
render` bajó de **7.7 µs a 7.1 µs** (~8%, ~0.6 µs/llamada). Verificado: `log_cli`/`trace_cli`
verdes, línea JSON idéntica (`{"ts":...,"trace_id":...}` bien formada, probado con `curl`).

**Medición end-to-end (`oha`, ABBA, `-c 200 -q 15000`)**: sin diferencia distinguible del ruido
(p50 ~0.78–0.83 ms para ambos binarios, dentro del rango de variación normal entre corridas). Es
un resultado ESPERADO y honesto: 0.6 µs de ahorro está muy por debajo del piso de ruido de
medición de este harness (decenas de µs) — el mismo patrón que M96e/M96g en la ronda anterior
(fix correcto y verificado, ganancia real pero no discernible en `oha` a esta escala). Se
mantiene por ser la práctica ya establecida en el propio archivo, no por una promesa de
performance medible.

## 9. Decisión pendiente — ítems 2/3 (regex/`build_app` cacheados thread-local)

Antes de tocar código: estos dos ítems comparten la misma pieza de lenguaje nueva
(`once_per_thread<T>(f: fn() -> T) -> T` o equivalente) — un builtin que no existe hoy en
raylang. A diferencia de M96c-g (glue interno del backend nativo, invisible a cualquier programa
raylang) y del ítem 1 de esta ronda (una función de stdlib reescrita, misma firma), esto agrega
**superficie nueva al lenguaje**: nueva regla en el checker, nueva semántica que la VM y el
nativo deben implementar DISTINTO (nativo: `thread_local!` real; VM: debe ser un no-op — "llamar
siempre" — porque sus fibras M:1 comparten hilo de SO pero NO deben compartir estado entre sí).

Es la hipótesis de mayor impacto de esta investigación (elimina el 100% de `build_app()`, ~7 µs,
para la inmensa mayoría de los requests — el pool reusa hilos entre miles de conexiones). Antes
de implementarla hace falta decidir: ¿se justifica agregar este builtin al lenguaje por esta
ganancia, o se prefiere no crecer la superficie del lenguaje y aceptar el costo de reconstrucción
como el precio del modelo de actores de heap-aislado?
