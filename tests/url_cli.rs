//! Pruebas de url (percent-encoding + query string) y cookies (`examples/web/{url,cookie}.ray`, M20.4).
//! Cómputo puro determinista → se corre `examples/web/url_demo.ray` por ambos motores y se compara con
//! los vectores de referencia (`urllib.parse.quote`).

use std::process::Command;

const ESPERADO: &[&str] = &[
    "hola%20mundo%20%26%20m%C3%A1s%3Dcosas%2F%C3%B1", // url_encode con UTF-8
    "a-b_c.d~e",                                       // unreserved intactos
    "hola mundo & más",                                // url_decode
    "a b",                                              // '+' → espacio
    "Ada Lovelace",                                    // query: '+' en valor
    "admin",
    "1&2",                                             // %26 dentro del valor
    "page=2&q=raylang%20lang",                         // build_query (claves ordenadas)
    "abc123",                                          // cookie session
    "dark",                                            // cookie theme
    "sid=a%20b%2Fc; Path=/; Max-Age=3600; HttpOnly; Secure", // Set-Cookie encadenado
    "plain=x",                                         // cookie sin atributos
    // M71 — SameSite (anti-CSRF): None implica Secure; Lax se emite tal cual.
    "s=1; Secure; SameSite=None",
    "s=1; SameSite=Lax",
    // M71 — saneo anti-inyección: el CRLF del nombre se elimina → no hay response splitting.
    "aSet-Cookie: evil=v",
];

fn run(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/url_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta url_demo.ray");
    assert!(
        out.status.success(),
        "url_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn url_query_cookies_interpreter() {
    assert_eq!(run(&[]), ESPERADO);
}

#[test]
fn url_query_cookies_vm() {
    assert_eq!(run(&["--vm"]), ESPERADO);
}
