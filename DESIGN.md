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

**M1–M8 están COMPLETOS.** A partir de M9 hay dos ejes que se alternan: *lenguaje* (lo
que raylang expresa) y *tooling/runtime* (lo que lo hace usable y rápido).

| Hito | Contenido | Aprendes | Estado |
|------|-----------|----------|--------|
| **M1** | lexer + parser + checker + **intérprete**; expresiones, primitivos, funciones | pipeline completo, type checking, orientación a expresiones | ✅ |
| **M2** | reescribir backend como **bytecode + VM** (mismo front-end) | diseño de VM, stack frames | ✅ |
| **M3** | structs + arreglos | layout de datos en memoria | ✅ |
| **M4** | closures + **garbage collector** | captura de entorno, GC | ✅ |
| **M5** | **tipos suma (`enum`) + pattern matching (`match`)** | uniones etiquetadas, exhaustividad | ✅ |
| **M6** | **genéricos** → habilita `Option<T>` / `Result<T,E>` + operador `?` | tipos paramétricos, propagación de errores | ✅ |
| **M7** | **UFCS (`.`) + pipelines (`\|>`) + stdlib** (`map`/`filter`/`fold`) | azúcar sobre llamada, resolución de métodos | ✅ |
| **M8** | inferencia local (`let x = 3`), REPL, mejores errores | unificación básica, tooling | ✅ |
| **Limpieza** | reservar `@` (lexer), coma final en arreglos, sincronizar IDEAS | deuda de front-end y de documentación | ✅ |
| **M9** | **traits / interfaces** (estilo Rust) → polimorfismo + *bounds* de genéricos | despacho estático vs. dinámico, abstracción | ✅ (M9.1 trait+impl · M9.2 bounds · M9.2b impls genéricos · M9.3 defectos + trait objects) |
| **M10** | **tooling**: LSP (reusa el checker) + anotaciones (`@test`, `@derive`, `@builtin`) | language servers, metadatos en el AST | ✅ M10.1 (anotaciones) · M10.2 (LSP: diagnósticos, JSON-RPC a mano) · M10.2b (hover/ir-a-definición) |
| **M11** | **módulos + `pub`** + I/O/stdlib (`args`/`input`/`env`/archivos, builtins de string) | sistema de módulos, visibilidad, API de runtime | ✅ M11.1 stdlib de string (`+`/`len`/`to_string`/`trim`/`split`) · M11.2 I/O (`eprint`/`input`/`parse_int`/`read_int`/`env`/`args`/`read_file`/`write_file`) · M11.3 módulos (`import M;`+`M.f`, `from M import a as b`, `pub`) |
| **M12** | **concurrencia** (dirección probable: goroutines + channels) | scheduler, green threads, suspensión | ⏳ |
| **Transversal** | optimización de la VM (incremental, midiendo) y **self-hosting** (capstone) | rendimiento, bootstrapping | ⏳ |

> El detalle y la clasificación de impacto de los hitos M9+ viven en
> [IDEAS.md](IDEAS.md) hasta que cada uno se especifique en su propia sección al
> arrancarlo. Dependencias clave: `@derive`/`@delegate` (M10) necesitan **traits** (M9);
> el self-hosting necesita **módulos + I/O de archivos** (M11).

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

## 16. M7 — UFCS (`.`), pipelines (`|>`) y stdlib

M7 añade **azúcar sobre la llamada de función** —UFCS y pipelines— y la primera
**biblioteca estándar** de verdad. No introduce conceptos nuevos de tipos ni de
runtime: es el hito que **unifica** "método", "pipeline" y "función libre" en un solo
mecanismo (DESIGN §0), y la prueba de que con genéricos ya se puede escribir librería
útil (`map`, `filter`) **en el propio lenguaje**.

La lección central: **UFCS y `|>` son reescrituras del front-end.** Igual que la
construcción de enums (M5) o los genéricos (M6) no llegan al runtime, aquí `s.trim()`
y `x |> f(a)` se **reescriben a llamadas ordinarias** antes de ejecutar. El intérprete
y la VM no aprenden nada nuevo.

### 16.0 Decisiones de diseño (cerradas)

1. **UFCS: campo primero, luego función libre.** En `recv.nombre(args)`, si el tipo de
   `recv` es un struct con un **campo** `nombre`, es acceso a campo (y se llama si el
   campo es una función): `(recv.nombre)(args)`. Si **no** hay tal campo, se reescribe
   a la **función libre** `nombre(recv, args)` (UFCS). El campo gana en caso de colisión;
   la regla es total y sin ambigüedad. Es una resolución **del checker** (necesita el
   tipo de `recv`), análoga a `resolve_enum_construction` de M5.
2. **`|>` inserta como primer argumento.** `x |> f(a, b)` ≡ `f(x, a, b)`; `x |> f` ≡
   `f(x)`. Coherente con UFCS (el receptor es el **primer** parámetro), así que
   `xs.map(f)` y `xs |> map(f)` significan lo mismo. Sin *placeholder* `_`. Es una
   reescritura **del parser** (puramente sintáctica, no depende de tipos).
3. **stdlib mixta: builtins + prelude en raylang.** Lo que necesita el runtime
   (mutar `[T]`, manipular `string`) son **builtins** nativos (`push`, `split`, `trim`,
   …); lo de **orden superior** (`map`, `filter`, `fold`) se escribe **en raylang** en
   el prelude, reusando genéricos. Demuestra ambas técnicas y que la maquinaria de M6
   basta para una librería real.

Forzado por las decisiones (no abierto):
- **UFCS solo en posición de llamada.** `recv.f(args)` puede ser UFCS; `recv.f` a secas
  es **solo** acceso a campo (no hay "método como valor" sin llamarlo). Mantiene la
  regla total y evita el currying implícito.
- **Orden de resolución de `X.nombre(args)`** en el checker: (1) construcción de enum
  (`Color.Rojo`, ya en M5); (2) campo de struct; (3) UFCS a función libre; (4) error.

### 16.1 Sintaxis nueva

```rust
// UFCS: recv.f(args) ≡ f(recv, args)  (si f no es campo de recv)
let s: string = "  hola  ";
let limpio: string = s.trim();              // ≡ trim(s)
let n: int = nums.len();                    // ≡ len(nums)
let r: [int] = nums.map(doble).filter(par); // encadena: filter(map(nums, doble), par)

// Pipeline: x |> f(args) ≡ f(x, args)
let total: int =
    nums
    |> map(doble)
    |> filter(par)
    |> fold(0, suma);

// Coexisten con el acceso a campo (campo gana):
struct Caja { valor: int }
let c: Caja = Caja { valor: 10 };
let v: int = c.valor;        // campo (no UFCS)
let d: int = c.doble();      // no hay campo 'doble' -> doble(c)  [UFCS]
```

- El **`.`** ya se parsea para acceso a campo (M3); UFCS **no cambia el parser**: el
  checker reescribe el nodo cuando `recv.nombre(args)` no es un campo.
- El **`|>`** es un operador binario nuevo, de **precedencia mínima** y asociativo a la
  izquierda (`a |> f |> g` ≡ `g(f(a))`). El parser lo **desazucara** directamente a la
  llamada con el receptor como primer argumento.

### 16.2 UFCS — resolución de método (checker)

`recv.nombre(args)` llega del parser como `Call(Field(recv, nombre), args)`. El checker:

1. **¿Construcción de enum?** Si `recv` nombra un enum y `nombre` una variante, ya lo
   resuelve `resolve_enum_construction` (M5) → `EnumLit`. No es UFCS.
2. **¿Campo?** Se tipa `recv`; si es `Struct S` y `S` tiene campo `nombre`, es acceso a
   campo. Se mantiene `Call(Field(recv, nombre), args)`: se llama al **valor del campo**
   (que debe ser de tipo función).
3. **UFCS.** Si no es campo, se busca una **función libre** `nombre`. Se **reescribe** el
   nodo a `Call(Ident(nombre), [recv, ...args])` y se tipa como una llamada normal (con
   inferencia de genéricos de M6 incluida). El receptor pasa a ser el primer argumento.
4. Si no hay ni campo ni función, **error con posición**: *"no existe campo ni función
   'nombre' aplicable a `T`"*.

Un **`recv.nombre` sin llamada** (no en posición de `Call`) sigue siendo solo acceso a
campo: si `nombre` no es campo, es error (no hay UFCS sin paréntesis). La reescritura
es **bottom-up**, así que las cadenas `a.f().g()` se resuelven receptor a receptor.

> Como en M5/M6, la reescritura toma `&mut` del AST: el nodo UFCS se sustituye por una
> llamada ordinaria, de modo que **intérprete y VM ven solo llamadas**. Cero runtime.

### 16.3 Pipelines (`|>`) — desugaring (parser)

`|>` es puramente sintáctico y **no depende de tipos**, así que se resuelve en el
parser (a diferencia de UFCS). En la jerarquía de precedencia ocupa el nivel **más
bajo**:

```
pipeline   = igualdad ( "|>" llamada )* ;     // asociativo a la izquierda
```

Para cada `izq |> rhs`:
- si `rhs` es una llamada `f(a, b)` → se emite `f(izq, a, b)` (receptor primero);
- si `rhs` es una expresión llamable `f` (sin paréntesis) → se emite `f(izq)`.

No hay nodo de AST nuevo: el parser construye directamente el `Call`. Por eso `|>`
compone con UFCS y con genéricos sin esfuerzo —al checker le llegan llamadas normales—.

### 16.4 stdlib inicial

El corazón de la stdlib de M7.3 es **orden superior escrito en el propio raylang**, en
el prelude. No toca el runtime: se apoya en los builtins que ya existían (`len`, `push`),
los genéricos (M6) y los closures (M4).

**Prelude en raylang** (`src/prelude.rs`, junto a `Option`/`Result`):

```rust
fn map<T, U>(xs: [T], f: fn(T) -> U) -> [U] {
    var out: [U] = [];
    var i: int = 0;
    while (i < len(xs)) { push(out, f(xs[i])); i = i + 1; }
    out
}
fn filter<T>(xs: [T], pred: fn(T) -> bool) -> [T] { /* análogo, con if */ }
fn fold<T, A>(xs: [T], init: A, f: fn(A, T) -> A) -> A { /* acumula desde init */ }
```

Se **inyectan** como el prelude de M6: en `check()`, las funciones del prelude se
anteponen a las del usuario (saltando las que el usuario ya definió con ese nombre, lo
que da **override** e idempotencia). Quedan en el AST que también compilan el intérprete
y la VM. Son la demostración de que M6 + M4 bastaban para escribir librería real, y
**lucen** con UFCS (`xs.map(f).filter(g)`) y pipelines (`xs |> map(f) |> filter(g)`).

**Builtins que ya existen** y cuentan como stdlib: `len(xs)` (`[T] -> int`),
`push(xs, x)` (`([T], T) -> unit`, muta), `print`.

**Builtins de string** (`trim`, `split`, `to_string`, `concat`/`+`): se **difieren**
(ver §16.8). A diferencia del prelude, cada uno necesita un **opcode nuevo** en la VM
(los builtins de la VM son opcodes dedicados: `Print`/`Len`/`Push`) más su manejo en el
intérprete y su tipado en el checker; `split` además aloja un `[string]` en el heap. Es
trabajo de runtime independiente del azúcar de M7, y se aborda como una expansión futura
de la stdlib.

### 16.5 Runtime

- **UFCS, `|>` y el prelude**: **no tocan el runtime**. Tras la reescritura (checker /
  parser) solo quedan llamadas ordinarias; `map`/`filter`/`fold` son funciones raylang
  normales. M7, tal como se entregó, es **front-end puro**: cero opcodes nuevos.
- (Si en el futuro se añaden builtins de string, *ahí* sí habría toque de runtime —un
  opcode por builtin—; ver §16.4 y §16.8.)
- El **oráculo** (intérprete ↔ VM) cubre UFCS, pipelines y la stdlib, incl. modo estrés
  del GC (los arreglos que crea `map`/`filter` viven en el heap).

### 16.6 Léxico/sintaxis nuevos
- Token **`|>`** (pipeline). El `.` y los paréntesis de llamada ya existen.
- Regla de precedencia nueva para `|>` (la más baja, asociativa a la izquierda).

### 16.7 Sub-fases
- **M7.1 — UFCS (`.`)**: resolución de método en el checker (campo primero, luego
  función libre) con reescritura a llamada normal. Testeable con funciones libres y
  structs existentes; sin runtime nuevo.
- **M7.2 — Pipelines (`|>`)**: token, precedencia y *desugaring* en el parser (receptor
  como primer argumento). Compone con UFCS y genéricos.
- **M7.3 — stdlib**: prelude de orden superior (`map`, `filter`, `fold`) escrito en
  raylang e inyectado. Front-end puro (reusa `len`/`push` + genéricos + closures). Cierra
  M7 dejando el lenguaje "usable" y demostrando "la librería es raylang". Los builtins de
  string se difieren (§16.4, §16.8).

### 16.8 Deferido
- **Builtins de string** (`trim`, `split`, `to_string`, `concat`) → expansión futura de
  la stdlib: requieren un opcode nuevo por builtin en la VM (más intérprete y checker);
  es trabajo de runtime, ortogonal al azúcar de M7.
- **Métodos como valor** (`recv.f` sin llamar) / *method references* → idea futura.
- **`|>` con placeholder** (`x |> f(a, _, b)`) → no en M7; primer argumento basta.
- **UFCS sobre primitivos definido por el usuario con resolución por módulos/imports**
  → no hay módulos aún; toda función libre en ámbito es candidata.
- **Operador `.` encadenado con `?`** (`x.f()?`) ya funciona por composición; sin azúcar
  extra.

## 17. M8 — Inferencia local, REPL y mejores errores

M8 mira hacia la **comodidad de quien escribe** raylang, no hacia el sistema de tipos.
Son tres mejoras de *tooling* y ergonomía que cierran la hoja de ruta: inferir el tipo
de las variables locales, un REPL interactivo, y mensajes de error más útiles.

### 17.0 Decisiones de diseño (cerradas)

1. **Inferencia solo de locales.** La anotación de `let`/`var` se vuelve **opcional**: si
   falta, el tipo de la variable se **infiere del inicializador**. Los **parámetros y el
   retorno** de las funciones siguen siendo **obligatorios** (decisión fundacional §0:
   firmas explícitas, sin inferencia global). Esto mantiene el checker simple —cada firma
   da su tipo sin resolver un sistema de ecuaciones— y conserva la documentación que una
   firma anotada aporta.
2. **Empezar por la inferencia** (M8.1), luego REPL (M8.2) y mejores errores (M8.3): la
   inferencia es lo fundacional y reusa la maquinaria de M6; el REPL y los errores se
   apoyan en un lenguaje ya completo.

### 17.1 Inferencia local (`let x = 3`)

```rust
let x = 3;                       // infiere int
let nombre = "ana";              // infiere string
let xs = [1, 2, 3];              // infiere [int]
let p = Punto { x: 1, y: 2 };    // infiere Punto
let c = Caja.Llena(7);           // infiere Caja<int> (de los genéricos de M6)
var total = 0;                   // var también; sigue siendo mutable
```

**AST**: el campo `ty` de `StmtKind::Let` pasa de `Type` a `Option<Type>` —`None` cuando
no hubo anotación—.

**Parser**: la anotación `: tipo` tras el nombre se vuelve **opcional**. `let x = e;`
(sin `:`) es ahora válido; `let x: T = e;` sigue igual. Desaparece el error "el tipo es
obligatorio".

**Checker** (`StmtKind::Let`):
- **Con anotación** (`Some(T)`): como hasta ahora —el tipo declarado es el **esperado**
  del inicializador (chequeo bidireccional de M6.2), se valida la igualdad, y la variable
  se declara con `T`—.
- **Sin anotación** (`None`): se **infiere** tipando el inicializador sin tipo esperado
  (`check_expr`), y la variable se declara con el tipo resultante.

**Lo que no se puede inferir.** Algunos inicializadores no determinan su tipo por sí
solos —el arreglo vacío `[]`, `Option.None`, `Caja.Vacia`—: necesitan el tipo esperado
que solo daba la anotación. Sin ella, `check_expr` ya falla con un mensaje que **pide la
anotación** ("no se puede inferir el tipo de [] aquí; anótalo…"). La regla es simple: se
infiere lo que el valor determina; lo que no, se anota. La inferencia es **local** (de la
expresión a la izquierda), nunca de uso posterior.

**Runtime**: **sin cambios**. Los tipos se borran antes de ejecutar (como los genéricos);
una variable inferida es, en runtime, una variable como cualquier otra. La inferencia es,
una vez más, **solo del checker**.

> **Por qué no rompe §0.** El norte pedía "un type checker real sin el coste de la
> inferencia global". Inferir `let x = 3` es inferencia **local y trivial** —el tipo está
> *ahí mismo*, en el inicializador—; no hay variables de tipo que propagar entre
> sentencias ni un sistema que resolver. Las firmas siguen explícitas, que es donde la
> anotación documenta y ancla la inferencia. Es la línea exacta que separa "comodidad
> barata" de "inferencia global".

### 17.2 REPL (M8.2)

Un *read-eval-print loop*: leer una sentencia/expresión, verificarla, ejecutarla y mostrar
el resultado, manteniendo el estado entre líneas.

**Decisión clave: un cliente 100% externo.** El REPL (`src/repl.rs`) usa **solo la API
pública** —`lexer::lex`, `parser::parse`, `checker::check`, `interpreter::run` y el builtin
`print`—; **no añade ni una línea al checker ni al intérprete**. El precio de esa pureza es
que muestra el **valor**, no el tipo: obtener el tipo estático exigiría una API del checker
(que devuelva el tipo de una expresión), y se prefirió no incrustar lógica de REPL en el
core por una comodidad de presentación.

**Estrategia: re-ejecutar el preámbulo.** No hay entorno vivo entre líneas. El REPL
**acumula** las definiciones (`fn`/`struct`/`enum`) y las sentencias (`let`/`var`/asignación)
y, en cada entrada, **reconstruye un programa completo** y lo verifica y ejecuta:

```text
  <definiciones acumuladas>
  fn main() { <sentencias acumuladas>  print(<entrada>); }
```

El valor se imprime haciendo que el `main` sintetizado llame a `print` sobre la entrada. Si
la entrada es de tipo `unit` (p. ej. `print(x)` o un `while`), `print(...)` no tiparía: se
reintenta ejecutándola como sentencia, sin envolver. El estado "vive" en el historial de
fuente que se re-ejecuta (coste: recomputar el historial; aceptable). Redefinir un nombre se
resuelve solo: la última `let` gana al re-ejecutar (shadowing), y una definición homónima
reemplaza a la anterior. Una entrada que no verifica/ejecuta se **descarta**.

> **Por qué externo y no integrado.** Una primera versión integró dos ganchos en el core
> (`check_repl` para devolver el tipo de la cola; `run_named` para ejecutar una función
> que no fuera `main`). Funcionaba y mostraba `valor : tipo`, pero metía conceptos de REPL
> en el checker y el intérprete. Revertirlos a favor de un cliente externo deja el core
> intacto y demuestra que el front-end ya expone lo suficiente para construir herramientas
> encima. La lección: una herramienta de *tooling* no debería ensuciar el lenguaje.

### 17.3 Mejores errores (M8.3)

Los errores ahora se muestran con **contexto de fuente**: la línea y un `^` bajo la
posición.

```text
error de tipos en 2:13: el operador '+' requiere ambos operandos int o ambos float
  2 |     let x = 1 + true;
    |             ^
```

**Decisiones (cerradas):** un solo `^` en `(línea, columna)` —que todo token/nodo/error
ya lleva, así que **no hizo falta añadir spans** al AST ni a los errores— y **texto
plano** (sin ANSI: portable y fácil de testear).

**Solo presentación.** Un módulo nuevo `src/diagnostic.rs` con una función `render(source,
line, col, headline)` que antepone la cabecera del error (su `Display` de siempre) y le
añade la línea de fuente y el cursor. No toca el lexer, el parser, el checker ni el
intérprete: cada fase sigue reportando `(línea, columna)`; el renderizador dibuja el
contexto. Lo usan el runner de archivos (`main.rs`, en las cuatro fases: léxico, sintaxis,
tipos, ejecución) y el REPL (que renderiza contra su fuente sintetizada: el `^` apunta al
token ofensor; el número de línea es el de esa fuente, una limitación conocida).

> Spans (`^^^^` sobre el token/expresión entero) y color quedan como mejora futura:
> exigirían añadir rangos a tokens/nodos/errores, un cambio que cruza todo el front-end.

### 17.4 Sub-fases
- **M8.1 — Inferencia local**: `ty` opcional en el AST, anotación opcional en el parser,
  inferencia desde el inicializador en el checker. Solo locales; firmas intactas.
- **M8.2 — REPL**: bucle interactivo, **cliente externo** del front-end + intérprete
  (cero cambios al core); muestra el valor vía `print`.
- **M8.3 — Mejores errores**: contexto de fuente (línea + `^`) en los diagnósticos.
  Módulo `diagnostic`, solo presentación; cero spans (reusa `(línea, col)`).

### 17.5 Deferido
- **Inferencia de retornos/parámetros** → no; §0 fija firmas explícitas.
- **Inferencia con flujo** (deducir el `T` de `[]` por un `push` posterior) → no; la
  inferencia es local al inicializador.

## 18. M9 — Traits (interfaces + comportamiento)

M9 es el salto de **polimorfismo** de raylang. Hasta aquí los structs son **datos** y las
funciones (más UFCS) dan los "métodos", pero no hay forma de programar **contra una
abstracción**: de decir "cualquier tipo que sepa *mostrarse*" y escribir código que sirva
para todos. Eso es un **trait** (la *interfaz* / *typeclass* de otros lenguajes). La
decisión de fondo —fijada desde IDEAS §4— es **structs (datos) + traits estilo Rust
(comportamiento)**, no clases con herencia: composición sobre herencia, despacho estático
por defecto, e integración limpia con UFCS y genéricos.

### 18.0 Decisiones de diseño (cerradas)

1. **Traits estilo Rust, no clases.** Un `trait` declara **firmas** de métodos; un bloque
   `impl Trait for Tipo` las **implementa** para un tipo concreto. El dato (struct/enum) y
   el comportamiento (impl) viven **separados**: un mismo tipo puede implementar varios
   traits, y un trait puede implementarse para tipos que no controlas. Sin herencia.
2. **Despacho estático primero (M9.1).** En `recv.metodo(args)` con `recv` de tipo
   **concreto conocido**, el checker resuelve el `impl` en tiempo de chequeo y **reescribe**
   la llamada a una función ordinaria —igual que UFCS (§16)—. Es **front-end puro**: el
   runtime no cambia (erasure, como los genéricos). El despacho **dinámico** (*trait
   objects*) se difiere a M9.3.
3. **`self` como receptor.** El primer parámetro de un método es `self`, sin anotación: su
   tipo es el tipo implementador. En las firmas, el tipo **`Self`** denota ese mismo tipo
   (p. ej. `fn duplicar(self) -> Self`). Es la única forma nueva de "tipo".
4. **Sub-fases.** M9.1 trait + impl + despacho estático concreto; **M9.2** *bounds* de
   genéricos (`T: Trait`); **M9.3** métodos por defecto y trait objects. Una a una.

### 18.1 Sintaxis (M9.1)

```rust
trait Mostrable {
    fn mostrar(self) -> string;       // firma: cuerpo ausente, termina en ';'
}

struct Punto { x: int, y: int }

impl Mostrable for Punto {
    fn mostrar(self) -> string {       // 'self' es el Punto receptor
        "punto"
    }
}

fn main() -> int {
    let p = Punto { x: 1, y: 2 };
    print(p.mostrar());                // UFCS: resuelve al método del impl
    0
}
```

- Un `trait` lista **firmas** (`fn nombre(self, ...) -> R;`). El `self` es siempre el
  primer parámetro; puede haber más parámetros normales tras él.
- `impl Trait for Tipo { ... }` da el **cuerpo** de cada método. M9.1 exige que el impl
  cubra **exactamente** las firmas del trait (mismos nombres, mismos tipos con
  `self`/`Self` = `Tipo`); ni de menos (falta cobertura) ni con firma distinta.
- `Tipo` puede ser un struct, un enum o un primitivo (`impl Mostrable for int`). M9.1 no
  admite **impls genéricos** (`impl Mostrable for Caja<T>`): se difiere a M9.2.

### 18.2 AST

- **`Type::SelfType`** — el tipo `Self`. Extiende el `Type` (diseñado abierto, §0). Como
  con `Var`/`Enum`, el parser produce `Struct("Self")` para el identificador en posición de
  tipo y el checker lo **reclasifica** (`resolve_type`) a `SelfType` cuando hay un tipo
  implementador en ámbito.
- **`TraitDef { name, methods: Vec<MethodSig>, line, col }`** y
  **`MethodSig { name, params, return_type, line, col }`** — `params` incluye `self` como
  primero (su `ty` es `SelfType`).
- **`ImplBlock { trait_name, target: Type, methods: Vec<Function>, line, col }`** — `target`
  es el tipo implementador (concreto en M9.1).
- `Program` gana `traits: Vec<TraitDef>` e `impls: Vec<ImplBlock>`.

### 18.3 Parser

- Tokens nuevos `trait` e `impl` (palabras clave). `self` se reconoce como **primer
  parámetro especial** en las firmas/métodos: sin `: tipo`, su tipo se fija a `SelfType`.
- `trait` y `impl` son **ítems de nivel superior** (como `fn`/`struct`/`enum`). El cuerpo de
  un `trait` son firmas terminadas en `;`; el de un `impl`, funciones completas.

### 18.4 Checker — el núcleo de M9.1

La idea clave: un método de `impl` **es** una función ordinaria con un primer parámetro
`self` de tipo concreto. Por eso M9.1 no necesita maquinaria de runtime nueva —**reusa**
toda la de funciones—:

1. **Registro.** Se registran los traits (nombre → firmas) y los impls. Cada impl se
   **valida** contra su trait: el tipo destino existe; cubre exactamente las firmas (mismos
   nombres, y al sustituir `Self` = destino, mismos tipos de parámetros y retorno).
2. **Bajada a funciones libres.** Cada método de `impl` se inyecta en `program.functions`
   con un **nombre desambiguado** (*mangling*) `«Tipo#metodo»` (p. ej. `Punto#mostrar`) y su
   `self` convertido en un parámetro concreto `self: Tipo`. Como el usuario no puede
   escribir `#`, no hay colisión de nombres. A partir de aquí, el chequeo de cuerpos, la
   inferencia y el lowering de UFCS **ya existentes** las procesan sin código especial.
3. **Tabla de resolución de métodos.** `(Tipo, metodo) → «Tipo#metodo»`. M9.1 **prohíbe**
   que un mismo tipo reciba dos métodos homónimos de traits distintos (sería ambiguo):
   error en el registro.
4. **Resolución en UFCS.** En `recv.metodo(args)` el orden es: (1) **campo** del struct
   receptor (M3/M4); si no, (2) **método de trait** del tipo concreto del receptor (nuevo);
   si no, (3) **función libre** (UFCS de M7.1). El método de trait se baja igual que UFCS,
   reescribiendo el `Call(Field)` a `Call(Ident("«Tipo#metodo»"), [recv, ...args])`.

Para (4), `ufcs_sites` se generaliza de un conjunto de `(línea, col, nombre)` a un **mapa**
`(línea, col, nombre) → nombre_destino`: para UFCS de función libre el destino es el mismo
nombre; para un método de trait, el nombre *manglado*. Un único `lower_ufcs` baja ambos.

### 18.5 Runtime: sin cambios

M9.1 es **erasure** como los genéricos: traits e impls se borran antes de ejecutar. Lo que
llega al intérprete y a la VM son funciones ordinarias (`Punto#mostrar(self: Punto)`) y
llamadas ordinarias. **Cero opcodes nuevos, cero cambios en los dos motores.** El oráculo
VM↔intérprete sigue valiendo sin tocar `vm.rs`.

> **Por qué encaja tan limpio.** El despacho estático sobre tipos concretos es,
> literalmente, "elige la función correcta en tiempo de chequeo y llámala directo". raylang
> ya hace exactamente eso con UFCS. Un trait añade el **contrato** (qué métodos, qué firmas)
> y la **agrupación** (varios impls para varios tipos), pero el mecanismo de despacho es el
> mismo. El polimorfismo *de verdad* —resolver el método sin conocer el tipo concreto—
> llega con los bounds (M9.2) y los trait objects (M9.3), y es ahí donde el runtime entra
> en juego.

### 18.6 M9.2 — Bounds de genéricos (paso de diccionarios)

Un *bound* acota un parámetro de tipo: `fn imprimir_todo<T: Mostrable>(xs: [T])` permite
llamar `x.mostrar()` dentro del cuerpo, porque `T` **garantiza** implementar `Mostrable`. El
reto es que, con **erasure**, `T` no existe en runtime: hay **un solo cuerpo** compilado
para todos los `T`, así que dentro del genérico no se puede "elegir la función en tiempo de
chequeo". Tres salidas clásicas: paso de diccionarios, monomorfización y despacho por tipo
en runtime (ver historia de la decisión más abajo).

**Decisión (cerrada): paso de diccionarios.** Es la única que conserva las dos invariantes
del proyecto a la vez —*erasure* (una sola copia) y **runtime intacto**— y además **reusa
las funciones de primera clase** que ya existen. La idea: lo que un bound aporta es "saber
cómo llamar los métodos del trait para `T`". Eso es un **diccionario**: el conjunto de
funciones del impl. raylang ya sabe pasar funciones como valores, así que un bound se baja a
**parámetros ocultos de tipo función**, uno por cada método de cada trait acotado.

**Cómo se baja** (todo front-end, *erasure*):

1. **Parámetros ocultos.** Una función con bounds gana, al final de su lista de parámetros,
   un parámetro función por cada `(parámetro de tipo, trait, método)`. Su nombre lleva `#`
   (no colisiona con nombres del usuario): `T#Mostrable#mostrar`. Su tipo es la firma del
   método con `Self → T` (p. ej. `fn(T) -> string`).

   ```rust
   fn imprimir<T: Mostrable>(x: T) { ... x.mostrar() ... }
           │
           ▼  (parámetro oculto añadido en el checker)
   fn imprimir<T>(x: T, «T#Mostrable#mostrar»: fn(T) -> string) { ... }
   ```

2. **Llamada de método sobre `T`.** Dentro del cuerpo, `x.mostrar()` con `x: T` acotado se
   resuelve al **parámetro-diccionario** y se baja como una llamada a ese valor función:
   `x.mostrar()` → `«T#Mostrable#mostrar»(x)`. Reusa exactamente el lowering de UFCS/M9.1
   (el destino registrado es el nombre del diccionario en vez de un `Tipo#metodo`).

3. **Sitio de llamada.** Al llamar `imprimir(p)`, la inferencia ya calcula `σ` (M6); con él
   sabemos a qué tipo resolvió cada parámetro acotado. Por cada `(parámetro, trait, método)`
   del llamado se **añade un argumento** tras los del usuario:
   - si `σ[T]` es un **tipo concreto** `C`: se pasa `«C#metodo»` (el método del impl de
     M9.1). Aquí se **verifica el bound**: si `C` no implementa el trait, error.
   - si `σ[T]` es un **parámetro de tipo del llamador** `U` (rígido): el llamador **debe**
     tener el mismo bound `U: Trait`; se **reenvía** su propio parámetro-diccionario. Esto
     es lo que hace componer a los genéricos acotados entre sí.

   ```rust
   imprimir(p)            // σ: T = Punto  →  imprimir(p, «Punto#mostrar»)
   // dentro de fn g<U: Mostrable>(u: U):
   imprimir(u)            // σ: T = U      →  imprimir(u, «U#Mostrable#mostrar»)  (reenvío)
   ```

El orden de los parámetros y de los argumentos ocultos es el **mismo** (bounds en orden, y
por bound los métodos del trait en orden), así casan posicionalmente.

**Runtime: sin cambios.** Los diccionarios son **valores función** que el intérprete y la
VM ya saben pasar y llamar (M4, primera clase). Cero opcodes, cero cambios en los motores;
el oráculo VM↔intérprete sigue valiendo. El despacho sigue siendo, en esencia, **estático**:
*qué* función concreta viaja en cada diccionario se decide en tiempo de chequeo en el sitio
de llamada; en runtime solo se llama el valor que ya se eligió.

> **Por qué diccionarios y no las otras dos.** *Monomorfización* (una copia del genérico por
> tipo) daría despacho estático puro, pero **rompe la invariante de una-sola-copia** de los
> genéricos y obliga a recolectar todas las instanciaciones del programa. *Despacho por tipo
> en runtime* es el más simple en el front-end, pero es **despacho dinámico de facto** y
> exige tocar **ambos motores** (tabla global de métodos + un mecanismo de llamada por tipo),
> rompiendo el "runtime intacto" que se ha mantenido desde M6. El paso de diccionarios es el
> único que respeta las dos invariantes y, de paso, demuestra que las funciones de primera
> clase de M4 bastan para construir polimorfismo acotado encima, sin magia nueva.

**Alcance de M9.2 y lo diferido a M9.2b / M9.3:**
- ✅ Bounds en **funciones** (`fn f<T: A + B>(...)`), con varios bounds por parámetro.
- ✅ Reenvío de diccionarios entre genéricos acotados.
- ✅ **Impls genéricos** (`impl<T> Trait for Caja<T>`) → **M9.2b** (§18.6b).
- ⏳ Bounds en parámetros de tipo de **struct/enum** → más adelante.

### 18.6b M9.2b — Impls genéricos (diccionarios anidados)

Hasta M9.2 un `impl` solo cubre un tipo **concreto** (`impl Mostrable for Punto`). M9.2b habilita
implementar un trait para un **constructor de tipos** —toda una familia `Caja<T>`—, opcionalmente
**condicionado** a que `T` también cumpla un trait:

```rust
impl<T> Contar for Caja<T> { fn contar(self) -> int { 1 } }      // para cualquier T
impl<T: Mostrable> Mostrable for Caja<T> {                       // si T es Mostrable
    fn mostrar(self) -> string { self.contenido.mostrar() }
}
```

Es lo que vuelve los traits **composicionales** sobre contenedores. La sorpresa de diseño: casi
todo se reduce a maquinaria que ya existe.

**Idea central — un método de impl genérico *es* una función genérica acotada.** En el paso 0c,
cada método de `impl<T: B> Trait for Caja<T>` se baja a una función manglada `Caja#metodo` que
**hereda los `type_params` y `bounds` del impl** (hasta M9.2 se bajaban con ambos vacíos). Con
eso:

- `append_dict_params` le añade sus parámetros-diccionario `T#B#m` (M9.2) **automáticamente**;
- dentro del cuerpo, `self.contenido.metodo()` con `self: Caja<T>` y `T: B` acotado resuelve por
  `resolve_bound_method` al diccionario, **igual que en una función con bounds**;
- el `self` se tipa sustituyendo `Self → Caja<T>` (con `T` como `Var`, no concreto).

**Resolución de instancia.** La clave de la tabla de métodos sigue siendo el **constructor**
(`type_key_of(Caja<T>) = "Caja"`). Por eso `caja.mostrar()` con `caja: Caja<int>` despacha a
`Caja#mostrar`; como ahora es genérica, `check_generic_call` infiere `T=int` y el sitio registra
el diccionario interno que necesita. *Alcance:* solo impls **plenamente genéricos** (los args del
objetivo son exactamente los parámetros de tipo del impl: `Caja<T>`, no `Caja<int>`), **un impl
por `(constructor, trait)`** — sin instancias solapadas ni especializadas (se difieren).

**El punto genuinamente nuevo — diccionarios anidados.** Al pasar un `Caja<int>` a *otro*
genérico acotado, su diccionario ya **no es una función plana**:

```rust
fn imprime<X: Mostrable>(x: X) { ... x.mostrar() ... }
imprime(caja)        // caja: Caja<int>; σ: X = Caja<int>
```

`Caja#mostrar` espera `(self, «int#mostrar»)` —dos argumentos—, pero dentro de `imprime` se le
llamará con uno (`X#Mostrable#mostrar(x)`). La solución es pasar un **closure que captura el
diccionario interno** y presenta la aridad que el llamador espera:

```rust
imprime(caja, fn(c: Caja<int>) -> string { Caja#mostrar(c, int#mostrar) })
//                                          └── el dict interno, capturado
```

Ese closure-que-cierra-sobre-otros-diccionarios **es** el diccionario anidado. Y, una vez más,
**los closures de M4 ya hacen exactamente eso**: cero opcodes, runtime intacto, oráculo válido.

**Cómo se construye.** El argumento-diccionario de un sitio de llamada pasa de ser un *nombre*
(`Vec<String>`) a una *expresión* (`Vec<Expr>`). `dict_for(tipo, trait, método)`:
- `Var(U)` rígido del llamador con el mismo bound → **reenvía** `Ident(U#Trait#m)` (M9.2);
- tipo concreto con impl **no genérico** → `Ident(C#m)` (M9.1, función plana);
- tipo concreto con impl **genérico** (p. ej. `Caja<int>`) → **sintetiza el closure**:
  liga los parámetros del impl al instanciar (`T=int`), y por cada `(bound, método)` del impl
  rellena el argumento interno llamando recursivamente a `dict_for` (`int#mostrar`, que a su vez
  podría ser otro closure si `int` tuviera un impl genérico — la recursión sigue la estructura del
  tipo). El cuerpo del closure llama a `Caja#m(self, params..., dicts_internos...)`.

Todo es **lowering post-check** (los argumentos-diccionario se inyectan tras verificar; el
programa no se re-chequea), así que estas funciones/closure sintéticos no necesitan pasar el
checker: solo importan en runtime, donde *erasure* los reduce a valores función.

**Sub-pasos de implementación:**
- **M9.2b-1** — impls genéricos **sin bounds del impl** (`impl<T> Contar for Caja<T>`, métodos que
  no usan `T`). Solo: parsear `<...>` en el `impl`, llevar `type_params` en el paso 0c, relajar
  `ensure_impl_target`, registrar el impl genérico. Sin closures (la función manglada no tiene
  parámetros-diccionario, así que pasarla plana es correcto).
- **M9.2b-2** — **bounds del impl + diccionarios anidados** (`impl<T: Mostrable> ...`): argumentos
  -diccionario como expresiones y síntesis del closure en `dict_for`.

**Runtime: sin cambios** (igual que M9.2). Bounds en parámetros de struct/enum → **M9.4** (§18.6c);
`dyn Trait` sobre impls genéricos → **M9.4** (§18.6c, vtable vía `dict_for`). Diferido a futuro:
instancias solapadas / especializadas (coherencia/especialización; research-grade).

### 18.6c M9.4 — Bounds en struct/enum y `dyn` sobre impls genéricos

M9.4 cierra dos diferidos de genéricos que comparten una pieza (`satisfies_bound`/`dict_for`).

Hasta aquí solo las **funciones** y los **impls** podían acotar sus parámetros de tipo (`fn f<T: A>`,
`impl<T: A> …`). M9.4 lo extiende a los **tipos del usuario**: `struct Caja<T: Show> { v: T }` y
`enum Lista<T: Eq> { … }`. La sintaxis reusa `type_params_with_bounds` (el mismo parser de los `<…>`
acotados); `StructDef`/`EnumDef` ganan un `bounds: Vec<(String, String)>`.

**Semántica: comprobación en la construcción** (no hay runtime). Un struct/enum es **datos**: el bound
no dispara ningún método, así que no hay diccionarios que pasar —cero *lowering*, cero opcodes—. El
bound es una **promesa que el checker verifica en cada construcción**: al construir `Caja { v: x }`,
tras inferir `T = typeof(x)`, ese tipo debe **satisfacer** el bound (implementar el trait, o ser un
parámetro de tipo del llamador que ya lo declara). La verificación reusa la misma lógica que los
diccionarios de M9.2 (`satisfies_bound`: impl concreto, o `Var` rígido con el mismo bound en ámbito).

Esto da la **propagación gratis**: construir `Caja<U>` dentro de `fn g<U>(…)` exige que `U` lleve el
bound (si no, `U` no lo satisface → error), así que `fn g<U: Show>` es lo único que compila. No se
exige el bound en una función que solo *recibe* un `Caja<U>` sin construirlo (regla más laxa que Rust,
defendible aquí: el `impl<T: Show> Show for Caja<T>` ya reexige `T: Show` al llamar a sus métodos, así
que no hay agujero). `check_bounds` se generaliza para validar también los bounds de struct/enum (cada
uno acota un parámetro real con un trait existente). **Runtime intacto** (erasure total).

**`dyn Trait` sobre impls genéricos.** La realización de M9.3b construye, en la coerción concreto→
objeto, un struct sintetizado (la *vtable*): `data` + un valor función por método. Para un impl
**concreto** ese valor es el método manglado plano (`Punto#m`); para un impl **genérico acotado**
(`impl<T: Show> Show for Caja<T>`) el método manglado lleva parámetros-diccionario ocultos y no se
puede pasar plano: hace falta el mismo **closure anidado** que arma `dict_for`. La solución reusa eso:
la vtable se calcula en el checker con `dict_for(&actual, trait, m)` —que ya elige plano-vs-closure
según el impl— y se guarda en `dyn_coercions`; `lower_dyn` solo la coloca. Así `Caja<int>` (o anidado,
`Caja<Caja<N>>`) coacciona a `dyn Show` sin runtime nuevo. Los closures sintéticos los renumera la
pasada final `renumber_fn_exprs`, como los de M9.2b.

### 18.7 M9.3 — Métodos por defecto y trait objects

M9.3 cierra la historia de polimorfismo con dos piezas de naturaleza opuesta.

#### 18.7a Métodos por defecto (front-end, *erasure*)

Una firma de trait puede traer **cuerpo**: el comportamiento por defecto que un impl hereda
si no lo redefine.

```rust
trait Saludo {
    fn nombre(self) -> string;            // requerido (sin cuerpo)
    fn saludar(self) -> string {          // por defecto: puede usar otros métodos
        self.nombre()
    }
}

impl Saludo for Persona {
    fn nombre(self) -> string { self.n }
    // 'saludar' no se implementa → se usa el cuerpo por defecto
}
```

**AST**: `MethodSig` gana `default_body: Option<Block>` —`Some` para un método por defecto—.

**Parser**: una firma de método termina en `;` (requerido) **o** lleva un bloque (defecto).

**Checker**: es una extensión natural de la bajada de M9.1. Para cada `impl`, un método del
trait que **no** está en el impl pero **tiene** cuerpo por defecto se **sintetiza** como un
método más del impl: su cuerpo es el del defecto, con `Self → Tipo`. Se baja como cualquier
método (función manglada `Tipo#metodo`, inyectada en `program.functions`; entrada en la tabla
de métodos; `current_self = Tipo` al verificar el cuerpo). Así un defecto puede llamar otros
métodos del trait sobre `self` —se resuelven por el tipo concreto como cualquier otro—.

La **cobertura** se relaja: falta un método solo si no está en el impl **y** no tiene
defecto. Un impl que sí lo da **redefine** (gana sobre el defecto). Como todo lo de M9, es
*erasure*: el método sintetizado es una función ordinaria; **runtime intacto**. Compone con
los bounds (M9.2): el método por defecto está en la lista del trait, así que un genérico
`T: Saludo` puede llamarlo y el diccionario recibe la versión sintetizada.

#### 18.7b Trait objects / despacho dinámico (M9.3b)

Un **trait object** es un valor cuyo tipo concreto **no se conoce estáticamente**: una
`[dyn Mostrable]` con `Punto`s y `Color`es mezclados. El método se despacha **en runtime**
según el valor. Es el único punto de M9 donde el despacho deja de ser estático.

**Sintaxis y tipo.** Una palabra clave `dyn` introduce el tipo `dyn Trait` (`Type::Dyn`).
Vale en posición de tipo (parámetros, anotaciones, elementos de arreglo, campos, retorno).

```rust
fn dibujar_todo(xs: [dyn Dibujable]) {
    var i = 0;
    while (i < len(xs)) { xs[i].dibujar(); i = i + 1; }   // despacho por valor
}
```

**Representación (decisión cerrada): un *fat value* `(dato, vtable)`.** El objeto carga su
propia tabla de métodos (la vtable), reusando los diccionarios de M9.2. La realización elegida
—que mantiene el **runtime intacto**, fiel a todo M9— es representar ese fat value como un
**struct sintetizado**: un `dyn Trait` es, en runtime, un struct `«__dyn_Trait»` con un campo
`data` (el valor subyacente) y un campo función por cada método del trait (la vtable). Así se
reusa toda la maquinaria de structs —construcción, acceso a campo, **trazado del GC**— sin una
variante de valor ni opcodes ni cambios en el GC: cero runtime nuevo.

**Coerción concreto → objeto.** Donde un valor concreto `C` (que implementa `Trait`) fluye a
una posición `dyn Trait` (argumento, elemento de arreglo, `let` anotado, retorno), el checker
inserta una coerción que **construye el struct**: `«__dyn_Trait» { data: c, m0: «C#m0», ... }`.
Las funciones de método son las mangladas de M9.1 (incluidos los defectos de M9.3a). La vtable
se fija **en la coerción**, donde el tipo concreto aún se conoce: el despacho es dinámico, pero
*qué* funciones viajan se decide estáticamente.

**Despacho.** `obj.m(args)` con `obj: dyn Trait` baja a llamar el campo-método con el `data`
como receptor: conceptualmente `(obj.m)(obj.data, args)`. Para no evaluar `obj` dos veces, se
baja a un bloque con un temporal: `{ let r = obj; (r.m)(r.data, args) }`. Todo son accesos a
campo y llamadas ordinarias: el intérprete y la VM no saben de trait objects.

**Seguridad de objeto (*object safety*).** Una vtable no puede llevar métodos que dependan del
tipo concreto borrado: si `m` usa `Self` fuera del receptor (p. ej. `-> Self` o `otro: Self`),
**no es invocable** sobre un `dyn Trait` (error en el sitio de llamada). El resto de métodos
—los de firma concreta— sí.

**Runtime: sin cambios.** El trait object es un struct; el despacho, acceso a campo + llamada.
Cero opcodes, cero cambios en los motores ni en el GC; el oráculo VM↔intérprete sigue valiendo.
La lección de M9.3b: incluso el despacho *dinámico* se reduce a "un struct que lleva sus
funciones", sobre las piezas que ya existían (structs + funciones de primera clase).

**Alcance de M9.3b y diferido:**
- ✅ `dyn Trait` como tipo; coerción concreto→objeto; despacho dinámico; arreglos heterogéneos.
- ⏳ `dyn` con métodos que usan `Self` (no *object-safe*) → no invocables sobre el objeto.
- ✅ `dyn A + B` (varios traits en un objeto) y *upcasting* a un subconjunto → **M9.5** (§18.7c).

### 18.7c M9.5 — Trait objects multi-trait (`dyn A + B`) y upcasting

M9.3b realizó `dyn Trait` (un solo trait). M9.5 lo generaliza a **varios traits** en un objeto y al
**upcasting** entre conjuntos. Sigue siendo *erasure*: un trait object es un struct sintetizado.

`Type::Dyn` pasa de `String` a **`Vec<String>`** —el conjunto de traits, **canónico** (ordenado y sin
duplicados)— así que `dyn A + B` y `dyn B + A` son el mismo tipo. El parser lee `dyn A + B + …`. El
struct sintetizado de un conjunto tiene `data` + **un campo por método** de la unión de los traits (en
orden canónico: traits ordenados, métodos en orden de declaración). Nombres de método **duplicados**
entre los traits del conjunto son error (no se sabría a cuál despachar).

- **M9.5a — `dyn A + B`**: la coerción concreto→`dyn {A,B}` exige que el tipo implemente **todos** los
  traits; la vtable se arma con `dict_for` por cada método de la unión (reusa M9.4 → vale también con
  impls genéricos). El despacho `obj.m()` busca `m` entre **todos** los traits del conjunto. `lower_dyn`
  genera un struct por **conjunto distinto** que aparezca en una coerción (no uno por trait).
- **M9.5b — upcasting**: coercionar un valor `dyn S1` a `dyn S2` cuando **S2 ⊆ S1** (olvidar traits).
  Se baja a reconstruir el struct menor proyectando los campos del mayor: `{ let r = <obj>; __dyn_S2 {
  data: r.data, m: r.m, … } }` (solo los métodos de S2). Sin supertraits: el upcast es por subconjunto.

**Runtime: sin cambios** (structs + funciones de primera clase). Object safety por método, como M9.3b.

### 18.8 Deferido (más allá de M9.3)
- **Impls genéricos** (`impl Trait for Caja<T>`) → ✅ M9.2b (§18.6b, diccionarios anidados).
- **`dyn Trait` sobre impls genéricos** → ✅ M9.4 (§18.6c): la vtable de la coerción se arma con
  `dict_for`, así un `Caja<int>` (impl genérico acotado) coacciona a `dyn Trait` con un closure anidado.
- **Instancias solapadas/especializadas** (`impl Trait for Caja<int>` junto a `Caja<T>`) → futuro
  (coherencia/especialización; research-grade, no se hará a la ligera).
- **Traits con `Self` en posición de argumento** que exija dos receptores del mismo tipo
  (p. ej. `fn igual(self, otro: Self) -> bool`) → soportado por M9.1 (ambos = destino), pero
  sin la garantía de igualdad estructural que daría un trait `Eq` del prelude (futuro).

## 19. M10 — Tooling: anotaciones y LSP

M10 mira hacia las **herramientas** alrededor del lenguaje, no al lenguaje en sí. Dos
piezas independientes: **anotaciones** (`@nombre`, metadatos sobre declaraciones) y un
**Language Server (LSP)** que reusa el checker para dar diagnósticos en vivo a cualquier
editor. Se abordan por separado: **M10.1** anotaciones (front-end puro, sin dependencias),
**M10.2** LSP (su propia spec y decisión de implementación al arrancarla).

### 19.1 M10.1 — Anotaciones

Una **anotación** es un metadato adherido a una declaración: `@nombre` o `@nombre(arg, …)`
antes de una función, struct o enum. La dirección (IDEAS §9) es **conjunto cerrado que el
compilador conoce** —barato, didáctico, sin macros de usuario—. M10.1 implementa la
infraestructura más dos anotaciones: `@test` y `@derive(Eq)`.

**Sintaxis y AST.** `@` ya está reservado (`TokenKind::At`). Una declaración puede llevar
cero o más anotaciones; los argumentos (entre paréntesis) son **identificadores**.

```rust
@test
fn suma_ok() -> bool { 1 + 1 == 2 }

@derive(Eq)
enum Color { Rojo, Verde, Azul }
```

- `Annotation { name, args: Vec<String>, line, col }`.
- `Function`, `StructDef` y `EnumDef` ganan `annotations: Vec<Annotation>`.
- **Parser**: en el bucle de nivel superior se recogen las anotaciones que preceden a un
  ítem y se adjuntan. Anotar un `trait`/`impl` es error en M10.1.
- **Checker**: valida que cada anotación sea **conocida** y esté bien colocada (nombre del
  conjunto cerrado; `@test` solo en funciones, `@derive` solo en struct/enum). Una
  anotación desconocida es error.

**`@test`** — marca una función de prueba. La firma debe ser `() -> bool` (pasa si devuelve
`true`). No cambia la ejecución normal (es una función más, ignorada salvo en modo test). El
**runner** (`raylang prog.ray --test`) es un **cliente** (como el REPL): lee las funciones
`@test` del AST, sintetiza un `main` que las llama e imprime `ok`/`FALLO` por cada una, y
ejecuta. **Cero cambios** en checker/intérprete por el runner (solo la validación de firma).

**`@derive(Eq)`** — sobre un struct/enum **no genérico**, genera su `impl Eq`. Es el "pago"
de M9: una anotación que **genera código** sobre traits. Mecánica:
- El prelude aporta `trait Eq { fn igual(self, otro: Self) -> bool; }` (inyectado como los
  enums/funciones del prelude; se salta si el usuario define `Eq`).
- Por cada tipo con `@derive(Eq)`, el checker **sintetiza un `ImplBlock`** `impl Eq for T`
  con el método `igual`, y lo añade a `program.impls`. **El resto lo hace M9** (la bajada de
  M9.1 lo convierte en `T#igual`, etc.): `@derive` solo *genera el AST del impl*.
- El cuerpo de `igual`:
  - **struct**: conjunción de los campos, `self.f1 == otro.f1 && … && self.fn == otro.fn`
    (struct sin campos → `true`).
  - **enum**: `match` sobre `self`; por cada variante, `match` sobre `otro`: misma variante
    → comparar el payload posición a posición con `==` (variante *unit* → `true`); otra
    variante → `false`.
- Las comparaciones hoja usan `==`, así que los campos/payload deben ser **comparables**
  (primitivos, string, bool, struct, arreglos de esos). Un payload que sea **otro enum** no
  es comparable con `==` (limitación conocida: la derivación recursiva de `Eq` para enums
  anidados se difiere). `@derive(Eq)` sobre un tipo **genérico** también se difiere (M9.1 no
  admite impls genéricos).

> **Por qué `igual` y no `==` para enums.** `==` ya compara structs estructuralmente, pero
> **no** enums (pueden ser recursivos / portar funciones; §M5). `@derive(Eq)` da una
> igualdad **explícita** (`a.igual(b)`) para enums, demostrando codegen sobre traits sin
> tocar la semántica de `==` (sobrecarga de operadores queda fuera de alcance).

**`@derive(Show)`** (limpieza post-M11, L2) — sobre un struct/enum **no genérico**, genera su
`impl Show` con `mostrar(self) -> string` (trait `Show { fn mostrar(self) -> string; }` en el
prelude). Misma mecánica que `@derive(Eq)` (sintetiza el `ImplBlock`, lo baja M9); se generaliza
`generate_derives`/`validate_derive` para ambos traits, y `@derive(Eq, Show)` genera los dos. El
cuerpo de `mostrar` renderiza por tipo de cada campo/payload: primitivos vía `to_string`;
struct/enum vía `mostrar()` recursivo (los anidados deben implementar Show). A diferencia de `Eq`,
**Show sí funciona para enums recursivos** (la recursión está en los datos, no impide `mostrar()`).
Se difieren campos de tipo arreglo/función/etc. (error claro) y los tipos genéricos. Formato:
`Nombre { campo: v, … }` para structs, `Nombre.Variante(v0, …)` para enums.

**Runtime: sin cambios.** Las anotaciones son metadatos del front-end; `@test` lo consume un
cliente externo y `@derive` se reduce a un `impl` que M9 ya sabe bajar. Erasure, una vez más.

### 19.2 M10.2 — LSP (diagnósticos en vivo)

Un **Language Server** (LSP) que, ante cada cambio de documento, corre lexer→parser→checker y
devuelve los errores como `Diagnostic` al editor. Se escribe **una vez** y sirve a todos los
editores (VSCode, Neovim, Helix…) que hablen LSP.

**Decisiones (tomadas al arrancar M10.2):**

- **Transporte: JSON-RPC a mano.** Nada de `lsp-server`/`tower-lsp`/`serde`. Mantiene la
  invariante del proyecto —**cero dependencias de Cargo**— y, fiel al espíritu pedagógico, nos
  hace *ver el protocolo por dentro*: el *framing* (`Content-Length: N\r\n\r\n` + N bytes) y un
  **mini-parser/serializador JSON** propios (`mod json` dentro de `src/lsp.rs`).
- **Alcance: solo diagnósticos.** `initialize` + `textDocument/didOpen`/`didChange`/`didClose`
  → `textDocument/publishDiagnostics`. Sin `hover` ni `definition` (exigirían exponer una API de
  tipos del checker —que evitamos ya en el REPL— y un índice de símbolos). Quedan para un futuro
  M10.2b.

**Realización: cliente externo, cero cambios en el núcleo** (como el REPL —M8.2— y el runner de
`@test` —M10.1—). `src/lsp.rs` usa solo la API pública:

- `analizar(src)` corre `lex` → `parse` → `check` (**solo el front-end**, no ejecuta) y devuelve
  el **primer** error como `(línea, col, mensaje)`, reusando el `Display` de cada fase. Nuestro
  compilador es *fail-fast* (devuelve el primer error), así que se publica **un** diagnóstico por
  documento; reportar *todos* exigiría recolección de errores en cada fase (futuro).
- **Coordenadas:** nuestras fases dan `(línea, col)` **1-basadas**; LSP las quiere **0-basadas**
  (`line`, `character`). El `range` del diagnóstico va de `(línea-1, col-1)` al **fin de esa
  línea** (subrayado visible); si la columna cae fuera, se subraya un solo carácter.
- **Presentación:** el mensaje es el `to_string()` del error (la cabecera), **no** el render de
  M8.3 (línea + `^`): en un editor el subrayado lo dibuja el cliente, no nosotros. Es el mismo
  `(línea, col)` y el mismo mensaje que el terminal, con otra presentación.

**Protocolo mínimo soportado:** `initialize` (responde `capabilities.textDocumentSync = 1`, o
sea *Full sync*), `initialized` (se ignora), `shutdown` (responde `null`), `exit` (termina),
`textDocument/didOpen`/`didChange`/`didClose` (analiza y publica; `didClose` limpia con una
lista vacía). Una petición desconocida con `id` recibe un error JSON-RPC `-32601`.

**Conexión desde editores:** se lanza `raylang --lsp` y se le apunta el cliente LSP del editor a
archivos `.ray`. **Neovim/Helix**: un par de líneas de config, sin npm (demuestra la pureza del
servidor). **Sublime Text 4** (M10.2d): el paquete `editors/sublime/` aporta el coloreado
(`raylang.sublime-syntax`) y se conecta al servidor declarándolo en el paquete **LSP** (solo
config, sin compilar). **VSCode** (M10.2c): la extensión `editors/vscode/` incluye un cliente
(`src/extension.ts` sobre `vscode-languageclient`) que lanza el servidor; eso sí trae deps de
**npm**, pero **del lado del editor** —el binario de raylang sigue sin dependencias—. Ver el
`README.md` de cada carpeta. Solo VSCode necesita compilar un cliente; los demás son config
porque su soporte LSP es externo (paquete/built-in del editor). El binario es el mismo de siempre, con un modo más.

### 19.2b M10.2b — Hover e ir-a-definición

M10.2 da **diagnósticos** corriendo el front-end y traduciendo el primer error. M10.2b añade las
dos features de IDE que faltan: **hover** (el tipo bajo el cursor) e **ir-a-definición** (saltar
del uso a la declaración).

**El cambio de fondo: el checker pasa de *validador* a *consultable*.** Hasta hoy `check`
devuelve `Result<(), TypeError>` y **tira** los tipos que calcula (mentalidad *erasure*). Hover y
definición necesitan que el checker **exponga** lo que sabe: un mapa `(línea, col) → tipo` y otro
`uso → posición de la declaración`. Es justo la "API de tipos" que evitamos en el REPL (M8.2); aquí
se abre, pero **contenida**: un índice que se *recolecta* durante una pasada de chequeo, sin cambiar
la semántica ni el runtime (sigue siendo introspección pura).

**Realización — un `SemanticIndex` recolectado al vuelo:**
- Se factoriza el front-end (`run_frontend`) para poder correrlo con un flag `gather`. Con él, el
  `Checker` puebla, **durante `check_program`** (antes de cualquier *lowering*, así las posiciones
  son las de la fuente original), dos listas: *hovers* `(línea, col, largo, texto)` y *defs*
  `(línea, col, largo, línea_def, col_def)`. Una función pública nueva, `semantic_index(program)`,
  corre esa pasada y devuelve el índice (tolerando errores: un programa a medio escribir aún da
  info parcial). `check` no cambia su firma —sigue sirviendo a main/REPL/runner/VM—.
- **Granularidad: identificadores.** Se registra por cada `Ident` que resuelve (variable, parámetro,
  función, tipo): su **tipo** (para hover) y su **posición de declaración** (para definición). Esto
  exige que `VarInfo` lleve la posición de su `let`/parámetro, y sendos mapas `nombre→posición` para
  funciones y tipos. El *largo* del identificador da el rango que el editor subraya.
- **Colisiones con el prelude.** El prelude se antepone a `program.functions`, y el código de
  usuario se verifica **después**: en un mapa por posición, las entradas del usuario **sobrescriben**
  las del prelude que colisionen. Suficiente en la práctica; los cuerpos sintéticos (defectos
  renumerados, closures de M9.2b) tienen posiciones fuera de rango y no estorban.

**El LSP gana estado y capacidades:**
- Ahora **guarda los documentos** (`didOpen`/`didChange` → texto; `didClose` → lo olvida): una
  petición `hover`/`definition` trae solo `uri` + posición, no el texto. (Los diagnósticos no lo
  necesitaban; por eso M10.2 era *stateless*.)
- `initialize` anuncia `hoverProvider` y `definitionProvider`.
- `textDocument/hover`: construye el índice del documento, busca la entrada cuyo rango contiene el
  cursor y responde el tipo. `textDocument/definition`: igual, responde una `Location` (uri + rango
  de la declaración). Coordenadas 1-basadas (fases) ↔ 0-basadas (LSP), como en los diagnósticos.

**Sub-pasos:** **M10.2b-1** hover (tipos); **M10.2b-2** ir-a-definición (posiciones de declaración).

**Alcance y diferido.** Solo identificadores (no toda sub-expresión); el tipo mostrado es el del
checker (con *erasure*). Ir-a-definición cubre nombres con declaración conocida (locales, parámetros,
funciones, tipos); métodos/UFCS quedan limitados. *Completion*, *find-references*, *rename* y
*signature help* → futuro. **Runtime y semántica intactos**: M10.2b es introspección; no cambia qué
programas son válidos ni qué significan.

### 19.3 Deferido (más allá de M10.1)
- **LSP**: diagnósticos (M10.2 §19.2) + hover/definición (M10.2b §19.2b) + *find-references*/*rename*/
  *completion* (cluster 4) + **M10.2f**: hover/def de **tipos**, **signature help** y **completion por
  ámbito** (firma textual robusta ante el doc a medio escribir; alcance = función, sin spans). Quedan:
  hover/def del **nombre de método** (sin posición propia) y completion por **bloque** anidado.
- `@builtin`/`@extern` (limpiar el *special-casing* de `print`/`len`/`push`), `@deprecated`,
  `@inline`, `@delegate` → anotaciones futuras.
- **Derivación recursiva** (`Eq` de enums con payload-enum) y **derive genérico** → futuro.
- **Anotaciones definidas por el usuario que transforman código** (macros) → capstone.

## 20. M11 — Módulos, I/O y stdlib

M11 conecta el lenguaje con el mundo y lo escala a varios archivos. Tres piezas independientes,
que se abordan por separado: **stdlib de string** (§20.1), **I/O / API de runtime** (§20.2,
*args*/*input*/*env*/archivos) y **módulos + `pub`** (§20.3, la pieza arquitectónica). Cada una
se especifica al arrancarla.

### 20.1 M11.1 — stdlib de string

Hasta M10 los **strings son casi opacos**: se imprimen y se comparan con `==`, pero no se pueden
concatenar, medir ni transformar. M11.1 salda esa deuda (diferida desde M7.3, §16.8) con un puñado
de operaciones foundational. Sin ellas no hay forma de construir mensajes, ni de parsear lo que se
lea en M11.2, ni de avanzar hacia el self-hosting.

**Primer cambio de runtime desde M6.3.** A diferencia de casi todo M7–M10 (front-end / *erasure*),
las operaciones de string **tocan los dos motores**: el intérprete y la VM han de saber operar
sobre el `String` en tiempo de ejecución. Se vuelve, pues, a la disciplina de **oráculo**
(VM↔intérprete, incluido estrés del GC, ya que los strings nuevos son objetos del heap en la VM).

**Cómo se exponen: como builtins** (igual que `print`/`len`/`push`; DESIGN §16.4). El checker los
conoce, valida sus tipos y devuelve el resultado; el compilador emite un **opcode** por builtin y
la VM lo ejecuta. **Bonus de UFCS:** como `recv.f(args)` se reescribe a `f(recv, args)` (§16.1),
definirlos como builtins les da **sintaxis de método gratis** (`s.trim()`, `"a,b".split(",")`),
sin nada extra —solo añadir el nombre a la lista de invocables—.

**Operaciones (dos sub-pasos):**

- **M11.1a — construir:**
  - **Concatenación** `s1 + s2 -> string`. Se **extiende el operador `+`** (no un builtin): el
    checker permite `string + string → string`; el intérprete y la VM extienden su `Add` para
    concatenar (reusa el opcode `Add`, sin uno nuevo).
  - **`len(s) -> int`**: se extiende el builtin/opcode `Len` para aceptar también un string;
    devuelve el número de **caracteres** (Unicode scalar values), consistente con `len` de arreglo.
  - **`to_string(x) -> string`**: convierte un primitivo imprimible (`int`/`float`/`bool`/`string`)
    a su representación textual (la misma que `print`). Opcode nuevo `ToString`.
- **M11.1b — descomponer:**
  - **`trim(s) -> string`**: quita el espacio en blanco de los extremos (Unicode). Opcode `Trim`.
  - **`split(s, sep) -> [string]`**: parte `s` por el separador `sep` (substring no vacío) y
    devuelve los trozos. Opcode `Split`. Construye un arreglo en runtime (objeto del heap).

**Lo que NO incluye M11.1** (diferido): tipo `char` / indexar un string (raylang no tiene `char`);
`parse_int`/`int_of_string` (va con I/O, M11.2); `replace`/`contains`/`to_upper`… (aditivos,
cuando hagan falta). El conjunto mínimo es *concatenar + medir + convertir + recortar + partir*.

### 20.2 M11.2 — I/O y API de runtime

Hoy raylang tiene **un único cable hacia afuera**: `print` (stdout) y el código de salida de `main`.
M11.2 abre el resto —**leer entrada, parsear, escribir a stderr, argumentos, entorno**— para poder
escribir apps de verdad (CLI, interactivas). Es lo que IDEAS §10 fijó como "API de runtime".

**Decisión 1 — `main` sigue sin parámetros** (§0/§11, IDEAS §10). El acceso al exterior se hace por
**builtins**, estilo Go/Python (`args()`, no `main(argc, argv)`): no especializa la firma de `main`
y la capacidad está en *cualquier* función. Encaja con cómo ya funciona `print`.

**Decisión 2 — la I/O falible devuelve `Option`/`Result`** (el norte de diseño "errores como
valores, sin null"). Leer entrada puede llegar a EOF; parsear puede fallar; una variable de entorno
puede no existir. En vez de un valor centinela (`-1`, `""`) o de abortar, esas operaciones devuelven
`Option<T>` (M6.3). Aquí es donde el prelude de M6.3 **paga**: por fin hay productores naturales de
`Option`.

**Decisión 3 — primitivos mínimos + envoltorios en el prelude** (el patrón de M7.3). Construir un
`Option` en la VM exigiría que el runtime conociera el `enum_id`/tags de `Option` (acoplamiento
feo). Se evita: cada operación falible se parte en (a) un **primitivo builtin** que devuelve un
**`[T]`** (vacío = "nada", un elemento = "el valor") —una representación que el runtime ya sabe
construir (como `split`)— y (b) un **envoltorio en el prelude, escrito en raylang**, que lo traduce
a `Option` con `Option.Some(r[0])`/`Option.None` corrientes. Así el intérprete y la VM **siguen sin
saber qué es `Option`**: solo devuelven arreglos; el prelude pone la ergonomía.

```raylang
// en el prelude (raylang), sobre el primitivo __parse_int(s) -> [int]:
fn parse_int(s: string) -> Option<int> {
  let r = __parse_int(s);
  if len(r) == 0 { Option.None } else { Option.Some(r[0]) }
}
```

**Cambio de runtime → oráculo.** Como M11.1, esto toca los dos motores (los primitivos son
opcodes). Lo **determinista** (`parse_int`, `to_string` ida y vuelta) se prueba con el **oráculo**
VM↔intérprete; lo **interactivo** (stdin, stderr, argv, entorno) con **tests de integración por
subproceso** (alimentando stdin / capturando stderr / pasando env y args), como `tests/repl_cli.rs`.

**Operaciones (dos sub-pasos):**

- **M11.2a — salida de error + entrada interactiva:**
  - **`eprint(x)`** — como `print` pero a **stderr**; devuelve unit. Opcode `EPrint`.
  - **`parse_int(s) -> Option<int>`** — primitivo `__parse_int(s) -> [int]` (opcode `ParseInt`) +
    envoltorio del prelude. **Determinista → oráculo.**
  - **`input() -> Option<string>`** — lee una línea de stdin (sin el `\n`); `None` en EOF. Primitivo
    `__read_line() -> [string]` (opcode `ReadLine`) + envoltorio.
  - **`read_int() -> Option<int>`** — azúcar del prelude: `input()` y luego `parse_int` (pura
    composición en raylang, sin primitivo nuevo).
- **M11.2b — entorno + argumentos:**
  - **`env(nombre) -> Option<string>`** — variable de entorno; `None` si no existe. Primitivo
    `__env(s) -> [string]` (opcode `Env`) + envoltorio.
  - **`args() -> [string]`** — argumentos de la línea de comandos del programa (sin el binario ni
    las flags de raylang). Opcode `Args`. El runner (`main.rs`) deja los args del programa en un
    almacén de proceso (`OnceLock`) que ambos motores leen; los clientes sin args (REPL, tests,
    runner de `@test`) ven `[]`. No cambia la firma de `run`.

- **M11.2c — I/O de archivos** (cierra el diferido): leer y escribir archivos, devolviendo
  **`Result`** (el otro productor natural de errores-como-valores; abre la puerta al self-hosting,
  que necesita leer fuentes). El reto era construir `Result` —con *dos* payloads— sin acoplar el
  runtime al enum. Se resuelve **generalizando el truco del `[T]`** a un **arreglo etiquetado**: el
  primitivo devuelve `[string]` cuyo **primer elemento es la etiqueta** (`"ok"`/`"err"`) y el resto
  el payload; el envoltorio del prelude lo traduce a `Result.Ok`/`Result.Err`. El runtime sigue sin
  saber qué es `Result`.
  - **`read_file(path) -> Result<string, string>`** — primitivo `__read_file(path) -> [string]`
    (opcode `ReadFile`): `["ok", contenido]` o `["err", mensaje]`.
  - **`write_file(path, contenido) -> Result<int, string>`** — primitivo `__write_file(path,
    contenido) -> [string]` (opcode `WriteFile`): `["ok"]` o `["err", mensaje]`; el envoltorio da
    `Result.Ok(len(contenido))` (caracteres escritos). `Result<int, …>` evita necesitar un literal
    unit.
  - Determinista (leer un archivo inexistente → `Err`) → **oráculo**; el ida y vuelta real (escribir
    y releer) → **integración por subproceso** (archivos temporales).

**Lo que NO incluye M11.2** (diferido): *append*/borrado/`exists`/listar directorios, *buffering* o
streaming (es lectura/escritura del archivo completo), y *prompting* en `input`. Lo justo para apps
de CLI y para alimentar el camino al self-hosting (leer y escribir fuentes `.ray`).

### 20.3 M11.3 — Módulos y `pub`

Hasta M10 un programa raylang es **un solo archivo**. M11.3 lo escala a varios, con
**encapsulamiento**: cada archivo es un módulo, y `pub` controla qué exporta. Es la pieza
arquitectónica de M11 (y un prerrequisito del self-hosting).

**Decisiones (cerradas):**
- **Módulo = archivo.** El nombre del módulo es el *stem* del archivo: `math.ray` → módulo `math`.
- **Visibilidad `pub` explícita** (IDEAS §6): un ítem de nivel superior (`fn`/`struct`/`enum`/`trait`)
  es **privado a su módulo** salvo que lleve `pub`. (No por mayúscula, estilo Go.)
- **Dos formas de importar, estilo Python:**
  - `import math;` — trae el módulo como **espacio de nombres**; se usa **calificado**:
    `math.doble(21)`. Reusa `.` (no se añade `::`): el checker **desambigua** —si la cabeza es un
    módulo importado, es una ruta; si una variable local la tapa, es campo/UFCS—.
  - `from math import doble [as d] {, otro [as o]};` — trae **funciones `pub`** al **ámbito** del
    módulo (sin calificar), con **renombrado opcional** (`as`) para evitar colisiones. (Cruzar
    *tipos* con `from` está diferido: los tipos ya son globales —ver abajo—.)

**Arquitectura — flatten en el front-end, núcleo intacto.** Igual que UFCS/diccionarios/`dyn`, los
módulos se resuelven **antes** del checker y se borran: el intérprete y la VM nunca saben de
módulos. Tres fases nuevas, todas en el front-end:

1. **Loader** (`src/loader.rs`, *cliente* host-side): desde el archivo de entrada, parsea, lee sus
   `import`/`from`, resuelve cada módulo a `<dir>/<nombre>.ray`, lo lee y parsea, y **recurre** por
   sus imports. Carga **cada módulo una vez** (los ciclos del grafo de imports no son problema: se
   cargan una vez y las referencias cruzadas se resuelven tras fusionar). Produce los módulos
   parseados + su grafo de imports. (Único I/O de archivos de M11.3, y es del *host* en Rust, no un
   builtin de raylang.)
2. **Namespacing + fusión:** los ítems de nivel superior de cada módulo **no-entrada** se renombran
   a nombres globales únicos `modulo::nombre` (el `::` es ilegal en identificadores → no colisiona
   con nombres del usuario, como el `#` del *mangling* de traits). El módulo de **entrada** (el del
   `fn main`) **no** se renombra (sus nombres ya son globales; `main` debe seguir llamándose `main`).
   Todo se fusiona en un único `Program`.
3. **Resolución de referencias** (pasada *scope-aware*): por cada módulo se arma un mapa
   `nombre local → nombre global` —sus propias defs + lo que importó— y se **reescribe** cada
   referencia a un nombre de nivel superior a su global, **respetando los ámbitos locales** (una
   variable/parámetro que tape un nombre global no se reescribe). El acceso calificado `math.item`
   (un `Field`/`Call` cuya cabeza es un módulo importado) se reescribe a `math::item`. **Aquí se
   aplica `pub`:** referenciar un ítem no-`pub` de otro módulo es error. Tras esta pasada el
   `Program` es **plano** (nombres únicos, referencias resueltas) → el checker/intérprete/VM corren
   sin cambios.

**Sub-pasos:**
- **M11.3a** — loader + namespacing + resolución + `pub` + **`import M;`** con llamadas calificadas
  `M.f(...)`. Núcleo, con la superficie mínima: **solo se namespacan funciones** (`modulo::fn`) y
  se cruzan **funciones `pub`**. Los **tipos/enums/traits** siguen en un **espacio global único**
  (deben tener nombre único entre módulos; un choque es error) —cruzar tipos entre módulos es -b/
  diferido—, así el resolutor solo reescribe referencias a *funciones* (no tipos ni patrones).
- **M11.3b** — **`from M import a [as b]{, …}`**: trae **funciones `pub`** al ámbito del módulo, con
  alias. Reusa el loader/namespacing/`pub` de -a; lo único nuevo es **inyectar esos nombres en el
  mapa de resolución** del módulo (con su renombrado) durante la pasada *scope-aware*: una referencia
  a `d` (o a `doble`) se reescribe a `math::doble` salvo que una local la tape. Un `as` resuelve
  colisiones (con una función propia o con otro import). El módulo importado se vuelve dependencia del
  loader igual que con `import M;`. **Importar un tipo** con `from` queda **diferido** (los tipos no
  se namespacan; ya son globales, así que se referencian por su nombre tal cual): si el nombre
  importado no es una función `pub` pero sí un tipo del módulo, el loader lo dice explícitamente.

**Desambiguación de `.`** (el punto delicado): `math.doble(21)` llega como `Call(Field(Ident
"math", "doble"), ...)`, igual que UFCS y que la construcción de enums. La regla, en orden: si hay
una **variable local** `math` en ámbito → campo/UFCS sobre ese valor; si `math` es un **módulo
importado** → ruta calificada `math::doble`; si no, sigue la resolución actual (campo de struct →
método → función libre). Es el mismo estilo de desambiguación por contexto que ya usa el `.`.

**Desambiguación de posiciones entre módulos (L3).** El lowering de M9 (UFCS, diccionarios, `dyn`)
indexa sus tablas por la posición `(línea, col)` del nodo. Sobre el programa **fusionado**, dos
sitios de módulos distintos en la misma `(línea, col)` **colisionarían** (bug real: crash en ambos
motores). El loader lo evita dando a cada módulo una **banda de líneas disjunta**: desplaza todas
las posiciones del módulo por un `delta` (el de entrada en `delta` 0 → un solo archivo es idéntico
a antes; cada módulo siguiente en una banda superior). Así las posiciones son **globalmente únicas**
y, de paso, un error del checker/runtime se **renderiza contra su archivo** con su **línea local**,
prefijado `[módulo]`. La columna y las posiciones *relativas* dentro de un módulo se conservan (de
ellas dependen las pre-pasadas, p. ej. que un `Call` comparta posición con su receptor).

**Tipos por módulo (M11.3c).** Hasta -b los **tipos** (`struct`/`enum`/`trait`) eran globales-únicos
(un choque de nombres entre módulos era error; sin encapsulamiento). M11.3c los pone **por módulo**,
igual que las funciones: los tipos de un módulo no-entrada se **namespacan** a `modulo::Tipo` (el de
entrada no, así un solo archivo no cambia), y `pub` controla su exportación. Dos módulos ya pueden
reusar un nombre (`Node`, `Error`…); un tipo es **privado a su módulo** salvo `pub` + importado.

El reto frente a las funciones: una referencia de tipo aparece en **muchas posiciones** —anotaciones
(params, retorno, `let`), campos de struct, payloads de variante, objetivo/trait de un `impl`,
bounds, `dyn Trait`— **y** en expresiones que nombran tipos: literal de struct (`Punto { … }`),
construcción de enum (`Color.Rojo`, que llega como `Field`), y patrones de `match` (`Color.Rojo(x)`).
El loader las reescribe todas. Complicación clave: el parser emite `Type::Struct(name)` para
**cualquier** identificador en posición de tipo —incluidos los **parámetros de tipo** `T` (que el
checker reclasifica a `Var` luego)—. Por eso el reescritor de tipos es *scope-aware* sobre los
**params de tipo** del `fn`/`impl`/`struct`/`enum` envolvente: un nombre ligado por `<…>` se deja
intacto; un tipo propio del módulo → `modulo::Tipo`; uno importado (`from M import Punto`) → su
global; un primitivo (no es `Struct`) o un desconocido → intacto (el checker lo resuelve/erra). Es la
encapsulación: una referencia *bare* a un tipo de otro módulo no resuelve (hay que importarlo).

**Sub-pasos:**
- **M11.3c-1** — `pub` en tipos (relaja `no_pub`); namespacing de los tipos de módulos no-entrada +
  el **reescritor de referencias de tipo** completo (posiciones de tipo + expresiones que nombran
  tipos), *scope-aware* sobre params de tipo; `comprobar_tipos_unicos` deja de exigir unicidad global
  (cada módulo usa sus propios tipos; dos módulos pueden reusar un nombre).
- **M11.3c-2** — **`from M import Tipo [as T]`** ✅: trae un **tipo `pub`** de otro módulo al ámbito
  (reusa el reescritor de -1 + la plomería de `from` de -b: el nombre local se inyecta en el mapa de
  **tipos** del reescritor, no en el de valores). Cierra el cruce de tipos.

**Alcance y diferido:**
- ✅ -a: `import M;` + `M.f(...)` (funciones `pub`), visibilidad, multi-archivo, ciclos seguros.
- ✅ -b: `from M import a [as b]{, …}` (funciones `pub` al ámbito, con alias).
- ✅ L3: desambiguación de posiciones entre módulos (sin colisiones) + errores atribuidos al módulo.
- ✅ -c: tipos por módulo (`pub` en tipos, namespacing + reescritor) + `from M import Tipo [as T]`.
- ✅ -c-3: **referencias calificadas por módulo** — `M.Punto` en **posición de tipo** (anotación,
  campo, payload, `dyn M.T`, bounds), `M.Punto { … }` (**literal de struct** calificado),
  `M.Color.Rojo[(…)]` (**construcción de enum** calificada) y `M.Color.Rojo` en **patrones**.
- ✅ M11.5: **módulos por directorios** (`import geo/formas/circulo;`, leaf-binding, `as`).
- ✅ M11.6: **cápsula `mod.ray`** — directorio direccionable + reexport (`pub from … import …`, -a) +
  **enforcement** del borde (importar un submódulo interno desde fuera = error, -b).
- ⏳ Imports relativos, `pub` granular (campos) → futuro.

**Referencias calificadas (M11.3c-3).** Cierra el cruce de tipos por la vía calificada (la otra es
`from M import`). El **parser** produce nombres con un `.` interno: `Type::Struct("M.Punto")` (posición
de tipo), `enum_name = "M.Color"` (patrón), nombre `"M.Punto"` del literal de struct; la construcción
de enum `M.Color.Rojo` llega como `Field`/`Call` anidados naturales. El **loader** resuelve `M.X →
M::X` validando que `M` esté **importado** (`import M;`) y `X` sea **`pub`**. Reparto: las posiciones
de **valor** (`M.Color.Rojo`) las colapsa el `Resolver` (consciente de ámbitos: una local `M` tapa al
módulo), extendiendo `qualified_field` para resolver también tipos `pub`; las posiciones de **tipo**
(anotaciones, nombre del literal, `enum_name` del patrón) las resuelve el `TypeRewriter` (un nombre
con `.` → `rewrite_name` lo parte y valida). Una referencia que no resuelve **se deja con el `.`**:
ningún tipo/enum definido lleva `.`, así que el checker la rechaza → **encapsulación** (un tipo
privado o un módulo no importado no se alcanza). Gramática: el literal `M.Tipo { … }` se ancla a que
el receptor del `.` sea un `Ident` (mismo compromiso struct-literal-vs-bloque que `Tipo { … }`).

**Módulos por directorios (M11.5).** Hasta -c todos los módulos viven en una **carpeta plana** (el
loader resuelve `import M;` contra `<dir-de-entrada>/M.ray`). M11.5 los organiza en **jerarquía de
directorios**, con la sintaxis tradicional estilo Unix: `import geo/formas/circulo;` resuelve
`<raíz>/geo/formas/circulo.ray`. La **raíz** del proyecto es el directorio del archivo de entrada;
**todas las rutas son absolutas desde la raíz** (un módulo que importa a un vecino escribe la ruta
completa `import geo/formas/util;`, no `import util;`; los imports **relativos** quedan diferidos).

Decisiones de diseño (cerradas con el usuario):
- **Separador `/`** (no `::` ni `.`). En la **línea de `import` no hay ambigüedad**: ahí no aparece
  ninguna división, así que el parser lee una ruta `IDENT ('/' IDENT)*` sin choque con el operador.
- **Solo leaf-binding**: `import geo/formas/circulo;` liga **el último segmento** (`circulo`) como
  nombre local; el acceso es `circulo.area(...)`, `circulo.Punto` (reusa el `.` de M11.3c tal cual).
  Con `as`: `import geo/formas/circulo as c;`. Una colisión de leaves (dos rutas con el mismo último
  segmento) **pide `as`**, como los `from`-imports.
- **Prohibido el acceso por ruta completa en expresiones** (`geo/formas/circulo.area(x)`): sería
  ambiguo con la división **y** es mala práctica. La ruta vive **solo** en la declaración `import`;
  toda referencia posterior usa el leaf (o su alias). Así nunca hay un `/` fuera de un `import`.

La identidad de un módulo deja de ser su *stem* y pasa a ser su **ruta** (`geo/formas/circulo`). Eso
desacopla los cinco roles que el *stem* cumplía a la vez: **ruta de archivo** (resolución), **prefijo
de namespacing**, **clave de `pub`** y **nombre local**. Implementación (front-end puro, **runtime
intacto**):
- El **parser** lee la ruta y la guarda en `ImportDecl.module` con sus `/` (`"geo/formas/circulo"`);
  añade `alias: Option<String>` (el `as`). El `from M import …` admite la misma ruta en `M`.
- El **loader** namespaca con `::` igual que antes, pero el prefijo es la ruta con los `/` traducidos
  a `::` (`ns_prefix("geo/formas/circulo") = "geo::formas::circulo"`) → nombres globales
  `geo::formas::circulo::area`. El **leaf** (último segmento, o el alias) es el nombre local. Los
  mapas de acceso calificado (`Resolver.imports`, `TypeRewriter.imports`) pasan de un *conjunto* de
  nombres a un **mapa leaf → ruta**: `circulo.area`/`circulo.Tipo` busca el leaf, valida `pub` contra
  la ruta y baja a `geo::formas::circulo::…`. Un solo archivo o una carpeta plana quedan **idénticos**
  (sin `/`, `ns_prefix` es la identidad y el leaf es el propio nombre).
- ⏳ Diferido aún: **imports relativos** (`import ./util;`), `pub` granular (campos).

**Aislamiento de módulos: la cápsula `mod.ray` (M11.6).** Tras M11.5, `pub` es **binario**: un ítem
es privado a su archivo o alcanzable desde **todo el proyecto** por su ruta; no hay un nivel
intermedio "público dentro de `geo/`, invisible fuera", y un directorio **no es direccionable**.
M11.6 cierra ese hueco con un modelo de **cápsula** (estrategia elegida frente a `internal/`-estilo-Go
y al árbol explícito estilo Rust `mod x;`+`pub(crate)`, descartado por duplicar el sistema de
archivos —raylang sostiene "el filesystem **es** la estructura"—).

**La idea**: la **presencia de un `mod.ray`** conmuta un directorio de *transparente* a *cápsula*.
- **Directorio sin `mod.ray`** → transparente (lo de hoy, compat total): sus archivos se alcanzan por
  ruta con `pub`.
- **Directorio `geo/` con `geo/mod.ray`** → cápsula:
  - Se vuelve **direccionable**: `import geo;` carga `geo/mod.ray` (módulo de identidad `geo`,
    prefijo `geo`, ítems `geo::…`). Desde fuera solo se ven sus ítems `pub`.
  - `mod.ray` arma su **cara pública reexportando** de sus submódulos, con una única sintaxis nueva:
    `pub from geo/formas/circulo import Circulo, area;` — un `from`-import marcado `pub` que, además
    de traer los nombres al ámbito de `mod.ray`, los **añade a la superficie pública** de `geo`.
  - Los submódulos internos (`geo/formas/circulo`) quedan **inalcanzables desde fuera**: un
    `import geo/formas/circulo;` externo es **error** (M11.6b). *Dentro* de `geo/`, los submódulos se
    importan entre sí por ruta como hoy.

`pub` conserva su significado (exportar de un archivo); lo nuevo es que **cruzar el borde de una
cápsula obliga a pasar por su `mod.ray`** (Go `paquete`+`internal/`, pero gobernado por un archivo
explícito en vez de un nombre mágico). Sigue siendo **front-end puro**: el loader ya conoce cada ruta
y cada arista de `import`; el aislamiento es una **comprobación de rutas en las aristas** + reusar la
clasificación valor-vs-tipo de M11.3c para el reexport. Runtime intacto.

**Diseño de implementación (dos sub-pasos):**

*M11.6a — directorios direccionables + reexport (fachada, sin enforcement):*
1. **Resolución de módulo**: `resolve_module_path(root, P)` prueba `P.ray`, y si no, `P/mod.ray`
   (error si **ambos** existen → una sola forma canónica, evitando el lío histórico de Rust). La
   identidad del módulo sigue siendo la ruta `P` (prefijo `ns_prefix(P)`). Así `import geo;` resuelve
   `geo/mod.ray` y sus ítems quedan `geo::…`.
2. **AST/Parser**: `FromImport.is_pub: bool`; el parser acepta `pub from … import …;` (lookahead
   `Pub`+`From` en el bucle de `parse`, antes del camino `[anns][pub] item`).
3. **Superficie pública con globals**. Hoy `recolectar_pub_fns/tipos` devuelve `módulo → {nombres}` y
   `qualified_field`/`clasificar_from_name` **recomputan** el global como `ns_prefix(ruta)::nombre`
   —válido solo si el ítem se **define** ahí—. Para reexportar (el ítem se define en *otro* módulo)
   eso no sirve. Refactor: una **`Surface { values: Map<nombre,global>, types: Map<nombre,global> }`
   por módulo**, que **lleva el nombre global de destino**:
   - ítem `pub` **definido** en `m` → `global_fn(prefix(m), nombre)` (lo de hoy);
   - `pub from P import a as b` → el global **resuelto** de `a` en `Surface[P]` (recursivo, con guarda
     de visitados para las cadenas reexport-de-reexport; el caso común —reexportar un `pub` definido—
     es un solo salto). `Surface[m][b] = ese global`.
   `qualified_field`/`clasificar_from_name`/la clasificación de `from` pasan a **consultar `Surface`**
   (en vez de recomputar). Unifica definido-pub y reexport; compat hacia atrás exacta (para un ítem
   definido, `Surface` da el mismo global que antes).

*M11.6b — enforcement de la cápsula* (✅): por cada arista `import` (importador `I` → objetivo `T`,
ya recorridas en el BFS), `capsula_violada(root, I, T)` busca el **ancestro-directorio estricto más
cercano** de `T` con un `mod.ray` (filesystem: itera los prefijos de `T` de profundo a superficial,
`root/C/mod.ray`). Si no hay cápsula → `T` es libre (hoy). Si la hay → se exige `I == C` **o** `I`
bajo `C/`; si no, **error** ("el módulo '`T`' es interno a la cápsula '`C`'; impórtalo con
`import C;`"). Basta la más cercana porque las cápsulas anidadas componen (estar en la interior
implica estar en la exterior). Se comprueba **aunque `T` ya esté visitado** (cada sitio cuenta), y las
aristas de `from … import` igual. Cero coste sin `mod.ray` (ninguna cápsula).

**Compat hacia atrás**: sin ningún `mod.ray`, no hay cápsulas → toda arista es libre y la `Surface`
da los mismos globals → **idéntico a M11.5**.

**Diferido de M11.6**: reexport que *renombra el submódulo entero* (`pub import geo/x as y;`), `pub`
granular por campo, imports relativos. (Detalle abierto a confirmar: la sintaxis del reexport —
`pub from … import …` recomendada vs un `pub use …` separado.)

**Runtime: sin cambios.** Los módulos se borran en el front-end; el programa fusionado es uno solo
con nombres únicos. Oráculo VM↔intérprete intacto.

### 20.4 M11.4 — Cierre de diferidos aditivos de la stdlib

M11.1/M11.2 dejaron explícitamente fuera un puñado de operaciones **aditivas** (no foundational,
"cuando hagan falta"): más string, más I/O de archivos, y el tipo `char` con indexado. M11.4 las
salda. Todas siguen la disciplina ya establecida: **tras el registro único de builtins (L1), añadir
un builtin de runtime es una fila en la tabla `BUILTINS` + un opcode + su impl por motor**, y como
tocan runtime, **vuelven al oráculo** (VM↔intérprete, con estrés del GC para las que asignan heap).

- **M11.4a — más string** (puros, sin I/O):
  - **`contains(s, sub) -> bool`**: ¿`s` contiene la subcadena `sub`? Opcode `Contains`.
  - **`replace(s, de, a) -> string`**: reemplaza **todas** las ocurrencias de `de` por `a`. Opcode
    `Replace`. Asigna un string nuevo (heap en la VM → estrés del GC). Con `de` vacío se respeta la
    semántica de `str::replace` de Rust (oráculo idéntico por construcción).
- **M11.4b — I/O de archivos aditiva** (devuelven `Result`/`bool`, norte "errores como valores"):
  - **`exists(ruta) -> bool`**: ¿existe la ruta? Builtin primitivo (opcode `Exists`), total (no falla).
  - **`append_file(ruta, cont) -> Result<int,string>`**: añade al final (crea si no existe). Reusa el
    **arreglo etiquetado** de M11.2c (`["ok"]`/`["err", msg]`) + envoltorio en el prelude; opcode
    `AppendFile`.
- **M11.4c — tipo `char` + indexado** (el más profundo: un **tipo nuevo**, no solo builtins). Dos
  sub-pasos:
  - **4c-1 — el tipo `char`**: `Type::Char` + keyword `char`; literal `'a'` con escapes (`\n \t \\ \'`)
    en el lexer (`TokenKind::Char`) y parser (`ExprKind::Char`); runtime `Value::Char`/`HeapValue::Char`
    en ambos motores; `print`/`to_string`/`==` (Display = el carácter, sin comillas, como el string).
    Se compone con `@derive(Eq/Show)` (campos `char`) gratis (Eq por `==`, Show por `to_string`).
  - **4c-2 — indexar e iterar**: `s[i] -> char` (extiende `Index` para strings; out-of-bounds = error
    de runtime como en arreglos) y `chars(s) -> [char]` (builtin `Chars`, asigna heap → estrés del GC).
    Cierra el círculo: con `char` + indexado, un string se recorre carácter a carácter.

**Lo que sigue fuera:** `to_upper`/`to_lower`/`starts_with`/`find`, listar directorios, *buffering* —
aditivos ulteriores cuando hagan falta. M11.4 cubre lo que pidieron los diferidos nombrados.

### 20.5 M11.7 — Cierre de la stdlib aditiva (string, arreglos, I/O, sort)

Cierra los diferidos aditivos restantes de la stdlib, elegidos con el usuario. Misma disciplina que
M11.4: **tras L1, cada builtin de runtime = una fila en `BUILTINS` + un opcode + su impl por motor**,
con **oráculo** VM↔intérprete (estrés del GC para los que asignan heap). Las funciones que devuelven
`Option`/`Result` reusan el patrón del **arreglo etiquetado/`[T]` + envoltorio en el prelude** (el
runtime no sabe de `Option`). Los helpers *puros* compartidos por ambos motores viven en `builtins.rs`
(como `append_to_file`), para no duplicar lógica.

**Nota de naming (sin sobrecarga).** raylang no tiene overloading, así que dos funciones no pueden
compartir nombre. Por eso la búsqueda de posición es **`index_of`** para string y **`position`** para
arreglos (idiomático, estilo Rust `Iterator::position`).

- **M11.7a — string**: `starts_with`/`ends_with` (`-> bool`), `to_upper`/`to_lower` (heap),
  `substring(s,i,j)` (índices de **carácter**, *clamp* a rango válido → sin error de runtime),
  `repeat(s,n)` (`n<0` → `""`), `index_of(s,sub) -> Option<int>` (primitivo `__index_of -> [int]` +
  envoltorio) y `join(arr,sep) -> string` (une un `[string]`). Todo por **carácter** (consistente con
  `len`/`chars`/`s[i]`).
- **M11.7b — arreglos**: `pop(a) -> Option<T>` (muta; `__pop`-style), `reverse(a) -> [T]` (heap),
  `contains(a,x) -> bool` e `position(a,x) -> Option<int>` (igualdad de elementos, ad-hoc), y
  concatenación `a + b` de arreglos (se extiende la regla de `+`/`Add`, como `string+string`).
- **M11.7c — I/O**: `remove_file(ruta) -> Result<int,string>` y `list_dir(ruta) ->
  Result<[string],string>` (arreglo etiquetado + envoltorio; no deterministas → integración por
  subproceso, no oráculo).
- **M11.7d — sort**: trait **`Ord`** en el prelude (`fn menor(self, otro: Self) -> bool`), impl para
  `int`/`float`/`string`/`char`, y `sort<T: Ord>(a: [T]) -> [T]` **escrito en raylang** (reusa bounds/
  diccionarios de M9.2 + `len`/`push`/índice): **front-end puro, cero opcodes nuevos**.

### 20.6 M11.8 — I/O con buffering (handles de archivo)

El último diferido grande de I/O: lectura/escritura **con estado** (abrir una vez, leer/escribir por
partes, cerrar). Introduce un **handle de archivo**, pero **sin un nuevo tipo de valor ni tocar el
GC**: el handle es un `int` y los archivos abiertos viven en un **almacén de proceso** del host (un
`Mutex<HashMap<i64, File>>` en `interpreter.rs`, como el almacén de `args`), compartido por ambos
motores. Builtins: `open(ruta, modo) -> Result<int,string>`, `read_line_handle(h) ->
Option<string>`, `write_handle(h, s) -> Result<int,string>`, `close(h) -> int`. No determinista →
integración por subproceso.

## 21. M12 — Concurrencia (dirección propuesta)

Se analizó el "stack ideal" de un lenguaje moderno —*algebraic effects* (concurrencia *colorless*
componible) + *structured concurrency* + ownership/regiones (data-race freedom) + runtime **M:N**
preemptivo + backpressure *demand-driven*—. Como visión es el estado del arte; como M12 de raylang,
**el stack completo no encaja** sin abandonar tres pilares del proyecto: el **oráculo de dos motores**,
el **GC mono-hilo** y el modelo **GC'd con mutabilidad compartida**. La decisión es tomar las ideas
**probadas que encajan** y diferir/descartar las research-grade, con razones.

**Tensiones de fondo** (el *por qué*, no solo el *qué*):
1. **Continuaciones vs. el intérprete.** Effects/async-colorless necesitan capturar y reanudar la
   pila (continuaciones delimitadas). El **intérprete tree-walking recurre sobre la pila de Rust** →
   no puede suspenderse sin reescribirse a una máquina de pila explícita (CPS). La **VM sí** puede
   (tiene `frames`). ⇒ **la concurrencia vive en la VM; el intérprete queda como oráculo secuencial**.
2. **No-determinismo vs. el oráculo.** Los interleavings rompen la comparación de salidas exactas. ⇒
   **scheduler determinista** (mismo orden de planificación) y tests con interleaving fijado.
3. **Paralelismo vs. GC mono-hilo.** M:N *paralelo* exige GC thread-safe y valores `Send` (hoy el
   intérprete usa `Rc`, `!Send`; el heap de la VM es mono-hilo). ⇒ **scheduler cooperativo M:1** (un
   hilo): concurrencia, no paralelismo. Con un solo hilo **no hay carreras de memoria por
   construcción** → no hacen falta ni regiones ni GC concurrente.

**Dirección propuesta — CSP/actores sobre la VM:** `spawn` de *green threads* + **canales tipados**
(`send`/`recv`), con **structured concurrency** (un *scope* que posee y hace *join* de sus tareas) y
**canales acotados** para el backpressure. Scheduler **cooperativo M:1** (fibra = pila de `frames`
guardada en la VM; *yield* en puntos definidos: `recv`/`send`/`yield`). La **data-race freedom viene
de la disciplina CSP** —*comparte comunicando*, el canal es el único punto de paso—, no de ownership.
Es el modelo Go/Erlang: el grueso del valor pedagógico ("concurrencia segura") sin el coste de
ownership + GC concurrente.

**Veredicto:**
- ✅ **Adoptar**: green threads cooperativos (M:1) en la VM · canales tipados · structured concurrency
  · backpressure (canales acotados) · data-race freedom **vía CSP**.
- 🔬 **Diferir** (puertas abiertas): **algebraic effects** (requiere reescribir el intérprete a pila
  explícita) · **M:N paralelo preemptivo** (requiere GC thread-safe + valores `Send`).
- ❌ **Descartar para raylang**: **ownership/regiones** (contradice el modelo GC'd con mutabilidad
  compartida; "sería otro lenguaje").

### 21.1 raylang de producción (rama futura)

Las cosas descartadas/diferidas arriba no son "malas": son **las correctas para un lenguaje nuevo de
producción**, y chocan solo porque raylang **eligió** ser pedagógico (dos motores, GC mono-hilo,
mutabilidad compartida, cero dependencias). Queda anotada la idea de, **en otra rama**, dejar de ser
pedagógico y perseguir producción: un único motor (la VM, jubilando el oráculo), **ownership/regiones**
o aislamiento por actores para data-race freedom, **GC concurrente** + runtime **M:N paralelo**,
**algebraic effects** sobre una VM de pila explícita, gestor de paquetes y FFI. Es un cambio de
*norte* (no una fase más), por eso vive en una rama aparte y no en la hoja de ruta principal.

## 22. M13 — Habilitadores de self-hosting

> **Orden de ejecución: M13 va ANTES que M12.** El capstone del proyecto es el self-hosting (§7
> de IDEAS, "raylang en raylang"), y **la concurrencia (M12) no es un prerrequisito** para
> compilar. M13 cierra los tres huecos prácticos que separan "self-hosting es expresable" de
> "self-hosting no es penoso". El número de sección (§22 > §21) refleja el orden cronológico de
> *anotación*, no el de *implementación*.

Tras M11, el lenguaje **ya es suficientemente expresivo** para escribir un lexer + parser +
checker + intérprete de raylang en raylang: `read_file`/`write_file` (M11.2c), `char`/`s[i]`/
`chars` (M11.4c), clasificación de `char` por comparación de code point (M11.7d), `enum` recursivo
con semántica de referencia para el AST (M5), `Result`/`Option`/`?` (M6.3), módulos por
directorios y cápsulas (M11.3–M11.6). Lo que **falta** no son features del lenguaje sino tres
asperezas que harían el bootstrap doloroso:

1. **No hay tipo mapa.** Un compilador vive de tablas de símbolos (`nombre → tipo`, ámbitos,
   `(tipo,método) → manglado`). Sin un `Map`, se haría con listas de asociación `O(n)`: correcto,
   pero lento y verboso.
2. **El tooling de test es mínimo.** `@test` (`() -> bool`) + `--test` (exit code = nº de fallos)
   no basta para validar un compilador contra sí mismo: falta `assert` y un reporte usable.
3. **La recursión profunda puede romper.** El **intérprete** recurre sobre la pila de Rust
   (`eval_expr`/`eval_stmt`) y el **parser de Rust** es descenso recursivo; un parser de descenso
   recursivo *escrito en raylang* y corrido en el intérprete es justo el caso peligroso. (La **VM
   ya es robusta**: marcos explícitos en el heap, bucle de bytecode — no usa la pila de Rust por
   llamada raylang.)

Tres hilos independientes (separables, pero recomendado el orden 13.3 → 13.2 → 13.1: primero que
no se caiga, luego poder testear, luego la pieza grande).

### 22.1 M13.1 — `Map<K,V>`

**Decisión central: un `Map` es un objeto del heap en ambos motores**, como los arreglos (M3) y los
structs, **no** un almacén del host. Razón: claves y valores son `Value`, y cada motor representa
los valores distinto (`Rc` en el intérprete, handles GC en la VM); un almacén del host no podría
guardarlos de forma compartida. Sigue el molde de los arreglos: nuevo `Obj::Map` / `HeapValue::Map`
**trazado por el GC**. **Toca el runtime y el GC** ⇒ oráculo VM↔intérprete con estrés de GC.

**Claves limitadas a primitivos hashables** (`string`, `int`, `char`, `bool`) en la primera
versión. Cubre el 99% de un compilador (claves string/int) y **evita** la maquinaria de un trait
`Hash` con paso de diccionarios. `Key` = enum interno de primitivos hashables. Clave genérica de
usuario (vía trait `Hash` + dicts, como M9.2) → **diferido**.

- `Type::Map(Box<Type>, Box<Type>)` — el `Type` es extensible (CLAUDE.md), encaja sin tratarlo como
  enum cerrado.
- `map_new() -> Map<K,V>` es **indeterminado** como `[]`/`None` ⇒ reusa el chequeo bidireccional
  (`check_expr_expected`, M6.2): `let m: Map<string,int> = map_new();`.
- Como cada builtin tras L1 es una fila en `BUILTINS` + opcode + impl por motor, el coste marginal
  por operación es bajo; lo caro es el primer `Obj::Map` (tipo + heap obj + tracing).

**Sub-fases:**
- **M13.1a — núcleo** ✅ **COMPLETO** (341 tests lib): `Type::Map(Box<Type>, Box<Type>)` (el parser
  lo trae como `Struct("Map", [K, V])`; `resolve_type` lo reclasifica, como `Enum`/`Var`).
  **Runtime**: `Value::Map(Rc<RefCell<HashMap<MapKey, Value>>>)` (intérprete) y `Obj::Map(HashMap<
  MapKey, HeapValue>)` (VM, **trazado por el GC** — solo los valores; las claves son primitivos
  *inline*). `MapKey` = enum hashable (Int/Str/Char/Bool; **no** float). Builtins (opcodes `MapNew`/
  `MapInsert`/`MapContainsKey`/`MapGet`): `map_new`, `insert(m,k,v)`, `contains_key(m,k) -> bool`,
  `__map_get(m,k) -> [V]` (+ envoltorio `get(m,k) -> Option<V>` en el prelude, patrón M11.2); `len`
  **extendido** a mapas. UFCS gratis: `m.insert(k,v)`, `m.get(k)`, `m.contains_key(k)`. `map_new()`
  es **indeterminado** (como `[]`/`None`): su tipo lo fija el esperado (`check_expr_expected`); sin
  anotación → error claro. **Clave hashable** validada en `ensure_type` (`Map<float,_>` se rechaza).
  Oráculo `map_basico/claves_variadas` + `map_estres_gc` (estrés del GC). `print` de un Map
  **diferido** (no es *printable*; Display ordena por clave → determinista). Ejemplo `examples/mapa.ray`.
- **M13.1b — recorrido** ✅ **COMPLETO** (343 tests lib): `__map_remove(m,k) -> [V]` (+ envoltorio
  `remove(m,k) -> Option<V>` en el prelude), `keys(m) -> [K]`, `values(m) -> [V]` (opcodes
  `MapRemove`/`MapKeys`/`MapValues`; los tres asignan heap → estrés del GC). `MapKey` gana
  `PartialOrd, Ord`: `keys` se devuelve **ordenada** y `values` en **ese mismo orden de clave**
  (casan posición a posición) → determinista pese al `HashMap` (clave del oráculo). En un mapa
  concreto todas las claves son del mismo tipo, así que el orden entre variantes nunca se observa.
  Capítulo del libro `m13/mapas.md`. **M13.1 COMPLETO.**

**Diferido:** clave genérica (`Hash`), `@derive(Eq/Show)` sobre `Map`, orden de iteración estable
expuesto al usuario.

### 22.2 M13.2 — `assert` + tooling de test

- **M13.2a — `panic` + `assert` + `assert_eq`** ✅ **COMPLETO** (338 tests lib): el **único** toque
  de runtime es el builtin **`panic(msg)`** (opcode `Panic`), que aborta con `msg` en la posición de
  la llamada — en el intérprete se intercepta en `eval_call` (devuelve `Flow::Error`); en la VM es el
  opcode `Panic` (saca el string, retorna `Err`). Ambos motores dan el **mismo mensaje** ⇒ oráculo
  (`panic_y_assert_falla_oraculo`). Sobre él, en el **prelude (raylang puro)**: `assert(cond)`
  (mensaje genérico) y `assert_eq<T: Eq + Show>(a, b)` (mensaje con ambos valores, vía `.igual()` +
  `.mostrar()`; los bounds se bajan a diccionarios M9.2). **Sin sobrecarga** en raylang, así que en
  vez de `assert(cond)` + `assert(cond, msg)` se ofrece `assert` + `assert_eq` + `panic("…")` directo
  para el mensaje a medida. Habilitadores: (1) `impl Eq`/`impl Show` para los **primitivos** en el
  prelude (faltaban; los pide `assert_eq`); (2) **`panic` diverge** (`expr_diverges` reconoce la
  llamada a `panic`) ⇒ una rama que termina en `panic` cede el tipo a la otra
  (`match (x) { Some(v) => v, None => panic("…") }` cuadra). *Limitación honesta*: el error de un
  `assert` apunta a la línea del prelude donde está el `panic` (no al sitio de la aserción; raylang
  no tiene backtraces); el mensaje sí es descriptivo.
- **M13.2b — runner mejorado** ✅ **COMPLETO** (cliente externo `test_runner.rs`, **no toca el
  core**; 338 tests lib + 6 en `tests/test_cli.rs`):
  - `@test` admite también `() -> unit`: **pasa si no dispara ningún `assert`/`panic`** (el checker
    relaja la firma a `bool` *o* `unit`; el runner detecta el tipo del AST y decide el criterio).
  - **Aislamiento por prueba**: cada test corre en su **propia** ejecución del intérprete (se clona
    el programa base y se le sintetiza un `main` que llama solo a esa prueba). Así un `panic`/aserción
    que falle aborta *esa* ejecución, **no la batería**, y se captura su mensaje. (Antes: un único
    `main` con todas → un panic abortaba todo.)
  - Reporte por test: `ok NAME` / `FALLO NAME` + el mensaje del fallo; resumen final
    (`N de M prueba(s) fallaron`). Código de salida = nº de fallos (compat hacia atrás).
  - Filtrado por nombre (subcadena): `raylang --test archivo.ray patron`.

**Diferido:** setup/teardown, *property testing*, timing por test.

### 22.3 M13.3 — Robustez de recursión profunda

Diagnóstico: frágiles son **(a) el intérprete** (recursión en la pila de Rust) y **(b) el parser de
Rust**; la **VM ya es robusta** (marcos en el heap).

- **M13.3a — sin segfaults, techo alto** ✅ **COMPLETO** (336 tests lib):
  1. **Hilo worker con pila grande**: `lib::with_big_stack` corre todo el trabajo del binario en
     un hilo con pila de **256 MiB** (`std::thread::Builder::stack_size`), muy por encima de los
     ~8 MiB del hilo principal. Cubre (a) el intérprete y (b) el parser de Rust a la vez. Es lo
     que hace rustc.
  2. **Límite compartido**: `interpreter::MAX_CALL_DEPTH = 1024`, que la VM reusa como su
     `MAX_FRAMES`. El intérprete cuenta llamadas a `call_body` (campo `depth`, comprobado **antes**
     de incrementar, igual que la VM mira `frames.len()` antes de empujar) → **ambos motores
     coinciden en la frontera**: `cuenta(1000)` corre en los dos, `cuenta(1100)` da el **mismo
     error** ("desbordamiento de pila (recursión demasiado profunda)") en los dos. El intérprete
     reporta en la posición del cuerpo de la función; la VM, en el sitio de llamada (el mensaje es
     idéntico, que es lo que el oráculo compara). Test `overflow_recursion_oraculo`.
- **M13.3b — TCO en AMBOS motores** ✅ **COMPLETO** (346 tests lib): recursión de cola en O(1) de
  pila. Se hizo en **los dos motores** (no solo la VM) para no romper el oráculo: con TCO solo en la
  VM, una recursión de cola profunda correría en la VM pero el intérprete cortaría en
  `MAX_CALL_DEPTH` → divergencia. La detección de posición de cola usa **reglas estructurales
  idénticas** en ambos (cuerpo de función, ramas de `if`/`match`, tail de bloque, valor de `return`),
  así coinciden por construcción.
  - **VM**: *peephole* `optimize_tail_calls` sobre el bytecode ya emitido — un `Call`/`CallValue`
    cuya continuación es un `Return` (directo o vía saltos incondicionales, `returns_immediately`) se
    convierte en `TailCall`/`TailCallValue`, que **reutilizan el marco actual** en vez de apilar uno.
    No hay que tocar la emisión (el compilador ya genera ese patrón de forma natural).
  - **Intérprete**: **trampolín** — `Flow::TailCall { index, args, captured }`; `call_body` es un
    **bucle** que, al recibir una `TailCall`, reemplaza la función actual y reitera (no recurre, no
    crece `depth`). `eval_tail`/`eval_tail_block` evalúan en posición de cola (una llamada ahí produce
    `TailCall`; `if`/`match`/bloque propagan; el resto delega en `eval_expr`); `return e` evalúa `e`
    en cola. Los builtins (incl. `panic`) NO son tail (son hoja).
  - **Gotcha**: el viejo test `overflow_recursion_oraculo` usaba `bucle(n+1)` (cola) esperando
    desbordar; con TCO eso es un **bucle infinito legítimo** (como `while(true)`). Se cambió a
    recursión **no de cola** (`1 + bucle(...)`) para seguir probando el límite. Verificado: 1 000 000
    de llamadas en cola y recursión mutua profunda corren en O(1) marcos y **coinciden** VM↔intérprete.

**Diferido:** reescribir el intérprete a pila explícita/CPS (gran obra; el trampolín de M13.3b ya
cubre la recursión de cola, que es el caso que importa; solo valdría la pena el CPS completo para
*algebraic effects* en la rama de producción de §21.1).

### 22.4 Resumen de impacto

| Hilo | Toca runtime | Oráculo | Tamaño |
|------|--------------|---------|--------|
| M13.1 `Map<K,V>` | **Sí** (nuevo heap obj + GC) | Sí, con estrés GC | Grande |
| M13.2 `assert`/test | `assert` sí; runner no | `assert` sí | Medio |
| M13.3 recursión | Sí (hilo + límites) | Sí (mismo error) | Pequeño-medio |

El self-hosting (capstone, §7 de IDEAS) queda **después de M13** y sigue siendo ortogonal a M12.

## 23. M14 — Self-hosting (bootstrap)

El capstone (IDEAS §7): reescribir el compilador de raylang **en raylang**. Vive en la rama
`feature/self-hosting` y en el directorio `selfhost/`. Se ataca **fase a fase** (lexer → parser →
checker → …), cada una validada contra su equivalente en Rust como **oráculo**: para la misma
entrada, las dos implementaciones deben producir la misma salida. Es el mismo principio del oráculo
VM↔intérprete, ahora aplicado raylang↔Rust.

**Estrategia del oráculo (texto canónico).** No se exponen los tipos internos de Rust a raylang.
En su lugar, cada fase define un **formato de texto canónico** de su salida, implementado *idéntico*
en los dos lados; el test compara los textos. Para el lexer: un token por línea, `<KIND>@<línea>:
<col>` (`Let@1:1`, `Int(42)@2:3`, `Str("a\nb")@..`); las cadenas/caracteres se re-escapan igual en
ambos lados. Los flotantes se formatean con el `Display` de `f64` de Rust en los dos motores → caza
exacta (de ahí que `parse_float` fuera prerrequisito).

### 23.1 M14.1 — El lexer (`selfhost/lexer.ray`)

Port casi 1:1 de `src/lexer.rs`. **Viabilidad clave** (lo que el lenguaje ya daba): structs con
**mutación de campos por referencia** (sin `var`) para el estado del cursor; `chars`/indexar string/
`len` (M11.4c) para recorrer; comparación de `char` (M11.7d) para clasificar; `parse_int`/
`parse_float` para los literales. **Diferencias de port** (anotadas en el archivo): no hay `match`
sobre literales de carácter → cadenas `if/else`; el fin de entrada se maneja con guardas
`at_end(lx)` + indexación directa (raylang no acepta `'\0'` como literal centinela).

- **Driver** `selfhost/lex_dump.ray`: importa el lexer, lee un archivo (`args()[0]` + `read_file`),
  imprime los tokens en el formato canónico. Es el cliente que el test ejecuta por subproceso.
- **Oráculo** `tests/selfhost_lexer.rs`: para cada fuente (snippets + **archivos reales**: ejemplos
  y el *propio* `lexer.ray`/`lex_dump.ray`) compara el stdout del lexer-en-raylang con `canonical()`
  (el lexer de Rust formateado igual). Que el lexer en raylang **se lexee a sí mismo** igual que el
  de Rust es la señal de fidelidad.

**Dos prerrequisitos de lenguaje** que el bootstrap destapó (ambos aditivos, mejoras legítimas):
1. **`parse_float`** (builtin, ya descrito) — el lexer necesita parsear flotantes.
2. **Escape `\r`** en cadenas y literales de carácter (lexer de Rust + el auto-alojado) — estándar y
   necesario para que el propio código del lexer (que lo usa) lexee.

**Dos huecos de divergencia cerrados** (extienden M13.2a, que solo cubría ramas de `if`): un brazo de
`match` que termina en `panic`/`return` ahora **cede el tipo** a los demás (igual que una rama de
`if`). Sin esto, `match (o) { Some(v) => v, None => panic("…") }` no tipaba —y el lexer lo usa por
todas partes—.

### 23.2 M14.1b — Errores del lexer como valores

El lexer de M14.1a cubría el **camino feliz**: ante una entrada inválida hacía `panic` (abortaba). En
M14.1b se vuelve robusto **igual que el de Rust**: `lex` devuelve `Result<[Token], LexError>` y un
`struct LexError { msg, line, col }` (espejo del de Rust). Cada función reconocedora (`number`,
`string_lit`, `char_lit`, `next_token`) devuelve `Result<TokKind, LexError>`; `lex` propaga el primer
error con el operador **`?`** (M6.3) y el helper `lex_error(lx, msg)` fija la posición al **inicio del
token** (no al carácter ofensor), como el `self.error(...)` del original.

**Lo importante para el oráculo:** los mensajes se construyen *idénticos* a los de Rust, incluyendo el
fragmento ofensor (`carácter inesperado '#'`, `secuencia de escape inválida '\q'`, …). El driver
`lex_dump.ray` imprime el error con el mismo formato que el `Display` de `LexError`
(`error léxico en <l>:<c>: <msg>`), y el oráculo (`canonical`) hace `format!("{e}")` cuando el lexer de
Rust falla → la comparación cubre **también las entradas inválidas** (carácter inesperado, cadena/
carácter sin cerrar, salto de línea en cadena, escape inválido, `&`/`|` sueltos, char vacío/multi).
**Nota de tipos:** `parse_int`/`parse_float` devuelven `Option`, así que `?` no cruza Option→Result; se
desenvuelven con `match` y se reempaquetan como `Result.Err(lex_error(...))`. **M14.1 COMPLETO.**

### 23.3 M14.2 — El parser (`selfhost/parser.ray`)

Tercera fase del bootstrap: tokens (del lexer auto-alojado) → **AST**. Es ~4× el lexer → se ataca en
sub-fases. El parser auto-alojado **se alimenta del lexer auto-alojado** (`from lexer import Token,
TokKind;`), así que la cadena de bootstrap crece. Camino feliz con `panic` (errores como valores al
cerrar el parser, como en el lexer).

**Oráculo (volcado canónico del AST).** Misma estrategia que el lexer, ahora sobre un árbol: un
formato **S-expression** con la **posición `@línea:col` en cada nodo** de Expr/Stmt (decisión
tomada: máximo rigor, caza bugs de propagación de posición —binario hereda del izquierdo, paréntesis
conserva el `(`, call/index/field heredan del receptor—). El driver `selfhost/parse_dump.ray` lo
imprime (una función por línea); el oráculo `tests/selfhost_parser.rs` lo reconstruye desde el AST
del parser de Rust con `dump_program`. Importante: el dump se hace **sobre el AST crudo del parser**
(sin checker), así que los nombres en posición de tipo son `Struct(n, [])` (no `Enum`/`Var`) y no hay
`EnumLit` —el parser auto-alojado produce su equivalente `TNamed`, y la comparación cuadra—.

**Viabilidad clave (lo nuevo que el bootstrap del parser exige):** el AST es **mutuamente
recursivo** (`struct Expr` lleva un `EKind`, que contiene `Expr`/`Block`); funciona porque
structs/enums viven en el heap (semántica de referencia → recursión sin tamaño infinito). Validado
con spikes: tipos mutuamente recursivos, **arreglos** del tipo recursivo (`[Expr]`) y **`Option`** del
tipo recursivo (`Option<Expr>` para `tail`/`else`). El estado del parser es un `struct Parser` mutado
por referencia (como el `Lexer`). Para los checks de token se usa `tok_name(k) -> string` (la grafía
canónica del token: `"("`, `"->"`, `"let"`; nombre simbólico para los que cargan valor: `"ident"`,
`"int-lit"`) → `check`/`eat`/`expect` comparan strings legibles, sin números mágicos.

- **M14.2a COMPLETO** — núcleo: expresiones (toda la cadena de precedencia: `logic_or`→…→`primary`),
  sentencias (let/var, asignación, return, expr), tipos básicos (primitivos, `[T]`, `fn(..)->R`,
  nombre), bloques y funciones de nivel superior. Cubre fib/fizzbuzz. Oráculo: snippets + los archivos
  reales que solo usan estas features.
- **M14.2b COMPLETO** — datos y control: definiciones `struct`/`enum` (sin genéricos), literal de
  struct `Nombre { campo: valor }`, funciones anónimas `fn(..) { .. }` (con `id` denso en pre-orden,
  como el parser de Rust) y `match`/patrones (comodín, binding, `Enum.Variante(sub-bindings)`). El AST
  crece con structs/enums propios y tres variantes de `EKind` (`EStructLit`/`EFunc`/`EMatch`); el
  `Program` ahora lleva `funcs`+`structs`+`enums` (orden de volcado fijo: funciones, structs, enums).
  Oráculo: snippets + `examples/enums.ray` y `examples/match_figuras.ray` reales. **Gotcha**: un
  `Option.None` suelto en argumento de builtin (`push(binds, Option.None)`) no infiere su `T`; se
  materializa en una `var` tipada (`var bv: Option<string> = Option.None;`) cuyo tipo declarado fija el
  `None`. Diferido: M14.2c (traits/impls/genéricos/dyn/Map/`?`/pipelines/anotaciones/imports/`pub`).
- **M14.2c-1 COMPLETO** — sistema de tipos: genéricos (`<T: A + B>`) en fn/struct/enum/impl, argumentos
  de tipo (`Caja<int>`; `Map<K,V>` es un genérico ordinario a nivel de parser —no hay nodo especial,
  como en Rust, donde el checker reclasifica `Struct("Map",…)`—), trait objects `dyn A + B` (conjunto
  **canónico**: el parser ordena+dedup con `sort` del prelude + pasada lineal), `trait` (firmas +
  cuerpos por defecto), `impl [<…>] Trait for Tipo`, y el receptor `self` (tipo `Self`). El AST gana
  `Bound`/`TraitDef`/`MethodSig`/`ImplBlock`, genéricos en las declaraciones, `Type::TNamed(string,
  [Type])` (antes sin args) y `TDyn([string])`; `Program` lleva ahora `traits`+`impls`. **Decisión de
  fidelidad**: el `self` receptor se representa como `TNamed("Self",[])` y se vuelca `"Self"` —igual que
  el `SelfType` de Rust—, así el dump cuadra sin un nodo `Self` propio. Oráculo: snippets + 14 ejemplos
  reales (genericos, bounds, traits, tipos_genericos, impls_genericos, trait_objects,
  metodos_por_defecto, ufcs, inferencia, funciones…). Diferido a M14.2c-2: `?`, `|>`, anotaciones,
  `pub`, imports, tipos calificados `M.Tipo`/`M.Enum.V`.
- **M14.2c-2 COMPLETO** — azúcar y módulos, **cierra el parser**: operador `?` (`ETry`), pipelines
  `|>` (desugar puro a `Call` con el receptor como primer argumento, `make_pipeline`), anotaciones
  `@nombre[(args)]`, `pub`, `import M [as x]` / `import a/b/c` (vía `module_path`), `[pub] from M
  import a [as b]{, …}`, y referencias calificadas por módulo en posición de tipo (`M.Tipo`, con el `.`
  guardado en el nombre), literal de struct (`M.Tipo { … }`, en `call()` si el receptor del `.` es un
  Ident) y patrón (`M.Enum.Variante`). El AST gana `Annotation`/`ImportDecl`/`ImportName`/`FromImport`,
  `annotations`+`is_pub` en fn/struct/enum (+ `is_pub` en trait), y `Program` lleva
  `imports`+`from_imports`. **Hito de fidelidad**: el test fuerte parsea los **35 ejemplos** y los **4
  fuentes del self-hosting** (`lexer.ray`/`lex_dump.ray`/`parser.ray`/`parse_dump.ray`) → **el parser
  se parsea a sí mismo** idéntico al de Rust, nodo a nodo con posiciones.
- **M14.2d COMPLETO** — errores del parser como valores: `parse` pasa de camino feliz (`panic`) a
  **`Result<Program, ParseError>`** + `struct ParseError { msg, line, col }`; cada función de parseo
  propaga con **`?`** (igual que el lexer en M14.1b). `expect`/`expect_ident` devuelven `Result`;
  `perr_here(p, msg)` fija la posición en el token actual (como `error_here` de Rust), `perr_at(l,c,
  msg)` en una explícita. Los mensajes son **idénticos** a los de Rust, incluido el caso peliagudo
  "se esperaba una expresión, se encontró `<Debug>`": se reproduce la **repr Debug** de `TokenKind`
  con `tok_debug(k)` (los nombres de variante: `Semicolon`, `LParen`…). Se añade también el
  *enforcement* de `parse_program` (anotaciones/`pub` mal colocados sobre trait/impl). El oráculo
  compara también **entradas inválidas** (`error de sintaxis en L:C: msg`): 11 casos. **M14.2 COMPLETO**
  (parser: lexer→parser auto-alojados, ambos con errores como valores, validados sobre todo el corpus
  + auto-aplicación).

**Próximas fases:** el **checker** (§23.4), y finalmente ejecutar. Cada una, su oráculo.

### 23.4 M14.3 — El checker (diseño)

La fase más compleja del compilador. El checker de Rust (`src/checker.rs`, ~5000 líneas incl. tests)
hace dos trabajos separables que el diseño aprovecha. Mirando `check()`:

```
prepare_program  → inyecta prelude + genera @derive + baja métodos de impl + resuelve enums
check_program    → LA VALIDACIÓN (dos pasadas: firmas → cuerpos)        ← produce el veredicto
lower_ufcs / append_dict_params / lower_dict_calls / lower_dyn / renumber ← reescrituras para el back-end
```

El **veredicto** (¿type-checkea?, ¿qué error?) sale de `check_program` (+ la resolución de `prepare`).
Todo el *lowering* de M9 (UFCS→llamada, paso de diccionarios, síntesis de structs `dyn`, inyección de
funciones mangladas, renumerado de fn-exprs) son reescrituras del AST **para el back-end**, no para el
veredicto.

**Decisión de alcance (validador).** El checker auto-alojado es un **validador**: consume el AST (del
parser auto-alojado) y produce un veredicto —`ok` o `error de tipos en L:C: msg` byte-idéntico al de
Rust—. **No** reproduce el lowering de M9 (queda para una fase de back-end posterior). Sí incluye la
*resolución necesaria para validar* (reconocer construcción de enums, resolver métodos/UFCS para
obtener su tipo, inferencia de genéricos); solo se omiten el registro de sitios y las pasadas
post-check. Esto acota M14.3 a algo tratable.

**Decisión de oráculo (solo veredicto).** Misma filosofía que lexer/parser: la misma fuente por los dos
pipelines (Rust: `lex→parse→check`; raylang: `self-lex→self-parse→self-check`), comparar el veredicto
(`ok` / `error de tipos en L:C: msg`). Corpus = programas **válidos** (los 35 ejemplos + los fuentes del
self-hosting) **e inválidos** (errores de tipo). El corpus inválido caza la sobre-aceptación; los
mensajes de error son idénticos a los de Rust.

**Reutilización y habilitadores.** El checker reusa el `Type`/`Expr`/AST del parser auto-alojado; el
`Type` del parser **dobla** como representación del tipo inferido. Ámbitos y firmas con **`Map<K,V>`**
(habilitador de M13.1 — por eso vino antes). **Builtins** vía una tabla (espejo de `builtins.rs`). El
**prelude** (Option/Result/Eq/Show/Ord/map/filter/fold) se inyecta cuando haga falta (parseando el
fuente compartido) — diferido hasta genéricos/traits. Los **errores como valores** son inherentes (el
checker devuelve `Result<_, TypeError>` desde el inicio; no hay sub-fase `d` aparte como en el parser).

**Sub-fases.**
- **M14.3a — núcleo monomórfico**: dos pasadas (firmas → cuerpos), pila de ámbitos, literales,
  operadores (reglas de tipo de aritmética/comparación/lógica), variables (`let`/`var`, mutabilidad,
  ámbito), llamadas (aridad + tipos de args), `if`/`while`/`block`/`return`, anotaciones de tipo,
  análisis de divergencia, builtin `print`. Cubre fib/fizzbuzz. La más grande (sienta el armazón).
- **M14.3b — datos**: arreglos (`[T]`, índice, `len`/`push`), structs (def, literal, acceso a campo),
  enums (resolución de construcción, `match` + exhaustividad + patrones, tipos recursivos).
- **M14.3c — genéricos**: parámetros de tipo, `unify`/`subst`, llamadas genéricas, structs/enums
  genéricos, chequeo bidireccional (tipo esperado), `Option`/`Result` + inyección del prelude, `?`.
- **M14.3d — traits/impls**: registro de trait/impl, resolución de métodos (UFCS + métodos de trait),
  bounds, registro de `@derive`, `dyn` (solo validación). La más difícil.

Es la fase más grande del proyecto; serán varios commits por sub-fase. Tras el checker queda el
**back-end** (ejecución / lowering) para cerrar el self-hosting.

- **M14.3a COMPLETO** — núcleo monomórfico. `selfhost/checker.ray` valida el AST del parser
  auto-alojado y devuelve `Result<int, TypeError>` (`ok` o `error de tipos en L:C: msg`). Dos pasadas:
  registrar firmas (`Map<string, FnSig>`) → exigir `main` → verificar cuerpos. Pila de ámbitos
  (`[Map<string, VarInfo>]`, `push`/`pop`/`get`/`insert` del stdlib). Cubre literales, operadores
  (mismas reglas/mensajes que Rust: aritmética/orden/igualdad/lógica, con `bin_op_str`/`is_comparable`/
  `order_ok`), variables (let/var, mutabilidad, ámbito), llamadas (aridad + tipos, con el builtin
  `print`), `if`/`while`/`block`/`return`, anotaciones (`ensure_type`/`resolve_type` monomórficos),
  divergencia (`block/stmt/expr_diverges`, incl. `panic`). El `Type` del parser **dobla** como tipo
  inferido; `type_eq`/`type_str` (= `Display`) propios. Driver `selfhost/check_dump.ray`. Oráculo
  (`tests/selfhost_checker.rs`): 8 válidos + 20 errores de tipo + 4 ejemplos reales (fib/fizzbuzz/gcd/
  primes), veredicto byte-idéntico a Rust. Diferido: M14.3b (datos), c (genéricos), d (traits).
- **M14.3b COMPLETO** — datos (monomórficos). `selfhost/checker.ray` gana arreglos (literal `[T]`,
  índice `a[i]` con string→char, `len`/`push`), structs (definición + tablas `structs`/`enums` con
  campos/variantes ya resueltos en `register_types`, literal `Nombre { c: v }`, acceso a campo,
  asignación a campo/índice sin exigir `var`) y enums (construcción `Enum.Variante(args)` —reconocida
  **en el sitio** comprobando si el nombre es un enum, sin reescribir el AST a `EnumLit` como hace
  Rust—, `match` con patrones `_`/binding/variante, **exhaustividad**, brazos convergentes, tipos
  recursivos). El `Type` del parser **dobla** para struct y enum (`TNamed`); se distinguen por en qué
  tabla está el nombre. Chequeo bidireccional **mínimo** (`check_expr_expected`): el tipo esperado fija
  el `[]` vacío (`let xs: [int] = []`) y se propaga al cuerpo de función (`check_block_expected`) — el
  bidireccional completo (`None`, construcciones indeterminadas) llega en M14.3c. `ensure_type` ahora
  recibe `c` (un `TNamed` debe ser un struct/enum registrado). Oráculo: 8 programas de datos válidos +
  31 errores (struct/arreglo/enum/match, incl. tipos duplicados que el checker detecta directamente) +
  9 ejemplos reales (añade structs/match_figuras/enums/arrays/matriz). UFCS/métodos, `Map` en el
  checker, genéricos y `dyn` siguen diferidos a M14.3c/d.
