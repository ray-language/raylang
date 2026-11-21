//! net 0.15.1: los hallazgos pequeños de la re-medición de 1.27.108 sobre el paquete `net`. Cada
//! programa levanta su servidor en el puerto 0 y su cliente en el mismo proceso, y sale solo.
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

fn run(name: &str, main: &str, envs: &[(&str, &str)]) -> (String, String, i32) {
    let base = std::env::temp_dir().join(format!("ray_net_0151_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    let repo = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"probe\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(base.join("src/main.ray"), main).unwrap();
    let mut c = Command::new(BIN);
    c.arg("run").current_dir(&base);
    for (k, v) in envs {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

/// raycdc #406: un handler crudo sobre un listener ya abierto (`serve_raw_on`), con un upgrade a
/// WebSocket: se enlaza el puerto 0, se lee el puerto y se sirve sin la carrera de re-bind.
#[test]
fn a_raw_handler_serves_a_websocket_upgrade_on_an_open_listener() {
    let main = r#"import std/net;
import net/webserver;
import net/websocket;
import net/websocket_client;
from net/webserver import Request;

fn on_conn(req: Request, conn: int) {
    // raycdc #468: `is_upgrade` + `accept` (lo que cita la doc de `serve_raw_on`).
    if (!websocket.is_upgrade(req.headers)) {
        return;
    }
    let ws = match (websocket.accept(req.headers, conn)) {
        Result.Ok(w) => w,
        Result.Err(_) => { return; },
    };
    match (websocket.read_message(ws)) {
        Result.Ok(f) => {
            let _ = net.socket_write_bytes(conn, websocket.encode_frame(websocket.op_text(), f.payload));
        },
        Result.Err(_) => { },
    }
}

fn main() -> int {
    let listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(listener);
    let _ = spawn(fn() -> int {
        let _ = webserver.serve_raw_on(listener, on_conn);
        0
    });
    let ws = websocket_client.connect("127.0.0.1", port, "/").unwrap();
    let _ = websocket_client.send_text(ws, "ping");
    print("echo " + websocket_client.recv_text(ws).unwrap_or("?"));
    exit(0);
    0
}
"#;
    let (out, err, code) = run("raw_on", main, &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("echo ping"), "{out}{err}");
}

/// Revisión de 1.27.111 (net 0.17.2): `is_upgrade` compara `websocket` como token de `Upgrade`, y
/// `accept` contesta 426 con la versión admitida a una petición de otra versión.
#[test]
fn a_websocket_upgrade_is_detected_by_token_and_a_wrong_version_gets_426() {
    let main = r#"import std/net;
import net/websocket;

fn headers(upgrade: string, version: string) -> Map<string, string> {
    var h: Map<string, string> = Map.new();
    h.insert("upgrade", upgrade);
    h.insert("connection", "keep-alive, Upgrade");
    h.insert("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
    h.insert("sec-websocket-version", version);
    h
}

fn main() -> int {
    print(websocket.is_upgrade(headers("websocket", "13")));
    print(websocket.is_upgrade(headers("h2c, WebSocket", "13")));
    print(websocket.is_upgrade(headers("websocket2", "13")));
    var no_key = headers("websocket", "13");
    no_key.remove("sec-websocket-key");
    print(websocket.is_upgrade(no_key));
    print(websocket.is_upgrade(headers("websocket/13", "13")));
    let listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    let client = net.tcp_connect("127.0.0.1", net.local_port(listener)).unwrap();
    let server = net.tcp_accept(listener).unwrap();
    match (websocket.accept(headers("websocket", "8"), server)) {
        Result.Ok(_) => print("accepted"),
        Result.Err(e) => print(e),
    }
    close(server);
    let answer = net.socket_read(client).unwrap_or("");
    print(answer.starts_with("HTTP/1.1 426 "));
    print(answer.contains("Sec-WebSocket-Version: 13\r\n"));
    print(answer.contains("Upgrade: websocket\r\n"));
    // Sin `Connection: Upgrade` no es un upgrade válido: 400, también contestado por `accept`.
    let client2 = net.tcp_connect("127.0.0.1", net.local_port(listener)).unwrap();
    let server2 = net.tcp_accept(listener).unwrap();
    var plain = headers("websocket", "13");
    plain.insert("connection", "keep-alive");
    print(websocket.accept(plain, server2).is_err());
    close(server2);
    print(net.socket_read(client2).unwrap_or("").starts_with("HTTP/1.1 400 "));
    0
}
"#;
    let (out, err, code) = run("ws_detect", main, &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        out,
        "true\ntrue\nfalse\nfalse\ntrue\nwebsocket: unsupported Sec-WebSocket-Version (only 13)\ntrue\ntrue\ntrue\ntrue\ntrue\n",
        "{err}"
    );
}

/// Revisión de net 0.17.2: las líneas repetidas de una cabecera de la petición se unen con comas
/// (RFC 9110 §5.3) en vez de quedarse con la última; `Cookie`, con `; `.
#[test]
fn repeated_request_header_lines_are_joined() {
    let main = r#"import std/net;
import net/webserver;
from net/webserver import Request, Response;

fn show(req: Request) -> Response {
    webserver.text(200, req.headers.get_or("connection", "") + "|" + req.headers.get_or("cookie", ""))
}

fn main() -> int {
    let listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(listener);
    let _ = spawn(fn() -> int {
        let _ = webserver.serve_on(listener, show);
        0
    });
    let c = net.tcp_connect("127.0.0.1", port).unwrap();
    let _ = net.socket_write(
        c,
        "GET / HTTP/1.1\r\nHost: h\r\nConnection: Upgrade\r\nCookie: a=1\r\nConnection: close\r\nCookie: b=2\r\n\r\n"
    );
    var answer = "";
    while (true) {
        match (net.socket_read(c)) {
            Result.Ok(part) => {
                if (part == "") { break; }
                answer = answer + part;
            },
            Result.Err(_) => { break; },
        }
    }
    print(answer.ends_with("Upgrade, close|a=1; b=2"));
    exit(0);
    0
}
"#;
    let (out, err, code) = run("joined_headers", main, &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.ends_with("true\n"), "{out}{err}");
}

/// raycrawl #371: con `quiet()`, el cliente que corta la descarga a mitad del cuerpo no deja un
/// `response: Broken pipe` en stderr. Sin `quiet()` se sigue anotando.
#[test]
fn quiet_does_not_log_a_client_that_went_away() {
    let main = r#"import std/net;
import std/time;
import net/webserver;
import net/http;
from net/webserver import Request, Response;

fn big(req: Request) -> Response {
    var chunk = "";
    for _ in 0..1024 { chunk = chunk + "0123456789abcdef"; }
    var body = "";
    for _ in 0..256 { body = body + chunk; }
    webserver.text(200, body)
}

fn main() -> int {
    let listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(listener);
    let quiet = env("PROBE_QUIET").unwrap_or("") == "1";
    let _ = spawn(fn() -> int {
        let opts = if (quiet) { webserver.options().quiet() } else { webserver.options() };
        let _ = webserver.serve_options_on(listener, opts, fn() -> fn(Request) -> Response { big });
        0
    });
    let none: Map<string, string> = Map.new();
    for _ in 0..5 {
        match (http.stream("GET", "http://127.0.0.1:${port}/", b"", none)) {
            Result.Ok(s) => {
                let _ = http.stream_read(s);
                http.stream_close(s);
            },
            Result.Err(e) => print("stream ERR " + e),
        }
    }
    time.sleep(300);
    print("done");
    exit(0);
    0
}
"#;
    let (out, err, code) = run("quiet_on", main, &[("PROBE_QUIET", "1")]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("done"), "{out}{err}");
    assert!(!err.contains("response:"), "{err}");
    let (out, err, code) = run("quiet_off", main, &[("PROBE_QUIET", "0")]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(err.contains("response:"), "sin quiet el corte se sigue anotando: {err}");
}

/// raygrpc #385: una ruta sin handler responde UNIMPLEMENTED al llegar las cabeceras, aunque el
/// cliente no cierre su lado (un stream bidireccional, la reflexión de grpcurl). Antes se quedaba
/// colgada hasta el plazo del cliente.
#[test]
fn an_unknown_grpc_method_answers_unimplemented_before_the_request_ends() {
    let main = r#"import std/net;
import std/time;
import net/grpc_server;
import net/grpc_h2 as h2;
from net/http2 import preface, settings_empty;
from net/grpc_status import Status;

fn say(c: grpc_server.Call) -> Result<bytes, Status> {
    Result.Ok(c.message)
}

fn build() -> grpc_server.Router {
    grpc_server.router().route("/echo.Echo/Say", say)
}

fn main() -> int {
    let l = grpc_server.bind("127.0.0.1", 0).unwrap();
    let port = l.port;
    let _ = spawn(fn() -> int {
        let _ = grpc_server.serve_router(l, build);
        0
    });
    let conn = net.tcp_connect("127.0.0.1", port).unwrap();
    let link = h2.new_link(conn);
    let _ = h2.write(link, preface() + settings_empty());
    h2.set_deadline(link, time.monotonic() + 3000);
    let hs = [
        h2.hdr(":method", "POST"),
        h2.hdr(":scheme", "http"),
        h2.hdr(":path", "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo"),
        h2.hdr(":authority", "127.0.0.1"),
        h2.hdr("content-type", "application/grpc"),
        h2.hdr("te", "trailers")
    ];
    h2.open_stream(link, 1);
    // Sin END_STREAM: el cliente deja abierto su lado.
    let _ = h2.send_headers(link, 1, hs, false);
    let started = time.monotonic();
    var status = "none";
    while (status == "none") {
        match (h2.read_frame(link)) {
            Result.Ok(fr) => {
                if (fr.ftype == h2.HEADERS && fr.stream_id == 1) {
                    match (h2.decode_headers(link, fr.payload)) {
                        Result.Ok(got) => { status = h2.find(got, "grpc-status"); },
                        Result.Err(e) => { status = "ERR " + e; },
                    }
                } else {
                    let _ = h2.control(link, fr);
                }
            },
            Result.Err(e) => { status = "ERR " + e; },
        }
    }
    print("grpc-status ${status} fast=${time.monotonic() - started < 1500}");
    exit(0);
    0
}
"#;
    let (out, err, code) = run("grpc_unknown", main, &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("grpc-status 12 fast=true"), "{out}{err}");
}
