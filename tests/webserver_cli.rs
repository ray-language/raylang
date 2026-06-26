//! Pruebas del servidor web en raylang (M19.1, `examples/webserver.ray`): petición/respuesta HTTP y
//! streaming SSE. La concurrencia (spawn/scope) es **solo VM**, y la red no es determinista para el
//! oráculo → subproceso: un servidor `.ray` **acotado** (sirve N conexiones vía `scope` y termina,
//! imprime su puerto) que importa la librería, y un cliente de Rust que comprueba la respuesta. Mismo
//! molde que `http_cli.rs` (copiar la librería + un driver al temporal) + `concurrency_net_cli.rs`
//! (leer el puerto del stdout y conectar).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Copia `webserver.ray` a un temporal único, escribe `driver` como `main.ray` a su lado, lanza con
/// `--vm` y devuelve el proceso hijo + el puerto efímero que imprimió en su primera línea.
fn lanzar_servidor(name: &str, driver: &str) -> (Child, u16) {
    let mut dir = std::env::temp_dir();
    dir.push(format!("ray_web_{name}"));
    std::fs::create_dir_all(&dir).expect("crea dir");
    let src = format!("{}/examples/webserver.ray", env!("CARGO_MANIFEST_DIR"));
    std::fs::copy(&src, dir.join("webserver.ray")).expect("copia webserver.ray");
    let driver_path = dir.join("main.ray");
    std::fs::File::create(&driver_path).expect("crea driver").write_all(driver.as_bytes()).expect("escribe");

    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&driver_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza servidor");

    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee el puerto");
    let port: u16 = linea.trim().parse().unwrap_or_else(|_| panic!("puerto inválido: {linea:?}"));
    (child, port)
}

/// Conecta, envía `req` cruda y lee toda la respuesta hasta que el servidor cierra.
fn pedir(port: u16, req: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.write_all(req.as_bytes()).expect("envía");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("lee respuesta");
    resp
}

// Driver: servidor concurrente acotado a 2 conexiones (cada una en su fibra) que enruta /hola.
const SRV_HTTP: &str = r#"
import webserver;
fn manejar(conn: int) {
    match (webserver.read_request(conn)) {
        Result.Ok(req) => {
            var resp: webserver.Response = webserver.not_found();
            if (req.path == "/hola") { resp = webserver.ok("hola " + req.method); }
            match (webserver.send_response(conn, resp)) {
                Result.Ok(_) => {}, Result.Err(e) => eprint(e),
            }
        },
        Result.Err(e) => eprint(e),
    }
    close(conn);
}
fn main() -> int {
    match (tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            print(local_port(srv));
            scope(fn() {
                var i: int = 0;
                while (i < 2) {
                    match (tcp_accept(srv)) {
                        Result.Ok(conn) => { spawn(fn() { manejar(conn) }); },
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

#[test]
fn servidor_http_responde_y_enruta() {
    let (mut child, port) = lanzar_servidor("http", SRV_HTTP);

    let ok = pedir(port, "GET /hola HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(ok.contains("200 OK"), "esperaba 200 OK, got: {ok}");
    assert!(ok.contains("hola GET"), "esperaba el cuerpo 'hola GET', got: {ok}");
    assert!(ok.contains("Content-Length: 8"), "esperaba Content-Length del cuerpo, got: {ok}");

    let nf = pedir(port, "GET /otra HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(nf.contains("404 Not Found"), "esperaba 404, got: {nf}");

    let _ = child.wait();
}

// Driver: servidor SSE acotado a 1 conexión que emite 3 eventos y cierra.
const SRV_SSE: &str = r#"
import webserver;
fn manejar(conn: int) {
    match (webserver.read_request(conn)) {
        Result.Ok(req) => {
            match (webserver.sse_open(conn)) { Result.Ok(_) => {}, Result.Err(e) => eprint(e) }
            var i: int = 0;
            while (i < 3) {
                match (webserver.sse_event(conn, "tick " + to_string(i))) {
                    Result.Ok(_) => {}, Result.Err(e) => eprint(e),
                }
                i = i + 1;
            }
        },
        Result.Err(e) => eprint(e),
    }
    close(conn);
}
fn main() -> int {
    match (tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            print(local_port(srv));
            scope(fn() {
                match (tcp_accept(srv)) {
                    Result.Ok(conn) => { spawn(fn() { manejar(conn) }); },
                    Result.Err(e) => eprint(e),
                }
            });
            close(srv);
            0
        },
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;

#[test]
fn servidor_sse_emite_eventos() {
    let (mut child, port) = lanzar_servidor("sse", SRV_SSE);

    let resp = pedir(port, "GET /eventos HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(resp.contains("text/event-stream"), "esperaba Content-Type SSE, got: {resp}");
    assert!(resp.contains("data: tick 0"), "esperaba el evento 0, got: {resp}");
    assert!(resp.contains("data: tick 1"), "esperaba el evento 1, got: {resp}");
    assert!(resp.contains("data: tick 2"), "esperaba el evento 2, got: {resp}");

    let _ = child.wait();
}
