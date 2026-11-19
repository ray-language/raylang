//! raygrpc #384 — `net.tls_accept_alpn` y `net.tls_alpn`: un servidor raylang anuncia protocolos
//! ALPN y dice cuál se negoció. El cliente es rustls (el CA de prueba de `tests/fixtures/`), que
//! ofrece su propia lista: gana la preferencia del SERVIDOR; un cliente sin ALPN conecta igual
//! (protocolo vacío) y uno que solo ofrece otros protocolos ve fallar el handshake.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore};

const CA_PEM: &str = include_str!("fixtures/tls_ca.pem");

/// El servidor: acepta UNA conexión, la envuelve con los protocolos del tercer argumento y pide
/// el protocolo negociado SIN I/O previa (`tls_alpn` conduce el handshake).
const SERVER: &str = r#"
import std/net;
import std/fs;

fn main() -> int {
    let a = args();
    let cert = fs.read_file(a[0]).unwrap();
    let key = fs.read_file(a[1]).unwrap();
    let protocols = if (a[2] == "") { [] } else { a[2].split(",") };
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    print(net.local_port(l));
    let c = net.tcp_accept(l).unwrap();
    let h = match (net.tls_accept_alpn(c, cert, key, protocols)) {
        Result.Ok(h) => h,
        Result.Err(e) => {
            print("accept err: " + e);
            return 0;
        },
    };
    match (net.tls_alpn(h)) {
        Result.Ok(p) => {
            print("alpn [" + p + "]");
            let _ = net.socket_write(h, "hi");
        },
        Result.Err(e) => print("alpn err"),
    }
    close(h);
    0
}
"#;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn write_server(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("tls_alpn_{tag}_{}.ray", std::process::id()));
    std::fs::write(&path, SERVER).expect("escribe el servidor");
    path
}

/// Lanza el servidor (`cmd` ya lleva el programa), conecta ofreciendo `offered` y devuelve lo que
/// negoció el cliente (`None` = el handshake falló) y la línea que imprimió el servidor.
fn round(mut cmd: Command, server_protocols: &str, offered: &[&str]) -> (Option<String>, String) {
    let mut child = cmd
        .args([fixture("tls_cert.pem"), fixture("tls_key.pem"), server_protocols.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza el servidor");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).expect("lee el puerto");
    let port: u16 = line.trim().parse().unwrap_or_else(|_| {
        let mut err = String::new();
        let _ = child.stderr.take().unwrap().read_to_string(&mut err);
        panic!("el servidor no imprimió el puerto: {line:?}\nstderr: {err}")
    });

    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(CA_PEM.as_bytes()) {
        roots.add(cert.expect("CA de prueba válido")).expect("añade el CA");
    }
    let mut config = ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    config.alpn_protocols = offered.iter().map(|p| p.as_bytes().to_vec()).collect();
    let mut conn = ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).expect("cliente");
    let mut sock = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    let mut negotiated = None;
    let mut failed = false;
    while conn.is_handshaking() {
        if conn.complete_io(&mut sock).is_err() {
            failed = true;
            break;
        }
    }
    if !failed {
        negotiated = Some(conn.alpn_protocol().map(|p| String::from_utf8_lossy(p).into_owned()).unwrap_or_default());
        let mut tls = rustls::Stream::new(&mut conn, &mut sock);
        let mut buf = [0u8; 8];
        let _ = tls.read(&mut buf);
        let _ = tls.flush();
    }
    drop(sock);
    let mut rest = String::new();
    out.read_to_string(&mut rest).expect("lee la salida");
    let status = child.wait().expect("espera al servidor");
    assert!(status.success(), "el servidor falló: {rest}");
    (negotiated, rest.trim().to_string())
}

fn check_engine(make: &dyn Fn() -> Command) {
    // La preferencia es la del servidor, no la del cliente.
    assert_eq!(round(make(), "h2,http/1.1", &["http/1.1", "h2"]), (Some("h2".to_string()), "alpn [h2]".to_string()));
    // Un cliente sin ALPN conecta; no se negocia nada.
    assert_eq!(round(make(), "h2", &[]), (Some(String::new()), "alpn []".to_string()));
    // Sin protocolos, el servidor no anuncia ALPN (lo que hace `tls_accept`).
    assert_eq!(round(make(), "", &["h2"]), (Some(String::new()), "alpn []".to_string()));
    // Sin protocolo en común, el handshake falla en los dos lados.
    assert_eq!(round(make(), "h2", &["http/1.1"]), (None, "alpn err".to_string()));
}

#[test]
fn tls_alpn_vm() {
    let path = write_server("vm");
    check_engine(&|| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_raylang"));
        c.arg("--vm").arg(&path);
        c
    });
}

#[test]
fn tls_alpn_interpreter() {
    let path = write_server("interp");
    check_engine(&|| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_raylang"));
        c.arg("--interp").arg(&path);
        c
    });
}

#[test]
fn tls_alpn_native() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        assert!(std::env::var_os("CI").is_none(), "rustc no disponible bajo CI: falso verde");
        eprintln!("saltando tls_alpn_native: rustc no disponible");
        return;
    }
    let src = write_server("native");
    let bin = std::env::temp_dir().join(format!("ray_tls_alpn_{}{}", std::process::id(), std::env::consts::EXE_SUFFIX));
    let build = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["build", src.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza build --native");
    assert!(build.status.success(), "build --native falló: {}", String::from_utf8_lossy(&build.stderr));
    check_engine(&|| Command::new(&bin));
    let _ = std::fs::remove_file(&bin);
}
