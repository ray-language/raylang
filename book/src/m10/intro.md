# Tooling: anotaciones y LSP

M9 cerró la historia del *lenguaje*. M10 mira hacia las **herramientas** que lo rodean —lo
que lo hace cómodo de usar—. Dos piezas independientes:

- **Anotaciones** (M10.1): metadatos `@nombre` adheridos a declaraciones. El compilador
  conoce un conjunto cerrado; en M10.1, `@test` (pruebas) y `@derive(Eq)` (codegen).
- **LSP** (M10.2): un *language server* que reusa el checker para dar diagnósticos en vivo a
  cualquier editor. Se escribe una vez y sirve a todos.

Como M8 (inferencia/REPL/errores), es un hito de **ergonomía y tooling**, no de poder
expresivo. Y, fiel al patrón del proyecto, **casi no toca el núcleo**: las anotaciones son
metadatos del front-end (el runtime ni se entera), y el LSP será un cliente del checker.

## Anotaciones: un conjunto cerrado

Una anotación es `@nombre` o `@nombre(args)` antes de una función, struct o enum:

```rust
@test
fn suma_ok() -> bool { 1 + 1 == 2 }

@derive(Eq)
enum Color { Rojo, Verde, Azul }
```

La decisión de diseño clave es **qué las consume**, y elegimos lo más simple y didáctico:
un **conjunto cerrado que el compilador conoce**. Nada de anotaciones definidas por el
usuario que "hacen algo" —eso es un sistema de macros, de lo más difícil del diseño de
lenguajes, y queda como capstone—. Con un conjunto cerrado, cada anotación es una pequeña
feature que el front-end entiende, y una anotación desconocida es un error claro.

Las dos de M10.1 muestran las **dos caras** de las anotaciones:

- **`@test`** — un *metadato leído por una herramienta*. No cambia la compilación; un runner
  externo (`--test`) las recolecta y ejecuta.
- **`@derive(Eq)`** — una anotación que *genera código*. Es el "pago" de tener traits (M9):
  a partir de una marca, el compilador sintetiza un `impl`.

Ambas son **erasure**: metadatos del front-end que no llegan al runtime. Las dos páginas
siguientes las detallan; el LSP (M10.2) se especifica al arrancarlo.
