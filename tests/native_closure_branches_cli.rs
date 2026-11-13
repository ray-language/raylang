//! Una closure literal como VALOR de una rama (`if`/`else`, brazo de `match`) compila a nativo:
//! cada closure de Rust tiene su propio tipo y sin un cast a `Rc<dyn Fn(…)>` las ramas no tenían
//! tipo común — `ray build --native` moría con un E0308 de rustc sobre el código generado.
use std::process::Command;

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn closures_in_branches_build_natively_and_match_the_vm() {
    let d = std::env::temp_dir().join("raylang_test_closure_branches");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("prog.ray"),
        r#"trait Sh { fn area(self) -> int; }
struct Sq { s: int, }
impl Sh for Sq { fn area(self) -> int { self.s * self.s } }
fn pick<T: Sh>(a: T, b: T, first: bool) -> int {
    let f = if (first) { fn(x: T) -> int { x.area() } } else { fn(x: T) -> int { x.area() * 10 } };
    f(a) + f(b)
}
fn plain(first: bool) -> int {
    let f = if (first) { fn(x: int) -> int { x + 1 } } else { fn(x: int) -> int { x * 10 } };
    f(2)
}
fn by_match<T: Sh>(a: T, k: int) -> int {
    let f = match (k) { 0 => fn(x: T) -> int { x.area() }, _ => fn(x: T) -> int { 0 - x.area() }, };
    f(a)
}
fn main() {
    print(pick(Sq { s: 1 }, Sq { s: 2 }, false));
    print(pick(Sq { s: 1 }, Sq { s: 2 }, true));
    print(plain(true) + plain(false));
    print(by_match(Sq { s: 3 }, 0) + by_match(Sq { s: 3 }, 1));
}
"#,
    )
    .unwrap();
    let want = "50\n5\n23\n0\n";
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(&d).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{args:?}");
    }
    if has_rustc() {
        let bin = d.join("prog_bin");
        let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
        let o = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "nativo");
    }
}
