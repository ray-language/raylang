//! M54.2 — conexión del cliente MongoDB (`packages/db/mongo.ray`): OP_MSG + `hello` + auth
//! SCRAM-SHA-256 vía SASL, contra un **servidor MongoDB de juguete** (Rust std, TCP plano). Reusa
//! las constantes SCRAM **precomputadas** de `tests/postgres_cli.rs` (user=raylang, pass=secret,
//! nonce=clientnonce123456, i=64) → offline y determinista, sin cripto en Rust. El servidor
//! responde OP_MSG canned; verifica que el client-first lleve el usuario/nonce esperados y sirve
//! `ok: 0.0` + errmsg ante un usuario desconocido. La contraseña la verifica el CLIENTE: con una
//! clave mala, la firma del servidor (`v=` del server-final) no cuadra y `scram_verify` falla.
//! Oráculo conductual en ambos motores (mismo stdout).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

// Las mismas constantes que el toy server de PostgreSQL (mismo usuario/clave/nonce/sal/i).
const SERVER_FIRST: &[u8] = &[
    114, 61, 99, 108, 105, 101, 110, 116, 110, 111, 110, 99, 101, 49, 50, 51, 52, 53, 54, 115,
    101, 114, 118, 101, 114, 110, 111, 110, 99, 101, 55, 56, 57, 44, 115, 61, 65, 81, 73, 68, 66,
    65, 85, 71, 66, 119, 103, 74, 67, 103, 115, 77, 68, 81, 52, 80, 69, 65, 61, 61, 44, 105, 61,
    54, 52,
];
const SERVER_FINAL: &[u8] = &[
    118, 61, 80, 87, 88, 53, 71, 72, 119, 87, 43, 66, 120, 108, 101, 99, 70, 122, 79, 56, 116,
    105, 79, 81, 69, 86, 111, 98, 86, 117, 43, 54, 80, 66, 103, 56, 55, 49, 110, 109, 121, 101,
    81, 51, 52, 61,
];
const CLIENT_FIRST: &[u8] = b"n,,n=raylang,r=clientnonce123456";

// --- Mini-constructor de BSON (solo lo que responde el servidor) ---

fn cstr(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

fn elem_double(name: &str, f: f64) -> Vec<u8> {
    let mut v = vec![1u8];
    v.extend(cstr(name));
    v.extend(f.to_le_bytes());
    v
}

fn elem_i32(name: &str, n: i32) -> Vec<u8> {
    let mut v = vec![16u8];
    v.extend(cstr(name));
    v.extend(n.to_le_bytes());
    v
}

fn elem_bool(name: &str, b: bool) -> Vec<u8> {
    let mut v = vec![8u8];
    v.extend(cstr(name));
    v.push(b as u8);
    v
}

fn elem_str(name: &str, s: &str) -> Vec<u8> {
    let mut v = vec![2u8];
    v.extend(cstr(name));
    v.extend(((s.len() + 1) as i32).to_le_bytes());
    v.extend(s.as_bytes());
    v.push(0);
    v
}

fn elem_bin(name: &str, data: &[u8]) -> Vec<u8> {
    let mut v = vec![5u8];
    v.extend(cstr(name));
    v.extend((data.len() as i32).to_le_bytes());
    v.push(0);
    v.extend(data);
    v
}

fn doc(elems: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = elems.concat();
    let mut v = ((body.len() + 5) as i32).to_le_bytes().to_vec();
    v.extend(body);
    v.push(0);
    v
}

/// Envuelve un documento en un OP_MSG de respuesta (kind 0, sin flags).
fn op_msg(response_to: i32, d: Vec<u8>) -> Vec<u8> {
    let mut v = ((21 + d.len()) as i32).to_le_bytes().to_vec();
    v.extend(999i32.to_le_bytes()); // requestID del servidor (arbitrario)
    v.extend(response_to.to_le_bytes());
    v.extend(2013i32.to_le_bytes());
    v.extend(0i32.to_le_bytes()); // flagBits
    v.push(0); // kind 0
    v.extend(d);
    v
}

/// Lee un OP_MSG del cliente: devuelve (requestID, mensaje completo). None en EOF.
fn read_op_msg(s: &mut TcpStream) -> Option<(i32, Vec<u8>)> {
    let mut hdr = [0u8; 4];
    if s.read_exact(&mut hdr).is_err() {
        return None;
    }
    let total = i32::from_le_bytes(hdr) as usize;
    let mut rest = vec![0u8; total - 4];
    s.read_exact(&mut rest).expect("cuerpo del mensaje");
    let req_id = i32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
    let mut full = hdr.to_vec();
    full.extend(rest);
    Some((req_id, full))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// El servidor de juguete: hello → ok; saslStart (verifica el client-first) → server-first;
/// saslContinue → server-final. Un usuario desconocido recibe ok: 0.0 + errmsg.
fn atender(mut s: TcpStream) {
    while let Some((req, msg)) = read_op_msg(&mut s) {
        let reply = if contains(&msg, b"hello") {
            doc(&[elem_double("ok", 1.0)])
        } else if contains(&msg, b"saslStart") {
            if contains(&msg, CLIENT_FIRST) {
                doc(&[
                    elem_i32("conversationId", 1),
                    elem_bool("done", false),
                    elem_bin("payload", SERVER_FIRST),
                    elem_double("ok", 1.0),
                ])
            } else {
                doc(&[
                    elem_double("ok", 0.0),
                    elem_str("errmsg", "Authentication failed."),
                    elem_i32("code", 18),
                ])
            }
        } else if contains(&msg, b"saslContinue") {
            doc(&[
                elem_i32("conversationId", 1),
                elem_bool("done", true),
                elem_bin("payload", SERVER_FINAL),
                elem_double("ok", 1.0),
            ])
        } else {
            doc(&[elem_double("ok", 0.0), elem_str("errmsg", "comando desconocido")])
        };
        s.write_all(&op_msg(req, reply)).expect("responde");
    }
}

fn lanzar_servidor() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    thread::spawn(move || atender(s));
                }
                Err(_) => break,
            }
        }
    });
    port
}

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
        r#"import db/mongo;

fn main() -> int {{
    // 1. Credenciales correctas: hello + SCRAM completo (con verificación de la firma del servidor).
    match (mongo.connect("127.0.0.1", {port}, "raylang", "secret", "test", "clientnonce123456")) {{
        Result.Ok(c) => {{ print("conectado"); mongo.disconnect(c); }},
        Result.Err(e) => {{ print("no debería: " + e); return 1; }},
    }}
    // 2. Contraseña mala: el proof sale distinto y la firma del servidor NO verifica (lo detecta
    //    el CLIENTE, no el servidor de juguete).
    match (mongo.connect("127.0.0.1", {port}, "raylang", "malaclave", "test", "clientnonce123456")) {{
        Result.Ok(_) => {{ print("no debería"); return 1; }},
        Result.Err(e) => {{ print("mala clave: " + e); }},
    }}
    // 3. Usuario desconocido: el servidor responde ok: 0.0 + errmsg.
    match (mongo.connect("127.0.0.1", {port}, "otro", "secret", "test", "clientnonce123456")) {{
        Result.Ok(_) => {{ print("no debería"); return 1; }},
        Result.Err(e) => {{ print("mal usuario: " + e); }},
    }}
    0
}}
"#
    );
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

fn correr(app: &std::path::Path, flags: &[&str]) -> String {
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    let out = Command::new(BIN).args(&args).current_dir(app).output().expect("lanza el binario");
    assert!(
        out.status.success(),
        "corre sin error\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const ESPERADO: &str = "conectado\n\
mala clave: mongo: la firma del servidor no verifica (autenticación fallida)\n\
mal usuario: mongo: Authentication failed.\n";

#[test]
fn mongo_hello_scram_y_errores_de_auth() {
    let base = std::env::temp_dir().join("ray_mongo_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let port = lanzar_servidor();
    let app = proyecto(&base, port);

    // VM (motor de producto) e intérprete (oráculo): mismo stdout exacto.
    assert_eq!(correr(&app, &[]), ESPERADO, "VM");
    assert_eq!(correr(&app, &["--interp"]), ESPERADO, "intérprete");
}
