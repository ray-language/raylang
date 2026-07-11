# Referencia de raylang

El **catálogo exhaustivo** de la superficie del lenguaje: palabras clave, símbolos, operadores,
builtins, prelude, biblioteca estándar y herramientas. Complementa al [`MANUAL.md`](MANUAL.md) (la
guía práctica, con prosa y ejemplos) y a la [`SPEC.md`](SPEC.md) (la referencia normativa: ante una
discrepancia, manda la SPEC).

## Índice

1. [Palabras clave](#1-palabras-clave)
2. [Símbolos y operadores](#2-símbolos-y-operadores)
3. [Literales y escapes](#3-literales-y-escapes)
4. [Tipos](#4-tipos)
5. [Builtins globales](#5-builtins-globales)
6. [Métodos por tipo de receptor](#6-métodos-por-tipo-de-receptor)
7. [Funciones asociadas a tipos](#7-funciones-asociadas-a-tipos)
8. [El prelude](#8-el-prelude)
9. [Iteradores](#9-iteradores)
10. [La biblioteca estándar `std/`](#10-la-biblioteca-estándar-std)
11. [Paquetes adicionales (`net`, `db`)](#11-paquetes-adicionales-net-db)
12. [Anotaciones](#12-anotaciones)
13. [FFI: tipos marshalables](#13-ffi-tipos-marshalables)
14. [El CLI `ray`](#14-el-cli-ray)
15. [Códigos de salida](#15-códigos-de-salida)

---

## 1. Palabras clave

Reservadas (no pueden usarse como identificadores):

| Grupo | Palabras |
|---|---|
| Declaraciones | `fn` `let` `var` `const` `struct` `enum` `trait` `impl` `extern` |
| Control | `if` `else` `while` `for` `in` `return` `match` |
| Módulos | `import` `from` `pub` |
| Valores/tipos | `true` `false` `dyn` `as` `self` `Self` |
| Tipos primitivos | `int` `float` `bool` `string` `char` `bytes` `ptr` `u8` `u32` `u64` |

> Ojo con `from`: al ser palabra clave, no vale como nombre de parámetro o variable (usa `source`,
> `origin`, etc.).

## 2. Símbolos y operadores

### Tabla de precedencia (de menor a mayor; SPEC §6.1)

| Nivel | Operadores | Asociatividad |
|---|---|---|
| 1 | `\|>` (pipeline) | izquierda |
| 2 | `\|\|` (OR lógico, cortocircuita) | izquierda |
| 3 | `&&` (AND lógico, cortocircuita) | izquierda |
| 4 | `\|` (OR bit a bit) | izquierda |
| 5 | `^` (XOR bit a bit) | izquierda |
| 6 | `&` (AND bit a bit) | izquierda |
| 7 | `==` `!=` | izquierda |
| 8 | `<` `<=` `>` `>=` | izquierda |
| 9 | `<<` `>>` (desplazamientos) | izquierda |
| 10 | `+` `-` | izquierda |
| 11 | `*` `/` `%` | izquierda |
| 12 | `as` (cast) | izquierda |
| 13 | `-` `!` `~` (unarios) | prefijo |
| 14 | llamada `f(…)` · campo/método `x.f` · índice `x[i]` · `?` | postfijo |
| 15 | primarios (literales, `(…)`, bloques…) | — |

> Consecuencia práctica: los **bit a bit ligan más flojo que las comparaciones** (estilo C).
> `flags & 32 != 0` se parsea como `flags & (32 != 0)` → error de tipos. Escribe
> `(flags & 32) != 0`.

### Todos los símbolos

| Símbolo | Significado |
|---|---|
| `+` | suma (int/float/u\*); **concatena** strings, bytes y arreglos; sobrecargable (trait `Add`) |
| `-` | resta; negación unaria; sobrecargable (`Sub`/`Neg`) |
| `*` `/` `%` | producto, cociente, resto (división/módulo por cero = error de ejecución); `Mul`/`Div` |
| `==` `!=` | igualdad estructural (primitivos, strings, bytes; tipos de usuario vía `Eq`) |
| `<` `<=` `>` `>=` | comparación: números, strings (lexicográfica), chars (code point) |
| `&&` `\|\|` `!` | lógicos (cortocircuito en `&&`/`\|\|`) |
| `&` `\|` `^` `~` `<<` `>>` | bit a bit sobre `int`/`u8`/`u32`/`u64` (semántica *wrapping*; el shift enmascara el ancho) |
| `=` | asignación (sentencia, **no** expresión) |
| `( )` | agrupación, llamadas, tuplas, escrutinio de `match`/`if`/`while` |
| `{ }` | bloques (producen valor), literales de struct, cuerpos |
| `[ ]` | literales de arreglo `[1, 2, 3]` (coma final permitida), tipos `[T]`, indexación `a[i]` |
| `,` `;` `:` | separadores; fin de sentencia; anotación de tipo |
| `.` | campo, método (UFCS), variante de enum (`Option.Some`), módulo (`math.PI`), tupla (`t.0`) |
| `..` | rango en `for i in a..b` (semiabierto) |
| `->` | tipo de retorno de función |
| `=>` | brazo de `match` |
| `?` | propagación de `Err`/`None` (retorna temprano); con `impl From<E1> for E2`, convierte el error |
| `\|>` | pipeline: `x \|> f(a)` ≡ `f(x, a)` |
| `@` | anotaciones: `@test`, `@derive(…)` |
| `_` | comodín en patrones; descarte en `let _ = …` |
| `${…}` | interpolación dentro de un literal de string |
| `//` `///` | comentario de línea; comentario de documentación (`ray doc`, hover del LSP) |
| `b"…"` | literal de bytes |

## 3. Literales y escapes

| Literal | Forma | Notas |
|---|---|---|
| Entero | `42`, `-7` | decimal; debe caber en i64. Sin hex/octal/binario ni `_` (diferidos) |
| Flotante | `3.14` | `dígitos . dígitos` (el `.` exige decimal: `2.0`, no `2.`) |
| Booleano | `true` / `false` | |
| String | `"hola"` | escapes `\n \t \r \\ \" \$`; sin saltos de línea literales |
| String interpolada | `"x = ${expr}"` | `${expr}` = **una** expresión; desazucara a `+ to_string(expr)`. `\${` = literal. `"$5"` y `"{n}"` son literales (el `$` solo es especial ante `{`) |
| Char | `'a'`, `'\n'` | un code point Unicode; escapes `\n \t \r \\ \'` |
| Bytes | `b"ok\x00\xff"` | escapes de string + `\xNN` (octeto en hex) |
| Arreglo | `[1, 2, 3,]` | coma final permitida; `[]` vacío necesita contexto o anotación |
| Tupla | `(1, "a")` | acceso `t.0`, `t.1`; destructuring `let (a, b) = t;` |
| Struct | `Punto { x: 1, y: 2 }` | |
| Enum | `Option.Some(5)`, `Color.Rojo` | |
| Función anónima | `fn(x: int) -> int { x * 2 }` | closure: captura el ámbito por referencia |

## 4. Tipos

| Tipo | Descripción |
|---|---|
| `int` | entero de 64 bits con signo; el desbordamiento aritmético es **error de ejecución** |
| `u8` `u32` `u64` | enteros sin signo; la aritmética **envuelve** (wrapping) por diseño; literales coercionan del contexto (`let x: u8 = 5`) |
| `float` | IEEE 754 de 64 bits |
| `bool` | `true`/`false` |
| `string` | texto UTF-8 inmutable; indexable **por carácter** (`s[i] -> char`) |
| `char` | un code point Unicode |
| `bytes` | secuencia inmutable de octetos; `b[i] -> int` |
| `unit` | "sin valor útil" (retorno de `print`, etc.) |
| `[T]` | arreglo dinámico, semántica de **referencia** |
| `(A, B, …)` | tupla (agregado inmutable, se copia como valor; `t.0 = x` es error) |
| `Map<K, V>` | tabla clave→valor; claves *hashables*: int/u\*/string/char/bool/bytes (**no** float); recorridos en orden de clave (determinista) |
| `fn(A, B) -> R` | tipo función (funciones y closures son valores de primera clase) |
| `Option<T>` / `Result<T, E>` | del prelude; la ausencia y el error como valores (no hay `null` ni excepciones) |
| `Channel<T>` / `Task<T>` | concurrencia (§9 de la SPEC) |
| `struct` / `enum` propios | semántica de referencia (struct/enum); genéricos con bounds (`struct Caja<T: Show>`) |
| `dyn Trait`, `dyn A + B` | trait objects (despacho dinámico); *upcasting* a un subconjunto de traits |
| `Iter<T>` | iterador perezoso (respaldado por closure) |
| `ptr` | puntero opaco del FFI (no desreferenciable desde raylang) |
| `Set<T>` / `Deque<T>` / `StringBuilder` | de `std/collections` (§10) |

**Casts** con `as`: `float as int` (trunca; satura en el borde), `int as float`, `char as int`,
`int as char` (valida el code point), `int ↔ u8/u32/u64` y entre anchos sin signo (enmascaran),
`float ↔ u*`.

## 5. Builtins globales

Disponibles siempre, sin `import`. Los primitivos `__nombre` son **internos e inestables** — no los
uses; cada uno tiene su envoltorio público en el prelude o en `std/`.

> Un builtin **gana** a una función de usuario homónima: no redefinas `print`, `len`, etc.

### Núcleo y salida

| Función | Firma | Descripción |
|---|---|---|
| `print` | `(valor) -> unit` | imprime a stdout + salto de línea (int, float, bool, string, char, u\*, bytes→hex, arreglos, tipos con `Show`) |
| `eprint` | `(valor) -> unit` | como `print`, a stderr |
| `to_string` | `(valor) -> string` | representación textual (misma que `print`): int/float/bool/string/char/bytes/u\* |
| `panic` | `(msg: string) -> unit` | aborta el programa con el mensaje y la posición; para invariantes rotas, no para errores esperables |
| `args` | `() -> [string]` | argumentos de línea de comandos (tras la ruta del programa) |

### Concurrencia (solo VM; §11 del manual)

| Función | Firma | Descripción |
|---|---|---|
| `spawn` | `(f: fn() -> T) -> Task<T>` | lanza una tarea concurrente; `join` espera su valor |
| `join` | `(t: Task<T>) -> T` | bloquea hasta que la tarea termina (re-lanza su fallo). *Ad-hoc*: `join(arr, sep)` es el de strings |
| `scope` | `(body: fn() -> R) -> R` | concurrencia estructurada: al volver une todas las tareas lanzadas dentro; si una falla, cancela a sus hermanas y propaga |
| `send` | `(ch: Channel<T>, v: T) -> unit` | envía; bloquea si el canal acotado está lleno (backpressure) |
| `recv` | `(ch: Channel<T>) -> Option<T>` | recibe; bloquea si vacío y abierto; `None` al cerrar y drenar |
| `select` | `(chs: [Channel<T>]) -> int` | bloquea hasta que un canal esté listo; devuelve el índice menor listo (determinista) |
| `close` | `(ch \| handle) -> …` | cierra un canal (los valores pendientes aún se reciben) **o** un handle de archivo/socket |

### Matemáticas, reloj y azar

También expuestas con nombre calificado en `std/math`, `std/time` y `std/random`.

| Función | Firma | Descripción |
|---|---|---|
| `sqrt` `sin` `cos` `tan` `ln` `log10` `exp` `floor` `ceil` `round` | `(float) -> float` | las de siempre (radianes; `round` mitad-fuera) |
| `pow` | `(base: float, exp: float) -> float` | potencia |
| `abs` | `(int) -> int` · `(float) -> float` | valor absoluto (*ad-hoc* por tipo) |
| `min` / `max` | `(a, b) -> …` | de dos ints o dos floats |
| `pi` / `e` | `() -> float` | constantes (prefiere `math.PI`/`math.E`) |
| `now` | `() -> int` | reloj de pared, ms desde el epoch |
| `monotonic` | `() -> int` | reloj monótono, ms (para medir duraciones) |
| `sleep` | `(ms: int) -> unit` | suspende la fibra actual |
| `random` | `() -> float` | pseudoaleatorio en `[0, 1)` |
| `random_int` | `(n: int) -> int` | pseudoaleatorio en `[0, n)` |

### Cripto de producción (respaldada por `ring`, tiempo constante)

Expuestas con nombre calificado en `std/crypto`.

| Función | Firma | Descripción |
|---|---|---|
| `sha256` / `sha512` | `(bytes) -> bytes` | digest (32/64 octetos) |
| `sha1` | `(bytes) -> bytes` | **legado** (20 octetos): solo para protocolos que lo exigen (WebSocket) |
| `hmac_sha256` | `(key: bytes, msg: bytes) -> bytes` | MAC (32 octetos); base de JWT/SigV4 |
| `ed25519_public_key` | `(seed: bytes) -> Option<bytes>` | clave pública desde la semilla (32 octetos; si no, `None`) |
| `ed25519_sign` | `(seed: bytes, msg: bytes) -> Option<bytes>` | firma (64 octetos) |
| `ed25519_verify` | `(pubkey, msg, sig) -> bool` | verificación (total: nunca falla) |
| `chacha20poly1305_seal` | `(key, nonce, aad, plaintext) -> Option<bytes>` | AEAD: cifra y autentica (`cifrado ‖ etiqueta`) |
| `chacha20poly1305_open` | `(key, nonce, aad, ciphertext) -> Option<bytes>` | descifra; `None` si la autenticación falla |

### Otros

| Función | Firma | Descripción |
|---|---|---|
| `bytes_of` | `([int]) -> bytes` | arma bytes desde octetos 0–255 |
| `char_code` | `(char) -> int` | code point Unicode |
| `range` | `(a: int, b: int) -> Iter<int>` | iterador semiabierto `[a, b)` (del prelude) |
| `sum` | `(Iter<int>) -> int` | suma un iterador de ints (vía UFCS: `it.sum()`) |

## 6. Métodos por tipo de receptor

Se llaman con punto (`recv.metodo(args)`); casi todos son traits del prelude o builtins-método, así
que también existen como llamada libre (`metodo(recv, args)`).

### `string`

| Método | Resultado | Descripción |
|---|---|---|
| `s.len()` | `int` | longitud **en caracteres** |
| `s[i]` | `char` | indexación por carácter (fuera de rango = error); `s[i] = c` está prohibido (inmutable) |
| `s.trim()` | `string` | sin espacios en los bordes |
| `s.split(sep)` | `[string]` | partes |
| `s.contains(sub)` | `bool` | subcadena |
| `s.replace(de, a)` | `string` | reemplaza todas |
| `s.chars()` | `[char]` | caracteres |
| `s.starts_with(p)` / `s.ends_with(p)` | `bool` | prefijo/sufijo |
| `s.to_upper()` / `s.to_lower()` | `string` | mayúsculas/minúsculas |
| `s.substring(i, j)` | `string` | `[i, j)` por carácter, con *clamp* (nunca falla) |
| `s.repeat(n)` | `string` | repetida (`n <= 0` → `""`) |
| `s.index_of(sub)` | `Option<int>` | índice de la primera ocurrencia |
| `s.to_bytes()` | `bytes` | codifica UTF-8 |
| `s.parse_int()` / `s.parse_float()` | `Option<int/float>` | parseo (vía UFCS del prelude) |
| `a + b` | `string` | concatenación |
| `<` `<=` `>` `>=` | `bool` | orden lexicográfico |

### Arreglos `[T]`

| Método | Resultado | Descripción |
|---|---|---|
| `a.len()` | `int` | elementos |
| `a[i]` / `a[i] = v` | `T` / — | indexación y asignación (fuera de rango = error) |
| `a.push(x)` | `unit` | añade al final, **en el sitio** |
| `a.pop()` | `Option<T>` | quita y devuelve el último |
| `a.contains(x)` | `bool` | pertenencia (igualdad estructural) |
| `a.position(x)` | `Option<int>` | índice de la primera ocurrencia |
| `a.reverse()` | `[T]` | copia invertida |
| `a.sort()` | `[T]` | copia ordenada (`T: Ord`) |
| `a.join(sep)` | `string` | solo `[string]` |
| `a.map(f)` / `a.filter(p)` / `a.fold(init, f)` | eager | materializan un arreglo/valor (§9 para la versión perezosa) |
| `a.iter()` | `Iter<T>` | iterador perezoso |
| `a + b` | `[T]` | concatenación |

### `bytes`

| Método | Resultado | Descripción |
|---|---|---|
| `b.len()` | `int` | octetos |
| `b[i]` | `int` | octeto (0–255) |
| `b.sub_bytes(i, j)` | `bytes` | rebanada `[i, j)` con *clamp* |
| `from_utf8(b)` | `Result<string, string>` | decodifica UTF-8 |
| `b1 + b2` | `bytes` | concatenación |
| `to_string(b)` / `print(b)` | hex | representación hexadecimal |

### `Map<K, V>`

| Método | Resultado | Descripción |
|---|---|---|
| `m.len()` | `int` | entradas |
| `m.insert(k, v)` | `unit` | inserta/actualiza, en el sitio |
| `m.get(k)` | `Option<V>` | consulta |
| `m.remove(k)` | `Option<V>` | quita y devuelve |
| `m.contains_key(k)` | `bool` | |
| `m.keys()` / `m.values()` | `[K]` / `[V]` | **ordenados por clave** (determinista); casan posición a posición |
| `for (k, v) in m` | — | recorrido en orden de clave |

### `Task<T>` y `Channel<T>`

`t.join()`, `ch.send(v)`, `ch.recv()`, `ch.close()`, `chs.select()` — los builtins de concurrencia
vía UFCS.

## 7. Funciones asociadas a tipos

Constructores con la sintaxis `Tipo.funcion(…)`:

| Función | Firma | Descripción |
|---|---|---|
| `Map.new` | `() -> Map<K, V>` | mapa vacío (el tipo lo fija el contexto: `var m: Map<string, int> = Map.new();`) |
| `Channel.new` | `() -> Channel<T>` | canal sin límite (send nunca bloquea) |
| `Channel.bounded` | `(n: int) -> Channel<T>` | canal acotado a `n` (backpressure; `n = 0` = rendezvous síncrono) |

## 8. El prelude

Funciones y traits **escritos en raylang**, inyectados en todo programa (puedes *override*
definiendo el mismo nombre).

### Funciones

| Función | Firma | Descripción |
|---|---|---|
| `parse_int` / `parse_float` | `(string) -> Option<int/float>` | parseo |
| `char_from_code` | `(int) -> Option<char>` | inverso de `char_code` (valida el code point) |
| `input` | `() -> Option<string>` | una línea de stdin (`None` en EOF) |
| `read_int` | `() -> Option<int>` | `input` + `parse_int` |
| `env` | `(name: string) -> Option<string>` | variable de entorno |
| `map` / `filter` / `fold` | eager sobre `[T]` | ver §6 |
| `sort` | `([T]) -> [T]` con `T: Ord` | insertion sort estable |
| `iter` / `range` / `sum` | iteradores | ver §9 |
| `get` / `remove` | sobre `Map` | ver §6 |
| `recv` | `(Channel<T>) -> Option<T>` | ver §5 |
| `assert` | `(bool) -> unit` | aborta si es falso |
| `assert_eq` | `(a: T, b: T)` con `T: Eq + Show` | aborta mostrando ambos valores |
| `pop` / `position` / `index_of` / `from_utf8` | — | ver §6 |

### Traits

| Trait | Método(s) | Notas |
|---|---|---|
| `Eq` | `eq(self, other: Self) -> bool` | habilita `==`/`!=` en tipos de usuario; derivable |
| `Show` | `show(self) -> string` | habilita `print`/`to_string`; derivable |
| `Ord` | `less(self, other: Self) -> bool` | habilita `sort`/`min`/`max`; impls para int/float/string/char |
| `Hash` | `hash(self) -> int` | claves de `Set`; derivable |
| `Add` `Sub` `Mul` `Div` | `add/sub/mul/div(self, other: Self) -> Self` | sobrecarga de `+ - * /` en tipos de usuario |
| `Neg` | `neg(self) -> Self` | sobrecarga del `-` unario |
| `From<S>` | `desde(source: S) -> Self` | conversión; la usa `?` para convertir errores (`from` es keyword → el método se llama `desde`) |
| `Iterator<T>` | `next(self) -> Option<T>` | el protocolo de iteración; trae los adaptadores como métodos por defecto |
| `Len` / `Push<T>` / `Contains<T>` | `len`/`push`/`contains` | los métodos de contenedor, como traits |
| `Signed` | `abs(self) -> Self` | para el `abs` genérico de `std/math` |

`Option<T>` (`Some`/`None`) y `Result<T, E>` (`Ok`/`Err`) son enums del prelude.

## 9. Iteradores

`xs.iter()` y `range(a, b)` producen un `Iter<T>` **perezoso**: los adaptadores no calculan nada
hasta que un terminal recorre la cadena (una sola pasada, sin arreglos intermedios).

| Adaptador (perezoso) | Descripción |
|---|---|
| `.map(f)` | transforma cada elemento |
| `.filter(pred)` | deja pasar los que cumplen |
| `.take(n)` / `.skip(n)` | corta / salta los primeros n |
| `.enumerate()` | pares `(índice, elemento)` — se consumen con patrón de tupla: `for (i, x) in …` |
| `.zip(otro)` | empareja dos iteradores en `(T, U)`; se agota con el más corto |

| Terminal | Descripción |
|---|---|
| `.fold(init, f)` | reduce a un valor |
| `.collect()` | materializa a `[T]` |
| `.sum()` | suma (`Iter<int>`) |
| `for x in it { … }` | recorre |

Un tipo tuyo se vuelve iterable implementando `Iterator<T>` (solo `next`); hereda todos los
adaptadores.

## 10. La biblioteca estándar `std/`

**Embebida en el binario** (funciona sin archivos en disco). Se importa por ruta y se usa
calificado por el *leaf*: `import std/math;` → `math.gcd(12, 18)`.

| Módulo | Superficie pública |
|---|---|
| `std/math` | `PI` `E` · `sqrt pow sin cos tan ln log10 exp floor ceil round` · `abs<T: Signed>` `min<T: Ord>` `max<T: Ord>` · `iabs sign clamp gcd lcm ipow factorial is_prime` · `float_bits float_from_bits` (bits IEEE 754) |
| `std/text` | `is_empty pad_left pad_right capitalize reverse count words` |
| `std/sort` | `is_sorted sort_desc min max binary_search dedup merge` (todas con `T: Ord`) |
| `std/fs` | `read_file write_file append_file remove_file list_dir exists read_file_bytes write_file_bytes` (→ `Result`) · handles: `open(path, "r"/"w"/"a") -> Result<int, _>` `read_line(h) -> Option<string>` `write(h, s)` + `close(h)` |
| `std/net` | `tcp_connect tcp_listen tcp_accept local_port` · `socket_read socket_write socket_read_bytes socket_write_bytes` · `tls_connect tls_connect_h2 tls_accept tls_upgrade` (STARTTLS) — todo `Result`; + `close(h)` |
| `std/time` | `now monotonic sleep` + fechas civiles UTC (M57.1): `DateTime`, `now_utc`, `from_epoch_millis`/`to_epoch_millis`, `to_iso8601[_basic]`, `date_stamp`, `to_rfc1123`, `parse_iso8601[_millis]` (RFC 3339 con offset/fracción), `format_duration` (`net/time` queda como reexport) |
| `std/random` | `next() -> float` · `below(n) -> int` |
| `std/crypto` | los builtins de cripto (§5) con nombre calificado |
| `std/collections/set` | `Set<T>` (exige `T: Hash + Eq`): `new add has remove size items` → `set.new()`, `set.add(s, x)`… |
| `std/collections/deque` | `Deque<T>`: `new len is_empty push_back push_front pop_front pop_back peek_front` |
| `std/collections/stringbuilder` | `StringBuilder`: `new push build count` (une una vez; evita el O(n²) de `+` en bucle) |
| `std/json` | `enum Json` (`JNull JBool JNum JStr JArray JObject`) · `parse -> Result<Json, string>` · `stringify` (canónico, claves ordenadas). Escapes `\uXXXX` con pares surrogate |
| `std/hex` | `hex_encode([int]) -> string` · `hex_decode -> Result<[int], string>` |
| `std/base64` | `base64 base64url` (`[int] -> string`) · `base64_decode base64url_decode` |
| `std/url` | `url_encode url_decode parse_query build_query` |
| `std/regex` | motor Thompson NFA (tiempo lineal): `full_match search find find_str find_all replace_all`. Soporta `. * + ? \| ( ) [a-z] [^…] \d \w \s ^ $` |
| `std/csv` | `parse_csv -> Result<[[string]], string>` (RFC 4180) · `write_csv` |
| `std/toml` | `parse_toml toml_get toml_show` (subconjunto: tablas, escalares, arrays) |
| `std/template` | plantillas estilo Jinja: `compile(tpl) -> Result<Template, _>` + `render(t, ctx)` (SSR: compilar 1 vez) · `render_template` (una vez) · `{{ var }}` (autoescape), `{{& var }}`, `{% if/elif/else %}`, `{% for %}` · contexto: `ctx_str ctx_int ctx_bool ctx_list val_str val_int` · `escape_html` · templates COMPILADOS: `ray templ` (§14) |
| `std/inflate` | `inflate_raw gunzip zlib_inflate` (→ `Result<bytes, string>`) · `crc32` |
| `std/deflate` | `deflate_raw gzip_compress zlib_compress` |
| `std/huffman` | `huffman_encode huffman_decode` (la tabla HPACK del RFC 7541) |
| `std/protobuf` | `PbWriter`: `writer write_varint write_string write_bytes write_fixed64 write_fixed32 finish` · `parse -> Result<[PbField], _>` `get_int get_bytes get_string` · framing gRPC: `grpc_frame grpc_unframe` |
| `std/uuid` | `uuid_v4() -> string` · `is_uuid_v4` · `uuid_v7()`/`uuid_v7_at(ms)` (RFC 9562, ordenables por tiempo) · `is_uuid_v7` |

## 11. Paquetes adicionales (`net`, `db`)

Tier 2: **no** van en el binario; se declaran en `ray.toml` (por ruta o git) y se importan igual
(`import net/http;` → `http.fetch(…)`). Viven en `packages/` del repo.

### `packages/net` — la pila de red (23 módulos, raylang puro)

| Grupo | Módulos |
|---|---|
| HTTP | `http` (fetch/request, redirects, chunked, gzip, https) · `http2` + `hpack` (framing/HPACK) · `http2_client` · `webserver` (servidor async + SSE) |
| RPC | `grpc_client` (gRPC unario e2e sobre TLS+ALPN h2) |
| Tiempo real | `websocket` (servidor) · `websocket_client` (ws/wss) |
| Auth/identidad | `jwt` (HS256) · `jwt_eddsa` (EdDSA) · `oauth2` (client_credentials) · `scram` (SCRAM-SHA-256) · `sigv4` (AWS) · `cookie` |
| Infra | `dns` + `dns_cache` (A/AAAA/MX/CNAME/TXT/NS/SRV) · `udp` · `redis` (RESP2) · `postgres` (consulta simple; el cliente completo está en `db`) |
| Observabilidad | `log` (JSON estructurado) · `metrics` (Prometheus) · `time` (DateTime UTC, ISO 8601/RFC 1123) |
| Cripto | `crypto` (adaptadores de los builtins para el resto del paquete) |

### `packages/db` — clientes de bases de datos

| Módulo | Qué da |
|---|---|
| `db/mysql` | wire v10: `connect`/`connect_tls` · `query`/`exec` con `?` (prepared/binario) o texto · auth native + caching_sha2 (full-path por TLS) |
| `db/postgres` | wire v3 extendido: `connect`/`connect_tls` (sslRequest) · `query`/`exec` con `$1, $2…` · SCRAM |
| `db/sqlite` | embebido (rusqlite en el host): `connect(":memory:" \| ruta)` · `query`/`exec` con `?1…` · `last_insert_rowid` |
| `db/mongo` | OP_MSG + BSON: `connect`/`connect_tls` · `insert find update delete` (filtros = documentos BSON) · `run_command` · cursores completos (getMore) |
| `db/bson` | `enum Bson` · `encode`/`decode` · `dump` · puente JSON (`doc_from_json from_json to_json`) |

API uniforme en los 4 clientes: `connect → Conn`, `query → Result<[[string]], string>` (mongo:
documentos), `exec → Result<int, string>`, `disconnect`. Binding de parámetros (anti-inyección) en
todos.

## 12. Anotaciones

| Anotación | Sobre | Efecto |
|---|---|---|
| `@test` | `fn () -> bool` o `fn () -> unit` | la corre `ray test`: bool pasa si `true`; unit pasa si no dispara `assert`/`panic`. Cada test corre aislado |
| `@derive(Eq)` | struct/enum no genérico | genera `impl Eq` (igualdad estructural) |
| `@derive(Show)` | struct/enum no genérico | genera `impl Show` (`Nombre { c: v }` / `Nombre.Variante(v)`); soporta enums recursivos |
| `@derive(Hash)` | struct/enum no genérico | genera `impl Hash` (para claves de `Set`) |

Se combinan: `@derive(Eq, Show, Hash)`.

## 13. FFI: tipos marshalables

`extern "lib" { fn nombre(args) -> ret; }` declara funciones C (dlopen/LoadLibrary en runtime).
Aridad **0 a 3**. Es la única frontera insegura del lenguaje: la firma declarada se **confía**.

| Tipo raylang | En C (argumento) | En C (retorno) |
|---|---|---|
| `int` | entero por registro | `int` de C (32 bits, extiende signo) |
| `u64` | entero por registro | `long`/`size_t` (64 bits) |
| `float` | `double` | `double` |
| `bool` | `int` | `int` |
| `unit` | — | `void` |
| `string` | `char*` NUL-terminado (copia temporal) | ✗ (usa `Option<string>`) |
| `bytes` | puntero al buffer (NUL-termínalo tú: `b"…\x00"`) | ✗ (usa `Option<bytes>`) |
| `ptr` | puntero opaco | puntero opaco |
| `Option<string>` / `Option<bytes>` | ✗ | `char*`: NULL → `None`; si no, **copia** hasta el NUL (nunca libera) |
| `Option<ptr>` | ✗ | puntero: NULL → `None` |

Fuera de contrato: funciones **variádicas** (`printf` — UB en arm64), structs por valor, callbacks
(anotados para un FFI v2). No disponible en el playground wasm.

## 14. El CLI `ray`

(`raylang` es un alias del mismo binario.)

| Comando | Qué hace |
|---|---|
| `ray new <nombre>` | crea un proyecto (ray.toml + src/main.ray + .gitignore) |
| `ray run [archivo] [args…]` | ejecuta (por defecto `src/main.ray`); resuelve dependencias |
| `ray build [archivo]` | chequea y compila sin ejecutar (0 ok / 65 error) |
| `ray test [archivo] [filtro]` | corre las `@test` (filtro por subcadena del nombre) |
| `ray fmt <archivo>` | imprime la versión canónica |
| `ray templ <ruta>…` | compila templates `.ray.html` (firma `{% params %}`) a módulos raylang tipados |
| `ray doc <archivo>` | documentación Markdown de la superficie pública (`///`) |
| `ray repl` | REPL interactivo |
| `ray lsp` | Language Server (diagnósticos, hover, ir-a-definición, references, rename, completion, signature help) |
| `ray add <nombre>[@req]` | añade una dependencia del registro (`1.2.0`, `^1.2`, `~1.2.3`, `*`) |
| `ray remove <nombre>` | la elimina (y su caché si nadie más la usa) |
| `ray search [patrón]` | lista paquetes del registro |
| `ray fetch` | descarga las dependencias a `.ray-deps/` |
| `ray update` | re-resuelve a las versiones más nuevas compatibles |
| `ray publish [--repo <spec>] [--sign]` | publica esta versión en el registro (valida + check semántico + hash; `--sign` la firma Ed25519 y reclama/verifica el dueño del nombre) |
| `ray keygen [--out F]` | genera la clave Ed25519 de publicación (`RAY_KEY` o `~/.ray/publish.key`) |
| `ray index-verify [dir]` | audita las firmas de un índice contra sus dueños (CI del repo del índice) |
| `ray yank <nom>@<ver> [--undo]` | retira/restaura una versión publicada |
| `ray version` | versión |

Flags de `run`:

| Flag | Efecto |
|---|---|
| `--interp` | fuerza el intérprete (oráculo de desarrollo; sin concurrencia) |
| `--deterministic` | scheduler M:1 reproducible (un hilo, FIFO); también `RAYLANG_THREADS=1` |
| `--fuel N` | límite de instrucciones de la VM (para embeber confinado) |
| `--heap N` | tope de objetos vivos del heap (fuerza GC; si no basta, aborta) |

Variables de entorno: `SSL_CERT_FILE` (CAs extra para TLS), `RAY_INDEX` (registro de paquetes),
`RAYLANG_THREADS`.

## 15. Códigos de salida

| Código | Significado |
|---|---|
| retorno de `main` | un `main -> int` sale con ese valor; `main -> unit` sale con 0 |
| 64 | uso incorrecto del CLI |
| 65 | error de compilación (léxico/sintaxis/tipos) o de configuración |
| 66 | archivo de entrada no encontrado |
| 70 | error de ejecución (panic, overflow, índice fuera de rango, deadlock…) |
| 73 | no se pudo crear un archivo (`ray new`) |
| 101 | ICE (error interno del compilador — repórtalo) |
| nº de fallos | `ray test` sale con la cantidad de tests fallidos |
