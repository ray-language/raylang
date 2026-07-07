# Manual de raylang

Una guía **práctica** para escribir programas en raylang: el lenguaje de un vistazo, sus idiomas, y
recomendaciones. Complementa a los otros documentos:

- **`SPEC.md`** — la referencia **normativa** (gramática y semántica exactas). Cuando el manual y la SPEC
  discrepen, manda la SPEC.
- **`book/`** — el libro *Construyendo raylang*: cómo se **construyó** el lenguaje, fase a fase (pedagogía de
  implementación).
- **Este manual** — cómo **usar** raylang para programar.

> raylang es un **proyecto de aprendizaje**: real y cuidado, pero no pensado para producción crítica.

## Índice

1. [Primeros pasos](#1-primeros-pasos)
2. [Fundamentos](#2-fundamentos)
3. [Control de flujo](#3-control-de-flujo)
4. [Datos compuestos](#4-datos-compuestos)
5. [Strings](#5-strings)
6. [Errores como valores](#6-errores-como-valores)
7. [Funciones de orden superior e iteradores](#7-funciones-de-orden-superior-e-iteradores)
8. [Genéricos y traits](#8-genéricos-y-traits)
9. [Pattern matching](#9-pattern-matching)
10. [Módulos y paquetes](#10-módulos-y-paquetes)
11. [Concurrencia](#11-concurrencia)
12. [Herramientas](#12-herramientas)
13. [Recomendaciones y mejores prácticas](#13-recomendaciones-y-mejores-prácticas)
14. [Gotchas](#14-gotchas)

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

`print` es un **builtin** (imprime cualquier valor imprimible + un salto de línea). Ejecuta un archivo suelto
con `ray run archivo.ray`.

## 2. Fundamentos

### Valores y tipos primitivos

| Tipo | Ejemplo | Notas |
|------|---------|-------|
| `int` | `42`, `-7` | entero de 64 bits con signo; el desbordamiento es error de ejecución |
| `float` | `3.14`, `2.0` | coma flotante de 64 bits |
| `bool` | `true`, `false` | |
| `string` | `"hola"` | inmutable; con interpolación (§5) |
| `char` | `'a'`, `'\n'` | un carácter Unicode |
| `bytes` | `b"\x00\xff"` | octetos crudos, inmutable |
| `unit` | `()` | "sin valor útil"; lo que devuelve p. ej. `print` |

raylang **no tiene `null`**: la ausencia se modela con `Option` (§6).

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
(salvo casos indeterminados como `[]` vacío o `None`, que piden anotación).

Constantes globales con `const` (su valor debe ser un literal):

```rust
const GRAVEDAD: float = 9.81;     // para π/e usa los builtins `pi()`/`e()`, no una const
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

### Comentarios

```rust
// comentario de línea
/// comentario de documentación (lo lee `ray doc`)
```

## 3. Control de flujo

Todo `if`, bloque y `match` **produce un valor**.

```rust
let max = if (a > b) { a } else { b };        // if como expresión
```

`while` (no hay `break`/`continue`; ver §14):

```rust
var i = 0;
while (i < 5) { print(i); i = i + 1; }
```

`for` sobre cualquier iterador (arreglos, `range`, `.iter()`…):

```rust
for x in [10, 20, 30] { print(x); }
for i in range(0, 5) { print(i); }            // 0,1,2,3,4
```

`match` **destructura enums** (`Option`/`Result`/los tuyos), como expresión (ver §9 a fondo). El escrutinio
va **entre paréntesis**. Para despachar sobre **primitivos** (int/bool/string) se usa `if/else`, no `match`:

```rust
let etiqueta = match (buscar(k)) {      // match: solo sobre enums
    Option.Some(v) => "encontrado",
    Option.None => "no está",
};

let clase = if (n == 0) { "cero" } else if (n < 0) { "negativo" } else { "positivo" };  // int → if/else
```

## 4. Datos compuestos

### Arreglos `[T]`

Semántica de **referencia** (se comparten al pasarlos/asignarlos). Se mutan con `.push(x)`/`a[i] = v`.

```rust
var xs: [int] = [1, 2, 3];
xs.push(4);
xs[0] = 99;
print(xs[0]);          // 99
print(xs.len());       // 4
print(xs.contains(4)); // true — len/push/contains/… son métodos (traits Len/Push/Contains)
```

### Tuplas

```rust
let par = (1, "uno");
print(par.0);                  // 1
let (a, b) = (10, 20);         // destructuring
```

### Structs

```rust
struct Punto { x: int, y: int }

fn main() -> int {
    var p = Punto { x: 1, y: 2 };
    p.x = 10;                  // semántica de referencia (mutable)
    print(p.x);
    0
}
```

### Enums (tipos suma)

```rust
enum Figura { Circulo(float), Rect(float, float), Nada }

fn area(f: Figura) -> float {
    match (f) {
        Figura.Circulo(r) => pi() * r * r,     // pi() es un builtin de matemáticas
        Figura.Rect(w, h) => w * h,
        Figura.Nada => 0.0,
    }
}
```

### `Map<K,V>` y `Set<T>`

```rust
var edades: Map<string, int> = Map.new();
edades.insert("ana", 30);
match (edades.get("ana")) {           // get -> Option<V>
    Option.Some(e) => print(e),
    Option.None => print(0 - 1),
}

var vistos: Set<int> = set_new();
set_add(vistos, 7);
print(set_has(vistos, 7));            // true
```

## 5. Strings

Inmutables, indexables por carácter (`s[i] -> char`), con una stdlib rica y **interpolación** `${expr}`:

```rust
let nombre = "raylang";
let n = 3;
print("hola ${nombre}, ${n} veces");     // "a${x}b" ≡ "a" + to_string(x) + "b"

print("Hola".to_upper());                // "HOLA"
print("a,b,c".split(","));               // ["a", "b", "c"]
print("  x  ".trim());                   // "x"
print("café".len());                     // 4 (por carácter)
print("abc" + "def");                    // concatenación
```

## 6. Errores como valores

No hay excepciones: los fallos son **valores** de `Option<T>` (ausencia) o `Result<T, E>` (éxito/error).

```rust
fn dividir(a: int, b: int) -> Result<int, string> {
    if (b == 0) { Result.Err("división por cero") } else { Result.Ok(a / b) }
}
```

El operador **`?`** desempaqueta el `Ok`/`Some`, o **retorna** el `Err`/`None` de inmediato:

```rust
fn calc() -> Result<int, string> {
    let x = dividir(10, 2)?;       // si Err, calc retorna ese Err
    let y = dividir(x, 0)?;        // aquí retorna Err("división por cero")
    Result.Ok(x + y)
}
```

Para abortar de verdad (bug, invariante rota): `panic("mensaje")`, `assert(cond)`, `assert_eq(a, b)`.

## 7. Funciones de orden superior e iteradores

**Closures** (funciones anónimas con captura):

```rust
let doblar = fn(x: int) -> int { x * 2 };
print(doblar(21));                     // 42
```

**UFCS** (`recv.f(args)` ≡ `f(recv, args)`) y **pipelines** (`x |> f(a)` ≡ `f(x, a)`) para encadenar legible:

```rust
let r = [1, 2, 3, 4]
    |> map(fn(x: int) -> int { x * x })
    |> filter(fn(x: int) -> bool { x > 4 });
```

**Iteradores perezosos** (`.iter()` sobre arreglos, `range`, …) con adaptadores que se fusionan y no
materializan hasta un terminal:

```rust
let suma = [1, 2, 3, 4, 5]
    .iter()
    .map(fn(x: int) -> int { x * x })
    .filter(fn(x: int) -> bool { x % 2 == 1 })
    .sum();                            // 1 + 9 + 25 = 35
```

Adaptadores: `map`/`filter`/`take`/`skip`/`zip`/`enumerate`. Terminales: `fold`/`collect`/`sum`.

## 8. Genéricos y traits

**Funciones genéricas** (inferencia desde los argumentos):

```rust
fn id<T>(x: T) -> T { x }
fn primero<T>(xs: [T]) -> T { xs[0] }
```

**Traits** (comportamiento sobre tipos), con `impl` y despacho estático:

```rust
trait Area { fn area(self) -> float; }        // firma: termina en ';'

struct Circulo { r: float }
impl Area for Circulo {
    fn area(self) -> float { pi() * self.r * self.r }   // pi(): builtin de matemáticas
}

fn imprime_area<T: Area>(x: T) {              // bound: T debe implementar Area
    print(x.area());
}
```

Un trait puede traer **métodos por defecto**, y hay **trait objects** (`dyn Area`) para colecciones
heterogéneas. Traits derivables con `@derive`:

```rust
@derive(Eq, Show)
struct Par { a: int, b: int }
```

## 9. Pattern matching

`match` es exhaustivo (el checker exige cubrir todos los casos). El escrutinio va entre paréntesis; los
brazos con cuerpo de **bloque** llevan **coma** detrás.

```rust
match (opt) {
    Option.Some(v) => print(v),
    Option.None => print(0),
}
```

Patrones: variante con bindings (`Enum.V(x, y)`), binding suelto (`n`), comodín (`_`), **anidados**
(`Result.Ok(Option.Some(v))`), de **struct** (`Punto { x, y }`), y **guardas** (`patrón if cond`):

```rust
match (o) {
    Option.Some(v) if v < 0 => "negativo",
    Option.Some(v)          => "no negativo",
    Option.None             => "sin valor",
}
```

Y azúcar `if let` para un solo caso:

```rust
if let Option.Some(v) = buscar(k) { print(v); }
```

## 10. Módulos y paquetes

Un módulo es un archivo; se importa por su ruta desde la raíz. `pub` expone lo que otros módulos pueden ver.

```rust
// geo/punto.ray  — el struct entero es `pub` (el `pub` por campo aún no existe)
pub struct Punto { x: int, y: int }
pub fn origen() -> Punto { Punto { x: 0, y: 0 } }

// main.ray
import geo/punto;                 // referencia calificada por el LEAF: punto.f()
from geo/punto import origen;     // ...o trae nombres al ámbito

fn main() -> int {
    let p = origen();             // por el from-import
    let q = punto.origen();       // calificado — se usa el LEAF (`punto`), no la ruta
    print(p.x + q.x);
    0
}
```

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

**Paquetes** con `ray.toml`:

```toml
[package]
name = "hola"
version = "0.1.0"

[dependencies]
geo = "git+https://github.com/user/geo@v1.0.0"
util = "path:../util"
```

`ray build`/`run`/`test` resuelven y cachean las dependencias (lockfile `ray.lock` con hashes SHA-256).

## 11. Concurrencia

Modelo de **actores con aislamiento de heap**: fibras (`spawn`) que se comunican por **canales** tipados. No
hay estado mutable compartido; corre en multicore por defecto.

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

- `Channel.new()` / `Channel.bounded(n)` (acotado, backpressure) · `send` · `recv` · `close` · `select([chs])`.
- **Structured concurrency**: `scope(fn() -> R { … })` posee y une las tareas lanzadas dentro; `spawn` devuelve
  un `Task<T>` y `join(t)` espera su valor.
- Para **salida reproducible** (tests): `ray run --deterministic` (o `RAYLANG_THREADS=1`) fuerza un solo hilo
  con orden FIFO.

## 12. Herramientas

```sh
ray fmt archivo.ray      # formatea
ray test [archivo]       # corre las funciones @test
ray doc archivo.ray      # documentación Markdown desde ///
ray repl                 # REPL interactivo
ray lsp                  # servidor LSP (para tu editor)
```

Pruebas con `@test` (una función `() -> bool` o `() -> unit` que usa `assert`):

```rust
@test
fn suma_ok() -> bool { 2 + 2 == 4 }

@test
fn con_assert() {
    assert(1 < 2);
    assert_eq(cuadrado(3), 9);
}
```

`ray test` sale con el número de fallos como código.

## 13. Recomendaciones y mejores prácticas

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
- **Módulos: `pub` mínimo + cápsulas.** Expón solo la superficie que otros necesitan; usa `mod.ray` para que
  los internos de un paquete queden protegidos.
- **Concurrencia por canales, no por estado compartido.** El modelo de actores lo hace natural: pasa datos por
  un canal en vez de compartir una estructura mutable.
- **Prueba con `@test` + `assert_eq`.** Barato y va con el código; `ray test` en tu CI.
- **Formatea con `ray fmt`.** Un estilo único, sin discusiones.
- **Nombres sin sobrecarga.** raylang no tiene sobrecarga de funciones, así que la stdlib desambigua por
  nombre: p. ej. `index_of` (buscar en string) vs `position` (buscar en arreglo). Sigue esa convención.

## 14. Gotchas

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
- **Firmas explícitas siempre.** Los parámetros y el retorno de una función se anotan; no se infieren.
- **Un enum genérico "vacío" necesita contexto.** `Arbol.Hoja` (una variante sin payload de un enum genérico)
  no puede inferir el tipo solo → anótalo (`let t: Arbol<int> = …`) o dáselo el contexto.
- **Semántica de referencia** en arreglos/structs/`Map`: al asignarlos o pasarlos a una función, comparten el
  objeto (mutar uno muta el otro). Los primitivos (int/float/bool/char) se copian.
- **No hay `null`.** Usa `Option<T>` y `match`/`?`.
- **En una ruta calificada de módulo se usa el nombre del archivo (leaf)**, no la ruta completa: importas
  `geo/formas/circulo` y lo usas como `circulo.area(...)`.

---

¿Falta algo o quieres el detalle exacto de una construcción? Mira **`SPEC.md`** (normativa) o los **156
ejemplos** en `examples/`.
