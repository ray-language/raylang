//! Pruebas del runner `@test` (M10.1) sobre el binario: ejecutan `raylang --test
//! <archivo>` y comprueban el informe y el código de salida (número de fallos).

use std::io::Write;
use std::process::Command;

/// Escribe `src` a un archivo temporal, ejecuta `raylang --test <archivo>` y devuelve
/// `(stdout, código de salida)`.
fn run_tests(src: &str, name: &str) -> (String, i32) {
    run_tests_filtro(src, name, None)
}

/// Como `run_tests`, pero pasa un filtro de nombre tras la ruta (M13.2b).
fn run_tests_filtro(src: &str, name: &str, filtro: Option<&str>) -> (String, i32) {
    let mut path = std::env::temp_dir();
    path.push(name);
    let mut f = std::fs::File::create(&path).expect("crea el file temporal");
    f.write_all(src.as_bytes()).expect("escribe la source");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.arg("--test").arg(&path);
    if let Some(p) = filtro {
        cmd.arg(p);
    }
    let out = cmd.output().expect("ejecuta el binary");
    let code = out.status.code().unwrap_or(-1);
    (String::from_utf8_lossy(&out.stdout).into_owned(), code)
}

#[test]
fn runs_las_tests_e_informa_fallos() {
    let src = r#"
fn square(x: int) -> int { x * x }
@test fn square_ok() -> bool { square(3) == 9 }
@test fn sum_ok() -> bool { 1 + 1 == 2 }
@test fn fails() -> bool { 2 + 2 == 5 }
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests(src, "ray_test_mix.ray");
    assert!(out.contains("ok    square_ok"), "informa las what pasan\n{out}");
    assert!(out.contains("ok    sum_ok"), "{out}");
    assert!(out.contains("FALLO fails"), "informa la what fails\n{out}");
    assert_eq!(code, 1, "el código de output es el número de fallos");
}

#[test]
fn todas_pasan_code_cero() {
    let src = "@test fn a() -> bool { true }\n@test fn b() -> bool { !false }\nfn main() -> int { 0 }\n";
    let (out, code) = run_tests(src, "ray_test_ok.ray");
    assert!(out.contains("todas pasaron"), "{out}");
    assert_eq!(code, 0);
}

#[test]
fn sin_tests_lo_indica() {
    let (out, code) = run_tests("fn main() -> int { 0 }\n", "ray_test_none.ray");
    assert!(out.contains("no hay tests"), "{out}");
    assert_eq!(code, 0);
}

#[test]
fn tests_unit_con_assert() {
    // M13.2b: una @test puede devolver unit; pasa si no dispara assert/panic.
    let src = r#"
@test fn assert_ok() { assert_eq(2 + 2, 4); assert(true); }
@test fn assert_fails() { assert_eq(2 + 2, 5); }
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests(src, "ray_test_unit.ray");
    assert!(out.contains("ok    assert_ok"), "la unit what pasa\n{out}");
    assert!(out.contains("FALLO assert_fails"), "la unit what fails\n{out}");
    assert!(out.contains("assert_eq falló: 4 != 5"), "shows el mensaje del assert\n{out}");
    assert_eq!(code, 1);
}

#[test]
fn panic_no_abort_la_bateria() {
    // M13.2b: cada prueba corre aislada; un panic en una no impide ejecutar las demás.
    let src = r#"
@test fn first() { panic("boom"); }
@test fn second() -> bool { true }
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests(src, "ray_test_panic.ray");
    assert!(out.contains("FALLO first"), "{out}");
    assert!(out.contains("boom"), "shows el mensaje del panic\n{out}");
    assert!(out.contains("ok    second"), "la second runs pese al panic de la first\n{out}");
    assert_eq!(code, 1);
}

#[test]
fn filtro_por_name() {
    // M13.2b: un argumento tras la ruta selecciona por subcadena del nombre.
    let src = r#"
@test fn sum_ok() -> bool { 1 + 1 == 2 }
@test fn resta_ok() -> bool { 3 - 1 == 2 }
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests_filtro(src, "ray_test_filtro.ray", Some("sum"));
    assert!(out.contains("ok    sum_ok"), "runs la what casa\n{out}");
    assert!(!out.contains("resta_ok"), "no runs la what no casa\n{out}");
    assert_eq!(code, 0);
}
