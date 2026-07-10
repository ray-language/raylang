//! Pruebas del servidor web en raylang (M19.1, `examples/web/webserver.ray`): petición/respuesta HTTP y
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
    let src = format!("{}/examples/web/webserver.ray", env!("CARGO_MANIFEST_DIR"));
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
    String::from_utf8_lossy(&pedir_bytes(port, req.as_bytes())).into_owned()
}

/// Igual que `pedir` pero envía/recibe **octetos crudos** (para verificar cuerpos binarios).
fn pedir_bytes(port: u16, req: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.write_all(req).expect("envía");
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).expect("lee respuesta");
    resp
}

/// El cuerpo de una respuesta HTTP cruda: lo que sigue al primer "\r\n\r\n".
fn cuerpo(resp: &[u8]) -> &[u8] {
    let sep = b"\r\n\r\n";
    resp.windows(4).position(|w| w == sep).map(|i| &resp[i + 4..]).unwrap_or(&[])
}

// Driver: servidor concurrente acotado a 2 conexiones (cada una en su fibra) que enruta /hola.
const SRV_HTTP: &str = r#"
import webserver;
import std/net;
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
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            print(net.local_port(srv));
            scope(fn() {
                var i: int = 0;
                while (i < 2) {
                    match (net.tcp_accept(srv)) {
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
import std/net;
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
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            print(net.local_port(srv));
            scope(fn() {
                match (net.tcp_accept(srv)) {
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

// Driver: servidor que eco-devuelve el cuerpo de la petición como respuesta BINARIA. Verifica que un
// cuerpo binario (con \x00/\xff) cruza intacto read_request (por Content-Length) y send_response (M19.2).
const SRV_ECO_BIN: &str = r#"
import webserver;
import std/net;
fn manejar(conn: int) {
    match (webserver.read_request(conn)) {
        Result.Ok(req) => {
            match (webserver.send_response(conn, webserver.bytes_response(200, req.body))) {
                Result.Ok(_) => {}, Result.Err(e) => eprint(e),
            }
        },
        Result.Err(e) => eprint(e),
    }
    close(conn);
}
fn main() -> int {
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            print(net.local_port(srv));
            scope(fn() {
                match (net.tcp_accept(srv)) {
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

/// Como `pedir`, pero cierra el lado de escritura tras enviar (el servidor ve EOF) y lee la respuesta.
fn pedir_con_eof(port: u16, req: &[u8]) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.write_all(req).expect("envía");
    stream.shutdown(std::net::Shutdown::Write).expect("shutdown write");
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).expect("lee respuesta");
    String::from_utf8_lossy(&resp).into_owned()
}

// Driver: servidor con límites PEQUEÑOS (M56.1) — 256 octetos de cabeceras, 16 de cuerpo. Una
// petición que los viola (o que el peer no completa) es Err → responde 400 y cierra.
const SRV_LIMITES: &str = r#"
import webserver;
import std/net;
fn manejar(conn: int) {
    let lim: webserver.Limits = webserver.Limits { max_header_bytes: 256, max_body_bytes: 16, max_conns: 8 };
    match (webserver.read_request_limits(conn, lim)) {
        Result.Ok(req) => {
            match (webserver.send_response(conn, webserver.ok("ok " + to_string(req.body.len())))) {
                Result.Ok(_) => {}, Result.Err(e) => eprint(e),
            }
        },
        Result.Err(e) => {
            eprint(e);
            match (webserver.send_response(conn, webserver.text(400, "Bad Request"))) {
                Result.Ok(_) => {}, Result.Err(e2) => eprint(e2),
            }
        },
    }
    close(conn);
}
fn main() -> int {
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            print(net.local_port(srv));
            scope(fn() {
                var i: int = 0;
                while (i < 4) {
                    match (net.tcp_accept(srv)) {
                        Result.Ok(conn) => { spawn(fn() { manejar(conn) }); },
                        Result.Err(e) => { eprint(e); i = 4; },
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
fn servidor_aplica_limites_de_seguridad() {
    let (mut child, port) = lanzar_servidor("limites", SRV_LIMITES);

    // (a) Una petición normal dentro de los límites responde 200.
    let ok = pedir(port, "POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc");
    assert!(ok.contains("200 OK") && ok.contains("ok 3"), "esperaba 200 'ok 3', got: {ok}");

    // (b) Cabeceras que exceden el tope (256) → 400, sin esperar a que el cliente termine.
    let gigante = format!("GET / HTTP/1.1\r\nX-Relleno: {}\r\n\r\n", "a".repeat(400));
    let hd = pedir(port, &gigante);
    assert!(hd.contains("400 Bad Request"), "esperaba 400 por cabeceras gigantes, got: {hd}");

    // (c) Content-Length declarado mayor que el tope (16) → 400 ANTES de leer el cuerpo.
    let cl = pedir(port, "POST / HTTP/1.1\r\nContent-Length: 999\r\n\r\n");
    assert!(cl.contains("400 Bad Request"), "esperaba 400 por cuerpo declarado gigante, got: {cl}");

    // (d) Cuerpo truncado (declara 10, envía 3 y cierra) → 400, no un Ok silencioso a medias.
    let tr = pedir_con_eof(port, b"POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc");
    assert!(tr.contains("400 Bad Request"), "esperaba 400 por cuerpo incompleto, got: {tr}");

    let _ = child.wait();
}

#[test]
fn servidor_eco_cuerpo_binario_intacto() {
    let (mut child, port) = lanzar_servidor("ecobin", SRV_ECO_BIN);

    // POST con un cuerpo binario de 7 octetos, incl. 0x00 y 0xFF (que UTF-8 lossy corrompería).
    let body: [u8; 7] = [0, 255, 1, 2, b'b', b'i', b'n'];
    let mut req: Vec<u8> = format!(
        "POST /eco HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    req.extend_from_slice(&body);

    let resp = pedir_bytes(port, &req);
    assert_eq!(cuerpo(&resp), &body, "el cuerpo binario debe cruzar intacto, got: {:?}", cuerpo(&resp));

    let _ = child.wait();
}
