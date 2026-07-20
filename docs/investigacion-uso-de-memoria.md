# Investigación: uso de memoria — los 3 motores vs node/python/php

> **Fecha**: 20 jul 2026 · **Commit**: c04ab74 · **Máquina**: MacBook Pro M3 Pro, 36 GiB, macOS 26.5.1
> · **Versiones**: rustc 1.96.0 · node v26.3.1 · Python 3.14.6 · PHP 8.5.7 (CLI, NTS)
>
> Hasta ahora el proyecto midió **tiempo** (PERFORMANCE.md) y **latencia bajo carga**
> (docs/investigacion-p99-framework-web.md). Esta es la primera pasada sistemática sobre **memoria**.
> Resultado en una línea: la huella de arranque de raylang es la **mejor de la mesa** en los tres
> motores, los datos escalares en la VM cuestan ~4× lo que deberían, y la investigación destapó
> **dos bugs de producción** (una fuga real en la VM y un crash por explosión de hilos en el nativo)
> que pasan a ser la prioridad del plan (§8).

## 1. Metodología

- **Métrica**: pico de RSS (*maximum resident set size*) por proceso, con `/usr/bin/time -l`
  (macOS). Es la métrica que un operador ve (y por la que un contenedor te mata con OOM).
- **Protocolo**: 3 corridas por combinación, se reporta la **mediana**; stdout del programa a
  `/dev/null`; máquina en reposo verificada con `ps` (lección de la investigación p99: procesos
  huérfanos contaminan). Para el intérprete en cargas largas, 1 corrida (orden de magnitud).
- **Corpus**: el banco políglota de `~/Desktop/benchmarks` (los mismos programas por lenguaje):
  `empty` (huella de arranque), `fibrec` (cómputo puro, casi sin heap), `logparse` (parseo),
  `wordcount` (Map + split, 120k líneas), `jsonserialize` (construcción de strings, 400k registros).
  Más micro-benchmarks dirigidos escritos para esta investigación (§4–§6).
- **Motores raylang**: VM (`ray run`, release), intérprete (`--interp`), nativo
  (`ray build --native --release`, opt3+lto+native).

## 2. Resultados — el corpus políglota (pico de RSS, MB)

| Benchmark | ray nativo | ray VM | ray interp | node | python | php |
|---|---:|---:|---:|---:|---:|---:|
| `empty` | **1.5** | 6.8 | 6.6 | 48.9 | 15.3 | 26.1 |
| `fibrec` | **1.6** | 6.8 | 6.7 | 52.6 | 15.3 | 26.3 |
| `logparse` | **1.8** | 7.1 | 6.9 | 56.0 | 15.4 | 26.4 |
| `wordcount` | **2.2** | 7.9 | 7.1 | 56.2 | 15.4 | 26.6 |
| `jsonserialize` | **77.9** | 90.4 | 89.4 | 234.5 | 76.8 | 88.9 |

Lecturas:

1. **La huella de arranque de raylang es la mejor de la mesa, por mucho**: el binario nativo arranca
   en 1.5 MB (33× menos que node, 10× menos que python); la VM y el intérprete en ~6.8 MB (7× menos
   que node). Para CLIs, lambdas y contenedores pequeños es una ventaja real y publicable.
2. En cargas ligeras/medianas (`logparse`, `wordcount`) los tres motores apenas se despegan de su
   línea base: el GC de la VM mantiene el churn a raya (wordcount: +1.1 MB sobre la base).
3. En la carga pesada de strings (`jsonserialize`) **domina el dato, no el motor**: VM 90.4 ≈
   intérprete 89.4 ≈ php 88.9 ≈ python 76.8 ≈ nativo 77.9. El dato vivo real en el pico ronda los
   ~40 MB (400k strings de ~45 B + el join de salida); todos los runtimes pagan ~2× por
   fragmentación/transitorios. node (234 MB) es el glotón de la mesa.

## 3. La política del GC de la VM (contexto para leer los picos)

El GC de la VM (mark-and-sweep por conteo de objetos, `src/gc.rs`) dispara cuando los vivos cruzan
`next_gc = max(vivos × 2, vivos + trabajo_trazado/4, INITIAL)` (Opt.13). Implicación de memoria: en
régimen de churn el pico de **objetos** puede llegar a ~2× los vivos — estándar (V8/Go usan factores
similares) y visible solo cuando los objetos son muchos y pequeños. Los benchmarks de GC del repo
salen sanos: `gcnested` 7.3 MB, `strings` 6.8 MB, `arrays` 8.7 MB (VM) — el GC no es hoy un problema
de memoria. Nota: `malloc` de macOS rara vez devuelve páginas al SO, así que el RSS pico ≈ RSS final
aunque el GC libere; es retención del allocator, común a todos los lenguajes de la mesa.

## 4. El punto débil: arreglos de escalares en la VM (micro dirigido)

`iter.ray` del banco del repo (arreglo de 1M de ints) dio VM 85.9 MB vs nativo 11.7 MB (7.3×).
Aislado con `arr_while.ray` (mismo arreglo, suma con `while` — sin iteradores) y portado a los pares:

| 1M de ints en un arreglo | RSS | neto de su línea base |
|---|---:|---:|
| ray nativo (`Vec<i64>`) | 11.7 MB | **10.2 MB** |
| php (packed array) | 43.4 MB | 17.2 MB |
| node (smis) | 84.8 MB | 35.9 MB |
| python (list + int objects) | 57.5 MB | 42.2 MB |
| ray intérprete | 55.8 MB | 49.1 MB |
| **ray VM** | 72.4 MB | **65.6 MB** |

El neto de la VM es **el peor de la mesa**. Dos causas, ambas estructurales y conocidas:

- **`HeapValue` mide 32 B** (la variante más ancha es `Str(String)` = 24 B + discriminante): un int
  en un arreglo cuesta 32 B vs 8 B del `Vec<i64>` nativo → 4× inherente (~33.5 MB de datos).
- **Transitorios de crecimiento**: el `Vec` duplica capacidad; el realloc final mantiene vivo
  ~48 MB (viejo+nuevo) y los chunks liberados de tamaños anteriores no siempre se reusan
  (fragmentación por size-class) → RSS ≈ 2× los datos.

El camino del iterador (`xs.iter()`) añade ~13 MB más de churn sobre esto (85.9 vs 72.4).

Nota: esto pega solo en programas que materializan **colecciones grandes de escalares**. El corpus de
servicios (§2) no lo nota porque sus colecciones son pequeñas o de strings (donde los 32 B del valor
son el caso legítimo).

## 5. Producción: el webserver bajo carga — nativo acotado, la VM FUGA

Escenario: `~/Desktop/framework` (framework express M93, 15 rutas), `oha -c 100` sobre
`/users/42`, rounds de 10 s (~680k requests por round el nativo, ~300k la VM).

**Nativo** (68.2k req/s): arranca en **1.7 MB**; bajo carga sube a ~80 MB y se **estabiliza en
~90–100 MB** con crecimiento decreciente por round (+14 → +5.7 → +1.3 MB); un proceso fresco tras
3.4M requests queda en 93.8 MB. Es estado estacionario (pool de hilos crecido + retención de
malloc), **no fuga**. ~1 MB por conexión concurrente a c=100 — alto pero acotado y estable.

**VM** (misma app, `ray run`): arranca en 17 MB y crece **lineal y sin freno**: 343 → 604 →
**924 MB** en 3 rounds. ≈ **1 KB retenido por request**. Eso es una **fuga real**, no retención.

### 5a. Causa raíz de la fuga de la VM: los almacenes de tareas/canales nunca liberan

`Vm.shared.tasks: Vec<VmTask>` y `Vm.shared.channels: Vec<VmChannel>` solo hacen `push` (el handle
es el índice); **ninguna entrada se retira jamás**. Peor: una tarea terminada guarda su resultado
(`TaskState::Done(v)` + el heap transferido de la fibra, M38.1b-2) **para siempre**, aunque ya se
haya hecho `join`. El webserver hace `spawn` + `try_join` por request (M56.5) → cada request deja en
el almacén una entrada con **la respuesta HTTP entera** retenida.

Verificado con un micro dirigido (`task_churn.ray`: N tareas efímeras `spawn`+`join` que devuelven un
string de 1 KB): la VM retiene **1.17 KB/tarea** (100k tareas → 123.8 MB; 20k → 29.5 MB; lineal
perfecto). Los canales tienen la misma anatomía (id = índice, nunca se libera; un canal muerto
retiene su cola y su heap).

Esto **no** lo ve ningún test actual: los tests de concurrencia usan decenas de tareas, y el corpus
de benchmarks mide tiempo, no residencia. Solo aparece en procesos de larga vida con churn — es
decir, exactamente en producción.

### 5b. Bug del nativo: explosión de hilos por la trampa de paridad del pool shardeado (M96e)

El mismo micro `task_churn` en el **nativo** directamente **crashea**: `failed to spawn thread:
Os { code: 35 … EAGAIN }` (con 184 MB de RSS de pilas de hilos), incluso con solo 20k tareas.

Causa raíz — una **trampa de paridad**: el spawner (pop de worker ocioso) y el worker (park al
terminar) eligen shard con el **mismo contador round-robin global** (`__ray_pool_next_shard`). En
churn secuencial `spawn → join → spawn → …` las llamadas alternan estrictamente: los pops caen en los
valores pares del contador y los parks en los impares. Con `N` shards **par** — y `N` es **siempre
par**: `available_parallelism()*2` clamp [4,64] — pares e impares mod N son residuos disjuntos → **el
spawner nunca encuentra al worker aparcado** → cada `spawn` crea un hilo del SO nuevo. Con el timeout
de ocio de 10 s, miles de hilos se acumulan en segundos → EAGAIN → el proceso muere (exit 70).

El webserver no lo sufre a c=100 porque la concurrencia entremezcla las llamadas al contador (los
residuos se mezclan), pero cualquier programa nativo con churn secuencial de tareas (un worker de
lotes: `while … { join(spawn(f)) }`) muere hoy. Es un regresión funcional introducida por M96e
(antes del sharding había una sola lista: el pop siempre veía al worker).

## 6. Conclusiones

1. **Publicable**: raylang tiene la mejor huella de arranque de la mesa en los tres motores
   (nativo 1.5 MB, VM 6.8 MB vs node 49 MB / php 26 MB / python 15 MB) y en cargas de servicio
   normales se mantiene a la par o mejor que python/php y muy por debajo de node.
2. **Estructural, no urgente**: los arreglos grandes de escalares en la VM cuestan 32 B/elemento
   (4× el nativo) + transitorios de crecimiento → el peor neto de la mesa en ese micro. Solo pega a
   colecciones grandes de escalares; candidatos de mejora en §8 (medir antes de decidir).
3. **Bug de producción #1 (VM)**: los almacenes de tareas/canales nunca liberan y una tarea `Done`
   retiene su resultado para siempre → el webserver sobre la VM fuga ~1 KB/request sin techo
   (924 MB en 30 s de carga). El modelo *dev = VM / deploy = nativo* salva al deploy, pero un `ray
   dev` con tráfico sostenido o cualquier daemon sobre la VM se hincha hasta el OOM.
4. **Bug de producción #2 (nativo)**: la trampa de paridad del pool shardeado (M96e) hace que el
   churn secuencial de tareas cree un hilo por spawn → EAGAIN → crash. Un batch worker nativo con
   `join(spawn(f))` en bucle muere hoy.

## 7. Qué NO encontramos

- Ninguna fuga en los caminos de CLI/cómputo/strings de los tres motores (los picos se explican por
  datos vivos + factores de allocator normales).
- El GC de la VM (umbral, multi-raíz, heap-por-fibra) se comporta bien en churn puro (`gcnested`,
  `strings`: sin despegue de la línea base).
- El nativo bajo carga **concurrente** (webserver) es estable y acotado; no hay fuga en el runtime
  de red/print de M96c–g.

## 8. Plan por pasos (propuesta: arco M98 — memoria)

Orden por severidad; cada paso con su verificación. Los pasos de código van por rama + PR.

> **Progreso (20 jul 2026, rama `feature/m98-memoria`)**: ✅ **M98.1** y ✅ **M98.2** implementados y
> verificados — `task_churn` 100k: VM 123.8 MB → **6.9 MB** (línea base), nativo crash → **2.1 MB**;
> webserver sobre la VM a c=100: 343→924 MB creciendo → **~31 MB plano**. Semántica fijada:
> **una tarea es de un solo consumidor** (`join`/`try_join` consumen; el scope consume a sus hijas;
> doble join = `task already consumed`, byte-idéntico en VM y nativo). Espec en DESIGN §21.7.

- **M98.1 — liberar el almacén de tareas de la VM** (bug #1, prioridad máxima).
  *Decisión de semántica primero* (DESIGN): **`join`/`try_join` consumen la tarea** (precedente:
  `JoinHandle::join` de Rust toma `self`; hoy re-unir una tarea ya unida es un caso sin especificar).
  Con eso, la entrada se libera en el join/observación (y las hijas de un `scope` al cerrarse);
  el almacén pasa a **free-list con generación** (el handle deja de ser índice desnudo → id con
  generación para cazar dobles-join con error claro, no colisión silenciosa). Cubrir el caso
  "spawn sin join que termina" (fire-and-forget del webserver antiguo): liberable al terminar si
  el handle ya no es raíz de ningún heap (o política simple: al terminar + no adscrita a scope +
  nunca observada → retener solo el estado, descartar el heap del resultado). Verificación:
  `task_churn.ray` con RSS plano; el webserver VM bajo `oha` estabilizado como el nativo; tándem
  con el runtime nativo si la semántica de consumo cambia mensajes (paridad).
- **M98.2 — matar la trampa de paridad del pool nativo** (bug #2, prioridad máxima, fix pequeño).
  El spawner, tras fallar el pop en su shard round-robin, **sondea los demás shards** antes de crear
  hilo (el primer probe mantiene la baja contención de M96e; el barrido solo corre en el caso miss).
  Alternativa: el worker aparca en el shard indexado por **su propio id** (elimina la correlación
  con el contador del spawner). Añadir un tope de hilos de salvavidas (p. ej. 4096) con error claro
  mejor que EAGAIN críptico. Verificación: `task_churn` nativo termina con RSS plano y ≤ N hilos;
  re-medir el webserver a c=100/150 para confirmar que el p99 de M96e no se pierde.
- **M98.3 — canales: misma anatomía, misma cura** (tras 98.1, reusa la maquinaria).
  `close` + cola vacía + sin aparcados → liberable con free-list/generación; `recv` sobre un id
  liberado → error claro. Menos urgente que tasks (el webserver crea menos canales que tareas),
  pero es la misma fuga a menor tasa.
- **M98.4 — memoria en el banco de regresión** (barato, evita recaídas).
  `benchmarks/measure.py`/`regress.py` ganan la columna RSS (`/usr/bin/time -l` / `getrusage`);
  `task_churn.ray` y `arr_while.ray` entran al banco con umbral de regresión. El método del
  webserver (oha + `ps` por rounds, §5) queda documentado aquí como procedimiento manual.
- **M98.5 — evaluar (medir, no comprometer): el coste de 32 B/elemento** (§4).
  Dos candidatos con trade-off CPU/memoria a medir en ambos ejes: (a) `HeapValue` 32→16 B
  (boxing de `Str`/`Bytes`; Opt.10/Opt.3 lo descartaron **por CPU** — el criterio ahora incluye
  memoria: −50% en arreglos de escalares); (b) **arreglos homogéneos especializados**
  (`Obj::IntArray(Vec<i64>)` elegido por el compilador cuando el tipo estático es `[int]`):
  8 B/elemento (−75%), sin tocar el resto del sistema de valores, a costa de duplicar caminos en
  índice/push/GC. Solo si (a) o (b) gana en memoria sin perder >3–5% de CPU en el banco; si ambos
  pierden, documentar el 4× como coste aceptado del diseño de valores dinámicos.

## Apéndice: reproducción rápida

```sh
# pico de RSS de una corrida
/usr/bin/time -l <cmd> 2>&1 >/dev/null | grep "maximum resident"
# webserver por rounds (fuga vs retención)
./framework-demo & PID=$!
for r in 1 2 3 4; do oha -z 10s -c 100 --no-tui http://127.0.0.1:8080/users/42 >/dev/null 2>&1; ps -o rss= -p $PID; done
```

Micro-benchmarks de esta investigación: `task_churn.ray` (N tareas efímeras de 1 KB) y
`arr_while.ray` (1M de ints, suma con while) — a integrar al banco en M98.4.
