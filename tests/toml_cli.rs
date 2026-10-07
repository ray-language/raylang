//! Prueba del parser TOML (`examples/stdlib/toml.ray`, M32.2b — subconjunto). El demo parsea un
//! documento con comentarios, tablas y valores string/int/float/bool/array, y muestra cada valor por su
//! ruta con puntos. El test exige la salida esperada y que ambos motores coincidan.

use std::process::Command;

const EXPECTED: &[&str] = &[
    "title = \"raylang\"",
    "server.host = \"localhost\"",
    "server.port = 8080",
    "server.debug = true",
    "server.ratio = 1.5",
    "server.tags = [\"a\", \"b\", \"c\"]",
    "server.ports = [80, 443, 8080]",
    // M63.1 — strings conformes: \uXXXX/\UXXXXXXXX (incl. astral), literal '...' sin escapes,
    // y escape desconocido/incompleto = Err (antes: corrupción silenciosa, "cafu00E9").
    "s = \"café 😀\"",
    "p = \"C:\\ruta\\ne\"",
    "err: unknown escape '\\q' in the string (line 1)",
    "err: non-hex digit in \\u escape (line 1)",
    // M63.2 — números conformes: separadores `_` (entre dígitos) e inf/nan.
    "n = 1000000",
    "f = 1024.5",
    "a = inf",
    "b = -inf",
    "c = NaN",
    "err: misplaced '_' separator in number: 1__0 (line 1)",
    // M63.3 — rigor del documento: lo que la spec prohíbe ya no pasa en silencio.
    "err: duplicate key: 'a' (line 2)",
    "err: empty table header (line 1)",
    "err: expected end of line after the value of 'a' (line 1)",
    // M128 — arreglos de tablas [[ruta]] aplanados como ruta.N.clave + toml_array_len.
    "route.0.path = \"/a\"",
    "route.0.port = 1",
    "route.1.path = \"/b\"",
    "server.host = \"x\"",
    "route len = 2",
    "nope len = 0",
    "err: empty array-of-tables header (line 1)",
    "err: missing ']]' in array-of-tables header (line 1)",
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/stdlib/toml_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta toml_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

#[test]
fn toml_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "toml_demo falló en el intérprete");
    assert_eq!(lines, EXPECTED);
}

#[test]
fn toml_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "toml_demo falló en la VM");
    assert_eq!(lines, EXPECTED);
}
