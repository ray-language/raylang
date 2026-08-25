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
        TokenKind::Int(n, _) => format!("Int({})", n),
        TokenKind::Float(f) => format!("Float({})", f),
        TokenKind::Str(s) => format!("Str(\"{}\")", escape(s)),
        // M27.3: cadena interpolada (el lexer auto-alojado aún no la produce; no aparece en el corpus).
        TokenKind::InterpStr(_) => "InterpStr".into(),
        TokenKind::Char(c) => format!("Char('{}')", escape(&c.to_string())),
        // M16: el lexer auto-alojado no tokeniza `bytes`/`b"..."`; estos tokens no aparecen en el corpus
        // (ningún archivo probado los usa), pero el `match` debe ser exhaustivo.
        TokenKind::Bytes(b) => format!("Bytes({:?})", b),
        TokenKind::BytesType => "BytesType".into(),
        // M28.3: u8/u32/u64. El lexer auto-alojado aún no los tokeniza; no aparecen en el corpus,
        // pero el `match` debe ser exhaustivo.
        TokenKind::UIntType(w) => format!("UIntType({})", w),
        TokenKind::Ident(name) => format!("Ident({})", name),
        TokenKind::Let => "Let".into(),
        TokenKind::Var => "Var".into(),
        TokenKind::Fn => "Fn".into(),
        TokenKind::Return => "Return".into(),
        TokenKind::If => "If".into(),
        TokenKind::Else => "Else".into(),
        TokenKind::While => "While".into(),
        TokenKind::For => "For".into(),
        TokenKind::In => "In".into(),
        TokenKind::DotDot => "DotDot".into(),
        TokenKind::Const => "Const".into(),
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
        // M19.3a: operadores bit a bit. El lexer auto-alojado aún no los tokeniza; no aparecen
        // en el corpus (ningún archivo probado los usa), pero el `match` debe ser exhaustivo.
        TokenKind::Amp => "Amp".into(),
        TokenKind::Pipe => "Pipe".into(),
        TokenKind::Caret => "Caret".into(),
        TokenKind::Tilde => "Tilde".into(),
        TokenKind::Shl => "Shl".into(),
        TokenKind::Shr => "Shr".into(),
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
        // M41: FFI (`extern`, tipo `ptr`). El lexer auto-alojado no los tokeniza (son posteriores al
        // self-hosting); no aparecen en el corpus, pero el `match` debe ser exhaustivo.
        TokenKind::Extern => "Extern".into(),
        TokenKind::PtrType => "PtrType".into(),
        TokenKind::Eof => "Eof".into(),
    }
}

/// La salida canónica del lexer de Rust (el oráculo) para una fuente. En el caso feliz, los
/// tokens (uno por línea); ante un error léxico, el `Display` de `LexError`
/// (`lex error at <l>:<c>: <msg>`) — exactamente lo que imprime `lex_dump.ray`.
fn canonical(src: &str) -> String {
    match raylang::lexer::lex(src) {
        Ok(tokens) => tokens
            .iter()
            .map(|t| format!("{}@{}:{}", kind_str(&t.kind), t.line, t.col))
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => format!("{e}"),
    }
}

/// Ruta a un archivo del repo (relativa a la raíz del crate).
fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Ejecuta el lexer auto-alojado sobre `src`: lo escribe a un temporal y corre
/// `raylang selfhost/lex_dump.ray <temporal>`. Devuelve su stdout (sin el salto final).
fn lex_dump(src: &str, name_tmp: &str) -> String {
    let mut tmp = std::env::temp_dir();
    tmp.push(name_tmp);
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
fn compare(src: &str, name_tmp: &str) {
    let expected = canonical(src);
    let obtained = lex_dump(src, name_tmp);
    assert_eq!(obtained, expected, "el lexer auto-alojado difiere del oráculo para:\n{src}");
}

#[test]
fn basic_cases() {
    compare("fn fib(n: int) -> int { fib(n - 1) }", "sh_basico.ray");
    compare("", "sh_empty.ray");
    compare("let x = 42; var y = 3.14;", "sh_literals.ray");
}

#[test]
fn flotantes_con_exponente() {
    // M80: exponente con y sin punto/signo, y las degradaciones de la guarda
    // conservadora (`1eabc` = Int + Ident; `1e+` = Int + Ident + Plus).
    compare("let a = 1e21; let b = 1.5e-3; let c = 2E+10; let d = 7e0;", "sh_exp.ray");
    compare("let x = 1eabc; let y = 1e+; let z = 3.14e2;", "sh_exp_keeps.ray");
}

#[test]
fn all_operators_and_punctuation() {
    compare(
        "+ - * / % == != < <= > >= && || ! = -> => ? |> @ ( ) { } [ ] , ; : .",
        "sh_ops.ray",
    );
}

#[test]
fn keywords_and_types() {
    compare(
        "let var fn return if else while true false struct enum match trait impl dyn pub import from as int float bool string char",
        "sh_keywords.ray",
    );
}

#[test]
fn strings_chars_and_escapes() {
    compare(r#""hello\nmundo\t\\\"fin""#, "sh_str.ray");
    compare(r"'a' '\n' '\t' '\\' '\''", "sh_char.ray");
    compare("\"con\\rretorno\"", "sh_cr.ray");
}

#[test]
fn floats_and_dot() {
    // El '.' sin dígito detrás no es flotante: 1.x → Int(1) Dot Ident(x).
    compare("12.5 0.0 100.25 1.x 7", "sh_float.ray");
}

#[test]
fn comments_are_ignored() {
    compare("let // comment\n x // other\n y", "sh_coment.ray");
}

#[test]
fn multiline_positions() {
    compare("let x\n  42\n    foo", "sh_pos.ray");
}

/// M14.1b: errores como valores. Ante una entrada inválida, el lexer auto-alojado debe
/// producir el MISMO mensaje y ubicación que el de Rust (no abortar con `panic`).
#[test]
fn lex_errors_match_the_oracle() {
    compare("#", "sh_err_inesperado.ray"); // unexpected character '#'
    compare("\"sin close", "sh_err_str_abierta.ray"); // cadena sin cerrar
    compare("\"rota\nlinea\"", "sh_err_str_nl.ray"); // salto de línea en cadena
    compare("\"mal\\q\"", "sh_err_escape.ray"); // secuencia de escape inválida '\q'
    // M19.3a: '&' y '|' sueltos dejaron de ser error en el lexer de Rust (son AND/OR bit a
    // bit). El lexer auto-alojado (`selfhost/lexer.ray`) aún NO tokeniza bitops —diferido,
    // como `bytes`—, así que estas entradas quedan fuera del corpus del oráculo.
    compare("''", "sh_err_char_empty.ray"); // literal de carácter vacío
    compare("'ab'", "sh_err_char_multi.ray"); // más de un carácter
    compare("'a", "sh_err_char_abierto.ray"); // carácter sin cerrar
    // El error apunta al INICIO del token, no al carácter ofensor: aquí, columna 5.
    compare("let x #", "sh_err_pos.ray");
}

/// El test más fuerte: lexar archivos REALES (los ejemplos y el propio lexer auto-alojado) y
/// exigir que el lexer en raylang coincida con el de Rust en cada uno.
#[test]
fn lexes_real_files_matching_the_oracle() {
    let files = [
        "examples/basics/fib.ray",
        "examples/basics/fizzbuzz.ray",
        "examples/data/enums.ray",
        "examples/types/genericos.ray",
        "examples/data/mapa.ray",
        "examples/data/match_figuras.ray",
        "selfhost/lexer.ray",
        "selfhost/lex_dump.ray",
    ];
    for rel in files {
        let src = std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("lee {rel}: {e}"));
        let expected = canonical(&src);
        let name_tmp = format!("sh_real_{}.ray", rel.replace('/', "_"));
        let obtained = lex_dump(&src, &name_tmp);
        assert_eq!(obtained, expected, "el lexer auto-alojado difiere del oráculo en {rel}");
    }
}
