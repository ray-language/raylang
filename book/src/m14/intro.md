# Self-hosting: el plan y el oráculo

Todo el proyecto apuntaba aquí. Hemos construido raylang **en Rust**: lexer, parser, checker, dos
motores de ejecución, traits, genéricos, módulos, una stdlib. M14 es el *capstone*: **escribir el
compilador de raylang en raylang** y hacer que corra sobre sí mismo. Cuando un lenguaje puede
compilarse con un compilador escrito en él, decimos que es **auto-alojado** (*self-hosted*); cuando
además ese compilador puede ejecutarse con su propio intérprete, alcanzamos la **meta-circularidad**.

No es solo un truco. Es la prueba definitiva de que el lenguaje es **completo y coherente**: si
puedes escribir en raylang algo tan exigente como su propio compilador —recursión profunda, tablas
de símbolos, AST mutuamente recursivos, manejo de errores, I/O— es que las piezas que añadimos
(M13: `Map`, aserciones, recursión robusta) y todo lo anterior encajan de verdad.

## El oráculo: Rust como juez

La pregunta inmediata es: ¿cómo sabemos que el lexer escrito en raylang es **correcto**? Tenemos un
lujo que un compilador desde cero no tiene: **ya existe una implementación de referencia**, la de
Rust. Esa es nuestra **oráculo**.

La estrategia de todo M14 es la misma que usamos entre el intérprete y la VM (M2): correr la **misma
entrada por ambos caminos** y exigir que coincidan.

```text
fuente.ray ──> [compilador en Rust]    ──> resultado A
fuente.ray ──> [compilador en raylang] ──> resultado B
                                            assert A == B
```

Qué se compara depende de la fase:

- **Lexer y parser** producen una estructura (tokens, AST). El oráculo compara un **volcado canónico
  en texto** —carácter a carácter— de esa estructura.
- **Checker** produce un veredicto. El oráculo compara el **veredicto** (`ok` o el mensaje de error
  exacto), byte a byte.
- **Intérprete** produce comportamiento. El oráculo es **conductual**: el mismo `stdout` y el mismo
  código de salida.

## Tres decisiones que dan forma a M14

Antes de escribir una línea, tres elecciones marcan el resto del módulo.

**1. Intérprete, no VM.** El back-end auto-alojado se escribe como **intérprete tree-walking** (port
de `src/interpreter.rs`), no como la VM. Es el mismo orden M1→M2 que seguimos en Rust: primero el
motor simple y claro, la VM auto-alojada queda como trabajo opcional posterior.

**2. El checker es un validador.** El checker de Rust hace dos cosas: comprueba tipos **y** baja
(*lowers*) el azúcar de M9 (UFCS, diccionarios de bounds, trait objects) a construcciones simples. El
checker auto-alojado hace **solo lo primero**: produce el veredicto, sin el lowering. Esto lo
simplifica enormemente, y tiene una consecuencia preciosa para el intérprete.

**3. Resolución en runtime, no lowering.** Como el checker auto-alojado no baja nada, el intérprete
auto-alojado resuelve la construcción de enums, UFCS, métodos y `dyn` **en tiempo de ejecución**,
mirando la **etiqueta del valor** concreto. El efecto colateral es elegante: `dyn`, los bounds y los
genéricos se vuelven **no-ops** —el intérprete nunca consulta tipos, así que el *borrado de tipos*
(*erasure*) ocurre solo, sin ninguna pasada—. Diverge a propósito del intérprete de Rust (que es
"tonto" porque el lowering ya pasó), pero como el oráculo del back-end es **conductual**, esa
diferencia interna es invisible.

## El recorrido

M14 sigue el mismo pipeline que construimos en Rust, ahora en raylang:

1. **El lexer** (`selfhost/lexer.ray`) — texto a tokens, con errores como valores.
2. **El parser** (`selfhost/parser.ray`) — tokens a AST.
3. **El checker** (`selfhost/checker.ray`) — el validador de tipos.
4. **El intérprete** (`selfhost/interprete.ray`) — el back-end, *cabalgando sobre el host*.
5. **El loader** (`selfhost/loader.ray`) — juntar varios archivos en uno.
6. **Meta-circularidad** — el compilador entero corriendo sobre sí mismo.

Cada pieza es un programa raylang que corre sobre el compilador de Rust; al final, hacemos que corra
sobre el compilador **de raylang**. Empecemos por el principio del pipeline: el lexer.
