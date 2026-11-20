//! Ronda de hallazgos re-medidos sobre 1.27.107, cada uno con su repro mínimo.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_f107_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

/// raycdc #407: una función de un submódulo con nombre de builtin (`send`) tapa al builtin dentro
/// de su módulo; `ray check` del archivo suelto da el mismo veredicto que el proyecto, y el mismo
/// mensaje cuando la llamada está mal.
#[test]
fn a_loose_module_binds_its_builtin_named_functions_like_the_project() {
    let d = tmp("loose_module");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"loose\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(d.join("src/wire.ray"), "fn send(a: int, b: int, c: int) -> int { a + b + c }\npub fn total() -> int { send(1, 2, 3) }\n").unwrap();
    std::fs::write(d.join("src/main.ray"), "import wire;\nfn main() -> int { print(wire.total()); 0 }\n").unwrap();
    let (out, err, code) = ray(&d, &["check", "src/wire.ray"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("compiles (module without main)"), "{out}{err}");
    let (out, err, code) = ray(&d, &["run"]);
    assert_eq!((out.as_str(), code), ("6\n", 0), "{err}");

    // Una llamada mal hecha: el mismo mensaje suelto que desde el proyecto.
    std::fs::write(d.join("src/wire.ray"), "fn send(a: int, b: int, c: int) -> int { a + b + c }\npub fn total() -> int { send(1, 2) }\n").unwrap();
    let expected = "'wire::send' expects 3 argument(s), received 2 (the module's own 'send' shadows the builtin here; call builtin.send(…) for the builtin)";
    for args in [&["check", "src/wire.ray"][..], &["check"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 65, "{args:?}: {out}{err}");
        assert!(err.contains(expected), "{args:?}: {err}");
    }

    // La entrada (con `main`) sigue sin poder redefinir un builtin.
    std::fs::write(d.join("src/root.ray"), "fn send(a: int) -> int { a }\nfn main() -> int { send(1) }\n").unwrap();
    let (_out, err, code) = ray(&d, &["check", "src/root.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("'send' is a language builtin and cannot be redefined"), "{err}");
}

/// raysolve #358: la inferencia de una llamada genérica no depende del ORDEN de sus argumentos —
/// el que no puede tiparse solo (`empty()`) espera a que los demás fijen el parámetro de tipo.
/// Antes `count(a, empty())` valía y `count(empty(), a)` no.
#[test]
fn generic_arguments_are_inferred_in_any_order() {
    let d = tmp("infer_order");
    std::fs::write(
        d.join("prog.ray"),
        "fn empty<V>() -> [V] {\n    let xs: [V] = [];\n    xs\n}\nfn count<V>(a: [V], b: [V]) -> int { a.len() + b.len() }\nfn pick<A, B>(x: Option<A>, y: [B], z: A, w: B) -> int { y.len() }\nfn main() {\n    let a = [1, 2, 3];\n    print(count(a, empty()));\n    print(count(empty(), a));\n    print(pick(Option.None, empty(), 1, \"s\"));\n}\n",
    )
    .unwrap();
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!((out.as_str(), code), ("3\n3\n0\n", 0), "{args:?}: {err}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        assert_eq!(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout), "3\n3\n0\n");
    }
    // Si NADIE fija el parámetro, sigue siendo un error de inferencia.
    std::fs::write(d.join("bad.ray"), "fn empty<V>() -> [V] {\n    let xs: [V] = [];\n    xs\n}\nfn count<V>(a: [V], b: [V]) -> int { a.len() + b.len() }\nfn main() { print(count(empty(), empty())); }\n").unwrap();
    let (_out, err, code) = ray(&d, &["check", "bad.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("could not infer the type parameter 'V' of 'empty'"), "{err}");
}

/// De la revisión previa a 1.27.108 (raysolve #358): el mapa vacío `[:]` también espera a que los
/// demás argumentos fijen el parámetro de tipo, como `[]`.
#[test]
fn an_empty_map_argument_waits_for_the_others_like_an_empty_array() {
    let d = tmp("empty_map_arg");
    std::fs::write(d.join("prog.ray"), "fn size<V>(m: Map<string, V>, v: V) -> int { m.len() }\nfn main() { print(size([:], 1)); }\n").unwrap();
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, "0\n", "{args:?}");
    }
}

/// raysolve #456: `"${to_string(v)}"` con un `v` que tiene `Show`. La envoltura `to_string(…)` de
/// la interpolación comparte posición y nombre con la llamada escrita, y el renombre a `N#show`
/// caía en las dos: ICE en la VM y E0308 en nativo.
#[test]
fn to_string_of_a_show_value_inside_an_interpolation_is_applied_once() {
    let d = tmp("interp_to_string");
    let src = r#"@derive(Show)
struct N {
    n: int,
}

struct M {
    n: int,
}

impl Show for M {
    fn show(self) -> string {
        "M(${self.n})"
    }
}

fn wrap<V: Show>(v: V) -> string {
    "<${to_string(v)}>"
}

fn main() -> int {
    let v = N { n: 2 };
    print("${to_string(v)}");
    print("a ${to_string(v)} b");
    print("${to_string(v).len()}");
    print("${to_string(Option.Some(N { n: 3 }))}");
    print("${to_string(M { n: 4 })}");
    print(wrap(M { n: 5 }));
    print("${"${to_string(v)}"}");
    print("${to_string(to_string(v))}");
    print("${v} ${to_string(7)}");
    0
}
"#;
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let expected = "N { n: 2 }\na N { n: 2 } b\n10\nOption.Some(N { n: 3 })\nM(4)\n<M(5)>\nN { n: 2 }\nN { n: 2 }\nN { n: 2 } 7\n";
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, expected, "{args:?}");
    }
    let (out, err, code) = ray(&d, &["build", "--native", "prog.ray", "-o", "prog"]);
    assert_eq!(code, 0, "{out}{err}");
    let native = Command::new(d.join("prog")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&native.stdout), expected);
}
