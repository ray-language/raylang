# raylang in half an hour

[Español](getting-started.md) · English

The short guide: install, a project, the language in fifteen minutes, the tools and where to go
next. The details live in the [`MANUAL.md`](../MANUAL.md) (Spanish) and the signature catalog in
[`REFERENCE.en.md`](../REFERENCE.en.md); this is the shortest path to your first useful program.

## 1. Install

```sh
curl -sSfL https://raylang.dev/install.sh | sh        # macOS / Linux → ~/.local/bin/ray
```

```powershell
irm https://raylang.dev/install.ps1 | iex             # Windows (PowerShell)
```

Check and update:

```sh
ray version
ray upgrade --check        # 0 = up to date, 1 = a newer version exists
```

Without installing anything: the [playground](https://raylang.dev/playground/) runs the VM in the
browser, with diagnostics and completion.

## 2. A project

```sh
ray new hello && cd hello
ray run                    # runs src/main.ray on the VM
ray test                   # runs the @test functions
ray build --native         # produces a native binary, byte-identical to the VM
```

`ray new` leaves a `ray.toml` (name, version, dependencies) and `src/main.ray`:

```raylang
fn main() -> int {
    print("hello from hello");
    0
}
```

`main` returns an `int` (the exit code) or `unit`. A loose file works too: `ray run file.ray`, and
the arguments after the file arrive through `args()`.

## 3. The language in fifteen minutes

### Values, variables and functions

```raylang
fn square(x: int) -> int { x * x }          // the last value of a block is its result

fn sign(x: int) -> int {
    if (x > 0) { return 1; }                // `return` only to leave early
    if (x < 0) { return -1; }
    0
}

fn main() -> int {
    let x = 10;                             // immutable, inferred type
    var total = 0;                          // mutable
    total = total + square(x);
    let ratio: float = 2.5;                 // explicit annotation whenever you want one
    print("total ${total}, ratio ${ratio}, sign ${sign(-4)}");   // interpolation
    0
}
```

Everything is an **expression**: `if`, `match` and blocks produce a value. Function signatures are
always annotated; locals are inferred. There is no `null`.

### Structs, enums and `match`

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
    p.x = 5;                                // structs have reference semantics
    print(p.x + p.y);
    print(area(Shape.Rect(2.0, 3.0)));
    0
}
```

`match` is **exhaustive** (the compiler demands every variant be covered) and the scrutinee goes in
parentheses. On primitives (`int`, `string`) you use `if`/`else`, not `match`.

### Errors as values: `Option`, `Result` and `?`

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
    let q = divide(10, 2)?;                 // unwraps, or returns the Err to the caller
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

There are no exceptions: a function that can fail says so in its type, and `?` propagates the
failure upwards with a single keystroke.

### Arrays, maps and iterators

```raylang
fn main() -> int {
    var xs = [3, 1, 2];
    xs.push(4);
    print(xs.sort());                       // [1, 2, 3, 4] (sorted copy)
    print(xs.contains(2));

    var ages: Map<string, int> = Map.new();
    ages.insert("ada", 36);
    ages.insert("grace", 45);
    for (name, age) in ages {              // iterates in key order, deterministic
        print("${name}: ${age}");
    }

    let squares = xs.iter()
        .filter(fn(x: int) -> bool { x % 2 == 0 })
        .map(fn(x: int) -> int { x * x })
        .collect();                         // iterators are lazy until the terminal
    print(squares);
    print(range(1, 6).sum());              // 15
    0
}
```

`x.f(args)` is sugar for `f(x, args)` (UFCS), so any free function can be chained; `x |> f(a)` is
the same thing in pipeline form.

### Loops, `break` and `continue`

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
    for k in 0..3 { print(k); }             // half-open range
    0
}
```

### Traits and generics

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
    print(a == Version { major: 1, minor: 7 });   // derived Eq
    print(a);                                     // derived Show
    0
}
```

Traits dispatch statically (and dynamically with `dyn Trait`). `Eq`, `Show`, `Hash` and `ToJson`
can be derived; `Ord` and the operators (`Add`, `Sub`, …) are implemented by hand.

### Modules

A module is a file. `pub` exposes; you import by path and use it qualified by the last segment:

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
    let p = point.origin();                 // qualified
    let q = origin();                       // brought into scope
    print(p.x + q.y);
    0
}
```

The standard library is **embedded** in the binary and imported the same way: `import std/math;` →
`math.sqrt(2.0)`. The full catalog is in `REFERENCE.en.md` §10.

### Tests

```raylang
fn double(x: int) -> int { x * 2 }

@test
fn double_doubles() -> bool { double(21) == 42 }

@test
fn double_zero() { assert_eq(double(0), 0); }

fn main() -> int { 0 }
```

`ray test` runs the project's `@test` functions (also those in `tests/*.ray`); each one runs
isolated.

## 4. Concurrency in two minutes

Fibers with an **isolated heap** that talk through typed channels, on a multicore scheduler. There
is no shared mutable state: whatever a closure passed to `spawn` captures is **copied**; only
channels and handles are shared between fibers.

```raylang
fn main() -> int {
    let ch: Channel<int> = Channel.bounded(4);      // bounded: backpressure
    let producer = spawn(fn() {
        for i in 0..10 { send(ch, i * i); }
        close(ch);                                  // closing is the "end" signal
    });
    var total = 0;
    while (true) {
        match (recv(ch)) {
            Option.Some(v) => { total = total + v; },
            Option.None => { break; },              // channel closed and drained
        }
    }
    join(producer);
    print(total);                                   // 285
    0
}
```

`scope(fn() { … })` joins on exit every task spawned inside it (and if one fails, cancels its
siblings); `try_join` and `try_call` turn a failure into a `Result`; `try_send`/`try_recv` never
block; `select`/`select_timeout` wait on several channels. With `--deterministic` the scheduling is
reproducible.

## 5. A web server

The web framework is a tier-2 package (not in the binary): you declare it in `ray.toml` and import
it like any module.

```toml
[dependencies]
web = "path:../raylang/packages/web"      # or `ray add web` against the registry
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

`ray dev --port 8080` recompiles and restarts on save (without losing the socket) and reloads the
browser. Routes with parameters, middleware, typed JSON, static files, cookies and sessions: in
[`web-framework.md`](web-framework.md) (Spanish).

## 6. The tools

| Command | What for |
|---|---|
| `ray run` / `ray dev` | run on the VM / development mode with restart and live reload |
| `ray test` | the `@test` functions; `--watch` re-runs on save |
| `ray fmt --write src/` | canonical formatting (keeps your parentheses and comments) |
| `ray build --native --release` | optimized native binary; `ray toolchain install` if you have no Rust |
| `ray bundle` | desktop app (`.app`, `.desktop`, `.exe`) or an iOS/Android project |
| `ray doc src/main.ray` | documentation from `///` comments |
| `ray lsp` / `ray mcp` | editors (VSCode, Sublime, Zed, Neovim/Helix) / LLM agents |
| `ray add`, `ray search`, `ray registry publish` | dependencies and publishing |

## 7. Where to go next

- [`REFERENCE.en.md`](../REFERENCE.en.md): everything that exists, with signatures — the source
  whenever a function "should exist" (it almost always does).
- [`MANUAL.md`](../MANUAL.md): the long guide, with the idioms and the decisions explained
  (Spanish).
- [`examples/`](../examples/): more than 170 programs, from `fib` to a server with WebSockets.
- With an agent: paste [`llms.txt`](../llms.txt) into the prompt and connect `ray mcp`
  ([`mcp.en.md`](mcp.en.md)) so it verifies what it writes.

<!-- sync: sha256:b7d1a0fca00d -->
