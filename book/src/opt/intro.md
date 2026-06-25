# Optimizar la VM: medir, cambiar, a veces revertir

Con el lenguaje completo, llega una pregunta distinta a todas las anteriores: no *qué* hace
la VM, sino *cuánto cuesta* hacerlo. Optimizar no cambia el comportamiento —los tests y el
oráculo siguen verdes— solo el tiempo. Y eso obliga a una disciplina nueva: **medir**. Una
intuición de rendimiento sin un número al lado es una hipótesis, no un hecho; este capítulo
es la historia de tres hipótesis, dos confirmadas y una refutada.

## La regla de oro: no optimizar a ciegas

raylang tiene dos motores: el **intérprete** (el oráculo, simple) y la **VM** (bytecode +
pila, la rápida). La VM es la que importa para el rendimiento. Para medirla usamos
[hyperfine](https://github.com/sharkdp/hyperfine) sobre programas de `benchmarks/`:
`fib(32)` (recursión exponencial → millones de llamadas; mide el coste de *llamar* y
*despachar*) y `strings.ray` (mueve y construye strings en bucle).

El punto de partida: en `fib(32)`, la VM corría en **735 ms**, **2.4×** más rápido que el
intérprete. La regla que seguimos en todo el capítulo: **una optimización no existe hasta
que el benchmark la confirma**. Se mide antes, se cambia, se mide después. Si no mejora —o
empeora— se revierte. Sin excepciones, por muy "obvia" que parezca la mejora.

## Opt.1 — no clonar la instrucción en cada vuelta

El bucle de la VM, por cada instrucción, hacía esto:

```rust
let op = self.program.functions[func].chunk.code[ip].clone();  // ← un clon por instrucción
// ... match sobre `op`, mutando self (push, pop, frames...) ...
```

¿Por qué clonar? Por el *borrow checker*. La instrucción vive dentro de `self.program`, y el
cuerpo del `match` **muta** `self` (empuja a la pila, crea marcos). No puedes tener una
referencia inmutable a `self.program` y mutar `self` a la vez. Clonar la instrucción soltaba
el préstamo... a costa de copiar un `OpCode` por instrucción, decenas de millones de veces.

La clave para evitarlo: `self.program` es un `&CompiledProgram` **inmutable que vive tanto
como la VM**. Y una referencia compartida (`&T`) es `Copy`. Así que basta copiarla a un local
**una vez**, antes del bucle:

```rust
let program = self.program;             // copia la referencia (no el programa)
// dentro del bucle:
let instr = &program.functions[func].chunk.code[ip];   // préstamo de `program`, NO de `self`
match instr { /* ... el cuerpo puede mutar self sin conflicto ... */ }
```

El truco es que `instr` ahora toma prestado de `program` (un binding aparte), no de `self`.
El préstamo y las mutaciones de `self` ya no se pisan, y el clon desaparece. **735 → 685 ms**
(~7%; 2.59×). Una victoria limpia de una sola línea conceptual.

## Opt.2 — reciclar los locales de cada llamada

El siguiente sospechoso: cada llamada a función asignaba un `Vec<Local>` nuevo para los
locales del marco. En `fib`, eso son **millones de asignaciones pequeñas** al heap (con su
`malloc`/`free`), una por llamada. El asignador es rápido, pero "rápido × millones" pesa.

La solución es un patrón clásico: un **pool** (*free-list*) de `Vec` reutilizables.

```rust
fn new_locals(&mut self, fn_idx: usize) -> Vec<Local> {
    let mut locals = self.locals_pool.pop().unwrap_or_default(); // reusa uno del pool
    locals.clear();
    // ... rellena n locales ...
    locals
}
fn recycle_locals(&mut self, locals: Vec<Local>) {
    if self.locals_pool.len() < 256 { self.locals_pool.push(locals); } // devuélvelo (acotado)
}
```

Cuando un marco se descarta (en `Return`, al final del chunk, o al reutilizar el marco en una
llamada en cola), su `Vec` no se libera: vuelve al pool, conservando su capacidad. La próxima
llamada lo reusa. La asignación por llamada desaparece casi del todo.

Hay una sutileza de **seguridad con el GC** que merece atención, porque es el tipo de error
que un recolector hace difícil de ver. El pool **no debe ser raíz del GC**. Entre que un `Vec`
se recicla y se reusa, arrastra los `Local` viejos del marco anterior —posiblemente *handles*
a objetos ya muertos—. Si rooteáramos el pool, mantendríamos vivos esos objetos muertos (una
fuga). No rootearlo es correcto *siempre que nunca leamos lo viejo*: y no lo hacemos, porque
`new_locals` hace `clear()` y reconstruye el `Vec` entero antes de usarlo. Un handle reciclado
a una celda muerta jamás se desreferencia; si la celda sigue viva, la rootea su closure, no el
pool. **685 → 552 ms** (~19%; **3.22×**). El mayor salto del capítulo.

## Opt.3 — `Rc<str>`: la hipótesis que el número refutó

La tercera idea era de manual: clonar un `String` copia todos sus bytes; si guardáramos los
strings como `Rc<str>`, clonar sería un *bump* de contador. `GetLocal`, paso de argumentos,
acceso a campos... todo lo que mueve un string se abarataría. Parecía una victoria segura.

Se hizo el cambio (`HeapValue::Str(String)` → `Rc<str>`, ~60 sitios), pasó el oráculo... y el
benchmark dijo que no:

- `strings.ray`: **79 → 77 ms**. Nada, dentro del ruido. ¿Por qué? Porque los builtins que
  *producen* strings (`to_upper`, `split`, …) devuelven `String`, y convertir `String → Rc<str>`
  **copia los bytes** a un buffer nuevo del `Rc`. Abaratamos el clon, pero encarecimos la
  construcción justo lo mismo. Un lavado.
- `fib(32)`: **552 → 609 ms**. Un **10% más lento** — ¡y `fib` no toca ni un string! Cambiar el
  tamaño y el layout de `HeapValue` desplazó las decisiones de *codegen* de LLVM sobre el enorme
  `match` del bucle, y degradó el camino aritmético. Un efecto a distancia, invisible a la
  lógica, visible solo al medir.

Net negativo. Se **revirtió**. Y esa es la lección más valiosa del capítulo: la optimización
"de manual" empeoró las cosas, y sin el benchmark la habríamos dado por buena. `Rc<str>` solo
gana en código *clone-heavy* (leer el mismo string muchas veces sin construir, como el `src`
del lexer auto-alojado); no compensa cuando se construyen strings, y arrastra un riesgo de
codegen difícil de prever. Queda anotado en `IDEAS.md` con su medición, por si algún día un
perfil muestra el clon de strings dominando.

## El techo y la honestidad

Tras Opt.1 + Opt.2, la VM corre **3.22×** más rápido que el intérprete (era 2.4×), un ~25%
de mejora bien medido. ¿Qué queda? Las opciones con mejor relación esfuerzo/ganancia ya se
tomaron. Lo que resta son refactors grandes con retorno decreciente —locales en la propia
pila de operandos (estilo *clox*; pero Opt.2 ya se llevó la asignación, que era el grueso),
*bytecode* empaquetado en bytes, *direct threading* (que Rust estable no ofrece directamente)—
o micro-optimizaciones (deduplicar constantes, plegado de constantes) de impacto pequeño.

Una nota de honestidad sobre la **VM auto-alojada** (`selfhost/vm.ray`): optimizarla casi no
tiene payoff práctico —es un intérprete corriendo sobre otro intérprete; nadie ejecuta
producción por ahí—. Reflejar estas optimizaciones en raylang sería un bonito ejercicio de
simetría, pero el rendimiento que importa es el de la VM de Rust.

El cierre del capítulo no es un número, es un método: **mide, cambia, mide; y ten el valor de
revertir lo que no funciona, por elegante que sea la idea.**
