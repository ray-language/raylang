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

const CLIENT: &str = r#"
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
        let src = CLIENT.replace("__PORT__", &port.to_string());
        let (out, code) = run("ray_tcp_ok", &src, vm);
        assert_eq!(out.trim(), "pong:ping", "intercambio TCP (vm={vm}): {out}");
        assert_eq!(code, 0, "output 0 (vm={vm})");
    }
}

/// El `.ray` es el SERVER (M15.3): escucha en puerto 0, imprime el puerto, acepta UNA conexión,
/// lee y responde con eco. El test lee el puerto de su stdout EN VIVO (el servidor bloquea en accept,
/// así que no se puede usar `.output()` que espera a que termine) y se conecta como cliente.
const SERVER: &str = r#"
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
        std::fs::File::create(&path).expect("crea").write_all(SERVER.as_bytes()).expect("escribe");
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
        let src = CLIENT.replace("__PORT__", &port.to_string());
        let (out, code) = run("ray_tcp_err", &src, vm);
        assert!(out.contains("connect err"), "conexión rechazada (vm={vm}): {out}");
        assert_eq!(code, 1, "output 1 en error (vm={vm})");
    }
}

// ---------------------------------------------------------------------------------------------
// M122 — `net.tcp_connect_timeout`: el connect con plazo.
// ---------------------------------------------------------------------------------------------

/// (1) Contra una ruta negra (TEST-NET-1, RFC 5737 — descarta los SYN), el intento falla ACOTADO
/// con el error estable "connect timeout" (antes: ~75 s del SO). (2) Con plazo puesto, un connect
/// a un listener real sigue funcionando. Ambos motores. NOTA: si la red local responde a
/// 192.0.2.1 con un rechazo inmediato (poco común), el error del SO también vale — lo que el test
/// exige es que NO cuelgue (elapsed acotado) y que sea Err.
#[test]
fn tcp_connect_timeout_bounds_the_wait_and_still_connects() {
    let src = r#"
import std/net;
import std/time;

fn main() -> int {
    let t0 = time.monotonic();
    match (net.tcp_connect_timeout("192.0.2.1", 81, 400)) {
        Result.Ok(_) => print("unexpected connect"),
        Result.Err(_e) => print("bounded err"),
    }
    let dt = time.monotonic() - t0;
    print(dt < 10000);
    let l = match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(x) => x,
        Result.Err(e) => {
            print("listen err: " + e);
            return 1;
        },
    };
    let port = net.local_port(l);
    match (net.tcp_connect_timeout("127.0.0.1", port, 2000)) {
        Result.Ok(h) => {
            print("connected");
            close(h);
        },
        Result.Err(e) => print("connect err: " + e),
    }
    0
}
"#;
    for vm in [false, true] {
        let start = std::time::Instant::now();
        let (out, code) = run("net_connect_timeout", src, vm);
        assert!(start.elapsed() < std::time::Duration::from_secs(30), "el connect no quedó acotado (vm={vm})");
        assert_eq!(code, 0, "exit (vm={vm}): {out}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines, vec!["bounded err", "true", "connected"], "vm={vm}");
    }
}

// ---------------------------------------------------------------------------------------------
// M123 — `net.peer_addr`: la dirección del peer de una conexión.
// ---------------------------------------------------------------------------------------------

/// El programa se conecta a su propio listener: el peer del CLIENTE es "127.0.0.1:<port>" —
/// determinista sin servidor externo. Un listener y un handle inválido fallan limpio.
const PEER_ADDR_SRC: &str = r#"
import std/net;

fn main() -> int {
    let l = match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(x) => x,
        Result.Err(e) => {
            print("listen err: " + e);
            return 1;
        },
    };
    let port = net.local_port(l);
    let c = match (net.tcp_connect("127.0.0.1", port)) {
        Result.Ok(x) => x,
        Result.Err(e) => {
            print("connect err: " + e);
            return 1;
        },
    };
    match (net.peer_addr(c)) {
        Result.Ok(a) => print(a == "127.0.0.1:" + to_string(port)),
        Result.Err(e) => print("err: " + e),
    }
    match (net.peer_addr(l)) {
        Result.Ok(_) => print("unexpected"),
        Result.Err(_e) => print("listener errs"),
    }
    match (net.peer_addr(99999)) {
        Result.Ok(_) => print("unexpected"),
        Result.Err(e) => print(e),
    }
    0
}
"#;

const PEER_ADDR_EXPECTED: &[&str] = &["true", "listener errs", "invalid handle: 99999"];

#[test]
fn peer_addr_reports_the_remote_endpoint() {
    for vm in [false, true] {
        let (out, code) = run("net_peer_addr", PEER_ADDR_SRC, vm);
        assert_eq!(code, 0, "exit (vm={vm}): {out}");
        assert_eq!(out.lines().collect::<Vec<_>>(), PEER_ADDR_EXPECTED, "vm={vm}");
    }
}

/// El binario NATIVO (sabor default): mismo programa, misma salida byte a byte.
#[test]
fn peer_addr_native() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        assert!(std::env::var_os("CI").is_none(), "rustc no disponible bajo CI: falso verde");
        eprintln!("saltando peer_addr_native: rustc no disponible");
        return;
    }
    let mut src_path = std::env::temp_dir();
    src_path.push("net_peer_addr_native.ray");
    std::fs::write(&src_path, PEER_ADDR_SRC).expect("escribe el fuente");
    let bin = std::env::temp_dir().join(format!("ray_peer_addr_{}", std::process::id()));
    let build = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["build", src_path.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza build --native");
    assert!(build.status.success(), "build --native falló: {}", String::from_utf8_lossy(&build.stderr));
    let out = Command::new(&bin).output().expect("corre el binario nativo");
    let _ = std::fs::remove_file(&bin);
    assert!(out.status.success(), "el binario nativo falló: {}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<&str> = std::str::from_utf8(&out.stdout).unwrap_or("").lines().collect();
    assert_eq!(lines, PEER_ADDR_EXPECTED, "el nativo diverge de la VM en peer_addr");
}

// ---------------------------------------------------------------------------------------------
// M130 — `net.shutdown_write`: half-close (shutdown SHUT_WR).
// ---------------------------------------------------------------------------------------------

/// Secuencial contra el propio listener (sin spawn → corre también en el intérprete): el cliente
/// escribe y hace half-close; el lado aceptado lee los datos Y el EOF (solo visible con
/// SHUT_WR), y aún puede responder — el cliente sigue leyendo tras su shutdown.
const SHUTDOWN_WRITE_SRC: &str = r#"
import std/net;

fn main() -> int {
    let l = match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(x) => x,
        Result.Err(e) => {
            print("listen err: " + e);
            return 1;
        },
    };
    let port = net.local_port(l);
    let c = match (net.tcp_connect("127.0.0.1", port)) {
        Result.Ok(x) => x,
        Result.Err(e) => {
            print("connect err: " + e);
            return 1;
        },
    };
    let s = match (net.tcp_accept(l)) {
        Result.Ok(x) => x,
        Result.Err(e) => {
            print("accept err: " + e);
            return 1;
        },
    };
    let _ = net.socket_write(c, "hola");
    match (net.shutdown_write(c)) {
        Result.Ok(_z) => print("shutdown ok"),
        Result.Err(e) => print("shutdown err: " + e),
    }
    match (net.socket_read(s)) {
        Result.Ok(datos) => print("got " + datos),
        Result.Err(e) => print("read err: " + e),
    }
    match (net.socket_read(s)) {
        Result.Ok(eof) => print("eof " + to_string(eof.len())),
        Result.Err(e) => print("read err: " + e),
    }
    let _ = net.socket_write(s, "resp");
    match (net.socket_read(c)) {
        Result.Ok(r) => print("client got " + r),
        Result.Err(e) => print("client err: " + e),
    }
    match (net.shutdown_write(l)) {
        Result.Ok(_z) => print("unexpected"),
        Result.Err(e) => print(e),
    }
    match (net.shutdown_write(99999)) {
        Result.Ok(_z) => print("unexpected"),
        Result.Err(e) => print(e),
    }
    0
}
"#;

const SHUTDOWN_WRITE_EXPECTED: &[&str] = &[
    "shutdown ok",
    "got hola",
    "eof 0",
    "client got resp",
    "handle 1 is not a TCP socket",
    "invalid handle: 99999",
];

#[test]
fn shutdown_write_half_closes_and_still_reads() {
    for vm in [false, true] {
        let (out, code) = run("net_shutdown_write", SHUTDOWN_WRITE_SRC, vm);
        assert_eq!(code, 0, "exit (vm={vm}): {out}");
        assert_eq!(out.lines().collect::<Vec<_>>(), SHUTDOWN_WRITE_EXPECTED, "vm={vm}");
    }
}

/// El binario NATIVO (sabor default): mismo programa, misma salida byte a byte.
#[test]
fn shutdown_write_native() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        assert!(std::env::var_os("CI").is_none(), "rustc no disponible bajo CI: falso verde");
        eprintln!("saltando shutdown_write_native: rustc no disponible");
        return;
    }
    let mut src_path = std::env::temp_dir();
    src_path.push("net_shutdown_write_native.ray");
    std::fs::write(&src_path, SHUTDOWN_WRITE_SRC).expect("escribe el fuente");
    let bin = std::env::temp_dir().join(format!("ray_shutdown_write_{}", std::process::id()));
    let build = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["build", src_path.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza build --native");
    assert!(build.status.success(), "build --native falló: {}", String::from_utf8_lossy(&build.stderr));
    let out = Command::new(&bin).output().expect("corre el binario nativo");
    let _ = std::fs::remove_file(&bin);
    assert!(out.status.success(), "el binario nativo falló: {}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<&str> = std::str::from_utf8(&out.stdout).unwrap_or("").lines().collect();
    assert_eq!(lines, SHUTDOWN_WRITE_EXPECTED, "el nativo diverge de la VM en shutdown_write");
}
