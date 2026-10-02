# Performance

[Español](performance.md) · English

How to make a raylang program fast, in the order it pays to do it: measure, find where the time
goes, fix the algorithm, compile to native and, if needed, use more cores. The example is a report
over an access log, written three times.

The project is in [`examples/apps/perf-lab`](../examples/apps/perf-lab/). The raylang blocks are
copied from it and CI checks that they still are. The figures are from a MacBook Pro M3 Pro, the
median of five runs: on your machine they will differ, the ratios much less.

## 1. Measure before touching anything

The program generates its own input, always the same, and times only the work being compared.
`time.monotonic_millis()` is a clock that never goes backwards, the right one for measuring:

<!-- check: project=examples/apps/perf-lab -->
```rust
    // Time only the work being compared, not the generation of the input.
    let start = time.monotonic_millis();
    let report = if (which == "slow") {
        slow.report(lines)
    } else if (which == "parallel") {
        parallel.report(lines, 4)
    } else {
        fast.report(lines)
    };
    let elapsed = time.monotonic_millis() - start;
```

```sh
ray run -- slow 200000
```

```
slow: 200000 lines, 6005 paths, 141343 chars, 7274 ms
```

Seven seconds for 200,000 lines. Before optimizing, you need to know where they go.

## 2. Where the time goes: `ray profile`

`ray profile` runs the program and, when it ends, prints a table per function: self time (without
what it calls), the percentage, the time including its calls, how many times it was called and the
average.

```sh
ray profile -- slow 200000
```

```
   self ms  self%    incl ms     calls    avg µs  function
  5332.172  85.3%   5332.172         1 5332171.67  slow::order
   731.924  11.7%    731.924    200000      3.66  position
    90.331   1.4%    822.255         1 822255.21  slow::count
    77.339   1.2%     77.339         1  77338.88  data::log
    17.885   0.3%     17.885         1  17884.71  slow::render
```

The first version has three steps, each in its own function, and the profile tells them apart:

- **`slow::order` takes 85%.** It is a hand-written selection sort: for each path it walks all the
  others, 36 million iterations for 6005 paths.
- **`position` is 12%.** The count keeps the paths in an array and looks each line up by scanning
  it.
- **`slow::render` is 0.3%.** Building the text by concatenating in a loop looked like the usual
  suspect, and it weighs nothing.

That last line is the reason to measure: intuition would have started in the wrong place.

Two tips for reading the profile. Some library functions, such as `position`, `get` or `sort_by`,
show up under their own name; basic operations, such as `split`, `to_string` or concatenation, are
counted inside the function that uses them. And a profile only tells functions apart: if all the
work is in one, split it into steps and measure again.

## 3. Fix the algorithm

This is the count in the first version, with its linear search:

<!-- check: project=examples/apps/perf-lab -->
```rust
// Requests and total milliseconds per path.
fn count(lines: [string]) -> Tally {
    let t = Tally { paths: [], counts: [], totals: [] };
    for line in lines {
        let parts = line.split(" ");
        let path = parts[1];
        let ms = parts[3].parse_int().unwrap_or(0);
        // Linear search: every line scans the paths seen so far.
        match (t.paths.position(path)) {
            Option.Some(i) => {
                t.counts[i] = t.counts[i] + 1;
                t.totals[i] = t.totals[i] + ms;
            },
            Option.None => {
                t.paths.push(path);
                t.counts.push(1);
                t.totals.push(ms);
            },
        }
    }
    t
}
```

The second version replaces the array with a `Map`, where finding a path costs the same with ten
or ten thousand of them:

<!-- check: project=examples/apps/perf-lab -->
```rust
/// Requests and total milliseconds per path.
pub fn tally(lines: [string]) -> Map<string, Stat> {
    var stats: Map<string, Stat> = Map.new();
    for line in lines {
        let parts = line.split(" ");
        let path = parts[1];
        let ms = parts[3].parse_int().unwrap_or(0);
        // Hash lookup: constant time, however many paths there are.
        match (stats.get(path)) {
            Option.Some(s) => {
                s.count = s.count + 1;
                s.total = s.total + ms;
            },
            Option.None => stats.insert(path, Stat { path: path, count: 1, total: ms }),
        }
    }
    stats
}
```

Structs have reference semantics: `s.count = s.count + 1` modifies the value that is in the map,
without inserting it again. The hand-written sort becomes `sort_by`, and the text is composed by
joining the lines once:

<!-- check: project=examples/apps/perf-lab -->
```rust
/// The report text: one line per path, most requested first (ties by path).
pub fn render(stats: Map<string, Stat>) -> string {
    let sorted = sort_by(stats.values(), fn(a: Stat, b: Stat) -> bool {
        a.count > b.count || (a.count == b.count && a.path < b.path)
    });
    // Collect the lines and join them once.
    var out: [string] = [];
    for s in sorted {
        out.push(s.path + " " + to_string(s.count) + " " + to_string(s.total / s.count));
    }
    out.join("\n") + "\n"
}
```

```
fast: 200000 lines, 6005 paths, 141343 chars, 143 ms
```

From 7274 ms to 143 ms: **51 times faster**, without changing engines. A test checks that both
versions produce exactly the same report; without that test, an optimization is a bet.

## 4. Compile to native

`ray run` executes the program on the VM. `ray build --native` translates it to Rust and compiles
it to machine code, with the same output byte for byte.

```sh
ray build --native --release -o perf-lab
./perf-lab fast 200000
```

| 200,000 lines | VM | Native |
|---|---|---|
| First version | 7274 ms | 614 ms |
| Version with `Map` and `sort_by` | 143 ms | 23 ms |

The native binary is 6 to 12 times faster on this program, and uses half the memory (124 MB
against 242 MB with two million lines). But compiling the first version to native leaves it at
614 ms: four times slower than the good version **on the VM**. The algorithm comes first.

The build options tune what is left:

- **`--release`** turns on every Rust optimization. Here it improves the work with strings and
  maps by 20%; it takes longer to compile, so it is for the deliverable.
- **`--fast`** swaps checked arithmetic for arithmetic that does not detect overflow. On this
  program it changes nothing measurable; it helps in numeric loops, at the price of a guarantee.

To compare with other languages, the [benchmarks page](https://raylang.dev/en/bench.html) measures
the native binary against Go, Rust and Node on fourteen programs.

## 5. Use more cores

Every raylang fiber has its own memory: there is no shared data and there are no locks. Launching
a fiber per slice of work is direct, with one condition you need to know. The fiber receives a
**copy** of what it captures, and its result is copied back.

<!-- check: project=examples/apps/perf-lab -->
```rust
/// The same report as `fast.report`, computed by `workers` fibers.
pub fn report(lines: [string], workers: int) -> string {
    let chunk = lines.len() / workers + 1;
    var tasks: [Task<Map<string, Stat>>] = [];
    var from = 0;
    while (from < lines.len()) {
        let part = lines.slice(from, from + chunk);
        tasks.push(spawn(fn() -> Map<string, Stat> { fast.tally(part) }));
        from = from + chunk;
    }
    // Merge: add up each path's counts across the slices.
    var merged: Map<string, Stat> = Map.new();
    for t in tasks {
        for (path, s) in join(t) {
            match (merged.get(path)) {
                Option.Some(m) => {
                    m.count = m.count + s.count;
                    m.total = m.total + s.total;
                },
                Option.None => merged.insert(path, s),
            }
        }
    }
    fast.render(merged)
}
```

| 2,000,000 lines | VM | Native |
|---|---|---|
| One fiber | 1234 ms | 221 ms |
| Four fibers | 460 ms | 111 ms |

Four fibers give twice the speed on native and 2.7 times on the VM, not four: copying the lines to
each fiber has a cost. With `RAYLANG_THREADS=1`, which forces a single thread, the parallel version
takes 296 ms: more than the one-fiber version, because it pays for the copies without gaining
cores. Splitting the work pays when each slice costs much more than copying it.

## 6. Servers

On a web server almost none of the above is needed: every connection already runs in its own fiber
and the server uses all the cores. What matters there is something else:

- **A connection pool** to the database, shared between requests, as in the [API](api.en.md)
  chapter. Opening a connection per request is the cost you notice most.
- **The native binary.** The framework's server, compiled, serves on the order of 188,000 requests
  per second in the project's load benchmark, with about 21 KB per connection.
- **`app.gzip()`** for large responses, and `static_embedded` for static files.

## In short

1. Measure with a clock and a fixed input.
2. Profile with `ray profile` and believe the table, not your intuition.
3. Fix the algorithm: here, 51 times.
4. Compile to native with `--release`: another 6 times.
5. Spread across cores only the work that weighs more than its copy: here, 2 times.

## Next step

[**Shipping**](shipping.en.md): signing, publishing and updating what you have built.

<!-- sync: sha256:e24e1bd798b0 -->
