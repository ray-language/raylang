//! M446 (ray-sublime #317) — `std/charset`: decode/encode/labels con las tablas WHATWG, idéntico
//! en VM, intérprete y nativo; y el `Err` claro de un binario construido `--without charset`.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_charset_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

const PROG: &str = r#"import std/charset;
import std/hex;

fn main() -> int {
    print(charset.decode(b"\xE9t\xE9 \x80", "windows-1252"));
    print(charset.decode(b"caf\xE9", "latin1"));
    print(charset.encode("café €", "cp1252").map(fn(b: bytes) -> string { hex.hex_encode(b) }));
    print(charset.encode("日本語", "shift_jis").map(fn(b: bytes) -> string { hex.hex_encode(b) }));
    print(charset.decode(b"\x93\xFA\x96\x7B\x8C\xEA", "sjis"));
    print(charset.decode(b"\xC4\xE3\xBA\xC3", "gbk"));
    print(charset.decode(b"\xA4\xA4\xA4\xE5", "big5"));
    print(charset.decode(b"\xBE\xC8\xB3\xE7", "euc-kr"));
    print(charset.decode(b"a\x00b\x00", "utf-16le"));
    print(charset.encode("a日b", "iso-8859-1"));
    print(charset.decode(b"A\x82", "shift_jis"));
    print(charset.decode(b"x", "klingon"));
    print(charset.encode("x", "utf-16le"));
    let labels = charset.labels();
    print(labels.len());
    print(labels.contains("windows-1252") && labels.contains("Shift_JIS") && labels.contains("Big5"));
    0
}
"#;

const WANT: &str = "Result.Ok(été €)
Result.Ok(café)
Result.Ok(636166e92080)
Result.Ok(93fa967b8cea)
Result.Ok(日本語)
Result.Ok(你好)
Result.Ok(中文)
Result.Ok(안녕)
Result.Ok(ab)
Result.Err(charset: '日' (U+65E5) cannot be encoded in windows-1252 at char index 1)
Result.Err(charset: malformed Shift_JIS sequence at byte 1)
Result.Err(charset: unknown charset 'klingon' (labels() lists the known ones))
Result.Err(charset: 'UTF-16LE' has no encoder (the WHATWG standard encodes it as UTF-8): use \"utf-8\")
40
true
";

/// Las conversiones, los errores con posición y la lista de etiquetas: lo mismo en los tres motores.
#[test]
fn charset_converts_identically_on_all_engines() {
    let d = tmp("engines");
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, engine);
        assert_eq!(code, 0, "{engine:?}: {err}");
        assert_eq!(out, WANT, "{engine:?}");
    }
    if has_rustc() {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        assert!(!err.contains("not supported in the native subset"), "{err}");
        let out = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo");
        // `--without charset`: el mismo programa compila y responde con el Err del build slim.
        let slim = d.join("prog_slim");
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "--without", "charset", "-o", slim.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native --without charset: {err}");
        let out = Command::new(&slim).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.starts_with("Result.Err(charset: charset conversion is not available in this build (feature 'charset'))\n"), "{text}");
        assert!(text.contains("\n0\nfalse\n"), "labels() vacío sin la feature: {text}");
    }
}
