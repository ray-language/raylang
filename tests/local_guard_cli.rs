//! M297 (IDEAS §96 #10): un servidor en 127.0.0.1 de una app de escritorio/móvil, con
//! `local_limits(token)`. (1) Guarda de origen: una petición por loopback con un `Origin` http(s)
//! cuyo host no es el `Host` es una página web en otra pestaña atacando el servidor local → 403; sin
//! `Origin`, con el origen propio, con un origen no-http (`ray://app`) o detrás de un proxy
//! (`X-Forwarded-For`) pasa. (2) Token local: sin token → 403; con `X-Ray-Token`, cookie `ray_local`
//! o `?ray_token=` pasa, y el query siembra la cookie. Sin `local_limits` (modo plain) NADA de esto
//! aplica: un servidor corriente sigue igual. Sobre el paquete `net` real.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const MAIN: &str = r#"
import net/webserver;
import std/net;

fn handler(req: webserver.Request) -> webserver.Response {
    webserver.ok("hello " + req.path)
}

fn main() -> int {
    let mode = args()[0];
    let srv = match (net.tcp_listen("127.0.0.1", 0)) { Result.Ok(s) => s, Result.Err(e) => { print(e); return 1; } };
    print(net.local_port(srv));
    let limits = if (mode == "token") { webserver.local_limits("s3cr3t") } else { webserver.default_limits() };
    match (webserver.serve_on_limits(srv, limits, handler)) {
        Result.Ok(_) => 0,
        Result.Err(e) => { print("serve: " + e); 1 },
    }
}
"#;

fn launch(mode: &str) -> (Child, u16) {
    let dir = std::env::temp_dir().join(format!("ray_local_guard_{}_{mode}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::write(dir.join("main.ray"), MAIN).unwrap();
    std::fs::write(dir.join("ray.toml"), format!("[package]\nname = \"guard\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{root}/packages/net\"\n")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["run", "main.ray", mode])
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("lanza");
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let port: u16 = line.trim().parse().unwrap_or_else(|_| panic!("puerto: {line:?}"));
    // El servidor sigue imprimiendo («listening on port N»): si se suelta el lector, el pipe se
    // cierra y el siguiente print mata el proceso (EPIPE) → conexión reseteada. Se drena aparte.
    std::thread::spawn(move || { let mut sink = String::new(); while reader.read_line(&mut sink).map(|n| n > 0).unwrap_or(false) { sink.clear(); } });
    (child, port)
}

fn ask(port: u16, req: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn status(resp: &str) -> u16 {
    resp.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("sin status: {resp:?}"))
}

#[test]
fn cross_site_origin_is_forbidden_only_under_local_limits() {
    // Sin local_limits: un servidor corriente no aplica la guarda (CORS, proxies, APIs: sin cambio).
    let (mut plain, pport) = launch("plain");
    let phost = format!("127.0.0.1:{pport}");
    assert_eq!(status(&ask(pport, &format!("POST /a HTTP/1.1\r\nHost: {phost}\r\nOrigin: https://evil.example\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"))), 200);
    plain.kill().ok();
    plain.wait().ok();

    let (mut child, port) = launch("token");
    let host = format!("127.0.0.1:{port}");
    let tok = "X-Ray-Token: s3cr3t\r\n";
    // Sin Origin (la propia ventana, curl): pasa.
    assert_eq!(status(&ask(port, &format!("GET /a HTTP/1.1\r\nHost: {host}\r\n{tok}Connection: close\r\n\r\n"))), 200);
    // Origen propio: pasa.
    assert_eq!(status(&ask(port, &format!("GET /a HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\n{tok}Connection: close\r\n\r\n"))), 200);
    // Origen no-http (la ventana ray://app): pasa.
    assert_eq!(status(&ask(port, &format!("GET /a HTTP/1.1\r\nHost: {host}\r\nOrigin: ray://app\r\n{tok}Connection: close\r\n\r\n"))), 200);
    // Una página web en otra pestaña (aunque robara el token): 403 — también un POST y un upgrade WebSocket pasan por aquí.
    let r = ask(port, &format!("POST /a HTTP/1.1\r\nHost: {host}\r\nOrigin: https://evil.example\r\n{tok}Content-Length: 0\r\nConnection: close\r\n\r\n"));
    assert_eq!(status(&r), 403, "{r}");
    assert!(r.contains("cross-site"), "{r}");
    // Hosts distintos (localhost vs 127.0.0.1): no se normaliza → 403; la ventana propia usa el mismo.
    assert_eq!(status(&ask(port, &format!("GET /a HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://{host}\r\n{tok}Connection: close\r\n\r\n"))), 403);
    // Detrás de un proxy (X-Forwarded-For): el peer no es el navegador → la guarda de origen no aplica.
    assert_eq!(status(&ask(port, &format!("GET /a HTTP/1.1\r\nHost: {host}\r\nOrigin: https://evil.example\r\nX-Forwarded-For: 10.0.0.9\r\n{tok}Connection: close\r\n\r\n"))), 200);
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn local_token_gates_every_request_and_seeds_the_cookie() {
    let (mut child, port) = launch("token");
    let host = format!("127.0.0.1:{port}");
    // Sin token: 403.
    let r = ask(port, &format!("GET /a HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"));
    assert_eq!(status(&r), 403, "{r}");
    assert!(r.contains("local token"), "{r}");
    // Token equivocado: 403.
    assert_eq!(status(&ask(port, &format!("GET /a HTTP/1.1\r\nHost: {host}\r\nX-Ray-Token: nope\r\nConnection: close\r\n\r\n"))), 403);
    // Cabecera: 200, sin cookie.
    let r = ask(port, &format!("GET /a HTTP/1.1\r\nHost: {host}\r\nX-Ray-Token: s3cr3t\r\nConnection: close\r\n\r\n"));
    assert_eq!(status(&r), 200, "{r}");
    assert!(!r.to_ascii_lowercase().contains("set-cookie: ray_local"), "{r}");
    // Query: 200 y siembra la cookie HttpOnly; SameSite=Strict.
    let r = ask(port, &format!("GET /a?x=1&ray_token=s3cr3t HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"));
    assert_eq!(status(&r), 200, "{r}");
    let sc = r.lines().find(|l| l.to_ascii_lowercase().starts_with("set-cookie: ray_local=")).unwrap_or_else(|| panic!("sin cookie: {r}"));
    assert!(sc.contains("HttpOnly") && sc.contains("SameSite=Strict"), "{sc}");
    // Cookie: 200.
    assert_eq!(status(&ask(port, &format!("GET /a HTTP/1.1\r\nHost: {host}\r\nCookie: ray_local=s3cr3t\r\nConnection: close\r\n\r\n"))), 200);
    // La guarda de origen sigue delante: token correcto pero Origin ajeno → 403.
    assert_eq!(status(&ask(port, &format!("GET /a HTTP/1.1\r\nHost: {host}\r\nX-Ray-Token: s3cr3t\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n"))), 403);
    child.kill().ok();
    child.wait().ok();
}
