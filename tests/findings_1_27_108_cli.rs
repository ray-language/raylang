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
