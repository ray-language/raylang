# Rendimiento

Español · [English](performance.en.md)

Cómo hacer que un programa de raylang vaya rápido, en el orden en que conviene hacerlo: medir,
encontrar dónde se va el tiempo, arreglar el algoritmo, compilar a nativo y, si hace falta, usar
más núcleos. El ejemplo es un informe sobre un log de accesos, escrito tres veces.

El proyecto está en [`examples/apps/perf-lab`](../examples/apps/perf-lab/). Los bloques de raylang
están copiados de él y el CI comprueba que sigan siéndolo. Las cifras son de un MacBook Pro M3
Pro, con la mediana de cinco corridas: en tu máquina cambiarán, las proporciones no tanto.

## 1. Medir antes de tocar nada

El programa genera su propia entrada, siempre la misma, y cronometra solo el trabajo que se
compara. `time.monotonic_millis()` es un reloj que nunca retrocede, el adecuado para medir:

<!-- check: project=examples/apps/perf-lab -->
```rust
    // Time only the work being compared, not the generation of the input.
    let start = time.monotonic_millis();
    let report = if (which == "slow") {
        slow.report(lines)
    } else if (which == "parallel") {
        parallel.report(lines, 4)
    } else {
        fast.report(lines)
    };
    let elapsed = time.monotonic_millis() - start;
```

```sh
ray run -- slow 200000
```

```
slow: 200000 lines, 6005 paths, 141343 chars, 7274 ms
```

Siete segundos para 200 000 líneas. Antes de optimizar hay que saber dónde se van.

## 2. Dónde se va el tiempo: `ray profile`

`ray profile` ejecuta el programa y, al terminar, imprime una tabla por función: el tiempo propio
(sin contar lo que llama), el porcentaje, el tiempo con sus llamadas, cuántas veces se llamó y la
media.

```sh
ray profile -- slow 200000
```

```
   self ms  self%    incl ms     calls    avg µs  function
  5332.172  85.3%   5332.172         1 5332171.67  slow::order
   731.924  11.7%    731.924    200000      3.66  position
    90.331   1.4%    822.255         1 822255.21  slow::count
    77.339   1.2%     77.339         1  77338.88  data::log
    17.885   0.3%     17.885         1  17884.71  slow::render
```

La primera versión tiene tres pasos, cada uno en su función, y el perfil los separa:

- **`slow::order` se lleva el 85 %.** Es una ordenación por selección escrita a mano: por cada
  ruta recorre todas las demás, 36 millones de vueltas para 6005 rutas.
- **`position` es el 12 %.** La cuenta guarda las rutas en un arreglo y busca cada línea
  recorriéndolo.
- **`slow::render` es el 0,3 %.** Construir el texto concatenando en un bucle parecía el
  sospechoso habitual, y no pesa nada.

Esa última línea es la razón de medir: la intuición habría empezado por el sitio equivocado.

Dos consejos para leer el perfil. Algunas funciones de la biblioteca, como `position`, `get` o
`sort_by`, aparecen con su propio nombre; las operaciones básicas, como `split`, `to_string` o la
concatenación, se cuentan dentro de la función que las usa. Y un perfil solo distingue funciones:
si todo el trabajo está en una sola, pártela en pasos y vuelve a medir.

## 3. Arreglar el algoritmo

Esta es la cuenta de la primera versión, con su búsqueda lineal:

<!-- check: project=examples/apps/perf-lab -->
```rust
// Requests and total milliseconds per path.
fn count(lines: [string]) -> Tally {
    let t = Tally { paths: [], counts: [], totals: [] };
    for line in lines {
        let parts = line.split(" ");
        let path = parts[1];
        let ms = parts[3].parse_int().unwrap_or(0);
        // Linear search: every line scans the paths seen so far.
        match (t.paths.position(path)) {
            Option.Some(i) => {
                t.counts[i] = t.counts[i] + 1;
                t.totals[i] = t.totals[i] + ms;
            },
            Option.None => {
                t.paths.push(path);
                t.counts.push(1);
                t.totals.push(ms);
            },
        }
    }
    t
}
```

La segunda versión cambia el arreglo por un `Map`, donde encontrar una ruta cuesta lo mismo haya
diez o diez mil:

<!-- check: project=examples/apps/perf-lab -->
```rust
/// Requests and total milliseconds per path.
pub fn tally(lines: [string]) -> Map<string, Stat> {
    var stats: Map<string, Stat> = Map.new();
    for line in lines {
        let parts = line.split(" ");
        let path = parts[1];
        let ms = parts[3].parse_int().unwrap_or(0);
        // Hash lookup: constant time, however many paths there are.
        match (stats.get(path)) {
            Option.Some(s) => {
                s.count = s.count + 1;
                s.total = s.total + ms;
            },
            Option.None => stats.insert(path, Stat { path: path, count: 1, total: ms }),
        }
    }
    stats
}
```

Los structs tienen semántica de referencia: `s.count = s.count + 1` modifica el valor que está en
el mapa, sin volver a insertarlo. La ordenación a mano se sustituye por `sort_by`, y el texto se
compone juntando las líneas una sola vez:

<!-- check: project=examples/apps/perf-lab -->
```rust
/// The report text: one line per path, most requested first (ties by path).
pub fn render(stats: Map<string, Stat>) -> string {
    let sorted = sort_by(stats.values(), fn(a: Stat, b: Stat) -> bool {
        a.count > b.count || (a.count == b.count && a.path < b.path)
    });
    // Collect the lines and join them once.
    var out: [string] = [];
    for s in sorted {
        out.push(s.path + " " + to_string(s.count) + " " + to_string(s.total / s.count));
    }
    out.join("\n") + "\n"
}
```

```
fast: 200000 lines, 6005 paths, 141343 chars, 143 ms
```

De 7274 ms a 143 ms: **51 veces más rápido**, sin cambiar de motor. Un test comprueba que las dos
versiones producen exactamente el mismo informe; sin ese test, una optimización es una apuesta.

## 4. Compilar a nativo

`ray run` ejecuta el programa en la VM. `ray build --native` lo traduce a Rust y lo compila a
código máquina, con la misma salida byte a byte.

```sh
ray build --native --release -o perf-lab
./perf-lab fast 200000
```

| 200 000 líneas | VM | Nativo |
|---|---|---|
| Primera versión | 7274 ms | 614 ms |
| Versión con `Map` y `sort_by` | 143 ms | 23 ms |

El binario nativo es entre 6 y 12 veces más rápido en este programa, y usa la mitad de memoria
(124 MB frente a 242 MB con dos millones de líneas). Pero compilar la primera versión a nativo la
deja en 614 ms: cuatro veces más lenta que la versión buena **en la VM**. El algoritmo va primero.

Las opciones de compilación ajustan lo que queda:

- **`--release`** activa todas las optimizaciones de Rust. Aquí mejora un 20 % el trabajo con
  cadenas y mapas; tarda más en compilar, así que es para el entregable.
- **`--fast`** cambia la aritmética comprobada por aritmética que no detecta desbordamientos. En
  este programa no cambia nada medible; ayuda en bucles numéricos, y a cambio de una garantía.

Para comparar con otros lenguajes, la [página de benchmarks](https://raylang.dev/bench.html) mide
el binario nativo frente a Go, Rust y Node en catorce programas.

## 5. Usar más núcleos

Cada fibra de raylang tiene su propia memoria: no hay datos compartidos ni candados. Lanzar una
fibra por trozo de trabajo es directo, con una condición que hay que conocer. La fibra recibe una
**copia** de lo que captura, y su resultado se copia de vuelta.

<!-- check: project=examples/apps/perf-lab -->
```rust
/// The same report as `fast.report`, computed by `workers` fibers.
pub fn report(lines: [string], workers: int) -> string {
    let chunk = lines.len() / workers + 1;
    var tasks: [Task<Map<string, Stat>>] = [];
    var from = 0;
    while (from < lines.len()) {
        let part = lines.slice(from, from + chunk);
        tasks.push(spawn(fn() -> Map<string, Stat> { fast.tally(part) }));
        from = from + chunk;
    }
    // Merge: add up each path's counts across the slices.
    var merged: Map<string, Stat> = Map.new();
    for t in tasks {
        for (path, s) in join(t) {
            match (merged.get(path)) {
                Option.Some(m) => {
                    m.count = m.count + s.count;
                    m.total = m.total + s.total;
                },
                Option.None => merged.insert(path, s),
            }
        }
    }
    fast.render(merged)
}
```

| 2 000 000 de líneas | VM | Nativo |
|---|---|---|
| Una fibra | 1234 ms | 221 ms |
| Cuatro fibras | 460 ms | 111 ms |

Cuatro fibras dan el doble de velocidad en nativo y 2,7 veces en la VM, no cuatro: la copia de las
líneas a cada fibra tiene su coste. Con `RAYLANG_THREADS=1`, que fuerza un solo hilo, la versión
paralela tarda 296 ms: más que la de una fibra, porque paga las copias sin ganar núcleos. Repartir
el trabajo compensa cuando cada trozo cuesta bastante más que copiarlo.

## 6. Servidores

En un servidor web casi nada de lo anterior hace falta: cada conexión ya corre en su fibra y el
servidor usa todos los núcleos. Lo que importa ahí es otra cosa:

- **Un pool de conexiones** a la base de datos, compartido entre peticiones, como en el capítulo
  de la [API](api.md). Abrir una conexión por petición es el coste que más se nota.
- **El binario nativo.** El servidor del framework, compilado, sirve del orden de 188 000
  peticiones por segundo en el banco de carga del proyecto, con unos 21 KB por conexión.
- **`app.gzip()`** para las respuestas grandes, y `static_embedded` para los estáticos.

## En resumen

1. Mide con un reloj y una entrada fija.
2. Perfila con `ray profile` y cree a la tabla, no a la intuición.
3. Arregla el algoritmo: aquí, 51 veces.
4. Compila a nativo con `--release`: otras 6 veces.
5. Reparte entre núcleos solo el trabajo que pesa más que su copia: aquí, 2 veces.

## Siguiente paso

[**Distribuir**](shipping.md): firmar, publicar y actualizar lo que has construido.
