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
11. [Paquetes adicionales (`net`, `rpc`, `db`)](#11-paquetes-adicionales-net-rpc-db)
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
| `try_recv` | `(ch: Channel<T>) -> Received<T>` | recibe **sin bloquear**: `Received.Got(v)` (valor listo, lo consume), `Received.Empty` (abierto y vacío), `Received.Closed` (cerrado y drenado). Para "revisa datos O una orden de control sin quedarte bloqueado" |
| `select_timeout` | `(chs: [Channel<T>], ms: int) -> Option<int>` | `select` con **plazo**: `Some(i)` (índice menor listo), `None` si vencen los `ms` ms; `ms <= 0` = poll no bloqueante. Event-driven (despierta al llegar un canal, no sondea) |
| `signals` | `() -> Channel<int>` | M88.1/M107.4: el canal de señales del SO (SIGTERM=15, SIGINT=2, SIGWINCH=28); singleton del proceso, para el apagado ordenado y el re-maquetado al redimensionar (`select` + `term.size()`) — compone con `recv`/`select`. Unix; VM y binario nativo |
| `close` | `(ch \| handle) -> …` | cierra un canal (los valores pendientes aún se reciben) **o** un handle de archivo/socket |

### Recuperación de fallos

| Función | Firma | Descripción |
|---|---|---|
| `try_call` | `(f: fn() -> T) -> Result<T, string>` | ejecuta `f` y convierte un `panic`/error de ejecución en `Err(mensaje)`. Recupera en la **misma fibra**: lo que `f` mutó sigue mutado (como el `catch_unwind` de Rust). Los tres motores |
| `try_join` | `(t: Task<T>) -> Result<T, string>` | el fallo de una tarea como valor, en vez de re-lanzarlo. Aísla de verdad (heap propio de la fibra). Solo VM |

> ⚠️ **Matemáticas, reloj, azar, cripto, disco y red NO son builtins globales.** Viven en módulos
> `std/` desde M49/M50 y se usan calificados: `math.sqrt(2.0)`, `time.now()`, `random.below(10)`,
> `crypto.sha256(b)`, `fs.read_file(p)`, `net.tcp_connect(h, p)`. Catálogo en §10.

### Entrada y entorno

| Función | Firma | Descripción |
|---|---|---|
| `env` | `(name: string) -> Option<string>` | variable de entorno; `None` si no está definida |
| `input` | `() -> Option<string>` | una línea de stdin (sin el salto); `None` en EOF |
| `read_int` | `() -> Option<int>` | una línea de stdin parseada como entero |

### Otros

| Función | Firma | Descripción |
|---|---|---|
| `bytes_of` | `([int]) -> bytes` | arma bytes desde octetos 0–255 |
| `char_code` | `(char) -> int` | code point Unicode |
| `char_from_code` | `(int) -> Option<char>` | la inversa; `None` si no es un code point válido |
| `range` | `(a: int, b: int) -> Iter<int>` | iterador semiabierto `[a, b)` (del prelude) |
| `iter` | `(xs: [T]) -> Iter<T>` | iterador perezoso sobre un arreglo |
| `sum` / `sum_float` | `(Iter<int>) -> int` · `(Iter<float>) -> float` | suma un iterador (vía UFCS: `it.sum()`) |
| `min` / `max` | `(Iter<T: Ord>) -> Option<T>` | **terminales de iterador** (no son el mínimo de dos valores: eso es `math.min`) |
| `sort` | `(xs: [T: Ord]) -> [T]` | ordena un arreglo (copia ordenada) |
| `assert` / `assert_eq` | `(bool)` · `(a: T, b: T)` | aserciones del runner de tests; fallan con `panic` |

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

`t.join()`, `ch.send(v)`, `ch.recv()`, `ch.try_recv()`, `ch.close()`, `chs.select()` — los builtins
de concurrencia vía UFCS. `try_recv` devuelve `Received<T>` (`enum Received<T> { Got(T), Empty,
Closed }` del prelude): recepción no bloqueante.

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
| `map` / `filter` / `fold` / `any` / `all` | eager sobre `[T]` | ver §6 |
| `sort` | `([T]) -> [T]` con `T: Ord` | merge sort bottom-up, estable, O(n log n); devuelve un arreglo nuevo |
| `iter` / `range` / `sum` / `sum_float` / `min` / `max` | iteradores | ver §9 (`min`/`max` son terminales: `Iter<T> -> Option<T>`) |
| `get` / `get_or` / `remove` | sobre `Map` | ver §6 |
| `try_call` / `try_join` | recuperación de fallos | ver §5 |
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
| `From<S>` | `convert(source: S) -> Self` | conversión; la usa `?` para convertir errores (`from` es keyword → el método se llama `convert`) |
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
| `std/fs` | `read_file write_file append_file remove_file list_dir exists read_file_bytes write_file_bytes` (→ `Result`) · handles: `open(path, "r"/"w"/"a") -> Result<int, _>` `read_line(h) -> Option<string>` `write(h, s)` + `close(h)` · **streaming** (M113): `read_bytes(h, max) -> Result<Option<bytes>, string>` (hasta `max` octetos desde la posición actual — exactos salvo cerca del final; `None` = EOF; memoria acotada por lo leído) · `seek(h, pos) -> Result<int, string>` (posición absoluta desde el inicio; devuelve la nueva posición → transferencias reanudables) · **durabilidad** (M115.1): `write_bytes(h, data) -> Result<int, string>` (octetos crudos en la posición actual del handle — el gemelo binario de `write`; compone con `seek`) · `sync(h) -> Result<int, string>` (vuelca los búferes Y fuerza el archivo a almacenamiento estable — fsync; sin él un append es durable ante el crash del proceso pero no ante un corte de luz) · **candados** (M115.2): `try_lock(h) -> Result<bool, string>` (candado consultivo EXCLUSIVO sin bloquear — flock; `true` = adquirido, `false` = lo tiene otra open file description; el patrón LOCK-file del proceso único) · `unlock(h) -> Result<int, string>` (`close(h)` también lo suelta) · **metadatos** (M115.3): `stat(path) -> Result<Stat, string>` — SIN seguir symlinks (lstat): `Stat { kind, mode, size, mtime_ms }` con kind `"file"`/`"dir"`/`"symlink"`/`"other"`, mode = los 12 bits de permiso en decimal (0o600 = 384), size en bytes (de un symlink: la longitud del propio enlace), mtime en epoch-ms · `chmod(path, mode) -> Result<int, string>` (cambia los bits de permiso; 384 = 0o600, 493 = 0o755) · **watch** (M115.4, eventos de KERNEL — FSEvents/inotify, no sondeo de mtimes): `watch(path) -> Result<int, string>` (directorio → recursivo; archivo → él mismo; `close(h)` lo detiene) · `next_event(h) -> Result<WatchEvent, string>` (espera lo que haga falta — la fibra APARCA, el proceso duerme) · `next_event_timeout(h, ms) -> Result<Option<WatchEvent>, string>` (`None` = plazo vencido; útil para agrupar ráfagas) · `WatchEvent { kind, path }` con kind `"create"`/`"modify"`/`"remove"`/`"rename"`/`"other"` — los kinds pueden ser gruesos según la plataforma: trata el evento como "algo cambió aquí" y re-examina |
| `std/io` | consola por bytes. Escritura **sin salto de línea**: `write(s)` / `ewrite(s)` (stderr) / `write_bytes(b)` → `Result<int, string>` (nº de caracteres/bytes) · `flush() -> Result<int, string>`. stdout va con buffer: tras un `write` sin `\n`, llama `flush()` para verlo; `ewrite` es visible al instante; `write_bytes` no pasa por UTF-8. Lectura: `read(max) -> Option<bytes>` (1..=max octetos; `None` = EOF) · `read_timeout(max, timeout_ms) -> ReadResult` (`Data(bytes)` \| `Eof` \| `TimedOut`; `0` = sondeo puro). En la VM una lectura sin datos **aparca la fibra**, no la VM; un solo lector de stdin a la vez; no mezclar con `input()`/`fs.read_line` de stdin (aquél lee bufferizado, esto lee el fd crudo). El orden respecto a `print`/`eprint` es el de programa |
| `std/term` | el terminal. `is_tty(fd) -> bool` (0/1/2) · `size() -> Option<(int, int)>` (cols, rows) · `raw<T>(f: fn() -> T) -> Result<T, string>` — corre `f` en modo **crudo** (sin eco, byte a byte, sin señales) y restaura SIEMPRE (también si `f` falla, y al salir el proceso vía `atexit`; una señal fatal/`kill -9` deja el terminal crudo → `reset`) · `read_key() -> Option<Key>` (una tecla; `None` = EOF; un ESC suelto se resuelve tras 25 ms) · `decode(b: bytes) -> Option<(Key, int)>` — el decodificador **puro** (tecla + octetos consumidos; `None` = prefijo incompleto), para procesar ráfagas o probar sin tty · `enum Key { Char(char) Enter Tab Backspace Esc Up Down Left Right Home End PageUp PageDown Insert Delete Ctrl(char) F(int) }`. En modo crudo no hay OPOST: termina las líneas con `\r\n` explícito |
| `std/net` | `tcp_connect tcp_listen tcp_accept local_port` · `socket_read socket_write socket_read_bytes socket_write_bytes` · `tls_connect tls_connect_h2 tls_accept tls_upgrade` (STARTTLS) — todo `Result`; + `close(h)` |
| `std/process` | Procesos del SO, **sin shell** (argv tipado). `run(program, args) -> Result<Output, string>` · `cmd(program, args) -> Cmd` + builder encadenable `.dir .env .env_clear .stdin(bytes)` (se escribe y se CIERRA; sin él, el hijo lee `/dev/null`) `.timeout_ms .max_output .merge_output .run()`. `Err` = solo "no se pudo lanzar"; salir ≠ 0 o morir por señal es `Ok`. `Output { exit: Exit, stdout: bytes, stderr: bytes, timed_out, truncated }` con `Exit.Code(int)` \| `Exit.Signal(int)` (nunca `128+sig`). El timeout devuelve el Output PARCIAL con `timed_out` tras matar al GRUPO del hijo; `truncated` marca el tope de captura (~16 MB por defecto). **Streaming** (VM/nativo): `.stream() -> Result<Proc, string>` con `Proc { out, err: Channel<bytes>, … }` (canales ACOTADOS = contrapresión; su cierre = fin del flujo; con merge, `err` nace cerrado) · `Proc.wait() -> Exit` (cosecha; una vez) · `Proc.kill(force)` (señal al GRUPO; no-op tras wait). **Sesión persistente** (M100 v3): `.stdin_pipe()` deja el stdin del hijo ABIERTO y `Proc.write(bytes) -> Result<int, string>` / `Proc.close_stdin()` lo alimentan mientras vive — lo que necesita un cliente MCP/LSP o un driver de REPL (escribir petición → leer respuesta → repetir). `write` escribe TODO el dato y **aparca la fibra** si el pipe se llena (contrapresión); un hijo que cerró su stdin o murió da `Err` (EPIPE visible, no un silencio); `close_stdin` ES el EOF que el hijo espera. Tiene precedencia sobre `.stdin(data)`. El proceso es HIJO DE SCOPE: una hermana que falla lo mata y cosecha, y uno sin `wait()` no sobrevive a su scope. `stream()` no tiene `timeout_ms`/`max_output` (el canal es el tope; el plazo se compone con deadline + kill). Unix; en Windows `Err` honesto |
| `std/time` | `now monotonic sleep` + fechas civiles UTC (M57.1): `DateTime`, `now_utc`, `from_epoch_millis`/`to_epoch_millis`, `to_iso8601[_basic]`, `date_stamp`, `to_rfc1123`, `parse_iso8601[_millis]` (RFC 3339 con offset/fracción), `format_duration` (`net/time` queda como reexport) · constructores de duración → **ms** (la moneda de la stdlib): `millis seconds minutes hours days` — importados sin calificar habilitan la forma UFCS `30.seconds()`, `2.hours()` |
| `std/units` | constructores de tamaño → **bytes**, convención binaria (1 KB = 1024): `kb mb gb` — importados sin calificar habilitan la forma UFCS `64.kb()`, `16.mb()` |
| `std/random` | `next() -> float` (en `[0,1)`) · `below(n) -> int` · `between(lo, hi) -> int` · `choice(xs) -> Option<T>` · `shuffle(xs)` (baraja **en sitio**, devuelve unit) · `seed(n)` (secuencia reproducible) |
| `std/crypto` | cripto de **producción** (respaldada por `ring`, tiempo constante): `sha256 sha512 sha1` (`bytes -> bytes`; `sha1` es legado, solo para protocolos que lo exigen) · `hmac_sha256(key, msg) -> bytes` · `ed25519_public_key(seed)` / `ed25519_sign(seed, msg)` → `Option<bytes>` (`None` si la semilla no mide 32 octetos) y `ed25519_verify(pubkey, msg, sig) -> bool` (total) · `chacha20poly1305_seal(key, nonce, aad, plain)` → `Option<bytes>` (`cifrado ‖ etiqueta`) y `chacha20poly1305_open(…)` (`None` si falla la autenticación) · `random_bytes(n)` (CSPRNG) · **acuerdo de claves** (respaldado por `x25519-dalek`): `x25519_public_key(secret)` / `x25519_shared_secret(secret, peer_public)` → `Option<bytes>` (`None` si alguna clave no mide 32 octetos, o si la pública del par es de orden pequeño — salida toda-ceros) · `hkdf_sha256(salt, ikm, info, len)` → `Option<bytes>` (RFC 5869; `None` fuera de `1..=8160`; `info` distinto → clave independiente) · `constant_time_eq(a, b) -> bool` (total). El secreto de `x25519_shared_secret` es el DH **crudo**: pasa siempre por `hkdf_sha256` antes de usarlo como clave AEAD. Las versiones en raylang puro de `examples/web/` son demostración, no producción |
| `std/resilience` | M88.2, el kit de resiliencia para servicios: `Retry`/`policy(attempts, base_ms, max_ms)` + `retry<T,E>(p, f)` (backoff exponencial + jitter; devuelve el primer `Ok` o el último `Err`) · `Breaker`/`breaker(threshold, cooldown_ms)` + `guard<T,E>(b, err_open, f)` (circuit breaker fail-fast; el error de circuito abierto lo aporta el llamador) + `is_open` · `Deadline`/`deadline(ms)` + `remaining expired` (presupuesto de tiempo monótono; aplícalo a la E/S con `net.set_read_timeout(h, remaining(d))`) |
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
| `std/template` | plantillas estilo Jinja: `compile(tpl) -> Result<Template, _>` + `render(t, ctx)` (SSR: compilar 1 vez) · `render_template` (una vez) · `{{ var }}` (autoescape), `{{& var }}`, `{% if/elif/else %}`, `{% for %}` · contexto: `ctx_str ctx_int ctx_bool ctx_list val_str val_int` · `escape_html` · templates COMPILADOS: `.ray.html` importables, compilados en memoria (§14) |
| `std/markdown` | Markdown → AST tipado + HTML (M111). `parse(md) -> [Block]` · `render(blocks) -> string` · `to_html(md) -> string` · `parse_inline(s) -> [Inline]`. `enum Block { Heading(int, [Inline]) Paragraph Code(lang, texto) Quote([Block]) List(ordered, start, [[Block]]) Rule Table(aligns, header, rows) }` · `enum Inline { Text Code Emph Strong Link([Inline], href) Image(alt, src) }`. Subconjunto CommonMark: ATX `#`..`######`, párrafos, código cercado con lenguaje, listas anidadas (-/*/+ y `1.`; cambio de tipo = lista nueva; el número del primer marcador es el `start`: `2.` → `<ol start="2">`), citas `>`, `---`, **tablas GFM** (cabecera + separadora `|---|:--:|`; `\|` escapado; filas cortas rellenadas; sin separadora no hay tabla), diagramas **Mermaid** (una cerca ` ```mermaid ` emite `<pre class="mermaid">` con el texto escapado — el render es client-side con mermaid.js; en el AST sigue siendo `Code("mermaid", …)`), énfasis/negrita (el `_` intra-palabra NO crea énfasis — regla 17: `snake_case_name` va literal)/`código`/[enlaces]/![imágenes], escapes `\`. **Seguridad por diseño**: el HTML embebido se ESCAPA (no se interpreta) y las URLs `javascript:`/`vbscript:`/`data:` no-imagen se neutralizan a `#` — la salida se puede servir sin sanitizador. Fuera de v1: setext, footnotes, saltos duros |
| `std/inflate` | `inflate_raw gunzip zlib_inflate` (→ `Result<bytes, string>`) · `crc32` |
| `std/deflate` | `deflate_raw gzip_compress zlib_compress` |
| `std/huffman` | `huffman_encode huffman_decode` (la tabla HPACK del RFC 7541) |
| `std/protobuf` | `PbWriter`: `writer write_varint write_string write_bytes write_fixed64 write_fixed32 finish` · `parse -> Result<[PbField], _>` `get_int get_bytes get_string` · framing gRPC: `grpc_frame grpc_unframe` |
| `std/uuid` | `uuid_v4() -> string` · `is_uuid_v4` · `uuid_v7()`/`uuid_v7_at(ms)` (RFC 9562, ordenables por tiempo) · `is_uuid_v7` |
| `std/ffi` | `errno() -> int`: el `errno` del hilo — el motivo del último fallo de una extern C estilo POSIX (`fopen`/`unlink`…). **Leerlo inmediatamente** tras la llamada, sin E/S en medio (§13). En wasm: 0 |

## 11. Paquetes adicionales (`net`, `rpc`, `db`)

Tier 2: **no** van en el binario; se declaran en `ray.toml` (por ruta o git) y se importan igual
(`import net/http;` → `http.fetch(…)`). Viven en `packages/` del repo.

### `packages/net` — la pila de red (24 módulos, raylang puro)

| Grupo | Módulos |
|---|---|
| HTTP | `http` (fetch/request, redirects, chunked, gzip, https; **streaming** M108: `stream[_with](method, url, body, headers[, idle_ms]) -> Result<Stream, _>` — status/cabeceras ya, `stream_read(s) -> Result<Option<bytes>, _>` entrega cada trozo al llegar, des-chunkeado incremental, `Ok(None)` = fin limpio; `stream_close`) · `sse` (cliente Server-Sent Events sobre `stream`: `open(url, headers)` + `next(es) -> Result<Option<Event>, _>` con `Event { data, event, id }`, y el decodificador **puro** `decode(bytes) -> Option<(Event, int)>`) · `http2` + `hpack` (framing/HPACK) · `http2_client` · `webserver` (servidor async + SSE; apagado ordenado M88.1b: `serve_graceful(host, port, drain_ms, handler)` sobre `signals()`, forma general `serve_shutdown[_limits]` con canal `stop`) |
| RPC | `grpc_client` (gRPC unario e2e sobre TLS+ALPN h2) |
| Tiempo real | `websocket` (servidor) · `websocket_client` (ws/wss) |
| Auth/identidad | `jwt` (HS256) · `jwt_eddsa` (EdDSA) · `oauth2` (client_credentials) · `scram` (SCRAM-SHA-256) · `sigv4` (AWS) · `cookie` |
| Infra | `dns` + `dns_cache` (A/AAAA/MX/CNAME/TXT/NS/SRV) · `udp` · `redis` (RESP2) · `postgres` (consulta simple; el cliente completo está en `db`) |
| Observabilidad | `log` (JSON estructurado; `with_trace` estampa `trace_id` en cada línea, M88.3) · `metrics` (Prometheus) · `time` (DateTime UTC, ISO 8601/RFC 1123) · `trace` (W3C Trace Context: `Trace`, `new_trace`/`child`/`traceparent`/`parse_traceparent`/`from_headers`; el webserver lo adopta con `trace_of(req)` y el cliente http lo propaga con `request_traced`/`fetch_traced`) |
| Cripto | `crypto` (adaptadores de los builtins para el resto del paquete) |

### `packages/rpc` — RPC raylang↔raylang (M88.4)

| Pieza | Superficie |
|---|---|
| Protocolo | frame = 4 octetos BE de longitud + payload JSON: petición `{"id","method","params"[,"deadline_ms","traceparent"]}` → respuesta `{"id","ok"}` \| `{"id","err"}` (protobuf: diferido) |
| Servidor | `serve(host, port, handler)` · `serve_graceful(host, port, drain_ms, handler)` (señales + drenado, M88.1b) · `serve_shutdown[_limits](…, stop, drain_ms, …)` · handler `fn(Req) -> Result<Json, string>`; `Req { method, params, deadline_ms, traceparent }`; una fibra por conexión; panic del handler → `err` sin matar la conexión; `Limits { max_frame_bytes }` (10 MiB) |
| Cliente | `connect(host, port) -> Result<Client, _>` · `call(c, method, params)` · `call_deadline(…, ms)` (acota la espera; tras timeout: reconectar) · `call_full(…, deadline_ms, traceparent)` · `disconnect` — conexión persistente, id correlado y validado |

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
| `@test` | `fn () -> bool` o `fn () -> unit` | la corre `ray test`: bool pasa si `true`; unit pasa si no dispara `assert`/`panic`. Cada test corre aislado; puede vivir en cualquier módulo del proyecto (corre calificada: `math.t`) y usar `import`; un fallo reporta `at módulo:línea:col` |
| `@derive(Eq)` | struct/enum no genérico | genera `impl Eq` (igualdad estructural) |
| `@derive(Show)` | struct/enum no genérico | genera `impl Show` (`Nombre { c: v }` / `Nombre.Variante(v)`); soporta enums recursivos |
| `@derive(Hash)` | struct/enum no genérico | genera `impl Hash` (para claves de `Set`/`Dict`) |
| `@derive(ToJson)` | struct/enum no genérico | genera `impl ToJson` (`to_json(self) -> string`), que usan las respuestas JSON tipadas del framework web. El trait vive en `std/json`: hay que tenerlo en ámbito (`from std/json import ToJson;`) |

Se combinan: `@derive(Eq, Show, Hash, ToJson)`. Son las **cuatro** derivables; `Ord` se implementa
a mano (cualquier otro nombre es error de compilación).

## 13. FFI: tipos marshalables

`extern "lib" { fn nombre(args) -> ret; }` declara funciones C (dlopen/LoadLibrary en runtime).
Aridad **0 a 6** (la rechaza el checker más allá: mismo límite en todos los motores). Es la única frontera insegura del lenguaje: la firma declarada se **confía**.

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

**`errno`**: `import std/ffi;` + `ffi.errno()` lee el `errno` del hilo (el motivo del último fallo
POSIX). Regla: **inmediatamente** después de la extern, sin E/S de raylang en medio (una operación
que aparque la fibra deja correr a sus hermanas del mismo hilo, que pueden pisarlo; dos sentencias
consecutivas sin E/S no aparcan). Tras una extern `blocking`, el runtime repone en el worker el
errno del hilo del pool — la regla es la misma.

**Pila para el código C** (binario nativo con fibras): las llamadas C corren sobre la pila de la
fibra. Con externs declaradas, el default sube solo de 128 KiB a **1 MiB** por fibra (reserva
virtual: solo cuestan las páginas tocadas); `RAY_FIBER_STACK_KIB` lo ajusta y siempre gana. Un
desborde da SIGSEGV por página de guarda (nunca corrupción silenciosa).

**`extern "lib" blocking { … }`** marca las firmas del bloque como llamadas **bloqueantes de verdad**
(E/S, C-libs lentas). Mismos tipos y valores; solo cambia la planificación: en el binario nativo con
fibras (el default) la llamada se descarga a un pool bloqueante y la fibra aparca (el worker M:N no
se vara). Donde no hay scheduler que proteger (VM, intérprete, `--without fibers`, fuera de fibra) es
inerte. `blocking` es contextual: sigue valiendo como identificador.

## 14. El CLI `ray`

(`raylang` es un alias del mismo binario.)

| Comando | Qué hace |
|---|---|
| `ray new <nombre>` | crea un proyecto (ray.toml + src/main.ray + .gitignore) |
| `ray run [archivo] [args…]` | ejecuta (por defecto `src/main.ray`); resuelve dependencias |
| `ray dev [archivo] [args…]` | como `run`, pero reinicia ante cambios en `.ray`/`.ray.html`/`ray.toml` (SIGTERM → drenado con `serve_graceful`) |
| `ray build [archivo] [--native …]` | chequea y compila sin ejecutar (0 ok / 65 error); `--native` transpila a Rust y produce un **binario nativo** (24–61× la VM, byte-idéntico) |
| `ray test [archivo] [filtro]` | corre las `@test` del proyecto: la entrada y todos sus módulos (calificadas: `math.t`) + cada `tests/*.ray` como suite de integración; filtro por subcadena; sale con 0/1 (65 si algo no compila) |
| `ray fmt <archivo>... [--write]` | imprime la versión canónica (indentación de 4; lo que pase de 100 columnas se reparte: un `from … import` a un nombre por línea, una cadena de métodos a un eslabón por línea, y las listas delimitadas —argumentos, parámetros de `fn`, literales— a un elemento por línea con el cierre en línea propia). `--write`/`-w` reescribe en el sitio y admite varios archivos |
| `ray build --templates-only [ruta…]` | **materializa** en disco el módulo generado de cada template `.ray.html` (firma `{% params %}`), para inspección (sin rutas: la raíz del proyecto). La vía normal no lo necesita: el loader compila los templates **en memoria** al resolver sus imports (M102) e ignora un `.ray` hermano |
| `ray doc <archivo>` | documentación Markdown de la superficie pública (`///`) |
| `ray repl` | REPL interactivo |
| `ray lsp` | Language Server (diagnósticos, hover, ir-a-definición, references, rename, completion, signature help) |
| `ray mcp` | servidor MCP para agentes LLM: tools `check`/`run`/`test`/`fmt`/`doc`, con el código confinado (fuel + heap + plazo). Guía: [`docs/mcp.md`](docs/mcp.md) |
| `ray add <nombre>[@req]` | añade una dependencia del registro (`1.2.0`, `^1.2`, `~1.2.3`, `*`) |
| `ray remove <nombre>` | la elimina (y su caché si nadie más la usa) |
| `ray search [patrón]` | lista paquetes del registro |
| `ray fetch` | descarga las dependencias a `.ray-deps/` |
| `ray update` | re-resuelve a las versiones más nuevas compatibles |
| `ray registry publish [--repo <spec>] [--sign]` | publica esta versión en el registro (valida + check semántico + hash; `--sign` la firma Ed25519 y reclama/verifica el dueño del nombre) |
| `ray registry keygen [--out F]` | genera la clave Ed25519 de publicación (`RAY_KEY` o `~/.ray/publish.key`) |
| `ray registry verify [dir]` | audita las firmas de un índice contra sus dueños (CI del repo del índice) |
| `ray registry yank <nom>@<ver> [--undo]` | retira/restaura una versión publicada |
| `ray version` | versión |
| `ray help` | la ayuda: todos los subcomandos con sus flags (también sin argumentos) |

Flags de `run`:

| Flag | Efecto |
|---|---|
| `--interp` | fuerza el intérprete (oráculo de desarrollo; sin concurrencia) |
| `--deterministic` | scheduler M:1 reproducible (un hilo, FIFO); también `RAYLANG_THREADS=1` |
| `--fuel N` | límite de instrucciones de la VM (para embeber confinado) |
| `--heap N` | tope de objetos vivos del heap (fuerza GC; si no basta, aborta) |

Flags de `build --native`:

| Flag | Efecto |
|---|---|
| `-o <ruta>` | nombre del binario de salida (por defecto, el *stem* del archivo) |
| `--release` | tier de optimización `opt-level=3 + lto=fat + codegen-units=1 + target-cpu=native` (más lento de compilar, no portable) |
| `--fast` | cambia la aritmética **chequeada** por **envolvente** (no detecta desbordamientos): más rendimiento a cambio de una garantía; para código propio, no para entrada hostil |
| `--target <triple>` | *cross-compile* al triple indicado (requiere el target instalado en la toolchain) |
| `--without <lista>` | excluye subsistemas: `crypto,tls,sqlite,regex` (caen en un *stub* con error claro o en la implementación en raylang) y `mimalloc,ahash,fibers,process` (que van por defecto). Se une a `[native] without` del `ray.toml` |

Los subsistemas con crate de producción (TLS/`rustls`, cripto/`ring`, SQLite/`rusqlite`, regex acelerada)
se enlazan **solo cuando el programa los usa** (proyecto Cargo generado; el binario llama al mismo código
que la VM vía el crate `ray-runtime`). **mimalloc, aHash y las fibras van por defecto** — son las que
hacen que el default sea la vía Cargo; `--without mimalloc,ahash,fibers` recupera el `rustc` pelado con
hilo-por-tarea. `[native] without = ["tls", …]` en `ray.toml` fija una política de exclusión estable del
proyecto.

Variables de entorno: `SSL_CERT_FILE` (CAs extra para TLS), `RAY_INDEX` (índice de paquetes),
`RAY_MIRROR` (mirror de descarga), `RAY_KEY` (clave Ed25519 de publicación),
`RAYLANG_THREADS` (nº de hilos worker del scheduler; `1` = determinista),
`RAY_FIBER_STACK_KIB` (reserva de pila por fibra en el binario nativo).

## 15. Códigos de salida

| Código | Significado |
|---|---|
| retorno de `main` | un `main -> int` sale con ese valor; `main -> unit` sale con 0 |
| 64 | uso incorrecto del CLI |
| 65 | error de compilación (léxico/sintaxis/tipos) o de configuración |
| 66 | archivo de entrada no encontrado |
| 69 | el binario no incluye el subsistema que el comando necesita (build *slim* sin TLS/cripto: `registry keygen`/`verify`) |
| 70 | error de ejecución (panic, overflow, índice fuera de rango, deadlock…) |
| 73 | no se pudo crear un archivo (`ray new`) |
| 101 | ICE (error interno del compilador — repórtalo) |
| 0 / 1 | `ray test` sale con 0 (todo verde) o 1 (hubo fallos); 65 si una suite no compila |
