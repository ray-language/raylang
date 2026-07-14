//! M88.4 — RPC raylang↔raylang (`packages/rpc`): framing con prefijo de longitud sobre TCP,
//! sobre JSON, request/response con id + deadline. DOS procesos (servidor y cliente `ray run`
//! separados, mismo proyecto temporal con `ray.toml` → paquete rpc): así el test valida el
//! WIRE de verdad, no un atajo en memoria. El servidor imprime su puerto efímero (molde de
//! `webserver_shutdown_cli`); el apagado ordenado se dispara por RPC (método "apagar").

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_ray");

const SERVER: &str = r#"import rpc/rpc;
from std/json import Json;
import std/time;

fn main() -> int {
    let stop: Channel<int> = Channel.new();
    let r = rpc.serve_shutdown("127.0.0.1", 0, stop, 2000, fn(req: rpc.Req) -> Result<Json, string> {
        if (req.method == "sum") {
            match (req.params) {
                Json.JArray(xs) => {
                    var t = 0.0;
                    for x in xs {
                        match (x) {
                            Json.JNum(f) => { t = t + f; },
                            _ => { },
                        }
                    }
                    Result.Ok(Json.JNum(t))
                },
                _ => Result.Err("sum espera un array"),
            }
        } else if (req.method == "echo") {
            Result.Ok(req.params)
        } else if (req.method == "trace") {
            Result.Ok(Json.JStr(req.traceparent))
        } else if (req.method == "deadline") {
            Result.Ok(Json.JNum(req.deadline_ms as float))
        } else if (req.method == "boom") {
            panic("bum")
        } else if (req.method == "lento") {
            time.sleep(1000);
            Result.Ok(Json.JStr("tarde"))
        } else if (req.method == "apagar") {
            send(stop, 1);
            Result.Ok(Json.JStr("bye"))
        } else {
            Result.Err("método unknown: " + req.method)
        }
    });
    print("rpc servidor off");
    0
}
"#;

const CLIENT: &str = r#"import rpc/rpc;
from std/json import Json, stringify;

fn conectar(port: int) -> rpc.Client {
    match (rpc.connect("127.0.0.1", port)) {
        Result.Ok(c) => c,
        Result.Err(e) => panic("connection: " + e),
    }
}

fn shows(name: string, r: Result<Json, string>) {
    match (r) {
        Result.Ok(j) => print(name + "=" + stringify(j)),
        Result.Err(e) => print(name + " err=" + e),
    }
}

fn main() -> int {
    let port = match (parse_int(args()[0])) {
        Option.Some(p) => p,
        Option.None => panic("port inválido"),
    };
    let c = conectar(port);
    // Lo básico: varias peticiones secuenciales por la MISMA conexión, con id correlado.
    shows("sum", rpc.call(c, "sum", Json.JArray([Json.JNum(1.0), Json.JNum(2.0), Json.JNum(3.0)])));
    var o: Map<string, Json> = Map.new();
    o.insert("k", Json.JStr("v"));
    shows("echo", rpc.call(c, "echo", Json.JObject(o)));
    // El traceparent y el deadline viajan en el sobre (el handler los ve en Req).
    shows("trace", rpc.call_full(c, "trace", Json.JNull, 0, "00-abc-def-01"));
    shows("deadline", rpc.call_full(c, "deadline", Json.JNull, 500, ""));
    // El Err del handler llega como Err del call.
    shows("err", rpc.call(c, "nada", Json.JNull));
    // Un handler que PANICA responde err y la conexión sigue viva.
    match (rpc.call(c, "boom", Json.JNull)) {
        Result.Ok(j) => print("boom=?"),
        Result.Err(e) => print("boom handler_err=" + to_string(e.starts_with("handler:"))),
    }
    shows("sum2", rpc.call(c, "sum", Json.JArray([Json.JNum(2.0), Json.JNum(2.0)])));
    // Deadline vencido: el call falla y la conexión queda desincronizada → reconectar.
    match (rpc.call_deadline(c, "lento", Json.JNull, 150)) {
        Result.Ok(j) => print("timeout=?"),
        Result.Err(e) => print("timeout=true"),
    }
    rpc.disconnect(c);
    let c2 = conectar(port);
    shows("sum3", rpc.call(c2, "sum", Json.JArray([Json.JNum(5.0)])));
    // El apagado ordenado, POR RPC: el handler manda al canal stop; la respuesta llega igual.
    shows("apagar", rpc.call(c2, "apagar", Json.JNull));
    rpc.disconnect(c2);
    0
}
"#;

/// Crea el proyecto temporal (ray.toml → packages/rpc) con server.ray y client.ray.
fn project(name: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("ray_rpc_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).expect("crea el dir temporal");
    let repo = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"rpcapp\"\nversion = \"0.1.0\"\n\n[dependencies]\nrpc = \"path:{repo}/packages/rpc\"\n"),
    )
    .unwrap();
    std::fs::write(base.join("src/server.ray"), SERVER).unwrap();
    std::fs::write(base.join("src/client.ray"), CLIENT).unwrap();
    base
}

/// Lanza el servidor y devuelve (hijo, puerto anunciado).
fn launch_servidor(base: &std::path::Path) -> (Child, u16) {
    let mut child = Command::new(BIN)
        .args(["run", "src/server.ray"])
        .current_dir(base)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza el servidor");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee el port");
    let port: u16 = linea.trim().rsplit(' ').next().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("invalid port: {linea:?}"));
    child.stdout = Some(reader.into_inner());
    (child, port)
}

fn run_client(base: &std::path::Path, port: u16) -> (String, i32) {
    let out = Command::new(BIN)
        .args(["run", "src/client.ray", &port.to_string()])
        .current_dir(base)
        .output()
        .expect("lanza el client");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

/// La batería completa por el wire: peticiones secuenciales con id, params JSON de ida y
/// vuelta, traceparent/deadline en el sobre, Err del handler, panic del handler que NO mata la
/// conexión, deadline vencido + reconexión, y apagado ordenado por RPC (el servidor drena la
/// respuesta "bye", devuelve de serve_shutdown y sale 0).
#[test]
fn rpc_de_punta_a_punta() {
    let base = project("e2e");
    let (server, port) = launch_servidor(&base);
    let (out, code) = run_client(&base, port);
    assert_eq!(code, 0, "client sale 0\n{out}");
    let expected = "sum=6\n\
echo={\"k\":\"v\"}\n\
trace=\"00-abc-def-01\"\n\
deadline=500\n\
err err=método unknown: nada\n\
boom handler_err=true\n\
sum2=4\n\
timeout=true\n\
sum3=5\n\
apagar=\"bye\"\n";
    assert_eq!(out, expected, "la batería del client, byte a byte");
    let sout = server.wait_with_output().expect("espera al servidor");
    let s_stdout = String::from_utf8_lossy(&sout.stdout);
    assert_eq!(sout.status.code(), Some(0), "el servidor sale 0 after el off\n{s_stdout}");
    assert!(s_stdout.contains("rpc apagando"), "anuncia el off: {s_stdout}");
    assert!(s_stdout.contains("rpc servidor off"), "serve_shutdown devolvió a main: {s_stdout}");
}

/// Dos clientes CONCURRENTES contra el mismo servidor: cada conexión tiene su fibra y sus ids
/// propios; ambos obtienen sus resultados correctos.
#[test]
fn dos_clientes_concurrentes() {
    let base = project("dos");
    // Un cliente mínimo parametrizado por sus sumandos (argv[1], argv[2]).
    std::fs::write(
        base.join("src/mini.ray"),
        r#"import rpc/rpc;
from std/json import Json, stringify;

fn main() -> int {
    let port = match (parse_int(args()[0])) { Option.Some(p) => p, Option.None => panic("port"), };
    let a = match (parse_int(args()[1])) { Option.Some(x) => x, Option.None => panic("a"), };
    let b = match (parse_int(args()[2])) { Option.Some(x) => x, Option.None => panic("b"), };
    let c = match (rpc.connect("127.0.0.1", port)) { Result.Ok(x) => x, Result.Err(e) => panic(e), };
    var i = 0;
    while (i < 20) {
        match (rpc.call(c, "sum", Json.JArray([Json.JNum(a as float), Json.JNum(b as float)]))) {
            Result.Ok(j) => {
                if (stringify(j) != to_string(a + b)) { print("mal: " + stringify(j)); return 1; }
            },
            Result.Err(e) => { print("err: " + e); return 1; },
        }
        i = i + 1;
    }
    print("ok " + to_string(a + b));
    rpc.disconnect(c);
    0
}
"#,
    )
    .unwrap();
    let (server, port) = launch_servidor(&base);
    let base2 = base.clone();
    let h1 = std::thread::spawn(move || {
        Command::new(BIN).args(["run", "src/mini.ray", &port.to_string(), "1", "2"])
            .current_dir(&base2).output().expect("client 1")
    });
    let base3 = base.clone();
    let h2 = std::thread::spawn(move || {
        Command::new(BIN).args(["run", "src/mini.ray", &port.to_string(), "10", "20"])
            .current_dir(&base3).output().expect("client 2")
    });
    let o1 = h1.join().unwrap();
    let o2 = h2.join().unwrap();
    assert_eq!(String::from_utf8_lossy(&o1.stdout), "ok 3\n", "client 1");
    assert_eq!(String::from_utf8_lossy(&o2.stdout), "ok 30\n", "client 2");
    assert_eq!(o1.status.code(), Some(0));
    assert_eq!(o2.status.code(), Some(0));
    // El servidor sigue vivo (no lo apagó nadie): lo matamos para no fugarlo.
    let mut server = server;
    server.kill().ok();
    server.wait().ok();
}
