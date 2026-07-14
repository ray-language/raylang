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

fn elem_doc(name: &str, d: &[u8]) -> Vec<u8> {
    let mut v = vec![3u8];
    v.extend(cstr(name));
    v.extend(d);
    v
}

fn elem_arr(name: &str, d: &[u8]) -> Vec<u8> {
    let mut v = vec![4u8];
    v.extend(cstr(name));
    v.extend(d);
    v
}

fn elem_i64(name: &str, n: i64) -> Vec<u8> {
    let mut v = vec![18u8];
    v.extend(cstr(name));
    v.extend(n.to_le_bytes());
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
fn read_op_msg<S: Read>(s: &mut S) -> Option<(i32, Vec<u8>)> {
    let mut hdr = [0u8; 4];
    if s.read_exact(&mut hdr).is_err() {
        return None;
    }
    let total = i32::from_le_bytes(hdr) as usize;
    let mut rest = vec![0u8; total - 4];
    s.read_exact(&mut rest).expect("body del mensaje");
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
fn handle(mut s: TcpStream) {
    handle_stream(&mut s);
}

/// La sesión en sí, genérica sobre el flujo (TCP plano o rustls::Stream → sirve para el test TLS).
fn handle_stream<S: Read + Write>(s: &mut S) {
    while let Some((req, msg)) = read_op_msg(&mut *s) {
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
        } else if contains(&msg, b"no_existe") {
            // Cualquier operación sobre la colección "no_existe" → error del servidor.
            doc(&[elem_double("ok", 0.0), elem_str("errmsg", "ns not found"), elem_i32("code", 26)])
        } else if contains(&msg, b"insert") {
            // Verifica que el documento insertado VIAJÓ dentro del comando (el binding fluye).
            if contains(&msg, b"documents") && contains(&msg, b"ada") {
                doc(&[elem_i32("n", 2), elem_double("ok", 1.0)])
            } else {
                doc(&[elem_double("ok", 0.0), elem_str("errmsg", "insert sin documentos")])
            }
        } else if contains(&msg, b"getMore") {
            // Paginación multi-ronda: el id 77 pide la segunda página (deja vivo el 88); el 88,
            // la tercera y última (id 0 = cursor agotado). El id viaja como int64 LE.
            if contains(&msg, &77i64.to_le_bytes()) {
                let d = doc(&[elem_str("name", "grace")]);
                let batch = doc(&[elem_doc("0", &d)]);
                let cursor = doc(&[
                    elem_arr("nextBatch", &batch),
                    elem_i64("id", 88),
                    elem_str("ns", "test.paginada"),
                ]);
                doc(&[elem_doc("cursor", &cursor), elem_double("ok", 1.0)])
            } else if contains(&msg, &88i64.to_le_bytes()) {
                let d = doc(&[elem_str("name", "lin")]);
                let batch = doc(&[elem_doc("0", &d)]);
                let cursor = doc(&[
                    elem_arr("nextBatch", &batch),
                    elem_i64("id", 0),
                    elem_str("ns", "test.paginada"),
                ]);
                doc(&[elem_doc("cursor", &cursor), elem_double("ok", 1.0)])
            } else {
                doc(&[elem_double("ok", 0.0), elem_str("errmsg", "cursor unknown"), elem_i32("code", 43)])
            }
        } else if contains(&msg, b"find") {
            if contains(&msg, b"paginada") {
                // El firstBatch trae un documento y deja el cursor VIVO (id 77) → el cliente
                // debe agotar con getMore.
                let d = doc(&[elem_str("name", "ada")]);
                let batch = doc(&[elem_doc("0", &d)]);
                let cursor = doc(&[
                    elem_arr("firstBatch", &batch),
                    elem_i64("id", 77),
                    elem_str("ns", "test.paginada"),
                ]);
                doc(&[elem_doc("cursor", &cursor), elem_double("ok", 1.0)])
            } else {
                // Un cursor con dos documentos en el firstBatch (el segundo sin `nota`) e id 0.
                let d0 = doc(&[elem_str("name", "ada"), elem_i32("nota", 36)]);
                let d1 = doc(&[elem_str("name", "grace")]);
                let batch = doc(&[elem_doc("0", &d0), elem_doc("1", &d1)]);
                let cursor = doc(&[
                    elem_arr("firstBatch", &batch),
                    elem_i64("id", 0),
                    elem_str("ns", "test.usuarios"),
                ]);
                doc(&[elem_doc("cursor", &cursor), elem_double("ok", 1.0)])
            }
        } else if contains(&msg, b"update") {
            if contains(&msg, b"$set") {
                doc(&[elem_i32("n", 1), elem_i32("nModified", 1), elem_double("ok", 1.0)])
            } else {
                doc(&[elem_double("ok", 0.0), elem_str("errmsg", "update sin $set")])
            }
        } else if contains(&msg, b"delete") {
            doc(&[elem_i32("n", 3), elem_double("ok", 1.0)])
        } else {
            doc(&[elem_double("ok", 0.0), elem_str("errmsg", "comando unknown")])
        };
        s.write_all(&op_msg(req, reply)).expect("responde");
    }
}

fn launch_servidor() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    thread::spawn(move || handle(s));
                }
                Err(_) => break,
            }
        }
    });
    port
}

fn project(base: &std::path::Path, port: u16) -> std::path::PathBuf {
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
    match (mongo.connect("127.0.0.1", {port}, "other", "secret", "test", "clientnonce123456")) {{
        Result.Ok(_) => {{ print("no debería"); return 1; }},
        Result.Err(e) => {{ print("mal user: " + e); }},
    }}
    0
}}
"#
    );
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

fn run(app: &std::path::Path, flags: &[&str]) -> String {
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    let out = Command::new(BIN).args(&args).current_dir(app).output().expect("lanza el binary");
    assert!(
        out.status.success(),
        "runs sin error\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const ESPERADO: &str = "conectado\n\
mala clave: mongo: the server signature does not verify (authentication failed)\n\
mal user: mongo: Authentication failed.\n";

#[test]
fn mongo_hello_scram_y_errors_de_auth() {
    let base = std::env::temp_dir().join("ray_mongo_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let port = launch_servidor();
    let app = project(&base, port);

    // VM (motor de producto) e intérprete (oráculo): mismo stdout exacto.
    assert_eq!(run(&app, &[]), ESPERADO, "VM");
    assert_eq!(run(&app, &["--interp"]), ESPERADO, "intérprete");
}

// --- M54.3: CRUD ---

fn project_crud(base: &std::path::Path, port: u16) -> std::path::PathBuf {
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
import db/bson;

fn main() -> int {{
    var c = match (mongo.connect("127.0.0.1", {port}, "raylang", "secret", "test", "clientnonce123456")) {{
        Result.Ok(conn) => conn,
        Result.Err(e) => {{ print(e); return 1; }},
    }};

    // insert: dos documentos (el _id lo asigna el servidor).
    let docs = [
        [bson.field("name", bson.Bson.Str("ada")), bson.field("nota", bson.Bson.Int(36))],
        [bson.field("name", bson.Bson.Str("grace"))],
    ];
    match (mongo.insert(c, "usuarios", docs)) {{
        Result.Ok(n) => {{ print("insertados: " + to_string(n)); }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}

    // find: el firstBatch del cursor, documento a documento.
    let filter = [bson.field("name", bson.Bson.Str("ada"))];
    match (mongo.find(c, "usuarios", filter)) {{
        Result.Ok(rows) => {{
            var i = 0;
            while (i < rows.len()) {{
                print(bson.dump_doc(rows[i]));
                i = i + 1;
            }}
        }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}

    // update con $set explícito (fiel al protocolo).
    let set = [bson.field("$set", bson.Bson.Doc([bson.field("nota", bson.Bson.Int(37))]))];
    match (mongo.update(c, "usuarios", filter, set, false)) {{
        Result.Ok(n) => {{ print("modificados: " + to_string(n)); }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}

    // delete de todas las coincidencias.
    match (mongo.delete(c, "usuarios", filter)) {{
        Result.Ok(n) => {{ print("borrados: " + to_string(n)); }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}

    // Cursor paginado: el firstBatch deja el cursor vivo → find agota con getMore (2 rondas).
    let sin: [bson.Field] = [];
    match (mongo.find(c, "paginada", sin)) {{
        Result.Ok(rows) => {{
            print("paginados: " + to_string(rows.len()));
            var i = 0;
            while (i < rows.len()) {{
                print(bson.dump_doc(rows[i]));
                i = i + 1;
            }}
        }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}

    // Error del servidor como valor: colección inexistente.
    match (mongo.find(c, "no_existe", sin)) {{
        Result.Ok(_) => {{ print("no debería"); return 1; }},
        Result.Err(e) => {{ print(e); }},
    }}

    mongo.disconnect(c);
    0
}}
"#
    );
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

const ESPERADO_CRUD: &str = "insertados: 2\n\
{name: \"ada\", nota: 36}\n\
{name: \"grace\"}\n\
modificados: 1\n\
borrados: 3\n\
paginados: 3\n\
{name: \"ada\"}\n\
{name: \"grace\"}\n\
{name: \"lin\"}\n\
mongo: ns not found\n";

#[test]
fn mongo_crud_y_error_del_servidor() {
    let base = std::env::temp_dir().join("ray_mongo_cli_crud");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let port = launch_servidor();
    let app = project_crud(&base, port);

    assert_eq!(run(&app, &[]), ESPERADO_CRUD, "VM");
    assert_eq!(run(&app, &["--interp"]), ESPERADO_CRUD, "intérprete");
}

// --- TLS: connect_tls (cifrado desde el octeto 0, sin STARTTLS) ---

fn launch_servidor_tls() -> u16 {
    use rustls::pki_types::pem::PemObject;
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls::pki_types::CertificateDer::pem_slice_iter(include_str!("fixtures/tls_cert.pem").as_bytes())
            .collect::<Result<_, _>>()
            .expect("certificado de prueba válido");
    let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(include_str!("fixtures/tls_key.pem").as_bytes())
        .expect("clave de prueba válida");
    let config = std::sync::Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("config de servidor"),
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            // TLS desde el octeto 0: el ClientHello es lo primero que llega.
            let mut conn = rustls::ServerConnection::new(config.clone()).expect("server conn");
            let mut tls = rustls::Stream::new(&mut conn, &mut s);
            handle_stream(&mut tls);
            conn.send_close_notify();
            let _ = conn.complete_io(&mut s);
        }
    });
    port
}

fn project_tls(base: &std::path::Path, port: u16) -> std::path::PathBuf {
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
import db/bson;

fn main() -> int {{
    // Conexión TLS completa (handshake rustls + hello + SCRAM) y un find sobre el canal cifrado.
    var c = match (mongo.connect_tls("localhost", {port}, "raylang", "secret", "test", "clientnonce123456")) {{
        Result.Ok(conn) => conn,
        Result.Err(e) => {{ print(e); return 1; }},
    }};
    print("conectado seguro");
    let filter = [bson.field("name", bson.Bson.Str("ada"))];
    match (mongo.find(c, "usuarios", filter)) {{
        Result.Ok(rows) => {{
            var i = 0;
            while (i < rows.len()) {{
                print(bson.dump_doc(rows[i]));
                i = i + 1;
            }}
        }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    mongo.disconnect(c);
    0
}}
"#
    );
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

fn run_tls(app: &std::path::Path, flags: &[&str]) -> String {
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    let out = Command::new(BIN)
        .args(&args)
        .current_dir(app)
        .env("SSL_CERT_FILE", &ca)
        .output()
        .expect("lanza el binary");
    assert!(
        out.status.success(),
        "runs sin error\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const ESPERADO_TLS: &str = "conectado seguro\n\
{name: \"ada\", nota: 36}\n\
{name: \"grace\"}\n";

#[test]
fn mongo_tls_conexion_y_find_cifrados() {
    let base = std::env::temp_dir().join("ray_mongo_cli_tls");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let port = launch_servidor_tls();
    let app = project_tls(&base, port);

    assert_eq!(run_tls(&app, &[]), ESPERADO_TLS, "VM");
    assert_eq!(run_tls(&app, &["--interp"]), ESPERADO_TLS, "intérprete");
}
