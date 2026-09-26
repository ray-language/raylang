//! IDEAS §97 — los hallazgos de lenguaje del barrido de `ray-apps` a 1.27.11, cada uno con su
//! programa mínimo en los tres motores (VM, intérprete y nativo si hay rustc).

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_findings_{name}"));
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
        let out = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo");
    }
}

/// #2 (M300): `break`/`continue` como expresión en un brazo de `match` y en un `if`-valor.
#[test]
fn break_and_continue_as_expressions_run_on_all_engines() {
    let d = tmp("break_expr");
    std::fs::write(
        d.join("prog.ray"),
        r#"fn sum_until_error(xs: [Result<int, string>]) -> int {
    var total = 0;
    for r in xs {
        let v = match (r) {
            Result.Ok(v) => v,
            Result.Err(e) => break,
        };
        let w = if (v < 0) { continue } else { v };
        total = total + w;
    }
    total
}

fn main() -> int {
    print(sum_until_error([Result.Ok(1), Result.Ok(0 - 5), Result.Ok(2), Result.Err("x"), Result.Ok(100)]));
    var i = 0;
    while (true) {
        i = i + 1;
        let _ = match (Option.Some(i)) {
            Option.Some(n) => if (n >= 3) { break } else { n },
            Option.None => continue,
        };
    }
    print(i);
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "3\n3\n");
}
