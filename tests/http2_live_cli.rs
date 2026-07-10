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

// --- M58.3: flow control + PING + RST contra servidores de juguete más exigentes ---

/// Lee UN frame HTTP/2 del stream TLS: (tipo, flags, stream_id, payload).
fn leer_frame_h2(tls: &mut rustls::Stream<ServerConnection, std::net::TcpStream>) -> Option<(u8, u8, u32, Vec<u8>)> {
    let mut hdr = [0u8; 9];
    let mut off = 0;
    while off < 9 {
        match tls.read(&mut hdr[off..]) {
            Ok(0) => return None,
            Ok(n) => off += n,
            Err(_) => return None,
        }
    }
    let len = ((hdr[0] as usize) << 16) | ((hdr[1] as usize) << 8) | hdr[2] as usize;
    let (ftype, flags) = (hdr[3], hdr[4]);
    let stream = ((hdr[5] as u32 & 0x7f) << 24) | ((hdr[6] as u32) << 16) | ((hdr[7] as u32) << 8) | hdr[8] as u32;
    let mut payload = vec![0u8; len];
    let mut p = 0;
    while p < len {
        match tls.read(&mut payload[p..]) {
            Ok(0) => return None,
            Ok(n) => p += n,
            Err(_) => return None,
        }
    }
    Some((ftype, flags, stream, payload))
}

fn tls_config_h2() -> Arc<ServerConfig> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(CERT_PEM.as_bytes())
        .collect::<Result<_, _>>()
        .expect("cert de prueba");
    let key = PrivateKeyDer::from_pem_slice(KEY_PEM.as_bytes()).expect("clave de prueba");
    let mut cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("config servidor");
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(cfg)
}

/// Servidor h2 que envía `total` octetos de DATA respetando el flow control REAL: gasta la
/// ventana inicial (65535) y solo sigue cuando el cliente concede crédito con WINDOW_UPDATE.
/// También manda un PING a mitad y EXIGE su ACK antes de continuar. Sin las dos cosas del
/// cliente (M58.3), este servidor se queda esperando y el test falla por timeout.
fn lanzar_servidor_h2_grande(total: usize) -> u16 {
    let config = tls_config_h2();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            sock.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
            let mut conn = ServerConnection::new(config).expect("server conn");
            let mut tls = rustls::Stream::new(&mut conn, &mut sock);
            // Preface del cliente (24 octetos) + frames hasta ver su HEADERS.
            let mut preface = [0u8; 24];
            let mut off = 0;
            while off < 24 {
                match tls.read(&mut preface[off..]) {
                    Ok(0) => return,
                    Ok(n) => off += n,
                    Err(_) => return,
                }
            }
            loop {
                match leer_frame_h2(&mut tls) {
                    Some((1, _, _, _)) => break, // HEADERS de la petición
                    Some(_) => {}
                    None => return,
                }
            }
            // SETTINGS + HEADERS de respuesta (:status 200).
            let _ = tls.write_all(&frame(4, 0, 0, &[]));
            let _ = tls.write_all(&frame(1, 4, 1, &[0x88]));
            // DATA respetando la ventana: crédito inicial 65535; WINDOW_UPDATE lo repone.
            let mut credito: i64 = 65535;
            let mut enviados = 0usize;
            let chunk = vec![b'x'; 16384];
            let mut ping_enviado = false;
            let mut ping_ack = false;
            while enviados < total {
                let n = chunk.len().min(total - enviados);
                let fin = enviados + n >= total;
                // Sin crédito (o último chunk con el ACK del PING pendiente): lee frames del
                // cliente hasta poder seguir — EXIGE los WINDOW_UPDATE y el ACK de M58.3.
                if (n as i64) > credito || (fin && ping_enviado && !ping_ack) {
                    match leer_frame_h2(&mut tls) {
                        Some((8, _, _, p)) if p.len() == 4 => {
                            let inc = ((p[0] as i64 & 0x7f) << 24) | ((p[1] as i64) << 16) | ((p[2] as i64) << 8) | p[3] as i64;
                            credito += inc / 2; // llegan por duplicado (conexión + stream): media cuenta
                        }
                        Some((6, flags, _, _)) if flags & 1 == 1 => ping_ack = true,
                        Some(_) => {}
                        None => return,
                    }
                    continue;
                }
                let _ = tls.write_all(&frame(0, if fin { 1 } else { 0 }, 1, &chunk[..n]));
                enviados += n;
                credito -= n as i64;
                // A mitad de la transferencia, sonda de vida: PING que exige ACK.
                if !ping_enviado && enviados > total / 2 {
                    let _ = tls.write_all(&frame(6, 0, 0, &[1, 2, 3, 4, 5, 6, 7, 8]));
                    ping_enviado = true;
                }
            }
            // Drena hasta el EOF del cliente (cierra él tras END_STREAM): si el servidor soltara
            // el socket con datos sin leer (los últimos WINDOW_UPDATE), el RST cortaría al cliente.
            while leer_frame_h2(&mut tls).is_some() {}
        }
    });
    port
}

/// Servidor h2 que resetea el stream 1 con RST_STREAM (código 8 = CANCEL) tras el HEADERS.
fn lanzar_servidor_h2_rst() -> u16 {
    let config = tls_config_h2();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut conn = ServerConnection::new(config).expect("server conn");
            let mut tls = rustls::Stream::new(&mut conn, &mut sock);
            let mut buf = [0u8; 4096];
            let _ = tls.read(&mut buf);
            let mut out = Vec::new();
            out.extend_from_slice(&frame(4, 0, 0, &[]));
            out.extend_from_slice(&frame(1, 4, 1, &[0x88]));
            out.extend_from_slice(&frame(3, 0, 1, &[0, 0, 0, 8])); // RST_STREAM código 8 (CANCEL)
            let _ = tls.write_all(&out);
            let _ = tls.flush();
            // Mantén el socket un momento (que el cliente lea el RST, no un EOF).
            thread::sleep(std::time::Duration::from_millis(500));
        }
    });
    port
}

fn correr_len(port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/http2_len_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&demo)
        .arg("localhost")
        .arg(port.to_string())
        .env("SSL_CERT_FILE", &ca)
        .output()
        .expect("ejecuta http2_len_demo.ray");
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect()
}

#[test]
fn http2_get_respuesta_grande_con_flow_control() {
    // M58.3: 200 000 octetos > la ventana inicial (65535). Sin los WINDOW_UPDATE del cliente el
    // servidor se pararía a los 64 KiB (y sin el ACK del PING, se quedaría esperándolo).
    let port = lanzar_servidor_h2_grande(200_000);
    let lineas = correr_len(port);
    assert_eq!(lineas, vec!["status: 200".to_string(), "len: 200000".to_string()]);
}

#[test]
fn http2_get_rst_stream_es_error_con_causa() {
    // M58.3: un RST_STREAM ya no deja al cliente leyendo hasta EOF: es un Err con el código.
    let port = lanzar_servidor_h2_rst();
    let lineas = correr_len(port);
    assert_eq!(lineas.len(), 1, "esperaba solo la línea de error: {lineas:?}");
    assert!(
        lineas[0].contains("RST_STREAM") && lineas[0].contains("código 8"),
        "esperaba el error de RST con su código, got: {lineas:?}"
    );
}
