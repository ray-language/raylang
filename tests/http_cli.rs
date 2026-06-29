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
    for lib in ["http.ray", "json.ray"] {
        let src = format!("{}/examples/{lib}", env!("CARGO_MANIFEST_DIR"));
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
                Result.Ok(texto) => {
                    match (parse(texto)) {
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
fn cliente_http_compone_con_json() {
    for vm in [false, true] {
        let port = toy_http_server();
        let driver = DRIVER.replace("__PORT__", &port.to_string());
        let (out, code) = run_with_libs("compose", &driver, vm);
        let esperado = "200\napplication/json\n{\"n\":42,\"ok\":true}";
        assert_eq!(out.trim(), esperado, "http+json (vm={vm}): {out}");
        assert_eq!(code, 0, "salida 0 (vm={vm})");
    }
}

#[test]
fn fetch_falla_con_url_no_http() {
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
        assert_eq!(out.trim(), "solo se soporta http:// o https://", "rechaza no-http (vm={vm}): {out}");
        assert_eq!(code, 1);
    }
}
