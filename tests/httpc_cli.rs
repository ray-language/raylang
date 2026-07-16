//! Pruebas del cliente HTTP robusto (M20.7, ampliación de `examples/web/http.ray`): cabeceras/métodos
//! arbitrarios (`request_with`), seguimiento de redirecciones (`fetch_follow`) y Transfer-Encoding
//! chunked. Servidor de juguete en Rust (bucle de conexiones, una por `fetch`) por ambos motores.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

/// Servidor que atiende varias conexiones (cada `fetch` abre una nueva con `Connection: close`).
/// Decide la respuesta por la ruta de la línea de petición.
fn toy_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut stream = match conn {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = req.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("/").to_string();

            // /gzip: cuerpo binario (gzip de "raylang es un lenguaje...") con Content-Encoding: gzip.
            if path == "/gzip" {
                const GZIP: &[u8] = &[
                    31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 43, 74, 172, 204, 73, 204, 75, 87, 72, 45, 86, 40,
                    205, 83, 200, 73, 205, 75, 47, 77, 204, 74, 85, 72, 73, 85, 72, 44, 40, 74, 205, 75,
                    201, 172, 2, 114, 245, 20, 138, 16, 202, 82, 50, 203, 82, 139, 74, 50, 83, 242, 17,
                    162, 104, 180, 34, 0, 168, 108, 192, 3, 85, 0, 0, 0,
                ];
                let cab = format!(
                    "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    GZIP.len()
                );
                let _ = stream.write_all(cab.as_bytes());
                let _ = stream.write_all(GZIP);
                continue;
            }

            let resp: String = if path == "/headers" {
                // Eco del valor de la cabecera X-Token.
                let token = req
                    .lines()
                    .find(|l| l.to_lowercase().starts_with("x-token:"))
                    .map(|l| l[8..].trim().to_string())
                    .unwrap_or_default();
                format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{token}")
            } else if path == "/redirect" {
                "HTTP/1.1 302 Found\r\nLocation: /final\r\nConnection: close\r\n\r\n".to_string()
            } else if path == "/final" {
                "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nllegada".to_string()
            } else if path == "/chunked" {
                // "Wiki"+"pedia "+"in chunks." = "Wikipedia in chunks." (a = 10 hex).
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                 4\r\nWiki\r\n6\r\npedia \r\na\r\nin chunks.\r\n0\r\n\r\n"
                    .to_string()
            } else {
                "HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n".to_string()
            };
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

const DRIVER: &str = r#"
from http import request_with, fetch_follow, fetch, header, body_text, Response;

fn main() -> int {
    // 1. Cabecera personalizada (request_with) → el servidor la hace eco.
    var hdrs: Map<string, string> = Map.new();
    hdrs.insert("X-Token", "secreto42");
    match (request_with("GET", "http://127.0.0.1:__PORT__/headers", "", hdrs)) {
        Result.Ok(r) => print(text(r)),
        Result.Err(e) => print("err: " + e),
    };
    // 2. Redirección 302 → /final, seguida automáticamente.
    match (fetch_follow("http://127.0.0.1:__PORT__/redirect", 5)) {
        Result.Ok(r) => { print(to_string(r.status)); print(text(r)); },
        Result.Err(e) => print("err: " + e),
    };
    // 3. Transfer-Encoding chunked, decodificado.
    match (fetch("http://127.0.0.1:__PORT__/chunked")) {
        Result.Ok(r) => print(text(r)),
        Result.Err(e) => print("err: " + e),
    };
    // 4. Content-Encoding: gzip, descomprimido automáticamente (M20.10b).
    match (fetch("http://127.0.0.1:__PORT__/gzip")) {
        Result.Ok(r) => print(text(r)),
        Result.Err(e) => print("err: " + e),
    };
    0
}

fn text(r: Response) -> string {
    match (body_text(r)) {
        Result.Ok(t) => t,
        Result.Err(e) => "utf8 err: " + e,
    }
}
"#;

fn run_with_http(driver: &str, vm: bool) -> (String, i32) {
    run_with_http_en("httpc", driver, vm)
}

// Cada test usa su propio directorio (`caso`): los tests corren en paralelo y compartirlo
// haría que se pisaran el `main.ray`.
fn run_with_http_en(case: &str, driver: &str, vm: bool) -> (String, i32) {
    let mut dir = std::env::temp_dir();
    dir.push(format!("ray_{case}_{}", if vm { "vm" } else { "interp" }));
    std::fs::create_dir_all(&dir).expect("crea dir");
    for lib in ["http.ray", "inflate.ray"] {
        let src = format!("{}/examples/web/{lib}", env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(&src, dir.join(lib)).unwrap_or_else(|_| panic!("copia {lib}"));
    }
    let driver_path = dir.join("main.ray");
    std::fs::write(&driver_path, driver).expect("escribe driver");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let out = cmd.arg(&driver_path).output().expect("lanza raylang");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn http_robust_headers_redirect_chunked() {
    for vm in [false, true] {
        let port = toy_server();
        let driver = DRIVER.replace("__PORT__", &port.to_string());
        let (out, code) = run_with_http(&driver, vm);
        let expected = "secreto42\n200\nllegada\nWikipedia in chunks.\n\
                        raylang es un lenguaje de aprendizaje. raylang es divertido. raylang raylang raylang!";
        assert_eq!(out.trim(), expected, "http robust (vm={vm}): {out}");
        assert_eq!(code, 0, "output 0 (vm={vm})");
    }
}

// ── Conexión persistente (keep-alive, M90.2) ─────────────────────────────────────────

/// Servidor keep-alive: numera cada CONEXIÓN y cada petición dentro de ella, y sirve varias
/// peticiones por conexión. Rutas: `/quien` → "c<conn> r<req>" (keep-alive); `/chunked` →
/// cuerpo chunked keep-alive; `/cierra` → responde y cierra con `Connection: close`;
/// `/silencio` → responde keep-alive pero cierra el socket SIN avisar (la carrera del
/// keep-alive ocioso: la siguiente petición debe reintentar transparente).
fn toy_server_keepalive() -> u16 {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let conns = Arc::new(AtomicUsize::new(0));
    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut stream = match conn {
                Ok(s) => s,
                Err(_) => continue,
            };
            let cid = conns.fetch_add(1, Ordering::SeqCst) + 1;
            let conns = Arc::clone(&conns);
            let _ = conns; // el contador vive en el hilo aceptador; cada conexión ya tiene su id
            thread::spawn(move || {
                let mut nreq = 0usize;
                loop {
                    // Leer una petición completa (sin cuerpo: GET) hasta "\r\n\r\n".
                    let mut req = Vec::new();
                    let mut byte = [0u8; 1024];
                    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut byte) {
                            Ok(0) | Err(_) => return, // el cliente cerró
                            Ok(n) => req.extend_from_slice(&byte[..n]),
                        }
                    }
                    nreq += 1;
                    let text = String::from_utf8_lossy(&req);
                    let path = text.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("/");
                    match path {
                        "/chunked" => {
                            let _ = stream.write_all(
                                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n\
                                  4\r\nWiki\r\n6\r\npedia \r\na\r\nin chunks.\r\n0\r\n\r\n",
                            );
                        }
                        "/cierra" => {
                            let _ = stream.write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nadios",
                            );
                            return;
                        }
                        "/silencio" => {
                            let _ = stream.write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: keep-alive\r\n\r\nultima",
                            );
                            return; // cierra SIN "Connection: close" (carrera del keep-alive)
                        }
                        _ => {
                            let body = format!("c{cid} r{nreq}");
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        }
                    }
                }
            });
        }
    });
    port
}

const DRIVER_KEEPALIVE: &str = r#"
from http import connect, conn_request, conn_close, Conn, Response, body_text;

fn text(r: Response) -> string {
    match (body_text(r)) {
        Result.Ok(t) => t,
        Result.Err(e) => "utf8 err: " + e,
    }
}

fn asks(c: Conn, path: string) -> string {
    match (conn_request(c, "GET", path, "", Map.new())) {
        Result.Ok(r) => text(r),
        Result.Err(e) => "err: " + e,
    }
}

fn main() -> int {
    match (connect("http://127.0.0.1:__PORT__")) {
        Result.Err(e) => {
            print("err conexión: " + e);
            1
        },
        Result.Ok(c) => {
            print(asks(c, "/quien"));    // c1 r1
            print(asks(c, "/quien"));    // c1 r2 — MISMA conexión (keep-alive real)
            print(asks(c, "/chunked"));  // chunked delimitado sin EOF, misma conexión
            print(asks(c, "/quien"));    // c1 r4 — la delimitación chunked no se comió de más
            print(asks(c, "/cierra"));   // el servidor pide cerrar
            print(asks(c, "/quien"));    // c2 r1 — reconexión tras Connection: close
            print(asks(c, "/silencio")); // c2 r2; el servidor cierra sin avisar
            print(asks(c, "/quien"));    // c3 r1 — reintento transparente sobre conexión fresca
            conn_close(c);
            0
        },
    }
}
"#;

#[test]
fn http_keepalive_reusa_y_reintenta() {
    for vm in [false, true] {
        let port = toy_server_keepalive();
        let driver = DRIVER_KEEPALIVE.replace("__PORT__", &port.to_string());
        let (out, code) = run_with_http_en("httpka", &driver, vm);
        let expected = "c1 r1\nc1 r2\nWikipedia in chunks.\nc1 r4\nadios\nc2 r1\nultima\nc3 r1";
        assert_eq!(out.trim(), expected, "keep-alive (vm={vm}): {out}");
        assert_eq!(code, 0, "output 0 (vm={vm})");
    }
}
