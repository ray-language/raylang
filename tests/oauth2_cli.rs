//! Prueba del cliente OAuth2 (`examples/web/oauth2.ray`, M23). Se levanta un **token endpoint de
//! juguete** en Rust que acepta un POST client_credentials y responde el JSON del token. Se corre
//! `oauth2_demo.ray` contra él por ambos motores y se comprueba el token parseado + la cabecera Bearer
//! + la URL de autorización (pura).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

/// Token endpoint de juguete: lee la petición POST y, si es un grant client_credentials válido, responde
/// 200 con el JSON del token. Atiende varias conexiones. Devuelve el puerto.
fn toy_token_endpoint() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut stream = match conn {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            // El cuerpo va tras la línea en blanco.
            let body = req.split("\r\n\r\n").nth(1).unwrap_or("");
            let ok = body.contains("grant_type=client_credentials")
                && body.contains("client_id=mi-cliente")
                && body.contains("client_secret=secreto");
            let json = if ok {
                r#"{"access_token":"tok-abc-123","token_type":"Bearer","expires_in":3600,"scope":"read"}"#
            } else {
                r#"{"error":"invalid_client"}"#
            };
            let status = if ok { "200 OK" } else { "401 Unauthorized" };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                json.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

fn correr(flags: &[&str], port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/oauth2_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg(port.to_string())
        .output()
        .expect("ejecuta oauth2_demo.ray");
    assert!(
        out.status.success(),
        "oauth2_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

const ESPERADO: &[&str] = &[
    "https://auth.example.com/authorize?response_type=code&client_id=mi-cliente&redirect_uri=https%3A%2F%2Fapp.example.com%2Fcallback&scope=read%20write&state=xyz123",
    "access_token=tok-abc-123",
    "token_type=Bearer",
    "expires_in=3600",
    "Authorization: Bearer tok-abc-123",
];

#[test]
fn oauth2_client_credentials_interprete() {
    let port = toy_token_endpoint();
    assert_eq!(correr(&[], port), ESPERADO);
}

#[test]
fn oauth2_client_credentials_vm() {
    let port = toy_token_endpoint();
    assert_eq!(correr(&["--vm"], port), ESPERADO);
}
