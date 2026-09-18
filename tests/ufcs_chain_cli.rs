//! M267 (IDEAS §92): dos métodos de trait del MISMO nombre encadenados —
//! `r.unwrap_or(x).unwrap_or(y)` con `r: Result<Option<string>, string>`— son dos `Call(Field)`
//! que comparten posición y nombre; el checker distingue el sitio por profundidad de cadena.
//! Antes el intérprete aplicaba `Option#unwrap_or` al `Result` ("no match branch matched") y la
//! VM acertaba por accidente (variantes por índice). Los tres motores.

use std::process::Command;

const PROG: &str = r#"fn pick(tag: string) -> Result<Option<string>, string> {
    if (tag == "ok") { Result.Ok(Option.Some("v")) } else { Result.Err("e") }
}

fn main() {
    print(pick("ok").unwrap_or(Option.Some("x")).unwrap_or("?"));
    print(pick("no").unwrap_or(Option.Some("x")).unwrap_or("?"));
    print(pick("ok").unwrap_or(Option.None).unwrap_or("?"));
    print(pick("no").unwrap_or(Option.None).unwrap_or("?"));
    // tres eslabones del mismo nombre, con un `map` propio en medio
    let n: Result<Result<Option<int>, string>, string> = Result.Ok(Result.Ok(Option.Some(7)));
    print(n.unwrap_or(Result.Err("a")).unwrap_or(Option.None).unwrap_or(0));
}
"#;

const WANT: &str = "v\nx\nv\n?\n7\n";

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_ufcs_chain_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    d
}

#[test]
fn chained_same_name_trait_methods_on_both_engines() {
    for engine in ["--vm", "--interp"] {
        let d = dir(&engine[2..]);
        let out = Command::new(env!("CARGO_BIN_EXE_raylang")).arg(engine).arg(d.join("prog.ray")).output().unwrap();
        assert!(out.status.success(), "{engine}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine}");
    }
}

#[test]
fn chained_same_name_trait_methods_natively() {
    if !Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("(sin rustc: se omite el nativo)");
        return;
    }
    let d = dir("native");
    let bin = d.join("prog_bin");
    let st = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
        .current_dir(&d)
        .output()
        .expect("build nativo");
    assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
    let out = Command::new(&bin).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo");
}
