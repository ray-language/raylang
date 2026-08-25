//! M131 — `net/mail`: las codificaciones de correo (RFC 2047 encoded-words, plegado a 78,
//! base64 a 76 columnas, dot-stuffing, mailbox con display-name). Golden determinista (los
//! encoded-words se verificaron a mano contra la RFC) en ambos motores, sobre un proyecto
//! temporal con path-dep al paquete net (patrón del test de jwt).

use std::process::Command;

const MAIN: &str = r#"
import net/mail;

fn show(s: string) -> string {
    s.replace("\r", "\\r").replace("\n", "\\n")
}

fn main() -> int {
    // encoded-word: ASCII pasa tal cual; no-ASCII → =?UTF-8?B?...?=; largo → varios words.
    print(mail.encoded_word("plain ascii subject"));
    print(mail.encoded_word("Canción de cumpleaños"));
    // header: plegado a 78 con continuación de UN espacio, CRLF.
    print(show(mail.header("Subject", "hola")));
    print(show(mail.header(
        "Subject",
        "Un asunto muy largo que definitivamente no cabe en una sola linea de setenta y ocho columnas y debe plegarse"
    )));
    print(show(mail.header(
        "Subject",
        "Reunión de mañana: café y planificación estratégica del trimestre próximo"
    )));
    // base64_body: 76 columnas exactas + CRLF.
    var data = b"";
    var i = 0;
    while (i < 100) {
        data = data + b"abcdefghij";
        i = i + 1;
    }
    let lines = mail.base64_body(data).split("\r\n");
    print(to_string(lines[0].len()) + " " + to_string(lines.len()));
    // dot_stuffing: punto inicial doblado, finales de línea normalizados a CRLF.
    print(show(mail.dot_stuff(".inicio\nnormal\n..ya doblado\r\nfin.")));
    // address: vacío, atext, con comillas, no-ASCII.
    print(mail.address("", "ana@rayala.org"));
    print(mail.address("Ana", "ana@rayala.org"));
    print(mail.address("Ana R. Ayala", "ana@rayala.org"));
    print(mail.address("Ana Ñandú", "ana@rayala.org"));
    0
}
"#;

const EXPECTED: &[&str] = &[
    "plain ascii subject",
    "=?UTF-8?B?Q2FuY2nDs24gZGUgY3VtcGxlYcOxb3M=?=",
    "Subject: hola\\r\\n",
    "Subject: Un asunto muy largo que definitivamente no cabe en una sola linea de\\r\\n setenta y ocho columnas y debe plegarse\\r\\n",
    "Subject:\\r\\n =?UTF-8?B?UmV1bmnDs24gZGUgbWHDsWFuYTogY2Fmw6kgeSBwbGFuaWZpY2FjacOzbiBl?=\\r\\n =?UTF-8?B?c3RyYXTDqWdpY2EgZGVsIHRyaW1lc3RyZSBwcsOzeGltbw==?=\\r\\n",
    "76 19",
    "..inicio\\r\\nnormal\\r\\n...ya doblado\\r\\nfin.",
    "<ana@rayala.org>",
    "Ana <ana@rayala.org>",
    "\"Ana R. Ayala\" <ana@rayala.org>",
    "=?UTF-8?B?QW5hIMORYW5kw7o=?= <ana@rayala.org>",
];

fn run(vm: bool) -> (Vec<String>, bool) {
    let repo = env!("CARGO_MANIFEST_DIR");
    let base = std::env::temp_dir().join(format!("ray_mail_{}", if vm { "vm" } else { "interp" }));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).expect("crea el dir");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"mailtest\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(base.join("src/main.ray"), MAIN).unwrap();
    let flag = if vm { "--vm" } else { "--interp" };
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(flag)
        .arg(base.join("src/main.ray"))
        .current_dir(&base)
        .output()
        .expect("ejecuta");
    let lines = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    (lines, out.status.success())
}

#[test]
fn mail_encodings_both_engines() {
    for vm in [false, true] {
        let (lines, ok) = run(vm);
        assert!(ok, "falló (vm={vm}): {lines:?}");
        assert_eq!(lines, EXPECTED, "vm={vm}");
    }
}
