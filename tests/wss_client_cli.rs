//! Prueba del cliente WebSocket sobre TLS (`wss://`, M24+). Reusa el echo server `wss_echo.ray`
//! (servidor raylang con `tls_accept`, solo VM) lanzado con el cert/clave de `tests/fixtures/`, y corre
//! `websocket_client_demo.ray` en modo `wss` (`connect_tls`) confiando en la CA de prueba vía
//! `SSL_CERT_FILE`. Cliente raylang ↔ servidor raylang, todo cifrado, por ambos motores.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};

/// Lanza `wss_echo.ray` con el cert/clave de prueba; devuelve el proceso + el puerto efímero.
fn lanzar_servidor() -> (Child, u16) {
    let raiz = env!("CARGO_MANIFEST_DIR");
    let echo = format!("{raiz}/examples/web/wss_echo.ray");
    let cert = format!("{raiz}/tests/fixtures/tls_cert.pem");
    let key = format!("{raiz}/tests/fixtures/tls_key.pem");
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&echo)
        .arg(&cert)
        .arg(&key)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("lanza wss_echo");

    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee puerto");
    let port: u16 = linea.trim().parse().unwrap_or_else(|_| panic!("puerto inválido: {linea:?}"));
    std::thread::spawn(move || {
        let mut sumidero = Vec::new();
        let _ = reader.read_to_end(&mut sumidero);
    });
    (child, port)
}

fn correr_cliente(flags: &[&str], port: u16) -> Vec<String> {
    let raiz = env!("CARGO_MANIFEST_DIR");
    let demo = format!("{raiz}/examples/web/websocket_client_demo.ray");
    let ca = format!("{raiz}/tests/fixtures/tls_ca.pem"); // el cliente confía en la CA de prueba
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg(port.to_string())
        .arg("wss")
        .env("SSL_CERT_FILE", &ca)
        .output()
        .expect("ejecuta el cliente wss");
    assert!(
        out.status.success(),
        "cliente wss falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

const ESPERADO: &[&str] = &["hola", "mundo", "raylang ☃ unicode"];

fn caso(flags: &[&str]) {
    let (mut servidor, port) = lanzar_servidor();
    let salida = correr_cliente(flags, port);
    let _ = servidor.kill();
    let _ = servidor.wait();
    assert_eq!(salida, ESPERADO);
}

#[test]
fn cliente_wss_eco_interprete() {
    caso(&[]);
}

#[test]
fn cliente_wss_eco_vm() {
    caso(&["--vm"]);
}
