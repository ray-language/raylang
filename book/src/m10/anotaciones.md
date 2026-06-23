# @test y @derive (Eq, Show)

M10.1 añade la **infraestructura** de anotaciones (parsearlas y adjuntarlas a las
declaraciones) más dos integradas. Todo es front-end; el runtime no cambia.

## La infraestructura

El `@` ya estaba reservado en el lexer (desde la limpieza post-M8). El parser, antes de cada
declaración de nivel superior, recoge las anotaciones que la preceden:

```text
annotations = { '@' IDENT [ '(' IDENT { ',' IDENT } ')' ] }
```

y las adjunta al ítem. En el AST, `Function`, `StructDef` y `EnumDef` ganan un campo
`annotations: Vec<Annotation>`. El checker valida que cada anotación sea **conocida** y esté
bien colocada (`@test` solo en funciones, `@derive` solo en tipos); una desconocida es error.

## `@test`: metadato leído por una herramienta

`@test` marca una función de prueba. Su firma debe ser `() -> bool` (pasa si devuelve
`true`). No cambia la ejecución normal —es una función más, que se ignora salvo en modo
prueba—.

```rust
@test
fn cuadrado_de_3() -> bool { cuadrado(3) == 9 }
```

El **runner** se invoca con `raylang archivo.ray --test`. Y aquí está lo elegante: es un
**cliente externo**, en el espíritu del REPL (M8.2) —usa solo la API pública, sin tocar el
checker ni el intérprete—. Como no hay forma de "ejecutar la función N", el runner
**sintetiza un `main`** que llama a cada prueba e informa el resultado:

```text
corriendo 3 prueba(s)

ok    cuadrado_de_3
ok    suma_ok
FALLO falla_a_proposito

resultado: 1 fallo(s) ✗
```

El código de salida es el **número de fallos** (0 = todas pasaron), útil en CI. El único
toque al núcleo fue la *validación* de la firma `@test`; el runner vive aparte
(`src/test_runner.rs`).

## `@derive(Eq)`: una anotación que genera código

Esta es la cara interesante —y el pago de M9—. `@derive(Eq)` sobre un struct/enum genera su
`impl Eq`, donde `Eq` es un trait del prelude:

```rust
trait Eq { fn igual(self, otro: Self) -> bool; }   // en el prelude

@derive(Eq)
enum Color { Rojo, Verde, Azul }
// genera: impl Eq for Color { fn igual(self, otro: Self) -> bool { ... } }
```

¿Por qué `igual` y no `==`? Porque `==` ya compara structs estructuralmente, pero **no**
enums (pueden ser recursivos / portar funciones). `@derive(Eq)` da una igualdad **explícita**
para enums (`a.igual(b)`), sin tocar la semántica de `==`.

**La implementación es sorprendentemente pequeña**, porque se apoya en todo lo anterior. El
checker no arma el `impl` nodo a nodo: **genera su fuente y lo parsea**, y deja que **M9**
haga el resto (bajarlo a `Color#igual`, registrarlo). El cuerpo de `igual`:

- **struct**: conjunción de los campos —`self.x == otro.x && self.y == otro.y`—.
- **enum**: `match` sobre `self`; por cada variante, `match` sobre `otro`: misma variante →
  comparar el payload posición a posición con `==`; otra → `false`.

```text
@derive(Eq) enum Forma { Circulo(int), Rect(int, int) }
        │
        ▼  (el checker genera y parsea este impl)
impl Eq for Forma {
    fn igual(self, otro: Self) -> bool {
        match (self) {
            Forma.Circulo(a0)    => match (otro) { Forma.Circulo(b0)    => a0 == b0,            _ => false },
            Forma.Rect(a0, a1)   => match (otro) { Forma.Rect(b0, b1)   => a0 == b0 && a1 == b1, _ => false },
        }
    }
}
```

Y como el resultado es un `impl Eq` normal, **compone con todo M9**: un tipo derivado
satisface un bound `T: Eq`, se puede usar en código genérico, etc.

```rust
fn iguales<T: Eq>(a: T, b: T) -> bool { a.igual(b) }
iguales(Color.Rojo, Color.Verde)   // false — funciona porque Color deriva Eq
```

**Límites de M10.1** (diferidos): las comparaciones hoja de `Eq` usan `==`, así que un payload
que sea **otro enum** no es comparable (derivación recursiva, futura); y `@derive` sobre un tipo
**genérico** no se admite (M9.1 no tiene impls genéricos).

### `@derive(Show)` (añadido en L2)

La consolidación post-M11 amplió el mecanismo a un segundo trait, `Show`, que genera
`mostrar(self) -> string` —una representación textual—. La generalización fue mínima
(`generate_derives`/`validate_derive` pasan a aceptar ambos), y `@derive(Eq, Show)` genera los
dos impls:

```rust
trait Show { fn mostrar(self) -> string; }   // en el prelude

@derive(Show)
struct Punto { x: int, y: int }
// "Punto { x: 3, y: 4 }"   ──  p.mostrar()
```

El cuerpo renderiza **por tipo**: primitivos vía `to_string`, struct/enum vía `mostrar()`
recursivo (los anidados deben implementar Show). Por eso `Show` **sí funciona con enums
recursivos** (una lista enlazada se imprime entera) donde `Eq` no llegaba: la recursión vive en
los datos, no impide la llamada a `mostrar`. Se difieren los campos de tipo arreglo/función
(error claro) y los tipos genéricos, como en `Eq`.

> **La lección.** `@derive(Eq)` ronda las ~60 líneas de checker, no porque hagamos trampa,
> sino porque cada capa previa carga su peso: el parser parsea el `impl` generado, M9 lo baja
> y lo despacha, los genéricos lo aceptan en bounds. Una anotación que genera código resultó
> ser, sobre todo, *reutilización*. Es la misma señal que dio M9: cuando las capas están bien
> puestas, la siguiente feature es barata.
