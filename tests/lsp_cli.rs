//! Pruebas del Language Server (M10.2) sobre el binario: lanzan `raylang --lsp`, le envían
//! marcos LSP por stdin y comprueban las respuestas por stdout. Es la verificación de que el
//! servidor habla el protocolo de verdad, no solo en memoria (ver los tests unitarios en
//! `src/lsp.rs`).

use std::io::Write;
use std::process::{Command, Stdio};

/// Enmarca un cuerpo JSON con su cabecera `Content-Length`, como hace un cliente LSP.
fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

/// Lanza `raylang --lsp`, le envía `entrada` por stdin y devuelve todo su stdout.
fn lsp(entry: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("lanza el servidor LSP");
    child.stdin
        .take()
        .unwrap()
        .write_all(entry.as_bytes())
        .expect("escribe en stdin del servidor");
    let output = child.wait_with_output().expect("espera al servidor");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn responde_initialize_con_capacidades() {
    let entry = frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
        + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"id\":1"), "echo del id\n{out}");
    assert!(out.contains("\"capabilities\""), "anuncia capacidades\n{out}");
    assert!(out.contains("\"textDocumentSync\":1"), "Full sync\n{out}");
    assert!(out.contains("\"hoverProvider\":true"), "anuncia hover (M10.2b)\n{out}");
    assert!(out.contains("\"referencesProvider\":true"), "anuncia find-references\n{out}");
    assert!(out.contains("\"renameProvider\":true"), "anuncia rename\n{out}");
    assert!(out.contains("\"signatureHelpProvider\""), "anuncia signature help (M10.2f)\n{out}");
}

#[test]
fn hover_shows_el_ty_de_one_variable() {
    // didOpen un programa y pide hover sobre el uso de `x` (línea 2, carácter 2, 0-basado).
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn main() -> int {\n  let x = 5;\n  x\n}"}}}"#;
    let hover = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":2,"character":2}}}"#;
    let entry = frame(open) + &frame(hover) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"id\":2"), "responde a la petición de hover\n{out}");
    assert!(out.contains("x: int"), "shows el type de x\n{out}");
}

#[test]
fn definicion_salta_a_la_declaracion() {
    // Ir-a-definición del uso de `x` (línea 2) → su `let` (línea 1).
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn main() -> int {\n  let x = 5;\n  x\n}"}}}"#;
    let def = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":2,"character":2}}}"#;
    let entry = frame(open) + &frame(def) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"id\":3"), "responde a la petición de definición\n{out}");
    assert!(out.contains("file:///t.ray"), "returns one Location\n{out}");
    // La declaración está en la línea 1 (0-basado): el `let`.
    assert!(out.contains("\"line\":1"), "apunta a la línea del let\n{out}");
}

#[test]
fn references_list_los_usos() {
    // find-references del uso de `x` (línea 2) → declaración + los dos usos en `x + x`.
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn main() -> int {\n  let x = 5;\n  x + x\n}"}}}"#;
    let refs = r#"{"jsonrpc":"2.0","id":4,"method":"textDocument/references","params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":2,"character":2},"context":{"includeDeclaration":true}}}"#;
    let entry = frame(open) + &frame(refs) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"id\":4"), "responde a la petición de references\n{out}");
    // Tres Locations: la declaración (línea 1) y los dos usos (línea 2).
    assert!(out.contains("\"line\":1"), "incluye la declaración\n{out}");
    assert!(out.contains("\"line\":2"), "incluye los usos\n{out}");
}

#[test]
fn rename_returns_un_workspace_edit() {
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn main() -> int {\n  let x = 5;\n  x + x\n}"}}}"#;
    let rn = r#"{"jsonrpc":"2.0","id":5,"method":"textDocument/rename","params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":2,"character":2},"newName":"y"}}"#;
    let entry = frame(open) + &frame(rn) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"id\":5"), "responde al rename\n{out}");
    assert!(out.contains("\"changes\""), "returns un WorkspaceEdit\n{out}");
    assert!(out.contains("\"newText\":\"y\""), "renombra a y\n{out}");
}

#[test]
fn completion_offers_symbols() {
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn doble(n: int) -> int { n + n }\nfn main() -> int { 0 }"}}}"#;
    let comp = r#"{"jsonrpc":"2.0","id":6,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":1,"character":19}}}"#;
    let entry = frame(open) + &frame(comp) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"id\":6"), "responde a la petición de completion\n{out}");
    assert!(out.contains("\"label\":\"double\""), "offers la función propia\n{out}");
    assert!(out.contains("\"label\":\"print\""), "offers builtins\n{out}");
}

#[test]
fn hover_de_miembro_de_modulo_incluye_doc() {
    // M49.1: el hover de `math.sqrt` muestra la firma + el `///` de std/math (módulo embebido, sin
    // archivo en disco → su fuente se toma del programa cargado).
    let dir = std::env::temp_dir().join("ray_lsp_hoverdoc");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("m.ray");
    std::fs::write(&file, "import std/math;\nfn main() -> int {\n    print(math.sqrt(16.0));\n    0\n}").unwrap();
    let uri = format!("file://{}", file.display());
    let open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","text":"import std/math;\nfn main() -> int {{\n    print(math.sqrt(16.0));\n    0\n}}"}}}}}}"#
    );
    // hover sobre `sqrt` (línea 2, char 16).
    let hov = format!(
        r#"{{"jsonrpc":"2.0","id":10,"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":2,"character":16}}}}}}"#
    );
    let entry = frame(&open) + &frame(&hov) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("math.sqrt: fn(float) -> float"), "signature qualified\n{out}");
    assert!(out.contains("Square root"), "incluye el /// de std/math\n{out}");
}

#[test]
fn completion_de_members_de_modulo() {
    // M49.1: tras `math.` (módulo importado) el LSP ofrece los ítems pub del módulo: funciones, consts.
    // Usa un archivo real (la resolución del `import` necesita un path de proyecto válido, no `/t.ray`).
    let dir = std::env::temp_dir().join("ray_lsp_modcomp");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("m.ray");
    std::fs::write(&file, "import std/math;\nfn main() -> int {\n    math.\n    0\n}").unwrap();
    let uri = format!("file://{}", file.display());
    let open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","text":"import std/math;\nfn main() -> int {{\n    math.\n    0\n}}"}}}}}}"#
    );
    let comp = format!(
        r#"{{"jsonrpc":"2.0","id":9,"method":"textDocument/completion","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":2,"character":9}}}}}}"#
    );
    let entry = frame(&open) + &frame(&comp) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"id\":9"), "responde a la completion\n{out}");
    assert!(out.contains("\"label\":\"sqrt\""), "offers la función sqrt de std/math\n{out}");
    assert!(out.contains("\"label\":\"PI\""), "offers la constante PI\n{out}");
    assert!(!out.contains("\"label\":\"print\""), "NO offers builtins globales after `math.`\n{out}");
}

#[test]
fn public_diagnostic_ante_un_error() {
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn main() -> int { 1 + true }"}}}"#;
    let entry = frame(open) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(
        out.contains("textDocument/publishDiagnostics"),
        "public diagnósticos\n{out}"
    );
    assert!(out.contains("\"severity\":1"), "severidad Error\n{out}");
    assert!(out.contains("\"source\":\"raylang\""), "la source es raylang\n{out}");
}

#[test]
fn public_diagnostic_al_redefine_un_builtin() {
    // M48.3: redefinir un builtin del núcleo (`fn print`) → diagnóstico en vivo. (M48.4e: los builtins de
    // contenedor como `len` se retiraron y ya NO disparan el footgun; `print` sigue siendo builtin.)
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn print(x: int) -> int { x }\nfn main() -> int { 0 }"}}}"#;
    let entry = frame(open) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("textDocument/publishDiagnostics"), "public diagnósticos\n{out}");
    assert!(out.contains("es un builtin del lenguaje"), "mensaje del footgun\n{out}");
}

#[test]
fn program_valid_public_list_empty() {
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///ok.ray","text":"fn main() -> int { 42 }"}}}"#;
    let entry = frame(open) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"diagnostics\":[]"), "sin errors, list vacía\n{out}");
}

#[test]
fn signature_help_shows_la_signature() {
    // M10.2f: al escribir los argumentos de `suma(`, signatureHelp da su firma y el param activo.
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn suma(a: int, b: int) -> int { a + b }\nfn main() -> int { suma(1, ) }"}}}"#;
    // Cursor tras la coma (línea 1, carácter 26, 0-basado): segundo argumento.
    let sh = r#"{"jsonrpc":"2.0","id":7,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":1,"character":26}}}"#;
    let entry = frame(open) + &frame(sh) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"id\":7"), "responde a signatureHelp\n{out}");
    assert!(out.contains("fn sum(a: int, b: int) -> int"), "shows la signature\n{out}");
    assert!(out.contains("\"activeParameter\":1"), "param activo = 1 (segundo)\n{out}");
}

#[test]
fn completion_incluye_locals_por_ambito() {
    // M10.2f: la completion incluye los params y locales de la función bajo el cursor.
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn calc(factor: int) -> int {\n  let total = factor + 1;\n  \n}"}}}"#;
    let comp = r#"{"jsonrpc":"2.0","id":8,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":2,"character":2}}}"#;
    let entry = frame(open) + &frame(comp) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"id\":8"), "responde a completion\n{out}");
    assert!(out.contains("\"label\":\"factor\""), "offers el parámetro factor\n{out}");
    assert!(out.contains("\"label\":\"total\""), "offers la local total\n{out}");
}

#[test]
fn hover_de_ty_en_literal_de_struct() {
    // M10.2f: hover sobre el nombre de tipo en un literal de struct.
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"struct Punto { x: int }\nfn main() -> int {\n  let p = Punto { x: 1 };\n  p.x\n}"}}}"#;
    // `Punto` en la línea 2 (0-basado), carácter 10.
    let hov = r#"{"jsonrpc":"2.0","id":9,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":2,"character":10}}}"#;
    let entry = frame(open) + &frame(hov) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("\"id\":9"), "responde a hover\n{out}");
    assert!(out.contains("struct Punto"), "shows el type del struct\n{out}");
}

#[test]
fn diagnostica_con_las_dependencies_por_path_del_manifest() {
    // Regresión: un proyecto con una path-dep en su ray.toml (como examples/db) compilaba con
    // `ray run` pero el LSP marcaba "no se encuentra el módulo" — dep_roots_for solo miraba
    // `.ray-deps/`, no las dependencias por ruta. Ahora el LSP resuelve con las MISMAS raíces
    // que el CLI (deps::dependency_roots_for).
    let base = std::env::temp_dir().join("ray_lsp_pathdep");
    let _ = std::fs::remove_dir_all(&base);
    // El paquete: base/paquetes/util/mod.ray con una función pub.
    std::fs::create_dir_all(base.join("packages/util")).unwrap();
    std::fs::write(base.join("packages/util/mod.ray"), "pub fn double(n: int) -> int { n * 2 }\n").unwrap();
    // El proyecto: base/app con ray.toml (path-dep) y src/main.ray que la importa.
    std::fs::create_dir_all(base.join("app/src")).unwrap();
    std::fs::write(
        base.join("app/ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nutil = \"path:../packages/util\"\n",
    )
    .unwrap();
    let main_path = base.join("app/src/main.ray");
    std::fs::write(&main_path, "").unwrap(); // el contenido viaja por didOpen
    let text = r#"import util;\n\nfn main() -> int {\n    util.double(21)\n}"#;
    let open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"file://{}","text":"{}"}}}}}}"#,
        main_path.display(),
        text
    );
    let entry = frame(&open) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("publishDiagnostics"), "public diagnósticos\n{out}");
    assert!(
        out.contains("\"diagnostics\":[]"),
        "cero diagnósticos: la path-dep resolves como en `ray run`\n{out}"
    );
}

#[test]
fn diagnostica_templates_ray_html() {
    // M55: un buffer `.ray.html` se diagnostica con el pipeline de `ray templ`. (1) Un typo en una
    // variable ({{ titluo }}) genera código que no compila y el error vuelve TRADUCIDO a la línea
    // del template (línea 2 → 1 en 0-based) con el prefijo "template:". (2) Un error del propio
    // template (if sin endif) sale con su línea. (3) hover sobre el TYPO (no declarado) devuelve
    // null — no hereda el hover de un nodo envolvente del generado. (4) hover sobre un nombre
    // declarado (el param en la cabecera) da su tipo, vía el módulo generado.
    let open_typo = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///vista.ray.html","text":"{% params titulo: string %}\n<h1>{{ titluo }}</h1>\n"}}}"#;
    let open_sin_endif = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///rota.ray.html","text":"{% params x: int %}\n{% if x > 0 %}abierto\n"}}}"#;
    let hover = r#"{"jsonrpc":"2.0","id":9,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///vista.ray.html"},"position":{"line":1,"character":9}}}"#;
    let hover_param = r#"{"jsonrpc":"2.0","id":10,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///vista.ray.html"},"position":{"line":0,"character":12}}}"#;
    let entry = frame(open_typo) + &frame(open_sin_endif) + &frame(hover) + &frame(hover_param)
        + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let out = lsp(&entry);
    assert!(out.contains("titluo"), "el typo llega como diagnóstico\n{out}");
    assert!(out.contains("template:"), "prefijo de template\n{out}");
    // El typo está en la línea 2 del template → "line":1 (0-based) en su rango.
    let seccion_typo = out.split("vista.ray.html").nth(1).unwrap_or("");
    assert!(seccion_typo.contains("\"line\":1"), "el error mapea a la línea del template\n{out}");
    assert!(out.contains("endif"), "el if sin close se reports\n{out}");
    assert!(out.contains("\"id\":9,\"result\":null"), "hover about el typo = null\n{out}");
    let seccion_param = out.split("\"id\":10").nth(1).unwrap_or("");
    assert!(seccion_param.contains("titulo: string"), "hover del param da su type\n{out}");
}
