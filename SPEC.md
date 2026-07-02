# Especificación del lenguaje raylang

**Versión del lenguaje: 1.0.0-beta.1** (esta especificación versiona con el lenguaje; ver §12).

Este documento es **normativo**: define qué es un programa raylang válido y qué hace. Los otros
dos documentos del proyecto no lo son: [DESIGN.md](DESIGN.md) es la *crónica de diseño* (el
porqué de cada decisión, fase a fase) y el libro (`book/`) es la *pedagogía* (cómo se construyó).
Ante un conflicto, manda esta SPEC; un conflicto entre la SPEC y la implementación es un bug de
una de las dos y debe resolverse explícitamente.

**Conformidad.** raylang tiene dos motores (intérprete de árbol y máquina virtual de bytecode)
que deben producir **comportamiento observable idéntico** (stdout, errores, código de salida)
para todo programa determinista; la suite lo verifica por oráculo cruzado. Las excepciones están
acotadas y listadas: la concurrencia (§9) y la E/S asíncrona solo existen en la VM (el intérprete
da un error limpio), y el intérprete es el oráculo *secuencial*.

Notación de gramática: EBNF con `{ x }` = cero o más, `[ x ]` = opcional, `|` = alternativa,
`'x'` = literal. Las producciones léxicas (§1) operan sobre caracteres; las sintácticas (§2, §4–
§6) sobre tokens.

---

## 1. Léxico

La entrada es texto **UTF-8** (una entrada con UTF-8 inválido se rechaza al leer el archivo). El
lexer produce tokens; cada token lleva posición 1-basada `(línea, columna)` y su longitud en
caracteres. **Ningún token cruza líneas.**

- **Comentarios**: `//` hasta el fin de línea. No hay comentarios de bloque.
- **Identificadores**: `[A-Za-z_][A-Za-z0-9_]*`, excluidas las palabras clave. Los nombres con
  `#` o `::` no son escribibles por el usuario (los usan las bajadas internas y los módulos).
- **Palabras clave** (reservadas):
  `let var fn return if else while for in true false struct const enum match trait impl dyn
  pub import from extern as` y las de tipo `int float bool string char bytes u8 u32 u64`.
- **Literales**:
  - *Entero*: dígitos decimales (`42`). Debe caber en `int` (i64); si no, error léxico. No hay
    literales hex/octales/binarios ni separador `_` (diferido).
  - *Flotante*: `dígitos '.' dígitos` (`3.14`). Un `.` sin dígito decimal no es flotante.
  - *Cadena*: `"…"` con escapes `\n \t \r \\ \" \$`. No admite saltos de línea literales.
  - *Cadena interpolada*: cualquier cadena `"…${expr}…"`. `${expr}` contiene **una** expresión; el
    `$` solo es especial seguido de `{` (`"$5"`, `"{n}"` son literales; `\${` es un `${` literal).
    Azúcar: se desazucara a concatenación con `to_string(expr)` (§6.6).
  - *Carácter*: `'a'` con escapes `\n \t \r \\ \'`. Un code point Unicode.
  - *Bytes*: `b"…"` con los escapes de cadena más `\xNN` (octeto arbitrario, dos dígitos hex).
  - *Booleano*: `true` / `false`.
- **Operadores y puntuación**: `+ - * / % == != < <= > >= && || ! = & | ^ ~ << >> ( ) { } [ ]
  , ; : . .. -> => ? |> @`. El lexer siempre emite `>>` como un token; el parser lo **parte**
  en dos `>` al cerrar argumentos de tipo (`Caja<Caja<int>>` es válido).

## 2. Programas, módulos e ítems

```ebnf
programa    = { item } ;
item        = import | from_import | [ anotaciones ] [ 'pub' ] declaracion ;
declaracion = funcion | struct | enum | trait | impl | const ;
import      = 'import' ruta_modulo [ 'as' IDENT ] ';' ;
from_import = [ 'pub' ] 'from' ruta_modulo 'import' nombre [ 'as' IDENT ]
              { ',' nombre [ 'as' IDENT ] } ';' ;
ruta_modulo = IDENT { '/' IDENT } ;
anotaciones = { '@' IDENT [ '(' IDENT { ',' IDENT } ')' ] } ;
```

- **Módulo = archivo**; su identidad es su **ruta** desde la raíz del proyecto (el directorio del
  archivo de entrada). `import a/b/c;` liga el *leaf* (`c`; `as` renombra). El acceso es
  calificado: `c.f(...)`, `c.Tipo`, `c.Enum.Variante`. El separador `/` solo existe en el
  `import`.
- **`pub`** exporta funciones, structs, enums, traits y consts. Referenciar un ítem no-`pub` de
  otro módulo es error. `pub from M import x;` **reexporta** (construye la cara pública).
- **Cápsulas**: la presencia de `P/mod.ray` vuelve `P/` direccionable (`import P;` carga
  `P/mod.ray`) y **encapsula** su subárbol: importar `P/interno` desde fuera de `P/` es error.
  `P.ray` y `P/mod.ray` a la vez es error (forma canónica única).
- Los **tipos** se namespacan por módulo (dos módulos pueden definir `Node`); las funciones
  también. `main` vive en el módulo de entrada.
- **Anotaciones** (conjunto cerrado): `@test` sobre funciones `() -> bool` o `() -> unit`;
  `@derive(Eq, Show, Hash)` sobre structs/enums **no genéricos** (Hash → `hash(self) -> int`,
  combinando el `.hash()` de los campos; un campo `float`/array no es hashable). Cualquier otra es error.
- **`main`** es obligatoria en el programa de entrada: sin parámetros, retorno `int` o `unit`.
  El código de salida del proceso es ese `int` (`& 0xFF`) o `0`.

## 3. Tipos

```ebnf
tipo = 'int' | 'float' | 'bool' | 'string' | 'char' | 'bytes'
     | 'u8' | 'u32' | 'u64'
     | '[' tipo ']'
     | '(' tipo ',' tipo { ',' tipo } ')'
     | 'fn' '(' [ tipo { ',' tipo } ] ')' [ '->' tipo ]
     | 'dyn' IDENT { '+' IDENT }
     | IDENT [ '<' tipo { ',' tipo } '>' ]        (* struct/enum/Map/Channel/Task/param de tipo *)
     | IDENT '.' IDENT [ '<' … '>' ] ;            (* tipo calificado por módulo: M.Punto *)
```

- **Primitivos**: `int` (entero con signo de 64 bits), `float` (IEEE-754 doble), `bool`,
  `string` (secuencia inmutable de caracteres Unicode; se indexa y mide **por carácter**),
  `char` (code point), `bytes` (secuencia inmutable de octetos), `unit` (el tipo del bloque
  vacío y del retorno omitido; no es escribible).
- **Enteros sin signo** `u8`/`u32`/`u64`: aritmética, comparación y bits **con wrapping** al
  ancho (por diseño). Solo operan con su mismo ancho; la conversión es explícita con `as`. Un
  **literal entero** adopta el ancho del contexto si cabe (fuera de rango = error de tipos).
- **Arreglos `[T]`**, **`Map<K,V>`** (claves *hashables*: `int`, `string`, `char`, `bool`,
  `bytes` — `float` no), **structs** y **enums**: **semántica de referencia** (§8). Tuplas
  `(A, B, …)`: acceso posicional `t.0` y desestructuración; las posiciones son de **solo
  lectura** (`t.0 = v` es error de tipos: la tupla es un agregado inmutable — para mutar,
  desestructura o usa un arreglo), así que la tupla se comporta como **valor**.
- **Funciones de primera clase** `fn(T…) -> R`; los closures capturan **por referencia**.
- **Genéricos** con **erasure total**: `Type` de runtime no existe; la inferencia es del checker
  (§7). Bounds `T: A + B` en funciones, structs, enums e impls.
- **Trait objects** `dyn A + B`: conjunto canónico (ordenado, sin duplicados); *upcasting* a un
  subconjunto; un método que usa `Self` fuera del receptor no es invocable sobre el objeto.
- **`Channel<T>`** y **`Task<T>`**: tipos de la concurrencia (§9); `Self` solo dentro de
  traits/impls.

## 4. Declaraciones

```ebnf
funcion  = 'fn' IDENT [ genericos ] '(' [ param { ',' param } ] ')' [ '->' tipo ] bloque ;
param    = IDENT ':' tipo ;
genericos= '<' IDENT [ ':' IDENT { '+' IDENT } ] { ',' … } '>' ;
struct   = 'struct' IDENT [ genericos ] '{' { IDENT ':' tipo ',' } '}' ;
enum     = 'enum' IDENT [ genericos ] '{' variante { ',' variante } [ ',' ] '}' ;
variante = IDENT [ '(' tipo { ',' tipo } ')' ] ;
trait    = 'trait' IDENT [ '<' IDENT { ',' IDENT } '>' ] '{' { firma_metodo } '}' ;
firma_metodo = 'fn' IDENT '(' 'self' { ',' param } ')' [ '->' tipo ] ( ';' | bloque ) ;
impl     = 'impl' [ genericos ] IDENT [ '<' tipo … '>' ] 'for' tipo '{' { metodo } '}' ;
const    = 'const' IDENT ':' tipo '=' literal ';' ;
extern   = 'extern' STRING '{' { firma_extern } '}' ;
firma_extern = 'fn' IDENT '(' [ param { ',' param } ] ')' [ '->' tipo ] ';' ;
```

- Las **firmas son explícitas** (parámetros y retorno); la inferencia es solo local (§5).
- Un método de trait puede traer **cuerpo por defecto**; un impl lo redefine u omite. La
  cobertura del impl debe ser exacta (ni métodos de más ni de menos, firmas idénticas con
  `Self` sustituido). Un impl puede ser **genérico** (`impl<T: B> Trait for Caja<T>`), aplicado
  exactamente a los parámetros propios; a lo sumo un impl por `(constructor, trait)`.
- Un método (de trait o de impl) puede tener **parámetros de tipo propios** (M40.2c):
  `fn map<U>(self, f: fn(T) -> U) -> Iter<U>`. Se suman a los del impl al resolver la llamada;
  la inferencia los fija por los argumentos. Habilita p. ej. los adaptadores de `Iterator` (§10).
- Los **traits con parámetros de tipo** (`trait From<S>`, `trait Iterator<T>`) existen con
  semántica limitada: `From<S> { fn desde(origen: S) -> Self; }` alimenta la conversión de `?`
  (§6.7), e `Iterator<T> { fn next(self) -> Option<T>; }` habilita `for x in it` (§5) por despacho
  por punto ordinario. Usar un trait parametrizado del usuario en bounds o `dyn` es error.
- `const` de nivel superior: el valor es un **literal** (o literal negado).
- **FFI** (`extern "lib" { … }`, M41): declara funciones de una librería C. Cada firma va **sin
  cuerpo**; su nombre es a la vez el identificador en raylang y el símbolo a resolver. La librería se
  carga con `dlopen` y los símbolos con `dlsym` en tiempo de ejecución (el nombre corto `"m"` se
  resuelve al archivo de plataforma o al proceso). Los tipos deben ser **marshalables**: en M41.1, los
  primitivos `int`↔long, `float`↔double, `bool`↔int (retorno además puede ser `unit`↔void), con aridad
  0..=3. Una firma fuera del catálogo, o un tipo no marshalable, es error. Llamar a una `extern fn` se
  ve como cualquier llamada. **Declarar una `extern fn` es la única operación insegura del lenguaje**:
  cruzar a C anula las garantías (memoria, firmas); todo lo demás es seguro por construcción.

## 5. Sentencias

```ebnf
bloque    = '{' { sentencia } [ expresion ] '}' ;   (* la expresión final sin ';' es el valor *)
sentencia = 'let' ( IDENT | '(' IDENT ',' IDENT { ',' IDENT } ')' ) [ ':' tipo ] '=' expresion ';'
          | 'var' IDENT [ ':' tipo ] '=' expresion ';'
          | destino '=' expresion ';'
          | 'return' [ expresion ] ';'
          | 'while' '(' expresion ')' bloque
          | 'for' patron_for 'in' iterable bloque
          | expresion ';'
          | expresion_con_bloque ;                   (* if/match/bloque como sentencia, sin ';' *)
destino   = IDENT | expresion_postfija '.' IDENT | expresion_postfija '[' expresion ']' ;
patron_for= IDENT | '(' IDENT ',' IDENT ')' ;
iterable  = expresion [ '..' expresion ] ;
```

- **`let` es inmutable, `var` mutable**; los parámetros son inmutables. Reasignar un `let` es
  error de tipos; *shadowing* permitido en ámbitos internos. La anotación de tipo es opcional si
  el inicializador determina el tipo; `[]`, `None`, `map_new()`, `channel()` y `Caja.Vacia` son
  **indeterminados** y exigen anotación o contexto (§7).
- La **mutación interior no exige `var`**: `obj.campo = v` y `arr[i] = v` mutan el objeto
  referenciado (§8); `var` gobierna la *ligadura*, no el objeto. `s[i] = c` sobre string es
  error (inmutable).
- **`for`** itera: arreglo (elemento), rango `a..b` (enteros, `a` inclusivo, `b` exclusivo; el
  rango solo existe en la cabecera del `for`), string (`char`), `Map` (tupla `(clave, valor)`
  en orden de clave — determinista) y cualquier tipo que implemente **`Iterator<T>`** (§7): el
  bucle llama a `next(self) -> Option<T>` hasta `None`, ligando cada elemento.
- **`return`** sale de la función envolvente; `return;` devuelve unit. El valor de una función
  también puede *caer* del bloque (retorno implícito: la expresión final sin `;`).

## 6. Expresiones

### 6.1 Precedencia (de menor a mayor)

| Nivel | Operadores | Asociatividad |
|---|---|---|
| 1 | `\|>` (pipeline) | izquierda |
| 2 | `\|\|` | izquierda |
| 3 | `&&` | izquierda |
| 4 | `\|` (OR bit a bit) | izquierda |
| 5 | `^` (XOR) | izquierda |
| 6 | `&` (AND) | izquierda |
| 7 | `==` `!=` | izquierda |
| 8 | `<` `<=` `>` `>=` | izquierda |
| 9 | `<<` `>>` | izquierda |
| 10 | `+` `-` | izquierda |
| 11 | `*` `/` `%` | izquierda |
| 12 | `as` (cast) | izquierda |
| 13 | `-` `!` `~` (unarios) | prefijo |
| 14 | llamada `f(…)`, campo/método `x.f`, índice `x[i]`, `?` | postfijo, izquierda |
| 15 | primarios | — |

`&&` y `||` **cortocircuitan**. Los operandos se evalúan de **izquierda a derecha**.

### 6.2 Primarios

Literales (§1), identificadores, `(expr)` (agrupación), tuplas `(a, b, …)`, arreglos
`[a, b, c[,]]`, literales de struct `Nombre { campo: expr, … }` (también calificado
`M.Nombre { … }`), funciones anónimas `fn(params) [-> R] bloque`, `if`, `match`, bloques.

- **`if (cond) bloque [else (bloque | if …)]`** es **expresión**: con `else`, ambas ramas deben
  converger en tipo (una rama que **diverge** —`return`, `panic`— cede el tipo a la otra); sin
  `else`, unit.
- **`if let patrón = expr bloque [else (bloque | if …)]`** (M40.1b) es azúcar de **`match (expr) {
  patrón => bloque, _ => else }`** (sin `else`, el brazo `_` es unit). El patrón usa la misma
  gramática que el match (variantes calificadas). El escrutinio va sin paréntesis, hasta el `{`.
- **`match (expr) { patrón [if guarda] => (expr | bloque), … }`** es expresión; brazos convergentes
  (misma regla de divergencia; todos divergentes → unit). **Exhaustivo** sobre enums. Patrones
  `Enum.Variante(sub-patrón…)` (también `M.Enum.Variante`), binding suelto, `_`. **Patrones anidados**
  (M40.1c): cada posición del payload es un sub-patrón completo, recursivo (`Result.Ok(Option.Some(v))`).
  **Guardas** (M40.1a): `patrón if <cond>` casa solo si el patrón liga Y la `cond` (`bool`, con los
  bindings del patrón en ámbito) es `true`; si no, se sigue al siguiente brazo. **Patrón de struct**
  (M40.1d): `Nombre { campo [: sub-patrón], … }` destructura un struct (forma corta `{ x, y }` =
  `{ x: x, y: y }`); solo anidado (el escrutinio es un enum). **Exhaustividad conservadora**: una
  variante cubre solo si sus sub-patrones son **irrefutables** (`_`/binding, o un struct de campos
  irrefutables como `Punto { x, y }`) y sin guarda; una variante anidada es refutable → hace falta un
  fallback (`Ok(_)`). No hay patrones de literal (diferido).
- **Ambigüedad struct-literal/bloque**: `Nombre { … }` se reconoce como literal solo si el
  receptor es un identificador (o `M.Nombre`) en posición de expresión; en la cabecera de un
  `for`/`if`/`while` sin paréntesis el `{` abre el cuerpo. El escrutinio de `match` y las
  condiciones van **entre paréntesis** por esta razón.

### 6.3 Llamadas, UFCS y métodos

`recv.f(args)` resuelve, en orden: (1) construcción de variante de enum, (2) **campo** del
struct de tipo función, (3) **método de trait** del tipo del receptor (incluye el trait object y
el parámetro acotado), (4) **UFCS**: `f(recv, args)` con `f` función libre o builtin (el
receptor participa en la inferencia de genéricos). Todo se resuelve **estáticamente** en el
checker (salvo el despacho de `dyn`, que es una llamada a través del objeto).

### 6.4 Pipelines

`x |> f(a, b)` ≡ `f(x, a, b)`; `x |> f` ≡ `f(x)`. Precedencia mínima, asociativo a la
izquierda; el operando derecho es un objetivo de llamada (nivel 14). Azúcar puro del parser.

### 6.5 Casts

`e as T` con `T ∈ {int, float, char, u8, u32, u64}`: `float as int` **trunca hacia cero**
(saturando en los extremos de `int`); `int as float` es la conversión IEEE más cercana;
`char as int`/`u*` es el code point; `int as char` **valida** el code point (error de ejecución
si no lo es); `int`↔`u*` y `u*`↔`u*` truncan al ancho destino (bits bajos).

### 6.6 Interpolación

`"a${x}b"` ≡ `"a" + to_string(x) + "b"`. Cada `${expr}` debe ser de tipo imprimible (§10). El `$`
solo interpola seguido de `{`; en cualquier otra posición es un carácter literal.

### 6.7 El operador `?`

`e?` con `e: Result<T, E>` en función que devuelve `Result<U, E2>`: si `Ok(v)` produce `v`; si
`Err(err)`, retorna `Err(err)` si `E == E2`, o `Err(E2.desde(err))` si existe `impl From<E>
for E2` (si no, error de tipos). Análogo para `Option<T>` en función que devuelve `Option<U>`.
`?` no cruza `Option`↔`Result`.

## 7. Sistema de tipos (reglas)

- **Inferencia local**: `let x = e;` toma el tipo de `e`. **Bidireccional**: un tipo esperado
  (anotación, tipo de retorno, parámetro) fija los indeterminados (`[]`, `None`, `map_new()`,
  `channel()`, construcción de enum genérico) y la **coerción de literal entero a `u*`**
  (también en asignación y elementos de arreglo).
- **Genéricos**: la instanciación se **infiere de los argumentos** por unificación (las
  variables de la firma llamada son incógnitas; las del llamador, rígidas). Dos usos
  incompatibles de `T` en una llamada son error; un parámetro no determinado exige anotación.
- **Bounds**: `x.m()` con `x: T` y `T: Trait` se verifica contra la firma del trait; en cada
  llamada a una función acotada, el tipo que instancia `T` debe **implementar** el trait (o ser
  un parámetro rígido del llamador con el mismo bound). La construcción de un
  `struct`/`enum` acotado exige lo mismo. (Implementación por diccionarios; semánticamente es
  despacho estático.)
- **Operadores sobrecargables**: `+ - * /` binarios y `-` unario sobre un tipo de usuario que
  implemente `Add/Sub/Mul/Div/Neg` (`fn add(self, otro: Self) -> Self`, etc.), ambos operandos
  del mismo tipo. `==`/`<` no son sobrecargables (usar `igual`/`menor` de `Eq`/`Ord`).
- **Igualdad `==`/`!=`**: primitivos, `string`, `char`, `bytes`, `u*` (mismo ancho) y
  **estructural** para arreglos/tuplas. Structs/enums de usuario: con `@derive(Eq)` o `impl
  Eq`, vía `igual` (no `==`). **Orden** `< <= > >=`: `int`, `float`, `string` (lexicográfico),
  `char` (code point), `u*`.
- **Divergencia**: `return`, `panic(…)` y las ramas que terminan en ellos tipan como "cede el
  tipo al resto".

## 8. Semántica de evaluación

- **Orden**: estricta, izquierda a derecha (argumentos incluidos).
- **Valores de referencia**: arreglos, structs, enums con payload, `Map`, `Channel`, `Task` —
  los alias comparten el objeto; la mutación se observa a través de cualquier alias. `string`,
  `bytes`, los primitivos y las tuplas (§3) se comportan como **valores** (inmutables). Los
  closures capturan las **celdas** de las variables (la mutación posterior se ve).
- **Aritmética**:
  - `int`: **el desbordamiento es error de ejecución** ("desbordamiento aritmético en int"):
    `+ - * /`(solo `MIN / -1`)` %`(solo `MIN % -1`) y `-` unario (`-MIN`). División y módulo
    por cero son errores de ejecución.
  - `u8/u32/u64`: **wrapping** al ancho, por diseño (también los bits `& | ^ ~ << >>`).
  - Los operadores **bit a bit sobre `int`** operan sobre los 64 bits con wrapping (los
    desplazamientos enmascaran el contador, semántica de Rust).
  - `float`: IEEE-754 (división por cero da `inf`/`NaN`; `NaN != NaN`).
- **Índices**: `arr[i]`/`s[i]`/`b[i]` con `i` fuera de rango es **error de ejecución** con
  posición.
- **Recursión**: profundidad máxima de llamadas `MAX_CALL_DEPTH = 1024` (error limpio de
  "desbordamiento de pila"), con **TCO garantizado**: una llamada en *posición de cola*
  (cuerpo de función, ramas de `if`/`match` en cola, expresión final de bloque, valor de
  `return`) reutiliza el marco — la recursión de cola corre en O(1) de pila. Los builtins no
  son posiciones de cola.
- **Límites del parser**: anidamiento máximo `MAX_PARSE_DEPTH = 1000` (error de sintaxis).
- **`panic(msg)`** aborta la ejecución con "pánico: msg" y la posición de la llamada; el
  proceso sale con 70.

## 9. Concurrencia (solo VM)

Modelo **CSP con green threads cooperativos M:1** y scheduler **determinista** (cola FIFO;
puntos de cesión fijos). En el intérprete, estas primitivas dan error limpio ("requiere la VM").

- `spawn(f: fn() -> T) -> Task<T>` lanza una fibra (no cede). `join(t: Task<T>) -> T` bloquea
  hasta que termina y **re-lanza** su fallo.
- `scope(body: fn() -> R) -> R` **posee** las tareas lanzadas dentro: al salir las une; si una
  falla, **cancela** a las hermanas pendientes (transitivo) y propaga el fallo original.
- `channel() -> Channel<T>` (no acotado), `channel(n)` (acotado; `n = 0` rendezvous).
  `send(ch, v)` bloquea con la cola llena; `recv(ch) -> Option<T>` bloquea vacío-y-abierto,
  `None` cerrado-y-vacío; `close(ch)` despierta receptores (un `send` sobre cerrado es error;
  un `close` con emisor bloqueado es error). `select(chs: [Channel<T>]) -> int` bloquea hasta
  que alguno esté listo para recibir y devuelve el **menor índice listo** (un canal cerrado
  está listo para siempre).
- El programa termina cuando **main retorna** (las fibras pendientes se abandonan). Si todas
  las fibras quedan bloqueadas: error "deadlock". La cancelación es **cooperativa** (en los
  puntos de cesión), no preemptiva.

## 10. Builtins y prelude (superficie estable)

La superficie estable es la **unión** de los builtins visibles y las funciones/traits del
prelude (escritas en raylang, inyectadas salvo redefinición del usuario — el usuario puede
*override*). Los primitivos `__nombre` son **internos e inestables** (no los uses).

- **Núcleo**: `print eprint to_string len panic assert assert_eq` · tipos imprimibles: `int
  float bool string char bytes u*` y (vía `mostrar`) tipos con `Show`.
- **Caracteres**: `char_code(c) -> int` (code point Unicode). — **String**: `trim split chars contains replace starts_with ends_with to_upper to_lower
  substring repeat index_of join to_bytes parse_int parse_float` · **Arreglos**: `push pop
  reverse contains position sort map filter fold iter` (+ `a + b` concatena) · **Bytes**:
  `bytes_of sub_bytes from_utf8` (+ `b1 + b2`, `to_string` → hex).
- **Iteradores** (M40.2b–f): `xs.iter()` y `range(a, b)` (semi-abierto) son iteradores de primera
  clase (`Iter<T>`, respaldados por un closure) recorribles con `for x in …`. **Adaptadores
  perezosos** (métodos de `Iterator`): `.map(f)`, `.filter(pred)`, `.take(n)`, `.skip(n)`,
  `.enumerate()` (pares `(int, T)`) y `.zip(otra)` (empareja dos iteradores en `(T, U)`, se agota con
  el más corto) devuelven otro iterador que solo calcula al recorrerse, encadenables
  (`range(0,n).map(f).filter(p).take(k)`). **Terminales**: `.fold(init, f)` reduce a un valor,
  `.collect()` materializa a `[T]`, y `sum(it)` (función libre sobre `Iter<int>`, vía UFCS `it.sum()`)
  suma enteros. `enumerate`/`zip` se consumen con **patrón de tupla** en el `for`:
  `for (i, x) in it.enumerate() { … }`. No colisionan con el `map`/`filter`/`fold` **eager** de
  arreglos (`xs.map(f) -> [U]`): se desambigua por el tipo del receptor.
- **Map**: `map_new insert get remove contains_key keys values len` (recorridos en orden de
  clave).
- **Set** (`Set<T>`, M40.3b; `T` debe derivar/implementar `Hash` + `Eq`): `set_new` (constructor
  vacío, el tipo lo fija el contexto), `set_add set_has set_remove set_size set_items`. Tabla hash
  escrita en el prelude; el prefijo `set_` evita chocar con builtins (`s.set_add(x)` por UFCS).
- **StringBuilder** (M40.3c): `sb_new sb_push sb_build sb_count` — acumula trozos y los une una vez
  (evita el O(n²) de `+` en bucle). **Deque** (`Deque<T>`, M40.3d): `deque_new deque_push_back
  deque_push_front deque_pop_front deque_pop_back deque_peek_front deque_len deque_is_empty` (pop/peek
  → `Option<T>`); cola/pila/doble-extremo sobre arreglo + índice `head`.
- **Matemáticas**: `sqrt pow floor ceil round abs min max sin cos tan ln log10 exp pi e` ·
  **Reloj/azar**: `now monotonic sleep random random_int`.
- **Proceso/E-S**: `args env input read_int read_file write_file append_file exists
  remove_file list_dir open read_line write close read_file_bytes write_file_bytes`.
- **Red**: `tcp_connect tcp_listen tcp_accept local_port socket_read socket_write
  socket_read_bytes socket_write_bytes udp_* tls_connect tls_accept tls_connect_h2`.
- **Concurrencia**: §9. — **Traits del prelude**: `Eq(igual) Show(mostrar) Ord(menor) Hash(hash)
  Add/Sub/Mul/Div/Neg From<S>(desde) Iterator<T>(next)` y `Option<T>`/`Result<T,E>`.

**Decisiones de nombres, congeladas** (raylang **no tiene sobrecarga**; cada firma un nombre):
`index_of` (string) vs `position` (arreglos); `fetch` (HTTP de la stdlib) porque `get` es de
`Map` y colisionaría bajo UFCS; `bytes_of([int])` vs `to_bytes(string)` (entradas distintas);
`join(arr, sep)` vs `join(task)` es la única dualidad *ad-hoc* (aridad), junto a `close`
(handle/canal) y `len`/`contains`/`+` (polimórficos por tipo).

## 11. Diagnósticos y códigos de salida

- Formatos de cabecera (estables): `error léxico en L:C: msg` · `error de sintaxis en L:C:
  msg` · `error de tipos en L:C: msg` · `error en ejecución en L:C: msg`. El render añade la
  línea de fuente (acotada a una ventana si es larguísima) y el subrayado `^…` del span; en
  multi-módulo se antepone `[módulo]` y la línea es la local del archivo.
- El compilador reporta **múltiples errores** (hasta 20) con recuperación; el primer error es
  siempre el mismo que en modo fail-fast.
- Un **ICE** (bug del compilador) imprime "error interno del compilador (ICE): …" y pide
  reporte; **ningún programa de usuario debe provocarlo**.
- **Códigos de salida**: el `int` de `main` (`& 0xFF`, 0 si unit) · **65** error de
  compilación (léxico/sintaxis/tipos/carga de módulos) · **66** archivo ilegible · **70**
  error de ejecución · **101** ICE.

## 12. Versionado y estabilidad

- El lenguaje versiona con **SemVer**: `MAYOR.MENOR.PARCHE[-pre]`. La versión vive en el
  binario (`raylang --version`) y en la cabecera de esta SPEC; **cambia con esta SPEC**.
  - **MAYOR**: cambios incompatibles en algo declarado estable por esta SPEC.
  - **MENOR**: superficie nueva compatible (sintaxis, builtins, stdlib).
  - **PARCHE**: correcciones sin cambio de superficie.
- **Estable** = todo lo definido en §§1–11, salvo lo marcado como interno/inestable:
  los primitivos `__nombre`, el formato del bytecode y del texto del LSP interno, los detalles
  del GC y del scheduler más allá de lo garantizado en §9, y los mensajes de error **más allá
  de la cabecera** (la línea/ventana/subrayado pueden mejorar en MENOR).
- **Deprecación**: un elemento estable se marca *deprecado* en esta SPEC (con reemplazo) al
  menos una versión MENOR antes de retirarse en la siguiente MAYOR.
- Las **librerías de `examples/`** (web, cripto, formatos…) aún **no** son stdlib versionada:
  se congelan como `std/` en M40. Las escritas puras (`time`, `log`, `csv`, `toml`, …) están
  cubiertas de facto por los oráculos.

## 13. Notas de implementación (informativas)

Dos motores compartiendo el front-end; los genéricos/traits/bounds/dyn se **borran** en
compilación (el runtime no conoce tipos); las posiciones `(línea, col)` acompañan a todo token,
nodo y error; el binario ejecuta todo en un hilo con pila de 256 MiB. El compilador
auto-alojado (`selfhost/`) implementa este mismo lenguaje y sirve de validador cruzado de esta
gramática: el parser de Rust y el auto-alojado producen el mismo AST nodo a nodo sobre el
corpus del repo.
