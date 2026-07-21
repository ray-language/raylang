//! Prueba del cliente SNTP v4 (`packages/net/ntp.ray`, M90.7). NTP va sobre UDP → se levanta un
//! **servidor SNTP de juguete** en Rust que valida la petición (48 octetos, VN=4 modo=3) y responde
//! con una hora FIJA conocida (determinista) — el driver imprime la hora del servidor y el stratum,
//! que no dependen del reloj local (offset/delay sí, y por eso no se comprueban). Ambos motores.

use std::net::UdpSocket;
use std::process::Command;
use std::thread;

/// La hora fija que "tiene" el servidor de juguete: 2026-01-01 00:00:00.5 UTC.
const UNIX_SECS: u64 = 1_767_225_600;
const NTP_EPOCH_DELTA: u64 = 2_208_988_800;
const FRAC_MEDIO_SEGUNDO: u32 = 0x8000_0000; // 0.5 en punto fijo .32

/// Servidor SNTP de juguete: responde a cada petición válida con stratum 2 y Receive/Transmit
/// Timestamp fijos (la hora de arriba). Devuelve el puerto.
fn toy_sntp_server() -> u16 {
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind sntp");
    let port = sock.local_addr().expect("addr").port();
    thread::spawn(move || {
        let mut buf = [0u8; 128];
        loop {
            let (n, origen) = match sock.recv_from(&mut buf) {
                Ok(x) => x,
                Err(_) => return,
            };
            // Petición SNTP: 48 octetos, LI=0 VN=4 Modo=3 → primer octeto 0x23.
            if n != 48 || buf[0] != 0x23 {
                continue; // no es nuestra petición; ignorar
            }
            let mut resp = [0u8; 48];
            resp[0] = 0x24; // LI=0, VN=4, Modo=4 (server)
            resp[1] = 2; // stratum 2
            let ntp_secs = (UNIX_SECS + NTP_EPOCH_DELTA) as u32;
            // Receive (32..40) y Transmit (40..48) Timestamp = la hora fija.
            for base in [32usize, 40usize] {
                resp[base..base + 4].copy_from_slice(&ntp_secs.to_be_bytes());
                resp[base + 4..base + 8].copy_from_slice(&FRAC_MEDIO_SEGUNDO.to_be_bytes());
            }
            let _ = sock.send_to(&resp, origen);
        }
    });
    port
}

const DRIVER: &str = r#"
from net/ntp import query, NtpResult;

fn main() -> int {
    match (query("127.0.0.1", __PORT__)) {
        Result.Ok(r) => {
            print(to_string(r.unix_millis));
            print(to_string(r.stratum));
            0
        },
        Result.Err(e) => {
            eprint(e);
            1
        },
    }
}
"#;

/// Copia `net/ntp.ray` y `net/udp.ray` bajo `<tmp>/net/` (rutas de módulo desde la raíz del
/// driver), escribe el driver y lo corre en el motor dado.
fn run_ntp(driver: &str, vm: bool) -> (String, String, i32) {
    let mut dir = std::env::temp_dir();
    dir.push(format!("ray_ntp_{}", if vm { "vm" } else { "interp" }));
    let net = dir.join("net");
    std::fs::create_dir_all(&net).expect("crea dir");
    for lib in ["ntp.ray", "udp.ray"] {
        let src = format!("{}/packages/net/{lib}", env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(&src, net.join(lib)).unwrap_or_else(|_| panic!("copia {lib}"));
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
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn sntp_time_matches_server() {
    for vm in [false, true] {
        let port = toy_sntp_server();
        let driver = DRIVER.replace("__PORT__", &port.to_string());
        let (out, err, code) = run_ntp(&driver, vm);
        assert_eq!(code, 0, "output 0 (vm={vm})\n{err}");
        // t3 = 2026-01-01 00:00:00.5 UTC en ms; stratum 2. offset/delay dependen del reloj local.
        assert_eq!(out.trim(), "1767225600500\n2", "hora del servidor (vm={vm}): {out}");
    }
}
