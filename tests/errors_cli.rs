//! Pruebas de los diagnósticos con contexto (M8.3) sobre el binario: ejecutan un
//! archivo `.ray` con un error y comprueban que el stderr incluye la línea de fuente y
//! el cursor `^`.

use std::io::Write;
use std::process::Command;

/// Escribe `src` a un archivo temporal, ejecuta `raylang <archivo>` y devuelve su stderr.
fn run_file_stderr(src: &str, name: &str) -> String {
    let mut path = std::env::temp_dir();
    path.push(name);
    let mut f = std::fs::File::create(&path).expect("crea el archivo temporal");
    f.write_all(src.as_bytes()).expect("escribe la fuente");
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(&path)
        .output()
        .expect("ejecuta el binario");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn error_de_tipos_muestra_linea_y_cursor() {
    let err = run_file_stderr("fn main() -> int {\n    let x = 1 + true;\n    x\n}\n", "ray_err_tipo.ray");
    assert!(err.contains("error de tipos en 2:"), "cabecera con ubicación\n{err}");
    assert!(err.contains("2 |     let x = 1 + true;"), "muestra la línea de fuente\n{err}");
    assert!(err.contains("|             ^"), "dibuja el cursor alineado\n{err}");
}

#[test]
fn error_de_ejecucion_muestra_contexto() {
    let err = run_file_stderr("fn main() -> int {\n    let d = 0;\n    10 / d\n}\n", "ray_err_run.ray");
    assert!(err.contains("error en ejecución en 3:"), "{err}");
    assert!(err.contains("3 |     10 / d"), "{err}");
    assert!(err.contains("^"), "{err}");
}

#[test]
fn error_de_sintaxis_subraya_el_token_completo() {
    // M33a: el token ofensor se subraya entero (^^^^), no solo su primer carácter.
    let err = run_file_stderr("fn main() -> int {\n    let x = enum;\n    x\n}\n", "ray_err_span.ray");
    assert!(err.contains("error de sintaxis en 2:13"), "cabecera con ubicación\n{err}");
    assert!(err.contains("2 |     let x = enum;"), "muestra la línea de fuente\n{err}");
    assert!(err.contains("|             ^^^^"), "subraya los 4 chars de 'enum'\n{err}");
    assert!(!err.contains("^^^^^"), "y no más de 4\n{err}");
}

#[test]
fn error_de_tipos_subraya_la_expresion_completa() {
    // M33a-2: el checker subraya la expresión entera, no solo su inicio.
    let err = run_file_stderr("fn main() -> int {\n    let x = 1 + true;\n    x\n}\n", "ray_err_span_expr.ray");
    assert!(err.contains("error de tipos en 2:13"), "cabecera\n{err}");
    assert!(err.contains("|             ^^^^^^^^"), "subraya '1 + true' (8 chars)\n{err}");
    assert!(!err.contains("^^^^^^^^^"), "y no más de 8\n{err}");
}

#[test]
fn el_cli_muestra_todos_los_errores_de_tipos() {
    // M33c: dos cuerpos con error → los dos diagnósticos renderizados, exit 65.
    let err = run_file_stderr(
        "fn f() -> int { 1 + true }\nfn g() -> int { \"x\" * 2 }\nfn main() -> int { f() + g() }\n",
        "ray_err_multi.ray",
    );
    assert!(err.contains("error de tipos en 1:17"), "primer error\n{err}");
    assert!(err.contains("error de tipos en 2:17"), "segundo error\n{err}");
    assert!(err.contains("int y bool") && err.contains("string y int"), "{err}");
}

#[test]
fn error_de_ejecucion_muestra_la_traza_de_llamadas() {
    // M79: la traza de llamadas — `en <fn>` el marco interno, `desde <fn>` los
    // llamadores, cada uno con su posición local. El assert del prelude se etiqueta
    // `prelude` (fuera de banda) y el sitio del USUARIO aparece con su línea real.
    let err = run_file_stderr(
        "fn helper(x: int) -> int {\n    assert(x > 0);\n    x\n}\nfn main() -> int {\n    helper(0 - 1) + 0\n}\n",
        "ray_err_trace.ray",
    );
    assert!(err.contains("aserción falló"), "cabecera\n{err}");
    assert!(err.contains("en assert (prelude:"), "marco del prelude etiquetado\n{err}");
    assert!(err.contains("desde helper (ray_err_trace:2:5)"), "sitio del assert en helper\n{err}");
    assert!(err.contains("desde main (ray_err_trace:6:5)"), "sitio de la llamada en main\n{err}");
}

#[test]
fn error_directo_en_main_no_imprime_traza() {
    // M79: con un solo marco la traza no aporta (la cabecera ya lo dice).
    let err = run_file_stderr("fn main() -> int {\n    let d = 0;\n    10 / d\n}\n", "ray_err_sin_traza.ray");
    assert!(err.contains("error en ejecución en 3:"), "{err}");
    assert!(!err.contains("desde "), "sin traza con un solo marco\n{err}");
}
