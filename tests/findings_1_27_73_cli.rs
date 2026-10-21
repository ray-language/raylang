//! Hallazgos atendidos en 1.27.73 (raydata #206, M418): `==`/`!=` despachan al `impl Eq` escrito a
//! mano de un struct/enum y al bound de un `T: Eq`; un `@derive(Eq)`, `Option`/`Result`, las tuplas y
//! los arreglos siguen siendo estructurales. Igual en VM, intérprete y nativo.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ray_f1_27_73_{name}_{}", std::process::id()));
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
    for flags in [&[][..], &["--interp"][..]] {
        let mut args = vec!["run"];
        args.extend_from_slice(flags);
        args.push("prog.ray");
        let (out, err, code) = ray(dir, &args);
        assert_eq!(code, 0, "{flags:?}: {out}\n{err}");
        assert_eq!(out, want, "{flags:?}");
    }
    if has_rustc() {
        let bin = dir.join("prog_native");
        let (out, err, code) = ray(dir, &["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build: {out}\n{err}");
        let o = Command::new(&bin).current_dir(dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "nativo");
    }
}

/// raydata #206 (M418): el `impl Eq` a mano manda en `==`/`!=` (struct y enum, también dentro de
/// un genérico `T: Eq`); el derive y el prelude siguen estructurales; `contains` no cambia.
#[test]
fn equality_operators_dispatch_to_a_hand_written_impl_eq_on_all_engines() {
    let d = tmp("eq_dispatch");
    std::fs::write(
        d.join("prog.ray"),
        r#"struct Tag { name: string }
impl Eq for Tag {
    fn eq(self, other: Self) -> bool { self.name.to_lower() == other.name.to_lower() }
}
@derive(Eq)
struct P { x: int, y: int }
enum Color { Rgb(int, int, int), Named(string) }
impl Eq for Color {
    fn eq(self, other: Self) -> bool {
        match ((self, other)) {
            (Color.Rgb(r1, g1, b1), Color.Rgb(r2, g2, b2)) => r1 == r2 && g1 == g2 && b1 == b2,
            (Color.Named(a), Color.Named(b)) => a.to_lower() == b.to_lower(),
            (Color.Named(n), Color.Rgb(r, g, b)) => n.to_lower() == "black" && r == 0 && g == 0 && b == 0,
            (Color.Rgb(r, g, b), Color.Named(n)) => n.to_lower() == "black" && r == 0 && g == 0 && b == 0,
        }
    }
}
fn same<T: Eq>(a: T, b: T) -> bool { a == b }
fn differ<T: Eq>(a: T, b: T) -> bool { a != b }
fn main() -> int {
    let a = Tag { name: "Hello" };
    let b = Tag { name: "HELLO" };
    print(a == b);
    print(a != b);
    print(same(a, b));
    print(differ(a, b));
    print(P { x: 1, y: 2 } == P { x: 1, y: 2 });
    print(P { x: 1, y: 2 } != P { x: 2, y: 1 });
    print(Color.Named("Black") == Color.Rgb(0, 0, 0));
    print(Color.Named("red") != Color.Named("RED"));
    let o: Option<int> = Option.Some(3);
    print(o == Option.Some(3));
    print(o != Option.None);
    print([a].contains(b));
    print(same(2, 2));
    print(same("x", "y"));
    print(("h", 1) == ("h", 1));
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "true\nfalse\ntrue\nfalse\ntrue\ntrue\ntrue\nfalse\ntrue\ntrue\nfalse\ntrue\nfalse\ntrue\n");
}
