//! M133 — dogfood del cliente gRPC contra un servidor **REAL** (`google.golang.org/grpc`, la
//! implementación canónica), cerrando IDEAS §72.4: `grpc_client` era la única superficie de red
//! del paquete sin dogfood. El servidor vive como fixture reproducible en
//! `tests/fixtures/grpc_go_server/` (codec crudo, sin protoc) y responde el MISMO servicio
//! `greet.Greeter/Hello` que el toy-server de `grpc_cli.rs` — el mismo demo valida ambos.
//!
//! Lo que este test cazó al estrenarse (arreglado en M133): (1) grpc-go comprime SIEMPRE sus
//! cabeceras con HUFFMAN y el decoder HPACK las rechazaba (la tabla ya vivía en std/huffman:
//! solo faltaba puentearla); (2) un error gRPC llega como respuesta TRAILERS-ONLY (sin DATA) y
//! `grpc_unframe(b"")` lo rompía como "frame too short".
//!
//! Necesita el toolchain de Go (y la caché de módulos, o red para bajarlos) → `#[ignore]`:
//!   cargo test --test grpc_real_cli -- --ignored

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

/// Compila el servidor Go del fixture (una vez por corrida) y lo lanza; devuelve (hijo, puerto).
fn launch_real_server(mode: Option<&str>) -> (Child, u16) {
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = format!("{repo}/tests/fixtures/grpc_go_server");
    let bin = std::env::temp_dir().join("ray_grpc_go_server");
    let build = Command::new("go")
        .args(["build", "-o", bin.to_str().unwrap(), "."])
        .current_dir(&dir)
        .output()
        .expect("lanza go build");
    assert!(
        build.status.success(),
        "go build del servidor real falló: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let cert = format!("{repo}/tests/fixtures/tls_cert.pem");
    let key = format!("{repo}/tests/fixtures/tls_key.pem");
    let mut cmd = Command::new(&bin);
    cmd.arg(&cert).arg(&key);
    if let Some(m) = mode {
        cmd.arg(m);
    }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("lanza el servidor");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut line = String::new();
    reader.read_line(&mut line).expect("lee el puerto");
    let port: u16 = line.trim().parse().unwrap_or_else(|_| panic!("puerto inválido: {line:?}"));
    (child, port)
}

fn run_demo(flags: &[&str], port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/grpc_call_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg("localhost")
        .arg(port.to_string())
        .env("SSL_CERT_FILE", &ca)
        .output()
        .expect("ejecuta grpc_call_demo.ray");
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect()
}

fn go_available() -> bool {
    Command::new("go").arg("version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Llamada unaria OK contra grpc-go real: HEADERS Huffman + DATA + trailers, ambos motores.
#[test]
#[ignore]
fn real_grpc_server_unary_ok() {
    if !go_available() {
        eprintln!("saltando: toolchain de Go no disponible");
        return;
    }
    let (mut child, port) = launch_real_server(None);
    let vm = run_demo(&["--vm"], port);
    assert_eq!(vm, &["grpc-status: 0", "greeting: hola, raylang"], "VM contra grpc-go");
    let _ = child.kill();
    let _ = child.wait();
    // El intérprete, contra una conexión nueva (el servidor real atiende muchas; relanzamos para
    // aislar): misma salida byte a byte.
    let (mut child, port) = launch_real_server(None);
    let interp = run_demo(&["--interp"], port);
    assert_eq!(interp, &["grpc-status: 0", "greeting: hola, raylang"], "intérprete contra grpc-go");
    let _ = child.kill();
    let _ = child.wait();
}

/// Método no registrado contra grpc-go real: UNIMPLEMENTED (12) como respuesta trailers-only.
#[test]
#[ignore]
fn real_grpc_server_unimplemented() {
    if !go_available() {
        eprintln!("saltando: toolchain de Go no disponible");
        return;
    }
    let (mut child, port) = launch_real_server(Some("unimplemented"));
    let lines = run_demo(&["--vm"], port);
    assert_eq!(lines, &["grpc-status: 12", "greeting: "], "trailers-only real → status 12");
    let _ = child.kill();
    let _ = child.wait();
}
