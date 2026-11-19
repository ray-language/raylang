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
