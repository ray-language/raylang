# Manual de raylang

Una guía **práctica** para escribir programas en raylang: el lenguaje de un vistazo, sus idiomas, y
recomendaciones. Complementa a los otros documentos:

- **`SPEC.md`** — la referencia **normativa** (gramática y semántica exactas). Cuando el manual y la SPEC
  discrepen, manda la SPEC.
- **[`REFERENCE.md`](REFERENCE.md)** — el **catálogo exhaustivo**: todas las palabras clave, símbolos,
  operadores con precedencia, builtins, prelude, módulos `std/` y paquetes, con sus firmas.
- **`book/`** — el libro *Construyendo raylang*: cómo se **construyó** el lenguaje, fase a fase (pedagogía de
  implementación).
- **[`PUBLISH.md`](PUBLISH.md)** — la guía del **publicador**: empaquetar, versionar y publicar
  en el registro (`ray registry publish`, índice, yank, garantías del lock).
- **Este manual** — cómo **usar** raylang para programar.

## Índice

1. [Primeros pasos](#1-primeros-pasos)
2. [Fundamentos](#2-fundamentos)
3. [Operadores y números](#3-operadores-y-números)
4. [Control de flujo](#4-control-de-flujo)
5. [Datos compuestos](#5-datos-compuestos)
6. [Strings, chars y bytes](#6-strings-chars-y-bytes)
7. [Errores como valores](#7-errores-como-valores)
8. [Funciones de orden superior e iteradores](#8-funciones-de-orden-superior-e-iteradores)
9. [Genéricos y traits](#9-genéricos-y-traits)
10. [Pattern matching](#10-pattern-matching)
11. [Módulos y paquetes](#11-módulos-y-paquetes)
12. [La biblioteca estándar](#12-la-biblioteca-estándar)
13. [I/O y sistema](#13-io-y-sistema)
14. [Red y bases de datos](#14-red-y-bases-de-datos)
15. [Concurrencia](#15-concurrencia)
16. [FFI: llamar a C](#16-ffi-llamar-a-c)
17. [Herramientas](#17-herramientas)
18. [Recomendaciones y mejores prácticas](#18-recomendaciones-y-mejores-prácticas)
19. [Gotchas](#19-gotchas)

---

## 1. Primeros pasos

Instala el binario (`ray`, con su alias `raylang`):

```sh
curl -sSfL https://raw.githubusercontent.com/roberto-ayala/raylang/main/install.sh | sh
```

Crea y ejecuta un proyecto:

```sh
ray new hola
cd hola
ray run          # ejecuta src/main.ray
```

Un programa es un archivo `.ray` con una función `main`. `main` devuelve `int` (su código de salida) o
`unit` (0):

```rust
fn main() -> int {
    print("hola, raylang");
    0
}
```

`print` es un **builtin** (imprime cualquier valor imprimible + un salto de línea). Un archivo suelto se
ejecuta con `ray run archivo.ray`; los argumentos tras el archivo llegan al programa vía `args()`:

```sh
ray run saluda.ray Ada Grace     # args() == ["Ada", "Grace"]
```

Estructura de un proyecto:

```
hola/
├── ray.toml        # manifiesto: nombre, versión, dependencias
├── ray.lock        # lockfile (lo genera ray; se commitea)
├── src/main.ray    # entrada
└── .ray-deps/      # caché de dependencias (gitignoreada)
```

## 2. Fundamentos

### Valores y tipos primitivos

| Tipo | Ejemplo | Notas |
|------|---------|-------|
| `int` | `42`, `-7` | entero de 64 bits con signo; el desbordamiento es **error de ejecución** (no envuelve en silencio) |
| `u8`, `u32`, `u64` | `let x: u8 = 255;` | sin signo; la aritmética **envuelve** por diseño (para hashes, protocolos, bits) |
| `float` | `3.14`, `2.0` | coma flotante de 64 bits |
| `bool` | `true`, `false` | |
| `string` | `"hola"` | inmutable; con interpolación `${…}` (§6) |
| `char` | `'a'`, `'\n'` | un code point Unicode |
| `bytes` | `b"\x00\xff"` | octetos crudos, inmutable |
| `unit` | | "sin valor útil"; lo que devuelve p. ej. `print` |

raylang **no tiene `null`**: la ausencia se modela con `Option` (§7).

### Variables: `let` y `var`

`let` declara un binding **inmutable**; `var`, uno **mutable**. Prefiere `let`.

```rust
fn main() -> int {
    let x = 10;          // inmutable; tipo inferido (int)
    var total = 0;       // mutable
    total = total + x;
    let y: float = 2.5;  // con anotación explícita
    print(total);
    0
}
```

Los **parámetros son inmutables**. Las **firmas de función se anotan siempre**; los **locales** se infieren
(salvo casos indeterminados como `[]` vacío, `Option.None` o `Map.new()`, que piden anotación o contexto).

La asignación es una **sentencia**, no una expresión (`x = y = 5` no existe). Para descartar un valor:
`let _ = f();`.

Constantes globales con `const` (su valor debe ser un **literal**):

```rust
const GRAVEDAD: float = 9.81;     // para π/e usa `math.PI`/`math.E` (import std/math), no una const
const MAX_INTENTOS: int = 3;
```

### Funciones

Orientado a **expresiones**: el último valor de un bloque es su resultado (retorno implícito); `return` es
opcional (útil para salir antes).

```rust
fn cuadrado(x: int) -> int { x * x }          // retorno implícito

fn signo(x: int) -> int {
    if (x > 0) { return 1; }                  // retorno explícito temprano
    if (x < 0) { return 0 - 1; }
    0
}
```

Las funciones son **valores de primera clase** (se pasan, se devuelven, se guardan):

```rust
fn aplica(f: fn(int) -> int, x: int) -> int { f(x) }

fn main() -> int {
    print(aplica(cuadrado, 7));    // 49
    0
}
```

### Comentarios

```rust
// comentario de línea
/// comentario de documentación (lo leen `ray doc` y el hover del editor)
```

## 3. Operadores y números

La tabla completa de precedencia (15 niveles) está en [`REFERENCE.md` §2](REFERENCE.md#2-símbolos-y-operadores).
Lo esencial:

```rust
// Aritmética: + - * / % (división/módulo por cero = error de ejecución)
// Comparación: == != < <= > >=   (strings: lexicográfica; chars: por code point)
// Lógicos: && || !               (cortocircuitan)
// Bit a bit: & | ^ ~ << >>       (sobre int/u8/u32/u64; semántica wrapping)
let mezcla = (hash << 5) ^ (hash >> 2);
```

> **Precedencia estilo C**: los bit a bit ligan más flojo que `==`. Escribe `(flags & 32) != 0` — sin los
> paréntesis, se parsea como `flags & (32 != 0)` y es error de tipos.

### int vs u8/u32/u64

`int` es el entero de trabajo; su desbordamiento **aborta** (mejor un error que un número corrupto). Los
sin signo (`u8`/`u32`/`u64`) **envuelven** — son para el código que quiere esa semántica: hashes, checksums,
protocolos binarios. Los literales se adaptan al contexto sin cast:

```rust
let b: u8 = 200;               // literal coercionado
let h: u32 = 2166136261;       // FNV offset basis
var acc: u32 = h;
acc = (acc ^ b as u32) * 16777619;   // aritmética u32, envuelve
```

### Casts con `as`

Nunca hay conversión implícita entre tipos numéricos; siempre `as`:

```rust
let f = 3.99;
print(f as int);        // 3   (trunca hacia cero; satura en el borde)
print(7 as float);      // 7
print('A' as int);      // 65  (code point)
print(66 as char);      // 'B' (valida el code point)
let w = 300 as u8;      // 44  (enmascara al ancho)
print(w);               // los u8/u32/u64 se imprimen directo (decimal sin signo)
```

### Conversiones con string

`as` no convierte hacia ni desde `string` — a propósito: sus casts son totales (truncar, enmascarar,
validar), mientras que parsear un string **puede fallar** y formatear un valor es otra operación.
Cada dirección tiene su herramienta:

**Hacia string** (formateo, nunca falla): `to_string(v)` o interpolación `"${v}"` — que es azúcar
del mismo `to_string`, con la misma representación que `print`:

```rust
let s = to_string(42);          // "42"
let msg = "total: ${3.5}";      // "total: 3.5"
```

Para tus propios tipos, implementa (o deriva) el trait `Show` y quedan admitidos por
`print`/`to_string`/`${}` (§9).

**Desde string** (parseo, puede fallar): `parse_int` / `parse_float` devuelven `Option` — el fallo
es un valor, no una excepción ni un cero silencioso:

```rust
let n = "42".parse_int();               // Some(42) : Option<int>
let m = "abc".parse_int();              // None
let k = entrada.parse_int().unwrap_or(0);
let f = "2.5".parse_float();            // Some(2.5) : Option<float>
```

Para el caso interactivo, `read_int()` ya combina `input` + `parse_int` (→ `Option<int>`).

### Sobrecarga de operadores

`+ - * /` y el `-` unario se sobrecargan implementando los traits `Add`/`Sub`/`Mul`/`Div`/`Neg`:

```rust
struct Vec2 { x: float, y: float }

impl Add for Vec2 {
    fn add(self, other: Vec2) -> Vec2 { Vec2 { x: self.x + other.x, y: self.y + other.y } }
}

fn main() -> int {
    let v = Vec2 { x: 1.0, y: 2.0 } + Vec2 { x: 3.0, y: 4.0 };
    print(v.x);      // 4
    0
}
```

## 4. Control de flujo

Todo `if`, bloque y `match` **produce un valor**.

```rust
let max = if (a > b) { a } else { b };        // if como expresión
```

`while` (no hay `break`/`continue`; patrones abajo):

```rust
var i = 0;
while (i < 5) { print(i); i = i + 1; }
```

`for` sobre cualquier iterador (arreglos, rangos, `Map`, strings vía `.chars()`, los tuyos — §8):

```rust
for x in [10, 20, 30] { print(x); }
for i in 0..5 { print(i); }                   // rango semiabierto: 0,1,2,3,4
for (k, v) in edades { print("${k}: ${v}"); } // Map, en orden de clave
```

### Salir temprano sin `break`

raylang no tiene `break` ni `continue` (decisión de diseño: `return` ya es la salida temprana).
Los patrones idiomáticos, del más al menos frecuente:

**1. Extrae el bucle a una función: `return` ES tu `break`.** Es el reemplazo canónico y suele
dejar una función con nombre propio que el código llamador lee mejor:

```rust
fn primer_negativo(xs: [int]) -> Option<int> {
    for x in xs {
        if (x < 0) { return Option.Some(x); }   // "break" con el resultado en la mano
    }
    Option.None
}
```

**2. Muchas búsquedas ya están resueltas: ni siquiera escribas el bucle.**

```rust
xs.position(99);          // Option<int>: índice de la primera ocurrencia
s.index_of("clave");      // Option<int>: lo mismo en strings
xs.any(es_par);           // bool: ¿alguno cumple?
xs.all(es_par);           // bool: ¿todos cumplen?
```

**3. `continue` ≈ invertir la condición.** En vez de saltarte el elemento, procesa solo los que
te interesan:

```rust
for x in xs {
    if (x >= 0) {         // en vez de `if (x < 0) { continue; }`
        procesar(x);
    }
}
```

**4. Corta el iterador o compón la condición del `while`.**

```rust
for x in xs.iter().take(3) { print(x); }        // solo los 3 primeros

var seguir = true;                              // el "break" de un while de servidor/REPL
while (seguir) {
    // … en el punto de salida: seguir = false;
}
```

`match` **destructura enums** (`Option`/`Result`/los tuyos), como expresión (§10 a fondo). El escrutinio
va **entre paréntesis**. Para despachar sobre **primitivos** (int/bool/string) se usa `if/else`, no `match`:

```rust
let etiqueta = match (buscar(k)) {      // match: solo sobre enums
    Option.Some(v) => "encontrado",
    Option.None => "no está",
};

let clase = if (n == 0) { "cero" } else if (n < 0) { "negativo" } else { "positivo" };  // int → if/else
```

## 5. Datos compuestos

### Arreglos `[T]`

Homogéneos, dinámicos, con semántica de **referencia** (se comparten al pasarlos/asignarlos).

```rust
var xs: [int] = [1, 2, 3];
xs.push(4);                    // muta en el sitio
xs[0] = 99;
print(xs.len());               // 4
print(xs.contains(4));         // true
print(xs.position(99));        // Some(0)
print(xs.pop());               // Some(4) — y xs queda [99, 2, 3]
let ys = xs + [7, 8];          // concatenación (copia nueva)
let z = xs.sort();             // copia ordenada (T: Ord)
```

### Tuplas

Agregados **inmutables** de tipos mixtos; se copian como valor.

```rust
let par = (1, "uno");
print(par.0);                  // 1
let (a, b) = (10, 20);         // destructuring

fn min_max(xs: [int]) -> (int, int) {          // retorno múltiple
    // …
    return (lo, hi);
}
let (lo, hi) = min_max(datos);
```

### Structs

Campos nombrados; semántica de **referencia** (un alias muta el mismo objeto).

```rust
struct Punto { x: int, y: int }

fn main() -> int {
    var p = Punto { x: 1, y: 2 };
    let alias = p;
    alias.x = 10;              // p.x también es 10 (mismo objeto)
    print(p.x);
    0
}
```

### Enums (tipos suma)

Variantes con o sin payload; se consumen con `match` (exhaustivo):

```rust
import std/math;

enum Figura { Circulo(float), Rect(float, float), Nada }

fn area(f: Figura) -> float {
    match (f) {
        Figura.Circulo(r) => math.PI * r * r,
        Figura.Rect(w, h) => w * h,
        Figura.Nada => 0.0,
    }
}
```

Los enums pueden ser **recursivos** (a través de payloads con arreglos u otros enums) y **genéricos**
(`enum Arbol<T> { Hoja, Nodo(T, [Arbol<T>]) }`).

### `Map<K, V>`

Tabla clave→valor. Claves *hashables*: int/u\*/string/char/bool/bytes (**no** float). Los recorridos van
**en orden de clave** — deterministas.

```rust
var edades: Map<string, int> = Map.new();
edades.insert("ana", 30);
edades.insert("luis", 25);
match (edades.get("ana")) {            // get -> Option<V>
    Option.Some(e) => print(e),
    Option.None => print("no está"),
}
print(edades.keys());                  // ["ana", "luis"] — ordenadas
for (nombre, edad) in edades { print("${nombre}: ${edad}"); }
edades.remove("luis");                 // -> Option<V>
```

### `Set<T>`, `Dict<K,V>`, `Deque<T>`, `StringBuilder`

En `std/collections` (se usan calificados por el leaf):

```rust
import std/collections/set;
import std/collections/dict;
import std/collections/deque;
import std/collections/stringbuilder;

fn main() -> int {
    var vistos: set.Set<int> = set.new();      // el tipo, calificado; T necesita Hash + Eq
    set.add(vistos, 7);
    print(set.has(vistos, 7));                 // true

    // Dict<K,V> (M82): claves de USUARIO vía Hash + Eq (p. ej. @derive(Hash, Eq)).
    // Para claves primitivas prefiere el Map<K,V> builtin (más rápido, keys() ordenadas).
    var indice: dict.Dict<string, int> = dict.new();
    dict.insert(indice, "ada", 1815);
    print(dict.get(indice, "ada"));            // Some(1815)

    var cola: deque.Deque<int> = deque.new();
    deque.push_back(cola, 1);
    deque.push_front(cola, 0);
    print(deque.pop_front(cola));              // Some(0)

    var sb = stringbuilder.new();              // evita el O(n²) de `+` en bucle
    for i in 0..3 { stringbuilder.push(sb, to_string(i)); }
    print(stringbuilder.build(sb));            // "012"
    0
}
```

## 6. Strings, chars y bytes

### Strings

Inmutables, indexables **por carácter** (`s[i] -> char`), con interpolación `${expr}`:

```rust
let nombre = "raylang";
let n = 3;
print("hola ${nombre}, ${n * 2} veces");  // una expresión por hueco; "\${" = literal

print("Hola".to_upper());                // "HOLA"
print("a,b,c".split(","));               // ["a", "b", "c"]
print("  x  ".trim());                   // "x"
print("café".len());                     // 4 (por carácter, no por byte)
print("café"[3]);                        // 'é'
print("abc" + "def");                    // concatenación
print("banana".replace("na", "NA"));     // "baNANA"
print("hola.ray".ends_with(".ray"));     // true
print("hola".substring(1, 3));           // "ol"  (con clamp: nunca falla)
print("ab".repeat(3));                   // "ababab"
print("banana".index_of("na"));          // Some(2)
print("uno dos".chars().len());          // 7
```

Un string **no se muta** (`s[i] = c` es error): construye uno nuevo, o usa `StringBuilder` para acumular.

### Chars

```rust
let c = 'ñ';
print(char_code(c));                     // 241
print(char_from_code(65));               // Some('A')  — None si el code point es inválido
print('a' < 'b');                        // true (por code point)
```

No hay aritmética de chars (`'a' + 1` es error): pasa por `char_code`/`char_from_code`.

### Bytes

Datos binarios crudos, inmutables. El puente con strings es explícito (UTF-8):

```rust
let b = "señal".to_bytes();              // string -> bytes (UTF-8)
print(b.len());                          // 6 (¡octetos, no caracteres!)
print(b[0]);                             // 115 — indexar da el octeto como int
let s = from_utf8(b);                    // bytes -> Result<string, string>
let corte = b.sub_bytes(0, 3);           // rebanada [0, 3)
let crudo = bytes_of([72, 111, 108, 97]); // [int] -> bytes
print(crudo);                            // se imprime en hex: 486f6c61
let dos = b"AB" + b"\x00\xff";           // literal b"…" y concatenación
```

## 7. Errores como valores

No hay excepciones: los fallos son **valores** de `Option<T>` (ausencia) o `Result<T, E>` (éxito/error).

```rust
fn dividir(a: int, b: int) -> Result<int, string> {
    if (b == 0) { Result.Err("división por cero") } else { Result.Ok(a / b) }
}

fn buscar(m: Map<string, int>, k: string) -> Option<int> { m.get(k) }
```

El operador **`?`** desempaqueta el `Ok`/`Some`, o **retorna** el `Err`/`None` de inmediato:

```rust
fn calc() -> Result<int, string> {
    let x = dividir(10, 2)?;       // si Err, calc retorna ese Err
    let y = dividir(x, 0)?;        // aquí retorna Err("división por cero")
    Result.Ok(x + y)
}
```

`?` exige que el tipo de error **case** con el retorno de la función envolvente — o que exista una
conversión vía el trait `From`:

```rust
struct ErrorApp { detalle: string }

impl From<string> for ErrorApp {
    fn convert(source: string) -> ErrorApp { ErrorApp { detalle: source } }  // `from` es keyword → `convert`
}

fn correr() -> Result<int, ErrorApp> {
    let x = dividir(10, 0)?;       // el Err(string) se convierte a ErrorApp automáticamente
    Result.Ok(x)
}
```

Para abortar de verdad (bug, invariante rota — no para entrada del usuario):

```rust
panic("estado imposible");         // aborta con mensaje y posición
assert(x > 0);                     // aborta si es falso
assert_eq(resultado, esperado);    // aborta mostrando ambos (T: Eq + Show)
```

Un error de ejecución imprime, además de la cabecera con su posición, la **traza de
llamadas** (`en <fn> … desde <fn> …`). Si el error nace en el prelude o en la `std`
(p. ej. un `assert` fallido), la cabecera y el `^` apuntan a **tu** llamada — el sitio
real queda igualmente en la traza:

```text
error en ejecución en 2:5: aserción falló
  2 |     assert(x > 0);
    |     ^
  en assert (prelude:836:9)
  desde helper (mi_prog:2:5)
  desde main (mi_prog:8:13)
```

## 8. Funciones de orden superior e iteradores

**Closures** (funciones anónimas con captura por referencia del ámbito):

```rust
let doblar = fn(x: int) -> int { x * 2 };
print(doblar(21));                     // 42

fn contador() -> fn() -> int {
    var n = 0;
    fn() -> int { n = n + 1; n }       // captura `n` viva entre llamadas
}
```

**UFCS** (`recv.f(args)` ≡ `f(recv, args)`) y **pipelines** (`x |> f(a)` ≡ `f(x, a)`) para encadenar legible:

```rust
let r = [1, 2, 3, 4]
    |> map(fn(x: int) -> int { x * x })
    |> filter(fn(x: int) -> bool { x > 4 });
```

### Eager vs perezoso

Sobre arreglos, `map`/`filter`/`fold` son **eager** (materializan un arreglo por paso). La cadena
**perezosa** (`.iter()`, `range`) fusiona todo en una pasada y no materializa hasta un terminal:

```rust
let suma = [1, 2, 3, 4, 5]
    .iter()
    .map(fn(x: int) -> int { x * x })
    .filter(fn(x: int) -> bool { x % 2 == 1 })
    .sum();                            // 1 + 9 + 25 = 35, sin arreglos intermedios

for (i, x) in ["a", "b"].iter().enumerate() { print("${i}: ${x}"); }
let pares = xs.iter().zip(ys.iter());  // (T, U); se agota con el más corto
```

Adaptadores: `map` `filter` `take` `skip` `zip` `enumerate`. Terminales: `fold` `collect` `sum`, y el
propio `for`.

### Tu propio iterador

Cualquier tipo con `impl Iterator<T>` (solo `next`) es recorrible con `for` y hereda todos los adaptadores:

```rust
struct Cuenta { hasta: int, actual: int }

impl Iterator<int> for Cuenta {
    fn next(self) -> Option<int> {
        if (self.actual >= self.hasta) { return Option.None; }
        self.actual = self.actual + 1;
        Option.Some(self.actual - 1)
    }
}

fn cuenta(hasta: int) -> Cuenta { Cuenta { hasta: hasta, actual: 0 } }

fn main() -> int {
    for i in cuenta(3) { print(i); }           // 0, 1, 2
    // (la cabecera del for no admite un literal de struct — usa un constructor)
    0
}
```

## 9. Genéricos y traits

**Funciones genéricas** (inferencia desde los argumentos):

```rust
fn id<T>(x: T) -> T { x }
fn primero<T>(xs: [T]) -> T { xs[0] }
fn pares<A, B>(a: A, b: B) -> (A, B) { (a, b) }
```

**Tipos genéricos**, con **bounds** opcionales que se comprueban al construir:

```rust
struct Caja<T> { valor: T }
enum Arbol<T: Show> { Hoja, Nodo(T) }
```

**Traits** (comportamiento sobre tipos), con `impl` y despacho estático:

```rust
import std/math;

trait Area { fn area(self) -> float; }        // firma: termina en ';'

struct Circulo { r: float }
impl Area for Circulo {
    fn area(self) -> float { math.PI * self.r * self.r }
}

fn imprime_area<T: Area>(x: T) {              // bound: T debe implementar Area
    print(x.area());
}
```

Más allá de lo básico:

- **Métodos por defecto**: una firma del trait puede traer cuerpo; el impl lo hereda si no lo redefine.
- **Bounds múltiples**: `fn f<T: Eq + Show>(x: T)`.
- **Impls genéricos**: `impl<T: Show> Area for Caja<T> { … }`.
- **Trait objects** (`dyn`): despacho dinámico para colecciones heterogéneas —
  `let figuras: [dyn Area] = […];` — incluso multi-trait (`dyn Area + Show`) y *upcasting* a un
  subconjunto.
- **Traits genéricos**: `trait From<S>` (§7), `trait Iterator<T>` (§8).

Traits derivables con `@derive` (struct/enum no genéricos):

```rust
from std/json import ToJson;      // ToJson vive en std/json: derivarlo exige tenerlo en ámbito

@derive(Eq, Show, Hash, ToJson)
struct Par { a: int, b: int }

// Eq     → habilita == y assert_eq
// Show   → habilita print/to_string ("Par { a: 1, b: 2 }")
// Hash   → habilita usarlo como elemento de Set (o clave de Dict)
// ToJson → genera to_json(self) -> string (respuestas JSON tipadas del framework web)
```

Son las **cuatro** derivables. `Ord` no lo es: se implementa a mano (`fn less(self, other: Self)
-> bool`), porque el criterio de orden rara vez es "campo a campo en el orden declarado".

## 10. Pattern matching

`match` es exhaustivo (el checker exige cubrir todos los casos). El escrutinio va entre paréntesis; los
brazos con cuerpo de **bloque** llevan **coma** detrás.

```rust
match (opt) {
    Option.Some(v) => print(v),
    Option.None => print(0),
}
```

Patrones disponibles:

```rust
match (r) {
    Result.Ok(Option.Some(v)) => v,          // anidado
    Result.Ok(Option.None) => 0,
    Result.Err(_) => 0 - 1,                  // comodín en el payload
}

match (evento) {
    Evento.Click(Punto { x, y }) => x + y,   // patrón de struct (forma corta {x, y} ≡ {x: x, y: y})
    Evento.Tecla(c) if c == 'q' => 0 - 1,    // guarda: patrón if condición
    Evento.Tecla(_) => 0,
    otro => procesar(otro),                  // binding suelto (captura el valor entero)
}
```

Y azúcar `if let` para un solo caso:

```rust
if let Option.Some(v) = buscar(k) { print(v); }
```

Aprovecha la exhaustividad: si añades una variante al enum, el compilador señala cada `match` que falta
actualizar — **evita el `_` catch-all** cuando quieras esa red de seguridad.

## 11. Módulos y paquetes

Un módulo es un archivo; se importa por su ruta desde la raíz. `pub` expone lo que otros módulos pueden ver.

```rust
// geo/punto.ray  — el struct entero es `pub` (el `pub` por campo aún no existe)
pub struct Punto { x: int, y: int }
pub fn origen() -> Punto { Punto { x: 0, y: 0 } }

// main.ray
import geo/punto;                 // referencia calificada por el LEAF: punto.f()
from geo/punto import origen;     // ...o trae nombres al ámbito
import geo/punto as pt;           // ...o con alias (si el leaf colisiona)

fn main() -> int {
    let p = origen();             // por el from-import
    let q = punto.origen();       // calificado — se usa el LEAF (`punto`), no la ruta
    print(p.x + q.x);
    0
}
```

Los **tipos** también cruzan módulos: `from geo/punto import Punto;`, o calificado `punto.Punto { x: 1, y: 2 }`
y `punto.Color.Rojo` (en anotaciones, literales y patrones).

Una lista larga de nombres se puede repartir en **varias líneas** —el léxico ignora los saltos— y admite
**coma final**. `ray fmt` lo decide por ancho: deja el import en una línea si cabe en 100 columnas y lo
envuelve a un nombre por línea si no.

```ray
from web/framework import
    new_app,
    GET,
    POST,
    listen_graceful,
    static_files_cached;

from std/json import obj, field, list;   // cabe en 100 columnas → una línea
```

El mismo umbral reparte una **cadena de métodos** (el patrón *builder*) de dos o más eslabones: el
receptor se queda en su sitio y cada `.metodo(…)` baja una línea.

```ray
fn to_json(self) -> string {
    render(obj()
        .field("id", self.id)
        .field("slug", self.slug)
        .field("name", self.name))
}

print(s.trim().to_lower());   // cabe → una línea
```

Y reparte las **listas delimitadas** que no quepan —argumentos, parámetros de `fn`, literales de
arreglo, tupla, struct y Map—, con el delimitador de cierre **en su propia línea** (como ya hacían
`struct`, `enum`, `match` y los bloques) y **sin coma final**:

```ray
Result.Ok(
    Reply.Html(
        home.render(
            frame,
            featured[0],
            respond.or_empty(catalog.on_sale(conn, 4)),
            schema.product_count(conn)?
        )
    )
)
```

La regla que separa los dos cierres: **si la forma tiene delimitador propio, cierra en línea propia**;
si no lo tiene —el `;` de un import es un terminador, y el `)` final de una cadena pertenece a la
llamada que la envuelve— el terminador va pegado al último elemento.

### Cápsulas (`mod.ray`)

Un directorio con un `mod.ray` es una **cápsula**: `import geo;` carga `geo/mod.ray`, que define la **cara
pública** del directorio reexportando ítems de sus submódulos. Los submódulos internos quedan **protegidos**
—importarlos desde fuera es un error—, así que la cápsula es una frontera de encapsulación, no solo ergonomía.

```rust
// geo/mod.ray  — su presencia convierte `geo/` en una CÁPSULA. Arma la cara pública
//                reexportando lo que otros pueden usar (el resto queda interno).
pub from geo/formas/circulo import Circulo, area;

// geo/formas/circulo.ray  — submódulo INTERNO de la cápsula
import geo/util;                             // un vecino interno: permitido (vive bajo `geo/`)
pub struct Circulo { radio: int }
pub fn area(c: Circulo) -> int { 3 * util.cuadrado(c.radio) }

// geo/util.ray  — otro submódulo interno; NO se reexporta → privado a la cápsula
pub fn cuadrado(n: int) -> int { n * n }

// main.ray  — desde FUERA solo se ve la cara pública de `geo`
import geo;                                  // carga geo/mod.ray
fn main() -> int {
    let c = geo.Circulo { radio: 4 };        // Circulo, reexportado por la cápsula
    geo.area(c)                              // 3 * 16 = 48  → código de salida
    // import geo/util;  → ERROR: 'geo/util' es interno a la cápsula 'geo'; impórtalo con 'import geo;'
}
```

Un `pub from … import …` (con `pub`) **reexporta**: trae nombres de un submódulo interno y los añade a la
cara pública de la cápsula. Sin `pub`, el `from`-import es privado al `mod.ray`. El ejemplo completo está en
[`examples/capsula/`](examples/capsula/).

### Paquetes y dependencias (`ray.toml`)

```toml
[package]
name = "hola"
version = "0.1.0"

[dependencies]
textutils = "^1.2"                              # del registro central (rangos semver: 1.2.0, ^1.2, ~1.2.3, *)
geo = "git+https://github.com/user/geo@v1.0.0"  # git directo (ref obligatoria → reproducible)
util = "path:../util"                           # local (no se bloquea; para desarrollo)
```

`ray build`/`run`/`test` resuelven y cachean las dependencias en `.ray-deps/` (con dependencias
**transitivas** y resolución "gana la mayor compatible"). El lockfile `ray.lock` fija cada dependencia con
su **hash SHA-256** — un contenido alterado se detecta como error de supply-chain. Gestión desde el CLI:

```sh
ray add textutils@^1.2      # añade al manifiesto y descarga
ray search json             # busca en el registro
ray update                  # re-resuelve a las más nuevas compatibles
ray remove textutils
ray registry publish                 # publica TU paquete en el registro (valida + chequea + hashea)
```

El flujo completo del **publicador** (empaquetar, el índice, versionado, `yank`, garantías y
receta de punta a punta) está en [`PUBLISH.md`](PUBLISH.md).

## 12. La biblioteca estándar

Tres capas (catálogo completo en [`REFERENCE.md`](REFERENCE.md#10-la-biblioteca-estándar-std)):

1. **Builtins + prelude** — disponibles sin `import`: `print`, `to_string`, los métodos de
   string/arreglo/Map, `Option`/`Result`, `parse_int`, `sort`, `assert`, la cripto (`sha256`,
   `hmac_sha256`, `ed25519_*`, `chacha20poly1305_*`)…
2. **`std/`** — embebida en el binario, se importa por ruta y se usa calificada:

   ```rust
   import std/math;          // math.gcd(12, 18), math.PI, math.clamp(x, 0, 10)
   import std/text;          // text.capitalize("hola"), text.pad_left("7", 3, "0")
   import std/sort;          // sort.binary_search(xs, 42), sort.dedup(xs)
   import std/fs;            // fs.read_file("x.txt") -> Result<string, string>
   import std/json;          // json.parse(s) -> Result<Json, string>, json.stringify(j)
   import std/regex;         // regex.find_all("\\d+", texto)
   import std/csv;           // csv.parse_csv(src) -> Result<[[string]], string>
   import std/random;        // random.below(6)
   ```

   El catálogo: `math` `text` `sort` `fs` `net` `process` `time` `units` `random` `crypto` `resilience`
   `collections/{set,deque,stringbuilder}` `json` `hex` `base64` `url` `regex` `csv` `toml` `template`
   `inflate` `deflate` `huffman` `protobuf` `uuid`.

3. **Paquetes** (`packages/net`, `packages/db`) — no embebidos; se declaran como dependencia (§14).

## 13. I/O y sistema

### Consola sin salto de línea (`std/io`)

`print`/`eprint` siempre añaden `\n`. Para prompts, barras de progreso o secuencias de escape de
terminal está `std/io`:

```rust
import std/io;

fn main() -> int {
    let _ = io.write("¿Continuar? [s/n] ");   // sin salto…
    let _ = io.flush();                       // …y flush: stdout va con buffer
    match (input()) {
        Option.Some(r) => print("dijiste: " + r),
        Option.None => print("EOF"),
    }
    let _ = io.write_bytes(b"\x1b[1mnegrita\x1b[0m\n"); // bytes crudos (escapes ANSI)
    let _ = io.ewrite("aviso sin salto en stderr");        // stderr: sin buffer, sin flush
    0
}
```

Las tres escrituras devuelven `Result<int, string>` (nº de caracteres/bytes). Regla práctica: tras
`write`/`write_bytes` sin `\n`, llama `io.flush()` si necesitas ver la salida ya; `ewrite` no lo
necesita. El orden entre `print` e `io.write` es siempre el del programa, también en el binario
nativo.

### Leer stdin por bytes (`io.read`)

`input()` lee líneas; `io.read` lee **octetos** — la pieza para teclas, protocolos por stdin o
consumir un pipe a trozos:

```rust
import std/io;

fn main() -> int {
    // Bloquea LA FIBRA (las demás siguen corriendo), no la VM:
    match (io.read(64)) {                       // hasta 64 octetos; None = EOF
        Option.Some(b) => print("llegaron " + to_string(b.len())),
        Option.None => print("fin de la entrada"),
    }
    // Con plazo (0 = sondeo puro): tres desenlaces distintos.
    match (io.read_timeout(64, 50)) {
        io.ReadResult.Data(b) => print(b.len()),
        io.ReadResult.Eof => print("eof"),
        io.ReadResult.TimedOut => print("nada en 50 ms"),
    }
    0
}
```

Dos reglas: **un solo lector** de stdin a la vez, y no mezclar `io.read` con `input()`/
`fs.read_line` sobre stdin en el mismo programa (aquéllos leen con buffer de líneas; esto lee el fd
crudo — cada uno dejaría de ver lo que retiene el otro).

### El terminal (`std/term`)

La pieza para TUIs: modo crudo, tamaño y teclas decodificadas. `examples/term/keys.ray` es la
demo completa; el esqueleto:

```rust
import std/io;
import std/term;

fn main() -> int {
    if (!term.is_tty(0)) { print("necesito una terminal"); return 1; }
    match (term.size()) {
        Option.Some(wh) => print("terminal de ${wh.0}x${wh.1}"),
        Option.None => { },
    }
    let r = term.raw(fn() -> int {          // modo crudo; restaura SIEMPRE (también si f falla)
        var go = true;
        while (go) {
            match (term.read_key()) {        // bloquea la FIBRA, no la VM
                Option.Some(k) => {
                    match (k) {
                        term.Key.Char(c) => { if (c == 'q') { go = false; } },
                        term.Key.Up => { let _ = io.write("↑\r\n"); let _ = io.flush(); },
                        _ => { },
                    }
                },
                Option.None => { go = false; },
            }
        }
        0
    });
    0
}
```

Tres cosas que saber del modo crudo: no hay eco ni Ctrl-C (decide tu programa: `read_key` los
entrega como `Key.Char`/`Key.Ctrl`); no hay OPOST, así que las líneas terminan con `\r\n`
explícito; y la restauración está garantizada al salir del proceso — salvo señal fatal o
`kill -9`, como en cualquier programa de terminal (`reset` lo arregla). Para redimensionamiento,
`select` sobre `signals()` + `term.size()` (SIGWINCH llega en M107.4).

### Archivos (`std/fs` — todo con errores como valores)

```rust
import std/fs;

fn main() -> int {
    let _ = fs.write_file("saludo.txt", "hola\n");        // Result<int, string>
    match (fs.read_file("saludo.txt")) {
        Result.Ok(contenido) => print(contenido.trim()),
        Result.Err(e) => { print("error: ${e}"); return 1; },
    }
    let _ = fs.append_file("saludo.txt", "otra línea\n");
    print(fs.exists("saludo.txt"));                       // true
    match (fs.list_dir(".")) {                            // nombres ordenados (determinista)
        Result.Ok(nombres) => { for n in nombres { print(n); } },
        Result.Err(e) => print(e),
    }
    let _ = fs.remove_file("saludo.txt");

    // Streaming con handles (lectura bufferizada línea a línea):
    match (fs.open("grande.log", "r")) {                  // modos "r" / "w" / "a"
        Result.Ok(h) => {
            var seguir = true;
            while (seguir) {
                match (fs.read_line(h)) {
                    Option.Some(linea) => print(linea),
                    Option.None => { seguir = false; },   // EOF
                }
            }
            close(h);
        },
        Result.Err(e) => print(e),
    }
    0
}
```

Variantes binarias: `fs.read_file_bytes`/`fs.write_file_bytes` (→ `bytes`).

### Stdin, entorno y argumentos

```rust
let linea = input();               // Option<string> (None en EOF)
let n = read_int();                // Option<int>
let home = env("HOME");            // Option<string>
let argv = args();                 // [string]
```

### Procesos del SO (`std/process`)

Ejecuta comandos externos **sin shell**: el `argv` es tipado, así que la inyección clásica no es
posible por construcción (una tubería se escribe `run("sh", ["-c", …])`, visible en el código).
La regla central: **`Err` significa "no se pudo lanzar"** (binario inexistente, permisos). Un hijo
que corrió y salió con código ≠ 0 —o murió por señal— es `Ok`: el estado va DENTRO del `Output`.

```rust
import std/process;

fn main() -> int {
    // El caso del 90 %:
    match (process.run("git", ["rev-parse", "HEAD"])) {
        Result.Ok(o) => {
            match (o.exit) {
                process.Exit.Code(c) => {
                    if (c == 0) {
                        match (from_utf8(o.stdout)) {           // stdout/stderr son bytes
                            Result.Ok(s) => print(s.trim()),
                            Result.Err(e) => print("salida no UTF-8"),
                        }
                    } else {
                        print("git salió con ${c}");
                    }
                },
                process.Exit.Signal(s) => print("git murió por la señal ${s}"),  // nunca 128+sig
            }
        },
        Result.Err(e) => print("no se pudo lanzar: ${e}"),
    }

    // El builder, para todo lo demás:
    let r = process.cmd("wc", ["-l"])
        .dir("/tmp")                       // directorio de trabajo
        .env("LC_ALL", "C")                // añade/pisa sobre el entorno heredado (.env_clear() lo vacía)
        .stdin("a\nb\n".to_bytes())        // se escribe entero y se CIERRA (el hijo ve EOF)
        .timeout_ms(5000)                  // presupuesto total; al vencer, mata al GRUPO del hijo
        .max_output(1048576)               // tope de captura por flujo (default ~16 MB)
        .run();
    match (r) {
        Result.Ok(o) => {
            if (o.timed_out) { print("se pasó del plazo (salida parcial abajo)"); }
            if (o.truncated) { print("salida truncada al tope"); }
        },
        Result.Err(e) => print(e),
    }
    0
}
```

Detalles que evitan los errores clásicos de otras plataformas:

- **stdin es `/dev/null`** salvo que llames a `.stdin(…)` — el hijo jamás hereda el stdin de tu
  proceso (un `cat` accidental no se queda colgado esperando tu terminal).
- **Los dos flujos se drenan a la vez**: el deadlock de "esperar antes de leer con el pipe lleno"
  no puede ocurrir, produzca lo que produzca el hijo.
- **El timeout no es una excepción**: devuelve el `Output` PARCIAL con `timed_out: true` (puedes
  diagnosticar con lo que el hijo alcanzó a imprimir). La escalera de apagado mata al **grupo**
  completo (los nietos de un `sh -c "a | b"` también).
- **`.merge_output()`** manda stderr al MISMO pipe que stdout: el entrelazado es el orden real
  en que el hijo escribió (fusionar después inventa un orden); `stderr` vuelve vacío.
- Solo Unix (macOS/Linux); en Windows devuelve un `Err` honesto de plataforma.

**Streaming** (`.stream()`): para consumir la salida MIENTRAS el hijo corre (logs largos, un
`tail -f`, un proceso que no termina), en vez de esperar el `Output` final. Devuelve un `Proc`
con dos canales acotados (`out`/`err`, trozos `bytes`; su cierre marca el fin del flujo) y
`wait()`/`kill(force)`:

```rust
import std/process;

fn main() -> int {
    let p = match (process.cmd("sh", ["-c", "echo uno; echo dos"]).stream()) {
        Result.Ok(p) => p,
        Result.Err(e) => { print("no se pudo lanzar: ${e}"); return 1; },
    };
    var going = true;
    while (going) {
        match (recv(p.out)) {                       // bloquea hasta el siguiente trozo
            Option.Some(chunk) => {
                match (from_utf8(chunk)) {
                    Result.Ok(s) => print("chunk: ${s.trim()}"),
                    Result.Err(e) => print("chunk binario"),
                }
            },
            Option.None => { going = false; },      // el canal cerró: fin de stdout
        }
    }
    match (p.wait()) {                              // cosecha SIEMPRE al final
        process.Exit.Code(c) => print("salió con ${c}"),
        process.Exit.Signal(s) => print("señal ${s}"),
    }
    0
}
```

- La **contrapresión** es parte del diseño: si dejas de recibir, el canal (acotado) se llena, la
  bomba deja de leer, el pipe del SO se llena y el hijo se bloquea en su `write` — nadie acumula
  memoria sin límite. Por eso `stream()` **no tiene** `timeout_ms` ni `max_output`: el tope ES el
  canal, y un plazo lo compones tú (`deadline` de `std/resilience` + `p.kill(false)`).
- `p.kill(force)` manda `SIGTERM` (o `SIGKILL` con `force`) al **grupo** del hijo; tras `wait()`
  es un no-op (jamás una señal a un pid reciclado). `wait()` se llama una vez, tras drenar.
- Con `.merge_output()`, todo llega por `p.out` y `p.err` nace cerrado.
- El proceso es **hijo del scope**, como una tarea: si una hermana falla, el grupo del hijo se
  mata y cosecha con la cancelación; y un proceso al que nunca llamaste `wait()` **no sobrevive a
  su scope** (se mata y cosecha al salir). Fuera de un `scope`, el ciclo de vida es tuyo
  (`wait`/`kill`).
- Solo **VM y binario nativo** (usa fibras y canales, como todo `spawn`); el intérprete lo
  rechaza con su error de concurrencia.

### Tiempo y duraciones (`std/time`)

Relojes (`now()` epoch-ms, `monotonic()` para medir intervalos), `sleep(ms)` cooperativo, y las
fechas civiles UTC (`DateTime`, ISO 8601/RFC 1123). La **moneda de duración** de toda la stdlib es
el `int` en **milisegundos**; para no escribir `300000` a mano están los **constructores de
duración**, pensados para leerse en UFCS:

```rust
import std/time;
from std/time import seconds, minutes, hours;

fn main() {
    time.sleep(2.seconds());                      // 2000 ms
    let ttl = 15.minutes();                       // 900000
    print(time.format_duration(1.hours() + 30.minutes()));  // 1h30m
}
```

`millis(n)` (identidad, para explicitar la unidad), `seconds`, `minutes`, `hours` y `days` (de 24
horas exactas; los calendarios son asunto de `DateTime`). Nota: la forma `2.seconds()` requiere el
import **sin calificar** (`from std/time import seconds`); con `import std/time;` se usa
`time.seconds(2)`.

El hermano para los **tamaños** es `std/units`: `kb`, `mb`, `gb` → bytes, en convención **binaria**
(1 KB = 1024 bytes, la lectura habitual en código de sistemas — buffers, límites de memoria):

```rust
from std/units import kb, mb;

let buf_max = 64.kb();     // 65536
let upload_cap = 16.mb();  // 16777216
```

## 14. Red y bases de datos

### Sockets y TLS (`std/net`, embebida)

```rust
import std/net;

let h = net.tcp_connect("example.com", 80)?;      // Result<int, string> (handle)
let _ = net.socket_write_bytes(h, peticion.to_bytes())?;
let respuesta = net.socket_read_bytes(h)?;        // un trozo; repite hasta tener el mensaje
close(h);
```

TLS: `net.tls_connect(host, 443)` (verifica el certificado; CAs extra vía `SSL_CERT_FILE`),
`net.tls_upgrade(h, host)` (STARTTLS sobre un socket ya abierto), `net.tls_accept` (lado servidor),
`net.tcp_listen`/`net.tcp_accept` para servir.

### La pila de protocolos (`packages/net`, dependencia)

24 módulos en raylang puro: HTTP(S) cliente (`http.fetch` con redirects/chunked/gzip), **servidor web**
async con SSE (`webserver`), WebSocket (cliente y servidor, ws/wss), HTTP/2 + gRPC, DNS (7 tipos de
registro + caché), Redis, OAuth2, JWT (HS256/EdDSA), SCRAM, AWS SigV4, cookies, logging JSON, métricas
Prometheus, fechas UTC, tracing distribuido W3C (`trace`). Se declara como dependencia (`net = "path:…"` o git) y:

```rust
import net/http;

fn main() -> int {
    match (http.fetch("https://example.com/")) {
        Result.Ok(resp) => print(resp.body),
        Result.Err(e) => print(e),
    }
    0
}
```

### El framework web (`packages/web`, dependencia)

Sobre el `webserver` de `net`, el paquete `web` trae un **framework de aplicación estilo Express**
(`web/framework`): rutas por método con parámetros (`/users/:id`), catch-all (`*resto`) y regex,
middleware (global, por prefijo o por ruta), hooks `after`, CORS en una línea, archivos estáticos
con ETag/304 y `Cache-Control`, cookies, redirects, 404 personalizable, sub-aplicaciones (`mount`),
logging JSON por petición con trace-id, y respuestas JSON tipadas vía el trait `ToJson`:

```rust
from web/framework import new_app, listen, GET, json_of, text, App, Ctx, Res, ToJson;

fn build_app() -> App {
    var app = new_app();
    app.GET("/hola/:nombre", fn(c: Ctx, r: Res) {
        r.text("hola, " + c.param("nombre"));
    });
    app.GET("/yo", fn(c: Ctx, r: Res) {
        r.json_of(User { id: 7, name: "Ada" });   // User implementa ToJson
    });
    app
}

fn main() -> int {
    match (listen(build_app, "127.0.0.1", 8080)) {   // keep-alive + límites + panic→500 heredados
        Result.Ok(_) => 0,
        Result.Err(e) => { eprint(e); 1 },
    }
}
```

El mismo fuente corre en la VM (`ray run`/`ray dev`) y compila a binario nativo (`ray build
--native`). Variantes: `listen_tls` (HTTPS), `listen_graceful` (drena al recibir SIGTERM),
`listen_limits`. La **guía completa** (composición de middleware, formularios, `json_body`,
SSR con templates, estado compartido, deploy) está en [`docs/web-framework.md`](docs/web-framework.md);
el demo en [`examples/web/framework/`](examples/web/framework/).

### RPC entre servicios (`packages/rpc`, dependencia)

La comunicación **nativa** servicio-a-servicio (M88.4), sin el peso de HTTP: framing con prefijo
de longitud sobre TCP + JSON, request/response con id correlado, deadline y traceparent en el
sobre. Servidor con una fibra por conexión y **apagado ordenado de serie**:

```rust
import rpc/rpc;
from std/json import Json;

// Servidor: rpc.serve_graceful("0.0.0.0", 7070, 5000, handler)  (SIGTERM → drena y devuelve)
// Cliente:
fn main() -> int {
    let c = match (rpc.connect("127.0.0.1", 7070)) {
        Result.Ok(x) => x,
        Result.Err(e) => { print(e); return 1; },
    };
    match (rpc.call(c, "ping", Json.JNull)) {              // → Result<Json, string>
        Result.Ok(j) => print("respuesta ok"),
        Result.Err(e) => print("err: " + e),
    }
    let r = rpc.call_deadline(c, "consulta", Json.JNull, 500);  // espera acotada a 500 ms
    rpc.disconnect(c);
    0
}
```

Un handler que devuelve `Err` (o que panica) llega al cliente como `Err` del `call`, sin matar
la conexión. Interop externo entrante: el webserver (HTTP/1.1 + JSON), que ya está.

### Bases de datos (`packages/db`, dependencia)

Clientes para **MySQL**, **PostgreSQL**, **SQLite** (embebido, sin servidor) y **MongoDB**, con API
uniforme y **binding de parámetros** (anti-inyección) en los cuatro:

```rust
import db/sqlite;

fn main() -> int {
    var c = match (sqlite.connect(":memory:")) {
        Result.Ok(conn) => conn,
        Result.Err(e) => { print(e); return 1; },
    };
    let sin: [string] = [];
    let _ = sqlite.exec(c, "CREATE TABLE u (id INTEGER PRIMARY KEY, nombre TEXT)", sin);
    let _ = sqlite.exec(c, "INSERT INTO u (nombre) VALUES (?1)", ["ada"]);   // ?1 enlazado
    match (sqlite.query(c, "SELECT id, nombre FROM u", sin)) {
        Result.Ok(rows) => { for fila in rows { print(fila.join(" | ")); } },
        Result.Err(e) => print(e),
    }
    sqlite.disconnect(c);
    0
}
```

MySQL/PostgreSQL/MongoDB hablan su protocolo wire en raylang puro, con variantes `connect_tls` (canal
cifrado) — placeholders `?` (mysql), `$1` (postgres), documentos BSON (mongo). Demos en
[`examples/db/`](examples/db/).

### Tracing distribuido (`net/trace`, en el paquete `net`)

Para seguir UNA petición a través de varios servicios: el contexto W3C Trace Context
(`traceparent`) se adopta en el servidor, se estampa en los logs y se propaga en las llamadas
salientes — cada salto como un *span* hijo (mismo `trace_id`, `span_id` fresco):

```rust
import net/webserver;
import net/http;
import net/log;

fn handler(req: webserver.Request) -> webserver.Response {
    let t = webserver.trace_of(req);              // adopta el traceparent entrante (o crea uno)
    let lg = log.with_trace(log.logger("mi-svc"), t.trace_id);
    log.emit(log.info(lg, "procesando"));         // …,"trace_id":"4bf9…","msg":"procesando"…
    let r = http.request_traced("GET", "http://otro-svc/dato", "", Map.new(), t);
    webserver.ok("hecho")                          // el agregador junta los logs por trace_id
}
```

### Resiliencia (`std/resilience`, embebida)

En un servicio los fallos parciales son lo normal; el kit da las tres piezas estándar, genéricas
sobre cualquier `fn() -> Result<T, E>`:

```rust
import std/resilience;

fn main() -> int {
    // Retry con backoff exponencial + jitter (duerme cediendo la fibra).
    let p = resilience.policy(4, 100, 2000);      // 4 intentos: ~100, 200, 400 ms + jitter
    let r = resilience.retry(p, fn() -> Result<string, string> { llamada_flaky() });

    // Circuit breaker: 5 fallos seguidos lo ABREN 10 s (falla en seco sin llamar).
    let b = resilience.breaker(5, 10000);
    let r2 = resilience.guard(b, "circuito abierto", fn() -> Result<string, string> {
        llamada_flaky()
    });

    // Deadline: un presupuesto de tiempo que se enhebra por las llamadas.
    let d = resilience.deadline(1500);
    if (!resilience.expired(d)) {
        // … y a la E/S de verdad: net.set_read_timeout(h, resilience.remaining(d))
    }
    0
}
```

Sin preempción (el modelo es cooperativo), un deadline no corta una llamada bloqueada "desde
fuera": es un **presupuesto** que consultas (`remaining`/`expired`) y aplicas de verdad en la E/S
con `net.set_read_timeout`.

## 15. Concurrencia

Modelo de **actores con aislamiento de heap**: fibras (`spawn`) que se comunican por **canales** tipados. No
hay estado mutable compartido entre fibras; corre en **multicore** por defecto (los programas sin `spawn` no
pagan nada).

```rust
fn main() -> int {
    let ch: Channel<int> = Channel.new();
    spawn(fn() {
        var i = 0;
        while (i < 5) { send(ch, i * i); i = i + 1; }
        close(ch);
    });
    var total = 0;
    var seguir = true;
    while (seguir) {
        match (recv(ch)) {                 // recv -> Option<T>; None al cerrar+vaciar
            Option.Some(v) => { total = total + v; },
            Option.None => { seguir = false; },
        }
    }
    print(total);                          // 0+1+4+9+16 = 30
    0
}
```

### Canales

- `Channel.new()` — sin límite (send nunca bloquea).
- `Channel.bounded(n)` — acotado: con la cola llena, `send` **bloquea** (backpressure). `n = 0` =
  rendezvous síncrono.
- `close(ch)` — los valores pendientes aún se reciben; después `recv` da `None`.
- `signals() -> Channel<int>` — el canal de **señales del SO** (SIGTERM=15, SIGINT=2), para el
  **apagado ordenado** de un servicio: compone con `recv`/`select` (drena tu canal de trabajo O
  apaga). Singleton del proceso; solo VM y unix. Ejemplo completo en
  [`examples/concurrency/senales.ray`](examples/concurrency/senales.ray). Para un servidor web no
  hace falta cablearlo a mano: `webserver.serve_graceful(host, port, drain_ms, handler)` ya lo
  hace — con SIGTERM deja de aceptar, **drena las peticiones en vuelo** con plazo y devuelve 0
  (cero peticiones perdidas al desplegar); `serve_shutdown` es la forma general con cualquier
  canal `stop`.
- `select([ch1, ch2, …]) -> int` — espera al primero listo y devuelve su índice (el menor listo;
  determinista). Sigue con `recv(chs[i])`. Ojo: un canal cerrado queda "listo" para siempre — sácalo de
  la lista.

### Tareas y concurrencia estructurada

`spawn` devuelve un `Task<T>`; `join` espera su resultado. `scope` ata el ciclo de vida:

```rust
fn main() -> int {
    let total = scope(fn() -> int {
        let a = spawn(fn() -> int { calcular(1) });
        let b = spawn(fn() -> int { calcular(2) });
        join(a) + join(b)
        // al salir del scope: une TODO lo lanzado dentro; si una tarea falló,
        // cancela a sus hermanas y propaga el primer fallo
    });
    print(total);
    0
}
```

### Recuperación de errores fatales (`try_call` y `try_join`)

Un error fatal (`panic`, división por cero, índice fuera de rango, overflow) aborta el programa… salvo
que lo envuelvas. Hay dos formas, y la diferencia entre ellas es cuánto aíslan:

| | qué hace | aislamiento | motores |
|---|---|---|---|
| **`try_call(f)`** | corre `f` **aquí mismo** y devuelve `Result<T, string>` | ninguno: lo que `f` mutó antes de fallar sigue mutado | los tres |
| **`try_join(spawn(f))`** | corre `f` en una **tarea** y observa su desenlace | total: heap propio, se descarta entero al fallar | solo VM y nativo (`spawn` no corre en el intérprete) |

**Empieza por `try_call`**: es el `recover` general, no necesita tarea, y no paga ni un cambio de hilo.

```rust
fn main() -> int {
    match (try_call(fn() -> int { procesar_entrada_dudosa() })) {
        Result.Ok(v) => print("resultado: " + to_string(v)),
        Result.Err(msg) => eprint("fallo, sigo con el resto: " + msg),
    }
    0
}
```

⚠️ `try_call` recupera **en la misma fibra**, así que el estado que el cuerpo mutó antes de fallar
sigue ahí (el mismo trade-off que `catch_unwind` en Rust, o que un `except` en Python). Si lo que
necesitas es que un fallo no deje nada a medias, usa una tarea:

Un error fatal dentro de una **tarea** queda capturado en su `Task<T>` y raylang te deja decidir.
`join` lo **re-lanza** (propagación por defecto: un fallo no se pierde en silencio); **`try_join` lo
devuelve como valor** — `Result<T, string>` con el mensaje del fallo — y el programa sigue. Es el
`recover` de Go, pero sin magia posicional: el fallo es un `Result` ordinario que se maneja con
`match`/`?`.

```rust
fn main() -> int {
    let t = spawn(fn() -> int { procesar_entrada_dudosa() });
    match (try_join(t)) {
        Result.Ok(v) => print("resultado: " + to_string(v)),
        Result.Err(msg) => eprint("fallo, sigo con el resto: " + msg),
    }
    0
}
```

Reglas:

- Los dos capturan **cualquier** error de ejecución, no solo `panic` explícito: división por cero,
  índice fuera de rango, overflow.
- `try_call` **anida**: el `try_call` más interno gana, y un fallo posterior en el cuerpo de fuera lo
  recupera el de fuera.
- En el binario nativo, el mensaje de algunos errores de runtime (p. ej. un índice fuera de rango)
  difiere del de la VM, porque allí el chequeo lo hace el propio Rust. El comportamiento —que se
  recupere— es idéntico en los tres motores; solo cambia el texto.
- **Una tarea es de un solo consumidor**: `join`/`try_join` la *consumen* (liberan su resultado — así
  un servidor de larga vida no acumula memoria por tarea terminada). Unirla dos veces, o unir una
  tarea cuyo `scope` ya cerró, es un error de ejecución (`task already consumed`).
- **Un fallo observado es un fallo manejado**: dentro de un `scope`, una tarea cuyo fallo ya
  observaste con `try_join` cuenta como terminada — el scope no cancela a sus hermanas ni re-lanza.
  Un fallo **no** observado conserva el comportamiento estructurado: cancela hermanas y propaga.
- `join` re-lanza siempre (observar ≠ unir). Un deadlock del scheduler no es recuperable (no queda
  ninguna fibra viva que pueda observarlo).
- Para lotes tolerantes a fallos: lanza una tarea por ítem y acumula los `Err` — un ítem corrupto no
  aborta el lote. El webserver ya usa este mecanismo por dentro: un handler que revienta responde
  500 (con su `trace_id` en el log) y el servidor sigue sirviendo.
- La tarea aísla el fallo también en memoria: su heap es propio (modelo de actores) y se descarta
  entero al fallar — no hay estado compartido a medio mutar que quede visible.

### Determinismo y límites

- `ray run --deterministic` (o `RAYLANG_THREADS=1`): un solo hilo, orden FIFO — salida reproducible
  (tests).
- `ray run --fuel N` / `--heap N`: límites de instrucciones y de objetos vivos, para embeber raylang
  confinado (un bucle infinito o una fuga no cuelgan al anfitrión).
- La concurrencia requiere la **VM** (el default); `--interp` es el oráculo secuencial de desarrollo.

## 16. FFI: llamar a C

`extern "lib" { … }` declara funciones C que se cargan dinámicamente y se llaman como cualquier función.
**Es la única frontera insegura del lenguaje**: la firma declarada se confía (una firma equivocada es
comportamiento indefinido).

```rust
extern "m" {                                   // "m" → libm (o el propio proceso si ya está enlazada)
    fn sqrt(x: float) -> float;
    fn pow(base: float, exp: float) -> float;
}

extern "c" {
    fn strlen(s: string) -> int;               // string → char* NUL-terminado (copia temporal)
    fn getenv(name: string) -> Option<string>; // char* de retorno: NULL → None; si no, copia
    fn fopen(path: string, mode: string) -> Option<ptr>;   // puntero opaco (no desreferenciable)
    fn fgetc(f: ptr) -> int;
    fn fclose(f: ptr) -> int;
}

fn main() -> int {
    print(sqrt(2.0));
    print(strlen("hola"));       // 4
    0
}
```

Reglas (tabla completa en [`REFERENCE.md` §13](REFERENCE.md#13-ffi-tipos-marshalables)):

- Aridad 0 a 6 (el checker rechaza más parámetros); tipos: `int` (C `int`), `u64` (C `long`/`size_t`), `float`, `bool`, `string`/`bytes`
  (solo argumento), `ptr`, `unit`; retornos falibles como `Option<string>`/`Option<bytes>`/`Option<ptr>`.
- Los `bytes` que pases NUL-termínalos tú si el C lo espera (`b"texto\x00"`).
- Fuera de contrato: variádicas (`printf`), structs por valor, callbacks.
- No disponible en el playground web (wasm).

**¿La función C bloquea?** Márcala: `extern "c" blocking { … }`. En el binario nativo la concurrencia
corre sobre fibras fijadas a pocos workers; una llamada C que bloquea (E/S, una C-lib lenta) dejaría
varadas a las demás fibras de su worker. Con `blocking`, la llamada corre en un hilo de un pool aparte
y la fibra espera aparcada — mismo resultado, worker libre:

```rust
extern "c" blocking {
    fn sleep(seconds: int) -> int;             // bloquea de verdad → al pool
}
```

La marca no cambia valores ni tipos; donde no hay fibras (VM, `--without fibers`, fuera de una tarea
`spawn`) es inerte. Para llamadas cortas de CPU (`sqrt`, `strlen`) no la uses: el viaje al pool cuesta
más que la llamada.

**¿La función C falló y quieres saber por qué?** Las APIs POSIX dejan el motivo en `errno`:

```rust
import std/ffi;

extern "c" {
    fn fopen(path: string, mode: string) -> Option<ptr>;
}

fn main() -> int {
    match (fopen("/no/existe.txt", "r")) {
        Option.Some(f) => print("open ok"),
        Option.None => print(ffi.errno()),   // 2 = ENOENT; leerlo JUSTO tras la llamada
    }
    0
}
```

La regla: lee `ffi.errno()` **inmediatamente** después de la extern, sin E/S de raylang en medio
(cualquier operación que aparque la fibra deja correr a sus hermanas, que pueden pisarlo). Funciona
igual tras una extern `blocking` (el runtime trae el errno del hilo del pool de vuelta).

**Pila**: en el binario nativo con fibras, tu llamada C corre sobre la pila de la fibra. Con externs
declaradas, el default sube solo a **1 MiB** por fibra (de los 128 KiB habituales; reserva virtual,
solo cuestan las páginas que se tocan). Si una C-lib necesita aún más, `RAY_FIBER_STACK_KIB=4096`
manda — la variable siempre gana al default.

## 17. Herramientas

```sh
ray dev [archivo]        # modo desarrollo: recompila y REINICIA ante cambios (solo si compila)
ray fmt archivo.ray      # formatea (canónico e idempotente); --write / -w reescribe en el sitio
ray test [archivo]       # corre las funciones @test (filtro opcional por nombre)
ray doc archivo.ray      # documentación Markdown desde ///
ray build --templates-only vistas/        # compila templates .ray.html a funciones raylang tipadas (ver abajo)
ray repl                 # REPL interactivo
ray lsp                  # servidor LSP (diagnósticos, hover, definición, rename, completion…)
ray mcp                  # servidor MCP: expone check/run/test/fmt/doc a un agente LLM (docs/mcp.md)
ray build                # chequea + compila sin ejecutar (para CI: 0 ok / 65 error)
ray build --native       # transpila a Rust y compila un binario nativo (deploy)
```

**Compilación a binario nativo** (`ray build --native`): además de correr sobre la VM, un programa se
puede **transpilar a Rust** y compilar a un ejecutable de código máquina — el modelo *dev = VM / deploy =
nativo*, como el ciclo dev/release de Rust. El binario es **byte-idéntico a la VM** (verificado con
oráculos) y mucho más rápido: **3–4× la VM** en cargas de servicio y **28–57×** en cómputo puro. En el
banco poliglota le gana a node en 9 de los 10 programas de cómputo, a Go en seis y a `rustc -O` en
cuatro, empatando con ambos en otros dos (medido 29 jul 2026; tablas en `benchmarks/poly/README.md`).

```sh
ray build --native fib.ray            # → binario './fib' (rustc -O, ~0,2 s, portable)
ray build --native fib.ray -o bin/fib # nombre de salida a medida
ray build --native fib.ray --release  # tier opt3+lto+target-cpu=native (~10% extra en alloc, binario no portable)
```

Transpila el **lenguaje completo** (genéricos, traits, `dyn`, closures, `Map`, `match`…) + toda la I/O de
`std/fs`, sockets TCP/UDP, procesos del SO, FFI y la concurrencia: por defecto sobre el **scheduler M:N
de fibras** (corrutinas de pila propia + reactor `kqueue`/`epoll`, el mismo modelo que la VM), con
`--without fibers` como escape al modelo hilo-por-tarea. Los subsistemas con un
**crate de producción** —TLS (`rustls`), criptografía (`ring`), SQLite (`rusqlite`), regex acelerada— se enlazan **solo
cuando el programa los usa**: el transpilador genera un proyecto Cargo y compila con `cargo` enlazando solo
ese crate (mensaje `ok: … [ray-runtime: crypto]`). Si tu programa no toca ninguno, se compila con `rustc`
pelado (rápido, sin red). El binario nativo llama **al mismo código** que la VM (vía el crate compartido
`ray-runtime`) → paridad por construcción.

Para excluir un subsistema (build hermético, *cross-compile*, contenedor endurecido, o cuando el camino
con-crate es inalcanzable), `--without` lo deja fuera: los detectados por uso (`crypto`, `tls`, `sqlite`)
caen en un *stub* con error claro, `regex` vuelve al motor escrito en raylang, y los que van **por
defecto** (`mimalloc`, `ahash`, `fibers`, `process`) se desactivan — quitar los tres primeros devuelve el
binario a la vía rápida de `rustc` pelado con hilo-por-tarea:

```sh
ray build --native app.ray --without tls,sqlite
ray build --native app.ray --without mimalloc,ahash,fibers   # rustc pelado, sin proyecto Cargo
```

Cuando es una **política estable** del proyecto, decláralala en `ray.toml` (el `--without` de CLI se
**une** a ella):

```toml
[native]
without = ["tls", "sqlite"]   # este servicio nunca enlaza TLS ni SQLite en el binario nativo
```

**Modo desarrollo** (`ray dev`): vigila los fuentes del proyecto (`.ray`, `.ray.html`, `ray.toml`)
y ante un cambio reinicia el programa — un template editado se regenera al relanzar, y un servidor
con `serve_graceful` **drena** sus conexiones antes de morir (el reinicio manda SIGTERM). Con la
compilación en milisegundos, el ciclo editar→ver es de decenas de ms. Un programa que termina solo
queda a la espera del siguiente cambio.

Antes de reiniciar, `ray dev` **compila primero** (chequeo en ms) y **solo reinicia si el cambio
compila**: un error a medio escribir imprime su diagnóstico y **deja el programa anterior en marcha**
(no tira un servidor que funciona por un cambio roto). Además hace **debounce** de una ráfaga de
guardados (p. ej. tu editor + un formateador) en un solo reinicio.

**Sin conexiones caídas entre reinicios** (unix): con `ray dev --port 8080` (o `--listen host:port`,
o `[dev] listen = "127.0.0.1:8080"` en `ray.toml`) el supervisor **retiene el socket de escucha** y
cada reinicio lo **adopta** en vez de re-vincularlo — el puerto nunca se cierra, así que una petición
que llega durante el reinicio **se encola** (no se rechaza) y la atiende el programa nuevo. El downtime
percibido es el de una compilación (ms). Tu `tcp_listen`/`serve` no cambia: adopta el socket heredado
de forma transparente cuando el `host:port` coincide. (El estado en memoria del programa **se resetea**
por reload; si quieres estado persistente entre reloads, guárdalo en un `sqlite.connect("dev.db")`.)

Y **live-reload del navegador**: en una sesión con `--port`, `ray dev` levanta un canal SSE lateral e
**inyecta** en tus respuestas HTML (del paquete `webserver`) un `<script>` que **refresca la página**
sola cuando un cambio compila y reinicia. No tocas nada; en producción no se inyecta. (Es un canal
solo-de-dev, distinto del SSE de tu aplicación: tu `sse_open`/`sse_event` sigue en tu puerto, intacto.)

Pruebas con `@test` (una función `() -> bool` o `() -> unit` que usa `assert`); cada test corre
**aislado** (un panic no tumba la batería):

```rust
@test
fn suma_ok() -> bool { 2 + 2 == 4 }

@test
fn con_assert() {
    assert(1 < 2);
    assert_eq(cuadrado(3), 9);
}
```

Las `@test` pueden vivir **en cualquier módulo** del proyecto (junto al código que prueban): `ray
test` corre las de la entrada y las de todos los módulos que importa, calificadas por su módulo
(`math.suma_ok`). Además, cada archivo `tests/*.ray` junto al `ray.toml` es una **suite de
integración**: importa los módulos del proyecto (`import math;`) y sus `@test` corren como una
suite aparte. Un fallo reporta su mensaje **y su ubicación** (`at módulo:línea:col`, apuntando a tu
`assert`, no al prelude).

`ray test` sale con **0** (todo verde) o **1** (hubo fallos) — ideal para CI; 65 si algo no
compila. Un filtro (`ray test suma`, o `ray test archivo.ray suma`) selecciona por subcadena del
nombre.

### Templates compilados (`.ray.html`)

Para SSR, además del motor runtime (`std/template`, §12), un template puede **compilarse a una
función raylang tipada**: el archivo `vistas/lista.ray.html` declara su firma en la primera línea y
**es un módulo importable tal cual** — el loader lo compila en memoria al resolver `import
vistas/lista;`, sin generar ningún archivo en el proyecto. Reparto de roles: los compilados son
la opción por defecto (tipados, y solo ellos soportan `{% include %}`/`{% extends %}`/`{% block %}`/
`{% let %}`); el motor runtime es un subconjunto (interpolación + `if`/`for`) para plantillas
**dinámicas** — cargadas de disco o BD en caliente — que no existen en build time:

```html
{% params titulo: string, filas: [string] %}
<h1>{{ titulo }}</h1>
<ul>{% for fila in filas %}<li>{{ fila }}</li>{% endfor %}</ul>
```

No hay nada que regenerar ni commitear: `ray run`/`ray build`/`ray test` compilan el template al
vuelo cada vez (el template es la única fuente de verdad), y **los errores apuntan al template**:
un typo en `{{ titluo }}`, una expresión mal formada o un error de runtime se reportan con la
línea y el fuente del `.ray.html`. Para **inspeccionar** el módulo generado,
`ray build --templates-only vistas/` lo materializa al lado
(`vistas/lista.ray`, `pub fn render(...) -> string`); si ese `.ray` queda en el proyecto, el
loader lo ignora y sigue prefiriendo el template.

```rust
import vistas/lista;
let html = lista.render("Informe", filas);
```

Los `{{ expr }}` admiten expresiones raylang (`{{ p.nombre }}`, `{{ n + 1 }}`) con autoescape HTML
(`{{& expr }}` crudo); `{% if/elif/else %}`, `{% for %}` y `{% let nombre = expr %}` (local
inmutable) son los de raylang. Los templates **componen**: `{% include vistas/tarjeta(n) %}`
incluye otro template **por su ruta** — sin conocer el nombre de la función generada ni importar
nada (el generador importa y llama al `render` él) — y empalma su HTML **sin re-escapar** (el
partial ya escapó sus datos). Un `{% include expr %}` sin la forma `ruta(args)` empalma la
expresión cruda (p. ej. el `contenido` de un layout); `{% import ruta [as x] %}` sigue disponible
para usar funciones de otro módulo en las expresiones. Y **heredan** (estilo Jinja, resuelto en
compilación): el layout marca huecos con `{% block cuerpo %}defecto{% endblock %}` y el hijo hace

```html
{% params titulo: string, precios: [int] %}
{% extends vistas/base %}
{% block cuerpo %}<p>{{ precios.len() }} precios</p>{% endblock %}
```

Las rutas de `{% include %}`, `{% import %}` y `{% extends %}` siguen **una sola convención**:
desde la **raíz del proyecto** (donde vive `ray.toml`), como los `import` de raylang.

— la firma es la del hijo (las variables que use el layout deben estar en sus params: el checker
lo exige), un bloque no sobreescrito conserva su defecto, y el layout compila también standalone.
La ventaja sobre el
motor runtime: **un typo en una variable es error de compilación** (no un `""` silencioso) y el
render es ~2× más rápido (cero parseo en runtime). El motor runtime queda para plantillas dinámicas
(cargadas de disco/BD en caliente). En el editor (VSCode/Sublime + `ray lsp`), un `.ray.html` tiene
diagnósticos en vivo (errores del template y de tipos, mapeados a su línea), **autocompletado
dentro de `{{ }}`/`{% %}`** (los params tipados de la cabecera, las variables de los `{% for %}`
en ámbito, y tras un `.` los métodos/builtins del receptor), **hover** con el tipo real de la
expresión, **ir-a-definición** (un param lleva a su `{% params %}`), **signature help** al
escribir los argumentos de una llamada, y **formateo** (también `ray fmt archivo.ray.html`):
cada `{% %}` en su línea con indentación por bloques, `{{ }}` inline.

**Editores**: extensión de VSCode (`editors/vscode`) y paquete de Sublime Text (`editors/sublime`), ambos
sobre `ray lsp`; Neovim/Helix lo usan directo. **Playground web**: la VM compilada a WASM corre el lenguaje
núcleo en el navegador (sin red/FFI/cripto).

Documenta con `///` (en la línea anterior a `fn`/`struct`/`enum` públicos): lo leen `ray doc` y el hover
del editor.

## 18. Recomendaciones y mejores prácticas

- **`let` por defecto, `var` solo si mutas.** Un binding inmutable comunica intención y evita bugs.
- **Errores como valores, no `panic`.** Devuelve `Result`/`Option` y propaga con `?`. Reserva
  `panic`/`assert` para invariantes internas (bugs), no para entrada del usuario.
- **Deja que el checker trabaje por ti.** Los `match` son exhaustivos: si añades una variante a un enum, el
  compilador te señala cada `match` que falta actualizar. Aprovecha eso — evita el brazo `_` cuando quieras
  esa red de seguridad.
- **Anota las firmas, infiere los locales.** Las firmas son documentación y frontera de tipos; los locales
  (`let x = …`) se leen mejor sin anotación salvo que sea indeterminado.
- **UFCS y pipelines para legibilidad.** `datos.filtra().mapea()` o `datos |> filtra() |> mapea()` se leen de
  izquierda a derecha, en el orden en que ocurren.
- **Iteradores perezosos para transformar colecciones.** `.iter().map().filter().collect()` no crea arreglos
  intermedios; fusiona el trabajo en una sola pasada.
- **`StringBuilder` para acumular strings en bucle.** `s = s + trozo` en un bucle es O(n²).
- **Módulos: `pub` mínimo + cápsulas.** Expón solo la superficie que otros necesitan; usa `mod.ray` para que
  los internos de un paquete queden protegidos.
- **Concurrencia por canales, no por estado compartido.** El modelo de actores lo hace natural: pasa datos por
  un canal en vez de compartir una estructura mutable. Canales acotados donde haya productores rápidos.
- **SQL siempre con parámetros.** Los cuatro clientes de `db` enlazan (`?`/`$1`/BSON); no interpoles valores
  en el SQL.
- **Prueba con `@test` + `assert_eq`.** Barato y va con el código; `ray test` en tu CI.
- **Formatea con `ray fmt`.** Un estilo único, sin discusiones.
- **Nombres sin sobrecarga.** raylang no tiene sobrecarga de funciones, así que la stdlib desambigua por
  nombre: p. ej. `index_of` (buscar en string) vs `position` (buscar en arreglo). Sigue esa convención.
- **La cripto de producción son los builtins** (`sha256`, `hmac_sha256`, …, respaldados por `ring`, tiempo
  constante). Las implementaciones en raylang de `examples/web/` son demostración del lenguaje.

## 19. Gotchas

Cosas que sorprenden viniendo de otros lenguajes:

- **No hay `break` ni `continue`.** El reemplazo canónico es extraer el bucle a una función y usar
  `return`; los demás patrones (búsquedas de la stdlib, invertir la condición, `.take(n)`, variable
  de control) están en §4, "Salir temprano sin `break`".
- **`match` es solo para enums.** Destructura `Option`/`Result`/tus enums; **no** hay patrones de literal ni
  `match` sobre `int`/`bool`/`string`. Para despachar sobre un primitivo, usa `if/else`. Las guardas
  (`patrón if cond`) sí permiten condiciones dentro de un `match` de enum.
- **Los brazos de `match` con cuerpo de bloque llevan coma.** `Option.Some(v) => { hacer(v); },` — la coma
  detrás de `}` es necesaria.
- **El escrutinio de `match` va entre paréntesis** (`match (e) { … }`), como `if`/`while`. Evita la
  ambigüedad con el literal de struct.
- **La asignación no es expresión.** `x = y` es sentencia; en un brazo de `match` va en bloque:
  `=> { x = y; },`.
- **Precedencia estilo C en los bit a bit**: `(flags & 32) != 0`, con paréntesis — `&`/`|`/`^` ligan más
  flojo que `==`/`!=`.
- **Un literal en cola tras un `if`/`while` de sentencia se pega al postfijo.** `if (c) { return a; }`
  seguido de `[1, 2]` en posición de retorno se parsea como *indexación* del if (y una tupla, como
  *llamada*). Solución: `return [1, 2];` explícito. Desde M87 **el checker te lo dice**: el error
  ("no se puede llamar/indexar…") lleva la pista "…se parsea como llamada a su valor — sepárala
  con 'return' o 'let'".
- **Firmas explícitas siempre.** Los parámetros y el retorno de una función se anotan; no se infieren.
- **Lo indeterminado pide contexto.** `[]`, `Option.None`, `Map.new()`, `Arbol.Hoja` (variante sin payload
  de un enum genérico) no pueden inferir su tipo solos → anótalo (`let t: Arbol<int> = …`) o dáselo el
  contexto (argumento, retorno).
- **Semántica de referencia** en arreglos/structs/`Map`: al asignarlos o pasarlos a una función, comparten el
  objeto (mutar uno muta el otro). Los primitivos (int/float/bool/char/u\*) y las tuplas se copian.
- **No hay `null`.** Usa `Option<T>` y `match`/`?`.
- **No hay excepciones ni sobrecarga de funciones.** Errores como valores (§7); un nombre, una firma.
- **Un builtin gana a una función de usuario homónima.** No llames `print`/`len`/`get`/`join`… a tus
  funciones: la llamada iría al builtin.
- **`from` es palabra clave** (del sistema de módulos): no vale como nombre de variable o parámetro.
- **Sin aritmética de `char`.** `'a' + 1` es error; usa `char_code`/`char_from_code`.
- **Los strings se indexan por carácter y son inmutables.** `s[i]` da `char` (no byte); `s[i] = c` es
  error. Para bytes crudos: `s.to_bytes()`.
- **`t.0.1` confunde al lexer** (parece el float `0.1`): con tuplas anidadas usa un binding intermedio.
- **En una ruta calificada de módulo se usa el nombre del archivo (leaf)**, no la ruta completa: importas
  `geo/formas/circulo` y lo usas como `circulo.area(...)`. Con colisión de leafs, `import … as otro;`.
- **La interpolación es `"${expr}"` en strings normales** (no hay prefijo `f"…"`). Un `$` sin `{` es
  literal; `\${` escapa.

---

¿Falta algo o quieres el detalle exacto de una construcción? Mira **[`REFERENCE.md`](REFERENCE.md)**
(el catálogo completo), **`SPEC.md`** (normativa) o los **160+ ejemplos** en `examples/`.
