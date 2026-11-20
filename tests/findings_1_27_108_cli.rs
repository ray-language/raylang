//! Ronda de hallazgos re-medidos sobre 1.27.108, cada uno con su repro mínimo.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_f108_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

/// rayserial #460: declarar con aridad fija una variádica de libc (`fcntl`, `ioctl`) compila, pero
/// en Apple arm64 la llamada no hace nada o deja basura; el checker avisa. `open` con dos
/// argumentos no usa la parte variádica y no avisa.
#[test]
fn an_extern_that_declares_a_variadic_libc_function_warns() {
    let d = tmp("variadic_extern");
    let src = "extern \"c\" {\n    fn fcntl(fd: int, cmd: int, arg: int) -> int;\n    fn open(path: string, flags: int) -> int;\n    fn getpid() -> int;\n}\n\nfn main() -> int {\n    print(getpid() > 0);\n    0\n}\n";
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let (out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "true\n");
    assert!(err.contains("warning at 2:5: `fcntl` is variadic in C after its first 2 parameter(s) and `extern` cannot pass variadic arguments"), "{err}");
    assert!(!err.contains("`open`"), "{err}");
}

/// rayfocus #459: un enum o un struct sin `Show` se interpola con la forma por defecto, la misma
/// que muestra `print`. Antes `"${kind}"` era un error de tipo.
#[test]
fn a_struct_or_an_enum_without_show_interpolates_like_print() {
    let d = tmp("interp_default_form");
    let src = "enum K {\n    A,\n    B(int),\n}\n\nstruct P {\n    x: int,\n    k: K,\n}\n\nfn main() -> int {\n    print(\"k: ${K.A} ${K.B(3)}\");\n    let p = P { x: 1, k: K.A };\n    print(\"${p}\");\n    print(p);\n    print(to_string(K.A).len());\n    0\n}\n";
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let expected = "k: K.A K.B(3)\nP { x: 1, k: K.A }\nP { x: 1, k: K.A }\n3\n";
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
