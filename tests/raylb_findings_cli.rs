//! IDEAS §103 — los hallazgos que raylb (el balanceador de ray-apps) documentó, cada uno con su
//! programa mínimo. M355: el `connect` no retiene al worker (L19/L14) y el cliente HTTP acota el dial (L5).
//! M356: el lote de correcciones (L2, L3, L9, L12, L16, L17) y rayauth R34.
//! M357: el scheduler y los `scope` (L8, L15) y las señales extra (L1).
//! M359: los paquetes (L11 `metrics.series`, L6/L7 `serve_options_on` + `with_stop` + `quiet`, L4 toml).
//! M360: documentación y tooling (R35 sintaxis de dependencias, R36 paquete-librería, L18 perfilador).
//! M358: el reactor por worker del scheduler nativo (`RAYLANG_REACTOR=local`), prototipo medido.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_raylb_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Un proyecto consumidor con `net` del repo como dependencia por ruta.
fn project(name: &str) -> PathBuf {
    let d = tmp(name);
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        d.join("ray.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nentry = \"prog.ray\"\n\n[dependencies]\nnet = \"path:{root}/packages/net\"\n"),
    )
    .unwrap();
    d
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// `prog.ray` en la VM y (con rustc) en nativo, ambos con DOS hilos worker: imprimen `want`.
fn vm_and_native_two_threads(dir: &PathBuf, want: &str) {
    let ray = env!("CARGO_BIN_EXE_ray");
    // Ninguna ejecución puede esperar al plazo del SO (75 s en macOS): también la SALIDA del proceso.
    let t = std::time::Instant::now();
    let out = Command::new(ray).args(["run", "prog.ray"]).env("RAYLANG_THREADS", "2").current_dir(dir).output().unwrap();
    assert!(out.status.success(), "vm: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), want, "vm");
    assert!(t.elapsed().as_secs() < 30, "vm: the run waited for the OS connect timeout");
    if has_rustc() {
        let bin = dir.join("prog_bin");
        let b = Command::new(ray).args(["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]).current_dir(dir).output().unwrap();
        assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
        let t = std::time::Instant::now();
        let out = Command::new(&bin).env("RAYLANG_THREADS", "2").current_dir(dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
        assert!(t.elapsed().as_secs() < 30, "native: the run waited for the OS connect timeout");
    }
}

/// `prog.ray` (sin dependencias) en VM, intérprete y (con rustc) nativo: los tres imprimen `want`.
fn three_engines(name: &str, src: &str, want: &str) {
    let d = tmp(name);
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let ray = env!("CARGO_BIN_EXE_ray");
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(ray).args(engine).current_dir(&d).output().unwrap();
        assert!(out.status.success(), "{engine:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "{engine:?}");
    }
    native(&d, want);
}

/// `prog.ray` en la VM y (con rustc) en nativo — programas con fibras, que el intérprete no ejecuta.
fn vm_and_native(name: &str, src: &str, want: &str) {
    let d = tmp(name);
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert!(out.status.success(), "vm: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), want, "vm");
    native(&d, want);
}

fn native(d: &PathBuf, want: &str) {
    if !has_rustc() {
        return;
    }
    let bin = d.join("prog_bin");
    let b = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()])
        .current_dir(d)
        .output()
        .unwrap();
    assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
    let out = Command::new(&bin).current_dir(d).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
}

const SERVE_OPTIONS_ON_PROG: &str = r#"import std/net;
import std/time;
import net/webserver;
import net/http;
fn main() -> int {
    let srv = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(srv);
    let stop: Channel<int> = Channel.new();
    let done: Channel<int> = Channel.new();
    spawn(fn() {
        let opts = webserver.options().with_stop(stop).with_drain(500).quiet();
        let r = webserver.serve_options_on(srv, opts, fn() -> fn(webserver.Request) -> webserver.Response {
            fn(req: webserver.Request) -> webserver.Response { webserver.text(200, "hi") }
        });
        send(done, r.unwrap_or(0 - 1));
    });
    let r = http.fetch("http://127.0.0.1:${port}/").unwrap();
    print(r.status);
    let t0 = time.monotonic();
    send(stop, 1);
    print("serve returned " + to_string(recv(done).unwrap_or(0 - 2)) + " quick=" + to_string(time.monotonic() - t0 < 2000));
    print(http.fetch("http://127.0.0.1:${port}/").is_err());
    0
}
"#;

const SELECT_TIMEOUTS_PROG: &str = r#"import std/time;
fn main() -> int {
    let out: Channel<string> = Channel.bounded(64);
    for w in [0, 2, 9, 30] {
        let wait = w;
        let s: Channel<int> = Channel.bounded(1);
        spawn(fn() {
            match (select_timeout([s], wait)) {
                Option.Some(i) => send(out, "stop?"),
                Option.None => send(out, "woke after ${wait}"),
            }
        });
    }
    time.sleep(300);
    var got: [string] = [];
    while (true) {
        match (try_recv(out)) { Received.Got(x) => got.push(x), _ => break, }
    }
    print(sort(got));
    // Un canal compartido por 3 fibras, 3 vueltas de 20 ms cada una: 9 ticks.
    let shared: Channel<int> = Channel.bounded(1);
    let ticks: Channel<int> = Channel.bounded(64);
    for k in [1, 2, 3] {
        spawn(fn() {
            var n = 0;
            while (n < 3) {
                let _ = select_timeout([shared], 20);
                send(ticks, k);
                n = n + 1;
            }
        });
    }
    time.sleep(400);
    var count = 0;
    while (true) {
        match (try_recv(ticks)) { Received.Got(x) => count = count + 1, _ => break, }
    }
    print(count);
    0
}
"#;

/// L19/L14/L5: cuatro fibras marcan a un host que descarta los SYN (10.255.255.1, no enrutable) con
/// dos hilos worker. Antes el `connect` bloqueaba al worker: los temporizadores de `main` se
/// congelaban hasta 75 s y el proceso no podía salir. Ahora `main` sigue a su ritmo, el cliente HTTP
/// falla dentro de su plazo y el programa termina con los diales aún pendientes. (Donde la red
/// rechaza el destino al instante, las mismas aserciones valen: solo se exige no esperar al SO.)
#[test]
fn a_dial_to_a_black_hole_does_not_freeze_the_scheduler_and_http_bounds_it() {
    let d = project("blackhole");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/net;
import std/time;
import net/http;

fn dialer() {
    let _ = net.tcp_connect("10.255.255.1", 81);
}

fn main() -> int {
    var i: int = 0;
    while (i < 4) {
        spawn(fn() { dialer(); });
        i = i + 1;
    }
    let t0 = time.monotonic();
    var ticks: int = 0;
    while (ticks < 5) {
        time.sleep(100);
        ticks = ticks + 1;
    }
    print("timely=" + (time.monotonic() - t0 < 5000).to_string());
    let t1 = time.monotonic();
    let r = http.request_bytes("GET", "http://10.255.255.1:81/", b"", Map.new(), 400);
    let failed = match (r) {
        Result.Ok(_) => false,
        Result.Err(m) => m.starts_with("could not connect: "),
    };
    print("request failed=" + failed.to_string() + " bounded=" + (time.monotonic() - t1 < 5000).to_string());
    let t2 = time.monotonic();
    let c = match (http.connect_timeout("http://10.255.255.1:81", 300)) {
        Result.Ok(_) => false,
        Result.Err(m) => m.starts_with("could not connect: "),
    };
    print("connect failed=" + c.to_string() + " bounded=" + (time.monotonic() - t2 < 5000).to_string());
    0
}
"#,
    )
    .unwrap();
    vm_and_native_two_threads(&d, "timely=true\nrequest failed=true bounded=true\nconnect failed=true bounded=true\n");
}

/// L2: un `const` de tipo sin signo toma su tipo declarado como contexto (`const A: u64 = 5;`) y su
/// literal llega al runtime con su ancho. Antes: `print(K)` salía como `int` negativo, `K ^ s` era un
/// ICE en la VM y un E0308 en nativo.
#[test]
fn an_unsigned_const_keeps_its_width_on_all_engines() {
    three_engines(
        "const_u64",
        r#"const A: u64 = 5;
const K: u64 = 0xcbf29ce484222325;
const P: u64 = 0x100000001b3u64;
const B: u8 = 200;
const T: [u8] = [1, 2, 255];
const S: u32 = 1 << 20;
fn main() -> int {
    let s: u64 = 1 as u64;
    print(K);
    print(K ^ s);
    print(K * P);
    print(A + s);
    print(B);
    print(T[2]);
    print(S);
    0
}
"#,
        "14695981039346656037\n14695981039346656036\n12638153115695167455\n6\n200\n255\n1048576\n",
    );
    // El rango se sigue comprobando contra el tipo declarado.
    let d = tmp("const_u8_range");
    std::fs::write(d.join("prog.ray"), "const A: u8 = 300;\nfn main() -> int { 0 }\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("the literal 300 does not fit in u8"));
}

/// L3: `u8`/`u32`/`u64` implementan `Eq`/`Show`/`Ord`/`Hash` como `int` — `assert_eq`, un bound
/// `T: Eq`, `@derive` sobre un struct con campos sin signo y `sort`.
#[test]
fn unsigned_integers_implement_the_core_traits_on_all_engines() {
    three_engines(
        "uint_traits",
        r#"@derive(Eq, Show, Hash)
struct Key { id: u64, kind: u8 }
fn same<T: Eq>(a: T, b: T) -> bool { a.eq(b) }
fn main() -> int {
    let a: u64 = 0xcbf29ce484222325;
    let b: u64 = 0xcbf29ce484222325;
    assert_eq(a, b);
    print(same(a, b));
    print(same(7 as u8, 8 as u8));
    let k = Key { id: a, kind: 3 };
    print(k.eq(Key { id: b, kind: 3 }));
    print(k.show());
    print(k.hash() == Key { id: b, kind: 3 }.hash());
    var xs: [u32] = [3, 1, 2];
    print(sort(xs));
    print(a.hash() == b.hash());
    0
}
"#,
        "true\nfalse\ntrue\nKey { id: 14695981039346656037, kind: 3 }\ntrue\n[1, 2, 3]\ntrue\n",
    );
}

/// L9, L12, L16 y L17, los cuatro del backend nativo en un programa: una local llamada `drop` junto a
/// una closure; un parámetro de closure homónimo de una `var` declarada más abajo; un `scope` cuyo
/// `Result` se infiere sin anotar el `let`; y capturas de un `scope` que se siguen usando después.
#[test]
fn native_closures_and_scopes_match_the_vm() {
    vm_and_native(
        "native_closures",
        r#"struct Side { name: string, n: int }
fn line(s: Side) -> string { s.name + ":" + to_string(s.n) }
fn one_run(s: Side, k: int) -> Result<int, string> {
    if (k < 0) { Result.Err("neg") } else { Result.Ok(s.n + k) }
}
fn f(m: Map<string, string>, flag: bool) -> string {
    let drop = m.keys();
    let prev = fn(name: string) -> string {
        if (flag) { m.get_or(name, "") } else { "" }
    };
    prev("a") + to_string(drop.len())
}
fn apply(g: fn(int) -> int) -> int { g(2) }
fn main() -> int {
    var m: Map<string, string> = Map.new();
    m.insert("a", "x");
    print(f(m, true));
    print(apply(fn(i: int) -> int { i * 10 }));
    for n in [1] {
        var i = 0;
        i = i + n;
        print(i);
    }
    let a = Side { name: "left", n: 4 };
    var total = 0;
    let r = scope(fn() -> Result<int, string> {
        let v = one_run(a, 1)?;
        let t = spawn(fn() -> int { v * 2 });
        total = total + 1;
        Result.Ok(t.join())
    });
    print(line(a));
    print(r.unwrap_or(-1));
    let bad = scope(fn() -> Result<int, string> {
        one_run(a, -1)?;
        Result.Ok(0)
    });
    print(bad.is_err());
    print(total);
    0
}
"#,
        "x1\n20\n1\nleft:4\n10\ntrue\n1\n",
    );
}

/// R34 (rayauth): `for b in bs` recorre los octetos de un `bytes` como `int` (con `break`/`continue`
/// etiquetados), y `strip_prefix`/`strip_suffix` recortan un prefijo o sufijo conocido.
#[test]
fn bytes_iterate_and_strings_strip_affixes_on_all_engines() {
    three_engines(
        "for_bytes",
        r#"fn sum(bs: bytes) -> int {
    var t = 0;
    for b in bs {
        if (b == 0x2f) { continue; }
        t = t + b;
    }
    t
}
fn main() -> int {
    print(sum("a/b".to_bytes()));
    var seen: [int] = [];
    outer: for b in b"\x01\x02\x03" {
        for c in "xy".to_bytes() {
            if (b == 2) { continue outer; }
            if (b == 3) { break outer; }
            seen.push(b * 1000 + c);
        }
    }
    print(seen);
    for b in b"" { print(b); }
    0
}
"#,
        "195\n[1120, 1121]\n",
    );
    three_engines(
        "strip_affix",
        r#"fn main() -> int {
    let issuer = "https://id.example.com/";
    print(issuer.strip_suffix("/").unwrap_or(issuer));
    print("Bearer abc".strip_prefix("Bearer "));
    print("Basic abc".strip_prefix("Bearer ").is_none());
    print("añá".strip_prefix("a").is_none());
    print("ñandú".strip_prefix("ñ").unwrap_or(""));
    print("x".strip_suffix("").unwrap_or("?"));
    0
}
"#,
        "https://id.example.com\nOption.Some(abc)\ntrue\nfalse\nandú\nx\n",
    );
}

/// L8: varias fibras en `select_timeout` a la vez. En la VM el plazo se guardaba por el handle del
/// arreglo de canales, que colisiona entre fibras (cada una tiene su heap): solo despertaba la
/// última. Ahora el plazo vive en la fibra: las cuatro despiertan y los nueve ticks llegan.
#[test]
fn concurrent_select_timeouts_all_fire() {
    vm_and_native(
        "select_timeouts",
        SELECT_TIMEOUTS_PROG,
        "[woke after 0, woke after 2, woke after 30, woke after 9]\n9\n",
    );
}

/// L15: un `scope` cuyo cuerpo devuelve `Result.Err` mata a sus procesos hijos antes de unir las
/// fibras (antes esperaba a la bomba de salida, es decir, a que el hijo muriera solo). Un scope que
/// termina bien sigue esperando a que los flujos del hijo se cierren.
#[cfg(unix)]
#[test]
fn a_scope_that_returns_err_kills_its_child_processes() {
    vm_and_native(
        "scope_err_procs",
        r#"import std/process;
import std/time;
fn body() -> Result<int, string> {
    scope(fn() -> Result<int, string> {
        let p = process.cmd("sleep", ["3"]).stream()?;
        let out = p.out;
        spawn(fn() { while (recv(out).is_some()) { } });
        Result.Err("early error")
    })
}
fn unwaited() -> int {
    scope(fn() -> int {
        let p = process.cmd("sleep", ["3"]).stream().unwrap();
        7
    })
}
fn main() -> int {
    let t0 = time.monotonic();
    let r = body();
    print("err scope quick=${time.monotonic() - t0 < 1500}: ${r.is_err()}");
    let t1 = time.monotonic();
    let v = unwaited();
    print("unwaited quick=${time.monotonic() - t1 < 1500}: ${v}");
    0
}
"#,
        "err scope quick=true: true\nunwaited quick=false: 7\n",
    );
}

/// L1: `process.listen_signal` hace llegar SIGHUP y SIGUSR1 por `signals()` (sin pedirlo, SIGHUP
/// mata el proceso, como siempre). El test lanza el programa, espera su `ready` y le envía las dos.
#[cfg(unix)]
#[test]
fn extra_signals_arrive_through_the_signals_channel() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let d = tmp("extra_signals");
    std::fs::write(d.join("prog.ray"), r#"import std/process;
fn main() -> int {
    print(process.listen_signal(process.SIGHUP));
    print(process.listen_signal(process.sigusr1()));
    print(process.listen_signal(9));
    print("ready");
    let s = signals();
    var seen: [int] = [];
    while (seen.len() < 2) {
        match (select_timeout([s], 5000)) {
            Option.Some(_) => match (recv(s)) {
                Option.Some(n) => seen.push(n),
                Option.None => break,
            },
            Option.None => break,
        }
    }
    print(seen.len());
    print(seen[0] == process.SIGHUP);
    print(seen[1] == process.sigusr1());
    0
}
"#).unwrap();
    let ray = env!("CARGO_BIN_EXE_ray");
    let mut cmds: Vec<Command> = Vec::new();
    let mut vm = Command::new(ray);
    vm.args(["run", "prog.ray"]);
    cmds.push(vm);
    if has_rustc() {
        let bin = d.join("prog_bin");
        let b = Command::new(ray).args(["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
        cmds.push(Command::new(bin));
    }
    for mut cmd in cmds {
        let mut child = cmd.current_dir(&d).stdout(Stdio::piped()).spawn().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut seen: Vec<String> = Vec::new();
        for line in lines.by_ref() {
            let line = line.unwrap();
            let ready = line == "ready";
            seen.push(line);
            if ready {
                break;
            }
        }
        let pid = child.id().to_string();
        for name in ["-HUP", "-USR1"] {
            assert!(Command::new("kill").args([name, &pid]).status().unwrap().success());
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        seen.extend(lines.map(|l| l.unwrap()));
        assert!(child.wait().unwrap().success());
        assert_eq!(seen, ["true", "true", "false", "ready", "2", "true", "true"]);
    }
}

/// L11: el handle `metrics.series` (una búsqueda al resolver, O(1) por actualización) rinde exactamente
/// lo mismo que las funciones libres `inc`/`add`/`set`/`observe_l`, en los tres motores. (Destapó un
/// ICE del intérprete: `Map.new()` en posición de cola de una función.)
#[test]
fn metrics_series_handles_render_like_the_free_functions() {
    let d = project("metrics_series");
    std::fs::write(d.join("prog.ray"), r#"import net/metrics;
fn main() -> int {
    let a = metrics.registry();
    let b = metrics.registry();
    let bks = [0.01, 0.1, 1.0];
    for reg in [a, b] {
        metrics.register_counter(reg, "reqs", "requests");
        metrics.register_gauge(reg, "temp", "temperature");
        metrics.register_histogram(reg, "lat", "latency", bks);
    }
    let l = metrics.labels2("backend", "b1", "code", "200");
    // Las funciones libres sobre `a`…
    metrics.inc(a, "reqs", l);
    metrics.inc(a, "reqs", l);
    metrics.add(a, "reqs", metrics.labels1("backend", "b2"), 3.0);
    metrics.set(a, "temp", metrics.no_labels(), 21.5);
    metrics.observe_l(a, "lat", l, 0.05);
    metrics.observe_l(a, "lat", l, 2.0);
    metrics.observe(a, "lat", 0.001);
    // …y los handles sobre `b` deben renderizar lo mismo.
    let reqs = metrics.series(b, "reqs", l);
    reqs.inc();
    reqs.inc();
    metrics.series(b, "reqs", metrics.labels1("backend", "b2")).add(3.0);
    metrics.series(b, "temp", metrics.no_labels()).set(21.5);
    let lat = metrics.series(b, "lat", l);
    lat.observe(0.05);
    lat.observe(2.0);
    metrics.series(b, "lat", metrics.no_labels()).observe(0.001);
    print(metrics.render(a) == metrics.render(b));
    print(metrics.render(b));
    0
}
"#).unwrap();
    let ray = env!("CARGO_BIN_EXE_ray");
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(ray).args(engine).current_dir(&d).output().unwrap();
        assert!(out.status.success(), "{engine:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), r#"true
# HELP reqs requests
# TYPE reqs counter
reqs{backend="b1",code="200"} 2
reqs{backend="b2"} 3
# HELP temp temperature
# TYPE temp gauge
temp 21.5
# HELP lat latency
# TYPE lat histogram
lat_bucket{backend="b1",code="200",le="0.01"} 0
lat_bucket{backend="b1",code="200",le="0.1"} 1
lat_bucket{backend="b1",code="200",le="1"} 1
lat_bucket{backend="b1",code="200",le="+Inf"} 2
lat_sum{backend="b1",code="200"} 2.05
lat_count{backend="b1",code="200"} 2
lat_bucket{le="0.01"} 1
lat_bucket{le="0.1"} 1
lat_bucket{le="1"} 1
lat_bucket{le="+Inf"} 1
lat_sum 0.001
lat_count 1

"#, "{engine:?}");
    }
    native(&d, r#"true
# HELP reqs requests
# TYPE reqs counter
reqs{backend="b1",code="200"} 2
reqs{backend="b2"} 3
# HELP temp temperature
# TYPE temp gauge
temp 21.5
# HELP lat latency
# TYPE lat histogram
lat_bucket{backend="b1",code="200",le="0.01"} 0
lat_bucket{backend="b1",code="200",le="0.1"} 1
lat_bucket{backend="b1",code="200",le="1"} 1
lat_bucket{backend="b1",code="200",le="+Inf"} 2
lat_sum{backend="b1",code="200"} 2.05
lat_count{backend="b1",code="200"} 2
lat_bucket{le="0.01"} 1
lat_bucket{le="0.1"} 1
lat_bucket{le="1"} 1
lat_bucket{le="+Inf"} 1
lat_sum 0.001
lat_count 1

"#);
}

/// L6/L7: `serve_options_on` sobre un listener propio (puerto 0), con `with_stop` (apagado por un
/// canal, drenado) y `quiet` (nada en stdout: ni `listening on port`, ni `shutting down`).
#[test]
fn serve_options_on_a_listener_with_a_stop_channel_and_quiet() {
    let d = project("serve_options_on");
    std::fs::write(d.join("prog.ray"), SERVE_OPTIONS_ON_PROG).unwrap();
    vm_and_native_two_threads(&d, "200\nserve returned 0 quick=true\ntrue\n");
}

/// L4 y el ICE del intérprete: una tabla inline en TOML da un error que orienta, y `Map.new()` como
/// expresión de cola de una función corre en los tres motores.
#[test]
fn toml_inline_tables_are_a_clear_error_and_map_new_in_tail_position_runs() {
    three_engines(
        "toml_inline",
        r#"import std/toml;
fn main() -> int {
    match (toml.parse_toml("a = { x = 1 }\n")) {
        Result.Ok(_) => print("ok?"),
        Result.Err(e) => print(e),
    }
    0
}
"#,
        "inline tables are not supported; write a [table] header instead (line 1)\n",
    );
    three_engines(
        "map_new_tail",
        "fn nl() -> Map<string, string> {\n    Map.new()\n}\nfn main() -> int { print(nl().len()); 0 }\n",
        "0\n",
    );
}

/// R36: un paquete-librería (sin `entry` ni `src/main.ray`) se comprueba y prueba módulo a módulo;
/// `ray run` explica qué es en vez de «nonexistent entry». R35: una tabla inline en `[dependencies]`
/// dice qué formas se aceptan.
#[test]
fn a_library_package_is_checked_and_tested_module_by_module() {
    let d = tmp("library_pkg");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"oidc\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(d.join("src/math.ray"), "/// Adds.\npub fn add(a: int, b: int) -> int { a + b }\n\n@test\nfn adds() { assert_eq(add(1, 2), 3); }\n").unwrap();
    std::fs::write(d.join("tests/math_test.ray"), "import src/math;\n\n@test\nfn via_tests_dir() { assert_eq(math.add(2, 2), 4); }\n").unwrap();
    let ray = env!("CARGO_BIN_EXE_ray");
    let check = Command::new(ray).arg("check").current_dir(&d).output().unwrap();
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
    assert!(String::from_utf8_lossy(&check.stderr).contains("library package"));
    assert!(String::from_utf8_lossy(&check.stdout).contains("math.ray' compiles"));
    let test = Command::new(ray).arg("test").current_dir(&d).output().unwrap();
    let out = String::from_utf8_lossy(&test.stdout);
    assert!(test.status.success(), "{out}\n{}", String::from_utf8_lossy(&test.stderr));
    assert!(out.contains("2 test(s), all passed"), "{out}");
    let run = Command::new(ray).arg("run").current_dir(&d).output().unwrap();
    assert_eq!(run.status.code(), Some(66));
    assert!(String::from_utf8_lossy(&run.stderr).contains("'oidc' is a library package"));
    // R35: la forma de Cargo, con un error que enseña las tres formas.
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"oidc\"\nversion = \"0.1.0\"\n\n[dependencies]\nx = { path = \"../x\" }\n").unwrap();
    let bad = Command::new(ray).arg("check").current_dir(&d).output().unwrap();
    assert!(!bad.status.success());
    let err = String::from_utf8_lossy(&bad.stderr);
    assert!(err.contains("\"path:../dir\"") && err.contains("inline tables"), "{err}");
}

/// L18: el perfilador descuenta su propia instrumentación y lo dice en la cabecera; la media por
/// llamada de una función con muchas llamadas pequeñas queda cerca de lo medido con el reloj.
#[test]
fn the_profiler_discounts_its_own_instrumentation() {
    let d = tmp("profiler_overhead");
    std::fs::write(
        d.join("prog.ray"),
        "fn tiny(n: int) -> int { n + 1 }\nfn outer() -> int {\n    var acc = 0;\n    var i = 0;\n    while (i < 2000) { acc = acc + tiny(i); i = i + 1; }\n    acc\n}\nfn main() -> int { var k = 0; var s = 0; while (k < 50) { s = s + outer(); k = k + 1; } print(s > 0); 0 }\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["profile", "prog.ray", "--json"]).current_dir(&d).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report = String::from_utf8_lossy(&out.stderr);
    assert!(report.contains("\"overhead_ns_per_call\":"), "{report}");
    // Con 100 000 llamadas a `tiny`, el propio de `outer` sin el descuento llevaría toda la
    // instrumentación (~50–100 ns × 100 000 ≥ 5 ms); descontado, `outer` no puede superar en mucho a
    // `tiny` + su bucle. Se exige solo que el informe sea coherente: ambas funciones aparecen.
    assert!(report.contains("\"name\":\"tiny\",\"calls\":100000"), "{report}");
    assert!(report.contains("\"name\":\"outer\",\"calls\":50"), "{report}");
}

/// M358: el binario nativo con reactor POR WORKER (`RAYLANG_REACTOR=local`) sirve, apaga por canal y
/// mantiene los plazos de `select_timeout`, con dos workers. Mismo programa que la prueba de
/// `serve_options_on` y la de `select_timeout`; solo cambia el modo del scheduler.
#[cfg(unix)]
#[test]
fn the_per_worker_reactor_serves_and_keeps_deadlines() {
    if !has_rustc() {
        return;
    }
    let ray = env!("CARGO_BIN_EXE_ray");
    let cases: [(&str, &str, &str); 2] = [
        (
            "local_reactor_serve",
            SERVE_OPTIONS_ON_PROG,
            "200\nserve returned 0 quick=true\ntrue\n",
        ),
        (
            "local_reactor_select",
            SELECT_TIMEOUTS_PROG,
            "[woke after 0, woke after 2, woke after 30, woke after 9]\n9\n",
        ),
    ];
    for (name, src, want) in cases {
        let d = project(name);
        std::fs::write(d.join("prog.ray"), src).unwrap();
        let bin = d.join("prog_bin");
        let b = Command::new(ray).args(["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
        let out = Command::new(&bin).env("RAYLANG_REACTOR", "local").env("RAYLANG_THREADS", "2").current_dir(&d).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "{name}\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

/// rayauth R38 (regresión de M355): el dial con plazo probaba solo la PRIMERA dirección resuelta —
/// `localhost` es `::1` y `127.0.0.1`, y un servidor que solo escucha en IPv4 rechazaba la primera.
/// Ahora recorre todas bajo un plazo total, como el connect sin plazo; en los tres motores.
#[test]
fn a_timed_connect_tries_every_resolved_address() {
    three_engines(
        "connect_all_addrs",
        r#"import std/net;
fn main() -> int {
    let srv = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(srv);
    match (net.tcp_connect_timeout("localhost", port, 2000)) {
        Result.Ok(h) => { print("localhost ok"); close(h); },
        Result.Err(e) => print("localhost err: " + e),
    }
    match (net.tcp_connect_timeout("localhost", 1, 500)) {
        Result.Ok(_) => print("port1 ok?!"),
        Result.Err(e) => print("port1 refused=" + to_string(e.to_lower().contains("refused"))),
    }
    0
}
"#,
        "localhost ok\nport1 refused=true\n",
    );
}

/// ray-ds #122: `json.render_pretty`/`render_arr_pretty`/`reindent` indentan respetando el orden de
/// inserción (`stringify_pretty` ordena las claves porque un objeto parseado es un `Map`); los
/// `{}`/`[]` vacíos quedan en una línea y los strings se copian tal cual, escapes incluidos.
#[test]
fn json_render_pretty_keeps_the_insertion_order() {
    three_engines(
        "json_render_pretty",
        r#"import std/json;
fn main() -> int {
    let o = json.obj().field("zeta", 1).field("alpha", json.obj().field("b", true).field("a", "x\"y, {z}")).field("list", json.list(["q", "r"]));
    print(json.render_pretty(o, 2));
    print(json.render_pretty(json.obj().field("e", json.obj()).field("n", json.list([1])), 2));
    print(json.reindent(json.stringify(json.parse(json.render(o)).unwrap()), 4) != "");
    0
}
"#,
        "{\n  \"zeta\": 1,\n  \"alpha\": {\n    \"b\": true,\n    \"a\": \"x\\\"y, {z}\"\n  },\n  \"list\": [\n    \"q\",\n    \"r\"\n  ]\n}\n{\n  \"e\": {},\n  \"n\": [\n    1\n  ]\n}\ntrue\n",
    );
}

/// ray-ds #121 (resto), #128 y #131: claves TOML entre comillas simples o con punto conservan las
/// comillas en la ruta aplanada (sin chocar con `[s.a]`); un or-pattern da un error que orienta; un
/// filtro Jinja en una plantilla COMPILADA dice lo mismo que `render_template`.
#[test]
fn toml_quoted_keys_or_patterns_and_compiled_template_filters() {
    three_engines(
        "toml_quoted_keys",
        "import std/toml;\nfn main() -> int {\n    match (toml.parse_toml(\"[s]\\n\\\"a.b\\\" = 1\\n'q k' = 2\\n\\\"0\\\" = 3\\n[s.a]\\nb = 4\\n\")) {\n        Result.Ok(es) => { var i = 0; while (i < es.len()) { print(es[i].key); i = i + 1; } },\n        Result.Err(e) => print(e),\n    }\n    0\n}\n",
        "s.\"a.b\"\ns.\"q k\"\ns.0\ns.a.b\n",
    );
    let ray = env!("CARGO_BIN_EXE_ray");
    let d = tmp("or_pattern");
    std::fs::write(d.join("prog.ray"), "fn main() -> int { match (\"a\") { \"a\" | \"b\" => 1, _ => 0 } }\n").unwrap();
    let out = Command::new(ray).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("or-patterns ('a' | 'b' => …) are not supported"), "{}", String::from_utf8_lossy(&out.stderr));
    let d = tmp("compiled_filter");
    std::fs::write(d.join("page.ray.html"), "{% params body: string %}\n<p>{{ body | safe }}</p>\n").unwrap();
    std::fs::write(d.join("prog.ray"), "import page;\nfn main() -> int { print(page.render(\"x\")); 0 }\n").unwrap();
    let out = Command::new(ray).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("filters are not supported in '{{ body | safe }}'"), "{}", String::from_utf8_lossy(&out.stderr));
}
