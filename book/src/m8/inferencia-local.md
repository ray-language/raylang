# Inferencia local: `let x = 3`

Desde M1, declarar una variable exigía anotar su tipo: `let x: int = 3;`. Tenía sentido
como punto de partida —obliga a pensar en tipos—, pero en la práctica es ruido cuando el
tipo es evidente. M8.1 hace la anotación **opcional**:

```rust
let x = 3;                      // int
let nombre = "ana";             // string
let xs = [1, 2, 3];             // [int]
let p = Punto { x: 1, y: 2 };   // Punto
let c = Caja.Llena(7);          // Caja<int>, con los genéricos de M6
var total = 0;                  // var también; sigue siendo mutable
```

## El cambio, de punta a punta

Es uno de los cambios más pequeños del proyecto, repartido en tres puntos:

- **AST**: el campo `ty` de `StmtKind::Let` pasa de `Type` a `Option<Type>` —`None` cuando
  no se anotó—.
- **Parser**: la anotación `: tipo` se vuelve opcional. `let x = e;` ya es válido.
- **Checker**: si hay anotación, todo sigue igual (el tipo declarado es el **esperado** del
  inicializador, el chequeo bidireccional de M6.2). Si **no** la hay, se **infiere** tipando
  el inicializador sin tipo esperado, y la variable se declara con el tipo resultante.

Eso es todo. La inferencia local *es* "preguntarle al inicializador qué tipo tiene y usar
ese". No hace falta maquinaria nueva: el checker ya sabía calcular el tipo de cualquier
expresión.

## Lo que no se puede inferir

Algunos inicializadores **no determinan su tipo solos**: el arreglo vacío `[]`,
`Option.None`, `Caja.Vacia`. De `[]` no hay de dónde sacar el tipo de elemento. Para esos,
la anotación sigue siendo necesaria —es justo el caso que el **tipo esperado** de M6.2
resolvía—. Sin ella, el checker ya falla con un mensaje que **pide la anotación**:

```text
error de tipos: no se puede inferir el tipo de [] aquí; anótalo (p. ej. let xs: [int] = [];)
```

La regla, dicha en una frase: **se infiere lo que el valor determina; lo que no, se anota.**
Y la inferencia es siempre **local** —de la expresión del `=`—, nunca por uso posterior
(deducir el `T` de un `[]` por un `push` más adelante sería análisis de flujo, y no se hace).

## Por qué esto no rompe la §0

La §0 prometió "un type checker real **sin el coste de la inferencia global**". ¿No es
inferir tipos justamente lo que se quería evitar? No: hay dos cosas muy distintas bajo el
mismo nombre.

- Inferir `let x = 3` es **local y trivial**: el tipo está *ahí mismo*, en el inicializador.
  No hay incógnitas que propagar entre sentencias.
- Inferir las **firmas** (`fn f(n) { ... }` sin tipos) sería **global**: habría que
  resolver un sistema de ecuaciones por todo el programa, con recursión mutua y todo.

M8.1 hace lo primero y deja lo segundo fuera a propósito. Las firmas siguen explícitas,
que es donde una anotación de verdad documenta —y donde ancla la inferencia local, porque
los argumentos de una llamada toman su tipo esperado del parámetro—. Es la línea exacta
entre "comodidad barata" e "inferencia global".

## Y el runtime, intacto

Como con los genéricos, **nada de esto llega al runtime**. Los tipos se borran antes de
ejecutar; una variable inferida es, en tiempo de ejecución, una variable como cualquier
otra. La inferencia local es, una vez más, **solo del checker**.

> Código: `src/ast.rs` (`StmtKind::Let.ty: Option<Type>`), `src/parser.rs` (anotación
> opcional en `let_stmt`), `src/checker.rs` (la rama `None` en `StmtKind::Let`).
