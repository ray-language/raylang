//! net 0.17.0 (raygrpc #385/#386/#387): handlers de streaming en `net/grpc_server`, streams en
//! `net/grpc_conn`, y cabeceras y trailers de respuesta. Servidor y cliente en el mismo proceso.
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

const MAIN: &str = r#"import net/grpc_server;
import net/grpc_conn;
import net/grpc_status;
import net/grpc_reflection;
import std/protobuf;
from net/grpc_status import Status;
from net/grpc_server import Call, ServerStream;

fn num(b: bytes) -> int {
    parse_int(from_utf8(b).unwrap_or("0")).unwrap_or(0)
}

// Unaria con cabecera y trailer propios.
fn say(c: Call) -> Result<bytes, Status> {
    grpc_server.set_header(c, "X-Request-Id", "r-1");
    grpc_server.set_trailer(c, "x-cost", "3");
    if (c.message == b"boom") {
        return grpc_status.fail(grpc_status.INVALID_ARGUMENT, "boom is not allowed");
    }
    Result.Ok(c.message)
}

// Server-streaming: un número N entra, salen N mensajes.
fn count(s: ServerStream) -> Status {
    grpc_server.stream_header(s, "x-kind", "count");
    let n = match (grpc_server.recv(s)) {
        Result.Ok(Option.Some(m)) => num(m),
        Result.Ok(Option.None) => 0,
        Result.Err(st) => { return st; },
    };
    for i in 0..n {
        match (grpc_server.send(s, to_string(i + 1).to_bytes())) {
            Result.Ok(_) => { },
            Result.Err(st) => { return st; },
        }
    }
    grpc_server.stream_trailer(s, "x-sent", to_string(n));
    grpc_status.new(grpc_status.OK, "")
}

// Client-streaming: suma lo que llega y responde una vez.
fn sum(s: ServerStream) -> Status {
    var total = 0;
    while (true) {
        match (grpc_server.recv(s)) {
            Result.Ok(Option.Some(m)) => { total = total + num(m); },
            Result.Ok(Option.None) => { break; },
            Result.Err(st) => { return st; },
        }
    }
    match (grpc_server.send(s, to_string(total).to_bytes())) {
        Result.Ok(_) => grpc_status.new(grpc_status.OK, ""),
        Result.Err(st) => st,
    }
}

// Bidireccional: cada mensaje vuelve al momento; "stop" termina con un estado de error.
fn echo(s: ServerStream) -> Status {
    while (true) {
        match (grpc_server.recv(s)) {
            Result.Ok(Option.Some(m)) => {
                if (m == b"stop") {
                    return grpc_status.new(grpc_status.ABORTED, "stopped by the client");
                }
                match (grpc_server.send(s, b"<" + m + b">")) {
                    Result.Ok(_) => { },
                    Result.Err(st) => { return st; },
                }
            },
            Result.Ok(Option.None) => { break; },
            Result.Err(st) => { return st; },
        }
    }
    grpc_status.new(grpc_status.OK, "")
}

fn build() -> grpc_server.Router {
    let r = grpc_server.router()
        .route("/t.S/Say", say)
        .route_stream("/t.S/Count", count)
        .route_stream("/t.S/Sum", sum)
        .route_stream("/t.S/Echo", echo);
    grpc_reflection.enable(r, [])
}

fn text(r: Result<Option<bytes>, string>) -> string {
    match (r) {
        Result.Ok(Option.Some(m)) => from_utf8(m).unwrap_or("?"),
        Result.Ok(Option.None) => "<end>",
        Result.Err(e) => "ERR " + e,
    }
}

fn main() -> int {
    let l = grpc_server.bind("127.0.0.1", 0).unwrap();
    let port = l.port;
    let stop: Channel<int> = Channel.bounded(1);
    let server = spawn(fn() -> int {
        match (grpc_server.serve_router_until(l, stop, grpc_server.default_options(), build)) {
            Result.Ok(_) => 0,
            Result.Err(e) => { print("serve: " + e); 1 },
        }
    });
    let c = grpc_conn.connect("127.0.0.1", port).unwrap();
    let none: Map<string, string> = Map.new();

    // 1. Unaria: cabeceras y trailers separados, también con un estado de error.
    let r = grpc_conn.call(c, "/t.S/Say", b"hola", none, 2000).unwrap();
    print("say ${r.status} ${from_utf8(r.message).unwrap_or("?")} h=${r.headers.get_or("x-request-id", "-")} t=${r.trailers.get_or("x-cost", "-")} h.has(x-cost)=${r.headers.contains_key("x-cost")}");
    let e = grpc_conn.call(c, "/t.S/Say", b"boom", none, 2000).unwrap();
    print("boom ${grpc_status.code_name(e.status)} ${e.status_message} t=${e.trailers.get_or("x-cost", "-")}");

    // 2. Server-streaming.
    let s = grpc_conn.open_stream(c, "/t.S/Count", none, 2000).unwrap();
    let _ = grpc_conn.stream_send(s, b"3");
    let _ = grpc_conn.stream_close_send(s);
    var got = "";
    while (true) {
        let t = text(grpc_conn.stream_recv(s));
        if (t == "<end>" || t.starts_with("ERR")) { got = got + t; break; }
        got = got + t + " ";
    }
    let rs = grpc_conn.stream_reply(s);
    print("count ${got} status=${rs.status} h=${rs.headers.get_or("x-kind", "-")} t=${rs.trailers.get_or("x-sent", "-")}");

    // 3. Client-streaming.
    let s2 = grpc_conn.open_stream(c, "/t.S/Sum", none, 2000).unwrap();
    for n in ["10", "20", "12"] {
        let _ = grpc_conn.stream_send(s2, n.to_bytes());
    }
    let _ = grpc_conn.stream_close_send(s2);
    print("sum ${text(grpc_conn.stream_recv(s2))} ${text(grpc_conn.stream_recv(s2))} status=${grpc_conn.stream_reply(s2).status}");

    // 4. Bidireccional: cada envío recibe su respuesta antes del siguiente; "stop" da un error.
    let s3 = grpc_conn.open_stream(c, "/t.S/Echo", none, 2000).unwrap();
    let _ = grpc_conn.stream_send(s3, b"a");
    let first = text(grpc_conn.stream_recv(s3));
    let _ = grpc_conn.stream_send(s3, b"b");
    let second = text(grpc_conn.stream_recv(s3));
    let _ = grpc_conn.stream_send(s3, b"stop");
    let third = text(grpc_conn.stream_recv(s3));
    let rs3 = grpc_conn.stream_reply(s3);
    print("echo ${first} ${second} ${third} ${grpc_status.code_name(rs3.status)} ${rs3.status_message}");

    // 5. Cancelar un stream deja la conexión usable.
    let s4 = grpc_conn.open_stream(c, "/t.S/Echo", none, 2000).unwrap();
    let _ = grpc_conn.stream_send(s4, b"x");
    let _ = grpc_conn.stream_recv(s4);
    grpc_conn.stream_cancel(s4);
    print("cancel ${grpc_status.code_name(grpc_conn.stream_reply(s4).status)}");
    let again = grpc_conn.call(c, "/t.S/Say", b"otra", none, 2000).unwrap();
    print("again ${again.status} ${from_utf8(again.message).unwrap_or("?")} usable=${grpc_conn.usable(c)}");

    // 6. Una ruta de streaming llamada como unaria también funciona (un mensaje y fin).
    let u = grpc_conn.call(c, "/t.S/Sum", b"7", none, 2000).unwrap();
    print("unary-on-stream ${u.status} ${from_utf8(u.message).unwrap_or("?")}");

    // 7. Plazo: un stream bidireccional al que nadie escribe vence en el cliente.
    let s5 = grpc_conn.open_stream(c, "/t.S/Echo", none, 150).unwrap();
    print("deadline ${text(grpc_conn.stream_recv(s5))} ${grpc_status.code_name(grpc_conn.stream_reply(s5).status)}");
    let last = grpc_conn.call(c, "/t.S/Say", b"fin", none, 2000).unwrap();
    print("last ${last.status} ${from_utf8(last.message).unwrap_or("?")}");

    // 8. Reflexión: la lista de servicios sale de las rutas; sin descriptores, describe da NOT_FOUND.
    let rf = grpc_conn.open_stream(c, "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo", none, 2000).unwrap();
    let ask = protobuf.writer();
    protobuf.write_string(ask, 7, "");
    let _ = grpc_conn.stream_send(rf, protobuf.finish(ask));
    var names = "";
    match (grpc_conn.stream_recv(rf)) {
        Result.Ok(Option.Some(m)) => {
            let list = protobuf.get_message(protobuf.parse(m).unwrap(), 6).unwrap();
            for svc in protobuf.get_all(list, 1) {
                names = names + protobuf.get_string(protobuf.parse(svc.bytes_val).unwrap(), 1) + " ";
            }
        },
        _ => { names = "?"; },
    }
    let ask2 = protobuf.writer();
    protobuf.write_string(ask2, 4, "t.S");
    let _ = grpc_conn.stream_send(rf, protobuf.finish(ask2));
    var code = 0 - 1;
    match (grpc_conn.stream_recv(rf)) {
        Result.Ok(Option.Some(m)) => {
            code = protobuf.get_int(protobuf.get_message(protobuf.parse(m).unwrap(), 7).unwrap(), 1);
        },
        _ => { },
    }
    let _ = grpc_conn.stream_close_send(rf);
    print("reflection ${names.trim()} | symbol error ${code} | ${text(grpc_conn.stream_recv(rf))} ${grpc_conn.stream_reply(rf).status}");

    grpc_conn.disconnect(c);
    send(stop, 1);
    join(server)
}
"#;

const EXPECTED: &str = "say 0 hola h=r-1 t=3 h.has(x-cost)=false\n\
boom INVALID_ARGUMENT boom is not allowed t=3\n\
count 1 2 3 <end> status=0 h=count t=3\n\
sum 42 <end> status=0\n\
echo <a> <b> <end> ABORTED stopped by the client\n\
cancel CANCELLED\n\
again 0 otra usable=true\n\
unary-on-stream 0 7\n\
deadline <end> DEADLINE_EXCEEDED\n\
last 0 fin\n\
reflection grpc.reflection.v1.ServerReflection grpc.reflection.v1alpha.ServerReflection t.S | symbol error 5 | <end> 0\n";

fn run(flags: &[&str], name: &str) -> (String, String, i32) {
    let base = std::env::temp_dir().join(format!("ray_grpc_streaming_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    let repo = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"probe\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(base.join("src/main.ray"), MAIN).unwrap();
    let out = Command::new(BIN).args(flags).current_dir(&base).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn streaming_handlers_and_response_metadata() {
    let (out, err, code) = run(&["run"], "vm");
    assert_eq!(out, EXPECTED, "{err}");
    assert_eq!(code, 0, "{err}");
}

/// `grpcurl` deja abierto el stream de reflexión y hace su llamada por la misma conexión: un
/// handler de streaming que espera su siguiente mensaje no bloquea las demás llamadas. Se habla
/// HTTP/2 a mano (dos streams a la vez sobre un `Link`), porque `grpc_conn` lleva uno por conexión.
const INTERLEAVED: &str = r#"import std/net;
import std/time;
import net/grpc_server;
import net/grpc_status;
import net/grpc_h2 as h2;
from net/http2 import preface, settings_empty;
from net/hpack import HEntry;
from std/protobuf import grpc_frame;
from net/grpc_status import Status;
from net/grpc_server import Call, ServerStream;

fn say(c: Call) -> Result<bytes, Status> {
    Result.Ok(c.message)
}

fn echo(s: ServerStream) -> Status {
    while (true) {
        match (grpc_server.recv(s)) {
            Result.Ok(Option.Some(m)) => {
                match (grpc_server.send(s, b"<" + m + b">")) {
                    Result.Ok(_) => { },
                    Result.Err(st) => { return st; },
                }
            },
            Result.Ok(Option.None) => { break; },
            Result.Err(st) => { return st; },
        }
    }
    grpc_status.new(grpc_status.OK, "")
}

fn build() -> grpc_server.Router {
    grpc_server.router().route("/t.S/Say", say).route_stream("/t.S/Echo", echo)
}

fn head(path: string) -> [HEntry] {
    [
        h2.hdr(":method", "POST"),
        h2.hdr(":scheme", "http"),
        h2.hdr(":path", path),
        h2.hdr(":authority", "127.0.0.1"),
        h2.hdr("content-type", "application/grpc"),
        h2.hdr("te", "trailers")
    ]
}

fn ignore(fr: h2.Frame) -> Result<int, string> {
    Result.Ok(0)
}

// Lee frames hasta el primer DATA del stream `id`; devuelve su mensaje (sin el prefijo gRPC).
fn first_data(link: h2.Link, id: int) -> string {
    while (true) {
        match (h2.read_frame(link)) {
            Result.Ok(fr) => {
                if (fr.ftype == h2.DATA && fr.stream_id == id) {
                    let _ = h2.release(link, fr, true);
                    return from_utf8(fr.payload.sub_bytes(5, fr.payload.len())).unwrap_or("?");
                }
                if (fr.ftype == h2.HEADERS) {
                    let _ = h2.decode_headers(link, fr.payload);
                } else {
                    let _ = h2.control(link, fr);
                }
            },
            Result.Err(e) => { return "ERR " + e; },
        }
    }
    "?"
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
    // Stream 1: bidireccional, abierto y a la espera.
    h2.open_stream(link, 1);
    let _ = h2.send_headers(link, 1, head("/t.S/Echo"), false);
    let _ = h2.send_data(link, 1, grpc_frame(b"one"), false, ignore);
    print("stream 1: " + first_data(link, 1));
    // Stream 3: una llamada unaria entera mientras el 1 sigue abierto.
    h2.open_stream(link, 3);
    let _ = h2.send_headers(link, 3, head("/t.S/Say"), false);
    let _ = h2.send_data(link, 3, grpc_frame(b"unary"), true, ignore);
    print("stream 3: " + first_data(link, 3));
    // El stream 1 sigue vivo.
    let _ = h2.send_data(link, 1, grpc_frame(b"two"), false, ignore);
    print("stream 1: " + first_data(link, 1));
    exit(0);
    0
}
"#;

#[test]
fn a_waiting_stream_handler_does_not_block_the_other_calls_of_its_connection() {
    let base = std::env::temp_dir().join("ray_grpc_streaming_interleaved");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    let repo = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"probe\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(base.join("src/main.ray"), INTERLEAVED).unwrap();
    let out = Command::new(BIN).arg("run").current_dir(&base).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "stream 1: <one>\nstream 3: unary\nstream 1: <two>\n", "{}", String::from_utf8_lossy(&out.stderr));
}
