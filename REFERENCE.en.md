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
11. [Additional packages (`net`, `rpc`, `db`)](#11-additional-packages-net-web-rpc-db)
12. [Annotations](#12-annotations)
13. [FFI: marshalable types](#13-ffi-marshalable-types)
14. [The `ray` CLI](#14-the-ray-cli)
15. [Exit codes](#15-exit-codes)

---

## 1. Keywords

Reserved (cannot be used as identifiers):

| Group | Words |
|---|---|
| Declarations | `fn` `let` `var` `const` `struct` `enum` `trait` `impl` `extern` |
| Control | `if` `else` `while` `for` `in` `match` `return` `break` `continue` |
| Modules | `import` `pub` (`from` is **contextual**: only at the start of an item, `from M import x;`) |
| Values/types | `true` `false` `dyn` `as` `self` `Self` |
| Primitive types | `int` `float` `bool` `string` `char` `bytes` `ptr` `u8` `u32` `u64` |

> `from` is valid as a parameter or variable name (M192): it is a keyword only at the head of a
> `from M import …;`.

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
| `==` `!=` | structural equality (primitives, strings, bytes; user types via `Eq`) |
| `<` `<=` `>` `>=` | comparison: numbers, strings (lexicographic), chars (code point) |
| `&&` `\|\|` `!` | logical (short-circuit in `&&`/`\|\|`) |
| `&` `\|` `^` `~` `<<` `>>` | bitwise on `int`/`u8`/`u32`/`u64` (*wrapping* semantics; the shift masks the width) |
| `=` | assignment (a statement, **not** an expression) |
| `( )` | grouping, calls, tuples, scrutinee of `match`/`if`/`while` |
| `{ }` | blocks (produce values), struct literals, bodies |
| `[ ]` | array literals `[1, 2, 3]` (trailing comma allowed), types `[T]`, indexing `a[i]` |
| `,` `;` `:` | separators; end of statement; type annotation |
| `.` | field, method (UFCS), enum variant (`Option.Some`), module (`math.PI`), tuple (`t.0`), and the `builtin` pseudo-module (`builtin.close(h)`: the builtin even when the module defines its own `close`, M196) |
| `..` | range in `for i in a..b` (half-open) |
| `->` | function return type |
| `=>` | `match` arm |
| `?` | propagation of `Err`/`None` (early return); with `impl From<E1> for E2`, converts the error |
| `\|>` | pipeline: `x \|> f(a)` ≡ `f(x, a)` |
| `@` | annotations: `@test`, `@derive(…)` |
| `_` | wildcard in patterns; discard in `let _ = …` |
| `${…}` | interpolation inside a string literal |
| `//` `///` | line comment; documentation comment (`ray doc`, LSP hover) |
| `b"…"` | bytes literal |

## 3. Literals and escapes

| Literal | Form | Notes |
|---|---|---|
| Integer | `42`, `-7`, `0xFF`, `0o755`, `0b1010`, `255u8`, `0xFFu32` | decimal or with a `0x`/`0o`/`0b` prefix (hex/octal/binary, uppercase too). Optional suffix `u8`/`u32`/`u64` = that type without context (M192). Without a suffix: fits in `int` → `int` (coerces to the `u*` of the context); does not fit in `int` but fits in `u64` (`0xFFFFFFFFFFFFFFFF`) → **wide**, `u64`. No `_` (deferred) |
| Float | `3.14` | `digits . digits` (the `.` requires a fraction: `2.0`, not `2.`); always decimal |
| Boolean | `true` / `false` | |
| String | `"hello"` | escapes `\n \t \r \\ \" \$ \0`, `\xNN` (hex octet, U+0000..U+00FF) and `\u{H…H}` (1–6 hex digits, Unicode code point); no literal line breaks |
| Interpolated string | `"x = ${expr}"` | `${expr}` = **one** expression; desugars to `+ to_string(expr)`. `\${` = literal. `"$5"` and `"{n}"` are literals (the `$` is only special before `{`) |
| Char | `'a'`, `'\n'`, `'\x41'`, `'\u{1F600}'` | one Unicode code point; escapes `\n \t \r \\ \' \0`, `\xNN` and `\u{H…H}` |
| Bytes | `b"ok\x00\xff"` | string escapes + `\xNN` (octet in hex) |
| Array | `[1, 2, 3,]` | trailing comma allowed; an empty `[]` needs context or an annotation |
| Tuple | `(1, "a")` | access `t.0`, `t.1`; destructuring `let (a, b) = t;` |
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
| `bytes` | immutable sequence of octets; `b[i] -> int`. Performance: `+` on `bytes` is amortized linear (gathering 8 MB in 128 chunks: ~20 ms); building octet by octet with `push` into `[int]` + `bytes_of` is two orders of magnitude slower — accumulate `bytes` with `+` or `sub_bytes` |
| `unit` | "no useful value" (return of `print`, etc.) |
| `[T]` | dynamic array, **reference** semantics |
| `(A, B, …)` | tuple (immutable aggregate, copied as a value; `t.0 = x` is an error) |
| `Map<K, V>` | key→value table; *hashable* keys: int/u\*/string/char/bool/bytes (**not** float); traversed in key order (deterministic) |
| `fn(A, B) -> R` | function type (functions and closures are first-class values) |
| `Option<T>` / `Result<T, E>` | from the prelude; absence and error as values (there is no `null` and no exceptions) |
| `Channel<T>` / `Task<T>` | concurrency (SPEC §9) |
| your own `struct` / `enum` | reference semantics (struct/enum); generics with bounds (`struct Box<T: Show>`) |
| `dyn Trait`, `dyn A + B` | trait objects (dynamic dispatch); *upcasting* to a subset of traits |
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
| `to_string` | `(value) -> string` | textual representation (same as `print`): int/float/bool/string/char/bytes/u\* |
| `panic` | `(msg: string) -> unit` | aborts the program with the message and the position; for broken invariants, not for expected errors |
| `exit` | `(code: int) -> unit` | M130: terminates the PROCESS with that code, from any fiber (flushes stdout/stderr). Diverges like `panic`; it is not an error (no message, no trace) and `try_call` does not catch it |
| `args` | `() -> [string]` | command-line arguments (after the program path) |

### Concurrency (VM and native binary — the interpreter has no fibers; §11 of the manual)

| Function | Signature | Description |
|---|---|---|
| `spawn` | `(f: fn() -> T) -> Task<T>` | launches a concurrent task; `join` waits for its value |
| `join` | `(t: Task<T>) -> T` | blocks until the task finishes (re-raises its failure). *Ad-hoc*: `join(arr, sep)` is the string one |
| `scope` | `(body: fn() -> R) -> R` | structured concurrency: on return it joins every task launched inside; if one fails, it cancels its siblings and propagates |
| `send` | `(ch: Channel<T>, v: T) -> unit` | sends; blocks if the bounded channel is full (backpressure) |
| `recv` | `(ch: Channel<T>) -> Option<T>` | receives; blocks while empty and open; `None` once closed and drained |
| `select` | `(chs: [Channel<T>]) -> int` | blocks until a channel is ready; returns the lowest ready index (deterministic) |
| `try_recv` | `(ch: Channel<T>) -> Received<T>` | receives **without blocking**: `Received.Got(v)` (a value was ready, it consumes it), `Received.Empty` (open and empty), `Received.Closed` (closed and drained). For "check for data OR a control command without getting stuck" |
| `select_timeout` | `(chs: [Channel<T>], ms: int) -> Option<int>` | `select` with a **deadline**: `Some(i)` (lowest ready index), `None` if the `ms` ms elapse; `ms <= 0` = non-blocking poll. Event-driven (wakes when a channel arrives, does not poll) |
| `signals` | `() -> Channel<int>` | M88.1/M107.4: the OS signal channel (SIGTERM=15, SIGINT=2, SIGWINCH=28); a process singleton, for graceful shutdown and re-layout on resize (`select` + `term.size()`) — composes with `recv`/`select`. Unix; VM and native binary |
| `try_send` | `(ch: Channel<T>, v: T) -> bool` | sends **without blocking or failing**: `true` if delivered (a parked receiver) or queued (room), `false` if the channel is closed or full. For producers whose consumer may be gone (M190) |
| `close` | `(ch \| handle) -> …` | closes a channel (pending values can still be received; blocked senders wake up and their `send` fails; idempotent) **or** a file/socket handle |

### Failure recovery

| Function | Signature | Description |
|---|---|---|
| `try_call` | `(f: fn() -> T) -> Result<T, string>` | runs `f` and turns a `panic`/runtime error into `Err(message)`. Recovers in the **same fiber**: whatever `f` mutated stays mutated (like Rust's `catch_unwind`). All three engines |
| `try_join` | `(t: Task<T>) -> Result<T, string>` | a task's failure as a value instead of re-raising it. True isolation (the fiber's own heap). VM and native |

> ⚠️ **Math, clock, randomness, crypto, disk and network are NOT global builtins.** They live in
> `std/` modules since M49/M50 and are used qualified: `math.sqrt(2.0)`, `time.now()`,
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
| `sort` | `(xs: [T: Ord]) -> [T]` | sorts an array (sorted copy) |
| `assert` / `assert_eq` | `(bool)` · `(a: T, b: T)` | test-runner assertions; they fail with `panic` |

## 6. Methods by receiver type

Called with a dot (`recv.method(args)`); almost all are prelude traits or method-builtins, so they
also exist as free calls (`method(recv, args)`).

### `string`

| Method | Result | Description |
|---|---|---|
| `s.len()` | `int` | length **in characters** |
| `s[i]` | `char` | indexing by character (out of range = error); `s[i] = c` is forbidden (immutable) |
| `s.trim()` | `string` | without surrounding whitespace |
| `s.split(sep)` | `[string]` | parts |
| `s.contains(sub)` | `bool` | substring |
| `s.replace(from, to)` | `string` | replaces all |
| `s.chars()` | `[char]` | characters |
| `s.starts_with(p)` / `s.ends_with(p)` | `bool` | prefix/suffix |
| `s.to_upper()` / `s.to_lower()` | `string` | uppercase/lowercase |
| `s.substring(i, j)` | `string` | `[i, j)` by character, *clamped* (never fails) |
| `s.repeat(n)` | `string` | repeated (`n <= 0` → `""`) |
| `s.index_of(sub)` | `Option<int>` | index of the first occurrence |
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
| `a.sort()` | `[T]` | sorted copy (`T: Ord`) |
| `a.join(sep)` | `string` | only `[string]` |
| `a.map(f)` / `a.filter(p)` / `a.fold(init, f)` | eager | materialize an array/value (§9 for the lazy version) |
| `a.iter()` | `Iter<T>` | lazy iterator |
| `a + b` | `[T]` | concatenation |

### `bytes`

| Method | Result | Description |
|---|---|---|
| `b.len()` | `int` | octets |
| `b[i]` | `int` | octet (0–255) |
| `b.sub_bytes(i, j)` | `bytes` | slice `[i, j)`, *clamped* |
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
| `assert_eq` | `(a: T, b: T)` with `T: Eq + Show` | aborts showing both values |
| `pop` / `position` / `index_of` / `from_utf8` | — | see §6 |

### Traits

| Trait | Method(s) | Notes |
|---|---|---|
| `Eq` | `eq(self, other: Self) -> bool` | enables `==`/`!=` on user types; derivable |
| `Show` | `show(self) -> string` | enables `print`/`to_string`; derivable |
| `Ord` | `less(self, other: Self) -> bool` | enables `sort`/`min`/`max`; impls for int/float/string/char |
| `Hash` | `hash(self) -> int` | keys of `Set`; derivable |
| `Add` `Sub` `Mul` `Div` | `add/sub/mul/div(self, other: Self) -> Self` | overloading of `+ - * /` on user types |
| `Neg` | `neg(self) -> Self` | overloading of unary `-` |
| `From<S>` | `convert(source: S) -> Self` | conversion; `?` uses it to convert errors (`from` is a keyword → the method is called `convert`) |
| `Iterator<T>` | `next(self) -> Option<T>` | the iteration protocol; brings the adapters as default methods |
| `Len` / `Push<T>` / `Contains<T>` | `len`/`push`/`contains` | the container methods, as traits |
| `Signed` | `abs(self) -> Self` | for the generic `abs` of `std/math` |

`Option<T>` (`Some`/`None`) and `Result<T, E>` (`Ok`/`Err`) are prelude enums.

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
| `std/text` | `is_empty pad_left pad_right capitalize reverse count words lines` · Unicode normalization (M131): `nfc nfd nfkc nfkd` (the K forms flatten presentation variants; an accent-insensitive slug = `nfd` + drop combining marks U+0300..U+036F). Without the `unicode` feature (slim): a clear error |
| `std/sort` | `is_sorted sort_desc min max binary_search dedup merge` (all with `T: Ord`) |
| `std/fs` | `read_file write_file append_file remove_file list_dir exists mkdir read_file_bytes write_file_bytes` (→ `Result`) · handles: `open(path, "r"/"w"/"a") -> Result<int, _>` `read_line(h) -> Option<string>` `write(h, s)` + `close(h)` · **streaming** (M113): `read_bytes(h, max) -> Result<Option<bytes>, string>` (up to `max` octets from the current position — exact except near the end; `None` = EOF; memory bounded by what is read) · `seek(h, pos) -> Result<int, string>` (absolute position from the start; returns the new position → resumable transfers) · **durability** (M115.1): `write_bytes(h, data) -> Result<int, string>` (raw octets at the handle's current position — the binary twin of `write`; composes with `seek`) · `sync(h) -> Result<int, string>` (flushes the buffers AND forces the file to stable storage — fsync; without it an append survives a process crash but not a power cut) · **locks** (M115.2): `try_lock(h) -> Result<bool, string>` (EXCLUSIVE advisory lock without blocking — flock; `true` = acquired, `false` = another open file description holds it; the single-process LOCK-file pattern) · `unlock(h) -> Result<int, string>` (`close(h)` also releases it) · **metadata** (M115.3): `stat(path) -> Result<Stat, string>` — WITHOUT following symlinks (lstat): `Stat { kind, mode, size, mtime_ms }` with kind `"file"`/`"dir"`/`"symlink"`/`"other"`, mode = the 12 permission bits in decimal (0o600 = 384), size in bytes (of a symlink: the length of the link itself), mtime in epoch-ms · `chmod(path, mode) -> Result<int, string>` (changes the permission bits; 384 = 0o600, 493 = 0o755) · **watch** (M115.4, KERNEL events — FSEvents/inotify, not mtime polling): `watch(path) -> Result<int, string>` (directory → recursive; file → itself; `close(h)` stops it) · `next_event(h) -> Result<WatchEvent, string>` (waits as long as needed — the fiber PARKS, the process sleeps) · `next_event_timeout(h, ms) -> Result<Option<WatchEvent>, string>` (`None` = deadline elapsed; useful for coalescing bursts) · `WatchEvent { kind, path }` with kind `"create"`/`"modify"`/`"remove"`/`"rename"`/`"other"` — kinds can be coarse depending on the platform: treat the event as "something changed here" and re-examine |
| `std/io` | the console by bytes. Writing **without a newline**: `write(s)` / `ewrite(s)` (stderr) / `write_bytes(b)` → `Result<int, string>` (number of characters/bytes) · `flush() -> Result<int, string>`. stdout is buffered: after a `write` without `\n`, call `flush()` to see it; `ewrite` is visible at once; `write_bytes` does not go through UTF-8. Reading: `read(max) -> Option<bytes>` (1..=max octets; `None` = EOF) · `read_timeout(max, timeout_ms) -> ReadResult` (`Data(bytes)` \| `Eof` \| `TimedOut`; `0` = pure poll). On the VM a read without data **parks the fiber**, not the VM; a single stdin reader at a time; do not mix with `input()`/`fs.read_line` on stdin (those read buffered, this reads the raw fd). The order with respect to `print`/`eprint` is program order |
| `std/term` | the terminal. `is_tty(fd) -> bool` (0/1/2) · `size() -> Option<(int, int)>` (cols, rows) · `raw<T>(f: fn() -> T) -> Result<T, string>` — runs `f` in **raw** mode (no echo, byte by byte, no signals) and ALWAYS restores (also if `f` fails, and at process exit via `atexit`; a fatal signal/`kill -9` leaves the terminal raw → `reset`) · `read_key() -> Option<Key>` (one key; `None` = EOF; a lone ESC resolves after 25 ms) · `decode(b: bytes) -> Option<(Key, int)>` — the **pure** decoder (key + octets consumed; `None` = incomplete prefix), to process bursts or test without a tty · `enum Key { Char(char) Enter Tab Backspace Esc Up Down Left Right Home End PageUp PageDown Insert Delete Ctrl(char) F(int) }`. In raw mode there is no OPOST: end lines with an explicit `\r\n`. **Cell width** (M117, portable — needs no tty): `width(s: string) -> int` (terminal cells, not characters) · `char_width(c: char) -> int` (pragmatic wcwidth: control/combining → 0, CJK/kana/fullwidth/emoji → 2, the rest → 1) · `fit(s, cells) -> string` (truncates to `cells` without splitting a wide character and pads with spaces; left) · `fit_right(s, cells) -> string` (pads on the left, for numeric columns). **Hidden input** (M125): `read_hidden(prompt) -> Result<string, string>` — one line WITHOUT echo (passphrases; prompt to stderr like getpass(3), Backspace deletes a whole UTF-8 character, Ctrl-C = `Err("interrupted")`, no tty = `Err`) · the **pure** core `hidden_feed(acc: bytes, chunk: bytes) -> Hidden` (`More(bytes) Done(string) Cancelled`), testable without a tty. **Graphical terminal** (M143, IDEAS §78): `size_px() -> Option<(int, int)>` (area in PIXELS via `ws_xpixel`/`ws_ypixel`; `None` if the terminal does not report them — many leave 0) · `cell_px() -> Option<(int, int)>` (pixels of ONE cell = area/grid — to scale sixel/kitty graphics to the layout) · `capabilities() -> Capabilities { truecolor, colors_256, sixel, kitty_graphics }` (with stdin AND stdout on a tty it asks the terminal ITSELF — a DA1 query for sixel + the kitty graphics APC probe (M161), one raw session, ~150 ms deadline; under tmux the probe yields `false`, correctly: APCs do not pass; without a tty, kitty falls back to the `TERM`/`KITTY_WINDOW_ID` env hint; everything undetectable is `false`: degrade, never guess upwards) · `parse_device_attributes(resp: bytes) -> [int]` — the **pure** DA1 parser (`ESC [ ? 64;1;4 c` → `[64, 1, 4]`; malformed → `[]`) · `parse_graphics_reply(resp: bytes) -> bool` — the **pure** parser of the kitty probe (`;OK` → true). **Kitty graphics** (M161, IDEAS §83 — kitty/Ghostty/WezTerm; ids `> 0` chosen by the caller, stable; 1-based cells; `q=2` silent and without moving the cursor; they EMIT even without a tty — check `capabilities().kitty_graphics` first; under tmux nothing is drawn): `transmit_image(id, img: image.Image) -> Result<int, string>` (uploads the pixels WITHOUT showing — once per sprite) · `place_image(id, col, row, cols, rows) -> Result<int, string>` (shows what was transmitted, ~30 octets per frame; `cols`/`rows` scale to cells, `0` = natural size) · `draw_image(id, col, row, img) -> Result<int, string>` (transmit+show, the convenience) · `draw_png(id, col, row, data: bytes) -> Result<int, string>` (the terminal decodes the PNG — compressed bytes, for assets) · `clear_image(id)` (removes from screen; the terminal KEEPS the pixels: `place_image` without retransmitting) · `clear_images()` · `kitty_chunks(control: string, payload: bytes) -> string` — the **pure** brick: assembles any APC command of the protocol with the mandated chunking (4096 base64 chars/chunk), for what is not covered (animation, z-index) |
| `std/net` | `tcp_connect tcp_listen tcp_accept local_port` · `tcp_connect_timeout(host, port, ms)` (M122: an attempt that exhausts the deadline fails with the stable error `"connect timeout"` instead of the OS's ~75 s against a host that drops SYNs; bounded but blocking wait) · `socket_read socket_write socket_read_bytes socket_write_bytes` · `shutdown_write(h) -> Result<int, string>` (M130: half-close `SHUT_WR` — the peer sees EOF, this side keeps reading; the netcat/HTTP-1.0 idiom; TCP only) · `peer_addr(h) -> Result<string, string>` (M123: the peer's address `"ip:port"` of a TCP/TLS connection; IPv6 in brackets) · `set_read_timeout(h, ms)` (M56.4/M121: a read that waits longer fails with the stable error `"read timeout"`; applies to TCP, TLS **and UDP**) · `tls_connect tls_connect_h2 tls_accept tls_upgrade` (STARTTLS) · `tls_peer_cert(h) -> Result<PeerCert, string>` (M124: the peer's certificate — `PeerCert { subject, issuer, not_before_ms, not_after_ms, san }`; "expires in N days" = `(not_after_ms - time.now()) / 86400000`; drives the pending handshake, bounded to 10 s) — all `Result`; + `close(h)` |
| `std/process` | OS processes, **without a shell** (typed argv). `run(program, args) -> Result<Output, string>` · `cmd(program, args) -> Cmd` + chainable builder `.dir .env .env_clear .stdin(bytes)` (written and CLOSED; without it, the child reads `/dev/null`) `.timeout_ms .max_output .merge_output .run()`. `Err` = only "could not launch"; exiting ≠ 0 or dying by signal is `Ok`. `Output { exit: Exit, stdout: bytes, stderr: bytes, timed_out, truncated }` with `Exit.Code(int)` \| `Exit.Signal(int)` (never `128+sig`). The timeout returns the PARTIAL Output with `timed_out` after killing the child's GROUP; `truncated` marks the capture cap (~16 MB by default). **Streaming** (VM/native): `.stream() -> Result<Proc, string>` with `Proc { out, err: Channel<bytes>, … }` (BOUNDED channels = backpressure; their close = end of stream; with merge, `err` is born closed) · `Proc.wait() -> Exit` (reaps; once) · `Proc.kill(force)` (signal to the GROUP; no-op after wait). **Persistent session** (M100 v3): `.stdin_pipe()` leaves the child's stdin OPEN and `Proc.write(bytes) -> Result<int, string>` / `Proc.close_stdin()` feed it while it lives — what an MCP/LSP client or a REPL driver needs (write request → read reply → repeat). `write` writes ALL the data and **parks the fiber** if the pipe fills (backpressure); a child that closed its stdin or died yields `Err` (a visible EPIPE, not silence); `close_stdin` IS the EOF the child waits for. It takes precedence over `.stdin(data)`. The process is a SCOPE CHILD: a failing sibling kills and reaps it, and one without `wait()` does not outlive its scope. `stream()` has no `timeout_ms`/`max_output` (the channel is the cap; the deadline composes with deadline + kill). macOS, Linux and Windows (M175) |
| `std/time` | `now monotonic sleep` + civil UTC dates (M57.1): `DateTime`, `now_utc`, `from_epoch_millis`/`to_epoch_millis`, `to_iso8601[_basic]`, `date_stamp`, `to_rfc1123`, `parse_iso8601[_millis]` (RFC 3339 with offset/fraction), `format_duration` (`net/time` remains as a re-export) · duration constructors → **ms** (the stdlib's currency): `millis seconds minutes hours days` — imported unqualified they enable the UFCS form `30.seconds()`, `2.hours()` |
| `std/units` | size constructors → **bytes**, binary convention (1 KB = 1024): `kb mb gb` — imported unqualified they enable the UFCS form `64.kb()`, `16.mb()` |
| `std/random` | `next() -> float` (in `[0,1)`) · `below(n) -> int` · `between(lo, hi) -> int` · `choice(xs) -> Option<T>` · `shuffle(xs)` (shuffles **in place**, returns unit) · `seed(n)` (reproducible sequence) |
| `std/crypto` | **production** crypto (backed by `ring`, constant time): `sha256 sha512 sha1` (`bytes -> bytes`; `sha1` is legacy, only for protocols that require it) · **incremental hasher** (M126, for large files in chunks): `sha256_init()`/`sha512_init() -> Result<int, string>` + `hash_update(h, chunk) -> Result<int, string>` + `hash_final(h) -> Result<bytes, string>` (`final` CONSUMES the handle; digest identical to the one-shot) · `hmac_sha256(key, msg) -> bytes` · `ed25519_public_key(seed)` / `ed25519_sign(seed, msg)` → `Option<bytes>` (`None` if the seed is not 32 octets) and `ed25519_verify(pubkey, msg, sig) -> bool` (total) · `chacha20poly1305_seal(key, nonce, aad, plain)` → `Option<bytes>` (`ciphertext ‖ tag`) and `chacha20poly1305_open(…)` (`None` if authentication fails) · `random_bytes(n)` (CSPRNG) · **key agreement** (backed by `x25519-dalek`): `x25519_public_key(secret)` / `x25519_shared_secret(secret, peer_public)` → `Option<bytes>` (`None` if either key is not 32 octets, or if the peer's public key has small order — all-zero output) · `hkdf_sha256(salt, ikm, info, len)` → `Option<bytes>` (RFC 5869; `None` outside `1..=8160`; a different `info` → an independent key) · `constant_time_eq(a, b) -> bool` (total). The secret of `x25519_shared_secret` is the **raw** DH: always pass it through `hkdf_sha256` before using it as an AEAD key. The pure-raylang versions in `examples/web/` are demonstrations, not production |
| `std/crypto/md5` `std/crypto/aes` `std/crypto/des` | **LEGACY crypto in pure raylang** (M194): for talking to protocols that require it, not for designing (not constant time; for anything new, `std/crypto`). `md5.md5(bytes) -> bytes` (RFC 1321) · `aes.encrypt_block/decrypt_block(key, block)`, `encrypt_ecb/decrypt_ecb(key, data)`, `encrypt_cbc/decrypt_cbc(key, iv, data)` (keys of 16/24/32 octets = AES-128/192/256; whole blocks, no padding) · `des.encrypt_block/decrypt_block`, `encrypt_ecb/decrypt_ecb`, `encrypt_cbc/decrypt_cbc` (8-octet key) and **3DES** `tdes_encrypt_block/tdes_decrypt_block`, `tdes_encrypt_ecb/tdes_decrypt_ecb`, `tdes_encrypt_cbc/tdes_decrypt_cbc` (24-octet key = K1 K2 K3, or 16 = K3 K1). All `-> Result<bytes, string>`; official vectors in `tests/crypto_legacy_cli.rs` |
| `std/bigint` | **unsigned big integers** on big-endian `bytes` (M195; `num-bigint` runtime, feature `bigint`, `--without bigint` on native): `from_int(n) -> bytes`, `from_hex(s) -> Result<bytes,_>`, `from_bytes(b)` (normalizes), `to_hex(a) -> string`, `to_int(a) -> Option<int>`, `bit_len`, `is_zero`, `cmp(a, b) -> int` · arithmetic `add sub mul div rem gcd shl shr` and **`modpow(base, exp, m)`**, `modinv(a, m)` (all `-> Result<bytes, string>`: division by zero, negative subtraction, no inverse). A 4096-bit `modpow` costs milliseconds (before, seconds in pure raylang). **Not constant time**: client-side DH/RSA yes; a server's private key under a timing attack no |
| `std/resilience` | M88.2, the resilience kit for services: `Retry`/`policy(attempts, base_ms, max_ms)` + `retry<T,E>(p, f)` (exponential backoff + jitter; returns the first `Ok` or the last `Err`) · `Breaker`/`breaker(threshold, cooldown_ms)` + `guard<T,E>(b, err_open, f)` (fail-fast circuit breaker; the open-circuit error is supplied by the caller) + `is_open` · M129: the composable pair `admit(b) -> bool` / `report(b, ok)` — the loose transitions for when the call runs in another fiber/actor (`guard` is sugar over them) · `Deadline`/`deadline(ms)` + `remaining expired` (monotonic time budget; apply it to I/O with `net.set_read_timeout(h, remaining(d))`) |
| `std/collections/set` | `Set<T>` (requires `T: Hash + Eq`): `new add has remove size items` → `set.new()`, `set.add(s, x)`… |
| `std/collections/deque` | `Deque<T>`: `new len is_empty push_back push_front pop_front pop_back peek_front` |
| `std/collections/stringbuilder` | `StringBuilder`: `new push build count` (joins once; avoids the O(n²) of `+` in a loop) |
| `std/collections/dict` | `Dict<K, V>` — a GENERIC hash map (M82): USER keys via the `Hash` + `Eq` traits (the builtin `Map<K,V>` requires primitive keys). `new insert get has remove size keys values` (module functions: `dict.insert(d, k, v)`) |
| `std/kv` | `Store` — persisted key/value state (M83): `open(path) -> Result<Store, _>`/`empty(path)`, and the operations are METHODS of the `StoreOps` trait — `s.get(k) -> Option<bytes>` · `s.set(k, v: bytes)` · `s.get_string(k) -> Option<string>` · `s.set_string(k, v)` · `s.delete(k) -> bool` · `s.keys() -> [string]` · `s.save() -> Result<int, _>` (atomic save: temp + rename) · **`s.incr(k, delta) -> Result<int, _>`** (M154: atomic add; absent = 0; a decimal UTF-8 value, readable with `get_string`; non-integer = Err) · **`s.set_if(k, expected: Option<bytes>, new) -> bool`** (CAS; `None` = only-if-absent) — NOT free functions (`kv.get(s, k)` does not exist). `share`/`open_shared`/`stop` = the ACTOR form for access across fibers (same methods on the handle; `incr`/`set_if` run WHOLE in the owning fiber — atomic under concurrency). Motivated by `ray dev` (sessions/config surviving reloads) |
| `std/json` | `enum Json` (`JNull JBool JNum JStr JArray JObject`) · `parse -> Result<Json, string>` · `stringify` (canonical, sorted keys). `\uXXXX` escapes with surrogate pairs |
| `std/hex` | `hex_encode(bytes) -> string` · `hex_decode(string) -> Result<bytes, string>` |
| `std/base64` | `base64 base64url` (`bytes -> string`) · `base64_decode base64url_decode` (`string -> Result<bytes, string>`) |
| `std/url` | `url_encode url_decode parse_query build_query` |
| `std/regex` | Thompson NFA engine (linear time): `full_match search find find_str find_all replace_all`. Supports `. * + ? \| ( ) [a-z] [^…] \d \w \s ^ $` · compiled: `compile -> Result<Regex, string>` + trait `Matcher` (`captures`/`captures_str`) · NAMED groups `(?P<n>…)`/`(?<n>…)` (M128): `group_names(re) -> [string]` · `captures_map(re, s) -> Option<Map<string, string>>` |
| `std/csv` | `parse_csv -> Result<[[string]], string>` (RFC 4180) · `write_csv` · incremental (M128): `parser()` + `feed(p, chunk) -> [[string]]` (completed rows) + `finish(p) -> Result<[[string]], string>` (the tail; an unclosed quote = Err) — chunks may be cut anywhere |
| `std/toml` | `parse_toml toml_get toml_show` (subset: tables, scalars, arrays) · `[[path]]` arrays of tables (M128): flattened as `path.N.key` + `toml_array_len(entries, path)` |
| `std/template` | Jinja-style templates: `compile(tpl) -> Result<Template, _>` + `render(t, ctx)` (SSR: compile once) · `render_template` (one-shot) · `{{ var }}` (autoescape), `{{& var }}`, `{% if/elif/else %}`, `{% for %}` · context: `ctx_str ctx_int ctx_bool ctx_list val_str val_int` · `escape_html` · COMPILED templates: importable `.ray.html`, compiled in memory (§14) |
| `std/markdown` | Markdown → typed AST + HTML (M111). `parse(md) -> [Block]` · `render(blocks) -> string` · `to_html(md) -> string` · `parse_inline(s) -> [Inline]`. `enum Block { Heading(int, [Inline]) Paragraph Code(lang, text) Quote([Block]) List(ordered, start, [[Block]]) Rule Table(aligns, header, rows) }` · `enum Inline { Text Code Emph Strong Link([Inline], href) Image(alt, src) }`. CommonMark subset: ATX `#`..`######`, paragraphs, fenced code with language, nested lists (-/*/+ and `1.`; a type change = a new list; the number of the first marker is the `start`: `2.` → `<ol start="2">`), quotes `>`, `---`, **GFM tables** (header + separator `|---|:--:|`; escaped `\|`; short rows padded; without a separator there is no table), **Mermaid** diagrams (a ` ```mermaid ` fence emits `<pre class="mermaid">` with the escaped text — rendering is client-side with mermaid.js; in the AST it is still `Code("mermaid", …)`), emphasis/bold (an intra-word `_` does NOT create emphasis — rule 17: `snake_case_name` is literal)/`code`/[links]/![images], `\` escapes. **Secure by design**: embedded HTML is ESCAPED (not interpreted) and `javascript:`/`vbscript:`/non-image `data:` URLs are neutralized to `#` — the output can be served without a sanitizer. Out of v1: setext, footnotes, hard breaks |
| `std/audio` | **PCM audio output** (M145): `open(sample_rate, channels) -> Result<int, string>` (default device; interleaved s16le; 8000–192000 Hz, 1–8 channels) · `write(h, samples) -> Result<int, string>` — writes EVERYTHING and **parks the fiber** if the device is full: backpressure IS the pacing (synthesize as fast as you can; the device sets the tempo) · `drain(h)` (waits for what was written to play — before `close(h)` for a clean ending) · **`open_latency(rate, channels, latency_ms)`** (M158: the hint sizes ring/buffers/chunk — 20–1000 ms, 0 = default 200; on Android ≤50 ms requests LOW_LATENCY) · **`played_ms(h) -> Result<int, _>`** (M158: the REAL playback position — AudioQueueGetCurrentTime / snd_pcm_delay / AAudioStream_getFramesRead, refreshed ~latency/4 — to sync visuals) · `close(h)` (the generic one) ends the output. Backends: AudioQueue (macOS), ALSA via dlopen (Linux; without libasound → a clear `Err`), **AAudio via dlopen (Android, M158)**, `RAY_AUDIO_SINK=null` = real-time sink (tests/CI without a sound card). `--without audio` excludes it |
| `std/ui` | **window + webview** (M146, IDEAS §80 F1 — the desktop-app primitive): `open(title, url, width, height) -> Result<int, string>` — a native window with the SYSTEM webview loading `url` (small app: the built-in IPC bridge — `window.ray.send(text)` → a `"message"` event with `tag`=text and `window`=handle (0 on iOS), M152; non-strings travel as JSON and `request(v)` returns a Promise the program resolves with `as_request(e) -> Option<(int, string)>` + `reply(window, id, value)` — M157, on top of the existing eval_js — and `eval_js` back; with a backend: your embedded webserver on `127.0.0.1` and the web framework as IPC) · `eval_js(h, js)` (fire-and-forget) · events (a per-process queue; kinds: `"closed"` — exactly one per window —, `"menu"`, `"message"`; headless injects messages with `RAY_UI_MSG`): `next_event() -> Result<UiEvent, string>` (the fiber parks) · `next_event_timeout(ms) -> Result<Option<UiEvent>, string>` · `events() -> Channel<UiEvent>` (pump fiber; VM/native) · `split_events() -> (Channel<UiEvent>, Channel<UiEvent>)` (M159: `(messages, other)` — `"message"` events on the first, the rest on the second; ONE pump fiber: a single consumer like events(), call it once; the queue has a **hard cap** of 65536 — when full the oldest `"message"` is dropped, never a `"closed"`, with a stderr warning; only the main frame reaches the bridge on macOS/iOS) · `close(h)` (the generic one) closes the window · **menus** (M148): the standard App/Edit menu installs ITSELF (⌘Q/⌘W and the webview's clipboard/undo — without it, ⌘C/⌘V do not travel on macOS); `menu(title, [MenuItem{tag,title,shortcut}]) -> Result<int,_>` adds custom menus (click → a `"menu"` event with `tag`; shortcut = one character ⌘+key on macOS, uppercase adds ⇧; Linux v1 click-only and the menubar is per-window: it applies to windows opened afterwards) · `app_menu(name, [MenuItem]) -> Result<int,_>` (M151) puts items in macOS's **APPLICATION menu** (above Hide/Quit, with a separator) and a non-empty `name` retitles it (under `ray run` it read "ray"); the `"role:about"` tag installs the **native About** (standard panel, no event) and `set_about(name, version, description, copyright) -> Result<int,_>` (M155) declares its content — name in bold, a "Version …" line, description (as credits, Finder-style) and copyright; `""` omits the field (the bundle's value remains: `ray bundle` sets name/version/icon from ray.toml and the copyright from `[app] copyright`); on Linux they go as a normal menu titled `name` and ALL items emit the `"menu"` event (the program shows its own about) · **file dialogs** (M148): `pick_file()` / `pick_folder()` / `save_file(suggested)` -> `Result<Option<string>,_>` (None = cancelled; MODAL — one modal at a time; headless drives them with `RAY_UI_PICK`). No `ui.run()`: the runtime captures the main thread by itself. Backends: AppKit/WKWebView (macOS), GTK3+WebKitGTK (Linux, via dlopen — without the libs or a display → a clear `Err`), Win32+WebView2 (Windows, M179; without the WebView2 Runtime → a clear `Err`; per-window menus as on Linux, no system `app_menu`: its items go as a normal menu; M183: `shortcut`s are real `Ctrl+X` accelerators — uppercase = `Ctrl+Shift+X` — and dialogs are modal to the window), `RAY_UI_BACKEND=headless` = in-memory windows (tests/CI, any OS). `--without ui` excludes it |
| `std/embed` | **project assets** (M147, IDEAS §80 F2): the files of `[native] embed = ["assets"]` from ray.toml, with the SAME namespace on every engine — keys with `/` relative to the root ("assets/app.css"), lexicographic order, hidden files excluded, no `..`. `read(path) -> Result<bytes, string>` · `list() -> Result<[string], string>`. On VM/interp they are read LIVE from disk (dev); `ray build --native` BAKES them into the binary (`--embed dirs` adds ad hoc) → self-contained, runs from any cwd (what a .app needs: Finder launches with cwd=/). The web framework serves them with `static_embedded` (content ETag + 304 + Range) |
| `std/inflate` | **DEFLATE/zlib/gzip — decompression** (RFC 1951/1950/1952; if you are looking for "zlib" or "unzip": this is it): `inflate_raw gunzip zlib_inflate` (→ `Result<bytes, string>`; `_limit(data, max_out)` forms with an anti-bomb cap, default 64 MiB) · `crc32`. Serves e.g. the IDAT of a PNG (`zlib_inflate`). **Incremental** (M193): `inflate_stream()` / `zlib_stream() -> InflateStream`, `stream_push(z, chunk) -> Result<bytes, string>` (returns what the completed blocks produced; the 32 KiB LZ77 window survives across calls; cuts at any octet; sticky error), `stream_finished(z) -> bool`, `stream_set_limit(z, max_out)` — for a zlib stream that lasts the whole session (RFB's ZRLE/Zlib/Tight, WebSocket's `permessage-deflate`) |
| `std/deflate` | **DEFLATE/zlib/gzip — compression**: `deflate_raw gzip_compress zlib_compress` |
| `std/image` | **images** (M144): `decode_png(data: bytes) -> Result<Image, string>` with `Image { width, height, pixels: bytes }` — the output is ALWAYS **RGBA8** (4 octets/pixel, rows top to bottom), whatever the PNG contains. Supports color types 0/2/3/4/6 and depths 1/2/4/8/16 (16 bits → high octet), palette + `tRNS` (palette alpha and color-key for 0/2 at 8/16), None/Sub/Up/Average/Paeth filters, CRC verified per chunk and an anti-bomb cap in the zlib (via `std/inflate`). Interlaced (Adam7) → a clear `Err` (deferred). M64 spirit: corrupt/truncated input = `Err`, never a crash. **Encoding** (M164): `encode_png(img: Image) -> Result<bytes, string>` — writes the RGBA8 `Image` as PNG (color type 6, 8 bits, no interlacing, None filter, zlib from `std/deflate`); `decode_png(encode_png(img))` returns the same pixels; `Err` if `pixels.len() != width * height * 4` or the dimensions are not positive. With `term.draw_png` or `fs.write_file_bytes` it closes the loop: generating sprites/captures from raylang without external tools |
| `std/huffman` | `huffman_encode huffman_decode` (the HPACK table of RFC 7541) |
| `std/protobuf` | `PbWriter`: `writer write_varint write_string write_bytes write_fixed64 write_fixed32 finish` · `parse -> Result<[PbField], _>` `get_int get_bytes get_string` · gRPC framing: `grpc_frame grpc_unframe` |
| `std/uuid` | `uuid_v4() -> string` · `is_uuid_v4` · `uuid_v7()`/`uuid_v7_at(ms)` (RFC 9562, time-sortable) · `is_uuid_v7` |
| `std/ffi` | `errno() -> int`: the thread's `errno` — the reason of the last failure of a POSIX-style extern C function (`fopen`/`unlink`…). **Read it immediately** after the call, with no I/O in between (§13). On wasm: 0 |

## 11. Additional packages (`net`, `web`, `rpc`, `db`)

Tier 2: they do **not** ship in the binary; they are declared in `ray.toml` (by path or git) and
imported the same way (`import net/http;` → `http.fetch(…)`). They live in the repo's `packages/`.

### `packages/web` — the application framework (Express-style, on top of `net/webserver`)

| Surface | What it does |
|---|---|
| App and routes | `new_app()` · `GET/POST/PUT/PATCH/DELETE/ALL(app, pattern, handler)` with params `/users/:id` (`c.param`), a final catch-all `/*rest`, regex `GET_re` · `mount(app, prefix, sub)` (sub-apps) · `not_found(app, h)` |
| Startup | `listen(build_app, host, port)` (blocks; the builder is a TOP-LEVEL fn — the form that also compiles natively) · **`listen_on(build_app, listener)`** (M150: the bind/serve split — `net.tcp_listen(host, 0)` + `net.local_port` first → the program KNOWS its port without a close/re-bind race, and the backlog accepts from the bind on: the desktop-app pattern) · `listen_tls` · `listen_graceful` · `listen_limits` |
| Middleware | `use_mw` (global) · `use_on(prefix, mw)` · `with_mw([mw], handler)` (per route) · `after(app, hook)` · `Step.Next/Done` · `cors(app, origin)` · `log_requests(app)` (JSON per request with a trace-id) |
| Request | `c.param/query/body/json_body/form/form_field/header_of/cookie_of/local/put_local` |
| Response | `r.text/json/json_of (ToJson)/html/status/header/cookie/redirect` |
| Static files | `static_files(app, prefix, dir)` · `static_files_cached(+max_age)` (strong ETag + 304 + Range) · **`static_embedded(app, prefix, dir)`** (M147: serves from the `[native] embed` space — live disk in dev, baked into the native binary; content ETag) |
| Sessions | `ray_session` HttpOnly cookie + `std/kv` |

Full details in [`docs/web-framework.md`](docs/web-framework.md) (in Spanish); demo in
`examples/web/framework/`. Shared state between handlers: each connection runs in its own fiber with
an isolated heap — the standard form is **`web.state`** (M154): `state(path) -> Result<AppState, _>`
(the same switch as `sessions`: persists under `ray dev`, pure memory in production) or
`state_memory()`, with `state_get(st, k)` · `state_put(st, k, v)` · `state_delete(st, k)` ·
**`state_incr(st, k, delta) -> Result<int, _>`** (atomic counter — the RMW runs in the kv actor's owning
fiber). For custom typed state, the ACTOR pattern (one owning fiber + channels); recipe in MANUAL §15.

### `packages/net` — the network stack (24 modules, pure raylang)

| Group | Modules |
|---|---|
| HTTP | `http` (fetch/request, redirects, chunked, gzip, https; **streaming** M108: `stream[_with](method, url, body, headers[, idle_ms]) -> Result<Stream, _>` — status/headers right away, `stream_read(s) -> Result<Option<bytes>, _>` delivers each chunk as it arrives, incrementally de-chunked, `Ok(None)` = clean end; `stream_close`) · `sse` (Server-Sent Events client on top of `stream`: `open(url, headers)` + `next(es) -> Result<Option<Event>, _>` with `Event { data, event, id }`, and the **pure** decoder `decode(bytes) -> Option<(Event, int)>`) · `http2` + `hpack` (framing/HPACK; M133: the decoder accepts **Huffman** literals — mandatory against real servers) · `http2_client` · `webserver` (async server + SSE; graceful shutdown M88.1b: `serve_graceful(host, port, drain_ms, handler)` on top of `signals()`, the general form `serve_shutdown[_limits]` with a `stop` channel; M123: `Request.remote` = the client's `"ip:port"` + `remote_ip(req)` without the port — per-IP rate limiting, X-Forwarded-For, logs with origin; M129: `gzip(req, resp)` negotiates `Accept-Encoding` — compresses the body if the client accepts gzip, ≥ 512 octets, no previous `Content-Encoding` and not streaming; sets `Content-Encoding: gzip` + `Vary: Accept-Encoding`; `accepts_gzip(req)` standalone) |
| RPC | `grpc_client` (unary gRPC e2e over TLS+ALPN h2; M133: dogfooded against REAL grpc-go — Huffman headers and trailers-only errors covered; `GrpcResponse { message, grpc_status }`) |
| Real time | `websocket` (server) · `websocket_client` (ws/wss) |
| Auth/identity | `jwt` (HS256; `jwt_verify_claims(secret, tok, now_ms)` = signature + `exp`/`nbf`, M128) · `jwt_eddsa` (EdDSA) · `oauth2` (client_credentials) · `scram` (SCRAM-SHA-256) · `sigv4` (AWS) · `cookie` |
| Mail | `mail` (M131): `encoded_word` (RFC 2047 B, words ≤75 chars on UTF-8 boundaries) · `header(name, value)` (encoded + folded at 78, CRLF, RFC 5322) · `base64_body` (76 columns, RFC 2045) · `dot_stuff` (CRLF + leading dot doubled, RFC 5321) · `address(display, email)` (mailbox: atext / quotes / encoded-word). It does NOT speak SMTP: it produces the strings the client (tcp_connect + tls_upgrade) writes |
| Infra | `dns` + `dns_cache` (A/AAAA/MX/CNAME/TXT/NS/SRV; the reply wait is bounded to 5 s — a lost datagram yields `Err("recv: read timeout")`, not a hang) · `udp` (`recv_from` yields the fiber and honors `net.set_read_timeout`) · `redis` (RESP2) · `postgres` (simple query; the full client is in `db`) |
| Observability | `log` (structured JSON; `with_trace` stamps `trace_id` on every line, M88.3) · `metrics` (Prometheus) · `time` (UTC DateTime, ISO 8601/RFC 1123) · `trace` (W3C Trace Context: `Trace`, `new_trace`/`child`/`traceparent`/`parse_traceparent`/`from_headers`; the webserver adopts it with `trace_of(req)` and the http client propagates it with `request_traced`/`fetch_traced`) |
| Crypto | `crypto` (adapters of the builtins for the rest of the package) |

### `packages/rpc` — raylang↔raylang RPC (M88.4)

| Piece | Surface |
|---|---|
| Protocol | frame = 4 BE octets of length + JSON payload: request `{"id","method","params"[,"deadline_ms","traceparent"]}` → response `{"id","ok"}` \| `{"id","err"}` (protobuf: deferred) |
| Server | `serve(host, port, handler)` · `serve_graceful(host, port, drain_ms, handler)` (signals + draining, M88.1b) · `serve_shutdown[_limits](…, stop, drain_ms, …)` · handler `fn(Req) -> Result<Json, string>`; `Req { method, params, deadline_ms, traceparent }`; one fiber per connection; a handler panic → `err` without killing the connection; `Limits { max_frame_bytes }` (10 MiB) |
| Client | `connect(host, port) -> Result<Client, _>` · `call(c, method, params)` · `call_deadline(…, ms)` (bounds the wait; after a timeout: reconnect) · `call_full(…, deadline_ms, traceparent)` · `disconnect` — persistent connection, correlated and validated id |
| Pool (M127) | `pool(host, port, size) -> Pool` · `pool_call`/`pool_call_deadline`/`pool_call_full` · `pool_close` — up to `size` calls IN FLIGHT at once (one connection per slot: the server serves one fiber per connection → real parallelism); lazy dialing, a checkout that PARKS when exhausted (backpressure via a bounded channel) and **automatic reconnection** after a failure (the timeout discards the desynchronized connection; the next call re-dials) |

### `packages/db` — database clients

| Module | What it gives |
|---|---|
| `db/mysql` | wire v10: `connect`/`connect_tls` · `query`/`exec` with `?` (prepared/binary) or text · native + caching_sha2 auth (full path over TLS) |
| `db/postgres` | extended wire v3: `connect`/`connect_tls` (sslRequest) · `query`/`exec` with `$1, $2…` · SCRAM |
| `db/sqlite` | embedded (rusqlite in the host): `connect(":memory:" \| path)` · `query`/`exec` with `?1…` · `last_insert_rowid` |
| `db/mongo` | OP_MSG + BSON: `connect`/`connect_tls` · `insert find update delete` (filters = BSON documents) · `run_command` · full cursors (getMore) |
| `db/bson` | `enum Bson` · `encode`/`decode` · `dump` · JSON bridge (`doc_from_json from_json to_json`) |

A uniform API across the 4 clients: `connect → Conn`, `query → Result<[[string]], string>` (mongo:
documents), `exec → Result<int, string>`, `disconnect`. Parameter binding (anti-injection) in all of them.

## 12. Annotations

| Annotation | On | Effect |
|---|---|---|
| `@test` | `fn () -> bool` or `fn () -> unit` | `ray test` runs it: bool passes if `true`; unit passes if it triggers no `assert`/`panic`. Each test runs isolated — M129: on finish, ALL the OS handles it left alive are closed (listeners included; child processes are NOT killed) — it can live in any module of the project (it runs qualified: `math.t`) and use `import`; a failure reports `at module:line:col` |
| `@derive(Eq)` | non-generic struct/enum | generates `impl Eq` (structural equality) |
| `@derive(Show)` | non-generic struct/enum | generates `impl Show` (`Name { f: v }` / `Name.Variant(v)`); supports recursive enums |
| `@derive(Hash)` | non-generic struct/enum | generates `impl Hash` (for `Set`/`Dict` keys) |
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
| `ray new <name>` | creates a project (ray.toml + src/main.ray + .gitignore) |
| `ray run [file] [--] [args…]` | runs (by default `src/main.ray`); resolves dependencies; the first `--` separates the program's arguments |
| `ray dev [file] [args…]` | like `run`, but restarts on changes to `.ray`/`.ray.html`/`ray.toml` (SIGTERM → draining with `serve_graceful`) |
| `ray build [file] [--native …]` | checks and compiles without running (0 ok / 65 error); `--native` transpiles to Rust and produces a **native binary** (24–61× the VM, byte-identical); `--lib` (§80b) emits instead a **static library** with the C entry point `ray_start()` — what a mobile shell (or any C host) links; the shell registers its ui handlers with `ray_ui_set_handlers` and pushes events with `ray_ui_push_event` |
| `ray bundle [file] [--name N] [--icon i.png] [--id com.x.y] [-o dir] [--without list] [--ios [--ios-target device\|sim\|both]]` | packages a **desktop app** (M147c): a `--release` native build (with the `[native] embed` of ray.toml) → a `.app` on macOS (Info.plist + icns via sips/iconutil + ad-hoc codesign), a directory with a `.desktop` on Linux or, on Windows (M180), a directory with `<name>.exe` (WINDOWS subsystem: no console on double-click; icon and VERSIONINFO — name, version, `[app] copyright` — embedded as resources via `UpdateResourceW`, no crates) + `<name>.lnk` (absolute target and cwd; copy it to the Start menu); no Authenticode in v1 (SmartScreen warns); **`--ios`** (§80b) generates instead the XCODE PROJECT of an iOS app — a WKWebView shell in ObjC + device and simulator staticlibs (`ray build --native --lib` inside; the xcconfig picks the `.a` by SDK; `--ios-target device|sim` builds only one side — iterating against one destination, the other build is superfluous — and the `.a` of the side not built is PRESERVED from the previous project) + Info.plist; the SAME desktop source runs on the iPhone (`ui.open` hands the URL to the shell's webview; lifecycle as `lifecycle` events). The macOS `.app` writes `NSHumanReadableCopyright` from **`[app] copyright = "…"`** in ray.toml (M155 — the About panel shows it). Simulator unsigned; device: declare the team in `[ios] development_team = "…"` of ray.toml (M151; the bundle writes it into App.xcconfig and also PRESERVES an existing signature on regeneration — before, every bundle erased it), or open it in Xcode and pick it once. `--ios` excludes `process` and `audio`. **`--android`** (M156) generates the GRADLE PROJECT (Java shell + WebView; the program as the cdylib `libray_app.so` in `jniLibs/` — the JNI symbols go inside), `--android-abi arm64|x86_64|all` (arm64 default; preserves the `.so` of the ABI not built and `local.properties`), `[android] application_id` in ray.toml; same exclusions as iOS; stdout→logcat tag `ray`; build with `gradle assembleDebug` (Gradle 9.x + JDK 17+, AGP 9 pinned); `--icon` generates the multi-density `mipmap-*/ic_launcher.png` (M160, via sips; legacy — Android 8+ masks it to a circle) and the **release signing** goes through `keystore.properties` in the root of the generated project (conditional, zero secrets in ray.toml; keystore + properties PRESERVED on regeneration — full flow in the generated README). NOTE: the .app launches with cwd=/ → assets go embedded; no signing/notarization in v1 (macOS 15+: a downloaded unsigned app requires approval in Settings) |
| `ray test [file] [filter]` | runs the project's `@test`s: the entry and all its modules (qualified: `math.t`) + each `tests/*.ray` as an integration suite; substring filter; exits with 0/1 (65 if something does not compile) |
| `ray fmt <file>... [--write]` | prints the canonical version (4-space indentation; whatever exceeds 100 columns is split: a `from … import` one name per line, a method chain one link per line, `&&`/`\|\|`/`+` chains one operand per line, and delimited lists — arguments, `fn` parameters, literals — one element per line with the closer on its own line; trailing comments stay with their operand/element and your parentheses are kept). `--write`/`-w` rewrites in place and accepts several files |
| `ray build --templates-only [path…]` | **materializes** on disk the generated module of each `.ray.html` template (`{% params %}` signature), for inspection (without paths: the project root). The normal path does not need it: the loader compiles templates **in memory** when resolving their imports (M102) and ignores a sibling `.ray` |
| `ray doc <file>` | Markdown documentation of the public surface (`///`) |
| `ray repl` | interactive REPL |
| `ray lsp` | Language Server (diagnostics, hover, go-to-definition, references, rename, completion, signature help) |
| `ray mcp` | MCP server for LLM agents: `check`/`run`/`test`/`fmt`/`doc` tools, with the code sandboxed (fuel + heap + deadline), plus the `raylang://llms.txt` resource (the distilled context [`llms.txt`](llms.txt) from the repo root, for the model's prompt). Guide: [`docs/mcp.en.md`](docs/mcp.en.md) |
| `ray add <name>[@req]` | adds a dependency from the registry (`1.2.0`, `^1.2`, `~1.2.3`, `*`) |
| `ray remove <name>` | removes it (and its cache if nobody else uses it) |
| `ray search [pattern]` | lists registry packages |
| `ray fetch` | downloads the dependencies into `.ray-deps/` |
| `ray update` | re-resolves to the newest compatible versions |
| `ray registry publish [--repo <spec>] [--sign]` | publishes this version to the registry (validates + semantic check + hash; `--sign` signs it with Ed25519 and claims/verifies the name's owner) |
| `ray registry keygen [--out F]` | generates the Ed25519 publishing key (`RAY_KEY` or `~/.ray/publish.key`) |
| `ray registry verify [dir]` | audits the signatures of an index against its owners (the index repo's CI) |
| `ray registry yank <name>@<ver> [--undo]` | withdraws/restores a published version |
| `ray upgrade [tag] [--check]` | updates `ray`/`raylang` to the latest release (or the tag); `--check` only reports (0 = up to date, 1 = a newer one exists) |
| `ray toolchain install [--rust <channel>] [--force] [--no-vendor]` | installs a PRIVATE Rust toolchain for `build --native` under `~/.ray/toolchain` (rustup `minimal` profile, without touching the user's Rust or PATH) and the release's `ray-runtime` vendor (first build offline). With `cargo` already on the PATH it installs nothing (unless `--force`) |
| `ray toolchain status` | which `cargo`/`rustc` `build --native` would use and from where (`RAY_CARGO`/`RAY_RUSTC` → PATH → private), their version, the system linker and the installed vendor; exit 1 if any is missing |
| `ray version` | version |
| `ray help` | the help: every subcommand with its flags (also without arguments) |

`run` flags:

| Flag | Effect |
|---|---|
| `--interp` | forces the interpreter (development oracle; no concurrency) |
| `--deterministic` | reproducible M:1 scheduler (one thread, FIFO); also `RAYLANG_THREADS=1` |
| `--fuel N` | VM instruction limit (for sandboxed embedding) |
| `--heap N` | cap on live heap objects (forces GC; if not enough, aborts) |

`build --native` flags:

| Flag | Effect |
|---|---|
| `-o <path>` | name of the output binary (by default the `name` of `ray.toml` or the file's *stem*; on a Windows target it always ends in `.exe` — or `.lib` with `--lib`) |
| `--release` | optimization tier `opt-level=3 + lto=fat + codegen-units=1 + target-cpu=native` (slower to compile, not portable) |
| `--fast` | swaps **checked** arithmetic for **wrapping** (does not detect overflows): more performance in exchange for a guarantee; for your own code, not for hostile input |
| `--target <triple>` | *cross-compiles* to the given triple (requires the target installed in the toolchain) |
| `--without <list>` | excludes subsystems: `crypto,tls,sqlite,regex,bigint` (they fall back to a *stub* with a clear error or to the raylang implementation) and `mimalloc,ahash,fibers,process` (which are on by default). Merged with `[native] without` of `ray.toml` |

The subsystems backed by production crates (TLS/`rustls`, crypto/`ring`, SQLite/`rusqlite`, accelerated
regex) are linked **only when the program uses them** (generated Cargo project; the binary calls the same
code as the VM via the `ray-runtime` crate). **mimalloc, aHash and fibers are on by default** — they are
what makes the default the Cargo path; `--without mimalloc,ahash,fibers` goes back to bare `rustc` with
thread-per-task. `[native] without = ["tls", …]` in `ray.toml` fixes a stable exclusion policy for the
project.

Environment variables: `SSL_CERT_FILE` (extra CAs for TLS), `RAY_INDEX` (package index; without it or
`[registry] index` the official `ray-language/ray-index` is used; empty = no index), `RAY_MIRROR`
(download mirror), `RAY_KEY` (Ed25519 publishing key), `RAYLANG_THREADS` (number of scheduler worker
threads; `1` = deterministic), `RAY_FIBER_STACK_KIB` (stack reservation per fiber in the native binary).

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
| 101 | ICE (internal compiler error — report it) |
| 0 / 1 | `ray test` exits with 0 (all green) or 1 (there were failures); 65 if a suite does not compile |

<!-- sync: sha256:ad658495fc57 -->
