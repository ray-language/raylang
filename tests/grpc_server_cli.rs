//! M321 (findings #63): el servidor gRPC unario `net/grpc_server` (h2c) y el cliente de conexión
//! persistente `net/grpc_conn`, sobre el paquete `net` real. Un solo programa levanta el servidor
//! en una fibra (puerto 0) y lo llama desde otra: eco con metadata, un estado de error con
//! `grpc-message` percent-encoded (ida y vuelta intactos), UNIMPLEMENTED para una ruta sin
//! handler, RESOURCE_EXHAUSTED para un mensaje mayor que el tope, DEADLINE_EXCEEDED cuando el
//! handler tarda más que `grpc-timeout`, y varias llamadas por la misma conexión (streams
//! impares crecientes). En VM y, si hay rustc, en el binario nativo.

use std::path::PathBuf;
use std::process::Command;

fn project(name: &str, main: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_grpcsrv_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::write(dir.join("src/main.ray"), main).unwrap();
    std::fs::write(
        dir.join("ray.toml"),
        format!("[package]\nname = \"grpct\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{root}/packages/net\"\n"),
    )
    .unwrap();
    dir
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang")).args(args).current_dir(dir).output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

const MAIN: &str = r#"import net/grpc_server;
import net/grpc_conn;
import net/grpc_status;
from net/grpc_status import Status;
import std/time;

// /echo.Echo/Say: devuelve el mensaje con la metadata `x-tag` delante.
fn say(c: grpc_server.Call) -> Result<bytes, Status> {
    let tag = c.metadata.get_or("x-tag", "-");
    let body = from_utf8(c.message).unwrap_or("?");
    Result.Ok(("[" + tag + "] " + body).to_bytes())
}

// /echo.Echo/Fail: un estado con mensaje que necesita percent-encoding.
fn fail(c: grpc_server.Call) -> Result<bytes, Status> {
    grpc_status.fail(grpc_status.NOT_FOUND, "no está: 100% ñ")
}

// /echo.Echo/Slow: tarda más que el plazo del cliente.
fn slow(c: grpc_server.Call) -> Result<bytes, Status> {
    time.sleep(300);
    Result.Ok(b"late")
}

fn build() -> grpc_server.Router {
    grpc_server.router()
        .route("/echo.Echo/Say", say)
        .route("/echo.Echo/Fail", fail)
        .route("/echo.Echo/Slow", slow)
}

fn main() -> int {
    let l = match (grpc_server.bind("127.0.0.1", 0)) {
        Result.Ok(x) => x,
        Result.Err(e) => { print("bind: " + e); return 1; },
    };
    let port = l.port;
    let stop: Channel<int> = Channel.bounded(1);
    let opts = grpc_server.Options { max_message: 64 };
    let server = spawn(fn() -> int {
        match (grpc_server.serve_router_until(l, stop, opts, build)) {
            Result.Ok(_) => 0,
            Result.Err(e) => { print("serve: " + e); 1 },
        }
    });
    let c = match (grpc_conn.connect("127.0.0.1", port)) {
        Result.Ok(x) => x,
        Result.Err(e) => { print("connect: " + e); return 1; },
    };
    let md: Map<string, string> = Map.new();
    md.insert("X-Tag", "t1");
    // 1. Eco con metadata.
    match (grpc_conn.call(c, "/echo.Echo/Say", "hola".to_bytes(), md, 2000)) {
        Result.Ok(r) => print("say " + to_string(r.status) + " " + from_utf8(r.message).unwrap_or("?")),
        Result.Err(e) => print("say ERR " + e),
    }
    // 2. Estado con grpc-message percent-encoded (ida y vuelta).
    let none: Map<string, string> = Map.new();
    match (grpc_conn.call(c, "/echo.Echo/Fail", b"x", none, 2000)) {
        Result.Ok(r) => print("fail " + grpc_status.code_name(r.status) + " " + r.status_message),
        Result.Err(e) => print("fail ERR " + e),
    }
    // 3. Ruta sin handler.
    match (grpc_conn.call(c, "/echo.Echo/Nope", b"x", none, 2000)) {
        Result.Ok(r) => print("nope " + grpc_status.code_name(r.status)),
        Result.Err(e) => print("nope ERR " + e),
    }
    // 4. Mensaje mayor que el tope del servidor (64 octetos).
    var big = "";
    for _ in 0..100 { big = big + "y"; }
    match (grpc_conn.call(c, "/echo.Echo/Say", big.to_bytes(), none, 2000)) {
        Result.Ok(r) => print("big " + grpc_status.code_name(r.status)),
        Result.Err(e) => print("big ERR " + e),
    }
    // 5. Plazo vencido en el cliente (el handler tarda 300 ms, el plazo es 50).
    match (grpc_conn.call(c, "/echo.Echo/Slow", b"x", none, 50)) {
        Result.Ok(r) => print("slow " + grpc_status.code_name(r.status)),
        Result.Err(e) => print("slow ERR " + e),
    }
    // 6. La conexión sigue usable: otra llamada más.
    match (grpc_conn.call(c, "/echo.Echo/Say", "otra".to_bytes(), md, 2000)) {
        Result.Ok(r) => print("again " + from_utf8(r.message).unwrap_or("?") + " usable=" + to_string(grpc_conn.usable(c))),
        Result.Err(e) => print("again ERR " + e),
    }
    grpc_conn.disconnect(c);
    // 7. call_once + into_result.
    match (grpc_conn.call_once("127.0.0.1", port, "/echo.Echo/Say", "uno".to_bytes(), none, 2000)) {
        Result.Ok(r) => match (grpc_conn.into_result(r)) {
            Result.Ok(m) => print("once " + from_utf8(m).unwrap_or("?")),
            Result.Err(st) => print("once ERR " + to_string(st.code) + " " + st.message),
        },
        Result.Err(e) => print("once ERR " + e),
    }
    send(stop, 1);
    let code = join(server);
    print("server " + to_string(code));
    code
}
"#;

const WANT: &str = "say 0 [t1] hola\nfail NOT_FOUND no está: 100% ñ\nnope UNIMPLEMENTED\nbig RESOURCE_EXHAUSTED\nslow DEADLINE_EXCEEDED\nagain [t1] otra usable=true\nonce [-] uno\nserver 0\n";

#[test]
fn unary_server_and_persistent_client_round_trip_on_the_vm() {
    let d = project("vm", MAIN);
    let (out, err, code) = ray(&d, &["run", "src/main.ray"]);
    assert_eq!(code, 0, "vm: {err}");
    assert_eq!(out, WANT, "{err}");
}

#[test]
fn unary_server_and_persistent_client_round_trip_natively() {
    if !has_rustc() {
        return;
    }
    let d = project("native", MAIN);
    let bin = d.join("grpc_bin");
    let (_o, err, code) = ray(&d, &["build", "src/main.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native: {err}");
    let out = Command::new(&bin).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo: {}", String::from_utf8_lossy(&out.stderr));
}
