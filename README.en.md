<div align="center">

<!-- PNG rather than SVG: GitHub's mobile app does not render SVGs from the repo (even less in private ones) -->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/raylang-lockup-horizontal-dark.png">
  <img src="assets/raylang-lockup-horizontal.png" alt="raylang" width="380">
</picture>

**A statically typed, expression-oriented, self-hosting programming language — written in Rust, with a minimal and deliberate dependency surface.**

[![CI](https://github.com/ray-language/raylang/actions/workflows/ci.yml/badge.svg)](https://github.com/ray-language/raylang/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Version](https://img.shields.io/github/v/release/ray-language/raylang?label=version&color=brightgreen)](https://github.com/ray-language/raylang/releases)

**[raylang.dev](https://raylang.dev)** · [Install](#installation) · [A glimpse](#a-glimpse-of-the-language) · [Playground](https://raylang.dev/playground/) · [Documentation](#documentation) · [Highlights](#highlights)

[Español](README.md) · English

</div>

---

**raylang** is a language aimed at **real production**: generics, traits, pattern matching, multicore
actor concurrency, a tooling ecosystem and **three engines that agree byte for byte** — a bytecode VM
for development, a **native binary** for deployment and an interpreter as a validation oracle. It also
**compiles itself** (self-hosting) and runs **in the browser** via WebAssembly.

The host is **Rust**. The dependency policy is *minimal and deliberate*: a dependency comes in only when
hand-writing it would be worse engineering (TLS/`rustls`, crypto/`ring`, SQLite/`rusqlite`, the fiber
context switch) or when the improvement is **measured** (`mimalloc`, `ahash`). Everything else — HTTP/1.1
and HTTP/2, HPACK, JSON, TOML, DNS, WebSocket, protobuf, the database clients, the LSP, the I/O poller —
is written in raylang or in the project's own Rust. Details in
[`SECURITY.md`](SECURITY.md#política-de-dependencias) (in Spanish).

```rust
enum Tree { Leaf, Node(Tree, int, Tree) }

fn sum(t: Tree) -> int {
    match (t) {
        Tree.Leaf => 0,
        Tree.Node(left, v, right) => sum(left) + v + sum(right),
    }
}

fn main() -> int {
    let t = Tree.Node(Tree.Node(Tree.Leaf, 1, Tree.Leaf), 2, Tree.Node(Tree.Leaf, 3, Tree.Leaf));
    print("tree sum: ${t.sum()}");           // UFCS + string interpolation  → 6

    let s = [1, 2, 3, 4, 5]
        .iter()
        .map(fn(x: int) -> int { x * x })
        .sum();                              // lazy iterators + closure  → 55
    print("sum of squares: ${s}");
    0
}
```

## Why look at it

- **No `null` by design.** Errors are values: `Option<T>`/`Result<T,E>` plus the `?` operator.
- **Expression-oriented.** `if`, blocks and `match` produce values; implicit return.
- **A rich type system.** Generics with inference, **traits** (static dispatch, *bounds*, generic
  impls, default methods, `dyn A + B`), sum types and exhaustive **pattern matching** (with guards,
  `if let`, nested patterns).
- **Modern ergonomics.** UFCS (`x.f()`), pipelines (`x |> f()`), closures, `break`/`continue`, and lazy
  iterators (`map`/`filter`/`take`/`zip`/`fold`/`collect`/…).
- **Real concurrency.** An **actor model with heap isolation** plus typed channels, on an **M:N
  multicore** scheduler with *data-race freedom* by construction.
- **Production web.** An **Express-style framework** (`web/framework`: routes with parameters,
  middleware, CORS, static files with ETag, cookies, typed JSON via `ToJson`) on a concurrent HTTP/1.1
  server with keep-alive, TLS and graceful shutdown. The native binary runs on **M:N fibers** (Jul 2026):
  the framework peaks at **~188k req/s — 93% of axum, with p50/p99.9 tied (0.48/1.05 ms vs
  0.47/1.04) and 1.5× Go+chi** (`json` tier, dedicated load generator), serving with **14 threads and
  ~21 KB per connection**. Guide: [`docs/web-framework.md`](docs/web-framework.md).
- **OS processes without surprises.** `std/process` launches commands with a **typed argv, no shell**
  (`run`, a builder with deadline and caps, and *streaming* through bounded channels with backpressure).
  The child gets its own process group and is a **scope child**: nothing is left orphaned.
- **Self-hosted.** raylang's lexer, parser, checker, interpreter and VM are written **in raylang**.
- **Compiles to a native binary.** `ray build --native` transpiles the program to Rust and compiles it to
  an executable with byte-identical parity (*dev = VM / deploy = native*). On the 14-program polyglot
  bench (29 Jul 2026, M3 Pro) it **beats node in 9 of the 10 compute programs** (1.1×–20×), **Go in
  six** and **`rustc -O` in four** (tying both in two more), and starts in **1.80 ms — the fastest on
  the table**. In time×memory it ranks **#1 or #2 in 11 of the 12 programs** against 9 languages.
  Against its own VM: 3–4× on service workloads and 28–57× on pure compute. Tables:
  [`benchmarks/poly/README.md`](benchmarks/poly/README.md).
- **Runs in the browser.** The VM compiled to WebAssembly, without `wasm-bindgen`.

## Installation

### `curl | sh` (macOS / Linux)

```sh
curl -sSfL https://raylang.dev/install.sh | sh
```

Installs the `ray` binary (and its `raylang` alias) into `~/.local/bin`.

### PowerShell (Windows)

```powershell
irm https://raylang.dev/install.ps1 | iex
```

Places `ray.exe` (and `raylang.exe`) in `%LOCALAPPDATA%\Programs\raylang\bin` and adds it to the user
PATH (no administrator needed; open a new terminal). Same variables as the `.sh` (`RAYLANG_VERSION`,
`RAYLANG_BIN_DIR`, …). x86_64 and ARM64 binaries. What works and what does not on Windows:
[`PRODUCTION.md`](PRODUCTION.md#windows) and [`docs/windows.md`](docs/windows.md) (in Spanish).

To update to the latest version (or check whether there is one):

```sh
ray upgrade            # downloads the latest release and replaces the installed binaries
ray upgrade --check    # only reports (0 = up to date, 1 = a newer version exists)
ray toolchain install  # a private Rust for `ray build --native` on a machine without Rust (+ vendor: first build offline)
```

### From source

```sh
git clone https://github.com/ray-language/raylang
cd raylang
cargo build --release          # target/release/ray
```

> Rust is installed via [rustup](https://rustup.rs/). For a minimal binary (no TLS/crypto, SQLite or
> native code loading): `cargo build --release --no-default-features --features interp`. See
> [`docs/build.md`](docs/build.md).

## Usage

```sh
ray new hello          # creates a project (ray.toml + src/main.ray)
cd hello
ray run                # runs src/main.ray on the VM
ray dev                # development mode: rebuilds and restarts on changes (+ browser live-reload)
ray build              # checks and compiles without running
ray build --native     # transpiles to Rust and builds a native binary (3–57× the VM, depending on the load)
ray test               # runs the @test functions
ray fmt src/main.ray   # formats
ray doc src/main.ray   # generates documentation from ///
ray build --templates-only views/       # compiles .ray.html templates into typed raylang functions (SSR)
ray repl               # interactive REPL
ray lsp                # LSP server (diagnostics, hover, definition, refs, rename, completion, formatting, symbols…)
ray mcp                # MCP server for LLM agents (check/run/test/fmt/doc, with the code sandboxed)
```

**Package manager** (`ray.toml` manifest + `ray.lock` lockfile with SHA-256 hashes):

```sh
ray add textutils@^1.2 # adds a dependency from the registry and downloads it
ray remove textutils   # removes it (and its cache if nobody else uses it)
ray search json        # searches the registry
ray fetch              # downloads what ray.toml declares into .ray-deps/
ray update             # re-resolves to the newest compatible versions
ray registry publish            # publishes YOUR package (validates + checks + hashes; --sign signs it)
```

**Development mode** (`ray dev`): watches the sources, rebuilds in milliseconds and **only restarts when
the change compiles** (a half-written error does not take down the server that works). With
`--port 8080` the supervisor **holds the socket** across restarts (zero rejected connections) and
injects **live-reload** into the browser. Details in [`MANUAL.md`](MANUAL.md#17-herramientas) (in Spanish).

The exit code of `ray run` is the `int` that `main` returns (0 if it is `unit`). To embed raylang
sandboxed: `ray run --fuel N` (instruction limit) and `--heap N` (object cap). Reproducible concurrency:
`--deterministic`.

**Native deploy** (*dev = VM / deploy = native*, like Rust): `ray build --native prog.ray` produces a
machine-code executable, byte-identical to the VM, whose concurrency runs on an **M:N fiber**
scheduler. The subsystems backed by production crates (TLS, cryptography, SQLite, accelerated regex)
are linked **only when the program uses them**; `--release` raises the optimization tier, `--target`
cross-compiles and `--without crypto,tls,sqlite,regex,mimalloc,ahash,fibers,process` (or
`[native] without = [...]` in `ray.toml`) leaves out what you do not want inside — dropping
`mimalloc,ahash,fibers` goes back to the bare `rustc` fast path. See
[`docs/transpilador-nativo.md`](docs/transpilador-nativo.md) (in Spanish).

## A glimpse of the language

**Traits + generics:**

```rust
trait Showable { fn show(self) -> string; }

struct Point { x: int, y: int }
impl Showable for Point {
    fn show(self) -> string { "(${self.x}, ${self.y})" }   // string interpolation
}

fn display<T: Showable>(v: T) { print(v.show()); }
```

**Errors as values + `?`:**

```rust
fn divide(a: int, b: int) -> Result<int, string> {
    if (b == 0) { Result.Err("division by zero") } else { Result.Ok(a / b) }
}

fn calc() -> Result<int, string> {
    let x = divide(10, 2)?;    // unwraps or returns the Err
    Result.Ok(x + 1)
}
```

**Concurrency (actors + channels):**

```rust
fn main() -> int {
    let ch: Channel<int> = Channel.new();
    spawn(fn() { var i = 0; while (i < 5) { send(ch, i * i); i = i + 1; } close(ch); });
    var total = 0;
    while (true) {
        match (recv(ch)) {
            Option.Some(v) => { total = total + v; },
            Option.None => { break; },      // the channel was closed: done
        }
    }
    print("total: ${total}");   // 0+1+4+9+16 = 30
    0
}
```

There are **more than 170 examples** in [`examples/`](examples/): from `fib`/`fizzbuzz` to trait objects,
structured concurrency, a web server, WebSockets, and the self-hosted compiler itself in
[`selfhost/`](selfhost/).

## Web playground

**Try it right now, without installing anything: [raylang.dev/playground](https://raylang.dev/playground/)** —
a real editor (CodeMirror) with raylang's LSP running inside the wasm: diagnostics, completion, hover
and formatting in the browser.

To run it locally, raylang runs in the browser (the VM compiled to `wasm32`, **zero `wasm-bindgen`**):

```sh
./playground/build.sh
ray serve playground                            # → http://127.0.0.1:8000
```

It covers the core language (the whole language + prelude + pure stdlib). See [`playground/`](playground/).

## Highlights

- **Self-hosting + meta-circularity.** raylang lexes/parses/checks/runs raylang, with the Rust toolchain
  as the oracle. The self-hosted compiler runs itself on the self-hosted interpreter and VM.
- **Multicore via actors.** A heap per fiber + ownership transfer on `send` → *data-race freedom* without
  ownership in the type system. An M:N scheduler with measured real speedup; `--deterministic` for tests.
- **Three engines that agree.** A VM↔interpreter oracle guards every runtime change, and the native
  binary (`ray build --native`) verifies **byte-identical output to the VM**.
- **Counted and justified dependencies.** The network and format stack (HTTP/2, HPACK, JSON, TOML, DNS…)
  is **written in raylang** (`packages/`), and the runtime (LSP, `kqueue`/`epoll` poller, the package
  manager's SHA, the transpiler) in the project's own Rust, without crates. The crates that do come in —
  TLS, crypto, SQLite, library loading, fibers, allocator and hasher — are listed with their why and
  their scope in [`SECURITY.md`](SECURITY.md#política-de-dependencias); a *slim* build leaves the first
  three out.
- **Robust against arbitrary input.** A panic-free compiler + **continuous fuzzing** of the front-end.

## raylang and LLM agents

raylang ships the two pieces an AI coding agent needs to write correct raylang:

- **[`llms.txt`](llms.txt)** — the distilled context (~250 lines): the delta against Rust, the canonical
  forms and the exact error messages. Paste it into your prompt / `CLAUDE.md` (or let your MCP client
  load the `raylang://llms.txt` resource).
- **`ray mcp`** — an [MCP](https://modelcontextprotocol.io) server embedded in the binary, which gives
  the model the **write → verify → fix** loop: `ray_check` (exact diagnostics), `ray_run`, `ray_test`,
  `ray_fmt` and `ray_doc` tools, with the model's code **sandboxed** (fuel + heap + deadline, in a
  subprocess). With Claude Code: `claude mcp add raylang -- ray mcp`. Full guide:
  [`docs/mcp.md`](docs/mcp.md).

## Documentation

The reference documents are written in Spanish (the project's working language); the code, the
`///` doc comments and every message the compiler emits are in English. Translated so far:
[`REFERENCE.en.md`](REFERENCE.en.md) and [`docs/mcp.en.md`](docs/mcp.en.md) (kept in sync with their
originals by a CI guard).

| Document | What it is |
|----------|------------|
| [`MANUAL.md`](MANUAL.md) | The **practical guide**: how to use the language, idioms and best practices. |
| [`REFERENCE.en.md`](REFERENCE.en.md) | The **exhaustive catalog**: keywords, operators, builtins, prelude, `std/` and CLI, with signatures (in English; original: [`REFERENCE.md`](REFERENCE.md)). |
| [`PUBLISH.md`](PUBLISH.md) | The **publisher's guide**: packaging, versioning and publishing to the registry. |
| [`SPEC.md`](SPEC.md) | The **normative specification** of the language (grammar + semantics). |
| [`llms.txt`](llms.txt) | **raylang for LLMs**: the distilled context (delta vs Rust, canonical forms, exact errors) so a model writes correct raylang. Paste it into your prompt/CLAUDE.md. |
| [`docs/getting-started.en.md`](docs/getting-started.en.md) | **raylang in half an hour**: install, a project, the language in fifteen minutes, concurrency, a web server and the tools (in English; original: [`docs/getting-started.md`](docs/getting-started.md)). |
| [`docs/mcp.en.md`](docs/mcp.en.md) | The **MCP server** (`ray mcp`): the check/run/test/fmt/doc tools for LLM agents, with the code sandboxed (fuel/heap/deadline) (in English; original: [`docs/mcp.md`](docs/mcp.md)). |
| [`docs/web-framework.md`](docs/web-framework.md) | The **web framework** guide (Express-style): routes, middleware, SSR, deploy. |
| [`docs/build.md`](docs/build.md) | The **builds** guide: slim features, PGO, native binary. |
| [`docs/transpilador-nativo.md`](docs/transpilador-nativo.md) | The **native backend** from the inside: how it transpiles to Rust and how parity is guaranteed. |
| [`docs/diseno-concurrencia-nativa.md`](docs/diseno-concurrencia-nativa.md) | The native binary's **M:N fiber scheduler**: coroutines, reactor and decisions. |
| [`docs/windows.md`](docs/windows.md) | The **Windows** contract: what works, how, and the remaining debts. |
| [`PERFORMANCE.md`](PERFORMANCE.md) | The **performance chronicle**: every optimization arc, measured. |
| [`PRODUCTION.md`](PRODUCTION.md) | The **production contract**: axes, invariants and current quality criteria. |
| [`book/`](book/) | The **book** (mdBook): how the language was **built**, phase by phase. |
| [`DESIGN.md`](DESIGN.md) | The **design chronicle**: every decision and its why. |
| [`IDEAS.md`](IDEAS.md) | Backlog of features and their impact classification. |
| [`docs/organizacion-codigo.md`](docs/organizacion-codigo.md) | How the compiler's **code is organized** (directory modules, where the tests live). |
| [`SECURITY.md`](SECURITY.md) | Security policy, dependency policy and threat model. |
| [`CHANGELOG.md`](CHANGELOG.md) | What changed in each version. |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | The **contribution guide**: PR flow, non-negotiable principles and the admission battery for modules. |
| [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) | Code of conduct (Contributor Covenant 2.1) governing participation in the project. |
| [`RELEASE-1.0.md`](RELEASE-1.0.md) | Historical checklist of the 1.0 launch (releases are published from v1.1.0 on). |

Editors: a [VSCode](editors/vscode/) extension (with LSP client), a [Sublime Text](editors/sublime/)
package, a [Zed](https://github.com/ray-language/zed-raylang) extension (tree-sitter + `ray lsp`) and
config for Neovim/Helix (they use `ray lsp` directly).

## Status

The latest published version is on the [GitHub Releases](https://github.com/ray-language/raylang/releases)
page (binaries per platform: macOS and Linux on arm64 and x86_64, Windows on x86_64 and arm64, plus the
self-contained toolchain for `ray build --native`); `ray version` shows it and `ray upgrade` installs it.
Work continues along that line (performance, native concurrency, web framework, desktop and mobile).
Product engine = the VM; the native binary is the deployment target and the interpreter, the
development oracle. The suite includes unit tests per phase, more than a hundred integration test
files, a front-end fuzzer, the VM↔interpreter oracles and the native binary's parity corpus. What is
published and what is on the way, in [`CHANGELOG.md`](CHANGELOG.md).

The focus is **real production**, with the scope stated up front: it is built by a single maintainer
and has not been externally audited (see [`SECURITY.md`](SECURITY.md#alcance)).

## Contributing

The contractual guide is **[`CONTRIBUTING.md`](CONTRIBUTING.md)**: the flow (branch + PR against
`main`, Conventional Commits in Spanish, green CI with the `fmt`/`naming`/`module` guards), the
non-negotiable principles (the SPEC rules, byte-identity of the three engines, language, errors as
values) and the **admission battery** for new `std/`/`packages/` modules (`///` docs in English, a row
in `REFERENCE.md`, tests on both engines, demonstrated real use). Before touching behavior, read the
contract documents: [`SPEC.md`](SPEC.md) rules the semantics and [`DESIGN.md`](DESIGN.md) tells the
why. To report a vulnerability, see [`SECURITY.md`](SECURITY.md).

## License

Dual-licensed, at your option:

- **MIT** ([`LICENSE-MIT`](LICENSE-MIT))
- **Apache-2.0** ([`LICENSE-APACHE`](LICENSE-APACHE))

`SPDX-License-Identifier: MIT OR Apache-2.0`

---

<div align="center">
<img src="assets/raylang-mascot.png" alt="raylang's mascot: a smiling manta ray" width="130">
<br>
<sub>The brand identity (logo, variations, colors) lives in <a href="assets/"><code>assets/</code></a> · <a href="assets/branding/raylang-brand.pdf">brand book</a>.</sub>
</div>

<!-- sync: sha256:b8f99cae221a -->
