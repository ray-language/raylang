//! M53.1 — cliente MySQL (`packages/db/mysql.ray`). El cliente de raylang hace el handshake v10 +
//! auth `mysql_native_password` + COM_QUERY (texto) contra un **servidor MySQL de juguete escrito a
//! mano** (solo std, TCP plano) con scramble FIJO; la respuesta de auth esperada está PRECOMPUTADA
//! (python: SHA1(pass) XOR SHA1(scramble+SHA1(SHA1(pass))) para pass=secret) → sin cripto en Rust.
//! El servidor verifica la auth octeto a octeto y sirve un result set fijo, un OK de exec y un ERR.
//! Oráculo conductual en ambos motores (mismo stdout).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Scramble fijo del handshake (20 octetos) y la respuesta de auth PRECOMPUTADA para pass=secret.
const SCRAMBLE: [u8; 20] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20];
const AUTH_ESPERADA: [u8; 20] = [
    179, 43, 179, 165, 131, 225, 52, 12, 10, 17, 8, 213, 139, 27, 228, 151, 129, 173, 140, 47,
];

/// Un paquete MySQL: [longitud:3 LE][secuencia:1][carga].
fn pkt(seq: u8, payload: &[u8]) -> Vec<u8> {
    let n = payload.len();
    let mut m = vec![(n & 255) as u8, ((n >> 8) & 255) as u8, ((n >> 16) & 255) as u8, seq];
    m.extend_from_slice(payload);
    m
}

fn read_pkt(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut hdr = [0u8; 4];
    s.read_exact(&mut hdr).expect("cabecera");
    let len = hdr[0] as usize | (hdr[1] as usize) << 8 | (hdr[2] as usize) << 16;
    let mut payload = vec![0u8; len];
    s.read_exact(&mut payload).expect("carga");
    (hdr[3], payload)
}

/// El handshake v10 del servidor de juguete: versión, thread id, scramble en dos partes,
/// capacidades con PLUGIN_AUTH, y el nombre del plugin.
fn handshake_v10() -> Vec<u8> {
    let mut p = vec![10u8]; // protocolo v10
    p.extend_from_slice(b"8.0.0-juguete\0");
    p.extend_from_slice(&[1, 0, 0, 0]); // thread id
    p.extend_from_slice(&SCRAMBLE[..8]); // auth-data parte 1
    p.push(0); // filler
    p.extend_from_slice(&[0x00, 0x82]); // capacidades bajas: PROTOCOL_41 | SECURE_CONNECTION
    p.push(33); // charset
    p.extend_from_slice(&[0, 0]); // estado
    p.extend_from_slice(&[0x08, 0x00]); // capacidades altas: PLUGIN_AUTH (0x00080000 >> 16)
    p.push(21); // longitud del auth-data (20 + NUL)
    p.extend_from_slice(&[0; 10]); // reservado
    p.extend_from_slice(&SCRAMBLE[8..]); // auth-data parte 2 (12 octetos)
    p.push(0); // NUL del scramble
    p.extend_from_slice(b"mysql_native_password\0");
    p
}

/// Un string length-encoded corto (< 251 octetos).
fn lenc(s: &str) -> Vec<u8> {
    let mut v = vec![s.len() as u8];
    v.extend_from_slice(s.as_bytes());
    v
}

/// Una definición de columna mínima (el cliente se la salta, pero ha de ser un paquete).
fn col_def(nombre: &str) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&lenc("def")); // catálogo
    for _ in 0..3 {
        p.extend_from_slice(&lenc("")); // esquema, tabla, tabla original
    }
    p.extend_from_slice(&lenc(nombre)); // nombre
    p.extend_from_slice(&lenc(nombre)); // nombre original
    p.push(0x0c); // longitud del bloque fijo
    p.extend_from_slice(&[33, 0]); // charset
    p.extend_from_slice(&[255, 0, 0, 0]); // longitud de columna
    p.push(0xfd); // tipo VAR_STRING
    p.extend_from_slice(&[0, 0]); // flags
    p.push(0); // decimales
    p.extend_from_slice(&[0, 0]); // relleno
    p
}

const EOF: [u8; 5] = [0xfe, 0, 0, 0, 0]; // EOF clásico: marcador + warnings + estado

/// Atiende una sesión completa: handshake, auth verificada, y comandos hasta COM_QUIT.
fn atender(mut s: TcpStream) {
    s.write_all(&pkt(0, &handshake_v10())).unwrap();
    let (_seq, resp) = read_pkt(&mut s);
    // HandshakeResponse41: capacidades(4) + max(4) + charset(1) + reservado(23) + user NUL + auth.
    let mut i = 4 + 4 + 1 + 23;
    let user_start = i;
    while resp[i] != 0 {
        i += 1;
    }
    let user = String::from_utf8_lossy(&resp[user_start..i]).into_owned();
    i += 1;
    let auth_len = resp[i] as usize;
    i += 1;
    let auth = &resp[i..i + auth_len];
    if user != "raylang" || auth != AUTH_ESPERADA {
        let mut err = vec![0xffu8, 0x15, 0x04]; // ERR + código 1045
        err.extend_from_slice(b"#28000acceso denegado");
        s.write_all(&pkt(2, &err)).unwrap();
        return;
    }
    s.write_all(&pkt(2, &[0x00, 0, 0, 0, 0, 0, 0])).unwrap(); // OK

    // Fase de comandos.
    loop {
        let mut hdr = [0u8; 4];
        if s.read_exact(&mut hdr).is_err() {
            return; // el cliente cerró
        }
        let len = hdr[0] as usize | (hdr[1] as usize) << 8 | (hdr[2] as usize) << 16;
        let mut payload = vec![0u8; len];
        s.read_exact(&mut payload).unwrap();
        match payload.first() {
            Some(1) => return, // COM_QUIT
            Some(3) => {
                let sql = String::from_utf8_lossy(&payload[1..]).into_owned();
                if sql.starts_with("SELECT") {
                    // Result set: 2 columnas, 2 filas (la segunda con un NULL).
                    s.write_all(&pkt(1, &[2])).unwrap();
                    s.write_all(&pkt(2, &col_def("nombre"))).unwrap();
                    s.write_all(&pkt(3, &col_def("nota"))).unwrap();
                    s.write_all(&pkt(4, &EOF)).unwrap();
                    let mut fila1 = lenc("ada");
                    fila1.extend_from_slice(&lenc("36"));
                    s.write_all(&pkt(5, &fila1)).unwrap();
                    let mut fila2 = lenc("grace");
                    fila2.push(0xfb); // NULL
                    s.write_all(&pkt(6, &fila2)).unwrap();
                    s.write_all(&pkt(7, &EOF)).unwrap();
                } else if sql.starts_with("INSERT") {
                    // OK con 3 filas afectadas.
                    s.write_all(&pkt(1, &[0x00, 3, 0, 0, 0, 0, 0])).unwrap();
                } else {
                    let mut err = vec![0xffu8, 0x7a, 0x04]; // código 1146
                    err.extend_from_slice(b"#42S02la tabla no existe");
                    s.write_all(&pkt(1, &err)).unwrap();
                }
            }
            _ => return,
        }
    }
}

/// Lanza el servidor de juguete en un puerto efímero; atiende conexiones en serie (una por motor).
fn lanzar_servidor() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for s in listener.incoming().flatten() {
            atender(s);
        }
    });
    port
}

/// Crea el proyecto cliente (path-dep a packages/db) y devuelve su raíz.
fn proyecto(base: &std::path::Path, port: u16) -> std::path::PathBuf {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/db");
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ndb = \"path:{}\"\n",
            db.display()
        ),
    )
    .unwrap();
    let main = format!(
        r#"import db/mysql;

fn main() -> int {{
    var c = match (mysql.connect("127.0.0.1", {port}, "raylang", "secret", "demo")) {{
        Result.Ok(conn) => conn,
        Result.Err(e) => {{ print(e); return 1; }},
    }};
    match (mysql.query(c, "SELECT nombre, nota FROM alumnos")) {{
        Result.Ok(rows) => {{
            var i = 0;
            while (i < rows.len()) {{
                print(rows[i].join("|"));
                i = i + 1;
            }}
        }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    match (mysql.exec(c, "INSERT INTO alumnos VALUES (1)")) {{
        Result.Ok(n) => {{ print("afectadas: " + to_string(n)); }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    match (mysql.query(c, "BOOM")) {{
        Result.Ok(_) => {{ print("no debería"); }},
        Result.Err(e) => {{ print(e); }},
    }}
    mysql.disconnect(c);
    0
}}
"#
    );
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

fn correr(app: &std::path::Path, flags: &[&str]) -> (String, i32) {
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    let out = Command::new(BIN).args(&args).current_dir(app).output().expect("lanza el binario");
    assert!(
        out.status.success(),
        "corre sin error\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

const ESPERADO: &str = "ada|36\ngrace|\nafectadas: 3\nmysql: la tabla no existe\n";

#[test]
fn mysql_handshake_query_exec_y_error() {
    let base = std::env::temp_dir().join("ray_mysql_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let port = lanzar_servidor();
    let app = proyecto(&base, port);

    // VM (motor de producto) e intérprete (oráculo): mismo stdout exacto.
    let (out_vm, _) = correr(&app, &[]);
    assert_eq!(out_vm, ESPERADO, "VM");
    let (out_interp, _) = correr(&app, &["--interp"]);
    assert_eq!(out_interp, ESPERADO, "intérprete");
}

#[test]
fn mysql_password_incorrecta_da_error_claro() {
    let base = std::env::temp_dir().join("ray_mysql_cli_badpw");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let port = lanzar_servidor();
    let app = proyecto(&base, port);
    // Reescribe el main con una contraseña equivocada → el servidor rechaza con su ERR.
    let main = std::fs::read_to_string(app.join("src/main.ray")).unwrap();
    std::fs::write(app.join("src/main.ray"), main.replace("\"secret\"", "\"mala\"")).unwrap();
    let out = Command::new(BIN).args(["run"]).current_dir(&app).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("acceso denegado"), "ERR del servidor visible:\n{stdout}");
    assert_eq!(out.status.code(), Some(1), "el programa sale con 1");
}
