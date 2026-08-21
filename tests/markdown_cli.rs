//! Pruebas de **std/markdown** (M111): parser Markdown en raylang puro (AST + HTML). Determinista
//! → golden byte-idéntico sobre un documento completo (`tests/fixtures/markdown_doc.md`) en los
//! TRES motores, más una batería de casos con dientes (URLs con paréntesis, XSS neutralizado,
//! cambio de tipo de lista, cercas sin cerrar, escapes).

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_md_{name}"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn run_prog(dir: &PathBuf, engine: &str, prog: &str) -> (String, String, i32) {
    std::fs::write(dir.join("prog.ray"), prog).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args([engine, "prog.ray"])
        .current_dir(dir)
        .output()
        .expect("lanza ray");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

const GOLDEN_PROG: &str = "import std/fs;\nimport std/markdown;\n\nfn main() -> int {\n    match (fs.read_file(\"doc.md\")) {\n        Result.Ok(md) => { let _ = __stdout_write(markdown.to_html(md)); let _ = __stdout_flush(); 0 },\n        Result.Err(e) => { eprint(e); 1 },\n    }\n}\n";

#[test]
fn golden_document_is_byte_identical_on_all_three_engines() {
    let base = tmp("golden");
    std::fs::copy(
        format!("{}/tests/fixtures/markdown_doc.md", env!("CARGO_MANIFEST_DIR")),
        base.join("doc.md"),
    )
    .unwrap();
    let want = include_str!("fixtures/markdown_doc.html");
    for engine in ["--vm", "--interp"] {
        let (out, err, code) = run_prog(&base, engine, GOLDEN_PROG);
        assert_eq!(code, 0, "{engine}\n{err}");
        assert_eq!(out, want, "{engine}: golden exacto");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        std::fs::write(base.join("prog.ray"), GOLDEN_PROG).unwrap();
        let bin = base.join("prog_bin");
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let native = Command::new(&bin).current_dir(&base).output().expect("nativo");
        assert_eq!(String::from_utf8_lossy(&native.stdout), want, "nativo ≡ VM");
    }
}

/// Batería dirigida: cada caso es (markdown, html esperado). Corre en ambos motores.
#[test]
fn targeted_cases() {
    let cases: &[(&str, &str)] = &[
        // URL con paréntesis balanceados (CommonMark).
        ("[x](http://a/b(1))", "<p><a href=\"http://a/b(1)\">x</a></p>\n"),
        // javascript:/vbscript:/data: no-imagen → "#"; data:image pasa.
        ("[x](javascript:alert(1))", "<p><a href=\"#\">x</a></p>\n"),
        ("![a](data:text/html;base64,xxx)", "<p><img src=\"#\" alt=\"a\"></p>\n"),
        // Cambio de tipo de marcador = listas separadas.
        ("- a\n- b\n\n1. c\n", "<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n<ol>\n<li>c</li>\n</ol>\n"),
        // Cerca sin cerrar: lenidad hasta el final.
        ("```\ncode\n", "<pre><code>code\n</code></pre>\n"),
        // Énfasis sin cierre queda literal.
        ("un *suelto", "<p>un *suelto</p>\n"),
        // Encabezado con cierre opcional y ###### máximo.
        ("## dos ##\n####### siete\n", "<h2>dos</h2>\n<p>####### siete</p>\n"),
        // Cita con lista dentro.
        ("> - a\n> - b\n", "<blockquote>\n<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n</blockquote>\n"),
        // Código inline con < y &.
        ("`a<b&c`", "<p><code>a&lt;b&amp;c</code></p>\n"),
        // Negrita con guiones bajos y énfasis anidado.
        ("__b _i_ b__", "<p><strong>b <em>i</em> b</strong></p>\n"),
        // Regla con espacios.
        ("- - -\n", "<hr>\n"),
        // Documento vacío.
        ("", ""),
    ];
    let base = tmp("casos");
    for engine in ["--vm", "--interp"] {
        for (i, (md, want)) in cases.iter().enumerate() {
            let prog = format!(
                "import std/markdown;\nfn main() -> int {{\n    let _ = __stdout_write(markdown.to_html({md:?}));\n    let _ = __stdout_flush();\n    0\n}}\n"
            );
            let (out, err, code) = run_prog(&base, engine, &prog);
            assert_eq!(code, 0, "{engine} caso {i} ({md:?})\n{err}");
            assert_eq!(&out, want, "{engine} caso {i}: {md:?}");
        }
    }
}

/// El AST es utilizable directamente (el caso TUI): parse + match sobre los bloques.
#[test]
fn ast_is_directly_consumable() {
    let prog = "import std/markdown;\nfn main() -> int {\n    let blocks = markdown.parse(\"# T\\n\\nparrafo\\n\\n- a\\n- b\\n\");\n    var counts = \"\";\n    var i = 0;\n    while (i < blocks.len()) {\n        match (blocks[i]) {\n            markdown.Block.Heading(lvl, _) => { counts = counts + \"h\" + to_string(lvl); },\n            markdown.Block.Paragraph(_) => { counts = counts + \"p\"; },\n            markdown.Block.List(ordered, items) => { counts = counts + \"l\" + to_string(items.len()); },\n            _ => { counts = counts + \"?\"; },\n        }\n        i = i + 1;\n    }\n    print(counts);\n    0\n}\n";
    let base = tmp("ast");
    for engine in ["--vm", "--interp"] {
        let (out, err, code) = run_prog(&base, engine, prog);
        assert_eq!(code, 0, "{engine}\n{err}");
        assert_eq!(out, "h1pl2\n", "{engine}");
    }
}
