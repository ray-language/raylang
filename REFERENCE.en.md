# raylang Reference

[Español](REFERENCE.md) · English

The **exhaustive catalog** of the language surface: keywords, symbols, operators, builtins, prelude,
standard library and tools. It complements [`MANUAL.md`](MANUAL.md) (the practical guide, with prose
and examples — in Spanish) and [`SPEC.md`](SPEC.md) (the normative reference — in Spanish: on any
discrepancy, the SPEC rules).

## Contents

1. [Keywords](#1-keywords)
2. [Symbols and operators](#2-symbols-and-operators)
3. [Literals and escapes](#3-literals-and-escapes)
4. [Types](#4-types)
5. [Global builtins](#5-global-builtins)
6. [Methods by receiver type](#6-methods-by-receiver-type)
7. [Associated functions of types](#7-associated-functions-of-types)
8. [The prelude](#8-the-prelude)
9. [Iterators](#9-iterators)
10. [The standard library `std/`](#10-the-standard-library-std)
11. [Additional packages (`net`, `web`, `rpc`, `db`, `tz`, `cron`, `mcp`, `llm`, `agent`)](#11-additional-packages-net-web-rpc-db-tz-cron-mcp-llm-agent)
12. [Annotations](#12-annotations)
13. [FFI: marshalable types](#13-ffi-marshalable-types)
14. [The `ray` CLI](#14-the-ray-cli)
15. [Exit codes](#15-exit-codes)

---

## 1. Keywords

Reserved (cannot be used as identifiers):

| Group | Words |
|---|---|
| Declarations | `fn` `let` `var` `const` `struct` `enum` `trait` `impl` `extern` (`type` is **contextual**: `type Alias<T> = …;` only at the start of an item) |
| Control | `if` `else` `while` `for` `in` `match` `return` `break` `continue` |
| Modules | `import` `pub` (`from` is **contextual**: only at the start of an item, `from M import x;`) |
| Values/types | `true` `false` `dyn` `as` `self` `Self` |
| Primitive types | `int` `float` `bool` `string` `char` `bytes` `ptr` `u8` `u32` `u64` |

> `from` is valid as a parameter or variable name: it is a keyword only at the head of a
> `from M import …;`.

> `return [e]` is also an **expression**: in a `match` arm, an `else` or a `let` value,
> `Option.None => return code,` equals `{ return code; }` — it **diverges** and yields the type to
> the rest; as a block tail it needs no `;` (`else { return 99 }`). `ray fmt` keeps it.
> Likewise **`break` and `continue`**: `Result.Err(e) => break,` in an arm inside a loop,
> `if (c) { continue } else { v }`; same statement-spine restriction as with braces.
> **Labeled loops**: `outer: for row in grid { for x in row { if (x == 0) { break outer; } } }`
> — `break outer` / `continue outer` from any inner loop of the same function (`outer: while` too).

## 2. Symbols and operators

### Precedence table (lowest to highest; SPEC §6.1)

| Level | Operators | Associativity |
|---|---|---|
| 1 | `\|>` (pipeline) | left |
| 2 | `\|\|` (logical OR, short-circuits) | left |
| 3 | `&&` (logical AND, short-circuits) | left |
| 4 | `\|` (bitwise OR) | left |
| 5 | `^` (bitwise XOR) | left |
| 6 | `&` (bitwise AND) | left |
| 7 | `==` `!=` | left |
| 8 | `<` `<=` `>` `>=` | left |
| 9 | `<<` `>>` (shifts) | left |
| 10 | `+` `-` | left |
| 11 | `*` `/` `%` | left |
| 12 | `as` (cast) | left |
| 13 | `-` `!` `~` (unary) | prefix |
| 14 | call `f(…)` · field/method `x.f` · index `x[i]` · `?` | postfix |
| 15 | primaries (literals, `(…)`, blocks…) | — |

> Practical consequence: **bitwise operators bind looser than comparisons** (C style).
> `flags & 32 != 0` parses as `flags & (32 != 0)` → type error. Write `(flags & 32) != 0`.

### All symbols

| Symbol | Meaning |
|---|---|
| `+` | addition (int/float/u\*); **concatenates** strings, bytes and arrays; overloadable (trait `Add`) |
| `-` | subtraction; unary negation; overloadable (`Sub`/`Neg`) |
| `*` `/` `%` | product, quotient, remainder (division/modulo by zero = runtime error); `Mul`/`Div` |
| `==` `!=` | structural equality (primitives, strings, bytes; user types via `Eq`) · `f() == Option.None` / `r != Result.Err("x")` take `T` from the other operand |
| `<` `<=` `>` `>=` | comparison: numbers, strings (lexicographic), chars (code point) |
| `&&` `\|\|` `!` | logical (short-circuit in `&&`/`\|\|`) |
| `&` `\|` `^` `~` `<<` `>>` | bitwise on `int`/`u8`/`u32`/`u64` (*wrapping* semantics; the shift masks the width) |
| `=` | assignment (a statement, **not** an expression) |
| `( )` | grouping, calls, tuples, scrutinee of `match`/`if`/`while` |
| `{ }` | blocks (produce values), struct literals, bodies |
| `[ ]` | array literals `[1, 2, 3]` (trailing comma allowed), types `[T]`, indexing `a[i]` |
| `,` `;` `:` | separators; end of statement; type annotation |
| `.` | field, method (UFCS), enum variant (`Option.Some`), module (`math.PI`), tuple (`t.0`), and the `builtin` pseudo-module (`builtin.close(h)`: the builtin even when the module defines its own `close`) |
| `..` | range in `for i in a..b` (half-open) |
| `->` | function return type |
| `=>` | `match` arm |
| `?` | propagation of `Err`/`None` (early return); with `impl From<E1> for E2`, converts the error |
| `\|>` | pipeline: `x \|> f(a)` ≡ `f(x, a)` |
| `@` | annotations: `@test`, `@derive(…)` |
| `_` | wildcard in patterns; discard in `let _ = …` |
| `(p, q)` / `0` `"s"` `'c'` `true` | tuple pattern and literal patterns in `match` (also inside a payload: `Shape.Rect(w, 0)`); exhaustiveness is checked recursively and an `int`/`string`/`char` always needs `_` |
| `${…}` | interpolation inside a string literal |
| `//` `///` | line comment; documentation comment (`ray doc`, LSP hover) |
| `b"…"` | bytes literal |

## 3. Literals and escapes

| Literal | Form | Notes |
|---|---|---|
| Integer | `42`, `-7`, `0xFF`, `0o755`, `0b1010`, `255u8`, `0xFFu32` | decimal or with a `0x`/`0o`/`0b` prefix (hex/octal/binary, uppercase too). Optional suffix `u8`/`u32`/`u64` = that type without context. Without a suffix: fits in `int` → `int` (coerces to the `u*` of the context); does not fit in `int` but fits in `u64` (`0xFFFFFFFFFFFFFFFF`) → **wide**, `u64`. No `_` (deferred) |
| Float | `3.14` | `digits . digits` (the `.` requires a fraction: `2.0`, not `2.`); always decimal |
| Boolean | `true` / `false` | |
| String | `"hello"` | escapes `\n \t \r \\ \" \$ \0`, `\xNN` (hex octet, U+0000..U+00FF) and `\u{H…H}` (1–6 hex digits, Unicode code point); no literal line breaks |
| Interpolated string | `"x = ${expr}"` | `${expr}` = **one** expression; desugars to `+ to_string(expr)`. `\${` = literal. `"$5"` and `"{n}"` are literals (the `$` is only special before `{`) |
| Char | `'a'`, `'\n'`, `'\x41'`, `'\u{1F600}'` | one Unicode code point; escapes `\n \t \r \\ \' \0`, `\xNN` and `\u{H…H}` |
| Bytes | `b"ok\x00\xff"` | string escapes + `\xNN` (octet in hex) |
| Array | `[1, 2, 3,]` | trailing comma allowed; an empty `[]` needs context or an annotation |
| Tuple | `(1, "a")` | access `t.0`, `t.1`; destructuring `let (a, b) = t;`; shown as `(1, a)` by `print`/`to_string`/`assert_eq`, the same on the three engines (before, `print` gave `[[1, a]]` on the VM) |
| Struct | `Point { x: 1, y: 2 }` | |
| Enum | `Option.Some(5)`, `Color.Red` | |
| Anonymous function | `fn(x: int) -> int { x * 2 }` | closure: captures the scope by reference — within a fiber; what a closure passed to `spawn` captures is **copied** when the fiber starts (only channels and handles are shared) |

## 4. Types

| Type | Description |
|---|---|
| `int` | 64-bit signed integer; arithmetic overflow is a **runtime error** |
| `u8` `u32` `u64` | unsigned integers; arithmetic **wraps** by design; literals coerce from the context (`let x: u8 = 5`) |
| `float` | 64-bit IEEE 754 |
| `bool` | `true`/`false` |
| `string` | immutable UTF-8 text; indexable **by character** (`s[i] -> char`) |
| `char` | one Unicode code point |
| `bytes` | immutable sequence of octets; `b[i] -> int`. Performance: the buffer is **shared**: loading the variable, passing it, sending it over a channel or indexing it is O(1) whatever its size; `+` on `bytes` is amortized linear (gathering 8 MB in 128 chunks: ~20 ms); building octet by octet with `push` into `[int]` + `bytes_of` is two orders of magnitude slower — accumulate `bytes` with `+` or `sub_bytes` |
| `unit` | "no useful value" (return of `print`, etc.) |
| `[T]` | dynamic array, **reference** semantics |
| `(A, B, …)` | tuple (immutable aggregate, copied as a value; `t.0 = x` is an error) |
| `Map<K, V>` | key→value table; *hashable* keys: int/u\*/string/char/bool/bytes (**not** float); traversed in key order (deterministic) |
| `fn(A, B) -> R` | function type (functions and closures are first-class values) |
| `Option<T>` / `Result<T, E>` | from the prelude; absence and error as values (there is no `null` and no exceptions) |
| `Channel<T>` / `Task<T>` | concurrency (SPEC §9) |
| your own `struct` / `enum` | reference semantics (struct/enum); generics with bounds (`struct Box<T: Show>`) |
| `type Alias<T> = type;` | type alias: a name for a type, not a new type (`type Id = int` is `int`; `type Pair<T> = (T, T)`; `type Handler = fn(Req) -> Res`); `pub type` is exported like a type; expanded in the checker (erasure) and diagnostics show the expanded type |
| `dyn Trait`, `dyn A + B` | trait objects (dynamic dispatch); *upcasting* to a subset of traits; allowed as a struct field and with qualified paths (`dyn m.Trait`, `impl m.Trait for T`); shown as `<dyn Trait>` |
| `Iter<T>` | lazy iterator (closure-backed) |
| `ptr` | opaque FFI pointer (not dereferenceable from raylang) |
| `Set<T>` / `Deque<T>` / `StringBuilder` | from `std/collections` (§10) |

**Casts** with `as`: `float as int` (truncates; saturates at the edge), `int as float`, `char as int`,
`int as char` (validates the code point), `int ↔ u8/u32/u64` and between unsigned widths (they mask),
`float ↔ u*`.

## 5. Global builtins

Always available, without `import`. The `__name` primitives are **internal and unstable** — do not
use them; each has its public wrapper in the prelude or in `std/`.

> A builtin **wins** over a user function of the same name: do not redefine `print`, `len`, etc.

### Core and output

| Function | Signature | Description |
|---|---|---|
| `print` | `(value) -> unit` | prints to stdout + newline (int, float, bool, string, char, u\*, bytes→hex, arrays, types with `Show`) |
| `eprint` | `(value) -> unit` | like `print`, to stderr |
| `to_string` | `(value) -> string` | textual representation (same as `print`): int/float/bool/string/char/bytes/u\*; also any struct/enum with `Show` (same as `value.show()`) |
| `panic` | `(msg: string) -> unit` | aborts the program with the message and the position; for broken invariants, not for expected errors |
| `exit` | `(code: int) -> unit` | terminates the PROCESS with that code, from any fiber (flushes stdout/stderr). Diverges like `panic`; it is not an error (no message, no trace) and `try_call` does not catch it |
| `args` | `() -> [string]` | command-line arguments (after the program path) |
| `platform` | `() -> string` | the process's operating system — `"macos"`, `"linux"`, `"windows"` (the names of `std::env::consts::OS`); a target literal in native binaries |
| `arch` | `() -> string` | the CPU architecture as Rust names it: `"aarch64"`, `"x86_64"`. With `platform()` it forms the `<platform>-<arch>` key of an update manifest |

### Concurrency (VM and native binary — the interpreter has no fibers; §11 of the manual)

| Function | Signature | Description |
|---|---|---|
| `spawn` | `(f: fn() -> T) -> Task<T>` | launches a concurrent task; `join` waits for its value |
| `spawn_isolated` | `(f: fn() -> T) -> Task<T>` | like `spawn`, but in a fresh **handle domain**: the task cannot use the files/sockets/processes/windows of other domains (they behave as closed) nor they its own; its children inherit the domain |
| `join` | `(t: Task<T>) -> T` | blocks until the task finishes (re-raises its failure). *Ad-hoc*: `join(arr, sep)` is the string one |
| `scope` | `(body: fn() -> R) -> R` | structured concurrency: on return it joins every task launched inside; if one fails, it cancels its siblings and propagates |
| `send` | `(ch: Channel<T>, v: T) -> unit` | sends; blocks if the bounded channel is full (backpressure) |
| `recv` | `(ch: Channel<T>) -> Option<T>` | receives; blocks while empty and open; `None` once closed and drained |
| `select` | `(chs: [Channel<T>]) -> int` | blocks until a channel is ready; returns the lowest ready index (deterministic) |
| `try_recv` | `(ch: Channel<T>) -> Received<T>` | receives **without blocking**: `Received.Got(v)` (a value was ready, it consumes it), `Received.Empty` (open and empty), `Received.Closed` (closed and drained). For "check for data OR a control command without getting stuck" |
| `select_timeout` | `(chs: [Channel<T>], ms: int) -> Option<int>` | `select` with a **deadline**: `Some(i)` (lowest ready index), `None` if the `ms` ms elapse; `ms <= 0` = non-blocking poll. Event-driven (wakes when a channel arrives, does not poll) |
| `signals` | `() -> Channel<int>` | the OS signal channel (SIGTERM=15, SIGINT=2, SIGWINCH=28); a process singleton, for graceful shutdown and re-layout on resize (`select` + `term.size()`) — composes with `recv`/`select`. Unix; VM and native binary |
| `try_send` | `(ch: Channel<T>, v: T) -> bool` | sends **without blocking or failing**: `true` if delivered (a parked receiver) or queued (room), `false` if the channel is closed or full. For producers whose consumer may be gone |
| `close` | `(ch \| handle) -> …` | closes a channel (pending values can still be received; blocked senders wake up and their `send` fails; idempotent) **or** a file/socket handle |

### Failure recovery

| Function | Signature | Description |
|---|---|---|
| `try_call` | `(f: fn() -> T) -> Result<T, string>` | runs `f` and turns a `panic`/runtime error into `Err(message)`. Recovers in the **same fiber**: whatever `f` mutated stays mutated (like Rust's `catch_unwind`). All three engines |
| `try_join` | `(t: Task<T>) -> Result<T, string>` | a task's failure as a value instead of re-raising it. True isolation (the fiber's own heap). VM and native |

> ⚠️ **Math, clock, randomness, crypto, disk and network are NOT global builtins.** They live in
> `std/` modules and are used qualified: `math.sqrt(2.0)`, `time.now()`,
> `random.below(10)`, `crypto.sha256(b)`, `fs.read_file(p)`, `net.tcp_connect(h, p)`. Catalog in §10.

### Input and environment

| Function | Signature | Description |
|---|---|---|
| `env` | `(name: string) -> Option<string>` | environment variable; `None` if not defined |
| `input` | `() -> Option<string>` | one line from stdin (without the newline); `None` on EOF |
| `read_int` | `() -> Option<int>` | one line from stdin parsed as an integer |

### Others

| Function | Signature | Description |
|---|---|---|
| `bytes_of` | `([int]) -> bytes` | builds bytes from octets 0–255 |
| `char_code` | `(char) -> int` | Unicode code point |
| `char_from_code` | `(int) -> Option<char>` | the inverse; `None` if not a valid code point |
| `range` | `(a: int, b: int) -> Iter<int>` | half-open iterator `[a, b)` (from the prelude) |
| `iter` | `(xs: [T]) -> Iter<T>` | lazy iterator over an array |
| `sum` / `sum_float` | `(Iter<int>) -> int` · `(Iter<float>) -> float` | sums an iterator (via UFCS: `it.sum()`) |
| `min` / `max` | `(Iter<T: Ord>) -> Option<T>` | **iterator terminals** (not the minimum of two values: that is `math.min`) |
| `sort` | `(xs: [T: Ord]) -> [T]` | sorts an array (sorted copy, stable); `T` primitive or a user type with `impl Ord` — natively too |
| `sort_by` / `sort_by_key` | `(xs: [T], less: fn(T, T) -> bool) -> [T]` · `(xs: [T], key: fn(T) -> K: Ord) -> [T]` | sort by **comparator** (`less(a, b)` = "a goes first") or by **key**; stable, new copy; also via UFCS `xs.sort_by(…)`  — `less(a, b)` returns **`bool`**, not a three-way integer |
| builtin as a value | `xs.map(to_string)` · `let f: fn(int) -> string = to_string` | a builtin can be passed as a function wherever a function type is **expected** (its signature depends on the argument type: `to_string` on `int` is `fn(int) -> string`); with no expected type (`let g = to_string`) annotate it |
| `assert` / `assert_eq` | `(bool)` · `(a: T, b: T)` | test-runner assertions; they fail with `panic` |

## 6. Methods by receiver type

Called with a dot (`recv.method(args)`); almost all are prelude traits or method-builtins, so they
also exist as free calls (`method(recv, args)`).

### `string`

| Method | Result | Description |
|---|---|---|
| `s.len()` | `int` | length **in characters** |
| `s[i]` | `char` | indexing by character (out of range = error); `s[i] = c` is forbidden (immutable). **Cost**: ASCII in O(1); on non-ASCII text **sequential** access (ascending or descending) is amortized O(1) and random access O(distance) from the last position; `s.len()` is O(1) after the first time. A `while (i < s.len()) { s[i] }` loop is no longer quadratic; `s.chars()` remains the explicit way to materialize the array |
| `s.trim()` | `string` | without surrounding whitespace: spaces, tabs and line breaks (`"\r\n".trim()` is `""`) |
| `s.split(sep)` | `[string]` | parts |
| `s.contains(sub)` | `bool` | substring |
| `s.replace(from, to)` | `string` | replaces all |
| `s.chars()` | `[char]` | characters |
| `s.starts_with(p)` / `s.ends_with(p)` | `bool` | prefix/suffix |
| `s.to_upper()` / `s.to_lower()` | `string` | uppercase/lowercase |
| `s.substring(i, j)` | `string` | `[i, j)` by character, *clamped* (never fails) |
| `s.repeat(n)` | `string` | repeated (`n <= 0` → `""`) |
| `s.index_of(sub)` | `Option<int>` | index of the first occurrence |
| `s.last_index_of(sub)` | `Option<int>` | character index of the LAST occurrence (`Some(len)` for an empty `sub`) |
| `s.to_bytes()` | `bytes` | encodes UTF-8 |
| `s.parse_int()` / `s.parse_float()` | `Option<int/float>` | parsing (via the prelude's UFCS) |
| `a + b` | `string` | concatenation |
| `<` `<=` `>` `>=` | `bool` | lexicographic order |

### Arrays `[T]`

| Method | Result | Description |
|---|---|---|
| `a.len()` | `int` | elements |
| `a[i]` / `a[i] = v` | `T` / — | indexing and assignment (out of range = error) |
| `a.push(x)` | `unit` | appends at the end, **in place** |
| `a.pop()` | `Option<T>` | removes and returns the last |
| `a.contains(x)` | `bool` | membership (structural equality) |
| `a.position(x)` | `Option<int>` | index of the first occurrence |
| `a.reverse()` | `[T]` | reversed copy |
| `a.slice(from, to)` | `[T]` | copy of `[from, to)`, clamped like `substring` (`xs.slice(1, xs.len())` = all but the first) |
| `a.sort()` | `[T]` | sorted copy (`T: Ord`) |
| `a.join(sep)` | `string` | only `[string]` |
| `a.map(f)` / `a.filter(p)` / `a.fold(init, f)` | eager | materialize an array/value (§9 for the lazy version) |
| `a.iter()` | `Iter<T>` | lazy iterator |
| `a + b` | `[T]` | concatenation |

### `bytes`

| Method | Result | Description |
|---|---|---|
| `b.len()` | `int` | octets |
| `b[i]` | `int` | octet (0–255), in O(1) (unlike `s[i]`) |
| `b.sub_bytes(i, j)` | `bytes` | slice `[i, j)`, *clamped* |
| `b.index_of(needle)` | `Option<int>` | OCTET index of the first occurrence of the subsequence (`Some(0)` for an empty needle); allocation-free (finding a header's `\r\n\r\n` is one call) |
| `b.index_of_from(needle, start)` | `Option<int>` | like `index_of` from octet `start` (no copy: an incremental parser's `find(buf, from)`) |
| `b.last_index_of(needle)` | `Option<int>` | octet index of the LAST occurrence (`Some(len)` for an empty needle) |
| `b.starts_with(prefix)` | `bool` | does it begin with `prefix`? (the empty prefix always matches) |
| `from_utf8(b)` | `Result<string, string>` | decodes UTF-8 |
| `b1 + b2` | `bytes` | concatenation |
| `to_string(b)` / `print(b)` | hex | hexadecimal representation |

### `Map<K, V>`

| Method | Result | Description |
|---|---|---|
| `m.len()` | `int` | entries |
| `m.insert(k, v)` | `unit` | inserts/updates, in place |
| `m.get(k)` | `Option<V>` | lookup |
| `m.remove(k)` | `Option<V>` | removes and returns |
| `m.contains_key(k)` | `bool` | |
| `m.keys()` / `m.values()` | `[K]` / `[V]` | **sorted by key** (deterministic); they match position by position |
| `for (k, v) in m` | — | traversal in key order |

### `Task<T>` and `Channel<T>`

`t.join()`, `ch.send(v)`, `ch.recv()`, `ch.try_recv()`, `ch.close()`, `chs.select()` — the concurrency
builtins via UFCS. `try_recv` returns `Received<T>` (`enum Received<T> { Got(T), Empty, Closed }` from
the prelude): non-blocking reception.

## 7. Associated functions of types

Constructors with the `Type.function(…)` syntax:

| Function | Signature | Description |
|---|---|---|
| `Map.new` | `() -> Map<K, V>` | empty map (the type is fixed by the context: `var m: Map<string, int> = Map.new();`) |
| `Channel.new` | `() -> Channel<T>` | unbounded channel (send never blocks) |
| `Channel.bounded` | `(n: int) -> Channel<T>` | channel bounded to `n` (backpressure; `n = 0` = synchronous rendezvous) |

## 8. The prelude

Functions and traits **written in raylang**, injected into every program (you can *override* them by
defining the same name).

### Functions

| Function | Signature | Description |
|---|---|---|
| `parse_int` / `parse_float` | `(string) -> Option<int/float>` | parsing |
| `char_from_code` | `(int) -> Option<char>` | inverse of `char_code` (validates the code point) |
| `input` | `() -> Option<string>` | one line from stdin (`None` on EOF) |
| `read_int` | `() -> Option<int>` | `input` + `parse_int` |
| `env` | `(name: string) -> Option<string>` | environment variable |
| `map` / `filter` / `fold` / `any` / `all` | eager over `[T]` | see §6 |
| `sort` | `([T]) -> [T]` with `T: Ord` | bottom-up merge sort, stable, O(n log n); returns a new array |
| `iter` / `range` / `sum` / `sum_float` / `min` / `max` | iterators | see §9 (`min`/`max` are terminals: `Iter<T> -> Option<T>`) |
| `get` / `get_or` / `remove` | on `Map` | see §6 |
| `try_call` / `try_join` | failure recovery | see §5 |
| `recv` | `(Channel<T>) -> Option<T>` | see §5 |
| `assert` | `(bool) -> unit` | aborts if false |
| `assert_eq` | `(a: T, b: T)` with `T: Eq + Show` | aborts showing both values; tuples work (element-wise `Eq`/`Show`) |
| `assert_eq_msg` | `(a: T, b: T, msg: string)` with `T: Eq + Show` | like `assert_eq`, with a message naming WHAT is compared: `assert_eq failed: <msg>: <a> != <b>` |
| `pop` / `position` / `index_of` / `from_utf8` | — | see §6 |

### Traits

| Trait | Method(s) | Notes |
|---|---|---|
| `Eq` | `eq(self, other: Self) -> bool` | enables `==`/`!=` on user types; `Option<T>`/`Result<T, E>` implement it when their parameters do (`assert_eq(o, Option.Some(1))`), as does `Show` (`Option.Some(1)`); derivable |
| `Show` | `show(self) -> string` | enables `print`/`to_string`; derivable |
| `Ord` | `less(self, other: Self) -> bool` | enables `sort`/`min`/`max`; impls for int/float/string/char |
| `Hash` | `hash(self) -> int` | keys of `Set`; derivable |
| `Add` `Sub` `Mul` `Div` | `add/sub/mul/div(self, other: Self) -> Self` | overloading of `+ - * /` on user types |
| `Neg` | `neg(self) -> Self` | overloading of unary `-` |
| `From<S>` | `convert(source: S) -> Self` | conversion; `?` uses it to convert errors (`from` is a keyword → the method is called `convert`) |
| `Iterator<T>` | `next(self) -> Option<T>` | the iteration protocol; brings the adapters as default methods |
| `Len` / `Push<T>` / `Contains<T>` | `len`/`push`/`contains` | the container methods, as traits |
| `Signed` | `abs(self) -> Self` | for the generic `abs` of `std/math` |

`Option<T>` (`Some`/`None`) and `Result<T, E>` (`Ok`/`Err`) are prelude enums, with these
**methods** (prelude traits `OptionOps`/`ResultOps`; no import needed):

| Receiver | Method | Description |
|---|---|---|
| `Option<T>` | `is_some() / is_none() -> bool` | is there a value? |
| `Option<T>` | `unwrap_or(default: T) -> T` | the value or the default |
| `Option<T>` | `expect(msg: string) -> T` / `unwrap() -> T` | the value or `panic` (with `msg` as context; prefer `expect`) |
| `Option<T>` | `ok_or(err: E) -> Result<T, E>` | `None` → `Err(err)` |
| `Option<T>` | `map(f: fn(T) -> U) -> Option<U>` / `and_then(f: fn(T) -> Option<U>) -> Option<U>` | transform / chain a fallible step |
| `Option<T>` | `unwrap_or_else(f: fn() -> T) -> T` | the value or `f()` (the default is computed only if needed) |
| `Result<T, E>` | `is_ok() / is_err() -> bool` | success? |
| `Result<T, E>` | `unwrap_or(default: T) -> T` | the value or the default |
| `Result<T, E>` | `expect(msg: string) -> T` / `unwrap() -> T` | the value or `panic` |
| `Result<T, E>` | `ok() -> Option<T>` | drops the error |
| `Result<T, E>` | `map(f: fn(T) -> U) -> Result<U, E>` / `and_then(f: fn(T) -> Result<U, E>) -> Result<U, E>` | transform / chain the `Ok`; an `Err` passes through |
| `Result<T, E>` | `map_err(f: fn(E) -> F) -> Result<T, F>` / `unwrap_or_else(f: fn(E) -> T) -> T` | transform the error / the value or `f(e)` |


## 9. Iterators

`xs.iter()` and `range(a, b)` produce a **lazy** `Iter<T>`: the adapters compute nothing until a
terminal walks the chain (a single pass, no intermediate arrays).

| Adapter (lazy) | Description |
|---|---|
| `.map(f)` | transforms each element |
| `.filter(pred)` | lets through those that match |
| `.take(n)` / `.skip(n)` | cuts / skips the first n |
| `.enumerate()` | `(index, element)` pairs — consumed with a tuple pattern: `for (i, x) in …` |
| `.zip(other)` | pairs two iterators into `(T, U)`; ends with the shorter one |

| Terminal | Description |
|---|---|
| `.fold(init, f)` | reduces to a value |
| `.collect()` | materializes into `[T]` |
| `.sum()` | sum (`Iter<int>`) |
| `for x in it { … }` | iterates |

A type of yours becomes iterable by implementing `Iterator<T>` (only `next`); it inherits every adapter.

## 10. The standard library `std/`

**Embedded in the binary** (works without files on disk). Imported by path and used qualified by the
*leaf*: `import std/math;` → `math.gcd(12, 18)`.

| Module | Public surface |
|---|---|
| `std/math` | `PI` `E` · `sqrt pow sin cos tan ln log10 exp floor ceil round` · `abs<T: Signed>` `min<T: Ord>` `max<T: Ord>` · `iabs sign clamp gcd lcm ipow factorial is_prime` · `float_bits float_from_bits` (IEEE 754 bits) |
| `std/text` | `is_empty pad_left pad_right capitalize reverse count words lines` · Unicode normalization: `nfc nfd nfkc nfkd` (the K forms flatten presentation variants; an accent-insensitive slug = `nfd` + drop combining marks U+0300..U+036F). Without the `unicode` feature (slim): a clear error |
| `std/sort` | `is_sorted sort_desc min max binary_search dedup merge` (all with `T: Ord`) |
| `std/fs` | `read_file write_file append_file remove_file list_dir exists mkdir read_file_bytes write_file_bytes` (→ `Result`; **`mkdir` also creates the missing parents, like `mkdir -p`, and an existing directory is not an error**) · `remove_all(path)` (**recursive** removal; `remove_dir` stays empty-only) · `temp_dir() -> string` (the system temp) · `make_temp_dir(prefix) -> Result<string, _>` (a new, unique directory `<temp>/<prefix><pid>_<n>`: no collisions between parallel runs) · `real_path(path) -> Result<string, string>` (the real path: symlinks followed and `.`/`..` resolved; the path must exist; on Windows without the `\\?\` prefix) · `is_within_real(inner, outer) -> Result<bool, string>` (whether `inner` lives under `outer` once BOTH are resolved — the check that confines paths asked for by a user or a model: an internal symlink pointing outside yields `false`; `outer` itself counts as within) · handles: `open(path, "r"/"w"/"a") -> Result<int, _>` `read_line(h) -> Option<string>` `write(h, s)` + `close(h)` · **streaming**: `read_bytes(h, max) -> Result<Option<bytes>, string>` (up to `max` octets from the current position — exact except near the end; `None` = EOF; memory bounded by what is read) · `seek(h, pos) -> Result<int, string>` (absolute position from the start; returns the new position → resumable transfers) · **durability**: `write_bytes(h, data) -> Result<int, string>` (raw octets at the handle's current position — the binary twin of `write`; composes with `seek`) · `sync(h) -> Result<int, string>` (flushes the buffers AND forces the file to stable storage — fsync; without it an append survives a process crash but not a power cut; ⚠️ on macOS/APFS it is F_FULLFSYNC: 4–5 ms per call, a sync-per-record design tops out at ~200 ops/s) · `sync_data(h) -> Result<int, string>` (fdatasync — data only, no full flush — truly cheap on APFS: Rust's std uses `F_FULLFSYNC` for `sync_data` too on Apple, now a plain `fsync(2)`, ~12,000 ops/s vs ~210; cheap on APFS but a power cut may lose the last records: `sync` at checkpoints) · **locks**: `try_lock(h) -> Result<bool, string>` (EXCLUSIVE advisory lock without blocking — flock; `true` = acquired, `false` = another open file description holds it; the single-process LOCK-file pattern) · `unlock(h) -> Result<int, string>` (`close(h)` also releases it) · **metadata**: `stat(path) -> Result<Stat, string>` — WITHOUT following symlinks (lstat): `Stat { kind, mode, size, mtime_ms }` with kind `"file"`/`"dir"`/`"symlink"`/`"other"`, mode = the 12 permission bits in decimal (0o600 = 384), size in bytes (of a symlink: the length of the link itself), mtime in epoch-ms · `chmod(path, mode) -> Result<int, string>` (changes the permission bits; 384 = 0o600, 493 = 0o755) · **watch** (KERNEL events — FSEvents/inotify, not mtime polling): `watch(path) -> Result<int, string>` (directory → recursive; file → itself; `close(h)` stops it) · `next_event(h) -> Result<WatchEvent, string>` (waits as long as needed — the fiber PARKS, the process sleeps) · `next_event_timeout(h, ms) -> Result<Option<WatchEvent>, string>` (`None` = deadline elapsed; useful for coalescing bursts) · `WatchEvent { kind, path }` with kind `"create"`/`"modify"`/`"remove"`/`"rename"`/`"other"` — kinds can be coarse depending on the platform: treat the event as "something changed here" and re-examine · `copy_all(src, dst) -> Result<int, string>` copies the TREE (creates `dst`, overwrites, recurses; returns files copied) — the mirror of `remove_all`; a single file is `copy_file` · `symlink(target, link) -> Result<int, string>` (`ln -s target link`; `target` may be relative to the link and need not exist; `stat(link).kind` is `"symlink"`, `real_path(link)` follows it) |
| `std/io` | the console by bytes. Writing **without a newline**: `write(s)` / `ewrite(s)` (stderr) / `write_bytes(b)` → `Result<int, string>` (number of characters/bytes) · `flush() -> Result<int, string>`. stdout is buffered: after a `write` without `\n`, call `flush()` to see it; `ewrite` is visible at once; `write_bytes` does not go through UTF-8. Reading: `read(max) -> Option<bytes>` (1..=max octets; `None` = EOF) · `read_timeout(max, timeout_ms) -> ReadResult` (`Data(bytes)` \| `Eof` \| `TimedOut`; `0` = pure poll). On the VM a read without data **parks the fiber**, not the VM; a single stdin reader at a time; do not mix with `input()`/`fs.read_line` on stdin (those read buffered, this reads the raw fd). The order with respect to `print`/`eprint` is program order |
| `std/term` | the terminal. `is_tty(fd) -> bool` (0/1/2) · `size() -> Option<(int, int)>` (cols, rows) · `raw<T>(f: fn() -> T) -> Result<T, string>` — runs `f` in **raw** mode (no echo, byte by byte, no signals) and ALWAYS restores (also if `f` fails, and at process exit via `atexit`; a fatal signal/`kill -9` leaves the terminal raw → `reset`) · `read_key() -> Option<Key>` (one key; `None` = EOF; a lone ESC resolves after 25 ms) · `decode(b: bytes) -> Option<(Key, int)>` — the **pure** decoder (key + octets consumed; `None` = incomplete prefix), to process bursts or test without a tty · `enum Key { Char(char) Enter Tab Backspace Esc Up Down Left Right Home End PageUp PageDown Insert Delete Ctrl(char) F(int) }`. In raw mode there is no OPOST: end lines with an explicit `\r\n`. **Cell width** (portable — needs no tty): `width(s: string) -> int` (terminal cells, not characters) · `char_width(c: char) -> int` (pragmatic wcwidth: control/combining → 0, CJK/kana/fullwidth/emoji → 2, the rest → 1) · `fit(s, cells) -> string` (truncates to `cells` without splitting a wide character and pads with spaces; left) · `fit_right(s, cells) -> string` (pads on the left, for numeric columns). **Hidden input**: `read_hidden(prompt) -> Result<string, string>` — one line WITHOUT echo (passphrases; prompt to stderr like getpass(3), Backspace deletes a whole UTF-8 character, Ctrl-C = `Err("interrupted")`, no tty = `Err`) · the **pure** core `hidden_feed(acc: bytes, chunk: bytes) -> Hidden` (`More(bytes) Done(string) Cancelled`), testable without a tty. **Graphical terminal**: `size_px() -> Option<(int, int)>` (area in PIXELS via `ws_xpixel`/`ws_ypixel`; `None` if the terminal does not report them — many leave 0) · `cell_px() -> Option<(int, int)>` (pixels of ONE cell = area/grid — to scale sixel/kitty graphics to the layout) · `capabilities() -> Capabilities { truecolor, colors_256, sixel, kitty_graphics }` (with stdin AND stdout on a tty it asks the terminal ITSELF — a DA1 query for sixel + the kitty graphics APC probe, one raw session, ~150 ms deadline; under tmux the probe yields `false`, correctly: APCs do not pass; without a tty, kitty falls back to the `TERM`/`KITTY_WINDOW_ID` env hint; everything undetectable is `false`: degrade, never guess upwards) · `parse_device_attributes(resp: bytes) -> [int]` — the **pure** DA1 parser (`ESC [ ? 64;1;4 c` → `[64, 1, 4]`; malformed → `[]`) · `parse_graphics_reply(resp: bytes) -> bool` — the **pure** parser of the kitty probe (`;OK` → true). **Kitty graphics** (kitty/Ghostty/WezTerm; ids `> 0` chosen by the caller, stable; 1-based cells; `q=2` silent and without moving the cursor; they EMIT even without a tty — check `capabilities().kitty_graphics` first; under tmux nothing is drawn): `transmit_image(id, img: image.Image) -> Result<int, string>` (uploads the pixels WITHOUT showing — once per sprite) · `place_image(id, col, row, cols, rows) -> Result<int, string>` (shows what was transmitted, ~30 octets per frame; `cols`/`rows` scale to cells, `0` = natural size) · `draw_image(id, col, row, img) -> Result<int, string>` (transmit+show, the convenience) · `draw_png(id, col, row, data: bytes) -> Result<int, string>` (the terminal decodes the PNG — compressed bytes, for assets) · `clear_image(id)` (removes from screen; the terminal KEEPS the pixels: `place_image` without retransmitting) · `clear_images()` · `kitty_chunks(control: string, payload: bytes) -> string` — the **pure** brick: assembles any APC command of the protocol with the mandated chunking (4096 base64 chars/chunk), for what is not covered (animation, z-index) |
| `std/net` | `tcp_connect tcp_listen tcp_accept local_port` · `tcp_connect_timeout(host, port, ms)` (an attempt that exhausts the deadline fails with the stable error `"connect timeout"` instead of the OS's ~75 s against a host that drops SYNs; on the VM and the native fiber scheduler the wait PARKS the fiber — the dial runs on a helper thread —; on the interpreter and thread-per-task native it blocks only its own thread) · `socket_read socket_write socket_read_bytes socket_write_bytes` · `shutdown_write(h) -> Result<int, string>` (half-close `SHUT_WR` — the peer sees EOF, this side keeps reading; the netcat/HTTP-1.0 idiom; TCP only) · `peer_addr(h) -> Result<string, string>` (the peer's address `"ip:port"` of a TCP/TLS connection; IPv6 in brackets) · `set_read_timeout(h, ms)` (a read that waits longer fails with the stable error `"read timeout"`; applies to TCP, TLS **and UDP**, and also to a listener's `tcp_accept`) · `set_nodelay(h, on)` (`TCP_NODELAY` — no Nagle, small writes go out immediately; TCP and TLS; total) · `set_keepalive(h, on)` (`SO_KEEPALIVE` — the OS probes an idle connection and reports it dead when the peer vanished; OS default timing; TCP and TLS; total) · `tls_connect tls_connect_h2 tls_accept tls_upgrade` (STARTTLS) · `tls_peer_cert(h) -> Result<PeerCert, string>` (the peer's certificate — `PeerCert { subject, issuer, not_before_ms, not_after_ms, san }`; "expires in N days" = `(not_after_ms - time.now()) / 86400000`; drives the pending handshake, bounded to 10 s) — all `Result`; + `close(h)` |
| `std/process` | OS processes, **without a shell** (typed argv). `run(program, args) -> Result<Output, string>` · `cmd(program, args) -> Cmd` + chainable builder `.dir .env .env_clear .stdin(bytes)` (written and CLOSED; without it, the child reads `/dev/null`) `.timeout_ms .max_output .merge_output .run()`. `Err` = only "could not launch"; exiting ≠ 0 or dying by signal is `Ok`. `Output { exit: Exit, stdout: bytes, stderr: bytes, timed_out, truncated }` with `Exit.Code(int)` \| `Exit.Signal(int)` (never `128+sig`). The timeout returns the PARTIAL Output with `timed_out` after killing the child's GROUP; `truncated` marks the capture cap (~16 MB by default). **Streaming** (VM/native): `.stream() -> Result<Proc, string>` with `Proc { out, err: Channel<bytes>, … }` (BOUNDED channels = backpressure; their close = end of stream; with merge, `err` is born closed) · `Proc.wait() -> Exit` (reaps; once) · `Proc.kill(force)` (signal to the GROUP; no-op after wait) · `Proc.pid() -> int` (the OS pid; `-1` after `wait`) and `Proc.hangup()` (`SIGHUP` to the group — what a terminal emulator sends when its window closes: an interactive `bash` ignores SIGTERM but exits on SIGHUP and forwards it to its jobs; Windows: closes the pseudoconsole, and a child without a pty gets `CTRL_BREAK`; no-op after `wait`). The pty master and slave are born with `FD_CLOEXEC`: no later child inherits another one's terminal. **Persistent session**: `.stdin_pipe()` leaves the child's stdin OPEN and `Proc.write(bytes) -> Result<int, string>` / `Proc.close_stdin()` feed it while it lives — what an MCP/LSP client or a REPL driver needs (write request → read reply → repeat). `write` writes ALL the data and **parks the fiber** if the pipe fills (backpressure); a child that closed its stdin or died yields `Err` (a visible EPIPE, not silence); `close_stdin` IS the EOF the child waits for. It takes precedence over `.stdin(data)`. The process is a SCOPE CHILD: a failing sibling kills and reaps it, and one without `wait()` does not outlive its scope. `stream()` has no `timeout_ms`/`max_output` (the channel is the cap; the deadline composes with deadline + kill). macOS, Linux and Windows · **pseudo-terminal**: `cmd(...).pty(cols, rows).stream()` runs the child under a PTY (Linux/macOS: `setsid` + controlling terminal; `TERM`/`COLORTERM` when missing) — `out` carries the terminal stream with its VT sequences, `err` starts closed, `write` sends keys (Ctrl-C = `0x03`, the line discipline handles it), `Proc.resize(cols, rows)` → `SIGWINCH`; `pty(0, …)` is an `Err`; Windows: ConPTY — `CreatePseudoConsole` + `CreateProcessW` with `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`; the output carries VT; `out` closes when the child exits (a watcher closes the pseudoconsole) · `Cmd.spawn_detached() -> Result<int, string>` launches and forgets — the child **outlives the parent** (own session via `setsid` / `DETACHED_PROCESS` with no Job Object, stdio to the null device, not tied to any `scope`; returns the pid) · `self_command() -> [string]`: the command line that reproduces this program, honest with the engine (`[exe]` for a native binary or bundle; `[ray, "run", entry]` under `ray run`/`ray dev`, with `--interp` when in use) — `cmd(c[0], rest).spawn_detached()` relaunches the app |
| `std/time` | `now monotonic monotonic_millis monotonic_nanos sleep` (`monotonic` is in **ms**; `monotonic_millis` is its unit-named alias) + civil UTC dates: `DateTime`, `now_utc`, `from_epoch_millis`/`to_epoch_millis`, `to_iso8601[_basic]`, `date_stamp`, `to_rfc1123`, `parse_iso8601[_millis]` (RFC 3339 with offset/fraction; also the basic forms `YYYYMMDD`/`YYYYMMDDTHHMMSSZ` that `date_stamp`/`to_iso8601_basic` produce, and `YYYY-MM-DD` = midnight UTC), `format_duration` (`net/time` remains as a re-export) · duration constructors → **ms** (the stdlib's currency): `millis seconds minutes hours days` — imported unqualified they enable the UFCS form `30.seconds()`, `2.hours()` |
| `std/units` | size constructors → **bytes**, binary convention (1 KB = 1024): `kb mb gb` — imported unqualified they enable the UFCS form `64.kb()`, `16.mb()` |
| `std/random` | `next() -> float` (in `[0,1)`) · `below(n) -> int` · `between(lo, hi) -> int` · `choice(xs) -> Option<T>` · `shuffle(xs)` (shuffles **in place**, returns unit) · `seed(n)` (reproducible sequence) |
| `std/crypto` | **production** crypto (backed by `ring`, constant time): `sha256 sha512 sha1` (`bytes -> bytes`; `sha1` is legacy, only for protocols that require it) · **incremental hasher** (for large files in chunks): `sha256_init()`/`sha512_init() -> Result<int, string>` + `hash_update(h, chunk) -> Result<int, string>` + `hash_final(h) -> Result<bytes, string>` (`final` CONSUMES the handle; digest identical to the one-shot) · `hmac_sha256(key, msg) -> bytes` · **HMAC with another hash** (M347): `hmac_sha1` (TOTP/HOTP), `hmac_sha384`, `hmac_sha512` (`(key, msg) -> bytes`) and `hmac(alg, key, msg) -> Option<bytes>` (`"sha1"`/`"sha256"`/`"sha384"`/`"sha512"`; `None` otherwise) · **public-key** (M347, on ring; private keys as PKCS#8 DER — `std/pem` extracts them from a PEM): **ECDSA P-256** `p256_generate() -> Result<bytes, string>` (PKCS#8) · `p256_public_key(pkcs8) -> Result<bytes, string>` (uncompressed SEC1, 65 bytes) · `p256_sign(pkcs8, msg) -> Result<bytes, string>` (64 bytes `r‖s`, ES256) · `p256_verify(pub, msg, sig) -> bool` / `p256_verify_asn1(pub, msg, sig_der) -> bool` (DER signature: WebAuthn, OpenSSL); **RSA** (2048–8192 bits; ring does NOT generate RSA keys: import them) `rsa_public_key_of(pkcs8) -> Result<bytes, string>` (PKCS#1 `RSAPublicKey` DER) · `rsa_pkcs1_sign(pkcs8, msg)` (RS256) / `rsa_pss_sign(pkcs8, msg)` (PS256) `-> Result<bytes, string>` · `rsa_pkcs1_verify(pub, msg, sig)` / `rsa_pss_verify(pub, msg, sig) -> bool` · `rsa_public_key(n, e) -> bytes` (the DER from a JWK's magnitudes) / `rsa_public_components(der) -> Result<(bytes, bytes), string>` (to publish a JWKS) · `ed25519_public_key(seed)` / `ed25519_sign(seed, msg)` → `Option<bytes>` (`None` if the seed is not 32 octets) and `ed25519_verify(pubkey, msg, sig) -> bool` (total) · `chacha20poly1305_seal(key, nonce, aad, plain)` → `Option<bytes>` (`ciphertext ‖ tag`) and `chacha20poly1305_open(…)` (`None` if authentication fails) · `random_bytes(n)` (CSPRNG) · **key agreement** (backed by `x25519-dalek`): `x25519_public_key(secret)` / `x25519_shared_secret(secret, peer_public)` → `Option<bytes>` (`None` if either key is not 32 octets, or if the peer's public key has small order — all-zero output) · `hkdf_sha256(salt, ikm, info, len)` → `Option<bytes>` (RFC 5869; `None` outside `1..=8160`; a different `info` → an independent key) · **passwords**: `pbkdf2_hmac_sha256(password, salt, iterations, len)` → `Option<bytes>` (RFC 8018; `None` with `iterations` outside `1..=4294967295` or `len` outside `1..=1024`) · `password_hash(p) -> string` (`$pbkdf2-sha256$<iter>$<salt hex>$<key hex>`, 16-byte CSPRNG salt, `PASSWORD_ITERATIONS` = 600000) / `password_hash_with(p, iterations)` / `password_verify(p, encoded) -> bool` (constant time; `false` also for a malformed `encoded`) · `constant_time_eq(a, b) -> bool` (total). The secret of `x25519_shared_secret` is the **raw** DH: always pass it through `hkdf_sha256` before using it as an AEAD key. The pure-raylang versions in `examples/web/` are demonstrations, not production |
| `std/crypto/md5` `std/crypto/aes` `std/crypto/des` | **LEGACY crypto in pure raylang**: for talking to protocols that require it, not for designing (not constant time; for anything new, `std/crypto`). `md5.md5(bytes) -> bytes` (RFC 1321) · `aes.encrypt_block/decrypt_block(key, block)`, `encrypt_ecb/decrypt_ecb(key, data)`, `encrypt_cbc/decrypt_cbc(key, iv, data)` (keys of 16/24/32 octets = AES-128/192/256; whole blocks, no padding) · `des.encrypt_block/decrypt_block`, `encrypt_ecb/decrypt_ecb`, `encrypt_cbc/decrypt_cbc` (8-octet key) and **3DES** `tdes_encrypt_block/tdes_decrypt_block`, `tdes_encrypt_ecb/tdes_decrypt_ecb`, `tdes_encrypt_cbc/tdes_decrypt_cbc` (24-octet key = K1 K2 K3, or 16 = K3 K1). All `-> Result<bytes, string>`; official vectors in `tests/crypto_legacy_cli.rs` |
| `std/bigint` | **unsigned big integers** on big-endian `bytes` (`num-bigint` runtime, feature `bigint`, `--without bigint` on native): `from_int(n) -> bytes`, `from_hex(s) -> Result<bytes,_>`, `from_bytes(b)` (normalizes), `to_hex(a) -> string`, `to_int(a) -> Option<int>`, `bit_len`, `is_zero`, `cmp(a, b) -> int` · arithmetic `add sub mul div rem gcd shl shr` and **`modpow(base, exp, m)`**, `modinv(a, m)` (all `-> Result<bytes, string>`: division by zero, negative subtraction, no inverse). A 4096-bit `modpow` costs milliseconds (before, seconds in pure raylang). **Not constant time**: client-side DH/RSA yes; a server's private key under a timing attack no |
| `std/keychain` | **secrets in the system keychain** (Keychain Services on macOS, Secret Service/libsecret via dlopen on Linux, Credential Manager on Windows; zero crates): `get(service, account) -> Result<Option<string>, string>` · `set(service, account, secret) -> Result<int, string>` (creates or replaces) · `delete(service, account) -> Result<bool, string>` (`true` if there was one). Secrets are UTF-8 text (tokens, API keys); `service`/`account` non-empty and free of NUL/tab/newline. `Err` when there is no keychain (no Secret Service daemon, a slim build) or the system denies access. **`RAY_KEYCHAIN_FILE=<path>`** = a plain 0600 file as the keychain (tests/CI, never production). `--without keychain` natively → `Err` |
| `std/resilience` | the resilience kit for services: `Retry`/`policy(attempts, base_ms, max_ms)` + `retry<T,E>(p, f)` (exponential backoff + jitter; returns the first `Ok` or the last `Err`) · `Breaker`/`breaker(threshold, cooldown_ms)` + `guard<T,E>(b, err_open, f)` (fail-fast circuit breaker; the open-circuit error is supplied by the caller) + `is_open` · the composable pair `admit(b) -> bool` / `report(b, ok)` — the loose transitions for when the call runs in another fiber/actor (`guard` is sugar over them) · `Deadline`/`deadline(ms)` + `remaining expired` (monotonic time budget; apply it to I/O with `net.set_read_timeout(h, remaining(d))`) |
| `std/collections/set` | `Set<T>` (requires `T: Hash + Eq`): `new add has remove size items` → `set.new()`, `set.add(s, x)`… |
| `std/collections/deque` | `Deque<T>`: `new len is_empty push_back push_front pop_front pop_back peek_front` · `peek_back` · `get(d, i) -> Option<T>` (0 = front) · `to_array(d) -> [T]` (copy, front→back) · `iter(d) -> Iter<T>` (lazy snapshot: `for x in deque.iter(d)`) |
| `std/collections/stringbuilder` | `StringBuilder`: `new push build count` (joins once; avoids the O(n²) of `+` in a loop) |
| `std/collections/dict` | `Dict<K, V>` — a GENERIC hash map: USER keys via the `Hash` + `Eq` traits (the builtin `Map<K,V>` requires primitive keys). `new insert get has remove size keys values` (module functions: `dict.insert(d, k, v)`) |
| `std/kv` | `Store` — persisted key/value state: `open(path) -> Result<Store, _>`/`empty(path)`, and the operations are METHODS of the `StoreOps` trait — `s.get(k) -> Option<bytes>` · `s.set(k, v: bytes)` · `s.get_string(k) -> Option<string>` · `s.set_string(k, v)` · `s.delete(k) -> bool` · `s.keys() -> [string]` · `s.save() -> Result<int, _>` (atomic save: temp + rename) · **`s.incr(k, delta) -> Result<int, _>`** (atomic add; absent = 0; a decimal UTF-8 value, readable with `get_string`; non-integer = Err) · **`s.set_if(k, expected: Option<bytes>, new) -> bool`** (CAS; `None` = only-if-absent) — NOT free functions (`kv.get(s, k)` does not exist). `share`/`open_shared`/`stop` = the ACTOR form for access across fibers (same methods on the handle; `incr`/`set_if` run WHOLE in the owning fiber — atomic under concurrency). Motivated by `ray dev` (sessions/config surviving reloads) |
| `std/json` | `enum Json` (`JNull JBool JNum JStr JArray JObject`) · `parse -> Result<Json, string>` · `stringify` (canonical, sorted keys). `\uXXXX` escapes with surrogate pairs · **`parse_relaxed(s)`** accepts `//` `/* */` comments and trailing commas (the dialect of `tsconfig.json`, `.vscode/*.json`, `.sublime-settings`); `parse` stays strict · **bounded depth**: `parse`/`parse_relaxed` return `Err("nesting too deep …")` beyond `max_depth()` (200) levels of arrays/objects — never a stack overflow (on native it used to abort the process) · `Json` implements `ToJson` → `obj().field("d", Json.JNull)` / `.field("x", parse(s)?)` embed a dynamic value in the builder · the builder (`obj()…render()`, `render_arr`) and `@derive(ToJson)` write the same compact form as `stringify` (`{"k":1,"a":[1,2]}`) |
| `std/hex` | `hex_encode(bytes) -> string` · `hex_decode(string) -> Result<bytes, string>` |
| `std/base64` | `base64 base64url` (`bytes -> string`) · `base64_decode base64url_decode` (`string -> Result<bytes, string>`) |
| `std/url` | `url_encode url_decode parse_query build_query` |
| `std/regex` | Thompson NFA engine (linear time): `full_match search find find_str find_all replace_all`. Supports `. * + ? \| ( ) [a-z] [^…] \d \w \s ^ $`, `(?:…)`, lazy `*?`, `{n,m}` · **Not supported, and `compile` rejects with a named `Err`**: look-around `(?=` `(?!` `(?<=` `(?<!`, atomic groups `(?>`, inline flags `(?i)`, `\p{…}`, backreferences `\1`/`\k<n>`, `\G` `\A` `\z`, `\h`, possessive quantifiers `*+`, POSIX classes `[:alpha:]` and `&&` (any other escape `\X` is the literal character `X`; `\b` is the letter b) · compiled: `compile -> Result<Regex, string>` + trait `Matcher` (`captures`/`captures_str`) · NAMED groups `(?P<n>…)`/`(?<n>…)`: `group_names(re) -> [string]` · `captures_map(re, s) -> Option<Map<string, string>>` · **ONIGURUMA dialect** (native `fancy-regex` engine on all three engines; the one `.sublime-syntax`/Ruby/TextMate use): `onig(pattern) -> Result<Onig, string>` accepts look-around, backreferences `\1`/`\k<n>`, `\G`, atomic groups `(?>`, possessive quantifiers, `\p{…}`, `\h` (hex digit), inline flags `(?i)`/`(?x)`, `&&`; `^`/`$` are LINE anchors and `.` stops at `\n` unless `(?m)`/`(?s)` · `Onig { pattern, names: [string] }` (handle deduplicated by pattern, lives for the process) · trait `Scanner`: `search_from(text, from) -> Option<Match>` (`\G` = `from`) · `match_at(text, at) -> Option<Match>` (anchored: the match STARTS at `at`) · `is_match(text) -> bool` · `Match { start, end, groups: [Option<(int, int)>] }` (character indices; `groups[0]` = whole match) · `group_str(m, text, i) -> Option<string>` · `group_index(re, name) -> Option<int>` · a catastrophic pattern does not hang: panic `regex: backtrack limit exceeded …` past 10⁶ steps · native binaries need the `regex` feature (with `--without regex`, `onig` falls into the stub) |
| `std/csv` | `parse_csv -> Result<[[string]], string>` (RFC 4180) · `write_csv` · incremental: `parser()` + `feed(p, chunk) -> [[string]]` (completed rows) + `finish(p) -> Result<[[string]], string>` (the tail; an unclosed quote = Err) — chunks may be cut anywhere |
| `std/toml` | `parse_toml toml_get toml_show` (subset: tables, scalars, arrays) · `[[path]]` arrays of tables: flattened as `path.N.key` + `toml_array_len(entries, path)` |
| `std/template` | Jinja-style templates: `compile(tpl) -> Result<Template, _>` + `render(t, ctx)` (SSR: compile once) · `render_template` (one-shot) · `{{ var }}` (autoescape), `{{& var }}`, `{% if/elif/else %}`, `{% for %}` · context: `ctx_str ctx_int ctx_bool ctx_list val_str val_int` · `escape_html` · COMPILED templates: importable `.ray.html`, compiled in memory (§14) |
| `std/markdown` | Markdown → typed AST + HTML (**CommonMark + GFM**, checked against the cmark-gfm spec — `tests/corpus/gfm_spec.json`, 595/672 examples, ratchet in CI; the rest is raw HTML, escaped on purpose). `parse(md) -> [Block]` · `render(blocks) -> string` · `to_html(md) -> string` · `parse_inline(s) -> [Inline]` · `normalize_label(s)`. `enum Block { Heading(int, [Inline]) Paragraph Plain (TIGHT-list paragraph, no <p>) Code(lang, text) Quote([Block]) Alert(kind, [Block]) List(ordered, start, [[Block]]) Rule Table(aligns, header, rows) FootnoteDef(label, [Block]) }` · `enum Inline { Text Code Emph Strong Link([Inline], href, title) Image(alt, src, title) Strike HardBreak Check(bool) FootnoteRef(label) }` (title is `""` when absent). **Inlines**: delimiter-stack emphasis (`*`/`_` with flanking — `snake_case` stays literal —, `~~strike~~`), code spans by backtick runs, `\` escapes, HTML5 and numeric entities (`&ouml;` `&#35;` `&#x1F;`), inline links and images `[t](url "title")`, reference links `[t][ref]`/`[ref][]`/`[ref]`, autolinks `<https://…>`/`<a@b.c>` plus the GFM extended ones (`www.`, `http(s)://`, bare emails), hard breaks (`\` or two trailing spaces), percent-encoded URLs like cmark. Blocks: ATX and **setext** headings, fenced (```/~~~, fence indent) and **indented** code, block quotes and **alerts** `> [!NOTE]`/TIP/IMPORTANT/WARNING/CAUTION, lists with the real indentation rule (marker + spaces), **tight/loose**, **task items** `[ ]`/`[x]`, lazy continuation, thematic breaks, GFM tables (delimiter row under a one-line paragraph), **link reference definitions** `[x]: url "t"` (collected for links), **footnotes** `[^x]: …` (at the end, `<section class="footnotes">`). Output HTML follows the spec conventions (`<hr />`, `<img … />`, `<li>` hugging tight items, table cells on their own lines). Deliberately out: HTML blocks (HTML is escaped, security policy). |
| `std/audio` | **PCM audio output**: `open(sample_rate, channels) -> Result<int, string>` (default device; interleaved s16le; 8000–192000 Hz, 1–8 channels) · `write(h, samples) -> Result<int, string>` — writes EVERYTHING and **parks the fiber** if the device is full: backpressure IS the pacing (synthesize as fast as you can; the device sets the tempo) · `drain(h)` (waits for what was written to play — before `close(h)` for a clean ending) · **`open_latency(rate, channels, latency_ms)`** (the hint sizes ring/buffers/chunk and the queue between the program and the device — `write` parks with ~the latency queued IN TOTAL — split socket ½ / ring ¼ / device buffers ¼, before every stage got the whole latency and ~3× was queued — whatever the rate, with ~2 KiB floors per stage at low rates; 20–1000 ms, 0 = default 200; on Android ≤50 ms requests LOW_LATENCY) · **`played_ms(h) -> Result<int, _>`** (the REAL playback position — AudioQueueGetCurrentTime / snd_pcm_delay / AAudioStream_getFramesRead, refreshed ~latency/4 — to sync visuals) · `close(h)` (the generic one) ends the output. Backends: AudioQueue (macOS and iOS — with `[ios] background_audio` to keep playing in the background), ALSA via dlopen (Linux; without libasound → a clear `Err`), **AAudio via dlopen (Android)**, `RAY_AUDIO_SINK=null` = real-time sink (tests/CI without a sound card). `--without audio` excludes it |
| `std/ui` | **window + webview** (the desktop-app primitive): `open(title, url, width, height) -> Result<int, string>` — a native window with the SYSTEM webview loading `url`; **`open_with(title, url, WindowOptions)`** adds `min_width`/`min_height` (0 = no minimum; larger than the window = `Err`), `resizable`, `center` and `autosave` (a name under which the SYSTEM remembers size and position between launches — NSWindow's `frameAutosaveName`; Linux/Windows have no equivalent and ignore it) and **`titlebar_color`** (a `#rrggbb` that paints the system title bar with the app's theme color, Sublime-style — macOS: transparent title bar over the window background, light or dark title by luminance; Windows 11: the caption color; Linux ignores it; `""` = system title bar; any other form → `Err`), **`background`** (the `#rrggbb` the window shows UNTIL the page paints — no white/grey flash while WebKit parses the app; macOS: `underPageBackgroundColor` + window background; Linux: `webkit_web_view_set_background_color`; Windows: WebView2's `DefaultBackgroundColor` + HWND erase; `""` = `titlebar_color` when set, otherwise the system's) and **`minimizable`** (`false` disables the minimize button — the yellow one on macOS, `WS_MINIMIZEBOX` on Windows; GTK ignores it — for secondary panels such as an About); `options(width, height) -> WindowOptions` gives `open`'s defaults (no minimum, resizable, centered, no autosave, system title bar, minimizable) to tweak fields (small app: the built-in IPC bridge — `window.ray.send(text)` → a `"message"` event with `tag`=text and `window`=handle (0 on iOS); non-strings travel as JSON and `request(v)` returns a Promise the program resolves with `as_request(e) -> Option<(int, string)>` + `reply(window, id, value)` — on top of the existing eval_js; the JS literal is escaped natively in one pass (1 MB ≈ 4 ms round trip, was 16 s) and **`reply_json(window, id, json)`** delivers `JSON.parse(json)`: the page receives an object, not a string — and `eval_js` back; with a backend: your embedded webserver on `127.0.0.1` and the web framework as IPC) · **the `ray://app/…` scheme**: the page loads its files straight from the process, with no local TCP server (no port another app could reach, no local-network permission in the bundle), streamed in chunks with `Range`/206, `ETag`/304 and MIME by extension; on the three desktop backends and, since 1.27.17, in the iOS/Android shells of `ray bundle` (Android loads it through the alias `https://app.ray.invalid/…`; a shell generated with an older raylang makes `open` fail with the remedy) — `mount_dir(prefix, dir) -> Result<int,_>` serves `ray://app/<prefix>/<rel>` from disk (canonical path, `..` never escapes the directory, a directory serves its `index.html`), `mount_bytes(path, bytes)` an in-memory file (it WINS over a directory mounted on the same path — override or add files under a tree), `mount_embed(prefix, embed_prefix) -> Result<int,_>` the `std/embed` assets (returns how many; under `ray run`/`ray dev` it mounts the DIRECTORY live, natively the baked bytes; keeps the key whole: `assets/app.css` → `ray://app/assets/app.css`), `mount_embed_at(prefix, embed_prefix) -> Result<int,_>` the same but STRIPPING `embed_prefix` (`mount_embed_at("", "frontend/dist")` puts `index.html` at `ray://app/index.html`, what `app://index.html` expects) · **live-reload**: under `ray dev` the runtime subscribes to the hub and reloads every window when `reload` is emitted (embedded asset change: no restart; `.ray` change: restart and a fresh window); set by the toolchain, a native binary never looks; mount before the page asks; GET/HEAD only, anything not mounted is 404; `open("App", "ray://app/index.html", …)` needs no server at all; backends: macOS (WKURLSchemeHandler, chunked streaming), Linux (`webkit_web_context_register_uri_scheme` via dlopen; with WebKitGTK < 2.36 no status/headers: body+MIME only, no Range or 304) and Windows (scheme registered in the environment options + `WebResourceRequested`, WebView2 ≥ 112; the requested range is answered from memory, no streaming) · **`focus(h) -> Result<int,_>`**: brings an already open window to the front and focuses it (`makeKeyAndOrderFront` + app activation on macOS, `gtk_window_present` on GTK, restore + `SetForegroundWindow` on Windows) — `eval_js(h, "window.focus()")` only focuses the document, not the native window · `eval_js(h, js)` (fire-and-forget) · events (a per-process queue; kinds: `"closed"` — exactly one per window —, `"menu"`, `"message"`; headless injects messages with `RAY_UI_MSG`): `next_event() -> Result<UiEvent, string>` (the fiber parks) · `next_event_timeout(ms) -> Result<Option<UiEvent>, string>` · `events() -> Channel<UiEvent>` (pump fiber; VM/native) · `split_events() -> (Channel<UiEvent>, Channel<UiEvent>)` (`(messages, other)` — `"message"` events on the first, the rest on the second; ONE pump fiber: a single consumer like events(), call it once; the queue has a **hard cap** of 65536 — when full the oldest `"message"` is dropped, never a `"closed"`, with a stderr warning; only the main frame reaches the bridge on macOS/iOS) · `close(h)` (the generic one) closes the window · **menus**: the standard App/Edit menu installs ITSELF (⌘Q/⌘W and the webview's clipboard/undo — without it, ⌘C/⌘V do not travel on macOS); `menu(title, [MenuItem{tag,title,shortcut}]) -> Result<int,_>` adds custom menus (click → a `"menu"` event with `tag`; shortcut = one character ⌘+key on macOS, uppercase adds ⇧; Linux v1 click-only and the menubar is per-window: it applies to windows opened afterwards) · `app_menu(name, [MenuItem]) -> Result<int,_>` puts items in macOS's **APPLICATION menu** (above Hide/Quit, with a separator) and a non-empty `name` retitles it (under `ray run` it read "ray"); the `"role:about"` tag installs the **native About** (standard panel, no event) and `set_about(name, version, description, copyright) -> Result<int,_>` declares its content — name in bold, a "Version …" line, description (as credits, Finder-style) and copyright; `""` omits the field (the bundle's value remains: `ray bundle` sets name/version/icon from ray.toml and the copyright from `[app] copyright`); on Linux they go as a normal menu titled `name` and ALL items emit the `"menu"` event (the program shows its own about) · **file dialogs**: `pick_file()` / `pick_folder()` / `save_file(suggested)` -> `Result<Option<string>,_>` (None = cancelled; MODAL — one modal at a time; headless drives them with `RAY_UI_PICK`). No `ui.run()`: the runtime captures the main thread by itself. Backends: AppKit/WKWebView (macOS), GTK3+WebKitGTK (Linux, via dlopen — without the libs or a display → a clear `Err`), Win32+WebView2 (Windows; without the WebView2 Runtime → a clear `Err`; per-window menus as on Linux, no system `app_menu`: its items go as a normal menu; `shortcut`s are real `Ctrl+X` accelerators — uppercase = `Ctrl+Shift+X` — and dialogs are modal to the window), **devtools**: the webview inspector ("Inspect Element" context menu/F12; on macOS also Safari's Develop menu via `inspectable`) is decided by the **build or the toolchain, never the environment**: on under `ray dev` and with `ray run --devtools`; in a native binary only when built with `ray build --native --devtools` / `ray bundle --devtools` (without the flag the call does not exist in the program; Windows shipped it on and now it is off). Mobile: `ray bundle --ios|--android --devtools` makes the shell inspectable from the desktop (Safari Develop / chrome://inspect), never without the flag. `RAY_UI_BACKEND=headless` = in-memory windows (tests/CI, any OS; `RAY_UI_TRACE=1` writes every `open`/`eval_js`/`reply` to stderr — `[ui] eval <window> <js>` — and `RAY_UI_EXIT_AFTER_MS=N` ends the process with 0 after N ms without events). `--without ui` excludes it · **desktop and clipboard**: `open_path(path)` (the associated app: `open`/`xdg-open`/Windows handler, no shell) · `reveal(path)` (Finder `-R` / Explorer `/select,` / `FileManager1.ShowItems` with `xdg-open` of the parent as fallback); both `Err` when the path does not exist · `clipboard_write(text)` / `clipboard_read() -> Result<string,_>` (NSPasteboard / GtkClipboard / CF_UNICODETEXT; no window needed on macOS and Windows, Linux needs a graphical session; headless = in-process buffer) · **full menus**: `MenuItem { tag, title, shortcut, icon, enabled, checked }` with `item(tag, title, shortcut)` and `separator()` · `shortcut` takes chords (`"cmd+s"`, `"cmd+alt+s"`, `"ctrl+shift+p"`, `"f5"`, `"cmd+enter"`; `cmd` = Command on macOS and Ctrl on Linux/Windows; a single character is that key with the platform modifier; an unknown chord is an `Err`; GTK stays click-only) · `icon`: an image path or `"sf:<symbol>"` on macOS (Windows `.bmp` only; GTK `GtkImageMenuItem` when the lib has it) · initial `enabled`/`checked` and `set_menu_item(tag, enabled, checked)` afterwards (Err for an unknown tag) · `menu_at(position, title, items)`: position in the bar (macOS: after the app menu, 0 = before the standard Edit; Linux/Windows: from the first; -1/out of range = at the end) · **`set_titlebar_color(h, color)`**: the title bar of an OPEN window, live — `"#rrggbb"` tints it like `WindowOptions.titlebar_color`, `""` restores the system bar (macOS: transparency + background + appearance by luminance; Windows 11: `DWMWA_CAPTION_COLOR`/`DWMWA_COLOR_DEFAULT`; Linux ignores it); `Err` for a malformed color or a closed window · **`set_background(h, color)`**: the background under the page of an OPEN window, like `WindowOptions.background`; `""` = the system's; on macOS only the webview (the window background, which shows under the transparent bar, belongs to `set_titlebar_color`) · `replace_menu(title, items) -> Result<int, string>`: replaces the contents of an existing top-level menu (titles, shortcuts, items) — the old tags stop existing and the new ones are live for `set_menu_item`; immediate on macOS, rebuilds the bars of the open windows on Linux/Windows; `Err` if no menu has that title · **standard edit roles**: an item whose tag is `"role:undo"`, `"role:redo"`, `"role:cut"`, `"role:copy"`, `"role:paste"`, `"role:select_all"` or `"role:close"` behaves like the system's own (undo/clipboard/select-all on the focused webview; close the key window) and emits NO event; an empty title or shortcut takes the standard one (`"Paste"`, `"cmd+v"`), your own renames or rebinds it; `set_menu_item` works by tag as usual; backends: macOS responder-chain selector, Linux `webkit_web_view_execute_editing_command`, Windows DevTools Protocol `Input.dispatchKeyEvent` (the shortcut is shown but not registered as an accelerator: the key still reaches the webview) · `edit_menu(items) -> Result<int, string>`: the Edit menu rebuilt with your items among the roles — on macOS it replaces the standard Edit in place; on Linux/Windows (no default Edit) it creates one at the end of the bar the first time and replaces it afterwards · a `shortcut` is drawn and answers exactly as declared — macOS 12+ used to "localize" punctuation ones to the keyboard layout (`cmd+/` showed as `⌘'` on a Latin American keyboard and answered the physical US `/` key); items turn that off · with the Web Inspector docked, resizing the window no longer leaves the page cut off at the top (macOS: the webview lives in a container NSView and follows its size) · **window lifecycle**: `set_title(h, title)` retitles an open window (`setTitle:` / `gtk_window_set_title` / `SetWindowTextW`) · `set_edited(h, bool)` the "unsaved changes" dot in the macOS close button (`setDocumentEdited:`; Linux/Windows ignore it) · `intercept_close(h, bool)`: the USER's close (button, ⌘W/Alt+F4, the WM) emits the **`"close_requested"`** event (`window` = h) instead of closing — the program asks/saves and closes with `close(h)`, which always closes (macOS `windowShouldClose:`, GTK `delete-event`, Windows `WM_CLOSE`) · `intercept_quit(bool)`: ⌘Q / Quit in the app menu emits **`"quit_requested"`** (`window` = 0) instead of terminating (macOS `applicationShouldTerminate:`; Linux/Windows have no app-level quit besides closing windows) · all `Err` for a closed/unknown window · **window kinds and geometry**: `WindowOptions.kind` = `"document"` (the usual one), `"panel"` (a floating utility palette above the app: macOS `NSPanel` utility + floating, Windows `WS_EX_TOOLWINDOW` + topmost, GTK UTILITY type hint + keep-above) or `"borderless"` (no frame or title; macOS via a subclass that accepts focus, movable by its background; Windows `WS_POPUP`; GTK `set_decorated(false)`) and **`"full_content"`** (the page UNDER the title bar — macOS: `NSWindowStyleMaskFullSizeContentView` + transparent bar + hidden title, traffic lights floating; the page reserves `window.ray.titlebar_height` px and marks its handle with `data-ray-drag` (`button`/`a`/`input`/`select`/`textarea` children or `data-ray-no-drag` stay clickable; double-click zooms) — the shim sends a control message that macOS resolves with `performWindowDragWithEvent:` and Windows with `WM_NCLBUTTONDOWN`/`HTCAPTION`, also for `borderless`; Linux/Windows treat `full_content` as `document`); `always_on_top` (macOS `setLevel: floating`, Windows `HWND_TOPMOST`, GTK `keep_above`); `parent` = handle of the OWNING window (0 = none; the new one stays above it and follows it: macOS `addChildWindow:`, Windows owner, GTK `transient_for`) · live, on an open window: `set_fullscreen(h, bool)` (macOS `toggleFullScreen:`, GTK `gtk_window_fullscreen`, Windows frameless over its monitor with restore), `set_always_on_top(h, bool)`, `set_size(h, w, h)` (content area), `set_position(h, x, y)` (screen coordinates, top-left origin; converted for AppKit), `center(h)`, `minimize(h)`, `maximize(h)` (macOS zoom) — all `Err` for a closed/unknown window; an unknown kind or a parent that is not open is an `Err` at open · **context menu**: `popup_menu(h, items) -> Result<int, string>` shows `items` (the same `MenuItem`s, roles included) at the mouse position over window `h`; returns once shown and the choice arrives as a `"menu"` event (`tag`, `window` = h); the tags live while the menu is open; macOS `popUpMenuPositioningItem:atLocation:inView:`, GTK `gtk_menu_popup_at_pointer`, Windows `TrackPopupMenuEx`; pattern: the page captures `contextmenu` → `window.ray.send` → `popup_menu` · **message dialogs**: `message(title, text, buttons: [string]) -> Result<int, string>` — a MODAL native alert with 1 to 3 buttons carrying your labels; returns the INDEX pressed, and dismissing the dialog (Esc, the close box) counts as the LAST button (put "Cancel" last); `message_styled(title, text, style, buttons)` with `style` `"info"`/`"warning"`/`"error"` (system icon/alert style); `alert(title, text)` (one OK) and `confirm(title, text, ok_label, cancel_label) -> Result<bool, string>`; backends: macOS `NSAlert`, Linux `GtkMessageDialog`, Windows `TaskDialogIndirect` (fallback `MessageBoxW` with system labels by button count); headless: `RAY_UI_ANSWER` drives the index · **file dialogs with options**: `FileDialogOptions { title, directory, suggested, filters: [FileFilter], multiple }` from `file_options()`, `filter(name, extensions)` (extensions without the dot) · `pick_file_with(o)` / `pick_folder_with(o)` / `save_file_with(o) -> Result<Option<string>, string>` and `pick_files(o) -> Result<[string], string>` (multiple selection; `[]` = cancelled); macOS `setAllowedFileTypes:`/`setDirectoryURL:`/`URLs`, GTK `GtkFileFilter`+`get_filenames`, Windows `SetFileTypes`/`SetFolder`/`GetResults`; headless `RAY_UI_PICK` with `\n`-separated paths · **`"focused"`** event (`window` = the window that became key/active: on open, on `focus(h)` and whenever the user brings it to the front; to know which window a menu action refers to and to refresh on regaining focus), and the `"menu"` event carries in `window` the key window at that moment (0 if none). macOS `windowDidBecomeKey:`/`keyWindow`; Linux `notify::is-active`; Windows `WM_ACTIVATE`; headless: only on `focus` (opening produces no event) · **`app_url(url) -> string`** — resolves an **`app://<path>`** URL to wherever the page lives NOW: under `ray dev` with `[frontend]` (variable `RAY_FRONTEND_URL`, exported only by the toolchain) to `<dev server url>/<path>` (Vite with HMR); in every other case (`ray run`, native, bundle) to `ray://app/<path>` (the embedded build mounted with `mount_embed`); an `http://…` URL of your own webserver also swaps its origin for the dev server's under `ray dev` (the path is kept); anything else comes back untouched. `open`/`open_with` apply `app_url` to `app://` URLs (an explicit `http://` is honoured as is) |
| `std/embed` | **project assets**: the files of `[native] embed = ["assets"]` from ray.toml, with the SAME namespace on every engine — keys with `/` relative to the root ("assets/app.css"), lexicographic order, hidden files excluded, no `..`. `read(path) -> Result<bytes, string>` · `list() -> Result<[string], string>`. On VM/interp they are read LIVE from disk (dev); `ray build --native` BAKES them into the binary (`--embed dirs` adds ad hoc) → self-contained, runs from any cwd (what a .app needs: Finder launches with cwd=/). The web framework serves them with `static_embedded` (content ETag + 304 + Range) |
| `std/inflate` | **DEFLATE/zlib/gzip — decompression** (RFC 1951/1950/1952; if you are looking for "zlib" or "unzip": this is it): `inflate_raw gunzip zlib_inflate` (→ `Result<bytes, string>`; `_limit(data, max_out)` forms with an anti-bomb cap, default 64 MiB; `max_out <= 0` is a ZERO cap, never "no cap") · `crc32`. Serves e.g. the IDAT of a PNG (`zlib_inflate`). **Incremental**: `inflate_stream()` / `zlib_stream() -> InflateStream`, `stream_push(z, chunk) -> Result<bytes, string>` (returns what the completed blocks produced; the 32 KiB LZ77 window survives across calls; cuts at any octet; sticky error), `stream_finished(z) -> bool`, `stream_set_limit(z, max_out)` — for a zlib stream that lasts the whole session (RFB's ZRLE/Zlib/Tight, WebSocket's `permessage-deflate`) `inflate_raw`/`_limit` and `crc32` use the runtime (`miniz_oxide`) when the build ships it; the raylang code remains as the fallback (and is what reports the errors of an invalid stream). |
| `std/zip` | **ZIP archive reading**: `open(bytes) -> Result<Archive, string>` (central directory; ZIP64/multi-disk → `Err`) · `entries(a) -> [Entry]` (`Entry { name, size, compressed_size, method, crc32, offset }`; `method` 0 = stored, 8 = deflate) · `find(a, name) -> Option<Entry>` · `read(a, name)` / `read_entry(a, e)` `-> Result<bytes, string>` (STORE and DEFLATE over `std/inflate`, size and CRC-32 verified; other methods → `Err`). No writing, no encryption. `.sublime-package`, `.jar`, `.docx`… are read with `fs.read_file_bytes` + `zip.open` |
| `std/update` | **app self-update**: a three-layer contract — an `update.json` manifest at a fixed URL (`app`, `version`, `notes`, `min_version`, `artifacts` per `<platform>-<arch>` with `url`/hex `sha256`/`size`) signed with Ed25519 in the sibling file `update.json.sig` (hex signature over the manifest BYTES), the public key **baked** into the app (`[app] public_key` in ray.toml → `ray bundle`/`build --native`; `ray run` reads it from ray.toml), and this library. `check(manifest_url) -> Result<Option<Release>, string>` (`None` = up to date; `Err` = does not verify, malformed, or `min_version` unmet; `check_with(url, key)` for another key) · `download(release) -> Result<Package, string>` (to a temp dir; **verifies size and sha256 before** returning) · `apply(pkg) -> Result<string, string>` over `install_root()` (macOS/Linux: extract next to the bundle and atomic `rename`, the previous one stays as `<root>.old`; Windows: files in use are renamed `*.old` and overwritten; under `ray run` → `Err`; `apply_at(pkg, root)` for an explicit directory) · `relaunch() -> Result<int, string>` (`process.self_command()` + `spawn_detached`) · `cleanup() -> int` (removes the `.old` leftovers at startup) · `current()` (baked version or `"dev"`), `app_id()`, `public_key()`, `platform_key()`, `install_root()` · helpers: `compare_versions`, `verify_signature`, `parse_manifest`, `verify_package_bytes`, `http_get(url)` (GET with up to 5 redirects over `std/net`), `to_hex`/`from_hex`. An artifact `url` without a scheme resolves against the manifest's directory (what `ray release` writes without `--base-url`). The artifact is a `.zip` with **a single top-level directory** (the `.app` or the bundle directory); paths with `..` are rejected. Policy stays with the app |
| `std/deflate` | **DEFLATE/zlib/gzip — compression**: `deflate_raw gzip_compress zlib_compress` `deflate_raw` (and `adler32`) use the runtime (`miniz_oxide`, level 6) when the build ships it; without it (slim, wasm, `--without deflate`) the raylang encoder answers the same, slower. |
| `std/pem` | **PEM** (RFC 7468, M347): `decode(text) -> Result<Block, string>` (`Block { label, der }`), `decode_all(text) -> Result<[Block], string>` (a certificate chain), `decode_label(text, label) -> Result<bytes, string>` (`"PRIVATE KEY"` = PKCS#8), `encode(label, der) -> string` (64 columns). Only the envelope; the DER goes to `std/crypto` and `net.tls_*` |
| `std/qr` | **QR codes** (M347; TOTP enrollment): `encode(text)` / `encode_level(text, "L"\|"M"\|"Q"\|"H") -> Result<Code, string>` (`Code { size, modules: [bool] }`; `Err` if it does not fit or the build is `--without qr`) · `dark(code, x, y) -> bool` · `to_text(code) -> string` (Unicode half blocks, for the terminal) · `to_svg(code, module_px, margin_modules) -> string` · `to_png(code, module_px, margin_modules) -> Result<bytes, string>` (via `std/image`). The matrix comes from the runtime (`qrcode` crate); the rendering is raylang and matches on the VM and natively |
| `std/image` | **images**: `decode_png(data: bytes) -> Result<Image, string>` with `Image { width, height, pixels: bytes }` — the output is ALWAYS **RGBA8** (4 octets/pixel, rows top to bottom), whatever the PNG contains. Supports color types 0/2/3/4/6 and depths 1/2/4/8/16 (16 bits → high octet), palette + `tRNS` (palette alpha and color-key for 0/2 at 8/16), None/Sub/Up/Average/Paeth filters, CRC verified per chunk and an anti-bomb cap in the zlib (via `std/inflate`). Interlaced (Adam7) → a clear `Err` (deferred). Corrupt/truncated input = `Err`, never a crash. **Encoding**: `encode_png(img: Image) -> Result<bytes, string>` — writes the RGBA8 `Image` as PNG (color type 6, 8 bits, no interlacing, None filter, zlib from `std/deflate`); `decode_png(encode_png(img))` returns the same pixels; `Err` if `pixels.len() != width * height * 4` or the dimensions are not positive. With `term.draw_png` or `fs.write_file_bytes` it closes the loop: generating sprites/captures from raylang without external tools |
| `std/huffman` | `huffman_encode huffman_decode` (the HPACK table of RFC 7541) |
| `std/protobuf` | `PbWriter`: `writer write_varint write_string write_bytes write_fixed64 write_fixed32 finish` · `parse -> Result<[PbField], _>` `get_int get_bytes get_string` · gRPC framing: `grpc_frame grpc_unframe` |
| `std/uuid` | `uuid_v4() -> string` · `is_uuid_v4` · `uuid_v7()`/`uuid_v7_at(ms)` (RFC 9562, time-sortable) · `is_uuid_v7` |
| `std/ffi` | `errno() -> int`: the thread's `errno` — the reason of the last failure of a POSIX-style extern C function (`fopen`/`unlink`…). **Read it immediately** after the call, with no I/O in between (§13). On wasm: 0 |

## 11. Additional packages (`net`, `web`, `rpc`, `db`, `tz`, `cron`, `mcp`, `llm`, `agent`)

Tier 2: they do **not** ship in the binary; they are declared in `ray.toml` (by path or git) and
imported the same way (`import net/http;` → `http.fetch(…)`). They live in the repo's `packages/`.

### `packages/web` — the application framework (Express-style, on top of `net/webserver`)

| Surface | What it does |
|---|---|
| App and routes | `new_app()` · `GET/POST/PUT/PATCH/DELETE/ALL(app, pattern, handler)` with params `/users/:id` (`c.param`), a final catch-all `/*rest`, regex `GET_re` · `mount(app, prefix, sub)` (sub-apps) · `not_found(app, h)` |
| Startup | `listen(build_app, host, port)` (blocks; the builder is a TOP-LEVEL fn — the form that also compiles natively) · **`listen_local(build_app, listener, token)`** (`listen_on` + `webserver.local_limits(token)` — the desktop/mobile-app pattern locked to ONE window: open it with `?ray_token=<token>`; any other process or web page gets 403) · **`listen_on(build_app, listener)`** (the bind/serve split — `net.tcp_listen(host, 0)` + `net.local_port` first → the program KNOWS its port without a close/re-bind race, and the backlog accepts from the bind on: the desktop-app pattern) · `listen_tls` · `listen_graceful` · `listen_limits` |
| Middleware | `use_mw` (global) · `use_on(prefix, mw)` · `with_mw([mw], handler)` (per route) · `after(app, hook)` · `Step.Next/Done` · `cors(app, origin)` · `log_requests(app)` (JSON per request with a trace-id) |
| Request | `c.param/query/body/json_body/form/form_field/header_of/cookie_of/local/put_local` |
| Response | `r.text/json/json_of (ToJson)/html/status/header/cookie/redirect` · `r.stream(ch, content_type)` (chunked: SSE, generated bodies) · `r.stream_len(ch, length, content_type)` (Content-Length + keep-alive) · `r.sendfile(c, path)` (ETag/304, MIME, Range/206; from 1 MB streamed from disk) |
| Static files | `static_files(app, prefix, dir)` · `static_files_cached(+max_age)` (strong ETag + 304 + Range) · **`static_embedded(app, prefix, dir)`** (serves from the `[native] embed` space — live disk in dev, baked into the native binary; content ETag) |
| Sessions | `ray_session` cookie (128 CSPRNG bits; `HttpOnly; SameSite=Lax`, `Secure` behind an HTTPS proxy; `is_session_id` rejects foreign ids) + `std/kv` |

Full details in [`docs/web-framework.md`](docs/web-framework.md) (in Spanish); demo in
`examples/web/framework/`. Shared state between handlers: each connection runs in its own fiber with
an isolated heap — the standard form is **`web.state`**: `state(path) -> Result<AppState, _>`
(the same switch as `sessions`: persists under `ray dev`, pure memory in production) or
`state_memory()`, with `state_get(st, k)` · `state_put(st, k, v)` · `state_delete(st, k)` ·
**`state_incr(st, k, delta) -> Result<int, _>`** (atomic counter — the RMW runs in the kv actor's owning
fiber). For custom typed state, the ACTOR pattern (one owning fiber + channels); recipe in MANUAL §15.

### `packages/net` — the network stack (24 modules, pure raylang)

| Group | Modules |
|---|---|
| HTTP | `http` (fetch/request, redirects, chunked, gzip, https; **streaming**: `stream[_with](method, url, body, headers[, idle_ms]) -> Result<Stream, _>` — status/headers right away, `stream_read(s) -> Result<Option<bytes>, _>` delivers each chunk as it arrives, incrementally de-chunked, `Ok(None)` = clean end; `stream_close`) · `sse` (Server-Sent Events client on top of `stream`: `open(url, headers)` + `next(es) -> Result<Option<Event>, _>` with `Event { data, event, id }`, and the **pure** decoder `decode(bytes) -> Option<(Event, int)>`) · `http2` + `hpack` (framing/HPACK; the decoder accepts **Huffman** literals — mandatory against real servers) · `http2_client` · `webserver` (async server + SSE; graceful shutdown: `serve_graceful(host, port, drain_ms, handler)` on top of `signals()`, the general form `serve_shutdown[_limits]` with a `stop` channel; `Request.remote` = the client's `"ip:port"` + `remote_ip(req)` without the port — per-IP rate limiting, X-Forwarded-For, logs with origin; `gzip(req, resp)` negotiates `Accept-Encoding` — compresses the body if the client accepts gzip, ≥ 512 octets, no previous `Content-Encoding` and not streaming; sets `Content-Encoding: gzip` + `Vary: Accept-Encoding`; `accepts_gzip(req)` standalone) |
| RPC | `grpc_client` (one-shot unary gRPC over TLS+ALPN h2; dogfooded against REAL grpc-go — Huffman headers and trailers-only errors covered; `GrpcResponse { message, grpc_status }`) · **(net 0.5.0)**: `grpc_server` — a unary gRPC server over h2c: `bind(host, port) -> Result<Listener, _>` (port 0 = ephemeral, `l.port`) · `serve(l, handler)` · `serve_until(l, stop, opts, handler)` · `serve_router[_until](l, [stop, opts,] build: fn() -> Router)` (the `Router` is built inside every connection fiber: `router().route(path, f).fallback(f)`, `dispatch`) · `serve_conn(conn, opts, router)` for your own accept loops · `Call { path, message, metadata, deadline }` → `Handler = fn(Call) -> Result<bytes, Status>` · `Options { max_message }` (4 MiB; RESOURCE_EXHAUSTED) · `parse_timeout` · flow control, `grpc-timeout` → DEADLINE_EXCEEDED, UNIMPLEMENTED without a route, percent-encoded `grpc-message` · `grpc_conn` — a client over a persistent connection: `connect(host, port)` (h2c) / `connect_tls(host, port)` (ALPN h2) · `call(c, path, message, metadata, deadline_ms) -> Result<Reply, string>` (`Err` = transport; a non-OK status is `Ok(Reply { message, status, status_message, metadata })`) · `call_once` · `into_result(r) -> Result<bytes, Status>` · `outcome(Result<Reply, string>)` (transport → UNAVAILABLE) · `usable` · `disconnect` (an expired deadline cancels the stream; the connection stays usable) · `grpc_status` — `OK`…`UNAUTHENTICATED`, `Status { code, message }`, `new`/`fail`, `code_name`, `from_http`, `percent_encode`/`percent_decode` · `grpc_h2` — the shared HTTP/2 `Link` (frames, SETTINGS/PING/WINDOW_UPDATE/GOAWAY, HPACK, windows) |
| Real time | `websocket` (server) · `websocket_client` (ws/wss) |
| Local server (apps) | `webserver.local_token()` (128 CSPRNG bits, hex) · `local_limits(token)` (= `default_limits` + a required token via `X-Ray-Token`, the `ray_local` cookie or `?ray_token=`, which seeds the `HttpOnly; SameSite=Strict` cookie; plus the cross-site guard: over loopback, without `X-Forwarded-*`, an http(s) `Origin` whose host ≠ `Host` → 403) · `serve_with_on_limits(listener, limits, make_handler)` · `cross_site_blocked(req) -> bool` · `local_token_via(req, token) -> (bool, bool)` (accepted, came-via-`?ray_token=`: to seed the cookie from your own accept loop) · `token_cookie(token) -> string` (the `ray_local` `Set-Cookie` line, public) · `read_request_carry(conn, limits, carry) -> Result<(Request, bytes), string>` (reading with carry-over: the server answers pipelined HTTP/1.1 requests sent in one write; for your own keep-alive loops) |
| Auth/identity | `jwt` (HS256; `jwt_verify_claims(secret, tok, now_ms)` = signature + `exp`/`nbf`) · `jwt_eddsa` (EdDSA) · `oauth2` (client_credentials) · `scram` (SCRAM-SHA-256) · `sigv4` (AWS) · `cookie` |
| Mail | `mail`: `encoded_word` (RFC 2047 B, words ≤75 chars on UTF-8 boundaries) · `header(name, value)` (encoded + folded at 78, CRLF, RFC 5322) · `base64_body` (76 columns, RFC 2045) · `dot_stuff` (CRLF + leading dot doubled, RFC 5321) · `address(display, email)` (mailbox: atext / quotes / encoded-word). It does NOT speak SMTP: it produces the strings the client (tcp_connect + tls_upgrade) writes |
| Infra | `dns` + `dns_cache` (A/AAAA/MX/CNAME/TXT/NS/SRV; the reply wait is bounded to 5 s — a lost datagram yields `Err("recv: read timeout")`, not a hang) · `udp` (`recv_from` yields the fiber and honors `net.set_read_timeout`) · `redis` (RESP2) · `postgres` (simple query; the full client is in `db`) |
| Pools (net 0.4.0) | `net/pool`: the GENERIC connection pool (a channel of `Slot<T>` slots; fibers do not share a heap, the channel is shared) — `new<T>(size) -> Pool<T>` · `acquire(p) -> Result<Slot<T>, string>` (Ready-first; parks when exhausted) · `release(p, conn)` / `release_empty(p)` · `run(p, dial, drop, op, retry) -> Result<R, string>` (a wire failure discards the connection and, if it came from the pool and `retry`, repeats `op` ONCE on a fresh one; "read timeout" discards without retrying; a server error keeps it) · `is_wire_error(e)` · `shutdown(p, drop)` · `http.pool(size)` + `pool_fetch`/`pool_request`/`pool_request_bytes`/`pool_close` (keep-alive per host: each `Conn` remembers its server; one for another host is closed and re-dialed) · `redis.pool(host, port, size)` + `pool_command`/`pool_command_with(p, args, retry)`/`pool_close` · net 0.4.1: `run_tx(p, dial, drop, begin, op)` (repeats `begin` + `op` on a fresh connection when `begin` failed on the wire on a reused one; `op` never repeats) |
| Observability | `log` (structured JSON; `with_trace` stamps `trace_id` on every line) · `metrics` (Prometheus) · `time` (UTC DateTime, ISO 8601/RFC 1123) · `trace` (W3C Trace Context: `Trace`, `new_trace`/`child`/`traceparent`/`parse_traceparent`/`from_headers`; the webserver adopts it with `trace_of(req)` and the http client propagates it with `request_traced`/`fetch_traced`) |
| Crypto | `crypto` (adapters of the builtins for the rest of the package) |

### `packages/rpc` — raylang↔raylang RPC

| Piece | Surface |
|---|---|
| Protocol | frame = 4 BE octets of length + JSON payload: request `{"id","method","params"[,"deadline_ms","traceparent"]}` → response `{"id","ok"}` \| `{"id","err"}` (protobuf: deferred) |
| Server | `serve(host, port, handler)` · `serve_graceful(host, port, drain_ms, handler)` (only SIGTERM/SIGINT shut down — `signals()` also carries SIGWINCH; `shutdown_signals()` is that filter as a channel, to wire `serve_shutdown` by hand) (signals + draining) · `serve_shutdown[_limits](…, stop, drain_ms, …)` · handler `fn(Req) -> Result<Json, string>`; `Req { method, params, deadline_ms, traceparent }`; one fiber per connection; a handler panic → `err` without killing the connection; `Limits { max_frame_bytes }` (10 MiB) |
| Client | `connect(host, port) -> Result<Client, _>` · `call(c, method, params)` · `call_deadline(…, ms)` (bounds the wait; after a timeout: reconnect) · `call_full(…, deadline_ms, traceparent)` · `disconnect` — persistent connection, correlated and validated id |
| Pool | `pool(host, port, size) -> Pool` · `pool_call`/`pool_call_deadline`/`pool_call_full` · `pool_close` — up to `size` calls IN FLIGHT at once (one connection per slot: the server serves one fiber per connection → real parallelism); lazy dialing, a checkout that PARKS when exhausted (backpressure via a bounded channel) and **automatic reconnection** after a failure (the timeout discards the desynchronized connection; the next call re-dials) |

### `packages/db` — database clients

| Module | What it gives |
|---|---|
| `db/mysql` | wire v10: `connect`/`connect_tls` · `query`/`exec` with `?` (prepared/binary) or text · native + caching_sha2 auth (full path over TLS) · db 0.2.0: `pool`/`pool_tls(host, port, user, password, database, size) -> Pool` · `pool_query` (retries once on a fresh connection when the reused one fails on the wire) · `pool_exec` (no retry) · `pool_with(p, f)` (one connection for the whole of `f`) · `pool_tx(p, f)` (BEGIN/COMMIT/ROLLBACK) · `pool_close` · db 0.2.1: `pool_with_retry(p, f)` (idempotent blocks); `pool_tx` retries the `BEGIN` on a fresh connection when it failed on the wire |
| `db/postgres` | extended wire v3: `connect`/`connect_tls` (sslRequest) · `query`/`exec` with `$1, $2…` · SCRAM · db 0.2.0: `pool`/`pool_tls(host, port, user, password, database, size) -> Pool` · `pool_query` (retries once on a fresh connection when the reused one fails on the wire) · `pool_exec` (no retry) · `pool_with(p, f)` (one connection for the whole of `f`) · `pool_tx(p, f)` (BEGIN/COMMIT/ROLLBACK) · `pool_close` · db 0.2.1: `pool_with_retry(p, f)` (idempotent blocks); `pool_tx` retries the `BEGIN` on a fresh connection when it failed on the wire ; FATAL errors of class 57P (restart, `pg_terminate_backend`) are phrased as a closed connection so the pool discards them |
| `db/sqlite` | embedded (rusqlite in the host): `connect(":memory:" \| path)` · `query`/`exec` with `?1…` · `last_insert_rowid` |
| `db/mongo` | OP_MSG + BSON: `connect`/`connect_tls` · `insert find update delete` (filters = BSON documents) · `run_command` · full cursors (getMore) · db 0.2.0: `pool`/`pool_tls(…, size)` · `pool_find` (retries once) · `pool_insert`/`pool_run_command` (no retry) · `pool_with(p, f)` · `pool_close` · db 0.2.1: `pool_with_retry(p, f)` (idempotent blocks); `pool_tx` retries the `BEGIN` on a fresh connection when it failed on the wire |
| `db/bson` | `enum Bson` · `encode`/`decode` · `dump` · JSON bridge (`doc_from_json from_json to_json`) |

A uniform API across the 4 clients: `connect → Conn`, `query → Result<[[string]], string>` (mongo:
documents), `exec → Result<int, string>`, `disconnect`. Parameter binding (anti-injection) in all of them.

### `packages/tz` — IANA local time (v0.1.0)

Time zones over the system's TZif files (`/usr/share/zoneinfo`; RFC 8536 v1-v3, perpetual footer rules), in pure raylang; instants are epoch-ms UTC like `std/time`. `import tz/tz;` — `load(name) -> Result<Zone, string>` · `load_file(path)` · `system()` · `offset_at(z, ms) -> int` · `abbrev_at(z, ms) -> string` · `is_dst_at(z, ms) -> bool` · `to_local(z, ms) -> time.DateTime` · `to_utc(z, civil) -> LocalResult` (`Single(ms)` · `Ambiguous(before, after)` autumn overlap · `Gap` spring gap) · `name(z)`. Windows ships no zoneinfo: `load` returns `Err` (`std/time` UTC still works). Guide: [`packages/tz/README.md`](packages/tz/README.md).

### `packages/cron` — cron expressions and recurring timers (v0.1.0)

`import cron/cron;` — `parse(expr) -> Result<Schedule, string>` (5 fields `min hour dom month dow`; `*`, ranges, steps `*/n`, lists, aliases `@hourly/@daily/@weekly/@monthly/@yearly`; vixie-cron dom OR dow quirk) · `next_after(s, ms) -> Result<int, string>` (pure; `Err` for an impossible expression) · `run(s, job)` (cooperative runner: sleeps yielding the fiber, under `spawn`). Civil time with `import cron/local;` + the `tz` dependency: `local.next_after_in(s, zone, ms)` · `local.run_in(s, zone, job)` (DST: a time in the spring gap fires when the gap ends; one in the overlap fires only the first time). Guide: [`packages/cron/README.md`](packages/cron/README.md).

### `packages/mcp` — Model Context Protocol (v0.2.0)

`import mcp/mcp;` — the client: `stdio_server(name, command, args)` / `http_server(name, url) -> Server` (adjustable fields: `dir`, `env`, `headers`, `client_name`, `client_version`, `start_timeout_ms`, `call_timeout_ms`) · `connect(server) -> Result<Session, string>` (launches the process or opens the HTTP session and performs `initialize`) · shortcuts `connect_stdio(command, args)` / `connect_http(url)` · `tools(c) -> Result<[Tool], string>` with `Tool { name, description, schema: Json, read_only }` · `call(c, tool, arguments: Json) -> Result<string, string>` (a failing tool is an `Err`) · `call_json(c, tool, arguments: string)` (arguments as JSON text) · `resources(c) -> Result<[Resource], string>` (`[]` if the server declares none) · `read_resource(c, uri) -> Result<string, string>` · `instructions(c) -> string` · `offers(c, capability) -> bool` · `request(c, method, params: Json) -> Result<Json, string>` · `close(c)`. Two transports: **stdio** (a process with its stdin open, one JSON-RPC message per line) and **Streamable HTTP** (POST answered with JSON or SSE, the `Mcp-Session-Id` header, reopening on 404). One session per server in its own fiber: the `Session` copies across fibers and every copy talks to the same server; a server that died is relaunched on the next request. `import mcp/protocol;` — the pure pieces: `request` · `notification` · `initialize` · `reply_with_id` · `result_of` · `parse_tools` · `parse_resources` · `call_text` · `resource_text` · `offers` · `instructions_of` · `tool_name(server, tool)` (= `mcp__<server>__<tool>`) · `is_remote` · `split_name`. `import mcp/serve;` — the server, for an app to offer its tools to an assistant: `provider(name, version) -> Provider` (fields `instructions`, `token`) · `tool(p, name, description, schema, run)` with `run: fn(Json) -> Result<string, string>` and `schema` as JSON text (`""` = no arguments; an invalid schema or a repeated name aborts the program at startup: they are programming errors) · `read_only_tool(…)` (announced with `readOnlyHint`) · `resource(p, uri, name, description, mime, read)` · `stdio(p) -> int` (one message per line over stdin/stdout, until EOF) · `http(build, host, port)` / `http_on(build, listener)` (Streamable HTTP without sessions; `build` is a top-level function every connection calls in its own fiber) · `answer(p, req) -> Response` (to mount it on a route) · `handle(p, line) -> Option<string>` (pure). A tool that aborts is a failed call, not a dead server; over HTTP a foreign `Origin` is refused and, with `token`, `Authorization: Bearer` is required. Depends on `net`. VM and native.

### `packages/llm` — talking to language models (v0.2.0)

`import llm/llm;` — `for_anthropic(model, api_key)` / `for_openai(model, api_key)` / `for_endpoint(base_url, model, api_key) -> Config` (any endpoint that speaks the OpenAI dialect: Ollama, LM Studio, OpenRouter…) / `for_preset(id, model, api_key) -> Option<Config>` (`anthropic openai openrouter groq deepseek mistral xai gemini ollama lmstudio`) · `send(c, tools, history) -> Result<Reply, string>` (one round-trip) · `send_stream(c, tools, history, show) -> Result<Reply, string>` (the text reaches `show` piece by piece; returns what `send` would) · `user(text)` / `tool_result(call_id, text) -> Message` · `tool(name, description, schema) -> Tool` (`schema` as JSON text; an invalid one aborts the program) · `tool_calls(reply) -> [ToolCall]` · `models(c) -> Result<[string], string>` · `retry_after_ms` · `error_message`. `Reply { message, usage, stop_reason, model }`, `Message { role, text, tool_calls, tool_call_id, raw }`, `ToolCall { id, name, arguments }` (arguments as JSON text), `Usage { input_tokens, output_tokens, cached_tokens, latency_ms, measured }`. Adjustable `Config`: `system`, `max_tokens`, `temperature` (negative = not sent), `effort`, `timeout_ms`, `headers`, `extra` (extra body fields), `cache`, `max_attempts`. Two dialects, **Anthropic** (`/v1/messages`) and **OpenAI** (`/chat/completions`), pure in `llm/anthropic` and `llm/openai` (`build_body` · `headers` · `parse_reply` · `assembly`/`absorb`/`assembled` for a stream); `llm/message` holds the types and `was_truncated`/`was_refused`; `llm/config`, the presets and the URLs. Retries transient failures (no connection, 429, 408, 5xx) honouring `Retry-After`; a stream is only retried while it has delivered no text; fixes `max_tokens`/`max_completion_tokens` and the temperature by itself when the provider rejects them. On Anthropic it replays the assistant's blocks verbatim (`raw`: signed reasoning) and marks the prompt and the tools for the cache. Depends on `net`. VM and native.

### `packages/agent` — an agent loop (v0.2.0)

`import agent/agent;` — `new(config) -> Agent` (the model from `llm`; starts at `ASK`, 20 steps per turn) · `tool(a, name, description, schema, risk, run)` (a local tool; a repeated name, an unknown risk or an invalid schema aborts the program at startup: `schema` as JSON text, `risk` = `READ`/`WRITE`/`EXEC`, `run` takes `Json` and returns `Result<string, string>`) · `connect(a, name, session) -> Result<int, string>` (the tools of an `mcp` session, seen by the model as `mcp__<name>__<tool>`: read-only = `READ`, the rest = `EXEC`; its instructions join the prompt) · `on_approve(a, f)` with `f` from `(ToolCall, risk)` to `Decision` (`Yes`/`Always`/`No`) · `on_event(a, f)` with `Event` = `Thinking(step)` / `Text(piece)` / `Said(text)` / `Calling(call, risk)` / `Returned(call, output, failed)` / `Declined(call, why)` · `run(a, prompt) -> Result<Outcome, string>` (one turn to completion) · `run_cancellable(a, prompt, cancel)` (abandons the wait for the model if anything arrives on the channel) · `reset(a)` · `offered(a)` · `risk_of(a, name)` · `unattended(autonomy, risk) -> bool` · `autonomy_note(autonomy)`. `Outcome { text, stop, steps, usage }` with `stop` = `DONE`/`MAX_STEPS`/`TRUNCATED`/`REFUSED`/`CANCELLED`. Adjustable `Agent` fields: `config`, `autonomy` (`ASK`/`EDITS`/`AUTO`), `max_steps`, `stream`, `history`, `usage`, `allowed`. Runs unasked: `READ` always, `WRITE` from `EDITS`, `EXEC` only at `AUTO`; the rest is asked about, and without `on_approve` it does not run. A call that does not run, fails, aborts or does not exist still gets its result (`error: …`): `run` only returns `Err` when talking to the model fails. Depends on `llm` and `mcp`. VM and native.

## 12. Annotations

| Annotation | On | Effect |
|---|---|---|
| `@test` | `fn () -> bool` or `fn () -> unit` | `ray test` runs it: bool passes if `true`; unit passes if it triggers no `assert`/`panic`. Each test runs isolated — on finish, ALL the OS handles it left alive are closed (listeners included; child processes are NOT killed) — it can live in any module of the project (it runs qualified: `math.t`) and use `import`; a failure reports `at module:line:col` |
| `@derive(Eq)` | non-generic struct/enum | generates `impl Eq` (structural equality) |
| `@derive(Show)` | non-generic struct/enum | generates `impl Show` (`Name { f: v }` / `Name.Variant(v)`); supports recursive enums |
| `@derive(Hash)` | non-generic struct/enum | generates `impl Hash` (for `Set`/`Dict` keys) |
| `@derive(Clone)` | non-generic struct/enum | generates `impl Clone` (`clone(self) -> Self`), a **shallow** copy: value fields are copied, reference fields (arrays, maps, structs, channels) are shared — the same as writing the literal by hand. To copy children, `impl Clone` by hand calling their `clone()` |
| `@derive(ToJson)` | non-generic struct/enum | generates `impl ToJson` (`to_json(self) -> string`), used by the web framework's typed JSON responses. The trait lives in `std/json`: it must be in scope (`from std/json import ToJson;`) |

They combine: `@derive(Eq, Show, Hash, ToJson)`. Those are the **four** derivable traits; `Ord` is
implemented by hand (any other name is a compile error).

## 13. FFI: marshalable types

`extern "lib" { fn name(args) -> ret; }` declares C functions (dlopen/LoadLibrary at runtime). Arity
**0 to 6** (the checker rejects beyond that: the same limit on every engine). It is the language's
only unsafe boundary: the declared signature is **trusted**.

| raylang type | In C (argument) | In C (return) |
|---|---|---|
| `int` | integer in a register | C `int` (32 bits, sign-extended) |
| `u64` | integer in a register | `long`/`size_t` (64 bits) |
| `float` | `double` | `double` |
| `bool` | `int` | `int` |
| `unit` | — | `void` |
| `string` | NUL-terminated `char*` (temporary copy) | ✗ (use `Option<string>`) |
| `bytes` | pointer to the buffer (NUL-terminate it yourself: `b"…\x00"`) | ✗ (use `Option<bytes>`) |
| `ptr` | opaque pointer | opaque pointer |
| `Option<string>` / `Option<bytes>` | ✗ | `char*`: NULL → `None`; otherwise, **copies** up to the NUL (never frees) |
| `Option<ptr>` | ✗ | pointer: NULL → `None` |

Out of contract: **variadic** functions (`printf` — UB on arm64), structs by value, callbacks (noted
for an FFI v2). Not available in the wasm playground.

**`errno`**: `import std/ffi;` + `ffi.errno()` reads the thread's `errno` (the reason of the last
POSIX failure). Rule: **immediately** after the extern, with no raylang I/O in between (an operation
that parks the fiber lets its siblings on the same thread run, and they may clobber it; two
consecutive statements without I/O do not park). After a `blocking` extern, the runtime restores in the
worker the errno of the pool thread — the rule is the same.

**Stack for C code** (native binary with fibers): C calls run on the fiber's stack. With externs
declared, the default rises by itself from 128 KiB to **1 MiB** per fiber (virtual reservation: only the
touched pages cost); `RAY_FIBER_STACK_KIB` adjusts it and always wins. An overflow yields SIGSEGV via
the guard page (never silent corruption).

**`extern "lib" blocking { … }`** marks the block's signatures as **truly blocking** calls (I/O, slow
C libs). Same types and values; only the scheduling changes: in the native binary with fibers (the
default) the call is offloaded to a blocking pool and the fiber parks (the M:N worker does not stall).
Where there is no scheduler to protect (VM, interpreter, `--without fibers`, outside a fiber) it is
inert. `blocking` is contextual: it remains valid as an identifier.

## 14. The `ray` CLI

(`raylang` is an alias of the same binary.)

| Command | What it does |
|---|---|
| `ray new <name> [--frontend <vite-template>]` | creates a project (ray.toml + src/main.ray + .gitignore); **`--frontend react-ts`** (any `npm create vite` template) adds the **`[frontend]`** section (dev command + URL + build + `dist`), a `main.ray` that opens `app://index.html` and answers `window.ray.request`, and `.gitignore` entries for `frontend/node_modules` and `frontend/dist`; it does NOT run npm: it prints the commands that follow (`npm create vite@latest frontend -- --template …`, `npm --prefix frontend install`, `ray dev`) |
| `ray run [file] [--] [args…]` | runs (by default `src/main.ray`); resolves dependencies; the first `--` separates the program's arguments |
| `ray profile [file] [--json] [--out FILE] [--top N] [args…]` | like `run` (VM only) with the **per-function profiler**: on exit (also after `exit()` or a runtime error) it prints to stderr — or writes to `--out` — a table per function with self time (without children), % of the total, inclusive time, calls and average per call, sorted by self time; `--json` gives `{wall_ns, fibers, functions:[{name, calls, self_ns, inclusive_ns}]}`. Instrumented on the call path (two clock reads per call; off costs nothing); builtins fall into the caller's self time; in recursion the inclusive time counts once per outer activation; a TAIL call closes the caller's frame (its time excludes the callee) |
| `ray dev [file] [args…]` | like `run`, but restarts on changes to `.ray`/`.ray.html`/`ray.toml` (SIGTERM → draining with `serve_graceful`). **— `[frontend]` in ray.toml** (`dev = "npm --prefix frontend run dev -- --strictPort --port 5173 --clearScreen false"`, `url = "http://localhost:5173"` (default: Vite's), `build = "npm --prefix frontend run build"`, `dist = "frontend/dist"`): `ray dev` launches `dev` through the system shell at the project root ONCE per session (leader of its own process group; it survives the program's restarts, its HMR keeps the page), checks BEFORE that `url` is not already taken by another process (another Vite, an earlier session → 73 with the `lsof`/`netstat` line that shows it), waits ≤ 60 s for `url` to accept connections on any of the host's addresses —`localhost` is `127.0.0.1` and `::1`— (if the command dies first: 73), exports **`RAY_FRONTEND_URL`** to the program (`ui.app_url`/`app://` resolve there) and kills it —the whole tree: `npm`→`sh`→`node`— on exit by any route (window closed, `q`, Ctrl-C); if the dev server dies on its own, it warns and carries on. `ray build --native`/`ray bundle` run `build` first and embed `dist` (like one more `[native] embed`); `ray run` without `ray dev` serves `dist` live and warns if it is missing. A native binary never looks at `RAY_FRONTEND_URL`. **`--device`**: does not run the program here — it watches the same files, checks that they compile and sends the **source snapshot** (`.ray`, `ray.toml`, `.ray.html`, embedded assets, `.ray-deps`) to the linked devices (the development library on the phone, or `ray dev-client <url> <dir>` on a desktop), which stop the running program and start the new one on their VM without reinstalling; it prints the `<shell id>.dev://ip:port/token` link (LAN, random high port) with its **QR** in the terminal (scanned with the phone's camera it opens exactly that shell) and each device's states (`running`, `stopped`, `runtime error`…); the device's `print`/`eprint` reach this terminal (remote console); each device only receives the delta of what it already has; `path = …` dependencies outside the root travel under `.ray-path-deps/`. A change that does not compile is not sent |
| `ray build [file] [--native …]` | checks and compiles without running (0 ok / 65 error); `--native` transpiles to Rust and produces a **native binary** (24–61× the VM, byte-identical); `--lib` emits instead a **static library** with the C entry point `ray_start()` — what a mobile shell (or any C host) links; the shell registers its ui handlers with `ray_ui_set_handlers` and pushes events with `ray_ui_push_event` |
| `ray bundle [file] [--name N] [--icon i.png] [--id com.x.y] [-o dir] [--without list] [--ios [--ios-target device\|sim\|both]]` | packages a **desktop app** (name/icon/id come from **`[app] name/icon/id`** in ray.toml — the icon relative to the project root — and the flags override them; an unknown flag is error 64 with the usage and `--help` prints the usage without building; **`[app.plist]`** in ray.toml goes verbatim into the Info.plist of macOS and (since 1.27.13) of the iOS shell — `key = "text"` → `<string>`, `true`/`false` → `<true/>`/`<false/>`, `["a", "b"]` → `<array>` — and **`NSLocalNetworkUsageDescription`** is added automatically when the program imports `std/net`, `std/udp` or the `net` package, unless you declare it yourself)): a `--release` native build (with the `[native] embed` of ray.toml) → a `.app` on macOS (Info.plist + icns via sips/iconutil + ad-hoc codesign), a directory with a `.desktop` on Linux or, on Windows, a directory with `<name>.exe` (WINDOWS subsystem: no console on double-click; icon and VERSIONINFO — name, version, `[app] copyright` — embedded as resources via `UpdateResourceW`, no crates) + `<name>.lnk` (absolute target and cwd; copy it to the Start menu); without `--sign`, SmartScreen warns; **`--ios`** generates instead the XCODE PROJECT of an iOS app — a WKWebView shell in ObjC + device and simulator staticlibs (`ray build --native --lib` inside; the xcconfig picks the `.a` by SDK; `--ios-target device|sim` builds only one side — iterating against one destination, the other build is superfluous — and the `.a` of the side not built is PRESERVED from the previous project) + Info.plist; the SAME desktop source runs on the iPhone (`ui.open` hands the URL to the shell's webview; lifecycle as `lifecycle` events). The macOS `.app` writes `NSHumanReadableCopyright` from **`[app] copyright = "…"`** in ray.toml (the About panel shows it). Simulator unsigned; device: declare the team in `[ios] development_team = "…"` of ray.toml (the bundle writes it into App.xcconfig and also PRESERVES an existing signature on regeneration — before, every bundle erased it), or open it in Xcode and pick it once. `--ios` excludes `process` (`audio` plays through AudioQueue on iOS too). The `[app] icon` is wired into the pbxproj (Resources phase + `ASSETCATALOG_COMPILER_APPICON_NAME`); the `.a` preserved by `--ios-target sim|device` is checked with `nm` and, if it lacks a symbol the new shell calls, the bundle warns with the exact `ray build --native --lib` to rebuild it; **`[ios] background_audio = true`** activates a `playback` `AVAudioSession` in the AppDelegate and adds `UIBackgroundModes = ["audio"]` (the program keeps sounding in the background and with the silent switch on). **`--android`** generates the GRADLE PROJECT (Java shell + WebView; the program as the cdylib `libray_app.so` in `jniLibs/` — the JNI symbols go inside), `--android-abi arm64|x86_64|all` (arm64 default; preserves the `.so` of the ABI not built and `local.properties`), `[android] application_id` in ray.toml; same exclusions as iOS; **`[android] background_audio = true`** adds a `mediaPlayback` *foreground service* that MainActivity starts in `onPause` and stops in `onResume` (`FOREGROUND_SERVICE*`/`POST_NOTIFICATIONS` permissions; a "Playing in the background" notification while it lasts; the notification permission is NOT requested at launch — on Android 13+ without granting it in Settings the service runs all the same, just without a visible notification; the `RayPlaybackService` class is always generated, the flag only enables it); stdout→logcat tag `ray`; build with `gradle assembleDebug` (Gradle 9.x + JDK 17+, AGP 9 pinned); `--icon` generates the multi-density `mipmap-*/ic_launcher.png` (via sips; legacy — Android 8+ masks it to a circle) and the **release signing** goes through `keystore.properties` in the root of the generated project (conditional, zero secrets in ray.toml; keystore + properties PRESERVED on regeneration — full flow in the generated README). NOTE: the .app launches with cwd=/ → assets go embedded; a downloaded unsigned app requires approval in Settings (macOS 15+) · **signing and notarization**: `--sign IDENTITY` (or `[app] sign`, or `RAY_SIGN_IDENTITY`) signs the `.app` with `codesign --options runtime --timestamp` and the entitlements from `--entitlements`/`[app] entitlements` (an empty plist by default), verifies with `--verify --strict` and fails with 74 if it does not verify; `--notary PROFILE` (or `[app] notary`, or `RAY_NOTARY_PROFILE`; create the profile with `xcrun notarytool store-credentials`) submits the `.app` to Apple (`notarytool submit --wait`), requires `Accepted` and staples the ticket (`stapler`). On Windows `--sign` is the certificate subject or a `.pfx` (`RAY_SIGN_PFX_PASSWORD`) for `signtool sign /fd SHA256 /tr …`. Without an identity, ad-hoc as before (macOS 15+ asks for approval). `ray release` accepts the same flags and forwards them to the bundle. **`--dev`** (with `--ios` or `--android`): the DEVELOPMENT shell — the same app named `<app>-dev` with id `<id>.dev`, linking the development library (toolchain + VM for the phone, `ray_start` = the link; on Android the `.so` defines the JNI symbols) instead of the program; install it once, pair it by scanning the QR `ray dev --device` prints with the camera (the shell registers its id as a URL scheme: `CFBundleURLTypes` / `intent-filter`) or by typing the URL on its first page, and it reloads the program on the phone with every change (`--devtools` implied). The library is built from the toolchain's source tree or downloaded prebuilt from the release (`ray-dev-lib-<target>.tar.gz` → `~/.ray/dev-lib/<version>/`; `RAY_DEV_LIB` forces one); `ray dev-lib --target T -o f` builds it on its own |
| `ray keygen [--app-id X] [--force]` | creates the app's Ed25519 **signing key** (hex seed in `~/.ray/keys/<app-id>.key`, or `RAY_KEYS_DIR`; mode 0600) and writes the public key into `[app] public_key` of ray.toml (creating the section if missing). Distinct from `ray registry keygen` (package publishing). Refuses to overwrite an existing key without `--force`: apps in the field would stop trusting your manifests |
| `ray release [file] [-o dist] [--notes URL] [--min-version V] [--base-url URL] [--key HEX] [--without list] [--publish] [--tag vX.Y.Z]` | the publisher side of `std/update`: runs `ray bundle`, zips the bundle into `dist/<name>-<version>-<platform>-<arch>.zip` (deflate, single top-level directory, unix modes), writes `dist/update.json` (`app`, `version`, `notes`, `min_version`, `artifacts`; **keeps the artifacts of other platforms for the same version** already there: run it once per platform over the same `dist/`) and signs it into `update.json.sig` with the key from `--key`, `RAY_SIGNING_KEY` or `~/.ray/keys/<app-id>.key`; refuses if the key is not the one in `[app] public_key`. Without `--base-url` artifact URLs are relative to the manifest (serve `dist/` as is); with `--publish`, `gh release create` + `upload --clobber` on the tag (`v<version>`) and URLs point at `releases/download/<tag>/` of the `origin` remote — the app checks `https://github.com/<o>/<r>/releases/latest/download/update.json` |
| `ray test [file] [filter]` | runs the project's `@test`s: the entry and all its modules (qualified: `math.t`) + each `tests/*.ray` as an integration suite; substring filter; exits with 0/1 (65 if something does not compile — **every** suite is type-checked even without tests, the project entry included). **`--native [--release]`**: compiles each suite to a native binary (a `main` dispatching by test name; honours `[native] without`, the embedded assets and the project's `[app]`) and runs each test as a process — same report and exit codes, without the `at module:line:col` line (native binaries carry no trace); combines with `--watch` and the filter |
| `ray fmt <file>... [--write]` | prints the canonical version (keeps `if let` and the user's parentheses and comments; 4-space indentation; whatever exceeds 100 columns is split: a `from … import` one name per line, a method chain one link per line, `&&`/`\|\|`/`+` chains one operand per line, and delimited lists — arguments, `fn` parameters, literals — one element per line with the closer on its own line; trailing comments stay with their operand/element and your parentheses are kept). `--write`/`-w` rewrites in place and accepts several files. Canonical forms that surprise: an empty block is `{ }` (with a space) and two consecutive top-level `const`s get a blank line between them |
| `ray build --templates-only [path…]` | **materializes** on disk the generated module of each `.ray.html` template (`{% params %}` signature), for inspection (without paths: the project root). The normal path does not need it: the loader compiles templates **in memory** when resolving their imports and ignores a sibling `.ray` |
| `ray check [file]` | alias of `ray build`: type-check without running; 0 ok / 65 error |
| `ray doc <file \| std/<module> \| <module>.<symbol> \| <builtin>>` | Markdown documentation of a file's public surface (`///`); with a symbol the same answer as the MCP's `ray_doc`: `ray doc std/ui` lists the module, `ray doc ui.MenuItem` gives signature and doc · also public constants (`ray doc crypto.PASSWORD_ITERATIONS`), collection modules by path (`ray doc std/collections/deque`) and, in `ray_doc` with `path`, the surface of a project module or a package module (`"rpc/rpc"`, `"web/framework"`) · `ray doc string.last_index_of` / `bytes.index_of_from` / `Result.map` / `Option.and_then` (`Type.method` form: a free function by its first parameter or a prelude trait method; the bare name lists every variant); inside a project (a `ray.toml` above) it also resolves the project modules and its `.ray-deps` dependencies; `ray doc --help` |
| `ray serve [dir] [--host H] [--port N]` | serves a directory of static files over HTTP for previews (`_site/`, `playground/`, the output of `ray doc`): `index.html` per directory, MIME by extension, `Cache-Control: no-store`, no `..`; defaults to `.` on `127.0.0.1:8000`, `--port 0` picks a free one. It is the server written in raylang embedded in the binary, not a production server (use `net/webserver` for that) |
| `ray repl` | interactive REPL |
| `ray lsp` | Language Server (diagnostics, hover, go-to-definition, references, rename, completion, signature help) |
| `ray mcp` | MCP server for LLM agents: `check`/`run`/`test`/`fmt`/`doc` tools, with the code sandboxed (fuel + heap + deadline), plus the `raylang://llms.txt` resource (the distilled context [`llms.txt`](llms.txt) from the repo root, for the model's prompt). Guide: [`docs/mcp.en.md`](docs/mcp.en.md) |
| `ray add <name>[@req]` | adds a dependency from the registry (`1.2.0`, `^1.2`, `~1.2.3`, `*`) |
| `ray remove <name>` | removes it (and its cache if nobody else uses it) |
| `ray search [pattern]` | lists registry packages; the pattern matches the name and the description, keywords and modules of the `<name>.meta.toml` sidecar (`ray search http` → `net`, with the description on the row and `matches keyword/module/description` when the name did not match) |
| `ray fetch` | downloads the dependencies into `.ray-deps/` |
| `ray update` | re-resolves to the newest compatible versions |
| `ray registry publish [--repo <spec>] [--sign]` | publishes this version to the registry (validates + semantic check + hash; `--sign` signs it with Ed25519 and claims/verifies the name's owner) |
| `ray registry keygen [--out F]` | generates the Ed25519 publishing key (`RAY_KEY` or `~/.ray/publish.key`) |
| `ray registry verify [dir]` | audits the signatures of an index against its owners (the index repo's CI) |
| `ray registry yank <name>@<ver> [--undo]` | withdraws/restores a published version |
| `ray upgrade [tag] [--check]` | updates `ray`/`raylang` to the latest release (or the tag); `--check` only reports (0 = up to date, 1 = a newer one exists) |
| `ray toolchain install [--rust <channel>] [--targets ios,android,<triple>…] [--force] [--no-vendor]` | installs a PRIVATE Rust toolchain for `build --native` under `~/.ray/toolchain` (rustup `minimal` profile, without touching the user's Rust or PATH) and the release's `ray-runtime` vendor (first build offline). With `cargo` already on the PATH it installs nothing (unless `--force`). `--targets` also adds the std of those targets |
| `ray toolchain add-target <ios\|android\|triple>…` | installs the Rust standard library of those targets into the toolchain `build --native` uses (`ios` = `aarch64-apple-ios` + `aarch64-apple-ios-sim`; `android` = `aarch64-linux-android` + `x86_64-linux-android`), the private one included, which is not on the PATH. What `bundle --ios`/`--android` need. Exit 69 without `rustup` |
| `ray toolchain status` | which `cargo`/`rustc` `build --native` would use and from where (`RAY_CARGO`/`RAY_RUSTC` → PATH → private), their version, the system linker, the installed extra targets and the vendor; exit 1 if any is missing |
| `ray version` | version — `X.Y.Z` on a release; `X.Y.Z+dev.<sha>` when built from the repo HEAD (HEAD and release accept different code; `[package] raylang = "X.Y.Z"` in `ray.toml` requires a toolchain ≥ that version and warns when the toolchain is a development build) |
| `ray help` | the help: every subcommand with its flags (also without arguments). **`ray <subcommand> --help`** (or `-h`, as the first argument) prints that subcommand's usage (before, `ray dev --help` started dev mode and `ray new --help` created a project called `--help`) |

`run` flags:

| Flag | Effect |
|---|---|
| `--interp` | forces the interpreter (development oracle; no concurrency) |
| `--deterministic` | reproducible M:1 scheduler (one thread, FIFO); also `RAYLANG_THREADS=1` |
| `--fuel N` | VM instruction limit (for sandboxed embedding) |
| `--heap N` | cap on live heap objects (forces GC; if not enough, aborts) |
| `--devtools` | webview inspector in `std/ui` windows (on by itself under `ray dev`) |

`build --native` flags:

| Flag | Effect |
|---|---|
| `-o <path>` | name of the output binary (by default the `name` of `ray.toml` or the file's *stem*; on a Windows target it always ends in `.exe` — or `.lib` with `--lib`) |
| `--release` | optimization tier `opt-level=3 + lto=fat + codegen-units=1 + target-cpu=native` (slower to compile, not portable) |
| `--fast` | swaps **checked** arithmetic for **wrapping** (does not detect overflows): more performance in exchange for a guarantee; for your own code, not for hostile input |
| `--target <triple>` | *cross-compiles* to the given triple (requires the target installed in the toolchain) |
| `--lib` | builds the program as a static/dynamic library (`.a`/`.so`/`.lib`) for the iOS/Android shells and other hosts |
| `--embed <dirs>` | directories baked into the binary (`std/embed`, `static_embedded`); merged with `[native] embed` from `ray.toml` |
| `--devtools` | keeps the webview inspector available in the binary (without the flag the call does not exist) |
| `--no-stubs` / `--allow-stubs` | policy for a function outside the native subset: by default a development build emits it as a stub that fails when called (with a warning) and **`--release` rejects it** (exit 65), as does `ray bundle`; `--no-stubs` forces the error in dev builds too and `--allow-stubs` brings the stubs back in release |
| `--without <list>` | excludes subsystems: `crypto,tls,sqlite,regex,bigint,qr` (they fall back to a *stub* with a clear error or to the raylang implementation) and `mimalloc,ahash,fibers,process` (which are on by default). Merged with `[native] without` of `ray.toml` |

The subsystems backed by production crates (TLS/`rustls`, crypto/`ring`, SQLite/`rusqlite`, accelerated
regex) are linked **only when the program uses them** (generated Cargo project; the binary calls the same
code as the VM via the `ray-runtime` crate). **mimalloc, aHash and fibers are on by default** — they are
what makes the default the Cargo path; `--without mimalloc,ahash,fibers` goes back to bare `rustc` with
thread-per-task. `[native] without = ["tls", …]` in `ray.toml` fixes a stable exclusion policy for the
project.

Environment variables: `SSL_CERT_FILE` (extra CAs for TLS), `RAY_INDEX` (package index; without it or
`[registry] index` the official `ray-language/ray-index` is used; empty = no index), `RAY_MIRROR`
(download mirror), `RAY_KEY` (Ed25519 publishing key), `RAYLANG_THREADS` (number of scheduler worker
threads; `1` = deterministic), `RAY_FIBER_STACK_KIB` (stack reservation per fiber in the native binary),
`RAYLANG_SPIN_US` (µs of yielding spin before parking in the native scheduler; `0` disables it),
`RAYLANG_MAX_DEPTH` (allowed recursion frames; 1024 by default),
`RAY_SIGN_IDENTITY` / `RAY_NOTARY_PROFILE` / `RAY_SIGN_PFX_PASSWORD` (signing and notarization for `ray bundle`/`ray release`, equivalent to `[app] sign`/`notary` and to the `.pfx` password on Windows),
`RAY_SIGNING_KEY` / `RAY_KEYS_DIR` (`std/update` Ed25519 key for `ray release`, or the `ray keygen` directory; `~/.ray/keys` by default),
`RAY_TOOLCHAIN_HOME` (private toolchain for `build --native`; `~/.ray/toolchain` by default),
`RAY_DEV_LIB` (development library the mobile shell loads under `ray dev --device`),
`RAY_DEV_FRONTEND_URL` (Vite dev-server URL for a shell built with `--devtools`),
`RAY_UI_BACKEND=headless` (`std/ui` windows in memory, for tests and CI),
`RAY_AUDIO_SINK=null` (a `std/audio` sink without hardware, at real-time pace),
`RAY_KEYCHAIN_FILE` (a plain file as the `std/keychain` store, for tests and CI).

## 15. Exit codes

| Code | Meaning |
|---|---|
| return of `main` | a `main -> int` exits with that value; `main -> unit` exits with 0 |
| 64 | incorrect CLI usage |
| 65 | compile error (lexical/syntax/types) or configuration error |
| 66 | input file not found |
| 69 | the binary does not include the subsystem the command needs (*slim* build without TLS/crypto: `registry keygen`/`verify`) |
| 70 | runtime error (panic, overflow, index out of range, deadlock…) |
| 73 | a file could not be created (`ray new`) |
| 74 | `ray bundle` could not write the package (work directory, libraries) or the signature does not verify (`--sign`) |
| 101 | ICE (internal compiler error — report it) |
| 0 / 1 | `ray test` exits with 0 (all green) or 1 (there were failures); 65 if a suite does not compile |

<!-- sync: sha256:fa7a228e8c90 -->
