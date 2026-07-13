//! Pruebas de los diagnósticos con contexto (M8.3) sobre el binario: ejecutan un
//! archivo `.ray` con un error y comprueban que el stderr incluye la línea de fuente y
//! el cursor `^`.

use std::io::Write;
use std::process::Command;

/// Escribe `src` a un archivo temporal, ejecuta `raylang <archivo>` y devuelve su stderr.
fn run_file_stderr(src: &str, name: &str) -> String {
    let mut path = std::env::temp_dir();
    path.push(name);
    let mut f = std::fs::File::create(&path).expect("crea el file temporal");
    f.write_all(src.as_bytes()).expect("escribe la source");
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(&path)
        .output()
        .expect("ejecuta el binary");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn error_de_types_shows_linea_y_cursor() {
    let err = run_file_stderr("fn main() -> int {\n    let x = 1 + true;\n    x\n}\n", "ray_err_ty.ray");
    assert!(err.contains("error de types en 2:"), "header con ubicación\n{err}");
    assert!(err.contains("2 |     let x = 1 + true;"), "shows la línea de source\n{err}");
    assert!(err.contains("|             ^"), "dibuja el cursor alineado\n{err}");
}

#[test]
fn error_de_execution_shows_context() {
    let err = run_file_stderr("fn main() -> int {\n    let d = 0;\n    10 / d\n}\n", "ray_err_run.ray");
    assert!(err.contains("error en ejecución en 3:"), "{err}");
    assert!(err.contains("3 |     10 / d"), "{err}");
    assert!(err.contains("^"), "{err}");
}

#[test]
fn error_de_syntax_underscores_el_token_complete() {
    // M33a: el token ofensor se subraya entero (^^^^), no solo su primer carácter.
    let err = run_file_stderr("fn main() -> int {\n    let x = enum;\n    x\n}\n", "ray_err_span.ray");
    assert!(err.contains("error de syntax en 2:13"), "header con ubicación\n{err}");
    assert!(err.contains("2 |     let x = enum;"), "shows la línea de source\n{err}");
    assert!(err.contains("|             ^^^^"), "underscores los 4 chars de 'enum'\n{err}");
    assert!(!err.contains("^^^^^"), "y no más de 4\n{err}");
}

#[test]
fn error_de_types_underscores_la_expression_complete() {
    // M33a-2: el checker subraya la expresión entera, no solo su inicio.
    let err = run_file_stderr("fn main() -> int {\n    let x = 1 + true;\n    x\n}\n", "ray_err_span_expr.ray");
    assert!(err.contains("error de types en 2:13"), "header\n{err}");
    assert!(err.contains("|             ^^^^^^^^"), "underscores '1 + true' (8 chars)\n{err}");
    assert!(!err.contains("^^^^^^^^^"), "y no más de 8\n{err}");
}

#[test]
fn el_cli_shows_all_los_errors_de_types() {
    // M33c: dos cuerpos con error → los dos diagnósticos renderizados, exit 65.
    let err = run_file_stderr(
        "fn f() -> int { 1 + true }\nfn g() -> int { \"x\" * 2 }\nfn main() -> int { f() + g() }\n",
        "ray_err_multi.ray",
    );
    assert!(err.contains("error de types en 1:17"), "primer error\n{err}");
    assert!(err.contains("error de types en 2:17"), "segundo error\n{err}");
    assert!(err.contains("int y bool") && err.contains("string y int"), "{err}");
}

#[test]
fn error_de_execution_shows_la_traza_de_calls() {
    // M79: la traza de llamadas — `en <fn>` el marco interno, `desde <fn>` los
    // llamadores, cada uno con su posición local. El assert del prelude se etiqueta
    // `prelude` (fuera de banda) y el sitio del USUARIO aparece con su línea real.
    let err = run_file_stderr(
        "fn helper(x: int) -> int {\n    assert(x > 0);\n    x\n}\nfn main() -> int {\n    helper(0 - 1) + 0\n}\n",
        "ray_err_trace.ray",
    );
    // M79c: la cabecera y el `^` se reposicionan al primer marco de USUARIO (el
    // `assert(x > 0)` de helper), no al `panic` del prelude.
    assert!(err.contains("error en ejecución en 2:5: aserción falló"), "header repositionada\n{err}");
    assert!(err.contains("2 |     assert(x > 0);"), "shows la línea del user\n{err}");
    assert!(err.contains("en assert (prelude:"), "marco del prelude etiquetado\n{err}");
    assert!(err.contains("from helper (ray_err_trace:2:5)"), "sitio del assert en helper\n{err}");
    assert!(err.contains("from main (ray_err_trace:6:5)"), "sitio de la llamada en main\n{err}");
}

#[test]
fn error_direct_en_main_no_imprime_traza() {
    // M79: con un solo marco la traza no aporta (la cabecera ya lo dice).
    let err = run_file_stderr("fn main() -> int {\n    let d = 0;\n    10 / d\n}\n", "ray_err_sin_traza.ray");
    assert!(err.contains("error en ejecución en 3:"), "{err}");
    assert!(!err.contains("from "), "sin traza con un solo marco\n{err}");
}

#[test]
fn error_en_la_std_reposiciona_la_header_al_llamador() {
    // M79c: un trap dentro de `std/math` (factorial(25) desborda el int) apunta la
    // cabecera al SITIO del usuario; el marco real de la std queda en la traza.
    let err = run_file_stderr(
        "import std/math;\n\nfn main() -> int {\n    let x = math.factorial(25);\n    x\n}\n",
        "ray_err_std_trace.ray",
    );
    assert!(err.contains("error en ejecución en 4:13"), "header en el llamador\n{err}");
    assert!(err.contains("4 |     let x = math.factorial(25);"), "línea del user\n{err}");
    assert!(err.contains("en std::math::factorial (std/math:"), "marco real en la traza\n{err}");
    assert!(err.contains("from main (ray_err_std_trace:4:13)"), "llamador en la traza\n{err}");
}
