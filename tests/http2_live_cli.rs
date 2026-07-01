//! M31.2b — transporte HTTP/2 vivo. El cliente HTTP/2 de raylang (`http2_client.ray`) hace un GET
//! completo (preface + SETTINGS + HEADERS + lectura de la respuesta) sobre TLS con ALPN `h2` contra un
//! **servidor h2 de juguete escrito a mano** en este test (solo std + rustls, sin traer h2/hyper — fiel
//! al invariante cero-deps). El servidor responde `:status: 200` (índice 0x88 de la tabla estática) y
//! un cuerpo fijo en un frame DATA con END_STREAM. Determinista, sin red externa, ambos motores.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::Arc;
use std::thread;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};

const CERT_PEM: &str = include_str!("fixtures/tls_cert.pem");
const KEY_PEM: &str = include_str!("fixtures/tls_key.pem");

/// Un frame HTTP/2 a mano: cabecera de 9 octetos (long 24-bit BE, tipo, flags, stream 31-bit) + carga.
fn frame(ftype: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let n = payload.len();
    let mut f = vec![
        (n >> 16) as u8, (n >> 8) as u8, n as u8,
        ftype, flags,
        (stream >> 24) as u8 & 0x7f, (stream >> 16) as u8, (stream >> 8) as u8, stream as u8,
    ];
    f.extend_from_slice(payload);
    f
}

/// Levanta un servidor HTTP/2 de juguete (una conexión) con ALPN `h2`. Devuelve su puerto efímero.
fn lanzar_servidor_h2() -> u16 {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(CERT_PEM.as_bytes())
        .collect::<Result<_, _>>()
        .expect("cert de prueba");
    let key = PrivateKeyDer::from_pem_slice(KEY_PEM.as_bytes()).expect("clave de prueba");
    let mut cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("config servidor");
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    let config = Arc::new(cfg);

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut conn = ServerConnection::new(config).expect("server conn");
            let mut tls = rustls::Stream::new(&mut conn, &mut sock);
            // Lee la ráfaga inicial del cliente (preface + SETTINGS + HEADERS). Basta una lectura para
            // arrancar el handshake y drenar la petición; no la parseamos (servidor de juguete).
            let mut buf = [0u8; 4096];
            let _ = tls.read(&mut buf);
            // Respuesta: SETTINGS del servidor + HEADERS (:status 200 = índice 0x88) + DATA END_STREAM.
            let mut out = Vec::new();
            out.extend_from_slice(&frame(4, 0, 0, &[]));            // SETTINGS vacío
            out.extend_from_slice(&frame(1, 4, 1, &[0x88]));        // HEADERS END_HEADERS, :status 200
            out.extend_from_slice(&frame(0, 1, 1, b"hola-h2"));     // DATA END_STREAM
            let _ = tls.write_all(&out);
            let _ = tls.flush();
        }
    });
    port
}

fn correr(flags: &[&str], port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/http2_get_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg("localhost")
        .arg(port.to_string())
        .env("SSL_CERT_FILE", &ca)
        .output()
        .expect("ejecuta http2_get_demo.ray");
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect()
}

const ESPERADO: &[&str] = &["status: 200", "body: hola-h2"];

#[test]
fn http2_get_interprete() {
    let port = lanzar_servidor_h2();
    assert_eq!(correr(&[], port), ESPERADO);
}

#[test]
fn http2_get_vm() {
    let port = lanzar_servidor_h2();
    assert_eq!(correr(&["--vm"], port), ESPERADO);
}
