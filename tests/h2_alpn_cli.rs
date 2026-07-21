//! M31.2a — negociación ALPN 'h2' en el runtime TLS. Levanta un servidor TLS local (rustls) que
//! ofrece (o no) el protocolo ALPN `h2` y comprueba que el builtin `tls_connect_h2` de raylang lo
//! negocia correctamente: éxito cuando el servidor ofrece `h2`, error cuando no. Determinista, sin red.

use std::net::TcpListener;
use std::process::Command;
use std::sync::Arc;
use std::thread;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};

const CERT_PEM: &str = include_str!("fixtures/tls_cert.pem");
const KEY_PEM: &str = include_str!("fixtures/tls_key.pem");

/// Levanta un servidor TLS de una conexión; si `ofrece_h2`, anuncia ALPN `h2`. Completa el handshake y
/// cierra. Devuelve el puerto efímero.
fn launch_server(offers_h2: bool) -> u16 {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(CERT_PEM.as_bytes())
        .collect::<Result<_, _>>()
        .expect("cert de prueba");
    let key = PrivateKeyDer::from_pem_slice(KEY_PEM.as_bytes()).expect("clave de prueba");
    let mut cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("config servidor");
    if offers_h2 {
        cfg.alpn_protocols = vec![b"h2".to_vec()];
    }
    let config = Arc::new(cfg);

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut conn = ServerConnection::new(config).expect("server conn");
            // Conduce el handshake hasta terminar (o hasta que el cliente cierre).
            while conn.is_handshaking() {
                if conn.complete_io(&mut sock).is_err() {
                    break;
                }
            }
        }
    });
    port
}

/// Corre el demo con el binario de raylang, pasándole host y puerto, con `SSL_CERT_FILE` apuntando a la
/// CA de prueba para que confíe en el certificado autofirmado. Devuelve stdout (primera línea).
fn run(flags: &[&str], port: u16) -> String {
    let demo = format!("{}/examples/net/h2_alpn_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let cert_path = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg("localhost")
        .arg(port.to_string())
        .env("SSL_CERT_FILE", &cert_path)
        .output()
        .expect("ejecuta h2_alpn_demo.ray");
    String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").to_string()
}

#[test]
fn negotiates_h2_interpreter() {
    let port = launch_server(true);
    assert_eq!(run(&[], port), "h2 ok");
}

#[test]
fn negotiates_h2_vm() {
    let port = launch_server(true);
    assert_eq!(run(&["--vm"], port), "h2 ok");
}

#[test]
fn rejects_without_h2() {
    let port = launch_server(false);
    // El servidor no ofrece `h2` → `tls_connect_h2` debe fallar.
    assert!(run(&[], port).starts_with("h2 err"), "debería fallar sin ALPN h2");
}
