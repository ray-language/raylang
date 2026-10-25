//! Hallazgos de rayreq con 1.27.79 en los paquetes: `http.request_bytes("HEAD", …)` no espera el
//! cuerpo que anuncia `Content-Length` (#246), `http.request_pairs` manda cabeceras repetidas en
//! orden y `Stream.raw_headers` conserva los `Set-Cookie` repetidos (#250), y `sqlite.exec` corre
//! todas las sentencias sin parámetros y rechaza varias con parámetros (#247). VM; los servidores
//! escuchan en el puerto 0.

use std::path::{Path, PathBuf};
use std::process::Command;

fn project(tag: &str, dep: &str, main: &str) -> PathBuf {
    let pkg = Path::new(env!("CARGO_MANIFEST_DIR")).join("packages").join(dep);
    let app = std::env::temp_dir().join(format!("ray_rayreq79_{tag}_{}", std::process::id()));
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
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.contains("listening on port")).map(|l| format!("{l}\n")).collect()
}

const NET: &str = r##"import std/net;
import net/webserver;
import net/http;
from net/webserver import Request, Response;

fn handle(req: Request) -> Response {
    var r = webserver.text(200, "hi " + req.method);
    r.set_cookie.push("a=1");
    r.set_cookie.push("b=2");
    r
}

// Devuelve la cabecera de la petición recibida como cuerpo.
fn echo_server() -> int {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    let _ = spawn(fn() -> int {
        let c = net.tcp_accept(l).unwrap();
        let req = from_utf8(net.socket_read_bytes(c).unwrap()).unwrap_or("?");
        let head = req.split("\r\n\r\n")[0].replace("\r\n", "|");
        let _ = net.socket_write(c, "HTTP/1.1 200 OK\r\nContent-Length: " + to_string(head.to_bytes().len()) + "\r\nConnection: close\r\n\r\n" + head);
        close(c);
        0
    });
    port
}

fn main() {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    let _ = spawn(fn() -> int { let _ = webserver.serve_on(l, handle); 0 });
    let base = "http://127.0.0.1:" + to_string(port);
    match (http.request_bytes("HEAD", base + "/x", b"", Map.new(), 5000)) {
        Result.Ok(r) => print("head ${r.status} body=${r.body.len()} cl=" + http.header(r, "content-length").unwrap_or("-")),
        Result.Err(e) => print("head err " + e),
    }
    let ep = echo_server();
    match (http.request_pairs("GET", "http://127.0.0.1:${ep}/y", b"", [("X-A", "1"), ("X-A", "2"), ("X-B", "3")], 5000)) {
        Result.Ok(r) => print("pairs " + http.body_text(r).unwrap_or("?")),
        Result.Err(e) => print("pairs err " + e),
    }
    match (http.stream_with("GET", base + "/z", b"", Map.new(), 5000)) {
        Result.Ok(s) => {
            var cookies: [string] = [];
            for kv in s.raw_headers { if (kv.0 == "set-cookie") { cookies.push(kv.1); } }
            print("stream cookies " + cookies.join(";"));
            http.stream_close(s);
        },
        Result.Err(e) => print("stream err " + e),
    }
}
"##;

#[test]
fn head_has_no_body_and_repeated_headers_survive_both_ways() {
    let out = run(&project("net", "net", NET));
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("head 200 body=0 cl=7"), "{out}");
    let pairs = lines.next().unwrap_or("");
    assert!(pairs.starts_with("pairs GET /y HTTP/1.1|Host: 127.0.0.1:"), "{out}");
    assert!(pairs.ends_with("|Connection: close|Accept-Encoding: gzip|X-A: 1|X-A: 2|X-B: 3"), "{out}");
    assert_eq!(lines.next(), Some("stream cookies a=1;b=2"), "{out}");
}

const SQL: &str = r##"import db/sqlite;
fn show(r: Result<int, string>) -> string {
    match (r) { Result.Ok(n) => "ok " + to_string(n), Result.Err(e) => "err " + e }
}
fn rows(rs: [[string]]) -> string {
    var out: [string] = [];
    for r in rs { out.push(r.join(",")); }
    out.join(";")
}
fn main() {
    let c = sqlite.connect(":memory:").unwrap();
    print("migrate " + show(sqlite.exec(c, "CREATE TABLE a (x INTEGER); CREATE TABLE b (y INTEGER);\n-- fin\n", [])));
    print(rows(sqlite.query(c, "SELECT name FROM sqlite_master ORDER BY name", []).unwrap()));
    print("pragma " + show(sqlite.exec(c, "PRAGMA busy_timeout = 5000", [])));
    print("insert " + show(sqlite.exec(c, "INSERT INTO a VALUES (?1)", ["7"])));
    print("batch " + show(sqlite.exec(c, "INSERT INTO a VALUES (1); INSERT INTO a VALUES (2); UPDATE a SET x = x + 1", [])));
    print("params-multi " + show(sqlite.exec(c, "INSERT INTO b VALUES (?1); INSERT INTO b VALUES (?1)", ["5"])));
    print("b rows " + rows(sqlite.query(c, "SELECT count(*) FROM b", []).unwrap()));
    print("syntax " + show(sqlite.exec(c, "CREATE TABLE c (z); NOT SQL", [])));
    print("c exists " + rows(sqlite.query(c, "SELECT count(*) FROM sqlite_master WHERE name = 'c'", []).unwrap()));
    print("empty " + show(sqlite.exec(c, "", [])));
}
"##;

#[test]
fn sqlite_exec_runs_every_statement_or_refuses_parameters_with_more_than_one() {
    let out = run(&project("sql", "db", SQL));
    assert_eq!(out, r##"migrate ok 0
a;b
pragma ok 0
insert ok 1
batch ok 5
params-multi err sqlite: exec with parameters runs one statement, and the SQL has more; run the others in their own exec (without parameters they all run)
b rows 0
syntax err near "NOT": syntax error in  NOT SQL at offset 1
c exists 1
empty ok 0
"##);
}
