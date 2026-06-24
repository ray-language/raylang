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
    let mut f = std::fs::File::create(&path).expect("crea el archivo temporal");
    f.write_all(src.as_bytes()).expect("escribe la fuente");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.arg("--test").arg(&path);
    if let Some(p) = filtro {
        cmd.arg(p);
    }
    let out = cmd.output().expect("ejecuta el binario");
    let code = out.status.code().unwrap_or(-1);
    (String::from_utf8_lossy(&out.stdout).into_owned(), code)
}

#[test]
fn corre_las_pruebas_e_informa_fallos() {
    let src = r#"
fn cuadrado(x: int) -> int { x * x }
@test fn cuadrado_ok() -> bool { cuadrado(3) == 9 }
@test fn suma_ok() -> bool { 1 + 1 == 2 }
@test fn falla() -> bool { 2 + 2 == 5 }
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests(src, "ray_test_mix.ray");
    assert!(out.contains("ok    cuadrado_ok"), "informa las que pasan\n{out}");
    assert!(out.contains("ok    suma_ok"), "{out}");
    assert!(out.contains("FALLO falla"), "informa la que falla\n{out}");
    assert_eq!(code, 1, "el código de salida es el número de fallos");
}

#[test]
fn todas_pasan_codigo_cero() {
    let src = "@test fn a() -> bool { true }\n@test fn b() -> bool { !false }\nfn main() -> int { 0 }\n";
    let (out, code) = run_tests(src, "ray_test_ok.ray");
    assert!(out.contains("todas pasaron"), "{out}");
    assert_eq!(code, 0);
}

#[test]
fn sin_pruebas_lo_indica() {
    let (out, code) = run_tests("fn main() -> int { 0 }\n", "ray_test_none.ray");
    assert!(out.contains("no hay pruebas"), "{out}");
    assert_eq!(code, 0);
}

#[test]
fn pruebas_unit_con_assert() {
    // M13.2b: una @test puede devolver unit; pasa si no dispara assert/panic.
    let src = r#"
@test fn assert_ok() { assert_eq(2 + 2, 4); assert(true); }
@test fn assert_falla() { assert_eq(2 + 2, 5); }
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests(src, "ray_test_unit.ray");
    assert!(out.contains("ok    assert_ok"), "la unit que pasa\n{out}");
    assert!(out.contains("FALLO assert_falla"), "la unit que falla\n{out}");
    assert!(out.contains("assert_eq falló: 4 != 5"), "muestra el mensaje del assert\n{out}");
    assert_eq!(code, 1);
}

#[test]
fn panic_no_aborta_la_bateria() {
    // M13.2b: cada prueba corre aislada; un panic en una no impide ejecutar las demás.
    let src = r#"
@test fn primera() { panic("boom"); }
@test fn segunda() -> bool { true }
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests(src, "ray_test_panic.ray");
    assert!(out.contains("FALLO primera"), "{out}");
    assert!(out.contains("boom"), "muestra el mensaje del panic\n{out}");
    assert!(out.contains("ok    segunda"), "la segunda corre pese al panic de la primera\n{out}");
    assert_eq!(code, 1);
}

#[test]
fn filtro_por_nombre() {
    // M13.2b: un argumento tras la ruta selecciona por subcadena del nombre.
    let src = r#"
@test fn suma_ok() -> bool { 1 + 1 == 2 }
@test fn resta_ok() -> bool { 3 - 1 == 2 }
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests_filtro(src, "ray_test_filtro.ray", Some("suma"));
    assert!(out.contains("ok    suma_ok"), "corre la que casa\n{out}");
    assert!(!out.contains("resta_ok"), "no corre la que no casa\n{out}");
    assert_eq!(code, 0);
}
