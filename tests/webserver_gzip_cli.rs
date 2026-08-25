//! M129 — negociación de `Accept-Encoding` en el webserver de PRODUCCIÓN (`packages/net`):
//! `webserver.gzip(req, resp)` comprime el cuerpo si el cliente acepta gzip, la respuesta no es
//! streaming, no trae ya `Content-Encoding` y supera el umbral. Se monta un proyecto temporal
//! (path-dep al paquete net), el handler envuelve sus respuestas con `gzip`, y un cliente HTTP en
//! Rust verifica: cuerpo gzip REAL (lo descomprime `std/inflate` vía un programa raylang aparte —
//! sin dependencia nueva de test), identity sin la cabecera o con `q=0`, umbral y respeto de un
//! `Content-Encoding` previo.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

/// Crea el proyecto temporal (ray.toml → packages/net) y lanza `ray run`; devuelve el hijo y el
/// puerto anunciado por stdout (mismo patrón que webserver_remote_cli).
fn launch(name: &str, main: &str) -> (Child, u16) {
    let base = std::env::temp_dir().join(format!("ray_gzip_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).expect("crea el dir temporal");
    let repo = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"gz\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
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

/// Envía una petición cruda y devuelve (cabeceras, cuerpo).
fn ask(port: u16, req: &str) -> (String, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(req.as_bytes()).expect("escribe");
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).expect("lee");
    let sep = b"\r\n\r\n";
    let i = resp.windows(4).position(|w| w == sep).expect("separador de cabeceras");
    (String::from_utf8_lossy(&resp[..i]).into_owned(), resp[i + 4..].to_vec())
}

/// Descomprime `data` con `std/inflate` (raylang): escribe el gzip a un archivo, corre un
/// programa que lo lee, lo des-gzipea e imprime el texto plano. La verificación del cuerpo es
/// con NUESTRO gunzip (ya validado contra referencia en inflate_cli) — cero deps de test nuevas.
fn gunzip_via_raylang(data: &[u8]) -> String {
    let dir = std::env::temp_dir().join("ray_gzip_tool");
    std::fs::create_dir_all(&dir).unwrap();
    let gz = dir.join("body.gz");
    std::fs::write(&gz, data).unwrap();
    let prog = dir.join("gunzip.ray");
    std::fs::write(
        &prog,
        r#"
import std/fs;
import std/inflate;
fn main() -> int {
    let path = args()[0];
    let h = match (fs.open(path, "r")) {
        Result.Ok(x) => x,
        Result.Err(e) => { eprint(e); return 1; },
    };
    var data = b"";
    var going = true;
    while (going) {
        match (fs.read_bytes(h, 65536)) {
            Result.Ok(chunk) => {
                match (chunk) {
                    Option.Some(b) => { data = data + b; },
                    Option.None => { going = false; },
                }
            },
            Result.Err(e) => { eprint(e); return 1; },
        }
    }
    match (inflate.gunzip(data)) {
        Result.Ok(plain) => {
            match (from_utf8(plain)) {
                Result.Ok(s) => { print(s); 0 },
                Result.Err(e) => { eprint(e); 1 },
            }
        },
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&prog)
        .arg(&gz)
        .output()
        .expect("corre gunzip.ray");
    assert!(out.status.success(), "gunzip.ray falló: {}", String::from_utf8_lossy(&out.stderr));
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    // `print` añade '\n'; el cuerpo original no lo lleva.
    if s.ends_with('\n') {
        s.pop();
    }
    s
}

const SERVER: &str = r#"
import net/webserver;

// Un cuerpo repetitivo y comprimible, por encima del umbral (200 × 56 = 11200 octetos).
fn big_body() -> string {
    var s = "";
    var i = 0;
    while (i < 200) {
        s = s + "the quick brown fox jumps over the lazy dog 0123456789 ";
        i = i + 1;
    }
    s
}

fn handle(req: webserver.Request) -> webserver.Response {
    if (req.path == "/big") {
        return webserver.gzip(req, webserver.ok(big_body()));
    }
    if (req.path == "/small") {
        return webserver.gzip(req, webserver.ok("tiny"));
    }
    if (req.path == "/pre") {
        var r = webserver.ok(big_body());
        r.headers.insert("Content-Encoding", "identity");
        return webserver.gzip(req, r);
    }
    webserver.not_found()
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

fn expected_big() -> String {
    "the quick brown fox jumps over the lazy dog 0123456789 ".repeat(200)
}

#[test]
fn gzip_negotiates_and_roundtrips() {
    let (mut child, port) = launch("basic", SERVER);

    // 1) El cliente acepta gzip → cuerpo gzip real, más corto, con las cabeceras de la spec.
    let (h, b) = ask(
        port,
        "GET /big HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip, deflate\r\nConnection: close\r\n\r\n",
    );
    assert!(h.to_lowercase().contains("content-encoding: gzip"), "Content-Encoding: gzip\n{h}");
    assert!(h.to_lowercase().contains("vary: accept-encoding"), "Vary: Accept-Encoding\n{h}");
    assert!(b.len() >= 2 && b[0] == 0x1f && b[1] == 0x8b, "magic gzip");
    assert!(b.len() < 11200, "comprimido más corto que el original ({})", b.len());
    assert_eq!(gunzip_via_raylang(&b), expected_big(), "el gunzip devuelve el cuerpo original");

    // 2) Sin Accept-Encoding → identity intacto.
    let (h, b) = ask(port, "GET /big HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(!h.to_lowercase().contains("content-encoding"), "sin la cabecera no se comprime\n{h}");
    assert_eq!(String::from_utf8_lossy(&b), expected_big());

    // 3) `gzip;q=0` = el cliente lo PROHÍBE → identity.
    let (h, _b) = ask(
        port,
        "GET /big HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip;q=0\r\nConnection: close\r\n\r\n",
    );
    assert!(!h.to_lowercase().contains("content-encoding"), "q=0 desactiva gzip\n{h}");

    // 4) Bajo el umbral → identity (el gzip de 4 octetos no compensa).
    let (h, b) = ask(
        port,
        "GET /small HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
    );
    assert!(!h.to_lowercase().contains("content-encoding"), "umbral\n{h}");
    assert_eq!(String::from_utf8_lossy(&b), "tiny");

    // 5) Un Content-Encoding previo se respeta (no se re-comprime).
    let (h, _b) = ask(
        port,
        "GET /pre HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
    );
    assert!(h.to_lowercase().contains("content-encoding: identity"), "se respeta el previo\n{h}");
    assert!(!h.to_lowercase().contains("content-encoding: gzip"), "no re-comprime\n{h}");

    let _ = child.kill();
    let _ = child.wait();
}
