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
Activadas después: `struct` (M3), `enum` y `match` (M5).

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
- **Upvalues en la VM por *boxing* de las variables capturadas.** Una variable que
  alguna closure anidada captura se guarda, desde el inicio, en una **celda
  compartida** (`Rc<RefCell<Value>>`) en vez de directamente en la ranura local. La
  closure captura un clon de esa celda; leer/escribir el upvalue va a la misma
  celda, así que la mutación es visible para el dueño y para las closures hermanas, y
  la celda sobrevive al marco (vive mientras alguna closure la referencie).
  - El **compilador resuelve los upvalues** al estilo clox: cuando el cuerpo de una
    función nombra una variable que no es local suya, la busca en la función
    envolvente (un upvalue **local**) o, transitivamente, entre los upvalues de
    aquélla (un upvalue **de upvalue**). Esa resolución es la pieza central, y marca
    qué locales del marco envolvente deben *boxearse*.
  - **Por qué boxing y no el clásico abierto/cerrado.** En clox las locales viven en
    la pila de operandos, así que un upvalue *abierto* apunta a una ranura y se
    *cierra* (copia al heap) al salir del marco. En raylang las locales viven en un
    **arreglo aparte por marco** (decisión de M2.3): no hay ranura de pila a la que
    apuntar, y boxear la variable capturada desde el inicio es lo natural. Es el
    mismo concepto (la variable escapa al heap) sin la optimización de retrasar la
    copia; un refinamiento posible más adelante.
- **En el intérprete**: el entorno se representa con variables en celdas compartidas
  (`Rc<RefCell<Value>>`); la closure **captura las celdas visibles** en su punto de
  definición. Misma semántica observable, sin trazado.

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
- **Raíces**: la pila de operandos y las locales de todos los marcos (incluidas las
  celdas *boxeadas*); los objetos closure alcanzables arrastran sus upvalues. (Las
  funciones compiladas y constantes son estáticas.)
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

## 14. M5 — Tipos suma (`enum`) y pattern matching (`match`)

M5 introduce las **uniones etiquetadas** (tipos suma) y su forma de consumo,
`match`, con **exhaustividad** verificada por el checker —la lección central del
hito—. Es la base sobre la que M6 montará `Option<T>`/`Result<T,E>` al sumarle
genéricos.

### 14.0 Decisiones de diseño (cerradas)

Cuatro decisiones fijan el sabor de M5 (las demás las fuerza el norte del lenguaje):

1. **Payload posicional.** Una variante lleva datos como una tupla:
   `Circulo(float)`, `Rect(float, float)`. Sin datos = variante *unit*: `Punto`.
   Variantes con campos nombrados (`Circulo { r: float }`) se **defieren**.
2. **Variantes cualificadas.** Se construyen y se matchean con el nombre del enum
   delante: `Figura.Circulo(2.0)`. Reusa el token `.`; sin colisiones entre enums.
3. **`match` solo sobre enums** (más `_` y *binding*). Patrones de variante, comodín
   y ligar el valor completo a un nombre. Literales en `int`/`bool` se **defieren**.
4. **Patrones planos** (un nivel). El payload se liga a nombres simples o `_`:
   `Circulo(r)`, `Rect(w, h)`. Subpatrones anidados (`Ok(Circulo(r))`) se **defieren**.

Forzado por el norte del lenguaje (no son decisiones abiertas):
- **`enum` es nominal**, como `struct`: la igualdad de tipos compara el nombre.
- **`match` es una expresión**: produce un valor; todos los brazos convergen a un
  mismo tipo (unificado como en `if`/bloques, DESIGN §8).
- **Exhaustividad obligatoria**: sin `null` y siendo `match` una expresión que debe
  producir valor en todo camino, el checker exige cubrir **todas** las variantes o
  incluir un comodín/binding. Es *la* prueba de M5.

### 14.1 Sintaxis nueva

```
// Declaración (nivel superior, junto a struct y fn)
enum Figura {
    Circulo(float),
    Rect(float, float),
    Punto,            // variante unit
}

// Construcción (cualificada)
let a: Figura = Figura.Circulo(2.0);
let b: Figura = Figura.Punto;

// Consumo (match es una expresión). El escrutinio va ENTRE PARÉNTESIS, como las
// condiciones de if/while: es la convención de raylang y evita la ambigüedad con el
// literal de struct `Nombre { ... }`.
let area: float = match (figura) {
    Figura.Circulo(r)  => 3.14159 * r * r,
    Figura.Rect(w, h)  => w * h,
    Figura.Punto       => 0.0,
};
```

- **`enum` y `match` pasan a ser palabras clave** (ya reservadas en DESIGN §3.2). El
  lexer las reconoce desde M5.1.
- **Coma final permitida** tanto en la lista de variantes como en la de brazos.
- El cuerpo de un brazo es una **expresión** (puede ser un bloque `{ ...; valor }`).
- **Enums recursivos permitidos**: `enum Lista { Cons(int, Lista), Nil }`. El tipo es
  nominal (un nombre), así que no hay problema de tamaño infinito: el valor vive
  en el heap (Rc en el intérprete, handle en la VM).

### 14.2 Patrones (planos, M5)

Dentro de un brazo, un patrón es una de tres formas —el parser las distingue sin
ambigüedad porque las variantes van **cualificadas**—:

| Patrón | Forma | Liga | Cubre |
|--------|-------|------|-------|
| Variante | `Figura.Circulo(r)`, `Figura.Punto` | sus sub-bindings | esa variante |
| Sub-binding | un `Ident` o `_` dentro de la variante | nombre ↦ payload (o nada) | — |
| Comodín / binding | `_` o un `Ident` suelto | nada / valor completo ↦ nombre | **todo lo restante** |

- La **aridad** del patrón de variante debe igualar la del payload (el checker lo
  comprueba): `Rect(w, h)` sí, `Rect(w)` error.
- Un `Ident` suelto (no cualificado) es un **binding catch-all**: liga el escrutinio
  entero y, por sí solo, hace exhaustivo el `match`. `_` igual pero sin ligar.
- Las variables ligadas por un patrón son **inmutables** (como los parámetros) y
  viven solo en el cuerpo de su brazo.

### 14.3 Sistema de tipos

- **Tipo nuevo `Type::Enum(String)`** (calca `Type::Struct`): nominal, por nombre.
- `Program` gana `enums: Vec<EnumDef>`; `EnumDef { name, variants, .. }` y
  `VariantDef { name, payload: Vec<Type>, .. }`. Pasada 1 del checker registra las
  firmas de enum (junto a structs y funciones); pasada 2 chequea cuerpos.
- **Construcción** `Figura.Circulo(args)`: el tipo del literal es `Enum("Figura")`;
  los `args` deben tipar contra el payload de la variante (aridad y tipos).
- **`match`**: el escrutinio debe ser un `Enum`; cada patrón de variante debe
  pertenecer a ese enum; los cuerpos de los brazos se **unifican** a un tipo común,
  que es el tipo del `match`.
- **Exhaustividad**: el conjunto de variantes cubiertas debe ser **todas** las del
  enum, salvo que exista un comodín/binding. Variante repetida o inalcanzable
  (después de un catch-all) = error. Mensajes con la(s) variante(s) que faltan.
- **No comparables con `==`** (como las funciones): los enums pueden ser recursivos
  y portar funciones; su igualdad estructural se deja para un `@derive(Eq)` futuro.
  Se consumen por `match`, no por `==`. **Imprimibles**: `print` los muestra como
  `Figura.Circulo(2.0)` (unit: `Figura.Punto`).

### 14.4 Resolución de la construcción (front-end compartido)

`Figura.Circulo(2.0)` y `p.x` tienen la **misma forma sintáctica** (`Ident . Ident`
[`( args )`]): el parser no puede distinguirlas porque no sabe aún qué nombres son
enums. Por eso el parser emite los nodos genéricos de siempre (`Field`/`Call`), y una
**resolución** —parte del front-end, dentro del checker tras registrar las firmas—
**reescribe** los `Field`/`Call` cuya cabeza es un nombre de **tipo enum** a un nodo
explícito `ExprKind::EnumLit { enum_name, variant, args }`.

Así la ambigüedad se resuelve **una sola vez** y el intérprete y la VM reciben un AST
con `EnumLit` explícito —sin duplicar la regla en cada motor—. (Los patrones, en
cambio, se parsean directos: solo aparecen bajo `match`, sin ambigüedad.) El checker
pasa a tomar `&mut Program` para esta reescritura.

### 14.5 Runtime

- **Intérprete**: `Value::Enum(Rc<EnumValue>)` con `EnumValue { enum_name,
  variant, payload: Vec<Value> }`. Construcción evalúa los `args` y arma el valor;
  `match` prueba la variante, liga el payload en un ámbito nuevo y evalúa el brazo.
- **VM**: nuevo `Obj::Enum(VmEnum { variant, payload: Vec<HeapValue> })` en el heap,
  **trazado por el GC** (sus hijos son los handles del payload) — extiende M4.3 con
  un tipo de objeto más. `match` baja a bytecode: leer el *tag* de la variante,
  comparar, extraer el payload a locales, saltar al brazo o al siguiente. Opcodes
  nuevos para leer tag y payload; la cadena de decisión usa los saltos existentes.
  Como el checker garantiza exhaustividad, el "ningún brazo" es inalcanzable (se deja
  un trap defensivo).
- El **oráculo** (intérprete vs VM) cubre M5, incluido el **modo estrés del GC** con
  valores enum vivos.

### 14.6 Léxico/sintaxis nuevos
- Palabras clave **`enum`** y **`match`** (ya reservadas en §3.2; el lexer las activa).
- Token `=>` (flecha gruesa) para los brazos de `match`. El `.` y los `()` se reusan.
- **`match (e) { ... }`**: el escrutinio va entre paréntesis (como if/while), lo que
  evita la ambigüedad con el literal de struct y es consistente con el lenguaje. Los
  brazos se separan con `,` (coma final permitida).

### 14.7 Sub-fases
- **M5.1 — enums y construcción**: `enum`, `Type::Enum`, resolución de
  `Enum.Variante(args)` → `EnumLit`, chequeo de construcción, valores enum en
  intérprete y VM (con GC). Ya se pueden construir, pasar e imprimir enums. Sin
  `match` todavía.
- **M5.2 — `match` y exhaustividad (intérprete)**: `match`, patrones planos, binding
  y `_`; exhaustividad en el checker; ejecución en el intérprete (oráculo de M5.3).
- **M5.3 — `match` en la VM**: bajada a bytecode (tag, payload, saltos); oráculo
  verde, incluido el modo estrés.

### 14.8 Deferido (a M6+ o a un hito de patrones)
- Variantes con **campos nombrados** (`Circulo { r: float }`).
- **Patrones anidados** (`Ok(Circulo(r))`) y **literales** (`0 => ...`, `true => ...`).
- **Enums genéricos** → `Option<T>`/`Result<T,E>` y el operador `?` (M6).
- **`==`/`@derive(Eq, Show)`** para enums.

## 15. M6 — Genéricos, `Option`/`Result` y `?`

M6 añade **polimorfismo paramétrico** (genéricos) y, sobre él, el modelo de manejo de
errores del lenguaje: `Option<T>`, `Result<T, E>` y el operador `?`. Es el hito que
cumple el norte de §0 —**errores como valores, sin `null`**— y el que más toca el
sistema de tipos desde M1.

### 15.0 Decisiones de diseño (cerradas)

1. **Borrado de tipos (*type erasure*).** Los genéricos viven **solo en el checker**.
   Los valores de raylang ya son uniformes (cargan su etiqueta en runtime), así que una
   función genérica solo mueve valores: el intérprete y la VM **no cambian** por los
   genéricos. La lección: *los genéricos son una característica del type checker.*
2. **Inferencia desde los argumentos.** Los argumentos de tipo se **infieren** por
   unificación local (`Option.Some(5)` ⇒ `T = int`); no hay *turbofish*. Para los casos
   que los argumentos no determinan (p. ej. `Option.None`, o `[]`), se usa el **tipo
   esperado** del contexto (chequeo **bidireccional**).
3. **`Option`/`Result` en un *prelude*.** Son enums genéricos **definidos en raylang**
   y autoinyectados; el lenguaje no los trata especial salvo por `?`. El mecanismo
   (enums genéricos) es general: el usuario puede definir su propio `Either<A, B>`.
4. **`?` sobre `Result` y `Option`.** `e?` desempaqueta el valor o **retorna temprano**
   el `Err(e)`/`None`. La función que lo usa debe declarar un retorno compatible.

Forzado por el norte (no son decisiones abiertas):
- **Genéricos no acotados**: sin *traits*/bounds. El código genérico solo hace cosas
  agnósticas al tipo (pasar, guardar, construir/`match` enums conocidos). No puede,
  p. ej., `==` sobre un `T` cualquiera. Los *bounds* son idea futura (IDEAS.md).

### 15.1 Sintaxis nueva

```rust
// Funciones genéricas: parámetros de tipo entre <…> tras el nombre.
fn identidad<T>(x: T) -> T { x }
fn mapear<T, U>(xs: [T], f: fn(T) -> U) -> [U] { /* ... */ }

// Enums y structs genéricos.
enum Caja<T> { Llena(T), Vacia }
struct Par<A, B> { primero: A, segundo: B }

// Uso: los argumentos de tipo se INFIEREN; no se escriben en la llamada.
let n: int = identidad(5);                 // T = int, inferido
let c: Caja<int> = Caja.Llena(3);          // T = int, inferido del argumento
let v: Caja<int> = Caja.Vacia;             // T = int, del tipo ESPERADO

// Option/Result (del prelude) y el operador ?.
fn dividir(a: int, b: int) -> Result<int, string> {
    if (b == 0) { Result.Err("división por cero") } else { Result.Ok(a / b) }
}
fn calcular(x: int, y: int) -> Result<int, string> {
    let q: int = dividir(x, y)?;   // Err -> retorna Err; si no, desempaqueta a int
    Result.Ok(q + 1)
}
```

- Los `<…>` aparecen **solo en posición de tipo** (declaración de parámetros de tipo,
  o anotaciones como `Option<int>`): no hay ambigüedad con `<`/`>` de comparación, que
  viven en posición de **expresión**. Como los argumentos se infieren, no hay `<…>` en
  las llamadas ni en las construcciones.
- El operador **`?`** es *postfix*: `expr?`. Token nuevo: `?`.

### 15.2 Sistema de tipos

- **Parámetro de tipo**: nueva variante `Type::Var(String)` —una `T` dentro de una
  definición genérica—. Es opaca: dos `Var` solo son iguales si tienen el mismo nombre.
- **Aplicación de tipo**: `Type::Struct` y `Type::Enum` pasan a llevar **argumentos**:
  `Struct(String, Vec<Type>)`, `Enum(String, Vec<Type>)` (vacío = no genérico). Así
  `Option<int>` es `Enum("Option", [Int])`. (Es la `Named(nombre, Vec<Type>)` que §1
  anticipó, conservando la distinción struct/enum que el checker ya usa.)
- **Definiciones genéricas**: `Function`, `EnumDef`, `StructDef` ganan
  `type_params: Vec<String>`. Al verificar su cuerpo, esos nombres están en ámbito como
  `Type::Var`.
- **Buena formación** (`ensure_type`): `Option<int>` exige que `Option` exista con la
  **aridad** correcta de parámetros de tipo, y valida cada argumento.

### 15.3 Sustitución, unificación e inferencia

Tres operaciones, todas en el checker:

- **Sustitución** `subst(ty, σ)`: reemplaza cada `Var(name)` por `σ[name]`, recursivo
  bajo `[T]`, `fn(...)`, `Enum/Struct<...>`. Instanciar el payload de `Some(T)` para
  `Option<int>` es `subst(T, {T↦int}) = int`.
- **Unificación** `unify(declarado, real, σ)`: recorre ambos en paralelo; cuando
  `declarado` es `Var(n)`, liga `σ[n] = real` (o exige consistencia si ya estaba);
  cuando ambos son constructores (`Array`, `Enum`, `Fn`, …) recurre en sus componentes;
  desacuerdo = error. Es la inferencia: de `(T) ↔ (int)` sale `T = int`.
- **Chequeo bidireccional**: `check_expr` gana un **tipo esperado** opcional. La mayoría
  de las expresiones lo ignoran; la **construcción** (`EnumLit`/`StructLit`) y los casos
  sin argumentos (`Option.None`, `[]`) lo usan para fijar los parámetros que los
  argumentos no determinan. Esto **subsume** la aspereza del `[]` vacío (IDEAS §12).

**Llamada genérica** `mapear(nums, aTexto)`: se toman frescas las variables de tipo de
`mapear` (`T`, `U`), se **unifican** los tipos de los parámetros con los de los
argumentos (`[T] ↔ [int]`, `fn(T)->U ↔ fn(int)->string`) llenando `σ`, y el tipo del
resultado es `subst([U], σ) = [string]`. Si algún parámetro queda sin determinar y no
hay tipo esperado que lo fije, es error ("no se pudo inferir T").

### 15.4 `Option`/`Result` y el operador `?`

- **Prelude**: una cadena fuente de raylang, **inyectada antes** del programa del
  usuario en el front-end. Contiene:
  ```rust
  enum Option<T> { Some(T), None }
  enum Result<T, E> { Ok(T), Err(E) }
  ```
  El checker, el intérprete y la VM las tratan como enums genéricos normales.
- **`?`** (`ExprKind::Try`): `e?` con `e: Result<T, E>` o `e: Option<T>`.
  - **Tipo**: el valor desempaquetado, `T`.
  - **Contexto**: la función envolvente debe retornar un tipo **compatible**:
    `Result<_, E>` (misma `E`) para `?` sobre `Result`; `Option<_>` para `Option`. El
    checker lo valida.
  - **Semántica**: si es `Ok(v)`/`Some(v)`, el valor es `v`; si es `Err(e)`/`None`,
    **retorna** ese mismo valor de la función. Con erasure, el valor `Err(e)`/`None`
    *es* del tipo de retorno (no guarda argumentos de tipo), así que propagarlo es
    devolverlo tal cual.

### 15.5 Runtime (erasure)

- **Genéricos**: el intérprete y la VM **no cambian**. Un enum genérico es un enum; una
  función genérica es una función. Los argumentos de tipo no existen en runtime.
- **`?`**: se ejecuta **nativo** en ambos motores (no se puede *desugar* a un brazo de
  `match` porque `return` es sentencia, no expresión). El intérprete reusa su señal
  `Flow::Return`; la VM inspecciona el *tag* (Ok/Some = 0, Err/None = 1) y, en el caso
  de error, **retorna** el valor en la cima. Es el único toque de runtime de M6.
- El **oráculo** cubre genéricos, `Option`/`Result` y `?`, incluido el modo estrés.

### 15.6 Léxico/sintaxis nuevos
- Token **`?`** (operador postfix de propagación).
- `<` y `>` en **posición de tipo** delimitan argumentos/parámetros de tipo (reusan los
  tokens `Lt`/`Gt`).

### 15.7 Sub-fases
- **M6.1 — Funciones genéricas e inferencia desde argumentos**: `Type::Var`, parámetros
  de tipo en funciones, **sustitución** y **unificación desde los argumentos** (`subst`,
  `unify`). Cubre genéricos sobre `T`, `[T]`, `fn(T)->U`. Sin tipos genéricos del
  usuario ni tipo esperado todavía: si un parámetro de tipo no lo fijan los argumentos,
  es error.
- **M6.2 — Tipos genéricos (enums/structs) y chequeo bidireccional**: argumentos de
  tipo en `Struct`/`Enum`, construcción/campo/`match` con sustitución e inferencia, y el
  **tipo esperado** (bidireccional) que fija los parámetros que los argumentos no
  determinan (`Caja.Vacia`, `[]`) —arregla la aspereza del `[]` vacío de paso—.
- **M6.3 — `Option`/`Result` y `?`**: prelude autoinyectado y el operador `?` (Result y
  Option), validado contra el retorno y ejecutado nativamente reusando `return`.

### 15.8 Deferido
- **Inferencia de locales** (`let x = 3` sin anotación) → M8.
- **Bounds/traits** sobre genéricos → idea futura (IDEAS.md).
- **`?` definido por el usuario** (un *trait* `Try`) → no; `?` conoce `Result`/`Option`.
