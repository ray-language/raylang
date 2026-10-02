# Empezar

Español · [English](getting-started.en.md)

El camino más corto de cero a un programa útil: instalar, preparar el editor, crear un proyecto,
el lenguaje en quince minutos, la concurrencia, trabajar con un asistente LLM y las herramientas.
Al final sabrás qué guía seguir según lo que quieras construir.

## 1. Instalar

```sh
curl -sSfL https://raylang.dev/install.sh | sh        # macOS / Linux → ~/.local/bin/ray
```

```powershell
irm https://raylang.dev/install.ps1 | iex             # Windows (PowerShell)
```

Comprueba la instalación y busca versiones nuevas:

```sh
ray version
ray upgrade --check        # 0 = al día, 1 = hay versión nueva
ray upgrade                # instala la última
```

Sin instalar nada, el [playground](https://raylang.dev/playground/) corre el lenguaje en el
navegador, con diagnósticos y autocompletado.

`ray build --native` necesita una toolchain de Rust. Si no tienes una, `ray toolchain install`
instala una privada en `~/.ray/toolchain` sin tocar tu sistema.

## 2. El editor

Todas las extensiones hablan con el mismo servidor de lenguaje, `ray lsp`: diagnósticos al
escribir, autocompletado, hover con la firma, ir a la definición, renombrar y formatear.

| Editor | Cómo |
|---|---|
| VS Code | la extensión `raylang` del marketplace |
| Sublime Text | `Package Control: Install Package → raylang` |
| Zed | la extensión `raylang` |
| Neovim, Helix | apuntan a `ray lsp`; los snippets de configuración están en [editors/README.md](../editors/README.md) |

## 3. Un proyecto

```sh
ray new hola && cd hola
ray run                    # ejecuta src/main.ray en la VM
```

`ray new` deja tres archivos: `ray.toml`, `src/main.ray` y un `.gitignore`.

```toml
[package]
name = "hola"
version = "0.1.0"

[dependencies]
```

```rust
fn main() -> int {
    print("hello from hola");
    0
}
```

`main` devuelve un `int`, que es el código de salida del proceso, o nada. Un archivo suelto también
vale: `ray run archivo.ray`, y los argumentos que van detrás del archivo llegan por `args()`.

El ciclo de trabajo tiene cuatro comandos:

```sh
ray dev                    # recompila y reinicia al guardar
ray test                   # corre las funciones @test
ray fmt --write src/       # formato canónico
ray build --native         # binario nativo, con la misma salida que la VM
```

**Dos motores, un comportamiento.** Mientras desarrollas, el programa corre en la VM: arranca al
instante y `ray dev` lo reinicia en cada cambio. Para desplegar, `ray build --native` lo traduce a
Rust y lo compila a código máquina. Los dos producen exactamente la misma salida, byte a byte, y el
CI del lenguaje lo comprueba en cada cambio. El binario nativo es varias veces más rápido; las
cifras están en la [página de benchmarks](https://raylang.dev/bench.html).

### Dependencias

La biblioteca estándar va dentro del binario `ray` y se importa con `import std/…`. Lo demás son
paquetes que se declaran en `ray.toml`:

```sh
ray search http            # busca en el índice público
ray add web                # añade la dependencia a ray.toml y la descarga
```

Los paquetes oficiales son `net` (HTTP/1.1 y 2, WebSocket, DNS, TLS, gRPC), `web` (el framework de
aplicación al estilo Express), `rpc`, `db` (Postgres, MySQL, SQLite, Redis, MongoDB), `tz` y `cron`.
Las versiones quedan fijadas en `ray.lock` con su hash.

## 4. El lenguaje en quince minutos

### Valores, variables y funciones

```rust
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

```rust
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

```rust
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

```rust
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

```rust
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

```rust
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

<!-- check: skip (dos archivos de un mismo proyecto) -->
```rust
// src/geo/point.ray
pub struct Point { x: int, y: int }
pub fn origin() -> Point { Point { x: 0, y: 0 } }
```

<!-- check: skip (dos archivos de un mismo proyecto) -->
```rust
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
`math.sqrt(2.0)`. El catálogo completo está en la
[referencia](../REFERENCE.md#10-la-biblioteca-estándar-std).

### Tests

```rust
fn double(x: int) -> int { x * 2 }

@test
fn double_doubles() -> bool { double(21) == 42 }

@test
fn double_zero() { assert_eq(double(0), 0); }

fn main() -> int { 0 }
```

`ray test` corre los `@test` del proyecto (también los de `tests/*.ray`); cada uno corre aislado.

## 5. Concurrencia en dos minutos

Fibras con **heap aislado** que se comunican por canales tipados, sobre un scheduler multicore. No
hay estado mutable compartido: lo que captura una closure pasada a `spawn` se **copia**; entre fibras
solo se comparten canales y handles.

```rust
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

## 6. Trabajar con un asistente LLM

raylang trae dos piezas para que un asistente de código escriba código que compila:

- **[`llms.txt`](../llms.txt)** es el contexto destilado del lenguaje: en qué se diferencia de
  Rust, las formas canónicas y los mensajes de error exactos. Pégalo en el `CLAUDE.md` del proyecto
  o en el prompt.
- **`ray mcp`** es un servidor [MCP](https://modelcontextprotocol.io) que le da al asistente las
  herramientas `ray_check`, `ray_run`, `ray_test`, `ray_fmt` y `ray_doc`. El asistente escribe,
  compila, lee el diagnóstico exacto y corrige, sin que tengas que copiar errores a mano.

Con Claude Code se conecta así:

```sh
claude mcp add raylang -- ray mcp
```

En un proyecto, el asistente debe pasar **`path`** (el archivo o el directorio del proyecto) a las
herramientas, no el código suelto: así los imports entre archivos y las dependencias de `ray.toml`
resuelven igual que con `ray run`. Las instrucciones del propio servidor se lo indican. El
detalle está en [docs/mcp.md](../docs/mcp.md).

## 7. Las herramientas

| Comando | Para qué |
|---|---|
| `ray run` / `ray dev` | ejecutar en la VM / modo desarrollo con reinicio y recarga del navegador |
| `ray test` | las funciones `@test`; `--watch` vuelve a correr al guardar |
| `ray fmt --write src/` | formato canónico (conserva tus paréntesis y comentarios) |
| `ray build --native --release` | binario nativo optimizado |
| `ray bundle` | app de escritorio (`.app`, `.desktop`, `.exe`) o proyecto iOS (`--ios`) y Android (`--android`) |
| `ray dev --device` | recarga en caliente del programa en el teléfono |
| `ray doc src/main.ray` | documentación a partir de los comentarios `///` |
| `ray profile` | dónde se va el tiempo de un programa |
| `ray serve _site` | sirve un directorio estático para previsualizar |
| `ray lsp` / `ray mcp` | el editor / los asistentes LLM |
| `ray add`, `ray search`, `ray registry publish` | dependencias y publicación |

`ray help` lista todo, y `ray <comando> --help` explica cada uno.

## 8. Qué construir

Desde aquí, cada guía del handbook es un proyecto completo, con su app de ejemplo:

- una [**app móvil**](mobile.md) para iOS y Android con frontend React;
- la misma app en [**escritorio**](cross-platform.md) (macOS, Linux, Windows), y las
  [**ventanas a fondo**](windows.md);
- un [**sitio con plantillas**](ssr.md) renderizado en el servidor;
- una [**API web**](api.md) con el framework `web` y Postgres;
- un [**sitio con frontend React**](web-react.md) embebido en el binario;
- una [**herramienta de terminal**](cli.md);
- un [**agente LLM**](llm-mcp.md) con un cliente **MCP**, escrito en raylang.

Cuando funcione, [**rendimiento**](performance.md) enseña a medirlo y a hacerlo rápido.

Y para llevar todo eso a sus usuarios: [**distribuir**](shipping.md), con firma, tiendas
móviles y actualizaciones automáticas.

Para todo lo demás: la [referencia](../REFERENCE.md) tiene cada función con su firma, y el
[manual](../MANUAL.md) explica el lenguaje en detalle.
