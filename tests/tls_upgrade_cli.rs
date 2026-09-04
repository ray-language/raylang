//! Diferido TLS — `tls_upgrade` (STARTTLS de cliente): un socket TCP plano ya conectado se
//! envuelve en una sesión TLS de cliente, conservando el handle. El test levanta un servidor
//! STARTTLS de juguete (rustls, cert autofirmado de `tests/fixtures/`): fase EN CLARO
//! ("STARTTLS\n" → "GO\n") y luego handshake TLS sobre el mismo socket + eco cifrado. El cliente
//! raylang confía en la CA de prueba vía `SSL_CERT_FILE`. Ambos motores, mismo stdout.

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

/// Servidor STARTTLS de una conexión: lee "STARTTLS\n" en claro, responde "GO\n", y SOLO entonces
/// arranca TLS de servidor sobre el mismo socket; bajo TLS lee una línea y responde "hola-seguro\n".
fn launch_starttls_server() -> u16 {
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
        // Fase en claro.
        let mut buf = [0u8; 64];
        let n = sock.read(&mut buf).expect("lee STARTTLS");
        assert!(&buf[..n] == b"STARTTLS\n", "esperaba STARTTLS, llegó {:?}", &buf[..n]);
        sock.write_all(b"GO\n").expect("responde GO");
        // Fase TLS (el ClientHello del cliente llega tras nuestro GO).
        let mut conn = ServerConnection::new(config).expect("server conn");
        let mut tls = rustls::Stream::new(&mut conn, &mut sock);
        let mut req = [0u8; 64];
        let m = tls.read(&mut req).expect("lee bajo TLS");
        assert!(&req[..m] == b"hello\n", "esperaba hello bajo TLS, llegó {:?}", &req[..m]);
        tls.write_all(b"hello-seguro\n").expect("responde bajo TLS");
        let _ = tls.flush();
        conn.send_close_notify();
        let _ = conn.complete_io(&mut sock);
    });

    port
}

const CLIENT: &str = r#"import std/net;

fn main() -> int {
    let a = args();
    let port = match (parse_int(a[0])) {
        Option.Some(p) => p,
        Option.None => { return 64; },
    };
    var h = match (net.tcp_connect("localhost", port)) {
        Result.Ok(x) => x,
        Result.Err(e) => { print("conexión: " + e); return 1; },
    };
    // Fase en claro: negociar el upgrade.
    let _ = net.socket_write_bytes(h, "STARTTLS\n".to_bytes());
    let go = match (net.socket_read_bytes(h)) {
        Result.Ok(b) => match (from_utf8(b)) {
            Result.Ok(s) => s,
            Result.Err(_) => { return 1; },
        },
        Result.Err(e) => { print("lectura: " + e); return 1; },
    };
    print("claro: " + go.trim());
    // STARTTLS: el MISMO handle pasa a ser una sesión TLS de cliente (verifica el cert contra
    // "localhost", firmado por la CA de prueba de SSL_CERT_FILE).
    h = match (net.tls_upgrade(h, "localhost")) {
        Result.Ok(x) => x,
        Result.Err(e) => { print("upgrade: " + e); return 1; },
    };
    let _ = net.socket_write_bytes(h, "hello\n".to_bytes());
    let seguro = match (net.socket_read_bytes(h)) {
        Result.Ok(b) => match (from_utf8(b)) {
            Result.Ok(s) => s,
            Result.Err(_) => { return 1; },
        },
        Result.Err(e) => { print("lectura tls: " + e); return 1; },
    };
    print("tls: " + seguro.trim());
    // Upgrade de un handle que YA es TLS → error limpio, como valor.
    match (net.tls_upgrade(h, "localhost")) {
        Result.Ok(_) => { print("no debería"); return 1; },
        Result.Err(e) => { print("double: " + e); },
    }
    close(h);
    0
}
"#;

const EXPECTED: &str = "claro: GO\ntls: hello-seguro\ndouble: handle 1 is not a plain TCP socket\n";

fn run(port: u16, flags: &[&str]) -> String {
    let dir = std::env::temp_dir().join(format!("ray_tls_upgrade_{}", flags.join("_")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("main.ray");
    std::fs::write(&main, CLIENT).unwrap();
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&main)
        .arg(port.to_string())
        .env("SSL_CERT_FILE", &ca)
        .output()
        .expect("lanza raylang");
    assert!(
        out.status.success(),
        "runs sin error\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn starttls_upgrade_interpreter() {
    let port = launch_starttls_server();
    assert_eq!(run(port, &["--interp"]), EXPECTED);
}

#[test]
fn starttls_upgrade_vm() {
    let port = launch_starttls_server();
    assert_eq!(run(port, &["--vm"]), EXPECTED);
}

/// M96g: STARTTLS en el backend NATIVO — el handle pasa de Tcp a Tls a mitad de conexión
/// (`tls_upgrade`). Cubre exactamente el caso que la caché thread-local de `__ray_tls_get`
/// (M96g) debe manejar bien: lecturas EN CLARO antes del upgrade (caché negativa: "no es TLS"),
/// el upgrade (debe actualizar la caché al vuelo, no dejarla stale), y lecturas TLS después
/// (deben usar la sesión TLS nueva, no la caché negativa de antes). Sin esto, el corpus nativo
/// no ejercía `tls_upgrade` en absoluto (`starttls_upgrade_{interpreter,vm}` son los únicos
/// tests de este archivo, ninguno nativo).
#[test]
fn starttls_upgrade_native() {
    if Command::new("cargo").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando starttls_upgrade_native: cargo no disponible");
        return;
    }
    let port = launch_starttls_server();
    let dir = std::env::temp_dir().join("ray_tls_upgrade_native");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("main.ray");
    std::fs::write(&main, CLIENT).unwrap();
    let bin = dir.join(format!("client_bin{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["build", main.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza build --native");
    assert!(
        build.status.success(),
        "build --native starttls ok\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(&bin)
        .arg(port.to_string())
        .env("SSL_CERT_FILE", &ca)
        .output()
        .expect("corre el binario nativo");
    assert!(
        out.status.success(),
        "corre sin error\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), EXPECTED, "STARTTLS nativo ≡ VM/intérprete");
}
