//! Pruebas del **streaming del cliente HTTP** (`net/http.stream*`) y del **cliente SSE**
//! (`net/sse`), M108. La red no es determinista → servidores de juguete en Rust; el test clave
//! es de INCREMENTALIDAD y va por **handshake**: el servidor retiene el resto del cuerpo hasta
//! que el test ve el primer trozo en el stdout del cliente — si el cliente bufferizase hasta el
//! final (el `fetch` clásico), el test colgaría en vez de mentir (la lección de M107.2: con
//! temporizadores no hay tests honestos de concurrencia + procesos).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// Copia los módulos del paquete net bajo `<tmp>/net/`, escribe `main.ray` y devuelve el dir.
fn setup(name: &str, driver: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_hstream_{name}"));
    let net = dir.join("net");
    std::fs::create_dir_all(&net).expect("crea dir");
    for lib in ["http.ray", "trace.ray", "sse.ray"] {
        let src = format!("{}/packages/net/{lib}", env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(&src, net.join(lib)).unwrap_or_else(|_| panic!("copia {lib}"));
    }
    std::fs::write(dir.join("main.ray"), driver).expect("escribe driver");
    dir
}

const STREAM_DRIVER: &str = r#"import net/http;

fn main() -> int {
    let hs: Map<string, string> = Map.new();
    match (http.stream("GET", "http://127.0.0.1:__PORT__/s", b"", hs)) {
        Result.Err(e) => { eprint("open: " + e); 1 },
        Result.Ok(s) => {
            print("status " + to_string(s.status));
            var go = true;
            var code = 0;
            while (go) {
                match (http.stream_read(s)) {
                    Result.Ok(piece_opt) => {
                        match (piece_opt) {
                            Option.Some(b) => {
                                match (from_utf8(b)) {
                                    Result.Ok(t) => print("piece:" + t),
                                    Result.Err(_) => print("piece:<bin>"),
                                }
                            },
                            Option.None => { print("end"); go = false; },
                        }
                    },
                    Result.Err(e) => { eprint("read: " + e); code = 1; go = false; },
                }
            }
            code
        },
    }
}
"#;

#[test]
fn chunked_stream_delivers_pieces_incrementally() {
    // EL test del milestone. El servidor manda el chunk 1 y RETIENE el resto hasta que el test
    // ve "piece:hola " en el stdout del cliente (handshake): el primer trozo tiene que
    // atravesar el cliente ANTES de que exista el final del cuerpo.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        if let Ok((mut c, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = c.read(&mut buf);
            let _ = c.write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhola \r\n",
            );
            let _ = rx.recv(); // ← hasta que el cliente haya IMPRESO el primer trozo
            let _ = c.write_all(b"6\r\nmundo!\r\n0\r\n\r\n");
        }
    });

    for engine in ["--vm", "--interp"] {
        // El servidor de arriba atiende UNA conexión: por motor, servidor propio.
        let (port, tx) = if engine == "--vm" {
            (port, tx.clone())
        } else {
            let l2 = TcpListener::bind("127.0.0.1:0").expect("bind");
            let p2 = l2.local_addr().unwrap().port();
            let (tx2, rx2) = mpsc::channel::<()>();
            thread::spawn(move || {
                if let Ok((mut c, _)) = l2.accept() {
                    let mut buf = [0u8; 2048];
                    let _ = c.read(&mut buf);
                    let _ = c.write_all(
                        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhola \r\n",
                    );
                    let _ = rx2.recv();
                    let _ = c.write_all(b"6\r\nmundo!\r\n0\r\n\r\n");
                }
            });
            (p2, tx2)
        };
        let dir = setup(&format!("chunked_{}", engine.trim_start_matches("--")),
                        &STREAM_DRIVER.replace("__PORT__", &port.to_string()));
        let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
            .args([engine, "main.ray"])
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .spawn()
            .expect("lanza");
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        assert_eq!(lines.next().unwrap().unwrap(), "status 200", "{engine}");
        assert_eq!(lines.next().unwrap().unwrap(), "piece:hola ", "{engine}: el primer trozo llega ANTES del final del cuerpo");
        tx.send(()).unwrap(); // libera el resto
        assert_eq!(lines.next().unwrap().unwrap(), "piece:mundo!", "{engine}");
        assert_eq!(lines.next().unwrap().unwrap(), "end", "{engine}");
        assert!(child.wait().unwrap().success(), "{engine}: exit 0");
    }
}

/// Un servidor de una conexión que responde con `resp` tal cual y cierra.
fn one_shot_server(resp: &'static [u8]) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut c, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = c.read(&mut buf);
            let _ = c.write_all(resp);
        }
    });
    port
}

fn run_driver(dir: &std::path::Path, engine: &str) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args([engine, "main.ray"])
        .current_dir(dir)
        .output()
        .expect("lanza");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn content_length_stream_ends_cleanly_and_truncation_is_an_error() {
    for engine in ["--vm", "--interp"] {
        // Content-Length completo → trozos + end, exit 0.
        let port = one_shot_server(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nhola mundo");
        let dir = setup(&format!("cl_{}", engine.trim_start_matches("--")),
                        &STREAM_DRIVER.replace("__PORT__", &port.to_string()));
        let (out, err, code) = run_driver(&dir, engine);
        assert_eq!(code, 0, "{engine}\n{err}");
        assert!(out.starts_with("status 200\n"), "{engine}: {out}");
        assert!(out.ends_with("end\n"), "{engine}: {out}");
        let total: usize = out.lines().filter_map(|l| l.strip_prefix("piece:")).map(|p| p.len()).sum();
        assert_eq!(total, 10, "{engine}: los trozos suman el cuerpo entero: {out}");

        // Truncada (el servidor cierra a los 4 de 10) → Err, no un final limpio.
        let port = one_shot_server(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nhola");
        let dir = setup(&format!("cl_trunc_{}", engine.trim_start_matches("--")),
                        &STREAM_DRIVER.replace("__PORT__", &port.to_string()));
        let (out, err, code) = run_driver(&dir, engine);
        assert_eq!(code, 1, "{engine}: truncada = error\n{out}");
        assert!(err.contains("truncated body"), "{engine}: {err}");
    }
}

#[test]
fn eof_delimited_stream_works_without_length() {
    for engine in ["--vm", "--interp"] {
        let port = one_shot_server(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nsin longitud");
        let dir = setup(&format!("eof_{}", engine.trim_start_matches("--")),
                        &STREAM_DRIVER.replace("__PORT__", &port.to_string()));
        let (out, err, code) = run_driver(&dir, engine);
        assert_eq!(code, 0, "{engine}\n{err}");
        let body: String = out.lines().filter_map(|l| l.strip_prefix("piece:")).collect();
        assert_eq!(body, "sin longitud", "{engine}: {out}");
        assert!(out.ends_with("end\n"), "{engine}: {out}");
    }
}

const SSE_DRIVER: &str = r#"import net/sse;

fn main() -> int {
    let hs: Map<string, string> = Map.new();
    match (sse.open("http://127.0.0.1:__PORT__/events", hs)) {
        Result.Err(e) => { eprint("open: " + e); 1 },
        Result.Ok(es) => {
            var go = true;
            var code = 0;
            while (go) {
                match (sse.next(es)) {
                    Result.Ok(ev_opt) => {
                        match (ev_opt) {
                            Option.Some(ev) => print("[" + ev.event + "|" + ev.id + "] " + ev.data),
                            Option.None => { print("end"); go = false; },
                        }
                    },
                    Result.Err(e) => { eprint("next: " + e); code = 1; go = false; },
                }
            }
            code
        },
    }
}
"#;

#[test]
fn sse_events_flow_with_mid_utf8_splits_and_comments() {
    // El servidor parte el primer evento POR EL MEDIO de un carácter multibyte (la ñ de
    // "español") y mete un comentario keep-alive (que la spec no despacha).
    for engine in ["--vm", "--interp"] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Ok((mut c, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = c.read(&mut buf);
                let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                let ev = "event: token\ndata: espa\u{00f1}ol\n\n".as_bytes();
                let _ = c.write_all(&ev[..19]); // corta dentro de la ñ (0xc3 | 0xb1)
                let _ = c.flush();
                thread::sleep(std::time::Duration::from_millis(60));
                let _ = c.write_all(&ev[19..]);
                let _ = c.write_all(b": keep-alive\n\nid: 9\ndata: fin\n\n");
            }
        });
        let dir = setup(&format!("sse_{}", engine.trim_start_matches("--")),
                        &SSE_DRIVER.replace("__PORT__", &port.to_string()));
        let (out, err, code) = run_driver(&dir, engine);
        assert_eq!(code, 0, "{engine}\n{err}");
        assert_eq!(out, "[token|] español\n[|9] fin\nend\n", "{engine}: eventos exactos");
    }
}

const DECODE_DRIVER: &str = r#"import net/sse;

fn show(b: bytes) {
    match (sse.decode(b)) {
        Option.Some(evn) => print(
            "ev=[" + evn.0.event + "] id=[" + evn.0.id + "] data=[" + evn.0.data + "] +" + to_string(evn.1)
        ),
        Option.None => print("incomplete"),
    }
}

fn main() -> int {
    show(b"data: hola\n\n");
    show(b"data: hola\r\n\r\n");
    show(b"data: l1\ndata: l2\n\n");
    show(b"event: token\nid: 7\ndata: hola\n\n");
    show(b"data:sin-espacio\n\n");
    show(b": comentario keep-alive\n\n");
    show(b"data: a\xc3\xb1o\n\n");
    show(b"data: parcial");
    show(b"data: cr-final\r");
    show(b"data: dos\n\nevent: siguiente\n");
    0
}
"#;

const DECODE_WANT: &str = "ev=[] id=[] data=[hola] +12\n\
ev=[] id=[] data=[hola] +14\n\
ev=[] id=[] data=[l1\nl2] +19\n\
ev=[token] id=[7] data=[hola] +31\n\
ev=[] id=[] data=[sin-espacio] +18\n\
ev=[] id=[] data=[] +25\n\
ev=[] id=[] data=[año] +12\n\
incomplete\n\
incomplete\n\
ev=[] id=[] data=[dos] +11\n";

#[test]
fn sse_decoder_battery_is_pure_and_exact() {
    // El decodificador puro, sin red — CR/CRLF/LF, data multilínea, comentarios, sin-espacio,
    // UTF-8, prefijos incompletos (incluido el CR final que no decide). También en NATIVO.
    let dir = setup("decode", DECODE_DRIVER);
    for engine in ["--vm", "--interp"] {
        let (out, err, code) = run_driver(&dir, engine);
        assert_eq!(code, 0, "{engine}\n{err}");
        assert_eq!(out, DECODE_WANT, "{engine}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = dir.join("prog_bin");
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "main.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&dir)
            .output()
            .expect("build");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let native = Command::new(&bin).output().expect("nativo");
        assert_eq!(String::from_utf8_lossy(&native.stdout), DECODE_WANT, "nativo ≡ VM");
    }
}

/// La clase RefCell-en-args del NATIVO (ago 2026): `stream_take(s, s.remaining)` dejaba vivo el
/// guard del `borrow()` del argumento DURANTE la llamada (que hace `borrow_mut` del mismo Stream)
/// → "RefCell already borrowed" en el primer trozo. El fix iza los args a temporales. Este e2e
/// compila el cliente de streaming a binario nativo y lo corre contra un servidor real.
#[test]
fn native_stream_read_matches_expected() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando native_stream_read: rustc no disponible");
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut c, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = c.read(&mut buf);
            let body = b"hola mundo streaming nativo";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = c.write_all(head.as_bytes());
            let _ = c.write_all(body);
        }
    });
    let driver = r#"import net/http;

fn main() -> int {
    let hs: Map<string, string> = Map.new();
    match (http.stream("GET", "http://127.0.0.1:__PORT__/d", b"", hs)) {
        Result.Err(e) => { print("err " + e); 1 },
        Result.Ok(s) => {
            print("status " + to_string(s.status));
            var total = 0;
            var go = true;
            var code = 0;
            while (go) {
                match (http.stream_read(s)) {
                    Result.Err(e) => { print("read err " + e); code = 1; go = false; },
                    Result.Ok(opt) => match (opt) {
                        Option.Some(b) => { total = total + b.len(); },
                        Option.None => { go = false; },
                    },
                }
            }
            print("total " + to_string(total));
            code
        },
    }
}
"#
    .replace("__PORT__", &port.to_string());
    let dir = setup("native_read", &driver);
    let bin = dir.join("prog_bin");
    let st = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "main.ray", "--native", "-o", bin.to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .expect("build");
    assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
    let native = Command::new(&bin).output().expect("nativo");
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        "status 200\ntotal 27\n",
        "stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(native.status.code(), Some(0));
}
