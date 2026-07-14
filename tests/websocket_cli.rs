//! Pruebas de las librerías criptográficas de M19.3b (`examples/web/sha1.ray`, `examples/web/base64.ray`),
//! la base del handshake de WebSocket. Son **cómputo puro determinista** → no hace falta red ni
//! oráculo cruzado a mano: se corre el driver `examples/web/crypto_demo.ray` por ambos motores
//! (intérprete y VM) y se compara su salida con los **vectores de referencia** conocidos (RFC 3174
//! para SHA-1, RFC 4648 para base64, RFC 6455 §1.3 para el accept de WebSocket).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Las líneas que `crypto_demo.ray` debe imprimir, en orden. Cada una es un vector estándar.
const ESPERADO: &[&str] = &[
    // SHA-1 (hex)
    "da39a3ee5e6b4b0d3255bfef95601890afd80709", // ""
    "a9993e364706816aba3e25717850c26c9cd0d89d", // "abc"
    "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12", // "The quick brown fox jumps over the lazy dog"
    "c2db330f6083854c99d4b5bfb6e8f29f201be699", // 56 × 'a' (mensaje multi-bloque)
    // base64
    "TWFu",                        // "Man"
    "TWE=",                        // "Ma"
    "TQ==",                        // "M"
    "YW55IGNhcm5hbCBwbGVhc3VyZS4=", // "any carnal pleasure."
    // Handshake de WebSocket: base64(SHA-1(key + GUID)) — el ejemplo canónico del RFC 6455.
    "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
];

/// Corre `examples/web/crypto_demo.ray` con los flags dados y devuelve sus líneas de stdout.
fn run(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/crypto_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta crypto_demo.ray");
    assert!(
        out.status.success(),
        "crypto_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn vectors_sha1_base64_handshake_interpreter() {
    assert_eq!(run(&[]), ESPERADO);
}

#[test]
fn vectors_sha1_base64_handshake_vm() {
    assert_eq!(run(&["--vm"]), ESPERADO);
}

// --- Echo server de WebSocket de extremo a extremo (M19.3c) ---
//
// `examples/web/websocket_echo.ray` es un servidor real (handshake + framing). La concurrencia/red son
// **solo VM** y no deterministas para el oráculo → subproceso + un cliente WebSocket escrito aquí en
// Rust que hace el handshake y un intercambio de tramas, igual que `webserver_cli.rs`.

/// Lanza el echo server (`--vm`, desde `examples/` para que resuelva sus imports) y devuelve el
/// proceso + el puerto efímero que imprime en su primera línea.
fn launch_echo() -> (Child, u16) {
    let demo = format!("{}/examples/web/websocket_echo.ray", env!("CARGO_MANIFEST_DIR"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&demo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza el echo server");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee el port");
    let port: u16 = linea.trim().parse().unwrap_or_else(|_| panic!("invalid port: {linea:?}"));
    (child, port)
}

/// Construye una trama de cliente (texto, FIN) **enmascarada**, como exige el RFC 6455 §5.3.
fn trama_client(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [0x01u8, 0x02, 0x03, 0x04];
    let mut f = vec![0x80 | opcode, 0x80 | (payload.len() as u8)]; // asume len < 126
    f.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        f.push(b ^ mask[i % 4]);
    }
    f
}

/// Lee una trama del servidor (sin máscara; asume payload < 126) y devuelve (opcode, payload).
fn leer_trama(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr).expect("lee header de trama");
    let opcode = hdr[0] & 0x0f;
    let len = (hdr[1] & 0x7f) as usize;
    let mut payload = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut payload).expect("lee carga de trama");
    }
    (opcode, payload)
}

#[test]
fn echo_server_handshake_y_tramas() {
    let (mut child, port) = launch_echo();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

    // 1) Handshake: enviar el upgrade con una clave conocida y verificar el accept canónico.
    let req = "GET /chat HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
               Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    stream.write_all(req.as_bytes()).expect("envía upgrade");
    // Leer la respuesta de upgrade (termina en la línea en blanco \r\n\r\n).
    let mut resp = Vec::new();
    let mut buf = [0u8; 1];
    while !resp.ends_with(b"\r\n\r\n") {
        let n = stream.read(&mut buf).expect("lee response");
        assert!(n > 0, "el servidor cerró durante el handshake");
        resp.push(buf[0]);
    }
    let resp = String::from_utf8_lossy(&resp);
    assert!(resp.contains("101 Switching Protocols"), "esperaba 101, got: {resp}");
    assert!(
        resp.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        "accept incorrect, got: {resp}"
    );

    // 2) Enviar una trama de texto y verificar que vuelve idéntica (eco).
    stream.write_all(&trama_client(0x1, b"hello raylang")).expect("envía trama");
    let (opcode, payload) = leer_trama(&mut stream);
    assert_eq!(opcode, 0x1, "esperaba one trama de text");
    assert_eq!(payload, b"hello raylang", "el echo no coincide");

    // 3) Una segunda trama, para confirmar el bucle.
    stream.write_all(&trama_client(0x1, b"other")).expect("envía 2ª trama");
    let (_, payload2) = leer_trama(&mut stream);
    assert_eq!(payload2, b"other");

    // 4) Cerrar: enviar un close y esperar el close de cortesía del servidor.
    stream.write_all(&trama_client(0x8, b"")).expect("envía close");
    let (op_cierre, _) = leer_trama(&mut stream);
    assert_eq!(op_cierre, 0x8, "esperaba un close del servidor");

    let _ = child.kill();
    let _ = child.wait();
}

/// Trama de cliente enmascarada con el bit FIN a elección (para fragmentación, M58.1).
fn trama_client_fin(opcode: u8, payload: &[u8], fin: bool) -> Vec<u8> {
    let mask = [0x0Au8, 0x0B, 0x0C, 0x0D];
    let b0 = if fin { 0x80 | opcode } else { opcode };
    let mut f = vec![b0, 0x80 | (payload.len() as u8)]; // asume len < 126
    f.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        f.push(b ^ mask[i % 4]);
    }
    f
}

#[test]
fn echo_server_robust_ante_framing_real() {
    // M58.1: el lector bufferizado del servidor debe sobrevivir a lo que TCP hace de verdad:
    // tramas PARTIDAS entre lecturas, tramas PEGADAS en una lectura, pings intercalados y
    // mensajes FRAGMENTADOS. Antes: partida = OOB (moría la fibra), pegadas = la 2ª se descartaba.
    let (mut child, port) = launch_echo();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

    // Handshake mínimo.
    let req = "GET / HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
               Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    stream.write_all(req.as_bytes()).expect("envía upgrade");
    let mut resp = Vec::new();
    let mut b1 = [0u8; 1];
    while !resp.ends_with(b"\r\n\r\n") {
        let n = stream.read(&mut b1).expect("lee handshake");
        assert!(n > 0, "cerró en el handshake");
        resp.push(b1[0]);
    }

    // 1) Trama PARTIDA en dos escrituras (con pausa para forzar dos lecturas del servidor).
    let f = trama_client(0x1, b"partida");
    stream.write_all(&f[..3]).expect("half 1");
    stream.flush().ok();
    std::thread::sleep(Duration::from_millis(120));
    stream.write_all(&f[3..]).expect("half 2");
    let (op, payload) = leer_trama(&mut stream);
    assert_eq!((op, payload.as_slice()), (0x1, b"partida".as_slice()), "trama partida");

    // 2) Dos tramas PEGADAS en una sola escritura → dos ecos, en orden.
    let mut dos = trama_client(0x1, b"one");
    dos.extend_from_slice(&trama_client(0x1, b"dos"));
    stream.write_all(&dos).expect("pegadas");
    let (_, p1) = leer_trama(&mut stream);
    let (_, p2) = leer_trama(&mut stream);
    assert_eq!(p1, b"one", "1ª pegada");
    assert_eq!(p2, b"dos", "2ª pegada (antes se descartaba)");

    // 3) PING → el servidor contesta PONG con la misma carga (antes lo ecoaba como texto).
    stream.write_all(&trama_client(0x9, b"live")).expect("ping");
    let (op_pong, p_pong) = leer_trama(&mut stream);
    assert_eq!((op_pong, p_pong.as_slice()), (0xA, b"live".as_slice()), "pong automático");

    // 4) Mensaje FRAGMENTADO (texto FIN=0 + continuación FIN=1) → un solo eco reensamblado.
    stream.write_all(&trama_client_fin(0x1, b"frag-", false)).expect("fragmento 1");
    stream.write_all(&trama_client_fin(0x0, b"mento", true)).expect("fragmento 2");
    let (op_fr, p_fr) = leer_trama(&mut stream);
    assert_eq!((op_fr, p_fr.as_slice()), (0x1, b"frag-mento".as_slice()), "reensamblado");

    // 5) Close → close de cortesía.
    stream.write_all(&trama_client(0x8, b"")).expect("close");
    let (op_cl, _) = leer_trama(&mut stream);
    assert_eq!(op_cl, 0x8, "close de cortesía");

    let _ = child.kill();
    let _ = child.wait();
}
