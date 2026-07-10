# Iteradores: perezosos y ansiosos

Desde M7.3 raylang tenía `map`, `filter` y `fold` **escritos en el propio lenguaje**, en el prelude: la
prueba de que el núcleo ya bastaba para su biblioteca. Eran funciones libres sobre arreglos —`map(xs, f)`
devuelve un `[U]` nuevo— y con UFCS y pipelines se leían como métodos encadenados: `xs.map(f).filter(g)`.
Funcionaban, pero tenían un límite que no se ve hasta que la cadena crece.

## El problema de la cadena ansiosa

`map(xs, f)` recorre `xs` entero y **materializa** un arreglo con los resultados. `filter` hace lo mismo.
Así que esta expresión, inocente a la vista:

```raylang
xs.map(doble).filter(es_par).map(cuadrado)
```

asigna **tres arreglos intermedios**: uno tras `map(doble)`, otro tras `filter(es_par)`, otro tras
`map(cuadrado)`. Si `xs` tiene un millón de elementos, se recorre tres veces y se reservan tres millones de
casillas que se tiran enseguida. La operación es *ansiosa* (eager): cada etapa termina su trabajo —todo su
trabajo— antes de que empiece la siguiente.

La alternativa es hacerlo *perezoso* (lazy): que ninguna etapa compute nada hasta que alguien pida el
siguiente elemento, y que ese elemento fluya por toda la cadena de una vez. Entonces no hay arreglos
intermedios: un solo recorrido, una casilla viva a la vez. A eso se le llama **fusión** de la tubería.

Para expresar la pereza hace falta una abstracción: el **iterador**.

## El trait `Iterator` y el `Iter<T>` respaldado por un closure

Un iterador es, en su forma mínima, algo que sabe darte **el siguiente elemento, o decir que se acabó**:

```raylang
trait Iterator<T> {
    fn next(self) -> Option<T>;
    // map, filter, take, skip, zip, enumerate, fold, collect … como métodos por defecto
}
```

`for x in it` (M40.2a) funciona sobre cualquier cosa que implemente este trait: el checker detecta el impl
y baja el bucle a llamadas a `next()` hasta el `None`.

La representación concreta es la parte astuta. raylang no permite un bound sobre un trait *parametrizado*
(`I: Iterator<T>`), así que no podemos escribir `fn map<I: Iterator<T>>(it: I, …)`. La salida es el
**type-erasure con un closure**:

```raylang
struct Iter<T> { paso: fn() -> Option<T> }
impl<T> Iterator<T> for Iter<T> {
    fn next(self) -> Option<T> { (self.paso)() }
}
```

Un `Iter<T>` es *una función con estado* que entrega el siguiente elemento. El estado (la posición del
cursor, cuántos quedan por saltar…) vive en variables **capturadas por el closure** —mutadas por
referencia, que es justo lo que dan las celdas de M4—. Así `iter(xs)`, `range(a, b)`, `.map(f)`,
`.filter(g)`, `.take(n)` producen **todos el mismo tipo** `Iter<T>` y se encadenan sin necesidad de ese
bound prohibido. Cada adaptador envuelve el `paso` del iterador previo en uno nuevo:

```raylang
fn map<U>(self, f: fn(T) -> U) -> Iter<U> {
    Iter { paso: fn() -> Option<U> {
        match (self.next()) {          // pide UN elemento al de más arriba…
            Option.Some(x) => Option.Some(f(x)),   // …lo transforma…
            Option.None => Option.None,            // …o propaga el fin.
        }
    } }
}
```

Nada se ejecuta al construir la cadena: `map` solo devuelve otro `Iter` que, *cuando le pidan* un elemento,
pedirá uno al de abajo y le aplicará `f`. El trabajo ocurre en las **operaciones terminales** —`collect()`
(materializa en `[T]`), `fold(init, f)` (reduce a un valor), `sum(it)`— que son las únicas que llaman a
`next()` en un bucle. Hasta entonces, la tubería es puro andamiaje.

```raylang
xs.iter().map(doble).filter(es_par).map(cuadrado).collect()
//        └──────── perezoso: nada corre ────────┘  └ aquí sí, un solo recorrido
```

Un millón de elementos, un recorrido, una casilla viva a la vez. Sin arreglos intermedios.

## Dos caras de la misma operación

Aquí llegamos a la decisión de diseño. Tenemos ahora **dos** formas de `map`/`filter`/`fold`:

| | Firma | Coste | Ergonomía |
|---|---|---|---|
| **Ansiosa** (función libre) | `map(xs: [T], f) -> [U]` | asigna un arreglo por etapa | el resultado es un `[U]` indexable; sin ceremonia |
| **Perezosa** (método del trait) | `map(self, f) -> Iter<U>` | fusiona; sin intermedios | pide `.iter()` al entrar y `.collect()` al salir |

Se eligen **por el tipo del receptor**, gracias a la resolución campo→método→UFCS (M9.1):

```raylang
xs.map(f)          // xs: [T]     → la FUNCIÓN LIBRE  → [U] ansioso
xs.iter().map(f)   // .iter(): Iter → el MÉTODO del trait → Iter<U> perezoso
```

La tentación es quedarse solo con la perezosa —es la que fusiona, la "correcta"— y borrar la ansiosa. Pero
eso obligaría a escribir `.iter()…​.collect()` **hasta para el caso trivial** `xs.map(f)`, y a que el
resultado ya no fuera indexable sin un paso extra. Para una tarea de una sola etapa, la ansiosa es más
clara. La lección es que **no hay una respuesta única**: la cara ansiosa gana en ergonomía para el caso
simple; la perezosa gana en coste para las cadenas largas. raylang ofrece las dos y deja la elección a
quien escribe. Saber *cuándo* usar cada una —¿es una etapa o cinco?, ¿el resultado se indexa o se vuelve a
recorrer?— es parte de programar con iteradores.

## Una sola fuente de verdad… y su precio (M40.6 → M62.1)

Ofrecer las dos caras no obliga a **implementar** las dos. Tener dos bucles que hacen lo mismo es un olor:
cada bug o mejora habría que arreglarlo por duplicado. Así que en M40.6 las funciones ansiosas dejaron de
tener cuerpo propio y pasaron a **delegar en la maquinaria perezosa**:

```raylang
fn map<T, U>(xs: [T], f: fn(T) -> U) -> [U] { iter(xs).map(f).collect() }   // M40.6
```

Elegante: la lógica en un solo sitio, las libres como envoltorios de una línea. Pero la elegancia tenía un
precio que nadie midió hasta la revisión de producción (M62.1): **cada `next()` cuesta una llamada a
closure + un `Option` asignado en el heap del GC + un `match`, POR ELEMENTO** — y la cara ansiosa, la que
usa todo el código real (`xs.map(f)`), lo pagaba entero más un `collect` intermedio. Medido con un millón
de elementos: el bucle `while` a mano tardaba 107 ms; `xs.map(f).fold(…)` re-fundado, **36 441 ms** (340×).

Así que M62.1 lo revirtió: las funciones libres ansiosas vuelven a ser **bucles directos** (317 ms — 114×
recuperados), y la maquinaria perezosa queda para lo que de verdad la necesita: fusionar cadenas y cortar
trabajo (`take` temprano sobre una fuente cara). La duplicación de tres bucles triviales resultó ser el
menor de los dos males.

La lección es doble. Primero, la de siempre: **la estética no se come — mide**. Y segundo, una más fina:
la pereza en raylang **corta trabajo, no acelera el trabajo que sí se hace**. Una cadena
`iter().map().filter().fold()` evita arreglos intermedios, pero cada elemento que fluye paga el peaje del
`next()` (~6 µs). Si el cuello es el *throughput* de una sola etapa, el bucle (o la libre ansiosa) gana;
si el cuello es *cuántos* elementos tocas (un `take(10)` sobre un millón), la perezosa gana por paliza.

## La letra pequeña de la semántica

Tres comportamientos que conviene saber antes de que te sorprendan (los tres verificados, idénticos en
ambos motores):

- **`for x in xs` congela la longitud; `for x in xs.iter()` es una vista viva.** El `for` directo sobre
  un arreglo captura `len` al entrar: si el cuerpo hace `push`, los elementos nuevos NO se visitan. El
  `Iter` de `xs.iter()`, en cambio, relee `xs.len()` en cada paso: los elementos añadidos durante la
  iteración SÍ se visitan (y un `push` incondicional no termina nunca). Mutar lo que estás recorriendo es
  mala idea en general; si lo haces, ahora sabes qué hace cada forma.
- **Un iterador es one-shot y el aliasing se nota.** Dos adaptadores construidos sobre el MISMO `iter()`
  comparten el cursor: alternar `next()` entre ellos intercala el consumo. Construye un `iter()` nuevo
  por cadena.
- **`zip` puede descartar un elemento.** Cuando el lado corto se agota, el elemento que `zip` ya había
  pedido al lado largo se pierde (igual que en Rust). Si vas a seguir usando el iterador largo después
  del `zip`, cuenta con ese hueco.

## El resto del juego

Sobre esta base, el conjunto de adaptadores del trait se completa con lo esperable de una librería de
iteradores moderna, todo como métodos por defecto (puro prelude, cero runtime):

- **Perezosos** (devuelven `Iter`): `map`, `filter`, `take(n)` (corta), `skip(n)`, `enumerate()` (pares
  `(int, T)`), `zip(otra)` (pares `(T, U)`).
- **Terminales** (consumen): `fold(init, f)`, `collect() -> [T]`, `any(pred)`/`all(pred)` (con
  cortocircuito: sobre una cadena perezosa solo se evalúa hasta la primera respuesta), `count()`, y las
  funciones libres `sum(it: Iter<int>)` / `sum_float(it: Iter<float>)`. `any`/`all` existen también como
  libres ansiosas sobre arreglos (mismo nombre; resuelve el tipo del receptor, como `map`).

`sum` quedó como función libre y no como método por una razón instructiva: un `sum` genérico necesitaría un
cero y un `+` del tipo del elemento —es decir, un trait `Zero`/`Sum` que raylang no tiene—. Sin él, `sum`
solo puede prometer enteros. Es un recordatorio de que cada abstracción cómoda descansa sobre otra que hay
que construir primero.
