# raylang — Documento de diseño

> Borrador v0.2 · lenguaje de aprendizaje · host: Rust
> Este documento es el **contrato** del lenguaje. Lexer, parser, type checker,
> intérprete y VM deben respetarlo. Cambiarlo es una decisión deliberada, no un
> accidente de implementación.

## 0. Decisiones fundacionales (cerradas)

Estas decisiones moldean los cimientos y ya están tomadas. El resto del documento
se deriva de ellas.

| Eje | Decisión | Por qué |
|-----|----------|---------|
| Host | **Rust** | enums + `match` exhaustivo ideales para ASTs y compiladores |
| Ejecución | **intérprete (M1) → bytecode + VM (M2)** | el intérprete es el oráculo correcto contra el cual validar la VM |
| Tipado | **estático, anotaciones explícitas** | type checker real sin el coste de la inferencia global |
| Sintaxis | **llaves estilo C/Rust** | sin ambigüedad de indentación; parser directo |
| Mutabilidad | **`let` inmutable / `var` mutable** | regla semántica que el checker hace cumplir |
| Orientación | **a expresiones** (estilo Rust/ML) | `if`/bloques producen valor; compone con pipelines; rico pedagógicamente |
| Errores | **errores como valores** (`Result`/`Option` + `?`) | obliga a construir tipos suma + genéricos + pattern matching (el plato fuerte) |
| Métodos/pipes | **UFCS** (`s.trim()` ≡ `trim(s)`) + `\|>` | unifica métodos nativos, pipelines y métodos de structs en UN mecanismo |

## 1. Filosofía y objetivos

raylang es un lenguaje pequeño, **estáticamente tipado** con anotaciones
explícitas, sintaxis de llaves, **orientado a expresiones**, pensado para
**aprender a construir lenguajes**. No busca ser original ni práctico: busca tocar
de forma honesta cada fase de un compilador/intérprete.

Principios de diseño:

1. **Sin ambigüedad sintáctica.** Toda construcción debe ser parseable sin
   adivinanzas. Preferimos verboso pero claro.
2. **Tipos verificados antes de ejecutar.** Un programa mal tipado nunca corre.
3. **Errores con posición.** Todo token y nodo carga `(línea, columna)`. Se diseña
   desde el día 1, no se agrega después.
4. **Un solo front-end.** Lexer + parser + checker se escriben una vez; el
   intérprete y la VM solo cambian el *backend*.
5. **No pintarse en una esquina.** Aunque M1 solo use tipos primitivos, las
   estructuras de datos del compilador (sobre todo el tipo `Type`) se diseñan
   **extensibles** para admitir genéricos y tipos suma más adelante sin cirugía.

## 2. Hoja de ruta (hitos)

El front-end (lexer, parser, checker) se comparte. Los hitos M3+ son features del
lenguaje; el orden puede flexibilizarse al avanzar.

| Hito | Contenido | Aprendes |
|------|-----------|----------|
| **M1** | lexer + parser + checker + **intérprete**; expresiones, primitivos, funciones | pipeline completo, type checking, orientación a expresiones |
| **M2** | reescribir backend como **bytecode + VM** (mismo front-end) | diseño de VM, stack frames |
| **M3** | structs + arreglos | layout de datos en memoria |
| **M4** | closures + **garbage collector** | captura de entorno, GC |
| **M5** | **tipos suma (`enum`) + pattern matching (`match`)** | uniones etiquetadas, exhaustividad |
| **M6** | **genéricos** → habilita `Option<T>` / `Result<T,E>` + operador `?` | tipos paramétricos, propagación de errores |
| **M7** | **UFCS (`.`) + pipelines (`\|>`) + stdlib** (`trim`, `len`, `split`…) | azúcar sobre llamada, resolución de métodos |
| **M8** | inferencia local (`let x = 3`), REPL, mejores errores | unificación básica, tooling |

Este documento especifica **M1** en detalle y fija el norte de lo posterior.

## 3. Léxico

### 3.1 Comentarios
```
// comentario de línea, hasta el fin de línea
```
(Comentarios de bloque `/* */` quedan para después.)

### 3.2 Palabras clave reservadas (M1)
```
let  var  fn  return  if  else  while  true  false
int  bool  float  string
```
Reservadas para el futuro (el lexer puede ya conocerlas): `enum match struct`.

### 3.3 Identificadores
`[a-zA-Z_][a-zA-Z0-9_]*` — no pueden coincidir con una palabra clave.

### 3.4 Literales
- **Entero**: `[0-9]+` → `int` (entero con signo de 64 bits).
- **Flotante**: `[0-9]+\.[0-9]+` → `float` (IEEE-754 de 64 bits).
- **Booleano**: `true` | `false` → `bool`.
- **Cadena**: `"..."` con escapes `\n \t \\ \"` → `string`.

### 3.5 Operadores y signos de puntuación
```
+  -  *  /  %          aritméticos
==  !=  <  <=  >  >=   comparación
&&  ||  !              lógicos
=                     asignación
( )  { }              agrupación / bloques
,  ;  :               separadores
->                    flecha de tipo de retorno
```
Reservados para el futuro: `|>` (pipeline), `.` (UFCS), `?` (propagación),
`@` (anotaciones, p. ej. `@test`/`@derive`), `<` `>` también delimitarán
argumentos genéricos.

### 3.6 Espacios en blanco
Separan tokens y por lo demás se ignoran (la sintaxis usa `;` y `{}`, no la
indentación).

## 4. Sistema de tipos (M1)

Tipos primitivos:

| Tipo | Descripción |
|------|-------------|
| `int` | entero con signo 64 bits |
| `float` | flotante 64 bits |
| `bool` | `true` / `false` |
| `string` | cadena UTF-8 inmutable |
| `unit` | tipo del "nada"; valor `()`. Es el tipo de `while`, de un `if` sin `else`, y de un bloque sin expresión final |

Reglas clave:

- **No hay conversiones implícitas.** `int + float` es **error de tipo**.
- Variables: tipo declarado obligatorio en M1 (`let x: int = 0;`). La inferencia
  llega en M8.
- Funciones: tipo de cada parámetro y de retorno explícitos. El retorno se omite
  cuando es `unit`.

> **Nota de arquitectura (clave para no bloquear el futuro).** En el checker, el
> tipo `Type` se modela como un enum **pensado para crecer**. En M1 tendrá solo
> `Int, Float, Bool, String, Unit`, pero su forma debe admitir mañana una variante
> tipo `Named(nombre, Vec<Type>)` para `Option<int>`, `Result<int, string>`,
> structs y enums, sin reescritura. No cuesta nada hoy y abre la puerta a M3–M7.

## 5. Variables y mutabilidad

```
let x: int = 5;     // inmutable: reasignar x es error semántico
var y: int = 0;     // mutable:   y = y + 1;  es válido
```

## 6. Orientación a expresiones

raylang distingue **expresión** (produce un valor) de **sentencia** (se ejecuta
por su efecto). La regla central, tomada de Rust:

- Hay **expresiones con bloque** (`if`, `while`, `{ ... }`) y **expresiones sin
  bloque** (literales, llamadas, operadores).
- Una expresión **con bloque** puede usarse como sentencia **sin** `;` final.
- Una expresión **sin bloque** necesita `;` para ser sentencia.
- El **valor de un bloque** es su expresión final escrita **sin** `;`. Si el
  bloque no termina en una expresión, su valor es `unit`.
- **Retorno implícito**: el valor del bloque cuerpo de una función es su valor de
  retorno. `return` existe además para salida temprana.

Consecuencias de tipado:

- `if (c) { a } else { b }` como expresión exige que `a` y `b` tengan el **mismo
  tipo** (ese es el tipo del `if`).
- `if` **sin** `else` tiene tipo `unit` (no puede usarse como valor distinto de
  unit).
- `while` siempre tiene tipo `unit`.
- La condición de `if`/`while` debe ser `bool` (no hay "truthy").

Ejemplo del estilo:
```rust
let abs: int = if (x < 0) { -x } else { x };   // if como expresión
```

## 7. Gramática (EBNF, M1)

Notación: `{ X }` = cero o más, `[ X ]` = opcional, `|` = alternativa,
`'literal'` = token literal.

```ebnf
program        = { function } ;

function       = 'fn' IDENT '(' [ params ] ')' [ '->' type ] block ;
params         = param { ',' param } ;
param          = IDENT ':' type ;

type           = 'int' | 'bool' | 'float' | 'string' ;   (* M1: solo primitivos *)

block          = '{' { statement } [ expression ] '}' ;
                 (* la 'expression' final SIN ';' es el valor del bloque;
                    si falta, el bloque vale unit *)

statement      = letDecl
               | varDecl
               | assignStmt
               | returnStmt
               | exprStatement ;

letDecl        = 'let' IDENT ':' type '=' expression ';' ;
varDecl        = 'var' IDENT ':' type '=' expression ';' ;
assignStmt     = IDENT '=' expression ';' ;
returnStmt     = 'return' [ expression ] ';' ;

exprStatement  = exprWithBlock [ ';' ]          (* if/while/bloque: ';' opcional *)
               | exprWithoutBlock ';' ;         (* lo demás: ';' obligatorio *)

expression       = exprWithBlock | exprWithoutBlock ;

exprWithBlock    = ifExpr | whileExpr | block ;
ifExpr           = 'if' '(' expression ')' block [ 'else' ( block | ifExpr ) ] ;
whileExpr        = 'while' '(' expression ')' block ;

(* expresiones sin bloque, por precedencia de menor a mayor *)
exprWithoutBlock = logicOr ;
logicOr        = logicAnd { '||' logicAnd } ;
logicAnd       = equality { '&&' equality } ;
equality       = comparison { ( '==' | '!=' ) comparison } ;
comparison     = term { ( '<' | '<=' | '>' | '>=' ) term } ;
term           = factor { ( '+' | '-' ) factor } ;
factor         = unary { ( '*' | '/' | '%' ) unary } ;
unary          = ( '!' | '-' ) unary | call ;
call           = primary { '(' [ args ] ')' } ;
args           = expression { ',' expression } ;
primary        = INT | FLOAT | STRING | 'true' | 'false'
               | IDENT
               | '(' expression ')' ;
```

Notas:
- La precedencia está **codificada en la jerarquía** (logicOr → … → factor →
  unary → call → primary). Mayor profundidad = mayor precedencia. Técnica clásica
  de *recursive descent*.
- Dentro de `block`, el parser decide si una expresión es sentencia o valor final
  con un lookahead simple: si tras parsear la expresión viene `}`, es el valor del
  bloque; si no, es una sentencia.
- `print` es un **builtin** (no palabra clave) con firma especial
  `print(int|float|bool|string) -> unit`, resuelta de forma simple en M1.
- Punto de entrada: función `main() -> int` (o `-> unit`).

## 8. Semántica (resumen para checker e intérprete)

- **Scoping**: léxico, por bloque. Cada `block` abre un ámbito; shadowing en
  bloques interiores permitido.
- **Resolución de nombres**: variable declarada antes de usarse en su ámbito. Las
  funciones se registran en una pasada previa (permite recursión y llamadas hacia
  adelante).
- **Type checking** (pasada sobre el AST resuelto):
  - aritmética: ambos `int` → `int`, ambos `float` → `float`; mezcla = error.
  - comparaciones: ambos lados del mismo tipo ordenable → `bool`.
  - `&& || !`: sobre `bool` → `bool`.
  - condición de `if`/`while`: `bool`.
  - `if`/bloque como expresión: reglas de §6.
  - `return e`: tipo de `e` coincide con el retorno declarado; el valor final del
    cuerpo también.
  - asignar a un `let` = error.
- **Ejecución (intérprete, M1)**: tree-walking sobre el AST tipado. Entorno = pila
  de marcos nombre→valor; cada llamada crea un marco. Errores de runtime en M1
  (p. ej. división por cero) terminan el programa con un mensaje; el manejo de
  errores *del lenguaje* (`Result`/`?`) llega en M6.

## 9. Programa de ejemplo (objetivo de M1)

```rust
fn fib(n: int) -> int {
    if (n < 2) {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn main() -> int {
    var i: int = 0;
    while (i < 10) {
        print(fib(i));
        i = i + 1;
    }
    0
}
```

Ejercita: lexer (todos los tokens), parser (funciones, `if`/`while` como
expresiones, llamadas, precedencia, valor de bloque), checker (tipos de retorno,
ramas del `if` del mismo tipo, condición bool, recursión) e intérprete (marcos de
llamada, mutación, retorno implícito, builtin `print`).

## 10. Norte de diseño (features posteriores, ya comprometidas)

Registradas para no construir nada que las bloquee:

- **Tipos suma + pattern matching (M5)**: `enum`, `match` con exhaustividad.
- **Genéricos (M6)**: `Option<T>`, `Result<T,E>`; operador `?` para propagar
  errores. Es el modelo de manejo de errores del lenguaje.
- **UFCS + pipelines (M7)**: `s.trim()` ≡ `trim(s)`; `x |> f(a)` ≡ `f(x, a)`. Los
  métodos de tipos nativos, los pipelines y los métodos de structs son **el mismo
  mecanismo**: azúcar sobre la llamada a función libre.
- **Stdlib (M7)**: funciones sobre `string` (`trim`, `len`, `split`, …) y demás,
  conocidas por el checker.

## 11. Decisiones pendientes (menores; las cerramos al llegar)

- ¿`main` obligatoria, o permitir top-level? → propuesta: `main` obligatoria.
- `int / int` → entera truncada; `float / float` → flotante. Mezcla = error.
- `string + string` (concatenación) → sí, en M7 con la stdlib; en M1 solo se
  imprime.
- Comentarios de bloque `/* */` → más adelante.
