//! Prueba del cliente Redis (`examples/web/redis.ray`, M20.6). RESP sobre TCP → se levanta un
//! **servidor RESP de juguete** en Rust (un subconjunto: PING/SET/GET/INCR/RPUSH/LRANGE/DEL con estado
//! en memoria) y se corre `examples/web/redis_demo.ray` contra él por ambos motores (el cliente del
//! intérprete usa sockets bloqueantes; el de la VM, no bloqueantes). Se compara el stdout del cliente.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;

/// Lee exactamente un comando RESP (array de bulk strings) del stream. `None` si la conexión se cierra.
fn leer_comando(buf: &mut Vec<u8>, stream: &mut TcpStream) -> Option<Vec<String>> {
    // Asegura que hay al menos una línea completa disponible; rellena leyendo del socket.
    fn leer_linea(buf: &mut Vec<u8>, stream: &mut TcpStream) -> Option<String> {
        loop {
            if let Some(pos) = buf.windows(2).position(|w| w == b"\r\n") {
                let linea = String::from_utf8_lossy(&buf[..pos]).into_owned();
                buf.drain(..pos + 2);
                return Some(linea);
            }
            let mut tmp = [0u8; 1024];
            match stream.read(&mut tmp) {
                Ok(0) | Err(_) => return None,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        }
    }
    fn leer_n(buf: &mut Vec<u8>, stream: &mut TcpStream, n: usize) -> Option<String> {
        while buf.len() < n + 2 {
            let mut tmp = [0u8; 1024];
            match stream.read(&mut tmp) {
                Ok(0) | Err(_) => return None,
                Ok(k) => buf.extend_from_slice(&tmp[..k]),
            }
        }
        let s = String::from_utf8_lossy(&buf[..n]).into_owned();
        buf.drain(..n + 2); // el dato + su \r\n
        Some(s)
    }

    let cabecera = leer_linea(buf, stream)?;
    if !cabecera.starts_with('*') {
        return None;
    }
    let n: usize = cabecera[1..].parse().ok()?;
    let mut args = Vec::with_capacity(n);
    for _ in 0..n {
        let largo_linea = leer_linea(buf, stream)?; // $<len>
        let len: usize = largo_linea[1..].parse().ok()?;
        args.push(leer_n(buf, stream, len)?);
    }
    Some(args)
}

/// Atiende una conexión: un mini-Redis en memoria. Soporta lo que la demo ejercita.
fn atender(mut stream: TcpStream) {
    let mut kv: HashMap<String, String> = HashMap::new();
    let mut listas: HashMap<String, Vec<String>> = HashMap::new();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(args) = leer_comando(&mut buf, &mut stream) {
        let cmd = args[0].to_uppercase();
        let resp = match cmd.as_str() {
            "PING" => "+PONG\r\n".to_string(),
            "SET" => {
                kv.insert(args[1].clone(), args[2].clone());
                "+OK\r\n".to_string()
            }
            "GET" => match kv.get(&args[1]) {
                Some(v) => format!("${}\r\n{}\r\n", v.len(), v),
                None => "$-1\r\n".to_string(),
            },
            "INCR" => {
                let n = kv.get(&args[1]).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0) + 1;
                kv.insert(args[1].clone(), n.to_string());
                format!(":{}\r\n", n)
            }
            "RPUSH" => {
                let l = listas.entry(args[1].clone()).or_default();
                for v in &args[2..] {
                    l.push(v.clone());
                }
                format!(":{}\r\n", l.len())
            }
            "LRANGE" => {
                let vacia = Vec::new();
                let l = listas.get(&args[1]).unwrap_or(&vacia);
                let mut s = format!("*{}\r\n", l.len());
                for v in l {
                    s.push_str(&format!("${}\r\n{}\r\n", v.len(), v));
                }
                s
            }
            "DEL" => {
                let habia = kv.remove(&args[1]).is_some() || listas.remove(&args[1]).is_some();
                format!(":{}\r\n", if habia { 1 } else { 0 })
            }
            _ => "-ERR comando no soportado\r\n".to_string(),
        };
        if stream.write_all(resp.as_bytes()).is_err() {
            return;
        }
    }
}

const ESPERADO: &[&str] = &[
    "PONG", "OK", "valor", "(nil)", "1", "2", "3", "[a, b, c]", "1",
    // M69 — round-trip multibyte: el framing RESP cuenta octetos (pre-M69 contaba caracteres:
    // "añejo ñandú" declaraba $12 en vez de $14 y desincronizaba el protocolo).
    "OK", "añejo ñandú",
];

fn correr(flags: &[&str]) -> Vec<String> {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    // Atiende una conexión en un hilo (la demo abre una sola).
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            atender(stream);
        }
    });

    let demo = format!("{}/examples/web/redis_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg(port.to_string())
        .output()
        .expect("ejecuta redis_demo.ray");
    assert!(
        out.status.success(),
        "redis_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn redis_resp_interprete() {
    assert_eq!(correr(&[]), ESPERADO);
}

#[test]
fn redis_resp_vm() {
    assert_eq!(correr(&["--vm"]), ESPERADO);
}
