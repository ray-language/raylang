//! Hallazgos de rayreq con 1.27.82: `http.request*` termina al completar `Content-Length` aunque
//! el servidor no cierre (#275), una cabecera `Host`/`Connection` del llamador sustituye a la del
//! cliente y un `Content-Length` que no cuadra o un `Transfer-Encoding` son `Err` (#277), y los
//! errores de `json.parse`/`parse_doc` dicen línea y columna, saltan el BOM y nombran lo que hay
//! bajo el cursor (#276). VM; los servidores escuchan en el puerto 0.

use std::path::{Path, PathBuf};
use std::process::Command;

fn project(tag: &str, dep: &str, main: &str) -> PathBuf {
    let pkg = Path::new(env!("CARGO_MANIFEST_DIR")).join("packages").join(dep);
    let app = std::env::temp_dir().join(format!("ray_rayreq82_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n{dep} = \"path:{}\"\n", pkg.display().to_string().replace('\\', "/")),
    )
    .unwrap();
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

fn run(app: &PathBuf) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).arg("run").current_dir(app).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(app);
    String::from_utf8_lossy(&out.stdout).to_string()
}

// Un servidor TCP «mentiroso»: responde con Content-Length y NO cierra; y uno «espejo» que
// devuelve la cabecera de la petición tal como la recibió.
const NET: &str = r##"import std/net;
import std/time;
import net/http;

fn liar(l: int) -> int {
    while (true) {
        let h = net.tcp_accept(l).unwrap();
        let _ = net.socket_read(h);
        let _ = net.socket_write(h, "HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi");
        // sin cerrar: el cliente debe terminar al completar Content-Length
        time.sleep(3000);
        close(h);
    }
    0
}

fn mirror(l: int) -> int {
    while (true) {
        let h = net.tcp_accept(l).unwrap();
        let req = net.socket_read(h).unwrap();
        let head = req.split("\r\n\r\n")[0];
        let body = head.replace("\r\n", "|");
        let _ = net.socket_write(h, "HTTP/1.1 200 OK\r\nContent-Length: " + to_string(body.to_bytes().len()) + "\r\nConnection: close\r\n\r\n" + body);
        close(h);
    }
    0
}

fn show(r: Result<http.Response, string>) -> string {
    match (r) {
        Result.Ok(x) => "ok " + to_string(x.status) + " " + http.body_text(x).unwrap_or("?"),
        Result.Err(e) => "err " + e,
    }
}

fn main() {
    let l1 = net.tcp_listen("127.0.0.1", 0).unwrap();
    let p1 = net.local_port(l1);
    let _ = spawn(fn() -> int { liar(l1) });
    let t0 = time.monotonic();
    print("liar " + show(http.request_bytes("GET", "http://127.0.0.1:${p1}/", b"", Map.new(), 5000)));
    print("fast " + to_string(time.monotonic() - t0 < 2000));

    let l2 = net.tcp_listen("127.0.0.1", 0).unwrap();
    let p2 = net.local_port(l2);
    let _ = spawn(fn() -> int { mirror(l2) });
    let base = "http://127.0.0.1:${p2}/x";
    print("plain " + show(http.request_pairs("POST", base, b"hello", [], 5000)));
    print("host " + show(http.request_pairs("POST", base, b"hello", [("Host", "example.com"), ("Connection", "keep-alive"), ("Content-Length", "5")], 5000)));
    print("badlen " + show(http.request_pairs("POST", base, b"hello", [("Content-Length", "7")], 5000)));
    print("te " + show(http.request_pairs("POST", base, b"hello", [("Transfer-Encoding", "chunked")], 5000)));
    print("twolen " + show(http.request_pairs("POST", base, b"hello", [("Content-Length", "5"), ("Content-Length", "99")], 5000)));
    print("zero " + show(http.request_pairs("POST", base, b"", [("Content-Length", "0")], 5000)));
}
"##;

#[test]
fn content_length_ends_the_response_and_caller_headers_replace_the_clients() {
    let out = run(&project("net", "net", NET));
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("liar ok 200 hi"), "{out}");
    assert_eq!(lines.next(), Some("fast true"), "{out}");
    let plain = out.lines().nth(2).unwrap();
    assert!(plain.starts_with("plain ok 200 POST /x HTTP/1.1|Host: 127.0.0.1:"), "{out}");
    assert!(plain.ends_with("|Connection: close|Accept-Encoding: gzip|Content-Length: 5"), "{out}");
    assert_eq!(
        out.lines().nth(3),
        Some("host ok 200 POST /x HTTP/1.1|Host: example.com|Connection: keep-alive|Accept-Encoding: gzip|Content-Length: 5"),
        "{out}"
    );
    assert_eq!(
        out.lines().nth(4),
        Some("badlen err Content-Length 7 does not match the body (5 octets); the client sets it from the body"),
        "{out}"
    );
    assert_eq!(
        out.lines().nth(5),
        Some("te err Transfer-Encoding is not supported in requests: the client frames the body with Content-Length"),
        "{out}"
    );
    assert_eq!(
        out.lines().nth(6),
        Some("twolen err Content-Length 99 does not match the body (5 octets); the client sets it from the body"),
        "{out}"
    );
    let zero = out.lines().nth(7).unwrap();
    assert!(zero.starts_with("zero ok 200 POST /x HTTP/1.1|Host: 127.0.0.1:"), "{out}");
    assert!(zero.ends_with("|Connection: close|Accept-Encoding: gzip|Content-Length: 0"), "{out}");
}

const JSON: &str = r##"import std/json;

fn e(s: string) -> string {
    match (json.parse_doc(s)) {
        Result.Ok(_) => "ok",
        Result.Err(m) => m,
    }
}
fn e2(s: string) -> string {
    match (json.parse(s)) {
        Result.Ok(_) => "ok",
        Result.Err(m) => m,
    }
}

fn main() {
    print(e("{\"a\" 1}"));
    print(e("[1,\n 2,]"));
    print(e("tru"));
    print(e2("tru"));
    print(e("\u{FEFF}{\"a\":1}"));
    print(e2("\u{FEFF}{\"a\" 1}"));
    print(e("{} x"));
    print(e("{\"k\": \"é\", \"j\": }"));
    print(e2("[1,\n 2,]"));
}
"##;

#[test]
fn json_errors_say_line_and_column_and_the_bom_is_skipped() {
    let dir = std::env::temp_dir().join(format!("ray_rayreq82_json_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("prog.ray"), JSON).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&dir).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "JSON, line 1 column 6: expected ':'\n\
         JSON, line 2 column 4: unexpected ']'\n\
         JSON, line 1 column 1: expected 'true', 'false' or 'null'\n\
         JSON, line 1 column 1: expected 'true', 'false' or 'null'\n\
         ok\n\
         JSON, line 1 column 6: expected ':'\n\
         JSON, line 1 column 4: trailing text after the JSON\n\
         JSON, line 1 column 17: unexpected '}'\n\
         JSON, line 2 column 4: unexpected ']'\n"
    );
}
