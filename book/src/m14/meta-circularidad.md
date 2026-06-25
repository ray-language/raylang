# Meta-circularidad

Tenemos lexer, parser, checker, intérprete y loader, todos en raylang, todos verificados contra Rust.
La última pregunta es la que da nombre a este módulo: ¿puede el compilador auto-alojado **correr sobre
el intérprete auto-alojado**? Si raylang ejecuta el compilador de raylang, que a su vez compila
raylang, hemos cerrado el círculo.

## Lo que faltaba: la stdlib diferida

Para que el intérprete auto-alojado pueda ejecutar el **propio** compilador, tuvo que aprender los
builtins que el compilador usa pero que el intérprete/checker auto-alojados aún no conocían. Cada uno
siguió el patrón de la limpieza L1 —una fila en el checker, una impl en el intérprete— (M14.6):

- **`Map<K, V>`** en el checker e intérprete auto-alojados (el más invasivo: el intérprete lo
  representa con arrays paralelos, porque las claves serían un `Value`/enum no hasheable).
- **`panic`** y **`parse_int`/`parse_float`** (lo que el lexer usa al tokenizar números y abortar).
- **`assert`/`assert_eq`/`sort`** (el prelude de aserciones y orden).
- **I/O de archivos** (`read_file`/`write_file`/`exists`).
- Y los dos últimos cabos para que arrancara el compilador entero: **`pop`** (lo usa el checker) y la
  **concatenación de arreglos** `a + b` (la usa `run.ray`).

## El detalle fino: `args()` consistente

Los drivers piden su archivo de entrada con `args()`. Pero `args()` **divergía** entre los dos
caminos: al correr el compilador sobre el intérprete, `run.ray` ve el path del driver como su primer
argumento, mientras que Rust no.

La solución: `run.ray` **consume `argv[0]`** (el path del driver que va a ejecutar) y **enhebra
`argv[1..]`** al intérprete auto-alojado, que los expone vía el builtin `args()`.

```rust
// run.ray
match (run(prog, rest_args(argv))) { /* … */ }   // rest_args = argv[1..]
```

Así un driver ve sus **propios** argumentos igual que bajo Rust (`raylang <prog> [args]`), y un
programa de un solo archivo da `args() == []` por ambos caminos. Consistencia recuperada.

## El premio, eslabón a eslabón

Con eso, el oráculo conductual (`tests/selfhost_metacircular.rs`) compara cada driver corrido **por
Rust** contra corrido **sobre el intérprete auto-alojado**:

```text
raylang selfhost/lex_dump.ray   fuente.ray          # Rust corre el lexer
raylang selfhost/run.ray  selfhost/lex_dump.ray  fuente.ray   # el lexer corre SOBRE raylang
```

Y todos coinciden, eslabón a eslabón:

- **`lex_dump`** — el lexer auto-alojado corre sobre el intérprete auto-alojado y tokeniza idéntico.
- **`parse_dump`** — el parser, sobre el intérprete, produce el mismo AST.
- **`check_dump`** — el checker, sobre el intérprete, da el mismo veredicto (incluso en errores).
- **run-on-run** — `run.ray` corriendo `run.ray` corriendo un programa: **el back-end también**.

Ese último, *run-on-run*, es el cierre total. Tres niveles de la misma máquina:

```text
nivel 1:  Rust ejecuta run.ray
nivel 2:    run.ray (sobre Rust) ejecuta run.ray
nivel 3:      run.ray (sobre raylang) ejecuta el programa
```

Los tres producen el mismo `stdout` y el mismo código de salida. (Es lento —dos niveles de
*tree-walking*—, así que el test va marcado `#[ignore]`; se corre a mano con `--ignored`.)

## Qué significa

**raylang lexea, parsea, chequea y ejecuta raylang, con raylang corriendo sobre raylang.** El
lenguaje que empezó como un puñado de tokens en Rust es ahora lo bastante completo y coherente como
para describirse y ejecutarse a sí mismo. El compilador de Rust deja de ser imprescindible: es el
*bootstrap*, el primer eslabón del que cuelga todo lo demás.

Quedan caminos abiertos —la **VM auto-alojada** (el M2 de este módulo), `import` calificado y
directorios en el loader, el resto de la I/O— pero ninguno es necesario para lo que perseguíamos. El
círculo está cerrado.

> **Por qué importa.** La meta-circularidad no añade una *feature*; **valida el todo**. Que un
> lenguaje pueda alojar su propio compilador es la señal de que sus abstracciones —tipos, genéricos,
> traits, módulos, memoria— no son juguetes aislados sino un sistema que se sostiene. Es el final
> natural de "Construyendo raylang": el lenguaje, construyéndose a sí mismo.
