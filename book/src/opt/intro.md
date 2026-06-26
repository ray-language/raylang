# Optimizar la VM: medir, cambiar, a veces revertir

Con el lenguaje completo, llega una pregunta distinta a todas las anteriores: no *qué* hace
la VM, sino *cuánto cuesta* hacerlo. Optimizar no cambia el comportamiento —los tests y el
oráculo siguen verdes— solo el tiempo. Y eso obliga a una disciplina nueva: **medir**. Una
intuición de rendimiento sin un número al lado es una hipótesis, no un hecho; este capítulo
es la historia de varias hipótesis —a lo largo de dos pases—, unas confirmadas y otras
refutadas, y de la disciplina de no dar por buena ninguna sin medirla.

## La regla de oro: no optimizar a ciegas

raylang tiene dos motores: el **intérprete** (el oráculo, simple) y la **VM** (bytecode +
pila, la rápida). La VM es la que importa para el rendimiento. Para medirla usamos
[hyperfine](https://github.com/sharkdp/hyperfine) sobre programas de `benchmarks/`:
`fib(32)` (recursión exponencial → millones de llamadas; mide el coste de *llamar* y
*despachar*) y `strings.ray` (mueve y construye strings en bucle). Cuando hyperfine no está
a mano, `benchmarks/measure.py` hace lo mismo con solo python3 (mejor-de-N), fiel a la
invariante cero-dependencias.

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

## Opt.4 — el fast-path entero (segundo pase)

Tiempo después, un segundo pase volvió sobre la VM con la misma disciplina. La hipótesis: en el
brazo de operaciones binarias del lazo, **casi siempre** ambos operandos son enteros (bucles,
recursión aritmética), pero el código los trataba con toda la generalidad. Tras sacar los dos
operandos de la pila, llamaba a `apply_binary`, que volvía a hacer *match* del opcode y recorría
~30 combinaciones de tipos (`Int`/`Float`/`Str`/`Char`…) hasta dar con `Int + Int`. Dos *matches*
y una llamada para sumar dos enteros.

El **fast-path** lo resuelve en el sitio: si ambos operandos son `Int`, se hace la operación con un
`match` pequeño sobre el opcode, sin llamar a `apply_binary`. La semántica es **idéntica** al camino
general (los mismos `+`/`-`/`*`; en *debug*, ambos hacen *panic* al desbordar) → el oráculo ni se
entera. Medido (mejor de 5): **fib(35) −5 %, bucle aritmético 10M −6 %**; `arrays` sin cambio, como
se esperaba (no es aritmético). Pequeño, consistente entre cargas, por encima del ruido: **se queda**.

Ese mismo pase confirmó la otra mitad de la disciplina con tres ideas que **no** pasaron el corte:

- **Opt.5 — `new_locals` sin el *branch* por slot** para funciones sin capturas: medido **dentro del
  ruido** (las funciones calientes tienen pocos locales, el branch estaba bien predicho) → revertido.
- **Opt.6 — amortizar el *safepoint* del GC** (chequearlo 1 de cada N instrucciones): techo de ~2-3 %,
  pero **rompía el modo estrés** del GC —el test que caza raíces faltantes colectando en cada punto
  seguro—. Capturarlo bien exigiría un rediseño con riesgo sobre ese test sagrado, por ~2-3 %: no
  compensa.
- **LTO + `codegen-units=1`** en el perfil de release: la hipótesis "obvia" (inline a través de
  módulos del lazo) salió **igual o peor** que el perfil por defecto. Descartado.

Tres rechazos honestos por una mejora real. Exactamente la proporción que cabe esperar cuando se mide.

## El techo y la honestidad

Tras Opt.1 + Opt.2 + Opt.4, la VM corre del orden de **3×** más rápido que el intérprete (era 2.4×).
¿Qué queda? Las opciones con mejor relación esfuerzo/ganancia ya se tomaron, y los rechazos de Opt.5/
Opt.6 confirman que los próximos puntos porcentuales vienen con trade-offs (riesgo sobre el GC, churn
transversal). Lo que resta son refactors grandes con retorno decreciente —locales en la propia pila de
operandos (estilo *clox*; pero Opt.2 ya se llevó la asignación, que era el grueso), `HeapValue` de 32 a
16 bytes (a ese tamaño el *memcpy* ya es barato), *bytecode* empaquetado en bytes, *direct threading*
(que Rust estable no ofrece)— o micro-optimizaciones (deduplicar/plegar constantes) de impacto pequeño.

Una nota de honestidad sobre la **VM auto-alojada** (`selfhost/vm.ray`): optimizarla casi no
tiene payoff práctico —es un intérprete corriendo sobre otro intérprete; nadie ejecuta
producción por ahí—. Reflejar estas optimizaciones en raylang sería un bonito ejercicio de
simetría, pero el rendimiento que importa es el de la VM de Rust.

El cierre del capítulo no es un número, es un método: **mide, cambia, mide; y ten el valor de
revertir lo que no funciona, por elegante que sea la idea.**
