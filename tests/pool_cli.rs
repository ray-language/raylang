//! M318 (findings #68/#69): el pool de conexiones genérico `net/pool` y sus envoltorios —
//! `http.pool` (keep-alive por host) y `redis.pool` — sobre los paquetes reales. La semántica del
//! pool (Ready-first, aparcar al agotarse, descartar y reintentar una vez tras un fallo de cable)
//! se verifica con "conexiones" enteras en VM y nativo; el keep-alive contra un `net/webserver`
//! real (mismo puerto remoto = misma conexión) y el reintento contra un servidor RESP de prueba
//! que cierra la conexión tras la primera respuesta.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn project(name: &str, main: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_pool_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::write(dir.join("src/main.ray"), main).unwrap();
    std::fs::write(
        dir.join("ray.toml"),
        format!("[package]\nname = \"poolt\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{root}/packages/net\"\n"),
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

/// El pool genérico con "conexiones" enteras: Ready-first (una devuelta se prefiere a un hueco
/// vacío), un fallo de cable sobre una conexión REUTILIZADA descarta y reintenta una vez, un
/// error del servidor conserva la conexión, un pool agotado aparca hasta que vuelva un hueco.
#[test]
fn generic_pool_semantics_run_on_vm_and_native() {
    let d = project(
        "generic",
        r#"import net/pool;
import std/time;

// Las fibras usan funciones de nivel superior (un closure capturado no cruza a `spawn` en nativo).
fn dial_fixed() -> Result<int, string> { Result.Ok(30) }
fn drop_print(c: int) { print("drop " + to_string(c)); }

fn main() -> int {
    let p: pool.Pool<int> = pool.new(2);
    var dialed = 0;
    let dial = fn() -> Result<int, string> { dialed = dialed + 1; Result.Ok(dialed * 10) };
    let drop = fn(c: int) { print("drop " + to_string(c)); };
    // Primera operación: marca (10). Segunda: reutiliza 10 aunque quede un hueco vacío.
    let a = pool.run(p, dial, drop, fn(c: int) -> Result<int, string> { Result.Ok(c) }, true);
    let b = pool.run(p, dial, drop, fn(c: int) -> Result<int, string> { Result.Ok(c + 1) }, true);
    print(to_string(a.unwrap_or(-1)) + " " + to_string(b.unwrap_or(-1)));
    // Fallo de cable sobre la reutilizada: se descarta (drop 10), se marca otra (20) y se repite.
    var calls = 0;
    let flaky = fn(c: int) -> Result<int, string> {
        calls = calls + 1;
        if (calls == 1) { Result.Err("connection closed while reading") } else { Result.Ok(c) }
    };
    print(pool.run(p, dial, drop, flaky, true).unwrap_or(-1));
    // Error del servidor: la conexión (20) se conserva y no hay reintento.
    let sql = pool.run(p, dial, drop, fn(c: int) -> Result<int, string> { Result.Err("ERROR: relation x does not exist") }, true);
    match (sql) { Result.Ok(_) => print("?"), Result.Err(e) => print(e) }
    print(pool.run(p, dial, drop, fn(c: int) -> Result<int, string> { Result.Ok(c) }, true).unwrap_or(-1));
    // Sin reintento: el fallo de cable descarta y devuelve el error.
    match (pool.run(p, dial, drop, fn(c: int) -> Result<int, string> { Result.Err("read timeout") }, true)) {
        Result.Ok(_) => print("?"),
        Result.Err(e) => print("no retry: " + e),
    }
    // Agotado: dos fibras retienen los dos huecos; la tercera aparca hasta que una devuelve.
    let started: Channel<int> = Channel.bounded(4);
    let go: Channel<int> = Channel.bounded(4);
    let t1 = spawn(fn() -> int {
        pool.run(p, dial_fixed, drop_print, fn(c: int) -> Result<int, string> { send(started, c); let _ = recv(go); Result.Ok(c) }, true).unwrap_or(-1)
    });
    let t2 = spawn(fn() -> int {
        pool.run(p, dial_fixed, drop_print, fn(c: int) -> Result<int, string> { send(started, c); let _ = recv(go); Result.Ok(c) }, true).unwrap_or(-1)
    });
    let c1 = recv(started).unwrap_or(-1);
    let c2 = recv(started).unwrap_or(-1);
    let t3 = spawn(fn() -> int { pool.run(p, dial_fixed, drop_print, fn(c: int) -> Result<int, string> { Result.Ok(c) }, true).unwrap_or(-1) });
    time.sleep(100);
    send(go, 0);
    send(go, 0);
    let held = join(t1) + join(t2);
    let third = join(t3);
    print(to_string(c1 + c2 == held) + " " + to_string(third == c1 || third == c2));
    pool.shutdown(p, drop);
    match (pool.acquire(p)) { Result.Ok(_) => print("?"), Result.Err(e) => print(e) }
    0
}
"#,
    );
    let want = "10 11\ndrop 10\n20\nERROR: relation x does not exist\n20\ndrop 20\nno retry: read timeout\ntrue true\ndrop 30\ndrop 30\nthe pool is closed\n";
    let (out, err, code) = ray(&d, &["run", "src/main.ray"]);
    assert_eq!(code, 0, "vm: {err}");
    assert_eq!(out, want, "vm");
    if has_rustc() {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "src/main.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        let out = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo");
    }
}

/// `http.pool` contra un `net/webserver` real (keep-alive): peticiones secuenciales reutilizan
/// UNA conexión (mismo puerto remoto visto por el servidor); peticiones concurrentes no pasan
/// del tamaño del pool.
#[test]
fn http_pool_reuses_keep_alive_connections() {
    let d = project(
        "http",
        r#"import net/webserver;
import net/http;
import std/net;

fn handler(req: webserver.Request) -> webserver.Response {
    webserver.ok(req.remote)
}

fn main() -> int {
    let srv = match (net.tcp_listen("127.0.0.1", 0)) { Result.Ok(s) => s, Result.Err(e) => { print(e); return 1; } };
    let port = net.local_port(srv);
    let base = "http://127.0.0.1:" + to_string(port);
    let server = spawn(fn() -> int {
        match (webserver.serve_on(srv, handler)) { Result.Ok(_) => 0, Result.Err(_) => 1 }
    });
    let p = http.pool(3);
    var remotes: [string] = [];
    for _ in 0..8 {
        match (http.pool_fetch(p, base + "/seq")) {
            Result.Ok(r) => remotes.push(http.body_text(r).unwrap_or("?")),
            Result.Err(e) => remotes.push("ERR " + e),
        }
    }
    var same = true;
    for r in remotes { if (r != remotes[0]) { same = false; } }
    print("sequential same connection: " + to_string(same) + " " + to_string(remotes.len()));
    // Concurrentes: como mucho 3 conexiones distintas.
    let results: Channel<string> = Channel.bounded(16);
    for i in 0..9 {
        spawn(fn() {
            match (http.pool_fetch(p, base + "/c" + to_string(i))) {
                Result.Ok(r) => send(results, http.body_text(r).unwrap_or("?")),
                Result.Err(e) => send(results, "ERR " + e),
            }
        });
    }
    var seen: [string] = [];
    for _ in 0..9 {
        let r = recv(results).unwrap_or("none");
        if (r.starts_with("ERR")) { print(r); }
        if (!seen.contains(r)) { seen.push(r); }
    }
    print("concurrent distinct <= 3: " + to_string(seen.len() <= 3));
    http.pool_close(p);
    0
}
"#,
    );
    let (out, err, code) = ray(&d, &["run", "src/main.ray"]);
    assert_eq!(code, 0, "vm: {err}");
    assert!(out.ends_with("sequential same connection: true 8\nconcurrent distinct <= 3: true\n"), "{out}\n{err}");
}

/// Un servidor RESP mínimo: `PING` → `+PONG`; cuenta conexiones; en modo `drop_after_first`
/// cierra cada conexión tras su primera respuesta (un servidor "reiniciado" desde el punto de
/// vista del cliente que la reutiliza).
fn resp_server(drop_after_first: bool) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let conns = Arc::new(AtomicUsize::new(0));
    let counter = conns.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            counter.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || serve_resp(stream, drop_after_first));
        }
    });
    (port, conns)
}

fn serve_resp(mut stream: TcpStream, drop_after_first: bool) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    loop {
        // "*N\r\n" seguido de N "$len\r\n<arg>\r\n".
        let mut head = String::new();
        if reader.read_line(&mut head).unwrap_or(0) == 0 || !head.starts_with('*') {
            return;
        }
        let n: usize = head[1..].trim().parse().unwrap_or(0);
        let mut args = Vec::new();
        for _ in 0..n {
            let mut len = String::new();
            reader.read_line(&mut len).unwrap();
            let l: usize = len[1..].trim().parse().unwrap();
            let mut buf = vec![0u8; l + 2];
            reader.read_exact(&mut buf).unwrap();
            args.push(String::from_utf8_lossy(&buf[..l]).into_owned());
        }
        let reply = match args.first().map(|s| s.as_str()) {
            Some("PING") => "+PONG\r\n".to_string(),
            Some(other) => format!("+{other}\r\n"),
            None => "-ERR\r\n".to_string(),
        };
        stream.write_all(reply.as_bytes()).unwrap();
        if drop_after_first {
            return; // cierra el socket
        }
    }
}

/// `redis.pool`: comandos secuenciales sobre UNA conexión; con el servidor cerrando tras cada
/// respuesta, la conexión reutilizada falla por el cable y el comando se repite una vez sobre
/// una fresca (transparente para el usuario).
#[test]
fn redis_pool_reuses_and_retries_once_after_the_server_drops() {
    let main = r#"import net/redis;

fn main() -> int {
    let port = parse_int(args()[0]).unwrap_or(0);
    let p = redis.pool("127.0.0.1", port, 2);
    var ok = 0;
    for _ in 0..5 {
        match (redis.pool_command(p, ["PING"])) {
            Result.Ok(r) => { if (redis.reply_str(r) == "PONG") { ok = ok + 1; } },
            Result.Err(e) => print("ERR " + e),
        }
    }
    print(ok);
    redis.pool_close(p);
    0
}
"#;
    let d = project("redis", main);
    let (port, conns) = resp_server(false);
    let (out, err, code) = ray(&d, &["run", "src/main.ray", &port.to_string()]);
    assert_eq!(code, 0, "vm: {err}");
    assert_eq!(out, "5\n");
    assert_eq!(conns.load(Ordering::SeqCst), 1, "una sola conexión para 5 comandos secuenciales");

    let (port, conns) = resp_server(true);
    let (out, err, code) = ray(&d, &["run", "src/main.ray", &port.to_string()]);
    assert_eq!(code, 0, "vm: {err}");
    assert_eq!(out, "5\n", "cada comando sana con un reintento: {err}");
    assert_eq!(conns.load(Ordering::SeqCst), 5, "una conexión nueva por comando (el servidor cierra cada una)");
    let _ = Stdio::null();
}
