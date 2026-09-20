//! M270 — el lote de raystream (NOTES-raylang.md [1], [6], [8], [11], [12]): cinco bugs del checker
//! y de los diagnósticos, cada uno con su programa mínimo. Los tres motores donde aplica.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_raystream_{name}"));
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
        assert!(!err.contains("not supported in the native subset"), "sin stubs:\n{err}");
        let out = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo");
    }
}

/// [1] La raíz redefine `get` (una función del prelude): su código usa la suya y `std/json`, que
/// llama a `get(obj, k)`, sigue viendo la del prelude. Antes: "[std/json] type error … argument 1
/// of 'get': expected Box, got Map<…>".
#[test]
fn a_root_override_of_a_prelude_function_does_not_leak_into_modules() {
    let d = tmp("override");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/json;

pub struct Box { v: int, }

pub fn get(b: Box, key: string) -> int { b.v }

fn main() -> int {
    match (json.parse("{\"a\": 1}")) {
        Result.Ok(j) => print(json.stringify(j)),
        Result.Err(e) => print(e),
    }
    print(get(Box { v: 7 }, "k"));
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "{\"a\":1}\n7\n");
}

/// [8] Un tipo con el nombre de un trait del prelude: error en EL TIPO DEL USUARIO, con su posición
/// real (antes: "type error at 1000000666:1: 'Sub' is already a type; it cannot also be a trait").
#[test]
fn a_type_named_like_a_prelude_trait_is_reported_at_the_type() {
    let d = tmp("prelude_trait");
    std::fs::write(d.join("prog.ray"), "struct Sub { v: int, }\n\nfn main() -> int {\n    print(Sub { v: 1 }.v);\n    0\n}\n").unwrap();
    let (_o, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("type error at 1:1: 'Sub' is the name of a prelude trait; choose another name for this type"), "{err}");
    assert!(err.contains("| struct Sub"), "con extracto y cursor:\n{err}");
}

/// [11] `==`/`!=` entre enums (igualdad estructural) y `assert_eq` sobre un enum derivado: los
/// tres motores (antes: "requires both operands of the same comparable type, not Route and Route" en
/// la VM y E0369 en el nativo; `==` sobre structs tampoco compilaba nativo).
#[test]
fn enums_and_structs_compare_with_eq_on_every_engine() {
    let d = tmp("enum_eq");
    std::fs::write(
        d.join("prog.ray"),
        r#"@derive(Eq, Show)
enum Route { Home, Media(string), }

struct P { x: int, tags: [string], }

fn main() -> int {
    assert_eq(Route.Media("a"), Route.Media("a"));
    print(Route.Home == Route.Home);
    print(Route.Media("a") == Route.Media("b"));
    print(Route.Home != Route.Media("x"));
    print(P { x: 1, tags: ["t"] } == P { x: 1, tags: ["t"] });
    print(P { x: 1, tags: ["t"] } == P { x: 2, tags: ["t"] });
    let o: Option<Route> = Option.Some(Route.Home);
    print(o == Option.Some(Route.Home));
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "true\nfalse\ntrue\ntrue\nfalse\ntrue\n");
}

/// [12] `to_string(x)` con `x: Show` es `x.show()` (la referencia lo prometía; solo `print` cumplía).
#[test]
fn to_string_accepts_show_types() {
    let d = tmp("to_string_show");
    std::fs::write(
        d.join("prog.ray"),
        r#"@derive(Show)
enum Route { Home, Media(string), }

@derive(Show)
struct P { x: int, }

fn main() -> int {
    print(to_string(Route.Media("a")) + "|" + to_string(P { x: 3 }) + "|" + to_string(42));
    let s: string = to_string(Route.Home);
    print(s.len());
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "Route.Media(a)|P { x: 3 }|42\n10\n");
}

/// [6] `ray check` de un MÓDULO sin `main` lo verifica como módulo (antes: "missing entry function
/// 'main'"); un error real del cuerpo se sigue reportando; `ray run` sigue exigiendo `main`.
#[test]
fn ray_check_accepts_a_module_without_main() {
    let d = tmp("check_module");
    std::fs::write(d.join("lib.ray"), "pub fn twice(x: int) -> int { x * 2 }\n").unwrap();
    let (out, err, code) = ray(&d, &["check", "lib.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("compiles"), "{out}");
    std::fs::write(d.join("bad.ray"), "pub fn twice(x: int) -> int { x * \"2\" }\n").unwrap();
    let (_o, err, code) = ray(&d, &["check", "bad.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("type error"), "{err}");
    let (_o, err, code) = ray(&d, &["run", "lib.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("missing entry function 'main'"), "{err}");
}
