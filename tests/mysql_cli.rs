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

fn read_pkt<S: Read>(s: &mut S) -> (u8, Vec<u8>) {
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
    handshake_v10_plugin("mysql_native_password")
}

/// Como `handshake_v10` pero anunciando el plugin dado (el test TLS usa caching_sha2_password).
fn handshake_v10_plugin(plugin: &str) -> Vec<u8> {
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
    p.extend_from_slice(plugin.as_bytes());
    p.push(0);
    p
}

/// Un string length-encoded corto (< 251 octetos).
fn lenc(s: &str) -> Vec<u8> {
    let mut v = vec![s.len() as u8];
    v.extend_from_slice(s.as_bytes());
    v
}

/// Una definición de columna mínima (el cliente de texto se la salta; el binario lee tipo+flags).
fn col_def(nombre: &str) -> Vec<u8> {
    col_def_tipo(nombre, 0xfd)
}

fn col_def_tipo(nombre: &str, tipo: u8) -> Vec<u8> {
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
    p.push(tipo);
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
    fase_comandos(&mut s);
}

/// La fase de comandos (COM_QUERY/COM_QUIT), genérica sobre el flujo (TCP plano o rustls::Stream).
fn fase_comandos<S: Read + Write>(s: &mut S) {
    let mut prep_sql = String::new();
    let mut prep_nparams = 0usize;
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
            Some(0x16) => {
                // COM_STMT_PREPARE: id fijo 7; nº de params = los '?' del SQL; 0 columnas en el
                // prepare (las reales van en la respuesta al execute).
                prep_sql = String::from_utf8_lossy(&payload[1..]).into_owned();
                prep_nparams = prep_sql.bytes().filter(|b| *b == b'?').count();
                let mut ok = vec![0u8, 7, 0, 0, 0, 0, 0];
                ok.push(prep_nparams as u8);
                ok.push(0);
                ok.extend_from_slice(&[0, 0, 0]); // filler + warnings
                s.write_all(&pkt(1, &ok)).unwrap();
                let mut seq = 2u8;
                for _ in 0..prep_nparams {
                    s.write_all(&pkt(seq, &col_def("?"))).unwrap();
                    seq += 1;
                }
                if prep_nparams > 0 {
                    s.write_all(&pkt(seq, &EOF)).unwrap();
                }
                let _ = s.flush();
            }
            Some(0x17) => {
                // COM_STMT_EXECUTE: [id:4][flags][iter:4][bitmap][bound][tipos][valores lenc].
                let id = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
                assert_eq!(id, 7, "stmt id");
                let mut params: Vec<String> = Vec::new();
                if prep_nparams > 0 {
                    let mut i = 1 + 4 + 1 + 4 + (prep_nparams + 7) / 8;
                    assert_eq!(payload[i], 1, "new-params-bound");
                    i += 1 + prep_nparams * 2; // tipos (2 por parámetro)
                    for _ in 0..prep_nparams {
                        let l = payload[i] as usize; // lenc corto (los tests no pasan de 250)
                        i += 1;
                        params.push(String::from_utf8_lossy(&payload[i..i + l]).into_owned());
                        i += l;
                    }
                }
                if prep_sql.starts_with("BOOM") {
                    let mut err = vec![0xffu8, 0x7a, 0x04];
                    err.extend_from_slice(b"#42S02la tabla no existe");
                    s.write_all(&pkt(1, &err)).unwrap();
                } else if prep_sql.starts_with("SELECT") {
                    // Result set BINARIO: 4 columnas que ejercitan la decodificación por tipo.
                    s.write_all(&pkt(1, &[4])).unwrap();
                    s.write_all(&pkt(2, &col_def_tipo("nombre", 0xfd))).unwrap(); // VAR_STRING
                    s.write_all(&pkt(3, &col_def_tipo("nota", 8))).unwrap(); // LONGLONG
                    s.write_all(&pkt(4, &col_def_tipo("media", 5))).unwrap(); // DOUBLE
                    s.write_all(&pkt(5, &col_def_tipo("creado", 12))).unwrap(); // DATETIME
                    s.write_all(&pkt(6, &EOF)).unwrap();
                    // Fila 1: eco del primer parámetro + -5 + 2.5 + 2026-07-09 12:34:56.
                    let eco = params.first().cloned().unwrap_or_default();
                    let mut f1 = vec![0u8, 0]; // header + bitmap (sin NULLs)
                    f1.extend_from_slice(&lenc(&eco));
                    f1.extend_from_slice(&(-5i64).to_le_bytes());
                    f1.extend_from_slice(&2.5f64.to_le_bytes());
                    f1.extend_from_slice(&[7, 0xEA, 0x07, 7, 9, 12, 34, 56]);
                    s.write_all(&pkt(7, &f1)).unwrap();
                    // Fila 2: nota y media NULL (bits 3 y 4 del bitmap) + datetime cero (len 0).
                    let mut f2 = vec![0u8, 0b0001_1000];
                    f2.extend_from_slice(&lenc("fija"));
                    f2.push(0); // DATETIME len 0
                    s.write_all(&pkt(8, &f2)).unwrap();
                    s.write_all(&pkt(9, &EOF)).unwrap();
                } else {
                    // INSERT/UPDATE/…: OK con 4 filas afectadas.
                    s.write_all(&pkt(1, &[0x00, 4, 0, 0, 0, 0, 0])).unwrap();
                }
                let _ = s.flush();
            }
            Some(0x19) => {} // COM_STMT_CLOSE: sin respuesta
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
                } else if sql.starts_with("TRUNC") {
                    // M77: fila MALFORMADA — el string length-encoded declara 200 octetos pero el
                    // paquete solo trae 3. El cliente endurecido debe devolver Err, no reventar.
                    s.write_all(&pkt(1, &[1])).unwrap(); // 1 columna
                    s.write_all(&pkt(2, &col_def("x"))).unwrap();
                    s.write_all(&pkt(3, &EOF)).unwrap();
                    s.write_all(&pkt(4, &[200, b'a', b'b', b'c'])).unwrap(); // lenc=200, solo 3 octetos
                    s.write_all(&pkt(5, &EOF)).unwrap();
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
    match (mysql.query(c, "SELECT nombre, nota FROM alumnos", [])) {{
        Result.Ok(rows) => {{
            var i = 0;
            while (i < rows.len()) {{
                print(rows[i].join("|"));
                i = i + 1;
            }}
        }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    match (mysql.exec(c, "INSERT INTO alumnos VALUES (1)", [])) {{
        Result.Ok(n) => {{ print("afectadas: " + to_string(n)); }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    match (mysql.query(c, "BOOM", [])) {{
        Result.Ok(_) => {{ print("no debería"); }},
        Result.Err(e) => {{ print(e); }},
    }}
    // M77: una fila con un length-encoded truncado se rechaza como valor, sin reventar el cliente.
    match (mysql.query(c, "TRUNC", [])) {{
        Result.Ok(_) => {{ print("no debería (trunc)"); }},
        Result.Err(_) => {{ print("trunc rechazado"); }},
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

const ESPERADO: &str = "ada|36\ngrace|\nafectadas: 3\nmysql: la tabla no existe\ntrunc rechazado\n";

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

// --- TLS: connect_tls (SSLRequest → tls_upgrade → full-path de caching_sha2) ---

/// Atiende una sesión TLS con `caching_sha2_password` forzando el **full-path**: handshake en
/// claro → SSLRequest (verificado octeto a octeto) → TLS → respuesta completa cifrada →
/// AuthMoreData(full auth) → contraseña EN CLARO por el canal cifrado (verificada) → OK → comandos.
fn atender_tls(mut s: TcpStream, config: std::sync::Arc<rustls::ServerConfig>) {
    s.write_all(&pkt(0, &handshake_v10_plugin("caching_sha2_password"))).unwrap();
    // SSLRequest: el prefijo de la respuesta (32 octetos), con CLIENT_SSL encendido.
    let (_seq, ssl_req) = read_pkt(&mut s);
    assert_eq!(ssl_req.len(), 32, "longitud del SSLRequest");
    let caps = u32::from_le_bytes([ssl_req[0], ssl_req[1], ssl_req[2], ssl_req[3]]);
    assert_ne!(caps & 2048, 0, "CLIENT_SSL debe estar encendido");
    // El mismo socket sube a TLS; el resto de la sesión va cifrado.
    let mut conn = rustls::ServerConnection::new(config).expect("server conn");
    let mut tls = rustls::Stream::new(&mut conn, &mut s);
    let (_seq2, resp) = read_pkt(&mut tls);
    let mut i = 32; // capacidades(4) + max(4) + charset(1) + reservado(23)
    let user_start = i;
    while resp[i] != 0 {
        i += 1;
    }
    let user = String::from_utf8_lossy(&resp[user_start..i]).into_owned();
    // Full auth: se ignora la respuesta al scramble (la caché está "fría") y se exige la
    // contraseña en claro — solo posible porque el canal ya es TLS.
    tls.write_all(&pkt(3, &[1, 4])).unwrap(); // AuthMoreData + full-auth
    let (_seq3, pw) = read_pkt(&mut tls);
    if user != "raylang" || pw != b"secret\0" {
        let mut err = vec![0xffu8, 0x15, 0x04];
        err.extend_from_slice(b"#28000acceso denegado");
        tls.write_all(&pkt(5, &err)).unwrap();
        return;
    }
    tls.write_all(&pkt(5, &[0x00, 0, 0, 0, 0, 0, 0])).unwrap(); // OK
    fase_comandos(&mut tls);
    conn.send_close_notify();
    let _ = conn.complete_io(&mut s);
}

fn lanzar_servidor_tls() -> u16 {
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
        for s in listener.incoming().flatten() {
            atender_tls(s, config.clone());
        }
    });
    port
}

/// Cliente TLS: conexión buena (full-path completo) + contraseña mala (ERR sobre el canal cifrado).
fn proyecto_tls(base: &std::path::Path, port: u16) -> std::path::PathBuf {
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
    var c = match (mysql.connect_tls("localhost", {port}, "raylang", "secret", "")) {{
        Result.Ok(conn) => conn,
        Result.Err(e) => {{ print(e); return 1; }},
    }};
    print("conectado seguro");
    match (mysql.query(c, "SELECT nombre, nota FROM alumnos", [])) {{
        Result.Ok(rows) => {{
            var i = 0;
            while (i < rows.len()) {{
                print(rows[i].join("|"));
                i = i + 1;
            }}
        }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    mysql.disconnect(c);
    // Contraseña mala: el full-path la manda por TLS y el servidor la rechaza.
    match (mysql.connect_tls("localhost", {port}, "raylang", "malaclave", "")) {{
        Result.Ok(_) => {{ print("no debería"); return 1; }},
        Result.Err(e) => {{ print(e); }},
    }}
    0
}}
"#
    );
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

fn correr_tls(app: &std::path::Path, flags: &[&str]) -> String {
    let ca = format!("{}/tests/fixtures/tls_ca.pem", env!("CARGO_MANIFEST_DIR"));
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    let out = Command::new(BIN)
        .args(&args)
        .current_dir(app)
        .env("SSL_CERT_FILE", &ca)
        .output()
        .expect("lanza el binario");
    assert!(
        out.status.success(),
        "corre sin error\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const ESPERADO_TLS: &str = "conectado seguro\nada|36\ngrace|\nmysql: acceso denegado\n";

#[test]
fn mysql_tls_full_path_de_caching_sha2() {
    let base = std::env::temp_dir().join("ray_mysql_cli_tls");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let port = lanzar_servidor_tls();
    let app = proyecto_tls(&base, port);

    assert_eq!(correr_tls(&app, &[]), ESPERADO_TLS, "VM");
    assert_eq!(correr_tls(&app, &["--interp"]), ESPERADO_TLS, "intérprete");
}

// --- Protocolo binario (prepared statements) ---

/// Cliente del protocolo binario: SELECT con parámetro (eco + tipos LONGLONG/DOUBLE/DATETIME +
/// NULLs por bitmap), INSERT con parámetro, y un error del servidor en el execute.
fn proyecto_binario(base: &std::path::Path, port: u16) -> std::path::PathBuf {
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
    // SELECT preparado: el servidor devuelve el parámetro como primera celda (el binding fluye)
    // y tipos binarios de verdad (LONGLONG con signo, DOUBLE, DATETIME, NULLs por bitmap).
    match (mysql.query(c, "SELECT nombre, nota, media, creado FROM t WHERE nombre = ?", ["eco"])) {{
        Result.Ok(rows) => {{
            var i = 0;
            while (i < rows.len()) {{
                print(rows[i].join("|"));
                i = i + 1;
            }}
        }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    // INSERT preparado.
    match (mysql.exec(c, "INSERT INTO t (nombre) VALUES (?)", ["x"])) {{
        Result.Ok(n) => {{ print("afectadas: " + to_string(n)); }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    // Error del servidor en el execute → valor.
    match (mysql.query(c, "BOOM ?", ["x"])) {{
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

const ESPERADO_BIN: &str = "eco|-5|2.5|2026-07-09 12:34:56\n\
fija|||0000-00-00 00:00:00\n\
afectadas: 4\n\
mysql: la tabla no existe\n";

#[test]
fn mysql_protocolo_binario_prepared() {
    let base = std::env::temp_dir().join("ray_mysql_cli_bin");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let port = lanzar_servidor();
    let app = proyecto_binario(&base, port);

    let (out_vm, _) = correr(&app, &[]);
    assert_eq!(out_vm, ESPERADO_BIN, "VM");
    let (out_interp, _) = correr(&app, &["--interp"]);
    assert_eq!(out_interp, ESPERADO_BIN, "intérprete");
}
