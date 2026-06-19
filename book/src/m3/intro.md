# Datos compuestos

Hasta aquí raylang solo sabía hablar de **valores sueltos**: un `int`, un `bool`,
un `float`. Con ellos y la recursión se puede calcular cualquier cosa —`fib`, `gcd`,
primos—, pero no se puede *modelar* casi nada: un punto en el plano, una lista de
números, un rectángulo. M3 le da a raylang sus primeros **tipos compuestos**:
**arreglos** (`[int]`) y **structs** (`Punto { x, y }`).

## Lo que cambia (y lo que no)

Igual que en M2, el front-end ya construido carga con casi todo el peso. El lexer
gana dos tokens (`[ ]` y `.`) y una palabra clave (`struct`); el resto del trabajo
es **extender cada fase** con las formas nuevas: el parser aprende literales y
acceso, el checker aprende a tiparlos, y —la parte interesante— **los dos motores de
ejecución** (intérprete y VM) aprenden a representarlos en memoria.

Lo que **no** cambia es la estrategia: cada cosa que agregamos a la VM se verifica
contra el **oráculo** —el intérprete de M1—, así que un arreglo o un struct solo se
da por bueno cuando ambos motores producen el mismo resultado.

## La decisión transversal: semántica de referencia

Antes de escribir una línea había que decidir algo que tiñe todo M3: cuando haces

```rust
let a = [1, 2, 3];
let b = a;
```

¿`b` es una **copia** de `a`, o **el mismo arreglo** con otro nombre? Las dos
respuestas son legítimas y definen lenguajes distintos:

- **Semántica de valor** (como los `struct` de Rust o C): `b` es una copia. Mutar
  `b` no toca a `a`. Simple de razonar, pero copiar estructuras grandes es caro y
  obliga a pensar en *quién posee qué*.
- **Semántica de referencia** (como Python, Java, JS): `a` y `b` apuntan al **mismo
  objeto en el heap**. Mutar por uno se ve por el otro. Es lo que la gente espera de
  listas y objetos, y es barato (se copia un puntero, no los datos).

Elegimos **referencia**. Es la decisión que tomamos al arrancar M3, y la que hace
que este programa imprima `99`:

```rust
struct Punto { x: int, y: int }
struct Rect { origen: Punto, ancho: int }

fn main() -> int {
    var p = Punto { x: 3, y: 4 };
    var r = Rect { origen: p, ancho: 10 };
    r.origen.x = 99;   // muta el Punto compartido…
    p.x                // …y se ve aquí: p y r.origen son el mismo objeto
}
```

### Cómo se ve la referencia en Rust: `Rc<RefCell<…>>`

Esa decisión tiene una traducción directa al runtime. Un `Value` compuesto se
guarda así:

```rust
enum Value {
    // … primitivos …
    Array(Rc<RefCell<Vec<Value>>>),
    Struct(Rc<RefCell<StructInstance>>),
}
```

Cada mitad del envoltorio resuelve una necesidad:

- **`Rc`** (*reference counted*) da el **compartir**. Clonar un `Value::Array` clona
  el `Rc` —un puntero con un contador—, no el `Vec`. Así `let b = a;` deja a `a` y
  `b` apuntando al mismo `Vec`, y el dato vive mientras alguien lo referencie.
- **`RefCell`** da la **mutación interior**. Un `Rc` solo presta lecturas; para
  poder hacer `a[0] = 9` a través de una referencia compartida necesitamos mutar lo
  que hay dentro, y `RefCell` mueve esa verificación de préstamos de *compilación* a
  *ejecución*.

> **Una deuda que asumimos a conciencia.** `Rc` no sabe romper **ciclos**: si un
> struct llega a apuntarse a sí mismo (directa o indirectamente), su contador nunca
> llega a cero y la memoria se filtra. Lo aceptamos a propósito —es el problema que
> **M4 resolverá con un recolector de basura** real que sustituya al `Rc`. M3 se
> concentra en la *forma* de los datos; M4, en su *vida*.

## Mutabilidad: la variable, no el objeto

La semántica de referencia obliga a afinar qué significan `let` y `var`. En raylang
controlan **reasignar la variable**, no congelar el objeto:

```rust
let a: [int] = [1, 2];
a = [3];      // ❌ error: 'a' es inmutable (no puedes reatar el nombre)
a[0] = 9;     // ✅ ok: mutas el objeto, no la variable
push(a, 3);   // ✅ ok: igual
```

Es el modelo de `const` en JavaScript: ata el **nombre**, no el contenido. Por eso
el checker exige `var` para reasignar una variable, pero **no** para `a[i] = x` ni
`p.campo = x` —ahí no estás reatando nada, estás mutando el objeto compartido. La
inmutabilidad profunda queda como un refinamiento posible para más adelante.

## El plan de M3

Dos sub-fases, cada una testeable contra el oráculo:

1. **Arreglos** — el tipo `[T]`, literales, indexar, asignar elementos, y los
   builtins `len`/`push`. El primer dato que vive en el heap.
2. **Structs** — declaración a nivel superior, literales nombrados, acceso y
   asignación de campo, y tipado **nominal**.

Empecemos por los arreglos.
