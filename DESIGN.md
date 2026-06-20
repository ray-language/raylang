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
- Argumentos de CLI / I/O: **no** se meten en la firma de `main`; se exponen por
  funciones de la API de runtime (`args()`, `input()`, `env()`, stderr), estilo
  Go/Python. Ver `IDEAS.md` §10.
- `int / int` → entera truncada; `float / float` → flotante. Mezcla = error.
- `string + string` (concatenación) → sí, en M7 con la stdlib; en M1 solo se
  imprime.
- Comentarios de bloque `/* */` → más adelante.

## 12. M3 — Datos compuestos (arreglos y structs)

Hito que da a raylang sus primeros tipos compuestos. Decisiones cerradas:
**semántica de referencia** (los compuestos viven en el *heap*, compartidos) y
**arreglos dinámicos** (listas que crecen). La memoria se gestiona con conteo de
referencias hasta que la GC de M4 la sustituya (y resuelva los ciclos).

### 12.1 Arreglos
- **Tipo**: `[T]` — p. ej. `[int]`, `[[bool]]`. Tipado **estructural**
  (`[int]` ≡ `[int]`).
- **Literal**: `[1, 2, 3]`. El vacío necesita anotación: `let xs: [int] = [];`.
- **Indexar**: `a[i]` con `i: int`. Fuera de rango → error de ejecución.
- **Asignar elemento**: `a[i] = x;`.
- **Builtins**: `len(a) -> int`, `push(a, x)` (muta, devuelve unit).

### 12.2 Structs
- **Declaración** (nivel superior, como las funciones):
  `struct Punto { x: int, y: int }`
- **Literal**: `Punto { x: 1, y: 2 }` (todos los campos, nombrados).
- **Acceso**: `p.x`. **Asignación de campo**: `p.x = 5;`.
- **Tipado nominal**: `Punto` es un tipo distinto; dos structs con los mismos
  campos pero distinto nombre **no** son intercambiables.
- En M3 los structs son solo datos; los métodos llegan con UFCS/traits (M7).

### 12.3 Mutabilidad (importante)
`let`/`var` controlan **reasignar la variable**, no el contenido del objeto
apuntado. Con `let a: [int] = [1, 2];` no puedes `a = [3]` (rebind), pero sí
`a[0] = 9` o `push(a, 3)` (mutar el objeto compartido). Es el modelo de Python/JS
(`const` ata la variable, no congela el objeto). La inmutabilidad profunda queda
como posible refinamiento futuro.

### 12.4 Sistema de tipos
- `Type` crece con `Array(Box<Type>)` y `Struct(nombre)` — las variantes que
  anticipamos en §4. El checker registra las definiciones de struct en una
  pre-pasada (como las funciones).
- Igualdad `==`: **estructural** (elemento a elemento / campo a campo) para
  compuestos.

### 12.5 Runtime (intérprete y VM)
- `Value` crece con `Array(Rc<RefCell<Vec<Value>>>)` y un struct análogo: `Rc` da
  el compartir (referencia), `RefCell` la mutación interior. La GC de M4
  reemplazará el `Rc` para manejar ciclos.

### 12.6 Léxico/sintaxis nuevos
- Tokens nuevos: `[` `]` y `.` (acceso a campo; el mismo `.` servirá para UFCS).
- `struct` pasa a ser palabra reservada.

### 12.7 Sub-fases
- **M3.1**: arreglos (tipo, literal, indexar, asignar, `len`/`push`).
- **M3.2**: structs (declaración, literal, acceso, asignación de campo).

## 13. M4 — Closures y recolección de basura

Hito doble en el que **una feature obliga a la otra**. Las closures permiten que un
valor capturado **sobreviva al marco de pila** que lo creó: ese valor debe escapar
al heap, y una vez en el heap los valores capturados se referencian libremente y
forman **ciclos** que el `Rc` de M3 no sabe liberar. Por eso M4 introduce un
**recolector de basura trazador** (mark-and-sweep) que sustituye al `Rc`.

### 13.0 Las dos decisiones de captura/memoria

- **Captura por referencia (upvalues).** Una closure comparte la *celda* de la
  variable capturada, no una copia: puede **leer y mutar** un `var` capturado, y el
  cambio se ve fuera de la closure. Es la closure "de verdad" (clox/JS).
- **El GC vive en la VM, no en el intérprete.** El intérprete es un *tree-walker*:
  sus valores vivos están dispersos en la pila de llamadas de Rust, raíces
  imposibles de enumerar → **se queda con `Rc`** (representa el entorno como cadena
  de ámbitos compartidos). La VM tiene su estado reificado (pila, marcos, locales):
  sus raíces son explícitas y enumerables → **aquí vive el mark-and-sweep**. El
  oráculo compara resultados observables, no memoria, así que ambos motores siguen
  debiendo coincidir.

### 13.1 Funciones de primera clase
- **Tipo función**: `fn(T1, T2) -> R` — p. ej. `fn() -> int`, `fn(int, int) -> int`.
  Es la variante `Type::Fn(params, ret)` anticipada en §4.
- **Función anónima** (expresión): misma firma que una nombrada **sin el nombre**:
  `fn(x: int) -> int { x + 1 }`. Reutiliza la gramática de `fn`; en posición de
  expresión, `fn` abre una función anónima. Sin ambigüedad: la `fn` de nivel
  superior lleva nombre; la de expresión va seguida de `(`.
- Las funciones se pueden **pasar como argumento, devolver y guardar** en variables,
  arreglos y campos.
- **Igualdad/impresión**: las funciones **no** son comparables (`==`); se imprimen
  como un marcador opaco `<fn>`. (No tienen identidad estructural.)

### 13.2 Closures (captura de entorno)
- Una función anónima que referencia variables de un ámbito envolvente es una
  **closure**: empaqueta el código más sus **upvalues** (las celdas capturadas).
- **Upvalues en la VM** (el mecanismo central): mientras la variable capturada sigue
  en la pila, el upvalue está **abierto** (apunta a la ranura de la pila); cuando su
  marco se descarta, el upvalue se **cierra** (el valor se mueve al heap, dentro del
  objeto upvalue). La VM mantiene una lista de upvalues abiertos para compartir la
  misma celda entre varias closures.
- **En el intérprete**: el entorno se representa como una cadena de ámbitos
  compartidos (`Rc`), y la closure captura una referencia a esa cadena. Misma
  semántica observable, sin trazado.

### 13.3 Mutabilidad y captura
La captura por referencia respeta el modelo de §5/§12.3: capturar **no** reata la
variable. Una closure puede mutar un `var` capturado (su celda es compartida); un
`let` capturado se lee pero no se reasigna. Ejemplo canónico (contador con estado):

```rust
fn contador() -> fn() -> int {
    var n: int = 0;
    fn() -> int { n = n + 1; n }   // captura y muta n
}
// let c = contador(); c() -> 1; c() -> 2; c() -> 3   (n persiste en el heap)
```

### 13.4 Sistema de tipos
- `Type` crece con `Fn(Vec<Type>, Box<Type>)`. El checker tipa la función anónima
  como su firma, valida el cuerpo igual que una función nombrada (incluido el
  análisis de divergencia para el retorno implícito) y comprueba la aridad/tipos en
  la llamada de un valor-función.
- La captura se valida en el checker: una variable referenciada de un ámbito
  envolvente queda marcada como **capturada** (información que el compilador usa
  para emitir upvalues).

### 13.5 Runtime
- **Intérprete**: `Value::Closure` con el cuerpo (referencia al AST) y el entorno
  capturado (`Rc` de la cadena de ámbitos). Las variables pasan a vivir en celdas
  compartidas (`Rc<RefCell<Value>>`) para que la captura por referencia funcione.
- **VM**: objetos gestionados por el **heap del GC** (no `Rc`): arreglos, structs,
  closures y celdas de upvalue. Un `Value` compuesto referencia un objeto del heap
  por **handle**. Las funciones compiladas (`CompiledFn`) son datos estáticos del
  programa, **no** se recolectan; una closure referencia su `CompiledFn` + sus
  upvalues.

### 13.6 El recolector (mark-and-sweep, solo VM)
- **Raíces**: la pila de operandos, las locales de todos los marcos, la lista de
  upvalues abiertos. (Las funciones compiladas y constantes son estáticas.)
- **Marca**: desde las raíces, marca recursivamente todo lo alcanzable.
- **Barrido**: recorre el heap, libera lo no marcado, limpia las marcas de los
  sobrevivientes. **Los ciclos se liberan** (a diferencia del `Rc`).
- **Disparo**: cuando el tamaño del heap cruza un umbral que **crece** tras cada
  recolección (estilo clox `nextGC`). Un **modo de estrés** (recolectar en cada
  asignación) se usa en tests para destapar bugs de raíces faltantes.

### 13.7 Léxico/sintaxis nuevos
- **Ninguna palabra clave nueva**: `fn` ya existe; solo gana un uso en posición de
  expresión. El tipo `fn(...) -> R` reutiliza tokens existentes.

### 13.8 Sub-fases
- **M4.1**: funciones de primera clase (tipo `Fn`, función anónima sin captura,
  pasar/retornar/guardar). Sin upvalues ni GC todavía.
- **M4.2**: closures (captura por referencia; upvalues en la VM, entorno compartido
  en el intérprete).
- **M4.3**: GC mark-and-sweep en la VM (heap, raíces, marca/barrido, disparo y modo
  de estrés); reemplaza el `Rc` de la VM.
