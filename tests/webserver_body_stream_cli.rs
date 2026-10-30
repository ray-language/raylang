//! M436 (rayblob #284): el cuerpo de la petición BAJO DEMANDA (`ServeOptions.with_body_stream`:
//! `Request.body_stream` + `next_body_chunk`/`read_body`) y el 413 por el handler
//! (`pass_too_large` + `body_too_large(req)`). El servidor es `tests/fixtures/net_server/
//! body_stream_server.ray` sobre `packages/net` por ruta; el cliente, TCP crudo desde aquí: cuerpo
//! con Content-Length y chunked trozo a trozo, pipelining tras un cuerpo consumido, `read_body` de
//! un cuerpo pequeño, 413 en XML propio (y en texto plano sin la opción), cierre tras un cuerpo
//! que el handler no agotó, y stderr limpio bajo `quiet`. VM y nativo.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn project(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let d = std::env::temp_dir().join(format!("raylang_test_body_stream_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(
        d.join("ray.toml"),
        format!(
            "[package]\nname = \"bodystream\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{}\"\n",
            root.join("packages/net").display()
        ),
    )
    .unwrap();
    std::fs::copy(root.join("tests/fixtures/net_server/body_stream_server.ray"), d.join("src/main.ray")).unwrap();
    d
}

struct Server {
    child: Child,
    port: u16,
}

impl Server {
    fn start(dir: &PathBuf, native: Option<&PathBuf>, stream: bool) -> Server {
        let mut cmd = match native {
            Some(bin) => Command::new(bin),
            None => {
                let mut c = Command::new(env!("CARGO_BIN_EXE_ray"));
                c.args(["run", "src/main.ray"]);
                c
            }
        };
        if stream {
            cmd.arg("stream");
        }
        let mut child = cmd.current_dir(dir).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
        let port: u16 = line.trim().strip_prefix("port ").expect("línea `port N`").parse().unwrap();
        Server { child, port }
    }

    fn stop(mut self) -> String {
        let _ = self.child.kill();
        let mut err = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let _ = self.child.wait();
        err
    }
}

/// Envía `raw` y lee hasta tener la respuesta entera (por Content-Length) o cerrar/agotar el plazo.
fn request(port: u16, raw: &[u8]) -> Vec<u8> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(raw).unwrap();
    read_response(&mut s)
}

fn read_response(s: &mut TcpStream) -> Vec<u8> {
    let mut data = Vec::new();
    let mut buf = [0u8; 65536];
    loop {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                if let Some(pos) = find(&data, b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&data[..pos]).to_lowercase();
                    let cl = head.lines().find_map(|l| l.strip_prefix("content-length:")).map(|v| v.trim().parse::<usize>().unwrap());
                    if let Some(cl) = cl
                        && data.len() - pos - 4 >= cl
                    {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
    data
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn body_of(d: &[u8]) -> String {
    let pos = find(d, b"\r\n\r\n").unwrap_or(d.len());
    String::from_utf8_lossy(&d[(pos + 4).min(d.len())..]).into_owned()
}

fn status_of(d: &[u8]) -> String {
    String::from_utf8_lossy(d).lines().next().unwrap_or("").to_string()
}

fn put(path: &str, body: &[u8]) -> Vec<u8> {
    let mut r = format!("PUT {path} HTTP/1.1\r\nHost: a\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    r.extend_from_slice(body);
    r
}

fn check_streaming(port: u16) {
    let big = vec![b'x'; 150_000];
    let d = request(port, &put("/upload", &big));
    assert_eq!(body_of(&d), "bytes=150000 pieces>0=true streamed=true declared=150000 body_len=0", "{}", String::from_utf8_lossy(&d));
    // chunked: tres chunks de tamaños distintos, el último de 1 octeto.
    let mut raw = b"PUT /upload HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for c in [vec![b'a'; 5000], vec![b'b'; 70_000], vec![b'c'; 1]] {
        raw.extend_from_slice(format!("{:x}\r\n", c.len()).as_bytes());
        raw.extend_from_slice(&c);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"0\r\n\r\n");
    let d = request(port, &raw);
    assert_eq!(body_of(&d), "bytes=75001 pieces>0=true streamed=true declared=-1 body_len=0", "{}", String::from_utf8_lossy(&d));
    // Pipelining: la segunda petición viene pegada al cuerpo de la primera (el arrastre).
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(1500))).unwrap();
    s.write_all(b"PUT /upload HTTP/1.1\r\nHost: a\r\nContent-Length: 10\r\n\r\n0123456789GET /second HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
    let mut all = Vec::new();
    let mut buf = [0u8; 65536];
    while let Ok(n) = s.read(&mut buf) {
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n]);
        if find(&all, b"plain GET /second").is_some() {
            break;
        }
    }
    let text = String::from_utf8_lossy(&all);
    assert_eq!(text.matches("HTTP/1.1 200").count(), 2, "{text}");
    assert!(text.contains("bytes=10 ") && text.contains("plain GET /second"), "{text}");
    // read_body de un cuerpo pequeño.
    let d = request(port, &put("/small", b"hello"));
    assert_eq!(body_of(&d), "small=hello");
    // 413 por el handler, en XML, con el tamaño declarado; sin leer nada del cuerpo.
    let d = request(port, b"PUT /upload HTTP/1.1\r\nHost: a\r\nContent-Length: 300000000\r\n\r\n");
    assert_eq!(status_of(&d), "HTTP/1.1 413 Content Too Large");
    assert_eq!(body_of(&d), "<Error><Code>EntityTooLarge</Code><Path>/upload</Path><Size>300000000</Size></Error>");
    assert!(String::from_utf8_lossy(&d).to_lowercase().contains("content-type: application/xml"), "{}", String::from_utf8_lossy(&d));
    // Un cuerpo que el handler no agotó: responde y la conexión se cierra (no se reutiliza).
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut raw = format!("PUT /partial HTTP/1.1\r\nHost: a\r\nContent-Length: {}\r\n\r\n", big.len()).into_bytes();
    raw.extend_from_slice(&big[..1000]);
    s.write_all(&raw).unwrap();
    let d = read_response(&mut s);
    assert_eq!(status_of(&d), "HTTP/1.1 200 OK", "{}", String::from_utf8_lossy(&d));
    assert_eq!(body_of(&d), "partial");
    let _ = s.write_all(b"GET /x HTTP/1.1\r\nHost: a\r\n\r\n");
    let extra = read_response(&mut s);
    assert!(extra.is_empty(), "la conexión debía cerrarse: {}", String::from_utf8_lossy(&extra));
}

fn check_plain(port: u16) {
    // Sin la opción: el cuerpo llega entero y `next_body_chunk` lo entrega una vez (el mismo handler
    // sirve en los dos modos); el 413 es el texto plano del servidor.
    let d = request(port, &put("/upload", &vec![b'x'; 150_000]));
    assert_eq!(body_of(&d), "bytes=150000 pieces>0=true streamed=false declared=150000 body_len=0", "{}", String::from_utf8_lossy(&d));
    let d = request(port, b"PUT /upload HTTP/1.1\r\nHost: a\r\nContent-Length: 300000000\r\n\r\n");
    assert_eq!(status_of(&d), "HTTP/1.1 413 Content Too Large");
    assert_eq!(body_of(&d), "Content Too Large");
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn body_streaming_and_handler_413_on_the_vm() {
    let d = project("vm");
    let srv = Server::start(&d, None, true);
    check_streaming(srv.port);
    let err = srv.stop();
    assert!(err.trim().is_empty(), "stderr debía quedar limpio bajo quiet: {err}");
    let srv = Server::start(&d, None, false);
    check_plain(srv.port);
    srv.stop();
}

#[test]
fn body_streaming_and_handler_413_natively() {
    if !has_rustc() {
        eprintln!("sin rustc: se omite el nativo");
        return;
    }
    let d = project("native");
    let bin = d.join("srv_native");
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "--native", "src/main.ray", "-o"])
        .arg(&bin)
        .current_dir(&d)
        .env("RAYLANG_TRANSPILE_DEBUG", "1")
        .output()
        .unwrap();
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{log}");
    // El bucle del servidor con las opciones nuevas tiene que transpilar entero (sin stubs).
    assert!(!log.contains("[transpile stub]"), "{log}");
    let srv = Server::start(&d, Some(&bin), true);
    check_streaming(srv.port);
    let err = srv.stop();
    assert!(err.trim().is_empty(), "stderr debía quedar limpio bajo quiet: {err}");
    let srv = Server::start(&d, Some(&bin), false);
    check_plain(srv.port);
    srv.stop();
}
