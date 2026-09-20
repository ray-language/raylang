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
    // M271: stream de tamaño conocido (Content-Length, keep-alive).
    if (req.path == "/stream_len") {
        let ch: Channel<bytes> = Channel.bounded(4);
        let _ = spawn(fn() {
            send(ch, b"abc");
            send(ch, b"defgh");
            close(ch);
        });
        let r = webserver.stream_response_len(200, ch, 8);
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
    // M271: un archivo por encima del umbral de streaming (1 MB): patrón conocido por posición.
    let big: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(dir.join("public/big.bin"), &big).unwrap();
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

/// M271 (raystream [2]): un estático GRANDE se sirve desde el disco por trozos — el `Range` no lee
/// el archivo entero — con Content-Length (sin chunked), 206/Content-Range y el trozo exacto; el
/// GET completo anuncia el tamaño; el HEAD lleva las cabeceras sin cuerpo.
#[test]
fn big_static_files_stream_from_disk_with_ranges() {
    let srv = start_server("big");
    let expect = |i: u32| (i % 251) as u8;
    // Rango pequeño en medio del archivo.
    let (head, body) = head_and_body(&raw_get(srv.port, "/static/big.bin", "Range: bytes=1000000-1000010\r\n"));
    assert!(head.starts_with("HTTP/1.1 206"), "{head}");
    assert!(head.contains("Content-Range: bytes 1000000-1000010/1500000"), "{head}");
    assert!(head.contains("Content-Length: 11"), "{head}");
    assert!(!head.contains("Transfer-Encoding"), "sin chunked:\n{head}");
    assert!(head.contains("Accept-Ranges: bytes"), "{head}");
    assert_eq!(body, (1000000..=1000010u32).map(expect).collect::<Vec<u8>>());
    // Rango abierto hasta el final, y sufijo.
    let (head, body) = head_and_body(&raw_get(srv.port, "/static/big.bin", "Range: bytes=1499990-\r\n"));
    assert!(head.contains("Content-Range: bytes 1499990-1499999/1500000"), "{head}");
    assert_eq!(body.len(), 10);
    // Completo: 200 con el tamaño y el contenido íntegro (cruza varios trozos de 256 KB).
    let (head, body) = head_and_body(&raw_get(srv.port, "/static/big.bin", ""));
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.contains("Content-Length: 1500000"), "{head}");
    assert_eq!(body.len(), 1_500_000);
    assert!(body.iter().enumerate().all(|(i, b)| *b == expect(i as u32)), "contenido íntegro");
    // 416 fuera de rango.
    let (head, _b) = head_and_body(&raw_get(srv.port, "/static/big.bin", "Range: bytes=9000000-\r\n"));
    assert!(head.starts_with("HTTP/1.1 416") && head.contains("Content-Range: bytes */1500000"), "{head}");
    // HEAD: cabeceras del GET, sin cuerpo.
    let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
    s.write_all(b"HEAD /static/big.bin HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let (head, body) = head_and_body(&out);
    assert!(head.starts_with("HTTP/1.1 200") && head.contains("Content-Length: 1500000"), "{head}");
    assert!(body.is_empty(), "HEAD sin cuerpo");
}

/// M271 (raystream [3]): `stream_response_len` escribe el cuerpo en crudo con Content-Length y la
/// conexión sigue viva: dos peticiones por la misma conexión.
#[test]
fn known_length_streams_keep_the_connection_alive() {
    let srv = start_server("stream_len");
    let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET /stream_len HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut r = BufReader::new(s);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        if line == "\r\n" { break; }
        head.push_str(&line);
    }
    assert!(head.starts_with("HTTP/1.1 200") && head.contains("Content-Length: 8") && head.contains("Connection: keep-alive"), "{head}");
    assert!(!head.contains("Transfer-Encoding"), "{head}");
    let mut body = [0u8; 8];
    r.read_exact(&mut body).unwrap();
    assert_eq!(&body, b"abcdefgh");
    // Segunda petición por la MISMA conexión.
    r.get_mut().write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut rest = Vec::new();
    r.read_to_end(&mut rest).unwrap();
    let (head2, body2) = head_and_body(&rest);
    assert!(head2.starts_with("HTTP/1.1 200"), "{head2}");
    assert_eq!(body2, b"hola\n");
}

/// M271 (raystream [5]): un handler de `serve_raw` con estado en el nativo. Un closure GUARDADO EN UNA
/// VARIABLE no puede cruzar a las fibras de conexión (antes: tres E0277 de rustc sobre código
/// generado; ahora un error de raylang con el nombre). El mismo closure INLINE, y la fábrica
/// `serve_raw_with`, compilan.
#[test]
fn serve_raw_handlers_with_state_natively() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando: rustc no disponible");
        return;
    }
    let dir = std::env::temp_dir().join("ray_wstream_raw_native");
    let _ = std::fs::remove_dir_all(&dir);
    let net = dir.join("net");
    std::fs::create_dir_all(&net).unwrap();
    for lib in ["webserver.ray", "trace.ray", "http.ray", "log.ray", "time.ray"] {
        let src = format!("{}/packages/net/{lib}", env!("CARGO_MANIFEST_DIR"));
        let _ = std::fs::copy(&src, net.join(lib));
    }
    // 1) closure en variable → error claro de raylang (no rustc).
    std::fs::write(
        dir.join("bad.ray"),
        r#"import net/webserver;

fn main() -> int {
    let hits: Channel<int> = Channel.bounded(8);
    let handler = fn(req: webserver.Request, conn: int) {
        send(hits, 1);
        let _ = webserver.send_response(conn, webserver.text(200, "hi"));
    };
    if (false) { let _ = webserver.serve_raw("127.0.0.1", 0, handler); }
    0
}
"#,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "bad.ray", "--native", "-o", dir.join("bad_bin").to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(err.contains("'handler': a closure stored in a variable cannot be passed here in the native binary"), "{err}");
    assert!(!err.contains("E0277"), "sin errores de rustc:\n{err}");
    // 2) inline y con fábrica (serve_raw_with) → compila y corre.
    std::fs::write(
        dir.join("main.ray"),
        r#"import net/webserver;

fn main() -> int {
    let hits: Channel<int> = Channel.bounded(8);
    if (false) {
        let _ = webserver.serve_raw("127.0.0.1", 0, fn(req: webserver.Request, conn: int) {
            send(hits, 1);
            let _ = webserver.send_response(conn, webserver.text(200, "hi"));
        });
        let _ = webserver.serve_raw_with("127.0.0.1", 0, fn() -> fn(webserver.Request, int) {
            fn(req: webserver.Request, conn: int) {
                send(hits, 2);
                let _ = webserver.send_response(conn, webserver.text(200, "hi"));
            }
        });
    }
    print("built");
    0
}
"#,
    )
    .unwrap();
    let bin = dir.join("raw_bin");
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "main.ray", "--native", "-o", bin.to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "build --native inline + serve_raw_with\n{}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "built\n");
}
