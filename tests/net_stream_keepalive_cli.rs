//! raycrawl #370: streaming sobre una conexión persistente (`http.conn_stream`, `http.pool_stream`).
//! El servidor y el cliente viven en el mismo proceso; el servidor devuelve en el cuerpo el puerto
//! remoto del cliente, que identifica la conexión TCP por la que llegó la petición.
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

const MAIN: &str = r#"import std/net;
import net/webserver;
import net/http;
from net/webserver import Request, Response;

fn handler(req: Request) -> Response {
    if (req.path == "/empty") {
        return webserver.text(204, "");
    }
    var pad = "";
    if (req.path == "/big") {
        for _ in 0..4096 { pad = pad + "0123456789abcdef"; }
    }
    webserver.text(200, req.remote + "|" + pad)
}

// Lee el cuerpo entero y devuelve lo que hay antes de la barra (la dirección remota).
fn drain(s: http.Stream) -> string {
    var all = b"";
    while (true) {
        match (http.stream_read(s)) {
            Result.Ok(Option.Some(piece)) => { all = all + piece; },
            Result.Ok(Option.None) => { break; },
            Result.Err(e) => { return "ERR " + e; },
        }
    }
    let text = from_utf8(all).unwrap_or("?");
    match (text.index_of("|")) {
        Option.Some(at) => text.substring(0, at),
        Option.None => text,
    }
}

fn main() -> int {
    let listener = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(listener);
    let _ = spawn(fn() -> int {
        let _ = webserver.serve_options_on(listener, webserver.options().quiet(), fn() -> fn(Request) -> Response { handler });
        0
    });
    let none: Map<string, string> = Map.new();
    let base = "http://127.0.0.1:${port}";

    // 1. Pool: cinco streams leídos hasta el final van por UNA conexión.
    let p = http.pool(1);
    var first = "";
    var same = true;
    for i in 0..5 {
        let s = http.pool_stream(p, "GET", base + "/big", b"", none, 5000).unwrap();
        let who = drain(s);
        if (i == 0) { first = who; }
        if (who != first) { same = false; }
    }
    print("pool reuses the connection: ${same}");

    // 2. Cortar a mitad descarta el socket: el siguiente stream llega por otra conexión.
    let cut = http.pool_stream(p, "GET", base + "/big", b"", none, 5000).unwrap();
    let _ = http.stream_read(cut);
    http.stream_close(cut);
    let after = drain(http.pool_stream(p, "GET", base + "/big", b"", none, 5000).unwrap());
    print("an early close drops it: ${after != first && !after.starts_with("ERR")}");

    // 3. El hueco vuelve al pool: una petición normal lo usa, y por la misma conexión.
    let r = http.pool_request(p, "GET", base + "/small", "", none).unwrap();
    print("the slot is back: ${from_utf8(r.body).unwrap_or("?").starts_with(after)}");

    // 4. Una respuesta sin cuerpo termina sola y no retiene el hueco.
    let e = http.pool_stream(p, "GET", base + "/empty", b"", none, 5000).unwrap();
    print("204: ${e.status} ${http.stream_read(e)}");
    let r2 = http.pool_request(p, "GET", base + "/small", "", none).unwrap();
    print("still the same connection: ${from_utf8(r2.body).unwrap_or("?").starts_with(after)}");
    http.pool_close(p);

    // 5. Sobre una Conn: stream, petición normal y stream, todo por el mismo socket.
    let c = http.connect(base).unwrap();
    let a = drain(http.conn_stream(c, "GET", "/big", b"", none, 5000).unwrap());
    let b = http.conn_request(c, "GET", "/small", "", none).unwrap();
    let d = drain(http.conn_stream(c, "GET", "/small", b"", none, 5000).unwrap());
    print("conn: ${a == d && from_utf8(b.body).unwrap_or("?").starts_with(a) && !a.starts_with("ERR")}");
    http.conn_close(c);
    exit(0);
    0
}
"#;

fn run(flags: &[&str], name: &str) -> (String, String, i32) {
    let base = std::env::temp_dir().join(format!("ray_net_stream_ka_{name}"));
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

const EXPECTED: &str = "pool reuses the connection: true\n\
an early close drops it: true\n\
the slot is back: true\n\
204: 204 Result.Ok(Option.None)\n\
still the same connection: true\n\
conn: true\n";

#[test]
fn a_stream_read_to_the_end_returns_its_connection() {
    let (out, err, code) = run(&["run"], "vm");
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out, EXPECTED, "{err}");
}
