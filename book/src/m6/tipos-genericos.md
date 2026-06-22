# Tipos genéricos y chequeo bidireccional

Las funciones genéricas son útiles, pero la otra mitad —y la que habilita
`Option`/`Result`— son los **tipos** genéricos: enums y structs parametrizados.

```rust
enum Caja<T> { Llena(T), Vacia }
struct Par<A, B> { primero: A, segundo: B }
enum Lista<T> { Cons(T, Lista<T>), Nil }   // la lista de M5, ahora para cualquier T
```

## Tipos que llevan argumentos

Hasta aquí, `Type::Struct` y `Type::Enum` solo guardaban un nombre. Ahora llevan
también sus **argumentos de tipo**: `Caja<int>` es `Enum("Caja", [Int])`; `Par<int,
bool>` es `Struct("Par", [Int, Bool])`. Un tipo sin genéricos simplemente tiene la
lista vacía. Es la variante `Named(nombre, Vec<Type>)` que el diseño anticipó en M1,
conservando la distinción struct/enum que el checker ya usaba.

La igualdad de tipos, derivada, hace lo correcto sin esfuerzo: `Caja<int>` y
`Caja<bool>` son tipos **distintos** porque sus argumentos difieren. Y cada definición
guarda sus parámetros (`enum Caja<T>` recuerda que tiene un parámetro `T`), lo que da
dos cosas: la **aridad** (para validar que `Caja<int>` lleva un argumento y no dos) y
los **nombres** (para sustituir).

## Construir: inferir los argumentos de tipo

Al construir un valor genérico, sus argumentos de tipo se infieren —la misma
unificación de M6.1, aplicada al payload o a los campos—:

```rust
let a: Caja<int> = Caja.Llena(7);   // del argumento 7 sale T = int
let p: Par<int, bool> = Par { primero: 10, segundo: true };  // A=int, B=bool
```

`Caja.Llena(7)`: el payload declarado de `Llena` es `T`; el argumento es `int`;
unificar liga `T = int`, y el tipo del literal es `Caja<int>`.

## El caso que rompe la inferencia hacia adelante

Pero, ¿y `Caja.Vacia`? No tiene argumentos. De `Vacia` sola **no hay de dónde** sacar
`T`. Lo mismo le pasa al arreglo vacío `[]`, y le pasará a `None`. La inferencia "de
abajo hacia arriba" (de los valores a los tipos) se queda sin información.

La respuesta es mirar **hacia abajo** también: el **tipo esperado** del contexto. Si
escribes `let v: Caja<int> = Caja.Vacia`, el `Caja<int>` de la anotación le dice a la
construcción cuál es su `T`. Esto es el **chequeo bidireccional**: el tipo fluye en dos
direcciones —de las hojas hacia la raíz (inferencia) y de la raíz hacia las hojas (tipo
esperado)—.

En el checker, `check_expr` gana un compañero, `check_expr_expected`, que recibe un
tipo esperado. La mayoría de las expresiones lo ignoran; las que lo aprovechan son la
construcción (`Caja.Vacia`, `None`), el arreglo vacío, y las formas "transparentes"
—`if`, `match`, un bloque— que lo **propagan** a sus ramas. ¿De dónde sale el tipo
esperado? De los sitios que sí lo conocen: una anotación `let`, el `return` (que conoce
el retorno de la función), el cuerpo de la función (su tipo de retorno), y los
argumentos de una llamada (el tipo del parámetro).

> **Una aspereza vieja, resuelta de paso.** Desde M3, el arreglo vacío `[]` no se podía
> tipar solo (`let xs: [int] = []` necesitaba un caso especial torpe). El chequeo
> bidireccional lo subsume: `[]` con tipo esperado `[int]` *es* un `[int]`, sin reglas
> ad-hoc. A veces la forma correcta de arreglar un parche es construir la pieza que
> faltaba.

## Consumir: sustituir los argumentos

Si construir infiere los argumentos de tipo, consumir los **sustituye**. Cuando accedes
al campo de un struct genérico, o ligas el payload de un enum genérico en un `match`,
el tipo que obtienes sale de aplicar los argumentos del valor a la definición:

- `p.segundo` donde `p: Par<int, bool>`: el campo `segundo` se declaró de tipo `B`;
  con `σ = {A ↦ int, B ↦ bool}`, su tipo es `subst(B, σ) = bool`.
- `match (c) { Caja.Llena(v) => ... }` donde `c: Caja<int>`: el payload de `Llena` es
  `T`; `v` se liga como `subst(T, {T ↦ int}) = int`.

Así, dentro del `match`, `v` es un `int` de verdad, con todo lo que eso permite —y el
checker lo sabe—. La definición genérica es la plantilla; el valor concreto trae los
argumentos que la rellenan.

## Y el runtime, otra vez intacto

Como en M6.1, **nada de esto cambia el intérprete ni la VM**. Una `Caja<int>` es, en
runtime, un enum como cualquiera de M5; una `Lista<string>` se construye y se recorre
con el mismo código que la `Lista` no genérica. El test más exigente lo confirma: una
lista genérica recursiva, construida y sumada bajo el GC en **modo estrés**, da el
mismo resultado en los dos motores. Los argumentos de tipo se borraron; los valores
quedaron.

> Código: `src/ast.rs` (`Type::Struct/Enum` con `Vec<Type>`, `type_params` en las
> definiciones), `src/checker.rs` (`check_expr_expected`/`check_block_expected`,
> inferencia en `check_struct_lit`/`check_enum_lit`, sustitución en `check_field` y
> `check_pattern`).
