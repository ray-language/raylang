//! Oráculo del **lexer auto-alojado** (M14, self-hosting).
//!
//! El lexer escrito en raylang (`selfhost/lexer.ray`, vía el driver `selfhost/lex_dump.ray`) debe
//! producir EXACTAMENTE los mismos tokens que el lexer de Rust (`src/lexer.rs`). Este test los
//! compara en un formato canónico (`<KIND>@<línea>:<col>`, uno por línea): el lado raylang lo
//! imprime; el lado Rust (este archivo) lo reconstruye con `canonical`. Si difieren, el lexer
//! auto-alojado no es fiel.
//!
//! Estrategia: para cada fuente, se escribe a un archivo temporal y se ejecuta
//! `raylang selfhost/lex_dump.ray <archivo>`; su stdout se compara con `canonical(fuente)`.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use raylang::token::TokenKind;

/// Re-escapa una cadena igual que `escape` en `lex_dump.ray` (idéntico carácter a carácter).
fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out
}

/// El nombre canónico de un token, espejo de `kind_str` en `lex_dump.ray`.
fn kind_str(k: &TokenKind) -> String {
    match k {
        TokenKind::Int(n) => format!("Int({})", n),
        TokenKind::Float(f) => format!("Float({})", f),
        TokenKind::Str(s) => format!("Str(\"{}\")", escape(s)),
        TokenKind::Char(c) => format!("Char('{}')", escape(&c.to_string())),
        TokenKind::Ident(name) => format!("Ident({})", name),
        TokenKind::Let => "Let".into(),
        TokenKind::Var => "Var".into(),
        TokenKind::Fn => "Fn".into(),
        TokenKind::Return => "Return".into(),
        TokenKind::If => "If".into(),
        TokenKind::Else => "Else".into(),
        TokenKind::While => "While".into(),
        TokenKind::True => "True".into(),
        TokenKind::False => "False".into(),
        TokenKind::Struct => "Struct".into(),
        TokenKind::Enum => "Enum".into(),
        TokenKind::Match => "Match".into(),
        TokenKind::Trait => "Trait".into(),
        TokenKind::Impl => "Impl".into(),
        TokenKind::Dyn => "Dyn".into(),
        TokenKind::Pub => "Pub".into(),
        TokenKind::Import => "Import".into(),
        TokenKind::From => "From".into(),
        TokenKind::As => "As".into(),
        TokenKind::IntType => "IntType".into(),
        TokenKind::FloatType => "FloatType".into(),
        TokenKind::BoolType => "BoolType".into(),
        TokenKind::StringType => "StringType".into(),
        TokenKind::CharType => "CharType".into(),
        TokenKind::Plus => "Plus".into(),
        TokenKind::Minus => "Minus".into(),
        TokenKind::Star => "Star".into(),
        TokenKind::Slash => "Slash".into(),
        TokenKind::Percent => "Percent".into(),
        TokenKind::EqEq => "EqEq".into(),
        TokenKind::BangEq => "BangEq".into(),
        TokenKind::Lt => "Lt".into(),
        TokenKind::LtEq => "LtEq".into(),
        TokenKind::Gt => "Gt".into(),
        TokenKind::GtEq => "GtEq".into(),
        TokenKind::AmpAmp => "AmpAmp".into(),
        TokenKind::PipePipe => "PipePipe".into(),
        TokenKind::Bang => "Bang".into(),
        TokenKind::Eq => "Eq".into(),
        TokenKind::LParen => "LParen".into(),
        TokenKind::RParen => "RParen".into(),
        TokenKind::LBrace => "LBrace".into(),
        TokenKind::RBrace => "RBrace".into(),
        TokenKind::LBracket => "LBracket".into(),
        TokenKind::RBracket => "RBracket".into(),
        TokenKind::Comma => "Comma".into(),
        TokenKind::Semicolon => "Semicolon".into(),
        TokenKind::Colon => "Colon".into(),
        TokenKind::Dot => "Dot".into(),
        TokenKind::Arrow => "Arrow".into(),
        TokenKind::FatArrow => "FatArrow".into(),
        TokenKind::Question => "Question".into(),
        TokenKind::PipeArrow => "PipeArrow".into(),
        TokenKind::At => "At".into(),
        TokenKind::Eof => "Eof".into(),
    }
}

/// La salida canónica del lexer de Rust (el oráculo) para una fuente.
fn canonical(src: &str) -> String {
    let tokens = raylang::lexer::lex(src).expect("el oráculo (lexer de Rust) lexea sin error");
    tokens
        .iter()
        .map(|t| format!("{}@{}:{}", kind_str(&t.kind), t.line, t.col))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Ruta a un archivo del repo (relativa a la raíz del crate).
fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Ejecuta el lexer auto-alojado sobre `src`: lo escribe a un temporal y corre
/// `raylang selfhost/lex_dump.ray <temporal>`. Devuelve su stdout (sin el salto final).
fn lex_dump(src: &str, nombre_tmp: &str) -> String {
    let mut tmp = std::env::temp_dir();
    tmp.push(nombre_tmp);
    let mut f = std::fs::File::create(&tmp).expect("crea el temporal");
    f.write_all(src.as_bytes()).expect("escribe el temporal");
    drop(f);

    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(repo_path("selfhost/lex_dump.ray"))
        .arg(&tmp)
        .output()
        .expect("ejecuta el lexer auto-alojado");
    assert!(
        out.status.success(),
        "lex_dump falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// Compara el lexer auto-alojado con el oráculo para una fuente concreta.
fn comparar(src: &str, nombre_tmp: &str) {
    let esperado = canonical(src);
    let obtenido = lex_dump(src, nombre_tmp);
    assert_eq!(obtenido, esperado, "el lexer auto-alojado difiere del oráculo para:\n{src}");
}

#[test]
fn casos_basicos() {
    comparar("fn fib(n: int) -> int { fib(n - 1) }", "sh_basico.ray");
    comparar("", "sh_vacio.ray");
    comparar("let x = 42; var y = 3.14;", "sh_literales.ray");
}

#[test]
fn todos_los_operadores_y_puntuacion() {
    comparar(
        "+ - * / % == != < <= > >= && || ! = -> => ? |> @ ( ) { } [ ] , ; : .",
        "sh_ops.ray",
    );
}

#[test]
fn palabras_clave_y_tipos() {
    comparar(
        "let var fn return if else while true false struct enum match trait impl dyn pub import from as int float bool string char",
        "sh_keywords.ray",
    );
}

#[test]
fn cadenas_caracteres_y_escapes() {
    comparar(r#""hola\nmundo\t\\\"fin""#, "sh_str.ray");
    comparar(r"'a' '\n' '\t' '\\' '\''", "sh_char.ray");
    comparar("\"con\\rretorno\"", "sh_cr.ray");
}

#[test]
fn flotantes_y_punto() {
    // El '.' sin dígito detrás no es flotante: 1.x → Int(1) Dot Ident(x).
    comparar("12.5 0.0 100.25 1.x 7", "sh_float.ray");
}

#[test]
fn comentarios_se_ignoran() {
    comparar("let // comentario\n x // otro\n y", "sh_coment.ray");
}

#[test]
fn posiciones_multilinea() {
    comparar("let x\n  42\n    foo", "sh_pos.ray");
}

/// El test más fuerte: lexar archivos REALES (los ejemplos y el propio lexer auto-alojado) y
/// exigir que el lexer en raylang coincida con el de Rust en cada uno.
#[test]
fn lexa_archivos_reales_igual_que_el_oraculo() {
    let archivos = [
        "examples/fib.ray",
        "examples/fizzbuzz.ray",
        "examples/enums.ray",
        "examples/genericos.ray",
        "examples/mapa.ray",
        "examples/match_figuras.ray",
        "selfhost/lexer.ray",
        "selfhost/lex_dump.ray",
    ];
    for rel in archivos {
        let src = std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("lee {rel}: {e}"));
        let esperado = canonical(&src);
        let nombre_tmp = format!("sh_real_{}.ray", rel.replace('/', "_"));
        let obtenido = lex_dump(&src, &nombre_tmp);
        assert_eq!(obtenido, esperado, "el lexer auto-alojado difiere del oráculo en {rel}");
    }
}
