//! M123 — `Request.remote` + `remote_ip` del webserver de PRODUCCIÓN (`packages/net/webserver`):
//! el bucle de servicio rellena la dirección remota del cliente al leer la petición. Se monta un
//! proyecto temporal (ray.toml → path-dep al paquete net), el handler responde con `req.remote` y
//! `webserver.remote_ip(req)`, y un cliente HTTP en Rust verifica que el origen reportado es el
//! MISMO extremo local del socket del cliente (ip:puerto exactos, no solo "no vacío").

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

/// Crea el proyecto temporal (ray.toml → packages/net) y lanza `ray run`; devuelve el hijo y el
/// puerto anunciado por stdout (mismo patrón que webserver_shutdown_cli).
fn launch(name: &str, main: &str) -> (Child, u16) {
    let base = std::env::temp_dir().join(format!("ray_remote_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).expect("crea el dir temporal");
    let repo = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"remote\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(base.join("src/main.ray"), main).unwrap();
    let mut child = Command::new(BIN)
        .arg("run")
        .current_dir(&base)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza ray run");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut port_line = String::new();
    reader.read_line(&mut port_line).expect("lee el port");
    let port: u16 = port_line
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("invalid port: {port_line:?}"));
    child.stdout = Some(reader.into_inner());
    (child, port)
}

const SERVER: &str = r#"
import net/webserver;

fn handle(req: webserver.Request) -> webserver.Response {
    webserver.ok(req.remote + "|" + webserver.remote_ip(req))
}

fn main() -> int {
    match (webserver.serve("127.0.0.1", 0, handle)) {
        Result.Ok(_) => 0,
        Result.Err(e) => {
            eprint("serve: " + e);
            1
        },
    }
}
"#;

#[test]
fn request_remote_reports_the_client_endpoint() {
    let (mut child, port) = launch("basic", SERVER);
    // Conecta y guarda el extremo LOCAL del cliente: es exactamente lo que el servidor debe ver.
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let local = s.local_addr().expect("local_addr").to_string();
    let req = "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
    s.write_all(req.as_bytes()).expect("escribe");
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let _ = child.kill();
    let _ = child.wait();
    // El cuerpo es "<remote>|<remote_ip>".
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").trim();
    let (remote, ip) = body.split_once('|').unwrap_or_else(|| panic!("cuerpo inesperado: {body:?}"));
    assert_eq!(remote, local, "req.remote debe ser el extremo local del cliente");
    assert_eq!(ip, "127.0.0.1", "remote_ip debe quitar el puerto");
}
