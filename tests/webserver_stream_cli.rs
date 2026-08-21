//! Pruebas del **streaming del webserver** (`stream_response`, M110) y de **Range/206** en
//! `static_mount`. El test de incrementalidad usa el margen del PRODUCTOR: el handler emite el
//! chunk 1 y duerme 2 s antes del chunk 2 — si el servidor bufferizase la respuesta entera, el
//! primer trozo no podría llegar al cliente en menos de ese sueño.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SERVER: &str = r#"import net/webserver;
import std/time;

fn handler(req: webserver.Request) -> webserver.Response {
    if (req.path == "/stream") {
        let ch: Channel<bytes> = Channel.bounded(4);
        let _ = spawn(fn() {
            send(ch, b"primero");
            time.sleep(2000);
            send(ch, b"segundo");
            close(ch);
        });
        let r = webserver.stream_response(200, ch);
        r.headers.insert("Content-Type", "text/plain");
        return r;
    }
    if (req.path.starts_with("/static/")) {
        return webserver.static_mount("/static/", "public", req);
    }
    webserver.ok("hola\n")
}

fn main() -> int {
    match (webserver.serve("127.0.0.1", __PORT__, handler)) {
        Result.Ok(_) => 0,
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;

struct Server {
    child: std::process::Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Levanta el servidor raylang (VM) con `public/data.bin` = "0123456789ABCDEF" y espera a que acepte.
fn start_server(name: &str) -> Server {
    let dir = std::env::temp_dir().join(format!("ray_wstream_{name}"));
    let net = dir.join("net");
    std::fs::create_dir_all(&net).expect("crea net/");
    std::fs::create_dir_all(dir.join("public")).expect("crea public/");
    for lib in ["webserver.ray", "trace.ray", "http.ray", "log.ray", "time.ray"] {
        let src = format!("{}/packages/net/{lib}", env!("CARGO_MANIFEST_DIR"));
        let _ = std::fs::copy(&src, net.join(lib));
    }
    std::fs::write(dir.join("public/data.bin"), b"0123456789ABCDEF").unwrap();
    // Puerto efímero: bind propio, se libera y se le pasa al servidor (carrera improbable en CI).
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    std::fs::write(dir.join("main.ray"), SERVER.replace("__PORT__", &port.to_string())).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["--vm", "main.ray"])
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("lanza el servidor");
    // Espera activa a que el puerto acepte.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "el servidor no llegó a escuchar");
        std::thread::sleep(Duration::from_millis(50));
    }
    Server { child, port }
}

/// GET crudo por TCP; devuelve la respuesta completa como bytes.
fn raw_get(port: u16, path: &str, extra: &str) -> Vec<u8> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    let req = format!("GET {path} HTTP/1.1\r\nHost: x\r\n{extra}Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    out
}

fn head_and_body(resp: &[u8]) -> (String, Vec<u8>) {
    let sep = resp.windows(4).position(|w| w == b"\r\n\r\n").expect("separador");
    (String::from_utf8_lossy(&resp[..sep]).into_owned(), resp[sep + 4..].to_vec())
}

#[test]
fn stream_response_delivers_chunks_as_produced() {
    let srv = start_server("stream");
    // Lee el PRIMER chunk y cronometra: el productor duerme 2 s antes del segundo — si el
    // servidor bufferizase el cuerpo entero, nada llegaría antes de ese sueño.
    let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
    s.write_all(b"GET /stream HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let t0 = Instant::now();
    let mut r = BufReader::new(s);
    // Cabecera: hasta la línea en blanco.
    let mut line = String::new();
    let mut chunked = false;
    loop {
        line.clear();
        r.read_line(&mut line).unwrap();
        if line.to_lowercase().starts_with("transfer-encoding:") && line.to_lowercase().contains("chunked") {
            chunked = true;
        }
        if line == "\r\n" {
            break;
        }
    }
    assert!(chunked, "la respuesta va en chunked");
    // Primer chunk: "<hex>\r\n" + datos.
    line.clear();
    r.read_line(&mut line).unwrap();
    let n = usize::from_str_radix(line.trim(), 16).expect("tamaño hex");
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).unwrap();
    let first_at = t0.elapsed();
    assert_eq!(&body, b"primero");
    assert!(
        first_at < Duration::from_millis(1500),
        "el primer trozo llegó a los {first_at:?}: el servidor lo bufferizó (el productor duerme 2 s)"
    );
    // El resto: segundo chunk + terminador.
    let mut rest = Vec::new();
    r.read_to_end(&mut rest).unwrap();
    let rest_s = String::from_utf8_lossy(&rest);
    assert!(rest_s.contains("segundo"), "llega el segundo trozo: {rest_s}");
    assert!(rest_s.ends_with("0\r\n\r\n"), "terminador chunked: {rest_s:?}");
}

#[test]
fn range_requests_on_static_mount() {
    let srv = start_server("range");
    // 200 completo anuncia Accept-Ranges.
    let (h, b) = head_and_body(&raw_get(srv.port, "/static/data.bin", ""));
    assert!(h.contains("200"), "{h}");
    assert!(h.to_lowercase().contains("accept-ranges: bytes"), "{h}");
    assert_eq!(b, b"0123456789ABCDEF");
    // 206 con rango cerrado.
    let (h, b) = head_and_body(&raw_get(srv.port, "/static/data.bin", "Range: bytes=4-7\r\n"));
    assert!(h.contains("206 Partial Content"), "{h}");
    assert!(h.to_lowercase().contains("content-range: bytes 4-7/16"), "{h}");
    assert_eq!(b, b"4567");
    // Sufijo y abierto.
    let (_, b) = head_and_body(&raw_get(srv.port, "/static/data.bin", "Range: bytes=-4\r\n"));
    assert_eq!(b, b"CDEF");
    let (_, b) = head_and_body(&raw_get(srv.port, "/static/data.bin", "Range: bytes=12-\r\n"));
    assert_eq!(b, b"CDEF");
    // Insatisfacible → 416 con el tamaño total.
    let (h, _) = head_and_body(&raw_get(srv.port, "/static/data.bin", "Range: bytes=99-\r\n"));
    assert!(h.contains("416 Range Not Satisfiable"), "{h}");
    assert!(h.to_lowercase().contains("content-range: bytes */16"), "{h}");
    // Multi-rango → 200 completo (permitido por RFC; fuera de v1).
    let (h, b) = head_and_body(&raw_get(srv.port, "/static/data.bin", "Range: bytes=0-1,4-5\r\n"));
    assert!(h.contains("200"), "{h}");
    assert_eq!(b, b"0123456789ABCDEF");
    // If-Range con validador distinto → 200 completo (el archivo pudo cambiar bajo la descarga).
    let (h, _) = head_and_body(&raw_get(
        srv.port,
        "/static/data.bin",
        "Range: bytes=4-7\r\nIf-Range: \"otro\"\r\n",
    ));
    assert!(h.contains("200"), "{h}");
    // If-Range con NUESTRO ETag → 206.
    let (h, _) = head_and_body(&raw_get(srv.port, "/static/data.bin", ""));
    let etag = h.lines().find(|l| l.to_lowercase().starts_with("etag:")).expect("etag")[5..].trim().to_string();
    let (h, b) = head_and_body(&raw_get(
        srv.port,
        "/static/data.bin",
        &format!("Range: bytes=4-7\r\nIf-Range: {etag}\r\n"),
    ));
    assert!(h.contains("206"), "{h}");
    assert_eq!(b, b"4567");
    // El 304 (If-None-Match) sigue ganando al Range.
    let (h, _) = head_and_body(&raw_get(
        srv.port,
        "/static/data.bin",
        &format!("Range: bytes=4-7\r\nIf-None-Match: {etag}\r\n"),
    ));
    assert!(h.contains("304"), "{h}");
}

#[test]
fn concat_position_collision_runs_on_both_engines() {
    // El bug que este arco destapó (V2): `("a" + "b").len() + 3` reventaba la VM con
    // "the checker guarantees strings" — el `+` exterior heredaba la posición del interior
    // registrado para ConcatN. Ejecutable en ambos motores, con el valor correcto.
    let dir = std::env::temp_dir().join("ray_wstream_concat");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("prog.ray"),
        "fn main() -> int {\n    let b = (\"ab\" + \"cd\").to_bytes() + b\"xy\" + b\"z\";\n    print(b.len());\n    (\"a\" + \"b\").len() + 3\n}\n",
    )
    .unwrap();
    for engine in ["--vm", "--interp"] {
        let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
            .args([engine, "prog.ray"])
            .current_dir(&dir)
            .output()
            .expect("lanza");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "7\n", "{engine}");
        assert_eq!(out.status.code(), Some(5), "{engine}: exit = 2 + 3");
        assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"), "{engine}");
    }
}
