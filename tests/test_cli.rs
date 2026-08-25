//! Pruebas del runner `@test` (M10.1; a nivel proyecto en M101) sobre el binario: ejecutan
//! `raylang --test <archivo>` / `ray test` y comprueban el informe y el código de salida
//! (0 = verde, 1 = fallos, 65 = no compila).

use std::io::Write;
use std::path::Path;
use std::process::Command;

/// Escribe `src` a un archivo temporal, ejecuta `raylang --test <archivo>` y devuelve
/// `(stdout, código de salida)`.
fn run_tests(src: &str, name: &str) -> (String, i32) {
    run_tests_with_filter(src, name, None)
}

/// Como `run_tests`, pero pasa un filtro de nombre tras la ruta (M13.2b).
fn run_tests_with_filter(src: &str, name: &str, filter: Option<&str>) -> (String, i32) {
    let mut path = std::env::temp_dir();
    path.push(name);
    let mut f = std::fs::File::create(&path).expect("crea el file temporal");
    f.write_all(src.as_bytes()).expect("escribe la source");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.arg("--test").arg(&path);
    if let Some(p) = filter {
        cmd.arg(p);
    }
    let out = cmd.output().expect("ejecuta el binary");
    let code = out.status.code().unwrap_or(-1);
    (String::from_utf8_lossy(&out.stdout).into_owned(), code)
}

/// Crea un árbol de proyecto bajo un directorio temporal único y ejecuta `ray test` con ese
/// directorio como cwd. `files` = (ruta relativa, contenido). Devuelve `(stdout+stderr, código)`.
fn run_project(dir_name: &str, files: &[(&str, &str)], args: &[&str]) -> (String, i32) {
    let mut root = std::env::temp_dir();
    root.push(dir_name);
    let _ = std::fs::remove_dir_all(&root);
    for (rel, content) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("crea los directorios");
        std::fs::write(&path, content).expect("escribe el file");
    }
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("test")
        .args(args)
        .current_dir(&root)
        .output()
        .expect("ejecuta el binary");
    let code = out.status.code().unwrap_or(-1);
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (text, code)
}

#[test]
fn runs_tests_and_reports_failures() {
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
    assert!(out.contains("FAIL  fails"), "informa la what fails\n{out}");
    assert_eq!(code, 1, "hubo fallos → exit 1 (M101)");
}

#[test]
fn todas_pasan_code_cero() {
    let src = "@test fn a() -> bool { true }\n@test fn b() -> bool { !false }\nfn main() -> int { 0 }\n";
    let (out, code) = run_tests(src, "ray_test_ok.ray");
    assert!(out.contains("all passed"), "{out}");
    assert_eq!(code, 0);
}

#[test]
fn no_tests_reports_it() {
    let (out, code) = run_tests("fn main() -> int { 0 }\n", "ray_test_none.ray");
    assert!(out.contains("no tests"), "{out}");
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
    assert!(out.contains("FAIL  assert_fails"), "la unit what fails\n{out}");
    assert!(out.contains("assert_eq failed: 4 != 5"), "shows el mensaje del assert\n{out}");
    assert_eq!(code, 1);
}

#[test]
fn failure_reports_location() {
    // M101: un fallo reporta `at módulo:línea:col` apuntando al assert del USUARIO.
    let src = "@test fn boom() {\n    assert_eq(1, 2);\n}\nfn main() -> int { 0 }\n";
    let (out, code) = run_tests(src, "ray_test_loc.ray");
    assert!(out.contains("at ray_test_loc:2:5"), "ubica el assert fallido\n{out}");
    assert_eq!(code, 1);
}

#[test]
fn panic_does_not_abort_the_suite() {
    // M13.2b: cada prueba corre aislada; un panic en una no impide ejecutar las demás.
    let src = r#"
@test fn first() { panic("boom"); }
@test fn second() -> bool { true }
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests(src, "ray_test_panic.ray");
    assert!(out.contains("FAIL  first"), "{out}");
    assert!(out.contains("boom"), "shows el mensaje del panic\n{out}");
    assert!(out.contains("ok    second"), "la second runs pese al panic de la first\n{out}");
    assert_eq!(code, 1);
}

#[test]
fn filter_by_name() {
    // M13.2b: un argumento tras la ruta selecciona por subcadena del nombre.
    let src = r#"
@test fn sum_ok() -> bool { 1 + 1 == 2 }
@test fn resta_ok() -> bool { 3 - 1 == 2 }
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests_with_filter(src, "ray_test_filtro.ray", Some("sum"));
    assert!(out.contains("ok    sum_ok"), "runs la what casa\n{out}");
    assert!(!out.contains("resta_ok"), "no runs la what no casa\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn many_failures_exit_one() {
    // M101: el código de salida es 1 con CUALQUIER número de fallos (antes era el número de
    // fallos & 0xFF: 256 fallos habrían dado exit 0, un falso verde en CI).
    let mut src = String::new();
    for i in 0..9 {
        src.push_str(&format!("@test fn f{i}() -> bool {{ false }}\n"));
    }
    src.push_str("fn main() -> int { 0 }\n");
    let (out, code) = run_tests(&src, "ray_test_many.ray");
    assert!(out.contains("9 of 9 test(s) failed"), "{out}");
    assert_eq!(code, 1, "fallos → exit 1, no el conteo");
}

#[test]
fn tests_resolve_imports() {
    // M101: el runner pasa por el loader — un archivo de tests puede importar módulos, y las
    // @test inline de los módulos importados corren con su nombre calificado.
    let files = [
        ("ray.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n"),
        (
            "src/math.ray",
            "pub fn double(x: int) -> int { x * 2 }\n\n@test\nfn double_inline_ok() -> bool { double(2) == 4 }\n",
        ),
        (
            "src/main.ray",
            "import math;\n\nfn main() { print(math.double(21)); }\n\n@test\nfn entry_ok() { assert_eq(math.double(3), 6); }\n",
        ),
    ];
    let (out, code) = run_project("ray_test_proj_imports", &files, &[]);
    assert!(out.contains("ok    entry_ok"), "la @test de la entry runs\n{out}");
    assert!(out.contains("ok    math.double_inline_ok"), "la @test del module runs calificada\n{out}");
    assert_eq!(code, 0, "{out}");
}

#[test]
fn discovers_tests_directory() {
    // M101: sin archivo explícito, `ray test` corre la entrada del proyecto y cada `tests/*.ray`
    // como suite de integración (que importa los módulos del proyecto contra src/).
    let files = [
        ("ray.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n"),
        ("src/math.ray", "pub fn double(x: int) -> int { x * 2 }\n"),
        ("src/main.ray", "import math;\n\nfn main() { print(math.double(21)); }\n\n@test\nfn entry_ok() -> bool { true }\n"),
        (
            "tests/integration.ray",
            "import math;\n\n@test\nfn integration_ok() { assert_eq(math.double(10), 20); }\n\n@test\nfn integration_fails() {\n    assert_eq(math.double(3), 7);\n}\n",
        ),
    ];
    let (out, code) = run_project("ray_test_proj_discovery", &files, &[]);
    assert!(out.contains("tests/integration.ray"), "header de la suite (hay dos suites con tests)\n{out}");
    assert!(out.contains("ok    integration_ok"), "{out}");
    assert!(out.contains("FAIL  integration_fails"), "{out}");
    assert!(out.contains("at integration:8:5"), "ubica el fallo en el file de tests\n{out}");
    assert_eq!(code, 1, "{out}");
}

#[test]
fn integration_suite_does_not_duplicate_module_tests() {
    // M101: las @test inline de un módulo importado corren en la suite del PROYECTO; una suite de
    // tests/ que importe ese módulo no las repite.
    let files = [
        ("ray.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n"),
        ("src/math.ray", "pub fn double(x: int) -> int { x * 2 }\n\n@test\nfn inline_ok() -> bool { double(1) == 2 }\n"),
        ("src/main.ray", "import math;\n\nfn main() { print(math.double(21)); }\n"),
        ("tests/uses_math.ray", "import math;\n\n@test\nfn own_ok() -> bool { math.double(4) == 8 }\n"),
    ];
    let (out, code) = run_project("ray_test_proj_dedup", &files, &[]);
    assert_eq!(out.matches("math.inline_ok").count(), 1, "la inline corre UNA vez\n{out}");
    assert!(out.contains("ok    own_ok"), "{out}");
    assert!(out.contains("result: 2 test(s), all passed"), "{out}");
    assert_eq!(code, 0, "{out}");
}

#[test]
fn broken_suite_exits_65_but_others_run() {
    // M101: una suite que no compila reporta su diagnóstico y no frena a las demás; el código de
    // salida global es 65.
    let files = [
        ("ray.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n"),
        ("src/main.ray", "fn main() -> int { 0 }\n\n@test\nfn entry_ok() -> bool { true }\n"),
        ("tests/broken.ray", "@test\nfn nope() -> bool { missing_fn() }\n"),
    ];
    let (out, code) = run_project("ray_test_proj_broken", &files, &[]);
    assert!(out.contains("ok    entry_ok"), "la suite sana runs\n{out}");
    assert!(out.contains("missing_fn"), "el diagnóstico de la rota se shows\n{out}");
    assert_eq!(code, 65, "{out}");
}

#[test]
fn first_arg_without_extension_is_filter() {
    // M101: `ray test <filtro>` (sin archivo) filtra sobre el proyecto del cwd.
    let files = [
        ("ray.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n"),
        ("src/main.ray", "fn main() -> int { 0 }\n\n@test\nfn alpha_ok() -> bool { true }\n\n@test\nfn beta_ok() -> bool { true }\n"),
    ];
    let (out, code) = run_project("ray_test_proj_filter", &files, &["alpha"]);
    assert!(out.contains("ok    alpha_ok"), "{out}");
    assert!(!out.contains("beta_ok"), "el filtro excluye a beta\n{out}");
    assert_eq!(code, 0, "{out}");
}

#[test]
fn missing_file_exits_66() {
    // La interfaz legada conserva 66 (EX_NOINPUT) para un archivo ilegible.
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--test")
        .arg(Path::new("no_such_file_ever.ray"))
        .output()
        .expect("ejecuta el binary");
    assert_eq!(out.status.code(), Some(66));
}

#[test]
fn listeners_do_not_survive_between_tests() {
    // M129 (§65.2): el runner descartaba las fibras del @test anterior pero los sockets de
    // ESCUCHA del SO sobrevivían — el siguiente test los veía aceptar conexiones que nadie
    // atendía (read timeout en vez de connection refused). Ahora el aislamiento drena TODO el
    // registro de handles: el segundo test debe ver el puerto CERRADO.
    let src = r#"
import std/net;
@test fn boots_listener() -> bool {
    match (net.tcp_listen("127.0.0.1", 36179)) {
        Result.Ok(l) => true,
        Result.Err(e) => false,
    }
}
@test fn listener_is_gone() -> bool {
    match (net.tcp_connect_timeout("127.0.0.1", 36179, 500)) {
        Result.Ok(c) => {
            close(c);
            false
        },
        Result.Err(e) => true,
    }
}
fn main() -> int { 0 }
"#;
    let (out, code) = run_tests(src, "ray_test_zombie_listener.ray");
    assert!(out.contains("ok    boots_listener"), "el listener del primer test arranca\n{out}");
    assert!(out.contains("ok    listener_is_gone"), "el puerto debe estar CERRADO en el segundo test\n{out}");
    assert_eq!(code, 0, "ambos verdes\n{out}");
}
