//! Hallazgos de raysec con 1.27.79 (#243, #244): el webserver de `net` no escribe una cabecera de
//! respuesta con CR/LF (500 y cierre), el cliente HTTP rechaza líneas plegadas, nombres con espacio
//! y valores con controles en la respuesta, trata `Content-Length` + `Transfer-Encoding` como
//! encuadre ambiguo, y una conexión keep-alive con octetos sobrantes tras una respuesta se descarta
//! en vez de entregarlos como la respuesta de la petición siguiente.
//!
//! Los servidores (el webserver y los upstreams crudos) escuchan en el puerto 0 dentro del programa.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

fn project(tag: &str, main: &str) -> PathBuf {
    let net = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/net");
    let app = std::env::temp_dir().join(format!("ray_raysec79_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{}\"\n",
            net.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

/// Corre el proyecto en la VM; stdout sin las líneas de «listening on port».
fn run(app: &PathBuf) -> (String, String) {
    let out = Command::new(BIN).arg("run").current_dir(app).output().expect("ejecuta ray");
    let stdout: String = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.contains("listening on port"))
        .map(|l| format!("{l}\n"))
        .collect();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    (stdout, stderr)
}

const PROG: &str = r#"import std/net;
import net/webserver;
import net/http;
from net/webserver import Request, Response;

// Un upstream crudo: la conexión i recibe resps[i] (la última se repite) y queda abierta.
fn upstream(resps: [string], close_after: bool) -> int {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    let _ = spawn(fn() -> int {
        var k = 0;
        while (true) {
            let c = net.tcp_accept(l).unwrap();
            let _ = net.socket_read_bytes(c);
            let i = if (k < resps.len()) { k } else { resps.len() - 1 };
            let _ = net.socket_write(c, resps[i]);
            if (close_after) { close(c); }
            k = k + 1;
        }
        0
    });
    port
}

fn show(r: Result<http.Response, string>) -> string {
    match (r) {
        Result.Ok(x) => "ok ${x.status} " + from_utf8(x.body).unwrap_or("?") + " x-injected=" + http.header(x, "x-injected").unwrap_or("-"),
        Result.Err(e) => "err " + e,
    }
}

// El servidor que copia una cabecera del cliente en la respuesta (lo que hace un proxy).
fn handle(req: Request) -> Response {
    var r = webserver.text(200, "hi");
    match (get(req.headers, "x-copy")) {
        Option.Some(v) => { r.headers.insert("X-A", v); },
        Option.None => { },
    }
    r
}

fn raw(port: int, payload: string) -> string {
    let c = net.tcp_connect("127.0.0.1", port).unwrap();
    net.set_read_timeout(c, 5000);
    let _ = net.socket_write(c, payload);
    var acc = "";
    while (true) {
        match (net.socket_read_bytes(c)) {
            Result.Ok(b) => { if (b.len() == 0) { break; } acc = acc + from_utf8(b).unwrap_or("?"); },
            Result.Err(e) => { acc = acc + "<" + e + ">"; break; },
        }
    }
    close(c);
    acc.split("\r\n")[0] + " injected=" + to_string(acc.to_lower().contains("x-injected"))
}

fn main() -> int {
    // #243 lado servidor
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let sport = net.local_port(l);
    let _ = spawn(fn() -> int { let _ = webserver.serve_on(l, handle); 0 });
    print("srv-cr " + raw(sport, "GET / HTTP/1.1\r\nHost: t\r\nX-Copy: a\rX-Injected: 1\r\nConnection: close\r\n\r\n"));
    print("srv-ok " + raw(sport, "GET / HTTP/1.1\r\nHost: t\r\nX-Copy: plain\r\nConnection: close\r\n\r\n"));
    // #243 lado cliente
    let p1 = upstream(["HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-A: a\r\n X-Injected: 1\r\n\r\nok"], true);
    print("cli-fold " + show(http.request("GET", "http://127.0.0.1:${p1}/", "")));
    let p2 = upstream(["HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-A: a\nX-Injected: 1\r\n\r\nok"], true);
    print("cli-lf " + show(http.request("GET", "http://127.0.0.1:${p2}/", "")));
    let p3 = upstream(["HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-A : a\r\n\r\nok"], true);
    print("cli-name " + show(http.request("GET", "http://127.0.0.1:${p3}/", "")));
    // #244 CL+TE
    let p4 = upstream(["HTTP/1.1 200 OK\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n"], true);
    print("cli-cl-te " + show(http.request("GET", "http://127.0.0.1:${p4}/", "")));
    // #244 sobrante en keep-alive: la 2.ª petición no recibe la respuesta encolada.
    let p5 = upstream([
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nokHTTP/1.1 200 OK\r\nContent-Length: 3\r\nX-Injected: 1\r\n\r\nbad",
        "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nfine"
    ], false);
    let c = http.connect("http://127.0.0.1:${p5}").unwrap();
    print("ka-1 " + show(http.conn_request(c, "GET", "/a", "", Map.new())));
    print("ka-2 " + show(http.conn_request(c, "GET", "/b", "", Map.new())));
    0
}
"#;

#[test]
fn response_headers_cannot_be_split_and_leftover_bytes_drop_the_connection() {
    let (out, err) = run(&project("split", PROG));
    let want = "srv-cr HTTP/1.1 500 Internal Server Error injected=false\n\
srv-ok HTTP/1.1 200 OK injected=false\n\
cli-fold err malformed response header (obsolete line folding)\n\
cli-lf err malformed response header value for X-A (CR, LF or another control character)\n\
cli-name err malformed response header name: X-A \n\
cli-cl-te err ambiguous response framing: Transfer-Encoding and Content-Length together\n\
ka-1 ok 200 ok x-injected=-\n\
ka-2 ok 200 fine x-injected=-\n";
    assert_eq!(out, want);
    assert!(
        err.contains("response: invalid value for response header X-A (CR, LF or another control character)"),
        "el servidor avisa por stderr de la cabecera rechazada\n{err}"
    );
}
