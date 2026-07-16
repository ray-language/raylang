//! Cesión cooperativa en `socket_write` (post-M19.4). Prueba **conductual**: si una fibra escribe un
//! blob gigantesco a un cliente que **no drena**, el buffer de envío se llena y la escritura bloquearía.
//! Antes giraba (`yield_now`) acaparando el único hilo del scheduler; ahora **cede la fibra** hasta que
//! el socket sea escribible. Lo verificamos sirviendo DOS conexiones a la vez: el cliente 1 no lee (su
//! escritura se aparca), y el cliente 2 debe recibir su blob completo igualmente. Con el viejo busy-spin,
//! el hilo se quedaría atascado escribiendo al cliente 1 y el cliente 2 nunca sería atendido (timeout).
//! Solo VM (concurrencia). Mismo molde que `webserver_cli`/`concurrency_net_cli`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

const BLOB: usize = 8_000_000; // > buffers del SO en loopback → la escritura bloquea si el peer no lee

/// Servidor acotado: atiende 2 conexiones, cada una en su fibra; manda "READY\n" y luego un blob de
/// `BLOB` 'X'. La escritura del blob a un cliente que no lee se APARCA (cede), no gira.
const DRIVER: &str = r#"
import std/net;
fn serve(conn: int) {
    match (net.socket_write(conn, "READY\n")) { Result.Ok(_) => {}, Result.Err(e) => eprint(e) }
    let blob = "X".repeat(8000000);
    match (net.socket_write(conn, blob)) { Result.Ok(_) => {}, Result.Err(e) => eprint(e) }
    close(conn);
}
fn main() -> int {
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            print(to_string(net.local_port(srv)));
            scope(fn() {
                var i = 0;
                while (i < 2) {
                    match (net.tcp_accept(srv)) {
                        Result.Ok(conn) => { spawn(fn() { serve(conn) }); },
                        Result.Err(e) => { eprint(e); i = 2; },
                    }
                    i = i + 1;
                }
            });
            close(srv);
            0
        },
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;

/// Escribe el driver a un temporal, lo lanza con `--vm` y devuelve (proceso, puerto).
fn launch() -> (std::process::Child, u16) {
    let mut dir = std::env::temp_dir();
    dir.push("ray_sockwrite");
    std::fs::create_dir_all(&dir).expect("crea dir");
    let path = dir.join("server.ray");
    std::fs::File::create(&path).unwrap().write_all(DRIVER.as_bytes()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm").arg(&path)
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("lanza servidor");
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee port");
    let port = linea.trim().parse().unwrap_or_else(|_| panic!("port: {linea:?}"));
    (child, port)
}

/// Lee exactamente la línea "READY\n" (6 octetos) de un stream.
fn leer_ready(s: &mut TcpStream) {
    let mut b = [0u8; 6];
    s.read_exact(&mut b).expect("READY");
    assert_eq!(&b, b"READY\n");
}

#[test]
fn socket_write_cede_y_no_acapara_el_scheduler() {
    let (mut child, port) = launch();

    // Cliente 1: lee "READY", luego NO drena el blob → su escritura en el servidor se aparca.
    let mut c1 = TcpStream::connect(("127.0.0.1", port)).expect("conecta c1");
    c1.set_read_timeout(Some(Duration::from_secs(15))).ok();
    leer_ready(&mut c1);

    // Cliente 2: debe ser atendido POR COMPLETO aunque c1 tenga al servidor bloqueado escribiendo.
    let mut c2 = TcpStream::connect(("127.0.0.1", port)).expect("conecta c2");
    c2.set_read_timeout(Some(Duration::from_secs(15))).ok();
    leer_ready(&mut c2);
    let mut recibido = 0usize;
    let mut buf = [0u8; 65536];
    while recibido < BLOB {
        match c2.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => recibido += n,
            Err(e) => panic!("c2 no recibió su blob complete (cesión rota → scheduler acaparado): {e}"),
        }
    }
    assert_eq!(recibido, BLOB, "el client 2 must recibir su blob entero mientras el 1 está aparcado");

    // Ahora c1 drena su blob → su escritura aparcada termina → el scope une y el servidor sale limpio.
    let mut total1 = 0usize;
    while total1 < BLOB {
        match c1.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => total1 += n,
            Err(_) => break,
        }
    }
    assert_eq!(total1, BLOB, "el client 1 también recibe su blob al drenar");

    let _ = child.wait();
}
