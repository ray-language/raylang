//! Pruebas del cliente TCP (M15.2) sobre el binario. La red no es determinista para el oráculo,
//! así que se prueba por subproceso, contra un **servidor TCP de juguete en el propio Rust** (un
//! hilo que acepta una conexión, lee la petición y responde). El `.ray` se conecta por el puerto
//! efímero que el SO asignó (sustituido en el fuente). Se verifica en ambos motores.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;

/// Escribe `src` en un `.ray` temporal, ejecuta el binario (con `--vm` opcional) y devuelve
/// `(stdout, código)`.
fn run(name: &str, src: &str, vm: bool) -> (String, i32) {
    let mut path = std::env::temp_dir();
    path.push(format!("{name}.ray"));
    std::fs::File::create(&path).expect("crea").write_all(src.as_bytes()).expect("escribe");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let out = cmd.arg(&path).output().expect("lanza raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

/// Levanta un servidor de eco TCP de juguete en un puerto efímero: acepta UNA conexión, lee la
/// petición y responde `"pong:" + <lo recibido, sin espacios>`. Devuelve el puerto. El hilo se
/// detiene solo al terminar (la conexión se cierra al salir del `move ||`).
fn toy_echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let resp = format!("pong:{}", req.trim());
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

const CLIENTE: &str = r#"
import std/net;
fn main() -> int {
    match (net.tcp_connect("127.0.0.1", __PORT__)) {
        Result.Ok(h) => {
            match (net.socket_write(h, "ping")) {
                Result.Ok(_) => {
                    match (net.socket_read(h)) {
                        Result.Ok(s) => print(s),
                        Result.Err(e) => print("read err: " + e),
                    }
                },
                Result.Err(e) => print("write err: " + e),
            }
            close(h);
            0
        },
        Result.Err(e) => { print("connect err: " + e); 1 },
    }
}
"#;

#[test]
fn tcp_client_exchanges_with_a_server() {
    for vm in [false, true] {
        // Un servidor nuevo por ejecución (cada uno acepta una sola conexión).
        let port = toy_echo_server();
        let src = CLIENTE.replace("__PORT__", &port.to_string());
        let (out, code) = run("ray_tcp_ok", &src, vm);
        assert_eq!(out.trim(), "pong:ping", "intercambio TCP (vm={vm}): {out}");
        assert_eq!(code, 0, "output 0 (vm={vm})");
    }
}

/// El `.ray` es el SERVIDOR (M15.3): escucha en puerto 0, imprime el puerto, acepta UNA conexión,
/// lee y responde con eco. El test lee el puerto de su stdout EN VIVO (el servidor bloquea en accept,
/// así que no se puede usar `.output()` que espera a que termine) y se conecta como cliente.
const SERVIDOR: &str = r#"
import std/net;
fn main() -> int {
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            print(net.local_port(srv));            // primera línea: el puerto efímero asignado
            match (net.tcp_accept(srv)) {
                Result.Ok(conn) => {
                    match (net.socket_read(conn)) {
                        Result.Ok(msg) => {
                            match (net.socket_write(conn, "echo:" + msg)) {
                                Result.Ok(_) => 0,
                                Result.Err(e) => { eprint("write: " + e); 1 },
                            }
                        },
                        Result.Err(e) => { eprint("read: " + e); 1 },
                    };
                    close(conn);
                    0
                },
                Result.Err(e) => { eprint("accept: " + e); 1 },
            };
            close(srv);
            0
        },
        Result.Err(e) => { eprint("listen: " + e); 1 },
    }
}
"#;

#[test]
fn tcp_server_accepts_and_responds() {
    for vm in [false, true] {
        // Escribir el servidor a un temporal y lanzarlo con stdout en pipe (lo leeremos en vivo).
        let mut path = std::env::temp_dir();
        path.push("ray_tcp_srv.ray");
        std::fs::File::create(&path).expect("crea").write_all(SERVIDOR.as_bytes()).expect("escribe");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
        if vm {
            cmd.arg("--vm");
        }
        let mut child = cmd.arg(&path).stdout(Stdio::piped()).spawn().expect("lanza servidor");

        // Leer el puerto (println! es line-buffered → se vacía con el salto, aunque el proceso siga).
        let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
        let mut line = String::new();
        reader.read_line(&mut line).expect("lee el port");
        let port: u16 = line.trim().parse().unwrap_or_else(|_| panic!("port inválido (vm={vm}): {line:?}"));

        // Conectar como cliente, escribir, y leer hasta que el servidor cierre.
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
        stream.write_all(b"hello").expect("escribe");
        let mut resp = String::new();
        stream.read_to_string(&mut resp).expect("lee response");
        assert_eq!(resp, "echo:hello", "el servidor hizo echo (vm={vm})");

        let status = child.wait().expect("espera al servidor");
        assert_eq!(status.code(), Some(0), "el servidor terminó bien (vm={vm})");
    }
}

#[test]
fn tcp_connect_fails_a_un_port_closed() {
    // Puerto efímero reservado y soltado: nadie escucha → la conexión debe ser rechazada.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("addr").port()
        // `l` se cierra aquí al salir del bloque.
    };
    for vm in [false, true] {
        let src = CLIENTE.replace("__PORT__", &port.to_string());
        let (out, code) = run("ray_tcp_err", &src, vm);
        assert!(out.contains("connect err"), "conexión rechazada (vm={vm}): {out}");
        assert_eq!(code, 1, "output 1 en error (vm={vm})");
    }
}
