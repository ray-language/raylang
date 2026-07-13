//! Prueba del cliente WebSocket (`examples/web/websocket_client.ray`, M24). Se lanza el echo server
//! `websocket_echo.ray` (servidor raylang, solo VM) y se corre `websocket_client_demo.ray` contra él
//! por ambos motores → un cliente WebSocket de raylang hablando con un servidor de raylang: handshake
//! (con Sec-WebSocket-Accept verificado), tramas de cliente enmascaradas, eco con UTF-8 multibyte.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};

/// Lanza `websocket_echo.ray` con `--vm`; devuelve el proceso + el puerto efímero (1.ª línea de stdout).
fn launch_servidor() -> (Child, u16) {
    let echo = format!("{}/examples/web/websocket_echo.ray", env!("CARGO_MANIFEST_DIR"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&echo)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("lanza websocket_echo");

    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee port");
    let port: u16 = linea.trim().parse().unwrap_or_else(|_| panic!("port inválido: {linea:?}"));
    // Drena el resto del stdout del servidor.
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = reader.read_to_end(&mut sink);
    });
    (child, port)
}

fn run_client(flags: &[&str], port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/websocket_client_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg(port.to_string())
        .output()
        .expect("ejecuta websocket_client_demo.ray");
    assert!(
        out.status.success(),
        "client WS falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

const ESPERADO: &[&str] = &["hola", "mundo", "raylang ☃ unicode"];

fn case(flags: &[&str]) {
    let (mut servidor, port) = launch_servidor();
    let output = run_client(flags, port);
    let _ = servidor.kill();
    let _ = servidor.wait();
    assert_eq!(output, ESPERADO);
}

#[test]
fn client_ws_echo_interpreter() {
    case(&[]);
}

#[test]
fn client_ws_echo_vm() {
    case(&["--vm"]);
}
