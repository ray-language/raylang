//! Pruebas del cliente HTTP en raylang (M15.4b) + su composición con la librería JSON. La red no es
//! determinista para el oráculo: se prueba por subproceso contra un **servidor HTTP de juguete en
//! Rust** (un hilo que responde con cabeceras + cuerpo JSON y cierra). El driver copia `http.ray` y
//! `json.ray` a un temporal, hace `fetch` y parsea el cuerpo con la librería JSON; se verifica en
//! ambos motores.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

/// Levanta un servidor HTTP de juguete: acepta UNA conexión, lee la petición y responde 200 con un
/// cuerpo JSON y `Connection: close`. Devuelve el puerto efímero.
fn toy_http_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Leer la petición (una lectura basta para un GET pequeño).
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            // Cuerpo JSON; el cliente lee hasta EOF (Connection: close), no hace falta Content-Length.
            let resp = "HTTP/1.1 200 OK\r\n\
                        Content-Type: application/json\r\n\
                        Connection: close\r\n\
                        \r\n\
                        {\"ok\": true, \"n\": 42}";
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

/// Copia `http.ray` + `json.ray` a un temporal único, escribe `driver` como `main.ray` a su lado,
/// ejecuta en el motor dado y devuelve `(stdout, código)`.
fn run_with_libs(name: &str, driver: &str, vm: bool) -> (String, i32) {
    let mut dir = std::env::temp_dir();
    dir.push(format!("ray_http_{name}_{}", if vm { "vm" } else { "interp" }));
    std::fs::create_dir_all(&dir).expect("crea dir");
    for lib in ["http.ray", "json.ray", "inflate.ray"] {
        let src = format!("{}/examples/web/{lib}", env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(&src, dir.join(lib)).unwrap_or_else(|_| panic!("copia {lib}"));
    }
    let driver_path = dir.join("main.ray");
    std::fs::File::create(&driver_path).expect("crea driver").write_all(driver.as_bytes()).expect("escribe");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let out = cmd.arg(&driver_path).output().expect("lanza raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

const DRIVER: &str = r#"
from http import fetch, header, body_text;
from json import parse, stringify;

fn main() -> int {
    match (fetch("http://127.0.0.1:__PORT__/api")) {
        Result.Ok(resp) => {
            print(to_string(resp.status));
            match (header(resp, "Content-Type")) {
                Option.Some(ct) => print(ct),
                Option.None => print("(sin content-type)"),
            };
            // M19.2: el cuerpo es bytes → decodificar a texto antes de parsearlo como JSON.
            match (body_text(resp)) {
                Result.Ok(text) => {
                    match (parse(text)) {
                        Result.Ok(j) => print(stringify(j)),
                        Result.Err(e) => print("json err: " + e),
                    }
                },
                Result.Err(e) => print("utf8 err: " + e),
            };
            0
        },
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;

#[test]
fn client_http_compone_con_json() {
    for vm in [false, true] {
        let port = toy_http_server();
        let driver = DRIVER.replace("__PORT__", &port.to_string());
        let (out, code) = run_with_libs("compose", &driver, vm);
        let expected = "200\napplication/json\n{\"n\":42,\"ok\":true}";
        assert_eq!(out.trim(), expected, "http+json (vm={vm}): {out}");
        assert_eq!(code, 0, "output 0 (vm={vm})");
    }
}

#[test]
fn fetch_fails_con_url_no_http() {
    let driver = r#"
from http import fetch;
fn main() -> int {
    match (fetch("ftp://ejemplo.com/x")) {
        Result.Ok(_) => { print("inesperado"); 0 },
        Result.Err(e) => { print(e); 1 },
    }
}
"#;
    for vm in [false, true] {
        let (out, code) = run_with_libs("noproto", driver, vm);
        assert_eq!(out.trim(), "solo se soporta http:// o https://", "rejects no-http (vm={vm}): {out}");
        assert_eq!(code, 1);
    }
}

// --- M58.2: robustez del cliente ---

/// Servidor de juguete que CAPTURA la petición (la envía por el canal) y responde 200 con "ok".
fn toy_capture() -> (u16, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut acc = Vec::new();
            let mut buf = [0u8; 2048];
            // Lee hasta tener cabeceras + (si hay) el cuerpo declarado; para el test basta leer
            // hasta que la petición pequeña esté completa (dos lecturas como mucho).
            while !String::from_utf8_lossy(&acc).contains("\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => acc.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }
            // Si hay Content-Length, lee el cuerpo restante.
            let text = String::from_utf8_lossy(&acc).into_owned();
            if let Some(cl) = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))) {
                let fin_cab = acc.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(acc.len());
                while acc.len() < fin_cab + cl {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => acc.extend_from_slice(&buf[..n]),
                        Err(_) => break,
                    }
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&acc).into_owned());
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    (port, rx)
}

#[test]
fn peticion_con_host_port_accept_encoding_y_octetos() {
    // M58.2: Host lleva el puerto no-default, Accept-Encoding: gzip se anuncia, y el
    // Content-Length cuenta OCTETOS ("café" = 5 octetos UTF-8, no 4 caracteres — antes fallaba).
    let (port, rx) = toy_capture();
    let driver = r#"
from http import request, body_text;

fn main() -> int {
    match (request("POST", "http://127.0.0.1:__PORT__/api", "café")) {
        Result.Ok(r) => {
            match (body_text(r)) {
                Result.Ok(t) => print(t),
                Result.Err(e) => print("err: " + e),
            }
            0
        },
        Result.Err(e) => {
            print("err: " + e);
            1
        },
    }
}
"#
    .replace("__PORT__", &port.to_string());
    let (out, code) = run_with_libs("robust", &driver, true);
    assert_eq!(code, 0, "driver falló: {out}");
    assert!(out.contains("ok"), "esperaba el body ok: {out}");

    let req = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("petición capturada");
    assert!(req.contains(&format!("Host: 127.0.0.1:{port}")), "Host must llevar el port: {req}");
    assert!(req.contains("Accept-Encoding: gzip"), "must anunciar gzip: {req}");
    assert!(req.contains("Content-Length: 5"), "Content-Length en octetos (café = 5): {req}");
    assert!(req.ends_with("café"), "el body must llegar entero: {req}");
}

#[test]
fn client_corta_por_timeout_a_servidor_mudo() {
    // M58.2: un servidor que acepta y no responde ya no cuelga al cliente para siempre.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            thread::sleep(std::time::Duration::from_secs(30)); // mudo
            drop(stream);
        }
    });
    let driver = r#"
from http import request_bytes;

fn main() -> int {
    match (request_bytes("GET", "http://127.0.0.1:__PORT__/", b"", Map.new(), 400)) {
        Result.Ok(_) => {
            print("no debería responder");
            1
        },
        Result.Err(e) => {
            print("err: " + e);
            0
        },
    }
}
"#
    .replace("__PORT__", &port.to_string());
    let start = std::time::Instant::now();
    let (out, code) = run_with_libs("timeout", &driver, true);
    assert_eq!(code, 0, "esperaba Err por timeout: {out}");
    assert!(out.contains("tiempo de espera"), "esperaba el error de timeout: {out}");
    assert!(start.elapsed() < std::time::Duration::from_secs(10), "tardó demasiado (¿sin timeout?)");
}

#[test]
fn response_truncada_es_error() {
    // M58.2: Content-Length declarado 10 pero solo llegan 3 octetos → Err, no un cuerpo a medias.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nabc");
        }
    });
    let driver = r#"
from http import fetch;

fn main() -> int {
    match (fetch("http://127.0.0.1:__PORT__/")) {
        Result.Ok(_) => {
            print("no debería aceptarla");
            1
        },
        Result.Err(e) => {
            print("err: " + e);
            0
        },
    }
}
"#
    .replace("__PORT__", &port.to_string());
    let (out, code) = run_with_libs("truncada", &driver, true);
    assert_eq!(code, 0, "esperaba Err por truncada: {out}");
    assert!(out.contains("response truncada"), "esperaba el error de truncada: {out}");
}
