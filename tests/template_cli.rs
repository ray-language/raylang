//! Prueba del motor de plantillas HTML (`examples/stdlib/template.ray`, M32.3; optimizado jul 2026).
//! El demo renderiza una plantilla con interpolación (autoescape), `{% if %}`/`{% elif %}`/`{% else %}`,
//! bucle `{% for %}` sobre una lista heterogénea, interpolación cruda `{{& … }}`, y la API de dos
//! niveles (compile una vez + render con dos contextos — el patrón de SSR). El test exige la salida
//! esperada (HTML escapado donde corresponde) y que ambos motores coincidan.

use std::process::Command;

const EXPECTED: &[&str] = &[
    "<h1>Hola, &lt;b&gt;Ada&lt;/b&gt;!</h1>",
    "<p>admin</p>",
    "<ul><li>a &amp; b</li><li>c</li><li>42</li></ul>",
    "raw: <i>raw</i>",
    // La plantilla COMPILADA, renderizada con dos contextos (elif y else).
    "<h1>Hola, Eva!</h1>",
    "<p>invitado</p>",
    "<ul><li>1</li></ul>",
    "raw: ",
    "<h1>Hola, Leo!</h1>",
    "<p>user</p>",
    "<ul><li>2</li></ul>",
    "raw: ",
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/stdlib/template_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta template_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

#[test]
fn template_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "template_demo falló en el intérprete");
    assert_eq!(lines, EXPECTED);
}

#[test]
fn template_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "template_demo falló en la VM");
    assert_eq!(lines, EXPECTED);
}
