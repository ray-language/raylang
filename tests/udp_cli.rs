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

const ESPERADO: &[&str] = &["8", "HOLA UDP"];

#[test]
fn udp_echo_interpreter() {
    let port = toy_udp_server();
    assert_eq!(run(&[], port), ESPERADO);
}

#[test]
fn udp_echo_vm() {
    let port = toy_udp_server();
    assert_eq!(run(&["--vm"], port), ESPERADO);
}
