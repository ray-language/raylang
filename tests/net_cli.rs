//! Pruebas del cliente TCP (M15.2) sobre el binario. La red no es determinista para el oráculo,
//! así que se prueba por subproceso, contra un **servidor TCP de juguete en el propio Rust** (un
//! hilo que acepta una conexión, lee la petición y responde). El `.ray` se conecta por el puerto
//! efímero que el SO asignó (sustituido en el fuente). Se verifica en ambos motores.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
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
fn main() -> int {
    match (tcp_connect("127.0.0.1", __PORT__)) {
        Result.Ok(h) => {
            match (socket_write(h, "ping")) {
                Result.Ok(_) => {
                    match (socket_read(h)) {
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
fn cliente_tcp_intercambia_con_un_servidor() {
    for vm in [false, true] {
        // Un servidor nuevo por ejecución (cada uno acepta una sola conexión).
        let port = toy_echo_server();
        let src = CLIENTE.replace("__PORT__", &port.to_string());
        let (out, code) = run("ray_tcp_ok", &src, vm);
        assert_eq!(out.trim(), "pong:ping", "intercambio TCP (vm={vm}): {out}");
        assert_eq!(code, 0, "salida 0 (vm={vm})");
    }
}

#[test]
fn tcp_connect_falla_a_un_puerto_cerrado() {
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
        assert_eq!(code, 1, "salida 1 en error (vm={vm})");
    }
}
