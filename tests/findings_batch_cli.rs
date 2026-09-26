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

/// Como `three_engines`, para programas con concurrencia (el intérprete no la ejecuta).
fn vm_and_native(dir: &PathBuf, want: &str) {
    let (out, err, code) = ray(dir, &["run", "prog.ray"]);
    assert_eq!(code, 0, "vm: {err}");
    assert_eq!(out, want, "vm");
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
    three_engines(&d, "3\ntrue\n3\n6\n6\ntrue\n20\n42\n4\n3!\ntrue\n4\n3\ntrue\n3\n2\n3\n2\ntrue\nsymlink\nhi\ntrue\n{\"d\":null,\"n\":1,\"arr\":[1,2]}\n");
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

/// M307 (IDEAS §97 #7 y #35): constantes con tuplas y referencias a otras constantes, en los tres
/// motores (la constante es su expresión inyectada en cada uso); y `fs.sync_data`.
#[test]
fn constant_tables_and_sync_data_run_on_all_engines() {
    let d = tmp("consts_sync");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/fs;

const ID_A: int = 1;
const ID_B: int = 2;
const IDS: [int] = [ID_A, ID_B];
const TABLE: [(int, string)] = [(ID_A, "a.png"), (ID_B, "b.png")];
const ORIGIN: (int, int) = (-3, 4);

fn asset(id: int) -> string {
    for t in TABLE {
        let (k, name) = t;
        if (k == id) {
            return name;
        }
    }
    "?"
}

fn main() -> int {
    print(asset(ID_B));
    print(IDS.len());
    let (x, y) = ORIGIN;
    print(x + y);
    let dir = fs.make_temp_dir("raysync").unwrap();
    let h = fs.open(dir + "/log", "w").unwrap();
    let _ = fs.write(h, "rec\n");
    print(fs.sync_data(h).is_ok());
    print(fs.sync(h).is_ok());
    let _ = close(h);
    let r = fs.open(dir + "/log", "r").unwrap();
    print(fs.sync_data(r).is_err());
    let _ = close(r);
    let _ = fs.remove_all(dir);
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "b.png\n2\n1\ntrue\ntrue\ntrue\n");
}

/// M308 (IDEAS §97 #4): bucles etiquetados — `break outer`/`continue outer` desde un bucle
/// interior (`for` y `while`, sobre rangos, arreglos, strings y en posición de expresión), en los
/// tres motores.
#[test]
fn labeled_break_and_continue_run_on_all_engines() {
    let d = tmp("labels");
    std::fs::write(
        d.join("prog.ray"),
        r#"fn find(grid: [[int]], target: int) -> (int, int) {
    var found = (-1, -1);
    rows: for i in 0..grid.len() {
        let row = grid[i];
        var j = 0;
        while (j < row.len()) {
            if (row[j] == target) {
                found = (i, j);
                break rows;
            }
            if (row[j] < 0) {
                continue rows;
            }
            j = j + 1;
        }
    }
    found
}

fn first_pair(xs: [int]) -> Result<(int, int), string> {
    outer: while (true) {
        for a in xs {
            for b in xs {
                if (a + b == 10) {
                    return Result.Ok((a, b));
                }
                if (a > 100) {
                    break outer;
                }
            }
        }
        return Result.Err("none");
    }
    Result.Err("stopped")
}

fn main() -> int {
    let (i, j) = find([[1, 2, 3], [4, -1, 5], [6, 7, 8]], 7);
    print(i);
    print(j);
    let (a, b) = find([[1, 2], [3, 4]], 9);
    print(a + b);
    print(first_pair([3, 7, 1]).unwrap_or((0, 0)).0);
    print(first_pair([200]).is_err());
    var n = 0;
    scan: for x in "ab:cd" {
        for y in [1, 2] {
            if (x == ':') {
                break scan;
            }
            n = n + y;
        }
    }
    print(n);
    var c = 0;
    lp: while (c < 10) {
        c = c + 1;
        let _ = if (c == 3) { break lp } else { c };
    }
    print(c);
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "2\n1\n-2\n3\ntrue\n6\n3\n");
}

/// M309 (findings #44): `Option`/`Result` satisfacen `Eq`/`Show` (con `assert_eq`) en los tres
/// motores, con la forma canónica `Option.Some(x)` en el mensaje.
#[test]
fn option_and_result_satisfy_eq_and_show_on_all_engines() {
    let d = tmp("option_eq");
    std::fs::write(
        d.join("prog.ray"),
        r#"fn main() -> int {
    assert_eq(Option.Some(133), Option.Some(133));
    let r: Result<int, string> = Result.Ok(2);
    assert_eq(r, Result.Ok(2));
    let e: Result<int, string> = Result.Err("x");
    assert_eq(e, Result.Err("x"));
    match (try_call(fn() { assert_eq(Option.Some((1, "a")), Option.None); })) {
        Result.Ok(_) => print("bad"),
        Result.Err(m) => print(m),
    }
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "assert_eq failed: Option.Some((1, a)) != Option.None\n");
}

/// M309 (findings #55, #61, #65): `-o dir/app` crea `dir`; un paquete se importa a sí mismo por su
/// nombre desde sus propios tests aunque declare `entry`; `[package] raylang = "X"` exige un
/// toolchain ≥ X.
#[test]
fn output_dir_self_import_and_required_raylang() {
    // #55: el directorio de salida se crea antes de compilar.
    let d = tmp("outdir");
    std::fs::write(d.join("prog.ray"), "fn main() { print(7); }\n").unwrap();
    if has_rustc() {
        let out = d.join("deep/er/app");
        let (_o, err, code) = ray(&d, &["build", "--native", "prog.ray", "-o", out.to_str().unwrap()]);
        assert_eq!(code, 0, "{err}");
        assert!(out.is_file(), "el binario está en el directorio creado");
    }

    // #61: un paquete-librería con entry que se importa por su nombre en sus tests.
    let p = tmp("selfimport").join("libs").join("grpc");
    std::fs::create_dir_all(p.join("tests")).unwrap();
    std::fs::write(p.join("ray.toml"), "[package]\nname = \"grpc\"\nversion = \"0.1.0\"\nentry = \"grpc.ray\"\n").unwrap();
    std::fs::write(p.join("grpc.ray"), "import grpc/h2;\npub fn hello() -> string { h2.frame(\"x\") }\n").unwrap();
    std::fs::write(p.join("h2.ray"), "pub fn frame(s: string) -> string { \"<\" + s + \">\" }\n").unwrap();
    std::fs::write(p.join("tests/h2_test.ray"), "import grpc/h2;\n@test\nfn frames() -> bool { h2.frame(\"a\") == \"<a>\" }\n").unwrap();
    let (out, err, code) = ray(&p, &["test"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("ok    "), "{out}");

    // #65: la versión mínima del lenguaje.
    let q = tmp("reqver");
    std::fs::create_dir_all(q.join("src")).unwrap();
    std::fs::write(q.join("ray.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nraylang = \"99.0.0\"\n").unwrap();
    std::fs::write(q.join("src/main.ray"), "fn main() { print(1); }\n").unwrap();
    let (_o, err, code) = ray(&q, &["run"]);
    assert_eq!(code, 65, "{err}");
    assert!(err.contains("requires raylang 99.0.0 or newer"), "{err}");
    std::fs::write(q.join("ray.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nraylang = \"1.0.0\"\n").unwrap();
    let (out, _e, code) = ray(&q, &["run"]);
    assert_eq!(code, 0);
    assert_eq!(out, "1\n");
}

/// #70 (M315): `xs.map(to_string)` (un builtin como valor), `xs.slice(from, to)` y el comparador
/// `bool` de `sort_by`, con la misma salida en los tres motores.
#[test]
fn builtins_as_values_and_slice_run_on_all_engines() {
    let d = tmp("m315_builtin_values");
    std::fs::write(
        d.join("prog.ray"),
        r#"fn apply(f: fn(char) -> int, c: char) -> int { f(c) }
fn main() -> int {
    let xs = [3, 1, 2];
    let strs = xs.map(to_string);
    print(strs.fold("", fn(acc: string, s: string) -> string { acc + s + "|" }));
    let f: fn(int) -> string = to_string;
    print(f(41) + "!");
    print(apply(char_code, 'A'));
    print(xs.slice(1, 3));
    print(xs.slice(-5, 2));
    print(xs.slice(2, 1).len());
    print(xs.sort_by(fn(a: int, b: int) -> bool { a > b }));
    print([1.5, 2.5].map(to_string).len());
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "3|1|2|\n41!\n65\n[1, 2]\n[3, 1]\n0\n[3, 2, 1]\n2\n");
}

/// #71 (M313): código GENÉRICO sobre `Channel<Enum<T>>` en nativo — una función genérica que crea
/// el canal (el `T` solo aparece en el retorno: turbofish por tipo esperado), un struct genérico cuyo
/// `T` solo vive dentro del canal (PhantomData) y valores genéricos que cruzan un `spawn` (conversión
/// Send por trait). Antes: E0425/E0283/E0392 en el Rust generado; la VM salía 0.
#[test]
fn generic_channels_of_enums_run_natively() {
    let d = tmp("m313_generic_channels");
    std::fs::write(
        d.join("prog.ray"),
        r#"enum Slot<T> { Ready(T), Empty }
struct Pool<T> { slots: Channel<Slot<T>>, size: int }
struct Conn { id: int, name: string }

fn new_pool<T>(size: int) -> Pool<T> {
    let c: Channel<Slot<T>> = Channel.bounded(size);
    for _ in 0..size { let e: Slot<T> = Slot.Empty; send(c, e); }
    Pool { slots: c, size: size }
}

fn take<T>(p: Pool<T>, make: fn(int) -> T) -> T {
    match (recv(p.slots)) {
        Option.Some(Slot.Ready(v)) => v,
        Option.Some(Slot.Empty) => make(p.size),
        Option.None => make(0),
    }
}

fn give<T>(p: Pool<T>, v: T) { send(p.slots, Slot.Ready(v)); }

fn fill<T>(size: int) -> Channel<Slot<T>> {
    let c: Channel<Slot<T>> = Channel.bounded(size);
    for _ in 0..size { let e: Slot<T> = Slot.Empty; send(c, e); }
    c
}

fn main() -> int {
    let p: Pool<Conn> = new_pool(2);
    let t = spawn(fn() -> int {
        let c = take(p, fn(n: int) -> Conn { Conn { id: n, name: "c" + n.to_string() } });
        print(c.name);
        give(p, Conn { id: 9, name: "back" });
        c.id
    });
    let got = join(t);
    print(take(p, fn(n: int) -> Conn { Conn { id: n, name: "fresh" } }).name);
    print(take(p, fn(n: int) -> Conn { Conn { id: n, name: "fresh" } }).name);
    let ip: Pool<int> = new_pool(1);
    print(take(ip, fn(n: int) -> int { n }) + 3);
    give(ip, 41);
    print(take(ip, fn(n: int) -> int { n }));
    let direct: Channel<Slot<string>> = fill(1);
    match (recv(direct)) { Option.Some(Slot.Empty) => print("empty"), _ => print("?") }
    got - 2
}
"#,
    )
    .unwrap();
    vm_and_native(&d, "c2\nfresh\nback\n4\n41\nempty\n");
}

/// #66 (M312): `ray test --native` compila cada suite a un binario (un `main` de despacho por
/// nombre de prueba) y corre cada prueba como proceso: mismo informe y códigos que la VM, sin la
/// línea `at módulo:línea:col` (el nativo no lleva traza).
#[test]
fn ray_test_native_runs_each_suite_as_a_binary() {
    if !has_rustc() {
        return;
    }
    let d = tmp("m312_test_native");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"m312\"\nversion = \"0.1.0\"\nentry = \"src/main.ray\"\n").unwrap();
    std::fs::write(
        d.join("src/math.ray"),
        "pub fn double(x: int) -> int { x * 2 }\n@test\nfn double_ok() -> bool { double(2) == 4 }\n@test\nfn double_fails() -> bool { double(2) == 5 }\n",
    )
    .unwrap();
    std::fs::write(
        d.join("src/main.ray"),
        "import math;\n@test\nfn prints_and_passes() { print(\"hello from test\"); assert_eq(math.double(3), 6); }\n@test\nfn asserts_fail() { assert_eq(math.double(3), 7); }\n@test\nfn panics() { panic(\"boom\"); }\nfn main() -> int { print(math.double(21)); 0 }\n",
    )
    .unwrap();
    std::fs::write(d.join("tests/extra.ray"), "import math;\n@test\nfn extra_ok() -> bool { math.double(5) == 10 }\n").unwrap();
    let (out, err, code) = ray(&d, &["test", "--native"]);
    assert_eq!(code, 1, "{out}\n{err}");
    assert!(out.contains("running 6 test(s) — native binaries"), "{out}");
    assert!(out.contains("hello from test\nok    prints_and_passes ("), "{out}");
    assert!(out.contains("FAIL  asserts_fail\n        assert_eq failed: 6 != 7\n"), "{out}");
    assert!(out.contains("FAIL  panics\n        boom\n"), "{out}");
    assert!(out.contains("FAIL  math.double_fails\n        the test returned false\n"), "{out}");
    assert!(out.contains("-- tests/extra.ray\nok    extra_ok ("), "{out}");
    assert!(out.contains("result: 3 of 6 test(s) failed ✗"), "{out}");
    // Filtro + `--release`: solo la prueba pedida, en verde; los flags no se toman por filtro.
    let (out, err, code) = ray(&d, &["test", "--native", "double_ok", "--release"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("running 1 test(s) — native binaries (release)"), "{out}");
    assert!(out.contains("result: 1 test(s), all passed ✓"), "{out}");
}

/// #54 (M311): alias de tipo — en firmas, campos, genéricos, closures, `Map`, `Option`/`Result`,
/// y a través de módulos (`pub type`, `geo.Pt`, `from geo import Named`; un alias privado no se ve).
#[test]
fn type_aliases_run_on_all_engines_and_across_modules() {
    let d = tmp("m311_aliases");
    std::fs::write(
        d.join("geo.ray"),
        "pub type Pt = (int, int);\npub type Named<T> = (string, T);\ntype Hidden = int;\npub fn origin() -> Pt { (0, 0) }\npub fn tag(n: string, p: Pt) -> Named<Pt> { (n, p) }\npub fn hidden() -> Hidden { 7 }\n",
    )
    .unwrap();
    std::fs::write(
        d.join("prog.ray"),
        r#"import geo;
from geo import Named;

type Id = int;
type Ids = [Id];
type Pair<T> = (T, T);
type Lookup<K, V> = Map<K, V>;
type Handler = fn(Id) -> string;
type MaybeId = Option<Id>;
type Res<T> = Result<T, string>;
type Point = geo.Pt;

struct User { id: Id, tags: Ids, pos: Point }
enum Ev { Moved(Point), Named(Named<Id>) }

fn describe(u: User, h: Handler) -> string { h(u.id) + " at " + u.pos.0.to_string() }
fn swap<T>(p: Pair<T>) -> Pair<T> { (p.1, p.0) }
fn find(m: Lookup<string, Id>, k: string) -> MaybeId { m.get(k) }
fn parse(s: string) -> Res<Id> {
    match (parse_int(s)) { Option.Some(v) => Result.Ok(v), Option.None => Result.Err("bad " + s) }
}

fn main() -> int {
    let u = User { id: 3, tags: [1, 2], pos: geo.origin() };
    let h: Handler = fn(i: Id) -> string { "user#" + i.to_string() };
    print(describe(u, h));
    let p: Pair<string> = ("a", "b");
    print(swap(p).0 + swap(p).1);
    var m: Lookup<string, Id> = Map.new();
    m.insert("x", 9);
    print(find(m, "x"));
    print(find(m, "y"));
    print(parse("12"));
    print(parse("zz"));
    let e = Ev.Named(("n", 5));
    let tg = geo.tag("n", geo.origin());
    let inner = tg.1;
    print(inner.1);
    match (e) { Ev.Moved(q) => print(q.0), Ev.Named((n, _)) => print(n) }
    let t: Named<Point> = ("t", (1, 2));
    print(t.0);
    print(geo.hidden());
    let ids: Ids = u.tags;
    let type = 4;
    ids.len() + type - 6
}
"#,
    )
    .unwrap();
    three_engines(
        &d,
        "user#3 at 0\nba\nOption.Some(9)\nOption.None\nResult.Ok(12)\nResult.Err(bad zz)\n0\nn\nt\n7\n",
    );
    // Un alias privado de otro módulo no se ve; el mensaje es el de un tipo desconocido calificado.
    std::fs::write(d.join("bad.ray"), "import geo;\ntype H = geo.Hidden;\nfn main() {}\n").unwrap();
    let (_o, e, code) = ray(&d, &["run", "bad.ray"]);
    assert_eq!(code, 65, "{e}");
    assert!(e.contains("unknown type: 'geo.Hidden' not declared"), "{e}");
}

/// #44/#52/#53 (M310): patrones de tupla y literales (anidados en variantes), exhaustividad por
/// matriz y `dyn Trait` como campo de struct, con la misma salida en los tres motores.
#[test]
fn tuple_and_literal_patterns_run_on_all_engines() {
    let d = tmp("m310_patterns");
    std::fs::write(
        d.join("prog.ray"),
        r#"enum Shape { Circle(int), Rect(int, int), Named(string) }
trait Greeter { fn hi(self) -> string; }
struct P { x: int }
impl Greeter for P { fn hi(self) -> string { "p" } }
struct Holder { g: dyn Greeter }

fn classify(n: int) -> string {
    match (n) { 0 => "zero", -1 => "minus", _ => "many" }
}

fn pair(t: (int, string)) -> string {
    match (t) {
        (0, "a") => "zero-a",
        (0, s) => "zero-" + s,
        (n, "b") => "b-" + n.to_string(),
        (n, s) => s + n.to_string(),
    }
}

fn nested(o: Option<Shape>) -> string {
    match (o) {
        Option.Some(Shape.Circle(0)) => "dot",
        Option.Some(Shape.Circle(r)) => "circle " + r.to_string(),
        Option.Some(Shape.Rect(w, 0)) => "line " + w.to_string(),
        Option.Some(Shape.Rect(w, h)) => "rect " + (w * h).to_string(),
        Option.Some(Shape.Named("x")) => "the x",
        Option.Some(Shape.Named(nm)) => "named " + nm,
        Option.None => "none",
    }
}

fn flags(b: (bool, bool)) -> int {
    match (b) { (true, true) => 3, (true, false) => 2, (false, true) => 1, (false, false) => 0 }
}

fn opts(o: (Option<int>, Option<int>)) -> int {
    match (o) {
        (Option.Some(a), Option.Some(b)) => a + b,
        (Option.Some(a), Option.None) => a,
        (Option.None, Option.Some(b)) => b,
        (Option.None, Option.None) => 0,
    }
}

fn main() -> int {
    let h = Holder { g: P { x: 1 } };
    print(h.g.hi());
    print(h);
    print(classify(0) + classify(-1) + classify(7));
    print(pair((0, "a")) + pair((0, "z")) + pair((5, "b")) + pair((5, "q")));
    print(nested(Option.Some(Shape.Circle(0))) + nested(Option.Some(Shape.Circle(3))));
    print(nested(Option.Some(Shape.Rect(4, 0))) + nested(Option.Some(Shape.Rect(4, 5))));
    print(nested(Option.Some(Shape.Named("x"))) + nested(Option.Some(Shape.Named("y"))) + nested(Option.None));
    let n: Option<int> = Option.None;
    print(flags((true, true)) + flags((false, true)) + opts((Option.Some(1), Option.Some(2))) + opts((n, Option.Some(5))) + opts((n, n)));
    match ('a') { 'a' => 0, _ => 1 }
}
"#,
    )
    .unwrap();
    three_engines(
        &d,
        "p\nHolder { g: <dyn Greeter> }\nzerominusmany\nzero-azero-zb-5q5\ndotcircle 3\nline 4rect 20\nthe xnamed ynone\n12\n",
    );
}


/// M316 (findings #76, #78, #79, #92, #93): un `let` sin anotar con el resultado de una función
/// genérica que recibe un closure (regresión 1.27.14), un campo de ese valor concatenado, un canal
/// guardado en una tupla, `[]` inferido desde la otra rama del `if` y la tupla mostrada como `[a, b]`
/// `(1, a)` también en `print` (VM/intérprete: antes `[[1, a]]`) — todo en VM y nativo.
#[test]
fn generic_closure_results_tuple_channels_and_empty_branches_run_natively() {
    let d = tmp("generic_closure_results");
    std::fs::write(
        d.join("prog.ray"),
        r#"struct Resp { status: int }
fn attempt<T, E>(f: fn() -> Result<T, E>) -> Result<T, E> { f() }
fn start() -> (int, Channel<int>) {
    let stop: Channel<int> = Channel.new();
    (7, stop)
}
fn main() -> int {
    let n = 200;
    let result = attempt(fn() -> Result<Resp, string> { Result.Ok(Resp { status: n }) });
    match (result) {
        Result.Ok(r) => print("ok " + to_string(r.status)),
        Result.Err(e) => print(e),
    }
    let s = start();
    send(s.1, 5);
    let v = recv(s.1);
    close(s.1);
    print(v);
    let c = args().len() > 5;
    let xs = if (c) { [] } else { [3, 4] };
    print(xs.len());
    print([(1, "a")]);
    print(Option.Some((1, "a")));
    let pairs: [(int, string)] = [(1, "a"), (2, "b")];
    print(pairs);
    0
}
"#,
    )
    .unwrap();
    vm_and_native(&d, "ok 200\nOption.Some(5)\n2\n[(1, a)]\nOption.Some((1, a))\n[(1, a), (2, b)]\n");
}

/// M316 (findings #77): un valor-función CALCULADO (`run(mk(2))`) hacia un parámetro que cruza a
/// `spawn` se rechaza en raylang (antes: tres E0277 de rustc sobre código generado).
#[test]
fn a_computed_function_value_crossing_to_spawn_is_diagnosed_in_raylang() {
    if !has_rustc() {
        return;
    }
    let d = tmp("computed_fn_spawn");
    std::fs::write(
        d.join("prog.ray"),
        r#"fn mk(k: int) -> fn(int) -> int { fn(x: int) -> int { x * k } }
fn run(f: fn(int) -> int) -> int {
    let t = spawn(fn() -> int { f(21) });
    join(t)
}
fn main() -> int { print(run(mk(2))); 0 }
"#,
    )
    .unwrap();
    let (out, _, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!((code, out.as_str()), (0, "42\n"), "vm");
    let bin = d.join("prog_bin");
    let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_ne!(code, 0, "el nativo debe rechazarlo");
    assert!(err.contains("a function value computed here") && err.contains("write the closure inline"), "{err}");
    assert!(!err.contains("E0277"), "sin errores de rustc: {err}");
}
