//! Prueba del cliente UDP (`examples/web/udp.ray`, M20.8). Se levanta un **servidor UDP de eco** en
//! Rust (recibe un datagrama, lo devuelve en MAYÚSCULAS al remitente) y se corre `udp_demo.ray` contra
//! él por ambos motores. Verifica el round-trip de datagramas + el remitente (el servidor responde a
//! la dirección de origen que `recv_from` reporta).

use std::net::UdpSocket;
use std::process::Command;
use std::thread;

/// Servidor UDP de eco-mayúsculas: atiende un datagrama y responde a su remitente. Devuelve el puerto.
fn toy_udp_server() -> u16 {
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind udp");
    let port = sock.local_addr().expect("addr").port();
    thread::spawn(move || {
        let mut buf = [0u8; 65536];
        // Atiende varios datagramas (uno por ejecución del demo: intérprete y VM comparten servidor
        // si se reusara, pero aquí cada test crea el suyo; el bucle tolera reintentos).
        loop {
            match sock.recv_from(&mut buf) {
                Ok((n, origen)) => {
                    let resp = String::from_utf8_lossy(&buf[..n]).to_uppercase();
                    let _ = sock.send_to(resp.as_bytes(), origen);
                }
                Err(_) => return,
            }
        }
    });
    port
}

fn run(flags: &[&str], port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/udp_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg(port.to_string())
        .output()
        .expect("ejecuta udp_demo.ray");
    assert!(
        out.status.success(),
        "udp_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

const EXPECTED: &[&str] = &["8", "HOLA UDP"];

#[test]
fn udp_echo_interpreter() {
    let port = toy_udp_server();
    assert_eq!(run(&[], port), EXPECTED);
}

#[test]
fn udp_echo_vm() {
    let port = toy_udp_server();
    assert_eq!(run(&["--vm"], port), EXPECTED);
}

// ---------------------------------------------------------------------------------------------
// M121 — timeout de lectura UDP (`net.set_read_timeout` aplica a sockets UDP).
// ---------------------------------------------------------------------------------------------

/// Corre `udp_timeout_demo.ray` con los flags dados y devuelve sus líneas de stdout.
fn run_timeout_demo(flags: &[&str], port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/udp_timeout_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg(port.to_string())
        .output()
        .expect("ejecuta udp_timeout_demo.ray");
    assert!(
        out.status.success(),
        "udp_timeout_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect()
}

/// Fase 1: sin datagrama, la espera vence con el error ESTABLE "read timeout" (~el plazo, no
/// para siempre — antes un datagrama perdido colgaba la fibra sin remedio). Fase 2: con el
/// timeout puesto, un dato que llega ANTES del plazo se recibe normal.
const TIMEOUT_EXPECTED: &[&str] = &["err: read timeout", "true", "HOLA PLAZO"];

#[test]
fn udp_read_timeout_interpreter() {
    let port = toy_udp_server();
    assert_eq!(run_timeout_demo(&[], port), TIMEOUT_EXPECTED);
}

#[test]
fn udp_read_timeout_vm() {
    let port = toy_udp_server();
    assert_eq!(run_timeout_demo(&["--vm"], port), TIMEOUT_EXPECTED);
}

/// El binario NATIVO (sabor default, fibras): mismo programa, misma salida byte a byte. Compila
/// con `ray build --native` y corre contra su propio servidor de eco.
#[test]
fn udp_read_timeout_native() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        assert!(std::env::var_os("CI").is_none(), "rustc no disponible bajo CI: falso verde");
        eprintln!("saltando udp_read_timeout_native: rustc no disponible");
        return;
    }
    let demo = format!("{}/examples/web/udp_timeout_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let bin = std::env::temp_dir().join(format!("ray_udp_timeout_{}{}", std::process::id(), std::env::consts::EXE_SUFFIX));
    let build = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["build", &demo, "--native", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza build --native");
    assert!(build.status.success(), "build --native falló: {}", String::from_utf8_lossy(&build.stderr));
    let port = toy_udp_server();
    let out = Command::new(&bin).arg(port.to_string()).output().expect("corre el binario nativo");
    let _ = std::fs::remove_file(&bin);
    assert!(out.status.success(), "el binario nativo falló: {}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    assert_eq!(lines, TIMEOUT_EXPECTED, "el nativo diverge de la VM en el timeout UDP");
}
