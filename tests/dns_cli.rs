//! Prueba del cliente DNS (`examples/web/dns.ray`, M22). DNS sobre UDP → se levanta un **servidor DNS
//! de juguete** en Rust que responde a una consulta A con una IP fija, usando un **puntero de
//! compresión** (0xC00C) en el nombre de la respuesta (para ejercitar el parser de nombres del
//! cliente). Se corre `dns_demo.ray` contra él por ambos motores (intérprete UDP bloqueante, VM no
//! bloqueante) y se comprueba la IP resuelta.

use std::net::UdpSocket;
use std::process::Command;
use std::thread;

/// Servidor DNS de juguete: responde según el QTYPE (A/AAAA/MX), con el nombre de la respuesta como
/// puntero a la pregunta (offset 12). El MX usa además un puntero de compresión en su exchange
/// ("mail" + puntero a "example.com"). Devuelve el puerto.
fn toy_dns_server() -> u16 {
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind dns");
    let port = sock.local_addr().expect("addr").port();
    thread::spawn(move || {
        let mut buf = [0u8; 1500];
        loop {
            let (n, origen) = match sock.recv_from(&mut buf) {
                Ok(x) => x,
                Err(_) => return,
            };
            let consulta = &buf[..n];
            // QTYPE = los 2 octetos antes del QCLASS final (pregunta = QNAME + QTYPE(2) + QCLASS(2)).
            let qtype = ((consulta[n - 4] as u16) << 8) | consulta[n - 3] as u16;
            // La pregunta va de offset 12 al final (un solo QNAME + QTYPE + QCLASS).
            let pregunta = &consulta[12..n];

            let mut resp: Vec<u8> = Vec::new();
            resp.extend_from_slice(&consulta[0..2]); // ID (eco)
            resp.extend_from_slice(&[0x81, 0x80]);   // flags: QR=1, RD=1, RA=1, RCODE=0
            resp.extend_from_slice(&[0, 1]);         // QDCOUNT = 1
            resp.extend_from_slice(&[0, 1]);         // ANCOUNT = 1
            resp.extend_from_slice(&[0, 0, 0, 0]);   // NSCOUNT, ARCOUNT
            resp.extend_from_slice(pregunta);        // eco de la pregunta
            // NAME de la respuesta = puntero al offset 12 (la pregunta), común a todos los tipos.
            resp.extend_from_slice(&[0xC0, 0x0C]);
            match qtype {
                28 => {
                    // AAAA: 2001:db8::1
                    resp.extend_from_slice(&[0, 28]);          // TYPE = AAAA
                    resp.extend_from_slice(&[0, 1]);           // CLASS = IN
                    resp.extend_from_slice(&[0, 0, 1, 0x2c]);  // TTL
                    resp.extend_from_slice(&[0, 16]);          // RDLENGTH = 16
                    resp.extend_from_slice(&[
                        0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01,
                    ]);
                }
                15 => {
                    // MX: preferencia 10, exchange "mail" + puntero a "example.com" (offset 12).
                    resp.extend_from_slice(&[0, 15]);          // TYPE = MX
                    resp.extend_from_slice(&[0, 1]);           // CLASS = IN
                    resp.extend_from_slice(&[0, 0, 1, 0x2c]);  // TTL
                    resp.extend_from_slice(&[0, 9]);           // RDLENGTH = 2 + 5 + 2
                    resp.extend_from_slice(&[0, 10]);          // preferencia = 10
                    resp.extend_from_slice(&[4, b'm', b'a', b'i', b'l']); // label "mail"
                    resp.extend_from_slice(&[0xC0, 0x0C]);     // puntero a "example.com"
                }
                5 => {
                    // CNAME: "cdn" + puntero a "example.com" → "cdn.example.com".
                    resp.extend_from_slice(&[0, 5]);           // TYPE = CNAME
                    resp.extend_from_slice(&[0, 1]);           // CLASS = IN
                    resp.extend_from_slice(&[0, 0, 1, 0x2c]);  // TTL
                    resp.extend_from_slice(&[0, 6]);           // RDLENGTH = 4 + 2
                    resp.extend_from_slice(&[3, b'c', b'd', b'n']); // label "cdn"
                    resp.extend_from_slice(&[0xC0, 0x0C]);     // puntero a "example.com"
                }
                16 => {
                    // TXT: una character-string "v=spf1 -all".
                    let txt = b"v=spf1 -all";
                    resp.extend_from_slice(&[0, 16]);          // TYPE = TXT
                    resp.extend_from_slice(&[0, 1]);           // CLASS = IN
                    resp.extend_from_slice(&[0, 0, 1, 0x2c]);  // TTL
                    resp.extend_from_slice(&[0, (txt.len() as u8) + 1]); // RDLENGTH = 1 + len
                    resp.push(txt.len() as u8);                // longitud de la character-string
                    resp.extend_from_slice(txt);
                }
                _ => {
                    // A: 93.184.216.34
                    resp.extend_from_slice(&[0, 1]);           // TYPE = A
                    resp.extend_from_slice(&[0, 1]);           // CLASS = IN
                    resp.extend_from_slice(&[0, 0, 1, 0x2c]);  // TTL
                    resp.extend_from_slice(&[0, 4]);           // RDLENGTH = 4
                    resp.extend_from_slice(&[93, 184, 216, 34]);
                }
            }
            let _ = sock.send_to(&resp, origen);
        }
    });
    port
}

fn correr(flags: &[&str], port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/dns_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg(port.to_string())
        .output()
        .expect("ejecuta dns_demo.ray");
    assert!(
        out.status.success(),
        "dns_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

const ESPERADO: &[&str] = &[
    "A 93.184.216.34",        // registro A (IPv4)
    "AAAA 2001:db8::1",       // registro AAAA (IPv6, con compresión ::)
    "MX 10 mail.example.com", // registro MX (preferencia + exchange con compresión de nombre)
    "CNAME cdn.example.com",  // registro CNAME (nombre con compresión)
    "TXT v=spf1 -all",        // registro TXT (character-string)
];

#[test]
fn dns_resuelve_todos_los_tipos_interprete() {
    let port = toy_dns_server();
    assert_eq!(correr(&[], port), ESPERADO);
}

#[test]
fn dns_resuelve_todos_los_tipos_vm() {
    let port = toy_dns_server();
    assert_eq!(correr(&["--vm"], port), ESPERADO);
}
