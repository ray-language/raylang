//! Arco de rendimiento del nativo (nov 2026): los programas mínimos de cada paso, idénticos en
//! VM, intérprete y nativo. Las mediciones viven en `benchmarks/probe/`; aquí solo se asevera que
//! cada optimización conserva el resultado.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_native_perf_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Corre `src` en VM, intérprete (salvo `interp = false`: fibras) y nativo; los tres dan `want`.
fn engines(name: &str, src: &str, want: &str, interp: bool) {
    let d = tmp(name);
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let mut runs: Vec<&[&str]> = vec![&["run", "prog.ray"]];
    if interp {
        runs.push(&["run", "--interp", "prog.ray"]);
    }
    for args in runs {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(&d).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{args:?}");
    }
    if has_rustc() {
        let bin = d.join("prog_bin");
        let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        let err = String::from_utf8_lossy(&b.stderr);
        assert!(b.status.success(), "build --native: {err}");
        assert!(!err.contains("not supported in the native subset"), "nativo con stubs: {err}");
        let o = Command::new(&bin).output().unwrap();
        assert!(o.status.success(), "nativo: {}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "nativo");
    }
}

/// Un tipo de usuario con el nombre de un tipo de Rust que el código generado usa sin calificar
/// (`Rc`, `Vec`, `String`, `Box`, `Send`…) compila a nativo y se muestra con su nombre original.
#[test]
fn user_types_named_like_rust_types_build_natively() {
    engines(
        "names",
        r#"@derive(Eq, Show)
struct Rc { w: int, }
@derive(Show)
struct Vec { n: int, }
struct String { s: string, }
struct Box { v: int, }
enum Arc { One(int), Two, }
struct RefCell { c: int, }
struct HashMap { k: int, }
struct Cell { k: int, }
trait Send { fn go(self) -> int; }
impl Send for Box { fn go(self) -> int { self.v } }
fn total<T: Send>(x: T) -> int { x.go() }
fn main() {
    let a = Rc { w: 1 };
    let v = Vec { n: 2 };
    let s = String { s: "x" };
    let b = Box { v: 3 };
    let e = Arc.One(4);
    let r = RefCell { c: 5 };
    let h = HashMap { k: 6 };
    let c = Cell { k: 7 };
    let xs = [a, Rc { w: 9 }];
    let m: Map<string, Rc> = Map.new();
    m.insert("k", a);
    print(a);
    print(v);
    print(xs.len() + m.len() + v.n + s.s.len() + total(b) + r.c + h.k + c.k);
    match (e) { Arc.One(x) => print(x), Arc.Two => print(0), }
    print(a == Rc { w: 1 });
}
"#,
        "Rc { w: 1 }\nVec { n: 2 }\n27\n4\ntrue\n",
        true,
    );
}
