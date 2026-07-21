//! Prueba del servidor concurrente (M15.5): `tcp_accept`/`socket_read` ceden al scheduler de M12, así
//! un servidor con `spawn` atiende varias conexiones a la vez sobre un hilo. **Solo VM** (la
//! concurrencia es VM-only). La prueba de concurrencia es de **ordenación**, no de tiempos: se piden
//! ecos fuera del orden de conexión; un servidor secuencial bloqueante se quedaría atascado leyendo al
//! primero y nunca respondería al segundo (los clientes ponen read-timeout para fallar, no colgarse).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

// Servidor de eco concurrente que atiende EXACTAMENTE 2 conexiones (vía `scope`) y termina. Cada
// conexión se atiende en su propia fibra (`spawn`); accept/read ceden al scheduler.
const SERVER: &str = r#"
import std/net;
fn handle(conn: int) {
    match (net.socket_read(conn)) {
        Result.Ok(msg) => {
            match (net.socket_write(conn, "echo:" + msg)) {
                Result.Ok(_) => {},
                Result.Err(e) => eprint("write: " + e),
            }
        },
        Result.Err(e) => eprint("read: " + e),
    };
    close(conn);
}

fn main() -> int {
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            print(net.local_port(srv));
            scope(fn() {
                var i: int = 0;
                while (i < 2) {
                    match (net.tcp_accept(srv)) {
                        Result.Ok(conn) => { spawn(fn() { handle(conn) }); },
                        Result.Err(e) => { eprint("accept: " + e); i = 2; },
                    }
                    i = i + 1;
                }
            });
            close(srv);
            0
        },
        Result.Err(e) => { eprint("listen: " + e); 1 },
    }
}
"#;

#[test]
fn concurrent_server_serves_out_of_order() {
    let mut path = std::env::temp_dir();
    path.push("ray_srv_concurrente.ray");
    std::fs::File::create(&path).expect("crea").write_all(SERVER.as_bytes()).expect("escribe");

    // Solo VM: la concurrencia (spawn/scope) requiere la VM; el intérprete daría error limpio.
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&path)
        .stdout(Stdio::piped())
        .spawn()
        .expect("lanza servidor");

    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut line = String::new();
    reader.read_line(&mut line).expect("lee el port");
    let port: u16 = line.trim().parse().unwrap_or_else(|_| panic!("invalid port: {line:?}"));

    // Dos clientes; el SEGUNDO en conectarse pide su eco PRIMERO. Un servidor secuencial bloqueado
    // leyendo al primero nunca respondería → el read del segundo daría timeout y el test fallaría.
    let c1 = TcpStream::connect(("127.0.0.1", port)).expect("conecta c1");
    let c2 = TcpStream::connect(("127.0.0.1", port)).expect("conecta c2");

    // c2 (conectado de segundo) intercambia primero.
    let mut c2 = c2;
    c2.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout c2");
    c2.write_all(b"dos").expect("envía c2");
    let mut r2 = String::new();
    c2.read_to_string(&mut r2).expect("lee c2 (¿servidor no concurrente?)");
    assert_eq!(r2, "echo:dos", "el segundo client recibió su echo pese a what el primero no había enviado");

    // Ahora c1.
    let mut c1 = c1;
    c1.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout c1");
    c1.write_all(b"one").expect("envía c1");
    let mut r1 = String::new();
    c1.read_to_string(&mut r1).expect("lee c1");
    assert_eq!(r1, "echo:one");

    let status = child.wait().expect("espera servidor");
    assert_eq!(status.code(), Some(0), "el servidor terminó bien after 2 conexiones");
}
