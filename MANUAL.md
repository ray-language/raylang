# Manual de raylang

Una guía **práctica** para escribir programas en raylang: el lenguaje de un vistazo, sus idiomas, y
recomendaciones. Complementa a los otros documentos:

- **`SPEC.md`** — la referencia **normativa** (gramática y semántica exactas). Cuando el manual y la SPEC
  discrepen, manda la SPEC.
- **[`REFERENCIA.md`](REFERENCIA.md)** — el **catálogo exhaustivo**: todas las palabras clave, símbolos,
  operadores con precedencia, builtins, prelude, módulos `std/` y paquetes, con sus firmas.
- **`book/`** — el libro *Construyendo raylang*: cómo se **construyó** el lenguaje, fase a fase (pedagogía de
  implementación).
- **[`PUBLICAR.md`](PUBLICAR.md)** — la guía del **publicador**: empaquetar, versionar y publicar
  en el registro (`ray publish`, índice, yank, garantías del lock).
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

La tabla completa de precedencia (15 niveles) está en [`REFERENCIA.md` §2](REFERENCIA.md#2-símbolos-y-operadores).
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

`while` (no hay `break`/`continue`; ver §19):

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
    fn desde(source: string) -> ErrorApp { ErrorApp { detalle: source } }   // `from` es keyword → `desde`
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
@derive(Eq, Show, Hash)
struct Par { a: int, b: int }

// Eq   → habilita == y assert_eq
// Show → habilita print/to_string ("Par { a: 1, b: 2 }")
// Hash → habilita usarlo como elemento de Set
```

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
ray publish                 # publica TU paquete en el registro (valida + chequea + hashea)
```

El flujo completo del **publicador** (empaquetar, el índice, versionado, `yank`, garantías y
receta de punta a punta) está en [`PUBLICAR.md`](PUBLICAR.md).

## 12. La biblioteca estándar

Tres capas (catálogo completo en [`REFERENCIA.md`](REFERENCIA.md#10-la-biblioteca-estándar-std)):

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

   El catálogo: `math` `text` `sort` `fs` `net` `time` `random` `crypto` `resilience`
   `collections/{set,deque,stringbuilder}` `json` `hex` `base64` `url` `regex` `csv` `toml` `template`
   `inflate` `deflate` `huffman` `protobuf` `uuid`.

3. **Paquetes** (`packages/net`, `packages/db`) — no embebidos; se declaran como dependencia (§14).

## 13. I/O y sistema

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

Reglas (tabla completa en [`REFERENCIA.md` §13](REFERENCIA.md#13-ffi-tipos-marshalables)):

- Aridad 0 a 3; tipos: `int` (C `int`), `u64` (C `long`/`size_t`), `float`, `bool`, `string`/`bytes`
  (solo argumento), `ptr`, `unit`; retornos falibles como `Option<string>`/`Option<bytes>`/`Option<ptr>`.
- Los `bytes` que pases NUL-termínalos tú si el C lo espera (`b"texto\x00"`).
- Fuera de contrato: variádicas (`printf`), structs por valor, callbacks.
- No disponible en el playground web (wasm).

## 17. Herramientas

```sh
ray fmt archivo.ray      # formatea (canónico e idempotente)
ray test [archivo]       # corre las funciones @test (filtro opcional por nombre)
ray doc archivo.ray      # documentación Markdown desde ///
ray repl                 # REPL interactivo
ray lsp                  # servidor LSP (diagnósticos, hover, definición, rename, completion…)
ray build                # chequea + compila sin ejecutar (para CI: 0 ok / 65 error)
```

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

`ray test` sale con el número de fallos como código (0 = todo verde, ideal para CI).

### Templates compilados (`ray templ`)

Para SSR, además del motor runtime (`std/template`, §12), un template puede **compilarse a una
función raylang tipada**: el archivo `vistas/lista.ray.html` declara su firma en la primera línea y
`ray templ` genera `vistas/lista.ray` al lado (commiteable). Reparto de roles: los compilados son
la opción por defecto (tipados, y solo ellos soportan `{% include %}`/`{% extends %}`/`{% block %}`/
`{% let %}`); el motor runtime es un subconjunto (interpolación + `if`/`for`) para plantillas
**dinámicas** — cargadas de disco o BD en caliente — que no existen en build time:

```html
{% params titulo: string, filas: [string] %}
<h1>{{ titulo }}</h1>
<ul>{% for fila in filas %}<li>{{ fila }}</li>{% endfor %}</ul>
```

```sh
ray templ vistas/        # genera vistas/lista.ray (pub fn render_lista(...) -> string)
```

No hace falta acordarse de regenerar: `ray run`/`ray build`/`ray test` **regeneran solos** los
templates cuyo `.ray` falte o esté desactualizado (aviso por stderr).

```rust
import vistas/lista;
let html = lista.render_lista("Informe", filas);
```

Los `{{ expr }}` admiten expresiones raylang (`{{ p.nombre }}`, `{{ n + 1 }}`) con autoescape HTML
(`{{& expr }}` crudo); `{% if/elif/else %}`, `{% for %}` y `{% let nombre = expr %}` (local
inmutable) son los de raylang. Los templates **componen**: `{% include vistas/tarjeta(n) %}`
incluye otro template **por su ruta** — sin conocer el nombre de la función generada ni importar
nada (el generador importa y llama al `render_<x>` él) — y empalma su HTML **sin re-escapar** (el
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

- **No hay `break` ni `continue`.** Estructura el bucle con una variable de control (`var seguir = true;
  while (seguir) { … seguir = false; … }`) o extrae la condición.
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

¿Falta algo o quieres el detalle exacto de una construcción? Mira **[`REFERENCIA.md`](REFERENCIA.md)**
(el catálogo completo), **`SPEC.md`** (normativa) o los **160+ ejemplos** en `examples/`.
