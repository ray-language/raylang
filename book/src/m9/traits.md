# trait e impl: despacho estático

M9.1 añade dos formas nuevas al lenguaje —`trait` e `impl`— y **ni un solo opcode** al
runtime. Toda la magia ocurre en el front-end. Veamos cómo.

## La sintaxis

Un `trait` lista firmas de métodos; cada una termina en `;` (sin cuerpo). El primer
parámetro es siempre `self`, el receptor, cuyo tipo es el que implemente el trait. Un
`impl Trait for Tipo` da los cuerpos.

```rust
trait Valor {
    fn valor(self) -> int;
}

struct Punto { x: int, y: int }

impl Valor for Punto {
    fn valor(self) -> int { self.x + self.y }
}
```

Dentro de una firma, el tipo **`Self`** denota el tipo implementador —útil cuando un
método devuelve "uno de los míos":

```rust
trait Punteable {
    fn doble(self) -> Self;     // devuelve un Punto cuando se implementa para Punto
}
```

Un trait se puede implementar para un **struct**, un **enum** (con `match` dentro del
método) o incluso un **primitivo**:

```rust
impl Valor for int {
    fn valor(self) -> int { self }
}

// ahora 42.valor() es válido y vale 42
```

## La idea clave: un método *es* una función

El truco que hace a M9.1 front-end puro es darse cuenta de que un método de impl no es
nada nuevo: es una **función ordinaria** cuyo primer parámetro es `self`. Así que el
checker, antes de verificar nada, **baja** cada método de impl a una función libre:

```text
impl Valor for Punto { fn valor(self) -> int { self.x + self.y } }
        │
        ▼  (bajada en el checker)
fn «Punto#valor»(self: Punto) -> int { self.x + self.y }
```

El nombre lleva un `#` (*name mangling*) que el usuario no puede escribir, así nunca
choca con sus funciones. Estas funciones mangladas se inyectan en la lista de funciones
del programa, y a partir de ahí **todo el pipeline las trata como cualquier otra
función**: registro de firmas, chequeo de cuerpos, el intérprete, la VM. El runtime no
sabe que existen los traits.

> Es el mismo patrón de *erasure* que los genéricos (M6): los traits son una construcción
> del checker que se **borra** antes de ejecutar. El precio de esa elegancia es que M9.1
> solo hace despacho estático; el dinámico (M9.3) sí tocará el runtime.

## Resolver `recv.metodo()`

Cuando el checker ve `p.valor()`, ya conoce el tipo de `p` (es `Punto`). La resolución
de la llamada por punto sigue ahora **tres pasos, en orden de prioridad**:

1. ¿Es `valor` un **campo** del struct receptor? (semántica de M3: invocar el valor del
   campo). Tiene la máxima prioridad.
2. ¿Es un **método de trait** del tipo concreto del receptor? (lo nuevo de M9.1).
3. Si no, ¿es una **función libre**? (UFCS de M7.1).

Para el paso 2, el checker mantiene una tabla `(tipo, método) → función manglada`. Al
encontrar `(Punto, valor)`, sabe que la llamada debe ir a `«Punto#valor»`, con `p` como
primer argumento. Registra el sitio y, tras verificar, una pasada de *lowering*
reescribe el árbol:

```text
p.valor()   ─►   «Punto#valor»(p)
```

Esta pasada de lowering es **la misma** que ya bajaba UFCS. Lo único que cambió es que el
registro de sitios pasó de "qué nombres bajar" a "qué nombre bajar **y a qué función
destino**": para UFCS de función libre el destino es el mismo nombre; para un método de
trait, el manglado. Un detalle pequeño que deja convivir las dos formas sin duplicar
código.

## Lo que el checker valida

Implementar un trait es cumplir un contrato, así que el checker comprueba que el `impl`
**casa exactamente** con el `trait`:

- **Cobertura**: el impl provee todos los métodos del trait, ni uno menos.
- **Sin sobras**: cada método del impl pertenece al trait.
- **Firmas idénticas**: sustituyendo `Self` por el tipo implementador en ambos lados, los
  tipos de parámetros y el retorno coinciden.
- **Sin ambigüedad**: un mismo tipo no puede recibir dos métodos homónimos de traits
  distintos (no habría forma de decidir cuál llamar en `recv.f()`).

Los mensajes de error reusan el contexto de fuente de M8.3:

```text
error de tipos en 3:16: el método 'a' devuelve bool, pero el trait pide int
  3 | impl T for S { fn a(self) -> bool { true } }
    |                ^
```

## Lo que M9.1 deja fuera

Por disciplina de "una fase a la vez", M9.1 difiere lo que añade complejidad real:

- **Impls genéricos** (`impl Valor for Caja<T>`) → M9.2 (requieren parámetros de tipo en
  el impl).
- **Bounds** (`fn f<T: Valor>(x: T)`) → M9.2. Es el salto difícil: dentro de código
  genérico, `T` ya no existe en runtime, así que no se puede "elegir el impl en tiempo de
  chequeo". Habrá que decidir entre paso de diccionarios, monomorfización o despacho por
  tipo en runtime.
- **Métodos por defecto** y **trait objects** (despacho dinámico) → M9.3.

Con M9.1, raylang ya tiene la pieza central del polimorfismo: contratos de comportamiento,
implementados por tipo, resueltos sin coste en runtime.
