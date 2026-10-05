//! IDEAS §103 — los hallazgos que raylb (el balanceador de ray-apps) documentó, cada uno con su
//! programa mínimo. M355: el `connect` no retiene al worker (L19/L14) y el cliente HTTP acota el dial (L5).
//! M356: el lote de correcciones (L2, L3, L9, L12, L16, L17) y rayauth R34.

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
