# Pipelines: el operador `|>`

El operador **pipeline** `|>` toma el valor de su izquierda y lo inserta como **primer
argumento** de la llamada de su derecha:

```rust
x |> f(a)        // ≡ f(x, a)
x |> f           // ≡ f(x)
```

Encadenado, convierte una expresión anidada —que se lee de dentro hacia afuera— en una
**tubería** que se lee de arriba abajo:

```rust
nums |> map(doble) |> filter(par) |> fold(0, suma)
// ≡ fold(filter(map(nums, doble), par), 0, suma)
```

Cada paso recibe el resultado del anterior. Es la misma idea que UFCS —meter el valor
como primer argumento—, con otra notación; por eso `xs.map(f)` y `xs |> map(f)`
significan lo mismo.

## Azúcar de parser, no de checker

Aquí está la diferencia instructiva con UFCS. Decidir qué hace `recv.f(args)` requería
**tipos** (¿`f` es campo o función?), así que UFCS vivía en el checker. El pipeline no
tiene esa duda: `x |> f(a)` **siempre** significa `f(x, a)`, sin mirar ningún tipo. Es
una transformación puramente **sintáctica**.

Y lo puramente sintáctico se resuelve en el sitio más temprano posible: el **parser**.
En cuanto el parser ve `x |> f(a)`, construye directamente el nodo `Call` de `f(x, a)`.
No hay un `ExprKind::Pipe` que el checker tenga que entender luego; el `|>` desaparece en
el mismo momento en que se lee. El checker y los dos motores **nunca** ven un pipeline.

> **Dos azúcares, dos capas.** UFCS y `|>` resuelven cosas parecidas (insertar un
> receptor como primer argumento) en capas distintas, y la razón es exactamente *cuánta
> información necesita cada uno*. UFCS necesita tipos → checker. El pipeline no → parser.
> Es un buen recordatorio de un principio de diseño de compiladores: **resuelve cada
> cosa en la fase más temprana que tenga la información suficiente**, y no más tarde.

## Precedencia: la más baja

El pipeline ocupa un nivel **nuevo** y el **más bajo** de la jerarquía de precedencia,
por debajo incluso de `||`. Su operando izquierdo es, por tanto, una expresión completa:

```rust
2 + 3 |> doble     // ≡ doble(2 + 3) = 10, no (2 + doble(3))
```

Y es **asociativo a la izquierda**, que es lo que hace que la tubería se lea en orden:

```rust
x |> f |> g        // ≡ g(f(x)), no f(g(x))
```

En la gramática, esto es una línea entre `expression` y `logic_or`:

```
pipeline = logic_or { '|>' call }
```

El operando **derecho** se parsea a nivel de `call` —un objetivo de llamada: `f`,
`f(args)`, `m.f(args)`—, no una expresión completa. Esto mantiene la regla simple y
evita ambigüedades; el precio es que para operar sobre el resultado de un pipeline hay
que parentizar:

```rust
(x |> f) + 1       // correcto
x |> f + 1         // error de sintaxis: el '+' no cabe ahí
```

Es un precio pequeño y predecible: un pipeline encadena llamadas con `|>`; si quieres
hacer aritmética con el resultado, lo cierras con paréntesis.

## El desugaring, en una función

Toda la lógica cabe en un ayudante del parser. Dado `recv |> rhs`:

- si `rhs` ya es una llamada `f(args)`, el resultado es `f(recv, args)` —el receptor se
  inserta delante de los argumentos existentes—;
- si `rhs` es cualquier otra expresión llamable `f`, el resultado es `f(recv)`.

El nodo resultante es un `Call` corriente. A partir de ahí, todo lo demás del pipeline
—componer con UFCS, con genéricos, con `?`— sale gratis, porque al checker le llega una
llamada como cualquier otra.

> Código: `src/token.rs` y `src/lexer.rs` (token `|>`), `src/parser.rs` (el nivel
> `pipeline` y el ayudante `make_pipeline`). Ni el checker ni el runtime cambian.
