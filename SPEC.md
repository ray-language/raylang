# Especificación del lenguaje raylang

**Versión del lenguaje: 1.5.0** (esta especificación versiona con el lenguaje; ver §12).

Este documento es **normativo**: define qué es un programa raylang válido y qué hace. Los otros
dos documentos del proyecto no lo son: [DESIGN.md](DESIGN.md) es la *crónica de diseño* (el
porqué de cada decisión, fase a fase) y el libro (`book/`) es la *pedagogía* (cómo se construyó).
Ante un conflicto, manda esta SPEC; un conflicto entre la SPEC y la implementación es un bug de
una de las dos y debe resolverse explícitamente.

**Conformidad.** raylang tiene **tres motores** que deben producir **comportamiento observable
idéntico** (stdout, errores, código de salida) para todo programa determinista:

1. la **máquina virtual de bytecode** — el motor de producto (`ray run`);
2. el **binario nativo** (`ray build --native`), que transpila el programa a Rust y lo compila a
   código máquina — su salida es **byte-idéntica** a la de la VM (corpus de paridad en la suite);
3. el **intérprete de árbol** (`ray run --interp`), que es el **oráculo secuencial** de desarrollo.

La suite lo verifica por oráculo cruzado en las tres direcciones. Las excepciones están acotadas
y listadas: la concurrencia (§9) y la E/S asíncrona **no existen en el intérprete** (da un error
limpio "requires the VM"), y los subsistemas que un binario nativo excluye a propósito
(`--without …`, ver [REFERENCE.md](REFERENCE.md) §14) responden con un error de ejecución
explícito en vez de silencio.

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
  pub import from extern as` y las de tipo `int float bool string char bytes ptr u8 u32 u64`.
- **Literales**:
  - *Entero*: dígitos decimales (`42`) o con **prefijo de base** (M118): `0x`/`0X` hexadecimal
    (`0x1F`), `0o`/`0O` octal (`0o755`), `0b`/`0B` binario (`0b1010`). Al menos un dígito tras el
    prefijo (`0x` solo es error léxico). Debe caber en `int` (i64); si no, error léxico. Sin
    separador `_` (diferido).
  - *Flotante*: siempre decimal (los prefijos de base son solo para enteros).
  - *Flotante*: `dígitos '.' dígitos` (`3.14`), con **exponente** opcional `e|E [+|-] dígitos`
    (`1e21`, `1.5e-3`, `2E+10`); un exponente hace el literal flotante aunque no lleve punto.
    Un `.` sin dígito decimal no es flotante; un `e` sin dígito (o sin dígito tras el signo) no
    es exponente (`1eabc` = entero `1` + identificador).
  - *Cadena*: `"…"` con escapes `\n \t \r \0 \\ \" \` \$`, más `\xNN` (dos dígitos hex → el code
    point `U+00NN`, 0–255) y `\u{H…H}` (1–6 dígitos hex → un code point Unicode; error si excede
    `U+10FFFF` o es un surrogate). No admite saltos de línea literales.
  - *Cadena plantilla* (M95): `` `…` `` — mismo valor y mismo token que `"…"`, con dos
    diferencias: la comilla doble `"` es **literal** (no se escapa) y los **saltos de línea**
    están permitidos (multilínea, literales). El backtick literal se escapa `` \` ``. Interpola
    igual que cualquier cadena.
  - *Cadena interpolada*: cualquier cadena (`"…"` o `` `…` ``) con `…${expr}…`. `${expr}`
    contiene **una** expresión; el `$` solo es especial seguido de `{` (`"$5"`, `"{n}"` son
    literales; `\${` es un `${` literal). Azúcar: se desazucara a concatenación con
    `to_string(expr)` (§6.6).
  - *Carácter*: `'a'` con escapes `\n \t \r \0 \\ \'`, más `\xNN` y `\u{H…H}` (como en cadena). Un
    code point Unicode.
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
              { ',' nombre [ 'as' IDENT ] } [ ',' ] ';' ;
ruta_modulo = IDENT { '/' IDENT } ;
anotaciones = { '@' IDENT [ '(' IDENT { ',' IDENT } ')' ] } ;
```

- **Módulo = archivo**; su identidad es su **ruta** desde la raíz del proyecto (el directorio del
  archivo de entrada). `import a/b/c;` liga el *leaf* (`c`; `as` renombra). El acceso es
  calificado: `c.f(...)`, `c.Tipo`, `c.Enum.Variante`. El separador `/` solo existe en el
  `import`.
- La lista de un `from … import` puede repartirse en **varias líneas** (el léxico ignora los saltos
  de línea) y admite **coma final**. `ray fmt` la deja en una línea si cabe en **100 columnas** y la
  envuelve a un nombre por línea si no —sin coma final, que dejaría el `;` colgando. Con el mismo
  umbral reparte una **cadena de métodos** de dos o más eslabones (el receptor se queda en su sitio y
  cada `.metodo(…)` baja una línea) y las **listas delimitadas**: argumentos de llamada, parámetros de
  `fn` y literales de arreglo, tupla, struct y Map. Una lista delimitada cierra su delimitador **en
  línea propia**; una forma sin delimitador propio (el `;` del import, el `)` que no es de la cadena)
  lo pega al último elemento.
- **`pub`** exporta funciones, structs, enums, traits y consts. Referenciar un ítem no-`pub` de
  otro módulo es error. `pub from M import x;` **reexporta** (construye la cara pública).
- **Cápsulas**: la presencia de `P/mod.ray` vuelve `P/` direccionable (`import P;` carga
  `P/mod.ray`) y **encapsula** su subárbol: importar `P/interno` desde fuera de `P/` es error.
  `P.ray` y `P/mod.ray` a la vez es error (forma canónica única).
- Los **tipos** se namespacan por módulo (dos módulos pueden definir `Node`); las funciones
  también. `main` vive en el módulo de entrada.
- **Anotaciones** (conjunto cerrado): `@test` sobre funciones `() -> bool` o `() -> unit`;
  `@derive(Eq, Show, Hash, ToJson)` sobre structs/enums **no genéricos** — `Hash` genera
  `hash(self) -> int` combinando el `.hash()` de los campos (un campo `float`/array no es
  hashable) y `ToJson` genera `to_json(self) -> string` (su trait vive en `std/json` y debe estar
  en ámbito para derivarlo). `Ord` **no** es derivable: se implementa a mano. Cualquier otra
  anotación es error.
- **`main`** es obligatoria en el programa de entrada: sin parámetros, retorno `int` o `unit`.
  El código de salida del proceso es ese `int` (`& 0xFF`) o `0`. Excepción: si `print`/`eprint`
  —o la salida del propio CLI (`ray fmt`, `ray doc`…)— encuentran su destino **cerrado** (un pipe
  roto, `programa | head`), el proceso termina en silencio con código **141** (128+SIGPIPE, la
  convención Unix); `io.write`/`io.flush` en cambio devuelven `Err` y el programa decide.

## 3. Tipos

```ebnf
tipo = 'int' | 'float' | 'bool' | 'string' | 'char' | 'bytes' | 'ptr'
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
  vacío y del retorno omitido; **escribible** en posición de tipo — `fn f() -> unit`, útil sobre
  todo en firmas `extern` (§FFI) y tipos `fn`; no es palabra clave: se resuelve como nombre de
  tipo, igual que `Map`/`Channel`/`Task`, y **sombrea** cualquier struct/enum del usuario que se
  llame así).
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
extern   = 'extern' STRING [ 'blocking' ] '{' { firma_extern } '}' ;
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
  semántica limitada: `From<S> { fn convert(origen: S) -> Self; }` alimenta la conversión de `?`
  (§6.7), e `Iterator<T> { fn next(self) -> Option<T>; }` habilita `for x in it` (§5) por despacho
  por punto ordinario. Usar un trait parametrizado del usuario en bounds o `dyn` es error.
- `const` de nivel superior: el valor es un **literal** (o literal negado).
- **FFI** (`extern "lib" { … }`, M41): declara funciones de una librería C. Cada firma va **sin
  cuerpo**; su nombre es a la vez el identificador en raylang y el símbolo a resolver. La librería se
  carga con `dlopen` y los símbolos con `dlsym` en tiempo de ejecución (el nombre corto `"m"` se
  resuelve al archivo de plataforma o al proceso). Los tipos deben ser **marshalables**: los primitivos
  `int`↔C `int` (32 bits, con signo), `u64`↔C `long`/`size_t` (64 bits), `float`↔double, `bool`↔int
  (aridad 0..=6 — límite del checker, idéntico en todos los motores), y como **argumento** `string`↔`char*` (NUL-terminado) y `bytes`↔puntero al buffer
  (M41.2). Un puntero opaco (`FILE*`, handle) se pasa como `u64` o, con seguridad de tipos, como **`ptr`**
  (M41.4b: un puntero **opaco** — se recibe/pasa/compara por identidad, pero no se desreferencia ni opera).
  El **retorno** admite `int`/`u64`/`float`/`bool`/`unit`, `ptr`/`Option<ptr>` (`NULL → None`, p. ej.
  `fopen`), y para un `char*`, **`Option<bytes>`** (`NULL → None`; la frontera copia los bytes hasta el NUL
  y no libera el puntero) o **`Option<string>`** (azúcar que valida UTF-8; bytes inválidos → error de
  ejecución) (M41.3). Un `string`/`bytes` **pelado** de retorno es error (un `char*` puede ser NULL y no
  hay `null`). Una firma fuera del catálogo, o un tipo no marshalable, es error.
  Llamar a una `extern fn` se ve como cualquier llamada. **Declarar una `extern fn` es la única
  operación insegura del lenguaje**: cruzar a C anula las garantías (memoria, firmas); todo lo demás es
  seguro por construcción.
- **`extern "lib" blocking { … }`**: marca todas las firmas del bloque como llamadas **bloqueantes de
  verdad** (E/S, librerías C lentas). `blocking` es una palabra **contextual** (no reservada: sigue
  siendo un identificador válido). La marca **no cambia los valores** — tipos, marshalling y resultado
  son idénticos a un bloque sin marcar —; es una directiva de **planificación**: en el binario nativo
  con fibras (el default), la llamada se descarga a un hilo de un pool bloqueante y la fibra queda
  aparcada, de modo que el worker M:N no se bloquea ni vara a las fibras hermanas fijadas a él. Donde
  no hay scheduler que proteger (la VM, el intérprete, un binario `--without fibers`, o una llamada
  fuera de fibra) la marca es inerte y la llamada es directa.

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
expresion_con_bloque = expresion_if | expresion_while | expresion_match | bloque ;
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
- **Expresión-con-bloque en posición de sentencia** (M153): dentro de un bloque, una expresión
  que COMIENZA con `if`/`while`/`match`/`{` se parsea exactamente como esa forma-con-bloque —
  ningún operador postfijo (`(`, `[`, `.`, `?`) ni binario la extiende; el token siguiente
  inicia una sentencia nueva o la cola del bloque. Así `if (c) { … }` seguido de `(a, b)` en
  la línea siguiente es el `if` como sentencia y la tupla como cola — no una llamada del valor
  del bloque. Para aplicar postfijos o binarios al VALOR de una forma-con-bloque, ponla en
  posición de expresión: paréntesis o un `let` (`let x = if (c) { f } else { g }(1);` sigue
  siendo una llamada). La resolución es la misma familia que la del struct-literal (§6.2):
  ante la ambigüedad, en posición de sentencia gana la lectura de sentencia.

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

- **Coma final**: toda lista delimitada la admite — argumentos de llamada `f(a, b,)`,
  parámetros de `fn`, literales de arreglo/tupla/struct/Map y la lista de un `from … import`
  (§2). `ray fmt` la elimina (la forma canónica no la lleva). Cerrada la inconsistencia del
  dogfood raydesk: llamadas y parámetros la rechazaban mientras arrays/structs la aceptaban.

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
- **Divergencia**: `return`, `panic(…)`, `exit(…)` y las ramas que terminan en ellos tipan como
  "cede el tipo al resto".

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
- **`exit(code)`** termina el proceso con ese código, desde cualquier fibra, flusheando
  stdout/stderr. No es un error: sin mensaje ni traza (y `try_call` NO lo captura — el proceso
  muere).

## 9. Concurrencia (VM y binario nativo; no en el intérprete)

Modelo **CSP → actores con aislamiento de heap**: cada fibra tiene su propio heap y la única
comunicación entre fibras son los **canales**, que **transfieren** el valor. No hay estado
mutable compartido → *data-race freedom* **por construcción**, sin *ownership* en el sistema de
tipos. En el intérprete estas primitivas dan un error limpio ("requires the VM").

**Ejecución.** El scheduler es **M:N**: M fibras sobre N hilos worker.

- En la **VM**, N se decide así, en orden: `--deterministic` → **1**; `RAYLANG_THREADS=N`
  explícito → **N**; el programa **no usa `spawn`** → **1**; en otro caso →
  `available_parallelism()` (**multicore por defecto**). N se acota a `1..=256`.
- En el **binario nativo**, las fibras son corrutinas de pila propia sobre un reactor del SO
  (por defecto); `ray build --native --without fibers` recupera el modelo hilo-por-tarea.
- En `wasm32` (playground) N es siempre **1**: no hay hilos del sistema.

**Garantías, y qué depende de N.** La semántica de cada primitiva (abajo) es la misma con
cualquier N. Lo que **solo** se garantiza con **N = 1** (`--deterministic` o `RAYLANG_THREADS=1`)
es el **orden observable** entre fibras: intercalado de la salida, orden de despertar y el
resultado de `select` entre varios canales listos a la vez. Un programa cuya salida deba ser
reproducible debe fijar N = 1 o sincronizar por canales. La cancelación es **cooperativa** (actúa
en los puntos de cesión), nunca preemptiva.

- `spawn(f: fn() -> T) -> Task<T>` lanza una fibra (no cede). `join(t: Task<T>) -> T` bloquea
  hasta que termina y **re-lanza** su fallo; `try_join(t) -> Result<T, string>` lo devuelve como
  valor en vez de re-lanzarlo.
- `scope(body: fn() -> R) -> R` **posee** las tareas lanzadas dentro: al salir las une; si una
  falla, **cancela** a las hermanas pendientes (transitivo) y propaga el fallo original.
- `channel() -> Channel<T>` (no acotado), `channel(n)` (acotado; `n = 0` rendezvous).
  `send(ch, v)` bloquea con la cola llena; `recv(ch) -> Option<T>` bloquea vacío-y-abierto,
  `None` cerrado-y-vacío; `close(ch)` despierta receptores (un `send` sobre cerrado es error;
  un `close` con emisor bloqueado es error). `select(chs: [Channel<T>]) -> int` bloquea hasta
  que alguno esté listo para recibir y devuelve el **menor índice listo en el momento de la
  comprobación** (un canal cerrado está listo para siempre). `try_recv(ch: Channel<T>) ->
  Received<T>` recibe **sin bloquear**: `Received.Got(v)` si había un valor listo (lo consume,
  como `recv`), `Received.Empty` si el canal está abierto y vacío, `Received.Closed` si está
  cerrado y drenado — el `enum Received<T> { Got(T), Empty, Closed }` del prelude.
  `select_timeout(chs: [Channel<T>], ms: int) -> Option<int>` es `select` con **plazo**: `Some(i)`
  con el menor índice listo, `None` si vencen los `ms` milisegundos antes; `ms <= 0` = poll no
  bloqueante (`None` inmediato si ninguno listo).
- `signals() -> Channel<int>` devuelve el canal **singleton** de señales del proceso (`SIGTERM`
  = 15, `SIGINT` = 2 y `SIGWINCH` = 28 —cambio de tamaño del terminal— llegan como enteros),
  para apagado ordenado y re-maquetado de TUIs; compone con `recv`/`select`. Solo unix (VM y
  binario nativo).
- El programa termina cuando **`main` retorna** (las fibras pendientes se abandonan). Si todas
  las fibras quedan bloqueadas y ninguna puede progresar: error "deadlock" (las que esperan E/S
  del exterior o el reloj no cuentan como bloqueadas).

## 10. Builtins y prelude (superficie estable)

La superficie estable tiene **tres capas**, y solo las dos primeras las fija esta SPEC:

1. **Global** (sin importar nada): los builtins visibles y las funciones/traits del **prelude**
   (escritas en raylang e inyectadas salvo redefinición del usuario, que puede hacer *override*).
2. **`std/…`** (opt-in con `import std/…;`): la biblioteca estándar, **embebida en el binario**
   y versionada con el lenguaje. Importar un módulo es además una *pista de capacidad* legible
   ("este archivo toca disco / red / procesos").
3. **Paquetes** (`net`, `db`, `web`, `rpc`, …): fuera de esta SPEC; versionan por separado y se
   instalan con el gestor de paquetes.

Los primitivos `__nombre` son **internos e inestables** (no los uses): son el borde con el host
sobre el que se escriben las capas 1 y 2.

### 10.1 Global

- **Núcleo**: `print eprint to_string len panic exit assert assert_eq` · tipos imprimibles: `int
  float bool string char bytes u*` y (vía `show`) tipos con `Show`.
- **Recuperación de fallos** (M97): `try_call(f: fn() -> T) -> Result<T, string>` ejecuta `f` y
  convierte un `panic` o error de ejecución en `Err(mensaje)` — el fallo **como valor**, sin
  excepciones. La recuperación ocurre en la **misma fibra**: lo que `f` mutó antes de fallar
  sigue mutado (mismo compromiso que `catch_unwind` de Rust); para aislamiento real, `spawn` +
  `try_join(t) -> Result<T, string>` (§9, VM y binario nativo). `try_call` funciona en los tres
  motores.
- **Caracteres**: `char_code(c) -> int` (code point Unicode) y `char_from_code(n) -> Option<char>`
  (su inversa; `None` si `n` no es un code point válido). — **String**: `trim split chars contains
  replace starts_with ends_with to_upper to_lower substring repeat index_of join to_bytes
  parse_int parse_float` · **Arreglos**: `push pop reverse contains position sort map filter fold
  any all iter` (+ `a + b` concatena) · **Bytes**: `bytes_of sub_bytes from_utf8` (+ `b1 + b2`,
  `to_string` → hex).
- **Entrada/entorno**: `args() -> [string]` (argumentos del programa), `env(name) ->
  Option<string>`, `input() -> Option<string>` (una línea de stdin), `read_int() ->
  Option<int>`. El disco vive en `std/fs`, no aquí.
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
- **Map** (`Map<K, V>`, claves primitivas: `int string char bool bytes`): `Map.new() insert get
  get_or remove contains_key keys values len` — `keys()`/`values()` recorren en **orden de
  clave** (el almacén interno no expone su orden). Para claves de usuario, `std/collections/dict`.
- **Handles**: `close(h)` cierra un archivo, socket o canal (el mismo nombre para los tres).
- **Concurrencia**: §9 (`spawn join try_join scope channel send recv close select signals`).
- **Traits del prelude** (los métodos son los que se implementan y se llaman por UFCS):
  `Eq(eq)` · `Show(show)` · `Ord(less)` · `Hash(hash)` · `Len(len)` · `Push(push)` ·
  `Reverse(reverse)` · `Contains(contains)` · `From<S>(convert)` · `Iterator<T>(next)` ·
  `Add(add)`/`Sub(sub)`/`Mul(mul)`/`Div(div)`/`Neg(neg)` (sobrecarga de operadores) ·
  `StrOps`/`BytesOps`/`MapOps`/`OptionOps`/`ResultOps` (los métodos de string, bytes, `Map`,
  `Option<T>` y `Result<T,E>`). **Derivables** con `@derive(…)`: `Eq`, `Show`, `Hash` y `ToJson`
  — y solo esos cuatro (`Ord` se implementa a mano).

### 10.2 La biblioteca estándar `std/`

Va **embebida en el binario**: `import std/math;` funciona sin que `std/` exista en disco. Se usa
**calificada** por el último segmento de la ruta (`math.sqrt(2.0)`, `set.add(s, x)`).

| Módulo | Qué cubre |
|---|---|
| `std/fs` | disco: leer/escribir (texto y `bytes`), `append`, `exists`, `list_dir`, metadatos, copiar/renombrar/borrar, directorios, handles con `read_line`/`write`/`write_bytes`/`read_bytes`/`seek`/`sync` (durabilidad: fsync) candados consultivos `try_lock`/`unlock` (flock), `stat` (lstat: detecta symlinks), `chmod`, y `watch`/`next_event` (cambios por eventos de kernel; la fibra aparca) |
| `std/net` | transporte: TCP (`tcp_connect`/`tcp_listen`/`tcp_accept`/`local_port`), I/O de sockets en texto y `bytes`, TLS (`tls_connect`/`tls_accept`/`tls_upgrade`) |
| `std/process` | ejecución de procesos del SO **sin shell** (argv tipado): `run`, el builder `cmd` y el modo *streaming* `stream` —con stdin escribible sobre un hijo vivo (`stdin_pipe`/`write`/`close_stdin`) para sesiones persistentes— (§ REFERENCE.md §10) |
| `std/math` | `PI`/`E`, `sqrt pow sin cos tan asin acos atan atan2 ln log2 log10 exp floor ceil round trunc`, y `abs`/`min`/`max` genéricos |
| `std/time` | reloj (`now`, `monotonic`), `sleep`, fechas UTC y formateo (ISO 8601/RFC 1123), constructores de duración a ms (`millis`/`seconds`/`minutes`/`hours`/`days`) |
| `std/random` | PRNG del proceso: `next`, `below`, `between`, `choice`, `shuffle` y `seed` (semilla explícita → secuencia reproducible) |
| `std/crypto` | cripto de **producción** respaldada por `ring` (tiempo constante): `sha256 sha512 sha1`, `hmac_sha256`, `ed25519_public_key`/`ed25519_sign`/`ed25519_verify`, `chacha20poly1305_seal`/`_open`, `random_bytes` (CSPRNG) y —acuerdo de claves— `x25519_public_key`/`x25519_shared_secret` (respaldados por `x25519-dalek`), `hkdf_sha256` y `constant_time_eq`. Las versiones escritas en raylang puro (`examples/web/`) son **demostración del lenguaje**, no producción |
| `std/collections/{set,deque,stringbuilder,dict}` | `Set<T>` y `Dict<K,V>` (claves de usuario vía `Hash`+`Eq`), `Deque<T>` y un constructor de strings que evita el O(n²) de concatenar en bucle |
| `std/text`, `std/sort`, `std/regex`, `std/csv`, `std/toml`, `std/json`, `std/template`, `std/markdown` | procesamiento de texto y datos (`markdown`: `parse -> [Block]` —AST tipado— y `to_html`; subconjunto CommonMark con tablas GFM, HTML embebido **escapado** y URLs `javascript:` neutralizadas por diseño) |
| `std/hex`, `std/base64`, `std/url`, `std/uuid`, `std/protobuf` | codificaciones e identificadores |
| `std/inflate`, `std/deflate`, `std/huffman` | compresión (gzip/zlib/DEFLATE) |
| `std/kv`, `std/resilience` | almacén clave-valor persistente (compartible entre fibras) y utilidades de resiliencia: reintentos con política, *circuit breaker* y plazos (`deadline`/`expired`) |
| `std/ffi` | helpers de la frontera C: `errno()` (el `errno` del hilo tras una extern estilo POSIX; leerlo inmediatamente tras la llamada) |
| `std/io` | consola **por bytes**: `write`/`ewrite`/`write_bytes` (→ `Result<int,string>`, sin salto de línea) y `flush()`; `read(max) -> Option<bytes>` (`None` = EOF) y `read_timeout(max, ms) -> ReadResult` (`Data`/`Eof`/`TimedOut`). stdout va con buffer; stderr no. En la VM, una lectura sin datos **aparca la fibra** (las demás siguen); un solo lector de stdin a la vez, y no se mezcla con `input()`/lecturas por línea (buffers distintos). El **orden** entre `print` e `io.write` es el de programa en los tres motores |
| `std/units` | constructores de tamaño a bytes, convención binaria 1024ⁿ (`kb`/`mb`/`gb`) |
| `std/term` | el terminal: `is_tty(fd)`, `size() -> Option<(int, int)>`, `raw(f)` (modo crudo con restauración garantizada — también al salir el proceso; no ante señal fatal/`kill -9`), `read_key() -> Option<Key>` y el decodificador **puro** `decode(bytes) -> Option<(Key, int)>` (`None` = secuencia incompleta). `enum Key`: `Char/Enter/Tab/Backspace/Esc/Up/Down/Left/Right/Home/End/PageUp/PageDown/Insert/Delete/Ctrl(char)/F(int)`. Unix; fuera: `is_tty` false, `size` `None`, `raw` `Err`. **Ancho en celdas** (portable, sin tty): `width(s) -> int`, `char_width(c) -> int` (wcwidth: control/combinantes 0, CJK/fullwidth/emoji 2, resto 1), `fit(s, cells)`/`fit_right(s, cells)` (trunca sin partir un carácter ancho y rellena a `cells`) |

Un módulo `std/…` puede depender de que el binario incluya un subsistema (TLS/cripto necesitan
`net-tls`; un binario *slim* o `--without` responde con un error de ejecución explícito). El
catálogo con firmas está en [REFERENCE.md](REFERENCE.md) §10.

**Decisiones de nombres, congeladas** (raylang **no tiene sobrecarga**; cada firma un nombre):
`index_of` (string) vs `position` (arreglos); `fetch` (el cliente HTTP del paquete `net`) porque
`get` es de `Map` y colisionaría bajo UFCS; `bytes_of([int])` vs `to_bytes(string)` (entradas
distintas);
`join(arr, sep)` vs `join(task)` es la única dualidad *ad-hoc* (aridad), junto a `close`
(handle/canal) y `len`/`contains`/`+` (polimórficos por tipo).

## 11. Diagnósticos y códigos de salida

- Formatos de cabecera (estables, **en inglés** — regla del 21 jul 2026: todo lo que el
  lenguaje entrega al usuario va en inglés): `lex error at L:C: msg` · `syntax error at L:C:
  msg` · `type error at L:C: msg` · `runtime error at L:C: msg`. El render añade la
  línea de fuente (acotada a una ventana si es larguísima) y el subrayado `^…` del span; en
  multi-módulo se antepone `[módulo]` y la línea es la local del archivo.
- El compilador reporta **múltiples errores** (hasta 20) con recuperación; el primer error es
  siempre el mismo que en modo fail-fast.
- Un **ICE** (bug del compilador) imprime "error interno del compilador (ICE): …" y pide
  reporte; **ningún programa de usuario debe provocarlo**.
- **Códigos de salida**: el `int` de `main` (`& 0xFF`, 0 si unit) · **64** uso incorrecto del
  CLI · **65** error de compilación (léxico/sintaxis/tipos/carga de módulos) · **66** archivo
  ilegible · **69** el binario no incluye el subsistema que el comando necesita (build *slim*) ·
  **70** error de ejecución · **73** no se pudo crear un archivo · **101** ICE. `ray test` sale
  con **0** (todo pasó), **1** (alguna prueba falló) o **65** (alguna suite no compila).

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
- **Qué versiona con el lenguaje y qué no.** El núcleo (§§1–11) y la **biblioteca estándar
  `std/`** (§10.2) versionan **juntos**: van embebidos en el mismo binario y una SPEC dada
  describe ambos. Los **paquetes** (`net`, `db`, `web`, `rpc`, …) versionan por separado con
  semver propio y se resuelven por el gestor de paquetes; las librerías de `examples/` son
  material de demostración y **no** tienen garantía de estabilidad, aunque algunas sean la
  fuente de un módulo `std/` (en cuyo caso manda lo que dice §10.2, no el ejemplo).

## 13. Notas de implementación (informativas)

Tres motores compartiendo el mismo front-end (§Conformidad); los genéricos/traits/bounds/dyn se
**borran** en compilación (el runtime no conoce tipos); las posiciones `(línea, col)` acompañan a
todo token, nodo y error. El programa corre en un hilo de **pila grande** (256 MiB), para que la
recursión profunda sea robusta, y la **recursión de cola se elimina** (TCO) tanto en el
intérprete como en la VM; en el binario nativo cada fibra reserva su propia pila (128 MiB de
reserva virtual por defecto, ajustable). El compilador auto-alojado (`selfhost/`) implementa este mismo lenguaje y sirve de
validador cruzado de esta gramática: el parser de Rust y el auto-alojado producen el mismo AST
nodo a nodo sobre el corpus del repo.
