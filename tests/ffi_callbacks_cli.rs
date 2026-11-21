//! Callbacks C→raylang en la FFI (IDEAS §131, ray-daw #425): una función de raylang pasada a C
//! como puntero a función, en los tres motores y en el binario nativo.
#![cfg(unix)]
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_ffi_cb_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

// Con libc basta: `qsort` y `bsearch` llaman al comparador durante la llamada.
const CORE: &str = r#"import std/ffi;
extern "c" {
    fn qsort(base: bytes, n: u64, size: u64, cmp: fn(ptr, ptr) -> int);
    fn bsearch(key: bytes, base: bytes, n: u64, size: u64, cmp: fn(ptr, ptr) -> int) -> Option<ptr>;
}
fn b(p: ptr) -> int { ffi.read(p, 1)[0] as int }
fn cmp(x: ptr, y: ptr) -> int { b(x) - b(y) }
fn sorted(xs: [int]) -> bytes {
    let d = bytes_of(xs);
    qsort(d, xs.len() as u64, 1 as u64, cmp);
    d
}
fn main() {
    // Un closure que captura, con basura de sobra para que el GC corra dentro del callback.
    let d = bytes_of([9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
    var junk: [[int]] = [];
    var calls = 0;
    qsort(d, 10 as u64, 1 as u64, fn(x: ptr, y: ptr) -> int {
        var i = 0;
        while (i < 2000) {
            junk = [[i, i + 1, i + 2]];
            i = i + 1;
        }
        calls = calls + 1;
        b(x) - b(y)
    });
    print(d);
    print(calls > 0);
    // Un callback que hace otra llamada con callback.
    let outer = bytes_of([3, 1, 2]);
    qsort(outer, 3 as u64, 1 as u64, fn(x: ptr, y: ptr) -> int {
        let inner = sorted([2, 1]);
        b(x) - b(y) + (inner[0] as int) - 1
    });
    print(outer);
    // Una función con nombre, y un retorno `Option<ptr>` de la extern.
    print(bsearch(bytes_of([7]), d, 10 as u64, 1 as u64, cmp).is_some());
    print(bsearch(bytes_of([77]), d, 10 as u64, 1 as u64, cmp).is_some());
    // Un error dentro del callback lo ve la llamada extern; `try_call` lo recupera.
    print(try_call(fn() -> int {
        qsort(bytes_of([2, 1]), 2 as u64, 1 as u64, fn(x: ptr, y: ptr) -> int {
            let z: [int] = [];
            z[3]
        });
        1
    }));
    // Un `try_call` dentro del callback recupera sin salir de él.
    let d3 = bytes_of([2, 1, 3]);
    qsort(d3, 3 as u64, 1 as u64, fn(x: ptr, y: ptr) -> int {
        let t = try_call(fn() -> int { panic("boom") });
        if (t.is_err()) { b(x) - b(y) } else { 0 }
    });
    print(d3);
}
"#;

const CORE_OUT: &str = "00010203040506070809\ntrue\n010203\ntrue\nfalse\nResult.Err(index 3 out of range (length 0))\n010203\n";

// Lo que necesita concurrencia: esperar dentro de un callback, y un callback en una fibra hija.
const WAITING: &str = r#"import std/ffi;
import std/time;
extern "c" {
    fn qsort(base: bytes, n: u64, size: u64, cmp: fn(ptr, ptr) -> int);
}
fn b(p: ptr) -> int { ffi.read(p, 1)[0] as int }
fn sorted(xs: [int]) -> bytes {
    let d = bytes_of(xs);
    qsort(d, xs.len() as u64, 1 as u64, fn(x: ptr, y: ptr) -> int { b(x) - b(y) });
    d
}
fn main() {
    print(try_call(fn() -> int {
        qsort(bytes_of([2, 1]), 2 as u64, 1 as u64, fn(x: ptr, y: ptr) -> int {
            time.sleep(5);
            0
        });
        1
    }));
    let ch: Channel<int> = Channel.new();
    print(try_call(fn() -> int {
        qsort(bytes_of([2, 1]), 2 as u64, 1 as u64, fn(x: ptr, y: ptr) -> int { ch.recv().unwrap_or(0) });
        1
    }));
    // La fibra sigue sana después de los dos errores.
    print(sorted([3, 1, 2]));
    let t = spawn(fn() -> bytes { sorted([4, 2, 9]) });
    print(join(t));
}
"#;

const WAIT_ERR: &str = "Result.Err(cannot wait inside an FFI callback (channels, sleep, join and network I/O are not available while C code is on the stack))\n";

fn native(dir: &PathBuf, prog: &str, extra: &[&str]) -> Option<String> {
    if !have("rustc") {
        return None;
    }
    let bin = dir.join("prog_bin");
    let mut args = vec!["build", prog, "--native", "--no-stubs", "-o", bin.to_str().unwrap()];
    args.extend_from_slice(extra);
    let (_o, err, code) = ray(dir, &args);
    assert_eq!(code, 0, "build --native {extra:?}: {err}");
    Some(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout).into_owned())
}

#[test]
fn a_raylang_function_is_called_back_by_c_in_every_engine() {
    let d = tmp("core");
    std::fs::write(d.join("prog.ray"), CORE).unwrap();
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!((out.as_str(), code), (CORE_OUT, 0), "{args:?}: {err}");
    }
    // El nativo por defecto (fibras) y el `rustc` pelado (sin `ray-runtime`).
    for extra in [&[][..], &["--without", "mimalloc,ahash,fibers"][..]] {
        if let Some(out) = native(&d, "prog.ray", extra) {
            assert_eq!(out, CORE_OUT, "native {extra:?}");
        }
    }
}

#[test]
fn a_callback_cannot_wait_and_the_fiber_survives_the_error() {
    let d = tmp("waiting");
    std::fs::write(d.join("prog.ray"), WAITING).unwrap();
    let expected = format!("{WAIT_ERR}{WAIT_ERR}010203\n020409\n");
    let (out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!((out.as_str(), code), (expected.as_str(), 0), "{err}");
    if let Some(out) = native(&d, "prog.ray", &[]) {
        assert_eq!(out, expected);
    }
}

// Las formas que libc no ejercita: flotantes mezclados con enteros, `bool`, `u64` anchos,
// `Option<ptr>` en los dos sentidos y un callback sin retorno.
const SHAPES_C: &str = r#"#include <stdbool.h>
#include <stdint.h>

double cb_mix(double (*f)(int, double, uint64_t, double, bool), int a, double b) {
    return f(a, b, (uint64_t)1 << 40, 0.5, true) + f(-a, -b, 7, 0.25, false);
}
int cb_each(int n, void *user, int (*f)(void *, int)) {
    int total = 0;
    for (int i = 0; i < n; i++) total += f(user, i);
    return total;
}
void *cb_pick(void *(*f)(void *), void *p) { return f(p); }
void cb_void(void (*f)(int), int n) { f(n); f(n + 1); }
uint64_t cb_wide(uint64_t (*f)(uint64_t, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t)) {
    return f(1, 2, 3, 4, 5, (uint64_t)1 << 50);
}
bool cb_bool(bool (*f)(bool, bool), bool a, bool b) { return f(a, b); }
"#;

const SHAPES: &str = r#"import std/ffi;
extern "cbshapes" {
    fn cb_mix(f: fn(int, float, u64, float, bool) -> float, a: int, b: float) -> float;
    fn cb_each(n: int, user: Option<ptr>, f: fn(Option<ptr>, int) -> int) -> int;
    fn cb_pick(f: fn(ptr) -> Option<ptr>, p: bytes) -> Option<ptr>;
    fn cb_void(f: fn(int) -> unit, n: int);
    fn cb_wide(f: fn(u64, u64, u64, u64, u64, u64) -> u64) -> u64;
    fn cb_bool(f: fn(bool, bool) -> bool, a: bool, b: bool) -> bool;
}
fn main() {
    print(cb_mix(fn(a: int, b: float, c: u64, d: float, e: bool) -> float {
        let big = if (c > (1000 as u64)) { 1000.0 } else { 0.0 };
        let flag = if (e) { 100.0 } else { 0.0 };
        (a as float) + b + d + big + flag
    }, 3, 1.5));
    var seen: [int] = [];
    print(cb_each(4, Option.None, fn(user: Option<ptr>, i: int) -> int {
        seen.push(i);
        if (user.is_none()) { i * 10 } else { -1 }
    }));
    print(seen);
    let buf = b"abc";
    print(cb_pick(fn(p: ptr) -> Option<ptr> { Option.Some(ffi.offset(p, 1)) }, buf).map(fn(p: ptr) -> bytes { ffi.read(p, 2) }));
    print(cb_pick(fn(p: ptr) -> Option<ptr> { Option.None }, buf).is_none());
    var total = 0;
    cb_void(fn(n: int) { total = total + n; }, 20);
    print(total);
    print(cb_wide(fn(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 { a + b + c + d + e + f }));
    print(cb_bool(fn(a: bool, b: bool) -> bool { a && !b }, true, false));
    print(cb_bool(fn(a: bool, b: bool) -> bool { a && !b }, true, true));
}
"#;

const SHAPES_OUT: &str = "1100.75\n60\n[0, 1, 2, 3]\nOption.Some(6263)\ntrue\n41\n1125899906842639\ntrue\nfalse\n";

#[test]
fn every_callback_signature_shape_reaches_raylang_intact() {
    if !have("cc") {
        return;
    }
    let d = tmp("shapes");
    std::fs::write(d.join("shapes.c"), SHAPES_C).unwrap();
    std::fs::write(d.join("prog.ray"), SHAPES).unwrap();
    let lib = if cfg!(target_os = "macos") { "libcbshapes.dylib" } else { "libcbshapes.so" };
    let cc = Command::new("cc").args(["-shared", "-fPIC", "-o", lib, "shapes.c"]).current_dir(&d).output().unwrap();
    assert!(cc.status.success(), "cc: {}", String::from_utf8_lossy(&cc.stderr));
    // El cargador dinámico y el enlazador encuentran la librería por el entorno.
    let env = [("DYLD_LIBRARY_PATH", &d), ("LD_LIBRARY_PATH", &d), ("LIBRARY_PATH", &d)];
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).envs(env).current_dir(&d).output().unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
    };
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = run(args);
        assert_eq!((out.as_str(), code), (SHAPES_OUT, 0), "{args:?}: {err}");
    }
    if have("rustc") {
        let bin = d.join("prog_bin");
        let (_o, err, code) = run(&["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        let out = Command::new(&bin).envs(env).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), SHAPES_OUT);
    }
}

#[test]
fn the_checker_rejects_callbacks_it_cannot_marshal() {
    let d = tmp("checker");
    let cases = [
        (
            "extern \"c\" { fn f(cb: fn(string) -> int); }\nfn main() {}\n",
            "the callback parameter 'cb' of 'extern fn f': a callback parameter of type string is not supported (int, u64, float, bool, ptr, Option<ptr>)",
        ),
        (
            "extern \"c\" { fn f(cb: fn(int) -> string); }\nfn main() {}\n",
            "the callback parameter 'cb' of 'extern fn f': a callback returning string is not supported (int, u64, float, bool, ptr, Option<ptr>, unit)",
        ),
        (
            "extern \"c\" { fn f(cb: fn(int, int, int, int, int, int, int) -> int); }\nfn main() {}\n",
            "a callback takes at most 6 integer/pointer parameters and 4 float parameters",
        ),
        (
            "extern \"c\" { fn f(cb: fn(fn(int) -> int) -> int); }\nfn main() {}\n",
            "a callback parameter of type fn(int) -> int is not supported",
        ),
        (
            "extern \"c\" blocking { fn f(cb: fn(int) -> int); }\nfn main() {}\n",
            "'extern fn f' is declared in a 'blocking' block and cannot take a callback (it would be called from another thread)",
        ),
        (
            "extern \"c\" { fn qsort(base: bytes, n: u64, size: u64, cmp: fn(ptr, ptr) -> int); }\nfn main() { qsort(b\"ba\", 2 as u64, 1 as u64, fn(x: ptr) -> int { 0 }); }\n",
            "argument 4 of 'qsort'",
        ),
    ];
    for (src, expected) in cases {
        std::fs::write(d.join("bad.ray"), src).unwrap();
        let (_out, err, code) = ray(&d, &["check", "bad.ray"]);
        assert_eq!(code, 65, "{src}");
        assert!(err.contains(expected), "{src}\n{err}");
    }
}
