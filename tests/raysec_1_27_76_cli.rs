//! Hallazgos de raysec (revisión de seguridad con 1.27.76): el webserver de `net` (#229 tamaño de
//! chunk que desbordaba, #230 Content-Length en conflicto, y de paso Content-Length no numérico y
//! Transfer-Encoding + Content-Length), `Request.raw_path` y el cliente que rechaza CR/LF (#237),
//! el `Host` del servidor HTTP de `mcp` (#231), los plazos del servidor de `rpc` (#235) y la
//! aritmética sin desbordar de `std/math` (#233), también en nativo.
//!
//! Los servidores escuchan en el puerto 0 dentro del propio programa de prueba.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

/// Un proyecto temporal que depende de `net`, `mcp` y `rpc` por ruta, con `main` como programa.
fn project(tag: &str, main: &str) -> PathBuf {
    let packages = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages");
    let p = |n: &str| packages.join(n).display().to_string().replace('\\', "/");
    let app = std::env::temp_dir().join(format!("ray_raysec_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{}\"\nmcp = \"path:{}\"\nrpc = \"path:{}\"\n",
            p("net"),
            p("mcp"),
            p("rpc")
        ),
    )
    .unwrap();
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

/// Corre el proyecto en la VM; stdout sin las líneas de «listening on port» de los servidores.
fn run(app: &PathBuf) -> String {
    let out = Command::new(BIN).arg("run").current_dir(app).output().expect("ejecuta ray");
    let stdout: String = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.contains("listening on port"))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    stdout
}

const NET_SERVER: &str = r#"import std/net;
import net/webserver;
import net/http;
from net/webserver import Request, Response;

fn handle(req: Request) -> Response {
    webserver.text(200, req.path + "|" + req.raw_path + "|" + webserver.request_text(req).unwrap_or("?"))
}

// La primera línea de la respuesta y, tras un 200, el cuerpo; la conexión debe acabar cerrada.
fn raw(port: int, payload: string) -> string {
    let c = net.tcp_connect("127.0.0.1", port).unwrap();
    net.set_read_timeout(c, 5000);
    let _ = net.socket_write(c, payload);
    var acc = "";
    while (true) {
        match (net.socket_read_bytes(c)) {
            Result.Ok(b) => {
                if (b.len() == 0) {
                    break;
                }
                acc = acc + from_utf8(b).unwrap_or("?");
            },
            Result.Err(e) => {
                acc = acc + "\r\n<" + e + ">";
                break;
            },
        }
    }
    close(c);
    let head = acc.split("\r\n")[0];
    if (head.contains("200")) {
        let parts = acc.split("\r\n\r\n");
        head + " " + parts[parts.len() - 1]
    } else {
        head
    }
}

fn main() -> int {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    let _ = spawn(fn() -> int {
        let _ = webserver.serve_on(l, handle);
        0
    });
    let h = "POST /x HTTP/1.1\r\nHost: t\r\n";
    // #229: 17 dígitos hex de tamaño de chunk → 400 y cierre (antes: sin respuesta ni cierre).
    print("chunk17 " + raw(port, h + "Transfer-Encoding: chunked\r\n\r\n10000000000000000\r\nab\r\n0\r\n\r\n"));
    print("chunk15 " + raw(port, h + "Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n000000000000002\r\nab\r\n0\r\n\r\n"));
    // #230: dos Content-Length distintos → 400; iguales valen.
    print("cl-conflict " + raw(port, h + "Content-Length: 2\r\nContent-Length: 3\r\n\r\nabc"));
    print("cl-same " + raw(port, h + "Content-Length: 2\r\nContent-Length: 2\r\nConnection: close\r\n\r\nab"));
    print("cl-alpha " + raw(port, h + "Content-Length: abc\r\n\r\n"));
    print("cl-negative " + raw(port, h + "Content-Length: -5\r\n\r\n"));
    print("te-and-cl " + raw(port, h + "Transfer-Encoding: chunked\r\nContent-Length: 2\r\n\r\n2\r\nab\r\n0\r\n\r\n"));
    // #237: la ruta original llega aparte de la descodificada (`path` lleva el CR/LF de verdad; el
    // `lines()` del test se come el CR).
    print("raw " + raw(port, "GET /a%20b/..%2fc%0d%0a?q=1 HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n"));
    // #237, cliente: CR/LF en la URL o en una cabecera no salen a la red.
    let base = "http://127.0.0.1:" + to_string(port);
    match (http.request("GET", base + "/a\r\nX-Evil: 1", "")) {
        Result.Ok(_) => print("url: sent"),
        Result.Err(e) => print("url: " + e),
    }
    var hs: Map<string, string> = Map.new();
    hs.insert("X-Trace", "1\r\nX-Evil: 1");
    match (http.request_with("GET", base + "/ok", "", hs)) {
        Result.Ok(_) => print("header: sent"),
        Result.Err(e) => print("header: " + e),
    }
    match (http.request("GET", base + "/fine", "")) {
        Result.Ok(r) => print("fine: ${r.status}"),
        Result.Err(e) => print("fine: " + e),
    }
    0
}
"#;

#[test]
fn the_webserver_rejects_ambiguous_framing_and_keeps_the_raw_path() {
    let out = run(&project("net", NET_SERVER));
    let want = "chunk17 HTTP/1.1 400 Bad Request\n\
chunk15 HTTP/1.1 200 OK /x|/x|ab\n\
cl-conflict HTTP/1.1 400 Bad Request\n\
cl-same HTTP/1.1 200 OK /x|/x|ab\n\
cl-alpha HTTP/1.1 400 Bad Request\n\
cl-negative HTTP/1.1 400 Bad Request\n\
te-and-cl HTTP/1.1 400 Bad Request\n\
raw HTTP/1.1 200 OK /a b/../c\n|/a%20b/..%2fc%0d%0a|\n\
url: invalid URL (control character or space; percent-encode it): http://127.0.0.1:PORT/a\\r\\nX-Evil: 1\n\
header: invalid value for header X-Trace (CR, LF or another control character)\n\
fine: 200\n";
    let port_free: String = out
        .lines()
        .map(|l| match l.find("127.0.0.1:") {
            Some(i) => {
                let rest = &l[i + 10..];
                let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
                format!("{}127.0.0.1:PORT{}\n", &l[..i], &rest[digits..])
            }
            None => format!("{l}\n"),
        })
        .collect();
    assert_eq!(port_free, want);
}

const MCP_HOSTS: &str = r#"import mcp/serve;
from net/webserver import Request;

fn status(p: serve.Provider, host: string, origin: string) -> int {
    var h: Map<string, string> = Map.new();
    if (host != "") {
        h.insert("host", host);
    }
    if (origin != "") {
        h.insert("origin", origin);
    }
    let req = Request {
        method: "POST",
        path: "/mcp",
        raw_path: "/mcp",
        query: "",
        version: "HTTP/1.1",
        headers: h,
        body: `{"jsonrpc":"2.0","id":1,"method":"ping"}`.to_bytes(),
        remote: "127.0.0.1:5000"
    };
    serve.answer(p, req).status
}

fn main() -> int {
    let p = serve.provider("t", "1");
    // #231: una página de evil.example que resuelve a 127.0.0.1 manda Origin y Host iguales.
    print("rebinding ${status(p, "evil.example:8765", "http://evil.example:8765")}");
    print("no-origin ${status(p, "evil.example:8765", "")}");
    print("localhost ${status(p, "localhost:8765", "http://localhost:8765")}");
    print("v4 ${status(p, "127.0.0.1:8765", "")} v6 ${status(p, "[::1]:8765", "")}");
    print("cross ${status(p, "127.0.0.1:8765", "http://evil.example")}");
    p.hosts = ["mcp.lan", "box:9000"];
    print("listed ${status(p, "MCP.lan:1", "")} port ${status(p, "box:9000", "")} other-port ${status(p, "box:9001", "")}");
    let q = serve.provider("t", "1");
    q.token = "s3cret";
    print("token-any-host ${status(q, "evil.example", "")}");
    0
}
"#;

#[test]
fn the_mcp_http_server_answers_only_loopback_or_listed_hosts_without_a_token() {
    let out = run(&project("mcp", MCP_HOSTS));
    assert_eq!(
        out,
        "rebinding 403\nno-origin 403\nlocalhost 200\nv4 200 v6 200\ncross 403\nlisted 200 port 200 other-port 403\ntoken-any-host 401\n"
    );
}

const RPC_TIMEOUTS: &str = r#"import std/net;
import std/time;
import rpc/rpc;
from std/json import Json;

fn handler(req: rpc.Req) -> Result<Json, string> {
    Result.Ok(Json.JStr("pong " + req.method))
}

// Cuánto tarda el servidor en cerrar una conexión a la que se mandó `pre`.
fn closes_after(port: int, pre: bytes) -> string {
    let c = net.tcp_connect("127.0.0.1", port).unwrap();
    net.set_read_timeout(c, 5000);
    if (pre.len() > 0) {
        let _ = net.socket_write_bytes(c, pre);
    }
    let t0 = time.monotonic();
    let r = net.socket_read_bytes(c);
    let ms = time.monotonic() - t0;
    close(c);
    let what = match (r) {
        Result.Ok(b) => if (b.len() == 0) { "closed" } else { "data" },
        Result.Err(e) => e,
    };
    let fast = if (ms < 2500) { "fast" } else { "slow" };
    what + " " + fast
}

fn serve_with(limits: rpc.Limits) -> int {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    let stop: Channel<int> = Channel.new();
    let _ = spawn(fn() -> int {
        let _ = rpc.serve_on_shutdown_limits(l, limits, stop, 0, handler);
        0
    });
    port
}

fn main() -> int {
    let d = rpc.default_limits();
    print("defaults ${d.max_frame_bytes} ${d.frame_timeout_ms} ${d.idle_timeout_ms}");
    // #235: un frame a medias se corta al vencer frame_timeout_ms.
    let a = serve_with(rpc.Limits { max_frame_bytes: 1000, frame_timeout_ms: 300, idle_timeout_ms: 0 });
    print("half-frame " + closes_after(a, bytes_of([0, 0])));
    // Y una conexión ociosa, con idle_timeout_ms.
    let b = serve_with(rpc.Limits { max_frame_bytes: 1000, frame_timeout_ms: 300, idle_timeout_ms: 300 });
    print("idle " + closes_after(b, b""));
    // Una llamada normal sigue funcionando con los plazos puestos.
    let c = rpc.connect("127.0.0.1", b).unwrap();
    match (rpc.call(c, "ping", Json.JNull)) {
        Result.Ok(Json.JStr(s)) => print("call " + s),
        Result.Ok(_) => print("call ?"),
        Result.Err(e) => print("call err " + e),
    }
    rpc.disconnect(c);
    0
}
"#;

#[test]
fn the_rpc_server_closes_half_frames_and_idle_connections() {
    let out = run(&project("rpc", RPC_TIMEOUTS));
    assert_eq!(out, "defaults 10485760 30000 0\nhalf-frame closed fast\nidle closed fast\ncall pong ping\n");
}

const MATH: &str = r#"import std/math;

fn show(o: Option<int>) -> string {
    match (o) {
        Option.Some(v) => to_string(v),
        Option.None => "None",
    }
}

fn main() -> int {
    let hi = math.MAX_INT;
    let lo = math.MIN_INT;
    print("${hi} ${lo}");
    print(show(math.checked_add(hi, 1)) + " " + show(math.checked_add(hi, -1)) + " " + show(math.checked_add(lo, -1)));
    print(show(math.checked_sub(lo, 1)) + " " + show(math.checked_sub(0, lo)) + " " + show(math.checked_sub(-1, lo)));
    print(show(math.checked_mul(lo, -1)) + " " + show(math.checked_mul(3037000499, 3037000499)) + " " + show(math.checked_mul(3037000500, 3037000500)) + " " + show(math.checked_mul(-4611686018427387904, 2)) + " " + show(math.checked_mul(0, lo)));
    print("${math.saturating_add(hi, 5)} ${math.saturating_sub(lo, 5)} ${math.saturating_mul(lo, 2)} ${math.saturating_mul(lo, -2)} ${math.saturating_add(2, 3)}");
    // Las puras de siempre, que en nativo eran «not supported».
    print("${math.gcd(12, 18)} ${math.lcm(4, 6)} ${math.clamp(9, 0, 3)} ${math.iabs(-4)} ${math.sign(-7)} ${math.ipow(2, 10)} ${math.factorial(5)} ${math.is_prime(97)}");
    0
}
"#;

#[test]
fn checked_and_saturating_math_agree_in_every_engine() {
    let d = std::env::temp_dir().join(format!("ray_raysec_math_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), MATH).unwrap();
    let want = "9223372036854775807 -9223372036854775808\n\
None 9223372036854775806 None\n\
None None 9223372036854775807\n\
None 9223372030926249001 None -9223372036854775808 0\n\
9223372036854775807 -9223372036854775808 -9223372036854775808 9223372036854775807 5\n\
6 12 3 4 -1 1024 120 true\n";
    for flags in [&[][..], &["--interp"][..]] {
        let mut args = vec!["run"];
        args.extend_from_slice(flags);
        args.push("prog.ray");
        let out = Command::new(BIN).args(&args).current_dir(&d).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "{flags:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = d.join("prog_native");
        let out = Command::new(BIN).args(["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let o = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "nativo");
    }
}

/// #232/#234/#236: la doc dice lo que pasa.
#[test]
fn the_docs_say_what_spawn_fs_and_net_do() {
    let doc = |q: &str| String::from_utf8_lossy(&Command::new(BIN).args(["doc", q]).output().unwrap().stdout).into_owned();
    assert!(doc("spawn").contains("is never joined ends silently"));
    assert!(doc("fs.is_file").contains("Follows symlinks"));
    assert!(doc("fs.is_dir").contains("Follows symlinks"));
    assert!(doc("net.socket_write_bytes").contains("`set_write_timeout` deadline passes"));
    assert!(doc("net.set_write_timeout").contains("fails with the stable error \"write timeout\""));
    assert!(doc("net.set_read_timeout").contains("Reads only"));
    assert!(doc("math.checked_mul").contains("or `None` if it overflows"));
}
