//! Hallazgos atendidos en 1.27.72 (rayffi #215, M417): nombres de biblioteca por plataforma en los
//! bloques `extern`, `ffi.library_available` y el mensaje de la biblioteca ausente — en los tres motores.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ray_f1_27_72_{name}_{}", std::process::id()));
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

/// rayffi #215 (M417): un nombre para OTRA plataforma no afecta al sistema en curso (`"m"` sigue
/// siendo libm aquí, aunque Windows diga `ucrtbase`); `blocking` convive con los nombres;
/// `ffi.library_available` responde sin llamar a nada (`m` y `c` siempre; una inexistente, `false`).
#[test]
fn platform_library_names_and_library_available_on_all_engines() {
    let d = tmp("ffi_platform");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/ffi;
extern "m" windows "ucrtbase" {
    fn sqrt(x: float) -> float;
}
extern "c" windows "ucrtbase" blocking {
    fn abs(n: int) -> int;
}
fn main() -> int {
    print(sqrt(16.0));
    print(abs(0 - 7));
    print(ffi.library_available("m"));
    print(ffi.library_available("c"));
    print(ffi.library_available("nosuchlib_raylang_xyz"));
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "4\n7\ntrue\ntrue\nfalse\n");
}

/// rayffi #215 (M417): en la VM, llamar a una biblioteca que no existe dice que falta LA BIBLIOTECA
/// (y qué nombres probó), no solo que el símbolo no aparece.
#[test]
fn a_missing_library_is_reported_as_such_on_the_vm() {
    let d = tmp("ffi_missing");
    std::fs::write(
        d.join("prog.ray"),
        "extern \"nosuchlib_raylang_xyz\" {\n    fn foo() -> int;\n}\nfn main() -> int {\n    print(foo());\n    0\n}\n",
    )
    .unwrap();
    for flags in [&[][..], &["--interp"][..]] {
        let mut args = vec!["run"];
        args.extend_from_slice(flags);
        args.push("prog.ray");
        let (out, err, code) = ray(&d, &args);
        assert_ne!(code, 0, "{flags:?}: {out}");
        assert!(err.contains("library 'nosuchlib_raylang_xyz' not found (tried "), "{flags:?}: {err}");
        assert!(err.contains("symbol 'foo' is not in the process either"), "{flags:?}: {err}");
    }
}
