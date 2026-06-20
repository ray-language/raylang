# El recolector de basura

Las closures introdujeron celdas que viven en el heap y se referencian libremente. Eso
crea **ciclos**: una celda que guarda una closure que captura esa misma celda.

```rust
fn make_cycle() {
    var f: fn() = fn() {};
    f = fn() { f(); };   // la celda f guarda una closure cuyo upvalue es la celda f
}
```

El `Rc` libera por **conteo de referencias**: cuando el contador llega a cero, libera.
Es simple y sin pausas, pero **no rompe ciclos** —el contador de una celda en un ciclo
nunca llega a cero, aunque nadie de afuera la alcance—. M4.3 lo resuelve con un
recolector **trazador** (mark-and-sweep) en la VM.

## Por qué un heap propio, con *handles*

Un GC trazador debe **poseer** los objetos para poder liberarlos. Y `Rc` no cede esa
propiedad: el objeto se libera cuando su contador llega a cero, no cuando el GC lo
decide. Así que la VM estrena su **propio heap**.

En Rust, un heap con punteros crudos exigiría `unsafe`. En su lugar, el heap es un
**arreglo de ranuras** y un objeto se referencia por su **índice** —un *handle*—. Es
la misma idea que un GC con punteros (marcar desde las raíces, barrer lo no marcado),
pero segura y clara, a costa de una indirección por acceso. Fiel a nuestro lema:
priorizar el aprendizaje.

Esto obliga a que **la VM tenga su propio valor** (`HeapValue`), distinto del `Value`
del intérprete: los primitivos van *inline*, y los compuestos (arreglo, struct,
closure, celda) son un `Obj(Handle)`. Se convierte de uno a otro solo en el borde —al
devolver el resultado o imprimir—.

## El algoritmo: marca y barrido

1. **Marca.** Desde las **raíces**, se marca todo lo alcanzable. Las raíces las aporta
   la VM, y son justo su estado reificado: la **pila de operandos**, las **locales de
   todos los marcos** (incluidas las celdas boxeadas) y los **upvalues** de las
   closures vivas. Se usa una *lista gris* (worklist) en vez de recursión, para no
   desbordar la pila de Rust.
2. **Barrido.** Se recorren las ranuras: lo **no** marcado se libera (su ranura vuelve
   a la lista de libres) y a lo marcado se le limpia la marca para la próxima vuelta.
   Los **ciclos inalcanzables se liberan**, porque la marca parte de las raíces, no de
   los contadores.
3. **Disparo.** Se recolecta cuando el número de objetos vivos cruza un umbral que
   **crece** tras cada recolección (estilo clox `nextGC`): así el costo del GC se
   amortiza a medida que el programa usa más memoria.

## El problema de los puntos seguros

Hay una trampa clásica. Si el GC corriera **en medio** de una instrucción, podría
liberar un objeto que la VM tiene a medio ensamblar en una variable temporal de Rust
—no en la pila ni en un marco—, y por tanto invisible para el marcado.

La solución es simple y robusta: recolectar solo en **puntos seguros**, al inicio del
bucle de instrucciones, cuando todos los valores vivos están en la pila o los marcos.
Entre instrucciones, el heap puede crecer; en el siguiente punto seguro se recolecta.
Así marcar desde la pila y los marcos es correcto sin más cuidado, y no hace falta
rootear temporales.

## Cómo se prueba un GC

El GC es invisible desde el lenguaje (no hay introspección), así que se prueba desde
Rust con dos estrategias:

- **Modo estrés**: recolectar en **cada** punto seguro. Si una raíz faltara, un valor
  vivo se liberaría y el resultado cambiaría o reventaría. Correr los programas de
  siempre en modo estrés y exigir el mismo resultado que el intérprete es la prueba de
  que el conjunto de raíces es correcto.
- **Liberación de ciclos**: ejecutar un programa que crea cientos de ciclos
  inalcanzables y verificar que el heap **queda acotado** (con `Rc` crecería sin
  parar).

## M4 completo

raylang ya no solo modela datos: tiene **funciones de primera clase, closures con
estado** y **gestión de memoria de verdad**. Dos motores que coinciden, uno con
conteo de referencias y otro con un recolector trazador, verificados en cada
`cargo test`. El siguiente hito, **M5**, le da **tipos suma (`enum`) y pattern
matching (`match`)** —la base de `Option`/`Result` y un salto expresivo grande—.

> Código: `src/gc.rs` (el heap y el recolector), `src/vm.rs` (las raíces, los puntos
> seguros, la conversión en el borde).
