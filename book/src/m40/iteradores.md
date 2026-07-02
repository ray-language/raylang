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

## Una sola fuente de verdad (M40.6)

Ofrecer las dos caras no obliga a **implementar** las dos. Tener dos bucles que hacen lo mismo es un olor:
cada bug o mejora habría que arreglarlo por duplicado. Así que las funciones ansiosas dejaron de tener
cuerpo propio y pasaron a **delegar en la maquinaria perezosa**:

```raylang
fn map<T, U>(xs: [T], f: fn(T) -> U) -> [U] { iter(xs).map(f).collect() }
fn filter<T>(xs: [T], pred: fn(T) -> bool) -> [T] { iter(xs).filter(pred).collect() }
fn fold<T, A>(xs: [T], init: A, f: fn(A, T) -> A) -> A { iter(xs).fold(init, f) }
```

La lógica de `map`/`filter`/`fold` vive ahora en **un solo sitio** —los métodos del trait—; las funciones
libres son un envoltorio de una línea que añade el `iter()…collect()` por comodidad. La cara ansiosa
conserva su firma y su semántica exactas (un `[U]` recién materializado), pero por dentro es la perezosa
con las tapas puestas.

¿Y no se recursiona? `map<T,U>(xs)` llama a `iter(xs).map(f)`. Pero `iter(xs)` es un `Iter<T>`, y sobre un
receptor `Iter` la resolución campo→método→UFCS elige el **método del trait**, nunca la función libre. El
despacho por tipo de receptor las mantiene separadas sin ambigüedad.

Nótese que la cara ansiosa, aun re-fundada, **no fusiona**: `xs.map(f).filter(g)` sigue materializando
entre etapas, porque cada función libre devuelve un `[T]` completo. Eso es inherente a su firma, no un
defecto de la implementación: si quieres fusión, entras por `.iter()`. Re-fundar eliminó la *duplicación de
código*, no la *distinción de coste* —que es real y deliberada—.

## El resto del juego

Sobre esta base, el conjunto de adaptadores del trait se completa con lo esperable de una librería de
iteradores moderna, todo como métodos por defecto (puro prelude, cero runtime):

- **Perezosos** (devuelven `Iter`): `map`, `filter`, `take(n)` (corta), `skip(n)`, `enumerate()` (pares
  `(int, T)`), `zip(otra)` (pares `(T, U)`).
- **Terminales** (consumen): `fold(init, f)`, `collect() -> [T]`, y la función libre `sum(it: Iter<int>)`.

`sum` quedó como función libre y no como método por una razón instructiva: un `sum` genérico necesitaría un
cero y un `+` del tipo del elemento —es decir, un trait `Zero`/`Sum` que raylang no tiene—. Sin él, `sum`
solo puede prometer enteros. Es un recordatorio de que cada abstracción cómoda descansa sobre otra que hay
que construir primero.
