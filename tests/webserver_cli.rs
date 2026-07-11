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
    // El último token de la línea: cubre tanto el puerto a secas (drivers manuales) como el
    // "escuchando en el puerto N" que imprime `serve` (M56.5: tests sobre el serve real).
    let port: u16 = linea.trim().rsplit(' ').next().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("puerto inválido: {linea:?}"));
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

// Driver: servidor concurrente acotado a 5 conexiones (cada una en su fibra) que enruta /hola y
// expone la query (M56.2: `path` viene decodificado y sin query; `query` aparte, cruda).
const SRV_HTTP: &str = r#"
import webserver;
import std/net;
fn manejar(conn: int) {
    match (webserver.read_request(conn)) {
        Result.Ok(req) => {
            var resp: webserver.Response = webserver.not_found();
            if (req.path == "/hola") { resp = webserver.ok("hola " + req.method); }
            if (req.path == "/q") {
                match (get(webserver.query_params(req), "x")) {
                    Option.Some(v) => { resp = webserver.ok("x=" + v + " cruda=" + req.query); },
                    Option.None => { resp = webserver.ok("sin x"); },
                }
            }
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
                while (i < 5) {
                    match (net.tcp_accept(srv)) {
                        Result.Ok(conn) => { spawn(fn() { manejar(conn) }); },
                        Result.Err(e) => { eprint(e); i = 5; },
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
    // M85a: la cabecera `Date:` (SHOULD RFC 7231) — presencia y FORMA RFC 1123
    // (`Wdy, DD Mon YYYY HH:MM:SS GMT`), no el valor (no determinista).
    let date = ok
        .lines()
        .find_map(|l| l.strip_prefix("Date: "))
        .unwrap_or_else(|| panic!("esperaba la cabecera Date:, got: {ok}"));
    assert!(
        date.len() == 29 && date.ends_with(" GMT") && &date[3..5] == ", ",
        "Date: no tiene forma RFC 1123: {date}"
    );

    let nf = pedir(port, "GET /otra HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(nf.contains("404 Not Found"), "esperaba 404, got: {nf}");

    // M56.2: la query NO forma parte del path → la ruta sigue casando.
    let conq = pedir(port, "GET /hola?x=1 HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(conq.contains("200 OK") && conq.contains("hola GET"), "esperaba que /hola?x=1 casara /hola, got: {conq}");

    // M56.2: query_params decodifica (+ = espacio, %XX) y req.query conserva la cruda.
    let q = pedir(port, "GET /q?x=Ada+Lovelace%21&y=2 HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(q.contains("x=Ada Lovelace!"), "esperaba el valor decodificado, got: {q}");
    assert!(q.contains("cruda=x=Ada+Lovelace%21&y=2"), "esperaba la query cruda, got: {q}");

    // M56.2: el path llega percent-decodificado ("/ho%6Ca" = "/hola").
    let dec = pedir(port, "GET /ho%6Ca HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(dec.contains("hola GET"), "esperaba /ho%6Ca decodificado a /hola, got: {dec}");

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
    let lim: webserver.Limits = webserver.Limits { max_header_bytes: 256, max_body_bytes: 16, max_conns: 8, read_timeout_millis: 0 };
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

// Driver: servidor con TIMEOUT de lectura corto (M56.4, 300 ms). Una conexión que no envía la
// petición completa a tiempo (slowloris) recibe 400 y se cierra; el servidor sigue vivo.
const SRV_TIMEOUT: &str = r#"
import webserver;
import std/net;
fn manejar(conn: int) {
    let lim: webserver.Limits = webserver.Limits { max_header_bytes: 65536, max_body_bytes: 1048576, max_conns: 8, read_timeout_millis: 300 };
    match (webserver.read_request_limits(conn, lim)) {
        Result.Ok(req) => {
            match (webserver.send_response(conn, webserver.ok("hola " + req.path))) {
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
fn servidor_corta_lecturas_lentas_por_timeout() {
    let (mut child, port) = lanzar_servidor("timeout", SRV_TIMEOUT);

    // Slowloris: abre, envía media petición y se queda callado (sin cerrar). El timeout (300 ms)
    // debe vencer y responder 400; sin él, esta conexión retendría su fibra para siempre.
    let inicio = std::time::Instant::now();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    stream.write_all(b"GET /lento HTTP/1.1\r\nX-Goteo: a").expect("envía parcial");
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).expect("lee respuesta");
    let resp = String::from_utf8_lossy(&resp);
    assert!(resp.contains("400 Bad Request"), "esperaba 400 por timeout, got: {resp}");
    assert!(inicio.elapsed() >= Duration::from_millis(250), "respondió antes del plazo (¿timeout no aplicado?)");
    assert!(inicio.elapsed() < Duration::from_secs(8), "tardó demasiado (¿esperó a EOF en vez del timeout?)");

    // El servidor sigue vivo: una petición normal posterior responde 200 al instante.
    let ok = pedir(port, "GET /hola HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(ok.contains("200 OK"), "el servidor debe seguir vivo tras el timeout, got: {ok}");

    let _ = child.wait();
}

// Driver: el `serve` REAL (bucle infinito; el test mata el proceso) con un handler que PANICA en
// /boom y max_conns = 2. M56.5: el panic no mata la fibra de la conexión (500 + cierre + token del
// semáforo liberado); sin la liberación, a la 3ª petición el accept se pararía y el test colgaría.
const SRV_PANIC: &str = r#"
import webserver;

fn pagina(req: webserver.Request) -> webserver.Response {
    if (req.path == "/boom") {
        panic("kaboom del handler");
    }
    webserver.ok("vivo")
}

fn main() -> int {
    let lim: webserver.Limits = webserver.Limits { max_header_bytes: 65536, max_body_bytes: 1048576, max_conns: 2, read_timeout_millis: 5000 };
    match (webserver.serve_limits("127.0.0.1", 0, lim, pagina)) {
        Result.Ok(_) => 0,
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;

#[test]
fn handler_que_panica_responde_500_y_no_fuga_recursos() {
    let (mut child, port) = lanzar_servidor("panic", SRV_PANIC);

    // Más peticiones que panican (4) que max_conns (2): si el panic fugara el fd o el token del
    // semáforo, el servidor dejaría de aceptar a la 3ª y esto colgaría (el timeout del cliente
    // haría fallar el test).
    for i in 0..4 {
        let r = pedir(port, "GET /boom HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(r.contains("500 Internal Server Error"), "petición {i}: esperaba 500, got: {r}");
    }

    // El servidor sigue vivo y sano. (Connection: close → serve cierra y read_to_end termina.)
    let ok = pedir(port, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(ok.contains("200 OK") && ok.contains("vivo"), "esperaba 200 tras los panics, got: {ok}");

    let _ = child.kill();
    let _ = child.wait();
}

/// Lee UNA respuesta HTTP completa (cabeceras + cuerpo delimitado por Content-Length) del stream.
/// Necesaria con keep-alive (M56.6): el servidor NO cierra, así que read_to_end no termina.
fn leer_una_respuesta(stream: &mut TcpStream) -> String {
    let mut resp: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    let mut fin_cab: Option<usize> = None;
    let mut total: Option<usize> = None;
    loop {
        if fin_cab.is_none() {
            if let Some(i) = resp.windows(4).position(|w| w == b"\r\n\r\n") {
                fin_cab = Some(i);
                continue;
            }
        } else if total.is_none() {
            let fc = fin_cab.unwrap();
            let head = String::from_utf8_lossy(&resp[..fc]).to_ascii_lowercase();
            let cl: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().expect("CL numérico")))
                .expect("respuesta sin Content-Length");
            total = Some(fc + 4 + cl);
            continue;
        } else if resp.len() >= total.unwrap() {
            break;
        }
        let n = stream.read(&mut buf).expect("lee respuesta");
        assert!(n > 0, "EOF a mitad de respuesta: {:?}", String::from_utf8_lossy(&resp));
        resp.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&resp).into_owned()
}

// Driver: el `serve` real (keep-alive, M56.6) con timeout de ocio corto (500 ms) para verificar
// también que una conexión keep-alive ociosa se cierra sola.
const SRV_KEEPALIVE: &str = r#"
import webserver;

fn pagina(req: webserver.Request) -> webserver.Response {
    webserver.ok("hola " + req.path)
}

fn main() -> int {
    let lim: webserver.Limits = webserver.Limits { max_header_bytes: 65536, max_body_bytes: 1048576, max_conns: 8, read_timeout_millis: 500 };
    match (webserver.serve_limits("127.0.0.1", 0, lim, pagina)) {
        Result.Ok(_) => 0,
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;

#[test]
fn servidor_mantiene_la_conexion_viva_entre_peticiones() {
    let (mut child, port) = lanzar_servidor("keepalive", SRV_KEEPALIVE);

    // Dos peticiones por la MISMA conexión (keep-alive HTTP/1.1 por defecto).
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.write_all(b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n").expect("envía 1ª");
    let r1 = leer_una_respuesta(&mut stream);
    assert!(r1.contains("200 OK") && r1.contains("hola /a"), "1ª respuesta: {r1}");
    assert!(r1.contains("Connection: keep-alive"), "esperaba keep-alive, got: {r1}");

    stream.write_all(b"GET /b HTTP/1.1\r\nHost: x\r\n\r\n").expect("envía 2ª");
    let r2 = leer_una_respuesta(&mut stream);
    assert!(r2.contains("hola /b"), "2ª respuesta por la misma conexión: {r2}");

    // `Connection: close` se honra: responde y CIERRA (EOF).
    stream.write_all(b"GET /c HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").expect("envía 3ª");
    let r3 = leer_una_respuesta(&mut stream);
    assert!(r3.contains("hola /c") && r3.contains("Connection: close"), "3ª respuesta: {r3}");
    let mut extra = [0u8; 16];
    let n = stream.read(&mut extra).expect("lee tras close");
    assert_eq!(n, 0, "esperaba EOF tras Connection: close");

    // Una conexión keep-alive OCIOSA se cierra sola al vencer el read timeout (500 ms), en
    // silencio (sin 400): el servidor no retiene la fibra para siempre.
    let mut ociosa = TcpStream::connect(("127.0.0.1", port)).expect("conecta ociosa");
    ociosa.set_read_timeout(Some(Duration::from_secs(5))).ok();
    ociosa.write_all(b"GET /d HTTP/1.1\r\nHost: x\r\n\r\n").expect("envía");
    let r4 = leer_una_respuesta(&mut ociosa);
    assert!(r4.contains("hola /d"), "respuesta antes del ocio: {r4}");
    let mut fin = Vec::new();
    ociosa.read_to_end(&mut fin).expect("lee hasta el cierre por ocio");
    assert!(fin.is_empty(), "esperaba cierre limpio sin datos extra, got: {:?}", String::from_utf8_lossy(&fin));

    let _ = child.kill();
    let _ = child.wait();
}

// Driver: una respuesta con DOS cookies (M56.7: `set_cookie` es lista — la única cabecera de
// respuesta que se repite de verdad; con `headers: Map` era imposible enviar sesión + flash).
const SRV_COOKIES: &str = r#"
import webserver;
from webserver import with_cookie;
import std/net;
fn manejar(conn: int) {
    match (webserver.read_request(conn)) {
        Result.Ok(req) => {
            let r = webserver.ok("con cookies")
                .with_cookie("sesion=abc123; Path=/; HttpOnly")
                .with_cookie("flash=bienvenido");
            match (webserver.send_response(conn, r)) {
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

#[test]
fn respuesta_con_varias_cookies() {
    let (mut child, port) = lanzar_servidor("cookies", SRV_COOKIES);

    let r = pedir(port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(r.contains("200 OK"), "esperaba 200, got: {r}");
    assert!(r.contains("Set-Cookie: sesion=abc123; Path=/; HttpOnly"), "falta la 1ª cookie: {r}");
    assert!(r.contains("Set-Cookie: flash=bienvenido"), "falta la 2ª cookie: {r}");
    // Dos líneas Set-Cookie separadas (no fusionadas con coma, que rompería las cookies).
    assert_eq!(r.matches("Set-Cookie:").count(), 2, "esperaba exactamente 2 líneas Set-Cookie: {r}");

    let _ = child.wait();
}

#[test]
fn head_devuelve_cabeceras_sin_cuerpo() {
    // M56.8: a un HEAD, el camino `serve` responde las cabeceras del GET (incl. Content-Length)
    // pero SIN el cuerpo. Reusa el driver keep-alive (handler: "hola " + path).
    let (mut child, port) = lanzar_servidor("head", SRV_KEEPALIVE);

    let r = pedir(port, "HEAD /a HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("200 OK"), "esperaba 200 a HEAD, got: {r}");
    assert!(r.contains("Content-Length: 7"), "esperaba el Content-Length del GET (7), got: {r}");
    assert!(!r.contains("hola /a"), "un HEAD no debe llevar cuerpo, got: {r}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cuerpo_chunked_se_decodifica() {
    // M56.8: Transfer-Encoding: chunked entrante → el eco devuelve el cuerpo DECODIFICADO
    // ("hola" + " mundo", con extensión de chunk ignorada). Antes llegaba vacío en silencio.
    let (mut child, port) = lanzar_servidor("chunked", SRV_ECO_BIN);

    let req = b"POST /eco HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nhola\r\n6;ext=1\r\n mundo\r\n0\r\n\r\n";
    let resp = pedir_bytes(port, req);
    assert_eq!(cuerpo(&resp), b"hola mundo", "cuerpo chunked decodificado, got: {:?}", String::from_utf8_lossy(cuerpo(&resp)));

    let _ = child.wait();
}

// Driver: archivos estáticos (M56.8). El directorio raíz llega por la variable RAY_STATIC_DIR.
const SRV_STATIC: &str = r#"
import webserver;
import std/net;
fn manejar(conn: int, dir: string) {
    match (webserver.read_request(conn)) {
        Result.Ok(req) => {
            match (webserver.send_response(conn, webserver.static_response(dir, req.path))) {
                Result.Ok(_) => {}, Result.Err(e) => eprint(e),
            }
        },
        Result.Err(e) => eprint(e),
    }
    close(conn);
}
fn main() -> int {
    var dir: string = "";
    match (env("RAY_STATIC_DIR")) {
        Option.Some(d) => { dir = d; },
        Option.None => {
            eprint("falta RAY_STATIC_DIR");
            return 1;
        },
    }
    match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(srv) => {
            print(net.local_port(srv));
            scope(fn() {
                var i: int = 0;
                while (i < 3) {
                    match (net.tcp_accept(srv)) {
                        Result.Ok(conn) => { spawn(fn() { manejar(conn, dir) }); },
                        Result.Err(e) => { eprint(e); i = 3; },
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
fn archivos_estaticos_con_saneo() {
    // Prepara la raíz estática: pub/index.html público y secreto.txt FUERA de la raíz.
    let mut base = std::env::temp_dir();
    base.push("ray_web_static_root");
    let publica = base.join("pub");
    std::fs::create_dir_all(&publica).expect("crea pub");
    std::fs::write(publica.join("index.html"), "<h1>hola estática</h1>").expect("escribe index");
    std::fs::write(base.join("secreto.txt"), "no debe salir").expect("escribe secreto");

    // Lanza el driver con RAY_STATIC_DIR apuntando a la raíz pública.
    let mut dir = std::env::temp_dir();
    dir.push("ray_web_static");
    std::fs::create_dir_all(&dir).expect("crea dir");
    let src = format!("{}/examples/web/webserver.ray", env!("CARGO_MANIFEST_DIR"));
    std::fs::copy(&src, dir.join("webserver.ray")).expect("copia webserver.ray");
    let driver_path = dir.join("main.ray");
    std::fs::write(&driver_path, SRV_STATIC).expect("escribe driver");
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&driver_path)
        .env("RAY_STATIC_DIR", publica.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza servidor estático");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee el puerto");
    let port: u16 = linea.trim().rsplit(' ').next().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("puerto inválido: {linea:?}"));

    // "/" sirve index.html con su Content-Type.
    let r = pedir(port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(r.contains("200 OK") && r.contains("hola estática"), "index: {r}");
    assert!(r.contains("Content-Type: text/html"), "mime del html: {r}");

    // Path traversal → 404 (el secreto vive fuera de la raíz).
    let tr = pedir(port, "GET /../secreto.txt HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(tr.contains("404 Not Found") && !tr.contains("no debe salir"), "traversal: {tr}");

    // Un archivo que no existe → 404.
    let nf = pedir(port, "GET /nada.css HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(nf.contains("404 Not Found"), "inexistente: {nf}");

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
