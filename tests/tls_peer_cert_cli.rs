//! M124 — `net.tls_peer_cert`: el resumen del certificado del peer. Un servidor TLS de juguete
//! (rustls, cert autofirmado de `tests/fixtures/` — CN=localhost, emitido por raylang-test-ca,
//! SAN DNS:localhost) acepta la conexión; el cliente raylang conecta con `tls_connect` (el CA de
//! prueba entra por `SSL_CERT_FILE`, como en tls_upgrade_cli) y pide el certificado SIN hacer
//! ninguna I/O antes — probando que `tls_peer_cert` CONDUCE el handshake pendiente. Se aseveran
//! los campos EXACTOS (subject/issuer/SAN — byte-idénticos entre motores porque el parseo X.509
//! es el mismo código de ray-runtime) y la ventana de validez.

use std::io::Read;
use std::net::TcpListener;
use std::process::Command;
use std::sync::Arc;
use std::thread;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};

const CERT_PEM: &str = include_str!("fixtures/tls_cert.pem");
const KEY_PEM: &str = include_str!("fixtures/tls_key.pem");

/// Servidor TLS de una conexión: completa el handshake y espera el cierre (una lectura que
/// devuelve 0/err al llegar el close_notify del cliente).
fn launch_tls_server() -> u16 {
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
        // Conduce el handshake por completo; luego espera a que el cliente cierre.
        while conn.is_handshaking() {
            if conn.complete_io(&mut sock).is_err() {
                return;
            }
        }
        let mut tls = rustls::Stream::new(&mut conn, &mut sock);
        let mut buf = [0u8; 16];
        let _ = tls.read(&mut buf);
    });
    port
}

/// El cliente: conecta y pide el certificado SIN I/O previa (el handshake lo conduce
/// `tls_peer_cert`). Imprime los campos y dos aserciones de validez.
const CLIENT: &str = r#"
import std/net;
import std/time;

fn main() -> int {
    let port = match (parse_int(args()[0])) {
        Option.Some(p) => p,
        Option.None => {
            print("bad port");
            return 1;
        },
    };
    let h = match (net.tls_connect("localhost", port)) {
        Result.Ok(x) => x,
        Result.Err(e) => {
            print("connect err: " + e);
            return 1;
        },
    };
    match (net.tls_peer_cert(h)) {
        Result.Ok(cert) => {
            print(cert.subject);
            print(cert.issuer);
            print(cert.san);
            let now = time.now();
            print(cert.not_before_ms < now);
            print(cert.not_after_ms > now);
        },
        Result.Err(e) => print("cert err: " + e),
    }
    close(h);
    0
}
"#;

const EXPECTED: &[&str] = &["CN=localhost", "CN=raylang-test-ca", "[localhost]", "true", "true"];

fn run_client(flags: &[&str], port: u16) -> Vec<String> {
    let mut path = std::env::temp_dir();
    path.push(format!("tls_peer_cert_{}.ray", flags.join("_")));
    std::fs::write(&path, CLIENT).expect("escribe el cliente");
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&path)
        .arg(port.to_string())
        .env("SSL_CERT_FILE", &ca)
        .output()
        .expect("lanza raylang");
    assert!(
        out.status.success(),
        "el cliente falló\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect()
}

#[test]
fn tls_peer_cert_interpreter() {
    let port = launch_tls_server();
    assert_eq!(run_client(&[], port), EXPECTED);
}

#[test]
fn tls_peer_cert_vm() {
    let port = launch_tls_server();
    assert_eq!(run_client(&["--vm"], port), EXPECTED);
}

/// El binario NATIVO (sabor default): mismo cliente, misma salida byte a byte (el parseo X.509 es
/// el mismo código de ray-runtime en los tres motores).
#[test]
fn tls_peer_cert_native() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        assert!(std::env::var_os("CI").is_none(), "rustc no disponible bajo CI: falso verde");
        eprintln!("saltando tls_peer_cert_native: rustc no disponible");
        return;
    }
    let mut src = std::env::temp_dir();
    src.push("tls_peer_cert_native.ray");
    std::fs::write(&src, CLIENT).expect("escribe el cliente");
    let bin = std::env::temp_dir().join(format!("ray_tls_peer_cert_{}", std::process::id()));
    let build = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["build", src.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza build --native");
    assert!(build.status.success(), "build --native falló: {}", String::from_utf8_lossy(&build.stderr));
    let port = launch_tls_server();
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(&bin).arg(port.to_string()).env("SSL_CERT_FILE", &ca).output().expect("corre el binario");
    let _ = std::fs::remove_file(&bin);
    assert!(
        out.status.success(),
        "el binario nativo falló\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    assert_eq!(lines, EXPECTED, "el nativo diverge en tls_peer_cert");
}
