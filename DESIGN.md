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
| **M13** | **habilitadores de self-hosting**: `Map<K,V>`, `panic`/`assert`+test, recursión profunda + **TCO** | tablas hash, aserciones, robustez de pila, llamadas en cola | ✅ M13.1 `Map` · M13.2 `panic`/`assert`+runner · M13.3 pila grande + límite + TCO (ambos motores) |
| **M14** | **self-hosting**: lexer/parser/checker/intérprete/loader en raylang → **meta-circularidad** | bootstrapping, oráculo (texto/conductual), *erasure* por resolución en runtime | ✅ **LOGRADO** (M14.1 lexer · M14.2 parser · M14.3 checker · M14.4 intérprete · M14.6 stdlib · M14.7 loader + meta-circularidad) |
| **M12** | **concurrencia**: CSP sobre la VM (green threads cooperativos M:1 + canales tipados) | scheduler determinista, green threads, fibras, GC multi-raíz | ✅ §21.2–§21.6: ✅ **M12.1** slice CSP · ✅ **M12.2** acotados/backpressure · ✅ **M12.3** structured concurrency · ✅ **M12.4** `select` · ✅ **M12.5** cancelación de hermanas. **M12 COMPLETO** (diferido: cancelación preemptiva, `Selected<T>`, select de send) |
| **M15** | **redes + base moderna**: sockets (builtins/`std::net`) + HTTP/JSON (librería raylang) + reloj/RNG/matemáticas | I/O de red, handles, librerías sobre builtins, base de runtime | 🚧 §24: ✅ **M15.1a** matemáticas (oráculo) · ✅ **M15.1b** reloj/RNG (`now`/`monotonic`/`sleep`/`random`/`random_int`; PRNG SplitMix64 propio; subproceso) · ✅ **M15.2** cliente TCP (`tcp_connect`/`socket_read`/`socket_write` sobre `std::net`; handle reusa el registro de M11.8; `close` extendido; subproceso vs. servidor de juguete) · ✅ **M15.3** servidor TCP (`tcp_listen`/`tcp_accept`/`local_port`; `OpenHandle::Listener`; servidor secuencial bloqueante; subproceso con el `.ray` de servidor) · ✅ **M15.4a** JSON (librería `examples/web/json.ray` en raylang: `parse`/`stringify`, objetos `Map<string,Json>`, salida canónica; cero runtime; subproceso golden) · ✅ **M15.4b** HTTP (librería `examples/web/http.ray` en raylang sobre TCP: `fetch`/`request`/`header`, parseo de URL/respuesta; compone con `json`; subproceso vs. servidor de juguete) · ✅ **M15.5** (capstone) sockets no bloqueantes + scheduler de M12 (`tcp_accept`/`socket_read` ceden la fibra; busy-poll cooperativo, `io_parked`, cero deps; servidor concurrente con `spawn`; solo VM). **M15 COMPLETO** (diferidos ya resueltos: `epoll` M17, bytes M16, TLS M19.4, cesión en `socket_write` post-M19) |
| **M16** | **tipo `bytes`** (datos binarios) | nuevo tipo en el pipeline, literal `b"..."`, I/O binaria | ✅ §25: ✅ **M16.1a** el tipo (literal `b"..."` con `\xNN`, `len`/index→int/`==`; oráculo) · ✅ **M16.1b** string-interop (`to_bytes`/`from_utf8` + `+`; oráculo) · ✅ **M16.1c** I/O binaria (`read_file_bytes`/`write_file_bytes`/`socket_read_bytes`/`socket_write_bytes`; lecturas → `[bytes]` etiquetado; socket cede al scheduler; subproceso). **M16 COMPLETO** (cierra la deuda binaria de M15). ✅ **`bytes` clave de Map** (post-M19; `MapKey::Bytes`). Diferido: mutabilidad |
| **M17** | **`epoll`/`kqueue`** (readiness real, sustituye el busy-poll de M15.5) | E/S asíncrona del SO, `unsafe` acotado, FFI cero-deps | ✅ §26: poller del SO (`kqueue` macOS/BSD, `epoll` Linux) en `src/poll.rs`; FFI propio (`extern "C"`, sin el crate `libc` → invariante cero-deps); el scheduler de la VM se **bloquea** hasta readiness real y despierta **solo** las fibras de los fds listos (`io_parked` lleva ahora el `fd`); fallback al busy-poll de M15.5 en plataformas sin poller o EINTR; comportamiento idéntico (regresión: tests de M15.5/red concurrente). **M17 COMPLETO** (✅ cesión en `socket_write` post-M19: el poller gana interés de escritura `wait(read_fds, write_fds)`; diferido: registro persistente del poller, `bytes`/bitops en el toolchain auto-alojado) |
| **M18** | **backend nativo** (bootstrap sin Rust) | codegen a máquina/LLVM/C | 💤 **aparcado** (decisión del usuario): no perseguir lo nativo/sin-toolchain por ahora; el esfuerzo va al transversal de optimización de la VM. Se retoma más adelante |
| **M19** | **la capa web** (servidor HTTP async + SSE · HTTP en bytes · WebSockets `ws://` · TLS) | protocolos de alto nivel como librería raylang sobre los sockets/scheduler; criptografía vs. cero-deps | ✅ §28: ✅ **M19.1** servidor web async + SSE (librería `webserver.ray` sobre el servidor concurrente de M15.5/M17; cero runtime) · ✅ **M19.2** HTTP en `bytes` (builtin `sub_bytes` + cliente `http.ray` y servidor `webserver.ray` con cuerpo `bytes`; round-trip binario `\x00`/`\xff` intacto) · 🚧 **M19.3** WebSockets `ws://` (handshake SHA-1+base64 en raylang + framing con `bytes`): ✅ **M19.3a** operadores bit a bit `& | ^ ~ << >>` (habilitador, único toque de lenguaje; oráculo) · ✅ **M19.3b** SHA-1 + base64 en raylang (`sha1.ray`/`base64.ray`, cero runtime nuevo; vectores RFC 3174/4648/6455) · ✅ **M19.3c** handshake + framing + echo server (`websocket.ray`/`websocket_echo.ray`; builtin `bytes_of`; e2e en el test) → **M19.3 COMPLETO** · ✅ **M19.4** TLS/SSL — **decidido: excepción cero-deps con `rustls`** (§28.4; 1.ª dependencia de Cargo): ✅ **M19.4a** cliente TLS + `https://` (`tls_connect`; handle TLS en el registro de sockets → `http.ray` habla https transparente; verificación con `webpki-roots` + `SSL_CERT_FILE`; test determinista con servidor TLS local) · ✅ **M19.4b** servidor TLS + `wss://` (`tls_accept`; rustls conducido a mano sobre el enum `Connection`, **integrado con el scheduler no bloqueante** —aparca la fibra en el fd al bloquear leyendo—; misma bomba sirve a ambos motores; `wss_echo.ray`; e2e con cliente WebSocket-sobre-TLS en el test) → **M19.4 + M19 COMPLETOS** |
| **M20** | **cripto, identidad y clientes cloud** (SHA-256/HMAC · JWT/UUID · URL/cookies · tiempo · Redis) | la capa que un servicio cloud/distribuido necesita, como librería raylang sobre M19 | 🚧 §29: ✅ **M20.1** SHA-256 (`sha256.ray`, FIPS 180-4 en raylang puro, gemelo de `sha1.ray`; vectores NIST por ambos motores) · 🚧 M20.2 HMAC-SHA256 + base64url + hex · M20.3 JWT (HS256) + UUID v4 · M20.4 URL/query/cookies · M20.5 tiempo/fechas · M20.6 cliente Redis (RESP) · M20.7+ HTTP robusto + gzip + UDP. Filosofía M15/M19: protocolos = librería en raylang, runtime intacto salvo lo imposible (UDP, fecha UTC) |
| **Transversal** | **VM auto-alojada** ✅ (M14.5) · optimización de la VM de Rust (incremental, midiendo) 🚧 | rendimiento, bootstrapping | 🚧 §27: banco `benchmarks/` (`bench.sh`+hyperfine, o `measure.py` sin deps; fib/bucle/arreglos, mejor-de-N); regla **medir-antes-y-después, conservar solo lo que supera el ruido**. Opt.1/Opt.2 ✅ (pase previo), Opt.3 (`Rc<str>`) ❌ descartada. ✅ **Opt.4** fast-path entero en el lazo de ops binarias (evita el doble match + la llamada a `apply_binary`; fib(35) −5%, bucle 10M −6%) · ✅ **Opt.7** posición `(línea,col)` perezosa (`pos!()`: se quita la lectura de `lines[ip]` por instrucción del camino caliente; **−7/−9/−8 % en fib/loop/arrays**, consistente; mejor-de-15 para destapar la señal bajo el ruido) · Opt.5/Opt.6/Opt.8/LTO **descartados** (medidos dentro del ruido o incorrectos). Oráculo VM↔intérprete intacto en cada paso |

> El detalle y la clasificación de impacto de los hitos viven en [IDEAS.md](IDEAS.md) hasta
> que cada uno se especifica en su propia sección al arrancarlo (M12 → §21, M13 → §22, M14 → §23,
> M15 → §24).
> Dependencias clave: `@derive`/`@delegate` (M10) necesitan **traits** (M9); el self-hosting
> (M14) necesitó **módulos + I/O de archivos** (M11) y los habilitadores de **M13**.

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
&  |  ^  ~  <<  >>     bit a bit (M19.3a)
=                     asignación
( )  { }              agrupación / bloques
,  ;  :               separadores
->                    flecha de tipo de retorno
```
Nota (M19.3a): `<<`/`>>` se lexean siempre como un token; en genéricos anidados
(`Caja<Caja<int>>`) el parser **parte** el `>>` en dos `>` al cerrar (estilo Rust).
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
logicAnd       = bitOr { '&&' bitOr } ;
bitOr          = bitXor { '|' bitXor } ;                       (* M19.3a *)
bitXor         = bitAnd { '^' bitAnd } ;                       (* M19.3a *)
bitAnd         = equality { '&' equality } ;                   (* M19.3a *)
equality       = comparison { ( '==' | '!=' ) comparison } ;
comparison     = shift { ( '<' | '<=' | '>' | '>=' ) shift } ;
shift          = term { ( '<<' | '>>' ) term } ;              (* M19.3a *)
term           = factor { ( '+' | '-' ) factor } ;
factor         = unary { ( '*' | '/' | '%' ) unary } ;
unary          = ( '!' | '-' | '~' ) unary | call ;          (* '~' M19.3a *)
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
  - bit a bit `& | ^ << >>` (binarios) y `~` (unario): ambos operandos `int` → `int`
    (M19.3a). Sin `float`. Los desplazamientos usan `wrapping_*` sobre `i64` (cuenta
    mod 64, sin panic) — idénticos en intérprete y VM.
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
- **UFCS resuelve también funciones `from`-importadas** (post-M19). El azúcar `recv.f(...)`
  llega como `Call(Field(recv, f))`; el loader **no** reescribe el nombre del método `f`
  (no tiene tipos para decidir campo-vs-función), así que una `f` traída por `from M import f`
  quedaba sin resolver (era `M::f` en la tabla, no `f`). Arreglo: el loader deja en
  `Program.ufcs_aliases` el mapa **nombre local → global** de las funciones from-importadas;
  el checker lo usa como **fallback** en el paso (3) —después de campo y método, así que la
  prioridad se conserva—. Un mismo alias que mapee a globales distintos en módulos distintos
  es ambiguo sin contexto → se **excluye** (degradación segura). Habilita librerías con API por
  punto (p. ej. el micro-framework `examples/web/framework.ray`, importado por su demo). Los
  imports **calificados** (`import M; M.f(...)`) no añaden alias UFCS (se usan como `M.f`).

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
    fn show(self) -> string;       // firma: cuerpo ausente, termina en ';'
}

struct Punto { x: int, y: int }

impl Mostrable for Punto {
    fn show(self) -> string {       // 'self' es el Punto receptor
        "punto"
    }
}

fn main() -> int {
    let p = Punto { x: 1, y: 2 };
    print(p.show());                // UFCS: resuelve al método del impl
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
llamar `x.show()` dentro del cuerpo, porque `T` **garantiza** implementar `Mostrable`. El
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
   fn imprimir<T: Mostrable>(x: T) { ... x.show() ... }
           │
           ▼  (parámetro oculto añadido en el checker)
   fn imprimir<T>(x: T, «T#Mostrable#mostrar»: fn(T) -> string) { ... }
   ```

2. **Llamada de método sobre `T`.** Dentro del cuerpo, `x.show()` con `x: T` acotado se
   resuelve al **parámetro-diccionario** y se baja como una llamada a ese valor función:
   `x.show()` → `«T#Mostrable#mostrar»(x)`. Reusa exactamente el lowering de UFCS/M9.1
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
    fn show(self) -> string { self.contenido.show() }
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
(`type_key_of(Caja<T>) = "Caja"`). Por eso `caja.show()` con `caja: Caja<int>` despacha a
`Caja#mostrar`; como ahora es genérica, `check_generic_call` infiere `T=int` y el sitio registra
el diccionario interno que necesita. *Alcance:* solo impls **plenamente genéricos** (los args del
objetivo son exactamente los parámetros de tipo del impl: `Caja<T>`, no `Caja<int>`), **un impl
por `(constructor, trait)`** — sin instancias solapadas ni especializadas (se difieren).

**El punto genuinamente nuevo — diccionarios anidados.** Al pasar un `Caja<int>` a *otro*
genérico acotado, su diccionario ya **no es una función plana**:

```rust
fn imprime<X: Mostrable>(x: X) { ... x.show() ... }
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
  (p. ej. `fn eq(self, otro: Self) -> bool`) → soportado por M9.1 (ambos = destino), pero
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
- El prelude aporta `trait Eq { fn eq(self, otro: Self) -> bool; }` (inyectado como los
  enums/funciones del prelude; se salta si el usuario define `Eq`).
- Por cada tipo con `@derive(Eq)`, el checker **sintetiza un `ImplBlock`** `impl Eq for T`
  con el método `eq`, y lo añade a `program.impls`. **El resto lo hace M9** (la bajada de
  M9.1 lo convierte en `T#igual`, etc.): `@derive` solo *genera el AST del impl*.
- El cuerpo de `eq`:
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

> **Por qué `eq` y no `==` para enums.** `==` ya compara structs estructuralmente, pero
> **no** enums (pueden ser recursivos / portar funciones; §M5). `@derive(Eq)` da una
> igualdad **explícita** (`a.eq(b)`) para enums, demostrando codegen sobre traits sin
> tocar la semántica de `==` (sobrecarga de operadores queda fuera de alcance).

**`@derive(Show)`** (limpieza post-M11, L2) — sobre un struct/enum **no genérico**, genera su
`impl Show` con `show(self) -> string` (trait `Show { fn show(self) -> string; }` en el
prelude). Misma mecánica que `@derive(Eq)` (sintetiza el `ImplBlock`, lo baja M9); se generaliza
`generate_derives`/`validate_derive` para ambos traits, y `@derive(Eq, Show)` genera los dos. El
cuerpo de `show` renderiza por tipo de cada campo/payload: primitivos vía `to_string`;
struct/enum vía `show()` recursivo (los anidados deben implementar Show). A diferencia de `Eq`,
**Show sí funciona para enums recursivos** (la recursión está en los datos, no impide `show()`).
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

#### 19.2g LSP con soporte de módulos (diagnósticos multi-archivo)

Hasta aquí el LSP analizaba **un archivo aislado** (lex→parser→checker sobre el buffer). En un
proyecto multi-archivo (M11.3) eso marcaba **errores espurios**: un `from geo import duplicar;`
daba "función 'duplicar' no declarada" porque nunca se corría el loader. Se arregla en los
**diagnósticos**:

- El loader gana `load_fuente(entry, fuente, dep_roots)`: como `load_con_deps` pero usa el **buffer
  en memoria** para el archivo de entrada (cambios sin guardar) y lee los imports **de disco**.
- `analizar_modular(uri, src)` corre el loader sobre el buffer, chequea el **programa fusionado** con
  `check_all` (multi-error) y publica solo los errores que caen en **este** archivo (banda de la
  entrada, `delta 0` → la línea global es la local; los de otros módulos pertenecen a sus URIs). Si
  el buffer no es un `file:` o no parsea, cae al análisis de un solo archivo (errores de sintaxis
  precisos). Si el loader falla con la entrada válida (import ausente, cápsula), un diagnóstico al
  inicio. `.ray-deps/` (M39c) se resuelve como en `ray run`, así que las dependencias también cuentan.

**Consultas (hover/def/references/rename) módulo-aware.** El mismo problema mataba TODO el hover de
un archivo con imports: el índice semántico (`semantic_index`) se construía sobre el buffer aislado,
el checker fallaba por el `import` y no recogía nada —ni siquiera los símbolos locales—. Se arregla
con `indice_para(uri, src)`: construye el índice sobre el **programa fusionado** del loader (delta 0
para la entrada → posiciones locales), con *fallback* al buffer aislado. Detalles:

- **hover** funciona en archivos multi-módulo: variables/params/funciones locales y también el tipo
  de un símbolo **importado** de otro módulo (muestra el nombre namespacado, `geo::area: fn(...)`).
- **Solapamiento por nombres namespacados**: un `geo::duplicar` registra un `len` mayor que el token
  `duplicar` de la fuente y solapaba al token siguiente. Se resuelve eligiendo el hover **más
  específico** (menor `len`) entre los que solapan la posición, y recortando el rango al identificador
  real de la fuente (`token_len`).
- **def/references/rename** filtran a la **banda de la entrada** (este archivo): una declaración en
  otro módulo aún no navega cross-archivo (def → sin resultado), y `rename` se **niega** en símbolos
  que cruzan módulos (los renombraría a medias) — solo renombra los que viven enteros en el archivo.

Diferido: navegación cross-archivo (def/references que salten a otro módulo) y completion de símbolos
`pub` de otros módulos (hoy completion es del buffer + prelude + builtins).

### 19.3 Deferido (más allá de M10.1)
- **LSP**: diagnósticos (M10.2 §19.2, **multi-archivo** §19.2g) + hover/definición (M10.2b §19.2b) +
  *find-references*/*rename*/*completion* (cluster 4) + **M10.2f**: hover/def de **tipos**, **signature
  help** y **completion por ámbito** + **M10.2g**: **hover de campos y métodos** (`p.x`, `xs.map()`,
  `n.metodo()`) — el AST `Field` no lleva la posición del nombre tras el `.`, así que el parser la registra
  en una side-table `field_name_pos` (clave `(línea,col,nombre)`, como `ufcs_sites`; el loader la desplaza)
  y el checker registra el hover del campo/método en `check_field`/`check_call` con su tipo/firma +
  **M10.2h**: **ir-a-definición cruzando archivos** — `LoadedModule` gana su `path`, y `definition_at`
  mapea la declaración (posición global del programa fusionado) a su **módulo** (archivo + línea local),
  devolviendo la `Location` en el archivo correcto (saltar a `geo.ray` desde `import geo;`) — y
  **find-references cruzando archivos**: `references_cross` recolecta las apariciones de todas las
  bandas del programa fusionado y mapea cada una a su módulo (archivo + línea local, largo recortado al
  token real con `token_len`); la declaración apunta al **nombre** (escaneando la fuente del módulo
  destino). Gotcha: `use_name` lee el identificador de la fuente (no `d.len`, que en un uso namespacado
  `geo::f` excede el token escrito) — y **rename cruzando módulos**: `WorkspaceEdit` multi-archivo con
  declaración + usos (todos los módulos) + los **especificadores de `from`-import** que traen el símbolo
  (el loader los expone en `Loaded.from_import_sites`, casados por el global de la decl vía
  `def_global_name`). **Gate de seguridad**: se exige que TODA posición contenga exactamente el nombre; un
  uso por **alias** (`as x`) o una referencia **calificada** tiene otro texto → el rename se **rechaza**
  (null) en vez de dejar el código a medias. Quedan: **def** del nombre de método (solo hover) y completion
  por **bloque** anidado / de símbolos `pub` de otros módulos. **Navegación cross-archivo (def/references/
  rename) COMPLETA.**
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
- **M11.7d — sort**: trait **`Ord`** en el prelude (`fn less(self, otro: Self) -> bool`), impl para
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

### 21.2 M12.1 — el slice CSP (especificación)

La primera sub-fase es un **corte vertical** del modelo: `spawn` de green threads + **canales tipados**
(`send`/`recv`/`close`), con un **scheduler cooperativo M:1 y determinista** en la VM. Es lo mínimo que
demuestra CSP de punta a punta (productor/consumidor comunicándose por un canal). Acotados/backpressure
→ M12.2; structured concurrency (scope + join) → M12.3; `select` → M12.4 (o diferido).

**Surface (decidida con el usuario):**
- `spawn(f)` — builtin que lanza `f` (una función de primera clase, `fn() -> T`) como green thread. Reusa
  las closures (cero gramática nueva, en el espíritu del proyecto). El resultado de `f` se **descarta** en
  M12.1 (el *join* con valor llega con structured concurrency, M12.3).
- `Channel<T>` — **tipo nuevo** (`Type::Channel(Box<Type>)`). El parser lo trae como `Struct("Channel",[T])`
  y el checker lo **reclasifica** en `resolve_type` (como `Map`).
- `channel() -> Channel<T>` — crea un canal. Es **indeterminado** (como `map_new()`/`[]`/`None`): su tipo
  lo fija el esperado por chequeo bidireccional (`check_expr_expected`; anotación o contexto). `T` no tiene
  restricción (a diferencia de la clave hashable de `Map`).
- `send(ch: Channel<T>, v: T) -> unit` — encola `v`. En M12.1 el canal es **no acotado** → `send` **nunca
  bloquea** (el acotado/rendezvous, donde `send` también es punto de yield, llega en M12.2).
- `recv(ch: Channel<T>) -> Option<T>` — `Some(v)` si hay valor; si el canal está **cerrado y vacío**,
  `None` (encaja con "ausencia como valor"). Si está **vacío y abierto**, **BLOQUEA** (punto de yield).
- `close(ch: Channel<T>) -> unit` — marca el canal cerrado y **despierta** a los receptores bloqueados (que
  recibirán `None`). `send` sobre un canal cerrado es error de ejecución (`panic`).
- UFCS gratis: `ch.send(v)`, `ch.recv()`, `ch.close()`.

**Scheduler (cooperativo, M:1, determinista):**
- Una **fibra** = `Fiber { frames: Vec<CallFrame>, stack: Vec<HeapValue> }` (el par que hoy son los campos
  de la VM). El `heap` es **compartido** entre fibras (un solo hilo → sin carreras por construcción).
- La VM gana una **cola de listas FIFO** (`ready: VecDeque<Fiber>`) y, por canal, una lista de **receptores
  bloqueados** (`waiters`). El programa arranca como la **fibra principal** (la de `main`).
- **Puntos de yield**: solo `recv` que bloquea (cola vacía y canal abierto) y la **terminación** de una
  fibra. `spawn` **no** cede (solo encola la fibra nueva); `send` **no** cede (no acotado). Determinismo: la
  `ready` es FIFO; un `send` que despierta a un receptor lo manda **al final** de `ready`.
- **Fin del programa**: cuando la **fibra principal** retorna (semántica Go) → el código de salida es el de
  `main`; las fibras pendientes se abandonan. Para tests deterministas, los programas hacen *join* por
  canales (la principal `recv` los resultados antes de terminar).
- **Deadlock**: si la fibra en curso bloquea, `ready` está vacía y aún hay fibras bloqueadas → error de
  ejecución ("deadlock: todas las fibras están bloqueadas").

**Runtime / GC:** el canal es un objeto del heap `Obj::Channel(VmChannel { queue: VecDeque<HeapValue>,
closed: bool, waiters })`, **trazado por el GC** (los valores en la cola son raíces). Cambio importante en
las **raíces del GC**: ya no basta con los `frames`/`stack` actuales; el `mark` debe rootear **todas las
fibras** (sus `frames`+`stack`) + las colas de los canales + las fibras en `ready`/`waiters`.

**Solo la VM (oráculo).** La concurrencia vive en la VM; el **intérprete** da un **error limpio** si topa
con `spawn`/`channel`/`send`/`recv`/`close` ("la concurrencia requiere la VM (`--vm`)") y sigue siendo el
oráculo de los programas **secuenciales**. Los programas **concurrentes** se corren con `--vm` y, como el
scheduler es **determinista**, su salida es fija → los tests comparan contra **salida esperada exacta** (no
hay oráculo cruzado VM↔intérprete para ellos). El checker acepta los builtins de concurrencia en ambos.

**Sub-fases de M12:** **M12.1** este slice (spawn + canales no acotados + scheduler determinista). **M12.2**
canales **acotados** (`channel(n)`; `send` bloquea al llenarse → backpressure; `send` pasa a ser punto de
yield). **M12.3** **structured concurrency** (un `scope`/`spawn` que posee y hace *join* de sus tareas, con
valor de retorno). **M12.4** `select` sobre varios canales (o diferido).

**M12.1 — estado: COMPLETO.** Implementado tal cual se especificó. Notas de implementación: una fibra es
`Fiber { frames, stack, is_main }`; la VM gana `ready: VecDeque<Fiber>` + `parked: Vec<Parked{chan, fiber}>`
+ `current_is_main`; opcodes `Spawn`/`ChannelNew`/`ChanSend`/`ChanRecv` (el cierre reusa `Close`, **ad-hoc
polimórfico** porque ya existía `close(h)` de handles de archivo de M11.8 y raylang no tiene sobrecarga);
canal = `Obj::Channel(VmChannel { queue, closed })` trazado por el GC; `collect` rootea TODAS las fibras +
el canal que cada *parked* espera. `recv` bloqueante guarda la fibra (su `ip` ya apunta tras `ChanRecv`) y
`wake_with` le deja el `[T]` en la pila al despertarla. Tests: `tests/concurrency_cli.rs` (productor/
consumidor, orden determinista con 2 productores, pipeline de fibras, closure capturada, `close`→`None`,
deadlock, `send` a canal cerrado, error limpio del intérprete). Ejemplo: `examples/concurrency/concurrencia.ray`.

### 21.3 M12.2 — canales acotados / backpressure (especificación)

M12.1 dejó los canales **no acotados**: `send` nunca bloquea (la cola crece sin límite). M12.2 añade
**capacidad** y, con ella, **backpressure**: un emisor que escribe más rápido de lo que el consumidor lee
acaba **bloqueándose** hasta que haya sitio. Es el segundo y último punto de yield del modelo (el primero,
`recv`, ya estaba) y completa la simetría productor↔consumidor de CSP.

**Surface (mínima, sin gramática nueva):**
- `channel() -> Channel<T>` — **no acotado** (como en M12.1; la cola crece sin límite, `send` nunca
  bloquea). Sin cambios.
- `channel(n) -> Channel<T>` — **acotado** con capacidad `n` (un `int` ≥ 0). `channel(0)` es un canal
  **síncrono / rendezvous**: `send` se completa solo cuando hay un `recv` esperando (cola de tamaño 0).
  El tipo de elemento sigue **indeterminado** (lo fija el esperado, como `channel()`); la capacidad es un
  valor de runtime, no parte del tipo (`Channel<T>` no lleva la capacidad).
- `send`/`recv`/`close` sin cambios de firma.

**Semántica del scheduler (extiende M12.1):**
- **`send` pasa a ser punto de yield.** Al enviar: (1) si hay un **receptor bloqueado** en el canal,
  entrega directo y lo despierta (rendezvous; ya en M12.1); si no, (2) si la cola **tiene hueco** (no
  acotado, o `len < cap`), encola y sigue; si no, (3) la cola está **llena** → el emisor se **bloquea**
  (se aparca con su valor) y se conmuta de fibra. Backpressure.
- **`recv` despierta a un emisor bloqueado.** Al recibir, tras liberar un hueco: si hay un **emisor
  bloqueado** en ese canal, su valor entra a la cola (ya hay sitio) y se le despierta. Con `cap = 0`
  (cola siempre vacía) el `recv` toma el valor **directo** del emisor aparcado y lo despierta.
- **Determinismo:** las fibras aparcadas se sirven **FIFO** (la que se bloqueó antes despierta antes);
  un emisor despertado vuelve **al final** de `ready` (igual que un receptor en M12.1).
- **`close` con emisores bloqueados = error de ejecución** en el sitio del `close` ("close sobre un canal
  con un emisor bloqueado"): cerrar un canal del que alguien todavía espera enviar es un error de
  programa, y a diferencia de "panic en otra fibra" sí es detectable y determinista en el `close`. Cerrar
  despierta a los **receptores** con `None` (como en M12.1).
- **Deadlock:** sin cambios — si la fibra en curso bloquea, `ready` está vacía y aún hay fibras aparcadas
  (receptores **o** emisores) → deadlock.

**Runtime / GC:** `VmChannel` gana `cap: Option<usize>` (`None` = no acotado; `Some(n)` = capacidad `n`,
con `n = 0` rendezvous). Una fibra aparcada distingue **receptor** de **emisor** (`Waiting::Recv` /
`Waiting::Send(valor)`); el `valor` que sostiene un emisor bloqueado es una **raíz del GC** nueva (en
M12.1 las aparcadas no sostenían ningún valor). Dos opcodes para crear: `ChannelNew` (no acotado, 0 args)
y `ChannelNewBounded` (saca la capacidad de la pila); el compilador elige según haya argumento (es el
mismo *special-case* que ya tiene `channel` en el checker por ser indeterminado).

### 21.4 M12.3 — structured concurrency (especificación)

M12.1/M12.2 dieron `spawn` "dispara y olvida" (estilo Go): la fibra corre suelta y su resultado se
descarta; si una fibra falla, hoy aborta todo. M12.3 trae el modelo **estructurado** (Trio/Kotlin): las
tareas tienen **valor de retorno**, un ámbito **posee** las que se lanzan dentro y **las une** al salir
(ninguna se fuga), y el **fallo de una hija se propaga** al punto de unión en vez de perderse o tumbar el
proceso. Cierra el "qué pasa con el resultado/el error de una fibra".

**Surface (builtins + closures; cero gramática nueva, en el espíritu del proyecto):**
- `Task<T>` — **tipo nuevo** (`Type::Task(Box<Type>)`), como `Channel<T>`: el parser lo trae como
  `Struct("Task",[T])` y el checker lo reclasifica en `resolve_type`.
- `spawn(f: fn() -> T) -> Task<T>` — **cambia su firma**: ya no devuelve unit, sino un **handle tipado**
  a la tarea. Retrocompatible: `spawn(fn() { … });` como sentencia descarta el `Task` (M12.1/M12.2 siguen
  compilando). Si hay un `scope` activo en la fibra que llama, la tarea queda **adscrita** a él.
- `join(t: Task<T>) -> T` — **bloquea** hasta que la tarea termina y devuelve su valor (punto de yield).
  Si la tarea **falló** (panic), `join` **re-lanza** ese fallo (propagación).
- `scope(body: fn() -> R) -> R` — corre `body` en la fibra actual; toda tarea lanzada **mientras el scope
  está activo** (adscripción **dinámica** a la fibra, no léxica) queda poseída por él. Al **retornar
  `body`**, el scope **une a todas** sus tareas (espera a que terminen) y, si alguna falló, **propaga** ese
  fallo; si no, devuelve `R`. Garantiza que **ninguna tarea sobrevive al scope**.
- UFCS gratis: `t.join()`.

**Tipado:** son builtins con regla de tipado en el registro único (`src/builtins.rs`): `spawn` toma
`fn() -> T` y da `Task<T>`; `join` toma `Task<T>` y da `T`; `scope` toma `fn() -> R` y da `R` (el tipo del
scope = el del cuerpo). `Task<T>` se integra como `Channel<T>` en `subst`/`unify`/`ensure_type`/etc.

**Runtime (solo VM):**
- `Task<T>` = objeto del heap `Obj::Task(VmTask { state })` con `state ∈ {Pending, Done(valor),
  Failed(msg)}`. La fibra de una tarea guarda **su** handle (`Fiber.task`); al terminar **normal** escribe
  `Done(resultado)`, al **fallar** escribe `Failed(mensaje)`; en ambos casos despierta a los que esperan.
- **Bloqueo en `join`/scope:** una fibra que espera una tarea pendiente se aparca (`Waiting::Join`); al
  completarse la tarea se la despierta y **re-ejecuta** el opcode (el `ip` se rebobina al `Join`/`ScopeEnd`,
  como el re-intento de `recv`). El **scope** se realiza con dos opcodes que el compilador intercala
  alrededor de la llamada al cuerpo —`ScopeBegin` (apila un marco de scope en la fibra) · `<cuerpo>()` ·
  `ScopeEnd` (espera a los hijos pendientes uno a uno; al estar todos, propaga el primer `Failed` o deja
  `R`)— igual que el *special-case* de `channel` (no usa el opcode de la tabla).
- **Propagación de fallos:** el bucle de la VM captura el error de una fibra **hija** (no `main`, con
  frames activos) en su `Task` como `Failed` y planifica la siguiente, en vez de abortar; los errores de
  `main` y los del scheduler (deadlock) siguen abortando. Un `Failed` se re-lanza en el `join`/`ScopeEnd`
  que lo observe → la propagación encadena hacia arriba (si llega a `main`, aborta). Un panic en una tarea
  **ni unida ni dentro de un scope** se pierde (la disciplina estructurada es usar `scope`/`join`).
- **Estado por fibra:** cada `Fiber` guarda `task: Option<Handle>` y `scopes: Vec<ScopeFrame>` además de
  `frames`/`stack`/`is_main`; la VM espeja `current_task`/`scopes` y los salva/restaura al conmutar.
- **GC multi-raíz:** se añaden a las raíces el `Done(v)` de cada `Task`, el handle de tarea que espera un
  joiner aparcado, y los hijos de cada `ScopeFrame` (en la fibra en curso, las listas y las aparcadas).

**Determinismo / deadlock:** FIFO intacto; el deadlock cubre ahora también a los joiners (si todas las
fibras quedan bloqueadas esperando canal **o tarea**). Tests por **salida esperada exacta** (solo VM; el
intérprete da error limpio en `spawn`/`join`/`scope`).

**Diferido:** **cancelación** de hermanas cuando una falla (Trio cancela el resto; raylang no tiene
primitivo de cancelación → si el **cuerpo** del scope hace panic, las tareas en curso quedan huérfanas en
vez de cancelarse) → puerta abierta. `select` → M12.4.

**M12.3 — estado: COMPLETO.** Implementado según la spec. Notas: `join` resultó **ad-hoc polimórfico**
(colisión con el `join(arr,sep)` de strings de M11.7a; raylang no tiene sobrecarga) → un builtin que
ramifica por tipo + el compilador elige el opcode por **aridad** (1 = `TaskJoin`, 2 = `Join` de string).
`scope(body)` se baja a `ScopeBegin; body(); ScopeEnd` (special-case del compilador como `channel`; la
llamada al cuerpo no está en cola → el TCO no la toca). `join`/`ScopeEnd` que bloquean **rebobinan el ip**
y re-ejecutan al despertar (TaskJoin re-empuja el handle). La **captura** del fallo de una hija se hace
corriendo cada instrucción en un **cierre** dentro del bucle de la VM (el error de una fibra hija con
frames activos → su `Task`; los de `main`/scheduler con frames vacíos → abortan). Tests:
`tests/concurrency_cli.rs` (scope+join con valor, auto-join, propagación por join y por scope, scope de
varias tareas). Ejemplo: `examples/concurrency/structured.ray`.

### 21.5 M12.4 — `select` sobre varios canales (especificación)

Con `recv` esperas a UN canal; `select` espera al **primero de varios** que esté listo. Es la última pieza
del modelo CSP (multiplexar canales: un consumidor que atiende varias fuentes, *timeouts* como un canal
más, etc.). Cierra M12.

**Surface (mínima, sin tipos ni tuplas nuevas):**
- `select(chs: [Channel<T>]) -> int` — bloquea hasta que **algún** canal de la lista esté **listo para
  recibir** (tiene un valor en cola, tiene un emisor bloqueado, o está cerrado) y devuelve el **índice** del
  primero listo (determinista: el de menor índice). Luego haces `recv(chs[i])` para tomar el valor (o
  detectar el cierre con `None`). Es seguro: entre el `select` y el `recv` **no hay punto de yield** (M:1
  cooperativo), así que la disponibilidad se mantiene. Los canales son **homogéneos** (`Channel<T>`); la
  variante índice+valor (`Selected<T>`) y el `select` de *send* quedan como azúcar/extensión futura.
- UFCS gratis: `chs.select()`.

**Semántica (scheduler):** `select` es un punto de yield más. Al evaluarlo: escanea la lista en orden; si
algún canal está listo (cola no vacía ∨ cerrado ∨ con emisor bloqueado) → devuelve su índice; si ninguno →
**bloquea** la fibra esperando al **conjunto** de canales (`Waiting::Select`, con el handle del arreglo en
`Parked.on`), rebobina el `ip` y re-ejecuta el `select` al despertar. Se la **despierta** cuando cualquiera
de sus canales pasa a estar listo: al **encolar** un valor (`send`), al **bloquearse un emisor** (rendezvous)
y al **cerrar** el canal (`wake_select_waiters(chan)` recorre los `Select` aparcados cuyo arreglo contiene
ese canal). Un despertar **espurio** (otra fibra consumió el valor antes) se reabsorbe: al re-ejecutar, el
`select` no halla nada listo y se vuelve a bloquear. **Determinismo:** menor índice listo; los aparcados se
despiertan en orden FIFO. **Prioridad:** un `send` entrega antes a un `recv` plano bloqueado que a un
`select` (que solo ve el valor vía la cola) — política determinista, documentada.

**Runtime / GC:** ningún objeto nuevo (reusa `Obj::Channel`). `Waiting` gana `Select`; el `Parked.on` de un
selector es el **handle del arreglo** de canales → el GC lo rootea (y con él, transitivamente, los canales).
Solo la VM; el intérprete da error limpio en `select`. Tests por **salida esperada exacta**.

**M12.4 — estado: COMPLETO.** Implementado según la spec. Diferido (puertas abiertas): `Selected<T>`
(índice+valor en un paso, azúcar del prelude) y `select` de operaciones de **send**.

### 21.6 M12.5 — cancelación de hermanas (especificación)

M12.3 dejó el `scope` con una semántica de fallo **incompleta**: al unir, esperaba a TODAS las tareas y
solo después propagaba el fallo de una; y si el **cuerpo** del scope fallaba, las tareas en vuelo quedaban
**huérfanas**. La concurrencia estructurada de verdad (Trio/Kotlin) **cancela a las hermanas** en cuanto una
falla: no tiene sentido seguir trabajando si el resultado ya se va a descartar. M12.5 lo añade.

**Sin superficie nueva** — es un cambio de **semántica** (automático, como Trio): el usuario no escribe
nada nuevo; el `scope` simplemente cancela mejor. Solo toca el runtime de la VM.

**Cancelar, en un scheduler cooperativo M:1, es trivial:** una fibra solo corre en los puntos de yield, así
que "cancelarla" es **quitarla** de `ready`/`parked` y no reanudarla nunca (sus marcos/locales los reclama
el GC; raylang no tiene finalizadores). `cancel_task(t)` marca la `Task` como `Failed("tarea cancelada …")`,
saca su fibra de `ready`/`parked` y **cancela recursivamente** los hijos de los scopes de esa fibra (una
hermana cancelada que a su vez era dueña de un scope no deja **nietos** huérfanos → cancelación transitiva).

**Dónde se dispara:**
- En `ScopeEnd`: antes de bloquearse esperando a una hija pendiente, **escanea** los hijos. Si alguno
  **falló**, captura ese fallo (el original), **cancela** a las hermanas que sigan pendientes y propaga el
  fallo original de inmediato — en vez de esperar a que terminen (M12.3). Es el cambio central.
- En `fail_current_fiber`: cuando una fibra **hija** falla teniendo scopes activos (su cuerpo hizo panic con
  tareas en vuelo), cancela los hijos de esos scopes antes de descartarla (cierra el "cuerpo falla →
  huérfanas" para fibras no-main; en `main` el programa aborta y el punto es discutible).

**Determinismo:** se propaga el **primer** fallo en orden de declaración de los hijos. **Limitaciones
(documentadas):** la cancelación es **cooperativa** —no interrumpe el *cuerpo* del scope a mitad de
ejecución, ni a una hermana que esté corriendo CPU sin ceder—; solo retira a las que están en `ready` o
aparcadas. El caso patológico "el `ScopeEnd` espera a una hija que se bloquea para siempre mientras otra
falla" degrada a **deadlock** (termina con error, no cuelga), no a propagación del fallo.

**M12.5 — estado: COMPLETO.** Runtime intacto salvo la VM (cero opcodes nuevos; reusa `TaskState::Failed`).
Tests en `tests/concurrency_cli.rs`. Con M12.5, **M12 (concurrencia) queda COMPLETO**. Diferido:
cancelación **preemptiva** (interrumpir el cuerpo / una hermana en CPU), `Selected<T>`, `select` de send,
y un primitivo de cancelación **explícito** (`cancel(t)`).

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
  **diferido** (no es *printable*; Display ordena por clave → determinista). Ejemplo `examples/data/mapa.ray`.
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
  (mensaje genérico) y `assert_eq<T: Eq + Show>(a, b)` (mensaje con ambos valores, vía `.eq()` +
  `.show()`; los bounds se bajan a diccionarios M9.2). **Sin sobrecarga** en raylang, así que en
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
  Oráculo: snippets + `examples/data/enums.ray` y `examples/data/match_figuras.ray` reales. **Gotcha**: un
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
- **M14.3c-1 COMPLETO** — genéricos: funciones. `selfhost/checker.ray` gana funciones genéricas
  (`fn id<T>(x: T) -> T`): `FnSig.type_params`; `Checker.tparams` lleva los parámetros de tipo rígidos
  en ámbito (un `TNamed(T, [])` con `T` ahí es un tipo VÁLIDO, no desconocido —`ensure_type` lo
  acepta—); `unify`/`subst`/`unify_list` (la maquinaria de inferencia: las incógnitas son las variables
  de tipo de la firma llamada, pasadas como `holes`); `check_generic_call` infiere los parámetros
  unificando params↔args y devuelve el retorno sustituido (mensajes byte-idénticos: inferencia fallida,
  inconsistencia `'T' no puede ser X y Y a la vez`, aridad). `check_unique_tparams` rechaza parámetros
  repetidos; `type_arity` valida la aridad de args de tipo en `ensure_type`. El `Type` del parser
  **dobla** como variable de tipo (`TNamed(T,[])`): dos `T` son iguales por nombre (`type_eq`), así los
  cuerpos genéricos (`[a, b]`, `f(x)`, `xs[i]`) cuadran sin código nuevo. Oráculo: 4 válidos + 5 errores
  + `genericos.ray`. Diferido a c-2: tipos genéricos (structs/enums) + bidireccional completo; a c-3:
  prelude Option/Result + `?`.
- **M14.3c-2 COMPLETO** — genéricos: tipos (structs/enums) + bidireccional completo. `check_struct_lit`/
  `check_enum_lit` ganan inferencia de args de tipo (`seed_sigma_from_expected` siembra σ del esperado,
  `unify` con el payload/campo, `finalize_type_args` exige que cada parámetro quede determinado; mismos
  mensajes: `'A' no puede ser X y Y a la vez`, `no se pudo inferir el parámetro de tipo 'T' … anota el
  tipo`). `check_field` SUSTITUYE el tipo del campo con los args del objeto (`Par<int,bool>.primero` es
  int); `check_match` arma el `enum_sigma` (params del enum ↔ args del escrutinio) y `check_pattern`
  sustituye el payload de los bindings. **Bidireccional completo**: `check_expr_expected` propaga el
  esperado a literal de struct, construcción de enum (`Caja.Vacia`), `if` y `match` (helpers
  `check_expr_opt`/`check_block_opt`); `check_call`/`check_call_field`/`check_field_or_enum` llevan
  `expected: Option<Type>`. `type_has_var`/`check_value_against` deciden si el esperado es concreto.
  `ensure_type` valida la aridad de args de tipo (`type_arity`). El monomórfico (M14.3b) es el caso
  σ-vacía → mensajes idénticos. Oráculo: 4 válidos + 5 errores + `tipos_genericos.ray`/`opcional.ray`.
  Diferido a c-3: prelude Option/Result + `?`.
- **M14.3c-3 COMPLETO — M14.3c COMPLETO** — prelude (Option/Result) + `?`. `inject_prelude` registra
  `Option<T>` y `Result<T, E>` como enums genéricos conocidos: en Rust el prelude se PARSEA de un
  fuente raylang compartido, pero el checker auto-alojado es un VALIDADOR que solo recibe el AST, así
  que se registran sus definiciones **directamente** (mismo efecto para el veredicto); si el usuario
  declara un tipo con ese nombre, su definición gana (override del prelude). Reusa toda la maquinaria
  genérica de c-2: `Result.Ok(x)` infiere/siembra T,E del tipo esperado, `match` sustituye el payload.
  El operador `?` (`check_try`, nodo `ETry`): el operando debe ser `Result<T,E>`/`Option<T>` y la
  función envolvente declarar un retorno compatible (`Result<_,E>` con la misma E, o `Option<_>`);
  devuelve el valor desempaquetado (mensajes byte-idénticos). Oráculo: 4 válidos + 5 errores +
  `errores.ray`. Diferido a M14.3d: traits/impls/bounds/`@derive`/`dyn`/UFCS-métodos y el resto del
  prelude (Eq/Show/Ord, map/filter/fold).
- **M14.3d-1 COMPLETO** — UFCS + funciones anónimas. `check_call_field` resuelve `recv.f(args)` por
  orden: (1) construcción de enum, (2) **campo** del struct receptor de tipo función (gana sobre UFCS),
  (3) **UFCS** (`check_ufcs`): `f(recv, args)` con el receptor como primer argumento, reusando
  `check_named_call` (builtins/función libre/genérica → el receptor cuenta para la inferencia). Si
  `name` no es llamable, el error habla de UFCS (`no existe campo ni función '…' aplicable a …`).
  Helpers `struct_field_type` (campo sustituido, o `None`), `name_is_callable`/`is_known_builtin`.
  Funciones anónimas (`check_func_expr`, nodo `EFunc`): cierre con captura (el cuerpo ve los ámbitos
  envolventes; `current_return` se guarda/restaura), devuelve `fn(params) -> R`. Oráculo: 6 válidos +
  3 errores + `ufcs.ray`/`closures.ray`. Diferido a d-2: traits/impls (despacho estático, `Self`,
  métodos por defecto).
- **M14.3d-2 COMPLETO** — traits + impls (despacho estático, `Self`, métodos por defecto). `Checker`
  gana `traits` (nombre → firmas) y `methods` (`Tipo#metodo` → FnSig). `register_traits_impls` (tras
  registrar firmas, antes de los cuerpos): valida traits (nombres únicos, métodos únicos), y cada impl
  CONCRETO (cobertura sin faltantes salvo defectos, sin métodos extra/repetidos, firmas que casan vía
  `check_method_sig` con `Self`→target). La **tabla de métodos** se puebla con la FnSig de cada método
  (params con `Self`→target, self incluido) — `method_fnsig`; los **defectos** no redefinidos también
  (su FnSig sale de la firma del trait). `check_impl_bodies` (tras los cuerpos de función) verifica cada
  cuerpo de método como una función con `self` del tipo concreto (los defectos heredados se chequean por
  impl). **Resolución** en `check_call_field`: campo → **método de trait** (`type_key_of(recv)`,
  `mangle`) → UFCS; un método aparece en los errores con su nombre manglado (`'Tipo#m'`), como en Rust.
  `ensure_impl_target` valida objetivos concretos (struct/enum no genérico, primitivo); genéricos/bounds
  → d-3. Helpers `subst_self`/`type_key_of`/`mangle`/`has_default`. Oráculo: 6 válidos + 7 errores +
  `traits.ray`. Diferido a d-3: bounds + impls genéricos.
- **M14.3d-3a COMPLETO** — bounds en funciones. `FnSig.bounds`, `Checker.bounds` (bounds en ámbito) y
  `Checker.impl_traits` (`Tipo#Trait`→sí, qué tipos implementan qué traits; lo puebla `register_impl`).
  `check_bounds` valida los bounds de una función (param real + trait existente). **Resolución de método
  por bound** (`resolve_bound_method`, paso 3b de `check_call_field`): `x.metodo()` con `x: T` y `T:
  Trait` → busca el trait acotado que declara el método (ambigüedad = error), valida los argumentos
  (sin el receptor) contra la firma con `Self`→T y devuelve el retorno. **Satisfacción en el sitio de
  llamada** (`check_call_bounds`/`check_bound_satisfied`, tras inferir σ en `check_generic_call`): cada
  parámetro acotado debe resolver a un tipo concreto con impl del trait (`impl_traits`) o a un parámetro
  rígido del llamador con el mismo bound (**reenvío del diccionario**); si no → `{T} no implementa
  '{Trait}' (requerido por la llamada)`. El paso de diccionarios (lowering) se OMITE; solo va la
  satisfacción (el veredicto). Oráculo: 4 válidos + 4 errores + `bounds.ray`/`metodos_por_defecto.ray`.
  Limitación: un cuerpo de método por defecto INVÁLIDO reporta su posición original (Rust renumera el
  clon) — solo afecta a errores contrived (colisión campo/método). Diferido a d-3b: impls genéricos.
- **M14.3d-3b COMPLETO — M14.3d-3 COMPLETO** — impls genéricos (`impl<T: B> Trait for Caja<T>`).
  `ensure_generic_impl_target` valida el objetivo genérico (aridad + el objetivo aplicado EXACTAMENTE a
  los propios parámetros del impl, `Var` distintos). Idea central (como Rust): el método de un impl
  genérico **es una función genérica acotada** — `method_fnsig` hereda `type_params`/`bounds` del impl,
  así su FnSig (`Caja#medir<T: Medir>(self: Caja<T>)`) se resuelve con la misma `check_generic_call`
  (inferencia + satisfacción de bounds) que cualquier función genérica — cero código nuevo de
  resolución. `call_method` ramifica: concreto → `check_args`, genérico → `check_generic_call`.
  `check_impl_bounds` valida los bounds del impl; `check_impl_bodies` pone `type_params`/`bounds` del
  impl en ámbito (el cuerpo usa `T`: `self.contenido.medir()` con `T: Medir`). La satisfacción anidada
  (`Caja<Caja<int>>`, pasar `Caja<int>` a otro genérico) la cubre el `impl_traits` por constructor
  (shallow) — el diccionario anidado (closure) es lowering, omitido; el corpus válido no genera falsos
  positivos. **M14.3d-3 COMPLETO.** Oráculo: 4 válidos + 3 errores + `impls_genericos.ray`. Diferido a
  d-4: dyn + @derive + resto del prelude (Eq/Show/Ord, map/filter/fold).
- **M14.3d-4a COMPLETO** — trait objects (`dyn Trait`, despacho dinámico). `ensure_type(TDyn)` valida
  el conjunto (cada trait existe, ningún método repetido entre ellos). **Coerción** concreto→objeto
  (`coerce_to_dyn`, en `check_expr_expected` cuando se espera `dyn` y la expr no propaga): el concreto
  implementa **todos** los traits (`impl_traits`); `dyn`→`dyn` idéntico (no-op) o a un subconjunto
  (**upcasting**, olvidar traits) — si no es subconjunto, error. **Despacho** `obj.m(args)` con
  `obj: dyn` (`dispatch_dyn_method`, paso 1.5 de `check_call_field`): busca el método entre los traits
  del conjunto, exige *object-safety* (`Self` solo en el receptor; `method_uses_self`/`type_uses_self`),
  valida los argumentos (sin el receptor) y devuelve el retorno. Helpers `propaga_esperado`/
  `subset_strs`/`find_dyn_method`. Mensajes byte-idénticos. La síntesis del struct vtable (lowering) se
  OMITE. Oráculo: 4 válidos + 5 errores + `trait_objects.ray`. Diferido a d-4b: `@derive` + resto del
  prelude (Eq/Show/Ord, map/filter/fold).
- **M14.3d-4b COMPLETO** — prelude de orden superior (map/filter/fold). `inject_prelude_fns` registra
  las FIRMAS de `map<T,U>`/`filter<T>`/`fold<T,A>` en `c.funcs` (en Rust se parsean del prelude; el
  validador solo necesita la firma para resolver llamadas). `inject_fn` salta las que el usuario
  redefina (override). Compone con UFCS (`xs.map(f)`) y pipelines (`xs |> map(f)`) — el receptor cuenta
  para la inferencia genérica — y con closures inline. Oráculo: 4 válidos + 1 error + `stdlib.ray`.
  Diferido a d-4c: Eq/Show/Ord + impls de primitivos + `@derive` + anotaciones.
- **M14.3d-4c COMPLETO — M14.3d COMPLETO — M14.3 COMPLETO** — `@derive` + Eq/Show/Ord + anotaciones.
  `inject_prelude_traits` registra los traits `Eq`/`Show`/`Ord` (firmas) y sus impls para primitivos
  (`int#igual`, `int#mostrar`, …, en `methods`+`impl_traits`). **`@derive(Eq, Show)`** (`generate_derives`/
  `validate_derive`): sobre struct/enum no genérico, registra los métodos derivados (`eq`/`show`) +
  `impl_traits` (idempotente: no pisa un impl existente); NO genera/chequea el cuerpo (es codegen
  conocido → un campo no derivable, p. ej. función, no se detecta: limitación). **`check_annotations`**:
  `@test` solo en funciones `() -> bool`/`() -> unit` (sin args/params), `@derive` solo en tipos, otras
  son desconocidas (mensajes byte-idénticos). Así un tipo derivado satisface `T: Eq` y responde a
  `.eq()`/`.show()`. Oráculo: 6 válidos + 8 errores + `anotaciones.ray`. **El checker auto-alojado
  valida el LENGUAJE COMPLETO** (núcleo + datos + genéricos + traits/impls/bounds/dyn + prelude + derive),
  con veredicto byte-idéntico a Rust sobre 22 ejemplos reales + ~80 casos. Diferidos (no en el corpus):
  `Map` en el checker, satisfacción de bounds anidada profunda, posición de cuerpos de defecto inválidos,
  prelude más allá de map/filter/fold (sort/assert/get/parse_int). **Siguiente: el back-end (ejecución/
  lowering) para cerrar el self-hosting.**

### 23.5 M14.4 — El back-end (intérprete auto-alojado, diseño)

La fase que **cierra el self-hosting**: ejecutar el AST validado. Tras lexer→parser→checker (todos en
raylang), el back-end es lo único que falta para que raylang procese raylang **de punta a punta**.

**Decisión 1 — el motor: el intérprete tree-walking, no la VM.** Rust tiene dos motores; portamos el
**intérprete** (`src/interpreter.rs`, ~1657 líneas: el oráculo, el motor simple) y dejamos la VM
(compilador + pila + GC, ~3960 líneas) como hito **posterior y opcional** (M14.5). Es el mismo orden
pedagógico del proyecto original (M1 intérprete → M2 VM): el intérprete es la referencia; la VM se
valida contra él. Para cerrar el self-hosting basta un motor.

**Decisión 2 — resolución en runtime (despacho dinámico), NO lowering.** Aquí el back-end auto-alojado
**diverge a propósito** de la arquitectura de Rust. El checker de Rust hace dos trabajos (validar +
*bajar*: UFCS→llamada, métodos→funciones mangladas, bounds→diccionarios, `dyn`→struct-vtable,
construcción de enum→`EnumLit`); su intérprete recibe un AST **ya bajado** y es *tonto* sobre
traits/genéricos (erasure por lowering). Nuestro **checker auto-alojado es solo un validador** —omitió
todo ese lowering (§23.4)—, así que el AST que llega al back-end **no está bajado**. En vez de
reproducir M9 (la parte más intrincada: diccionarios, síntesis de `dyn`, renumerado de fn-exprs), el
intérprete auto-alojado **resuelve el despacho en tiempo de evaluación**, mirando la **etiqueta del
valor** en runtime:

- **Construcción de enum** `Enum.Variante(args)` (llega como `Field`/`Call`): si el nombre es un enum
  conocido con esa variante → construye `VEnum`. Sin tipos: solo consulta las tablas del programa.
- **UFCS / métodos** `recv.f(args)`: evalúa `recv`; si es un struct y `f` es un **campo** que contiene
  una función → la llama; si `f` es una **función libre** → `f(recv, args)`; si hay un **método**
  `Tipo#f` para el tipo *en runtime* de `recv` → lo llama (tabla de métodos `(clave_tipo, método) →
  función`, poblada desde los `impl`).
- **`dyn`, bounds, genéricos → no-ops.** Como el despacho mira la etiqueta del valor concreto, un valor
  "coercionado" a `dyn Trait` es **el propio valor concreto** (sin vtable), y `obj.m()` despacha por su
  tipo en runtime. Igual los bounds (`x.m()` con `x: T: Trait` despacha por el tipo concreto de `x`, sin
  pasar diccionarios) y los genéricos (el intérprete **nunca consulta tipos**). **El borrado (erasure)
  ocurre solo**, sin una pasada de lowering — es la consecuencia elegante de la decisión.

El precio es la divergencia con el intérprete de Rust (que es *tonto* porque el lowering ya pasó; el
nuestro es *dinámico*). Pero el **oráculo es conductual** (ver Decisión 3), no estructural: comparamos
comportamiento, no el AST ni la arquitectura interna → la divergencia es invisible al oráculo.

**Por qué es factible — el back-end cabalga sobre el runtime del host.** Self-hostear un intérprete
suena circular (¿un intérprete necesita su propio GC, su propia gestión de memoria?). No: el `Value`
del intérprete auto-alojado es un **enum de raylang**, que vive en el heap de la **VM anfitriona** y lo
recolecta **su** GC. Y la semántica de referencia + mutación que en Rust pide `Rc<RefCell<…>>` la dan
**gratis** los tipos de raylang (M3): un arreglo/struct de raylang YA es un objeto del heap compartido
por referencia. En concreto, el `Cell` compartido que Rust usa para que una closure capture una variable
**por referencia** (M4.2) se modela con un **arreglo de longitud 1** (o un struct de un campo): varias
closures que comparten esa celda comparten el mismo arreglo → mutar `cell[0]` lo ven todas. **No
reimplementamos ni GC ni celdas**: ese fue justo el habilitador que faltaba, y por eso el back-end es la
última pieza y no una imposible.

**Decisión 3 — oráculo conductual (stdout + código de salida).** Las fases previas comparaban **texto
canónico** de su salida (tokens, dump del AST, veredicto). El back-end compara lo que ya **observa el
usuario final**: la **salida estándar** del programa (vía `print`) y su **código de salida** (el `int`
que devuelve `main`). Para cada `.ray` del corpus se ejecuta por los dos pipelines —Rust (`cargo run --
prog.ray`) y raylang (`raylang selfhost/run.ray prog.ray`, el driver `selfhost/run.ray` que
lex→parse→check→**ejecuta**)— y se comparan stdout+exit. Corpus = los **ejemplos deterministas** (la
mayoría de los 35). Los builtins de **I/O no determinista** (`input`/`args`/`read_file`/reloj…) se
excluyen del oráculo automático (como ya se hace en `tests/io_cli.rs`); el resto del runtime sí.

**Representación del `Value`** (espejo del de Rust, como enum de raylang):
```
enum Value { VInt(int), VFloat(float), VBool(bool), VStr(string), VChar(char), VUnit,
             VArray([Value]),              // semántica de referencia: el [Value] del host
             VStruct(string, Map<string,Value>),  // nombre + campos mutables
             VEnum(string, string, [Value]),      // enum, variante, payload
             VFunc(int),                   // función nombrada/anónima por índice (tabla de funciones)
             VClosure(int, [Captura]),     // índice + entorno capturado (celdas compartidas)
             VMap(Map<MapKey,Value>) }
```
Las **celdas** del ámbito (`struct Cell { v: Value }` o `[Value]` de longitud 1) hacen compartible cada
variable; el ámbito es `Map<string, Cell>` y la pila de ámbitos un `[Map<string,Cell>]`, igual que Rust.

**Flujo y control.** El intérprete de Rust señaliza con `enum Flow { Return, Error, TailCall }` propagado
como `Result<Value, Flow>`. En raylang se modela igual (un enum `Flow` + `?`/`match` para propagarlo).
**Recursión profunda y TCO**: el host ya da **TCO** (M13.3b) y una **pila grande** (M13.3a), así que el
intérprete auto-alojado hereda robustez; para casar la frontera de `MAX_CALL_DEPTH` se replica el conteo
de profundidad (o se acepta como diferido, fuera del corpus). El **trampolín de cola** se porta si hace
falta para que recursiones de cola profundas no desborden el intérprete *anfitrión*.

**Sub-fases** (varios commits cada una, como el checker):
- **M14.4a — núcleo**: `Value` (primitivos + unit), aritmética/comparación/lógica, variables (celdas,
  ámbitos, mutación), `if`/`while`/`block`/`return`, llamadas a funciones nombradas, recursión, builtin
  `print`. El armazón (tabla de funciones, `call_body`, `eval_expr`/`eval_stmt`, `Flow`). Cubre
  fib/fizzbuzz/gcd/primes. La más grande.
- **M14.4b — datos**: arreglos (`[T]`, índice, `len`/`push`), structs (literal, acceso/asignación de
  campo, semántica de referencia), enums (**construcción reconocida en runtime**, `match` + patrones,
  payload). Builtins de string/arreglo/Map del runtime.
- **M14.4c — funciones de primera clase**: funciones anónimas, **closures** (captura por celda
  compartida), funciones como valor, orden superior (`map`/`filter`/`fold` ejecutándose de verdad).
- **M14.4d — despacho dinámico**: tabla de métodos desde los `impl`, **UFCS/métodos** en runtime,
  métodos por defecto, `dyn` (despacho por tipo concreto, sin vtable), `@derive` (igual/mostrar), bounds
  (no-op). Aquí "vive" la Decisión 2. La más sutil.

Tras el intérprete, el self-hosting está **cerrado** (raylang lexea/parsea/chequea/ejecuta raylang). La
**VM auto-alojada** (M14.5) quedaría como capstone-del-capstone opcional.

- **M14.4a COMPLETO** — núcleo. `selfhost/interpreter.ray` ejecuta el AST validado y devuelve
  `Result<Value, RuntimeError>`. `Value` es un enum de raylang (M14.4a: primitivos `VInt`/`VFloat`/
  `VBool`/`VStr`/`VChar`/`VUnit`); el flujo se modela con `enum Flow { FReturn, FError }` propagado por
  el canal de error de `Result` (`?`), como el `Flow` de Rust (sin `TailCall`: TCO diferido). El estado
  es un `struct Interp { funcs: Map<string,Func>, scopes: [Map<string,Value>] }` mutado por referencia
  (como `Checker`/`Parser`/`Lexer`). Cubre: literales, aritmética/comparación/lógica (con **cortocircuito**
  de `&&`/`||`, división/módulo por cero como error de ejecución), variables (`define`/`lookup_opt`/
  `assign` sobre la pila de ámbitos; `var`/mutación; shadowing por celda nueva al declarar), `if`/`while`/
  `block`/`return`, llamadas a funciones nombradas + recursión (`call_named`: guarda/restaura `scopes` →
  scoping léxico; desenvuelve `FReturn` en el valor de la función), builtins `print`/`eprint`/`to_string`/
  `panic` (`panic` interceptado en la llamada, con su posición). Las formas no-núcleo (`EIndex`/`EField`/
  `EArray`/`EStructLit`/`EFunc`/`EMatch`/`ETry`, UFCS/llamada indirecta) terminan en `panic` con su
  sub-fase destino. Driver `selfhost/run.ray` (lex→parse→check→**ejecuta**; código de salida = el `int`
  de `main`). **Oráculo conductual** (`tests/selfhost_interpreter.rs`, 5 tests): los 4 ejemplos del
  núcleo (fib/fizzbuzz/gcd/primes) + snippets (aritmética/floats/lógica/comparación/concat, mutación/
  shadowing, llamadas/recursión/recursión-mutua, código de salida) → mismo stdout + exit que el runner de
  Rust. El corpus usa solo lo que aceptan **ambos** checkers (print/eprint; `to_string` lo rechaza el
  checker auto-alojado de M14.3 → fuera del corpus). Diferido: M14.4b (datos), c (primera clase), d
  (despacho dinámico); TCO/trampolín y `MAX_CALL_DEPTH` (el host ya da TCO + pila grande).
- **M14.4b COMPLETO** — datos. `Value` gana `VArray([Value])`, `VStruct(string, [SField])` y
  `VEnum(string, string, [Value])`; `Interp` gana las tablas `structs`/`enums` del programa. Aquí luce
  el **cabalgar sobre el host**: la **semántica de referencia** de arreglos/structs del invitado es la
  del `[Value]`/`[SField]` de raylang (un alias comparte el mismo objeto del heap; `push`/`a[i]=v`/
  `obj.f=v` mutan en el sitio y se ven por todos los alias —probado con `r.origen.x=99 ⇒ p.x=99` y arreglo
  de structs—). Y luce la **resolución en runtime**: la construcción de enum (`Enum.Variante` y
  `Enum.Variante(args)`), que el checker-validador NO reescribió, se **reconoce en eval** mirando
  `c.enums` (`enum_has_variant`); el resto de `obj.f`/`f.m(args)` es acceso a campo o método (M14.4d).
  Cubre: arreglos (literal/índice/`len`/`push`/asignación/anidados; índice de string → char), structs
  (literal en **orden de declaración** → Display/igualdad casan, acceso/asignación de campo), enums
  (construcción, `match` con patrones `_`/binding/variante + payload, tipos recursivos). `value_str`/
  `values_equal` extendidos (structural recursivo). Oráculo (`tests/selfhost_interpreter.rs`, 9 tests):
  los 5 ejemplos de datos (structs/enums/match_figuras/arrays/matriz) + snippets (aliasing, arreglo de
  structs, payload+binding+comodín, lista enlazada recursiva, indexación encadenada) → mismo stdout +
  exit que Rust. Builtins del corpus: `len`/`push` (los que conoce el checker auto-alojado). Diferido:
  M14.4c (closures/orden superior/Option/Result/`?`), d (UFCS/métodos/dyn/@derive).
- **M14.4c COMPLETO** — primera clase. `Value` gana `VFunc(Func)` (función nombrada como valor) y
  `VClosure(FnExpr, [Capture])` (anónima + entorno). **Ventaja del host**: en vez del esquema de índices
  de Rust (`Value::Function(idx)` + tabla de anónimas), el valor guarda el `Func`/`FnExpr` **directamente**
  (referencia + GC del host) → cero tabla de colección. **Las CELDAS**, ahora sí: los ámbitos pasan de
  `Map<string,Value>` a `Map<string,Cell>` (`struct Cell { v }`); `define` crea una celda **nueva**
  (shadowing seguro), `assign` **muta** la celda (no la reemplaza → las closures ven el cambio), `lookup`
  lee `cell.v`. Una `Cell` del host, por su semántica de referencia, ES la celda mutable compartida que en
  Rust pide `Rc<RefCell<Value>>` —sin reimplementar nada—. `capture_env` toma snapshot de las celdas
  visibles (fuera→dentro, interior tapa) y `call_body` liga las capturadas en el ámbito base (compartidas)
  + los params encima. Llamada **indirecta** (`call_value`: VFunc/VClosure → `call_body`). Operador **`?`**
  (`eval_try`: `Ok/Some` desempaqueta, `Err/None` → `FReturn`). Los enums del prelude `Option`/`Result` se
  **inyectan** en `c.enums` (`inject_prelude_enums`: el intérprete solo necesita reconocer las variantes
  para construcción/`match`; el usuario los puede sobrescribir). Oráculo (12 tests): `closures.ray`/
  `errores.ray`/`opcional.ray` + snippets (orden superior con función nombrada, closure que captura un
  `let`, **estado por celda** con instancias independientes, `?` encadenado con Result y con Option+None)
  → mismo stdout + exit que Rust. Diferido: M14.4d (UFCS/métodos/`dyn`/`@derive` + map/filter/fold del
  prelude).
- **M14.4d-1 COMPLETO** — despacho dinámico. Aquí la decisión **resolución en runtime** (Decisión 2)
  alcanza su máxima expresión. `Interp` gana `methods: Map<string, Method>` (`Tipo#metodo` → método,
  `struct Method { params, body }`), poblado por `register_methods` desde los `impl` del programa más los
  **métodos por defecto** del trait no redefinidos. `dispatch_method(recv, fname, args)` resuelve por
  orden: (a) **campo-función** del struct (gana sobre UFCS), (b) **método** (clave
  `type_key_of_value(recv) + "#" + fname`), (c) **`@derive`** (`eq` ≡ `values_equal`, `show` ≡
  `value_str` —el checker garantiza Eq/Show, así que (b) cubre los impls explícitos y aquí solo queda el
  caso derivado, sin leer la anotación—), (d) **UFCS** a función libre `fname(recv, args)`, (e) **builtin
  como método** (`xs.len()`). **Consecuencias elegantes**: **bounds y genéricos son no-ops** (`x.m()`
  con `x: T: Trait` despacha por el tipo concreto de `x` en runtime, sin diccionarios) y **`dyn` es
  trivial** (un valor "objeto" ES el valor concreto, sin vtable; `obj.m()` despacha por su etiqueta, y un
  `[dyn Trait]` es un arreglo de valores concretos). Los **impls genéricos** (`impl<T> .. for Caja<T>`)
  se clavan por **constructor** (`type_key_of_type(Caja<T>) = "Caja"`); el anidamiento (`Caja<Caja<int>>`)
  se resuelve solo por despacho recursivo. Oráculo (16 tests): `ufcs`/`traits`/`bounds`/
  `metodos_por_defecto`/`impls_genericos`/`trait_objects`/`anotaciones` + snippets (UFCS a builtin/libre/
  genérica, campo-función, despacho sobre primitivo, bound no-op, defecto, `dyn` heterogéneo con defecto,
  `@derive` anidado) → mismo stdout + exit que Rust. Diferido: M14.4d-2 (map/filter/fold del prelude →
  `stdlib.ray`, cierra el self-hosting).
- **M14.4d-2 COMPLETO → M14.4d COMPLETO → M14.4 COMPLETO → SELF-HOSTING CERRADO**. map/filter/fold del
  prelude. Como el checker auto-alojado es un validador (no inyecta el prelude en el programa) y el
  intérprete necesita los **cuerpos**, se replica lo que hace el `check()` de Rust: un archivo
  `selfhost/prelude.ray` (map/filter/fold escritos **en raylang** —genéricos + closures + `len`/`push`—)
  que el driver `selfhost/run.ray` **parsea y FUSIONA** en el programa del usuario (`add_prelude`: añade
  solo las que el usuario no redefina → override; la fusión no necesita desplazar posiciones porque el
  validador no baja por posición y el intérprete despacha por etiqueta de valor). Tras la fusión,
  map/filter/fold son funciones ordinarias: el despacho por UFCS (`xs.map(f)`) cae en la rama UFCS de
  `dispatch_method`, y los pipelines (`xs |> f`) ya los desazucara el parser a llamadas. Oráculo (18
  tests): `stdlib.ray` (UFCS encadenado + pipelines + closures inline) + `genericos.ray`/
  `tipos_genericos.ray` (genéricos = no-op en runtime) + snippets (UFCS/pipeline/override) → mismo stdout
  + exit que Rust. **Verificado: los 22 ejemplos del corpus corren idénticos por ambos pipelines** (Rust
  `cargo run` vs `raylang selfhost/run.ray`). **raylang lexea/parsea/chequea/EJECUTA raylang de punta a
  punta — el self-hosting está cerrado.** Diferido (fuera del corpus, como en el checker): builtins de
  string/IO/Map en el intérprete auto-alojado, TCO/`MAX_CALL_DEPTH`, `assert`/`sort`/resto del prelude.
  Siguiente hito posible: **M14.5** (la VM auto-alojada, capstone-del-capstone opcional).

#### M14.6 — diferidos aditivos hacia la META-CIRCULARIDAD

Norte: que el intérprete auto-alojado ejecute el **propio compilador auto-alojado** (lexer/parser/
checker corriendo sobre sí mismos). El compilador usa builtins que el checker/intérprete auto-alojados
aún no soportan (`to_string`/`chars`, `Map`, `parse_int`/`parse_float`, `panic`, …); cada grupo es un
diferido aditivo (fila en el checker + impl en el intérprete, como M11.4).

- **M14.6a COMPLETO** — **builtins de string** (checker + intérprete). El **checker auto-alojado** gana
  `to_string`/`trim`/`split`/`chars`/`contains`/`replace`/`starts_with`/`ends_with`/`to_upper`/
  `to_lower`/`substring`/`repeat`/`join` (reglas y mensajes byte-idénticos a `src/builtins.rs`; helpers
  `b_arity`/`check_args_types`/`want_string`/`want_int` + una `check_*` por builtin, registradas en
  `is_known_builtin`/`check_named_call`). El **intérprete auto-alojado** los implementa **delegando en
  los del host** (`is_builtin`/`dispatch_builtin`; `chars`/`split` envuelven `[char]`/`[string]` en
  `VArray`, `join`/`contains` van a mano sobre `[Value]`). **Gotcha cazado por el host checker**: al
  reescribir la rama de `push` con `return` dentro del `match`, sus dos brazos quedaron **divergentes**
  (`return` + `panic`) y el checker no podía calcular el tipo del `match` ("hay al menos un brazo") → se
  arregló devolviendo el `match` (`return match {...}`) con el brazo normal cediendo el valor. Oráculo
  (19 tests): snippet con toda la familia (UFCS + directa) + `chars`/indexar-string/recorrido → mismo
  stdout + exit que Rust. Pendiente hacia meta-circularidad: `Map` (checker + intérprete), `panic` en el
  checker, `parse_int`/`parse_float`, y luego ejecutar `selfhost/lex_dump.ray` sobre el intérprete.
- **M14.6b COMPLETO** — **`Map<K, V>`** (checker + intérprete; el diferido más invasivo). El **checker
  auto-alojado** reconoce `Map<K, V>` (llega del parser como `TNamed("Map", [K, V])` —no hay variante
  `TMap`, se trata por nombre—): `ensure_type` valida aridad 2 + clave hashable (`is_hashable_key`:
  int/string/char/bool o param de tipo), `len` lo acepta, y los builtins `map_new`/`insert`/`get`/
  `remove`/`contains_key`/`keys`/`values` (reglas/mensajes byte-idénticos a Rust; `get`/`remove` reflejan
  el envoltorio del prelude `(Map<K,V>, K) -> Option<V>`). **`map_new()` es indeterminado** (como `[]`/
  `None`): su tipo lo fija el esperado vía bidireccional (`check_map_new` recibe `expected`, interceptado
  en `check_call`); sin esperado → error idéntico a Rust. El **intérprete** añade `VMap(MapData)` con
  `struct MapData { keys: [Value], vals: [Value] }` —arrays PARALELOS con búsqueda lineal por
  `values_equal`, no un `Map` del host (sus claves serían un `Value`/enum, no hasheable); `MapData` es un
  struct → `insert`/`remove` mutan y los alias lo ven, como `VArray`—. `keys()`/`values()` se devuelven
  **ordenadas por clave** (insertion sort con `key_lt`) → deterministas como Rust. Oráculo (20 tests):
  snippets con claves string e int (insert/get/contains_key/len/keys/values/remove + mutación
  compartida) → mismo stdout + exit que Rust. (`examples/data/mapa.ray` aún no: usa `assert_eq`/`assert` del
  prelude → M14.6c.) Pendiente: `panic` en el checker, `parse_int`/`parse_float`, `assert`/`sort` → luego
  ejecutar el compilador auto-alojado.
- **M14.6c-1 COMPLETO** — **`panic` + `parse_int`/`parse_float`**. El **checker auto-alojado** gana el
  builtin **`panic(msg: string) -> unit`** (`check_panic`; el análisis de divergencia `expr_diverges` ya
  lo reconocía por nombre → ahora también lo TIPA) y los primitivos **`__parse_int`/`__parse_float`**
  (`check_parse_prim`, `(string) -> [int]`/`[float]`), todos registrados en `is_known_builtin`/
  `check_named_call`; las **firmas** de los envoltorios `parse_int`/`parse_float` (`(string) ->
  Option<int>`/`Option<float>`) se inyectan en `inject_prelude_fns`. El **intérprete** implementa
  `__parse_int`/`__parse_float` **delegando en los del host** (`Option` → `[T]` de 0/1 elemento) y ya
  tenía `panic` (M14.4a). Los **cuerpos** `parse_int`/`parse_float` se añaden a `selfhost/prelude.ray`
  (envoltorios sobre los primitivos → `Option`, como en Rust; el driver los fusiona en el programa). Es
  exactamente lo que el lexer usa al tokenizar números y abortar. Oráculo: intérprete (21 tests:
  parse_int/parse_float camino feliz y fallo, `panic` que no dispara —cede el tipo— y que sí dispara
  —exit 70 en ambos—); checker (25 tests: válidos + errores byte-idénticos). **Lo que falta para correr
  el lexer entero sobre el intérprete auto-alojado NO es la stdlib**, sino dos diferidos mayores: **carga
  de módulos** (el pipeline auto-alojado procesa un solo archivo, no resuelve `import`) y los **builtins
  de I/O** (`args`/`read_file`) que usa `lex_dump.ray`. Pendiente de M14.6c: `assert`/`assert_eq`/`sort`.
- **M14.6c-2 COMPLETO → M14.6c COMPLETO** — **`assert`/`assert_eq`/`sort`** (el prelude de aserciones y
  orden, sobre `panic` + los traits Eq/Show/Ord). El **checker auto-alojado** inyecta sus **firmas** en
  `inject_prelude_fns`: `assert(bool)`, `assert_eq<T: Eq + Show>(T, T)`, `sort<T: Ord>([T]) -> [T]` (los
  bounds resuelven contra los traits + impls de primitivos ya registrados en M14.3d-4c). Sus **cuerpos**
  (port de `src/prelude.rs`) van a `selfhost/prelude.ray` (fusionados por el driver, chequeados por ambos
  pipelines). El punto delicado: `sort` usa `x.less(out[j-1])` (Ord) y el intérprete-validador omitió el
  lowering de diccionarios → el intérprete resuelve **`.less()` sobre primitivos por fallback** en
  `dispatch_method` (junto a `eq`/`show`; helper `value_lt` para int/float/string/char). Un tipo de
  usuario con `impl Ord` se resuelve antes, por la tabla de métodos (`Tipo#menor`), así que el fallback
  solo ve los cuatro primitivos —exactamente lo que el checker garantiza vía `T: Ord`—. `assert_eq` reusa
  el fallback de `eq`/`show`. Oráculo: intérprete (22 tests: `sort` de int/string/float, tipo de
  usuario con `impl Ord`, `assert`/`assert_eq` ok y `assert_eq` que falla → exit 70, + **`examples/data/mapa.ray`**
  que estaba diferido por `assert_eq`/`assert` en M14.6b); checker (25 tests: válidos + error de bound
  `sort` sin Ord, byte-idéntico). **M14.6c COMPLETO** (`panic` + `parse_int`/`parse_float` + `assert`/
  `assert_eq`/`sort`). El compilador auto-alojado entero sobre el intérprete sigue bloqueado por la **carga
  de módulos** + los **builtins de I/O** (diferidos mayores, no la stdlib).
- **M14.6d COMPLETO** — **I/O de archivos** (`read_file`/`write_file`/`exists`). El **checker auto-alojado**
  gana los primitivos `__read_file`/`__write_file` (arreglo etiquetado `[string]`) y el builtin `exists ->
  bool` (`check_read_file`/`check_write_file`/`check_exists`; mensajes byte-idénticos a `src/builtins.rs`)
  en `is_known_builtin`/`check_named_call`, más las **firmas** de los envoltorios `read_file ->
  Result<string,string>` / `write_file -> Result<int,string>` en `inject_prelude_fns`. El **intérprete**
  delega `__read_file`/`__write_file`/`exists` directo en los primitivos del host (que ya devuelven el
  arreglo etiquetado / bool); los **cuerpos** `read_file`/`write_file` (port de `src/prelude.rs`, traducen
  el arreglo a `Result`) van a `selfhost/prelude.ray`. **Oráculo conductual posible porque es determinista**:
  ambos pipelines escriben el MISMO contenido a un temporal y lo releen → mismo stdout (1 test intérprete;
  4 checker: read/write/exists válidos + error de tipo en `exists`). **Diferidos a propósito** (no encajan
  en el oráculo conductual): **`args()`** —diverge entre pipelines: el self-hosted ve el path que `run.ray`
  consumió como `argv[0]`; necesita que el driver enhebre los args al intérprete—, y **stdin/env**
  (`input`/`read_line`/`env`) por no deterministas. Los handles de archivo (`open`/`close`/…) y
  `remove_file`/`list_dir`/`append_file` quedan fuera del alcance (no los usa el compilador). Con esto la
  stdlib de archivos que el compilador necesita (`read_file`) está cubierta; el bloqueo restante para la
  meta-circularidad es **solo la carga de módulos**.

#### M14.7 — el loader auto-alojado (carga de módulos)

El último bloqueo para la meta-circularidad: el pipeline auto-alojado procesaba **un solo archivo**, pero
los drivers y módulos del compilador se reparten en varios (`from lexer import …`, `from parser import …`).
`selfhost/loader.ray` (cliente host-side como `run.ray`) **aplana** la entrada y sus dependencias
transitivas en un único `Program` plano, que el checker/intérprete auto-alojados ya saben procesar. Port
recortado de `src/loader.rs`. **Dos simplificaciones frente a Rust**: (1) el self-hosting solo usa `from M
import …` (sin `import M;`/acceso calificado, sin directorios/cápsulas/reexports), y (2) **no hace falta el
position-shifting** —Rust desplaza cada módulo a una banda de líneas disjunta porque su checker baja por
posición (M9), pero el checker auto-alojado es un VALIDADOR y el intérprete despacha por etiqueta de valor:
para programas válidos las posiciones son irrelevantes al comportamiento—.
- **M14.7a COMPLETO** — la máquina de carga + cruce de **funciones**. `load(entry) -> Result<Program,
  LoadError>`: **BFS** sobre los `from`-imports (lee con `read_file`, lexea+parsea cada módulo una vez,
  ciclos seguros con un mapa `visited`; ruta = `dir(entry)/dep.ray`). Luego, por módulo: **superficie
  pública** (`build_surfaces`: función `pub` → global `modulo::fn`), clasificación de los `from M import
  <fn>` (`clasificar_from_values` → mapa local→global, valida `pub`), el **`Resolver`** (reescribe las
  referencias `EIdent` a su nombre global —propias e importadas—, *consciente de ámbitos*: una local/param/
  binding de `match` tapa a una función de nivel superior; pila `scopes`), y por fin **renombrar** las
  definiciones de función a su global y **fusionar** en el `Program` plano. `run.ray` pasa a usar
  `load(argv[0])` (también para el prelude); un archivo único no tiene `from`-imports → el loader es
  **identidad** (cero regresión). Los **tipos** se fusionan aún SIN namespacar (su cruce → M14.7b). La
  mutación in-place del AST se apoya en la **semántica de referencia del host** (los nodos `Expr`/`Func`
  son structs compartidos, como en el intérprete). Oráculo conductual: cruce de funciones entre 2 archivos,
  **alias** en el from-import, **shadowing** local que tapa la importada, **cadena transitiva** A→B→C y
  función-importada-como-valor (a `map`) → mismo stdout + exit que Rust. Pendiente: M14.7b (cruce de
  **tipos**: namespacing de defs + TypeRewriter + `from M import Tipo`) → desbloquea los módulos reales;
  M14.7c (correr el compilador auto-alojado + consistencia de `args()`).
- **M14.7b COMPLETO** — el cruce de **tipos** (desbloquea los módulos reales del compilador, que se
  importan `Type`/`Expr`/… entre sí). Tres piezas, port de Rust: (1) **superficie de tipos** —`Surface`
  gana `types` y `build_surfaces` la puebla con los struct/enum/trait `pub`—; (2) `clasificar_from`
  clasifica cada `from M import X` en **valor** (función → mapa del Resolver) o **tipo** (→ mapa del
  TypeRewriter), validando `pub`; (3) el **`TypeRewriter`**: `rename_type_defs` renombra las
  definiciones de tipo propias a `modulo::Tipo`, y `tw_program` **reescribe todas las referencias** —en
  posiciones de tipo (anotaciones, campos, payloads, target/trait de `impl`, bounds, `dyn`, args de
  genérico) y en expresiones que **nombran** tipos (literal de struct, construcción de enum `Tipo.Variante`
  que llega como `Field`/`Call`, patrones `match`)— *consciente de los parámetros de tipo en ámbito* (un
  `T` en `<T>` no se reescribe; pila `tparams`). El parser auto-alojado emite `TNamed` para todo
  identificador-tipo (incl. `Map`/`T`); `tw_name` deja los no encontrados igual, así que cubre ambos sin
  caso especial. Sin `import M;` calificado → el caso `M.Tipo` del Rewriter de Rust no hace falta. Oráculo
  conductual: cruce de **struct + enum** (construcción de variante + `match`), de **trait + impl + struct
  genérico + `dyn`**, y **alias de tipo** en el from-import → mismo stdout + exit que Rust. Pendiente:
  M14.7c (correr el compilador auto-alojado de punta a punta + consistencia de `args()`).
- **M14.7c COMPLETO → M14.7 COMPLETO → META-CIRCULARIDAD LOGRADA.** El compilador auto-alojado entero
  corre **sobre el intérprete auto-alojado**. Piezas: (1) **`args()` consistente** entre los dos
  pipelines —`run.ray` consume `argv[0]` (el path del driver) y **enhebra `argv[1..]`** al intérprete
  (`run(prog, args)`; `Interp` gana el campo `args`; el builtin `args()` los devuelve), así un driver ve
  sus PROPIOS args igual que bajo Rust (`raylang <prog> [args]`); un programa de un solo archivo da
  `args() == []` en ambos—; (2) `args()` añadido al checker (nulario → `[string]`); (3) `pop` (último
  builtin de stdlib que faltaba: el checker `checker.ray` lo usa) — primitivo `__pop` (muta + `[T]` de
  0/1) en checker e intérprete, envoltorio `pop<T>([T]) -> Option<T>` en el prelude; (4) **concatenación
  de arreglos** `a + b` en `eval_add` del intérprete (lo usa `run.ray` en `add_prelude`; el checker ya la
  aceptaba). **Verificado** (oráculo conductual `tests/selfhost_metacircular.rs`, comparando el driver
  corrido por Rust vs corrido sobre el intérprete auto-alojado): **`lex_dump`** (lexer), **`parse_dump`**
  (parser, AST idéntico), **`check_dump`** (checker, mismo veredicto incl. errores) y **run-on-run**
  (`run.ray` corriendo `run.ray` corriendo un programa → el back-end también) producen **stdout + exit
  idénticos**. raylang lexea/parsea/chequea/EJECUTA raylang **con raylang corriendo sobre raylang**.
  (run-on-run es `#[ignore]`: ~1 min por la interpretación de dos niveles; ejecútalo con `--ignored`.)
  Diferidos (fuera de la meta-circularidad lograda): la VM auto-alojada (M14.5), `import M;` calificado/
  directorios/cápsulas en el loader auto-alojado, el resto de builtins de I/O (stdin/env/handles).

### 23.6 M14.5 — La VM auto-alojada (diseño)

El **M2 de este módulo**: un segundo back-end auto-alojado que **compila el AST a bytecode** y lo
ejecuta en una **VM de pila**, en paralelo al intérprete tree-walking de M14.4 (mismo orden M1→M2 que
seguimos en Rust). Opcional —la meta-circularidad ya está lograda con el intérprete—, pero cierra el
arco "dos motores" también en el mundo auto-alojado.

**Decisión central (la elegancia del self-hosting): un solo `Value`.** En Rust, el intérprete y la VM
tienen **representaciones de valor distintas** —`Value` con `Rc` el intérprete, `gc::HeapValue` con
handles la VM— porque la VM gestiona su propio GC. La VM auto-alojada **reusa el mismo `Value` del
intérprete** (`selfhost/interpreter.ray`) y su runtime puro: `value_str`, `values_equal`,
`type_key_of_value`, la aritmética (`eval_add`/`eval_arith`/`eval_cmp`) y, sobre todo,
`dispatch_builtin`. Ambos motores **cabalgan sobre el GC del host** → **sin GC propio, sin conversión
en el borde**. Es la simplificación que el self-hosting regala y que Rust no tiene.

**Bytecode compacto.** A diferencia de la VM de Rust (un opcode por builtin: `Print`/`Len`/`Split`/…),
la auto-alojada tiene un opcode **genérico** `OBuiltin(nombre, argc)` que saca `argc` valores y delega
en `dispatch_builtin` (y, más adelante, `OMethod(nombre, argc)` para el despacho). Así el set de
opcodes es el **núcleo** (constantes, aritmética, comparación, saltos, locales, `Call`/`Return`,
`MakeArray`/`Index`/…) y los builtins reusan el trabajo del intérprete.

**Arquitectura.** `selfhost/compiler.ray` (AST → `[CompiledFn]`, cada una con su `Chunk` de `[Op]` +
`[Value]` de constantes; resolución de nombres a *slots* locales e índices de función) +
`selfhost/vm.ray` (pila de operandos `[Value]` + pila de marcos explícita `[Frame]`, en un bucle
iterativo — como la VM de Rust, sin usar la pila del host para los marcos del lenguaje). Driver
`selfhost/run_vm.ray`.

**Oráculo: conductual, como M14.4.** La misma `.ray` por ambos caminos —Rust directo vs la VM
auto-alojada (`run_vm.ray`)— comparando stdout + código de salida; corpus determinista. La VM
auto-alojada debe coincidir con Rust (y por tanto con el intérprete auto-alojado, que ya coincide).

**Sub-fases** (espejo de M14.4): **a** núcleo (constantes, aritmética/comparación/lógica con
cortocircuito, locales, if/while, llamadas nombradas, recursión, builtins escalares) → **b** datos
(arreglos/structs/enums/`match`) → **c** primera clase (funciones-valor, closures con celdas/upvalues)
→ **d** despacho dinámico (métodos/UFCS/`dyn`/`@derive` + prelude). TCO opcional al final.

- **M14.5a COMPLETO** — el **núcleo**. `selfhost/compiler.ray`: opcodes (`enum Op`: `OConst`/aritmética/
  comparación/`OJump`/`OJumpIfFalse`/`OGetLocal`/`OSetLocal`/`OInitLocal`/`OCall`/`OReturn`/`OBuiltin`),
  `Chunk`/`CompiledFn`/`CProgram`, y `compile(Program) -> CProgram` con resolución de slots (monotónicos,
  `max_slots` dimensiona el marco) y ámbitos de bloque (`begin_scope`/`end_scope`), cortocircuito de
  `&&`/`||`, e `if`/`while` orientados a expresión (port de `src/compiler.rs`). `selfhost/vm.ray`: pila de
  operandos + pila de marcos explícitas (cada `Frame` con su propia pila → sin *base pointer*); reusa la
  aritmética y `dispatch_builtin` del intérprete vía `?` sobre `Flow`; `run_vm(CProgram, args) ->
  Result<Value, RuntimeError>`. Driver `selfhost/run_vm.ray` (gemelo de `run.ray`). Habilitador: se
  hicieron `pub` los helpers reusados del intérprete y `dispatch_builtin` pasó a tomar `prog_args` (en vez
  de `Interp`). Oráculo conductual (`tests/selfhost_vm.rs`, VM auto-alojada vs Rust): fib/fizzbuzz/gcd/
  primes + snippets de aritmética/control/recursión/locales/builtins → stdout+exit idénticos. El **prelude
  no se fusiona aún** (map/filter/fold/sort → llamadas indirectas/métodos, M14.5c). Pendiente: b (datos),
  c (closures + prelude), d (despacho dinámico).
- **M14.5b COMPLETO** — **datos** (arreglos/structs/enums/`match`). Filosofía: **resolución por NOMBRE en
  runtime**, como el intérprete auto-alojado (M14.4b) y a diferencia de la VM de Rust, que numera variantes
  con *tags* y structs con *ids* en tablas (`MakeEnum(enum_id, tag)`, `MakeStruct(idx)`, `EnumTagEq(tag)`).
  Aquí los opcodes llevan los nombres: `OMakeArray(n)`, `OIndex`/`OSetIndex`, `OMakeStruct(nombre, [campos])`,
  `OGetField(s)`/`OSetField(s)`, `OMakeEnum(enum, variante, aridad)`, `OEnumTagEq(variante)`,
  `OGetEnumField(i)`, `OMatchFail`. El compilador gana tablas `structs` (nombre → campos en orden de
  declaración, para emitir el literal ordenado) y `enums` (nombre → variantes; incluye Option/Result vía
  `inject_prelude_enums`) para **reconocer la construcción de enum en compilación** (`Enum.Variante` con/sin
  payload, espejando `eval_field_or_enum`/el path de llamada del intérprete). `emit_match` es port de
  `emit_match` de Rust pero comparando la variante **por nombre** (`OEnumTagEq`), con el escrutinio en un
  local temporal `$match`; `?` (`ETry`) se difiere a M14.5c (va con Option/Result/primera clase). La VM
  **reusa el runtime del intérprete**: `OMakeStruct` construye `SField` (ahora `pub`), `OGetField`/`OSetField`
  delegan en `struct_get`/`struct_set`, `OIndex`/`OSetIndex` en `as_int` + bounds-check con el mismo mensaje;
  la **semántica de referencia** de arreglos/structs es la del `[Value]`/`[SField]` del host (mutación
  compartida y aliasing gratis). Oráculo conductual = corpus de datos del intérprete (`examples/structs`/
  `enums`/`match_figuras`/`arrays`/`matriz` + snippets de aliasing/payload/lista recursiva/OOB) → stdout+exit
  idénticos. Pendiente: c (closures/primera clase/`?` + fusión del prelude), d (despacho dinámico), TCO.
- **M14.5c COMPLETO** — **primera clase**: funciones como valor, funciones anónimas + closures (captura por
  upvalues), llamada indirecta y `?`. Esquema de upvalues **estilo clox** (resolución transitiva en el
  compilador: `resolve_upvalue` busca el nombre como local de la envolvente o, recursivo, como upvalue suyo;
  `add_upvalue` deduplica; `UpSrc.ULocal(slot)`/`UpSrc.UUp(idx)`), PERO **sin el análisis de boxing de Rust**
  (`captured_slots`/`mark_captured`): en la VM **toda local es una `Cell`** (como el intérprete auto-alojado,
  donde cada variable es una celda), así la celda siempre existe y el upvalue solo la referencia → el
  compilador no decide qué boxear. El compilador pasó de `Cc` (estado de UNA función) a `Comp` con una **pila
  de `Fscope`** (las funciones envolventes quedan debajo para que `resolve_upvalue` las consulte por índice de
  profundidad); las funciones anónimas se compilan **en línea** (empujan su `Fscope`, emiten, lo desapilan) y
  se **anexan** a `comp.out`, con las nombradas en índices reservados `0..n` (placeholders) para que las
  anónimas no los pisen. **Única extensión del `Value` compartido**: `VVmClosure(int, [Cell])` (índice de la
  fn compilada + celdas capturadas en orden de upvalue) — la representación de un cierre difiere genuinamente
  entre AST (intérprete: `FnExpr` + capturas por nombre) y bytecode (VM: índice + celdas); en Rust también
  difieren intérprete y VM. La VM: `OGetLocal`/`OSetLocal` leen/mutan `Cell.v` (la mutación la ven las
  closures que comparten la celda), `OInitLocal` estrena una celda fresca (shadowing/iteraciones de bucle),
  `OFunc(idx)` empuja `VVmClosure(idx, [])` (nombrada-como-valor o anónima sin captura), `OClosure(idx, srcs)`
  captura las celdas **por referencia** leyendo `srcs` del marco actual, `OCallValue(argc)` desempaqueta el
  `VVmClosure` y apila un marco con sus celdas como upvalues, `OTry` desempaqueta `Ok`/`Some` o retorna el
  valor entero (como `OReturn`). El **prelude no se fusiona aún** (map/filter/fold no usan métodos, pero
  `sort`/`assert_eq` sí → el prelude completo compila en M14.5d). Oráculo conductual = corpus de primera clase
  del intérprete (`closures`/`errores`/`opcional` + snippets de HOF/captura/estado-mutable/transitiva/`?` con
  Result y Option) → stdout+exit idénticos. Pendiente: d (despacho dinámico: métodos/UFCS/`dyn`/`@derive` +
  fusión del prelude), TCO.
- **M14.5d COMPLETO → M14.5 COMPLETO** — **despacho dinámico** (métodos de trait, UFCS, `dyn`, `@derive`,
  bounds) + **fusión del prelude completo**. Filosofía idéntica al intérprete auto-alojado (M14.4d):
  **resolución por la ETIQUETA del valor en runtime**, así `dyn`/bounds/genéricos son **no-ops** (se despacha
  por el tipo concreto; sin diccionarios ni vtable, el "objeto" ES el valor concreto). El compilador baja
  `recv.f(args)` (que no sea construcción de enum) a un solo opcode `ODispatch(fname, argc)` —no resuelve
  campo-vs-método-vs-UFCS en compilación, lo deja al runtime—. `compile_methods` (espejo de `register_methods`
  del intérprete) compila los métodos de cada `impl` como funciones con `self` como primer parámetro (reusa
  `compile_body`, extraída de `compile_named`) y los métodos por DEFECTO del trait no redefinidos, y puebla
  `CProgram.methods` (`Tipo#metodo → índice`, clave por constructor vía `type_key_of_type`: `Caja<T>`→"Caja", un
  impl genérico cubre la familia). `CProgram` lleva además `indices` (función libre → índice) para la UFCS. La
  VM resuelve en `resolve_dispatch` (espejo de `dispatch_method`): (a) campo-función del struct (gana, sin
  anteponer el receptor) → (b) método de la tabla (antepone `self`) → (c) `@derive` `eq`≡`values_equal`,
  `show`≡`value_str` y `less` de Ord sobre primitivos≡`value_lt` (todos reusados del intérprete, hechos
  `pub`) → (d) UFCS a función libre (`indices`) → (e) builtin como método (`dispatch_builtin`). Devuelve un
  `Dispatch` (`DFrame(idx, args, cells)` apila marco / `DValue(v)` empuja valor) para separar la resolución de
  la acción y NO usar `return` dentro del bucle de la VM. **Prelude completo fusionado** en `run_vm.ray` (como
  `run.ray`): map/filter/fold (indirectas, M14.5c) + sort/assert_eq (métodos `.less()`/`.eq()`/`.show()`
  por `ODispatch`) ya compilan. Gotcha reusado (M14.6a): un `match` con todas las ramas divergentes no tipa en
  el checker de Rust → el `match` interno del caso (a) cede un valor en el brazo normal. Oráculo conductual =
  corpus de despacho del intérprete (`ufcs`/`traits`/`bounds`/`metodos_por_defecto`/`impls_genericos`/
  `trait_objects`/`anotaciones`) + prelude (`stdlib`/`mapa`) + snippets (método/UFCS/`@derive`/`sort`); **los 33
  ejemplos deterministas corren idénticos por la VM auto-alojada y por Rust**. **La VM auto-alojada ejecuta el
  LENGUAJE COMPLETO** (núcleo + datos + primera clase + despacho dinámico + prelude); el compilador auto-alojado
  tiene ya DOS back-ends (intérprete M14.4 + VM M14.5), como Rust (M1 + M2). Diferido: TCO en la VM
  auto-alojada (opcional), VM meta-circular (correr el compilador auto-alojado sobre la VM auto-alojada).
- **M14.5e COMPLETO → M14.5 (VM auto-alojada) COMPLETA CON TCO** — **TCO** (recursión de cola en O(1) marcos),
  port de M13.3b. Un **peephole** `optimize_tail_calls` (corrido en `compile_body` sobre el bytecode ya
  generado) reescribe toda llamada `OCall`/`OCallValue`/`ODispatch` cuya continuación sea un `OReturn` —directo
  o a través de saltos incondicionales (`returns_immediately` sigue la cadena de `OJump`)— a su variante de
  cola `OTailCall`/`OTailCallValue`/`OTailDispatch`. Las variantes de cola **reutilizan el marco actual**
  (`frames[top] = new_frame(...)`) en lugar de apilar uno nuevo: al retornar, el valor va al llamador ORIGINAL,
  así la recursión de cola corre en O(1) marcos. A diferencia de Rust —que solo tiene `TailCall`/
  `TailCallValue` porque sus métodos se bajan a `Call`—, aquí también hay `OTailDispatch` (los métodos/UFCS van
  por `ODispatch`): reutiliza el marco si el despacho resuelve a una función (campo-función/método/UFCS), o
  empuja el valor si es directo (`@derive`/`less` primitivo/builtin) y deja que el `OReturn` siguiente lo
  retorne. El compilador ya emite el patrón llamada→`Return` de forma natural (rama-else cae al `Return` final,
  un `return e` lo emite tras `e`), así que basta reconocerlo. Gotcha: el `Option.None` del peephole no infería
  su `T` (la inferencia no cruza del `then` al `else` del `if`) → se anotó `let nuevo: Option<Op>`. Verificado:
  1M de recursión de cola directa y mutua corre por la VM auto-alojada idéntico a Rust (oráculo
  `recursion_de_cola`, `#[ignore]` por lento ~7 min doble-interpretado; `cargo test --test selfhost_vm --
  --ignored`); los 33 ejemplos deterministas siguen idénticos con el peephole activo (la corrección de los
  opcodes Tail* la cubre también el corpus por defecto: gcd/primes/sort tienen llamadas en cola). **La VM
  auto-alojada está completa (con TCO).** Diferido: VM meta-circular (verificar `run_vm.ray` corriendo
  `run_vm.ray`, análogo al run-on-run del intérprete de M14.7c).
- **M14.5f COMPLETO → SELF-HOSTING POR LA VM CERRADO** — **VM meta-circular**. Gemelo del oráculo
  meta-circular del intérprete (M14.7c, `selfhost_metacircular.rs`) para el SEGUNDO back-end. Los drivers del
  self-hosting (`lex_dump`/`parse_dump`/`check_dump`) **compilados y corridos sobre la VM auto-alojada**
  (`raylang selfhost/run_vm.ray <driver> <input>`) producen stdout+exit idénticos a Rust ejecutándolos
  directamente → raylang lexea/parsea/chequea raylang con el compilador (`compiler.ray`) + la VM (`vm.ray`) de
  raylang corriendo sobre la VM del host. El caso fuerte, **run-on-run de la VM** (`run_vm.ray` compilando y
  corriendo `run_vm.ray`, que a su vez compila y corre el programa: TRES niveles, Rust → VM auto-alojada → VM
  auto-alojada → programa), se verifica idéntico a Rust (`#[ignore]` por lento; `cargo test --test
  selfhost_metacircular_vm -- --ignored`): **la VM auto-alojada se ejecuta a sí misma**. No hizo falta código
  nuevo —solo el test (`tests/selfhost_metacircular_vm.rs`)—: la VM ya soportaba el lenguaje completo (M14.5a–e)
  + los builtins (Map/I/O/`args` vía `dispatch_builtin`) que usan los drivers, y `run_vm.ray` enhebra `args()`
  igual que `run.ray` (M14.7c). **Self-hosting CERRADO por AMBOS back-ends**: el intérprete (M14.4 + M14.7) y la
  VM (M14.5), como Rust tiene M1 (intérprete) y M2 (VM). raylang compila, type-checkea Y ejecuta —por
  tree-walking Y por bytecode— raylang, con raylang corriendo sobre raylang.

## 24. M15 — Redes y la base moderna

Con el lenguaje completo y auto-alojado, M15 mira hacia **afuera**: lo que un lenguaje moderno
necesita para tocar el mundo —reloj, aleatoriedad, matemáticas y, sobre todo, **redes**— sin
abandonar las dos invariantes del proyecto: **cero dependencias de Cargo** (todo sobre `std`) y el
**oráculo** (intérprete ↔ VM) donde el comportamiento sea determinista.

### 24.1 Dirección (fijada con el usuario)

- **Transporte = builtins.** Los sockets (TCP/UDP) y la resolución de nombres van como builtins sobre
  `std::net`, reusando el **molde de handles** de M11.8 (un handle es un `int`; los objetos abiertos
  viven en un almacén de proceso del host, `OnceLock<Mutex<…>>`).
- **Protocolos = librería en raylang.** HTTP/URL (y JSON) se escriben **en raylang** y se traen con
  `import` (no en el prelude). Demuestran el sistema de módulos y la filosofía "lo que se puede
  escribir en el lenguaje, se escribe en el lenguaje".
- **Carga útil = `string` por ahora.** `socket_read`/`socket_write` usan `string`, igual que el I/O de
  archivos (M11.2c): cómodo para texto, *lossy* para binario puro. Un tipo `bytes`/buffer (binario
  correcto) queda como milestone futuro bien acotado.
- **Bloqueante primero.** Los sockets de M15 **bloquean el hilo del SO** (y por tanto, en M:1,
  *todas* las fibras). Es la base honesta y simple. La integración con el **scheduler de M12**
  (sockets no bloqueantes que **ceden** la fibra → el "servidor async real") es el **capstone** de
  M15, una sub-fase posterior.
- **No determinismo → pruebas por subproceso.** Reloj, RNG y redes no son deterministas: no entran al
  oráculo VM↔intérprete; se prueban por **integración** (subproceso), como el I/O de M11.2/M11.7c.
  Las **matemáticas** sí son deterministas → **oráculo**.

### 24.2 Sub-fases

- **M15.1 — habilitadores (la base moderna).** Builtins pequeños, prerequisitos de cualquier
  programa de red realista y huecos por derecho propio:
  - **M15.1a — matemáticas** (determinista, oráculo): `sqrt`, `pow`, `floor`, `ceil`, `round`, `abs`,
    `min`, `max`, `sin`, `cos`, `tan`, `ln`, `log10`, `exp`, y las constantes `pi()`/`e()`.
  - **M15.1b — reloj y aleatoriedad** (no determinista, subproceso): `now()` (epoch en ms),
    `monotonic()` (reloj monótono en ms, para medir intervalos), `sleep(ms)`, `random()` (float en
    `[0,1)`) y `random_int(n)` (entero en `[0,n)`).
- **M15.2 — cliente TCP** (bloqueante): `tcp_connect(host, port) -> Result<int,string>` (resuelve el
  nombre vía `std::net`), `socket_read(h) -> Result<string,string>`, `socket_write(h, s) ->
  Result<int,string>`, y `close` extendido al handle de socket (ad-hoc polimórfico, como con canales).
- **M15.3 — servidor TCP** (bloqueante): `tcp_listen(host, port) -> Result<int,string>`,
  `tcp_accept(listener) -> Result<int,string>` (bloquea hasta una conexión). Ejemplo: servidor echo.
- **M15.4 — protocolos en raylang**: una librería `net/http.ray` (cliente HTTP/1.1) + parseo de URL,
  y una librería de **JSON** (parse/serialize) escritas en raylang e importadas. Showcase del sistema
  de módulos (cápsulas de M11.6).
- **(capstone) M15.5 — sockets no bloqueantes integrados con el scheduler de M12**: un `accept`/`read`
  que en vez de bloquear el hilo **aparca la fibra** y deja correr al scheduler, despertándola cuando
  el socket está listo (estilo *readiness*/poll). El "servidor concurrente real". Diferido.

### 24.3 M15.1a — matemáticas (especificación)

Las funciones trascendentes (`sqrt`, `sin`, …) necesitan los intrínsecos de `f64`: van como builtins,
no en raylang. Como son **uniformes** (casi todas `float -> float`), en vez de un opcode por función
—que inflaría el `match` gigante de la VM (cuyo *layout* afecta al *codegen*, lección de la fase de
optimización)— se usa **un opcode parametrizado**: `OpCode::MathF(MathFn)` lleva un enum `MathFn` que
dice cuál aplicar. La VM y el intérprete tienen **una sola** rama que delega en un helper compartido
(`builtins::apply_mathf`, determinista e idéntico en ambos motores → cuadra el oráculo).

| Builtin | Firma | Notas |
|---------|-------|-------|
| `sqrt`/`sin`/`cos`/`tan`/`ln`/`log10`/`exp` | `(float) -> float` | `MathF(MathFn)`; dominio inválido (p.ej. `sqrt(-1)`) → `NaN` (la semántica de `f64`, sin error de runtime) |
| `floor`/`ceil`/`round` | `(float) -> float` | `MathF(MathFn)` |
| `pow` | `(float, float) -> float` | opcode `Pow` |
| `abs` | `int -> int` / `float -> float` | **ad-hoc polimórfico**; opcode `Abs` (ramifica por tipo) |
| `min`/`max` | `(int,int) -> int` / `(float,float) -> float` | ad-hoc poli.; opcodes `Min`/`Max` (ambos argumentos del mismo tipo numérico) |
| `pi`/`e` | `() -> float` | constantes; opcodes `Pi`/`E` |

Como todo builtin tras la limpieza L1: una fila en la tabla `BUILTINS` (nombre + opcode + regla de
tipado) + el opcode + su rama en cada motor. Cero cambios en parser/compilador (las llamadas a
builtin se resuelven por nombre) y **runtime determinista** → oráculo VM↔intérprete (incluyendo un
caso con `NaN`/infinito para fijar la semántica de borde de `f64`).

### 24.4 M15.1b — reloj y aleatoriedad (especificación)

A diferencia de las matemáticas, estos builtins son **no deterministas** (su resultado depende del
reloj o del RNG): no entran al oráculo VM↔intérprete; se prueban por **integración** (subproceso),
comprobando **propiedades** (rangos, monotonía) en vez de valores exactos, como el I/O de M11.2.

| Builtin | Firma | Semántica |
|---------|-------|-----------|
| `now` | `() -> int` | milisegundos desde la época Unix (reloj de pared). Opcode `Now`. |
| `monotonic` | `() -> int` | milisegundos de un reloj **monótono** (origen arbitrario; sirve para medir intervalos, no inmune a ajustes de hora). Opcode `Monotonic`. |
| `sleep` | `(int) -> unit` | duerme el hilo `ms` milisegundos (`ms<=0` → no duerme). Opcode `Sleep`. |
| `random` | `() -> float` | un `float` en `[0, 1)`. Opcode `Random`. |
| `random_int` | `(int) -> int` | un entero en `[0, n)` (`n<=0` → `0`, total, sin error de runtime). Opcode `RandomInt`. |

**El RNG sin dependencias.** `std` no trae generador de aleatorios y la invariante es **cero deps de
Cargo**, así que raylang lleva un PRNG propio: **SplitMix64**, sembrado del reloj la primera vez, en
un almacén de proceso del host (`OnceLock<Mutex<…>>`, como el registro de archivos de M11.8). No es
criptográfico —es para simulación/jitter/ids, no para secretos—. El generador vive en `builtins.rs`
(helpers compartidos `random_f64`/`random_int`/`now_millis`/`monotonic_millis`/`sleep_millis`), así
ambos motores usan el **mismo** flujo. `monotonic` ancla un `Instant` de referencia en su primera
llamada. Diferido: `seed_random(n)` (reproducibilidad), reloj de alta resolución (ns).

**Nota sobre concurrencia (M12).** En el modelo M:1, `sleep` **bloquea el hilo del SO** → bloquea
*todas* las fibras (no es un yield al scheduler). Es coherente con la decisión "bloqueante primero"
de §24.1; un `sleep` que ceda la fibra llegaría con el capstone M15.5.

### 24.5 M15.2 — cliente TCP (especificación)

El primer trozo de **redes de verdad**: conectarse a un servidor TCP, escribirle y leerle. Sobre
`std::net::TcpStream`, **cero deps**, reusando el **molde de handles** de M11.8.

| Builtin | Firma | Semántica |
|---------|-------|-----------|
| `tcp_connect` | `(host: string, port: int) -> Result<int, string>` | resuelve `host` (DNS vía `std::net`), abre la conexión, devuelve un **handle** (int). `Err(msg)` si falla. |
| `socket_read` | `(h: int) -> Result<string, string>` | **una** lectura del socket (hasta 64 KiB); devuelve lo leído como `string` (UTF-8 *lossy*). `""` = EOF (el otro extremo cerró). Bloquea hasta que haya datos. |
| `socket_write` | `(h: int, s: string) -> Result<int, string>` | escribe `s` completo; `Ok(nº de bytes)`. |
| `close` | `(h: int) -> int` | **ya existe** (M11.8); se extiende al handle de socket (cierra la conexión). Ad-hoc polimórfico (archivo / canal / socket). |

**El handle de socket reusa el registro de archivos.** `OpenHandle` gana una variante `Tcp(TcpStream)`
y los sockets viven en el **mismo** `FileRegistry` que los archivos (contador `next` compartido →
handles globalmente únicos). Así `close(h)` (que solo quita del mapa) cierra archivos *y* sockets sin
saber de cuál se trata, y el `Drop` de `TcpStream` cierra el descriptor. **El patrón de builtin:**
primitivos con **arreglo etiquetado** (`__tcp_connect`/`__socket_read`/`__socket_write -> [string]`,
`["ok", payload]`/`["err", msg]`) + envoltorios en el prelude que arman el `Result` (igual que
`open`/`read_file`; el handle se decodifica con `parse_int`, como `open`). El runtime no sabe de
`Result`.

**Lectura por trozos (chunked), no hasta EOF.** `socket_read` hace **una** llamada `read()` y devuelve
lo que venga: deja al código raylang **iterar** (acumular hasta `""` o hasta tener el cuerpo
esperado), que es justo lo que necesita el cliente HTTP de M15.4. Para no retener el `Mutex` del
registro durante una lectura/escritura **bloqueante**, los helpers **clonan** el `TcpStream`
(`try_clone`, un `dup` del descriptor) y sueltan el lock antes del I/O.

**Pruebas por subproceso (no oráculo).** Red no determinista: un test levanta un **servidor TCP de
juguete en el propio Rust** (un hilo que acepta una conexión, eco/respuesta fija), y el `.ray` se
conecta por el puerto efímero asignado; se comprueba el intercambio en ambos motores.

### 24.6 M15.3 — servidor TCP (especificación)

El otro lado: **escuchar** y **aceptar** conexiones. Con esto raylang puede escribir un servidor (un
echo, un HTTP mínimo). Sobre `std::net::TcpListener`.

| Builtin | Firma | Semántica |
|---------|-------|-----------|
| `tcp_listen` | `(host: string, port: int) -> Result<int, string>` | hace *bind* + *listen* en `host:port` y devuelve un **handle de escucha** (int). `port=0` → el SO asigna un puerto efímero. |
| `tcp_accept` | `(listener: int) -> Result<int, string>` | **bloquea** hasta una conexión entrante; devuelve un **handle de conexión** (un socket normal, usable con `socket_read`/`socket_write`/`close`). |
| `close` | `(h: int) -> int` | cierra tanto el handle de escucha como el de conexión (ya extendido en M15.2). |

**Misma cápsula de handles.** `OpenHandle` gana `Listener(TcpListener)`; escuchas y conexiones
conviven en el registro de M11.8 (handles únicos; `close` no distingue). Como `accept()` **bloquea**,
`tcp_accept` **clona** el `TcpListener` (`try_clone`) y suelta el lock antes de bloquear —idéntico
patrón que `socket_read`—; la conexión aceptada se inserta como un `OpenHandle::Tcp` y se devuelve su
handle. Patrón de builtin: primitivos `__tcp_listen`/`__tcp_accept -> [string]` etiquetados +
envoltorios `Result` en el prelude (handle decodificado con `parse_int`, como `open`/`tcp_connect`).

**Modelo de servidor (bloqueante, M:1).** El bucle natural es `loop { accept(); atender(conn) }`. En
el modelo M:1 de M12, `accept` y `socket_read` **bloquean el hilo** → un servidor bloqueante atiende
**una conexión a la vez** (o se lanza una fibra por conexión, pero como las fibras comparten el hilo,
una lectura bloqueante de una congela a las demás). El servidor **concurrente real** (aceptar y
atender en paralelo cediendo al scheduler) es el capstone M15.5. M15.3 entrega el servidor
secuencial, que basta para un echo y para servir peticiones cortas.

**Pruebas por subproceso.** Inverso de M15.2: el `.ray` es el **servidor** (escucha en puerto 0, lo
imprime, acepta una conexión, hace eco) y el **cliente de juguete en Rust** se conecta y verifica el
eco. Para descubrir el puerto efímero, el `.ray` lo imprime y el test lo lee de su stdout.

### 24.7 M15.4 — protocolos como librería en raylang (JSON + HTTP)

El cambio de registro de M15: hasta aquí, la base eran **builtins** (Rust). Ahora los **protocolos** se
escriben **en el propio raylang** y se traen con `import` —**cero líneas de Rust**, cero builtins—.
Es la materialización de la filosofía "lo que se puede escribir en el lenguaje, se escribe en el
lenguaje" y un *showcase* del sistema de módulos (M11) sobre la stdlib (string/Map/Result). Dos
librerías, en dos sub-fases:

- **M15.4a — JSON** (`examples/web/json.ray`): `parse`/`stringify` de JSON, **en raylang**. Determinista
  → se prueba por subproceso con salida exacta (golden) en ambos motores.
- **M15.4b — HTTP** (`examples/web/http.ray`): un cliente HTTP/1.1 (`get`/`request`) + parseo de la
  respuesta, **en raylang** sobre los builtins de TCP de M15.2. Se prueba contra un servidor HTTP de
  juguete en Rust.

**M15.4a — JSON (especificación).** Un valor JSON es un enum recursivo:

```raylang
pub enum Json { JNull, JBool(bool), JNum(float), JStr(string), JArray([Json]), JObject(Map<string, Json>) }
pub fn parse(s: string) -> Result<Json, string>   // descenso recursivo sobre la cadena
pub fn stringify(j: Json) -> string                // serialización
```

Decisiones: los **objetos** se modelan con `Map<string, Json>` (M13.1) → claves únicas y, al
serializar, **ordenadas** (`keys` es determinista) → `stringify` canónico y *round-trip* estable.
Los **números** son `float` (un solo caso, simple). El parser es un descenso recursivo clásico con un
`struct P { s, i, n }` mutado por referencia (semántica de struct de M3, como el lexer auto-alojado);
usa `s[i]`/`chars`/comparación de `char`/`substring`/`parse_float` de la stdlib. Errores como
**valores** (`Result`), nunca `panic`. Limitación documentada: los escapes `\uXXXX` no se soportan
(convertir un *code point* a `char` necesitaría un builtin nuevo; fuera de la filosofía "solo
librería"). Como toda librería de raylang, **el runtime no cambia**.

**M15.4b — HTTP (especificación).** Un cliente HTTP/1.1 en `examples/web/http.ray`, **en raylang** sobre
los builtins TCP de M15.2 (`tcp_connect`/`socket_write`/`socket_read`/`close`). API:

```raylang
pub struct Response { status: int, headers: Map<string, string>, body: string }
pub fn fetch(url: string) -> Result<Response, string>        // atajo GET (no `get`: choca con el de Map)
pub fn request(method: string, url: string, body: string) -> Result<Response, string>
pub fn header(r: Response, name: string) -> Option<string>   // búsqueda case-insensitive
```

El atajo se llama `fetch` y no `get` porque `get` ya es el accesor de `Map` en el prelude y raylang
**no tiene sobrecarga**: un `fn get` taparía al de `Map` dentro de `http.ray`, donde `header` lo
necesita (lección clavada por la propia implementación).

Decisiones: solo **`http://`** (TLS pediría criptografía, fuera de alcance); se parsea la URL
(`host[:port]/path`, puerto 80 por defecto) con `starts_with`/`index_of`/`substring`. La petición usa
**`Connection: close`** → el servidor cierra al terminar y el cliente **lee hasta EOF** (acumula
`socket_read` hasta `""`), que es la forma más simple y correcta de delimitar el cuerpo (sin necesidad
de `Content-Length`/*chunked*). La respuesta se parte en cabeceras/cuerpo por el **primer**
`\r\n\r\n` (con `index_of`, no `split`, que partiría también dentro del cuerpo); las cabeceras se
guardan en un `Map` con **nombre en minúsculas** (`to_lower`) para el lookup case-insensitive. Errores
como `Result`. **Cero runtime.** Se prueba contra un **servidor HTTP de juguete en Rust** (un hilo que
responde con cabeceras + cuerpo JSON y cierra); el driver combina `http` + `json` (hace `get` y parsea
el cuerpo con la librería JSON) → *showcase* de dos librerías de raylang componiéndose, en ambos
motores.

### 24.8 M15.5 — sockets no bloqueantes + el scheduler (el servidor concurrente, capstone)

El capstone de M15: que `tcp_accept`/`socket_read`, en vez de bloquear el hilo (y con él **todas** las
fibras, §24.6), **cedan la fibra al scheduler de M12**. Así, con `spawn`, un servidor atiende **muchas
conexiones concurrentes** sobre un único hilo:

```raylang
scope(fn() {
    while (seguir) {
        match (tcp_accept(srv)) {           // cede si no hay conexión pendiente
            Result.Ok(conn) => { spawn(fn() { atender(conn) }); },   // una fibra por conexión
            Result.Err(e) => { seguir = false; },
        }
    }
})
```

mientras una fibra espera datos de su conexión (`socket_read` cede), otra avanza. **Solo en la VM**
(la concurrencia es VM-only; el intérprete sigue con sockets **bloqueantes** y un único hilo).

**Cómo: sockets no bloqueantes + busy-poll cooperativo, cero dependencias.** `std` no expone
`epoll`/`kqueue`/`poll`, y la invariante es **cero deps de Cargo**. La solución honesta: la VM pone sus
sockets en modo **no bloqueante** (`set_nonblocking`); cuando `accept`/`read` devuelven `WouldBlock`,
el opcode **aparca la fibra** (rebobina el `ip` y la mete en una lista nueva `io_parked`, gemela de
`parked` pero sin handle de GC —un socket es un `int` del registro del host, no un objeto del heap—) y
conmuta. Cuando **no queda ninguna fibra lista** pero sí hay fibras en `io_parked`, el scheduler
**duerme ~1 ms y las re-encola todas** para que reintenten su operación (las que sigan sin estar listas
se vuelven a aparcar). Es un *poll loop* cooperativo: ineficiente comparado con `epoll`, pero simple,
sin deps y didáctico. Nunca hay *deadlock* mientras haya `io_parked` (se sigue sondeando); el
*deadlock* clásico (todas en `recv`/`join`) se conserva tal cual (solo cuando `io_parked` está vacío).

**Reparto intérprete/VM.** Los *builtins* compartidos (`builtins.rs`) **crean sockets bloqueantes**
(el intérprete los usa así, un solo hilo). La **VM** los voltea a no bloqueantes (`set_nonblocking`)
tras crearlos y usa helpers no bloqueantes (`socket_read_nb`/`tcp_accept_nb` → `Result<Option<…>,
String>`: `Ok(None)` = `WouldBlock`). Cero opcodes nuevos: se reusan `SocketRead`/`TcpAccept` (cambia
solo su ejecución en la VM); el resto de fases intacto. El GC rootea las fibras de `io_parked`;
`cancel_task` (M12.5) también las busca.

**Limitación documentada.** `socket_write` no es punto de cesión: en un socket no bloqueante hace un
bucle de escritura que **gira** (spin) si el buffer del SO está lleno. Para las cargas reales (líneas
de eco, respuestas HTTP cortas) nunca gira; una escritura gigante a un peer que no lee sí giraría. La
cesión en la escritura (con estado de *offset* entre cesiones) queda diferida. El *poll* de 1 ms añade
algo de latencia frente a `epoll` — aceptable para un lenguaje de aprendizaje.

**Prueba.** Un servidor de eco **concurrente** en `.ray` (escucha, `scope { loop accept → spawn
atender }`), **solo VM**, sirviendo 2 conexiones. El test conecta dos clientes y pide el eco del
**segundo** antes de que el primero envíe: un servidor secuencial bloqueante se quedaría atascado
leyendo al primero y nunca respondería al segundo; que el segundo reciba su eco **prueba** la
concurrencia (los clientes de Rust ponen *read-timeout* para fallar en vez de colgarse si se rompe).


## 25. M16 — El tipo `bytes` (datos binarios)

M15 dejó una deuda explícita: la carga útil de sockets y archivos es `string` (UTF-8 *lossy*), que
**corrompe datos binarios** (una imagen, un `.zip`, un protocolo binario). M16 añade el tipo **`bytes`**:
una secuencia **inmutable** de octetos (0–255), hermano de `string` (también inmutable) pero sin la
restricción de ser UTF-8 válido. Es el primer tipo nuevo desde `char` (M11.4c) y el cimiento de las dos
problemáticas siguientes (TLS sobre `epoll`; el backend nativo emite bytes).

### 25.1 Decisiones

- **Inmutable, hermano de `string`.** Como `string`, un `bytes` no se muta in situ; se construye por
  literal o concatenación. La representación en runtime espeja a la de `string` (en la VM, **inline**
  en el `HeapValue` —no es objeto del heap ni lo toca el GC—; en el intérprete, `Rc<Vec<u8>>` para clon
  barato). Coherente con que los strings sean inmutables.
- **Literal `b"..."`.** Estilo Rust/Python. Su contenido son los **bytes UTF-8** del texto, con los
  escapes de string (`\n \t \r \\ \"`) más **`\xNN`** (dos dígitos hex → un byte arbitrario), que es lo
  que permite escribir binario literal: `b"\x00\xff"`.
- **Indexar da `int`.** `b[i] -> int` es el octeto en esa posición (0–255); fuera de rango = error de
  ejecución (como arreglos/strings). No se introduce un tipo `byte`.
- **`len`, `==`, `+`** se extienden a `bytes` (longitud en octetos; igualdad estructural; concatenación).
- **Interoperación con `string`** vía builtins: `to_bytes(s) -> bytes` (codifica UTF-8) y
  `from_utf8(b) -> Result<string, string>` (decodifica; falla si no es UTF-8 válido → `Result`, patrón
  del prelude). **`print(bytes)`/`to_string(bytes)` → hexadecimal** (post-M19, helper `bytes_to_hex`
  compartido por ambos motores; `b"Hi\xff"` → `"4869ff"`; era diferido —"sin repr textual obvia"— y se
  resolvió con la forma honesta para binario: los octetos en hex, que casa con los digests de M19.3b).

### 25.2 Sub-fases

- **M16.1a — el tipo (núcleo).** `Type::Bytes` + keyword `bytes`; literal `b"..."` (lexer/parser/AST);
  valor en ambos motores; `len(bytes)`, indexar `b[i] -> int`, igualdad `==`. Determinista → **oráculo**.
- **M16.1b — interoperación con string.** `to_bytes`/`from_utf8` (builtins) + concatenación `b1 + b2`.
  Determinista → oráculo (estrés del GC para `to_bytes`, que asigna).
- **M16.1c — I/O binaria.** `read_file_bytes`/`write_file_bytes` y `socket_read_bytes`/`socket_write_bytes`:
  cierran la deuda de M15 (binario correcto). No determinista → subproceso. Los handles de M15 se reusan;
  `socket_read_bytes` **cede al scheduler** igual que `socket_read` (M15.5, no bloqueante en la VM). El
  arreglo etiquetado del prelude tiene un giro: como un arreglo de raylang es **homogéneo**, no puede
  mezclar el tag `string` con un payload `bytes`. Por eso las **lecturas** (`__read_file_bytes`/
  `__socket_read_bytes`) devuelven **`[bytes]`** con el tag *también* en bytes (`[b"ok", datos]` /
  `[b"err", msg_utf8]`); el prelude desempaqueta con `from_utf8` el mensaje de error. Las **escrituras**
  siguen con `[string]` (`["ok"]`/`["err", msg]`), pues su payload de éxito es solo el conteo (`len`).

Como `char`, el grueso es mecánico (literal + tipo + valor por motor) y el runtime solo crece donde es
inevitable; el checker/compilador apenas cambian (un builtin es una fila en la tabla).


## 26. M17 — `epoll`/`kqueue` (readiness real de E/S)

M15.5 dejó la concurrencia de red funcionando con un **busy-poll cooperativo**: cuando ninguna fibra
está lista pero hay fibras aparcadas esperando E/S (`io_parked`), el scheduler **dormía ~1 ms y las
re-encolaba todas** para que reintentaran su operación no bloqueante. Es simple y cero-deps, pero paga
dos costes: **latencia** fija (hasta ~1 ms aunque los datos lleguen antes) y **CPU** ociosa (despierta y
reintenta los N sockets cada milisegundo aunque ninguno esté listo). M17 lo sustituye por **notificación
de readiness del SO**: el scheduler se **bloquea** en el kernel hasta que algún socket esté realmente
listo para leer y despierta **solo** las fibras de esos descriptores. **Cero cambios observables** (mismo
output, mismo orden determinista); solo mejora latencia y CPU. Solo VM (la concurrencia es VM-only).

### 26.1 Decisiones

- **`kqueue` (macOS/BSD) + `epoll` (Linux), con fallback al busy-poll.** Cada SO tiene su API de
  readiness; las dos cubren las plataformas reales del proyecto. En cualquier otra (Windows) el poller
  reporta `Unsupported` y el scheduler **conserva el busy-poll de M15.5** → degradación honesta, nunca un
  fallo.
- **FFI propio, no el crate `libc`.** La invariante del proyecto es **cero dependencias de Cargo** y
  `std` no expone `epoll`/`kqueue`/`poll`. La solución honesta: declarar nosotros los pocos `extern "C"`
  que hacen falta (`kqueue`/`kevent`, `epoll_create1`/`epoll_ctl`/`epoll_wait`, `close`). Viven en
  libSystem (macOS) / libc (Linux), **siempre enlazados** → no son una dependencia, solo FFI con `unsafe`
  **acotado** (encapsulado en `src/poll.rs`). Los descriptores salen de `std` vía `AsRawFd::as_raw_fd()`.
- **Bloqueo infinito, sin timeout.** Cuando todas las fibras están en E/S, no hay nada más que hacer →
  esperar indefinidamente en el kernel es correcto (el programa genuinamente espera a la red) y no quema
  CPU. El *deadlock* de canal/tarea (M12) se conserva tal cual (solo aplica cuando `io_parked` está vacío).
- **Despertar selectivo.** `io_parked` pasa de `Vec<Fiber>` a `Vec<IoParked { fd, fiber }>`: en cada
  sitio de parking (`SocketRead`/`SocketReadBytes`/`TcpAccept`) se guarda el `fd` del socket
  (`builtins::raw_fd`). El scheduler registra todos los fds en el poller, espera, y re-encola **solo** las
  fibras cuyo fd quedó listo; las demás siguen aparcadas (la ganancia real frente al busy-poll).

### 26.2 Cómo

`src/poll.rs` expone `wait_readable(fds, timeout_ms) -> PollResult` (`Ready(listos)` | `Unsupported`),
con tres ramas por `cfg`: kqueue (una sola llamada a `kevent` registra el changelist y espera el
eventlist → un syscall), epoll (`epoll_create1` + N×`epoll_ctl` + `epoll_wait`), y un fallback que
devuelve `Unsupported`. Un poller efímero por espera (crear/registrar/esperar/cerrar): solo ocurre cuando
**todas** las fibras están bloqueadas (ocioso), no es ruta caliente. La gestión del scheduler vive en
`Vm::schedule_next`, reescrito como **bucle**: saca la siguiente lista; si no hay y hay `io_parked`, llama
a `io_wait` y reintenta; si no, es deadlock o fin. `io_wait` espera readiness y despierta selectivamente;
si el poller no está o la espera vuelve vacía (EINTR), **cae al busy-poll** (duerme 1 ms y re-encola
todas) → **siempre hay progreso**. El GC (rootea las fibras de `io_parked`) y `cancel_task` (M12.5) se
adaptan al nuevo struct. Cero opcodes nuevos; el resto de fases intacto.

### 26.3 Prueba

El comportamiento no cambia, así que la **regresión** es la garantía: los tests de M15.5 / red concurrente
(`tests/concurrency_net_cli.rs`, el servidor de eco concurrente sobre `spawn`) siguen verdes, ahora
ejercitando el camino `kqueue` real en macOS. Que el servidor concurrente atienda a dos clientes en
desorden **prueba** que el readiness del SO desbloquea las fibras correctas.

### 26.4 Cesión en `socket_write` (post-M19)

Cerró el diferido principal de §26.4. Antes, una escritura que llenaba el buffer de envío del SO **giraba**
(`yield_now`) en `socket_write_raw` hasta poder seguir; en el modelo M:1 eso significaba que esa fibra
**acaparaba el hilo** del scheduler (las demás no avanzaban). Ahora la escritura **cede la fibra** como ya
hacían las lecturas, con dos piezas:

- **El poller gana interés de escritura.** `wait_readable(fds)` pasa a `wait(read_fds, write_fds)`: en
  `kqueue` se registra `EVFILT_WRITE` para los fds de escritura (eventos separados de `EVFILT_READ`); en
  `epoll`, `EPOLLOUT` (combinando `EPOLLIN|EPOLLOUT` por fd para no dar `EEXIST` al añadir el mismo fd
  dos veces). Devuelve los fds listos (de lectura **o** escritura); el scheduler casa por fd.
- **Escritura parcial + estado a través de la cesión.** `socket_write_nb` escribe lo que quepa y devuelve
  cuántos octetos entraron. Si no entró todo, la VM **aparca** la fibra con interés de escritura, guardando
  los octetos que faltan en `IoParked.pending_write` (en vez de re-pushear operandos como las lecturas —
  así no hay que reconstruir un `string` partido a mitad de carácter multibyte). Al despertar (socket
  escribible), `finish_parked_write` drena lo que falta; si completa, **empuja el resultado etiquetado**
  (`["ok",""]`) en la pila de la fibra y la pone lista; si aún bloquea, la re-aparca. Como `allocate` no
  colecta fuera de los safepoints del bucle, empujar el resultado en `io_wait` es seguro para el GC.

Cubre las dos formas (`SocketWrite` string y `SocketWriteBytes`; la de string convierte a octetos primero).
TLS sigue con su propia bomba (busy-spin en el raro bloqueo, M19.4b). **Prueba conductual**
(`tests/socket_write_cli.rs`): un servidor escribe un blob de 8 MB a un cliente que **no drena** (su
escritura se aparca) y a la vez atiende a un segundo cliente por completo. Con el viejo busy-spin el test
**se cuelga 15 s y falla** (el hilo gira en la escritura bloqueada y el 2.º cliente nunca es atendido); con
la cesión pasa en <0,5 s. Solo VM.

### 26.5 Diferido

Un registro **persistente** del poller (re-registrar fds entre esperas en vez de un poller efímero — más
eficiente con muchas conexiones, pero exige gestión de altas/bajas); `epoll`/`kqueue` *edge-triggered*;
`bytes`/bitops en el toolchain auto-alojado. El `unsafe` queda confinado a `src/poll.rs` (los syscalls),
con su contrato documentado.


## 27. Optimización de la VM de Rust (transversal)

Con el backend nativo (M18) **aparcado** por decisión del usuario, el hilo de rendimiento abierto es
optimizar la VM de bytecode existente. El principio rector, fijado con el usuario, es **incremental y
midiendo**: nada de optimizar a ciegas. Cada cambio se mide antes y después, y **solo se conserva si la
mejora supera el ruido** de medición; el oráculo VM↔intérprete debe quedar intacto en cada paso (las
optimizaciones cambian *cómo* se ejecuta, nunca *qué* resultado se produce).

### 27.1 El banco de pruebas (`benchmarks/`)

El banco vive en `benchmarks/`, con dos arneses y varias cargas que estresan ejes distintos de la VM:

- `benchmarks/bench.sh` — compara intérprete vs. VM con **hyperfine** (`fib.ray`, `strings.ray`).
- `benchmarks/measure.py` — alternativa **sin hyperfine** (solo python3): corre cada caso N veces sobre
  el binario de **release** y reporta el **mejor tiempo** (mejor-de-N filtra el ruido del planificador del
  SO mejor que la media). Compara la misma carga entre builds; el arranque (parse/check/compile) es coste
  constante → los deltas son fieles.
- Cargas: `fib.ray`/`fib35.ray` (recursión: llamadas, marcos, despacho), `loop.ray` (bucle aritmético
  apretado: pila, saltos, aritmética entera), `arrays.ray` (asignación en heap + GC), `gcnested.ray`
  (arreglos de arreglos → el GC traza objetos con **hijos en el heap**, no solo primitivos).

No se usa `cargo bench` (pediría `criterion`, una dependencia → rompería la invariante cero-deps). El
ruido típico observado es ~3–5 %; una optimización debe superarlo con holgura y de forma **consistente
entre cargas** para considerarse real.

(Numeración: **Opt.1** «instrucción prestada» y **Opt.2** «pool de locales» se aplicaron en un pase
previo; **Opt.3** = `Rc<str>` se evaluó y descartó —ver `IDEAS.md` §11—. De ahí que lo de este pase
empiece en Opt.4.)

### 27.2 Resultados

- **Opt.4 — fast-path entero** (sobre Opt.1/Opt.2). En el brazo de operaciones binarias del lazo, si
  ambos operandos son `Int` (el caso dominante en bucles y recursión aritmética) se resuelve la operación
  **en el sitio**, evitando el doble match (el `bin @ (...)` del lazo + el rematcheo de opcode y ~30
  combinaciones de tipos dentro de `apply_binary`) y la llamada a `apply_binary`. La semántica es
  **idéntica** al camino general (mismos `+`/`-`/`*`/…; en debug ambos hacen panic al desbordar) → el
  oráculo no se entera. Medido (mejor de 5, release): **fib(35) −5 %, bucle 10M −6 %**; `arrays` sin
  cambio (no es aritmético, como se esperaba).
- **Perfil de release (LTO + `codegen-units=1`): descartado.** La hipótesis natural (inline a través de
  módulos del lazo de despacho) **no se materializó**: medido, tanto LTO «fat» como «thin» salieron
  iguales o ligeramente peores que el perfil por defecto. El perfil se quedó por defecto. (Ejemplo
  canónico de por qué se mide antes de comprometer.)
- **Opt.7 — posición `(línea, col)` perezosa.** El lazo de despacho leía `chunk.lines[ip]` (una `(usize,
  usize)`) **por cada instrucción**, pero el camino caliente (locales, constantes, aritmética, saltos)
  **nunca la usa** —solo los sitios de error o de cesión del scheduler—. Ahora se resuelve **bajo
  demanda** con un macro `pos!()` (lee `lines[ip]` solo donde hace falta), quitando una lectura de
  memoria de cada iteración del lazo. Medido (mejor de **15** —best-of-5 no resolvía la señal del ruido,
  ver abajo): **fib(35) −7 %, bucle 10M −9 %, arrays −8 %**. **Consistente entre las tres cargas** (toda
  instrucción pasa por el lazo) y correcto (oráculo + tests de posición de error intactos: la posición se
  calcula igual, solo que perezosamente). **Lección de medición**: el efecto (~8 %) quedaba *enmascarado*
  por la varianza de mejor-de-5 (la baseline saltaba ±4 % entre corridas); subir a mejor-de-15 lo destapó
  limpio. Best-of-N con N grande filtra el ruido del planificador mejor que N pequeño.
- **M36.1 — superinstrucciones** (arranca el arco B, §44/PRODUCCION). El coste dominante de una VM de
  *switch* es el **despacho** (el `match` gigante + el avance del `ip`) por instrucción. `GetLocal` es el
  opcode más frecuente y casi siempre carga un operando seguido de otro `GetLocal` o una `Constant`. Un
  *peephole* (`fuse_superinstructions`, tras el TCO) fusiona esos pares en un opcode: `GetLocal(s);
  GetLocal(t)` → `GetLocalLocal(s,t)` y `GetLocal(s); Constant(c)` → `GetLocalConst(s,c)`, que hacen los dos
  empujes en **una** iteración del lazo. Fusionar acorta el código → **desplaza los índices**, así que el
  pase **remapea los destinos de salto** (`Jump`/`JumpIfFalse`, los únicos con destino de código) vía un
  mapa viejo→nuevo, y **no fusiona** si el segundo opcode es destino de un salto (algo aterrizaría entre
  medias). Semántica idéntica (mismos empujes, mismos saltos) → oráculo intacto (441 tests + suite completa).
  Medido (mejor de 15, release): **fib(35) −8 %, bucle 10M −9,5 %, arrays −11 %, gcnested −10 %** —
  **consistente en las cuatro cargas** (toda función pasa por el fusor) y muy por encima del ruido. Es el
  primer win estructural (vs. los micro de Opt.4/7); la baseline se actualiza a estos números.

### 27.3 Medidas y rechazadas (la disciplina en acción)

- **Opt.5 — `new_locals` sin el branch por slot para funciones sin capturas** (flag `has_captures`
  precomputado + `resize` en vez del bucle con `captured.get(s)`): **medido dentro del ruido** → revertido.
  La causa: las funciones calientes (p. ej. `fib`) tienen pocos locales, el branch estaba bien predicho.
- **Opt.6 — safepoint del GC amortizado** (chequear `should_collect()` 1 de cada N instrucciones): techo
  medido ~2-3 % en fib/loop, pero **incorrecto** — rompe el **modo estrés** del GC (que colecta en cada
  punto seguro para cazar raíces faltantes). Capturarlo bien exigiría mover el safepoint a solo los sitios
  de asignación + back-edges preservando el estrés: rediseño con riesgo sobre el test sagrado del GC →
  diferido, no compensa por ~2-3 %.
- **Opt.8 — `children()` del GC con buffer reusado** (un `Vec` por `trace` en vez de uno por objeto
  trazado, vía `collect_children` que vuelca en un buffer compartido): **medido dentro del ruido**, incluso
  con un benchmark nuevo `gcnested.ray` (arreglos de arreglos → objetos con hijos en el heap) → revertido.
  Causa: el `trace` solo corre en una recolección (infrecuente; el umbral crece ×2 con la población viva),
  y asignar un `Vec` pequeño no es el cuello de botella; para arreglos de `int` (la carga `arrays`) los
  hijos son primitivos → `children` ya devolvía un `Vec` vacío (que no asigna). El benchmark se conserva.
- **M37.2 — `children()` con buffer reusado, re-medido a escala** (misma idea que Opt.8 pero con el
  benchmark de heap grande de M37.1, 300k objetos vivos, midiendo la **pausa** del GC). Hipótesis: a 300k
  objetos, las 300k asignaciones de `Vec` sí pesarían. **Medido (5 corridas cada uno)**: pausa máxima
  mediana **8,87 ms (sin buffer) vs 8,96 ms (con buffer)**, media 1,12 vs 1,14 ms, total 15,7 vs 16,0 ms —
  **distribuciones solapadas, dentro del ruido** (una sola corrida engañaba con 8,88 vs 9,79) → **revertido**.
  **Dato clave para M37**: la pausa la domina el **recorrido O(heap)** (tocar 300k objetos → fallos de
  caché), no la asignación (muchas listas de hijos son de tamaño 1 → el `Vec` es diminuto). Corolario: **no
  hay tweak barato que baje la pausa stop-the-world de forma acotada**; el objetivo <1 ms exige la estructura
  (marcado/barrido incremental con *write barrier*, o el heap-por-actor de M38). Confirma §27.5.

### 27.4 Pendiente / ideas a medir

Reducir `HeapValue` de 32→16 bytes (boxear `Str`/`Bytes`; mucho churn, a ese tamaño el memcpy ya es
barato → probablemente no pague); evitar el clon de constantes `Str`/`Bytes` en bucles (internado o `Rc`);
reducir el coste del `children()` del GC (hoy asigna un `Vec` por objeto trazado, solo afecta a cargas
GC-pesadas). Cada una **se acepta solo si la medición la respalda**.

### 27.5 M37.1 — instrumentación de pausas del GC (medir antes de optimizar)

M37 (arco B) busca **pausas del GC acotadas** (<1 ms). Fiel a la disciplina de §27, **primero se mide**:
- **Instrumentación**: la VM cronometra cada `collect()` (una recolección stop-the-world) con `Instant` y
  acumula cuenta + pausa máxima/media; con `RAYLANG_GC_STATS=1` las imprime a stderr al terminar. Coste
  nulo desactivado (un `Instant` por recolección, que es rara).
- **Benchmark de heap grande** `benchmarks/gcpause.ray`: conserva ~300k arreglos vivos y genera basura
  para forzar GCs sobre ese heap grande (cada uno marca todo lo vivo + barre todos los slots).
- **Resultado (release, M3 Pro)**: 14 recolecciones, **pausa máxima 9,8 ms**, media 1,3 ms. La pausa está
  **~10× por encima del objetivo** → el problema es real a escala y el trabajo de M37 está justificado por
  datos. La pausa = marcar (O(vivos)) + barrer (O(slots)), ambos crecen con el heap.

**Camino a <1 ms** (fijado con el dato): acotar la pausa de forma **independiente del tamaño del heap** exige
**incrementalidad** —marcado incremental (tri-color + *write barrier*) + barrido incremental— **o** el
**heap-por-actor de M38** (heaps pequeños e independientes → pausa corta por construcción). El plan ya avisa
que M37 y M38 **se diseñan juntos** y que el heap-por-actor **simplifica** este GC; el *write barrier* es la
pieza de más riesgo (un barrier omitido = objeto vivo recolectado = corrupción). Por eso la incrementalidad
plena se **co-diseña con M38**; entretanto se recortan costes de la recolección stop-the-world que valen bajo
cualquiera de los dos futuros (M37.2+).

**M37 CERRADO por el heap-por-fibra de M38.1b** (§46.5). Medido (release, M3 Pro) con `gcpause.ray`
(1 actor, heap grande) vs. `gcpause_concurrent.ray` (60 fibras, la misma carga repartida): la pausa máxima
del GC cae de **10,5 ms** (un heap de 300k) a **0,12 ms** (cada GC recolecta el heap de una fibra pequeña),
un **87× menos** y muy por debajo del objetivo **<1 ms** — **por construcción**, sin marcado incremental ni
*write barrier* (la pieza de más riesgo, así evitada). Es la ruta que el plan anticipó: acotar la pausa vía
heaps por actor en vez de un GC incremental. (Un solo actor con un heap enorme sigue teniendo una pausa
grande —inherente—; lo que M38.1 garantiza es que **ninguna colección detiene a los demás actores** y que
cada heap está acotado.)


## 28. M19 — La capa web (servidor HTTP, SSE, WebSockets, TLS)

M15 dio el transporte TCP (builtins) + un cliente HTTP y JSON como **librerías en raylang**, y M15.5/M17
el servidor **concurrente** (una fibra por conexión, `accept`/`read` ceden al scheduler, readiness por
`kqueue`/`epoll`). M16 añadió `bytes` (octetos crudos) y lo cableó en los sockets. Sobre esa base, M19
construye la **capa de aplicación web**, en cuatro puntos de dificultad creciente. La filosofía de M15 se
mantiene: **transporte/cómputo = builtins; protocolos = librería en raylang** (cero runtime salvo donde
sea inevitable). El gran condicionante es la **invariante cero-deps de Cargo**, que choca de frente con
TLS (M19.4).

### 28.1 M19.1 — servidor web async + SSE

Una librería `examples/web/webserver.ray` (como `http.ray`/`json.ray`: importable, cero runtime) sobre el
servidor concurrente de M15.5/M17. Da el "servidor web async" de verdad: muchas conexiones a la vez en un
hilo. Piezas:

- `Request { method, path, headers: Map<string,string>, body }` y `Response { status, headers, body }`
  (espejo del `Response` del cliente HTTP de M15.4b).
- `read_request(conn) -> Result<Request, string>`: acumula `socket_read` hasta `\r\n\r\n` (fin de
  cabeceras), parsea la línea de petición + cabeceras (nombre en minúsculas para lookup); si hay
  `Content-Length`, sigue leyendo hasta completar el cuerpo.
- `send_response(conn, Response)`: serializa línea de estado + cabeceras (+ `Content-Length`,
  `Connection: close`) + cuerpo y `socket_write`.
- Atajos: `ok(body)`, `text(status, body)`, `not_found()`, `json_response(body)`.
- `serve(host, port, handler: fn(Request) -> Response)`: `tcp_listen` + bucle `accept → spawn(atender)`,
  concurrente (cada conexión en su fibra). `serve_raw(host, port, handler: fn(Request, int) -> unit)` da
  control total de la conexión (lo necesita SSE); `serve` se define sobre `serve_raw`.
- **SSE (server-sent events)**: es HTTP que no cierra. `sse_open(conn)` escribe `200` +
  `Content-Type: text/event-stream` + `Connection: keep-alive`; `sse_event(conn, data)` escribe
  `data: <…>\n\n`; el handler (vía `serve_raw`) hace `sse_open` y luego un bucle de `sse_event`. **Cero
  runtime nuevo**: todo es `socket_write` de strings sobre el servidor concurrente.

**Prueba** (`tests/webserver_cli.rs`, subproceso, **solo VM** —la concurrencia lo es—): un servidor `.ray`
acotado (sirve N conexiones vía `scope` y termina, imprime su puerto) que importa `webserver.ray`; un
cliente de Rust hace un `GET`, comprueba estado/cabeceras/cuerpo; y un caso SSE que lee varios eventos
`data:`. Mismo molde que `http_cli.rs` (copiar la librería + un `main.ray` driver al temporal).

### 28.2 M19.2 — HTTP en `bytes`  ✅

`http.ray` (cliente) y `webserver.ray` (servidor) usaban `socket_read`/`socket_write` **string** → un
cuerpo binario (imagen, `.zip`) se corrompía (UTF-8 lossy). M19.2 porta el cuerpo (de petición y
respuesta) a **`bytes`**: cabeceras como texto (ASCII, vía `from_utf8` para parsear) y cuerpo como
octetos crudos. Cierra la coherencia binaria de M16 en la capa de protocolo.

**No fue front-end puro** (estimación inicial errónea): separar cabeceras de cuerpo en un buffer de
`bytes` exige **cortar bytes**, y no existía. Se añadió un builtin: **`sub_bytes(b, i, j) -> bytes`**
(sub-secuencia por octeto, con *clamp*; análogo binario de `substring`), siguiendo el patrón de M16/
M11.7a (fila en la tabla + opcode `SubBytes` + impl por motor + helper `sub_bytes_octets` + oráculo).
El separador `\r\n\r\n` se localiza con un escaneo por octetos en raylang (`b[i]` da `int`), sin builtin.

**API**: `Response.body`/`Request.body` pasan a `bytes`; los atajos de texto (`ok`/`text`/`json_response`)
aceptan `string` y codifican con `to_bytes`; `bytes_response(status, body: bytes)` para cuerpos binarios;
`body_text(r)`/`request_text(r)` (= `from_utf8`) para leer texto. `Content-Length` ahora es octetos
(antes nº de caracteres → ligeramente incorrecto con no-ASCII; ahora correcto). El cliente envía la
petición con `socket_write_bytes(to_bytes(...))`. **Solo `bytes` toca runtime** (el builtin `sub_bytes`);
el resto es la librería. `http.ray`/`webserver.ray` salen del corpus del parser auto-alojado (no soporta
`bytes`, como `binario.ray`). **Prueba**: round-trip binario (`\x00`/`\xff`) por subproceso —el servidor
eco-devuelve el cuerpo de un POST y el cliente verifica los octetos crudos— + la composición HTTP+JSON
del cliente vía `body_text`.

### 28.3 M19.3 — WebSockets `ws://`

Dos partes: (1) **handshake** — el cliente manda `Upgrade: websocket` + `Sec-WebSocket-Key`; el servidor
responde con `Sec-WebSocket-Accept = base64(SHA-1(key + GUID))`. **SHA-1 y base64 son cómputo puro →
se escriben en raylang** (sin builtins, sin deps), operando sobre `bytes` (M16). (2) **framing** — los
mensajes van en tramas binarias (FIN/opcode/mask/length); se leen/escriben con `bytes`. **Cero runtime
nuevo** (SHA-1/base64 en raylang + el framing con `bytes`/sockets ya disponibles); ambicioso pero
autocontenido. `wss://` (sobre TLS) depende de M19.4.

**M19.3a — operadores bit a bit** ✅ (habilitador, **único toque de lenguaje** de M19.3). SHA-1 y el
framing necesitan `& | ^ ~ << >>` sobre `int`, que raylang no tenía. Decisión: **operadores**, no
builtins —`(a & b) | (c << 2)` lee infinitamente mejor que `bor(band(a,b), shl(c,2))` y es lo
pedagógicamente completo (precedencia bit a bit es un clásico)—. Tokens nuevos (`Amp`/`Pipe`/`Caret`/
`Tilde`/`Shl`/`Shr`; `&`/`|` sueltos dejan de ser error léxico), `BinaryOp::{BitAnd,BitOr,BitXor,Shl,
Shr}` + `UnaryOp::BitNot`, niveles de precedencia estilo C (`|` < `^` < `&` < igualdad; shift entre
comparación y aditivo), opcodes en la VM. Semántica `wrapping_*` sobre `i64` (sin panic, cuenta mod 64),
**idéntica en ambos motores** (oráculo `bitops_oraculo`). **Gotcha del lexer**: `>>` choca con genéricos
anidados (`Caja<Caja<int>>`); el lexer siempre emite `Shr` y el parser lo **parte** en dos `>` al cerrar
argumentos de tipo (`close_type_angle`, estilo Rust/Java). Diferido: bitops en el toolchain auto-alojado
(como `bytes`; `selfhost/lexer.ray` aún no los tokeniza → fuera del corpus del oráculo de self-hosting).

**M19.3b — SHA-1 + base64 (cómputo puro)** ✅. Ambos **escritos en raylang** (`examples/web/sha1.ray`,
`examples/web/base64.ray`), **cero runtime nuevo**: sobre los bitops de M19.3a + `bytes` (M16) + la stdlib
de strings. `sha1(msg: bytes) -> [int]` (digest de 20 octetos; `sha1_hex` para la forma hex);
`base64(data: [int]) -> string`. Clave: no hicieron falta builtins nuevos —leer octetos es `b[i]`
(indexado de `bytes`, M16.1a, ya da `int`) y el digest se modela como `[int]`—; SHA-1 es aritmética de
32 bits sobre el `int` de 64 (se enmascara con `& 0xFFFFFFFF` y se rota con `rotl`). Verificado contra
**vectores estándar** (RFC 3174 SHA-1, RFC 4648 base64) y el **accept canónico del RFC 6455**
(`base64(SHA-1("dGhlIHNhbXBsZSBub25jZQ==" + GUID)) = s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`), idéntico en
intérprete y VM (`tests/websocket_cli.rs`, driver `examples/web/crypto_demo.ray`). Diferido en el toolchain
auto-alojado (bitops, como `bytes`).

**M19.3c — handshake + framing + echo server** ✅ → **M19.3 COMPLETO**. Librería `examples/web/websocket.ray`
(sobre `sha1`/`base64` de M19.3b): handshake (`extract_key` de la petición de upgrade →
`handshake_response` con el `Sec-WebSocket-Accept`) + framing (`decode_frame` des-enmascara la trama
del cliente; `encode_frame`/`encode_text` construyen la del servidor, sin máscara; FIN/opcode/longitud
de 7/16/64 bits). **Único toque de runtime de M19.3c**: el builtin `bytes_of([int]) -> bytes` (dual del
indexado `b[i]`, que ya leía un octeto; oráculo `bytes_of_oraculo`) para *construir* tramas octeto a
octeto; el resto es `bytes` + `+` (concatenación) + bitops. Echo server real `examples/web/websocket_echo.ray`
(handshake + bucle de eco hasta `close`, secuencial, **solo VM**). Verificado de extremo a extremo con
un cliente WebSocket en el test (`tests/websocket_cli.rs`): handshake con el accept canónico + ida y
vuelta de tramas de texto enmascaradas + close. Alcance pedagógico (camino feliz, una trama por lectura,
sin fragmentación/extensiones). `wss://` (sobre TLS) sigue dependiendo de M19.4.

### 28.4 M19.4 — TLS / SSL

El **bloqueo duro**. `https`/`wss` exigen TLS: criptografía real (AEAD tipo AES-GCM/ChaCha20-Poly1305,
intercambio de claves ECDHE, certificados X.509, …). Implementarla a mano con seguridad es inviable, y
la **invariante cero-deps** prohíbe `rustls`/`native-tls`.

**Decisión (con el usuario): opción (a) — excepción explícita a la invariante con `rustls`.** Es la
primera dependencia de Cargo del proyecto, una **excepción consciente y acotada** (registrada en
IDEAS.md): TLS es el único dominio donde "hazlo a mano" es irresponsable (criptografía), así que se delega
en una librería revisada en vez de inventar. `rustls` (Rust puro, sin OpenSSL) arrastra un árbol
transitivo (su proveedor criptográfico + `webpki`/roots), de modo que "una dependencia" es el *grafo* de
rustls, no un único crate — el espíritu de la excepción es "una sola **decisión** de dependencia, en un
solo dominio (TLS)". El resto del lenguaje sigue cero-deps.

**Arquitectura**: igual que los sockets (M15) e I/O con handles (M11.8), una **sesión TLS vive en el
almacén del host** (un `Mutex<HashMap<i64, TlsConn>>`), y el handle es un `int`. Builtins nuevos envuelven
el stream; lecturas/escrituras reutilizan el patrón `Result`/arreglo etiquetado.

Sub-fases:
- **M19.4a — cliente TLS (bloqueante) + `https://`** ✅. `tls_connect(host, port) -> Result<int,string>`
  (handshake rustls sobre un `TcpStream` **bloqueante**); el handle es un `OpenHandle::Tls` en el **mismo
  registro** que sockets/archivos, así que **`socket_read_bytes`/`socket_write_bytes`/`close` lo manejan
  transparentes** (desvían al camino TLS bloqueante) → `http.ray` solo cambia `parse_url` (acepta
  `https://`, puerto 443) y elige `tls_connect` vs `tcp_connect`. La VM no pone el socket TLS en no
  bloqueante (rustls es síncrono; `SocketReadBytes` detecta el handle TLS y lee bloqueando, sin ceder).
  Verificación de certificado con las raíces de Mozilla (`webpki-roots`) + **`SSL_CERT_FILE`** para CAs
  extra (como curl). Test **determinista y sin red**: un servidor TLS local en el propio test (rustls +
  CA autofirmada de `tests/fixtures/`), el cliente raylang confía en esa CA vía `SSL_CERT_FILE`
  (`tests/tls_cli.rs`, ambos motores); demo `examples/web/https_demo.ray`. Verificado también contra HTTPS
  público real (`fetch("https://example.com/")`). **Único toque que NO es runtime puro: las deps de TLS.**
- **M19.4b — servidor TLS + `wss://`** ✅ → **M19.4 / M19 COMPLETOS**. `tls_accept(handle, cert, key)`
  envuelve una conexión TCP ya aceptada en una sesión TLS de **servidor** (reusa el handle). Lo difícil
  era la **integración no bloqueante** (enfoque elegido frente al primer-cut bloqueante): se conduce la
  máquina de estados de rustls a mano (`read_tls`/`write_tls`/`process_new_packets` sobre el enum
  unificado `rustls::Connection`, que vale para cliente y servidor) y, si haría falta **leer** del peer y
  el socket bloquearía, se devuelve "WouldBlock" → la VM **aparca la fibra en el fd** (el mismo mecanismo
  `io_parked`/poller de M15.5/M17 que los sockets planos; `raw_fd` del TLS = el fd subyacente). Las
  **escrituras** (handshake/datos, casi siempre pequeñas) se drenan girando en el raro `WouldBlock`
  (el poller de M17 solo notifica lectura). **Unificación clave**: la MISMA bomba sirve a los dos motores
  — sobre el socket **bloqueante** del intérprete, `read_tls` simplemente bloquea (nunca da WouldBlock),
  así que el intérprete no necesita un camino aparte (se eliminó `rustls::Stream`, que además no acepta
  el enum `Connection`). `tls_server_config` carga cert/clave PEM (`with_single_cert`). El echo server
  `examples/web/wss_echo.ray` = el de M19.3c + un `tls_accept` tras `tcp_accept`: todo el I/O (upgrade HTTP +
  tramas) viaja cifrado porque `socket_read_bytes`/`socket_write_bytes` se desvían a TLS. Verificado de
  extremo a extremo con un cliente WebSocket-sobre-TLS en el test (`tests/tls_cli.rs`): handshake con el
  accept canónico + tramas enmascaradas + close, sobre el scheduler no bloqueante. `wss://` real.

**M19 COMPLETO** (servidor web + SSE · HTTP en bytes · WebSockets `ws://` · TLS cliente y servidor →
`https`/`wss`). La invariante cero-deps se rompe **solo** en TLS (excepción consciente §28.4).

## §29 — M20: la capa de cripto, identidad y clientes cloud (librerías raylang)

Sobre el transporte (M15), la concurrencia (M15.5/M17), `bytes` (M16) y la web (M19), M20 construye la
capa que un servicio **cloud/distribuido** real necesita: criptografía moderna, identidad (tokens),
formatos de la web (URL/cookies), tiempo, y clientes de infraestructura. **Filosofía idéntica a M15/M19**:
todo lo que pueda escribirse en raylang se escribe en raylang (cero runtime nuevo), apilándose sobre los
operadores bit a bit (M19.3a) y `bytes` (M16). El runtime solo se toca para lo que es físicamente
imposible en el lenguaje (UDP, componentes de fecha UTC). Plan por fases (cada una compila y conserva el
oráculo VM↔intérprete; las librerías cripto son cómputo puro determinista → se verifican contra vectores
estándar por ambos motores):

- **M20.1 — SHA-256** ✅ (`examples/web/sha256.ray`). FIPS 180-4 en raylang puro, gemelo de `sha1.ray`:
  aritmética de 32 bits enmascarada (`& mask32()`), `rotr` (rotación a la derecha), tabla de 64
  constantes de ronda, planificación de mensaje de 64 palabras. Salida `[int]` de 32 octetos o hex con
  `sha256_hex`. Verificado contra los vectores NIST (`""`, `"abc"`, mensaje multi-bloque, fox/avalancha)
  por ambos motores (`tests/sha256_cli.rs`). Cimiento de HMAC/JWT/firma de requests.
- **M20.2 — HMAC-SHA256 + base64url + hex genérico** ✅ (`hmac.ray`, `hex.ray`, `base64.ray` ampliado).
  HMAC (RFC 2104) sobre SHA-256 en raylang puro: `SHA256((K'^opad) || SHA256((K'^ipad) || m))`, con la
  clave ajustada al bloque de 64 octetos. Habilitador: `sha256.ray` expone `sha256_octets([int])`
  (entrada por octetos) para encadenar hashes sin pasar por `bytes`. `hex_encode`/`hex_decode` (con
  `Result`; sin aritmética de `char` —no soportada— vía `index_of` en la tabla de dígitos).
  `base64url`/`base64url_decode` (RFC 4648 §5, alfabeto URL-safe `-`/`_`, sin relleno) para JWT.
  Verificado contra RFC 4231 + `openssl` por ambos motores (`tests/hmac_cli.rs`), incl. clave > bloque
  y round-trips. Cimiento de JWT (M20.3).
- **M20.3 — JWT (HS256) + UUID v4** ✅ (`jwt.ray`, `uuid.ray`). El *capstone* del cimiento cripto:
  `jwt_sign(secret, payload_json) -> token` y `jwt_verify(secret, token) -> Result<payload_json, msg>`
  apilando HMAC-SHA256 + base64url + `bytes`. El payload se pasa/devuelve como **JSON crudo** (raylang
  es tipado y las claims son heterogéneas → el JSON lo arma el usuario, opcionalmente con `json.ray`).
  Verificación con comparación en tiempo (casi) constante (`const_eq`, no filtra por el primer byte);
  NO comprueba `exp` (política de la app, sobre el JSON ya devuelto → M20.5). UUID v4 sobre `random_int`
  (versión/variante fijados) + `is_uuid_v4` (validador → permite probar el aleatorio por su forma de
  modo determinista). JWT verificado **byte a byte contra una implementación de referencia** (`hmac`+
  `base64url` de Python) por ambos motores (`tests/jwt_cli.rs`), incl. secreto incorrecto, tamper y
  token mal formado.
- **M20.4 — URL/query/cookies** ✅ (`url.ray`, `cookie.ray`). Percent-encoding (RFC 3986):
  `url_encode` (deja pasar unreserved, escapa el resto por octeto UTF-8 en `%XX` mayúscula) /
  `url_decode` (revierte `%XX` y `+`→espacio, form-urlencoded). `parse_query`→`Map` (parte por el
  PRIMER `=`, url-decodifica) / `build_query` (claves ordenadas → reproducible). Cookies:
  `parse_cookies` (cabecera `Cookie:`→`Map`) + un `struct Cookie` con setters `with_*` encadenables
  **por UFCS cross-module** (`cookie("sid","abc").with_path("/").with_http_only()` — showcase del fix
  de M19) y `set_cookie` que serializa a `Set-Cookie`. Verificado contra `urllib.parse.quote` por
  ambos motores (`tests/url_cli.rs`).
- **M20.5 — tiempo y fechas** ✅ (`time.ray`). **Cero runtime nuevo**: `now()` ya da los milisegundos
  UTC desde el epoch, y convertir eso en (año, mes, día, hora, …) es aritmética pura (el algoritmo
  "civil from days" de Howard Hinnant y su inverso) → **solo UTC** (sin base de zonas horarias, que es
  lo que piden las cabeceras HTTP, los logs y `exp` de JWT). `struct DateTime`; `from_epoch_millis`/
  `now_utc`/`to_epoch_millis` (inverso); `to_iso8601` (RFC 3339), `to_rfc1123` (cabecera `Date:` HTTP),
  `parse_iso8601`; `format_duration` (`1h2m3s`). Válido para fechas ≥ 1970 (epoch ≥ 0 → la división
  trunca = floor). Verificado contra `datetime` de Python por ambos motores (`tests/time_cli.rs`), incl.
  año bisiesto. **No usa `bytes`/bitops → lo cubre además el oráculo de self-hosting** (parser).
- **M20.6 — cliente Redis (RESP)** ✅ (`redis.ray`). Cliente Redis (protocolo RESP2) sobre los builtins
  TCP (M15.2), cero runtime nuevo. `encode_command([string])` (array de bulk strings); un `struct Conn`
  (handle + buffer) con un lector con framing por encima de `socket_read` (`pull`/`read_line`/`read_n`);
  `read_reply` parsea las 5 formas RESP (`+`/`-`/`:`/`$`/`*`, recursivo para arrays) a un `enum Reply`.
  Funciona en **ambos motores** (intérprete con sockets bloqueantes, VM no bloqueante). Verificado e2e
  contra un **servidor RESP de juguete en Rust** (PING/SET/GET/INCR/RPUSH/LRANGE/DEL en memoria) por
  ambos motores (`tests/redis_cli.rs`). Gotchas de raylang: una asignación en un brazo `match` necesita
  bloque + `;`; reasignar un `Result` no propaga el tipo esperado → `read_line` usa `return` directo.
  No usa `bytes`/bitops → lo cubre además el oráculo de self-hosting.
- **M20.7 — HTTP cliente robusto** ✅ (ampliación aditiva de `http.ray`). `request_with(method, url,
  body, headers)` (cabeceras/métodos arbitrarios — Authorization, Accept, …); `fetch_follow(url, max)`
  (sigue 301/302/303/307/308 por `Location`, absoluta o relativa, con límite anti-bucle); decodificación
  **Transfer-Encoding: chunked** (`decode_chunked` sobre bytes, integrada en `parse_response` cuando la
  cabecera lo indica). Sin regresión en el `http.ray` previo. Verificado e2e (cabecera eco + redirect +
  chunked) por ambos motores (`tests/httpc_cli.rs`). Gotcha: `from` es palabra clave → un parámetro no
  puede llamarse así.
- **M20.8 — UDP** ✅ (`udp.ray` + runtime). El único hueco de runtime de M20: sockets sin conexión sobre
  `std::net::UdpSocket` (cero deps), en el mismo registro de handles (`OpenHandle::Udp`). 3 builtins/
  opcodes (`__udp_bind`/`__udp_send_to`/`__udp_recv_from` ↔ `UdpBind`/`UdpSendTo`/`UdpRecvFrom`), impl
  en ambos motores. A diferencia de TCP, cada datagrama lleva su remitente → `__udp_recv_from` devuelve
  un `[bytes]` etiquetado `[b"ok", host, puerto, datos]` y la librería `udp.ray` lo traduce a un
  `struct Packet { host, port, data }` (el runtime no sabe de `Packet`/`Result`; patrón M11.2c, pero el
  envoltorio vive en una **librería de usuario**, no en el prelude). I/O **bloqueante** en ambos motores
  (la cesión cooperativa al scheduler queda diferida, como TCP antes de M15.5). No determinista (red) →
  probado por subproceso contra un servidor UDP de eco en Rust por ambos motores (`tests/udp_cli.rs`),
  verificando round-trip + remitente. Habilita DNS, statsd, descubrimiento, juegos.
- **M20.9 — AWS Signature V4** ✅ (`sigv4.ray`). El *capstone* del stack de M20: apila HMAC-SHA256 +
  SHA-256 hex + URL encoding + fecha (formato básico `to_iso8601_basic`/`date_stamp` añadidos a
  `time.ray`). `authorization_header(cred, method, path, query, headers, payload, amz_date)` produce la
  cabecera `Authorization` en los 4 pasos de AWS (canonical request → string to sign → signing key por
  cadena de HMAC → signature). Cabeceras canónicas vía `keys` (ordenadas), query canónica con `url_encode`
  + `sort`. Verificado contra el vector oficial **get-vanilla** de la suite de AWS + un caso con query
  desordenada y cuerpo (referencia Python) por ambos motores (`tests/sigv4_cli.rs`).
- **M20.10 — gzip/deflate (INFLATE)** 🚧 (`inflate.ray`). El algoritmo más complejo de la stdlib: el
  **descompresor DEFLATE** (RFC 1951) en raylang puro, port del `puff.c` de zlib. ✅ **M20.10a**: bit-stream
  sobre `bytes` (LSB-first), decodificación Huffman **canónica** (`build_huff`/`decode` al estilo puff:
  `counts[]` por longitud + `symbols[]` ordenados), referencias LZ77 hacia atrás (con copia solapada =
  run-length), y los **tres tipos de bloque** (almacenado, Huffman fijo, Huffman dinámico con los códigos
  de repetición 16/17/18). Envoltorios `gunzip` (RFC 1952, cabecera + tráiler, **verifica CRC-32**),
  `zlib_inflate` (RFC 1950) e `inflate_raw`. `crc32` propio (polinomio 0xEDB88320). Verificado contra
  blobs gzip de Python (los 3 tipos de bloque) + CRC-32 estándar por ambos motores (`tests/inflate_cli.rs`).
  ✅ **M20.10b**: `http.ray` importa `gunzip` y descomprime **automáticamente** las respuestas con
  `Content-Encoding: gzip` (en `parse_response`, tras el chunked). E2E: el servidor de juguete sirve un
  cuerpo gzip y el cliente lo entrega ya descomprimido (`tests/httpc_cli.rs`, ambos motores; los tests
  que copian `http.ray` ahora copian también `inflate.ray`). **M20.10 COMPLETO.**
- **M20.11 — cesión cooperativa de UDP en la VM** ✅. Cierra el diferido de M20.8: `udp_recv_from` deja
  de bloquear el scheduler. La VM pone el socket UDP en no bloqueante al `udp_bind` (`set_nonblocking`
  extendido a `OpenHandle::Udp`); `udp_recv_from_nb` devuelve `Ok(None)` en `WouldBlock` y el opcode
  `UdpRecvFrom` **aparca la fibra en el fd** (`raw_fd` extendido a UDP) y reintenta al despertar — el
  mismo `io_parked`/poller (kqueue/epoll, M17) que TCP/TLS. El intérprete sigue bloqueante (un hilo).
  Prueba conductual que **requiere** la cesión (dos fibras esperando datagramas a la vez → con un recv
  bloqueante habría deadlock; el test colgaría): `tests/udp_yield_cli.rs` (solo VM). **M20.11 COMPLETO.**
- **M20.12 — compresión DEFLATE (encoder)** ✅ (`deflate.ray`). Cierra M20: la pareja de `inflate.ray`.
  Huffman **fijo** (BTYPE=01, evita construir/transmitir árboles) + matching **LZ77** con cadenas de
  hash al estilo zlib (`head[hash3]`/`prev[pos]`, ventana 32 KiB, `max_chain` 256) → comprime de verdad.
  Escritor de bits dual (LSB-first para los extra, MSB-first para los códigos Huffman). `deflate_raw`,
  `gzip_compress` (cabecera + CRC-32 + ISIZE) y `zlib_compress` (cabecera + Adler-32). **Doble
  verificación** (`tests/deflate_cli.rs`): round-trip interno con `inflate.ray` (ambos motores) **y
  compatibilidad estándar** — el gzip que produce raylang lo descomprime **Python** (`gzip.decompress`),
  probando que el stream DEFLATE es válido, no solo auto-consistente. Gotcha: `while {…}` seguido de una
  línea que abre `(` se parsea como llamar al unit del while → variable temporal. **M20 COMPLETO.**

## §30 — M21: observabilidad (logging estructurado + métricas Prometheus)

Sobre el stack web/cloud de M20, M21 añade lo que un servicio necesita para ser **operable**: logs
estructurados y métricas. Ambas librerías raylang puras (cero runtime), verificadas contra herramientas
externas (Python `json`, formato de exposición de Prometheus).

- **M21.1 — logging estructurado en JSON** ✅ (`log.ray`). Cada entrada es una **línea JSON** (ts, level,
  service, msg + campos) lista para un agregador (Loki/ELK/CloudWatch). API **encadenable por UFCS
  cross-module**: `info(lg, "login").field("user","ada").field_int("n",3).emit()`. Niveles DEBUG/INFO/
  WARN/ERROR con filtro por `min_level`; campos tipados (`field`/`field_int`/`field_bool`, con flag
  `quoted` para no entrecomillar números/bools); escapado JSON propio (`"`, `\`, `\n`/`\t`/`\r`). `render(e,
  ts)` separa el formateo (determinista, testeable) de `emit` (que usa `now_utc` de `time.ray`). Verificado
  por golden + **validación con Python `json.loads`** por ambos motores (`tests/log_cli.rs`). Puro (sin
  `bytes`/bitops) → lo cubre además el oráculo de self-hosting.
- **M21.2 — métricas Prometheus** ✅ (`metrics.ray`). Un registro de **counters/gauges/histogramas** que se
  renderiza al **formato de exposición de texto** de Prometheus (`# HELP`/`# TYPE` + series), listo para
  servir en `/metrics`. Modelo de arreglos paralelos + búsqueda lineal; `inc`/`add`/`set` con **labels**
  (`labels1`/`labels2`/`no_labels`, claves ordenadas → determinista, con escapado de valores); `observe`
  para histogramas (buckets cumulativos pre-creados en orden canónico + `_sum`/`_count` + `+Inf`). Salida
  determinista verificada por golden + **validación estructural con Python** (formato de las series +
  cumulatividad de los buckets, `+Inf == _count`) por ambos motores (`tests/metrics_cli.rs`). Puro → lo
  cubre además el oráculo de self-hosting.
- **M21.4 — histogramas con labels** ✅ (cierra el diferido de M21.2). Cada conjunto de labels tiene su
  **propia familia de series** (buckets + `+Inf` + `_sum` + `_count`), creada en orden canónico la primera
  vez que se observa ese conjunto (`ensure_hist_series`, idempotente vía `find_series` del `_count`) — ya
  no se pre-crea al registrar (el conjunto de labels no se conoce entonces). `observe_l(reg, name, labels,
  v)` (y `observe` = azúcar con el conjunto vacío → el caso M21.2 es idéntico, regresión verde). El `le` se
  **fusiona** en el conjunto de labels de cada `_bucket` (`with_le`, render ordenado); `_sum`/`_count`
  llevan solo los labels del usuario. Verificado por golden + validación Python de **cumulatividad por
  conjunto de labels** (`+Inf == _count` por grupo) en ambos motores (`tests/metrics_labels_cli.rs`).
- **M21.3 — endpoint `/metrics` real** ✅ (`metrics_server_demo.ray`). Monta `metrics.ray` sobre
  `webserver.ray`: un `Registry` **compartido se captura en el handler** (closure por upvalue → la
  semántica de referencia del struct lo hace estado mutable común a todas las fibras; las ops `inc`/
  `observe`/`render` no ceden → atómicas en el scheduler M:1). Cada petición incrementa un counter por
  `(método, ruta)` y observa una duración; `GET /metrics` devuelve el registro en formato de exposición
  (`Content-Type: text/plain; version=0.0.4`) — escrapeable por un Prometheus real. E2E (solo VM): se
  genera tráfico y se escrapea `/metrics`, validando counters etiquetados + histograma (`tests/
  metrics_server_cli.rs`). **M21 COMPLETO** (observabilidad: logs + métricas + endpoint).

## §31 — M22: cliente DNS sobre UDP

Estrena los sockets UDP de M20 con un protocolo real. **M22 — cliente DNS** ✅ (`dns.ray`, RFC 1035).
Resuelve nombres a IPv4 (registros A) por UDP, librería raylang pura. `build_query(id, name, qtype)`
arma el mensaje (cabecera de 12 octetos con RD=1 + pregunta con QNAME en labels length-prefixed);
`parse_response` lee la cabecera (valida RCODE), salta la sección de preguntas y recorre las RRs
recogiendo las de tipo A. La pieza difícil es la **compresión de nombres**: un nombre puede acabar en un
puntero `0xC0xx` a un offset anterior → `read_name` sigue los punteros pero devuelve la posición
**siguiente** en el flujo original (no la del destino del salto), reconstruyendo el nombre con labels +
puntos. `query(server, port, name, qtype)` enlaza un socket UDP efímero, envía la consulta y parsea la respuesta a
**registros tipados** (`enum Record { A | Aaaa | Mx | Cname | Txt | Ns | Srv | Other }`); `query_a`/`query_aaaa`/
`query_mx`/`query_cname`/`query_txt` son envoltorios que formatean a string. **CNAME** (tipo 5): un nombre
de dominio (con compresión, vía `read_name`). **TXT** (tipo 16): una o más *character-strings*
(`<longitud><octetos>`) concatenadas (`read_txt`). **AAAA** (tipo 28): los 16 octetos → IPv6 canónica con compresión `::`
de la racha de ceros más larga (RFC 5952, `format_ipv6`). **MX** (tipo 15): preferencia + el nombre del
*exchange*, que **también puede llevar compresión** dentro del RDATA (lo resuelve el mismo `read_name`).
Verificado e2e contra un **servidor DNS de juguete en Rust** que responde A/AAAA/MX según el QTYPE (el MX
con un puntero de compresión `0xC00C` en su exchange) por ambos motores (`tests/dns_cli.rs`), y comprobado
a mano contra **DNS real** (8.8.8.8: A, AAAA con `::`, MX null de example.com, y sus 2 TXT reales —SPF +
token de verificación—). ID fijo (un resolver real lo aleatoriza y reintenta). Diferido: SOA/PTR/CAA, TCP fallback para respuestas truncadas.

**M22.1 — caché DNS por TTL** ✅ (`dns_cache.ray`). Envuelve el resolver respetando el TTL. `dns.ray`
expone `query_full`/`parse_full` → `DnsResult { records, ttl }` (TTL mínimo de la respuesta, vía `be32`);
`query`/`parse_records` pasan a ser envoltorios. La caché (`struct Cache` con arreglos paralelos clave→
registros+expiración; clave = `"qtype:nombre"`) sirve de la caché si `now() < expiración`, si no consulta
y guarda con `now() + ttl*1000`; contadores de aciertos/fallos. Verificado e2e: el servidor de juguete
**cuenta** las consultas → 3 resoluciones de 2 claves distintas dan solo **2** consultas (la repetida se
cachea), por ambos motores (`tests/dns_cache_cli.rs`).

## §32 — M23: cliente OAuth 2.0

**M23 — OAuth2** ✅ (`oauth2.ray`). Cliente OAuth 2.0 como librería raylang, apilado sobre `http.ray`
(peticiones) + `url.ray` (form-encoding) + `json.ray` (respuesta del token). Cubre el grant
**client_credentials** (POST `application/x-www-form-urlencoded` al token endpoint → `struct Token
{ access_token, token_type, expires_in }`), `authorize_url` (construye la URL del flujo de código) y
`bearer_header` (cabecera `Authorization: Bearer …` para las APIs protegidas). Extractores sobre el `Json`
de `json.ray` (`JObject`/`JStr`/`JNum`; `expires_in` entero vía `to_string`+`parse_int` porque JSON solo
tiene float). Maneja la respuesta de error de OAuth (`{"error": …}`) aun con HTTP no-200. Verificado e2e
contra un **token endpoint de juguete en Rust** (valida el grant, responde el JSON del token) por ambos
motores (`tests/oauth2_cli.rs`). Diferido: flujos authorization_code/refresh_token completos, PKCE.

## §33 — M24: cliente WebSocket (`ws://`)

**M24 — cliente WebSocket** ✅ (`websocket_client.ray`). El espejo del servidor de M19.3c: el cliente debe
**enmascarar** las tramas que envía (RFC 6455 §5.3) y lee tramas del servidor sin enmascarar (que
`websocket.decode_frame` ya maneja — comprueba el bit MASK). Reusa la cripto del handshake (`accept_key`
con SHA-1/base64) y el framing de lectura de `websocket.ray`. `connect(host, port, path)` hace el
handshake (genera una `Sec-WebSocket-Key` de 16 octetos aleatorios en base64, envía el upgrade, **verifica
`Sec-WebSocket-Accept`** = base64(SHA-1(key+GUID))); `send_text` (trama enmascarada con clave de máscara de
4 octetos aleatorios), `recv_text` (decodifica), `close_ws`. Funciona en **ambos motores** (cliente
bloqueante). Verificado e2e contra el propio `websocket_echo.ray` (servidor raylang) — **cliente raylang
hablando con servidor raylang** — con eco de UTF-8 multibyte (`☃`) por ambos motores
(`tests/websocket_client_cli.rs`). Diferido: fragmentación, ping/pong automático. (`wss://` ✅ vía `connect_tls`.)

## §34 — M25: protobuf + framing gRPC

**M25 — protobuf (la carga útil de gRPC)** ✅ (`protobuf.ray`). HTTP/2 completo (framing binario + HPACK +
streams + control de flujo) es un protocolo enorme; el corazón **autocontenido y verificable** de gRPC es
el **códec del formato wire de Protocol Buffers**, que es lo que esta fase implementa. Códec proto3
schema-less: un `PbWriter` acumula campos (`write_varint`/`write_string`/`write_bytes`/`write_fixed32`/
`write_fixed64`) con su *tag* (varint `número<<3 | wire_type`) + valor; `finish` → `bytes`. `parse` decodifica
a `[PbField]` (número, wire, valor entero o bytes) y `get_int`/`get_string`/`get_bytes` los leen por número.
Varints en LEB128; length-delimited y fixed32/64 little-endian. **Framing de gRPC**: `grpc_frame`/
`grpc_unframe` (prefijo de 5 octetos: flag de compresión + longitud big-endian + el protobuf). Verificado
por golden (los vectores canónicos de la doc: campo 1 varint 150 → `08 96 01`, `"testing"`) + round-trip +
**validación con un decodificador del wire format en Python sin dependencias** por ambos motores
(`tests/protobuf_cli.rs`). Es **puro** (bytes/bitops) — fuera del oráculo de self-hosting. Diferido (grande):
el **transporte HTTP/2** (framing + HPACK + multiplexado de streams + flow control) para un cliente gRPC
completo; `sint`/zigzag y los negativos de int64 en el varint.

## §35 — M26: transporte HTTP/2 (framing + HPACK)

**M26 — HTTP/2** 🚧 (`http2.ray` + `hpack.ray`). HTTP/2 completo es un protocolo de envergadura propia
(framing binario + HPACK + multiplexado de streams + control de flujo); esta fase entrega las dos piezas
**autocontenidas y verificables contra los vectores oficiales del RFC**, que son el grueso del trabajo.
- **Framing (`http2.ray`, RFC 7540)**: cabecera de 9 octetos (longitud 24 BE + tipo + flags + R/stream_id
  31) + carga; `encode_frame`/`parse_frame`/`frame_size`; tipos (DATA/HEADERS/SETTINGS/WINDOW_UPDATE/PING/
  GOAWAY/RST_STREAM) y flags; la **connection preface** del cliente; atajos `settings_empty`/`settings_ack`/
  `headers_frame`/`data_frame`. Verificado por round-trip.
- **HPACK (`hpack.ray`, RFC 7541)** — la parte difícil: tabla **estática** (61 entradas) + **dinámica**
  (inserción + evicción por tamaño), enteros con prefijo de N bits (§5.1), literales de string sin Huffman
  (§5.2), y las representaciones de campo (indexado, literal con indexado incremental, sin indexado/nunca
  indexado, actualización de tamaño). Codificador + decodificador con tabla dinámica compartida.
  **Verificado contra los vectores OFICIALES del RFC 7541 §C.3** (las tres peticiones, byte-idéntico,
  incluidas las referencias `be`/`bf` a la tabla dinámica) + round-trip, por ambos motores
  (`tests/http2_cli.rs`). El **Huffman** de strings queda diferido (el codificador emite literales crudos —
  válidos—; el decodificador rechaza un literal Huffman con error claro). **Diferido (grande)**: HPACK-
  Huffman (tabla de 257 códigos del Apéndice B), el **transporte vivo** (preface + SETTINGS + WINDOW_UPDATE
  + multiplexado de streams sobre una conexión TLS con ALPN `h2`), y con ello un cliente **gRPC** completo
  (HEADERS con `:path`=método + `content-type: application/grpc` + DATA con el protobuf de M25 enmarcado).

## §36 — Plan post-M26: ergonomía del lenguaje, tooling y librerías

Tras el gran arco de **librerías aplicadas** (M15–M26: red, cloud, cripto, compresión, observabilidad),
el proyecto llega a un punto de inflexión. Escribir toda esa capa destapó, una y otra vez, los mismos
huecos **ergonómicos del lenguaje** (bucles por índice, formato por concatenación, structs para retorno
múltiple…). El plan vuelve primero al **lenguaje** —lo más *en tema* para un proyecto que trata de
construir un lenguaje, y lo que mejora **retroactivamente** todo el código— y luego encara tooling y las
librerías de mayor valor.

**Principio de orden**: (1) ergonomía del lenguaje primero (cada feature toca todas las fases lexer→
parser→checker→ambos motores→self-hosting: el objetivo pedagógico), ordenada por *fundamento × impacto ×
dependencias*; (2) tooling que multiplica la productividad; (3) librerías por valor. Cada fase mantiene el
oráculo VM↔intérprete y el corpus de self-hosting; método incremental, un commit por paso.

### M27 — Ergonomía del lenguaje I (iteración y forma)
Lo que más limpia el código existente y futuro. Toca lexer/parser/checker/ambos motores.
- **M27.1 Tuplas / retorno múltiple** ✅ (`(a, b)`). `Type::Tuple(Vec<Type>)` (2+ elementos, tipado
  estructural), literal `(a, b, …)` (`ExprKind::TupleLit`), acceso por índice `t.0` (`Field` con nombre
  numérico), y **desestructuración** `let (a, b) = e;` (`StmtKind::LetTuple`, `_` descarta). **Erasure a
  arreglos**: la tupla ES un arreglo en runtime (heterogéneo), `t.N`→índice; cero valor nuevo, ambos motores
  reusan `MakeArray`/`Index`. Verificado en el oráculo (`tuplas_oraculo`) + ejemplo `basics/tuplas.ray`.
  **Gotchas** (documentados): el acceso encadenado `t.0.1` choca con el float `0.1` en el lexer → binding
  intermedio; una tupla `(…)` justo tras un `while {}` se parsea como llamada → `return`/`;`. El toolchain
  auto-alojado aún no soporta tuplas (excluido del oráculo de self-hosting).
- **M27.2 `for` / iteradores** ✅ (el mayor golpe ergonómico). `for x in arr`, `for i in a..b` (rango),
  `for c in "…"` (chars), `for (k, v) in map` (¡tuplas de M27.1!, `_` descarta). Tokens `for`/`in`/`..`
  (`DotDot`); `StmtKind::For { pat, iter, body }` (`ForPat` single/tuple, `ForIter` Range/In). El checker
  valida el iterable (arreglo→elemento, string→char, `Map`→tupla `(k,v)`) y liga la(s) variable(s) en un
  ámbito nuevo. **Ambos motores lo ejecutan directamente** (sin protocolo `Iterator` genérico —diferido—):
  el intérprete itera y el compilador baja a un bucle contado con locales `$…` (arreglo/string por
  `Index`/`Len`; `Map` por `MapKeys`/`MapValues` **ordenados** → determinista, casa con la VM). Gotcha:
  `for` obligó a que `for` sea keyword → el `impl X for Y` pasó de `expect_ident` a consumir el token; y
  en la cabecera del `for` (sin paréntesis) se desactiva el literal de struct (flag `no_struct_lit`, como
  Rust). Verificado en el oráculo (`for_oraculo`, incl. anidados/`return`/Map) + ejemplo
  `basics/for_bucles.ray`. Excluido del oráculo de self-hosting (el toolchain aún no lo soporta).
- **M27.3 Interpolación de strings** ✅ (**`"…${expr}…"`**, estilo Kotlin/Swift/shell). El segundo mayor
  golpe. **Puro léxico/sintaxis**: el lexer parte la cadena en partes (`InterpPart::Lit`/`Expr`, token
  `InterpStr`; balancea llaves anidadas de la expresión), y el parser **re-lexea+re-parsea** cada fragmento
  como expresión y baja todo a `"lit" + to_string(expr) + …`. **Rediseño (post-M39c)**: la sintaxis pasó del
  prefijo `f"…{x}…"` (estilo Python) al `${…}` **sin prefijo** en TODA cadena. **Decisión clave del marcador
  `$`**: el `$` solo es especial seguido de `{` — así `"$5"`, `"$PATH"` y, sobre todo, `"{…}"` (JSON/HPACK a
  mansalva) siguen siendo **literales sin escape** (las llaves ya no necesitan `{{`/`}}`); solo `${` debe
  escaparse, con `\$`. Esto da la ergonomía de "interpolar sin recordar prefijo" sin la fragilidad del `{`
  siempre-especial de Python. Cualquier tipo con `to_string` (primitivos/string); para structs, interpolar un
  campo o `.show()`. Ambos motores lo ven como concatenación (cero cambios de runtime). Verificado en el
  oráculo (`interpolacion_oraculo`: `${}`, `$` literal, `\$`, llaves literales) + tests del lexer
  (`interpolacion_de_cadenas`) + ejemplo `basics/interpolacion.ray`. Excluido del oráculo de self-hosting.
- **M27.4 Casts numéricos** ✅ (`x as int` / `y as float` / `c as int` / `n as char`). Cierra el papercut de
  OAuth2 (`parse_int(to_string(f))`). Reusa la keyword `as` (antes solo alias de import) en posición de
  expresión: nivel de precedencia `cast` entre `unary` y la multiplicación (como Rust). `ExprKind::Cast`;
  el checker valida las combinaciones (int↔float, char↔int, e identidad) y devuelve el destino. **Cambia la
  representación en runtime** (no es erasure): opcode `Cast(CastTarget)` que despacha por el valor +
  destino (int→float, float→int truncando hacia cero, char→int code point, int→char con error si el code
  point es inválido). Verificado en el oráculo (`cast_oraculo`) + ejemplo `basics/casts.ray` (cifrado
  César combinando casts + `for` + interpolación). Excluido del oráculo de self-hosting.
- **M27.5 `const` de nivel superior** ✅ (`const NAME: T = <literal>;`). Sustituye el patrón `fn guid() ->
  string { "…" }`. Keyword `const`; `ConstDef` en `Program.consts`. El valor debe ser un **literal** (o un
  literal numérico negado; computados → diferido). El checker registra `nombre → tipo`, valida el literal
  contra `T`, y resuelve una referencia `Ident(NAME)` global contra la tabla. **Sin reescritura global**:
  cada motor lleva su tabla de valores (`eval_const_literal`, compartido) y resuelve el `Ident` — el
  intérprete devuelve el valor, el compilador emite `Constant`. El loader fusiona los consts de todos los
  módulos (con shift de posiciones). Verificado en el oráculo (`const_oraculo`) + ejemplo
  `basics/constantes.ray`. Excluido del oráculo de self-hosting. **M27 (ergonomía I) COMPLETO** (tuplas,
  `for`/iteradores, interpolación `f"…"`, casts `as`, `const`).

### M28 — Ergonomía del lenguaje II (abstracción)
Hace que el lenguaje se sienta "completo"; construye sobre traits (M9).
- **M28.1 Sobrecarga de operadores** vía traits (`Add`/`Sub`/`Mul`/`Div`/`Neg`, `PartialEq`/`Ord`…). Hoy
  `+`/`==`/`<` están *special-cased*; se generalizan a métodos de trait para que un tipo de usuario los
  defina. Bonus: puede **unificar** `@derive(Eq)` (que pasa a derivar `PartialEq`).
  - **COMPLETO** (aritméticos + `-` unario): traits `Add`/`Sub`/`Mul`/`Div`/`Neg` en el prelude. **Front-end
    puro / erasure** (reusa el patrón de bajada por posición de UFCS): en `check_binary`, cuando el camino
    built-in falla y ambos operandos son el **mismo tipo de usuario** que implementa el trait del operador
    (`impl_traits`), se registra el sitio `(línea, col, "Add"/…)` → método manglado (`Vec2#add`) y el retorno
    es `Self`; `-x` análogo con `Neg`. Una pasada `lower_operators` (antes de `lower_ufcs`) reescribe el
    `Binary`/`Unary` a una llamada ordinaria `Vec2#add(a, b)` a la función que M9 ya inyectó → **runtime
    intacto** (cero opcodes, oráculo VM↔intérprete verde). La clave lleva el **nombre del trait** porque
    operadores encadenados (`a + b + c`) comparten `(línea, col)` en el AST (mismo operador → mismo método).
    Comparación (`==`/`<`) e impls genéricos de operador quedan **diferidos** (los aritméticos concretos son
    el caso útil; los primitivos siguen por el camino built-in). Ejemplo `examples/types/operadores.ray`.
- **M28.2 `?` con conversión de error** (traits `From`/`Into`). Hoy `?` no convierte el tipo de error; con
  `From<E1> for E2` el `?` convierte automáticamente → librerías con un enum de error propio en vez de
  arrastrar `string`.
  - **COMPLETO**. Habilitador: **parámetros de tipo en traits** (`trait From<S>`, primer trait con
    `<...>`; `TraitDef.type_params`, `ImplBlock.trait_args`). El trait `From<S> { fn desde(origen: S) ->
    Self; }` vive en el prelude; su método `desde` **no tiene `self`** (asociado; `from` es palabra clave
    del import). El usuario escribe `impl From<string> for MiError { fn desde(o: string) -> MiError {…} }`.
    **Front-end puro / erasure**: en el paso 0c el método se inyecta como función libre con nombre manglado
    **por origen** (`MiError#desde#string`, para que varios `impl From<…> for MiError` no colisionen);
    `register_typed_trait_impl` valida la firma y puebla `from_impls: (origen, destino) → manglado`.
    `check_try`, si el error del `Result` (E1) difiere del retorno (E2) pero hay `impl From<E1> for E2`,
    registra el sitio; `lower_try_conversions` reescribe ese `expr?` a un `match (expr) { Result.Ok($to)
    => $to, Result.Err($te) => { return Result.Err(MiError#desde#string($te)); } }` → **runtime intacto**
    (reusa `match`+`return`+construcción de enum; el `?` sin conversión sigue siendo el nodo `Try` nativo
    de M6.3). Oráculo `conversion_error_oraculo` + ejemplo `examples/types/conversion_error.ray`. Los
    parámetros de tipo en traits solo tienen semántica para `From`/`?` (otros usos —bounds, `dyn`,
    despacho `.metodo()`— se aceptan sintácticamente pero se **difieren**); `Into`, cadenas de conversión
    y `From` entre módulos también diferidos.
- **M28.3 Enteros con tamaño / unsigned** (`u8`/`u32`/`i32`/`u64`…). El más invasivo (toca todo el modelo
  numérico). Elimina el enmascarado a mano (`& 0xFFFFFFFF`) omnipresente en SHA-256/DEFLATE/HPACK/protobuf.
  Decisión de diseño pendiente: conjunto de tipos, reglas de conversión, `wrapping`/overflow. Puede quedar
  **acotado** (solo `u8`/`u32`/`u64` sin promoción implícita) para no volverse research-grade.
  - **Decisiones fijadas con el usuario**: conjunto **acotado `u8`/`u32`/`u64`** (`int` sigue siendo i64);
    aritmética con **wrapping** dentro del ancho; conversión **solo con `as`** (sin promoción implícita).
  - **M28.3a COMPLETO** (núcleo, casts explícitos): `Type::UInt(ancho)` (keywords `u8`/`u32`/`u64`;
    `TokenKind::UIntType(w)`). Runtime: `Value::UInt(u64, u8)` / `HeapValue::UInt(u64, u8)` (escalar inline
    como `Char`, sin GC; lleva el ancho para poder envolver). Helpers `uint_mask`/`make_uint`/`uint_heap`
    enmascaran al ancho (aplican el wrapping). Aritmética `+ - * / %`, bitops `& | ^ << >> ~` y comparación
    **sin signo** exigen **mismo ancho** en ambos operandos (el checker; sin mezclas). `as` convierte
    int↔uint, uint↔uint (cualquier ancho), float↔uint, char→uint (`CastTarget::UInt`). Ambos motores
    comparten la máscara → **oráculo** `uint_oraculo` verde; ejemplo `examples/types/enteros.ray` (FNV-1a
    en u32 sin enmascarar a mano). `Map<u8,_>` sigue rechazado (uint no es clave hashable, diferido).
    Los literales aún necesitan `as` (`5 as u8`); la coerción de literal polimórfico → **M28.3b**.
  - **M28.3b COMPLETO** (ergonomía: literal polimórfico). Un literal entero adopta el ancho uint del
    **contexto** sin `as`: tipo esperado (`let x: u8 = 5`, arg `f(42)`, elemento `[u8] = [1,2,3]`) u
    **operando** de un operador (`x + 100` con `x: u8` cede `100` a u8; `200 + 100` con esperado u8
    propaga a ambos literales, recursivo en aritmética/bitops). NO es promoción: solo cede el LITERAL
    (un `x: int` no se promociona). Fuera de rango → error (`el literal 300 no cabe en u8`). Front-end
    puro: `check_expr_expected`/`coerce_uint_binop` registran el sitio (`uint_literal_sites`) y
    `lower_uint_literals` envuelve el literal en un `Cast` al ancho (`5 as u8`) → reusa el `as` de
    M28.3a, runtime intacto. Oráculo `uint_literal_oraculo`; ejemplo actualizado a la sintaxis limpia.
    **M28.3 COMPLETO** (u8/u32/u64 con wrapping, casts, literal polimórfico). **M28 COMPLETO**
    (sobrecarga de operadores + `?`/From + enteros con tamaño). Diferido: comparación/impls genéricos
    de operador, `Into`/From entre módulos, `Map<u8,_>`, más anchos/con signo, literales hex.

### M29 — Tooling
- **M29.1 Regex** — la ausencia más llamativa de la stdlib. Motor propio (Thompson NFA / backtracking
  acotado); puede ser **librería en raylang** (una vez M27 facilita el parseo) o builtin-asistido. Alcance
  inicial: literales, clases, `*`/`+`/`?`/`|`, grupos, anclas.
  - **Enfoque elegido**: **librería raylang pura** (`examples/stdlib/regex.ray`) con la "VM de regex" de
    Russ Cox (**Thompson NFA**, tiempo lineal, sin blowup del backtracking) — cero cambios de runtime. El
    patrón se compila a bytecode (Char/Any/Match/Jmp/Split) y se simula manteniendo el conjunto de hilos
    activos. Reusa enums recursivos + structs mutables + recursión.
  - **M29.1a COMPLETO**: núcleo — literales, `.`, `*`/`+`/`?`, alternancia `|`, grupos `( )`, concatenación,
    escapes de literal (`\.`). API `full_match` (anclado a todo el texto) y `search` (algún substring; siembra
    un hilo en pc 0 en cada posición). Demo `regex_demo.ray` + `tests/regex_cli.rs` (batería de casos, ambos
    motores coinciden). `regex.ray` **pasa el oráculo del parser auto-alojado** (se parsea idéntico). Clases
    `[...]`, escapes `\d`/`\w`/`\s` y anclas `^`/`$` → M29.1b; `find`/captura/`replace` → M29.1c.
  - **M29.1b COMPLETO**: clases de caracteres `[abc]`/`[a-z]`/`[^...]` (rangos + negación; `struct Class`/
    `Range`, tabla `Prog.classes` indexada por el opcode Class), escapes predefinidos `\d`/`\w`/`\s` y sus
    negados `\D`/`\W`/`\S` (átomos y dentro de `[...]`), y anclas `^`/`$` como **aserciones de ancho cero**
    (opcodes AssertStart/AssertEnd; `add_thread` recibe la posición y sigue la aserción solo si `pos==0` /
    `pos==n`). Casos nuevos en el demo (clases, `\d+`, correo de juguete, `^\d+$`); `regex.ray` sigue
    parseándose idéntico bajo el toolchain auto-alojado. `find`/captura/`replace` → M29.1c.
  - **M29.1c COMPLETO → M29.1 COMPLETO**: localización. `match_at` corre el autómata **anclado** en cada
    posición y da el match **más largo** (leftmost-longest). API: `find -> Option<(int,int)>` (índices de la
    1.ª coincidencia; usa tuplas M27.1), `find_str -> Option<string>`, `find_all -> [string]` (no solapadas),
    `replace_all -> string`. Demo con `\d+`/`[a-z]+`/`\s+`; ambos motores coinciden. Al usar tuplas,
    `regex.ray` se excluye del corpus del parser auto-alojado. Diferido: grupos de captura (necesitan una Pike
    VM con listas de posiciones), cuantificadores `{n,m}`, no-greedy `*?`, backreferences. **M29.1 (regex)
    COMPLETO** como librería raylang pura, cero runtime.
- **M29.2 Formateador** (`rayfmt`, estilo `gofmt`) — cliente externo que reusa el parser (como el LSP/
  runner). Pretty-printer canónico del AST → idempotente, sin configuración.
  - **COMPLETO** (`src/fmt.rs`, `raylang --fmt <archivo>`). **Cliente externo**: `format_source` corre
    lexer+parser y hace *pretty-print* del AST; no toca el núcleo. Cubre TODO el AST: imports/from-imports,
    const, struct/enum (con `@derive`/`pub`/genéricos/bounds), trait (firmas + cuerpos por defecto), impl
    (con `trait_args` de M28.2), funciones, todas las sentencias y expresiones. **Impresión con precedencia**
    (`bin_prec`/`expr_prec` espejo de la jerarquía del parser) → paréntesis mínimos. Indentación de 4
    espacios; ítems de nivel superior en el **orden del archivo** (se ordenan por `line`, ya que el AST los
    bucketiza por categoría); formas con bloque (`if`/`while`/`match`) indentadas por `fmt_value`. Al trabajar
    sobre el AST, **normaliza** el estilo pero **preserva las features de superficie** que el parser
    desazucara (M29.3): la **interpolación** `"…${x}…"` y los **pipelines** `x |> f()`. El parser las baja
    al AST (concatenación/llamada) para el checker y los motores, pero guarda la forma original en
    `Program::interp_sites`/`pipe_sites` (por posición del nodo raíz); el formateador las reemite desde ahí
    (`fmt_expr`→`fmt_interp`/`fmt_pipe`, quitando la entrada mientras formatea para evitar la reentrada
    infinita por la colisión de posición raíz↔sub-expr). El resultado es válido e **idempotente**.
    **Comentarios preservados**: el lexer
    los descarta, así que se recolectan aparte (`collect_comments`, respetando cadenas/chars) y se re-insertan
    durante la emisión mediante un cursor (`Cur`) — doc-comments encima de ítems, comentarios sueltos entre
    sentencias/miembros, *trailing* al final de línea, y los de fin de bloque (antes del `}`, acotados con
    `Block.end_line`). También se **preservan las líneas en blanco** entre sentencias (agrupación visual; 2+
    colapsan a una). Invariante fuerte: **ningún comentario se pierde** y cada uno queda en su sitio.
    `tests/fmt_cli.rs`: idempotencia (`fmt(fmt(x))==fmt(x)`) + **preserva el comportamiento** (original y
    formateado dan la misma salida+exit en ambos motores). Diferido: reflow de líneas largas.
- **M29.3 Optimización de la VM** — retomar el transversal (DESIGN §27): dedup de constantes, peephole/
  plegado, `HeapValue` 32→16 B. Cobra relevancia por el coste de SHA-256/DEFLATE/HPACK. Método incremental,
  midiendo (banco `benchmarks/`), conservar solo lo que supera el ruido.
  - **COMPLETO** (cierra M29). Método: `measure.py` mejor-de-15 sobre release; baseline fib(35) 2.18 s /
    loop 1.04 s / arrays 0.196 s / gcnested 0.312 s. **Opt.9 dedup de constantes** ✅ (`add_constant`
    reutiliza el índice de una constante idéntica) — **conservado por memoria** (el pool encoge; los literales
    se repiten muchísimo), **velocidad neutra** pero sin contrapartida (es la optimización estándar de todo VM
    de bytecode). **Opt.10 `OpCode` 32→24 B** (boxear `GetField`/`SetField` a `Box<str>`) ❌ **medido y
    descartado**: sin efecto → estos benchmarks no están limitados por *fetch*/caché sino por el trabajo real
    (llamadas/aritmética/GC); por lo mismo, reducir `HeapValue` 32→16 (alta cirugía) no pagaría → no se
    intentó. Registro completo en IDEAS §11. Las ganancias fáciles ya estaban exprimidas (Opt.1/2/4/7); el
    salto restante es algorítmico (locales en la pila estilo clox), refactor grande de ROI decreciente. **M29
    (tooling) COMPLETO** (regex + rayfmt + optimización VM).

### M30 — Cripto avanzada (cifrado y firmas)
Cierra el dominio cripto: hoy hay *hashing*/HMAC pero **no cifrado ni firma asimétrica**.
- **M30.1 Cripto simétrica**: **ChaCha20-Poly1305** (AEAD moderno, aritmética de 32 bits → encaja en
  raylang) y/o **AES-GCM**. Verificable contra los vectores del RFC 8439.
  - **M30.1a COMPLETO** — **ChaCha20** (`examples/web/chacha20.ray`): 20 rondas de suma/XOR/rotación sobre
    palabras de 32 bits. **Showcase de M28.3**: aritmética `u32` pura, SIN el `& 0xFFFFFFFF` que plaga a
    `sha256.ray` — el código es idéntico al pseudocódigo del RFC. `rotl32`/`quarter_round`/`chacha20_block`/
    `chacha20_encrypt`. Verificado byte a byte contra el vector RFC 8439 §2.4.2 + round-trip, ambos motores
    (`tests/chacha20_cli.rs`).
  - **M30.1b COMPLETO** — **Poly1305** (`examples/web/poly1305.ray`): MAC de una sola vez, aritmética modular
    de 130 bits (mod 2^130−5). Port de poly1305-donna (32-bit): acumulador y `r` en **5 limbs de 26 bits**,
    productos en **`u64`** (52..55 bits) → otro showcase de M28.3 (antes exigiría emular 64 bits a mano).
    Selección final en tiempo constante con la máscara `(g4 >> 63) - 1`. Verificado byte a byte contra el
    vector RFC 8439 §2.5.2, ambos motores (`tests/poly1305_cli.rs`). **Habilitador de lenguaje**: la coerción
    de literal uint de M28.3b se extendió a la **asignación** (`x = 200` con `x: u8`; var/campo/elemento) vía
    `check_expr_expected` en `check_assign` (oráculo). AEAD ChaCha20-Poly1305 (seal/open) → M30.1c.
  - **M30.1c COMPLETO → M30.1 COMPLETO** — **AEAD ChaCha20-Poly1305** (`examples/web/chacha20poly1305.ray`):
    cifrado autenticado (RFC 8439 §2.8). Combina ChaCha20 (contador 1 para el criptograma) + Poly1305 (clave
    de una sola vez = bloque ChaCha20 con contador 0). El tag cubre `AAD ‖ pad ‖ cripto ‖ pad ‖ len(AAD) ‖
    len(cripto)`. `aead_seal -> Sealed{ciphertext, tag}`, `aead_open -> Option<[int]>` (verifica el tag en
    tiempo constante y descifra, o `None` si fue manipulado). Verificado byte a byte contra el vector RFC
    8439 §2.8.2 (criptograma + tag) + round-trip + rechazo de tamper, ambos motores (`tests/chacha20poly1305_cli.rs`).
    **Toda la aritmética en `u32`/`u64` de M28.3, sin enmascarado a mano.** AES-GCM se deja fuera (ChaCha20-
    Poly1305 es el AEAD moderno preferido; AES-GCM exigiría S-boxes + GHASH sobre GF(2^128), diferido).
- **M30.2 Cripto asimétrica**: **Ed25519** (firmas sobre curva de Edwards; aritmética de campo grande →
  ejercita `u64`/bignum). Verificable contra los vectores del RFC 8032.
  - **M30.2a COMPLETO** — **SHA-512** (`examples/web/sha512.ray`), prerrequisito. Aritmética de 64 bits:
    showcase de `u64` (como SHA-256 pero sin `& mask`). Constantes de 64 bits (> i64::MAX) compuestas con
    `w64(hi, lo)`. Vectores "abc"/vacío/"quick brown fox" (FIPS 180-4/NIST, vs Python).
  - **M30.2b/c/d COMPLETO → M30.2 COMPLETO** — **Ed25519** (`examples/web/ed25519.ray`), **port de
    TweetNaCl** (dominio público, la referencia más compacta y auditada). La parte más matemática del
    proyecto: (b) aritmética de campo mod 2^255−19 en **16 limbs de 16 bits** en `[int]` (i64 con signo; los
    productos de la multiplicación escolar y los pliegues `2^256≡38` caben de sobra en i64 → no hace falta
    u128), con carries por desplazamiento **aritmético** de i64 (`car25519`); (c) ley de grupo de Edwards en
    coordenadas extendidas (`point_add`/`scalarmult`/`scalarbase`), empaquetado/desempaquetado de puntos con
    raíz cuadrada (`pow2523`); (d) firma/verificación con reducción de escalares mod L (`mod_l`) y SHA-512.
    API `ed25519_public_key`/`ed25519_sign`/`ed25519_verify`. **Verificado byte a byte contra la
    implementación de referencia canónica del RFC 8032** (apéndice) para 3 semillas del §7.1: clave pública
    + firma idénticas, verificación acepta la firma válida y rechaza la manipulada, ambos motores
    (`tests/ed25519_cli.rs`). (Gotcha de depuración: un vector "esperado" mal transcrito hizo pensar en un
    bug; tres implementaciones independientes —raylang, una referencia propia y el apéndice del RFC—
    coincidieron, confirmando que raylang era correcto.) Sobre esto, M30.3 (JWT EdDSA/Ed25519).
- **M30.3 JWT RS256/ES256**: sobre M30.2, extiende `jwt.ray` más allá de HS256 (firma asimétrica de tokens).
  - **COMPLETO → M30 COMPLETO** — **JWT EdDSA/Ed25519** (`examples/web/jwt_eddsa.ray`, RFC 8037 alg
    `"EdDSA"`). Elegido EdDSA (sobre RS256/ES256) porque reusa Ed25519 (M30.2) directamente: RS256 exige RSA
    (exponenciación modular gigante) y ES256 la curva P-256 (ECDSA) — ambos serían otro M30.2 entero. El JWT
    es `base64url(header).base64url(payload).base64url(firma)` con firma = Ed25519(seed, "header.payload") y
    header fijo `{"alg":"EdDSA","typ":"JWT"}`. `jwt_eddsa_sign(seed, claims_json)` / `jwt_eddsa_verify(pubkey,
    token) -> Result<claims, motivo>` (el firmante tiene el seed privado; cualquiera verifica con la clave
    pública — firma **asimétrica**, a diferencia del HMAC simétrico de HS256). Verificado: el token es
    **byte-idéntico a una computación independiente en Python** (Ed25519 canónico + base64url) →
    interoperable; verificación acepta la firma válida, rechaza clave equivocada y payload manipulado, ambos
    motores (`tests/jwt_eddsa_cli.rs`; interp `#[ignore]` por lento, VM en la suite). **M30 (cripto avanzada)
    COMPLETO**: cifrado autenticado (ChaCha20-Poly1305) + firma (Ed25519) + JWT asimétrico (EdDSA). RS256/
    ES256 y AES-GCM quedan diferidos (RSA/P-256/GHASH son cada uno un módulo propio).

### M31 — Cerrar gRPC (transporte HTTP/2 vivo)
Los dos diferidos grandes de M26, que juntos dan un cliente gRPC real.
- **M31.1 HPACK-Huffman** — la tabla de 257 códigos del RFC 7541 Apéndice B (decodificación canónica, como
  `inflate`); verificable contra los vectores C.4/C.6. Cierra HPACK.
  - **COMPLETO** (`examples/web/huffman.ray`). La tabla estática de 257 símbolos (código + nº de bits) del
    RFC 7541 Apéndice B, **obtenida de una fuente de producción** (x/net/http2 de Go, vía WebFetch) y
    **validada**: es un código prefijo completo (Kraft = 2^30, sin colisiones) y reproduce el vector oficial
    C.4.1. `huffman_encode` concatena los códigos MSB-first y rellena el último octeto con 1s (prefijo de
    EOS); `huffman_decode` recorre un **trie binario** (construido de la tabla en arreglos paralelos) bit a
    bit, rechazando el relleno inválido y el símbolo EOS (RFC §5.2). Verificado byte a byte contra C.4.1/
    C.4.2/C.4.3 y C.6.1 + round-trip, ambos motores (`tests/huffman_cli.rs`). Excluido del corpus del parser
    auto-alojado (bitops). Falta integrarlo en `hpack.ray` (que hoy emite literales crudos) → parte de M31.3.
    Lección: WebFetch de una tabla numérica summarizada dio errores (255≡EOS, Kraft roto); una fuente
    legible por máquina + validación Kraft/prefijo/vector es lo fiable.
- **M31.2 Transporte HTTP/2 vivo** — la connection preface + intercambio de SETTINGS + WINDOW_UPDATE +
  multiplexado de streams, todo sobre TLS con **ALPN `h2`** (requiere exponer ALPN en `tls_connect`).
  - **M31.2a COMPLETO** — **ALPN `h2` en el runtime TLS**. Builtin `__tls_connect_h2` (opcode
    `TlsConnectH2`) + envoltorio `tls_connect_h2(host, port) -> Result<int,string>` en el prelude: conecta,
    configura `alpn_protocols=[b"h2"]` en un `ClientConfig` propio (la config cacheada no lleva ALPN),
    **completa el handshake bloqueante** (para poder consultar el ALPN negociado) y **exige** que el
    servidor negocie `h2` (si no, error). Reusa el registro de handles + rutas de I/O de `tls_connect`; la
    VM lo pone no bloqueante tras el handshake para el framing con cesión de fibras. Test `h2_alpn_cli.rs`
    (servidor rustls de juguete que ofrece/no ofrece `h2`; ambos motores). Falta la máquina de estados
    HTTP/2 viva (preface + SETTINGS + streams) → M31.2b.
  - **M31.2b COMPLETO → M31.2 COMPLETO** — **cliente HTTP/2 vivo** (`examples/web/http2_client.ray`):
    `http2_get(host, port, path) -> Result<Http2Response, string>` hace un GET completo cableando las
    primitivas de M26 (`http2.ray` framing + `hpack.ray` encode/decode) sobre el socket TLS con ALPN `h2`:
    envía la connection preface + SETTINGS del cliente + HEADERS (petición HPACK-comprimida, END_STREAM), y
    en un bucle acumula bytes, extrae frames completos (`frame_size`/`parse_frame`), responde el SETTINGS del
    servidor con ACK, decodifica el HEADERS de respuesta (`:status`) y concatena los DATA, hasta END_STREAM
    en el stream 1. Test `tests/http2_live_cli.rs`: un **servidor h2 de juguete escrito a mano** (solo std +
    rustls, sin traer h2/hyper → fiel al cero-deps) que responde `:status:200` (índice 0x88) + un DATA; el
    cliente raylang obtiene status 200 y el cuerpo, ambos motores. **Bug de checker cazado y arreglado**: un
    `match` con TODOS los brazos divergentes (`return`) hacía `panic` el checker ("hay al menos un brazo");
    ahora type-checkea (el match diverge → unit). Falta integrar HPACK+Huffman e ir a gRPC → M31.3.
- **M31.3 Cliente gRPC e2e** — HEADERS (`:path`=método, `content-type: application/grpc`) + DATA con el
  protobuf de M25 enmarcado (`grpc_frame`); leer HEADERS+DATA+trailers. Verificable contra un servidor
  gRPC real o un mock.
  - **COMPLETO → M31 COMPLETO** — **cliente gRPC unario** (`examples/web/grpc_client.ray`). `grpc_call(host,
    port, path, mensaje) -> Result<GrpcResponse{message, grpc_status}, string>` apila TODO lo construido:
    TLS+ALPN `h2` (M31.2a), framing HTTP/2 + HPACK (M26), y protobuf + `grpc_frame` (M25). Envía HEADERS
    (`POST /paquete.Servicio/Metodo`, `content-type: application/grpc`, `te: trailers`) **sin** END_STREAM
    + DATA (`grpc_frame(mensaje)`, END_STREAM); lee la respuesta: HEADERS (`:status`), DATA (mensaje
    gRPC-framed) y el HEADERS de **trailers** con `grpc-status`, hasta END_STREAM; desenmarca con
    `grpc_unframe`. Test `tests/grpc_cli.rs`: un **servidor gRPC de juguete escrito a mano** (solo std +
    rustls, sin traer h2/hyper/tonic → cero-deps) que responde `:status:200` + un mensaje protobuf
    gRPC-framed + trailer `grpc-status: 0`; el cliente raylang obtiene `grpc-status 0` y parsea el string de
    la respuesta ("hola, raylang"), ambos motores. **M31 (cerrar gRPC) COMPLETO**: HPACK-Huffman + transporte
    HTTP/2 vivo (ALPN) + cliente gRPC e2e — raylang tiene un cliente gRPC real, todo como librería raylang
    salvo el runtime de TLS/ALPN.

### M32 — Clientes y formatos
- **M32.1 Cliente PostgreSQL** (protocolo wire) — el siguiente gran ejemplo "cliente cloud" en raylang
  puro, al estilo del de Redis pero con autenticación (SCRAM-SHA-256, que reusa M20) y mensajes tipados.
  - **M32.1a COMPLETO** — **SCRAM-SHA-256** (`examples/web/scram.ray`, RFC 5802/7677), el mecanismo de
    autenticación de PostgreSQL. Showcase de la pila cripto: **PBKDF2-HMAC-SHA256** (implementado, dkLen=32,
    un bloque: `result = U1 ⊕ … ⊕ Uc`) + HMAC-SHA256 (M20.2) + SHA-256 (M20.1) + base64 (se añadió
    `base64_decode` estándar a `base64.ray`). Cliente: `scram_first(user, nonce)` → client-first;
    `scram_final(sc, password, server_first)` calcula ClientProof (ClientKey ⊕ HMAC(StoredKey, AuthMessage))
    y devuelve el client-final; `scram_verify(sc, server_final)` comprueba la ServerSignature. Verificado
    contra el ejemplo COMPLETO del RFC 7677 §3 (client-final byte-idéntico + firma del servidor verificada),
    ambos motores (`tests/scram_cli.rs`; `#[ignore]`: PBKDF2 a i=4096 es lento, se corre a demanda —la
    cobertura del código SCRAM en la suite la da el e2e de postgres a i=64). Falta el protocolo wire → M32.1b.
  - **M32.1b COMPLETO → M32.1 COMPLETO** — **cliente PostgreSQL** (`examples/web/postgres.ray`, protocolo
    wire v3). `pg_query(host, port, user, db, password, nonce, sql) -> Result<[string], string>`: abre TCP,
    envía el **StartupMessage** (sin octeto de tipo), conduce la máquina de mensajes `[tipo][longitud][carga]`
    —Authentication ('R': SASL/SASLContinue/SASLFinal/Ok, cableando `scram.ray`), ReadyForQuery ('Z': manda
    la Query), RowDescription/DataRow ('T'/'D': parsea las columnas), ErrorResponse ('E')— hasta la segunda
    ReadyForQuery. **Autenticación SCRAM-SHA-256 completa** con verificación de la firma del servidor. Test
    `tests/postgres_cli.rs`: un **servidor PostgreSQL de juguete escrito a mano** (solo std, TCP plano) que
    reproduce el intercambio SASL con valores **precomputados** (nonce/salt fijos, i=64 → sin cripto en Rust)
    y responde una fila; el cliente raylang autentica, verifica la firma del servidor y devuelve la fila
    ("hola-postgres"), ambos motores. **M32.1 COMPLETO**: cliente PostgreSQL real con SCRAM-SHA-256, todo
    librería raylang. Reusa toda la pila cripto (PBKDF2/HMAC/SHA-256/base64).
- **M32.2 Formatos de config**: TOML (y/o YAML/CSV) como librería raylang.
  - **M32.2a COMPLETO** — **CSV** (RFC 4180, `examples/stdlib/csv.ray`). `parse_csv(src) ->
    Result<[[string]], string>` (filas de campos como strings — heterogéneo → todo string, idiomático en un
    lenguaje tipado) con campos entrecomillados (coma/salto/comilla internos, `""` escapado), LF o CRLF; y
    `write_csv(rows) -> string` (entrecomilla y escapa donde hace falta). Puro cómputo, cero runtime. Demo +
    `tests/csv_cli.rs` (parseo de campos entrecomillados + round-trip, ambos motores). Al ser raylang puro
    (sin bitops), **pasa el oráculo del parser auto-alojado**. TOML → M32.2b.
  - **M32.2b COMPLETO → M32.2 COMPLETO** — **TOML** (subconjunto, `examples/stdlib/toml.ray`). Parser de
    cursor sobre los caracteres: `parse_toml(src) -> Result<[TomlEntry], string>` donde `TomlEntry{key,
    value}` con la clave como ruta con puntos (`server.port`) y `enum TomlValue { TStr, TInt, TFloat, TBool,
    TArray([TomlValue]) }` (recursivo). Soporta comentarios `#`, tablas `[a.b]`, claves desnudas, strings
    `"..."` con escapes, enteros/flotantes/booleanos, y arreglos `[…]` (posiblemente multilínea). Helpers
    `toml_get(entries, key) -> Option<TomlValue>` y `toml_show`. Demo + `tests/toml_cli.rs` (comentarios,
    tablas, todos los tipos, arrays; ambos motores); raylang puro → **pasa el parser auto-alojado**.
    Diferido: tablas en línea `{…}`, arreglos de tablas `[[…]]`, fechas, strings multilínea/literales.
    **M32.2 COMPLETO** (CSV + TOML como librerías raylang puras).
- **M32.3 Plantillas HTML** — un motor de plantillas simple (interpolación + bucles) sobre M27.
  - **COMPLETO → M32 COMPLETO → PLAN §36 COMPLETO** — **motor de plantillas HTML** (`examples/stdlib/
    template.ray`, estilo Jinja/Django). Tokenizador → parser de árbol (`enum Node`) → render. Sintaxis:
    interpolación `{{ var }}` con **autoescape HTML** (`< > & " '` → entidades), cruda `{{& var }}`,
    condicional `{% if cond %}…{% else %}…{% endif %}` y bucle `{% for x in lista %}…{% endfor %}`. Contexto
    con valores tipados (`enum TVal { VStr, VInt, VBool, VList }`) y bindings; el `for` **shadowa** la
    variable del bucle (prepend al contexto). API `render_template(tpl, ctx) -> Result<string, string>` +
    constructores `ctx_str`/`ctx_int`/`ctx_bool`/`ctx_list`/`val_*`. Demo + `tests/template_cli.rs`
    (autoescape, if/else, for sobre lista heterogénea, raw; ambos motores); raylang puro → **pasa el parser
    auto-alojado**. Diferido: filtros (`{{ x|upper }}`), `elif`, herencia de plantillas, expresiones en las
    condiciones (hoy la condición es una variable, por truthiness). **M32 COMPLETO** (PostgreSQL + CSV/TOML +
    plantillas HTML). **El plan post-M26 (DESIGN §36: M27 ergonomía · M28 ergonomía II · M29 tooling · M30
    cripto avanzada · M31 gRPC · M32 clientes/formatos) está COMPLETO.**

### Diferidos / research (fuera del plan por ahora)
- **Gestor de paquetes** (las "libs" siguen siendo archivos/cápsulas del proyecto). → plan de producción (§37, M39).
- **Debugger** (breakpoints/step; el LSP ya da diagnósticos/hover/rename).
- **FFI** (contradice la invariante cero-deps; solo si se abre esa puerta conscientemente, como TLS). → §37 (M41).
- **Reflection / serialización derivada** (`@derive(Json)`) — necesita introspección de runtime.
- **JIT / backend nativo** (M18, aparcado; el transpile-a-Rust queda como investigación post-1.0, §37).

## 37. El plan de producción (rama `feature/improvements`)

El cambio de norte anotado en §21.1 tiene ahora **documento-contrato propio**:
**[PRODUCCION.md](PRODUCCION.md)**. Contiene (I) el análisis a fondo del lenguaje post-§36 contra
los cinco ejes —moderno, flexible, ligero, seguro, elegante— con las **siete brechas** hacia
producción, y (II) el plan **M33–M44** en cuatro arcos (la numeración creció al separar la cripto en su
propio hito; ver el desglose del arco D abajo):

- **A — Estabilidad** (M33 spans + compilador sin ICEs + multi-error + fuzzing · M34 SPEC normativa
  + semver + congelación de API · M35 un solo motor de producto — la VM; el intérprete queda como
  oráculo de desarrollo).
- **B — Rendimiento y paralelismo** (M36 optimización profunda de la VM · M37 GC de pausas acotadas
  · M38 **M:N con aislamiento por actores**: heap por fibra + transferencia de propiedad en `send`).
- **C — Ecosistema** (M39 CLI unificado `ray` + gestor de paquetes con `ray.toml`/lockfile · M40
  stdlib 1.0 en `std/` + protocolo `Iterator` + `Hash` + patrones anidados/guardas/`if let` +
  `raydoc` · M41 FFI con ABI C).
- **D — Endurecimiento y lanzamiento**. Al ejecutarlo, la **cripto de producción vía `ring` se separó en
  su propio hito** (era demasiado sustantiva para un sub-punto de M42), corriendo la numeración:
  - **M42 — endurecimiento** (§44): política de overflow (ya resuelta) + auditoría de los `unsafe` +
    **fuel** (límite de instrucciones) + **tope de heap** (límite de objetos vivos) + **fuzzing continuo
    + CI + `cargo audit`**.
  - **M43 — cripto de producción vía `ring`** (§45): SHA/HMAC/Ed25519/AEAD como builtins de tiempo
    constante; el paquete `net` migrado a ellos; la cripto pura queda como demostración del lenguaje.
  - **M44 — distribución** → **1.0** (en curso):
    - **M44a — playground WASM HECHO** (`b17b414`+`f881f3b`+`278fd1c`): raylang corre EN EL NAVEGADOR (la
      VM compilada a `wasm32-unknown-unknown`), **sin `wasm-bindgen` ni deps nuevas** (ABI cruda a mano,
      como `dlopen`/`poll`). Tres pasos: (1) `ring`/`rustls` → deps solo-nativo (`[target.'cfg(not(
      target_arch="wasm32"))'.dependencies]`); su uso en `builtins.rs` (cripto SHA/HMAC/Ed25519/AEAD +
      TLS) cfg-partido con stubs de wasm. (2) FFI (`dlopen`/`dlsym`) cfg-gateado (era el bloqueador de
      LINK) + `num_workers`→1 en wasm (sin hilos). (3) `src/wasm.rs`: `alloc`/`run(ptr,len)->u64`/`dealloc`
      + captura de stdout (`print`→`host_print`→buffer); `[lib] crate-type=["cdylib","rlib"]`; `playground/`
      (index.html + build.sh + README). Alcance = lenguaje NÚCLEO (todo el lenguaje + prelude + stdlib
      pura; red/TLS/cripto/FFI/hilos no disponibles; un solo archivo). .wasm release 1,0M. Verificado E2E
      con Node. **Nativo intacto** en los tres pasos.
    - **M44b — `SECURITY.md` + criterios 1.0 HECHO**: `SECURITY.md` (modelo de seguridad, alcance, proceso
      de reporte) → el criterio "política de seguridad vigente" queda cumplido; y `RELEASE-1.0.md`, la
      **checklist viva** hacia la 1.0 (cada criterio con su estado honesto). Criterios técnicos: ✅ salvo
      SPEC (escrita, falta publicar) y benchmarks (guarda de regresión; el 3–5× aspiracional de M36 no se
      persiguió, decisión de alcance).
    - **M44c — instalador + CI de releases HECHO** (en-repo): `install.sh` (`curl -sSfL …/install.sh | sh`;
      detecta OS/arch → target, descarga de la Release, instala en `~/.local/bin`; `RAYLANG_VERSION`/`_BIN_DIR`/
      `_REPO`/`_DRY_RUN`) y `.github/workflows/release.yml` (en un tag `v*`, build NATIVO por plataforma —
      macOS arm/intel, Linux x86_64/arm64, Windows — empaqueta `ray`+`raylang` y sube con `gh`, sin acciones
      de terceros; assets sin versión en el nombre → `releases/latest/download`). Verificado localmente
      (detección de target + round-trip de empaquetado); el run real corre al empujar un tag.
    - **Pendiente de M44**: **ejecutar** el pipeline (empujar un tag → primera Release con binarios),
      publicación externa (marketplace VSCode, hosting libro/sitio/SPEC, brew tap), y **declarar raylang 1.0**
      (subir versión + notas + tag).

A precede a todo; B y C pueden ir en paralelo tras A; D cierra. Los principios del proyecto (una
fase a la vez, medir antes de conservar, oráculo en desarrollo, cero deps salvo excepción
consciente) siguen vigentes. Lo sacrificado está declarado al final de PRODUCCION.md.

**Estado del plan de producción** (al cierre del arco B): **arcos A, B, C y D-endurecimiento/cripto COMPLETOS**.
- **A — Estabilidad**: ✅ M33 (diagnósticos sin ICEs) · M34 (SPEC + semver `1.0.0-beta.1`) · M35 (la VM es
  el motor de producto; el intérprete es el oráculo de desarrollo).
- **B — Rendimiento y paralelismo**: ✅ M36.1 (superinstrucciones, win real medido) · M37 (pausas del GC
  acotadas, **cerrado por el heap-por-fibra de M38.1**: 10,5 ms → 0,12 ms) · M38 (**M:N con aislamiento por
  actores**: heap por fibra + pool de hilos multicore real, 3,84× en 4 tareas; multicore por defecto,
  `--deterministic`/`RAYLANG_THREADS=1` fuerza el M:1 reproducible; M38.2 move-on-send descartado por medición).
- **C — Ecosistema**: ✅ M39 (CLI `ray` + gestor de paquetes con `ray.toml`/lockfile/git/path-deps) · M40
  (stdlib `std/` embebida + `Iterator` + colecciones + `raydoc` + el paquete `net`) · M41 (FFI con ABI C,
  sin libffi).
- **D — Endurecimiento**: ✅ M42 (fuel + tope de heap + fuzzing/CI/`cargo audit`) · M43 (cripto de
  producción vía `ring`). **M44 (distribución) EN CURSO** — el ÚNICO hito restante del plan: **M44a
  (playground WASM) HECHO** (raylang en el navegador, cero wasm-bindgen); falta binarios/instalador/CI de
  releases + publicación + `SECURITY.md` → **declarar 1.0**.

## 38. M33 — Compilador sin pánicos y diagnósticos de producción

Primera fase del arco A (PRODUCCION.md). Sub-fases: **a)** spans, **b)** ICE→diagnóstico,
**c)** multi-error con recuperación, **d)** fuzzing. La (a) se parte en dos cortes verticales:

### 38.1 M33a-1 — spans en tokens + subrayado de rango

Hoy todo diagnóstico apunta a **un punto** `(línea, col)` y el renderizador (M8.3) dibuja un solo
`^`. El primer corte da al compilador la noción de **extensión**: cada token sabe cuánto mide, y
los errores léxicos y sintácticos subrayan el lexema completo (`^^^^`).

- **`Token.len`** — longitud del lexema en **caracteres**. Es exacta y barata por dos invariantes
  que ya teníamos: la emisión de tokens está **centralizada** (un solo `push` en `tokenize`, que
  conoce `start_col` y la posición del cursor al terminar) y **ningún token cruza líneas** (el
  lexer rechaza el salto de línea dentro de una cadena desde M1). `len = col − start_col` (≥1;
  `Eof` mide 1). El único `Token::new` fuera del lexer (el split de `>>` para genéricos anidados,
  M19.3a) sintetiza un `Gt` de `len` 1.
- **`LexError.len` / `ParseError.len` / `TypeError.len`** — la extensión del error. El lexer la
  toma del helper `error()` (lo consumido del token en curso → "cadena sin cerrar" subraya desde la
  comilla); el parser, del **token ofensor** (`se esperaba X, se encontró <tok>` subraya `<tok>`
  entero); el checker pone `1` por ahora (los spans de **expresiones** exigen extensión en los
  nodos del AST → **M33a-2**). Los `Display` no cambian (la extensión no va en la cabecera) → los
  mensajes byte-idénticos de los oráculos self-hosted quedan intactos.
- **`diagnostic::render(src, line, col, len, headline)`** — dibuja `^` repetido `len` veces,
  **acotado** al final de la línea de fuente (una extensión corrupta nunca desborda el render).
- **LSP**: `Diag` gana `len`; el rango publicado pasa de 1 carácter al **token completo** (el
  subrayado del editor cubre el lexema). Para errores del checker sigue siendo 1 (→ a-2).
- **Self-hosting intacto**: el formato canónico del oráculo (`<KIND>@<l>:<c>`, dumps del AST) no
  incluye longitudes; el espejo de `len` en `selfhost/lexer.ray` queda **diferido** hasta que algo
  lo necesite.

### 38.2 M33a-2 — spans de expresiones: el checker subraya la expresión completa

Con a-1, un error de tipos seguía marcando un punto (`len = 1`). El objetivo de a-2 es que
`1 + true` se subraye **entero**. La pregunta de diseño era dónde vive el fin de una expresión.

**Decisión: tabla lateral, no campos en el AST.** `Program.expr_spans: HashMap<(línea, col),
(línea_fin, col_fin)>`, poblada por el parser. Se descartó añadir `end_line`/`end_col` a `Expr`
porque el AST se construye en **cientos** de sitios (el parser, pero también todas las síntesis
del lowering de M9/M10 —dyn structs, closures-diccionario, derives— donde un "fin" no significa
nada); la tabla es el patrón de la casa (posición→dato, como `ufcs_sites`/`dyn_coercions`) y se
apoya en la misma infraestructura que ya garantiza **posiciones globalmente únicas** entre
módulos (bandas de líneas disjuntas, L3).

- **El parser registra en un solo punto**: `expression()`, la puerta por la que pasa toda
  expresión de usuario (sentencia-expresión, argumentos, índices, inicializadores, paréntesis,
  escrutinios). Clave = `(line, col)` del `Expr` devuelto; valor = el **fin del último token
  consumido** (`prev_end`). Política **max-end**: si dos expresiones comparten inicio (un nodo
  binario hereda la posición de su operando izquierdo), gana la más ancha — un error sobre `a`
  en `a + b` subraya `a + b` completo (degradación aceptable, y a menudo lo deseable).
- **El loader** desplaza la tabla con su módulo (claves y valores, el mismo delta de
  `shift_program`) y las fusiona (sin choques: bandas disjuntas).
- **El checker** guarda la tabla y `err()` la consulta: hit en la misma línea → `len = fin −
  col`; hit multi-línea → `len = usize::MAX` (el sentinela "hasta el fin de línea": ambos
  renderizadores acotan); **miss** (expresión sintética del lowering, clones renumerados,
  prelude) → `1`, el comportamiento de a-1. Degradación honesta y sin pánicos posibles.
- **LSP**: la suma `col + len` pasa a saturante (el sentinela no desborda) y, con `len > 1`
  real, el subrayado del editor pasa a la expresión exacta.
- Los `Display` siguen sin cambiar → oráculos self-hosted intactos.

### 38.3 M33b — ICE → diagnóstico

Un compilador de producción **no hace panic con ninguna entrada**: o acepta el programa o da un
diagnóstico. Un pánico interno (ICE, *internal compiler error*) puede seguir existiendo —las
invariantes internas se rompen cuando hay un bug— pero debe (a) distinguirse de un error del
usuario, (b) pedir un reporte, y (c) no depender de que cada sitio lo formatee bien.

**La auditoría, primero.** El análisis de PRODUCCION.md estimó ~200 `panic!`/`unwrap` en el
front-end; la auditoría real (excluyendo tests y el falso positivo del **método `expect` del
propio parser**, que devuelve `Result` y es el camino bueno) encontró **26 sitios** en el
front-end y sus clientes de compilación. Los tres sospechosos de ser alcanzables por entrada
del usuario (`f""`, `f"{}"`, `b"\xZZ"`) resultaron estar **guardados** (errores limpios); no se
encontró ningún ICE alcanzable conocido. El resto son invariantes ("recién registrado", "el
guard garantiza…").

**Las tres piezas:**
1. **`ice!(…)`** (macro en `diagnostic.rs`): el único panic permitido en el front-end. Panica
   con el prefijo `ICE:` (el hook estándar añade archivo:línea — útil para el reporte). Los 26
   sitios se convierten: `unwrap`/`expect` → `unwrap_or_else(|| ice!(…))`, `unreachable!` →
   `ice!` (mensajes preservados).
2. **La red central** `with_big_stack_or_ice` (`lib.rs`): todo el binario corre en el hilo
   worker; si el worker panica, el `join` ya no re-panica (doble traza fea) sino que imprime el
   **banner de ICE** (`diagnostic::ice_banner`: qué pasó + "esto es un bug de raylang, no de tu
   programa; repórtalo con el fuente que lo causó") y sale con **código 101** (convención Rust).
   La red caza también los pánicos **no auditados** (un índice fuera de rango futuro): es la
   garantía de UX, no la macro. `with_big_stack` (la variante que re-lanza) queda para los
   tests, donde un assert fallido dentro del closure debe tumbar SU test, no el proceso.
3. **El test de política** (`tests/ice_policy.rs`): lee los fuentes del front-end y sus clientes
   de compilación (token/lexer/parser/ast/checker/loader/prelude/diagnostic/fmt/lsp/repl/
   test_runner/main) y **falla si reaparece** un `panic!`/`unwrap()`/`expect("…")`/`unreachable!`
   fuera de tests (marcador `// ice-ok` para las excepciones deliberadas, hoy solo la definición
   de la macro). La política se **auto-defiende**.

**Alcance**: el runtime (`vm`/`interpreter`/`compiler`/`bytecode`/`gc`/`builtins`) queda
**fuera** — sus pánicos son del dominio de ejecución (el registro de handles, locks del host) y
su auditoría va con el arco del motor único (M35/M36); la red central de `with_big_stack` ya
los convierte en un ICE presentable mientras tanto. La validación de verdad de esta fase llega
con el fuzzing (M33d).

### 38.4 M33d — fuzzing del front-end

La validación de M33a/b: bombardear lex→parse→check con entradas corruptas y comprobar que
**ninguna** tumba el proceso (todo acaba en diagnóstico o, si hay bug, en un ICE presentable).

**Decisión: arnés propio de mutación, cero dependencias.** Se consideró `cargo-fuzz`
(libFuzzer, coverage-guided) y se descartó por ahora: exige nightly + una dependencia, y el
grueso del valor —mutar un corpus real y detectar pánicos— lo da un arnés de ~150 líneas con el
PRNG propio (SplitMix64, M15.1b). El fuzzing coverage-guided queda **diferido** (anotado en
IDEAS) para cuando el arnés simple deje de encontrar cosas.

- **Corpus semilla**: los `.ray` reales del repo (`examples/` + `selfhost/`, ~164 archivos).
- **Mutaciones** (por caso, 1–4 aplicadas): flip de byte, truncar, borrar/duplicar un span,
  insertar basura ASCII/UTF-8, **amplificar** un carácter (la que encuentra los anidamientos), y
  empalmar dos archivos del corpus.
- **Objetivo**: `lsp::analizar` (lex→parse→check **sin ejecutar** — fuzzeamos el front-end, no
  el programa del usuario) + `fmt::format_source`. Cada caso corre en un **hilo con pila
  grande**; un `join` con `Err` = pánico = hallazgo (se guarda la entrada y falla el test con la
  ruta). Un stack overflow no es capturable (aborta el proceso) — visible igualmente.
- **Dos marchas**: un *smoke* **determinista** en la suite (semilla fija, casos acotados,
  segundos) que impide regresiones; y la **campaña** `#[ignore]` (iteraciones por
  `RAYLANG_FUZZ_ITERS`) para búsquedas largas.

**El primer hallazgo llegó antes que el fuzzer** (hipótesis dirigida): el parser de descenso
recursivo no tenía **límite de profundidad** — `((((…` / `[[[[…` / `{{{{…` de 100k niveles
desbordaban incluso los 256 MiB del worker y **abortaban** el proceso (SIGABRT; ni la red de
ICEs puede cazar un stack overflow). Fix: **`MAX_PARSE_DEPTH = 1000`** (espejo del
`MAX_CALL_DEPTH` del runtime, M13.3a), un contador en los tres puntos de recursión del parser
(`expression`, `parse_type`, `block`) que corta con un `ParseError` posicionado ("anidamiento
demasiado profundo"). 1000 niveles × ~2.6 KB/nivel ≈ 2.6 MB: seguro incluso en un hilo estándar
de 8 MB (LSP embebido, tests). Nadie anida 1000 niveles legítimamente. El parser auto-alojado
no lo replica (el corpus del oráculo no anida así) — anotado como diferido.

**Hallazgo colateral**: el diagnóstico de ese mismo caso imprimía la **línea de fuente entera**
(200 KB, el archivo patológico es una sola línea). `diagnostic::render` gana una **ventana**:
una línea de más de 160 caracteres se recorta alrededor de la columna del error con `…` en los
bordes (el cursor sigue alineado). Pulido de presentación puro.

### 38.5 M33c — multi-error con recuperación

Hoy el pipeline es *fail-fast*: el primer error termina la pasada, y el LSP publica **un**
diagnóstico por documento. Producción quiere ver **todos** (hasta un tope) los errores de una
tacada. La restricción que gobierna el diseño: **el primer error debe seguir siendo
byte-idéntico** — los oráculos self-hosted (M14) comparan contra él, y el camino de ejecución
no cambia.

**Decisión: variantes acumuladoras, firmas intactas.** `parse`/`check` conservan firma y
comportamiento (fail-fast; es lo que ejecuta, y lo que el self-hosting espeja). Se añaden:

- **`parser::parse_all(tokens) -> (Program, Vec<ParseError>)`** — el cuerpo del bucle de ítems
  se extrae a `parse_item`; la variante acumuladora, ante un error, lo guarda y **resincroniza**
  (`sync_item`): avanza (garantizando progreso) hasta el próximo arranque de ítem top-level
  (`fn`/`struct`/`enum`/`trait`/`impl`/`pub`/`import`/`from`/`const`/`@`) con el **contador de
  llaves relativo ≤ 0** — así un error dentro de un cuerpo salta el cuerpo entero. El `Program`
  devuelto es **parcial** (los ítems que sí parsearon). Heurística honesta: un `fn` anónimo en
  medio de un cuerpo roto puede anclar de más (cascada acotada por el tope); los compiladores
  reales conviven con lo mismo.
- **`checker::check_all(&mut Program) -> Vec<TypeError>`** — granularidad **por función**: las
  pasadas tempranas (tipos, firmas, anotaciones) siguen fail-fast (sus tablas a medias
  envenenarían todo lo demás); el bucle de **cuerpos** acumula (flag `acumular`), truncando
  `scopes` tras un cuerpo fallido para no contaminar al siguiente (`check_function` ya restaura
  el resto de su estado incluso en error). Los métodos de impl ya viven en `program.functions`
  (paso 0c) → cubiertos gratis. **Sin lowering**: la variante es de diagnóstico; si hay errores
  no se ejecuta nada.
- **Tope `MAX_ERRORES = 20`** en ambos (la cascada tras una recuperación imperfecta no inunda).
- **LSP**: `analizar_todos` (lex → un error · parse → los de `parse_all` · si parse limpio →
  `check_all`) y `publishDiagnostics` publica la lista completa. No se mezclan fases: los
  errores del checker sobre un AST parcial serían cascada basura.
- **CLI**: si `check` falla, `main` re-corre `check_all` sobre una copia previa del programa y
  renderiza **todos** (cada uno localizado contra su módulo, L3). Los errores de parse en CLI
  siguen fail-fast (el loader corta por módulo); el multi-error de parse luce en el LSP, que es
  por-documento.
- **REPL**: sigue fail-fast a propósito — una entrada de REPL es una línea; el multi-error no
  aporta y la entrada errónea se descarta entera igualmente (M8.2).
- **El fuzzer (38.4) apunta a la variante nueva**: `analizar_todos` ejercita la recuperación
  (`sync_item`) y el `check_all` sobre programas parciales — la superficie que estrena esta fase.

## 39. M34 — Especificación y versionado (arco A)

La segunda fase del arco A entrega el documento que faltaba para que raylang sea *definible*
sin leer 25k líneas de Rust: **[SPEC.md](SPEC.md)**, normativo (DESIGN queda como crónica; el
libro como pedagogía; ante conflicto manda la SPEC). Cubre léxico, módulos/cápsulas, tipos,
gramática EBNF de declaraciones/sentencias/expresiones (tabla de precedencia de 15 niveles),
reglas del sistema de tipos, semántica de evaluación (referencia vs valor, TCO, límites),
concurrencia CSP, la superficie estable de builtins/prelude, diagnósticos/códigos de salida, y
la política de versionado. El parser auto-alojado (M14) queda como validador cruzado de la
gramática.

**Versionado (SemVer)**: el lenguaje pasa a **1.0.0-beta.1** (Cargo.toml; `raylang --version`).
Estable = §§1–11 de la SPEC salvo lo marcado interno (primitivos `__`, bytecode, detalles de
GC/scheduler, el render de errores más allá de la cabecera). Deprecación: anuncio en la SPEC ≥
una MENOR antes de retirar en la siguiente MAYOR.

**Congelación de API**: los nombres se congelan con su **porqué** documentado (SPEC §10):
`index_of`/`position`, `fetch`, `bytes_of`/`to_bytes` están justificados por la ausencia de
sobrecarga y la colisión UFCS con `Map.get`; no se renombró nada.

**Escribir la SPEC destapó dos bugs** (el patrón de M33d otra vez — especificar es verificar):
1. **El overflow de `int` dependía del build del compilador** (panic en debug presentado como
   ICE, wrap silencioso en release). La política de M42 se adelantó: **desbordamiento = error
   de ejecución** ("desbordamiento aritmético en int"), coherente con división por cero;
   Add/Sub/Mul/Div(MIN/-1)/Rem/Neg en ambos motores + el fast-path Opt.4; oráculo
   `overflow_aritmetico_oraculo`. `u8/u32/u64` y los bit a bit siguen wrapping por diseño.
2. **Asignar a una posición de tupla era un ICE** (`t.0 = 9` pasaba el checker sin bajarse y
   reventaba ambos motores; el fuzzer no lo vio — el corpus apenas usa tuplas). Decisión de
   lenguaje (SPEC §3/§5): las posiciones de tupla son de **solo lectura** — la tupla es un
   agregado inmutable y se comporta como *valor* (para mutar: desestructurar o arreglo). El
   checker lo rechaza con error claro.

También se verificó empíricamente lo afirmado: el contador de shift se enmascara (semántica
Rust), `float as int` satura en los extremos, y la ventana/subrayado de M33 sobre los nuevos
errores.

## 40. M35a — La VM es el motor de producto (arco A)

Tercera fase del arco A (PRODUCCION.md). Hasta aquí el binario ejecutaba con el **intérprete**
por defecto y `--vm` era opt-in — al revés de lo que quiere producción: el usuario que corre
`raylang prog.ray` recibía el tree-walker lento. M35a lo invierte.

**Decisión: la VM por defecto, el intérprete como oráculo de desarrollo.**
- `raylang <archivo>` corre en la **VM**. `--interp` selecciona el intérprete (rol nuevo: el
  oráculo secuencial, que da error limpio ante la concurrencia). `--vm` se mantiene **aceptado**
  (redundante) por compatibilidad con los scripts y los ~decenas de tests que lo pasan.
- El **REPL** y el **runner de `@test`** pasan también a la VM. Ganan la concurrencia gratis (una
  `@test` puede usar `spawn`/canales); el veredicto es idéntico porque la VM devuelve el mismo
  `Value`/`RuntimeError` que el intérprete en la frontera.
- Pieza compartida: `lib::run_on_vm(program)` compila (bajada a bytecode) el programa ya
  chequeado y lo corre en la VM, devolviendo `Result<Value, RuntimeError>` (un error de
  compilación —raro tras un check exitoso— se presenta como error de ejecución). Lo usan los tres
  clientes (binario, REPL, runner).

**Por qué es seguro invertir el default**: el oráculo VM↔intérprete garantiza comportamiento
observable idéntico para todo programa determinista (SPEC §Conformidad). Los tests de integración
que corrían sin flag (I/O, formatos, cripto…) ahora usan la VM y **pasan igual**; solo hubo que
tocar un test —el que verificaba el error de concurrencia del intérprete corriendo *sin* flag—
para que pida `--interp` explícitamente (ya no es el default). 688 tests verdes.

**Alcance de M35 restante tras M35a**: la VM ya es el motor de producto, pero el intérprete
seguía **en el binario**. Sacarlo (M35b) y la regresión de rendimiento en CI (M35c) quedaban
pendientes.

### 40.1 M35b — el intérprete tras una feature; el modelo de valores compartido

El intérprete y la VM compartían el **modelo de valores** (`Value`, `MapKey`, `EnumInstance`,
`StructInstance`, `RuntimeError`, `Cell`, `Closure`) más helpers (`MAX_CALL_DEPTH`, el almacén
de args de proceso, `uint_mask`/`make_uint`/`eval_const_literal`), todo dentro de
`interpreter.rs`. Eso impedía compilar la VM sin el tree-walker. M35b los separa:

- **Nuevo módulo `src/runtime.rs`** (se compila **siempre**): el modelo de valores + los
  helpers compartidos. Es el `Value` que ambos motores producen en la frontera (el intérprete
  nativamente; la VM al convertir su `HeapValue` al salir). Extracción **pura** (relocalización,
  sin cambio de comportamiento); el checker/compilador/VM/binario pasan a importar de `runtime::`.
- **`interpreter.rs`** queda solo con el tree-walker (`Interpreter`, `run`, `eval_*`, `Flow`,
  `cast_value`…) y se **gatea con la feature `interp`** (activa por defecto: `cargo build`/`cargo
  test` lo incluyen; el oráculo VM↔intérprete sigue corriendo).
- **`cargo build --no-default-features`** produce un binario **solo-VM** (~126 KB menos): corre
  normal y `--interp` avisa con claridad ("esta build no incluye el intérprete… ejecuta en la
  VM"). La *release* de producto puede excluir el oráculo; el desarrollo lo conserva.
- Los tests unitarios del oráculo (en `vm.rs`, `#[cfg(test)]`) referencian `crate::interpreter`
  y **solo compilan bajo `cargo test`** (feature `interp` on por defecto) — un `cargo build`
  no toca el código de test, así que no necesitan gate propio.

Verificado: 688 tests verdes con la feature; ambas builds (con y sin) compilan limpias; el
binario solo-VM ejecuta y rechaza `--interp`.

### 40.2 M35c — gate de regresión de rendimiento; CIERRA M35 y el arco A

El banco `benchmarks/` medía pero no **fallaba**. M35c le pone un gate: `regress.py`
(python3, cero deps) mide la VM de release (mejor-de-15) y la compara contra un baseline
commiteado (`baseline.json`), saliendo con **código 1 si algún caso es >5 % más lento**. Un
envoltorio `#[ignore]` (`tests/perf_regression.rs`) lo hace descubrible desde `cargo test`.

Dos decisiones que la medición real forzó:
- **La huella de máquina.** Los tiempos absolutos dependen del hardware → el baseline guarda
  una huella (plataforma + CPUs + modelo); si no casa, el gate degrada a **informativo** (sale
  0). En CI —mismo runner— casa y es estricto, que es lo que pide M35c; en otra máquina se
  graba el baseline propio (`--record`). Así el gate es seguro en cualquier sitio y protege de
  verdad donde se grabó.
- **Mejor-de-15, umbral 5 %.** A mejor-de-7 el banco tiene ~5 % de varianza en un portátil (el
  gate false-positivea, cazado en pruebas); a mejor-de-15 baja a ~1-1.5 % → ~3.5 % de holgura
  bajo el 5 %. Es la N que `measure.py` ya necesitaba. Verificado: tres corridas seguidas dan
  +1.5 %/+0.8 % (pasa), y forzar el umbral a 1 % hace fallar el gate (prueba de que muerde).

**M35 COMPLETO** (a: VM de producto · b: intérprete tras la feature · c: gate de regresión).
**Arco A (estabilidad) COMPLETO**: M33 diagnósticos de producción · M34 SPEC + versionado ·
M35 un solo motor. Siguen, en paralelo, el arco **B** (rendimiento: M36 VM, M37 GC, M38 M:N
por actores) y el arco **C** (ecosistema: M39 `ray`+paquetes, M40 stdlib 1.0, M41 FFI).

## 41. M39 — CLI unificado `ray` + gestor de paquetes (arco C)

Primera fase del arco C (ecosistema, PRODUCCION.md). El análisis marcó la falta de gestor de
paquetes como la **brecha nº1 de adopción**; M39 la ataca, empezando por el CLI que lo alojará.

### 41.1 M39a — el CLI de subcomandos

Hasta aquí el binario era `raylang` con **flags sueltas** (`--vm`/`--test`/`--fmt`/`--lsp`/
`--repl`/`--version`/`--interp`). M39a lo reorganiza en un CLI de **subcomandos** estilo
`cargo`/`go`, bajo el nombre de producto **`ray`**:

- `ray new <nombre>` — esqueleto de proyecto: `ray.toml` (el manifiesto que leerá M39b) +
  `src/main.ray` (hola-mundo) + `.gitignore`.
- `ray run [archivo]` — ejecuta; sin archivo usa `src/main.ray` (convención de proyecto).
  `--interp` fuerza el intérprete; los args tras el archivo van a `args()`.
- `ray build [archivo]` — **nuevo**: chequea y compila **sin ejecutar** (para CI / validar
  antes de publicar); sale 0 si compila, 65 si hay errores (multi-error de M33c).
- `ray test [archivo] [filtro]` · `ray fmt <archivo>` · `ray lsp` · `ray repl` · `ray version`
  · `ray help`.

**Dos binarios, un código.** La lógica del CLI se movió a `src/cli.rs` (módulo de la lib);
`src/main.rs` (→ `raylang`) y `src/bin/ray.rs` (→ `ray`) son envoltorios de una línea sobre
`cli::main`. Así el nombre de producto es `ray` y `raylang` sobrevive como alias — los tests
usan `CARGO_BIN_EXE_raylang` y no se tocan.

**Compatibilidad total.** La interfaz por flags se conserva: un primer argumento que no sea un
subcomando conocido cae al **modo legado** (`legacy`), que reconoce `--vm`/`--interp`/`--test`/
`--fmt`/`--lsp`/`--repl` y el `<archivo>` directo. Los 688 tests (todos con flags o ruta
directa) pasan sin cambios; los subcomandos nuevos se prueban en `tests/cli_cli.rs`.

### 41.2 M39b — el manifiesto `ray.toml` dirige el build

Un proyecto raylang es un directorio con `ray.toml` en su raíz. M39b lo **lee** y hace que
`run`/`build`/`test` se dirijan por él.

- **`src/manifest.rs`**: un `Manifest { name, version, entry, dependencies, root }` + un
  **lector TOML mínimo en Rust** (secciones `[tabla]`, `clave = "cadena"`, comentarios `#`;
  errores con nº de línea). **Decisión**: no se reusa el `toml.ray` de M32.2 — el CLI necesita
  leer la config *antes* de ejecutar nada, así que arrancar el intérprete solo para parsear el
  manifiesto sería circular. El subconjunto soportado es todo lo que `ray.toml` usa.
- **Descubrimiento del proyecto** (estilo `cargo`/`git`): `Manifest::find` sube por los
  ancestros del directorio actual hasta hallar un `ray.toml`. Así `ray run` funciona desde
  cualquier subdirectorio del proyecto.
- **`resolver_entrada`** (en `cli.rs`) unifica la resolución del archivo a procesar: (1) el
  archivo explícito de la línea de comandos; (2) la `entry` del manifiesto (por defecto
  `src/main.ray`); (3) `src/main.ray` en el cwd; si nada, error de uso. `ray build` imprime un
  banner `compilando <nombre> v<versión>`.
- **Dependencias**: se parsean pero **aún no se resuelven** (M39c); un manifiesto con
  dependencias avisa con claridad ("su resolución llega en M39c; se ignoran"). Un `ray.toml`
  mal formado es error de compilación (65) con la línea.

Tests: unitarios del parser (`manifest.rs`) + integración (`cli_cli.rs`: entry del manifiesto,
subida desde subdirectorio, aviso de deps, error del manifiesto).

### 41.3 M39c-1 — la caché `.ray-deps/` es raíz de módulos (un paquete = una cápsula)

Primer paso del gestor de paquetes: hacer que las dependencias ya descargadas sean **importables**.
La pieza clave es una decisión de diseño que reutiliza todo el sistema de módulos existente:

> **Una dependencia descargada es una cápsula.** Un paquete `geo` vive en `.ray-deps/geo/` con su
> `mod.ray`; por M11.6 eso es una cápsula direccionable. `import geo;` la trae; sus submódulos
> internos (`geo/interno`) quedan protegidos por el *enforcement* de cápsula (M11.6b) **gratis** —
> la frontera de encapsulación de un paquete es la misma que la de una cápsula del proyecto.

Implementación (front-end puro; runtime intacto):

- El **loader** pasa de una sola raíz a una **lista de raíces de búsqueda**: `load_con_deps(entry,
  dep_roots)` construye `[raíz_del_proyecto, ...dep_roots]`. `resolve_module_path` y
  `capsula_violada` iteran las raíces; la primera que resuelve gana → **lo local tapa** a una
  dependencia homónima. Con `dep_roots` vacío el comportamiento es idéntico (`load` delega con `&[]`).
- El **CLI** añade `.ray-deps/` (junto al `ray.toml`) como raíz si existe (`raices_de_dependencias`).
  El aviso de M39b se afina: solo avisa de las dependencias que **faltan** en la caché (las presentes
  ya se resuelven); su **descarga automática** desde git llega en M39c-2 (por ahora se colocan a mano).

Tests (`cli_cli.rs`): `from geo import` desde la caché, acceso calificado `geo.f()` con la cápsula
usando su propio interno, el *enforcement* que impide a la app alcanzar `geo/interno`, y el shadowing
local-sobre-dependencia.

### 41.4 M39c-2a — descarga git-first

Segundo paso: **descargar** de verdad las dependencias declaradas. Una dependencia es
`nombre = "git+<URL>@<ref>"` en `[dependencies]`; se clona el repositorio en `.ray-deps/<nombre>/`
(donde M39c-1 la encuentra como cápsula). Un paquete publicable tiene su `mod.ray` en la **raíz**
del repo (su cara pública).

**Decisión: se delega en el binario `git` del sistema** (`std::process::Command`, `src/deps.rs`).
Hablar el protocolo git a mano (packfiles, smart-HTTP, resolución de refs) sería enorme y ajeno a
lo pedagógico del proyecto; *shelling out* a `git` es la vía honesta y **no rompe la invariante
cero-dependencias de Cargo** — `git` es una dependencia del *entorno*, no un crate enlazado.

- `parse_spec("git+<URL>@<ref>")`: el `git+` marca el esquema; el `@<ref>` es **obligatorio** (fijar
  la versión → build reproducible); se parte por el **último** `@` (no rompe `usuario@host`).
- `fetch`: `git clone --quiet <URL> <dest>` + `git checkout --quiet <ref>` (sirve tag/rama/SHA) +
  `git rev-parse HEAD` (el commit resuelto, para el lockfile de 2b). Si el checkout falla (ref
  inexistente) se **limpia** el clon a medias.
- `asegurar(manifest)`: descarga las que **falten** (las presentes se saltan → sin red ni `git` si
  ya están cacheadas). La usan el subcomando **`ray fetch`** (explícito) y la **auto-descarga** en
  `run`/`build`/`test` (`resolver_entrada`, estilo cargo): un fallo de descarga aborta con 65.

Tests **offline y deterministas** (`deps_cli.rs`): la dependencia es un repo git local (`git init` +
tag) servido por `file://` — `ray fetch` la clona, `ray run` la auto-descarga y usa, una ref
inexistente falla dejando la caché limpia. Unitarios de `parse_spec` en `deps.rs`.

### 41.5 M39c-2b — lockfile `ray.lock` con hashes (supply-chain)

La descarga (2a) fija la versión; falta garantizar que lo que se **ejecuta** es lo que se
descargó y que **no se manipuló** después. Es lo que da el lockfile.

- **SHA-256 en Rust puro** (`src/sha256.rs`): implementado a mano —no vía `ring`— para respetar la
  invariante cero-deps de Cargo (la única excepción es TLS). Verificado contra los vectores del NIST.
- **Hash de contenido de un paquete** (`deps::hash_package`): un SHA-256 sobre el resumen
  `ruta:sha256(contenido)` de cada archivo (ordenados; ignora `.git`) — un árbol de hashes tipo
  Merkle, memoria acotada. Detecta cualquier cambio de contenido o de rutas.
- **`ray.lock`**: por dependencia, `url` + `ref` + `commit` resuelto + `hash` de contenido. Formato
  = el mismo subconjunto de TOML que `ray.toml`; entradas **ordenadas por nombre** (diffs limpios);
  **se commitea** (fija las versiones para el equipo). `.ray-deps/` sí se ignora, `ray.lock` no.
- **Verificación en cada resolución** (`asegurar`): para cada dependencia cacheada se **recomputa** el
  hash y se compara con el bloqueado (si el spec `url@ref` coincide); un desajuste = **error de
  *supply-chain*** ("su contenido cambió desde que se bloqueó"), con el hash esperado vs actual. Las
  que faltan se descargan y se registran; el lock se reescribe con el estado actual.

Tests (`deps_cli.rs`): `fetch` genera el lock con hash/commit; `run` re-verifica sin error; manipular
un archivo cacheado → aborta (65) con el mensaje de supply-chain. Vectores NIST en `sha256.rs`.

### 41.6 M39c-3 — dependencias transitivas + resolución de conflictos

Un paquete puede tener **sus propias dependencias** (su `ray.toml` con `[dependencies]`). `asegurar`
pasa de iterar las deps directas a un **BFS sobre el grafo**: por cada paquete descargado lee su
`ray.toml` (lenient, `deps_del_paquete`) y encola SUS dependencias, hasta agotar el grafo. Todo va a
la **caché plana** `.ray-deps/<nombre>/` (un slot por nombre) — el loader ya las encuentra a todas.

- **Ciclos y dedup**: un mapa `elegido` (nombre → spec resuelto) cierra los ciclos y evita re-procesar.
- **Conflictos** (el mismo nombre pedido con specs distintos): como la caché es plana, hay que elegir
  UNA versión. **MVS ligero** (`mvs`): con la misma URL y refs **semver** (`vX.Y.Z`), gana el mayor
  (reinterpretando `@vX` como "al menos vX", estilo Go-MVS); si un conflicto sube la versión ya
  cacheada, se re-descarga. URLs distintas o refs no-semver (rama/commit) → **error** (irreconciliables).
- El lock (M39c-2b) registra **todo el grafo** (directas + transitivas), cada uno con su hash.

### 41.7 M40.8a — dependencias por ruta local (`path:<dir>`)

Habilita **paquetes adicionales no embebidos**: una librería que vive en el mismo repo (o en una carpeta
local) y se consume sin publicarla en git. `ray.toml`: `nombre = "path:<dir>"`. A diferencia de una git,
una path-dep **no se descarga ni se bloquea/hashea** (es local y mutable, para desarrollo o un paquete
del propio proyecto): `deps::ruta_de_path_dep` la reconoce por el prefijo `path:`, `asegurar` la **salta**
(directas y transitivas), y el CLI (`raices_de_dependencias`) **registra su carpeta como raíz de
módulos** —añade el *padre* de `<dir>`, así `import <nombre>;` resuelve `<dir>/mod.ray` (cápsula) con la
misma maquinaria de M39c-1 (cápsula + enforcement M11.6b)—. El paquete puede vivir **fuera** del proyecto
que lo importa. Reusa todo el loader; cero cambios de runtime. Es la base para el paquete de **red**
(§tier de red de M40.7): las librerías que dependen de sockets/TLS **no** van en el binario base (serían
peso muerto para quien no las use), sino en un paquete que se trae por `path:`/git y se apoya en las
`std/` embebidas para lo fundacional (`from std/hmac import`, …). Tests: `distingue_path_dep_de_git`
(deps.rs) + `dependencia_por_ruta_local` (cli_cli, paquete-cápsula externo, sin `.ray-deps`).

### 41.8 M40.8b — el paquete `net` (adicional, no embebido)

Sobre las path-deps, el **tier de red** se materializa como un paquete: `packages/net/` — un directorio de
módulos (importados `net/jwt`, `net/http`, … como `std/math`; sin cápsula/`mod.ray`, todos públicos). **No
va en el binario base** (peso muerto para quien no lo use); se consume por `net = "path:…/packages/net"` (o
git al publicarse) y se apoya en las `std/` embebidas para lo fundacional (los `from hmac import` de los
ejemplos se reescriben a `from std/hmac import`). Primer grupo, **autenticación y firma** (deterministas →
testeables): `net/jwt` (HS256, sobre std/hmac+std/base64), `net/jwt_eddsa` (EdDSA, std/ed25519), `net/sigv4`
(AWS SigV4, std/hmac+sha256+url), `net/scram` (SCRAM-SHA-256, std/hmac+sha256+base64), `net/cookie` (std/url).
Los `examples/web/*.ray` siguen como material pedagógico + sus tests; el paquete es la versión distribuible
(imports a `std/`). Test `paquete_net_jwt_via_path_dep` (cli_cli, firma+verifica un JWT). Diferido: HTTP/
HTTP2/WebSocket/DNS/UDP/Redis/Postgres/gRPC/OAuth2 (los de sockets vivos, con servidor de juguete).

Tests: `resolve_dependencias_transitivas` (cadena app→geo→mathx, offline; el lock incluye ambos) +
unitarios de `semver`/`mvs`. **M39c COMPLETO** (gestor de paquetes: cápsula → git → lock → transitivas).
Diferido: versión-mínima con RANGOS de verdad (nuestras specs son refs exactas), `ray update`, re-exports.

## 42. M40 — stdlib 1.0 (arco C)

### 42.1 M40.1a — guardas en los brazos del `match`

Primera pieza de la **ergonomía de match** (deuda de M5.2). Un brazo puede llevar una **guarda**:
`patrón if <cond> => cuerpo`. Casa solo si el patrón liga Y la `cond` (`bool`, con los bindings del
patrón en ámbito) evalúa a `true`; si no, se sigue al siguiente brazo.

- **AST**: `MatchArm` gana `guard: Option<Expr>`. Parser: `if <expr>` entre el patrón y el `=>`.
- **Checker**: la guarda se chequea como `bool` en el ámbito de los bindings del patrón. **Clave para
  la exhaustividad**: un brazo con guarda **no** cuenta como cubierto (puede no casar) → se le pasan
  `covered`/`catchall` temporales, así `match (o) { Some(n) if n>0 => …, None => … }` es **no
  exhaustivo** (falta `Some` sin guarda) y tampoco vuelve inalcanzables los brazos siguientes.
- **Intérprete**: tras ligar el patrón, evalúa la guarda; si no es `true`, suelta el ámbito y sigue.
- **VM**: `emit_match` reescrito y unificado — los saltos "al siguiente brazo" (test de variante
  fallido, guarda falsa) se acumulan en `to_next`; cada uno deja UN bool y en runtime se toma como
  mucho uno, así un solo `Pop` los limpia. `end_scope` es compile-time → saltar por encima es seguro.
- **fmt**: renderiza `patrón if <cond> => …` (antes lo habría descartado — bug lossy corregido).

Oráculo VM↔intérprete (`guardas_oraculo`). SPEC §5 actualizado. Runtime: cero opcodes nuevos.

### 42.2 M40.1b — `if let`

`if let patrón = expr { then } [else …]`. **Azúcar puro del parser** (como pipelines/interpolación):
se desazucara a `match (expr) { patrón => { then }, _ => else }` (sin `else`, el brazo `_` es un bloque
vacío → unit). El parser detecta `if` seguido de `let` (`if_let_expr`); el escrutinio va sin paréntesis
hasta el `{` (con `no_struct_lit`, como la cabecera de un `for`); `else if` encadena poniendo la
expresión `if` en el brazo `_`. El patrón usa la **misma gramática que el match** (variantes
calificadas: `if let Option.Some(v) = o`). Checker, motores y fmt lo ven como un `match` corriente
(fmt lo reimprime como match, lossy como el resto del azúcar). Oráculo `if_let_oraculo`. Cero cambios
fuera del parser. Siguiente: **M40.1c** (patrones anidados).

### 42.3 M40.1c — patrones de variante anidados

Un sub-patrón de variante deja de ser un binding plano y pasa a ser un **patrón completo, recursivo**:
`match (r) { Result.Ok(Option.Some(v)) => v, Result.Ok(_) => …, Result.Err(e) => … }`.

- **AST**: `PatternKind::Variant.bindings: Vec<Option<String>>` → `subpatterns: Vec<Pattern>` (recursivo).
  El viejo `None` es un `Wildcard`; el viejo `Some(n)` un `Binding(n)`.
- **Parser**: cada posición se parsea con `pattern()` (recursivo).
- **Checker**: `check_pattern` se parte en `check_subpattern` (recursivo: resuelve el enum del **tipo**
  de cada sub-valor —el payload sustituido—, no de un parámetro externo; recurre sobre los sub-patrones)
  + `registrar_cobertura` (exhaustividad **conservadora**: una variante cuenta como cubierta solo si
  TODOS sus sub-patrones son catch-all; un sub-patrón anidado no la marca → hace falta un fallback).
- **Intérprete**: `match_pattern` recursivo (un sub-patrón que no casa hace fallar el brazo entero).
- **VM**: `emit_pattern_test` recursivo — cada `EnumTagEq` (a cualquier profundidad) que puede fallar
  añade su salto a `to_next`; el sub-valor se extrae a un local temporal (`$sub`) y se recurre. En
  runtime el primer fallo salta dejando UN bool → un solo `Pop` los limpia (invariante ya de las guardas).
- **loader**: `shift_pattern` (posiciones) y `rewrite_pattern` (namespacing del `enum_name`) recursivos;
  `recolectar_bindings` recursivo. **Bug latente de M40.1a corregido**: los 9 pases de lowering/
  renumerado (`resolve`/`freshen`/`renumber`/`lower_*`) recorrían `arm.body` pero NO la **guarda** —una
  guarda con UFCS/método/`?`/operador no se bajaba—; ahora también la recorren.
- **fmt**: renderiza los sub-patrones recursivamente.

Oráculo `patrones_anidados_oraculo` (`Result<Option<int>>` + `Option<Option<int>>`) + guarda con UFCS.
SPEC §5. Runtime: cero opcodes nuevos. Siguiente: **M40.1d** (patrón de struct `Punto { x, y }`).

### 42.4 M40.1d — patrón de struct

`Nombre { campo [: sub-patrón], … }` destructura un struct dentro de un match. Cierra el ejemplo
`match (o) { Option.Some(Punto { x, y }) => x + y, Option.None => 0 }`. **Caso aditivo** sobre la
maquinaria recursiva de M40.1c.

- **AST**: nueva `PatternKind::Struct { name, fields: Vec<(String, Pattern)> }`. Forma corta
  `{ x, y }` = `{ x: x, y: y }` (el parser la expande a bindings). Solo aparece **anidado** (el
  escrutinio de un match es un enum).
- **Parser**: `pattern()` detecta `Ident {` (inequívoco en posición de patrón; no hay ambigüedad con
  un bloque, a diferencia de las expresiones → sin `no_struct_lit`).
- **Checker**: caso `Struct` en `check_subpattern` (el valor debe ser ese struct; se resuelven los
  tipos de campo con σ del struct genérico; recurre por cada campo listado —parcial permitido—).
- **Motores**: intérprete recursivo (extrae el campo del `StructInstance`); VM `emit_pattern_test`
  sin tag (el struct siempre casa por tipo), extrae cada campo con `GetField` a un temporal y recurre.
- **loader**/**fmt**: recorren/renderizan los campos recursivamente; `rewrite_pattern` namespaca el
  nombre del struct.
- **Exhaustividad afinada**: se introduce `es_irrefutable(patrón)` (`_`/binding, o struct de campos
  irrefutables). Una variante de primer nivel cubre si sus sub-patrones son irrefutables → así
  `Option.Some(Punto { x, y })` + `Option.None` es **exhaustivo sin fallback** (antes lo pedía). Una
  variante anidada sigue siendo refutable (conservador).

Oráculo `patrones_struct_oraculo`. SPEC §5. Runtime: cero opcodes nuevos. **M40.1 (ergonomía de
match: guardas + `if let` + patrones anidados + struct) COMPLETO.** Diferido: patrones de literal,
patrón de struct de primer nivel (`let Punto { x, y } = p`), `M.Struct { … }` calificado.

### 42.5 M40.2 — el trait `Iterator` (`for x in it`)

`for x in it` deja de ser solo para arreglos/strings/Map: un tipo que implemente el trait del prelude
`Iterator<T> { fn next(self) -> Option<T>; }` es iterable. El bucle llama a `next` hasta `None`,
ligando cada elemento. **Caso aditivo** (elige el enfoque incremental; no re-funda map/filter/fold aún).

- **Prelude**: `trait Iterator<T> { fn next(self) -> Option<T>; }`. Como los structs son valores de
  referencia con campos mutables, `next(self)` avanza el estado del propio iterador entre iteraciones
  (no hace falta `&mut` — la semántica de referencia lo da gratis).
- **Traits genéricos con parámetro**: `Iterator<T>` es el segundo trait con args (tras `From<S>` de
  M28.2). Habilitó dos arreglos en el despacho: (1) `is_typed_trait_impl` se restringe a `From` (los
  demás traits parametrizados usan despacho por punto ordinario, no el mecanismo de conversión); (2)
  `check_method_sig` recibe un `trait_sigma` (los params del trait, p. ej. `T`→`int` en
  `impl Iterator<int>`) y sustituye la firma **tras** `resolve_type` (orden clave: primero `Struct("T")`
  → `Var("T")`, luego σ lo fija) → el `next` del impl valida contra `-> Option<int>`.
- **Detección + lowering en el checker**: en el caso `for` (rama `In`, receptor único), si el iterable
  no es arreglo/string/Map se prueba `iterator_de(ty)`: busca `(type_key, "Iterator")` en `impl_traits`,
  saca el `next` manglado de `methods` y extrae el elemento del `Option<T>` de su retorno. Se registra
  en `for_iter_sites` (posición del `for` → nombre manglado de `next`) y un pase **`lower_for_iters`**
  (espejo de `lower_ufcs`) reescribe `ForIter::In(e)` → `ForIter::Iter { expr, next_fn }`.
- **AST**: `ForIter` gana la variante `Iter { expr, next_fn }`. La produce solo el lowering; el parser
  emite siempre `In` (la sintaxis `for x in it` ya existía). Todos los walkers (renumber/freshen/
  resolve/lower_*/loader/fmt) tratan `Iter.expr` como `In`.
- **Motores**: intérprete — evalúa el iterable una vez y llama a `call_function(next, [it])` en bucle,
  desempaquetando el `Option` hasta `None`. VM (`emit_for`) — guarda el iterable en un local, y en cada
  vuelta `Call(next, 1)` → `EnumTagEq(0)` (¿`Some`?) → si no, sale; si sí, `GetEnumField(0)` liga el
  elemento y ejecuta el cuerpo. **Cero opcodes nuevos** (reusa los de enum + `Call`).

Oráculo `iterator_for_oraculo` (el estado del iterador muta por referencia entre iteraciones). Ejemplo
`examples/stdlib/iterador.ray`. SPEC §7.

#### M40.2b — `.iter()` sobre arreglos y `range`

Iteradores de **primera clase** para arreglos y rangos, **escritos en el prelude en raylang** sobre
`Iterator<T>` — front-end puro, cero runtime, reusan la maquinaria de M40.2a. `xs.iter()` (UFCS de la
función `iter`) da un `ArrayIter<T>` que recorre el arreglo; `range(a, b)` un `RangeIter` sobre los
enteros `a..b` (semi-abierto) — el `a..b` del `for`, pero como **valor** que se puede pasar, guardar y
recorrer. El estado del cursor vive en los campos del struct (mutados por referencia en `next`).

- **Prelude**: `struct ArrayIter<T> { datos, pos }` + `impl<T> Iterator<T> for ArrayIter<T>`, y
  `struct RangeIter { actual, fin }` + `impl Iterator<int> for RangeIter`, con `fn iter<T>(xs: [T]) ->
  ArrayIter<T>` y `fn range(desde, hasta) -> RangeIter`. **Primeros structs del prelude**: se añade
  `prelude::structs()` y un paso `0a` en `prepare_program` que los inyecta (idempotente, saltando los
  que el usuario redefina), como los enums Option/Result.
- **Impl genérico de un trait parametrizado**: `impl<T> Iterator<T> for ArrayIter<T>` es el primero que
  combina genéricos del impl con args del trait. Obligó a robustecer `iterator_de`: el `next` manglado
  es una función **genérica** (`-> Option<T>` con `T` sin fijar), así que se **unifica** el tipo del
  receptor (`self`, `params[0]`) con el tipo real del iterable para obtener σ y sustituir el retorno →
  el elemento sale concreto (`int` para `[int].iter()`), no la variable `T`. Un impl concreto (Contador)
  da σ vacío → `Option<int>` pasa tal cual (compatibilidad exacta con M40.2a).

Oráculo `iter_range_oraculo` (impl genérico + sustitución del elemento, int y string).

#### M40.2c — métodos genéricos + adaptadores perezosos `.map()`/`.filter()`

Adaptadores de iterador **perezosos**: `it.map(f)` y `it.filter(pred)` devuelven otro iterador que
solo calcula al recorrerse, encadenables (`range(0,n).map(f).filter(p)`). Dos decisiones de diseño
que se apoyan mutuamente:

- **Type-erasure con un closure**: un iterador ES, en el fondo, un `fn() -> Option<T>` con estado. La
  representación unificada es `struct Iter<T> { paso: fn() -> Option<T> }` (con `impl<T> Iterator<T>
  for Iter<T>`); `iter`/`range`/`map`/`filter` producen todos `Iter<T>`. Esto **esquiva el bound**
  `I: Iterator<T>` (raylang no permite bounds sobre traits parametrizados): al ser todos el mismo tipo
  concreto, no hace falta ser genérico sobre "cualquier iterador". El estado (posición, cursor) vive
  en variables capturadas por el closure (mutadas por referencia — la captura mutable de M4.2).
- **`map`/`filter` como métodos por DEFECTO de `Iterator`**: así los tiene todo iterador (incl. los de
  usuario) y **se desambiguan del `map`/`filter` eager de arreglos por el tipo del receptor** —un
  arreglo no implementa `Iterator` → cae en la función libre `map(xs, f) -> [U]`; un iterador → el
  método del trait `Iterator#map`—. raylang no tiene sobrecarga, y esta es la única vía que deja
  coexistir ambos con el mismo nombre.

El habilitador de fondo es una **feature nueva del lenguaje: métodos genéricos** (`fn map<U>(self,
f: fn(T) -> U) -> Iter<U>`) — un método con parámetros de tipo PROPIOS, distintos de los del impl:

- **AST/parser**: `MethodSig` (y el ya existente `Function` de los métodos de impl) gana `type_params`/
  `bounds`; `method_sig`/`impl_method` los parsean con `type_params_with_bounds` (como una `fn`).
- **Checker (bajada, paso 0c)**: el método manglado hereda los `type_params`/`bounds` del impl (M9.2b)
  **más los del propio método** → `Iter#map<T, U>` es una función genérica; la inferencia fija `T` por
  el receptor y `U` por el argumento `f`. Además, para un impl de trait **parametrizado**
  (`impl Iterator<int> for RangeIter`), se sustituyen los parámetros del trait por sus argumentos
  (`T → int`) tanto en la **firma** (`subst_named`, análogo a `subst_self` pero por nombre) como en el
  **cuerpo** del método (`subst_named_block`, recorre las anotaciones de tipo: `filter` escribe
  `Option<T>` en su closure). Solo se activa con traits parametrizados (σ vacío para Eq/Show/… → coste
  cero). El loader pone en ámbito los `type_params` del trait y del método al reescribir tipos.

Oráculo `adaptadores_perezosos_oraculo` (map cambia de tipo, filter avanza el origen, encadenamiento).
Ejemplo `examples/stdlib/iterador.ray`. SPEC §7/§10.

#### M40.2d — operaciones terminales `.fold()`/`.collect()`

Cierran la cadena de iterador: consumen el iterador (perezoso) y producen un valor concreto. Métodos
por defecto de `Iterator`, **puro prelude** (sin tocar el núcleo; reusan los métodos genéricos de
M40.2c). `.fold(init, f)` reduce de izquierda a derecha (método genérico sobre el acumulador `A`:
`fn fold<A>(self, init: A, f: fn(A, T) -> A) -> A`); `.collect()` acumula en un `[T]` (`fn collect(self)
-> [T]`, sin params de tipo propios — usa `T` del trait, sustituido en el cuerpo por `subst_named_block`
para impls concretos). Coexisten con el `fold` **eager** de arreglos por el mismo despacho según receptor
(`[T].fold` → función libre; `Iter<T>.fold` → método del trait). Oráculo `fold_collect_oraculo`.

#### M40.2e — `.take()`/`.enumerate()` + tuplas en la inferencia genérica

`.take(n)` (perezoso, corta a los primeros `n`) y `.enumerate()` (empareja cada elemento con su índice
en `(int, T)`), métodos del prelude. `enumerate` **produce tuplas**, lo que destapó y obligó a cerrar
varios huecos **preexistentes** en el manejo de `Type::Tuple`:

- Las tuplas no participaban en la **inferencia genérica**: `subst`/`unify`/`resolve_type`/`subst_self`/
  `subst_named` no recorrían `Type::Tuple` (caían en el caso por defecto) → `Iter<(int, T)>` no
  sustituía ni unificaba su `T`. Se añadió el caso `Tuple` a los cinco. Bug latente comprobado:
  `fn f<T>(x: T) -> Option<(int, T)> { Option.Some((0, x)) }` fallaba en `main`.
- **Higiene de la construcción**: la inferencia de `Option.Some`/struct-lit keyea σ por nombre de
  parámetro; el `T` de `Option` colisionaba con un `T` rígido del ámbito → `T := (int, T)` (occurs-check
  falso). `freshen_ctor_params` renombra a `$ctor$i` **solo los params que colisionan** (sin colisión,
  intacto → mensajes de error idénticos). El tipo esperado se **resuelve** antes de sembrar σ.
- **Patrón de tupla en el `for` sobre iteradores**: `for (i, x) in it.enumerate()` — el checker liga
  cada nombre a su componente (como el caso `Map`), y ambos motores destructuran el elemento-tupla por
  posición en la bajada de `ForIter::Iter`.

Oráculo `take_enumerate_oraculo`.

#### M40.2f — `.skip()`/`.zip()`/`.sum()`

Completan el juego de adaptadores. **Puro prelude** (reusan la maquinaria de M40.2c–e). `.skip(n)`
(perezoso, descarta los primeros `n` en la primera llamada a `next`) y `.zip(otra: Iter<U>)` (método
genérico, empareja posición a posición en `(T, U)`, se agota con el más corto — reusa la inferencia
sobre tuplas y el patrón-de-tupla-en-`for` de M40.2e) son métodos del trait. **`sum`** va como
**función libre** `sum(it: Iter<int>) -> int` (UFCS `it.sum()`), NO método del trait: un `sum` genérico
necesitaría un cero y un `+` del tipo del elemento (un trait `Zero`/`Sum`), que raylang no expresa aún
→ se especializa a `Iter<int>` (lo más común). `zip` exige que `otra` sea un `Iter<U>` (los adaptadores
lo devuelven; un iterador de usuario se convierte con `.map(...)`). Oráculo `skip_zip_sum_oraculo`.
Diferido: `sum` genérico (requiere `Zero`/`Sum`); bounds en métodos genéricos cruzando módulos.

#### M40.6 — re-fundar `map`/`filter`/`fold` eager sobre `Iterator`

Cierra el diferido de M40.2f. Hasta aquí coexistían **dos** implementaciones de `map`/`filter`/`fold`: las
**funciones libres ansiosas** sobre arreglos (M7.3, un bucle que materializa `[T]` por etapa) y los
**métodos perezosos** del trait `Iterator` (M40.2c/d, devuelven `Iter<T>` y fusionan). Duplicación de
lógica pura. Se eligió la **opción B** (frente a A = modelo Rust, arreglos perezosos + `.collect()`
obligatorio, rompedor; y C = documentar y dejar dos familias): **conservar las firmas ansiosas** pero
reimplementarlas como envoltorio de una línea sobre la maquinaria perezosa —`map(xs,f) =
iter(xs).map(f).collect()`, `filter` análogo, `fold(xs,init,f) = iter(xs).fold(init,f)`—. La lógica queda
en **un solo sitio** (el trait); las funciones libres solo añaden el `iter()…collect()` por ergonomía (un
`[U]` indexable sin ceremonia). **Sin recursión**: dentro del cuerpo, `iter(xs)` es un `Iter<T>`, así que
`.map`/`.filter`/`.fold` resuelven al método del trait (campo→método→UFCS), nunca a la función libre. B
elimina la **duplicación de código**, no la **distinción de coste**: la cara ansiosa sigue sin fusionar
(cada función libre devuelve un `[T]` completo) — inherente a su firma, deliberado (para fusión, se entra
por `.iter()`). Front-end puro, runtime intacto; los 424 oráculos idénticos (misma semántica exacta). La
distinción eager-vs-lazy se documenta en el libro (`book/src/m40/iteradores.md`). Diferido (hito propio si
se quiere): opción A (modelo Rust puro) con su migración; `sum` genérico vía `Zero`/`Sum`.

### 42.6 M40.3 — colecciones (Hash + Set/deque/string-builder)

#### M40.3a — `Hash` derivable

`@derive(Hash)` (3er trait derivable, tras Eq/Show) genera `impl Hash for T { fn hash(self) -> int }`.
El trait `Hash { fn hash(self) -> int; }` vive en el prelude con impls de primitivos **en raylang**:
`int` → sí mismo, `bool` → 0/1, `char` → `char_code(self)`, `string` → polinomio sobre sus caracteres
(`h = h*31 + char_code(c)`). `float` NO es hashable (como en `Map`). El cuerpo derivado combina el
`.hash()` de cada campo (struct) o del payload por variante + su índice (enum) — recursivo, en raylang.
Habilita usar tipos de usuario como claves de tablas hash (Set/HashMap, M40.3b+).

- **Único toque de runtime**: builtin `char_code(c: char) -> int` (opcode `CharCode`; el code point).
  Todo lo demás es front-end (trait + impls + derive, como Eq/Show).
- **Bug de colisión de posiciones (corregido, era latente)**: cada impl derivado se parsea desde la
  línea 1, así que dos derivados comparten `(línea, col)`. El lowering por posición (UFCS/despacho)
  colapsaba `self.x.hash()` (int) y `self.n.hash()` (string) de impls distintos al MISMO destino →
  despacho equivocado (`chars` sobre un int → ICE). Antes solo funcionaba por suerte cuando los campos
  colisionantes iban al mismo destino (`@derive(Show)` con campos del mismo tipo). Fix: `generate_derives`
  asigna a cada cuerpo derivado **posiciones sintéticas únicas y globales** (contador atómico, banda 50M
  disjunta de la 1M de los métodos por defecto), vía `freshen_positions`. Arregla Eq/Show/Hash a la vez.

Oráculo `hash_derive_oraculo` (el hash se calcula en raylang → ambos motores dan el mismo entero).
Diferido: `Hash` cruzando módulos con colisión de posiciones (raro; el fix no sobrevive al shift del
loader para módulos no-entrada).

#### M40.3b — `Set<T>` + inferencia bidireccional en llamadas

`Set<T>` (`struct Set<T> { buckets: [[T]], tam }`) es una **tabla hash bucketed escrita en el prelude**
sobre `@derive(Hash)` + `Eq` (`T` implementa ambos; los bounds se bajan a diccionarios, M9.2). API con
prefijo `set_` (para no chocar con builtins ya tomados: `contains`/`insert`/`remove`): `set_new`,
`set_add`, `set_has`, `set_remove`, `set_size`, `set_items`. `s.set_add(x)` por UFCS. Nº de buckets fijo
(sin resize aún). Deduplica por `.eq()`; el índice de bucket es `x.hash()` normalizado.

Habilitador: **inferencia bidireccional en llamadas genéricas** (M40.3b). `set_new() -> Set<T>` es un
constructor **vacío**: `T` no aparece en los argumentos, así que antes no se podía inferir (`let s:
Set<int> = set_new()` fallaba). Ahora, en `check_expr_expected`, una llamada a una función genérica de
usuario recibe el tipo **esperado**; `check_generic_call` rellena los parámetros de tipo que los
argumentos NO determinan unificando el retorno con el esperado (best-effort en un σ aparte → **los
argumentos siguen mandando**, cero cambio para el código existente). Generaliza el truco de `map_new`/
`channel` (que eran special-cases por nombre) a cualquier función genérica.

Oráculo `set_oraculo` (add/has/remove/size con primitivos y un tipo de usuario, dedup). Ejemplo
`examples/stdlib/conjunto.ray`. Diferido: resize/rehash, `HashMap<K,V>` de usuario, operaciones de
conjunto (unión/intersección).

#### M40.3c/d — StringBuilder + Deque

Dos colecciones lineales, **puro prelude** (structs + funciones, cero runtime). **StringBuilder**
(`struct StringBuilder { partes: [string] }`): `sb_new`/`sb_push`/`sb_build`/`sb_count`. Acumula trozos
y los une **una vez** con `join` al final (`sb_build`) → O(total) en vez del O(n²) de concatenar con `+`
en un bucle (cada `+` copia todo lo acumulado). **Deque** (`struct Deque<T> { datos: [T], head }`):
`deque_new`/`push_back`/`push_front`/`pop_front`/`pop_back`/`peek_front`/`len`/`is_empty` (los `pop`/
`peek` → `Option<T>`). Respaldada por un arreglo + índice `head` (los vivos son `datos[head..]`):
`push_back`/`pop_front`/`pop_back` O(1), `push_front` O(1) con hueco o O(n) reconstruyendo. Sirve de
cola (FIFO: push_back+pop_front), pila (LIFO: push_back+pop_back) o doble-extremo. Prefijos `sb_`/
`deque_` (evitan chocar con builtins; UFCS: `sb.sb_push(s)`, `d.deque_push_back(x)`). Los constructores
vacíos genéricos (`deque_new`) reusan la inferencia bidireccional de M40.3b + el patrón `var d: [T] = []`
(el `[]` inline en un campo de tipo genérico aún no se infiere — limitación menor).

Oráculo `sb_deque_oraculo`. Ejemplo `examples/stdlib/builder_deque.ray`. **M40.3 (colecciones: Hash +
Set + StringBuilder + Deque) COMPLETO.** Diferido: HashMap de usuario, resize del Set, `[]` inline en
campo genérico.

### 42.7 M40.4 — `std/` + raydoc

#### M40.4a — raydoc (`ray doc`)

Generador de **documentación Markdown** a partir de fuente raylang. **Cliente del front-end** (como
`fmt`/`lsp`, `src/raydoc.rs`): lexer+parser para la lista de ítems y sus **firmas**, y un escaneo del
fuente para los **comentarios de documentación** `///` que preceden a cada ítem (contiguos, unidos en un
párrafo). No toca el núcleo. Documenta la **superficie pública** (ítems `pub`) si la hay; si no (un
programa suelto), todos. Agrupa por Traits/Structs/Enums/Funciones, con la firma reconstruida del AST
(genéricos + bounds; el receptor `self` sin tipo). Nuevo subcomando `ray doc <archivo>` (`src/cli.rs`),
en paralelo a `ray fmt`. Tests unitarios (`raydoc.rs`) + integración (`cli_cli.rs`). Diferido: doc de
proyecto multi-archivo, salida HTML, enlaces cruzados.

#### M40.4b — la biblioteca estándar `std/`

Módulos de biblioteca **escritos en raylang**, importables con la sintaxis de módulos por ruta
(`import std/math;`, M11.5) y usados calificados (`math.gcd(...)`). A diferencia del **prelude**
(inyectado siempre: Option/Result, map/filter/fold, Set/Deque/StringBuilder, Iterator), la `std/` es
**opcional** (solo se carga lo importado). **Reusa el mecanismo de raíces de módulos** de M39c: el CLI
añade el directorio que **contiene** `std/` como una raíz más (junto a `.ray-deps/`), así `import
std/math;` resuelve `std/math.ray`. **Descubrimiento** (`raiz_std` en `cli.rs`): la env `RAYLANG_STD`,
o subiendo desde el ejecutable (en el repo, `target/…/ray` → la raíz con `std/`). Primer módulo:
**`std/math`** (utilidades enteras que complementan los builtins: `iabs`/`sign`/`clamp`/`gcd`/`lcm`/
`ipow`/`factorial`/`is_prime`), todo `pub` y documentado con `///` (→ `ray doc std/math.ray`). Integración
en `cli_cli.rs`. Primer módulo: **`std/math`**; **`std/text`** (M40.4c) le siguió. **M40.4 (std/ +
raydoc) COMPLETO.**

### 42.8 M40.5 — `std/` embebida en el binario (auto-contención)

Cierra el diferido de §42.7: la `std/` deja de vivir en disco relativa al ejecutable y se **empaqueta en
el binario**, igual que el prelude. Los `std/*.ray` del repo se compilan dentro con `include_str!` (módulo
nuevo `src/stdlib.rs`: tabla `MODULOS = [(nombre, fuente)]` + `embedded(nombre) -> Option<&'static str>`);
siguen siendo la **única fuente de verdad** (un módulo = un `.ray` + una fila). El **loader** consulta
`stdlib::embedded` **antes del filesystem**, tanto al resolver el nombre de módulo (ruta sentinela `<std>/…`)
como al leer la fuente → un `import std/math;` funciona **sin `std/` en disco**: el ejecutable es
auto-contenido (`ray run` desde cualquier directorio, con `RAYLANG_STD` roto, resuelve igual). El prefijo
`std/` queda **reservado** (gana a un archivo local homónimo). Se **elimina** el descubrimiento por
filesystem (`raiz_std`/`RAYLANG_STD` en `cli.rs`), ahora código muerto. `ray doc std/math.ray` sigue leyendo
el archivo directamente (por eso los `.ray` permanecen en el repo). Front-end puro, runtime intacto.
Diferido: más módulos, promover librerías de `examples/` a `std/`.

**M40.5b — `std/sort`**: tercer módulo de la stdlib (primero embebido de nacimiento). Orden y búsqueda
sobre arreglos **genéricos** (`T: Ord`, el trait del prelude), alrededor del `sort` genérico que ya vive
en el prelude: `is_sorted`, `sort_desc` (`reverse(sort(a))`), `min`/`max`, `binary_search` (O(log n) sobre
un arreglo ordenado; la igualdad se deriva de `Ord`: `x==y` ⟺ ni `x<y` ni `y<x`), `dedup` (ordena y quita
repetidos) y `merge` (fusiona dos ordenados). Demuestra que un módulo `std/` puede ser **genérico con
bounds** (el diccionario de `Ord` se reenvía a través de la frontera de módulo, M9.2) y componer con el
prelude (`sort`/`reverse`). Test en `cli_cli.rs` (`stdlib_sort_busca_y_deduplica`).

### 42.9 M40.7 — promover librerías de `examples/` a `std/`

Cierra el diferido de §42.8: las librerías que hasta ahora solo vivían como **ejemplos** (`examples/web/`,
`examples/stdlib/`) se promueven a `std/` para que sean importables (`import std/json;`) y auto-contenidas.

**Naturaleza dual del catálogo.** Las librerías de `examples/` se parten en dos:
- **Fundacionales, puras y deterministas** — encoding (hex/base64/url/json), hashing (sha1/256/512, hmac),
  compresión (inflate/deflate/huffman), primitivas cripto (chacha20/poly1305/ed25519), protobuf. **Son el
  corazón de una stdlib**: se promueven.
- **Red viva / protocolos** (udp/dns/http/http2/websocket/grpc/postgres/redis/oauth2/…) — dependen de
  sockets/TLS, no deterministas; **son un *framework de aplicación*, no una biblioteca estándar**. Se dejan
  como *tier* aparte (siguen en `examples/`), a la espera de un `net`/paquete propio. Diferido.

**Cero duplicación.** Como `std/` se embebe con `include_str!` (M40.5), un módulo `std/X` promovido apunta
al **`examples/…/X.ray` original** — la fuente es única, el ejemplo sigue siendo el artefacto pedagógico
(referenciado por el libro y los tests) y a la vez la fuente del módulo `std/`. No se copia código.

**M40.7a — encoding** (hojas puras, sin `import` → se embeben verbatim): `std/hex` (`hex_encode`/
`hex_decode`), `std/base64` (`base64`/`base64url` + `*_decode` → `Result`), `std/url` (`url_encode`/
`url_decode`, `parse_query`/`build_query` sobre `Map`), `std/json` (`enum Json`, `parse`/`stringify`). Cuatro
filas en `MODULOS`. Test `stdlib_encoding_hex_base64_url_json` (cli_cli). Las librerías con dependencias
(`from X import`) se namespacan a `std/X` en M40.7b+ (reescribir su import a `from std/…`, verificado contra
la resolución embebida). Diferido: hashing, compresión, cripto, protobuf; el *tier* de red.

**M40.7b — hashing**: `std/sha1`, `std/sha256` (hojas, verbatim), `std/sha512` (→`std/hex`), `std/hmac`
(→`std/sha256` + `std/hex`). Las dos con deps **dejan de ser hojas**: se reescribe su `from hex import` →
`from std/hex import` (etc.) **en el propio ejemplo**, y la resolución embebida lo satisface corran donde
corran (un temporal de test sin `hex.ray`/`sha256.ray` al lado resuelve igual — verificado con
`hmac_cli`/`sha512_cli`/`sigv4_cli`, que copian estas libs a un dir temporal). API sobre `bytes` (`string`
→ `bytes` con el builtin `to_bytes`). Vectores conocidos en `stdlib_hashing_vectores_conocidos` (cli_cli):
sha256("abc"), sha512(""), hmac_sha256("",""), sha1("abc").

**M40.7c — compresión**: `std/inflate` (`inflate_raw`/`zlib_inflate`/`gunzip` + `crc32`; hoja), `std/huffman`
(`huffman_encode`/`huffman_decode`; hoja) y `std/deflate` (`deflate_raw`/`zlib_compress`/`gzip_compress`;
→`std/inflate` para el CRC). Test de roundtrip `stdlib_compresion_roundtrip` (cli_cli): deflate→inflate
recupera el texto, huffman ida y vuelta. `deflate_cli`/`inflate_cli`/`http_cli` intactos.

**M40.7d — texto/datos**: librerías puras de `examples/stdlib/` (todas hojas, verbatim): `std/regex` (motor
NFA lineal de Thompson: `full_match`/`search`/`find`/`find_all`/`replace_all`), `std/csv` (`parse_csv`/
`write_csv`), `std/toml` (`enum TomlValue`, `parse_toml`/`toml_get`/`toml_show`; subconjunto), `std/template`
(plantillas HTML con autoescape). Test `stdlib_texto_regex_csv_toml` (cli_cli). Cero cambios en los ejemplos
→ sus tests intactos.

**M40.7e — cripto + protobuf**: `std/chacha20`/`std/poly1305`/`std/protobuf` (hojas), `std/chacha20poly1305`
(AEAD `aead_seal`/`aead_open`; →`std/chacha20`+`std/poly1305`) y `std/ed25519` (firmas EdDSA;
→`std/sha512`). Test `stdlib_cripto_aead_y_protobuf` (cli_cli): AEAD seal→open roundtrip + protobuf varint.
Verificado que `chacha20poly1305_cli`/`ed25519_cli`/`jwt_eddsa_cli`/`protobuf_cli` siguen verdes (copian estas
libs a temporales; los imports namespacados resuelven embebidos).

**M40.7f — uuid**: `std/uuid` (`uuid_v4()` sobre `random_int` del runtime, `is_uuid_v4(s)` validación;
→`std/hex`). `uuid_v4` no es determinista → el test valida el **roundtrip** (`is_uuid_v4(uuid_v4())` es
determinista) + rechazo de basura + longitud 36 (`stdlib_uuid_genera_y_valida`, cli_cli). **Con esto, el
subconjunto fundacional determinista (encoding + hashing + compresión + cripto + protobuf + texto/datos +
uuid) está en `std/`: **24 módulos** (3 nativos math/text/sort + 21 promovidos).**

**Diferido (tier de red / aplicación, sigue en `examples/`).** No se promueve: `udp`/`dns`/`dns_cache`,
`http`/`http2`/`http2_client`, `websocket`/`websocket_client`, `grpc_client`, `postgres`/`redis`, `oauth2`/
`scram`/`sigv4`/`jwt`/`cookie`, `framework`/`webserver`. Dependen de sockets/TLS y no son deterministas →
son un *framework de aplicación*, no una biblioteca estándar; esperan a un paquete `net`/aplicación propio.
También quedan como diferido menor `time`/`log`/`metrics` (reloj no determinista) y el `hpack` (detalle del
stack HTTP/2, va con el tier de red).

**M40 COMPLETO.** Ergonomía de match (guardas/`if let`/patrones anidados+struct) + Iterator (perezoso,
re-fundado bajo `map`/`filter`/`fold`) + colecciones (`@derive(Hash)`, `Set`/`Deque`/`StringBuilder`) +
raydoc + `std/` embebida y auto-contenida (24 módulos). La stdlib 1.0 del arco C está cerrada; el único
diferido grande es el tier de red como paquete propio. Siguiente: **M41 — FFI**.

## 43. M41 — FFI (llamar a C)

Hasta aquí la única vía de escape de raylang era **escribir un builtin en Rust y recompilar el
compilador**. M41 abre la frontera: declarar una función C, cargarla en runtime y llamarla como una
función normal. Es lo que permite que el ecosistema crezca **sin tocar el compilador**.

### 43.1 M41.1 — el slice de primitivos

**Superficie** (decidida con el usuario): bloque **`extern "lib" { fn nombre(params) -> ret; … }`**
(estilo Rust, agrupa símbolos por librería). El nombre de cada firma es a la vez el identificador
raylang y el símbolo `dlsym`. **Sin ceremonia por llamada**: declarar la `extern fn` ES el acto
inseguro (documentado); llamarla se ve como cualquier función (no se introdujo `unsafe {}` — superficie
que no rinde en un lenguaje pedagógico). **Solo primitivos** en M41.1: `int`↔long, `float`↔double,
`bool`↔int (retorno además `unit`↔void), aridad 0..=3.

**La restricción dura: cero deps de Cargo → sin libffi.** Llamar a una función C arbitraria en runtime
pide normalmente libffi (arma el marco según la ABI para cualquier firma), que sería una dependencia de
Cargo. Sin ella, raylang soporta un **catálogo acotado de firmas**: `dlsym` da el puntero del símbolo,
y se **transmuta** a un tipo `extern "C" fn(...)` **concreto** —uno por combinación de aridad y clases
de argumento— y se llama. La clase de cada argumento debe preservarse (la ABI SysV pasa enteros y
flotantes por bancos de registros distintos), así que el molde es la tupla exacta `(clases_arg, clase_ret)`.
Es una limitación honesta y documentada; la mayoría de APIs C caen en unas pocas formas.

**Implementación.** Módulo nuevo `src/ffi.rs` (compartido por ambos motores): `dlopen`/`dlsym`
declarados a mano como `unsafe extern "C"` (patrón de `poll.rs`, cero deps), caché de handles, y `call`
con el `match` de moldes. Un nombre de librería `"m"` se resuelve al archivo de plataforma
(`libm.dylib`/`libm.so`) o, si falla, al **handle global del proceso** (`dlopen(NULL)`), donde ya viven
libc/libm enlazadas por el binario → `extern "m" { fn sqrt… }` funciona en macOS y Linux. Pipeline:
lexer (keyword `extern`), parser (`extern_block` → `Program.externs: Vec<ExternFn>`), checker (registra
cada extern en la tabla de firmas para que la llamada typee, y **valida marshalabilidad**), y los
motores despachan: el intérprete tiene una tabla `externs` y llama a `ffi::call`; la VM baja a un opcode
nuevo `CallExtern(idx, argc)` con la tabla `CompiledProgram.externs`. `fmt` reagrupa las firmas por
librería (sin pérdidas, idempotente). Cero opcodes de aritmética/GC nuevos: una extern fn es como un
builtin cuya impl vive tras `dlsym`.

**El oráculo prueba FFI.** `sqrt`/`pow`/`abs` de libm/libc son **deterministas**, así que ambos motores
llaman a la misma función C y coinciden → `ffi_libm_oraculo` (vm.rs) verifica FFI con el mismo rigor que
el resto del lenguaje (a diferencia de la I/O de red, no determinista). Tests: 3 unit (`ffi.rs`) + el
oráculo + integración por subproceso (`ffi_llama_a_libm`, cli_cli). Ejemplo `examples/ffi/libm.ray`.

**Diferido**: M41.2 `bytes`/`string` (marshalling a puntero+longitud, `char*`); M41.3 punteros opacos /
handles (APIs con estado como sqlite); callbacks (fn raylang → C), structs por valor, variádicas; más
aridad/combinaciones de molde según haga falta; namespacing de externs entre módulos.

### 43.2 M41.2 — string/bytes como argumento (`char*`)

Cierra media parte del diferido de M41.1: pasar un `string` o `bytes` de raylang a una función C que
espera un puntero. Habilita las APIs C de cadenas (`strlen`, `atoi`, `puts`, …) y de buffers.

**Marshalling.** Un argumento `string` → `char*`: `ffi::call` crea una **`CString`** (NUL-terminada) y
pasa su puntero; la `CString` se **retiene viva** (`keep`) hasta el final de la llamada. Un `bytes` →
puntero al buffer crudo (sin NUL; el usuario lo termina si hace falta, p. ej. con un literal `b"…\x00"`).
La clave de la ABI: en 64 bits un **puntero es del tamaño de un `i64` y comparte convención de llamada**
(banco de registros enteros), así que un argumento puntero se pasa por los **mismos moldes `i64`** de
M41.1 —su dirección— sin ampliar el catálogo. `CKind` gana `Str`/`Bytes` (ambos → mold entero); `FfiVal`
gana `Str(&str)`/`Bytes(&[u8])`, **prestados** del `Value`/`HeapValue` (los strings/bytes son inline en
ambos motores → cero copia salvo la `CString`). Los motores retienen los valores evaluados mientras dura
la llamada (los `FfiVal` los toman prestados).

**Alcance.** Solo **argumento**. El **retorno** `char*` queda diferido por su problema de NULL (raylang
no tiene `null`; `getenv`/`strstr` devuelven NULL) y de propiedad (¿quién libera?). El checker distingue:
un `string`/`bytes` en un parámetro es válido; en el retorno, error claro ("string/bytes de retorno →
diferido"). Un `string` con un NUL interior → error de ejecución (`CString::new` falla).

**Verificación.** Determinista (strlen/atoi) → oráculo `ffi_strings_oraculo` (string vía strlen/atoi;
bytes vía `strlen(b"…\x00")`, programas separados porque el nombre extern ES el símbolo). 2 unit más en
`ffi.rs` + integración `ffi_marshala_strings_a_char_ptr` (cli_cli). Ejemplo ampliado `examples/ffi/libm.ray`.

**Diferido**: M41.3 retorno `char*` (con una convención honesta para NULL/propiedad), punteros opacos /
handles, callbacks, structs por valor, variádicas, buffer+longitud como par.

### 43.3 M41.3 — retorno `char*` → `Option<bytes>`/`Option<string>`

Recibir un `char*` de vuelta de C. Un `char*` de retorno arrastra **tres** ambigüedades que el tipo C no
expresa, y raylang las resuelve honestamente:

1. **Puede ser NULL** (`getenv`/`strstr` devuelven NULL). raylang **no tiene `null`** → se modela con
   **`Option`**: `NULL → None`, no-NULL → `Some(...)`. Declarar el retorno como `string` pelado sería
   deshonesto (no puede representar la ausencia) → el checker lo **rechaza**; hay que declarar `Option<…>`.
2. **Propiedad indefinida** (¿prestado como `getenv`? ¿`malloc`ado como `strdup`?). La frontera **copia**
   los bytes hasta el NUL y **nunca libera** el puntero: seguro para los prestados/estáticos; un retorno
   con propiedad **fuga** (honesto — sin conocer el allocator, liberar corrompería el heap; entre fuga y
   corrupción, la fuga). El `free` allocator-aware con anotación → futuro.
3. **No es texto garantizado** (`char*` son bytes, no UTF-8). La **primitiva** es **`Option<bytes>`**
   (crudo, sin asumir codificación; el usuario convierte con `from_utf8`); **`Option<string>`** es
   **azúcar** que valida UTF-8 (bytes inválidos → error de ejecución, no corrupción silenciosa).

**Implementación.** El molde de la ABI **no cambia**: un `char*` de retorno vuelve en el registro entero,
igual que un `i64` (como M41.2 trató los punteros de entrada). Lo nuevo es la interpretación del retorno
(`int_return` en `ffi.rs`): si `ret_kind` es `OptBytes`/`OptStr`, el `i64` es un `char*` (0 = NULL →
`None`; si no, `CStr::from_ptr` copia los bytes → `Some`). `ffi::call` devuelve `FfiRet::OptBytes(Option<
Vec<u8>>)` y **cada motor construye el `Option`** del prelude: el intérprete por nombre (`EnumInstance`
`Option`/`Some`/`None`); la VM por `enum_id`+`tag` (`option_variant` los localiza en la tabla compilada)
→ `Obj::Enum`. La validación UTF-8 de `OptStr` la hace el motor (tiene la maquinaria de error). Es el
primer punto donde la frontera FFI **fabrica un `Option`** (excepción acotada y justificada: el retorno
es inherentemente especial). `ret_ckind` clasifica el tipo de retorno (primitivos + `Option<bytes>`/
`Option<string>`, en forma cruda `Struct` o resuelta `Enum`).

**El oráculo lo prueba.** `strstr` es determinista (devuelve un puntero DENTRO del argumento —vivo durante
la llamada, la `CString` está en `keep`— o NULL) → `ffi_char_ptr_return_oraculo` verifica Some/None +
el azúcar de string en ambos motores. Unit `strstr_devuelve_char_ptr_como_optbytes` (`ffi.rs`) +
integración `ffi_retorno_char_ptr_como_option` (cli_cli) + ejemplo `examples/ffi/cstrings.ray`.

**Diferido**: `free` allocator-aware (retornos con propiedad, vía anotación); punteros opacos / handles
(APIs con estado como sqlite); callbacks (fn raylang → C), structs por valor, variádicas, buffer+longitud.

### 43.4 M41.4a — anchura correcta de enteros (`int` 32 / `u64` 64) + handles opacos

**Un bug de corrección destapado por los handles.** Un `FILE*` de `fopen` se pasa bien a `fgetc`/`fclose`
(el puntero de 64 bits round-trips), pero `fgetc` entraba en **bucle infinito**: devuelve un C `int` de
**32 bits** (EOF = `-1` = `0xFFFFFFFF`), y el molde lo leía como `i64` de 64 bits → los 32 bits altos, que
la ABI deja **indefinidos** para un retorno `int`, salían en cero → `-1` se leía como `4294967295` y el
bucle `while (c >= 0)` no terminaba. `abs`/`atoi` "funcionaban" en M41.1–3 **por suerte** (el callee
extendía el signo); era UB.

**El fondo** (decidido con el usuario): en C, `int` = 32 bits pero `long`/`size_t`/puntero = 64 (LP64). Se
mezclaban en el molde `i64`. Ahora: **`int` → C `int` (32 bits, con extensión de signo al volver)** —el
caso más común (fgetc/abs/atoi)—, **`u64` → C `long`/`size_t` (64 bits)** —valores anchos y punteros
opacos tratados como entero—.

**Refactor de moldes (clave para no explotar el catálogo).** En la ABI, **los argumentos** enteros van
todos por registro entero y el callee lee la anchura que quiera de los bits bajos → un `int`(32), un
`u64`/puntero(64) y un `bool` comparten el molde de argumento `i64`. **Solo el retorno** distingue anchura
(se lee el registro con un tipo concreto). Así el `match` de `call` se parte en dos: un `match` sobre la
firma de **argumentos** (molde `I`/`F` por posición) × una **macro `dispatch!`** que transmuta con la
anchura de **retorno** correcta (`i32` signo-extendido / `i64` / `f64`). Un solo sitio por firma de args
da las tres anchuras de retorno gratis. `CKind` gana `U64`; los motores marshalan `Value::UInt(_,64)` ↔
FfiVal y devuelven `Value::UInt` para `ret_kind == U64`.

**Handles opacos, ya.** Con esto un puntero opaco (`FILE*`, `sqlite3*`) se pasa como **`u64`** de 64 bits,
correcto y con estado: `fopen -> u64`, `fgetc(s: u64) -> int`, `fclose(s: u64)`. Test por subproceso
`ffi_anchura_int_y_puntero_opaco_como_u64` (lee un archivo con `fopen`/`fgetc` hasta EOF → 3 bytes).
Los 3 oráculos previos siguen verdes (abs/atoi/strlen dan lo mismo con la anchura correcta). Diferido a
**M41.4b**: un tipo `ptr` opaco (alias con seguridad de tipos sobre el `u64` crudo) + `Option<ptr>`.

### 43.4b M41.4b — el tipo `ptr` (puntero opaco con seguridad de tipos)

Sobre los handles-como-`u64` de M41.4a, un tipo **`ptr`** dedicado: un puntero foráneo **opaco**. Ventajas
sobre el `u64` crudo: (1) **seguridad de tipos** —el checker rechaza aritmética (`ptr + 1`) e indexado
(`p[0]`) por las reglas de tipo normales (un `ptr` no es int/float ni indexable), así que no puedes
fabricar/corromper un handle por accidente—; (2) **firmas autodocumentadas** (`fopen -> Option<ptr>`);
(3) **`Option<ptr>`** para los fallibles (`fopen` devuelve NULL → `None`) — el `match` idiomático en vez
de `if h == 0`.

**Escalar nuevo, como `char` (M11.4c).** Keyword `ptr` → `Type::Ptr`; `Value::Ptr(i64)`/`HeapValue::Ptr
(i64)` (inline, **no trazado por el GC** — no apunta al heap de raylang); igualdad por **identidad** (misma
dirección); `print` → `<ptr>` (la dirección real es no determinista por ASLR → repr opaca y estable). En
la frontera: arg → su dirección por registro entero; retorno `ptr` → `Value::Ptr`; `Option<ptr>` → `None`
si NULL, `Some(Ptr)` si no (reusa el armado de `Option` de M41.3). `CKind` gana `Ptr`/`OptPtr`; `FfiRet`
gana `Ptr(i64)`/`OptPtr(Option<i64>)`.

**Inseguridad inherente (documentada).** Un `ptr` a memoria liberada es un use-after-free (raylang no
puede saberlo); es responsabilidad del usuario mantenerlo válido —parte de la frontera insegura—. Por eso
el oráculo NO puede probar `ptr` en proceso de forma determinista (un `ptr` a un `CString` temporal muere
tras la llamada); lo real es I/O con estado. Test por subproceso `ffi_ptr_opaco_y_option_ptr`
(fopen/fgetc/fclose vía `Option<ptr>`: abre y cuenta bytes, o `None` si el archivo no existe) + unit
`strstr_devuelve_option_ptr` (sin desreferenciar) + ejemplo `examples/ffi/cstrings.ray`. **M41.4 COMPLETO.**

**Diferido**: `free` allocator-aware para retornos con propiedad (vía anotación); callbacks (fn raylang →
C), structs por valor, variádicas, buffer+longitud como par de args; más combinaciones de molde según haga
falta.

**M41 COMPLETO** (FFI usable de verdad): llamar a C con primitivos (41.1), pasar `string`/`bytes` como
`char*` (41.2), recibir `char*` de vuelta con una convención honesta para NULL/encoding/propiedad (41.3),
y APIs con estado vía handles opacos —anchura de enteros correcta (`int` 32 / `u64` 64) + el tipo `ptr`—
(41.4). Lo diferido (callbacks, structs por valor, variádicas) son piezas especializadas, no imprescindibles
para la 1.0. Siguiente: **M42 — endurecimiento de seguridad**.

## 44. M42 — endurecimiento de seguridad (arco D)

El último arco antes de la 1.0. Cuatro frentes: política de overflow, cripto de producción, límites de
recursos y auditoría/fuzzing.

**Overflow de `int` (ya resuelto).** El primer ítem del plan ya estaba hecho: el desbordamiento de `int`
es **error de ejecución** (`checked_*` en ambos motores y ambos modos —debug y release—, no UB), documentado
en SPEC §8 y con el oráculo `overflow_aritmetico_oraculo`. Los `u8/u32/u64` envuelven por diseño.

**Auditoría de los `unsafe`** (arranque de M42). 12 bloques: 8 en `src/ffi.rs` (FFI) + 4 en `src/poll.rs`
(epoll/kqueue). Todos **sound**: los de FFI por el contrato de la frontera (el `transmute` confía en la
firma declarada —responsabilidad del usuario, la única zona insegura del lenguaje—; `CStr::from_ptr` solo
sobre punteros no-NULL con la `CString` origen viva; `dlopen`/`dlsym` con punteros de `CString`s vivas).
Se documentan las invariantes SAFETY del bloque `extern "C"` y del `impl Send for Handle`.

### 44.1 M42.1 — fuel (límite de instrucciones de la VM)

Para embeber raylang como **lenguaje de scripts confinado** (un nicho natural para un runtime ligero): un
bucle infinito o una entrada maliciosa **no deben colgar al anfitrión**. La VM gana un contador `fuel`
(u64) que decrece una unidad por instrucción; al llegar a 0 aborta con "límite de instrucciones agotado
(fuel)". **Coste casi nulo cuando está desactivado**: el default es `u64::MAX` (sin límite), que nunca se
agota en la práctica → un decremento + comparación por instrucción, sin ramas de "¿activado?". Es
**solo de la VM** (el motor de producto), como la concurrencia: el intérprete es el oráculo de desarrollo,
no un sandbox. API `vm::run_program_con_limite(prog, Option<u64>)`; CLI `ray run --fuel N archivo.ray`
(`--fuel` con `--interp` es error: es un límite de la VM). Tests: `fuel_limita_la_ejecucion` (unit: un
bucle infinito aborta, un programa que termina da su resultado) + `fuel_aborta_un_bucle_infinito`
(integración: exit 70). Diferido: **tope de heap** (límite de memoria, el otro recurso), cripto de
producción vía `ring`, fuzzing continuo en CI + `cargo audit`.

### 44.2 M42.2 — tope de heap (límite de memoria de la VM)

El **segundo recurso a acotar** junto al fuel: un programa que retiene objetos sin cesar no debe agotar
la memoria del anfitrión. Se mide en **objetos vivos** (`Heap.live`, la unidad que el GC ya lleva),
análogo a cómo el fuel cuenta instrucciones — no bytes, que exigiría dimensionar cada `Obj` (más
invasivo y menos honesto con un heap de handles). El `Heap` gana `max_live: usize` (default `usize::MAX`
= sin límite, coste nulo). Al acercarse al tope, `should_collect` **fuerza un GC** (además de sus
disparos normales); si tras recolectar `live` sigue por encima (`over_cap`), la VM aborta con "límite de
memoria agotado (tope de heap)". La secuencia forzar-GC-luego-comprobar es lo que hace el tope **honesto**:
solo aborta si el programa realmente *necesita* más objetos vivos de los permitidos (la basura reciclable
no cuenta). Es **solo de la VM**, como el fuel. Los dos límites se agrupan en el mismo entrypoint:
`vm::run_program_con_limite(prog, Option<u64> fuel, Option<usize> heap_cap)`; CLI `ray run --heap N`
(comparte el parser `tomar_flag_num` con `--fuel`; ambos con `--interp` son error). Tests:
`tope_de_heap_limita_los_objetos_vivos` (unit: un programa que retiene arreglos aborta, uno frugal
termina normal aun con tope bajo porque el GC recicla) + `tope_de_heap_aborta_un_programa_glotón`
(integración: exit 70). Con fuel + tope de heap, raylang embebido está **acotado en tiempo y en memoria**.
Diferido de M42: cripto de producción vía `ring` (bifurcación de diseño), fuzzing continuo + `cargo audit`.

### 44.3 M42.3 — fuzzing continuo + CI + auditoría de dependencias

**Invariante que se endurece**: entrada arbitraria → **error de usuario limpio, jamás un *panic* de Rust**.
Un panic en el pipeline lo captura la red de ICEs (`lib::with_big_stack_or_ice`, exit 101, "reporta este
bug"): correcto para una invariante rota, pero *input del usuario* nunca debe dispararlo. El fuzzer busca
esos casos.

**Fuzzer determinista sin dependencias** (`tests/fuzz_frontend.rs`): la restricción cero-deps de Cargo
descarta cargo-fuzz (exige nightly + `libfuzzer-sys`/`arbitrary` + un crate aparte). En su lugar, un fuzzer
**sembrado escrito a mano** (como el SHA-256 de M39c-2b o el JSON del LSP): PRNG **SplitMix64** propio
(dominio público, ~6 líneas), corpus = los `.ray` reales del repo (examples/selfhost/std), seis estrategias
de mutación (bytes al azar, flip/inserción/borrado de bytes, truncado, empalme de dos semillas), y por cada
entrada corre `lex → parse → check → compile → VM` dentro de `catch_unwind` (con el panic hook silenciado),
afirmando que **no panica**. La VM se acota con **fuel + tope de heap** (M42.1/42.2): un programa generado
que resulte válido pero cicle no cuelga el proceso de test — sinergia directa con las dos fases previas. Las
entradas se truncan (≤4 KiB) para **acotar la profundidad de anidación** y con ella la recursión del parser
(que recurre en la pila de Rust); además corre en el hilo de pila grande (`with_big_stack`) por margen. Es
**determinista y reproducible**: semilla base fija, cada iteración deriva la suya, y un hallazgo imprime la
semilla + escribe la entrada culpable a un archivo (`RAYLANG_FUZZ_SEED=<n>` la re-ejecuta;
`RAYLANG_FUZZ_ITERS=<n>` sube el presupuesto). Corre dentro de `cargo test` → **fuzzing continuo**: cada
corrida local y de CI fuzzea (3000 iteraciones por defecto; 200 000 en el job nocturno).

**CI** (`.github/workflows/ci.yml`, GitHub Actions, primer CI del proyecto): job `test` (clippy +
`cargo test` —que incluye el fuzzer— + `cargo test --no-default-features` para la release solo-VM + build de
release), job `audit` (`cargo audit` contra los avisos de RustSec, para vigilar la **única excepción a
cero-deps**: rustls/ring/webpki), y job `fuzz-nocturno` programado (cron 03:17 UTC, presupuesto alto). **No
hay paso de `cargo fmt --check`**: el código usa alineación manual de comentarios deliberada que rustfmt
reformatearía — el estilo del proyecto no es el de rustfmt por defecto, imponerlo sería un rediseño masivo
sin valor. Con esto se cierra el arco D (endurecimiento): overflow (resuelto), auditoría de `unsafe`, fuel,
tope de heap, fuzzing continuo + CI + `cargo audit`. Diferido único: **cripto de producción vía `ring`** —
una **bifurcación de diseño** (la cripto pura en raylang es pedagógica; migrar a `ring`, ya enlazado por
rustls, no añade deps pero cambia la naturaleza del `net`/std) que se decide con el usuario.

## 45. M43 — cripto de producción (`ring`)

**Decisión del usuario** (resuelta la bifurcación de §44.3): raylang evoluciona hacia **producción real**, así
que la criptografía que usa el código de verdad (el paquete `net`: JWT, SCRAM, SigV4, WebSocket) pasa a
apoyarse en **`ring`** —primitivas de **tiempo constante**, auditadas—. Las implementaciones en raylang puro
(`examples/web/sha256.ray`, etc.) **se conservan como DEMOSTRACIÓN DEL LENGUAJE** (prueban que raylang es lo
bastante expresivo para bit-twiddling serio), **no como un segundo backend**: una sola implementación de
producción. El motivo es de seguridad, no de corrección: las versiones puras calculan el hash correcto
(probadas contra vectores RFC), pero corriendo sobre la VM interpretada **no pueden garantizar resistencia a
canales laterales de temporización**, requisito para tocar secretos reales.

**Arquitectura**: builtins **nativos** (Rust), no módulos raylang — `ring` es Rust, así que sus primitivas
entran en la tabla `BUILTINS` (nombre + opcode + regla de tipo) con impl en ambos motores, como
`split`/`to_bytes`. Devuelven **`bytes`** (el tipo honesto de la salida cripto). `ring` ya está en el árbol
(lo enlaza rustls); declararlo dependencia **directa** resuelve a la **misma copia 0.17.x** → cero crate
nuevo, dentro de la excepción "TLS/ring" ya sancionada (§28.4). **El oráculo se mantiene**: `ring` es
determinista para digest/HMAC/Ed25519-firma(RFC 8032)/verificación/AEAD-con-nonce-dado → ambos motores llaman
al MISMO `ring` → salida idéntica. La generación de claves (aleatoria) queda fuera del oráculo.

### 45.1 M43.1 — digests SHA (`sha256`/`sha512`/`sha1`)

Los tres hashes como builtins `bytes -> bytes` (opcodes `Sha256`/`Sha512`/`Sha1`; helpers `builtins::sha256`
/`sha512`/`sha1` compartidos por ambos motores). `sha1` se expone porque `ring` lo nombra
`SHA1_FOR_LEGACY_USE_ONLY` y algún protocolo lo exige por diseño (el accept-key de WebSocket, RFC 6455); está
roto para seguridad nueva. Test `sha_digests_oraculo` con **doble red**: el oráculo (interp==vm) verifica
CONSISTENCIA, y los **vectores conocidos** (NIST/RFC: `sha256("abc")`, entrada vacía, `sha512("abc")`,
`sha1("abc")`) verifican CORRECCIÓN —el programa devuelve 1 solo si el hex casa, así un fallo de corrección da
0 aunque ambos motores coincidieran—. Estrés del GC encadenando hashes (cada uno asigna un `bytes` nuevo).
Siguen: 45.2 `hmac_sha256`, 45.3 Ed25519, 45.4 ChaCha20-Poly1305 AEAD, 45.5 migrar el `net` + des-embeber la
cripto pura de `std/`.

### 45.2 M43.2 — HMAC-SHA256

`hmac_sha256(key: bytes, msg: bytes) -> bytes` (opcode `HmacSha256`, helper `builtins::hmac_sha256` sobre
`ring::hmac`). Es la base de JWT (HS256), SigV4 y esquemas de auth. El builtin solo **produce** la etiqueta;
la verificación honesta (recomputar y comparar en tiempo constante) es de quien compara. Test
`hmac_sha256_oraculo`: oráculo (interp==vm) + vector RFC 4231 (Test Case 2) + estrés de GC encadenando HMAC.

### 45.3 M43.3 — Ed25519 (firma de curva elíptica)

`ed25519_public_key(seed) -> Option<bytes>`, `ed25519_sign(seed, msg) -> Option<bytes>`,
`ed25519_verify(pubkey, msg, sig) -> bool`. La semilla privada es de **exactamente 32 octetos**; `ring` falla
si no → los dos primeros son **fallibles**: primitivos `__ed25519_public_key`/`__ed25519_sign` que devuelven
`[bytes]` etiquetado (`[]`/`[valor]`) y el prelude los envuelve en `Option<bytes>` (el patrón M11.2, "errores
como valores" — una semilla de mal tamaño es un dato inválido, no un ICE). `verify` es **total** (nunca falla;
`false` ante clave/firma inválida) → builtin `bool` directo. Ed25519 es **determinista** (RFC 8032: el nonce se
deriva por hash) → misma entrada, misma firma → el oráculo vale. Test `ed25519_oraculo`: oráculo (interp==vm) +
validación **relacional** con `ring` como impl de confianza (firma verifica, mensaje alterado no, semilla corta
→ `None`, firmar dos veces da lo mismo) → el programa devuelve 1 solo si TODO cuadra.

### 45.4 M43.4 — ChaCha20-Poly1305 AEAD

Cifrado autenticado (usado en TLS 1.3, WireGuard, age): `chacha20poly1305_seal(key, nonce, aad, plaintext) ->
Option<bytes>` (devuelve `texto_cifrado || etiqueta(16)`) y `chacha20poly1305_open(key, nonce, aad,
ciphertext_and_tag) -> Option<bytes>`. Clave de 32 octetos, nonce de 12; ambas `Option` (primitivos `[bytes]`
etiquetado + envoltorio prelude): `seal` da `None` si los tamaños no cuadran, `open` da `None` si la
**autenticación falla** (dato manipulado) o los tamaños no cuadran. Se usa `LessSafeKey` de `ring` porque el
nonce lo aporta quien llama (la API "segura" gestiona el nonce por secuencia; este primitivo es de más bajo
nivel). Test `chacha20poly1305_oraculo`: oráculo (interp==vm) + relacional (seal→open recupera, alterar el aad
→ `None`, clave corta → `None`). Con esto están **todas las primitivas cripto de producción**; queda 45.5:
migrar el `net` a ellas + des-embeber la cripto pura de `std/`.

### 45.5 M43.5 — migrar el `net` a la cripto de producción + des-embeber la pura

El paso que **cierra M43** y, a diferencia de 45.1–45.4 (aditivos), toca código existente. Se parte en dos.

**Hallazgo que dirige el diseño**: los builtins **ganan** a una función de usuario del mismo nombre (una `fn
hmac_sha256` de usuario se ignora; la llamada va al builtin). Así que un envoltorio no puede reusar el nombre
de un builtin (quedaría muerto). Además, los consumidores del `net` son `[int]`-céntricos (`base64url([int])`,
HMAC pura → `[int]`), mientras los builtins son `bytes`-céntricos.

**M43.5a — `net/crypto` + migración de consumidores**: un módulo `packages/net/crypto.ray` (adaptadores
finos sobre los builtins) presenta la API en `[int]`/hex que el `net` ya consumía, con **nombres distintos**
de los builtins para los que colisionan (`hmac_sha256_octets`, `sha1_octets`, `ed_sign`/`ed_verify`/
`ed_public_key`) y los de conveniencia que no colisionan tal cual (`sha256_octets`, `sha256_hex`,
`hmac_sha256_hex`). Un puente `octetos(bytes) -> [int]` cruza los dos mundos. Los 6 consumidores
(`jwt`/`scram`/`sigv4`/`jwt_eddsa`/`websocket`/`websocket_client`) cambian su `from std/… import` por `from
net/crypto import` y sus sitios de llamada. Ed25519: `ed_sign` NO recibe la clave pública (a diferencia del
pedagógico de 3 args; `ring` la deriva). **Verificado**: los 4 tests de integración del `net` pasan — jwt HS256
firma+verifica y el accept-key de WebSocket dan sus KATs de RFC, ahora con cripto de tiempo constante.
**M43.5b — des-embeber la cripto pura de `std/`**: se quitan `sha1`/`sha256`/`sha512`/`hmac`/`chacha20`/
`poly1305`/`chacha20poly1305`/`ed25519` de la tabla `MODULOS` de `src/stdlib.rs` → dejan de ser módulos
`std/` importables y vuelven a ser **solo ejemplos** en `examples/web/`. Ningún otro `std/` embebido dependía
de ellos (uuid → hex, no cripto), así que el des-embeber es local. **Ripple resuelto**: (1) los dos tests
M40.7 que probaban la std cripto embebida (`stdlib_hashing`, la parte AEAD de `stdlib_cripto`) se **convierten
a probar los builtins de producción** vía CLI (más valioso: chequeo a nivel CLI de la cripto real); (2) los
ejemplos cripto se cruzaban con `from std/X` (namespacado en M40.7 para resolver contra la std embebida) → se
**revierten a `from X` local** (sha256/sha512/chacha20/poly1305) para que corran standalone contra sus
hermanos en `examples/web/` (los demos `*_demo.ray` ya usaban imports locales; `from std/hex` se conserva, hex
sigue embebido). Verificado: los tests de demos (`chacha20poly1305_cli`/`ed25519_cli`/`chacha20_cli`) + la
suite completa siguen verdes. **M43.5 y M43 COMPLETOS**: la cripto de producción (`ring`) es la que usa el
código real; la pura es demostración del lenguaje.


## 46. M38 — Paralelismo M:N con aislamiento por actores (arco B) — DISEÑO

**Estado: diseño (aún sin implementar).** Esta sección fija la arquitectura y los riesgos ANTES de tocar
código, por ser la fase de más riesgo técnico del plan (PRODUCCION §Arco B). La implementación va después,
en sub-fases medidas, con la suite verde en cada paso.

### 46.1 El problema y por qué M38 cierra también M37

Dos brechas de producción convergen en una sola solución:
- **Multicore**: hoy la VM corre en **un** hilo (el scheduler M:1 de M12 reparte fibras cooperativas sobre
  un solo núcleo). En una máquina de 8/16 núcleos, 7/15 están ociosos. Es la brecha de rendimiento más
  profunda.
- **Pausas del GC** (M37): medido en §27.5, el GC stop-the-world da pausas de ~10 ms sobre un heap grande,
  10× por encima del objetivo <1 ms, y §27.3 (M37.2) probó que **ningún tweak barato lo acota** — la pausa
  la domina el recorrido O(heap).

La misma decisión resuelve las dos: **un heap (y un GC) por actor**. Cada actor tiene un heap **pequeño e
independiente**; recolectarlo es una pausa **corta por construcción** (acota M37 sin marcado incremental ni
*write barrier* —la pieza de más riesgo—), y como los heaps son independientes, **cada actor puede correr en
un hilo distinto** (multicore). CSP ya eligió este camino en M12; M38 lo lleva a su conclusión.

### 46.2 El modelo: aislamiento por actores (heap-por-fibra + move-on-send)

**Invariante central**: un objeto del heap del actor A **nunca** contiene un handle al heap del actor B. No
hay memoria compartida mutable entre actores → **data-race freedom por construcción** (no por *ownership* en
el sistema de tipos, como Rust; ni por un GC global concurrente). La única comunicación entre actores es el
**canal**, y `send` **transfiere la propiedad** del valor (lo saca del heap del emisor y lo mete en el del
receptor). Es el modelo de Erlang/Pony-lite.

**Transferencia en `send`** (`v` viaja del heap del emisor al del receptor):
- **Move** (barato) si `v` es de **propiedad única** en el emisor: nada más en el heap del emisor lo
  alcanza. Se **re-aloja** el subgrafo de `v` (sus objetos + handles) en el heap destino y se borra del
  origen. Coste O(tamaño de `v`), sin copia de datos primitivos.
- **Deep-copy** (fallback) si `v` está **aliaseado** (algo más en el emisor lo referencia): copiar
  moviéndolo rompería ese alias. Se copia profundamente el subgrafo al destino y el original se queda.
  Coste O(tamaño de `v`), correcto siempre.
- **Detección**: tras `send`, ¿queda `v` alcanzable desde las raíces del emisor **excluyendo** el propio
  `v`? Es una consulta de alcanzabilidad acotada al subgrafo (o, más simple y conservador al principio:
  **siempre deep-copy**, y optimizar a move cuando la unicidad se pueda probar barato — medir cuál paga).

**El canal, ¿de quién es el heap?** Un canal lo comparten emisor y receptor → no puede vivir en el heap de
un actor. Pasa a ser una **estructura del host compartida** (`Arc<Mutex<…>>` en Rust, fuera del GC de los
actores), con la cola de valores **en tránsito** (que no pertenecen a ningún actor entre `send` y `recv`).
Los valores en tránsito se **serializan** a una forma sin handles (o con handles a un mini-heap del canal) al
entrar y se **re-alojan** en el heap del receptor al salir. Es el único punto de sincronización real.

### 46.3 El scheduler M:N

- **N hilos del SO** (≈ núcleos), **M fibras** repartidas. Cada hilo tiene su cola de fibras listas
  (*run queue*) + **robo de trabajo** (*work-stealing*) cuando queda ocioso.
- Cada fibra lleva **su heap** con ella; corre en el hilo que la tenga. El GC de una fibra solo mira **sus**
  raíces (su pila + marcos) → sin "stop-the-world" global, sin sincronizar hilos para recolectar.
- **Bloqueo/despertar entre hilos**: `recv`/`send`/`join`/`select` que bloquean aparcan la fibra; el poller
  de E/S (M17, `kqueue`/`epoll`) y los canales despiertan fibras que pueden acabar en **otro** hilo →
  hace falta sincronización en las colas (mutex/lock-free), no en los heaps.

### 46.4 Determinismo como modo (`--deterministic`)

El scheduler M:1 determinista de hoy (orden FIFO reproducible) es lo que hace testeable la concurrencia (los
tests de M12 comparan contra salida exacta). Se **conserva** como **`--deterministic`**: un solo hilo,
round-robin, orden fijo → misma salida siempre, para tests y para el oráculo. El **multicore es el default**;
lo determinista es opt-in. (El oráculo VM↔intérprete sigue siendo secuencial: la concurrencia solo vive en la
VM, como desde M12.)

### 46.5 Sub-fases (implementación medida, tras este diseño)

- **M38.1 — heap-por-fibra** (single-thread todavía): cada `Fiber` gana su propio `Heap`; el GC recolecta
  solo el heap de la fibra en curso. Esto **ya acota las pausas** (heaps pequeños) → **cierra el objetivo de
  M37**, medible con el benchmark de §27.5. Es el paso que desacopla el heap del scheduler; el más invasivo
  del runtime pero sin hilos aún (riesgo acotado). Se implementa en incrementos:
  - **M38.1a HECHO**: `transfer_value` (subgrafo entre heaps, cycle/sharing-safe), probada en aislamiento.
  - **M38.1b-1 HECHO**: **canales y tareas al almacén del host** — dejan de ser `Obj::Channel`/`Obj::Task`
    en el GC y pasan a `Vm.channels`/`Vm.tasks` (`Vec`), referenciados por `HeapValue::Channel(id)`/`Task(id)`
    (distintos de `Obj`, para que con heaps por fibra un handle de canal no sea ambiguo de qué heap). El GC
    ya no traza canales/tasks; en su lugar `collect` rootea directamente los valores **en tránsito** de las
    colas y los de `Done` de las tareas. **Behavior-preserving**: heap único aún, toda la batería de
    concurrencia de M12 (23 tests) pasa idéntica. Prerrequisito para el split (un handle de canal ya no
    vive en el heap de objetos).
  - **M38.1b-2 HECHO**: **heap de objetos por fibra**. `Fiber` gana `heap` (`Vm.heap` = el de la fibra en
    curso; save/restore en `take_current_fiber`/`schedule_next`); cada `VmChannel`/`VmTask` gana su propio
    heap para los valores **en tránsito**/de `Done` (que no pertenecen a ninguna fibra). `transfer_value` se
    cablea en TODOS los cruces: capturas de `spawn` (spawner→hija), `send`→heap del canal, heap del canal→
    `recv`, emisor bloqueado→receptor, `Done`→heap de la tarea→`join`, y `wake_recv`/`finish_parked_write`
    (alojan en el heap de la fibra que despiertan). El heap del canal se **limpia al vaciarse la cola**.
    **`collect` se simplifica**: recolecta SOLO el heap de la fibra en curso, cuyas únicas raíces son sus
    marcos/pila (invariante de aislamiento) → **la pausa la acota el heap de una fibra**, no el total.
    Gotcha resuelto: el arreglo de canales de un `select` aparcado vive en el heap de LA FIBRA APARCADA
    (no en el de la que dispara el wake). Verificado: 442 lib (single-fiber idéntico) + 23 concurrency_cli.
    **Consecuencia semántica (data-race freedom por construcción)**: `spawn` deep-copia las capturas → **NO
    hay estado mutable compartido entre fibras**. El slice CSP de M12 compartía el heap (dos fibras podían
    mutar el mismo objeto capturado); M38 lo elimina — es la garantía del modelo de actores, pero un cambio
    de comportamiento. Lo destapó el webserver con métricas compartidas (`metrics_server_demo.ray`): su
    `Registry` compartido entre los handlers dejó de acumular. Se **migró al patrón de actores** (un actor
    de métricas que posee el `Registry` y recibe updates por un canal; `GET /metrics` le pide el render por
    un canal de respuesta) → cuentas correctas por el orden FIFO. Los **canales SÍ se comparten** (su id es
    un valor primitivo que se copia tal cual al transferir) → son el medio de comunicación entre actores.
    **Cierra M37**: pausa máxima del GC 10,5 ms → 0,12 ms (§27.5), <1 ms por construcción.
- **M38.2 — move/copy-on-send DESCARTADO por medición** (el deep-copy-siempre de M38.1 se conserva). Se midió
  (`benchmarks/send_heavy.ray` + variantes, `RAYLANG_XFER_STATS`, instrumentación temporal en `transfer_value`)
  el coste real de la transferencia deep-copy en tres escenarios: **(a) mensajes pequeños** (int, la carga
  típica de concurrencia): la transferencia es **~5% del tiempo y casi todo instrumentación** —un escalar no
  tiene objeto que mover, nada que optimizar—; **(b) valores grandes frescos/únicos** (donde el move sería
  seguro): la transferencia es **~10%**, la **construcción domina ~90%** (move no ayuda a construir); **(c)
  valores grandes aliaseados** (donde la transferencia SÍ domina, ~89%): el move es **inseguro** (el emisor
  aún lo referencia) → obligatorio deep-copy. Además, mover un subgrafo entre dos `Heap` separados (Vec+
  handles) es **igual de O(tamaño)** para estructuras con handles anidados (hay que remapear); solo un
  arreglo/string/bytes plano logra move O(1). Y la **detección de unicidad** (sin refcounts ni ownership
  types) exige un *scan* de alcanzabilidad O(heap vivo) por `send` que **penalizaría el caso común**. Por la
  disciplina de §27 (conservar solo lo que supera el ruido ~3-5% en cargas realistas), no paga. El canal ya
  quedó como estructura del host en M38.1b-1. **Cierra M38.**
- **M38.3 — pool de hilos M:N**: repartir fibras sobre N hilos + work-stealing; sincronizar las colas
  (no los heaps). Aquí llega el multicore real; medir *speedup* con una carga paralela nueva en el banco.
  - **M38.3a HECHO** (prep, single-thread aún): se agrupa el estado del scheduler que N hilos compartirían
    —`ready`/`parked`/`io_parked`/`channels`/`tasks`, ~83 sitios de acceso— en un `struct Shared` propiedad
    directa del `Vm` (sin lock todavía). Con los heaps aislados (M38.1), es lo ÚNICO compartido; la
    ejecución de cada fibra (frames/stack/heap/scopes/current_task/fuel) es thread-local. Behavior-preserving
    → `Arc<Mutex<Shared>>` queda a un wrap de distancia. **Riesgo cualitativo de M38.3b** (el pool de hilos):
    fallos de concurrencia = heisenbugs (deadlocks/races no deterministas); exige stress testing dedicado y
    resolver la integración del poller de M17 (`kqueue`/`epoll`, hoy single-thread) en M:N.
  - **M38.3b HECHO** (`8492a27`+`f28e4be`+`f689a74`): el **pool de hilos M:N**, en tres pasos. **Paso 2**:
    `Vm.shared` tras `Arc<Mutex<Shared>>` (single-thread a través del lock; un lock por handler para no
    reentrar el Mutex —que no es reentrante—; gotcha: `select`/`close` retenían un guard y re-bloqueaban →
    deadlock). **Paso 3a** (infra): `run()` orquesta N hilos worker (`thread::scope`, pila 256 MiB) sobre una
    cola `ready` compartida; cada worker con su `cur`/heap thread-local (el GC no sincroniza). `ProgRef`
    (wrapper `unsafe impl Send/Sync`) comparte el `&CompiledProgram` inmutable (constantes `Value`/`Rc` sólo
    leídas). **No se usó Condvar sino busy-poll** (`poll_next` reemplaza a `schedule_next`): más simple y a
    prueba de lost-wakeups; un worker ocioso espera con `SPIN_SLEEP_US` mientras `Shared.running>0` y sólo
    declara deadlock si `running==0`. El contador `running` (workers ejecutando una fibra) es la clave del
    deadlock M:N. Correcciones destapadas para M:N REAL: **TOCTOU** (Spawn/ChannelNew alojaban id con dos
    locks) y **lost-wakeup** (TaskJoin/ScopeEnd/Select comprobaban estado y aparcaban en locks separados) →
    un ÚNICO guard sostenido a través de comprobar+aparcar. **Default N=1** (determinista, idéntico a hoy;
    el suite de M12 lo exige); `RAYLANG_THREADS=N` habilita M:N (opt-in por ahora; M38.4 invertirá el
    default). **Paso 3b** (verificación): *speedup* medido en `benchmarks/parallel.ray` (4 tareas CPU-bound):
    N=1 16,1 s → N=4 **4,2 s (3,84×, casi lineal)** en un M3 Pro; N=8 no mejora (sólo 4 tareas). Tests
    `tests/multicore_cli.rs` (resultados independientes del scheduling). Bug real cazado: un worker ocioso
    declaraba deadlock con posición `(0,0)` → el localizador de errores underflowaba (`gline - start_line`,
    entry `start_line=1`) → `saturating_sub` (fix general de un bug latente pre-existente). Integración del
    poller: funciona (el worker con `running==0` hace `io_wait` bajo el lock; el corpus de M12 no usa red).
- **M38.4 — `--deterministic` HECHO** (`233532f`): **invierte el default a multicore**; lo determinista es
  opt-in. `num_workers(program)` decide en orden: (1) `--deterministic` (bandera global `AtomicBool` que fija
  la CLI) → 1 hilo (M:1 reproducible, FIFO); (2) `RAYLANG_THREADS=N` → N; (3) el programa **no usa `spawn`**
  → 1 (sólo hay `main`; el multicore no aporta y así el oráculo y la mayoría de programas no pagan el coste de
  lanzar hilos — cero regresión); (4) concurrente sin override → `available_parallelism()`. La CLI acepta
  `--deterministic` (order-independent) en `ray run` y en la interfaz legada `--vm`. Tests: `concurrency_cli`
  (salida FIFO exacta) usa `--deterministic`; los servidores (webserver/websocket/métricas/net) corren en el
  DEFAULT multicore y pasan (salida request-response determinista). Medido: `benchmarks/parallel.ray` corre
  multicore por defecto (4,4 s) vs `--deterministic` (16,0 s, serie); un programa sin `spawn` en N=1 sin
  overhead. Con M38.2 descartado por medición (ver abajo), **M38 (M:N por actores) COMPLETO** — y con él el
  grueso del arco B (M36 opt VM + M37 pausas GC cerradas vía heap-por-fibra + M38 multicore por actores).

### 46.6 Riesgos y mitigaciones

- **Re-alojar subgrafos entre heaps** (el corazón de `send`) es código nuevo delicado: recorrer el grafo,
  copiar/mover objetos, remapear handles. Mitigación: empezar por **deep-copy siempre** (un solo recorrido,
  sin la lógica de unicidad), validado por un oráculo de igualdad estructural antes/después.
- **La invariante "sin handles cruzados"** es la que garantiza la seguridad; un solo cruce = corrupción o
  data race. Mitigación: un **modo de verificación** (como el estrés del GC) que, en cada `send`, comprueba
  que el subgrafo transferido no deja ni toma handles cruzados.
- **Sincronización del scheduler** entre hilos (colas, parking, work-stealing) es la fuente clásica de bugs
  de concurrencia del propio runtime (no del programa raylang). Mitigación: **M38.3 va al final**; hasta
  entonces todo es single-thread y determinista, y el modo `--deterministic` da un oráculo reproducible.
- **Riesgo sobre un runtime impecable** (441 lib + 40 binarios verdes): cada sub-fase mantiene la suite
  verde; M38.1 (heap-por-fibra) es puramente interno (mismos resultados, distinta gestión de memoria) → el
  oráculo VM↔intérprete y los tests de M12 lo blindan.

**Regla de oro del arco B** (§27): cada sub-fase se **mide** (pausas del GC en M38.1; *speedup* multicore en
M38.3) y se conserva solo si los datos la respaldan. **Nota de invariante cero-deps**: el pool de hilos usa
`std::thread` (sin `rayon`/`tokio`); la sincronización, `std::sync` (`Arc`/`Mutex`/`Condvar`) — todo de la
librería estándar, sin dependencias de Cargo nuevas.

## 47. M45 — Completion de miembros en el LSP

Cierra el diferido de M10.2e/f (el completion era "de archivo", sin `recv.`). Tras `.`, el LSP ofrece
los **miembros del tipo del receptor**: campos del struct, métodos de trait/impl (incl. `@derive`),
builtins invocables como método (`s.len()`, `xs.push(...)`), y funciones UFCS del usuario/prelude
(`xs.map(f)`, `xs.sort()`). Cliente-LSP + una consulta al checker; cero runtime.

**El reto** es que el completion ocurre sobre código **incompleto** (`recv.` no parsea). En vez de
inferir el tipo textualmente (como el signature help), se **repara** la fuente insertando un
**centinela** en lugar de la palabra-miembro: `recv.par|` → `recv.__raycomplete__;`. Eso es sintaxis
válida, sobrevive a la recuperación de errores del parser (M33c, `parse_all`), y el checker recorre el
**mismo camino de tipado del receptor que ya usa el hover de campos** (M10.2g). El `;` termina la
sentencia para que el bloque parsee (si no, `parse_all` descartaría la función al resincronizar); se
omite si sigue un `(` (edición de una llamada `recv.m(args)`, ya válida).

**Consulta al checker** (`checker::member_completion`, hermana de `semantic_index`): corre el front-end
best-effort con un flag `completing`; al tipar el acceso `recv.__raycomplete__`, en vez de dar error
por miembro inexistente, enumera en `enumerate_members(tipo_receptor)`:
1. **Campos** del struct (con su tipo sustituido como *detail*).
2. **Métodos** de trait/impl del tipo concreto (tabla `methods`, por constructor → `Caja<int>`/`Caja<bool>`
   comparten).
3. **Builtins** de la categoría del tipo (`builtins::methods_for`: string/bytes/char/array/map/…), lista
   curada porque son ad-hoc polimórficos.
4. **UFCS libre** — funciones cuyo primer parámetro acepta el receptor (unificación), **solo para
   receptores compuestos** (array/map/struct/enum/tupla): ahí `recv.f()` es idiomático (captura
   `map`/`filter`/`fold`/`sort` y las UFCS del usuario). Para **primitivos NO**: una función que toma un
   `string` suele tratarlo como DATO (`read_file(path)`, `env(name)`), no como método → sería ruido; los
   primitivos ya reciben builtins (3) y métodos de trait (2). Se excluyen sintéticos (`#`/`::`/`__`) y el
   primer parámetro genérico pelado (`Var`, que unificaría con todo, p. ej. `assert_eq`).

Cada ítem lleva su `CompletionItemKind` (Field/Method/Function) y documentación (`builtins::doc` / los
`///` del prelude). `.` se añade a los `triggerCharacters`. **Diferidos**: docs `///` de métodos de impl
del usuario, receptores que son expresiones complejas (`f(x).`), y UFCS del usuario sobre primitivos.

### 47.1 M45b — refinamientos del completion de miembros

Sobre M45, cuatro mejoras a partir de casos reales:
- **Contexto de expresión** (bug): el reparado añadía `;` siempre, rompiendo `sum(x.)` →
  `sum(x.__raycomplete__;)` (inválido → `parse_all` descartaba la función). Ahora el `;` solo se
  añade en **posición de sentencia**; si el siguiente carácter no-espacio es `)`/`]`/`}`/`,`/`(` es
  posición de **expresión** y se omite. Cubre argumentos de llamada, elementos de arreglo e
  interpolación. (Los receptores-expresión `f(x).` ya funcionaban al ser posición de sentencia.)
- **Snippet de argumentos**: los miembros invocables (método/función) insertan `nombre($0)` como
  *snippet* (cursor entre paréntesis) y disparan el **signature help** (`triggerParameterHints`) si
  toman argumentos; sin argumentos, `nombre()`. La aridad sale del `FnSig` (métodos/UFCS) o de
  `builtins::method_takes_args` (los builtins ad-hoc, con lista curada de los sin-args). Los campos
  no reciben `()`.
- **Docs `///` de métodos de impl/UFCS**: `MemberItem` lleva la posición de declaración (`def`) del
  método/función destino (poblada con `gather`); el LSP resuelve sus `///` con `raydoc::
  doc_lineas_arriba` sobre la fuente original (el reparado no cambia números de línea). Prioridad:
  builtin → `///` de la def → prelude.
- **Interpolación** `"…${x.}…"`: el LSP ya la maneja (el centinela cae dentro de la interpolación,
  que se re-lexea como expresión). El bloqueo era de VSCode, que suprime sugerencias dentro de
  strings; se resuelve con un `configurationDefaults` por lenguaje (`editor.quickSuggestions.strings
  = true` para `[raylang]`) en la extensión.

Sigue diferido: hover/def de métodos sobre el nombre (comparte `(línea,col)` con el receptor, sin
spans) y el completion por ámbito de bloque (sin spans, el alcance es la función).

### 47.2 M45c — completion en los `import`

Extiende el completion a las líneas de `import` (antes caían al completion de archivo, inútil ahí).
Detección de contexto textual (el import a medio escribir no parsea), como el `.` de M45.

- **M45c-1 — símbolos de `from M import …`**: al detectar `[pub] from <ruta> import <cursor>`, se
  resuelve `<ruta>` desde disco (`loader::resolve_module_path` sobre las raíces del proyecto), se
  parsea y se ofrecen sus nombres **`pub`** (funciones/tipos/consts + re-exports `pub from`), con su
  kind. Lo privado no aparece.
- **M45c-2 — rutas de módulo** (`import <cursor>`, `from <cursor> import`): `loader::
  modulos_disponibles` recorre las raíces recolectando la identidad de cada `.ray`
  (`rel_module_name`) y **descarta las que cruzan el borde de una cápsula** desde el archivo actual
  (`capsula_violada`) — solo se ofrece lo que el checker aceptaría (un `util/interno` de la cápsula
  `util` no se ve desde fuera). Como las rutas llevan `/` (que VSCode no cuenta como carácter de
  palabra), cada ítem usa un **`textEdit`** que cubre la ruta parcial entera → el fuzzy match del
  editor funciona sobre `geo/for` y al aceptar reemplaza la ruta completa.

Las raíces (proyecto con `main.ray` ancestro + caché `.ray-deps`) las dan `project_root_for`/
`dep_roots_for`, reusadas de los diagnósticos modulares. Cliente-LSP + consultas al loader; cero
runtime.

- **M45c-3 — acceso calificado por el leaf/alias** (`import geo/util as u;` → `u.` , o `circulo.`
  del leaf): al pedir miembros con `recv.`, si el receptor no tipa como valor (un módulo no es un
  valor → sin miembros) **y** es el `leaf()` de algún `import` del archivo, se ofrecen los símbolos
  `pub` de ese módulo (reusa `simbolos_pub_de_modulo`). Va **después** del intento de valor, así un
  local que tape al módulo (el resolutor prefiere el local) gana. Cierra el `u.` que no autocompletaba.

Diferido: nombre calificado en expresiones más allá del leaf (`M.Color.Rojo` en dos saltos).

## 48. M46 — firmas visibles al completar

Ataca la fricción #1 del completion: aceptar una función sin ver qué parámetros toma. Idea central:
un **resolutor de firma unificado** (`SigCtx`, cliente-LSP) que halla la declaración `fn` de un
nombre —**textualmente**, tolerando el archivo a medio escribir— buscando en el buffer + los módulos
importados (leídos de disco) + el prelude, con `builtins::signature` para los ad-hoc. Reusa
`find_fn_signature` (la misma del signature help).

### 48.1 M46a — firma en el detalle del popup

Cada ítem invocable (función de archivo, método, builtin, UFCS, símbolo de módulo) lleva
`labelDetails` (`detail` = params inline tras el label, `description` = retorno a la derecha) + el
clásico `detail` (panel): `doblar(p: P, k: int)  int`. Así **ves los tipos en la lista** antes de
aceptar. En contexto de **método** (`recv.f`) se recorta el primer parámetro (el receptor) → se
muestran solo los argumentos que faltan (`s.split` → `(sep: string)`). `builtins::signature` se
extendió a los builtins-método (string/array/map), con test-guardián. Bonus: la completion de archivo
pasó a `parse_all` (recuperación) → ya no se queda **vacía** en archivos incompletos (el caso normal
al teclear). Queda **M46c** (snippet con placeholders por parámetro).

### 48.2 M46b — signature help cross-módulo y de métodos

El signature help (`f(` → firma con el parámetro activo) usaba `find_fn_signature(buffer)`: solo
funciones del archivo + builtins. Ahora usa el **`SigCtx`** de M46a → resuelve también funciones
**importadas** (`u.cuadrado(`) y del **prelude** (`sort(`). Además distingue **método** de **llamada
calificada de módulo**: `enclosing_call` devuelve el receptor; si es un **valor** (`p.doblar(`) se
recorta el receptor y el `activeParameter` cuenta los args visibles (`(k: int)`); si es un **módulo**
importado (`u.cuadrado(`, detectado con `es_modulo_importado`) se muestra la firma completa. Reusa
todo M46a; cero runtime.

### 48.3 M46c — snippet con placeholders por parámetro

Al aceptar una función/método, en vez de `nombre($0)` (cursor entre paréntesis vacíos) se inserta un
**snippet con un placeholder por parámetro** —`nombre(${1:p}, ${2:k})`—: caes en el primer argumento
y recorres los demás con Tab, cada uno con el nombre del parámetro como texto seleccionado. Reusa la
firma de `SigCtx` (`snippet_args` toma el nombre de cada `"p: T"`); en un **método** se omite el
receptor (`s.split` → `split(${1:sep})`); sin firma pero con args se cae a `nombre($0)`, y sin args a
`nombre()`. Cubre tanto el completion de miembros como el de archivo (que además ganó `insertText`,
que antes no tenía). **M46 COMPLETO** (48.1 detalle + 48.2 signature help + 48.3 placeholders).

## 49. M47 — completion de literales de struct

- **M47a — campos dentro del literal**: dentro de `Nombre { … | … }` (posición de nombre de campo) se
  ofrecen los **campos** del struct que faltan (kind Field, tipo como detalle, insertText `campo: `),
  en vez de los símbolos del archivo. `struct_literal_completion_items` detecta el contexto (escanea
  al `{` sin cerrar; el identificador previo es el struct), lo busca en el cierre de imports
  (`SigCtx::struct_campos` → archivo/importado/reexportado, `geo.Circulo { }`), y guarda contra
  falsos positivos de bloque (`-> T {`, `for T {`, `struct/enum/trait T {`). Excluye los campos ya
  escritos; en posición de VALOR (`campo: …`) cede a la completion normal.
- **M47b — snippet del literal al teclear el tipo**: por cada struct ofrecible, un ítem EXTRA
  `Nombre {…}` (kind Snippet) que inserta el literal completo con un placeholder por campo (`Nombre {
  c1: ${1:T1}, … }`), al estilo rust-analyzer. Aparte del tipo pelado `Nombre` (que sigue para las
  anotaciones); `filterText` = el nombre, así aparece al teclear el tipo. Cliente-LSP; cero runtime.

## 50. M48 — ergonomía de nombres y stdlib

Diagnóstico: raylang ya tiene varios espacios de nombres (tipos, rutas de módulo `::`, métodos de trait
`Tipo#metodo`); el saturado es el de **valores** (funciones libres + locales + builtins). Plan en tres
fases (ver `docs/M48-ergonomia-nombres.md`), **las tres completas**: (1) funciones asociadas + literal de
Map; (2) diagnóstico al redefinir un builtin; (3) builtins de contenedor → traits **+ retiro** (§50.5).

### 50.1 M48.1 — funciones asociadas a tipos (`Tipo.fn()`)

Un namespace **indexado por el tipo**, estilo `Vec::new()`. Sustituye los constructores poco idiomáticos
`map_new()`/`channel()` por **`Map.new()`**, **`Channel.new()`**, **`Channel.bounded(n)`**.

- **Registro** `ASSOC_FNS` en `src/builtins.rs` (`type_name`, `fn_name`, `arity`, `opcode`, `doc`, `sig`),
  consultado por checker/compilador/intérprete/LSP. `assoc_lookup`/`assoc_for_type`.
- **Sintaxis**: llega como `Call(Field(Ident(Tipo), fn))` —igual que la construcción de enum—; el checker
  lo reconoce en `try_assoc_call` **antes** de la resolución campo/método/UFCS. El resultado (Map/Channel)
  es **indeterminado**: lo fija el tipo esperado (bidireccional, como `[]`/`None`); sin él, error de
  "anota el tipo". Valida aridad y el arg `int` de `bounded`.
- **Runtime intacto**: se baja al mismo opcode (`MapNew`/`ChannelNew`/`ChannelNewBounded`); el intérprete
  construye el Map vacío (`Map.new()`) o da el error "requiere la VM" (canales).
- **Migración de golpe** (decisión con el usuario): `map_new`/`channel` **retirados** como builtins; todo
  el corpus (156 ejemplos + `std`/`packages` + `selfhost`) migrado. `map_new()` ahora es "función no
  declarada". El compilador **auto-alojado** se actualizó en paralelo (checker/interpreter/compiler) para
  reconocer `Map.new()` y preservar la meta-circularidad; el primitivo interno `"map_new"` sobrevive solo
  en `dispatch_builtin` (la VM auto-alojada baja `Map.new()` a `OBuiltin("map_new", 0)`).
- **LSP**: completado `Map.`/`Channel.` (kind Function, snippet + firma), hover del nombre asociado (su
  firma, vía `try_assoc_call` bajo `gather`), y signature help dentro de `Map.new(`/`Channel.bounded(`.
- Diferido: funciones asociadas **definidas por el usuario** (`impl Tipo { fn new() {…} }` sin `self`).

### 50.2 M48.2 — literal de Map (`[:]` / `[k: v]`)

Sintaxis idiomática para construir mapas, estilo Swift, en vez de `Map.new()` + `insert` manual.

- **Sintaxis**: `[k: v, …]` (poblado) y `[:]` (vacío). Nodo `ExprKind::MapLit(Vec<(Expr, Expr)>)`. El
  parser extiende el literal de corchetes: `[:]` es el Map vacío; el `:` tras el **primer** elemento
  decide Map vs arreglo (`[a, b]` arreglo, `[a: b]` Map); coma final permitida. No choca con `{}`
  (bloque/struct).
- **Tipado**: `[k: v]` infiere `Map<K,V>` del primer par (claves homogéneas, valores homogéneos, clave
  hashable). `[:]` es **indeterminado** (como `[]`): lo fija el esperado, o error de "anota el tipo".
- **Runtime**: baja a `Map.new()` + `insert` por par (erasure). Como `MapInsert` consume el handle del
  Map y no hay `Dup`, el compilador guarda el Map en un local temporal (`$maplit`, como el escrutinio del
  `match`) y lo recupera al final; el intérprete lo construye directo. Clave repetida → gana la última.
  Oráculo VM↔intérprete + estrés de GC.
- **Front-end**: `MapLit` se añadió a las ~14 pasadas de lowering/traversal (loader + checker) para que
  el lowering (`?`/UFCS/dyn/dicts) alcance las claves y valores; fmt lo reemite (`[:]` / `[k: v]`).
- Diferido: el literal en el compilador **auto-alojado** (parser/checker/intérprete/VM); el ejemplo
  `examples/data/mapa_literal.ray` se excluye del escaneo de `selfhost_parser`.

### 50.3 M48.3 — redefinir un builtin es error (footgun)

Un builtin (`len`/`push`/`insert`/`print`/…) se resuelve **antes** que cualquier función del usuario
(`un builtin no se tapa`), así que un `fn len` quedaba **inalcanzable** — un shadowing silencioso al
revés. Ahora es un error claro: *"'len' es un builtin del lenguaje y no puede redefinirse"*.

- **`check_builtin_redefinition`** corre **antes** de inyectar el prelude (ve solo las funciones del
  usuario ya fusionadas por el loader), llamado por `check` (fail-fast) **y** por `check_all`
  (recuperación M33c; sin ambos, el mensaje no se emitía y el CLI salía 65 sin diagnóstico).
- Solo nombres **pelados**: las de un módulo van namespacadas (`M::len`, con `::`) → no colisionan; las
  del prelude (`map`/`filter`/`fold`/`sort`/`assert`…) **no son builtins** → siguen siendo redefinibles
  (override); los internos `__x` se ignoran. La stdlib con nombres de builtin (`std/text.reverse`,
  `std/sort.min`/`max`, `redis.read_line`) solo se importa (namespacada) → no la afecta.
- El override real de un builtin llegará gratis con la Fase 3 (cuando `len` deje de ser builtin y pase a
  ser un método de trait, redefinir `fn len` como función libre será legal).
- Puro checker (sin runtime). Diagnóstico en vivo en el LSP (vía `analizar`).

### 50.4 M48.4 — builtins de contenedor → traits

Los builtins de contenedor pueden ser **métodos de trait**: misma sintaxis con punto (`xs.len()`),
pero **extensibles a tipos propios** e **usables en bounds** (`fn f<T: Len>(x: T)`). Runtime intacto.

**Maquinaria (M48.4a)** — prerrequisito reutilizable: `impl Trait for X` y el despacho de métodos ahora
aceptan los tipos incorporados `[T]`/`Map<K,V>`/`bytes` como objetivos (antes solo primitivos +
struct/enum). `ensure_impl_target` valida `[T]`/`Map<K,V>` como constructores siempre-genéricos (como
`Caja<T>`; solo impls plenamente genéricos) y `bytes` como concreto; `type_key_of` da las claves de
despacho `[]`/`Map`/`bytes`.

**Traits (prelude)** — cada método del trait baja a un **primitivo `__x`** (mismo opcode que el builtin,
oculto), o —para `StrOps`/`BytesOps` durante la coexistencia— llama al builtin público:
- `Len { len }` → string, `[T]`, `Map<K,V>`, bytes (M48.4a).
- `Push<T> { push }` / `Reverse { reverse -> Self }` / `Contains<T> { contains }` → `[T]` (Contains
  también string); bytes fuera de Contains (M48.4b).
- `MapOps<K,V> { insert; contains_key; keys; values }` → `Map<K,V>` (M48.4c). `get`/`remove` siguen
  siendo funciones del prelude (Option).
- `StrOps { trim; split; replace; chars; starts_with; ends_with; to_upper; to_lower; substring; repeat;
  to_bytes }` → string; `BytesOps { sub_bytes }` → bytes (M48.4d). `char_code` (char) y `join`
  (`[string]`, no impl-able para un array concreto) se quedan builtins.

**Coexistencia (M48.4a–d)** — estado intermedio: los builtins públicos convivían con los traits;
`recv.metodo()` resolvía por el trait (prioridad campo→método→UFCS), la forma prefija `metodo(args)` por
el builtin. Sirvió para migrar el corpus sin romper nada; lo cierra el retiro (M48.4e).

### 50.5 M48.4e — retiro de los builtins de contenedor

Vaciar el namespace **de verdad**: los 20 builtins de contenedor (`len`/`push`/`reverse`/`contains`/
`insert`/`contains_key`/`keys`/`values`, `trim`/`split`/`replace`/`chars`/`starts_with`/`ends_with`/
`to_upper`/`to_lower`/`substring`/`repeat`/`to_bytes`/`sub_bytes`) dejan de ser builtins y quedan **solo**
como métodos de trait. Así `fn len` libre pasa a ser legal (el footgun de §50.3 ya no dispara sobre estos
nombres) y la forma prefija `len(x)` **desaparece** (sola forma canónica: `x.len()`). Es un cambio
**incompatible** acotado (la forma de método existe desde M48.4a).

- **Prerrequisito — `ray fmt` sano + corpus canónico.** El retiro reescribe cada sitio prefijo a
  `.metodo()` sobre el **AST** y reemite con el formateador; para que el diff toque **solo** los sitios
  migrados (sin ruido de estilo) el corpus debía estar ya en forma canónica. Al canonizarlo se
  descubrieron y repararon **dos bugs de `fmt`**: (1) un `match`/bloque como **sub-expresión** (argumento
  de llamada, operando…) se des-indentaba desde la columna 0 (`fmt_expr` no llevaba la indentación del
  contexto → ahora `Cur.base`); (2) —**semántico**— un block-form como **última sentencia sin tail** se
  reemitía sin `;`, y al re-parsear un block-form final sin `;` es el **tail** → el bloque pasaba de
  `unit` al tipo del block-form (rompía el compilador auto-alojado). `fmt_block` conserva el `;` en ese
  caso.
- **El reescritor (codemod AST).** `Call(Ident(builtin), [recv, ...resto])` → `Call(Field(recv, builtin),
  [...resto])`, en post-orden (los anidados componen: `reverse(sort(a))` → `sort(a).reverse()`). Seguro
  porque se verificó **0** ocurrencias en el azúcar (pipes/interpolación) y **0** shadowing por
  locales/params del mismo nombre. Migró 2115 sitios del corpus (137 archivos) + 51 del prelude + ~247
  **fixtures de test embebidas en Rust** (a mano, mismo criterio). Los cuerpos de los impl de trait siguen
  llamando a los primitivos `__x` (evitan la recursión infinita).
- **El retiro.** Se quitan las 20 entradas públicas de `BUILTINS` (los gemelos `__x`, mismo opcode,
  quedan como impl → **runtime intacto**). Las tablas del LSP (`methods_for`/`signature`/`doc`) se
  conservan (ahora describen métodos de trait). Gramáticas VSCode/Sublime podadas.
- **Self-hosting (D5).** El checker auto-alojado es un **subconjunto** que sigue modelando estos como
  builtins de arreglo/string; para los 3 tests de error sobre tipo incorrecto (`xs.push(true)`,
  `(3).push(1)`, `(3).len()`) el oráculo exige misma **posición** de rechazo, tolerando la redacción (Rust
  los ve como métodos de trait, p. ej. `argumento 2 de '[]#push': …`). El resto (lexer/parser/checker/
  intérprete/VM/metacircular) sigue byte-idéntico; sus fuentes usan la forma de método (resuelta por su
  propia rama UFCS→builtin). **M48.4e / M48 COMPLETOS.**

## 51. M49 — stdlib importable (familias de builtins → módulos `std/…`)

Continuación de M48 (descongestionar el namespace de **valores**). Igual que M48 movió los builtins de
contenedor a métodos de trait, M49 mueve las familias **matemática / tiempo / criptografía** del global a
módulos importables `std/…`, dejando globales solo lo universal (`print`/`panic`/`assert`) y **core la
concurrencia** (atada al modelo de ejecución). Plan completo en `docs/M49-stdlib-importable.md`.

**Cero maquinaria nueva** — dos piezas probadas se combinan: (1) la **std embebida en el binario** (M40.5,
`src/stdlib.rs` + `include_str!`: `import std/math;` resuelve a la fuente embebida) y (2) el patrón **`__x`
interno + envoltorio `pub fn`** (el mismo de la I/O: `read_file`/`__read_file`).

### 51.1 M49.1a — `std/math`, funciones float

Las 11 funciones float (`sqrt sin cos tan ln log10 exp floor ceil round` + `pow`) dejan de ser builtins
globales y pasan a `std/math`: cada builtin se **renombra** a su primitivo interno `__x` (mismo opcode
`MathF(...)`/`Pow`; la VM despacha por opcode → intacta; el intérprete renombra su arm por nombre) y
`std/math.ray` lo expone con `pub fn sqrt(x: float) -> float { __sqrt(x) }`. Uso: `import std/math;
math.sqrt(2.0)`. La forma prefija global `sqrt(x)` **desaparece** (error "función no declarada"). Las
polimórficas `abs`/`min`/`max` y las constantes `pi()`/`e()` siguen globales (→ M49.1b).

**Verificación**: el **oráculo** VM↔intérprete prueba los **primitivos `__x`** directamente (el que computa;
sigue siendo builtin, no necesita el loader) y un test de integración (`tests/math_cli.rs`) cierra el
**envoltorio** end-to-end (`import std/math; math.sqrt(…)` compila y corre igual en ambos motores). El
migrado del corpus fue mínimo (1 ejemplo, `matematicas.ray`; `libm.ray`/tests-FFI usan `extern fn sqrt`,
no el builtin → intactos). LSP: las tablas `signature()`/`doc()` conservan las entradas (ahora sirven al
signature-help de `math.X`); dos tests de hover pasaron a un builtin conservado (`abs`). **M49.1a COMPLETO.**

### 51.2 M49.1b — `abs`/`min`/`max`/`pi`/`e` a `std/math` (puros en raylang)

Cierra `std/math`. Las polimórficas y las constantes dejan de ser builtins y pasan a `std/math` como
**raylang puro** (sin opcode): `min`/`max` genéricos sobre el trait **`Ord`** (`fn min<T: Ord>(a: T, b: T)
-> T { if (a.less(b)) { a } else { b } }` → sirve int/float/string/char); `abs` sobre un trait nuevo
**`Signed { fn abs(self) -> Self; }`** con `impl` para int/float (cuerpos puros) y `fn abs<T: Signed>(x:
T) -> T { x.abs() }`; `pi`/`e` como **funciones nularias** (`math.pi()`). **Se podan los opcodes**
`Abs`/`Min`/`Max`/`Pi`/`E` (+ sus arms en VM/intérprete y las reglas `numeric_*_check`) → el runtime
adelgaza. Un tipo de usuario que `impl Signed`/`Ord` funciona con `math.abs`/`math.min` (extensibilidad,
como los contenedores de M48.4).

Inicialmente `pi`/`e` quedaron como **funciones** (`math.pi()`) porque el acceso calificado a un `const`
de módulo no existía; **M49.1c** lo habilitó → ahora son `const` (`math.PI`/`math.E`).

**Verificación**: el oráculo de math (`matematicas_oraculo`) se reduce a los primitivos `__x` (abs/min/max
ya no son builtins → no hay opcode que oraculizar); `matematicas.ray` + `tests/math_cli.rs` cubren
`math.abs`/`min`/`max`/`PI` end-to-end en ambos motores (int y float, y `min` sobre string). LSP: los tests
de hover/completion de builtin pasaron a `char_code` (builtin estable). Suite completa verde.

### 51.3 M49.1c — acceso calificado a `const` de módulo (`M.CONST`)

Habilita `math.PI`/`math.E` (y cualquier `pub const` de un módulo). Los `const` de un módulo pasan a
**namespacarse como las funciones** (`modulo::CONST`) en vez de fusionarse globales por su nombre bare:
tres toques mínimos en el loader (reusando la maquinaria de funciones) — (1) el `Resolver.own` incluye los
`const` propios (una referencia interna `PI` → `modulo::PI`), (2) las **defs** de `const` de un módulo
no-entrada se renombran a `modulo::CONST` al fusionar, y (3) `build_surfaces` mete los `pub const` en la
**cara de valores** (`Surface.values`) → `qualified_field` resuelve `M.CONST` como una función pub. El
checker y los motores ya resolvían los `const` **por nombre** (`consts: HashMap<String, _>`), así que el
nombre namespacado funciona sin más. **Consecuencia (mejora de encapsulación)**: importar un módulo ya
**no filtra** sus `const` al ámbito global —solo son accesibles calificados (`M.CONST`)— y un `const`
no-`pub` no es accesible. Test `const_calificado_de_modulo` (acceso + encapsulación + no-pub) en
`tests/modules_cli.rs`. **M49.1c / M49.1 (`std/math`) COMPLETO.** Suite completa verde (77 binarios, 510 lib).

### 51.4 M49.2a — `std/random` (RNG)

`random`/`random_int` dejan de ser builtins globales y pasan a `std/random`: `import std/random;
random.next()` (float en `[0,1)`) y `random.below(n)` (int en `[0,n)`; nombres en inglés, evitando las
palabras reservadas `float`/`int`). Mismo patrón `__x`+envoltorio de M49.1a (renombra `random`→`__random`,
`random_int`→`__random_int`; VM por opcode intacta). **No deterministas** → sin oráculo; los cubre
`tests/time_random_cli.rs` por subproceso en ambos motores (rango + variedad). Migrado: `reloj_aleatorio`
(demo), `websocket_client` (examples + packages) y `std/uuid` (embebido → `import std/random;`, un módulo
embebido importando otro). `now`/`monotonic`/`sleep` siguen globales (→ M49.2b). **M49.2a COMPLETO.**

### 51.5 M49.2b — `std/time` (reloj)

`now`/`monotonic`/`sleep` dejan de ser builtins globales y pasan a `std/time`: `import std/time;
time.now()` (epoch ms UTC), `time.monotonic()` (reloj monótono para intervalos), `time.sleep(ms)`. Mismo
patrón `__x`+envoltorio (renombra a `__now`/`__monotonic`/`__sleep`; VM por opcode intacta). No
deterministas → sin oráculo; los cubre `tests/time_random_cli.rs` por subproceso. Migrado: dns_cache
(examples+packages), webserver_demo, reloj_aleatorio, y la **librería de fechas** `time.ray` (examples+
packages) + su demo `time_demo.ray` (usan `time.now()` para `now_utc`). **M49.2b / M49.2 COMPLETO.**

> **Nota de naming**: la librería de fechas se llama módulo `time` (`examples/web/time.ray`, no embebida)
> y el reloj es `std/time` (embebido). Ambos usan el leaf `time`, así que un archivo que importe los dos
> necesitaría `as` (colisión de leaf). No colisionan hoy (la de fechas se usa vía `from time import …`).
> Un futuro rename (`std/datetime`) lo limpiaría; fuera de alcance de M49.2.

### 51.6 M49.3 — `std/crypto` (criptografía de producción)

La cripto de producción (builtins de `ring`, M43) pasa a `std/crypto`: hashes/MAC `crypto.sha256`/
`sha512`/`sha1`/`hmac_sha256` (bytes→bytes), firma `crypto.ed25519_verify`/`ed25519_public_key`/
`ed25519_sign` y AEAD `crypto.chacha20poly1305_seal`/`open` (los fallibles → `Option<bytes>`). Los 5
builtins directos (sha*/hmac/ed25519_verify) se renombran a `__x`; los 4 envoltorios `Option` (ed25519
key/sign, chacha seal/open) **se mueven del prelude** a `std/crypto.ray` (llaman a los primitivos `__x`).

**Decisiones (afinan el plan)**:
- **`bytes_of` NO se mueve** — es un constructor de `bytes` desde `[int]` (como `b"…"` en runtime), no
  cripto; se usa en ~46 sitios (websocket/deflate/…); se queda **builtin**.
- **Colisión de nombres resuelta sola**: las impls **pedagógicas** en raylang puro (`sha256.ray`/`sha1.ray`/
  `hmac.ray`/…) definen su propio `sha256`/`sha1`/`hmac_sha256` y las usa el stack web (jwt/scram/sigv4/
  websocket vía `from … import`); NO tocan los builtins. Solo `packages/net/crypto.ray` (el wrapper de
  producción) y unas fixtures llamaban al builtin global → migración **dirigida por qué falla al compilar**.

**Verificación**: el oráculo VM↔intérprete prueba los **primitivos `__x`** (sha/hmac directos; ed25519/chacha
vía el arreglo etiquetado `[bytes]` en vez del `Option`); los envoltorios `crypto.*` los cubren
`cli_cli` (crypto.sha256/hmac/chacha end-to-end) y las suites de M20. **M49.3 / M49 COMPLETO.**

## 52. M50 — cerrar la descongestión del namespace (`std/fs`/`std/collections`/`std/net`)

Continúa M48/M49: mueve del **prelude global** (auto-inyectado) a **módulos `std/` opt-in** los tres
grupos grandes que aún ensucian el namespace de valores — archivos, colecciones y red —. Mismo mecanismo
que M49 (`__x`+envoltorio; migración dirigida por errores). Se **quedan globales** los esenciales
(`Option`/`Result`+`?`, `map`/`filter`/`fold`, `print`/`eprint`/`panic`/`assert`/`assert_eq`, `to_string`,
`close`, `input`/`read_int`/`env`).

### 52.1 M50.1 — `std/fs` (sistema de archivos)

Todo lo que toca disco → `fs.X` con `import std/fs;` (*capability hint* suave: importar `std/fs` señala
"este archivo toca el sistema de archivos"). Los **10 envoltorios** del prelude (`read_file`/`write_file`/
`read_file_bytes`/`write_file_bytes`/`append_file`/`remove_file`/`list_dir`/`open`/`read_line`/`write`) se
**cortan del prelude** a `std/fs.ray` (llaman a los primitivos `__x`, que devuelven el arreglo etiquetado
`["ok",…]`/`["err",msg]`). El builtin **`exists`** se renombra a **`__exists`** (Rust `builtins.rs` +
`interpreter.rs`; la VM no cambia, despacha por opcode `Exists`) y `std/fs` añade el envoltorio total
`fs.exists`. `std/fs` se registra en `stdlib::MODULOS` (embebido con `include_str!`).

**El self-hosting usa los primitivos `__x`** (no `import std/fs;`): su loader (M14.7) lee de **disco** y no
conoce la `std/` embebida, así que los drivers/loader del compilador auto-alojado (`lex_dump`/`parse_dump`/
`check_dump`/`loader`) llevan un **wrapper local** `fn read_file` sobre `__read_file`, y el intérprete +
checker auto-alojados usan `__exists` (patrón D5: el self-hosted trata los builtins como builtins). El
oráculo VM↔intérprete es **pre-loader** → sus fixtures fs también usan los primitivos `__x` directamente.

**Verificación**: no determinista (disco) → integración por subproceso (`io_cli`/`bytes_io_cli`, con
`import std/fs; fs.X`, ambos motores); el borrado/lectura de inexistente sí es determinista → oráculo con
`__read_file`. Corpus migrado (dirigido por errores): `examples/io/archivos.ray`/`binario.ray`,
`examples/web/deflate_demo.ray`/`wss_echo.ray`. Self-hosting revalidado (checker/interpreter/vm oráculos +
metacircular). **M50.1 COMPLETO.**

### 52.2 M50.2 — `std/collections/{set,deque,stringbuilder}` (colecciones)

Las tres estructuras de datos **puras en raylang** (sin primitivos `__x`) del prelude (`Set<T>` hash set,
`Deque<T>` cola doble, `StringBuilder`) se **cortan del prelude** a **submódulos** bajo `std/collections/`.
Con leaf-binding (M11.5) cada submódulo se usa por su hoja y **cae el prefijo** que dentro de un solo
módulo hacía falta (`set_`/`deque_`/`sb_`): `import std/collections/set;` → `set.new()`/`set.add(s, x)`/…;
`import std/collections/deque;` → `deque.push_back(d, x)`/…; `import std/collections/stringbuilder;` (o
`as sb`) → `sb.push(b, s)`/… Los tipos se namespacan al submódulo (`set.Set`/`deque.Deque`/
`stringbuilder.StringBuilder`); en el ejemplo se usan calificados en posición de tipo (`set.Set<int>`).
Mecanismo: tres filas en `stdlib::MODULOS` con match exacto por nombre anidado + el leaf-binding de
directorios ya probado. **Cero maquinaria nueva.** Los helpers internos del set (`bucket`/`en_bucket`) se
ocultan como no-`pub`. `Hash`/`Eq`/`Ord`/`join`/`pop` siguen globales (prelude), así que los submódulos no
importan nada.

**Verificación**: son deterministas, pero el uso pasa por el **loader** (resuelve el import) → el oráculo
de `vm.rs` (pre-loader) ya no aplica; se sustituye por un **oráculo por subproceso** (`collections_cli`)
que corre los ejemplos migrados (`conjunto.ray`, `builder_deque.ray`) por **ambos motores** y exige que
coincidan + la salida esperada, más un test de que las formas globales (`set_new`/…) ya no existen. Los
dos oráculos in-process de `vm.rs` (que usaban los nombres globales) se retiran. El self-hosting no usa
colecciones (su corpus no las incluye) → sin impacto; el oráculo del parser auto-alojado revalida que los
ejemplos migrados (con `import std/collections/…`) parsean idénticos. **M50.2 COMPLETO.**

### 52.3 M50.3 — `std/net` (transporte de red)

Los **10 envoltorios** de red del prelude —I/O binaria de socket (`socket_read_bytes`/`socket_write_bytes`),
cliente TCP (`tcp_connect`), TLS (`tls_connect`/`tls_connect_h2`/`tls_accept`), I/O de socket
(`socket_read`/`socket_write`) y servidor TCP (`tcp_listen`/`tcp_accept`)— se **cortan del prelude** a
`std/net` (un solo módulo; se conserva la distinción en los nombres: `net.tcp_connect`/`net.tls_connect`/…).
El builtin `local_port` se renombra a `__local_port` (la VM no cambia, opcode `LocalPort`) y `std/net` añade
el envoltorio `net.local_port`. Los primitivos `__tcp_connect`/`__socket_read`/… siguen builtins; `close`
(cerrar socket **o** handle de archivo) sigue global. **UDP no entra**: sus envoltorios ya vivían en el
módulo `net/udp` del paquete `net` (sobre `__udp_*`), así que nunca ensuciaron el namespace global.

**Migración**: es el grupo mayor (~21 archivos del stack web: `examples/net/*`, `examples/web/{http,
webserver,websocket_*,grpc_client,postgres,redis,http2_client,…}` y `packages/net/*`). Sin llamadas por
UFCS (todas planas) → codemod: prefijar las llamadas con `net.` (lookbehind que respeta `__x` y `net.` ya
puesto) + insertar `import std/net;`. **Ningún módulo embebido usa red** (los `std/*` promovidos son
hex/json/deflate/… sin sockets) → sin "embebido-importa-embebido"; y el paquete `net` importa por ruta
(`net/hpack`, `from net/crypto …`), nunca bare `import net;`, así que el leaf `net` de `std/net` no colisiona.

**Verificación**: no determinista (red) → integración por subproceso (`net_cli`/`socket_write_cli`/
`concurrency_net_cli`/`webserver_cli`/`bytes_io_cli` con fixtures migradas `import std/net; net.X`, más las
suites que corren los archivos migrados: `http`/`redis`/`postgres`/`grpc`/`http2`/`h2_alpn`/`tls`/
`websocket`/`dns`/`oauth2`/`scram`/`sigv4`/…). **M50.3 COMPLETO. M50 COMPLETO** (fs + collections + net):
el namespace global queda con los esenciales (`Option`/`Result`+`?`, `map`/`filter`/`fold`, `print`/
`eprint`/`panic`/`assert`/`assert_eq`, `to_string`, `close`, `input`/`read_int`/`env`) + los primitivos `__x`.

## 53. Política de tiers de librerías (gobernanza del ecosistema)

Toda capacidad de raylang que no sea del **núcleo del lenguaje** (sintaxis, checker, motores) vive en uno
de **tres tiers**. Esta sección fija *dónde* va cada cosa y *cómo se promueve* — la regla era implícita
(se aplicó a lo largo de M40/M49/M50); aquí queda explícita para que futuras decisiones sean consistentes.

### 53.1 Los tres tiers

1. **stdlib embebida (`std/`)** — módulos compilados en el binario base (`include_str!` en `src/stdlib.rs`,
   tabla `MODULES`). Cero instalación: `import std/math;` siempre funciona. Es la promesa de "baterías
   incluidas" y su API se versiona **con el lenguaje** (compromiso de estabilidad). Coste: cada módulo
   embebido es peso en **todos** los binarios, aunque no se use.

2. **paquetes adicionales (`packages/<pkg>/`, y externos)** — **no** embebidos. Se traen por `ray.toml`
   (dep por `path:`/git; §41.7–41.8, y por nombre desde el índice cuando exista §54). Se apoyan en `std/`
   para lo fundacional. Cada paquete versiona su API con su **propio** semver, independiente del lenguaje.
   Hoy existe `packages/net` (red y protocolos). Ejemplo de uso idéntico al de `std/`: `import net/http;`.

3. **`examples/`** — implementaciones de referencia y demos (`*_demo.ray`). **Material pedagógico**, no
   pensado para importarse desde un proyecto real. Muchos son el espejo didáctico de un módulo ya promovido
   a los tiers 1/2 (p. ej. `examples/stdlib/regex.ray` refleja `std/regex`).

### 53.2 Criterios de colocación (en orden de prioridad)

1. **Universalidad** — ¿lo necesitaría la mayoría de programas? Sí → candidato a `std/`. Nicho → paquete.
2. **Peso e independencia** — ¿sería peso muerto en el binario de quien no lo use, o depende de
   sockets/TLS/servicios externos? → paquete. (Es la razón de que todo el stack de red viva en `packages/net`
   y no en el binario base.)
3. **Estabilidad de API** — `std/` implica estabilidad atada al versionado del lenguaje (§39). Algo que
   aún itera su superficie → paquete primero (su semver absorbe los cambios sin tocar el del lenguaje).
4. **Superficie de seguridad** — cripto que toca **secretos reales** va en un paquete respaldado por `ring`
   (tiempo constante, auditado: `net/crypto`), **no** en la impl pura embebida; la versión en raylang puro
   se queda como **demo** del lenguaje (correcta, pero sobre la VM interpretada no garantiza resistencia a
   canales laterales). Regla ya vigente (M43; ver `packages/net/README.md`).
5. **Determinismo/pureza** — no decide el tier, pero orienta el *testing*: un módulo puro se verifica con el
   **oráculo** (ambos motores); uno con I/O no determinista, por **subproceso** (`*_cli`).

### 53.3 El ciclo de vida de un addon (pipeline de promoción)

`examples/` (prototipo/demo) → si madura y pasa los criterios → **`std/`** (universal, ligero, estable) o
**`packages/<pkg>/`** (nicho, pesado, o de API propia). La promoción a paquete reescribe los `import` del
prototipo a `std/` (lo fundacional embebido) y añade sus tests distribuibles; la demo original **se conserva**
en `examples/` como material del lenguaje. Un módulo puede además **degradarse** (deprecación → salir de `std/`
a un paquete, o retirarse), lo que en `std/` exige un cambio de versión mayor del lenguaje.

### 53.4 Clasificación actual (snapshot)

- **`std/` (embebida):** `math`, `text`, `fs`, `sort`, `random`, `time`, `collections/{set,deque,stringbuilder}`,
  `crypto`, `hex`, `url`, `uuid`, `json`, `csv`, `toml`, `template`, `regex`, `deflate`/`inflate`/`huffman`,
  `protobuf`, `net` (transporte básico sobre los builtins de socket).
- **`packages/net` (adicional):** `http`/`http2`/`webserver`, `websocket`(+cliente), `dns`/`dns_cache`, `udp`,
  `redis`, `postgres`, `grpc_client`, `oauth2`, `jwt`/`jwt_eddsa`, `sigv4`, `scram`, `cookie`, `hpack`, `metrics`,
  `log`, `time`, `crypto` (respaldo `ring`).
- **Solo `examples/` (aún sin promover):** `framework` (mini-framework web) — candidato natural a un futuro
  paquete `web` (o a `packages/net`); los servidores de eco (`websocket_echo`, `wss_echo`) **se quedan** como
  demos (son aplicaciones, no librerías). Las impls de cripto puro (`sha*`, `hmac`, `ed25519`, `chacha20`,
  `poly1305`, `base64`) permanecen como demos: su versión de producción es `net/crypto`.

## 54. M51 — Registro central de paquetes y `ray publish` (arco C, DISEÑO)

El gestor de paquetes (M39c) resuelve dependencias por **git** (`git+URL@ref`) y por **ruta** (`path:dir`):
para instalar hay que conocer y escribir la URL exacta. Falta la última pieza de "ecosistema": **instalar por
nombre** (`ray add foo`) contra un **índice** compartido, y **publicar** (`ray publish`) para poblarlo. Es la
brecha nº1 que `PRODUCCION.md` (Parte I §2) marca como "flexible en el lenguaje, ❌ en el ecosistema".

### 54.1 Decisión central: un índice **respaldado por git**, sin servidor propio

Coherente con la filosofía del proyecto (cero dependencias de Cargo salvo TLS; *shell out* a `git`; tests
deterministas *offline*): el "registro" **no** es un servicio web que operemos, sino un **repositorio git**
que mapea `nombre → metadatos` (URL del código + versiones publicadas + hash de contenido). Es el modelo del
*crates.io-index* (un repo git) o del enfoque proxy-less de Go, recortado. Ventajas:

- **Reusa toda la maquinaria existente**: el índice solo aporta el `nombre → (git URL, hash)`; la descarga,
  la cápsula (M39c-1), el lock/verificación (M39c-2b), el BFS transitivo y el MVS (M39c-3) son los mismos.
- **Sin infraestructura que mantener** (ni cuentas, ni base de datos, ni *uptime*): alojar el índice es
  *hostear un repo git*. Cualquiera puede tener el suyo; no baked-in un índice por defecto en v1.
- **Confianza = git + hash**: la integridad la sigue dando `ray.lock` (SHA-256 por versión, M39c-2b);
  el índice añade solo el **descubrimiento por nombre**. Sin tokens en v1: publicar = *commitear* al repo
  del índice (quien controle el repo controla el namespace; anti-squatting es política del repo, no del CLI).

Alternativa descartada: **índice hospedado** (servidor + API + auth). Contradice de raíz el "sin servidor";
además su valor (búsqueda web, cuentas, firmas) es ortogonal y se puede añadir después sobre el mismo índice git.

### 54.2 Formato del índice y del `ray.toml` por nombre

- **Índice** = repo git con **un archivo por paquete** (`index/<nombre>.toml`; el subconjunto de TOML de
  `manifest.rs`). Cada archivo lista versiones **inmutables** y **ordenadas**:
  ```toml
  [[version]]
  num  = "1.2.0"
  git  = "git+https://ejemplo/foo@v1.2.0"   # de dónde sale el código de ESA versión
  hash = "<sha256 del contenido>"            # el mismo hash Merkle de M39c-2b
  yanked = false                              # M51c: retirada sin borrar (como crates.io)
  ```
  Publicar **añade** una entrada (append-only); una versión ya publicada **no se sobrescribe** (build
  reproducible). El hash permite verificar sin volver a clonar.
- **`ray.toml` por nombre**: `foo = "1.2.0"` (una cadena de versión **sin** prefijo `git+`/`path:`) significa
  "resuélvelo por el índice". El resolutor busca `foo` en el índice, elige la versión (semver), saca su
  `git`+`hash` y **delega en la descarga git+lock existente**. Convive con `git+`/`path:` (que siguen para
  deps no publicadas / desarrollo local).

### 54.3 Prerrequisito: rangos semver de verdad

El índice hace que un nombre mapee a **muchas** versiones → hay que **elegir**. Hoy las specs son refs
**exactas** (M39c dejó "rangos de verdad" como diferido). M51 los necesita: `foo = "^1.2"` (compatible),
`"~1.2.3"`, `"1.2.0"` (exacta). Se extiende `semver`/`mvs` (§41.6) para casar un **requisito** contra las
versiones del índice y elegir la mayor compatible (MVS: la mínima que satisface, subiendo en conflicto).

### 54.4 Subcomandos nuevos

- **`ray add <nombre>[@<req>]`** — resuelve `<req>` (o la última) en el índice, **escribe** la dep en
  `ray.toml` y hace `fetch`. El azúcar de instalación que hoy falta.
- **`ray publish`** — desde un proyecto: **valida** (tiene `name`/`version` en su `ray.toml`, es importable
  —`mod.ray`/raíz—, todos los `.ray` lexean+parsean, y **supera el check semántico completo** (M51e):
  se resuelven sus dependencias y la cara se carga+chequea con el checker sin exigir `main`) y calcula el
  **hash de contenido** (`deps::hash_package`,
  reusado de M39c-2b) — todo **sobre un clon limpio del tag publicado** (M51d), no sobre el working tree—,
  y **genera la entrada de versión** para el índice. Publicar de verdad = *commit* +
  *push* al repo del índice (acción del autor, con sus credenciales git); el CLI produce/aplica el commit
  sobre un clon del índice. Rechaza sobrescribir una versión existente.
- **`ray update`** (M51c) — recomputa el lock a las versiones más nuevas que aún satisfagan los requisitos.
- **`ray yank <nombre>@<ver>`** (M51c) — marca `yanked = true` (un commit al índice); la resolución **salta**
  las yanked salvo que el `ray.lock` ya las hubiera fijado (no rompe builds existentes).
- **`ray remove <nombre>`** (M51f) — la inversa de `add`: quita la dep de `ray.toml`, re-resuelve (el lock
  se reescribe sin ella) y borra `.ray-deps/<nombre>` solo si el lock ya no la lista (podría seguir siendo
  transitiva de otra dep).
- **`ray search [patrón]`** (M51f) — lista los paquetes del índice (un `<nombre>.toml` por paquete) cuyo
  nombre contenga el patrón, con su versión instalable más alta (final, no retirada — como `ray add`).

### 54.5 Fases

- **M51a — leer del índice + `ray add` + rangos semver. ✅ COMPLETO.** `src/index.rs`: `VersionReq`
  (exacta `1.2.0`/`=`, caret `^`, tilde `~`, `*`; regla de cargo para caret con `0.x`) + lector del
  índice (`<index>/<nombre>.toml`, secciones `[<versión>]` con `git`/`hash`/`yanked`, mismo subconjunto
  TOML que `manifest.rs`) + `resolve`/`latest` (elige la **más alta no retirada** que casa). El índice
  se localiza por `RAY_INDEX` o `[registry] index` del `ray.toml` (`Manifest.registry_index`; un índice
  **remoto git** aún no, avisa → M51c). `deps::ensure` resuelve las specs por nombre vía el índice
  (`to_gitspec`, tanto directas como transitivas) y **delega en la descarga git + lock existente** —el
  lock guarda la `git URL@ref` resuelta, sin cambios de formato—. `ray add <nombre>[@<req>]`
  (`manifest::upsert_dependency`, edición mínima del `ray.toml`): sin versión escribe `^<latest>`, con
  versión respeta el requisito; valida contra el índice **antes** de tocar el manifiesto. Tests: unit
  (`index.rs`: parseo/casado de reqs, lectura/resolución; `manifest.rs`: upsert) + integración
  **offline** (`tests/registry_cli.rs`: índice local + repos git `file://`; dep por nombre, `ray add`
  con/sin versión, caret elige la mayor, paquete inexistente, falta de índice). **Cero runtime.**
  Limitación (→ M51c): re-resuelve del índice en cada `ensure` (no fija la versión resuelta en el lock),
  así un caret podría subir si el índice gana una versión; con índice fijo (tests) es determinista.
- **M51b — `ray publish`. ✅ COMPLETO.** `ray publish [--repo <git+URL@ref>]`: valida (name+version semver;
  la cara del paquete —`mod.ray` en la raíz, o la entrada— lexea+parsea), calcula el **hash de contenido**
  (`deps::hash_package`) y **añade** la entrada de versión al índice vía `index::append_version`
  (append-only, **inmutable**: rechaza sobrescribir una versión ya publicada). La spec git de dónde vive
  el código: `--repo`, o **derivada** del remoto `origin` del repo + el tag `v<version>` (que debe existir
  → se publica un punto fijo). El `ray publish` **no** hace commit/push del repo del índice —eso es acción
  del autor—. Probado *offline* con un bare repo local + `origin` (`tests/registry_cli.rs`): publicar,
  inmutabilidad al republicar, un consumidor que resuelve por nombre lo publicado, y el error claro si
  falta el tag.
- **M51c — índice remoto + mantenimiento. ✅ COMPLETO. → M51 COMPLETO.** (1) **Índice remoto por git**:
  `[registry] index = "git+<URL>[@ref]"` (o `RAY_INDEX`) se **clona/cachea** en `.ray-deps/.index`
  (`ensure_index_clone`; clona si falta, no re-clona en cada resolución) y se usa como dir local. (2)
  **Reproducibilidad (lock-pinning)**: `resolve_pinned` reusa la versión ya bloqueada si sigue satisfaciendo
  el requisito → un caret no sube solo porque el índice gane una versión; se cierra la limitación de M51a.
  (3) **`ray update`** (`deps::update`): refresca el índice (`git pull`) y **re-resuelve** a la más alta
  compatible ignorando el lock; requirió que `ensure` **re-descargue cuando el disco (según el lock) no
  es la versión elegida** —antes no refrescaba `.ray-deps/<dep>` si existía, un bug latente que solo se
  manifiesta al cambiar de versión entre ejecuciones—. (4) **`ray yank <n>@<v> [--undo]`**
  (`index::set_yanked`): marca una versión como retirada (no se elige en nuevas resoluciones; un lock que
  ya la fijó la sigue usando). Tests offline (`tests/registry_cli.rs`): índice git `file://` clonado, lock
  que fija la versión + `update` que la sube, yank que excluye y `--undo` que restaura.

- **M51d — endurecimiento. ✅ COMPLETO. → cierra la revisión de diseño (jul 2026).** Tres cierres de
  seguridad/confianza detectados en la revisión del gestor+registro:
  1. **Nombres de paquete validados** (`deps::valid_package_name`: alfanumérico ASCII + `-`/`_`, empezando
     por alfanumérico). El nombre construye rutas (`.ray-deps/<nombre>`, `<índice>/<nombre>.toml`) y viene
     también del `ray.toml` de **transitivas no confiables**: sin la valla, un nombre `../../x` escapaba de
     la caché (y el camino de re-descarga hace `remove_dir_all` sobre esa ruta). Se valida en `ray add`,
     en las deps directas, en cada transitiva (señalando al culpable) y en las APIs del índice (defensa en
     profundidad).
  2. **El hash del índice se VERIFICA** (antes se publicaba y parseaba pero nadie lo consultaba —decorativo—).
     `resolve_pinned` devuelve el hash publicado de la versión elegida y `ensure` compara `hash_package` de lo
     descargado contra él → el índice pasa de "descubrimiento" a **raíz de confianza** (cierra el TOFU del
     lock, que confía en la primera descarga). Sin hash publicado no hay verificación (compat).
  3. **`ray publish` valida y hashea el TAG, no el working tree** (`hash_publicado` en `cli.rs`): clona el
     repo local en la ref publicada a un temporal —exactamente lo que un consumidor descargará—, exige la
     cara del paquete en el clon, lexea+parsea **todos** los `.ray`, y hashea eso. Antes, cambios sin
     commitear o archivos sueltos contaminaban el hash (y con la verificación de (2) habrían roto a los
     consumidores).
  4. **Índice remoto pinneado que no se queda obsoleto**: la caché `.ray-deps/.index` registra la spec con
     la que se clonó (`.index.spec`) y **se re-clona si cambia** (URL o ref); `ray update` refresca un índice
     con `@ref` vía `fetch` + re-checkout (el `git pull` de antes falla en checkout *detached*).
  Tests: unit (`valida_nombres_de_paquete`) + 4 de integración offline (`registry_cli`: nombre malicioso
  directo/transitivo, publish con working tree sucio + consumidor verde, hash del índice manipulado corta la
  resolución con mensaje claro, índice re-cacheado al cambiar la spec).
- **M51e — cierre de límites de v1. ✅ COMPLETO. → M51 COMPLETO (revisión cerrada).** Cierra H5/H6/H7 de
  la revisión de jul 2026:
  1. **Check semántico completo en `ray publish`** (cierra el diferido de M51d): `check_publicado`
     (`cli.rs`) resuelve las **dependencias del clon del tag** (por el índice o git; escriben dentro del
     temporal, DESPUÉS de calcular el hash), carga la cara con el loader (imports internos + deps + `std/`)
     y la verifica con `check_all_modulo` (el checker **sin exigir `main`**: un paquete es una librería —el
     mismo modo que usa el LSP para módulos). Un error se reporta contra su archivo y línea local
     (`Loaded::locate`): "el paquete no supera el check semántico (mod.ray, línea N): …".
  2. **Pre-releases** (`1.0.0-rc1`): `Version` pasa de tupla a struct con componente `pre` y orden semver
     §11 (pre < final a triple igual; identificadores numéricos por valor, alfanuméricos ASCII, prefijo
     corto menor). **Matching (regla de cargo)**: una pre-release solo casa si el requisito la menciona
     explícitamente con el mismo triple (`^1.0` jamás elige `1.1.0-rc1`; `1.3.0-rc1` o `^1.3.0-rc1` sí);
     `*` y `latest`/`ray add` sin versión eligen solo finales (con sugerencia si solo hay pre-releases).
     `deps::semver` (refs git) deja de recortar la pre → `mvs` y el lock-pinning la ordenan/casan bien.
     Una pre exige el triple completo (`1.0-rc1` es error: ambiguo).
  3. **Aviso de *dependency confusion***: si una dependencia descargada declara su **propio**
     `[registry] index` distinto del de este proyecto Y tiene deps por nombre, `ensure` avisa (sus
     transitivas se resuelven contra el índice del CONSUMIDOR). Mitigan además el lock (URL+hash) y la
     verificación del hash del índice (M51d).
  Tests: unit (orden/matching de pre-releases, `semver` con pre) + 3 de integración offline
  (`registry_cli`: publish rechaza un paquete que no chequea y acepta uno cuyo check resuelve una dep por
  nombre; el caret y `ray add` excluyen la rc y el requisito explícito la instala; la transitiva con índice
  propio avisa y corre).

- **M51f — `ray remove` + `ray search` + limpieza de módulos. ✅ COMPLETO.** Cierra los dos diferidos de
  ergonomía de §54.7: (1) **`ray remove <nombre>`** (`manifest::remove_dependency`, edición mínima línea a
  línea como `upsert_dependency`) — quita la dep, **re-resuelve** (lock reescrito) y borra la caché
  `.ray-deps/<nombre>` **solo si el lock ya no la lista** (`deps::locked_names`: la dep podría seguir
  siendo transitiva de otra). (2) **`ray search [patrón]`** — enumera los `<nombre>.toml` del índice
  (local o remoto cacheado), filtra por subcadena (case-insensitive) y muestra la versión instalable más
  alta por paquete (`index::latest`: final, no retirada). (3) **Limpieza**: `index.rs` mezclaba dos
  responsabilidades → el semver (`Version` + orden §11 + `VersionReq` + parsing, ~250 líneas que también
  consume `deps.rs` para refs git y el CLI para validar en publish) se extrae a **`src/semver.rs`**;
  `index.rs` queda solo con el índice (lectura/escritura/resolución). Tests: unit
  (`remove_quita_la_dep_y_preserva_el_resto`) + 2 de integración offline (`registry_cli`: remove quita
  manifiesto+lock+caché y falla claro si no está; search con patrón / completo / sin resultados).

### 54.6 Testing (offline y determinista, como M39c)

El índice de prueba es un **repo git local** servido por `file://` (`git init` + archivos `index/*.toml` +
paquetes-repo con tags). `ray add`/`ray fetch`/`ray publish` operan sobre él sin red; `publish` commitea la
entrada y una segunda resolución la usa. Reusa `deps_cli`/`cli_cli`. **Cero runtime nuevo**: todo es
resolución en el front-end/CLI; los motores nunca ven un paquete.

### 54.7 Diferido (fuera de M51)

Búsqueda/UI web del índice; cuentas y **firmas de publicación** (estilo sigstore) sobre el hash ya existente;
mirrors/proxy; *namespaces* con dueño; `ray.lock` con el propio índice como fuente (hoy fija el commit git);
**multi-índice** (hoy el índice es único por proyecto: una dep transitiva por
nombre se resuelve contra el índice del CONSUMIDOR — M51e lo detecta y **avisa** cuando el paquete declara
un índice propio; mitigan el lock (URL+hash) y la verificación del hash del índice). Los otros límites de la
revisión de jul 2026 quedaron cerrados: pre-releases (M51e), check semántico al publicar (M51e) y
`ray remove`/`ray search` (M51f).

## 55. M53 — Clientes de bases de datos (`packages/db`)

Plan completo y análisis de factibilidad en IDEAS.md §14. Resumen: MySQL (raylang puro, protocolo wire),
Postgres (evolución del cliente de `packages/net`) y SQLite (embebido: primitivos del host sobre
`rusqlite`, §55.3 — el plan FFI original se reformuló). Ubicación: paquete **`packages/db`** (tier 2; SQLite no
es red → no encaja en `net`), con API uniforme tipada-a-texto v1: `connect -> Result<Conn>`,
`query(c, sql) -> Result<[[string]]>`, `exec(c, sql) -> Result<int>` y `disconnect(c)` (no `close`: dentro
del módulo taparía al builtin global que cierra el socket).

### 55.1 M53.1 — cliente MySQL. ✅ COMPLETO

`packages/db/mysql.ray` habla el protocolo cliente/servidor clásico sobre los sockets de `std/net` y la
cripto de `std/crypto` (SHA1/SHA256, respaldadas por `ring` desde M43):

- **Framing**: paquetes `[longitud:3 LE][secuencia:1][carga]`, sobre un **búfer de lectura** en la `Conn`
  (`fill`/`take`: los sockets entregan trozos arbitrarios; el framing exige cantidades exactas — el mismo
  problema que resolvió `redis.ray` con `read_n`, aquí sobre `bytes`). La secuencia se resetea a 0 por
  comando y se rastrea por paquete leído.
- **Handshake v10**: se parsea versión/thread-id/capacidades y el **scramble en dos partes** (8 + 12
  octetos); la respuesta declara `CLIENT_PROTOCOL_41 | SECURE_CONNECTION | PLUGIN_AUTH` (+
  `CONNECT_WITH_DB` si hay esquema).
- **Auth**: `mysql_native_password` completa (`SHA1(pass) XOR SHA1(scramble + SHA1(SHA1(pass)))`) y
  `caching_sha2_password` en su **fast-path** (`SHA256(pass) XOR SHA256(SHA256(SHA256(pass)) + scramble)`;
  vale con la contraseña ya cacheada en el servidor). Soporta **AuthSwitchRequest** (recomputa para el
  plugin pedido). El **full-path** de caching_sha2 exige TLS *upgrade* a mitad de conexión (SSLRequest) y
  `std/net` solo cifra desde el octeto 0 (`tls_connect`) → error claro con el remedio. Diferido: un
  primitivo `__tls_upgrade(handle)` habilitaría el full-path (y STARTTLS en general).
- **Consultas**: `COM_QUERY` (protocolo de texto). `query` parsea el result set clásico (columna-count
  length-encoded, definiciones de columna, EOF, filas de lenc-strings con `0xfb` = NULL → `""`, EOF);
  `exec` lee el paquete OK (filas afectadas como lenc-int). Errores `ERR` (`0xff`) → `Result.Err` con el
  mensaje del servidor (saltando el sqlstate).
- **Gotcha de gramática** (documentado en CLAUDE.md como el struct-literal-vs-bloque): un literal de
  **tupla en posición de tail tras un `while`** se parsea como *llamada* al bloque (`while {…} (v, 9)`) →
  `return (v, 9);` explícito.

**Verificación** (`tests/mysql_cli.rs`, patrón de `postgres_cli.rs`): servidor MySQL **de juguete** en el
test (Rust std, TCP plano, sin cripto: scramble fijo + respuesta de auth **precomputada** con python) que
verifica la auth octeto a octeto, sirve un result set con NULL, un OK de `exec` (3 filas afectadas) y un
`ERR`; el cliente corre por **ambos motores** con stdout idéntico (oráculo conductual), más el caso de
contraseña incorrecta (ERR de acceso denegado visible y exit 1). **Diferido** (fases siguientes o futuras):
protocolo binario (prepared statements/parámetros/tipos), TLS, multi-result sets, `caching_sha2` full-path.

Siguiente: **M53.2** (Postgres v2: conexión persistente + protocolo extendido) y **M53.3/53.4** (FFI
out-params + SQLite).

### 55.2 M53.2 — cliente PostgreSQL v2 (protocolo extendido). ✅ COMPLETO

`packages/db/postgres.ray` evoluciona el cliente de una-consulta de `net/postgres.ray` (M32.1, simple
query protocol, devolvía la primera fila) a una API con **conexión persistente** y el **protocolo
extendido**:

- **Conexión persistente**: `connect` hace startup + handshake SCRAM-SHA-256 (reusa `net/scram`:
  AuthenticationSASL → SASLContinue → SASLFinal, verificando la firma del servidor) y deja la `Conn`
  lista; `query`/`exec` la reusan; `disconnect` manda Terminate ('X'). El búfer de lectura va en la
  `Conn` (un mensaje puede partirse entre lecturas, o venir varios juntos).
- **Protocolo extendido** (`send_extended`): por consulta manda **Parse** ('P', el SQL con marcadores
  `$1`/`$2`/…) → **Bind** ('B', los parámetros en **formato texto**, enlazados aparte del SQL →
  **anti-inyección**) → **Describe** ('D', portal) → **Execute** ('E') → **Sync** ('S'); `collect_response`
  drena '1'/'2'/'T' y recoge los **DataRow** ('D') hasta CommandComplete ('C') + ReadyForQuery ('Z').
  `query` devuelve **todas** las filas (`[[string]]`, NULL → ""); `exec` saca las filas afectadas del tag
  de CommandComplete ("INSERT 0 N"/"UPDATE N"/… → el último entero). Un ErrorResponse ('E') se decodifica
  (campo 'M') y se drena hasta ReadyForQuery para no dejar la conexión a medias.
- **Transacciones**: SQL corriente sobre la misma conexión (`exec(c, "BEGIN", [])` / `"COMMIT"` /
  `"ROLLBACK"`) — la persistencia es lo que las hace útiles.
- **Dependencia entre paquetes**: `db/postgres` importa `net/scram`. Con path-deps, el loader añade el
  **padre** de la dep (`packages/`) como raíz de módulos, así `net/scram` resuelve como cápsula hermana
  sin que el consumidor declare `net` aparte (ambas viven bajo `packages/`).

**Verificación** (`tests/postgres_v2_cli.rs`): servidor PostgreSQL **de juguete** que hace SCRAM con los
mismos valores **precomputados** que `postgres_cli.rs` y habla el protocolo extendido — **parsea el Bind,
extrae los parámetros y los devuelve como primera fila** (prueba que el binding fluye: anti-inyección),
sirve una segunda fila fija, un CommandComplete con filas afectadas para `exec`, y un ErrorResponse para
"BOOM". El cliente corre por **ambos motores** con stdout idéntico (query multi-fila con params +
BEGIN + INSERT con param + error). **Diferido**: parámetros binarios/tipados, sentencias preparadas con
estado, TLS, COPY, multi-statement.

### 55.3 M53.3 — primitivos SQLite sobre `rusqlite`. ✅ COMPLETO (REFORMULADO)

El plan original era extender el FFI de M41 con *out-params* de doble puntero y envolver `libsqlite3`
a mano. **Se reformuló con el giro a foco producción** (jul 2026): SQLite se integra como los builtins
de cripto de M43 — una **dependencia Rust madura** (`rusqlite`, con `bundled`) expuesta como primitivos
del host. La comparación que decidió:

- **Seguridad**: envolver la API C a mano exponía por primera vez a raylang a segfaults reales
  (use-after-finalize, lifetimes de `sqlite3_column_text`, destructores `SQLITE_TRANSIENT`). Con
  `rusqlite` el borrow checker vigila todo eso y el peor caso vuelve a ser un `Result.Err`.
- **Portabilidad**: `bundled` compila SQLite **dentro del binario** → sin `dlopen`, sin nombres de
  librería por plataforma, sin test condicionado a que el sistema traiga `libsqlite3`, versión
  determinista.
- **Costo**: 3 opcodes + impls por motor (mecánico, patrón M11.4) vs. tocar checker+FFI+ambos motores
  y decidir superficie nueva del lenguaje.

La extensión FFI de out-params **queda como diferido propio** (IDEAS §14): se hará cuando aparezca la
segunda librería C que la necesite, con un caso real guiando el diseño.

**Primitivos** (`src/builtins.rs`, arreglo etiquetado como los de I/O; stubs en wasm — el playground no
compila la librería C):

- `__sqlite_open(path) -> [string]`: `["ok", handle]`/`["err", msg]`. La conexión vive en el **registro
  común de handles** (`OpenHandle::Sqlite`) → `close(h)` la cierra (el `Drop` de `Connection` cierra la
  base); un handle cerrado o de otro tipo da error claro.
- `__sqlite_exec(h, sql, params: [string]) -> [string]`: `["ok", n_afectadas]`/`["err", msg]`.
- `__sqlite_query(h, sql, params) -> [string]`: `["ok", ncols, celda0, …]` (celdas **aplanadas** fila a
  fila; el envoltorio reconstruye) o `["err", msg]`.

Parámetros posicionales `?1`, `?2`, … enlazados como **texto** (la afinidad de tipos de SQLite convierte).
Celdas a texto: INTEGER/REAL → repr decimal, NULL → `""` (consistente con `db/mysql`), BLOB → hex. El
ciclo prepare→bind→step→finalize ocurre **entero dentro de cada helper** (el statement jamás escapa) →
sin use-after-finalize posible; cada celda se copia **antes** de avanzar (un `ValueRef` solo vive hasta
el siguiente paso). Nota: la conexión no es clonable → el lock del registro se retiene durante la
consulta (I/O local; serializa entre fibras).

### 55.4 M53.4 — cliente `db/sqlite`. ✅ COMPLETO

`packages/db/sqlite.ray`: la **misma API** que mysql/postgres — `connect(path) -> Result<Conn, string>`
(`":memory:"` = en memoria), `query(c, sql, params) -> Result<[[string]], string>` (reconstruye las
filas desde el arreglo aplanado), `exec -> Result<int, string>`, `disconnect` (libera el handle; usar
la conexión después falla limpio, como valor). Transacciones = SQL corriente (`BEGIN`/`COMMIT`/
`ROLLBACK`).

**Verificación** (`tests/sqlite_cli.rs`): al ser embebida y `":memory:"` determinista, **no hay servidor
de juguete** — oráculo conductual puro por ambos motores (DDL + INSERT con params + SELECT con NULL→"" +
consulta con parámetro + transacción con ROLLBACK + error SQL como valor + uso tras disconnect), más el
caso de ruta inválida (abrir un directorio → `Result.Err`). Demo autónomo `examples/db/sqlite_demo.ray`
(corre sin servidor). **M53 COMPLETO**: los tres clientes (MySQL wire, PostgreSQL extendido, SQLite
embebido) con API uniforme. Diferido: tipos nativos (celdas no-texto), `last_insert_rowid`, modo WAL.

## 56. M54 — Cliente MongoDB (`packages/db/mongo`)

Plan y factibilidad en IDEAS.md §15. Resumen: raylang puro (tier 2), auth = **SCRAM-SHA-256 vía SASL**
(reusa `net/scram`, como Postgres), wire = **OP_MSG** (cabecera 16 bytes LE + flags + un documento
BSON). La pieza central es BSON (§56.1). Superficie: `enum Bson` recursivo (no JSON strings — no hay
parser JSON en el ecosistema y JSON pierde tipos). `_id` lo asigna el servidor (determinismo de los
tests); `find` v1 = `firstBatch`.

### 56.1 M54.1 — BSON. ✅ COMPLETO

**(a) Habilitador — bits de float** (los primeros builtins nuevos tras M53): `__float_bits(float) ->
int` / `__float_from_bits(int) -> float` (opcodes `FloatBits`/`FloatFromBits`; el f64 de Rust en ambos
motores → oráculo `float_bits_oraculo`), expuestos como `math.float_bits`/`math.float_from_bits` en
std/math. Totales (cualquier patrón de bits es un f64 válido). Los pedía el `double` de BSON —
obligatorio para un cliente: el propio servidor responde `{ ok: 1.0 }` como double — y sirven a
cualquier formato binario con doubles (protobuf).

**(b) `packages/db/bson.ray`** — codificador/decodificador del formato de documentos de MongoDB
(bsonspec.org), raylang puro:

- **Representación**: `enum Bson { Double, Str, Doc([Field]), Arr([Bson]), Bin(bytes),
  ObjectId(bytes), Bool, Null, Int }` + `struct Field { name, value }` + azúcar `field(name, v)`.
  Recursivo vía el heap (`[Field]`/`[Bson]`).
- **`Int` único**: codifica como int64 (0x12) y decodifica **ambos** (int32 0x10 e int64) a `Int` —
  el int de raylang es i64. La fidelidad de round-trip es semántica, no de octetos.
- **`encode(doc) -> bytes`**: documentos anidados por composición (`enc_doc` devuelve `[int]`); un
  arreglo ES un documento con claves `"0"`, `"1"`, … (spec). `ObjectId` con longitud ≠ 12 = error del
  programador → `panic`.
- **`decode(bytes) -> Result<[Field], string>`**: errores como valores con la **posición del octeto**
  (truncado, longitudes inválidas, string sin terminador, UTF-8 malo, tipo no soportado, datos
  sobrantes). Cursor `struct Dec` mutado por referencia. **Gotcha aritmético**: un int64 LE no se arma
  con `b[7] << 56` (desbordaría el i64) → por mitades `hi (int32 con signo) * 2^32 + lo` (exacto y
  total); el int32 con signo se corrige restando 2^32.
- **`dump`/`dump_doc`**: repr JSON-ish determinista (para depuración y el oráculo de los tests).

**Verificación** (`tests/bson_cli.rs`, sin servidor): la codificación de `{"hello": "world"}`
reproduce **byte a byte** el vector canónico de bsonspec.org; el segundo vector del spec
(`{"BSON": ["awesome", 5.05, 1986]}`, double + int32) se decodifica; round-trip **exacto en octetos**
de todos los tipos v1 (anidados, negativos, UTF-8 multi-byte, int64 > 2^53); errores como valores.
Ambos motores, mismo stdout. Diferido: Date/Timestamp/Regex/Decimal128 (error claro al decodificar;
se añadirán cuando el cliente los necesite).

Siguiente: **M54.2** (conexión: OP_MSG + `hello` + auth SCRAM con toy server) → **M54.3** (CRUD + demo).

### 56.2 M54.2 — conexión: OP_MSG + `hello` + auth SCRAM. ✅ COMPLETO

`packages/db/mongo.ray` — la conexión autenticada, raylang puro sobre `db/bson` + `std/net`:

- **Framing OP_MSG** (opCode 2013, MongoDB ≥ 3.6): `[longitud][requestID][responseTo][opCode]
  [flagBits][kind=0][documento BSON]`. `send_msg` arma el sobre (flags 0, una sección kind 0);
  `read_msg` acumula en el búfer de la `Conn` (una respuesta puede partirse entre lecturas), valida
  opCode/kind y decodifica con `bson.decode`. **`run_command(c, doc)`** (pub) = enviar + leer: el
  ladrillo de toda operación. `bson.get(fields, name)` (helper nuevo en db/bson) inspecciona las
  respuestas; `ok` llega como **double 1.0** (por eso BSON necesitaba float_bits) y se acepta int.
- **Auth = `net/scram` reusado tal cual** (la apuesta del plan, confirmada): MongoDB moderno hace
  SCRAM-SHA-256 vía SASL — el mismo mecanismo que PostgreSQL, solo cambia el **sobre**: client-first/
  client-final viajan como campo `payload` (binario) de los comandos `saslStart`/`saslContinue`
  (con `$db`), y server-first/server-final vuelven en el `payload` de las respuestas. `connect` =
  `hello` (mínimo v1: solo exige ok) → `saslStart` → `saslContinue` → **`scram_verify`** de la firma
  del servidor (una clave mala se detecta EN EL CLIENTE aunque el servidor mienta con ok).
- `disconnect` cierra el socket (el protocolo no tiene despedida). `Conn` lleva `req_id` y `db`.

**Verificación** (`tests/mongo_cli.rs`): servidor MongoDB **de juguete** (Rust std, TCP plano) que
habla OP_MSG con respuestas BSON armadas a mano y reusa las constantes SCRAM **precomputadas** del
toy de PostgreSQL (mismo user/clave/nonce/sal/i → sin cripto en Rust). Cubre: conexión completa;
**contraseña mala** (el proof difiere → la firma del servidor no verifica, camino del cliente);
**usuario desconocido** (el servidor responde `ok: 0.0` + errmsg → el cliente lo surfacea). Ambos
motores, mismo stdout. Diferido v1: negociación de `hello` (compresión/versiones), checksum OP_MSG,
más de una sección. Siguiente: **M54.3** (CRUD: insert/find/update/delete + demo).

### 56.3 M54.3 — CRUD. ✅ COMPLETO — **M54 COMPLETO**

Las cuatro operaciones sobre `run_command` (cada una es un comando BSON con la colección en el campo
homónimo y `$db` de la `Conn`):

- **`insert(c, coll, docs: [[bson.Field]]) -> Result<int, string>`** (`n` insertados; `_id` lo asigna
  el servidor — decisión de M54: sin ObjectId en cliente → sin aleatoriedad → tests deterministas).
- **`find(c, coll, filter) -> Result<[[bson.Field]], string>`**: navega `cursor.firstBatch` del reply
  (v1 sin `getMore`); cada elemento del batch debe ser un documento.
- **`update(c, coll, filter, u, multi) -> Result<int, string>`** (`nModified`): el documento de
  actualización lo arma el usuario (`{$set: {...}}` explícito — fiel al protocolo, sin azúcar que
  esconda semántica).
- **`delete(c, coll, filter) -> Result<int, string>`** (`n`; `limit: 0` = todas las coincidencias).

Los filtros/documentos son `[bson.Field]` — **anti-inyección por construcción** (no hay string de
consulta que interpolar). **Verificación** (`tests/mongo_cli.rs`, segundo test): el toy server sirve
un cursor con firstBatch de dos documentos (uno sin campo → dump distinto), verifica que el documento
insertado y el `$set` VIAJAN dentro del comando (el binding fluye), y responde `ok: 0.0` + errmsg para
la colección `no_existe` (error del servidor como valor). Ambos motores, mismo stdout. Demo
`examples/db/mongo_demo.ray`. **M54 COMPLETO**: el paquete `db` cubre MySQL (wire), PostgreSQL
(extendido), SQLite (embebido) y MongoDB (documental). Diferido: `getMore`/cursores, tipos BSON
Date/Timestamp/Decimal128, `char_from_code`+`\uXXXX` y puente Json↔Bson (IDEAS §16), TLS.

### 56.4 Diferidos JSON (IDEAS §16) — `\uXXXX` + puente Json↔Bson. ✅ COMPLETOS

**(a) Escapes `\uXXXX` en `std/json`.** La causa raíz era un hueco del lenguaje: existía `char_code`
(M40.3a) pero no su inverso. Nuevo primitivo `__char_from_code(int) -> [char]` (opcode
`CharFromCode`; `[]` si no es un code point válido — surrogates o fuera de [0, 10FFFF]; **guard de
rango contra el wrap del cast a u32**: 2^32+65 NO es 'A') + envoltorio `char_from_code ->
Option<char>` en el prelude. `std/json` parsea `\uXXXX` con **pares surrogate** para los astrales y
errores como valores (surrogate suelto, par incompleto, dígito no hex). Oráculo
`char_from_code_oraculo` + `escapes_unicode` en `tests/json_cli.rs`.

**(b) Puente Json↔Bson en `db/bson`** (sobre `std/json`, que SÍ existía — M15.4a, embebido):
`from_json` (número JSON → `Double`; claves ordenadas, el objeto es Map), `to_json` (degradación
EXPLÍCITA, JSON es el sistema de tipos menor: `Int` → número con pérdida > 2^53, `ObjectId`/`Bin` →
hex, orden de campos perdido) y `doc_from_json(s) -> Result<[Field], string>` — la ruta ergonómica
para filtros: `mongo.find(c, coll, bson.doc_from_json("{\"nombre\": \"ada\"}")?)`, con el tope
obligado a objeto. Test `bson_puente_json` (compone con los escapes: `"café"` → `café`).
Diferido: Extended JSON riguroso ($oid/$numberLong).

### 56.5 Diferido — cursores `getMore`. ✅ COMPLETO

`find` deja de truncarse en el primer batch (el límite real: ~101 documentos con un servidor de
verdad) y **agota el cursor**: lee `firstBatch` y, mientras el `cursor.id` de la respuesta sea ≠ 0,
emite `getMore` (`{getMore: <id>, collection, $db}`; el id es un **int64** del wire — 0x12, justo lo
que codifica nuestro `Int`) acumulando cada `nextBatch`, hasta que el servidor responde id 0 (cursor
agotado). Refactor: `cursor_of`/`append_batch` (firstBatch y nextBatch comparten la extracción).
**Verificación**: el toy server pagina en 3 rondas (firstBatch con id 77 → getMore 77 → nextBatch con
id 88 → getMore 88 → nextBatch final con id 0), verificando que **cada id viaja** como int64 LE en el
comando siguiente; un id desconocido responde error (código 43, CursorNotFound). Ambos motores.
Diferido: `batchSize` configurable, `killCursors` (abandonar un cursor a medias).

## 57. Diferido TLS — `tls_upgrade` (STARTTLS de cliente). ✅ COMPLETO

El primitivo que faltaba del lado cliente: **envolver un socket TCP plano YA conectado en una sesión
TLS de cliente** — el simétrico exacto de `tls_accept` (M19.4b, que ya hacía el upgrade del lado
servidor). Habilita los protocolos que negocian en claro y luego suben a TLS: **Postgres
`sslRequest`**, el **full-path de `caching_sha2_password` de MySQL** (mandar la contraseña en claro
exige canal cifrado), SMTP STARTTLS, etc.

- **`__tls_upgrade(h, host) -> [string]`** (opcode `TlsUpgrade`) + `net.tls_upgrade -> Result<int,
  string>` en std/net. Verifica el certificado del servidor contra `host` con la misma config que
  `tls_connect` (raíces Mozilla + `SSL_CERT_FILE`).
- **Reusa el MISMO handle**: saca el `OpenHandle::Tcp` del registro y lo reinsierta como
  `OpenHandle::Tls` con el mismo id → el I/O existente (`socket_read/write_bytes`) se desvía solo a
  TLS vía `is_tls_handle`, cero cambios en los llamadores. Un handle que no es TCP plano (ya-TLS,
  archivo, listener) da error limpio como valor.
- **Modo del socket conservado**: en la VM el TCP ya es no bloqueante → el handshake lo conduce el
  primer I/O cediendo la fibra (como `tls_accept`); en el intérprete, bloqueante (rustls::Stream).

**Verificación** (`tests/tls_upgrade_cli.rs`): servidor STARTTLS de juguete (rustls + cert
autofirmado de `tests/fixtures/`) — fase en claro (`STARTTLS` → `GO`), handshake TLS **sobre el mismo
socket**, eco cifrado, y el doble-upgrade como error-valor. El cliente confía en la CA de prueba vía
`SSL_CERT_FILE`. Ambos motores, mismo stdout. Siguiente natural (diferido): cablearlo en los
clientes — `postgres.connect` con `sslRequest` y el full-path de MySQL.

### 57.1 TLS cableado en el cliente PostgreSQL (`connect_tls`). ✅ COMPLETO

Primer consumidor del primitivo: **`postgres.connect_tls(host, port, user, password, database,
nonce)`**. Manda el **sslRequest** del protocolo por el socket plano (`[longitud=8][código
80877103]`, sin octeto de tipo), espera el octeto `'S'` (un `'N'` = el servidor no soporta TLS →
error como valor), hace `net.tls_upgrade(handle, host)` (verificación del cert contra `host`) y
corre el startup + SCRAM de siempre **sobre el canal cifrado** — refactor: el cuerpo de `connect`
se extrajo a `startup_and_auth(handle, …)`, compartido por ambos (el I/O se desvía solo por el tipo
del handle → cero cambios en query/exec/disconnect).

**Verificación** (`tests/postgres_v2_cli.rs`): los helpers del toy server se generalizaron de
`&mut TcpStream` a `S: Read + Write` (`atender_stream`) → la MISMA sesión de juguete (SCRAM
precomputado + protocolo extendido con eco de params) se sirve ahora también tras un handshake TLS
real (rustls + cert de fixtures): el servidor valida el sslRequest octeto a octeto, responde 'S' y
cifra. Cliente confía vía `SSL_CERT_FILE`. Ambos motores. Diferido: el full-path de
`caching_sha2_password` de MySQL (mismo primitivo, protocolo distinto) y `sslmode` negociable
(hoy: `connect` = nunca TLS, `connect_tls` = obligatorio).

### 57.2 TLS cableado en el cliente MySQL (`connect_tls` + full-path). ✅ COMPLETO

Segundo consumidor del primitivo, y el que **cierra el hueco de autenticación** de M53.1:
`mysql.connect_tls(host, port, user, password, database)`.

- **SSLRequest**: en MySQL el upgrade va A MITAD del handshake — tras leer el handshake v10 del
  servidor, el cliente manda el paquete SSLRequest (= el **prefijo** de la respuesta: capacidades
  con `CLIENT_SSL` (2048) + paquete máximo + charset + 23 reservados, truncada antes del usuario),
  sube el MISMO socket con `net.tls_upgrade`, y manda la respuesta completa **cifrada** (la
  secuencia de paquetes continúa: handshake=0, SSLRequest=1, respuesta=2 — la lleva la `Conn`).
  Refactor: `connect`/`connect_tls` → `connect_opts(…, tls)`; el prefijo se comparte (`pre`).
- **Full-path de `caching_sha2_password`**: con la caché fría el servidor responde
  `AuthMoreData(0x04)`; ahora, si la conexión es TLS, el cliente manda la **contraseña en claro
  (con NUL) por el canal cifrado** y sigue el OK/ERR. En claro, el error se mantiene pero el
  remedio cambió: "usa connect_tls". (El intercambio RSA — full-path sin TLS — sigue diferido.)

**Verificación** (`tests/mysql_cli.rs`): `read_pkt`/la fase de comandos se generalizaron sobre
`Read`/`Read + Write` → el toy server TLS anuncia `caching_sha2_password`, **valida el SSLRequest
octeto a octeto** (32 octetos, `CLIENT_SSL` encendido), hace el handshake rustls sobre el mismo
socket, fuerza el full-auth y **verifica la contraseña en claro** recibida por TLS; contraseña mala
→ ERR cifrado ("acceso denegado"). Después sirve la fase de comandos de siempre, cifrada. Ambos
motores. **El hilo TLS de los clientes de bases de datos queda CERRADO** (Postgres §57.1 + MySQL
§57.2; Mongo-TLS sería `tls_connect` desde el arranque — trivial, cuando haga falta).

### 55.5 Protocolo binario de MySQL (prepared statements). ✅ COMPLETO

Cierra la asimetría de seguridad del paquete `db`: MySQL era el único cliente sin anti-inyección
por binding. **API**: `query(c, sql, params)` / `exec(c, sql, params)` (uniforme con postgres) —
con `params` la sentencia se **prepara** (protocolo binario, una ronda COM_STMT_PREPARE →
COM_STMT_EXECUTE → COM_STMT_CLOSE, como el portal anónimo de postgres); con `[]`, el COM_QUERY de
texto de siempre (cero regresión, sin round-trip extra).

- **Execute**: todos los parámetros se enlazan como `VAR_STRING` (texto; la afinidad del servidor
  convierte), bitmap de NULLs a cero (la API no distingue NULL de ""), `lenc_of` (el inverso de
  `lenc_int`).
- **La fila binaria se decodifica POR TIPO de columna** (leído de su definición,
  `col_type_flags`): ints de 1/2/4 con/sin signo (flag UNSIGNED), LONGLONG por mitades (b7<<56
  desbordaría; UNSIGNED ≥ 2^63 se muestra envuelto — documentado), **FLOAT reconstruido desde sus
  bits f32** (raylang solo tiene bits de f64 → mantisa + `math.pow(2, e)`, exacto), DOUBLE vía
  `math.float_from_bits`, DATE/DATETIME/TIMESTAMP/TIME **empaquetados** (longitudes 0/4/7/11-12,
  ceros → "0000-00-00", TIME acumula días en horas, micro `.%06d`), y el resto (DECIMAL/VARCHAR/
  BLOB/JSON/…) length-encoded. Salida = texto → misma API `[[string]]` que el protocolo de texto.
- **Gotchas del lenguaje cazados**: (1) un literal `[…]` en cola tras un if-sentencia se parsea
  como INDEXACIÓN (mismo compromiso que las tuplas-como-llamada, §55.1) → `return` explícito;
  (2) precedencia estilo C: `&` liga más flojo que `==` → `(flags & 32) != 0` necesita paréntesis.

**Verificación** (`tests/mysql_cli.rs`): el toy server gana los tres comandos — el prepare cuenta
los `?`, el execute **parsea el binding** (bitmap/tipos/valores lenc) y devuelve el parámetro como
primera celda (el binding fluye), más una fila binaria con LONGLONG **negativo** (-5), DOUBLE
(2.5), DATETIME empaquetado (2026-07-09 12:34:56), NULLs por bitmap (bits desplazados 2) y un
datetime cero; INSERT preparado → OK; BOOM → ERR en el execute. Ambos motores, mismo stdout.
Diferido: sentencias con estado (cachear el stmt_id), tipos binarios en los PARÁMETROS (hoy texto).

### 56.6 Diferidos menores del arco DB (rowid · Date/Timestamp · Mongo-TLS). ✅ COMPLETOS

Tres cierres pequeños de la lista consolidada de IDEAS §15:

- **`sqlite.last_insert_rowid(c) -> Result<int, string>`**: raylang puro — `SELECT
  last_insert_rowid()` sobre la MISMA conexión (el registro de handles la conserva) → cero soporte
  del host. El modo WAL ya era posible sin código: `query(c, "PRAGMA journal_mode=WAL", [])` (los
  PRAGMA que devuelven una fila van por `query`, no `exec`).
- **BSON `Date` (0x09) y `Timestamp` (0x11)** en `db/bson`: dos variantes nuevas del enum (epoch-ms
  UTC / crudo interno de replicación, ambos int64). `dump` renderiza la fecha como **ISO 8601 con
  milisegundos** reusando `net/time` (from_epoch_millis + to_iso8601 — tercera cápsula de `net` que
  `db` consume) y el timestamp como `(segundos, contador)`; `to_json` degrada a string ISO / número.
  Round-trip exacto en octetos verificado. Sin esto, decodificar cualquier colección real con
  campos de fecha daba "tipo BSON no soportado: 9".
- **`mongo.connect_tls`**: MongoDB cifra desde el octeto 0 (NO es STARTTLS) → `net.tls_connect` y
  la sesión entera (hello + SCRAM + comandos) corre sin cambios sobre el canal (refactor: `connect`/
  `connect_tls` → `session(handle, …)`). Toy server TLS (rustls desde el accept) + find cifrado,
  ambos motores. Con esto los TRES clientes de red del paquete tienen su variante TLS.

## 58. Endurecimiento del FFI (jul 2026) — libloading + catálogo completo

Revisión del FFI de M41 bajo el foco de producción. Dos arreglos y un plan:

- **Carga → `libloading`** (dep nueva, política post-giro): los `dlopen`/`dlsym` declarados a mano
  no existían en Windows/MSVC — y como `ffi::call` se compila en todo target no-wasm, **el binario
  de Windows del workflow de release (nunca ejecutado aún) no habría linkeado**. `libloading` es
  puro Rust sobre las APIs de plataforma (dlopen/LoadLibrary), con los mensajes de error reales del
  loader. La caché pasa a `&'static libloading::Library` (Box::leak: las librerías viven todo el
  proceso, los punteros a símbolos que retiene la VM lo exigen); el fallback al handle global del
  proceso usa `os::{unix,windows}::Library::this()`. Se añade el patrón de nombre `*.dll`.
- **Catálogo de aridad 3 completado**: cubría `[I,I,I]`, `[I,I,F]` y `[F,F,F]` — un
  `extern fn f(float, int, int)` legítimo caía en "firma no soportada". Ahora las 8 combinaciones.
  La LLAMADA sigue con el catálogo de moldes propio (transmute a `extern "C" fn` concreto por
  firma): sound, probado por el oráculo, suficiente para los consumidores actuales.
- **FFI v2 (anotado en IDEAS §14, sin fecha)**: `libffi` cuando aparezca la segunda librería C real
  — aridad libre, structs por valor y **callbacks** (closures de raylang como punteros a función C).
  Pendiente conocido documentado: variádicas (printf) son UB en arm64; indetectable desde la firma.

## 59. M55 — Templates compilados (`ray templ`). ✅ COMPLETO

La versión "limpia" de la localidad de PHP (decidida con el usuario tras optimizar `std/template`,
IDEAS §14b): el archivo ES la página, pero el código incrustado se limita a la sintaxis restringida
del template y las variables son **parámetros tipados**. Un `.ray.html` se compila a una FUNCIÓN
raylang, en la línea de `templ` (Go) / `askama` (Rust).

- **Superficie** (las 3 decisiones fijadas con el usuario): (1) la firma va INLINE como primera
  directiva — `{% params titulo: string, filas: [string] %}` (tipos = sintaxis raylang normal;
  `split_params` respeta comas anidadas de `Map<K,V>`/tuplas/`fn(A) -> R`); (2) comando explícito
  **`ray templ <archivo|dir>…`** que genera el `.ray` AL LADO (commiteable, inspeccionable, cero
  magia; recursivo en directorios); (3) nombre `render_<stem>` en el módulo homónimo
  (`vistas/lista.ray.html` → `import vistas/lista;` → `lista.render_lista(…)`).
- **Generador** (`src/templ.rs`, cliente del front-end como fmt/raydoc): tokeniza el template
  (espejo en Rust del de std/template) y emite raylang legible — texto → `out.push("literal")` (con
  `\ " $ \n \t \r` escapados: un `${` del HTML jamás se vuelve interpolación del generado);
  `{{ e }}` → `out.push(escape_html(to_string(e)))` (la EXPRESIÓN se empalma verbatim: `{{ p.nombre
  }}`, `{{ n + 1 }}`); `{{& e }}` sin escape; `{% if/elif/else %}` → cadena `if/else if/else` de
  raylang; `{% for pat in expr %}` → el `for` real (arreglos, rangos, Map con `(k, v)`,
  iteradores). Pila de marcos valida el anidamiento (endif/endfor casados). El escape reusa
  **`escape_html` de std/template (ahora pub)**. Tras generar, se valida que el `.ray` **parsea**
  (error temprano contra el template); el checker/pipeline lo validan del todo al compilar el
  programa.
- **La promesa del diseño, probada**: un typo en `{{ titluo }}` genera código que NO compila — el
  error de tipos señala el nombre — en vez del `""` silencioso del motor runtime (probado en
  `tests/templ_cli.rs`).
- **Rendimiento** (página 21 KB / 500 filas, release): **0.6 ms por render** — 2× sobre el motor
  runtime optimizado (1.3 ms) y 7.7× sobre el original pre-optimización (4.6 ms). Cero parseo, cero
  TVal, cero lookup por nombre en runtime.
- **Convivencia**: `std/template` (compile/render con contexto `TVal`) sigue para plantillas
  dinámicas (de BD/disco en caliente); `ray templ` es la vía para las vistas del propio código.
- **Soporte de editor (misma sesión)**: (a) `templ.rs` gana **posiciones** (cada token lleva su
  línea del template; los errores salen como `TplError { line, msg }`) y un **line map**
  (`generate_with_map`: línea-generada → línea-del-template); (b) el **LSP** diagnostica los
  buffers `.ray.html` — errores del propio template con su línea, y errores de TIPOS del módulo
  generado (analizado con el loader contra la ruta del `.ray` hermano → std/template y path-deps
  resuelven) **traducidos de vuelta al template** con el mapa: el typo en `{{ titluo }}` se subraya
  en el HTML (probado en `tests/lsp_cli.rs`); (b2) **completion y hover en el template**: dentro de
  `{{ … }}`/`{% … %}` (y solo ahí — fuera, el HTML es del editor) se ofrecen los **params tipados**
  de la cabecera `{% params %}` (el tipo como detalle), las **variables de los `{% for %}` que
  encierran el cursor** (tipo inferido: `[T]` → `T`, rango → `int`) y, en etiqueta, las palabras
  clave del template (`templ::header_params` tolerante + escaneo textual del prefijo — el buffer a
  medio escribir no tokeniza entero); hover sobre un param o var de bucle → `nombre: tipo`
  (`template_hover_at`, mismo ámbito `template_scope`); (b3) **inteligencia semántica en las
  expresiones**: como la expresión se empalma VERBATIM en el módulo generado, la posición del
  cursor se **traduce al generado** (`template_pos_to_generated`: la aguja es el contenido del
  delimitador —en etiquetas sin la keyword— localizada en la línea generada que el line map
  atribuye a esa línea) y ahí corre la maquinaria existente — **hover con tipos reales** del
  checker (`fila.precio: float`), **completion de miembros** tras `.` (builtins/métodos del
  receptor; el `from std/template import` se stubbea porque member_completion es de un buffer),
  **ir-a-definición** (una def en otro archivo se devuelve tal cual; una en el generado —un
  param— vuelve al template por el line map: lleva al `{% params %}`) y **signature help**
  (`signature_help_at`, extraído). Cero lógica semántica nueva: solo traducción de posiciones;
  (b4) **references / rename / highlight / outline** en el template, sobre un motor común:
  `template_occurrences` escanea los delimitadores y resuelve cada ident a su binding (`p:<nombre>`
  param / `f:<línea>:<col>` var de for, con shadowing por pila de bloques; un miembro tras `.`, una
  keyword o un nombre de tipo no ligan) — rename de un param es seguro hacia afuera (los llamadores
  del `render_<x>` pasan args posicionales); el outline lista `render_<stem>` con params y vars de
  for como hijas; (c) **coloreado**: gramática de VSCode
  `raylang-template` (`text.html.raylang`: HTML base + `{{ }}`/`{% %}` con la expresión embebida
  como `source.raylang`; el cliente LSP añade el selector) y sintaxis de Sublime
  (`raylang template.sublime-syntax` — el nombre de ARCHIVO coincide con el `name:` para que el
  settings por-sintaxis `raylang template.sublime-settings` con el `auto_complete_selector` del
  popup lo encuentre —, `extends` HTML + `prototype` con los delimitadores; selector
  LSP `source.raylang | text.html.raylang` en el README).
- **Regeneración automática** (misma sesión, cierra el diferido): `ray run`/`ray build`/`ray test`
  regeneran antes de compilar cada `.ray.html` bajo el directorio de la entrada cuyo `.ray` falte o
  esté desactualizado (mtime; `regen_stale_templates` en `cli.rs`) — aviso por stderr (stdout es
  del programa); template roto → 65 con su error (mejor señal que compilar el generado viejo);
  generados al día → un stat por template. `collect_templates` salta directorios ocultos
  (`.git`, `.ray-deps`).
- **Composición de templates (misma sesión, cierra el diferido de include/layouts)**: dos
  etiquetas nuevas, cero maquinaria — un template compila a una **función en un módulo**, así que
  componer = importar y llamar. **`{% import vistas/tarjeta [as t] %}`** emite `import …;` en el
  generado (**hoisteado** a la cabecera con su línea en el line map; el argumento se valida —
  segmentos identificador separados por `/` — para no empalmar texto arbitrario).
  **`{% include ruta/al/template(args) %}`** incluye otro template **por su ruta** (misma sesión,
  a petición del usuario: el nombre `render_<x>` es un artefacto de compilación y quien escribe el
  template no tiene por qué conocerlo): `template_ref` reconoce la forma `ruta(args)` (segmentos
  identificador con `/`, sin puntos), el generador **auto-importa** el módulo (dedup con un import
  explícito) y llama a `leaf.render_<leaf>(args)`. La forma `{% include expr %}` (cualquier otra
  cosa: `contenido`, `m.f(x)`) empalma la expresión cruda **sin escapar** (HTML ya renderizado;
  equivale a `{{& expr }}` con la intención declarada). El autoescape compone: el partial escapó
  sus datos al renderizarse; el include no re-escapa. LSP: en la forma `ruta(args)` la aguja del
  mapeo de posiciones son los ARGS (la ruta no aparece en el generado) y la ruta no liga como
  variable en rename/references. **Layouts sin slots**: el layout es un template más con un param `contenido: string`
  que hace `{% include contenido %}`; quien compone llama `layout.render_layout(titulo,
  pagina.render_pagina(…))`. LSP/coloreado al día (keywords `include`/`import`; una ruta de import
  no liga como variable).
- **`{% let %}` + herencia de layout (misma sesión, cierra los diferidos)**:
  (a) **`{% let nombre = expr %}`** — local inmutable del template (baja a `let …;`; alcance = el
  bloque generado: dentro de un for/if muere en su end). (b) **`{% extends ruta %}` +
  `{% block nombre %}…{% endblock %}`** (estilo Jinja, resuelto **EN COMPILACIÓN**): el hijo
  declara `extends` como primera etiqueta tras `params` y solo aporta bloques (+ imports); el
  layout es otro template cuyos `{% block %}` marcan los huecos con contenido por defecto. La
  fusión es de **tokens** (`resolve_extends`/`merge_layout` en `templ.rs`): se descarta el
  `{% params %}` del layout (la firma manda la del HIJO; las variables que el layout use deben
  estar en los params del hijo — **el checker lo exige**, typo = error), cada bloque se sustituye
  por el del hijo o queda su defecto, y el stream fusionado genera normal. Ruta del layout
  **desde la raíz del proyecto** (el `ray.toml` más cercano por encima del template;
  `project_root_of`) — UNA sola convención de rutas en templates, la misma de import/include —,
  con fallback al directorio del template (proyectos sin manifiesto / layout hermano); el layout
  compila **standalone** (los marcadores de bloque son transparentes). Line map: los bloques del
  hijo mapean exactos; las líneas del layout se
  atribuyen al `{% extends %}` (degradación honesta). Errores dedicados: bloque que el layout no
  declara (typo, con su línea), contenido suelto en un hijo con extends, bloques anidados/sin
  cerrar, herencia encadenada (diferida). (c) **Snippets de bloque en el LSP**: teclear `for`/`if`
  en el HTML (o la keyword en un `{%`) ofrece el bloque entero con placeholders navegables
  (`{% for elem in coleccion %}…{% endfor %}`, if/else, let, block) — `template_block_snippets`.
- **Formateador de templates (misma sesión)**: `templ::format_template` — cada etiqueta `{% %}`
  en su propia línea, indentación por profundidad de bloques (`for`/`if`/`block` sangran;
  `elif`/`else` al nivel del abridor), `{{ }}` inline con su HTML, delimitadores normalizados
  (`{%for x%}` → `{% for x %}`) sin tocar el interior de las expresiones, blancos conservados;
  idempotente; un buffer que no tokeniza no se toca. Lo usan el **LSP** (`formatting_result`,
  honrando `tabSize`/`insertSpaces` del editor) y **`ray fmt archivo.ray.html`** (con la
  indentación de `.editorconfig`/`ray.toml`, canónico 4). El whitespace entre nodos cambia
  (inocuo en HTML). Diferido: re-indentar también por etiquetas HTML anidadas.
- Diferido: herencia encadenada (layout que extiende a otro), `{% block %}` con contenido del
  padre (`super()` de Jinja).

## 60. M56 — Webserver de producción (endurecimiento + funcionalidad HTTP)

> Revisión completa del webserver (jul 2026). Veredicto: el **núcleo es sólido** — la parte
> difícil (fibras + poller kqueue/epoll + escritura parcial + multicore M38) vive en el runtime
> genérico y está bien testeada; HTTP es librería pura (`packages/net/webserver.ray`, espejo en
> `examples/web/webserver.ray` que usan los tests/framework). Lo que falta está TODO en la capa
> raylang o son toques acotados de runtime. Clasificación en IDEAS §17.

**Los huecos, por severidad** (hallazgos de la revisión):

1. **Frontera de seguridad** (crítico si "producción real"): sin límite de tamaño de cabeceras ni
   de cuerpo (memoria ilimitada a petición del cliente), sin timeouts (slowloris deja fibras
   aparcadas para siempre), fuga de fd si el handler hace panic (el `close(conn)` de `atender` no
   corre), `find_fin_cabeceras` O(n²) (amplificador del punto 1), cuerpo truncado silencioso
   (`Ok` con el cuerpo a medias si el peer cierra antes de `Content-Length`), `serve_raw` muere
   al primer error de `accept` (un `EMFILE` transitorio tumba el servidor), sin tope de
   conexiones simultáneas.
2. **Bug funcional**: la query string va pegada a `req.path` (`GET /u/7?x=1` no casa la ruta del
   framework); sin `req.query` ni percent-decoding (y `url.ray` ya existe, sin integrar).
3. **HTTPS de servidor**: `net.tls_accept` existe y `wss_echo.ray` demuestra el patrón, pero
   `serve_raw` acepta y despacha él mismo → no hay dónde insertar el upgrade. Falta `serve_tls`.
4. **Keep-alive**: siempre `Connection: close` → un handshake TCP por petición (el mayor coste
   de rendimiento real).
5. **Cabeceras duplicadas**: `Map<string,string>` impide dos `Set-Cookie` en una respuesta (bloquea
   sesiones+flash) y pisa repetidas entrantes.
6. **Menores**: sin chunked entrante, sin helper de archivos estáticos (con saneo de `..`),
   `status_text` sin 3xx/401/403/405/503, enrutado sin `Option` en `param()`.

**Sub-fases** (orden por conveniencia; cada una committeable):

- **M56.1 — frontera de seguridad, parte librería** ✅ **COMPLETO** (cero runtime): `struct
  Limits { max_header_bytes, max_body_bytes, max_conns }` + `default_limits()` (64 KiB / 10 MiB /
  1024); las firmas existentes no cambian (usan los defaults) y las variantes
  `serve_limits`/`serve_raw_limits`/`read_request_limits` los configuran. Cabeceras acotadas
  (el tope se comprueba al acumular Y sobre el separador encontrado — sin eso el límite
  dependería del troceo de la red), Content-Length acotado ANTES de leer el cuerpo, cuerpo
  truncado por el peer → `Err` (antes: `Ok` a medias) y recortado a `n` exacto, escaneo
  incremental del fin de cabeceras (O(n), reanuda 3 octetos atrás por si el `\r\n\r\n` queda
  partido entre trozos), petición ilegible → **400 best-effort** antes de cerrar, accept-loop
  resiliente (log-y-seguir; solo 100 errores seguidos rinden), tope de conexiones simultáneas
  vía **canal acotado como semáforo** (`Channel.bounded(n)` de M12.2: `send` antes de spawn cede
  al llenarse → backpressure gratis, sin estado compartido — compatible con los heaps aislados de
  M38; `max_conns <= 0` = sin tope; el token se fuga si el handler panica → lo cierra M56.5),
  `status_text` completo (3xx/401/403/405/413/429/431/5xx). En `packages/net/webserver.ray` y su
  espejo `examples/web/webserver.ray`; test `servidor_aplica_limites_de_seguridad` (4 casos) +
  verificado en vivo sobre el demo SSR (cabecera de 70 KiB → 400, Content-Length gigante → 400
  rechazado en la declaración, el servidor sigue vivo).
- **M56.2 — query string + percent-decoding** ✅ **COMPLETO**: `Request` gana `query` (la query
  string CRUDA tras el `?`, "" si no hay) y `path` queda **decodificado y SIN query** — cambio
  semántico deliberado que arregla el enrutado (antes `GET /u/7?x=1` no casaba ninguna ruta del
  framework). La línea de petición se parte en el PRIMER `?`; un escape `%XX` inválido en la ruta
  es `Err` → 400 (no una ruta corrupta silenciosa). Nuevo en `std/url`: **`percent_decode`**
  (solo `%XX`; el `+` queda literal — la regla de RUTAS; el `+`=espacio es exclusivo de
  form-urlencoded) y `url_decode` pasa a delegar (`percent_decode(s.replace("+", " "))`).
  Helpers nuevos: `webserver.query_params(req) -> Map<string,string>` (parsea con
  `url.parse_query`, decodificada) y `framework.query(ctx, nombre)` (simétrico de `param`, "" si
  no existe). Demo `/saluda?nombre=Ada` en `framework_demo.ray`. Tests: enrutado con `?` +
  decodificación de path/query en `webserver_cli` y `framework_cli`; verificado en vivo sobre el
  SSR (`?utm=x` ya no contamina el último segmento; `/lang/ru%73t` decodifica).
- **M56.3 — HTTPS de servidor** ✅ **COMPLETO** (librería pura): `serve_tls`/`serve_tls_limits`/
  `serve_raw_tls`/`serve_raw_tls_limits` (`cert`/`key` = PEM en contenido, como `net.tls_accept`).
  Pieza central: el accept-loop se factoriza en **`bucle_servidor(host, port, limits, preparar,
  handler)`** donde `preparar: fn(int) -> Result<int,string>` transforma la conexión aceptada
  ANTES de atenderla — identidad para HTTP, `net.tls_accept` para HTTPS — y corre **dentro de la
  fibra de la conexión** (un handshake lento no frena el accept; un cliente que habla HTTP plano
  contra el puerto TLS falla SU conexión, logueada, sin tocar a las demás). `responder(handler)`
  factoriza el adaptador petición→respuesta. Compone con M56.1 (límites/semáforo) y M56.2 (query)
  sin código extra. Demo `examples/web/https_server_demo.ray` (`curl -k https://localhost:8443/hola`);
  test `servidor_https_sirve_sobre_tls` en `tls_cli.rs` (petición con query sobre TLS, 404,
  y resiliencia ante un cliente no-TLS) con los fixtures de CA locales.
- **M56.4 — timeouts de lectura** ✅ **COMPLETO** (primer toque de runtime del arco; aditivo,
  oráculo intacto). **Primitivo**: `__socket_set_read_timeout(h, ms)` (total; ms <= 0 lo quita) +
  envoltorio `net.set_read_timeout` en std/net. **Dos mecanismos según el motor**: en la **VM**
  (sockets no bloqueantes) el timeout vive en un mapa del host (`read_timeouts`); al aparcar una
  fibra por E/S, `IoParked` gana `handle` + `deadline` (calculado con `read_deadline(h)`) y
  `io_wait` espera en el poller **como mucho hasta el deadline más próximo** (antes: infinito);
  al vencer, marca el handle (`mark_read_timeout`) y despierta la fibra — su lectura re-ejecutada
  consume la marca (`take_read_expired`) y devuelve el error. En el **intérprete** (bloqueante)
  se aplica el `SO_RCVTIMEO` real y las lecturas mapean `WouldBlock`/`TimedOut` al MISMO mensaje
  (`"tiempo de espera de lectura agotado"`). El deadline aplica a cualquier espera aparcada del
  handle (lecturas TCP/TLS; también accept/UDP — semántica uniforme). Sin timeout puesto, cero
  cambios (espera infinita, idéntico a M17); en plataformas sin poller (busy-poll) el deadline no
  vence (documentado; macOS/Linux tienen poller). **Webserver**: `Limits` gana
  `read_timeout_millis` (default **10 s**, <= 0 = sin plazo) como **plazo TOTAL** de leer una
  petición — `leer_con_plazo` fija antes de cada lectura el timeout a lo que RESTA del plazo
  (`time.monotonic()`), así el goteo de octetos NO lo renueva (slowloris real); al completar la
  petición el timeout se quita (el handler no hereda un plazo casi vencido — SSE). Test
  `servidor_corta_lecturas_lentas_por_timeout` (petición a medias + silencio → 400 en ~300 ms y
  el servidor sigue vivo). Diferido: timeout del HANDLER (necesita cancelación por timer;
  emparejarlo con M56.5/`try_join`).
- **M56.5 — cierre en panic del handler** ✅ **COMPLETO** (segundo y último toque de runtime del
  arco; aditivo). **`try_join(t: Task<T>) -> Result<T, string>`** — une una tarea devolviendo su
  desenlace como VALOR (`Ok(valor)` / `Err(mensaje del panic)`), a diferencia de `join`, que
  re-lanza. **Cero maquinaria nueva de enums**: el patrón primitivo+prelude de M11.2 — primitivo
  `__task_failed(t) -> [string]` (opcode `TaskFailed`: espera como TaskJoin —mismo guard único +
  park `Waiting::Join`— y empuja `[]` si acabó bien / `[msg]` si falló) + envoltorio `try_join` en
  el prelude que arma el `Result` y reusa `join` para el valor (con la tarea ya terminada ni
  bloquea ni falla). Intérprete: error limpio "requiere la VM" (junto a spawn/recv). **Webserver**:
  `atender` corre el handler en su PROPIA tarea y observa el fallo con `try_join` → un panic del
  handler se loguea, responde **500 best-effort** y SIEMPRE se cierra la conexión — cierra la fuga
  de fd y la fuga del token del semáforo anotadas en M56.1. Tests:
  `try_join_observa_el_fallo_sin_relanzar` (concurrency_cli, salida exacta, incl. `Task<unit>` por
  inferencia — `unit` no es anotable como arg de tipo) y
  `handler_que_panica_responde_500_y_no_fuga_recursos` (webserver_cli: 4 panics > max_conns=2 → si
  fugara tokens colgaría; el servidor sigue en 200; usa el `serve` REAL, matado por el test).
  Diferido: timeout del handler (componer `try_join` con un timer de cancelación).
- **M56.6 — keep-alive** ✅ **COMPLETO** (librería pura). El camino **ergonómico** (`serve`/
  `serve_tls`, donde el servidor escribe la respuesta) sirve peticiones **en bucle sobre la misma
  conexión** (`atender_http`): keep-alive por defecto en HTTP/1.1, se cierra con `Connection:
  close`, con versión ≠ HTTP/1.1 (el keep-alive de 1.0 es opt-in y declinarlo es válido), al
  fallar algo, o por **ocio** (el `read_timeout_millis` de M56.4 corta la espera de la siguiente
  petición). `serve_raw`/SSE conservan una-petición-y-cerrar (el handler crudo posee la conexión).
  Piezas: `Request` gana **`version`** (de la línea de petición); `read_request_limits` distingue
  **"conexión cerrada"** (EOF/timeout SIN nada recibido — el fin normal de una keep-alive: cierre
  en silencio, sin 400 ni ruido) de una petición malformada; `send_response_keep(conn, r, keep)`
  interno parametriza el header `Connection` (el `send_response` público conserva `close`);
  `bucle_servidor` pasa de recibir el handler a recibir **`atender_conn: fn(int)`** (crudo =
  `atender`, ergonómico = `atender_http`); el handler de cada petición sigue corriendo en su
  tarea (M56.5). **El framework se sube gratis**: `listen` pasa de `serve_raw`+`send_response` a
  `webserver.serve` → keep-alive + límites + panic→500 sin código. Test
  `servidor_mantiene_la_conexion_viva_entre_peticiones` (2 peticiones por la misma conexión,
  `Connection: close` honrado con EOF, y cierre silencioso de una ociosa al vencer el timeout);
  verificado en vivo (curl "Re-using existing connection" sobre el SSR). Diferido: tope de
  peticiones por conexión (el ocio ya acota; añadir si un consumidor lo pide).
- **M56.7 — cabeceras múltiples (`Set-Cookie`)** ✅ **COMPLETO** (librería pura). **Decisión de
  API fijada con el usuario**: `Response.headers` sigue siendo `Map<string,string>` (una por
  nombre, ergonómico) y la ÚNICA cabecera de respuesta que se repite de verdad — `Set-Cookie`
  (sesión + flash…; hasta la RFC 7230 la trata como caso especial) — gana su campo dedicado
  **`set_cookie: [string]`** (una línea por cookie, emitida en orden de inserción, nunca fusionada
  con coma — eso rompería las cookies). Se descartó `headers: [(k,v)]` (más general pero rompe
  todo uso de `insert`/`get` para un caso que solo es Set-Cookie). Helper **`with_cookie(r, línea)
  -> Response`** encadenable (`ok(html).with_cookie(…).with_cookie(…)`; ojo: el encadenado por
  UFCS pide el nombre en ámbito — `from webserver import with_cookie` — o la forma calificada).
  El framework se suma: `Res` gana `set_cookie` + helper `cookie(res, línea)` encadenable, y
  `build_response` lo pasa. Rompe los literales `Response { … }` (campo nuevo obligatorio;
  actualizados los del repo: webserver ×5, framework, metrics_server_demo). Tests:
  `respuesta_con_varias_cookies` (2 líneas Set-Cookie exactas, no fusionadas) y ruta `/entra` del
  framework demo.
- **M56.8 — extras** ✅ **COMPLETO** (librería pura), **CIERRA M56**. (a) **Chunked entrante**:
  un cuerpo `Transfer-Encoding: chunked` se decodifica (`leer_cuerpo_chunked`: tamaños hex con
  extensiones `;ext` ignoradas, prioridad sobre Content-Length —RFC 7230 §3.3.3—, tamaño
  DECODIFICADO contra `max_body_bytes`, plazo total respetado; sin trailers). Antes llegaba vacío
  en silencio. (b) **`static_response(dir, req.path) -> Response`**: sirve archivos estáticos con
  SANEO — un segmento `..`/`.` es 404 directo (la ruta llega percent-decodificada por M56.2, así
  que `%2e%2e` cae igual), `/` final sirve `index.html`, Content-Type por extensión (`mime_of`,
  ~14 tipos comunes; resto octet-stream), archivo ilegible = 404. (c) **HEAD sin cuerpo**: el
  camino `serve` responde a un HEAD las cabeceras del GET equivalente (incl. su Content-Length)
  SIN el cuerpo (RFC 9110 §9.3.2; `omit_body` en la escritura); el framework enruta HEAD como GET.
  Tests: `head_devuelve_cabeceras_sin_cuerpo`, `cuerpo_chunked_se_decodifica` (con extensión de
  chunk), `archivos_estaticos_con_saneo` (index + traversal + 404). **M56 COMPLETO** (56.1–56.8;
  webserver_cli pasa de 3 a 11 tests). Diferidos menores: trailers de chunked, pipelining
  HTTP/1.1 (los navegadores no lo usan; los octetos de una petición adelantada se descartan),
  Range/cache de estáticos, tope de peticiones por conexión keep-alive.

## 61. M57 — Tiempo y fechas de producción

> Revisión jul 2026 (tras M56; clasificación en IDEAS §18). El modelo actual — **`int` epoch-ms
> UTC como moneda universal**, `monotonic()` para intervalos, `DateTime` como vista civil sin
> offset, **solo UTC** — es sano y NO se toca. M57 pule las aristas.

**Reparto std-vs-package fijado con el usuario** (política §53): las fechas civiles se
**promueven a `std/time`** (universales/ligeras/estables; hoy viven en `net/time` y las usan
net/log, net/sigv4 y db/bson — cruce de paquetes); `net/time` queda como reexport. Candidatos a
package (a demanda): `tz` (IANA, leyendo TZif de /usr/share/zoneinfo en raylang puro), `net/ntp`
(SNTP), `cron` (sobre el sleep de fibra), `dist` (HLC/Lamport).

- **M57.1 — promoción a `std/time` + parse endurecido** ✅ **COMPLETO**: `std/time` absorbe las
  fechas civiles (`DateTime` + Hinnant + formatos + `format_duration`); `net/time` y el espejo
  `examples/web/time.ray` quedan como **reexports** (`pub from std/time import …`, incluido el
  TIPO `DateTime` — el reexport de tipos y el acceso calificado a través del reexport funcionan;
  lo verifica `cli_cli` con `import net/time; time.to_iso8601(…)`). **Parse endurecido**: nuevo
  **`parse_iso8601_millis(s) -> Result<int, string>`** (a la moneda epoch-ms) que acepta
  `YYYY-MM-DDTHH:MM:SS[.fff…][Z|±HH:MM]` — offset normalizado a UTC, fracción → ms (dígitos de
  más truncados), `T`/`t`/espacio como separador — y rechaza con `Err` real: campo no numérico
  (antes: **0 en silencio**), mes/hora fuera de rango, **fecha inexistente** (validación exacta
  por round-trip civil: 31 de abril y 29-feb no bisiesto caen sin tabla de meses), offset
  malformado o ausente, texto sobrante, resultado pre-1970. `parse_iso8601` (compat) delega y
  devuelve el `DateTime` normalizado a UTC (fracción truncada a segundos). `monotonic`
  documentado como por-proceso (no persistible). Golden `time_cli` ampliado (10 casos nuevos,
  ambos motores, vectores Python).
- **M57.2 — sleep cooperativo de fibra** ✅ **COMPLETO** (único toque de runtime): `time.sleep`
  en la VM pasa de `thread::sleep` (bloqueaba el worker; en M:1 congelaba TODAS las fibras) a
  **aparcar la fibra con deadline sin fd** (`IoParked { fd: -1, handle: -1, deadline }` — la
  maquinaria de M56.4). El resultado (unit) se empuja ANTES de aparcar y el ip NO se rebobina: al
  despertar, la fibra continúa (no re-ejecuta el sleep). `io_wait`: las durmientes no entran al
  poller (fd < 0 filtrado); su expiración despierta SIN marcar timeout (la marca es solo para
  esperas con handle >= 0); si SOLO hay durmientes (sin fds), el poller con listas vacías retorna
  al instante → se duerme el hilo hasta el deadline más próximo (correcto: `running == 0`, un
  solo worker llega ahí) — de rebote, el sleep de fibra funciona también en plataformas SIN
  poller; el busy-poll de respaldo solo despierta las fibras CON fd. El intérprete sigue
  bloqueante (un hilo, documentado en std/time). Desbloquea timeout-de-handler (diferido de
  M56.5), retries y el futuro `cron`. Test `sleep_cede_la_fibra` (la hija corre y despierta
  mientras main duerme — antes no corría hasta el join); la propiedad `sleep >= ms` de
  `time_random_cli` intacta.
- **M57.3 — UUID v7** ✅ **COMPLETO**, **CIERRA M57**: `uuid_v7()` en `std/uuid` (RFC 9562:
  48 bits de timestamp Unix-ms big-endian + 74 aleatorios → el orden lexicográfico ES el orden
  temporal; claves de DB/trazas) + `uuid_v7_at(ms)` (timestamp explícito, testeable) +
  `is_uuid_v7`. La validación y el formato canónico se factorizan con v4 (`is_uuid_ver`/
  `formato`). Verificado por ambos motores contra el **vector de la RFC 9562** (2022-02-22T19:22:22Z
  → prefijo `017f22e2-79b0-7`) + orden lexicográfico de ms consecutivos. **M57 COMPLETO**
  (57.1–57.3). Diferidos (a demanda, IDEAS §18): `tz` IANA, `net/ntp`, `cron`, `dist`/HLC,
  `__local_offset_millis`, monotonicidad intra-ms del v7 (contador rand_a).

## 62. M58 — Clientes web de producción (WebSocket · HTTP/1.1 · HTTP/2)

> Revisión jul 2026 (tras M56/M57; clasificación en IDEAS §19). Las tres piezas comparten patrón:
> cripto y framing verificados contra vectores RFC (SHA-1/base64, HPACK+Huffman, frames h2);
> falta la robustez de red que el servidor recibió en M56. Todo librería raylang pura.

- **M58.1 — WebSocket robusto** ✅ **COMPLETO**: lector de tramas BUFFERIZADO — `WsConn { conn,
  buf, mask }` (`server_conn`/`client_conn`) con `read_frame[_limit]` (acumula hasta
  cabecera+longitud exactas — `frame_total` valida la longitud DECLARADA contra el límite, 16 MiB
  por defecto, ANTES de leer la carga — y conserva el sobrante para la siguiente llamada) y
  `read_message[_limit]` (auto **ping→pong** con el enmascarado del lado correcto, pongs
  ignorados, close-handshake de cortesía —devuelve el close para que el llamador pare—, y
  **fragmentación de recepción**: continuations hasta FIN con control intercalado permitido y el
  total también acotado). `encode_frame_masked` sube del cliente a la librería. **API del cliente
  rota a propósito**: `connect`/`connect_tls` devuelven `WsConn` (el estado es imprescindible — el
  buffer sobrevive entre `recv_text`; además `read_handshake` conserva los octetos que lleguen
  PEGADOS al 101 y siembra el buffer); `recv_text` distingue el close (`Err("conexión cerrada por
  el peer")`). `extract_key`/`verify_handshake` case-insensitive. Los ecos (`websocket_echo`/
  `wss_echo`) reescritos encima (más simples). Test `echo_server_robusto_ante_framing_real`: trama
  partida en dos escrituras (antes: OOB, moría la fibra), dos pegadas en una (antes: la 2ª se
  descartaba), ping→pong, fragmentado reensamblado, close de cortesía.
- **M58.2 — cliente HTTP/1.1** ✅ **COMPLETO**: nueva forma BASE **`request_bytes(method, url,
  body: bytes, headers, timeout_millis)`** — cuerpo binario y plazo TOTAL de lectura explícito
  (patrón `leer_con_plazo` de M56.4; `request`/`request_with` delegan con UTF-8 y **30 s**). De
  paso cayó un bug latente: el Content-Length contaba CARACTERES (`string.len()`), no octetos —
  mal con cuerpos no-ASCII ("café" = 4 chars, 5 octetos); el camino por bytes lo arregla de raíz.
  `Host` lleva el puerto no-default (vhosts en `:8080`; el cliente WS ya lo hacía),
  **`Accept-Encoding: gzip`** se anuncia salvo que el llamador ponga el suyo (el gunzip
  transparente de M20.10b por fin se activa de verdad), **Content-Length de la respuesta
  verificado** (menos octetos = `Err("respuesta truncada …")`; de más se recorta; no aplica a
  chunked — y el orden es correcto: se verifica ANTES de gunzip, sobre los octetos comprimidos),
  y `absolutizar` ancla a la raíz una Location relativa sin `/`. Tests nuevos (`http_cli`, toy
  servers): captura de la petición (Host:puerto + Accept-Encoding + `Content-Length: 5` para
  "café" + cuerpo entero), servidor mudo → timeout en ~400 ms, y respuesta truncada → Err.
- **M58.3 — cliente HTTP/2** ✅ **COMPLETO**, **CIERRA M58**. Helpers nuevos en `http2.ray`:
  `window_update_frame(stream, inc)` / `ping_ack(payload)` / `rst_error_code(payload)`. En los
  bucles de `http2_get` Y `grpc_call`: **flow control** — por cada DATA se devuelve la ventana
  consumida (WINDOW_UPDATE de conexión + stream) → desbloquea respuestas > 65535 octetos (antes:
  cuelgue); **ACK de PING** (sonda de vida en transferencias largas); **RST_STREAM del stream 1 =
  Err con el código** (antes: leer hasta EOF sin causa). Gotcha real: los WINDOW_UPDATE/ACK son
  **best-effort** — un peer que ya envió todo y cerró rompe la escritura (Broken pipe), y con TLS
  el error ni siquiera sale en el write (queda bufferizado en rustls y revienta en la SIGUIENTE
  lectura); el crédito es una cortesía, su fallo no invalida los datos ya recibidos. Tests
  (`http2_live_cli`, toy servers h2 a mano sobre rustls): `http2_get_respuesta_grande_con_flow_control`
  (200 000 octetos con un servidor que RESPETA la ventana — solo avanza con crédito — y exige el
  ACK de un PING a mitad antes del último chunk) y `http2_get_rst_stream_es_error_con_causa`
  (RST código 8 → Err con código). Demo `http2_len_demo.ray` (status + longitud). **M58 COMPLETO**
  (58.1–58.3). Diferidos (IDEAS §19): keep-alive/pool del cliente, multiplexado h2,
  fragmentación WS de envío, CONTINUATION.

## 63. M59 — Librerías de datos de la std (conformidad + rigor + rendimiento)

> Revisión jul 2026 (tras M58; clasificación en IDEAS §20). Las 8 librerías de datos
> (`std/json`, `std/regex`, `std/base64`, `std/hex`, `std/csv`, `std/protobuf`, `std/url`,
> `std/collections/stringbuilder`) comparten la filosofía correcta (librería raylang pura,
> errores como valores, tests golden contra referencias externas) y la lógica dura está bien
> (NFA de Thompson lineal, surrogates `\uXXXX`, RFC 4180). Los huecos son de conformidad en
> los bordes, rigor de validación y O(n²) por concatenación de strings. Cero runtime.

- **M59.1 — JSON conforme a RFC 8259 (escapes)** ✅ **COMPLETO**: (a) `unescape` acepta `\b` (backspace,
  U+0008) y `\f` (form feed, U+000C) — son escapes obligatorios de la RFC; hoy el parse
  rechaza JSON legal de terceros con "secuencia de escape no soportada". (b) `quote` emite
  `\b`/`\f` para esos caracteres y escapa TODO control < 0x20 restante como `\u00XX` — hoy
  un string con un control dentro se serializa crudo, produciendo JSON inválido que otro
  parser rechaza. Round-trip: `parse(stringify(x)) == x` para strings con cualquier control.
- **M59.2 — regex sin panic + tipo `Regex` compilado** ✅ **COMPLETO**: el parser del patrón
  devuelve `Result` (propagación con `?`; los 5 `panic` → `Err` con el mismo mensaje, más un
  OOB latente cazado: `\` colgante tras el `-` de un rango). `compile(pat) -> Result<Regex,
  string>` (el struct `Regex` envuelve el `Prog` NFA). **Los mismos seis nombres como métodos
  vía el trait `Matcher`** (`impl Matcher for Regex`) — raylang no tiene sobrecarga, pero un
  método de trait GANA a la función libre homónima en la resolución por punto (M9.1), así
  `re.find(txt)` y `find(pat, txt)` conviven; los métodos no recompilan (el `Regex` trae el
  programa; motores extraídos a `run_*`). Las funciones libres se conservan delegando en
  `must_compile` (contrato histórico: patrón malformado = panic, mismo mensaje; documentadas
  "prefer compile for user-supplied patterns"). Demo + golden `regex_cli` con los 4 errores
  como valores y la API compilada.
- **M59.3 — base64 estricto en decode** ✅ **COMPLETO**: `base64_decode` rechaza datos tras el
  relleno (`"Zg==basura"`), longitud no múltiplo de 4, relleno de más (`"A==="`) y bits
  sobrantes de la cola ≠ 0 (`"QR=="`; canónico: `"QQ=="`); `base64url_decode` rechaza la cola
  de 1 carácter y los bits sobrantes ≠ 0 (el `=` ya caía fuera del alfabeto). RFC 4648 §3.5:
  un solo encoding aceptado por payload — cierra la maleabilidad de representación bajo
  JWT/SCRAM (la firma cubre el ENCODING). Demo `base64_demo.ray` + golden `base64_cli`
  (vectores §10 + 9 rechazos); consumidores (scram/jwt/jwt_eddsa/hmac/websocket) intactos.
- **M59.4 — protobuf: negativos = error explícito** ✅ **COMPLETO**: `encode_varint` con valor
  negativo emitía octetos corruptos EN SILENCIO (el bucle `v >= 128` no entra con negativos y
  salía un solo octeto mal); ahora PANICA con mensaje claro. Se eligió panic sobre `Result`
  (decisión): cambiar el builder (`write_varint`…) a `Result` rompía todos los consumidores
  gRPC por una violación de contrato documentada, y **`write_fixed64` ya codifica negativos
  bien** (complemento a dos de 8 octetos LE, wire-correcto) — documentado como alternativa en
  el raydoc. El soporte real (varint de 10 octetos / zigzag `sint`) sigue diferido a demanda
  (IDEAS §16); el decode de un varint negativo de 10 octetos también. Test del panic en ambos
  motores en `protobuf_cli`.
- **M59.5 — StringBuilder + O(n) en los hot paths** ✅ **COMPLETO**. json/csv/hex/base64
  construían con `s = s + …` por carácter (cada `+` copia lo acumulado → O(n²)). Migrados a
  `StringBuilder` (push O(1) amortizado + join final): `parse_str_raw`/`quote`/`stringify_*`
  (json), `write_csv` (csv; `write_field` vía `replace` nativo), `hex_encode`, `base64`
  (`to_url` vía cadena de `replace`). **Dos gotchas de rendimiento del lenguaje descubiertos
  midiendo** (documentados en los comentarios): (1) `substring(i, j)` y (2) `s[i]` indexan por
  CARÁCTER sobre UTF-8 y escanean desde 0 → son **O(i)**, así que "copiar por rachas con
  substring" y "cursor de parser sobre el string" son cuadráticos — el primer intento con
  rachas salió PEOR que el original. Fixes: push por carácter al builder, y el parser de json
  tokeniza sobre **`[char]`** (`P.cs`, indexación O(1); `lit`/`parse_number` sin substring).
  El parser de csv conserva `field + c` (cuadrático solo en el largo del CAMPO, cortos).
  **Medidas** (release, misma máquina; ~140k chars json / 4000 filas csv / 60k octetos):
  json parse **10 096 → 87 ms (116×)**, json quote 329 → 46 ms, base64url **3 527 → 32 ms
  (110×)**, hex encode 360 → 43 ms, base64 171 → 32 ms, csv write 158 → 64 ms, csv parse
  igual (46 → 50 ms, sin tocar). `clear()` en StringBuilder no hizo falta.
- **Decisión aparte (pre-1.0, con el usuario)**: unificar `[int]` → `bytes` en hex/base64/
  PbWriter (APIs pre-M16); rompe ~6 consumidores (jwt, scram, crypto…).

## 64. M60 — `bytes` en las fronteras de la std (unificación `[int]` → `bytes`)

> Cierra la "decisión aparte" de M59 (§63), decidida con el usuario: pre-1.0 es la ventana de
> rotura. Diagnóstico: `std/crypto` (ring, M43) ya hablaba `bytes` de punta a punta; el mundo
> `[int]` sobrevivía SOLO en `std/hex`/`std/base64` (APIs pre-M16) y en los shims `*_octets` de
> `net/crypto`, que existían únicamente para traducir entre ambos. La migración mayormente
> BORRA conversiones. Protobuf no necesitó cambio (su `[int]` es interno; la superficie ya era
> `bytes`).

- **APIs**: `hex_encode(bytes) -> string`, `hex_decode -> Result<bytes, string>`,
  `base64/base64url(bytes) -> string`, `base64_decode/base64url_decode -> Result<bytes,
  string>`. Por dentro siguen acumulando `[int]` + `bytes_of` al final (a `bytes` no se le
  hace push); indexar `bytes` ya da `int` → los cuerpos casi no cambian.
- **`net/crypto` queda reducido a los helpers hex** (`sha256_hex`/`hmac_sha256_hex`, que ya
  eran bytes-nativos; los usa sigv4). Retirados: `octetos`, `sha256_octets`, `sha1_octets`,
  `hmac_sha256_octets`, `ed_public_key`, `ed_sign`, `ed_verify`.
- **Consumidores de producto** migrados a `std/crypto` directo: `jwt` (cae `str_octets`;
  `base64url(crypto.hmac_sha256(...))` sin conversión), `jwt_eddsa` (**API rota a propósito**:
  `seed`/`pubkey` pasan de `[int]` a `bytes`; seed inválida → segmento de firma vacío),
  `scram` (**100% bytes**: `Scram.server_sig: bytes`, `xor_bytes`/`pbkdf2_sha256` sobre bytes
  — desaparecieron TODAS las `bytes_of`/`octetos`), `websocket` (`crypto.sha1` directo),
  `websocket_client` (`base64(bytes_of(nonce))`), `sigv4` (`crypto.hmac_sha256` directo,
  borra una doble conversión), `uuid` (frontera `bytes_of`).
- **Los espejos pedagógicos** (`examples/web/jwt.ray`/`jwt_eddsa.ray`/`scram.ray`/
  `websocket.ray`) conservan la cripto pura en `[int]` (es su representación didáctica) y
  convierten con `bytes_of` SOLO en la frontera hex/base64; los demos (`chacha20`, `poly1305`,
  `ed25519`, `sha512`, `huffman`, `hmac`) igual.
- Sin cambios de golden: todas las salidas son idénticas; las suites de consumidores cubren
  la migración (jwt, jwt_eddsa, scram, websocket, hmac, sha256/512, base64, uuid, cli, oauth2,
  postgres/mongo vía scram).

## 65. M61 — El prelude de producción (hash, sort, ergonomía)

> Revisión jul 2026 (tras M60; clasificación en IDEAS §21). Dos defectos verificados con el
> binario + la ergonomía de Option/Result. Todo front-end puro (prelude + codegen del derive).

- **M61.1 — hash sin overflow** ✅ **COMPLETO**: el `int` de raylang es **checked** (desbordar = trap), pero el
  hash clásico `h = h*31 + c` asume **wrapping** → `Hash for string` panica con ≥ ~12 chars
  (mata `Set<string>` con claves reales) y el combinador de `@derive(Hash)` panica con un campo
  de valor grande (p. ej. un int de 4×10¹⁷). Fix: acumular acotado a 32 bits — `h = (h*31 + x) &
  4294967295` (producto máx ~2³⁷ ≪ i64::MAX) — en el impl del prelude y en `generate_derives`
  (que además enmascara los hashes ENTRANTES de los campos, que pueden ser cualquier i64).
  Tests: hash de string largo, Set<string> con claves reales, derive con int grande (ambos
  motores).
- **M61.2 — `sort` O(n log n)** ✅ **COMPLETO**: el insertion sort del prelude era O(n²). Reemplazo
  por **merge sort bottom-up** en raylang puro (front-end, cero runtime; sin recursión — anchos
  1, 2, 4, … fusionando pares de tramos), ESTABLE (el tramo derecho solo adelanta si es
  estrictamente menor; verificado con claves repetidas: `1b1d2a2c2e`). **Medido (release,
  20 000 ints aleatorios): 22 960 → 98 ms (234×)**. `sort_desc`/`dedup`/`binary_search`/`merge`
  de `std/sort` lo heredan. Test de estabilidad + bordes en `collections_cli` (ambos motores);
  el prelude del self-hosting conserva su insertion sort (oráculo conductual: mismo output).
- **M61.3 — ergonomía de Option/Result + Eq/Show de bytes/arreglos** ✅ **COMPLETO**. Métodos vía
  **traits** (`OptionOps<T>`/`ResultOps<T,E>` con impls genéricos, M9.2b), NO funciones libres:
  así los MISMOS nombres existen para ambos tipos sin sobrecarga (el despacho por punto resuelve
  por el receptor, como el `Matcher` de regex/M59.2). Option: `is_some`/`is_none`/`unwrap_or`/
  `expect(msg)`/`unwrap`/`ok_or<E>`/`map<U>`; Result: `is_ok`/`is_err`/`unwrap_or`/`expect`/
  `unwrap`/`ok`. Además `impl Eq/Show for bytes` (`==`/`to_string` ya operaban), `impl<T: Eq> Eq
  for [T]` (elemento a elemento — `==` sobre `[T]` RÍGIDO no tipa, gotcha) e `impl<T: Show> Show
  for [T]` (`"[a, b]"`; diccionarios anidados → `[[int]]` funciona), y `sum_float`. Colisión
  auditada: `mapa.ray` definía su propio `unwrap_or` libre llamado por UFCS — ahora gana el
  método del trait con la misma semántica (su fn queda muerta). El pipeline auto-alojado no
  conoce OptionOps → allí ese mismo call resuelve por UFCS a la fn local (oráculo conductual
  intacto). Test `option_result_ergonomia_oraculo` (vm.rs) + 15 suites verdes.
- **Fuera del arco** (idea aparte, IDEAS §21): posición-del-llamador para `assert`/`panic`
  (hoy un assert fallido reporta la línea del prelude) — exige diseño de runtime (stack trace
  o intrinsic de posición).

## 66. M62 — Iteradores de producción (el camino eager + documentación)

> Revisión jul 2026 (tras M61; clasificación en IDEAS §22). La correctitud del trait
> `Iterator`/`Iter` (M40.2) es impecable y ambos motores coinciden en toda la batería; los
> hallazgos son de rendimiento, medidos con 1M de elementos en release: while 107 ms · `for`
> 112 ms · eager `xs.map(f).fold(…)` 36 441 ms (¡340×!) · lazy `iter().map().fold()` 13 799 ms ·
> eager como bucle directo 317 ms.

- **M62.1 — las funciones libres eager vuelven a ser bucles directos** ✅ **COMPLETO**: M40.6 re-fundó
  `map`/`filter`/`fold` (las libres sobre arreglos, la API ergonómica) sobre la maquinaria
  perezosa por "única fuente de verdad" — estético, pero le cargó al camino MÁS USADO el coste
  íntegro de la abstracción (closure + `Option` en el heap del GC + match POR ELEMENTO, más el
  `collect` intermedio). Vuelven a bucles `while` directos: **36 441 → 319 ms medidos (114×)**,
  semántica idéntica (los tests existentes la fijan). La maquinaria lazy (métodos del trait) no
  cambia: sigue siendo la forma de FUSIONAR cadenas y cortar trabajo (`take` temprano). De paso:
  terminales `any`/`all`/`count` en el trait (con cortocircuito; verificado sobre una cadena de
  1M sin colgarse) + `any`/`all` eager sobre arreglos (mismo nombre, despacho por receptor).
  Oráculo `any_all_count_oraculo` (vm.rs); lib (542) + 8 suites verdes.
- **M62.2 — documentación de la semántica fina** ✅ **COMPLETO**, **CIERRA M62**: (a) `for x in xs` CONGELA la longitud al
  entrar (ambos motores, verificado) pero `for x in xs.iter()` es una vista VIVA (el `step`
  relee `xs.len()`) — mutar el arreglo durante la iteración da resultados distintos entre
  formas; (b) aliasing one-shot (dos adaptadores sobre el mismo `iter()` comparten el avance);
  (c) `zip` descarta el elemento ya consumido del lado largo al agotarse el corto (como Rust).
  Hecho en el libro (m40/iteradores): la sección "Una sola fuente de verdad" se reescribió como
  "…y su precio (M40.6 → M62.1)" — cuenta el arco estética→medición→reversión con los números, y
  la moraleja fina (la pereza CORTA trabajo, no acelera el que sí se hace: ~6 µs/`next()`); nueva
  sección "La letra pequeña de la semántica" (congelado-vs-vivo, one-shot/aliasing, el hueco de
  `zip`); inventario de terminales actualizado (any/all/count/sum_float).
- **Idea aparte** (IDEAS §22, runtime): abaratar el `next()` (~6 µs/elemento hoy) — `Option`
  sin alocación (representación inline/sentinela) o devirtualización del `step`. Se hará cuando
  el throughput lazy importe de verdad; el techo actual no bloquea (el camino eager queda en
  ~317 ms/1M tras 62.1).

## 67. M63 — std/toml de producción (conformidad del subconjunto)

> Revisión jul 2026 (tras M62; clasificación en IDEAS §23). `ray.toml` NO pasa por aquí
> (`src/manifest.rs` es un lector Rust aparte, deliberadamente — circular si no): std/toml es
> la librería de configs del USUARIO. El subconjunto está documentado con honestidad; los
> hallazgos son conformidad DENTRO del subconjunto: lo que acepta debe ser TOML, y el TOML
> legal de esa área no debe corromperse en silencio.

- **M63.1 — strings conformes** ✅ **COMPLETO**: (a) escapes completos de la spec — `\b \f \n \r \t \" \\` +
  `\uXXXX`/`\UXXXXXXXX` (vía `char_from_code`, como json M59.1) — y **escape desconocido =
  `Err`** (hoy `"café"` → `"cafu00E9"`: traga la barra y corrompe en silencio); (b)
  **strings literales `'...'`** (core TOML, sin escapes — rutas Windows, regex).
- **M63.2 — números conformes** ✅ **COMPLETO**: separadores `_` (`1_000`, lo más común en configs; validar
  posición: entre dígitos), `inf`/`nan`/`+inf`/`-inf`. Hex/octal/binario: diferido a demanda.
- **M63.3 — rigor del documento** ✅ **COMPLETO**, **CIERRA M63**: clave duplicada = `Err` (hoy se aceptan ambas y `toml_get`
  devuelve la PRIMERA), cabecera `[]` vacía = `Err` (hoy resetea a raíz en silencio), y salto
  de línea obligatorio tras cada valor (`a = 1 b = 2` hoy pasa como dos entradas).
- **Diferidos** (documentados en el módulo): inline tables, `[[…]]`, fechas, multilínea.
- Gotcha cazado por la propia sonda de 63.3: `expect_eol` debía aceptar el `\r` de un CRLF
  como fin de línea (si no, todo documento con finales Windows fallaba).

## 68. M64 — Compresión de producción (inflate robusto)

> Revisión jul 2026 (tras M63; clasificación en IDEAS §24). `deflate` (encoder) y `huffman`
> (HPACK) salieron bien parados; `inflate` tiene el algoritmo correcto (port de puff.c, CRC
> verificado) pero CERO robustez ante input corrupto — y descomprime datos EXTERNOS (el gunzip
> transparente del cliente HTTP, M58.2). Verificado: `inflate_raw(b"")` mata el programa con
> "índice fuera de rango" en vez de devolver `Err`.

- **M64.1 — inflate robusto (corrupto = `Err`, nunca crash) COMPLETO**: el bit-reader pasa a `Result`
  (`read_bit`/`read_bits` con bounds; `decode` propaga con `?` y conserva el -1 de código
  inválido dentro del `Ok`); `stored_block` valida LEN/NLEN contra lo que queda (y **verifica
  NLEN = ~LEN**, que puff comprueba y aquí faltaba); la cabecera de `gunzip` acota FNAME/
  FCOMMENT/FEXTRA/FHCRC (xlen del atacante ya no salta fuera) y exige que quede el tráiler;
  `dynamic_block` acota hlit ≤ 286 / hdist ≤ 30 y rechaza que las repeticiones 16/17/18
  rebasen hlit+hdist (como puff); zlib rechaza **FDICT** (diccionario preestablecido, no
  soportado — antes producía basura). API pública intacta (`Result<bytes, string>`).
  **Bonus: ICE de la VM cazado y arreglado** — las sondas de corrupción destaparon un bug
  LATENTE del core: el `Return` que baja `?` ocurre en mitad de una expresión
  (`v | read_bit(s)? << i`) y dejaba los operandos pendientes huérfanos en la pila de la
  fibra; el siguiente Call del llamador con argumentos ya apilados los leía corridos →
  `unreachable!` en el dispatch binario (o divergencia silenciosa). Fix: `CallFrame` gana
  `stack_base` (profundidad de la pila de operandos a la entrada) y `Return` **trunca a esa
  base** antes de entregar el valor. Test de regresión `try_err_con_operandos_pendientes_oraculo`
  (verificado: falla sin el fix). Goldens de robustez en `inflate_demo.ray`/`tests/inflate_cli.rs`
  (8 sondas de corrupción + round-trip de control, ambos motores).
- **M64.2 — límite de salida (anti-bomba) COMPLETO**: `inflate_raw_limit`/`gunzip_limit`/
  `zlib_inflate_limit(data, max_out)` — la salida acumulada se comprueba contra el tope en
  los TRES emisores (literal, tramo LZ77 y bloque almacenado; el chequeo va ANTES de copiar,
  así unos KB maliciosos no llegan a materializar GB); las formas sin límite delegan con
  **64 MiB** por defecto (`default_limit`); el cliente HTTP (`http.ray`, ambos espejos) usa
  `gunzip_limit(..., 64 MiB)`. Patrón `read_message_limit` del WebSocket (M58.1). Goldens:
  tope menor que la salida = Err, tope justo = round-trip OK.
- **M64.3 — menores DIFERIDOS a demanda**: validación de Kraft de los árboles (no crítica:
  un árbol sobresuscrito acaba en código inválido → `Err`, cubierto por M64.1), trie de
  HPACK reusable y crc32 con tabla (perf, no corrección). **M64 COMPLETO** (64.1 + 64.2).

## 69. M65 — std/math de producción

> Revisión jul 2026 (tras M64; clasificación en IDEAS §25). Verificado sano: dominios float
> totales IEEE (NaN/-inf/inf, sin traps), `round` ties-away-from-zero, `gcd`/`lcm` (divide
> antes de multiplicar) / `is_prime`, `float_bits` total. Dos defectos reales + huecos.

- **M65.1 — fixes de corrección COMPLETO**: (a) `ipow` — la exponenciación binaria hacía el cuadrado
  final `b = b*b` que ya no necesita; con el int checked eso trap-ea con resultados que CABEN
  (`ipow(2, 40)` = 1.1e12 reventaba por 2^64). Fix: solo cuadrar cuando quedan iteraciones.
  (b) `min`/`max` — la doc dice "Ties return `a`" pero el código devolvía `b` (observable con
  `impl Ord` de usuario). Fix: comparación invertida (empate → `a`).
- **M65.2 — trig inversa y compañía COMPLETO**: `asin`/`acos`/`atan`/`atan2`/`log2`/`trunc` como
  builtins (fila en `BUILTINS` + opcode + impl por motor + envoltorio `__x` en std/math,
  patrón M11.4). `atan2` es el hueco doloroso (ángulo desde coordenadas).
- **M65.3 — menores COMPLETO**: `clamp<T: Ord>` genérica (retrocompatible) + documentar la frontera
  del int checked (`factorial(n ≥ 21)` / `ipow` con resultado > 2^63-1 = trap).
  **M65 COMPLETO** (65.1 + 65.2 + 65.3).

## 70. M66 — std/text de producción

> Revisión jul 2026 (tras M65; clasificación en IDEAS §26). Corrección verificada SANA
> (reverse por carácter con UTF-8 astral, capitalize no-ASCII, count no-solapado, pads por
> carácter). Un solo paso: dos O(n²) + dos huecos de superficie.

- **M66 — de una pieza COMPLETO**: (a) `reverse` deja de concatenar `out + char` en bucle (O(n²);
  100k chars = 167 ms) → acumula `[string]` y un `join` final (O(n)). (b) `count` deja de
  re-materializar el resto con `substring` por ocurrencia (O(n²) por el `substring` O(i) de
  M59.5; 100k chars / 10k ocurrencias = 879 ms) → busca sobre `[char]` con offset propio.
  (c) `words` separa por **whitespace** (espacio/tab/`\n`/`\r`), no solo espacio — el
  contrato universal de split_whitespace. (d) **`lines(s)`** nueva: parte por `\n` y trata
  `\r\n` (el `\r` final se recorta); toml/csv/http la hand-rolleaban.

## 71. M67 — std/fs de producción (directorios y metadatos)

> Revisión jul 2026 (tras M66; clasificación en IDEAS §27). Sano: errores como valores,
> list_dir determinista, exists total, handles, I/O binaria. El hallazgo: el módulo era
> **solo-archivos** — sin mkdir/is_dir/rename, los directorios eran de solo lectura y la
> escritura atómica imposible.

- **M67 — de una pieza COMPLETO**: 8 primitivos nuevos (patrón M11.4; helpers `fs_*` compartidos en
  `builtins.rs` que devuelven el arreglo etiquetado ya montado): `__mkdir` (create_dir_all),
  `__is_dir`/`__is_file` (totales → bool), `__rename`, `__copy_file`, `__remove_dir` (solo
  vacío; el recursivo es peligroso → a demanda), `__file_size` (`["ok", n]` como `__open`),
  `__append_file_bytes`. Envoltorios `Result` en `std/fs` (`mkdir`/`is_dir`/`is_file`/
  `rename`/`copy_file`/`remove_dir`/`file_size`/`append_file_bytes`). Con `rename` queda
  habilitado el patrón de **escritura atómica** (temp + rename). Tests: integración por
  subproceso en `io_cli` (I/O real no determinista → no oráculo), ciclo completo
  mkdir → write → is_dir/is_file → file_size → copy → rename → remove.

## 72. M68 — aleatoriedad de producción

> Revisión jul 2026 (tras M67; clasificación en IDEAS §28). Sano: SplitMix64 canónico, Mutex
> de proceso (M:N seguro), below total, honestidad "no criptográfico" por doquier; el sesgo
> de módulo (~n/2^64) es inmedible. Dos huecos: reproducibilidad y secretos.

- **M68.1 — seed + kit COMPLETO**: primitivo `__random_seed(n)` (fija el estado del SplitMix64 —
  misma semilla, misma secuencia, en ambos motores comparten el PRNG del host) y, en raylang
  puro sobre `below` (cero opcodes): `between(lo, hi)` (ambos inclusive; `hi < lo` → `lo`),
  `choice(xs) -> Option<T>` (vacío → None) y `shuffle(xs)` (Fisher-Yates, **in place** por la
  semántica de referencia). Con `seed`, el golden de aleatoriedad es **determinista**.
- **M68.2 — aleatoriedad criptográfica COMPLETO**: `crypto.random_bytes(n) -> bytes` (primitivo
  `__crypto_random_bytes` sobre `ring::rand::SystemRandom`, dep ya presente desde M43).
  Los `///` de uuid/websocket apuntan a ella para quien necesite no-predecible.
  **M68 COMPLETO** (68.1 + 68.2).

## 73. M69 — cliente Redis de producción (framing en octetos)

> Revisión jul 2026 (tras M68; clasificación en IDEAS §29). Estructura sana; el defecto:
> RESP exige longitudes en OCTETOS y el cliente contaba CARACTERES (todo `string`) —
> `SET k ñ` declaraba `$1` y enviaba 2 octetos → desincronización del protocolo con
> cualquier valor no-ASCII; binarios imposibles.

- **M69 — de una pieza COMPLETO**: (a) framing interno 100% **`bytes`** (la migración M60 aplicada
  aquí): `Conn.buf: bytes`, `socket_read_bytes`/`socket_write_bytes` (M16.1c), `sub_bytes`,
  `encode_command -> bytes` con `to_bytes().len()`. API pública conservada: `command(c,
  args: [string])` codifica por dentro; `Reply.Str` sigue `string` (decodifica UTF-8, `?`
  si no). (b) longitudes RESP malformadas (`$basura`/`:abc`) = `Err` claro (antes `int_or`
  → 0 en silencio). (c) tope de bulk (64 MiB, como M64.2): `$<gigante>` = `Err`, no
  agotamiento. Espejos `packages/net/redis.ray` ↔ `examples/web/redis.ray` juntos.

## 74. M70 — observabilidad de producción (log + metrics)

> Revisión jul 2026 (tras M69; clasificación en IDEAS §30). Sano: render determinista y
> testeable en ambos, labels escapados/ordenados, histograma cumulativo correcto. Tres
> defectos: JSON inválido con controles, HELP sin escapar, tipo de métrica sin validar.

- **M70 — de una pieza COMPLETO**: (a) el `json_escape` del log cubre **todo control < 0x20** →
  `\uXXXX` (RFC 8259, clase M59.1; antes BEL/ESC/\x00 en el mensaje emitían JSON inválido,
  verificado); además `\b`/`\f` con su escape corto. (b) metrics escapa el texto de
  `# HELP` (`\\` y `\n`, como exige el formato de exposición). (c) `add`/`set`/`observe_l`
  validan el TIPO de la métrica (counter/gauge/histogram) y panican con mensaje claro si no
  cuadra — antes creaban una serie espuria que corrompía la exposición en silencio.
  Espejos `packages/net` ↔ `examples/web` juntos.

## 75. M71 — cookies seguras (inyección de cabeceras + SameSite)

> Revisión jul 2026 (tras M70; clasificación en IDEAS §31). oauth2 sano; sigv4 sólido (un bug
> de firma menor por espacios, diferido). El defecto real: `net/cookie` era vulnerable a
> **inyección de cabeceras** — un `\r\n` en el nombre o el Path partía la respuesta HTTP
> (response splitting; verificado: `Set-Cookie: admin=true` inyectado).

- **M71 — de una pieza COMPLETO** (sigv4: espacios DIFERIDO, IDEAS §31): (a) `set_cookie` rechaza (devuelve `Result`) o sanea los caracteres
  de control (`\r`/`\n`) y separadores (`;`) en el **nombre** y el **Path** — el valor ya
  estaba protegido (url-encode). Decisión con el usuario: **saneo silencioso** (quitar los
  caracteres peligrosos) vs `Result` en la API — se elige el que rompa menos consumidores. (b)
  `with_same_site(v)` (`"Strict"`/`"Lax"`/`"None"`, validado; `None` implica `Secure`) — el
  atributo anti-CSRF de facto que faltaba junto a HttpOnly/Secure. (c) sigv4: fichado el
  colapso de espacios internos en cabeceras canónicas (bug de firma, no seguridad) — se
  arregla aquí si es barato, o queda en IDEAS §31. Espejos `packages/net` ↔ `examples/web`.

## 76. M72 — cliente DNS de producción (robusto + anti-spoofing)

> Revisión jul 2026 (tras M71; clasificación en IDEAS §32). udp/dns_cache sanos. El parser
> DNS procesa datos EXTERNOS (resolución de nombres) y tenía la clase inflate pre-M64 (cero
> bounds → crash con respuesta truncada, verificado) MÁS un DoS (puntero de compresión cíclico
> → bucle infinito, verificado) MÁS spoofing (txid fijo 0x1234).

- **M72.1 — parser robusto COMPLETO (corrupto = `Err`, nunca crash ni cuelgue)**: helpers `be16`/`be32`
  con bounds (fin del mensaje = `Err`); `read_name` con **límite de saltos de puntero** (un
  puntero 0xC0 que cicla o una cadena larga = `Err`, no bucle infinito) y bounds en cada
  label; `parse_full` valida que cada RR (type/ttl/rdlength/rdata) quepa antes de leerlo;
  `format_ipv4`/`format_ipv6`/`read_txt`/MX/CNAME/SRV acotados. Un `read_name` que puede
  fallar devuelve `Result` y se propaga con `?`. El error es un valor, como en M64.1.
- **M72.2 — anti-spoofing COMPLETO** (CIERRA M72; la revisión en frío de net queda completa): `query_full` genera el **transaction ID aleatorio** con
  `crypto.random_bytes` (M68.2; 16 bits del CSPRNG, impredecible) y **valida** que el ID de la
  respuesta coincida con el de la consulta (una respuesta con otro ID = `Err`, no se acepta).
  Cierra el spoofing off-path trivial que el ID fijo 0x1234 permitía.

## 77. M73 — cliente gRPC / HTTP/2 de producción (anti-bomba + grpc-status)

> Revisión jul 2026 (tras M72; clasificación en IDEAS §33). Sano: framing, flow control,
> PING/RST/GOAWAY (M58.3). Dos defectos: acumulación sin cota (bomba de memoria) y una
> variable muerta que silencia un error de protocolo.

- **M73 — de una pieza COMPLETO**: (a) tope de respuesta acumulada (64 MiB, como M64.2) en `body` de
  `grpc_client` Y `http2_client` (el DATA que lo rebasa = `Err`, no agotamiento); tope de
  frame individual (rechazar un frame cuyo `frame_size` declara más que un máximo razonable,
  antes de esperar sus octetos). (b) `grpc_call` usa `tuvo_grpc_status` (hoy muerto): si la
  respuesta no trae `grpc-status` en los trailers (protocolo gRPC violado o trailers
  truncados) = `Err`, en vez de devolver `Ok(grpc_status: 0)` indistinguible de un OK.
  Espejos `packages/net` ↔ `examples/web` juntos (grpc + http2_client).

## 78. M74 — JWT de producción (validación del algoritmo del header)

> Revisión jul 2026 (tras M73; clasificación en IDEAS §34). `net/jwt` (HS256) y `net/jwt_eddsa`
> (EdDSA). Bien pensado: verificar-antes-de-usar, `const_eq` en tiempo casi constante, base64url
> estricto (M59.3) y —clave— **ambos verificadores recomputan con el algoritmo hardcodeado**, así
> que `alg:none`/confusión de algoritmo YA fallan (fail-closed; verificado con un token forjado).
> Un solo hallazgo: el campo `alg` del header nunca se valida — defecto canónico de JWT, hoy no
> explotable pero frágil ante un refactor y ante tokens de otro propósito.

- **M74 — de una pieza COMPLETO**: validación explícita del `alg` (defensa en profundidad). Un
  helper `header_alg` decodifica el header (`base64url_decode` → `from_utf8` → `std/json::parse`)
  y exige que la claim `alg` sea una cadena; `jwt_verify` exige `"HS256"` y `jwt_eddsa_verify`
  `"EdDSA"`, rechazando con mensaje claro (`algoritmo no soportado: se esperaba …`) ANTES de dar
  por buena la firma. Regresión: los demos forjan un `alg:none` y comprueban el rechazo por ambos
  motores (`jwt_demo`/`jwt_eddsa_demo`). Espejos `packages/net` ↔ `examples/web` juntos (el espejo
  usa la crypto pedagógica `hmac`/`ed25519` + `from json`, pero la misma lógica de validación).
  Diferido (documentado, no defecto): `jwt_eddsa_sign` con seed de longitud errónea emite firma
  vacía (degradación honesta); `exp`/`nbf` = política de la aplicación sobre el JSON devuelto.

## 79. M75 — SCRAM-SHA-256 de producción (RFC 5802/7677: escapado, nonce, tope de iteraciones)

> Revisión jul 2026 (tras M74; clasificación en IDEAS §35). `net/scram`, el mecanismo de auth que
> reusan `db/postgres`, `net/postgres` y `db/mongo`. Núcleo sano (bytes de punta a punta, PBKDF2
> correcto, verify en tiempo constante). Cuatro huecos frente a la RFC.

- **M75 — de una pieza COMPLETO**: (a) **escapado del nombre de usuario** (RFC 5802 §5.1) —
  `escape_saslname` en `scram_first` (`=` → `=3D` primero, luego `,` → `=2C`; el orden evita
  re-escapar el `=` que introduce `=2C`) cierra la inyección de atributos vía un usuario con
  `,`/`=`. (b) **verificación del nonce del servidor** (RFC 5802 §5.1, MUST) — `scram_final` exige
  que el `r=` del server-first empiece por el nonce del cliente (que vive en `client_first_bare`),
  si no `Err` (replay/MITM). (c) **tope de iteraciones** — `1 <= i <= 10_000_000` (bomba de CPU si
  el servidor manda un `i` enorme; no se impone el mínimo 4096 de la RFC para no romper el toy-server
  a i=64). (d) **guarda de `server_sig` vacío** en `scram_verify` (sin ella, un `v=` vacío casaría a
  longitud 0). Regresión rápida `scram_reject_demo` (todos los casos retornan antes del PBKDF2) por
  ambos motores; el vector RFC 7677 (`scram_cli`, `#[ignore]`) sigue byte-idéntico. Espejos
  `packages/net` ↔ `examples/web` juntos (el espejo usa la crypto pedagógica `hmac`/`sha256`).

## 80. M76 — clientes de BD de producción (MongoDB + PostgreSQL: anti-bomba, profundidad, NULL)

> Revisión jul 2026 (tras M75; clasificación en IDEAS §36). Parsers de wire de `db/bson`,
> `db/mongo`, `db/postgres` y `net/postgres` (legacy). Misma clase que Redis (M69)/DNS (M72):
> datos del servidor sin validar. `bson.decode` acotaba bien sus reads; los huecos estaban en la
> profundidad de recursión y en el framing de mensajes/filas.

- **M76 — de una pieza COMPLETO** (bson_cli 3, mongo_cli 3, postgres_cli 2, postgres_v2_cli 2):
  - **bson**: tope de anidamiento (`max_depth`=200; supera el ~100 propio de MongoDB y queda muy por
    debajo de la pila de raylang). `rd_doc`/`rd_value` llevan `depth`; un doc/arreglo demasiado
    anidado = `Err` en vez de "desbordamiento de pila". Verificado: 600 niveles (~4.8 KB) → `Err`.
  - **mongo**: `read_msg` acota el `total` del header a `max_message`=64 MiB (supera el
    `maxMessageSizeBytes` de 48 MB de MongoDB); rechazo al leer la cabecera → no acumula la bomba.
  - **postgres (db + net legacy)**: `read_msg`/`pg_read` con el mismo tope; además `mlen >= 4`
    (evita un `total < 5` que dejaba `sub_bytes` con fin < inicio). `parse_datarow` pasó a
    `Result` y (1) reinterpreta la longitud de columna como **int32 con signo** — el NULL (−1 =
    0xFFFFFFFF) lo leía `be32` sin signo como 4294967295 y `sub_bytes` reventaba el cliente ante
    **cualquier NULL** (bug vivo); (2) valida los límites del payload (columna truncada = `Err`).
    Regresión: una columna NULL del toy-server llega como "" (`postgres_v2_cli`).
  - Sin espejos (los paquetes `db`/`net` no son embebidos ni tienen twin en `examples/`).
  - Diferido a **M77**: `mysql` (`lenc_int` sin chequeo de límites → OOB en paquete truncado + posible
    overflow del i64 en el caso de 8 octetos; sin bomba ilimitada, el largo de paquete son 3 octetos
    ≤ 16 MB) y `sqlite` (fichero local → otro modelo de amenaza). **[Hecho en M77.]**

## 81. M77 — clientes de BD de producción (MySQL: endurecimiento del parseo; SQLite: sano)

> Revisión jul 2026 (tras M76; clasificación en IDEAS §37). Cierra el cluster `packages/db`.
> `sqlite` es sano (rusqlite parsea el formato en C; params enlazados; errores como valores → nada
> que arreglar). `mysql` tenía lecturas OOB pervasivas ante paquetes malformados del servidor.

- **M77 — refactor de endurecimiento COMPLETO** (mysql_cli 4, y el resto del cluster sin regresión):
  el parseo de mysql pasa a ser **robusto ante datos del servidor no confiables**. Accesor
  `at(p, i) -> Result` con chequeo de límites; los ~10 helpers de decodificación
  (`lenc_int`/`nul_str`/`int_le`/`int_cell`/`dec_date`/`dec_datetime`/`dec_time`/`col_type_flags`/
  `bin_cell`) devuelven `Result` y propagan con `?` por las DOS rutas de fila (texto COM_QUERY +
  binaria preparada) y por el handshake. `lenc_int` de 8 octetos se arma por mitades (hi/lo) y
  rechaza longitudes >= 2^32 → sin desbordar el i64 ni el OOB posterior. `read_packet` rechaza un
  paquete de carga vacía (cierra de golpe los `p[0]` de todos los consumidores). `stmt_prepare`
  valida los 12 octetos fijos del OK. Un paquete truncado/malformado ahora = `Err` como valor, no un
  trap que tumbe al cliente. Verificado: una fila con un length-encoded truncado = `Err`
  (`mysql_cli`, rama TRUNC). El NULL binario ya se manejaba (bitmap) → no había bug vivo (a
  diferencia del NULL de Postgres, M76). Threat model: servidor malicioso; trapeaba seguro (sin
  corrupción/RCE) y estaba acotado a 16 MB (largo de paquete de 3 octetos). **Cluster db CERRADO**
  (M76 mongo/postgres + M77 mysql/sqlite). Sin espejos (los paquetes `db` no son embebidos).
