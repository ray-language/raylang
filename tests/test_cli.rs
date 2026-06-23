//! Pruebas del runner `@test` (M10.1) sobre el binario: ejecutan `raylang --test
//! <archivo>` y comprueban el informe y el código de salida (número de fallos).

use std::io::Write;
use std::process::Command;

/// Escribe `src` a un archivo temporal, ejecuta `raylang --test <archivo>` y devuelve
/// `(stdout, código de salida)`.
fn run_tests(src: &str, name: &str) -> (String, i32) {
    let mut path = std::env::temp_dir();
    path.push(name);
    let mut f = std::fs::File::create(&path).expect("crea el archivo temporal");
    f.write_all(src.as_bytes()).expect("escribe la fuente");
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--test")
        .arg(&path)
        .output()
        .expect("ejecuta el binario");
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
