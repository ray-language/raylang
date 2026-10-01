# Getting started

[Español](empezar.md) · English

The shortest path from zero to a useful program: install, set up the editor, create a project, the
language in fifteen minutes, concurrency, working with an LLM assistant and the tools. At the end
you will know which guide to follow for what you want to build.

## 1. Install

```sh
curl -sSfL https://raylang.dev/install.sh | sh        # macOS / Linux → ~/.local/bin/ray
```

```powershell
irm https://raylang.dev/install.ps1 | iex             # Windows (PowerShell)
```

Check the installation and look for new versions:

```sh
ray version
ray upgrade --check        # 0 = up to date, 1 = a new version exists
ray upgrade                # installs the latest
```

Without installing anything, the [playground](https://raylang.dev/playground/) runs the language
in the browser, with diagnostics and completion.

`ray build --native` needs a Rust toolchain. If you have none, `ray toolchain install` installs a
private one under `~/.ray/toolchain` without touching your system.

## 2. The editor

Every extension talks to the same language server, `ray lsp`: diagnostics as you type, completion,
hover with the signature, go to definition, rename and formatting.

| Editor | How |
|---|---|
| VS Code | the `raylang` extension from the marketplace |
| Sublime Text | `Package Control: Install Package → raylang` |
| Zed | the `raylang` extension |
| Neovim, Helix | point at `ray lsp`; the configuration snippets are in [editors/README.md](../editors/README.md) |

## 3. A project

```sh
ray new hello && cd hello
ray run                    # runs src/main.ray on the VM
```

`ray new` leaves three files: `ray.toml`, `src/main.ray` and a `.gitignore`.

```toml
[package]
name = "hello"
version = "0.1.0"

[dependencies]
```

```rust
fn main() -> int {
    print("hello from hello");
    0
}
```

`main` returns an `int`, the process exit code, or nothing. A single file works too:
`ray run file.ray`, and the arguments after the file arrive through `args()`.

The working loop has four commands:

```sh
ray dev                    # rebuilds and restarts on save
ray test                   # runs the @test functions
ray fmt --write src/       # canonical formatting
ray build --native         # native binary, with the same output as the VM
```

**Two engines, one behavior.** While you develop, the program runs on the VM: it starts instantly
and `ray dev` restarts it on every change. To deploy, `ray build --native` translates it to Rust
and compiles it to machine code. Both produce exactly the same output, byte for byte, and the
language's CI checks it on every change. The native binary is several times faster; the figures
are on the [benchmarks page](https://raylang.dev/en/bench.html).

### Dependencies

The standard library ships inside the `ray` binary and is imported with `import std/…`. Everything
else is a package declared in `ray.toml`:

```sh
ray search http            # searches the public index
ray add web                # adds the dependency to ray.toml and downloads it
```

The official packages are `net` (HTTP/1.1 and 2, WebSocket, DNS, TLS, gRPC), `web` (the
Express-style application framework), `rpc`, `db` (Postgres, MySQL, SQLite, Redis, MongoDB), `tz`
and `cron`. Versions are pinned in `ray.lock` with their hash.

## 4. The language in fifteen minutes

### Values, variables and functions

```rust
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
    p.x = 5;                                // structs have reference semantics
    print(p.x + p.y);
    print(area(Shape.Rect(2.0, 3.0)));
    0
}
```

`match` is **exhaustive** (the compiler demands every variant be covered) and the scrutinee goes in
parentheses. On primitives (`int`, `string`) you use `if`/`else`, not `match`.

### Errors as values: `Option`, `Result` and `?`

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

```rust
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
    for k in 0..3 { print(k); }             // half-open range
    0
}
```

### Traits and generics

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
    print(a == Version { major: 1, minor: 7 });   // derived Eq
    print(a);                                     // derived Show
    0
}
```

Traits dispatch statically (and dynamically with `dyn Trait`). `Eq`, `Show`, `Hash` and `ToJson`
can be derived; `Ord` and the operators (`Add`, `Sub`, …) are implemented by hand.

### Modules

A module is a file. `pub` exposes; you import by path and use it qualified by the last segment:

<!-- check: skip (two files of one project) -->
```rust
// src/geo/point.ray
pub struct Point { x: int, y: int }
pub fn origin() -> Point { Point { x: 0, y: 0 } }
```

<!-- check: skip (two files of one project) -->
```rust
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
`math.sqrt(2.0)`. The full catalog is in the
[reference](../REFERENCE.en.md#10-the-standard-library-std).

### Tests

```rust
fn double(x: int) -> int { x * 2 }

@test
fn double_doubles() -> bool { double(21) == 42 }

@test
fn double_zero() { assert_eq(double(0), 0); }

fn main() -> int { 0 }
```

`ray test` runs the project's `@test` functions (also those in `tests/*.ray`); each one runs
isolated.

## 5. Concurrency in two minutes

Fibers with an **isolated heap** that talk through typed channels, on a multicore scheduler. There
is no shared mutable state: whatever a closure passed to `spawn` captures is **copied**; only
channels and handles are shared between fibers.

```rust
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

## 6. Working with an LLM assistant

raylang ships two pieces so that a coding assistant writes code that compiles:

- **[`llms.txt`](../llms.txt)** is the distilled context of the language: how it differs from
  Rust, the canonical forms and the exact error messages. Paste it into the project's `CLAUDE.md`
  or the prompt.
- **`ray mcp`** is an [MCP](https://modelcontextprotocol.io) server that gives the assistant the
  `ray_check`, `ray_run`, `ray_test`, `ray_fmt` and `ray_doc` tools. The assistant writes,
  compiles, reads the exact diagnostic and fixes it, without you copying errors by hand.

With Claude Code you connect it like this:

```sh
claude mcp add raylang -- ray mcp
```

In a project, the assistant should pass **`path`** (the file or the project directory) to the
tools, not loose code: that way imports across files and the `ray.toml` dependencies resolve
exactly as with `ray run`. The server's own instructions tell it so. The details are in
[docs/mcp.en.md](../docs/mcp.en.md).

## 7. The tools

| Command | What for |
|---|---|
| `ray run` / `ray dev` | run on the VM / development mode with restart and browser reload |
| `ray test` | the `@test` functions; `--watch` re-runs on save |
| `ray fmt --write src/` | canonical formatting (keeps your parentheses and comments) |
| `ray build --native --release` | optimized native binary |
| `ray bundle` | desktop app (`.app`, `.desktop`, `.exe`) or an iOS (`--ios`) and Android (`--android`) project |
| `ray dev --device` | hot reload of the program on the phone |
| `ray doc src/main.ray` | documentation from the `///` comments |
| `ray profile` | where a program spends its time |
| `ray serve _site` | serves a static directory for previews |
| `ray lsp` / `ray mcp` | the editor / LLM assistants |
| `ray add`, `ray search`, `ray registry publish` | dependencies and publishing |

`ray help` lists everything, and `ray <command> --help` explains each one.

## 8. What to build

From here, each handbook guide is a complete project. They are on the way, and the
[overview](index.en.md) links each one as soon as it is published:

- a [**mobile app**](movil.en.md) for iOS and Android with a React frontend;
- the same app on the [**desktop**](multiplataforma.en.md) (macOS, Linux, Windows);
- a **web API** with the `web` framework;
- a **server-rendered site** with templates;
- a **site with a React frontend** embedded in the binary;
- an **LLM agent** and an **MCP** server written in raylang;
- how to **ship** all of the above: packaging, signing and auto-updating.

For everything else: the [reference](../REFERENCE.en.md) has every function with its signature,
and the [manual](../MANUAL.md) (Spanish) explains the language in depth.

<!-- sync: sha256:b24f7bf5d49a -->
