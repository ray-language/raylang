//! M32.1b — cliente PostgreSQL (protocolo wire v3 + auth SCRAM-SHA-256). El cliente de raylang
//! (`postgres.ray`) hace startup + handshake SCRAM + una consulta simple contra un **servidor
//! PostgreSQL de juguete escrito a mano** (solo std, TCP plano) que reproduce el intercambio con
//! valores SCRAM PRECOMPUTADOS (nonce/salt fijos, i=64 para no ralentizar la suite → sin cripto en Rust) y responde una fila fija. El
//! cliente **verifica la firma del servidor** (cripto en raylang) y devuelve la fila; ambos motores.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;

// server-first y server-final PRECOMPUTADOS (python) para user=raylang, pw=secret, nonce fijo, i=64.
const SERVER_FIRST: &[u8] = &[114, 61, 99, 108, 105, 101, 110, 116, 110, 111, 110, 99, 101, 49, 50, 51, 52, 53, 54, 115, 101, 114, 118, 101, 114, 110, 111, 110, 99, 101, 55, 56, 57, 44, 115, 61, 65, 81, 73, 68, 66, 65, 85, 71, 66, 119, 103, 74, 67, 103, 115, 77, 68, 81, 52, 80, 69, 65, 61, 61, 44, 105, 61, 54, 52];
const SERVER_FINAL: &[u8] = &[118, 61, 80, 87, 88, 53, 71, 72, 119, 87, 43, 66, 120, 108, 101, 99, 70, 122, 79, 56, 116, 105, 79, 81, 69, 86, 111, 98, 86, 117, 43, 54, 80, 66, 103, 56, 55, 49, 110, 109, 121, 101, 81, 51, 52, 61];

/// Un mensaje con octeto de tipo: `[tipo][longitud=4+len][carga]`.
fn msg(t: u8, payload: &[u8]) -> Vec<u8> {
    let n = 4 + payload.len();
    let mut m = vec![t, (n >> 24) as u8, (n >> 16) as u8, (n >> 8) as u8, n as u8];
    m.extend_from_slice(payload);
    m
}

fn read_exact(s: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    s.read_exact(&mut b).expect("read");
    b
}

/// Lee un StartupMessage (sin tipo): `[longitud:4][resto]`.
fn read_startup(s: &mut TcpStream) {
    let hdr = read_exact(s, 4);
    let len = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
    let _ = read_exact(s, len - 4);
}

/// Lee un mensaje con tipo: `[tipo:1][longitud:4][carga]`.
fn read_msg(s: &mut TcpStream) {
    let mut t = [0u8; 5];
    s.read_exact(&mut t).expect("read type+len");
    let len = u32::from_be_bytes([t[1], t[2], t[3], t[4]]) as usize;
    let _ = read_exact(s, len - 4);
}

fn launch_servidor_pg() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            read_startup(&mut s);                              // StartupMessage
            // AuthenticationSASL: code 10 + "SCRAM-SHA-256\0" + "\0".
            let mut sasl = vec![0u8, 0, 0, 10];
            sasl.extend_from_slice(b"SCRAM-SHA-256\0");
            sasl.push(0);
            s.write_all(&msg(b'R', &sasl)).unwrap();
            read_msg(&mut s);                                  // SASLInitialResponse ('p')
            // AuthenticationSASLContinue: code 11 + server-first.
            let mut cont = vec![0u8, 0, 0, 11];
            cont.extend_from_slice(SERVER_FIRST);
            s.write_all(&msg(b'R', &cont)).unwrap();
            read_msg(&mut s);                                  // SASLResponse ('p')
            // AuthenticationSASLFinal: code 12 + server-final.
            let mut fin = vec![0u8, 0, 0, 12];
            fin.extend_from_slice(SERVER_FINAL);
            s.write_all(&msg(b'R', &fin)).unwrap();
            // AuthenticationOk (code 0) + ReadyForQuery ('I').
            s.write_all(&msg(b'R', &[0, 0, 0, 0])).unwrap();
            s.write_all(&msg(b'Z', b"I")).unwrap();
            read_msg(&mut s);                                  // Query ('Q')
            // RowDescription ('T'): 1 columna "greeting" (metadatos a cero, 18 octetos).
            let mut td = vec![0u8, 1];
            td.extend_from_slice(b"greeting\0");
            td.extend_from_slice(&[0u8; 18]);
            s.write_all(&msg(b'T', &td)).unwrap();
            // DataRow ('D'): 1 columna, valor "hello-postgres" (14 octetos).
            let mut dr = vec![0u8, 1, 0, 0, 0, 14];
            dr.extend_from_slice(b"hello-postgres");
            s.write_all(&msg(b'D', &dr)).unwrap();
            s.write_all(&msg(b'C', b"SELECT 1\0")).unwrap();   // CommandComplete
            s.write_all(&msg(b'Z', b"I")).unwrap();            // ReadyForQuery
            let _ = s.flush();
        }
    });
    port
}

fn run(flags: &[&str], port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/postgres_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg("127.0.0.1")
        .arg(port.to_string())
        .output()
        .expect("ejecuta postgres_demo.ray");
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect()
}

const ESPERADO: &[&str] = &["row: hello-postgres"];

#[test]
fn pg_query_vm() {
    let port = launch_servidor_pg();
    assert_eq!(run(&["--vm"], port), ESPERADO);
}

#[test]
fn pg_query_interpreter() {
    let port = launch_servidor_pg();
    assert_eq!(run(&[], port), ESPERADO);
}
