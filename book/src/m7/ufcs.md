# UFCS: funciones como métodos

**UFCS** (*Uniform Function Call Syntax*) es la idea de que `recv.f(args)` y `f(recv,
args)` son **lo mismo**: llamar a una función con `recv` como primer argumento, escrito
como si `f` fuera un método de `recv`.

```rust
fn doble(x: int) -> int { x * 2 }
let n: int = 5.doble();    // ≡ doble(5)

fn norma1(p: Punto) -> int { p.x + p.y }
let m: int = p.norma1();   // ≡ norma1(p)
```

No hay que declarar nada especial: cualquier función libre cuyo primer parámetro encaje
con el receptor se puede llamar "con punto". Así, los "métodos" no son una construcción
aparte —no hay clases, no hay `impl`—; son funciones normales vistas desde otro ángulo.

## El problema: `.` ya estaba ocupado

Desde M3, `p.x` es **acceso a campo** de un struct. Y desde M5, `Color.Rojo` es
**construcción de variante** de enum. Ahora `p.norma1()` quiere ser una **llamada**. Las
tres comparten la misma forma sintáctica: un identificador, un punto, un nombre. El
parser no puede distinguirlas —produce siempre un `Field` (envuelto en un `Call` si hay
paréntesis)— y no debe intentarlo: le falta la información para hacerlo.

La regla que cierra la ambigüedad, decidida en el diseño, es **campo primero, luego
función libre**:

1. Si `recv` es un struct que tiene un **campo** con ese nombre, `recv.nombre` es acceso
   a campo (y si el campo es una función, `recv.campo(args)` la llama).
2. Si **no** lo tiene, `recv.nombre(args)` se reescribe a la función libre
   `nombre(recv, args)`.

El campo gana siempre; la regla es total y no deja casos ambiguos.

```rust
struct Caja { op: fn(int) -> int }
let c: Caja = Caja { op: fn(x: int) -> int { x + 1 } };
c.op(41);       // 'op' ES un campo -> (c.op)(41), llamada al valor del campo
c.doble();      // 'doble' NO es campo -> doble(c), UFCS a la función libre
```

## Por qué UFCS vive en el checker (y no antes)

Aquí aparece la diferencia clave con la construcción de enums de M5. Aquella se resuelve
en una **pre-pasada**, antes de verificar, porque solo necesita saber **qué nombres son
enums** —un dato puramente léxico, disponible sin tipos—. UFCS, en cambio, necesita
saber si `recv` **es un struct con tal campo**, y eso es el **tipo** de `recv`, que solo
se conoce durante la verificación.

Así que UFCS se resuelve **dentro** del checker. Cuando este se topa con una llamada
`recv.nombre(args)`:

1. Tipa `recv`. Si su tipo es `Struct S` y `S` tiene el campo `nombre`, es una llamada al
   valor del campo: se comporta como en M3/M4.
2. Si no, busca una función libre `nombre` (con toda la maquinaria de llamada normal:
   builtins, genéricos, inferencia) y la verifica como `nombre(recv, args)`.
3. Si no hay ni campo ni función, error con posición: *"no existe campo ni función
   'nombre' aplicable a `T`"*.

## Reescribir el árbol después de verificar

Como UFCS se decide con tipos, no se puede reescribir el AST en una pre-pasada. Pero el
intérprete y la VM **no deben** ver `recv.nombre(args)` y tener que re-decidir si es
campo o método: eso duplicaría la lógica en tres sitios. La solución es la misma idea de
M5, en dos tiempos:

- Durante la verificación, cuando una llamada resulta ser UFCS, el checker **registra su
  sitio** —la posición `(línea, columna)` del nodo, más el nombre del método—.
- Al terminar (y solo si todo verificó), una pasada `lower_ufcs` recorre el AST y
  **reescribe** esos nodos de `recv.nombre(args)` a `nombre(recv, args)`: el receptor
  pasa a ser el primer argumento y el callee se vuelve un identificador.

Tras esa pasada, los dos motores reciben **llamadas ordinarias**. UFCS, como los
genéricos, fue una característica que empezó y terminó en el front-end.

> **Un detalle de identidad de nodos.** El sitio se registra con `(línea, columna,
> nombre)`, no solo con la posición. ¿Por qué el nombre? Porque el parser arranca el
> nodo `Call` en la posición de su receptor, así que el `Call` y su receptor **comparten
> `(línea, columna)`**. En una cadena `a.f().g()`, distinguir el sitio de `g` del de `f`
> exige el nombre del método. Es la clase de aspereza que solo aparece al implementarlo,
> y que conviene recordar: "todo nodo lleva posición" no garantiza que la posición sea
> *única*.

## El receptor cuenta para la inferencia

Como UFCS no es más que reordenar los argumentos, **compone con todo lo demás** sin
esfuerzo. En particular, con los genéricos de M6: el receptor entra en la unificación
igual que cualquier argumento.

```rust
fn primero<T>(xs: [T]) -> T { xs[0] }
let xs: [int] = [7, 8, 9];
xs.primero();   // primero(xs): de [int] se infiere T = int
```

No hace falta nada nuevo: el receptor es el primer argumento, y la inferencia ya sabía
qué hacer con los argumentos. Es la recompensa de haber construido bien la capa anterior.

> Código: `src/checker.rs` (`check_call` con la rama `Field`, `struct_field_type`,
> `check_ufcs`, `name_is_callable`, y la pasada `lower_ufcs`). El runtime, intacto.
