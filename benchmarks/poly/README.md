# Benchmarks poliglotas

14 programas, cada uno implementado en 9 variantes de lenguaje (js, lua, php, pl, py, ray,
rb, go, rust) — más los 3 binarios compilados (nativo de Raylang, Go, Rust) — **12 archivos
por directorio** en total, verificados a que produzcan **el mismo output determinista** (mismo
checksum, comparado por diff). Todos son **autocontenidos** (generan sus propios datos, sin
ficheros ni red): así el benchmark aísla el coste de cómputo del lenguaje, no la varianza del
disco/SO.

Cada programa vive en su propio directorio (`wordcount/`, `jsonserialize/`, `treealloc/`, …)
y se agrupa por categoría (ver tabla abajo) — el mismo agrupamiento que ves en
`./bench.py list` y en el menú de `./tui.py`. Se ejecutan con el arnés poliglota en Python
(ya no depende de `hyperfine`; usa `time.perf_counter` internamente y presenta los resultados
en una tabla):

```sh
./bench.py wordcount        # o cualquier otro programa
./bench.py all              # todos
./bench.py list             # listado agrupado por categoría, con descripción
```

## Los programas, agrupados por qué miden

### Arranque

Overhead puro de arrancar el runtime/intérprete, sin cómputo real — el piso de latencia de
cada lenguaje.

| Programa | Qué mide |
|---|---|
| **`empty`** | programa vacío |
| **`print`** | un solo `print` — arranque + I/O mínimo |

### CPU / aritmética-recursión

Aritmética y llamadas a función puras, sin tocar la stdlib — donde raylang, sin JIT ni
backend nativo, rinde **peor** relativo a los demás.

| Programa | Qué mide |
|---|---|
| **`loopsum`** | suma con módulo en un loop de 10M iteraciones |
| **`fibonacci`** | fibonacci recursivo para n=0..9 — llamadas a función, poco trabajo |
| **`fibrec`** | fibonacci recursivo profundo `fib(34)` — stress de llamadas a función |
| **`factorial`** | factorial recursivo para n=0..9 — recursión simple |

### Datos de servicio (stdlib string + hashmap)

El trabajo real de un **servicio** (I/O/datos-bound): parsear peticiones, agregar en tablas
hash y construir respuestas — ejercita la **stdlib**, no la aritmética.

| Programa | Modela | Primitivas ejercitadas |
|---|---|---|
| **`wordcount`** | Crunch de datos: contar frecuencias sobre líneas sintéticas | `split` + hash map `get`/`insert` + `sort` (agregación) |
| **`jsonserialize`** | Ruta de **salida**: serializar N registros a JSON | `to_string` + concatenación + `push`/`join` (construcción de strings) |
| **`jsondeserialize`** | Ruta de **entrada**, contraparte de `jsonserialize`: parsear los registros | búsqueda de substrings (`index_of`/`substring`) + `parse_int`, sin librería de JSON |
| **`logparse`** | Ruta de **entrada**: parsear N líneas de log | `split` + `parse_int` + agregación en **dos** maps |

- `wordcount`: 120 000 líneas, ~1 000 claves distintas, checksum sobre claves ordenadas.
- `jsonserialize` / `jsondeserialize`: 400 000 registros `{"id":…,"name":"user…","score":…}`.
- `logparse`: 150 000 líneas `GET /api/N STATUS MS`, cuenta por status + latencia por path.

### Estructuras de datos / GC

| Programa | Qué mide |
|---|---|
| **`treealloc`** | Presión de **GC/allocator**: construir, contar y descartar muchos árboles binarios pequeños (benchmark clásico "binary-trees"). Árboles completos, profundidad 4 a 15 (min_depth=4, max_depth=14, +stretch de 15). Sin hashmaps de por medio — mide alocación pura. |

### Numérico

| Programa | Qué mide |
|---|---|
| **`sortnums`** | Ordenar un array **numérico** grande (no claves de string como en `wordcount`/`logparse`): 1 000 000 de enteros generados con un LCG determinista (MINSTD, multiplicador 48271 mod 2³¹-1). |
| **`matrixmul`** | Multiplicación de matrices 200×200 — el único workload con **punto flotante** (todo lo demás en la suite es entero puro). Valores deterministas `(i·n+j) mod 13` / `(j·n+i) mod 17`. |

### Pattern matching

| Programa | Qué mide |
|---|---|
| **`regex`** | Extracción de campos con motor de **regex** (distinto del `split` ingenuo de `logparse`): 200 000 líneas `userN GET /api/M STATUS MSms`, patrón `^user(\d+) GET /api/(\d+) (\d+) (\d+)ms$`. |

> **Nota sobre `regex`**: `regex.rs` parsea a mano el mismo patrón fijo porque el crate `regex`
> de Rust requeriría Cargo + red, rompiendo el build de un solo archivo con `rustc` usado para
> el resto de la suite. `regex.ray` también parsea a mano, pero por una razón distinta: raylang
> **sí tiene** un motor de regex real en su stdlib (`import std/regex;` — Thompson NFA/Pike VM,
> sin backtracking, en `examples/stdlib/regex.ray` del repo del lenguaje), con `compile()` +
> trait `Matcher` para no recompilar el patrón en cada llamada. Se probó: el checksum coincide,
> pero al estar el motor escrito en el propio raylang (no bindea una lib en C como los demás
> lenguajes) resultó **~300× más lento** que el parseo manual en este workload (~59 s
> interpretado / ~2.3 s nativo contra ~0.2 s, para 200 000 líneas) — se optó por el parseo
> manual para no distorsionar los tiempos de `bench.py all`. Para patrones de
> usuario/config reales (no conocidos de antemano, que es donde importa tener un motor de
> verdad en vez de parsing ad-hoc), `std/regex` es la herramienta idiomática correcta.

## Resultados (29 jul 2026 — M3 Pro, mediana de 10 corridas, 5 de calentamiento)

Suite completa con el arnés actual (auto-medición: los 12 programas de cómputo cronometran su
propio workload, así que **no** cuentan el arranque del runtime; `empty`/`print` sí lo miden, que
es justo lo que evalúan). `ray` = la VM; `native` = `ray build --native --release`, que desde el
arco F enlaza **fibras M:N** por defecto.

**Tiempo del binario nativo frente a los compilados y a node** (× = cuántas veces más lento que
`native`; <1 significa que nos gana):

| Programa | native | vs node | vs go | vs rustc -O | VM ÷ native |
|---|---|---|---|---|---|
| `loopsum` | **27.3 ms** 🥇 | 9.06× | 1.01× | 1.00× | 28× |
| `fibrec` | 17.7 ms | 2.21× | 0.91× | 0.79× | 54× |
| `wordcount` | 48.6 ms | 2.60× | 0.90× | **1.24×** | 5.5× |
| `jsonserialize` | 28.6 ms | 2.50× | 0.96× | 0.92× | 3.4× |
| `jsondeserialize` | 85.2 ms | 1.10× | 0.52× | 0.57× | 4.2× |
| `logparse` | 27.0 ms | 1.86× | 0.83× | **1.17×** | 3.3× |
| `treealloc` | **18.1 ms** 🥇 | 1.13× | **1.59×** | **1.52×** | 42× |
| `sortnums` | **18.0 ms** 🥇 | 19.99× | **3.51×** | **1.12×** | 12× |
| `matrixmul` | **5.6 ms** 🥇 | **4.12×** | **1.35×** | 1.01× (empate) | 117× |
| `regex` | 70.7 ms | 0.87× | **1.08×** | 0.37× | 246× |

> La fila de `matrixmul` es la **re-medición post-N6** (PERFORMANCE.md Fase 67, 29 jul: el
> transpilador iza los `borrow()` de RefCell fuera de los loops puro-escalares → LLVM vectoriza).
> En la corrida original del mismo día iba en 11.6 ms, 0.50× de rustc.

- **Le gana a node en 9 de los 10** programas de cómputo (de 1.1× a 20×). El único que pierde es
  `regex`, y con matiz: node lleva un motor de regex en C++ mientras la variante `.ray` parsea a
  mano (ver la nota de arriba); el `native` va por el crate `regex` desde R5.
- **Le gana a `rustc -O` en cinco** (`wordcount` 1.24×, `treealloc` 1.52×, `sortnums` 1.12×,
  `logparse` 1.17×) **y empata en dos** (`loopsum`, `matrixmul`), y **le gana a Go en cinco**.
  Donde pierde contra ambos es en `jsondeserialize` — búsqueda de substrings.
- **Arranque** (proceso completo, sin auto-medición): `empty` **1.80 ms 🥇 #1 de 10** — por
  delante de rustc (1.92 ms) y Go (1.98 ms), con 1.8 MB de RSS. Las fibras no lo penalizan.

**Ranking combinado (tiempo × memoria, media geométrica)**: el binario nativo queda **#2 en 10 de
los 12 programas** y #3 en los dos restantes (`jsonserialize`, `matrixmul`) — nunca por debajo del
tercer puesto, contra 9 lenguajes.

### Lectura

- **El backend nativo cambió la categoría del proyecto.** La tabla anterior de este README (arco
  P0, 14 jul) medía la VM contra intérpretes y celebraba "2.0–2.7× del líder". Hoy la comparación
  relevante es contra **binarios compilados**, y ahí se pelea de tú a tú con Go y Rust.
- **La VM sigue siendo el motor de desarrollo**, y su distancia al nativo dice para qué es cada
  uno: 3–4× en los workloads de servicio (donde el coste real está en la stdlib, compartida) y
  28–57× en los micro de cómputo puro (donde solo se mide el bucle de despacho). El modelo
  *dev = VM / deploy = nativo* no es un eslogan: es esta tabla.
- **Las fibras M:N salen gratis en cómputo.** A/B del mismo programa con y sin `--without fibers`
  (mediana de 15 corridas): `fibrec` +0.4%, `wordcount` +1.9%, `treealloc` −2.3% — todo dentro del
  ruido — y el arranque de `empty` incluso mejora (1.57 vs 1.97 ms, porque el modelo de fibras no
  levanta el hilo worker que sí crea el de hilo-por-tarea). Su beneficio está en concurrencia e
  I/O, y no se paga en los programas secuenciales.
- **Dónde queda trabajo**: `jsondeserialize` (0.52× de Go — búsqueda de substrings) es el hueco
  claro que queda; `matrixmul` lo cerró N6 (Fase 67: hoist de borrows → vectorización). Plan en
  `raylang/PERFORMANCE.md`.

### Reproducir

```sh
# compilar primero los binarios nativos (.ray), Go (.go) y Rust (.rs) de cada directorio:
./build-all.sh

# tiempo Y memoria de los tres, con export a markdown (una sola pasada: cada
# corrida cronometra y mide RSS a la vez, así ambas tablas son de las MISMAS
# corridas):
./bench.py wordcount --export-md /tmp/wc.md
./bench.py jsonserialize --export-md /tmp/js.md
./bench.py logparse --export-md /tmp/lp.md
```

## Scripts

| Script | Qué mide | Cómo |
|---|---|---|
| **`build-all.sh`** | — | Compila todos los `.ray` (`ray build --native`), `.go` (`go build`) y `.rs` (`rustc -O`) de cada subdirectorio |
| **`bench.py`** | Tiempo + memoria en un solo set de corridas | `time.perf_counter` alrededor de un spawn directo y `ru_maxrss` del `wait4` de ESA misma corrida; tablas con mediana/mín/máx/MAD/ratio más un ranking combinado |
| **`tui.py`** | Tiempo + memoria + ranking, interactivo | Interfaz de texto con `curses`: selector de programa/variantes/corridas con teclado, progreso en vivo y gráficos de barras finales |
| **`benchlib.py`** | — | Módulo compartido: descubrimiento de programas/variantes, formato de tabla, export a Markdown/CSV, config |
| **`settings.toml`** | — | Activa/desactiva variantes de lenguaje y fija defaults de `--runs`/`--warmup`, de forma persistente |

### Metodología de medición

- **Auto-medición del cómputo (marcador `bench_ns`)**: cada programa que no es de Arranque
  cronometra su propio workload con un reloj monotónico y emite `bench_ns=<int>` por stderr como
  última acción; el harness usa ese valor como tiempo. Así "tiempo" mide SOLO cómputo, sin el
  arranque del runtime (los ~45 ms de node, los ~16 ms de python, etc.). Los programas de
  Arranque (`empty`, `print`) no llevan marcador y siguen midiendo el proceso completo — esa
  categoría existe justamente para eso. stdout no participa, así que la verificación de output
  por diff sigue intacta. La memoria sigue siendo del proceso completo (RSS pico, runtime
  incluido). Asterisco: lua no tiene reloj de pared monotónico en su stdlib, usa `os.clock()`
  (tiempo de CPU) — equivalente en estos workloads single-thread CPU-bound.
- **Spawn directo, sin intermediarios**: cada corrida se lanza con `posix_spawnp` y se espera con
  `wait4`, sin shell ni wrapper (`/usr/bin/time`) en el medio — el overhead ajeno al programa es
  <1 ms (con wrapper eran ~3-5 ms, más que la señal de los binarios más rápidos). Del `rusage` del
  mismo `wait4` salen la memoria pico (`ru_maxrss`) y el tiempo de CPU: pared, memoria y CPU son
  siempre de la misma corrida. *(Nota: `ru_maxrss` no es idéntico al "peak memory footprint" que
  reportaba `/usr/bin/time -l` — los números de memoria no son comparables con exports anteriores
  a este cambio.)*
- **Presupuesto de tiempo por variante** (`budget_s` en `[bench]`, `--budget` en CLI, default
  5 s; 0 = sin límite): estilo hyperfine, el número de corridas se adapta al costo de cada
  variante. Cuando la pared acumulada de una variante (proceso completo, warmup incluido) supera
  el presupuesto, deja de correr — nunca con menos de 5 muestras medidas. Las variantes rápidas
  hacen sus `runs` completas; las lentas no marcan el ritmo de la sesión. Mediana y MAD funcionan
  con n variable, y una variante lenta necesita menos muestras (su ruido relativo es menor).
  **Salvedad conocida** (sin resolver): la tabla no marca qué filas se cortaron por presupuesto,
  así que un MAD calculado sobre 5 muestras aparece junto a uno calculado sobre 20 sin
  distinguirse. Para una medición que vaya a citarse en `PERFORMANCE.md`, corre con `--budget 0`
  y `--runs` explícito, o comprueba a mano que ninguna variante se truncó.
- **Rondas intercaladas con rotación** (`A B C / B C A / ...`): el drift ambiental (térmico,
  procesos de fondo) se reparte entre todas las variantes en vez de caer entero sobre una, y la
  rotación elimina el sesgo de posición. El warmup son las primeras rondas completas.
- **Mediana y MAD**, no media y σ: en un workload determinista el ruido solo suma, así que un
  outlier arrastra la media pero no la mediana. El mínimo es el mejor estimador del costo sin
  interferencia.
- **Empates técnicos**: variantes cuyas ventanas `mediana ± max(2·MAD, 0.5%)` se solapan en tiempo
  y memoria comparten puesto en el ranking (`#1=`) — no se proclama ganador por diferencias del
  orden del ruido.
- **Avisos de sesión contaminada**: si >10% de las muestras son outliers (`> mediana+3·MAD`) o
  tienen pared ≫ CPU (desalojo del SO), la tabla lo marca con `⚠` — mejor repetir esa sesión.
- **Metadatos en los exports**: fecha, CPU, OS, versiones de cada runtime y `runs/warmup` quedan
  registrados en el Markdown/CSV; sin eso un resultado no es comparable con el del mes que viene.

`build-all.sh` compila en paralelo (un job por core): compilar no es medir, ahí el multicore
es gratis. Las corridas medidas son SIEMPRE seriales — variantes concurrentes se pelean por
caché/memoria/térmica y en Apple Silicon caen en E-cores: velocidad a cambio de exactitud, no.

Consejos de higiene al correr: equipo enchufado (sin Low Power Mode), sin compilaciones ni apps
pesadas en paralelo, y descartar la primera sesión tras un reboot (caches fríos).

`bench.py` (línea de comandos) y `tui.py` (interactivo) comparten el mismo arnés
(`benchlib.py`) y la misma interfaz:

```sh
./bench.py                 # lista los programas y pregunta cuál medir
./bench.py list            # solo lista
./bench.py <n>             # por número del listado
./bench.py <nombre>        # todas las variantes de <nombre>
./bench.py all             # todos los programas

# flags: --runs N --warmup N --exclude "a b c" --export-md FILE --export-csv FILE
#        --prepare CMD (o --clean, atajo de --prepare "sync")
```

### Interfaz interactiva (TUI)

`./tui.py` levanta una interfaz de texto (solo stdlib, `curses` — sin dependencias que instalar)
para no tener que memorizar flags:

```sh
./tui.py
```

El menú principal es solo **elegir programa** (o "ejecutar todos") — `enter` corre el benchmark
directo, sin pasos intermedios. La esquina superior derecha muestra un panel de configuración
persistente (`20r/10w · 7/10 lenguajes`); presionando **`s`** en cualquier momento se abre la
pantalla de **Configuración** (lenguajes a incluir, corridas, calentamiento), que queda guardada
para las corridas siguientes hasta que se vuelva a cambiar o se cierre la TUI.

Al elegir un programa: corre con **barras de progreso en vivo** por variante → pantalla de
**resultados** con gráficos de barras (tiempo, memoria) coloreados por puesto (verde = mejor,
rojo = peor) y la tabla de **ranking combinado**, con scroll (`↑`/`↓`) si no entra en la terminal.
`q`/`esc` vuelve un paso atrás en cualquier pantalla; desde el menú principal sale del programa.

### Activar/desactivar lenguajes

`settings.toml` (raíz del proyecto) controla qué variantes participan en `list`/`choose`/`bench`,
de forma persistente (sin repetir `--exclude` en cada corrida):

```toml
native = true   # binario nativo (ray build --native)
go     = true   # binario compilado (go build)
rs     = true   # binario compilado (rustc -O)
js     = true   # node
lua    = true   # lua
php    = true   # php
pl     = true   # perl
py     = true   # python3
ray    = true   # ray run (intérprete)
rb     = true   # ruby

# Defaults de corridas/calentamiento (se pueden sobreescribir con --runs/--warmup).
[bench]
runs   = 20   # corridas medidas por variante
warmup = 10   # corridas de calentamiento descartadas
```

Poner una clave de lenguaje en `false` la excluye en `bench.py` y en `tui.py`; se combina con
`--exclude` si también se pasa por línea de comandos. Un lenguaje habilitado cuyo intérprete no
esté instalado en la máquina no rompe la corrida: `variants_for` lo omite con un aviso, así que
el default commiteado deja los nueve activos y cada máquina mide lo que tiene.
La tabla `[bench]` fija los valores por defecto de `--runs`/`--warmup`; un flag explícito en la línea de comandos siempre gana sobre el config. Si el archivo no existe, todas
las variantes quedan habilitadas y se usan los defaults de fábrica (20 corridas, 10 de calentamiento).
