# El lexer auto-alojado

El primer eslabón del pipeline es el lexer: texto → tokens. `selfhost/lexer.ray` es un port casi 1:1
de `src/lexer.rs`. Es la fase ideal para estrenar la estrategia de oráculo, porque su salida —una
lista de tokens— es fácil de comparar.

## El port

El lexer en raylang reusa lo que ya teníamos: structs con mutación de campos por referencia (el
estado del cursor), `chars`/`s[i]`/comparación de `char`, `parse_int`/`parse_float`.

```rust
pub enum TokKind {
    Let, Var, Fn, /* … */ Int(int), Float(float), Ident(string), Str(string), Char(char), Eof,
}
pub struct Token { kind: TokKind, line: int, col: int }

pub struct Lexer { src: [char], pos: int, line: int, col: int }
```

El `Lexer` se muta por referencia, igual que cualquier struct de raylang: avanzar el cursor es
mutar `lx.pos`. Algunas diferencias menores con el original de Rust, impuestas por lo que raylang
ofrece hoy:

- Rust hace `match` sobre literales de `char`; raylang aún no, así que el port usa cadenas de
  `if`/`else` sobre comparaciones de `char`.
- El fin de archivo (EOF) no se marca con un centinela `'\0'` sino con guardas `at_end` antes de
  indexar.

## El oráculo: formato canónico de tokens

Para comparar, necesitamos un texto **idéntico** desde ambos lexers. El driver `selfhost/lex_dump.ray`
imprime cada token en un formato canónico, uno por línea:

```text
Fn@1:1
Ident(main)@1:4
LParen@1:8
Int(42)@1:28
Eof@2:1
```

`<KIND>@<línea>:<columna>`, con el valor entre paréntesis para los tokens que lo llevan. Las cadenas
y caracteres se re-escapan (`\n`, `\t`, `\\`, `\"`) para caber en una línea; el lexer de Rust hace
**el mismo escape**, así que las salidas coinciden carácter a carácter.

El test (`tests/selfhost_lexer.rs`) corre ambos lexers sobre snippets y sobre **archivos reales** —los
ejemplos del repo e incluso el propio `lexer.ray`—. Es decir: **el lexer se lexea a sí mismo**, y el
resultado debe ser el mismo que produce el lexer de Rust sobre ese mismo archivo.

## Errores como valores

La primera versión seguía el "camino feliz": ante un carácter inesperado, `panic`. Pero el norte de
diseño de raylang es **errores como valores** (`Result`/`?`), y el lexer debe predicar con el
ejemplo. Así que `lex` pasó de `[Token]` a:

```rust
pub struct LexError { msg: string, line: int, col: int }

fn lex(src: string) -> Result<[Token], LexError> { /* … */ }
```

Las funciones internas (`number`, `string_lit`, `char_lit`, `next_token`) devuelven
`Result<TokKind, LexError>`, y `lex` **propaga con `?`**. Los mensajes se construyen **idénticos** a
los de Rust, incluido el fragmento ofensor:

```text
error léxico en 3:5: carácter inesperado '#'
error léxico en 1:9: secuencia de escape inválida '\q'
```

Así el oráculo cubre **también las entradas inválidas**: cuando el lexer falla, se compara el
`Display` del error. Un gotcha de tipos apareció aquí: `parse_int`/`parse_float` devuelven `Option`,
y `?` no cruza de `Option` a `Result`, así que esos casos se desenvuelven con `match`.

## Prerrequisitos que destapó

Portar el lexer reveló dos huecos en el propio raylang que hubo que tapar antes:

- **`parse_float`**: raylang tenía `parse_int` pero no flotantes. Se añadió como builtin aditivo
  (mismo patrón de M11.4), porque el lexer necesita tokenizar literales como `3.14`.
- **El escape `\r`**: faltaba en el lexer de Rust y en el auto-alojado.

Y un hueco de tipado: un brazo de `match` que termina en `panic`/`return` debe **ceder su tipo a los
demás** (extiende la divergencia de M13.2, que solo cubría `if`). El lexer lo usa por todas partes.

> **Por qué importa.** El lexer demuestra que la estrategia funciona: un port directo, un oráculo de
> texto canónico, y errores como valores. Que el lexer **se lexee a sí mismo** igual que lo hace el
> de Rust es el primer indicio de que la meta-circularidad es alcanzable.
