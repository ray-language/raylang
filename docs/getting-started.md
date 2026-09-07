# raylang en media hora

Español · [English](getting-started.en.md)

La guía corta: instalar, un proyecto, el lenguaje en quince minutos, las herramientas y por dónde
seguir. El detalle vive en el [`MANUAL.md`](../MANUAL.md) y el catálogo de firmas en
[`REFERENCE.md`](../REFERENCE.md); esto es el camino más corto para escribir el primer programa
útil.

## 1. Instalar

```sh
curl -sSfL https://raylang.dev/install.sh | sh        # macOS / Linux → ~/.local/bin/ray
```

```powershell
irm https://raylang.dev/install.ps1 | iex             # Windows (PowerShell)
```

Comprueba y actualiza:

```sh
ray version
ray upgrade --check        # 0 = al día, 1 = hay versión nueva
```

Sin instalar nada: el [playground](https://raylang.dev/playground/) corre la VM en el navegador,
con diagnósticos y autocompletado.

## 2. Un proyecto

```sh
ray new hola && cd hola
ray run                    # ejecuta src/main.ray en la VM
ray test                   # corre las funciones @test
ray build --native         # produce un binario nativo, byte-idéntico a la VM
```

`ray new` deja un `ray.toml` (nombre, versión, dependencias) y `src/main.ray`:

```raylang
fn main() -> int {
    print("hello from hola");
    0
}
```

`main` devuelve un `int` (el código de salida) o `unit`. Un archivo suelto también vale:
`ray run archivo.ray`, y los argumentos tras el archivo llegan por `args()`.

## 3. El lenguaje en quince minutos

### Valores, variables y funciones

```raylang
fn square(x: int) -> int { x * x }          // el último valor del bloque es el resultado

fn sign(x: int) -> int {
    if (x > 0) { return 1; }                // `return` solo para salir antes
    if (x < 0) { return -1; }
    0
}

fn main() -> int {
    let x = 10;                             // inmutable, tipo inferido
    var total = 0;                          // mutable
    total = total + square(x);
    let ratio: float = 2.5;                 // anotación explícita cuando quieras
    print("total ${total}, ratio ${ratio}, sign ${sign(-4)}");   // interpolación
    0
}
```

Todo es una **expresión**: `if`, `match` y los bloques producen valor. Las firmas de función se
anotan siempre; los locales se infieren. No hay `null`.

### Structs, enums y `match`

```raylang
struct Point { x: int, y: int }

enum Shape {
    Circle(float),
    Rect(float, float),
    Dot,
}

fn area(s: Shape) -> float {
    match (s) {
        Shape.Circle(r) => 3.14159 * r * r,
        Shape.Rect(w, h) => w * h,
        Shape.Dot => 0.0,
    }
}

fn main() -> int {
    let p = Point { x: 1, y: 2 };
    p.x = 5;                                // los structs tienen semántica de referencia
    print(p.x + p.y);
    print(area(Shape.Rect(2.0, 3.0)));
    0
}
```

El `match` es **exhaustivo** (el compilador exige cubrir todas las variantes) y el escrutinio va entre
paréntesis. Sobre primitivos (`int`, `string`) se usa `if`/`else`, no `match`.

### Errores como valores: `Option`, `Result` y `?`

```raylang
fn divide(a: int, b: int) -> Result<int, string> {
    if (b == 0) { Result.Err("division by zero") } else { Result.Ok(a / b) }
}

fn average(xs: [int]) -> Option<int> {
    if (xs.len() == 0) { return Option.None; }
    var sum = 0;
    for x in xs { sum = sum + x; }
    Option.Some(sum / xs.len())
}

fn compute() -> Result<int, string> {
    let q = divide(10, 2)?;                 // desempaqueta o devuelve el Err al llamador
    Result.Ok(q + 1)
}

fn main() -> int {
    match (compute()) {
        Result.Ok(v) => print("ok ${v}"),
        Result.Err(e) => print("error: " + e),
    }
    match (average([3, 4, 5])) {
        Option.Some(m) => print(m),
        Option.None => print("empty"),
    }
    0
}
```

No hay excepciones: una función que puede fallar lo dice en su tipo, y `?` propaga el fallo hacia
arriba con una sola tecla.

### Arreglos, mapas e iteradores

```raylang
fn main() -> int {
    var xs = [3, 1, 2];
    xs.push(4);
    print(xs.sort());                       // [1, 2, 3, 4] (copia ordenada)
    print(xs.contains(2));

    var ages: Map<string, int> = Map.new();
    ages.insert("ada", 36);
    ages.insert("grace", 45);
    for (name, age) in ages {              // recorrido en orden de clave, determinista
        print("${name}: ${age}");
    }

    let squares = xs.iter()
        .filter(fn(x: int) -> bool { x % 2 == 0 })
        .map(fn(x: int) -> int { x * x })
        .collect();                         // los iteradores son perezosos hasta el terminal
    print(squares);
    print(range(1, 6).sum());              // 15
    0
}
```

`x.f(args)` es azúcar para `f(x, args)` (UFCS), así que cualquier función libre se puede encadenar;
`x |> f(a)` es lo mismo en forma de tubería.

### Bucles, `break` y `continue`

```raylang
fn main() -> int {
    var i = 0;
    var odd_sum = 0;
    while (true) {
        i = i + 1;
        if (i % 2 == 0) { continue; }
        if (i > 9) { break; }
        odd_sum = odd_sum + i;
    }
    print(odd_sum);                         // 1+3+5+7+9 = 25
    for k in 0..3 { print(k); }             // rango semiabierto
    0
}
```

### Traits y genéricos

```raylang
trait Describe { fn describe(self) -> string; }

struct User { name: string, age: int }

impl Describe for User {
    fn describe(self) -> string { "${self.name} (${self.age})" }
}

fn show_all<T: Describe>(items: [T]) {
    for it in items { print(it.describe()); }
}

@derive(Eq, Show)
struct Version { major: int, minor: int }

fn main() -> int {
    show_all([User { name: "Ada", age: 36 }, User { name: "Grace", age: 45 }]);
    let a = Version { major: 1, minor: 7 };
    print(a == Version { major: 1, minor: 7 });   // Eq derivado
    print(a);                                     // Show derivado
    0
}
```

Los traits despachan estáticamente (con `dyn Trait` también dinámicamente). `Eq`, `Show`, `Hash` y
`ToJson` se derivan; `Ord` y los operadores (`Add`, `Sub`, …) se implementan a mano.

### Módulos

Un módulo es un archivo. `pub` expone; se importa por ruta y se usa calificado por el último
segmento:

```raylang
// src/geo/point.ray
pub struct Point { x: int, y: int }
pub fn origin() -> Point { Point { x: 0, y: 0 } }
```

```raylang
// src/main.ray
import geo/point;
from geo/point import origin;

fn main() -> int {
    let p = point.origin();                 // calificado
    let q = origin();                       // traído al ámbito
    print(p.x + q.y);
    0
}
```

La biblioteca estándar va **embebida** en el binario y se importa igual: `import std/math;` →
`math.sqrt(2.0)`. El catálogo completo está en `REFERENCE.md` §10.

### Tests

```raylang
fn double(x: int) -> int { x * 2 }

@test
fn double_doubles() -> bool { double(21) == 42 }

@test
fn double_zero() { assert_eq(double(0), 0); }

fn main() -> int { 0 }
```

`ray test` corre los `@test` del proyecto (también los de `tests/*.ray`); cada uno corre aislado.

## 4. Concurrencia en dos minutos

Fibras con **heap aislado** que se comunican por canales tipados, sobre un scheduler multicore. No
hay estado mutable compartido: lo que captura una closure pasada a `spawn` se **copia**; entre fibras
solo se comparten canales y handles.

```raylang
fn main() -> int {
    let ch: Channel<int> = Channel.bounded(4);      // acotado: contrapresión
    let producer = spawn(fn() {
        for i in 0..10 { send(ch, i * i); }
        close(ch);                                  // cerrar es la señal de "fin"
    });
    var total = 0;
    while (true) {
        match (recv(ch)) {
            Option.Some(v) => { total = total + v; },
            Option.None => { break; },              // canal cerrado y drenado
        }
    }
    join(producer);
    print(total);                                   // 285
    0
}
```

`scope(fn() { … })` une al salir todas las tareas lanzadas dentro (y si una falla, cancela a las
hermanas); `try_join` y `try_call` convierten un fallo en `Result`; `try_send`/`try_recv` no
bloquean; `select`/`select_timeout` esperan a varios canales. Con `--deterministic` la planificación
es reproducible.

## 5. Un servidor web

El framework web es un paquete de nivel 2 (no va en el binario): se declara en `ray.toml` y se
importa como cualquier módulo.

```toml
[dependencies]
web = "path:../raylang/packages/web"      # o `ray add web` contra el registro
net = "path:../raylang/packages/net"
```

```raylang
from web/framework import new_app, GET, listen, text, param, App, Ctx, Res;

fn build_app() -> App {
    var app = new_app();
    app.GET("/", fn(c: Ctx, r: Res) { r.text("hello"); });
    app.GET("/hi/:name", fn(c: Ctx, r: Res) { r.text("hi " + c.param("name")); });
    app
}

fn main() -> int {
    match (listen(build_app, "127.0.0.1", 8080)) {
        Result.Ok(_) => 0,
        Result.Err(e) => { eprint(e); 1 },
    }
}
```

`ray dev --port 8080` recompila y reinicia al guardar (sin perder el socket) y recarga el navegador.
Rutas con parámetros, middleware, JSON tipado, estáticos, cookies y sesiones: en
[`web-framework.md`](web-framework.md).

## 6. Las herramientas

| Comando | Para qué |
|---|---|
| `ray run` / `ray dev` | ejecutar en la VM / modo desarrollo con reinicio y live-reload |
| `ray test` | los `@test`; `--watch` re-corre al guardar |
| `ray fmt --write src/` | formato canónico (conserva tus paréntesis y comentarios) |
| `ray build --native --release` | binario nativo optimizado; `ray toolchain install` si no tienes Rust |
| `ray bundle` | app de escritorio (`.app`, `.desktop`, `.exe`) o proyecto iOS/Android |
| `ray doc src/main.ray` | documentación desde los comentarios `///` |
| `ray lsp` / `ray mcp` | editor (VSCode, Sublime, Zed, Neovim/Helix) / agentes LLM |
| `ray add`, `ray search`, `ray registry publish` | dependencias y publicación |

## 7. Por dónde seguir

- [`REFERENCE.md`](../REFERENCE.md): todo lo que existe, con firmas — es la fuente cuando una función
  "debería existir" (casi siempre existe).
- [`MANUAL.md`](../MANUAL.md): la guía larga, con los idiomas y las decisiones explicadas.
- [`examples/`](../examples/): más de 170 programas, de `fib` a un servidor con WebSockets.
- Con un agente: pega [`llms.txt`](../llms.txt) en el prompt y conecta `ray mcp`
  ([`mcp.md`](mcp.md)) para que verifique lo que escribe.
