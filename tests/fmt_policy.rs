//! El repo es un **punto fijo** de `ray fmt`: pasar el formateador a cualquier `.ray` versionado no
//! cambia ni un byte. Y más fuerte (auditoría del 21 ago 2026, a raíz de un reporte de código
//! perdido): formatear **no altera el programa** — el AST del formateado es idéntico al de la
//! fuente (módulo posiciones) y el multiconjunto de comentarios `//` se conserva, sobre TODO el
//! repo y sobre un corpus adversarial (comentarios en sitios raros, templates multilínea con
//! URLs — el bug real que la auditoría cazó: `collect_comments` no conocía los backticks y un
//! `https://` dentro de un template inyectaba/duplicaba comentarios fantasma).
//!
//! Fallos y qué significan:
//! - **canónico** (`format(x) == x`): el archivo está sin formatear → `ray fmt --write <archivo>`.
//! - **convergente** (`format(format(x)) == format(x)`): bug del FORMATEADOR (no converge).
//! - **AST alterado / comentarios alterados**: bug GRAVE del formateador (pierde o inventa
//!   código/comentarios) — nunca es culpa del archivo.
//!
//! El barrido va sobre los archivos **versionados** (`git ls-files`), no sobre el árbol: así los
//! artefactos de una compilación nativa o un `target/` con `.ray` copiados no entran a la prueba.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Los `.ray` versionados del repo, en orden estable.
fn tracked_ray_files(root: &Path) -> Vec<PathBuf> {
    let out = Command::new("git")
        .args(["ls-files", "-z", "*.ray"])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    assert!(out.status.success(), "git ls-files falló: {}", String::from_utf8_lossy(&out.stderr));
    let mut files: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| root.join(s))
        .collect();
    files.sort();
    files
}

fn norm_ast(src: &str) -> Result<String, String> {
    let toks = raylang::lexer::lex(src).map_err(|e| format!("lex: {e}"))?;
    let prog = raylang::parser::parse(toks).map_err(|e| format!("parse: {e}"))?;
    let mut dbg = format!("{prog:?}");
    // Corta las TABLAS DE SUPERFICIE indexadas por posición (expr_spans/interp_sites/pipe_sites,
    // HashMaps con orden aleatorio): la semántica ya está entera en los nodos desazucarados.
    if let Some(i) = dbg.find(", ufcs_aliases:") {
        dbg.truncate(i);
    }
    // Borra posiciones: `line: N` / `col: N` / `field_lines: [..]` (solo-fmt) en el Debug.
    let re_line = regex_lite(&dbg);
    Ok(re_line)
}

// Sin crate regex: normalización a mano — sustituye dígitos tras "line: "/"col: " y vacía field_lines.
fn regex_lite(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let rest = &s[i..];
        if rest.starts_with("line: ") || rest.starts_with("col: ") {
            let key_len = if rest.starts_with("line: ") { 6 } else { 5 };
            out.push_str(&rest[..key_len]);
            i += key_len;
            out.push('N');
            while i < b.len() && b[i].is_ascii_digit() { i += 1; }
            continue;
        }
        if rest.starts_with("field_lines: [") {
            out.push_str("field_lines: []");
            i += "field_lines: [".len();
            while i < b.len() && b[i] != b']' { i += 1; }
            i += 1;
            continue;
        }
        out.push(s.as_bytes()[i] as char);
        // OJO: byte a byte solo vale para ASCII; para multibyte, copia el char entero.
        i += 1;
        if !s.is_char_boundary(i) {
            // retrocede y copia el char completo
            out.pop();
            let ch_start = i - 1;
            let ch = s[ch_start..].chars().next().unwrap();
            out.push(ch);
            i = ch_start + ch.len_utf8();
        }
    }
    out
}

/// Textos de comentario `//` (fuera de strings), multiconjunto ordenado.
fn comments_of(src: &str) -> Vec<String> {
    let mut v = Vec::new();
    for line in src.lines() {
        // máscara simple de strings/backticks/bytes: escaneo con estado
        let b = line.as_bytes();
        let mut i = 0usize;
        let mut in_str = false;
        let mut delim = b'"';
        while i < b.len() {
            let c = b[i];
            if in_str {
                if c == b'\\' { i += 2; continue; }
                if c == delim { in_str = false; }
                i += 1;
                continue;
            }
            match c {
                b'"' | b'`' => { in_str = true; delim = c; i += 1; }
                b'\'' => {
                    // char literal: sáltalo con escape
                    i += 1;
                    if i < b.len() && b[i] == b'\\' { i += 1; }
                    while i < b.len() && b[i] != b'\'' { i += 1; }
                    i += 1;
                }
                b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                    v.push(line[i..].trim_end().to_string());
                    i = b.len();
                }
                _ => i += 1,
            }
        }
    }
    v.sort();
    v
}


#[test]
fn every_tracked_ray_file_is_a_fixed_point_of_the_formatter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = tracked_ray_files(root);
    assert!(files.len() > 100, "el barrido debería ver los ~270 .ray del repo, vio {}", files.len());

    let mut unformatted: Vec<String> = Vec::new();
    let mut divergent: Vec<String> = Vec::new();
    let mut unparsed: Vec<String> = Vec::new();
    let mut ast_changed: Vec<String> = Vec::new();
    let mut comments_changed: Vec<String> = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(root).unwrap_or(file).display().to_string();
        let source = std::fs::read_to_string(file).expect("lee el archivo");
        // Un `.ray` que no parsea no se puede formatear. No se salta en silencio: se reporta, porque
        // un archivo del repo que no compila es en sí mismo una señal (y si alguno debiera quedar
        // fuera de la prueba a propósito, la exclusión tiene que ser explícita, no un `continue`).
        let Ok(once) = raylang::fmt::format_source(&source) else {
            unparsed.push(rel);
            continue;
        };
        if once != source {
            unformatted.push(rel.clone());
        }
        // Propiedades semánticas: formatear no altera el programa ni sus comentarios.
        match (norm_ast(&source), norm_ast(&once)) {
            (Ok(a), Ok(b)) if a != b => ast_changed.push(rel.clone()),
            (_, Err(e)) => ast_changed.push(format!("{rel} (¡el formateado no parsea!: {e})")),
            _ => {}
        }
        if comments_of(&source) != comments_of(&once) {
            comments_changed.push(rel.clone());
        }
        match raylang::fmt::format_source(&once) {
            Ok(twice) if twice == once => {}
            _ => divergent.push(rel),
        }
    }

    assert!(
        ast_changed.is_empty(),
        "el formateador ALTERÓ el programa de {} archivo(s) — bug GRAVE de src/fmt.rs (código \
         perdido o inventado), nunca culpa del archivo:\n  {}",
        ast_changed.len(),
        ast_changed.join("\n  ")
    );
    assert!(
        comments_changed.is_empty(),
        "el formateador alteró los COMENTARIOS de {} archivo(s) — bug de src/fmt.rs:\n  {}",
        comments_changed.len(),
        comments_changed.join("\n  ")
    );
    assert!(
        divergent.is_empty(),
        "el FORMATEADOR no converge en {} archivo(s) — esto es un bug de src/fmt.rs, no del \
         archivo (formatear dos veces da algo distinto que formatear una):\n  {}",
        divergent.len(),
        divergent.join("\n  ")
    );
    assert!(
        unparsed.is_empty(),
        "{} archivo(s) .ray versionados no parsean:\n  {}",
        unparsed.len(),
        unparsed.join("\n  ")
    );
    assert!(
        unformatted.is_empty(),
        "{} archivo(s) .ray no están en forma canónica; arréglalo con \
         `ray fmt --write <archivo>`:\n  {}",
        unformatted.len(),
        unformatted.join("\n  ")
    );
}

/// Casos adversariales: construcciones retorcidas que el repo quizá no ejercita. Cada una pasa
/// las DOS propiedades (AST intacto módulo posiciones; comentarios conservados) y además el
/// formateado debe re-formatear idempotente.
#[test]
fn adversarial_corpus() {
    let cases: &[&str] = &[
        // comentarios en TODOS los sitios raros
        "// arriba\nfn main() -> int { // firma\n    // dentro\n    let x = 1; // trailing\n    // antes del tail\n    x // tail\n    // tras el tail\n}\n// eof\n",
        // comentarios entre brazos de match y variantes de enum
        "enum E {\n    // antes de A\n    A, // tras A\n    B(int), // tras B\n}\nfn main() -> int {\n    match (E.A) {\n        // brazo A\n        E.A => 1, // trailing A\n        // brazo B\n        E.B(n) => n,\n    }\n}\n",
        // struct con comentarios por campo + banner separado por blanco
        "// ---- banner ----\n\nstruct S {\n    // encima de a\n    a: int, // tras a\n    b: string,\n}\nfn main() -> int { 0 }\n",
        // línea larguísima que dispara el reparto (wrap+force) con cadena de métodos y args
        "fn main() -> int {\n    let result_value_with_long_name = compute_one(1111111, 2222222, 3333333).chain_two(4444444).chain_three(5555555, 6666666);\n    result_value_with_long_name\n}\nfn compute_one(a: int, b: int, c: int) -> int { a + b + c }\n",
        // interpolación con llamada dentro (el bug de M107-era) + template multilínea
        "fn f(a: int, b: int, c: int) -> int { a + b + c }\nfn main() -> int {\n    let s = `linea1 con texto de relleno para pasar el umbral de las cien columnas facilmente ${f(1, 2, 3)}\nlinea2`;\n    print(s);\n    0\n}\n",
        // closures anidadas, capturas, tuplas, index, pipeline
        "fn main() -> int {\n    var acc = 0;\n    let add = fn(x: int) { acc = acc + x; };\n    let t = (1, 2);\n    add(t.0);\n    add(t.1);\n    let xs = [1, 2, 3];\n    let y = xs[1] |> to_string;\n    print(y);\n    acc\n}\n",
        // trait + impl + genéricos + bounds + dyn
        "trait Show2 {\n    fn show2(self) -> string;\n}\nstruct P { x: int }\nimpl Show2 for P {\n    fn show2(self) -> string { to_string(self.x) }\n}\nfn describe<T: Show2>(v: T) -> string { v.show2() }\nfn main() -> int {\n    print(describe(P { x: 7 }));\n    0\n}\n",
        // extern normal + blocking, unit escrito, ptr
        "extern \"c\" blocking {\n    fn sleep(s: int) -> int;\n}\n\nextern \"c\" {\n    fn free(p: ptr) -> unit;\n}\n\nfn main() -> int { 0 }\n",
        // const, from-import largo que envuelve, import normal
        "import std/math;\nfrom std/text import pad_left, pad_right;\nconst LIMIT: int = 100;\nfn main() -> int {\n    print(pad_left(\"x\", 3, \" \"));\n    LIMIT\n}\n",
        // if/else if/else con blancos internos y comentarios entre ramas
        "fn main() -> int {\n    let n = 3;\n\n    // primera\n    if (n == 1) {\n        1\n    } else if (n == 2) {\n        // dos\n        2\n    } else {\n        3\n    }\n}\n",
        // strings con //, escapes, bytes con \x, chars, backticks con comillas
        "fn main() -> int {\n    let a = \"no // comentario\";\n    let b = b\"\\x00\\xff//\";\n    let c = 'x';\n    let d = `tiene \"comillas\" y // barras`;\n    print(a); print(b); print(c); print(d);\n    0\n}\n",
        // for-in con range y sobre arreglo, break/continue... (si el lenguaje los tiene: for sí)
        "fn main() -> int {\n    var acc = 0;\n    for i in range(0, 5) {\n        acc = acc + i;\n    }\n    for x in [10, 20] {\n        acc = acc + x;\n    }\n    acc\n}\n",
        // match con guardas? no hay: match anidado + tuplas en patrones payload
        "enum R {\n    Ok2(int),\n    Err2(string),\n}\nfn main() -> int {\n    match (R.Ok2(5)) {\n        R.Ok2(n) => match (R.Err2(\"x\")) {\n            R.Ok2(m) => n + m,\n            R.Err2(_) => n,\n        },\n        R.Err2(_) => 0,\n    }\n}\n",
        // spawn/scope/canales (superficie de concurrencia)
        "fn main() -> int {\n    let ch: Channel<int> = Channel.bounded(1);\n    let t = spawn(fn() {\n        send(ch, 42);\n    });\n    let v = match (recv(ch)) {\n        Option.Some(n) => n,\n        Option.None => 0,\n    };\n    join(t);\n    v\n}\n",
        // anotaciones @derive/@test + Map + u8/u32/u64 + as
        "@derive(Eq)\nstruct K { a: int }\nfn main() -> int {\n    var m: Map<string, int> = Map.new();\n    m.insert(\"a\", 1);\n    let x: u8 = 255;\n    let y = (x as int) + 1;\n    print(y);\n    0\n}\n",
        // comentario trailing en la ÚLTIMA línea sin salto final
        "fn main() -> int {\n    7 // resultado\n}",
        // URL con // dentro de un template MULTILÍNEA (el caso real más probable del reporte)
        "fn main() -> int {\n    let page = `<a href=\"https://example.com/x\">link</a>\notra linea con https://otra.url/aqui y texto`;\n    print(page);\n    0\n}\n",
        // apóstrofe y comilla dentro del template, y backtick escapado
        "fn main() -> int {\n    let t = `it's a \"test\" con \\` backtick escapado`;\n    print(t);\n    0\n}\n",
        // // dentro de template de UNA línea seguido de comentario REAL
        "fn main() -> int {\n    let u = `https://a.b/c`; // comentario real\n    print(u);\n    0\n}\n",
        // LA FAMILIA DE COLISIÓN DE POSICIÓN del azúcar (ago 2026, reportada como "fmt borra
        // código"): un operador exterior hereda la posición del operando izquierdo, y un
        // paréntesis re-posiciona la raíz — el resurfacing de interpolación/pipeline debe
        // identificar al nodo DUEÑO, no fiarse de la posición.
        "fn main() -> int {\n    let n = 5;\n    print(\"x ${n}\" + \" tail\");\n    0\n}\n",
        "fn main() -> int {\n    let n = 5;\n    let y = (\"x ${n}\") + \" t2\";\n    print(y);\n    let solo = (\"x ${n}\");\n    print(solo);\n    0\n}\n",
        "fn main() -> int {\n    let n = 5;\n    let m = 6;\n    print(\"a ${n}\" + \"b ${m}\" + \"c\");\n    print(\"${n}\" + \" x\");\n    print(\"pre\" + \"x ${n}\");\n    0\n}\n",
        "fn main() -> int {\n    let a = 3;\n    let s = (a |> to_string) + \"!\";\n    print(s);\n    let t = a |> to_string;\n    print(t + \"?\");\n    0\n}\n",
        // doc comments /// en fn/struct/campo-comentario mixtos
        "/// Documented.\nfn documented(x: int) -> int {\n    x\n}\n/// Doc del struct.\nstruct D {\n    f: int,\n}\nfn main() -> int { documented(D { f: 2 }.f) }\n",
    ];
    let mut failures = Vec::new();
    for (i, src) in cases.iter().enumerate() {
        let fmted = match raylang::fmt::format_source(src) {
            Ok(f) => f,
            Err(e) => { failures.push(format!("caso {i}: no formatea: {e}\n{src}")); continue }
        };
        let (a, b) = (norm_ast(src), norm_ast(&fmted));
        match (a, b) {
            (Ok(a), Ok(b)) if a != b => failures.push(format!("caso {i}: AST CAMBIÓ\n--- fuente:\n{src}\n--- formateado:\n{fmted}")),
            (_, Err(e)) => failures.push(format!("caso {i}: el formateado NO PARSEA: {e}\n{fmted}")),
            _ => {}
        }
        let (ca, cb) = (comments_of(src), comments_of(&fmted));
        if ca != cb {
            failures.push(format!("caso {i}: comentarios alterados\n  antes: {ca:?}\n  después: {cb:?}\n--- formateado:\n{fmted}"));
        }
        match raylang::fmt::format_source(&fmted) {
            Ok(f2) if f2 != fmted => failures.push(format!("caso {i}: NO idempotente\n--- 1ª pasada:\n{fmted}\n--- 2ª:\n{f2}")),
            Err(e) => failures.push(format!("caso {i}: 2ª pasada no parsea: {e}")),
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{} fallos:\n{}", failures.len(), failures.join("\n====\n"));
}
