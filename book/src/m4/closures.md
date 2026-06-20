# Closures: capturar el entorno

Una **closure** es una función anónima que referencia variables de un ámbito
envolvente. El ejemplo canónico es un contador con estado:

```rust
fn contador() -> fn() -> int {
    var n: int = 0;
    fn() -> int { n = n + 1; n }   // captura n
}
// let c = contador(); c() -> 1; c() -> 2; c() -> 3
```

La función devuelta sigue usando `n` **después** de que `contador` retornó. Eso rompe
el supuesto que sostenía todo hasta ahora: que una variable muere con el marco que la
creó. `n` tiene que **escapar al heap**.

## La decisión: captura por referencia

Capturamos **por referencia**, no por valor: la closure comparte la *celda* de la
variable, no una copia. Por eso puede **leer y mutar** un `var` capturado, el cambio
se ve fuera, y dos closures que capturan la misma variable comparten su estado. Es la
closure "de verdad" (la de JavaScript, Lua, clox).

Capturar **no reata**: el modelo de mutabilidad de raylang se respeta. Una closure
puede mutar un `var` capturado (su celda es compartida), pero asignar a un `let`
capturado sigue siendo un error. El checker lo comprueba como siempre, ahora que ve el
entorno.

## La celda: la pieza común a los dos motores

Ambos motores convergen en una misma idea: **la variable capturada vive en una celda
compartida.** En Rust, una celda es `Rc<RefCell<Value>>` —`Rc` da el compartir,
`RefCell` la mutación interior—. La celda:

- la comparten el dueño de la variable y la closure;
- sobrevive al marco (vive mientras alguien la referencie);
- al mutarse por un lado, se ve por el otro.

Donde difieren los motores es en *cómo* llegan a esa celda.

## En el intérprete: celdas en el entorno

El intérprete pasa a guardar **todas** las variables en celdas. Cuando se crea una
closure, captura las celdas visibles en ese punto; al llamarla, esas celdas forman la
base de su entorno. Como cada declaración estrena una celda nueva, el *shadowing* es
seguro: una closure que capturó la celda vieja se queda con ella.

Un detalle clave: **asignar muta la celda, no la reemplaza.** Así el cambio se ve a
través de cualquier closure que la haya capturado.

## En la VM: upvalues y *boxing*

La VM no tiene un entorno de nombres; tiene slots numerados por marco. Aquí está el
mecanismo central de M4.2, y donde más se trabaja.

- **El compilador resuelve los upvalues** al estilo clox. Cuando el cuerpo de una
  función nombra algo que no es local suyo, lo busca en la función envolvente (un
  upvalue **local**) o, recursivamente, entre los upvalues de aquélla (un upvalue **de
  upvalue** —la captura *transitiva*, de dos o más niveles). Esa resolución produce,
  por función, la lista de sus upvalues, y **marca qué locales del marco envolvente
  hay que capturar**.
- Esas locales marcadas se guardan **boxeadas**: en vez de vivir directamente en su
  ranura, viven en una celda del heap. La closure captura un clon de esa celda.

> **Por qué *boxing* y no el clásico abierto/cerrado.** En clox las locales viven en
> la pila de operandos, así que un upvalue *abierto* apunta a una ranura y se *cierra*
> (copia al heap) al salir del marco. En raylang las locales viven en un **arreglo
> aparte por marco** —la decisión que tomamos en M2.3 por claridad—: no hay ranura de
> pila a la que apuntar, así que boxear la variable capturada desde el inicio es lo
> natural. Es el mismo concepto (la variable escapa al heap), sin la optimización de
> retrasar la copia. Una decisión de M2 vuelve a pagar —esta vez, simplificando M4.

Tres opcodes nuevos hacen el trabajo: `Closure` (arma el entorno tomando las celdas
del marco actual), y `GetUpvalue`/`SetUpvalue` (leen y mutan una celda capturada). Y
`InitLocal` reemplaza al viejo `SetLocal` en las declaraciones, porque una declaración
en un slot boxeado debe **estrenar celda**.

## El caso que lo prueba todo

El test más exigente: dos closures hermanas que comparten la misma variable.

```rust
struct Par { inc: fn(), get: fn() -> int }
fn hacer() -> Par {
    var n: int = 0;
    Par { inc: fn() { n = n + 1; }, get: fn() -> int { n } }
}
// p.inc(); p.inc(); p.inc(); p.get() -> 3
```

`inc` y `get` capturan la **misma** celda `n`: mutar por una se ve por la otra. Que el
intérprete y la VM coincidan aquí —y en la captura transitiva, y en instancias
independientes— es la prueba de que ambos modelos de captura son fieles entre sí.

> Código: `src/checker.rs` (permite la captura), `src/interpreter.rs` (celdas en el
> entorno, `Value::Closure`), `src/compiler.rs` (resolución de upvalues, *boxing*),
> `src/vm.rs` (`Closure`/`GetUpvalue`/`SetUpvalue`, locales boxeadas).
