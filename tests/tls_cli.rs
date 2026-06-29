//! M19.4a — cliente TLS (`https://`). El runtime habla TLS vía rustls (única dependencia, decisión
//! §28.4). Para un test **determinista y sin red**, levantamos un servidor TLS local **en el propio
//! test** (rustls, con un certificado autofirmado de `tests/fixtures/`) y conectamos el cliente
//! raylang a `https://localhost:<puerto>/`, haciéndole confiar en esa CA vía `SSL_CERT_FILE`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection};

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

// --- M19.4b: servidor TLS (`wss://`) — `examples/wss_echo.ray` ---
//
// El servidor wss en raylang corre en la VM (no bloqueante: la fibra cede mientras rustls hace el
// handshake/descifra). Lo atacamos con un cliente WebSocket-sobre-TLS escrito aquí en Rust.

/// Lanza `wss_echo.ray` (con el cert/clave de prueba) y devuelve su proceso + puerto efímero.
fn lanzar_wss_server() -> (Child, u16) {
    let demo = format!("{}/examples/wss_echo.ray", env!("CARGO_MANIFEST_DIR"));
    let cert = format!("{}/tests/fixtures/tls_cert.pem", env!("CARGO_MANIFEST_DIR"));
    let key = format!("{}/tests/fixtures/tls_key.pem", env!("CARGO_MANIFEST_DIR"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&demo)
        .arg(&cert)
        .arg(&key)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza wss_echo");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee el puerto");
    let port: u16 = linea.trim().parse().unwrap_or_else(|_| panic!("puerto inválido: {linea:?}"));
    (child, port)
}

/// Un cliente TLS (confía en la CA de prueba) conectado a `localhost:port`, como `rustls::Stream`.
fn cliente_tls(port: u16) -> (ClientConnection, TcpStream) {
    let mut roots = RootCertStore::empty();
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    for cert in CertificateDer::pem_file_iter(&ca).expect("CA de prueba").flatten() {
        roots.add(cert).expect("añade CA");
    }
    let config = ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    let server_name = ServerName::try_from("localhost").unwrap();
    let conn = ClientConnection::new(Arc::new(config), server_name).expect("client conn");
    let sock = TcpStream::connect(("localhost", port)).expect("conecta");
    sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
    (conn, sock)
}

/// Trama de cliente WebSocket (texto/close, FIN) **enmascarada**, como exige el RFC 6455 §5.3.
fn trama_cliente(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [0x11u8, 0x22, 0x33, 0x44];
    let mut f = vec![0x80 | opcode, 0x80 | (payload.len() as u8)];
    f.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        f.push(b ^ mask[i % 4]);
    }
    f
}

/// Lee una trama del servidor (sin máscara, payload < 126) de un stream TLS: (opcode, payload).
fn leer_trama_tls(tls: &mut rustls::Stream<ClientConnection, TcpStream>) -> (u8, Vec<u8>) {
    let mut hdr = [0u8; 2];
    tls.read_exact(&mut hdr).expect("cabecera de trama");
    let opcode = hdr[0] & 0x0f;
    let len = (hdr[1] & 0x7f) as usize;
    let mut payload = vec![0u8; len];
    if len > 0 {
        tls.read_exact(&mut payload).expect("carga de trama");
    }
    (opcode, payload)
}

#[test]
fn echo_server_wss_handshake_y_tramas() {
    let (mut child, port) = lanzar_wss_server();
    let (mut conn, mut sock) = cliente_tls(port);
    let mut tls = rustls::Stream::new(&mut conn, &mut sock);

    // 1) Handshake de WebSocket sobre TLS: upgrade + verificar el accept canónico.
    let req = "GET /chat HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
               Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    tls.write_all(req.as_bytes()).expect("envía upgrade");
    let mut resp = Vec::new();
    let mut b = [0u8; 1];
    while !resp.ends_with(b"\r\n\r\n") {
        let n = tls.read(&mut b).expect("lee respuesta");
        assert!(n > 0, "el servidor cerró durante el handshake");
        resp.push(b[0]);
    }
    let resp = String::from_utf8_lossy(&resp);
    assert!(resp.contains("101 Switching Protocols"), "esperaba 101, got: {resp}");
    assert!(
        resp.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        "accept incorrecto, got: {resp}"
    );

    // 2) Eco de tramas de texto, cifradas por TLS.
    tls.write_all(&trama_cliente(0x1, b"hola wss")).expect("envía trama");
    let (op, payload) = leer_trama_tls(&mut tls);
    assert_eq!(op, 0x1, "esperaba texto");
    assert_eq!(payload, b"hola wss", "el eco no coincide");

    tls.write_all(&trama_cliente(0x1, b"otra mas")).expect("envía 2ª");
    let (_, payload2) = leer_trama_tls(&mut tls);
    assert_eq!(payload2, b"otra mas");

    // 3) Close.
    tls.write_all(&trama_cliente(0x8, b"")).expect("envía close");
    let (op_cierre, _) = leer_trama_tls(&mut tls);
    assert_eq!(op_cierre, 0x8, "esperaba close del servidor");

    let _ = child.kill();
    let _ = child.wait();
}
