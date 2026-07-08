# Bounds: genéricos que exigen comportamiento

M9.1 resuelve `recv.metodo()` cuando se conoce el tipo concreto del receptor. Pero
¿y dentro de una función genérica, donde el tipo es un parámetro `T`?

```rust
fn imprimir_todo<T>(xs: [T]) {
    // ... xs[0].show() ...   // ERROR: ¿qué es T? ¿sabe 'mostrar'?
}
```

El checker no puede permitir `xs[0].show()`: `T` podría ser cualquier cosa, y no toda
cosa sabe mostrarse. Un **bound** arregla esto: acota `T` a los tipos que implementan un
trait.

```rust
fn imprimir_todo<T: Mostrable>(xs: [T]) {
    // ... xs[0].show() ...   // OK: T: Mostrable lo garantiza
}
```

Ahora `imprimir_todo` sirve para **cualquier** tipo con un `impl Mostrable` —`Punto`, `int`,
los que vengan— y el checker acepta `.show()` porque el bound es una promesa: quien llame
debe pasar un `T` que cumpla.

## El problema del *erasure*

Aquí choca con una invariante del proyecto. Desde M6, los genéricos son **erasure**: `T` se
borra antes de ejecutar y hay **una sola copia** compilada de `imprimir_todo`, para todos
los `T`. Pero entonces, en runtime, ¿qué función concreta es `.show()`? La de `Punto`
suma sus campos; la de `int` se devuelve a sí mismo. Con `T` borrado, el cuerpo único no
sabe a cuál llamar.

Hay tres salidas clásicas. Elegimos **paso de diccionarios** (las otras —monomorfización y
despacho por tipo en runtime— se discuten en el capítulo de diseño; ninguna conserva a la
vez *erasure* y *runtime intacto*).

## La idea: pasar el método como un argumento oculto

Lo que el bound aporta es "saber cómo llamar los métodos del trait para `T`". Eso es un
**diccionario**: el conjunto de funciones del impl. Y raylang ya sabe pasar funciones como
valores (M4). Así que un bound se baja a **parámetros ocultos de tipo función**:

```text
fn imprimir<T: Mostrable>(x: T) { ... x.show() ... }
        │
        ▼  (el checker añade un parámetro oculto)
fn imprimir<T>(x: T, «T#Mostrable#mostrar»: fn(T) -> string) {
    ... «T#Mostrable#mostrar»(x) ...
}
```

Dos reescrituras coordinadas, ambas en el front-end:

1. La función gana un parámetro función por cada método del trait acotado. Su nombre lleva
   `#` para no chocar con nada que el usuario pueda escribir.
2. La llamada de método `x.show()` se baja a una llamada a ese parámetro:
   `«T#Mostrable#mostrar»(x)`. (Es el mismo *lowering* de UFCS/M9.1; solo cambia el destino.)

## Quién llena el diccionario: el sitio de llamada

El parámetro oculto hay que rellenarlo al llamar. Y ahí **sí** se conoce el tipo concreto,
porque la inferencia de M6 ya calcula `σ` (qué tipo es cada parámetro). Con `σ` en mano, en
cada llamada a una función con bounds se **añaden los argumentos** correspondientes:

```rust
imprimir(p)        // σ: T = Punto  →  imprimir(p, «Punto#mostrar»)
```

Se pasa `«Punto#mostrar»`: el método del `impl Mostrable for Punto` que M9.1 ya había bajado
a una función ordinaria. Aquí se **verifica el bound**: si el tipo concreto no implementa el
trait, error en el sitio de llamada.

```text
error de tipos en 5:34: Punto no implementa 'Valor' (requerido por la llamada)
```

## Diccionarios que se reenvían

¿Y si un genérico acotado llama a **otro** genérico acotado?

```rust
fn resumen<T: Mostrable>(a: T, b: T) {
    imprimir(a)        // T sigue siendo un parámetro, no un tipo concreto
}
```

Dentro de `resumen`, `T` no es `Punto` ni `int`: es un parámetro de tipo rígido. La
inferencia da `σ: (T de imprimir) = (T de resumen)`. No hay un `«T#mostrar»` concreto que
pasar... pero `resumen` **ya recibió** su propio diccionario para `T`. Así que se **reenvía**:

```text
resumen<T>(a, b, «T#Mostrable#mostrar»)   // resumen recibe el diccionario...
    │
    └─ imprimir(a, «T#Mostrable#mostrar»)  // ...y se lo pasa a imprimir
```

Este reenvío es lo que hace **componer** a los genéricos acotados entre sí. El checker lo
detecta: cuando `σ[T]` resuelve a un parámetro de tipo del llamador, exige que el llamador
tenga el mismo bound (si no, error) y reenvía su diccionario.

## Runtime: sin cambios

Los diccionarios son **valores función**, que el intérprete y la VM ya saben pasar y llamar
desde M4. Cero opcodes nuevos, cero cambios en los motores; el oráculo VM↔intérprete sigue
valiendo sin tocar `vm.rs`.

Y el despacho sigue siendo, en esencia, **estático**: *qué* función viaja en cada diccionario
se decide en tiempo de chequeo, en el sitio de llamada; en runtime solo se invoca el valor ya
elegido. La lección de M9.2 es bonita: las **funciones de primera clase** de M4 bastaban,
por sí solas, para construir polimorfismo acotado encima —sin añadir ni una primitiva nueva
al lenguaje ni al runtime—.

## Bounds en struct/enum (M9.4)

Hasta aquí solo las **funciones** y los **impls** acotaban sus parámetros de tipo. M9.4 lo lleva a
los **tipos del usuario**: `struct Caja<T: Show> { v: T }`, `enum Lista<T: Eq> { … }`.

La diferencia clave con las funciones: **un struct/enum es datos, no llama a nada**. El bound no
dispara ningún método, así que **no hay diccionarios que pasar** —ni *lowering*, ni opcodes—. El
bound es una **promesa que el checker verifica en cada construcción**:

```rust
struct Caja<T: Show> { v: T }
let c = Caja { v: 5 };        // OK si int: Show; error si no
```

Tras inferir `T = typeof(v)`, ese tipo debe **satisfacer** el bound. La comprobación reusa la misma
lógica de satisfacción que los diccionarios de M9.2 (`satisfies_bound`): un impl concreto del trait,
o un parámetro de tipo del llamador que ya declara el mismo bound. De ahí sale la **propagación
gratis**:

```rust
fn env<U>(x: U) -> Caja<U> { Caja { v: x } }       // error: U podría no ser Show
fn env<U: Show>(x: U) -> Caja<U> { Caja { v: x } } // OK: U lleva el bound, se reenvía
```

Construir `Caja<U>` exige que `U` lleve el bound; si no, `U` no lo satisface y falla. (No se exige en
una función que solo *recibe* un `Caja<U>` sin construirlo: el `impl<T: Show> … for Caja<T>` ya
reexige `T: Show` al llamar a sus métodos, así que no hay agujero.) **Runtime intacto, erasure total.**

## Lo que sigue

- **Impls genéricos** (`impl<T: Mostrable> Mostrable for Caja<T>`): el diccionario de `Caja<int>`
  necesita a su vez el de `int` —diccionarios *anidados*—. Es M9.2b (capítulo siguiente).
- **Métodos por defecto** y **trait objects** (despacho dinámico) → M9.3.
