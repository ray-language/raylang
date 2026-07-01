//! M31.3 — cliente gRPC unario e2e. Cierra M31 (el diferido de M26+M25). El cliente gRPC de raylang
//! (`grpc_client.ray`) hace una llamada unaria (HTTP/2 POST + mensaje protobuf gRPC-framed) sobre TLS
//! con ALPN `h2` contra un **servidor gRPC de juguete escrito a mano** (solo std + rustls, sin traer
//! h2/hyper/tonic → fiel al cero-deps). El servidor responde `:status:200`, un mensaje protobuf de
//! respuesta gRPC-framed, y un HEADERS de trailers con `grpc-status: 0`. Determinista, ambos motores.

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

/// Servidor gRPC de juguete (una conexión) con ALPN `h2`. Puerto efímero.
fn lanzar_servidor_grpc() -> u16 {
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
            // Drena la ráfaga del cliente (preface + SETTINGS + HEADERS + DATA); no la parseamos.
            let mut buf = [0u8; 4096];
            let _ = tls.read(&mut buf);

            // Mensaje protobuf de respuesta: campo 1 (string) = "hola, raylang" (13 octetos).
            //   tag = (1<<3)|2 = 0x0a ; longitud = 0x0d ; luego los octetos.
            let mut pb = vec![0x0a, 0x0d];
            pb.extend_from_slice(b"hola, raylang");
            // gRPC frame: [flag=0 (sin comprimir)] [longitud u32 BE] [mensaje].
            let mut grpc = vec![0u8, (pb.len() >> 24) as u8, (pb.len() >> 16) as u8, (pb.len() >> 8) as u8, pb.len() as u8];
            grpc.extend_from_slice(&pb);

            // Trailers: `grpc-status: 0` como literal HPACK sin indexar (nombre nuevo, sin Huffman).
            //   0x00 (literal sin indexar) ; len(nombre)=11 ; "grpc-status" ; len(valor)=1 ; "0".
            let mut trailer = vec![0x00, 0x0b];
            trailer.extend_from_slice(b"grpc-status");
            trailer.push(0x01);
            trailer.push(b'0');

            let mut out = Vec::new();
            out.extend_from_slice(&frame(4, 0, 0, &[]));           // SETTINGS del servidor
            out.extend_from_slice(&frame(1, 4, 1, &[0x88]));       // HEADERS END_HEADERS, :status 200
            out.extend_from_slice(&frame(0, 0, 1, &grpc));         // DATA (mensaje gRPC-framed)
            out.extend_from_slice(&frame(1, 5, 1, &trailer));      // HEADERS END_HEADERS|END_STREAM (trailers)
            let _ = tls.write_all(&out);
            let _ = tls.flush();
        }
    });
    port
}

fn correr(flags: &[&str], port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/grpc_call_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg("localhost")
        .arg(port.to_string())
        .env("SSL_CERT_FILE", &ca)
        .output()
        .expect("ejecuta grpc_call_demo.ray");
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect()
}

const ESPERADO: &[&str] = &["grpc-status: 0", "greeting: hola, raylang"];

#[test]
fn grpc_call_interprete() {
    let port = lanzar_servidor_grpc();
    assert_eq!(correr(&[], port), ESPERADO);
}

#[test]
fn grpc_call_vm() {
    let port = lanzar_servidor_grpc();
    assert_eq!(correr(&["--vm"], port), ESPERADO);
}
