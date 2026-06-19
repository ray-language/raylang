# El lexer

El **lexer** (o *analizador léxico*, o *tokenizer*) es la primera estación de la
tubería. Su trabajo es modesto pero fundamental: convertir el texto fuente —una
secuencia plana de caracteres— en una secuencia de **tokens**, las unidades
mínimas con significado.

```
"fn fib(n: int)"  →  [Fn, Ident("fib"), LParen, Ident("n"), Colon, IntType, RParen]
```

El lexer no entiende de gramática ni de tipos. No sabe que `fib` es una función ni
que `int` es un tipo válido. Solo reconoce piezas: *esto es una palabra clave*,
*esto es un número*, *esto es un paréntesis*. Darle estructura a esas piezas es
trabajo del parser, en el próximo capítulo.

## El token

Antes del lexer, definimos qué produce. Un token tiene dos partes: **qué es** y
**dónde está**.

```rust
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub col: usize,
}
```

El `TokenKind` es un `enum` con una variante por cada clase de token: literales que
cargan su valor ya interpretado (`Int(i64)`, `Float(f64)`, `Str(String)`),
identificadores, palabras clave (`Let`, `Fn`, `If`…), operadores (`Plus`, `EqEq`,
`Arrow`…) y un centinela `Eof` que marca el final.

> **Decisión temprana: la posición desde el primer carácter.** Cada token guarda su
> `(línea, columna)`. Es muchísimo más fácil registrar la posición *mientras*
> tokenizamos que intentar reconstruirla después. Y es lo que permite que todos los
> errores del lenguaje —léxicos, sintácticos, de tipos— digan exactamente *dónde*
> está el problema. Lo tratamos como un principio, no como un extra.

Un detalle de nomenclatura que conviene fijar: `Int(i64)` es el **literal** `42`,
mientras que `IntType` es la **palabra clave de tipo** `int`. Son cosas distintas y
el lexer las distingue.

## El cursor

El lexer es, en esencia, un **cursor** que avanza por los caracteres del fuente.
Trabajamos sobre un `Vec<char>` (no sobre bytes) para no partir por la mitad un
carácter Unicode, y llevamos tres datos: la posición actual, y la línea y columna
"vivas".

Todo el lexer se construye sobre tres primitivas:

```rust
fn peek(&self) -> Option<char>      // mira el carácter actual sin consumirlo
fn advance(&mut self) -> char       // consume el actual y actualiza línea/columna
fn match_char(&mut self, c: char)   // consume solo si coincide con c
```

`advance` es donde se lleva la cuenta de la posición: al consumir un `\n`, la línea
sube y la columna vuelve a 1; en otro caso, la columna avanza. Con estas tres
piezas —mirar, consumir, consumir-si-coincide— se reconoce todo el lenguaje.

## El bucle principal

El corazón del lexer es un bucle que repite tres pasos:

1. **Saltar** espacios en blanco y comentarios (no producen tokens).
2. **Congelar** la posición de inicio del próximo token.
3. **Emitir** exactamente un token, consumiendo los caracteres que le correspondan.

Congelar la posición de inicio antes de leer el token es lo que hace que tanto el
token como sus posibles errores apunten al *comienzo* de la pieza, no a media
palabra. El bucle termina emitiendo un `Eof`, sobre el cual se apoyará el parser
para saber dónde acaba todo sin comprobar constantemente "¿quedan tokens?".

## Las decisiones interesantes

Reconocer un token parece mecánico, pero hay tres ideas que vale la pena mirar de
cerca.

### Munch máximo: el token más largo gana

Al ver un `-`, ¿es una resta o el principio de la flecha `->`? La regla es
*maximal munch*: siempre el token más largo posible. Por eso, tras consumir `-`,
miramos si sigue un `>` con `match_char` para decidir entre `Arrow` y `Minus`. Lo
mismo para `==` vs `=`, `!=` vs `!`, `<=` vs `<`. El lexer prefiere siempre la
pieza más larga que encaje.

### Las palabras clave son identificadores que resultaron estar reservados

Un truco elegante: el lexer **no** tiene una rama especial para cada palabra clave.
Lee el identificador completo —cualquier secuencia de letras, dígitos y guiones
bajos que empiece por letra o `_`— y *después* consulta una tabla:

```rust
fn keyword(s: &str) -> Option<TokenKind> {
    Some(match s {
        "let" => TokenKind::Let,
        "fn"  => TokenKind::Fn,
        "if"  => TokenKind::If,
        // ...
        _ => return None, // no es palabra clave: es un identificador normal
    })
}
```

Por eso `while123` es un identificador y no la palabra clave `while`: solo coincide
si la cadena es *exactamente* `while`. Y por eso `print` sale como identificador —
en raylang `print` es una función *incorporada* (builtin), no una palabra clave, lo
que el checker sabrá manejar más adelante.

### Números: entero o flotante, decidido por un punto

Al leer dígitos, el lexer mira si viene un `.` *seguido de otro dígito* para decidir
si es flotante. Esa segunda condición es importante: evita que `1.` o un futuro
`1.metodo()` se confundan con un número decimal.

## Errores con ubicación, desde el día uno

El lexer puede toparse con cosas que no sabe leer: una cadena sin cerrar, una
secuencia de escape inválida, un carácter inesperado como `@`, o un `&` suelto
(raylang solo tiene `&&`). En todos esos casos produce un `LexError` que lleva el
mensaje **y la posición**:

```
error léxico en 3:14: cadena sin cerrar
```

En esta fase, el lexer se detiene en el primer error. Acumular varios errores y
*recuperarse* para seguir tokenizando es un tema rico, pero lo dejamos para más
adelante para no complicar el arranque.

## Probándolo

Cada fase de raylang viene con sus tests. Los del lexer comprueban lo esperado
—literales, operadores de uno y dos caracteres, palabras clave vs identificadores,
escapes en cadenas— pero también lo importante de verdad: que **las posiciones son
correctas**. Por ejemplo, dado:

```
let x
  42
```

el token `42` debe reportarse en la línea 2, columna 3. Ese tipo de test es el que
te salva cuando, fases más adelante, un mensaje de error apunte al lugar
equivocado.

## Lo que sigue

Tenemos una secuencia plana de tokens. Por sí sola no dice nada sobre la
*estructura* del programa: no sabe que `fib(n - 1)` es una llamada cuyo argumento es
una resta. Darle esa estructura —convertir la lista de tokens en un árbol— es el
trabajo del **parser**.

> El código de esta fase vive en `src/token.rs` y `src/lexer.rs`.
