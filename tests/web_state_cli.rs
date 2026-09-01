//! M154 — `web.state`: el estado DE APLICACIÓN compartido entre handlers (IDEAS §81.5,
//! raydesk ROADMAP #8). Dos propiedades: (1) la ATOMICIDAD del contador — N requests
//! concurrentes a `/incr` suman EXACTO N (el read-modify-write corre entero en la fibra
//! dueña del actor kv; un get+put por separado perdería actualizaciones); (2) el interruptor
//! de persistencia de dev (RAY_DEV_RELOAD): el estado sobrevive a un restart del servidor.
//! Patrón de `tests/session_cli.rs`: proyecto consumidor real en un temporal.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const MAIN: &str = r#"from web/framework import new_app, GET, listen, text, App, Ctx, Res, state, state_get, state_incr, AppState;

fn main() -> int {
    let st = match (state("dev-state.rkv")) {
        Result.Ok(s) => s,
        Result.Err(e) => {
            print("state: " + e);
            return 1;
        },
    };
    let r = listen(fn() -> App {
        var app = new_app();
        app.GET("/incr", fn(c: Ctx, r: Res) {
            match (state_incr(st, "hits", 1)) {
                Result.Ok(n) => r.text(to_string(n)),
                Result.Err(e) => r.text("err: " + e),
            }
        });
        app.GET("/hits", fn(c: Ctx, r: Res) {
            r.text(state_get(st, "hits"));
        });
        app
    }, "127.0.0.1", 0);
    match (r) {
        Result.Ok(n) => 0,
        Result.Err(e) => {
            print("listen: " + e);
            1
        },
    }
}
"#;

fn launch(dir: &std::path::Path, envs: &[(&str, &str)]) -> (Child, u16) {
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::create_dir_all(dir).expect("creates the dir");
    std::fs::write(dir.join("main.ray"), MAIN).expect("writes main");
    std::fs::write(
        dir.join("ray.toml"),
        format!(
            "[package]\nname = \"state-test\"\nversion = \"0.1.0\"\nentry = \"main.ray\"\n\n\
             [dependencies]\nweb = \"path:{root}/packages/web\"\nnet = \"path:{root}/packages/net\"\n"
        ),
    )
    .expect("writes ray.toml");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.args(["run", "main.ray"]).current_dir(dir).stdout(Stdio::piped()).stderr(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("launches the server");
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).expect("reads the port line");
    let port: u16 = line
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("could not read the port from: {line:?}"));
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = reader.read_to_end(&mut sink);
    });
    (child, port)
}

fn ask(port: u16, req: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connects");
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(req.as_bytes()).expect("sends");
    let mut bytes = Vec::new();
    let _ = s.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn body_of(resp: &str) -> String {
    resp.split("\r\n\r\n").nth(1).unwrap_or("").trim().to_string()
}

fn test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_web_state_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// La propiedad estrella: 24 requests CONCURRENTES a /incr — el total final es EXACTO
/// (cada respuesta es un valor 1..=24 distinto y /hits termina en 24). Con un get+put por
/// separado esto perdería actualizaciones.
#[test]
fn concurrent_increments_are_exact() {
    let dir = test_dir("concurrent");
    let (mut child, port) = launch(&dir, &[]);

    const N: usize = 24;
    let mut handles = Vec::new();
    for _ in 0..N {
        handles.push(std::thread::spawn(move || {
            let resp = ask(port, "GET /incr HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
            body_of(&resp).parse::<i64>().unwrap_or(-1)
        }));
    }
    let mut seen: Vec<i64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    seen.sort_unstable();
    let want: Vec<i64> = (1..=N as i64).collect();
    assert_eq!(seen, want, "each increment lands exactly once");

    let final_resp = ask(port, "GET /hits HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!(body_of(&final_resp), N.to_string(), "the stored total is exact");

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// El interruptor de dev: con RAY_DEV_RELOAD el estado persiste (RKV1) y sobrevive a un
/// restart del servidor; sin la variable, arranca vacío y jamás toca disco.
#[test]
fn dev_mode_state_survives_a_restart() {
    let dir = test_dir("dev");
    let (mut child, port) = launch(&dir, &[("RAY_DEV_RELOAD", "1")]);
    let first = ask(port, "GET /incr HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!(body_of(&first), "1");
    let _ = child.kill();
    let _ = child.wait();

    // Relanzado en dev: el contador continúa donde iba.
    let (mut child2, port2) = launch(&dir, &[("RAY_DEV_RELOAD", "1")]);
    let second = ask(port2, "GET /incr HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!(body_of(&second), "2", "the state survived the restart");
    let _ = child2.kill();
    let _ = child2.wait();

    // Producción (sin la env): arranca vacío aunque el archivo exista.
    let (mut child3, port3) = launch(&dir, &[]);
    let third = ask(port3, "GET /incr HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!(body_of(&third), "1", "production starts empty");
    let _ = child3.kill();
    let _ = child3.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
