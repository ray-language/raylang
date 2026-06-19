# El parser

El lexer nos dejó una secuencia *plana* de tokens. Por sí sola no dice nada sobre
la **estructura** del programa: que `fib(n - 1)` es una llamada cuyo argumento es
una resta, o que en `1 + 2 * 3` la multiplicación se evalúa antes que la suma. El
**parser** (o *analizador sintáctico*) le da esa estructura: convierte la lista de
tokens en un **árbol de sintaxis abstracta** (AST).

```
[Int(1), Plus, Int(2), Star, Int(3)]   →      (+)
                                              /   \
                                             1    (*)
                                                 /   \
                                                2     3
```

## El árbol: el AST

Antes del parser, definimos la forma del árbol. Es "abstracto" porque descarta los
detalles sintácticos que no importan para la semántica: los paréntesis de
agrupación, los espacios, el `;` final. Solo guarda la forma esencial.

El AST de raylang tiene unos pocos tipos de nodo. Una **expresión** produce un
valor:

```rust
pub enum ExprKind {
    Int(i64), Float(f64), Bool(bool), Str(String),  // literales
    Ident(String),                                   // una variable
    Unary  { op: UnaryOp,  expr: Box<Expr> },        // -x, !b
    Binary { op: BinaryOp, left: Box<Expr>, right: Box<Expr> }, // a + b
    Call   { callee: Box<Expr>, args: Vec<Expr> },   // f(a, b)
    If     { cond: Box<Expr>, then_branch: Block, else_branch: Option<Box<Expr>> },
    While  { cond: Box<Expr>, body: Block },
    Block(Block),
}
```

Una **sentencia** se ejecuta por su efecto (declarar una variable, asignar,
retornar). Y como en el lexer, cada nodo guarda su `(línea, columna)`: el checker de
la próxima fase los necesitará para señalar errores de tipos.

> Nota de diseño que volverá a aparecer: el tipo `Type` se modela como un `enum`
> **pensado para crecer**. En M1 solo tiene primitivos, pero su forma admitirá
> mañana genéricos y tipos suma sin reescritura.

## La técnica: descenso recursivo

raylang usa *recursive descent* (descenso recursivo), la técnica de parseo más
directa y la más fácil de leer: **hay una función por cada regla de la gramática**,
y las reglas se llaman entre sí reflejando la estructura del lenguaje. Una función
`block` que llama a `statement`, que puede llamar a `expression`, etcétera.

El parser es un cursor sobre los tokens, con primitivas análogas a las del lexer:
`peek` (mira el token actual), `advance` (lo consume), `check` (¿es este tipo?),
`eat` (cónsumelo si coincide) y `expect` (cónsumelo o produce un error claro).

## La idea más bonita: precedencia por jerarquía

¿Cómo logra el parser que `1 + 2 * 3` se agrupe como `1 + (2 * 3)` sin ninguna tabla
de precedencia? La respuesta es elegante: **la precedencia está codificada en la
jerarquía de funciones**. Las reglas se encadenan de menor a mayor precedencia:

```
expression → logic_or → logic_and → equality → comparison
           → term → factor → unary → call → primary
```

Cada nivel más profundo amarra más fuerte. Como el `*` vive en `factor` —un nivel
más profundo que el `+`, que vive en `term`—, la multiplicación se agrupa primero.
Cada función tiene la misma forma: parsea el nivel superior, y luego, en un bucle,
consume sus propios operadores:

```rust
fn term(&mut self) -> Result<Expr, ParseError> {
    let mut left = self.factor()?;            // primero, lo que amarra más fuerte
    loop {
        let op = match self.peek_kind() {
            TokenKind::Plus  => BinaryOp::Add,
            TokenKind::Minus => BinaryOp::Sub,
            _ => break,
        };
        self.advance();
        let right = self.factor()?;
        left = make_binary(op, left, right);  // construye hacia la izquierda
    }
    Ok(left)
}
```

Ese bucle, que cuelga cada nuevo operando del árbol acumulado a la izquierda, es lo
que da **asociatividad a la izquierda**: `1 - 2 - 3` se agrupa como `(1 - 2) - 3`,
que es lo correcto para la resta.

## Orientación a expresiones: con bloque y sin bloque

Aquí el parser refleja la decisión de diseño más estructural de raylang. Como `if`
y los bloques **producen valor**, hay que distinguir dos familias de expresiones:

- **Expresiones con bloque**: `if`, `while`, `{ ... }`.
- **Expresiones sin bloque**: literales, llamadas, operadores.

La regla, tomada de Rust, gobierna cuándo una expresión necesita `;` para ser
sentencia:

- Una expresión **con bloque** puede usarse como sentencia **sin** `;`.
- Una expresión **sin bloque** necesita `;`.
- El **valor de un bloque** es su expresión final escrita **sin** `;` (el *tail*);
  si no hay tal expresión, el bloque vale `unit`.

Esto se ve en cómo el parser procesa un bloque. Tras parsear una expresión, decide:

```rust
if self.eat(&TokenKind::Semicolon) {
    // `expr ;`  → sentencia de expresión (su valor se descarta)
} else if self.check(&TokenKind::RBrace) {
    // `expr }`  → es el valor final del bloque (el tail)
} else if expr_has_block(&expr) {
    // if/while/{} usados como sentencia: no necesitan ';'
} else {
    // error: se esperaba ';'
}
```

Esa pequeña decisión es la que hace que en `fib` el `if` final sea el *valor* de la
función (retorno implícito), mientras que `i = i + 1;` es una sentencia.

## El resto de la gramática

Las demás reglas son traducciones directas de la gramática:

- **`if` como expresión**: `if (cond) { ... } else { ... }`, donde el `else` puede
  ser un bloque o, encadenado, otro `if` (el `else if`). Se modela con una rama
  `else` opcional que es a su vez una expresión.
- **Llamadas**: tras parsear una expresión primaria, mientras siga un `(`, se
  consume una lista de argumentos. Esto permite `f(x)`, y deja la puerta abierta a
  `f(x)(y)` del futuro.
- **Primarias**: literales, identificadores, y el paréntesis de agrupación —que no
  deja rastro en el AST: solo afecta el orden de parseo.

## Errores

Igual que el lexer, el parser produce errores con ubicación: `se esperaba ';' al
final de la declaración`, `se esperaba un tipo`, `se esperaba una expresión`. La
función `expect` centraliza el patrón "consume esto o explica qué faltaba".

## Probándolo

Para los tests del parser usamos un truco cómodo: renderizar las expresiones como
*S-expressions* (notación de Lisp), que hacen la estructura visible de un vistazo:

```
1 + 2 * 3            →  (+ 1 (* 2 3))
1 - 2 - 3            →  (- (- 1 2) 3)
a || b && c == d     →  (|| a (&& b (== c d)))
```

Así un test de precedencia es una sola línea legible. Con esto verificamos
precedencia, asociatividad, el efecto de los paréntesis, el `else if` encadenado, y
que un bloque distingue sus sentencias de su valor final.

## Lo que sigue

Tenemos un árbol que representa fielmente la *sintaxis* del programa. Pero el parser
acepta cosas que no tienen *sentido*: sumar un `bool` con un `string`, usar una
variable no declarada, o que una función `-> int` no devuelva un entero. Detectar
todo eso —darle *significado* y reglas al árbol— es trabajo del **checker**.

> El código de esta fase vive en `src/ast.rs` y `src/parser.rs`.
