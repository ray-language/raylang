//! M170 — la red SIN poller (Windows: `raw_fd` es `None`): las fibras aparcadas por `WouldBlock`
//! en accept/read/write no tienen fd y el scheduler debe reintentarlas por busy-poll, no tratarlas
//! como durmientes. Los dos programas son las sondas P2/P3 del censo de Windows, que colgaban tras
//! imprimir el puerto; corren en las tres plataformas (en unix el poller sigue mandando).

use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_ray");

fn tmp(name: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("ray_net_no_poller_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// Corre `ray run` con un plazo: un cuelgue (el bug) mata al hijo y falla con mensaje, en vez de
/// colgar la suite.
fn run_with_deadline(dir: &std::path::Path, args: &[&str], env: &[(&str, &str)], secs: u64) -> (String, String, Option<i32>) {
    let mut cmd = Command::new(BIN);
    cmd.args(args).current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("lanza ray");
    let start = std::time::Instant::now();
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        if start.elapsed().as_secs() > secs {
            let _ = child.kill();
            let _ = child.wait();
            return (String::new(), format!("PLAZO de {secs}s excedido (cuelgue)"), None);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

const PAIR: &str = r#"import std/net;

fn main() -> int {
    let ports: Channel<int> = Channel.new();
    let server = spawn(fn() -> int {
        match (net.tcp_listen("127.0.0.1", 0)) {
            Result.Ok(srv) => {
                send(ports, net.local_port(srv));
                match (net.tcp_accept(srv)) {
                    Result.Ok(conn) => {
                        match (net.socket_read(conn)) {
                            Result.Ok(msg) => { let _ = net.socket_write(conn, "eco: " + msg); },
                            Result.Err(e) => eprint("server read err: " + e),
                        }
                        close(conn);
                    },
                    Result.Err(e) => eprint("accept err: " + e),
                }
                close(srv);
                0
            },
            Result.Err(e) => { eprint("listen err: " + e); 1 },
        }
    });
    let port = match (recv(ports)) { Option.Some(p) => p, Option.None => 0 };
    match (net.tcp_connect("127.0.0.1", port)) {
        Result.Ok(c) => {
            let _ = net.socket_write(c, "hola");
            match (net.socket_read(c)) {
                Result.Ok(r) => print("cliente recibio: " + r),
                Result.Err(e) => print("cliente read err: " + e),
            }
            close(c);
        },
        Result.Err(e) => print("connect local err: " + e),
    }
    let code = join(server);
    print("fin " + to_string(code));
    0
}
"#;

#[test]
fn local_server_and_client_complete_without_a_poller() {
    let base = tmp("pair");
    std::fs::write(base.join("pair.ray"), PAIR).unwrap();
    for (label, env) in [("multicore", vec![]), ("un hilo", vec![("RAYLANG_THREADS", "1")])] {
        let (out, err, code) = run_with_deadline(&base, &["run", "pair.ray"], &env, 30);
        assert_eq!(code, Some(0), "{label}: exit 0\nstdout={out}\nstderr={err}");
        assert!(out.contains("cliente recibio: eco: hola"), "{label}: eco completo\n{out}");
        assert!(out.contains("fin 0"), "{label}: el servidor termina\n{out}");
    }
}

#[test]
fn local_webserver_and_fetch_complete_without_a_poller() {
    let base = tmp("http");
    let net_pkg = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages").join("net");
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        format!(
            "[package]\nname = \"http_local\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{}\"\n",
            net_pkg.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    std::fs::write(
        base.join("src/main.ray"),
        r#"import std/net;
import net/webserver;
import net/http;

fn handler(req: webserver.Request) -> webserver.Response {
    webserver.ok("hola")
}

fn main() -> int {
    let srv = match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(h) => h,
        Result.Err(e) => { eprint("listen: " + e); return 1; },
    };
    let port = net.local_port(srv);
    let _ = spawn(fn() -> int { match (webserver.serve_on(srv, handler)) { Result.Ok(_) => 0, Result.Err(e) => { eprint("serve: " + e); 1 } } });
    match (http.fetch("http://127.0.0.1:" + to_string(port) + "/")) {
        Result.Ok(resp) => { print("status " + to_string(resp.status) + " body " + to_string(resp.body.len()) + " bytes"); 0 },
        Result.Err(e) => { print("fetch err: " + e); 1 },
    }
}
"#,
    )
    .unwrap();
    let (out, err, code) = run_with_deadline(&base, &["run"], &[], 60);
    assert_eq!(code, Some(0), "exit 0\nstdout={out}\nstderr={err}");
    assert!(out.contains("status 200 body 4 bytes"), "fetch contra el webserver local\n{out}");
}
