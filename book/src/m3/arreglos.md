# Arreglos

El arreglo es el primer dato de raylang que **vive en el heap** y **crece**. Un
arreglo es una lista dinámica de elementos del mismo tipo: `[1, 2, 3]` tiene tipo
`[int]`, `[[true], []]` tiene tipo `[[bool]]`.

## El tipo `[T]`: estructural

El tipo de un arreglo es `[T]`, donde `T` es el tipo del elemento. En el `Type` del
AST eso es una variante recursiva:

```rust
enum Type {
    Int, Float, Bool, String, Unit,
    Array(Box<Type>),   // ← nuevo: [T]
    Struct(String),
}
```

El `Box` es obligatorio: sin él, `Type` se contendría a sí mismo y tendría tamaño
infinito. Con él, un `[int]` es `Array(Box::new(Int))` y un `[[int]]` anida.

Los arreglos usan tipado **estructural**: dos `[int]` son el mismo tipo siempre, no
importa de dónde vengan. (Contrasta con los structs, que veremos son **nominales**.)
La diferencia es natural: un arreglo *es* su forma; un struct *es* su nombre.

> **Un costo escondido de esta variante.** Antes `Type` era `Copy` —un enum de
> primitivos cabe en un registro. Al meter `Box<Type>` dejó de serlo (un `Box`
> posee memoria; copiarlo a la ligera duplicaría dueños). El cambio se propagó por
> el checker como `.clone()`s explícitos y `match (&lt, &rt)` en vez de `match (lt,
> rt)`. Es una lección típica de Rust: una decisión de modelado de datos te obliga
> a ser explícito sobre la propiedad en todo el código que lo toca.

## Sintaxis: literal e indexación

Tres formas nuevas en el parser:

```rust
[1, 2, 3]     // literal  → ExprKind::ArrayLit(vec![…])
a[i]          // indexar  → ExprKind::Index { array, index }
a[i] = x;     // asignar  → StmtKind::Assign { target: a[i], value: x }
```

La indexación se parsea como un **operador posfijo**, en el mismo bucle que las
llamadas `f(...)` y —ya en M3.2— el acceso a campo `p.x`. Eso hace que se encadenen
de forma natural: `m[0][1]`, `f(x)[i]`, `personas[0].nombre`.

La asignación a un elemento reusa una generalización clave: el destino de un `=` ya
no es un nombre, sino una **expresión-lvalue**. El parser parsea una expresión y
*luego* mira si viene un `=`; si el lado izquierdo es indexable (`a[i]`), es válido.
Esa decisión —tomada para los arreglos— hará que `p.x = 5` salga casi gratis en el
capítulo siguiente.

## El checker: indexar y el literal vacío

El checker añade reglas directas:

- **Indexar** `a[i]`: `a` debe ser un `[T]` e `i` debe ser `int`; el resultado es
  `T`. Indexar algo que no es arreglo, o con un índice no entero, es error de tipo.
- **Asignar** `a[i] = x`: el tipo de `x` debe coincidir con el elemento, y —como
  vimos— **no** se exige `var`, porque mutar un elemento no reata la variable.

El caso espinoso es el **literal vacío**. ¿Qué tipo tiene `[]`? No hay elementos de
los cuales inferirlo. La regla: `[]` **toma el tipo de su anotación**.

```rust
let xs: [int] = [];     // ✅ [] adopta [int]
let n: int = [];        // ❌ "arreglo vacío" — el destino no es un arreglo
foo([])                 // ❌ "no se puede inferir el tipo de []" sin contexto
```

Es un primer roce con la **inferencia de tipos**: a veces el tipo no nace de la
expresión, sino del contexto que la rodea. M8 generalizará esta idea; aquí la
aplicamos en el único punto donde hace falta.

## Builtins: `len` y `push`

Dos operaciones sobre arreglos llegan como **builtins**, no como métodos (los
métodos esperan a UFCS en M7):

```rust
len(a)        // -> int   : cuántos elementos
push(a, x)    // -> unit  : agrega x al final (muta a)
```

`push` exhibe la semántica de referencia en acción: **muta** el arreglo compartido y
devuelve `unit`. Si otro nombre apunta al mismo arreglo, también ve el elemento
nuevo.

## En el runtime: dos motores, una representación

### El intérprete

`Value` gana `Array(Rc<RefCell<Vec<Value>>>)`. Evaluar las formas nuevas es directo:

- **`ArrayLit`**: evalúa cada elemento y los envuelve en un `Rc::new(RefCell::new(…))`.
- **`Index`**: evalúa arreglo e índice, **verifica los límites**, y devuelve una
  *clonación* del `Value` en esa posición (que, si es compuesto, comparte por `Rc`).
- **`Assign` a `a[i]`**: verifica límites y escribe en el `RefCell`.

El fuera-de-rango es un **error de ejecución** con su línea y columna —no un pánico
de Rust. Una función `check_bounds(i, len, línea, col)` centraliza esa comprobación.

### La VM

La VM gana cinco opcodes:

| OpCode | Hace |
|--------|------|
| `MakeArray(n)` | saca `n` valores de la pila y arma un arreglo |
| `Index` | saca arreglo e índice, empuja el elemento (con chequeo de límites) |
| `SetIndex` | saca arreglo, índice y valor; escribe en el arreglo |
| `Len` | saca un arreglo, empuja su longitud |
| `Push` | saca arreglo y valor; agrega; empuja `unit` |

`len` y `push` se compilan a `Len`/`Push` directamente (el compilador reconoce esos
nombres), en vez de pasar por el mecanismo de llamada general —son operaciones
primitivas, no funciones de usuario.

### El oráculo confirma

Los tests compilan y corren los mismos programas en VM e intérprete y exigen
igualdad: construir, indexar, mutar, `len`, `push`, arreglos anidados y el aliasing
por referencia. Mientras coincidan, sabemos que las dos representaciones del mismo
arreglo —los dos motores— son fieles entre sí.

> Código: `src/ast.rs` (`Type::Array`, `ArrayLit`, `Index`), `src/checker.rs`
> (`check_index`, literal vacío), `src/interpreter.rs` (`Value::Array`,
> `check_bounds`), `src/compiler.rs` y `src/vm.rs` (los cinco opcodes).
