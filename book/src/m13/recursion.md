# Recursión profunda y llamadas en cola

Un compilador es recursivo de arriba abajo: el parser de descenso recursivo, el checker que recorre
el AST, el intérprete que evalúa árbol. Para que raylang pueda compilarse a sí mismo (M14), su
runtime tiene que **aguantar recursión profunda sin reventar** y **no crecer la pila en las llamadas
en cola**. M13.3 ataca las dos cosas, y —clave— en **ambos motores a la vez**, para no romper el
oráculo.

## El problema: dos formas de morir

El intérprete es *tree-walking*: evaluar una llamada de raylang **recurre sobre la pila de Rust**.
Una recursión raylang profunda se traduce en una pila de Rust profunda, y al pasarse → **segfault**,
no un error limpio. La VM no recurre sobre la pila de Rust (tiene su propia pila de marcos), pero sin
límite crecería sin tope.

## M13.3a — un techo limpio y compartido

Dos medidas. Primero, **una pila grande**: `lib::with_big_stack` corre todo el trabajo del binario
en un hilo *worker* con una pila de **256 MiB** (frente a los ~8 MiB del hilo principal).

```rust
const STACK_SIZE: usize = 256 * 1024 * 1024; // 256 MiB
// el parser (descenso recursivo) y el intérprete (recurre sobre la pila de Rust)
// alcanzan profundidades altas sin reventar
```

Segundo, **un límite compartido** que convierte el desbordamiento en un error de ejecución, no en un
segfault:

```rust
pub const MAX_CALL_DEPTH: usize = 1024;
```

La VM ya tenía su tope (`MAX_FRAMES`); M13.3a hace que **reuse esta misma constante**, y añade el
contador que le faltaba al intérprete. Cada `call_body` del intérprete lleva un campo `depth` que se
chequea **antes** de incrementar —igual que la VM mira `frames.len()` antes de empujar—.

```rust
if self.depth >= MAX_CALL_DEPTH {
    return Err(Flow::Error(/* "desbordamiento de pila …" */));
}
```

Así **los dos motores cortan en la misma frontera y dan el mismo error**. El oráculo
(`overflow_recursion_oraculo`) lo verifica con una recursión que se pasa de 1024.

## M13.3b — eliminación de llamadas en cola (TCO)

Una **llamada en cola** es la última cosa que hace una función: su resultado es directamente el
resultado de la función, sin trabajo pendiente después. Esas llamadas pueden **reutilizar el marco**
en vez de apilar uno nuevo → recursión en cola en **O(1) de pila**, sin tope.

```rust
fn cuenta(n: int, acc: int) -> int {
    if (n == 0) { acc }
    else { cuenta(n - 1, acc + 1) }   // ← llamada en cola: nada pasa después
}
```

El detalle decisivo de raylang: **el TCO va en los DOS motores**. Si solo lo tuviera la VM, una
recursión en cola profunda correría sin límite en la VM pero el intérprete cortaría en
`MAX_CALL_DEPTH` → **divergencia**, oráculo roto. Así que ambos detectan la posición de cola con
**reglas estructurales idénticas**: el cuerpo de una función, las ramas de un `if`/`match`, la
expresión final de un bloque y el valor de un `return`.

**En la VM** es un *peephole* tras la emisión: `optimize_tail_calls` busca un `Call`/`CallValue`
cuya continuación sea un `Return` (directo o vía saltos) y lo reescribe a `TailCall`/`TailCallValue`,
opcodes que **reutilizan el marco actual**.

**En el intérprete** es un **trampolín**. Una llamada en posición de cola no recurre: produce una
señal `Flow::TailCall`, y el bucle de `call_body` la atrapa, **reemplaza la función actual** por la
llamada y reitera —sin recurrir sobre la pila de Rust ni crecer `depth`—.

```rust
enum Flow {
    Return(Value),
    Error(RuntimeError),
    TailCall { index: usize, args: Vec<Value>, captured: Vec<(String, Cell)> },
}
```

`eval_tail`/`eval_tail_block` evalúan en posición de cola; `return e` evalúa `e` en cola. Los
**builtins no son llamadas en cola** (incluido `panic`): siempre se ejecutan normalmente.

### Un gotcha del oráculo

El viejo test `overflow_recursion_oraculo` usaba una recursión de cola —`bucle(n + 1)`— esperando
desbordar la pila. Con TCO, eso pasó a ser un **bucle infinito legítimo** (corre en O(1) para
siempre). Hubo que cambiarlo a una recursión **no** de cola, con trabajo pendiente tras la llamada:

```rust
fn bucle(n: int) -> int { 1 + bucle(n + 1) }   // el `1 +` deja trabajo → NO es cola → sí desborda
```

Verificado: un millón de llamadas en cola y una recursión mutua profunda corren en O(1) de pila y
**coinciden** entre los dos motores.

> **Por qué importa.** Juntas, estas medidas hacen el runtime *robusto*: la recursión acotada da un
> error limpio en la misma frontera en ambos motores, y la recursión en cola corre indefinidamente
> sin pila. Sin esto, el intérprete auto-alojado de M14 —que recorre árboles grandes— moriría con un
> segfault en vez de hacer su trabajo.
