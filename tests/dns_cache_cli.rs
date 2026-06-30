//! Prueba de la caché DNS por TTL (`examples/web/dns_cache.ray`, M22.1). El servidor DNS de juguete
//! **cuenta las consultas** que recibe (con un contador atómico compartido). `dns_cache_demo.ray`
//! resuelve el mismo nombre dos veces + otro una vez (3 resoluciones, 2 claves distintas) → con caché,
//! el servidor debe recibir solo **2** consultas (la repetida se sirve de la caché). Por ambos motores.

use std::net::UdpSocket;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

/// Servidor DNS de juguete que responde A=93.184.216.34 (TTL 300) e incrementa `contador` por consulta.
fn toy_dns_server(contador: Arc<AtomicUsize>) -> u16 {
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind dns");
    let port = sock.local_addr().expect("addr").port();
    thread::spawn(move || {
        let mut buf = [0u8; 1500];
        loop {
            let (n, origen) = match sock.recv_from(&mut buf) {
                Ok(x) => x,
                Err(_) => return,
            };
            contador.fetch_add(1, Ordering::SeqCst);
            let consulta = &buf[..n];
            let pregunta = &consulta[12..n];
            let mut resp: Vec<u8> = Vec::new();
            resp.extend_from_slice(&consulta[0..2]);  // ID
            resp.extend_from_slice(&[0x81, 0x80]);    // flags
            resp.extend_from_slice(&[0, 1]);          // QDCOUNT
            resp.extend_from_slice(&[0, 1]);          // ANCOUNT
            resp.extend_from_slice(&[0, 0, 0, 0]);    // NS, AR
            resp.extend_from_slice(pregunta);         // eco de la pregunta
            resp.extend_from_slice(&[0xC0, 0x0C]);    // NAME = puntero a la pregunta
            resp.extend_from_slice(&[0, 1]);          // TYPE = A
            resp.extend_from_slice(&[0, 1]);          // CLASS = IN
            resp.extend_from_slice(&[0, 0, 1, 0x2c]); // TTL = 300 (vive en caché)
            resp.extend_from_slice(&[0, 4]);          // RDLENGTH
            resp.extend_from_slice(&[93, 184, 216, 34]);
            let _ = sock.send_to(&resp, origen);
        }
    });
    port
}

fn correr(flags: &[&str], port: u16) -> Vec<String> {
    let demo = format!("{}/examples/web/dns_cache_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .arg(port.to_string())
        .output()
        .expect("ejecuta dns_cache_demo.ray");
    assert!(
        out.status.success(),
        "dns_cache_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

const ESPERADO: &[&str] = &[
    "93.184.216.34", // example.com (fallo)
    "93.184.216.34", // example.com (acierto de caché)
    "93.184.216.34", // example.org (fallo)
    "hits=1 misses=2",
];

fn caso(flags: &[&str]) {
    let contador = Arc::new(AtomicUsize::new(0));
    let port = toy_dns_server(contador.clone());
    assert_eq!(correr(flags, port), ESPERADO);
    // 3 resoluciones, 2 claves → el servidor recibe solo 2 consultas (la repetida se cachea).
    assert_eq!(contador.load(Ordering::SeqCst), 2, "el servidor debió recibir 2 consultas");
}

#[test]
fn cache_evita_la_segunda_consulta_interprete() {
    caso(&[]);
}

#[test]
fn cache_evita_la_segunda_consulta_vm() {
    caso(&["--vm"]);
}
