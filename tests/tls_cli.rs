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
fn run_demo(flags: &[&str], url: &str) -> String {
    let demo = format!("{}/examples/web/https_demo.ray", env!("CARGO_MANIFEST_DIR"));
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
fn https_client_against_local_server_interpreter() {
    let port = launch_tls_server();
    let out = run_demo(&[], &format!("https://localhost:{port}/"));
    assert!(out.contains("status=200"), "esperaba 200, got: {out}");
    assert!(out.contains("body=hola-tls"), "body incorrect, got: {out}");
}

#[test]
fn https_client_against_local_server_vm() {
    let port = launch_tls_server();
    let out = run_demo(&["--vm"], &format!("https://localhost:{port}/"));
    assert!(out.contains("status=200"), "esperaba 200, got: {out}");
    assert!(out.contains("body=hola-tls"), "body incorrect, got: {out}");
}

// --- M19.4b: servidor TLS (`wss://`) — `examples/web/wss_echo.ray` ---
//
// El servidor wss en raylang corre en la VM (no bloqueante: la fibra cede mientras rustls hace el
// handshake/descifra). Lo atacamos con un cliente WebSocket-sobre-TLS escrito aquí en Rust.

/// Lanza `wss_echo.ray` (con el cert/clave de prueba) y devuelve su proceso + puerto efímero.
fn launch_wss_server() -> (Child, u16) {
    let demo = format!("{}/examples/web/wss_echo.ray", env!("CARGO_MANIFEST_DIR"));
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
    let mut port_line = String::new();
    reader.read_line(&mut port_line).expect("lee el port");
    let port: u16 = port_line.trim().parse().unwrap_or_else(|_| panic!("invalid port: {port_line:?}"));
    (child, port)
}

/// Un cliente TLS (confía en la CA de prueba) conectado a `localhost:port`, como `rustls::Stream`.
fn client_tls(port: u16) -> (ClientConnection, TcpStream) {
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
fn trama_client(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [0x11u8, 0x22, 0x33, 0x44];
    let mut f = vec![0x80 | opcode, 0x80 | (payload.len() as u8)];
    f.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        f.push(b ^ mask[i % 4]);
    }
    f
}

/// Lee una trama del servidor (sin máscara, payload < 126) de un stream TLS: (opcode, payload).
fn read_tls_frame(tls: &mut rustls::Stream<ClientConnection, TcpStream>) -> (u8, Vec<u8>) {
    let mut hdr = [0u8; 2];
    tls.read_exact(&mut hdr).expect("header de trama");
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
    let (mut child, port) = launch_wss_server();
    let (mut conn, mut sock) = client_tls(port);
    let mut tls = rustls::Stream::new(&mut conn, &mut sock);

    // 1) Handshake de WebSocket sobre TLS: upgrade + verificar el accept canónico.
    let req = "GET /chat HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
               Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    tls.write_all(req.as_bytes()).expect("envía upgrade");
    let mut resp = Vec::new();
    let mut b = [0u8; 1];
    while !resp.ends_with(b"\r\n\r\n") {
        let n = tls.read(&mut b).expect("lee response");
        assert!(n > 0, "el servidor cerró durante el handshake");
        resp.push(b[0]);
    }
    let resp = String::from_utf8_lossy(&resp);
    assert!(resp.contains("101 Switching Protocols"), "esperaba 101, got: {resp}");
    assert!(
        resp.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        "accept incorrect, got: {resp}"
    );

    // 2) Eco de tramas de texto, cifradas por TLS.
    tls.write_all(&trama_client(0x1, b"hello wss")).expect("envía trama");
    let (op, payload) = read_tls_frame(&mut tls);
    assert_eq!(op, 0x1, "esperaba text");
    assert_eq!(payload, b"hello wss", "el echo no coincide");

    tls.write_all(&trama_client(0x1, b"other mas")).expect("envía 2ª");
    let (_, payload2) = read_tls_frame(&mut tls);
    assert_eq!(payload2, b"other mas");

    // 3) Close.
    tls.write_all(&trama_client(0x8, b"")).expect("envía close");
    let (op_cierre, _) = read_tls_frame(&mut tls);
    assert_eq!(op_cierre, 0x8, "esperaba close del servidor");

    let _ = child.kill();
    let _ = child.wait();
}

// --- M56.3: servidor HTTPS (`serve_tls` de webserver.ray) — `examples/web/https_server_demo.ray` ---

/// Copia `webserver.ray` + el demo HTTPS (puerto 8443 → 0, efímero) a un temporal y lo lanza con el
/// cert/clave de prueba; devuelve el proceso + el puerto ("escuchando en el puerto N").
fn launch_https_server() -> (Child, u16) {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut dir = std::env::temp_dir();
    dir.push("ray_https_srv");
    std::fs::create_dir_all(&dir).expect("crea dir");
    std::fs::copy(format!("{root}/examples/web/webserver.ray"), dir.join("webserver.ray")).expect("copia webserver");
    let demo = std::fs::read_to_string(format!("{root}/examples/web/https_server_demo.ray")).expect("lee demo");
    std::fs::write(dir.join("main.ray"), demo.replace("8443", "0")).expect("escribe demo");

    let cert = format!("{root}/tests/fixtures/tls_cert.pem");
    let key = format!("{root}/tests/fixtures/tls_key.pem");
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(dir.join("main.ray"))
        .arg(&cert)
        .arg(&key)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza https_server_demo");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut port_line = String::new();
    reader.read_line(&mut port_line).expect("lee el port");
    let port: u16 = port_line.trim().rsplit(' ').next().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no se pudo leer el port de: {port_line:?}"));
    (child, port)
}

/// Lee una respuesta HTTP del stream TLS hasta tener las cabeceras + `esperado` en el cuerpo (o EOF).
fn read_tls_response(tls: &mut rustls::Stream<ClientConnection, TcpStream>, expected: &str) -> String {
    let mut resp = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match tls.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                resp.extend_from_slice(&buf[..n]);
                let s = String::from_utf8_lossy(&resp);
                if s.contains("\r\n\r\n") && s.contains(expected) {
                    break;
                }
            }
            // El servidor cierra el socket tras responder (puede no enviar close_notify): lo ya leído vale.
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&resp).into_owned()
}

#[test]
fn https_server_serves_about_over_tls() {
    let (mut child, port) = launch_https_server();

    // Petición HTTPS normal (con query: compone con M56.2 — la ruta casa igual).
    let (mut conn, mut sock) = client_tls(port);
    let mut tls = rustls::Stream::new(&mut conn, &mut sock);
    tls.write_all(b"GET /hola?x=1 HTTP/1.1\r\nHost: localhost\r\n\r\n").expect("envía petición");
    let resp = read_tls_response(&mut tls, "hola https");
    assert!(resp.contains("200 OK"), "esperaba 200 OK, got: {resp}");
    assert!(resp.contains("hola https"), "esperaba el body, got: {resp}");

    // Segunda connection: 404 (el servidor sigue vivo tras la primera).
    let (mut conn2, mut sock2) = client_tls(port);
    let mut tls2 = rustls::Stream::new(&mut conn2, &mut sock2);
    tls2.write_all(b"GET /nope HTTP/1.1\r\nHost: localhost\r\n\r\n").expect("envía 2ª");
    let resp2 = read_tls_response(&mut tls2, "Not Found");
    assert!(resp2.contains("404 Not Found"), "esperaba 404, got: {resp2}");

    // Un cliente que habla HTTP PLANO contra el puerto TLS falla su conexión sin tumbar el servidor.
    {
        let mut plain = TcpStream::connect(("127.0.0.1", port)).expect("conecta plano");
        plain.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let _ = plain.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        let mut discard = Vec::new();
        let _ = plain.read_to_end(&mut discard); // el handshake TLS falla → el servidor cierra
    }
    let (mut conn3, mut sock3) = client_tls(port);
    let mut tls3 = rustls::Stream::new(&mut conn3, &mut sock3);
    tls3.write_all(b"GET /hola HTTP/1.1\r\nHost: localhost\r\n\r\n").expect("envía 3ª");
    let resp3 = read_tls_response(&mut tls3, "hola https");
    assert!(resp3.contains("200 OK"), "el servidor must follow live after un client no-TLS, got: {resp3}");

    let _ = child.kill();
    let _ = child.wait();
}
