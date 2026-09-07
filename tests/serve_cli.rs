//! M199 — `ray serve [dir] [--host H] [--port N]`: el servidor de archivos estáticos escrito EN
//! raylang (`src/serve.ray`, embebido) para previsualizar `_site/`, `playground/`, etc. Se
//! levanta en `--port 0`, se lee el puerto asignado de su primera línea y se habla HTTP crudo
//! por TcpStream (sin normalización de rutas del cliente: así se prueba el `..` de verdad).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_ray");

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(dir: &std::path::Path) -> Server {
    let mut child = Command::new(BIN)
        .arg("serve")
        .arg(dir)
        .args(["--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("arranca ray serve");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    // Sigue drenando el log del servidor: si se cerrara el pipe, su siguiente `print`
    // lo mataría por SIGPIPE.
    std::thread::spawn(move || std::io::copy(&mut out, &mut std::io::sink()));
    let port: u16 = first
        .split("127.0.0.1:")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("primera línea con el puerto: {first:?}"));
    Server { child, port }
}

/// Manda una petición cruda y devuelve (línea de estado, cabeceras, cuerpo).
fn request(port: u16, raw: &str) -> (String, String, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    s.write_all(raw.as_bytes()).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("fin de cabeceras");
    let head = String::from_utf8_lossy(&buf[..split]).to_string();
    let body = buf[split + 4..].to_vec();
    let (status, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
    (status.to_string(), headers.to_string(), body)
}

fn get(port: u16, path: &str) -> (String, String, Vec<u8>) {
    request(
        port,
        &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
    )
}

#[test]
fn serves_a_static_directory_with_index_mime_404_and_no_traversal() {
    let base = std::env::temp_dir().join(format!("ray_serve_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("sub")).unwrap();
    std::fs::write(base.join("index.html"), "<h1>root</h1>").unwrap();
    std::fs::write(base.join("sub/index.html"), "<h1>sub</h1>").unwrap();
    std::fs::write(base.join("app.js"), "console.log(1)").unwrap();
    std::fs::write(base.join("data.bin"), [0u8, 255, 7]).unwrap();
    std::fs::write(base.join("with space.txt"), "spaced").unwrap();
    let server = start(&base);
    let port = server.port;

    let (status, headers, body) = get(port, "/");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(
        headers.contains("Content-Type: text/html; charset=utf-8"),
        "{headers}"
    );
    assert!(headers.contains("Content-Length: 13"), "{headers}");
    assert!(headers.contains("Cache-Control: no-store"), "{headers}");
    assert_eq!(body, b"<h1>root</h1>");

    let (status, headers, body) = get(port, "/app.js?v=2");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(headers.contains("text/javascript"), "{headers}");
    assert_eq!(body, b"console.log(1)");

    let (status, headers, body) = get(port, "/data.bin");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(headers.contains("application/octet-stream"), "{headers}");
    assert_eq!(body, vec![0u8, 255, 7], "octetos crudos intactos");

    // Directorio sin barra → 301 a `dir/` (los enlaces relativos de su index resuelven bien).
    let (status, headers, _) = get(port, "/sub");
    assert_eq!(status, "HTTP/1.1 301 Moved Permanently");
    assert!(headers.contains("Location: /sub/"), "{headers}");
    let (status, _, body) = get(port, "/sub/");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(body, b"<h1>sub</h1>");

    // Escapes %XX en el nombre.
    let (status, _, body) = get(port, "/with%20space.txt");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(body, b"spaced");

    let (status, _, body) = get(port, "/missing.html");
    assert_eq!(status, "HTTP/1.1 404 Not Found");
    assert_eq!(body, b"not found\n");

    // Sin traversal: el `..` crudo (el cliente NO lo normaliza) y su forma escapada.
    for path in [
        "/../Cargo.toml",
        "/sub/../../Cargo.toml",
        "/%2e%2e/Cargo.toml",
    ] {
        let (status, _, _) = get(port, path);
        assert_eq!(
            status, "HTTP/1.1 404 Not Found",
            "traversal bloqueado: {path}"
        );
    }

    // HEAD: cabeceras sin cuerpo; otros métodos: 405.
    let (status, headers, body) = request(port, "HEAD / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(headers.contains("Content-Length: 13"), "{headers}");
    assert!(body.is_empty(), "HEAD sin cuerpo");
    let (status, _, _) = request(
        port,
        "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
    );
    assert_eq!(status, "HTTP/1.1 405 Method Not Allowed");

    drop(server);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn rejects_a_missing_directory_and_bad_options() {
    let r = Command::new(BIN)
        .args(["serve", "/definitely/not/here"])
        .output()
        .unwrap();
    assert_eq!(r.status.code(), Some(66));
    assert!(String::from_utf8_lossy(&r.stderr).contains("not a directory"));
    let r = Command::new(BIN)
        .args(["serve", ".", "--bogus"])
        .output()
        .unwrap();
    assert_eq!(r.status.code(), Some(64));
    let r = Command::new(BIN)
        .args(["serve", "--help"])
        .output()
        .unwrap();
    assert_eq!(r.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&r.stdout).contains("usage: ray serve"));
}
