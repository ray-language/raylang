# El parser

> 🚧 **Capítulo en construcción.** Se escribirá al consolidar esta fase.

Cubrirá: cómo el parser convierte la secuencia plana de tokens en un **árbol de
sintaxis abstracta (AST)** mediante *descenso recursivo*; cómo la **precedencia de
operadores** sale gratis de la jerarquía de funciones (`logic_or → … → factor →
unary → call → primary`); la asociatividad a la izquierda; y la regla de la
orientación a expresiones que distingue las *expresiones con bloque* (`if`, `while`,
`{}`) de las *expresiones sin bloque*.

> Código: `src/ast.rs` y `src/parser.rs`.
