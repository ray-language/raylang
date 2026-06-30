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
            let ruta = req.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("/").to_string();

            // /gzip: cuerpo binario (gzip de "raylang es un lenguaje...") con Content-Encoding: gzip.
            if ruta == "/gzip" {
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

            let resp: String = if ruta == "/headers" {
                // Eco del valor de la cabecera X-Token.
                let token = req
                    .lines()
                    .find(|l| l.to_lowercase().starts_with("x-token:"))
                    .map(|l| l[8..].trim().to_string())
                    .unwrap_or_default();
                format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{token}")
            } else if ruta == "/redirect" {
                "HTTP/1.1 302 Found\r\nLocation: /final\r\nConnection: close\r\n\r\n".to_string()
            } else if ruta == "/final" {
                "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nllegada".to_string()
            } else if ruta == "/chunked" {
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
    var hdrs: Map<string, string> = map_new();
    insert(hdrs, "X-Token", "secreto42");
    match (request_with("GET", "http://127.0.0.1:__PORT__/headers", "", hdrs)) {
        Result.Ok(r) => print(texto(r)),
        Result.Err(e) => print("err: " + e),
    };
    // 2. Redirección 302 → /final, seguida automáticamente.
    match (fetch_follow("http://127.0.0.1:__PORT__/redirect", 5)) {
        Result.Ok(r) => { print(to_string(r.status)); print(texto(r)); },
        Result.Err(e) => print("err: " + e),
    };
    // 3. Transfer-Encoding chunked, decodificado.
    match (fetch("http://127.0.0.1:__PORT__/chunked")) {
        Result.Ok(r) => print(texto(r)),
        Result.Err(e) => print("err: " + e),
    };
    // 4. Content-Encoding: gzip, descomprimido automáticamente (M20.10b).
    match (fetch("http://127.0.0.1:__PORT__/gzip")) {
        Result.Ok(r) => print(texto(r)),
        Result.Err(e) => print("err: " + e),
    };
    0
}

fn texto(r: Response) -> string {
    match (body_text(r)) {
        Result.Ok(t) => t,
        Result.Err(e) => "utf8 err: " + e,
    }
}
"#;

fn run_with_http(driver: &str, vm: bool) -> (String, i32) {
    let mut dir = std::env::temp_dir();
    dir.push(format!("ray_httpc_{}", if vm { "vm" } else { "interp" }));
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
fn http_robusto_headers_redirect_chunked() {
    for vm in [false, true] {
        let port = toy_server();
        let driver = DRIVER.replace("__PORT__", &port.to_string());
        let (out, code) = run_with_http(&driver, vm);
        let esperado = "secreto42\n200\nllegada\nWikipedia in chunks.\n\
                        raylang es un lenguaje de aprendizaje. raylang es divertido. raylang raylang raylang!";
        assert_eq!(out.trim(), esperado, "http robusto (vm={vm}): {out}");
        assert_eq!(code, 0, "salida 0 (vm={vm})");
    }
}
