//! Pruebas de la I/O binaria (M16.1c): `read_file_bytes`/`write_file_bytes` y
//! `socket_read_bytes`/`socket_write_bytes`. No determinista (toca disco/red) → subproceso, en
//! ambos motores. El punto clave: los octetos crudos (incl. `\x00`/`\xff`) viajan **intactos**, lo
//! que `string` (UTF-8 lossy) corrompería.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

fn run(name: &str, src: &str, vm: bool) -> (String, i32) {
    let mut path = std::env::temp_dir();
    path.push(format!("{name}.ray"));
    std::fs::File::create(&path).expect("crea").write_all(src.as_bytes()).expect("escribe");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let out = cmd.arg(&path).output().expect("lanza raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn archivo_binario_round_trip() {
    for vm in [false, true] {
        let dat = std::env::temp_dir().join(format!("ray_bin_{}.dat", if vm { "vm" } else { "in" }));
        let src = format!(
            r#"
import std/fs;
fn main() -> int {{
    let datos: bytes = b"RAY\x00\xff\x01\x02bin";
    match (fs.write_file_bytes("{ruta}", datos)) {{
        Result.Ok(n) => print(to_string(n)),
        Result.Err(e) => {{ eprint(e); return 1; }},
    }};
    match (fs.read_file_bytes("{ruta}")) {{
        Result.Ok(leido) => {{
            print(to_string(leido.len()));
            if (leido == datos) {{ print("identico") }} else {{ print("CORRUPTO") }}
        }},
        Result.Err(e) => print("err: " + e),
    }};
    0
}}
"#,
            ruta = dat.to_string_lossy()
        );
        let (out, code) = run("ray_bin_file", &src, vm);
        // 10 octetos: R A Y \x00 \xff \x01 \x02 b i n
        assert_eq!(out, "10\n10\nidentico\n", "round-trip binario (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

/// Servidor de juguete que envía 3 octetos crudos (0, 16, 255) y cierra. Devuelve el puerto.
fn toy_bin_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 64];
            let _ = stream.read(&mut buf); // consume la petición
            let _ = stream.write_all(&[0u8, 16u8, 255u8]);
        }
    });
    port
}

#[test]
fn socket_binario_lee_octetos_crudos() {
    const CLIENTE: &str = r#"
fn main() -> int {
    match (tcp_connect("127.0.0.1", __PORT__)) {
        Result.Ok(h) => {
            match (socket_write_bytes(h, b"req")) {
                Result.Ok(_) => {
                    match (socket_read_bytes(h)) {
                        Result.Ok(b) => {
                            print(to_string(b.len()));
                            print(to_string(b[0]));
                            print(to_string(b[1]));
                            print(to_string(b[2]));
                        },
                        Result.Err(e) => print("read err: " + e),
                    }
                },
                Result.Err(e) => print("write err: " + e),
            };
            close(h);
            0
        },
        Result.Err(e) => { eprint(e); 1 },
    }
}
"#;
    for vm in [false, true] {
        let port = toy_bin_server();
        let src = CLIENTE.replace("__PORT__", &port.to_string());
        let (out, code) = run("ray_bin_sock", &src, vm);
        // 3 octetos crudos: 0, 16, 255 (un string UTF-8 lossy los habría corrompido).
        assert_eq!(out, "3\n0\n16\n255\n", "lectura binaria de socket (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}
