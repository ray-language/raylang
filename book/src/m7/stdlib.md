# La stdlib en el propio lenguaje

La última pieza de M7 es la **biblioteca estándar**: las funciones de orden superior
`map`, `filter` y `fold`. Y la decisión de diseño que las define es que **no están
incrustadas en el compilador**: están escritas en raylang.

```rust
fn map<T, U>(xs: [T], f: fn(T) -> U) -> [U] {
    var out: [U] = [];
    var i: int = 0;
    while (i < len(xs)) {
        push(out, f(xs[i]));
        i = i + 1;
    }
    out
}

fn filter<T>(xs: [T], pred: fn(T) -> bool) -> [T] { /* análogo, con un if */ }
fn fold<T, A>(xs: [T], init: A, f: fn(A, T) -> A) -> A { /* acumula desde init */ }
```

Mira lo que usa `map` y de dónde viene cada cosa:

| Pieza | Hito |
|-------|------|
| genéricos `<T, U>` | M6 |
| una función como parámetro (`f: fn(T) -> U`) y su llamada | M4 |
| `var`, `while`, `[]` con tipo esperado, indexación | M1–M6 |
| los builtins `len` y `push` | M3 |

No hay nada de M7 en el cuerpo de `map`. **La biblioteca estándar es solo un programa
raylang** que se apoya en todo lo construido antes. Esa es la afirmación que M7.3
demuestra: el lenguaje ya era lo bastante expresivo como para escribir su propia
librería.

## Cómo entran: el prelude, otra vez

`map`/`filter`/`fold` viven en el mismo **prelude** que `Option`/`Result` (`src/prelude.rs`):
una cadena de fuente raylang que se parsea una vez y se **inyecta** al principio de cada
programa, en `check()`. Sus funciones se anteponen a las del usuario, y a partir de ahí
son indistinguibles de cualquier función definida a mano —el checker las verifica, el
intérprete y la VM las ejecutan—.

Hay un matiz: la inyección **salta** las funciones del prelude cuyo nombre el usuario ya
haya definido. Eso da dos cosas a la vez: **override** (si escribes tu propio `map`, gana
el tuyo) e **idempotencia** (verificar dos veces no duplica nada).

> **Por qué "librería en el lenguaje" no es un capricho.** Si `map` fuera un builtin en
> Rust, sería una caja negra: el usuario no podría leerlo, copiarlo ni variarlo. Al ser
> raylang, `map` es tan inspeccionable y modificable como el código del usuario, y prueba
> que el mecanismo es **general**: lo que el compilador ofrece (genéricos, closures,
> arreglos) basta para que cualquiera escriba su `zip`, su `take` o su `reduce`. El
> compilador da el poder; la librería elige cómo usarlo.

## Donde UFCS y los pipelines lucen

`map`/`filter`/`fold` son el ejemplo perfecto del "único mecanismo de llamada" de §0. El
mismo cálculo se puede escribir de las tres formas, y todas producen el mismo AST:

```rust
fold(filter(map(xs, doble), par), 0, suma)           // anidado
xs.map(doble).filter(par).fold(0, suma)               // UFCS
xs |> map(doble) |> filter(par) |> fold(0, suma)      // pipeline
```

La versión anidada es correcta pero se lee al revés (de dentro hacia afuera). UFCS y el
pipeline la enderezan. Sin una stdlib de orden superior, el azúcar de M7.1 y M7.2 no
tendría dónde brillar; sin el azúcar, la stdlib se leería incómoda. Las tres sub-fases se
necesitan mutuamente, y por eso forman un solo hito.

## Lo que se dejó fuera: los builtins de string

La stdlib de M7.3 es deliberadamente pequeña: solo el orden superior sobre arreglos. Los
builtins de **string** (`trim`, `split`, `to_string`) se **difieren**, y por una razón
arquitectónica concreta: a diferencia del prelude, **no** se pueden escribir en raylang.
Necesitan tocar el runtime.

En la VM, los builtins son **opcodes dedicados** (`Print`, `Len`, `Push`). Añadir `trim`
o `split` significa un opcode nuevo por cada uno —con su emisión en el compilador, su
manejo en la VM, su réplica en el intérprete y su tipado en el checker—; y `split`, que
devuelve un `[string]`, además aloja un arreglo en el heap. Es trabajo de runtime
**ortogonal** al azúcar de M7, y se aborda mejor como una expansión futura de la stdlib,
no mezclado con la ergonomía de la llamada.

## M7 completo

raylang cumple el último gran objetivo de su norte: las llamadas, los métodos y las
tuberías son **un solo mecanismo**, y la biblioteca con la que se programan está escrita
en el propio lenguaje. Y todo M7 resultó ser **front-end puro**: ni un opcode nuevo en la
VM. El siguiente hito, **M8**, mira hacia la comodidad del que escribe: inferencia de
tipos locales (`let x = 3` sin anotación), un REPL y mejores mensajes de error.

> Código: `src/prelude.rs` (`SOURCE` con `map`/`filter`/`fold`, `functions()`),
> `src/checker.rs` (la inyección en `check()` con override). El runtime, una vez más,
> intacto.
