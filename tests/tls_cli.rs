//! M19.4a — cliente TLS (`https://`). El runtime habla TLS vía rustls (única dependencia, decisión
//! §28.4). Para un test **determinista y sin red**, levantamos un servidor TLS local **en el propio
//! test** (rustls, con un certificado autofirmado de `tests/fixtures/`) y conectamos el cliente
//! raylang a `https://localhost:<puerto>/`, haciéndole confiar en esa CA vía `SSL_CERT_FILE`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};

const CERT_PEM: &str = include_str!("fixtures/tls_cert.pem");
const KEY_PEM: &str = include_str!("fixtures/tls_key.pem");

/// Levanta un servidor TLS de una sola conexión que responde un 200 con cuerpo fijo, y devuelve su
/// puerto efímero. Corre en un hilo aparte; termina tras servir una petición.
fn lanzar_servidor_tls() -> u16 {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(CERT_PEM.as_bytes())
        .collect::<Result<_, _>>()
        .expect("certificado de prueba válido");
    let key = PrivateKeyDer::from_pem_slice(KEY_PEM.as_bytes()).expect("clave de prueba válida");
    let config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("config de servidor"),
    );

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();

    thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        let mut conn = ServerConnection::new(config).expect("server conn");
        let mut tls = rustls::Stream::new(&mut conn, &mut sock);
        // Lee la petición del cliente (basta una lectura para arrancar el handshake + ver el GET).
        let mut buf = [0u8; 4096];
        let _ = tls.read(&mut buf);
        // Respuesta HTTP fija. El cuerpo "hola-tls" son 8 octetos.
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nhola-tls";
        let _ = tls.write_all(resp.as_bytes());
        let _ = tls.flush();
        conn.send_close_notify();
        let _ = conn.complete_io(&mut sock);
    });

    port
}

/// Corre el demo HTTPS contra `url`, confiando en la CA de prueba (`SSL_CERT_FILE`). Devuelve stdout.
fn correr_demo(flags: &[&str], url: &str) -> String {
    let demo = format!("{}/examples/https_demo.ray", env!("CARGO_MANIFEST_DIR"));
    // El cliente confía en la CA de prueba (que firmó el cert de `localhost`), no en la hoja.
    let cert = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg(url)
        .env("SSL_CERT_FILE", &cert)
        .output()
        .expect("ejecuta https_demo");
    assert!(
        out.status.success(),
        "https_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn cliente_https_contra_servidor_local_interprete() {
    let port = lanzar_servidor_tls();
    let out = correr_demo(&[], &format!("https://localhost:{port}/"));
    assert!(out.contains("status=200"), "esperaba 200, got: {out}");
    assert!(out.contains("body=hola-tls"), "cuerpo incorrecto, got: {out}");
}

#[test]
fn cliente_https_contra_servidor_local_vm() {
    let port = lanzar_servidor_tls();
    let out = correr_demo(&["--vm"], &format!("https://localhost:{port}/"));
    assert!(out.contains("status=200"), "esperaba 200, got: {out}");
    assert!(out.contains("body=hola-tls"), "cuerpo incorrecto, got: {out}");
}
