//! IDEAS §97 — los hallazgos de lenguaje del barrido de `ray-apps` a 1.27.11, cada uno con su
//! programa mínimo en los tres motores (VM, intérprete y nativo si hay rustc).

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_findings_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Corre `prog.ray` de `dir` en VM, intérprete y (si hay rustc) nativo; los tres deben dar `want`.
fn three_engines(dir: &PathBuf, want: &str) {
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(dir, engine);
        assert_eq!(code, 0, "{engine:?}: {err}");
        assert_eq!(out, want, "{engine:?}");
    }
    if has_rustc() {
        let bin = dir.join("prog_bin");
        let (_o, err, code) = ray(dir, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        let out = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo");
    }
}

/// #2 (M300): `break`/`continue` como expresión en un brazo de `match` y en un `if`-valor.
#[test]
fn break_and_continue_as_expressions_run_on_all_engines() {
    let d = tmp("break_expr");
    std::fs::write(
        d.join("prog.ray"),
        r#"fn sum_until_error(xs: [Result<int, string>]) -> int {
    var total = 0;
    for r in xs {
        let v = match (r) {
            Result.Ok(v) => v,
            Result.Err(e) => break,
        };
        let w = if (v < 0) { continue } else { v };
        total = total + w;
    }
    total
}

fn main() -> int {
    print(sum_until_error([Result.Ok(1), Result.Ok(0 - 5), Result.Ok(2), Result.Err("x"), Result.Ok(100)]));
    var i = 0;
    while (true) {
        i = i + 1;
        let _ = match (Option.Some(i)) {
            Option.Some(n) => if (n >= 3) { break } else { n },
            Option.None => continue,
        };
    }
    print(i);
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "3\n3\n");
}

/// #3 (M301): `while (true)` sin `break` como cuerpo de una función `-> Result`; #6 (M303):
/// `Option.None` inferido de la otra rama; #5 (M302): `assert_eq` sobre tuplas (y `Show` de
/// tuplas con la forma `(a, b)`, idéntica en los tres motores).
#[test]
fn infinite_loop_divergence_if_inference_and_tuple_bounds_run_on_all_engines() {
    let d = tmp("lang_batch");
    std::fs::write(
        d.join("prog.ray"),
        r#"fn first_even(xs: [int]) -> Result<int, string> {
    var i = 0;
    while (true) {
        if (i >= xs.len()) {
            return Result.Err("none");
        }
        if (xs[i] % 2 == 0) {
            return Result.Ok(xs[i]);
        }
        i = i + 1;
    }
}

fn pair() -> (string, int) { ("h", 81) }

fn main() -> int {
    print(first_even([1, 3, 4, 5]).unwrap_or(0 - 1));
    print(first_even([1, 3]).is_err());
    let o = if (true) { Option.Some(7) } else { Option.None };
    let p = if (false) { Option.None } else { Option.Some("s") };
    print(o.unwrap_or(0));
    print(p.unwrap_or("?"));
    assert_eq(pair(), ("h", 81));
    assert_eq((1, (2, true)), (1, (2, true)));
    match (try_call(fn() { assert_eq((1, (2, true), "z"), (1, (2, false), "z")); })) {
        Result.Ok(_) => print("bad"),
        Result.Err(e) => print(e),
    }
    match (try_call(fn() { assert_eq((1, "a"), (1, "b")); })) {
        Result.Ok(_) => print("bad"),
        Result.Err(e) => print(e),
    }
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "4\ntrue\n7\ns\nassert_eq failed: (1, (2, true), z) != (1, (2, false), z)\nassert_eq failed: (1, a) != (1, b)\n");
}

/// M304 (IDEAS §97 #24, #25, #26, #30, #31, #32): la stdlib que las apps rodeaban — `last_index_of`
/// (string y bytes), `bytes.index_of_from`, los combinadores de `Option`/`Result`, `Deque` con
/// acceso e iteración, `fs.symlink` y `Json` como `ToJson` en el builder.
#[test]
fn stdlib_batch_runs_on_all_engines() {
    let d = tmp("stdlib_batch");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/collections/deque;
import std/fs;
import std/json;

fn main() -> int {
    print("a:b:c".last_index_of(":").unwrap_or(0 - 1));
    print("abc".last_index_of("x").is_none());
    print("abc".last_index_of("").unwrap_or(0 - 1));
    print(b"ab\r\nab\r\n".last_index_of(b"\r\n").unwrap_or(0 - 1));
    print(b"ab\r\nab\r\n".index_of_from(b"\r\n", 3).unwrap_or(0 - 1));
    print(b"abc".index_of_from(b"c", 5).is_none());
    let o = Option.Some(2);
    print(o.and_then(fn(x: int) -> Option<int> { if (x > 1) { Option.Some(x * 10) } else { Option.None } }).unwrap_or(0));
    let n: Option<int> = Option.None;
    print(n.unwrap_or_else(fn() -> int { 42 }));
    let r: Result<int, string> = Result.Ok(3);
    print(r.map(fn(x: int) -> int { x + 1 }).unwrap_or(0));
    print(r.and_then(fn(x: int) -> Result<string, string> { Result.Ok(to_string(x) + "!") }).unwrap_or("?"));
    let e: Result<int, string> = Result.Err("boom");
    print(e.map_err(fn(m: string) -> int { m.len() }).is_err());
    print(e.unwrap_or_else(fn(m: string) -> int { m.len() }));
    var q: deque.Deque<int> = deque.new();
    deque.push_back(q, 1);
    deque.push_back(q, 2);
    deque.push_back(q, 3);
    let _ = deque.pop_front(q);
    print(deque.get(q, 1).unwrap_or(0));
    print(deque.get(q, 2).is_none());
    print(deque.peek_back(q).unwrap_or(0));
    for x in deque.iter(q) { print(x); }
    print(deque.to_array(q).len());
    let dir = fs.make_temp_dir("raysym").unwrap();
    let _ = fs.write_file(dir + "/target.txt", "hi");
    print(fs.symlink("target.txt", dir + "/link.txt").is_ok());
    print(fs.stat(dir + "/link.txt").unwrap().kind);
    print(fs.read_file(dir + "/link.txt").unwrap_or("?"));
    print(fs.symlink("target.txt", dir + "/link.txt").is_err());
    let _ = fs.remove_all(dir);
    print(json.obj().field("d", json.Json.JNull).field("n", 1).field("arr", json.parse("[1,2]").unwrap()).to_json());
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "3\ntrue\n3\n6\n6\ntrue\n20\n42\n4\n3!\ntrue\n4\n3\ntrue\n3\n2\n3\n2\ntrue\nsymlink\nhi\ntrue\n{\"d\": null, \"n\": 1, \"arr\": [1,2]}\n");
}

/// M306 (IDEAS §97 #14): `set_read_timeout(listener, ms)` acota también `tcp_accept` (la doc lo
/// prometía y colgaba para siempre): "read timeout" en los tres motores, y el listener sigue
/// sirviendo después (el flag no-bloqueante del plazo se repone).
#[test]
fn accept_honours_the_read_timeout_on_all_engines() {
    let d = tmp("accept_timeout");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/net;
import std/time;

fn main() -> int {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    net.set_read_timeout(l, 200);
    let t0 = time.monotonic();
    match (net.tcp_accept(l)) {
        Result.Ok(_) => print("bad: accepted"),
        Result.Err(e) => print(e),
    }
    let dt = time.monotonic() - t0;
    print(dt >= 150 && dt < 5000);
    net.set_read_timeout(l, 300);
    print(net.tcp_accept(l).is_err());
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "read timeout\ntrue\ntrue\n");
}

/// M306 (IDEAS §97 #15): `tcp_connect_timeout` APARCA la fibra en la VM (un solo worker,
/// `--deterministic`) y en el nativo con fibras (`RAYLANG_THREADS=1`): mientras el dial a una
/// dirección que descarta SYNs espera su plazo, la fibra principal sigue contando ticks. Si la
/// red rechaza al instante (sin ruta), el dial es "fast" y la prueba no afirma nada más.
#[test]
fn connect_timeout_parks_the_fiber() {
    let d = tmp("connect_parks");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/net;
import std/time;

fn main() -> int {
    let done: Channel<int> = Channel.new();
    spawn(fn() {
        let t0 = time.monotonic();
        let r = net.tcp_connect_timeout("10.255.255.1", 81, 1200);
        print(r.is_err());
        send(done, time.monotonic() - t0);
    });
    var ticks = 0;
    var took = 0;
    var waiting = true;
    while (waiting) {
        match (select_timeout([done], 50)) {
            Option.Some(_) => {
                took = recv(done).unwrap_or(0);
                waiting = false;
            },
            Option.None => { ticks = ticks + 1; },
        }
    }
    print(if (took < 300) { "fast" } else { to_string(ticks >= 3) });
    0
}
"#,
    )
    .unwrap();
    let ok = |out: &str| out == "true\ntrue\n" || out == "true\nfast\n";
    let (out, err, code) = ray(&d, &["run", "--deterministic", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(ok(&out), "VM: {out}");
    if has_rustc() {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        let out = Command::new(&bin).env("RAYLANG_THREADS", "1").output().unwrap();
        let out = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(ok(&out), "nativo: {out}");
    }
}
