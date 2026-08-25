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
fn file_binary_round_trip() {
    for vm in [false, true] {
        let dat = std::env::temp_dir().join(format!("ray_bin_{}.dat", if vm { "vm" } else { "in" }));
        let src = format!(
            r#"
import std/fs;
fn main() -> int {{
    let data: bytes = b"RAY\x00\xff\x01\x02bin";
    match (fs.write_file_bytes("{path}", data)) {{
        Result.Ok(n) => print(to_string(n)),
        Result.Err(e) => {{ eprint(e); return 1; }},
    }};
    match (fs.read_file_bytes("{path}")) {{
        Result.Ok(leido) => {{
            print(to_string(leido.len()));
            if (leido == data) {{ print("identico") }} else {{ print("CORRUPTO") }}
        }},
        Result.Err(e) => print("err: " + e),
    }};
    0
}}
"#,
            path = dat.to_string_lossy()
        );
        let (out, code) = run("ray_bin_file", &src, vm);
        // 10 octetos: R A Y \x00 \xff \x01 \x02 b i n
        assert_eq!(out, "10\n10\nidentico\n", "round-trip binary (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

/// M113: `fs.read_bytes(h, max)` + `fs.seek(h, pos)` — lectura por trozos con memoria acotada.
/// Trozos exactos de `max` (salvo el último), EOF → `None`, seek + relectura (reanudación), los
/// octetos crudos (`\x00`/`\xff`) intactos, y el error limpio sobre un handle de escritura.
#[test]
fn file_chunked_read_and_seek() {
    for vm in [false, true] {
        let dat = std::env::temp_dir().join(format!("ray_chunks_{}.dat", if vm { "vm" } else { "in" }));
        let src = format!(
            r#"
import std/fs;
fn main() -> int {{
    let data: bytes = b"\x00\x01\x02\x03\x04\x05\x06\xff";
    let _ = fs.write_file_bytes("{path}", data);
    match (fs.open("{path}", "r")) {{
        Result.Err(e) => {{ eprint(e); 1 }},
        Result.Ok(h) => {{
            var sizes = "";
            var total: bytes = b"";
            var go = true;
            while (go) {{
                match (fs.read_bytes(h, 3)) {{
                    Result.Ok(opt) => match (opt) {{
                        Option.Some(piece) => {{ sizes = sizes + to_string(piece.len()); total = total + piece; }},
                        Option.None => {{ sizes = sizes + "."; go = false; }},
                    }},
                    Result.Err(e) => {{ eprint(e); go = false; }},
                }}
            }}
            print(sizes);
            print(if (total == data) {{ "identico" }} else {{ "CORRUPTO" }});
            // Reanudación: seek al octeto 6 y releer la cola.
            match (fs.seek(h, 6)) {{
                Result.Ok(p) => print("pos=" + to_string(p)),
                Result.Err(e) => print("seek err: " + e),
            }}
            match (fs.read_bytes(h, 10)) {{
                Result.Ok(o2) => match (o2) {{
                    Option.Some(tail) => print(if (tail == b"\x06\xff") {{ "cola ok" }} else {{ "cola MAL" }}),
                    Option.None => print("cola vacia"),
                }},
                Result.Err(e) => print("err: " + e),
            }}
            let _ = close(h);
            // Un handle de escritura no se puede leer: error como valor, no crash.
            match (fs.open("{path}", "w")) {{
                Result.Ok(w) => {{
                    match (fs.read_bytes(w, 4)) {{
                        Result.Ok(_) => print("LEYO un writer"),
                        Result.Err(_) => print("writer rechazado"),
                    }}
                    let _ = close(w);
                }},
                Result.Err(e) => print("open w: " + e),
            }}
            let _ = fs.remove_file("{path}");
            0
        }},
    }}
}}
"#,
            path = dat.to_string_lossy()
        );
        let (out, code) = run("ray_chunks_file", &src, vm);
        assert_eq!(
            out, "332.\nidentico\npos=6\ncola ok\nwriter rechazado\n",
            "chunked read + seek (vm={vm}): {out}"
        );
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
fn socket_binary_reads_raw_bytes() {
    const CLIENT: &str = r#"
import std/net;
fn main() -> int {
    match (net.tcp_connect("127.0.0.1", __PORT__)) {
        Result.Ok(h) => {
            match (net.socket_write_bytes(h, b"req")) {
                Result.Ok(_) => {
                    match (net.socket_read_bytes(h)) {
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
        let src = CLIENT.replace("__PORT__", &port.to_string());
        let (out, code) = run("ray_bin_sock", &src, vm);
        // 3 octetos crudos: 0, 16, 255 (un string UTF-8 lossy los habría corrompido).
        assert_eq!(out, "3\n0\n16\n255\n", "lectura binaria de socket (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

/// M115.1: `fs.write_bytes(h, data)` + `fs.sync(h)` — escritura binaria sobre handle y fsync.
/// El gemelo binario de `write`: octetos crudos (`\x00`/`\xff`) intactos a través del handle,
/// compone con append ("a") y con `seek`+`read_bytes` para releer; `sync` fuerza a disco y
/// falla limpio sobre un lector o un handle inválido.
#[test]
fn file_write_bytes_and_sync() {
    for vm in [false, true] {
        let dat = std::env::temp_dir().join(format!("ray_wbytes_{}.dat", if vm { "vm" } else { "in" }));
        let _ = std::fs::remove_file(&dat);
        let src = format!(
            r#"
import std/fs;
fn main() -> int {{
    let h = match (fs.open("{path}", "w")) {{
        Result.Ok(h) => h,
        Result.Err(e) => {{ eprint(e); return 1; }},
    }};
    match (fs.write_bytes(h, b"WAL\x00\xff")) {{
        Result.Ok(n) => print(to_string(n)),
        Result.Err(e) => {{ eprint(e); return 1; }},
    }};
    match (fs.sync(h)) {{
        Result.Ok(_) => print("synced"),
        Result.Err(e) => {{ eprint(e); return 1; }},
    }};
    close(h);
    // append binario sobre handle + sync, y relectura completa
    let a = match (fs.open("{path}", "a")) {{
        Result.Ok(h) => h,
        Result.Err(e) => {{ eprint(e); return 1; }},
    }};
    let _ = fs.write_bytes(a, b"\x01tail");
    let _ = fs.sync(a);
    close(a);
    match (fs.read_file_bytes("{path}")) {{
        Result.Ok(all) => {{
            print(to_string(all.len()));
            if (all == b"WAL\x00\xff\x01tail") {{ print("identico") }} else {{ print("CORRUPTO") }}
        }},
        Result.Err(e) => print("err: " + e),
    }};
    // errores limpios: write_bytes/sync sobre un LECTOR, y sobre un handle inválido
    let r = match (fs.open("{path}", "r")) {{
        Result.Ok(h) => h,
        Result.Err(e) => {{ eprint(e); return 1; }},
    }};
    match (fs.write_bytes(r, b"x")) {{
        Result.Ok(_) => print("MAL"),
        Result.Err(e) => print(e),
    }};
    match (fs.sync(r)) {{
        Result.Ok(_) => print("MAL"),
        Result.Err(e) => print(e),
    }};
    close(r);
    match (fs.sync(99999)) {{
        Result.Ok(_) => print("MAL"),
        Result.Err(e) => print(e),
    }};
    0
}}
"#,
            path = dat.to_string_lossy()
        );
        let (out, code) = run("ray_wbytes", &src, vm);
        assert_eq!(
            out,
            "5\nsynced\n10\nidentico\n\
             the handle is open for reading, not writing\n\
             the handle is open for reading, not writing\n\
             invalid file handle: 99999\n",
            "write_bytes + sync (vm={vm}): {out}"
        );
        assert_eq!(code, 0);
    }
}
