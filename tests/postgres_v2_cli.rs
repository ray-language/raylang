//! M53.2 — cliente PostgreSQL v2 (`packages/db/postgres.ray`): conexión persistente + protocolo
//! **extendido** (Parse/Bind/Describe/Execute/Sync) con **parámetros**. El cliente autentica con
//! SCRAM-SHA-256 (valores PRECOMPUTADOS, como `postgres_cli.rs` → sin cripto en Rust) contra un
//! **servidor PostgreSQL de juguete** que parsea el mensaje Bind, **extrae los parámetros** y los
//! **devuelve** en la primera fila (prueba que el binding funciona: anti-inyección) y sirve una
//! segunda fila fija; para exec responde un CommandComplete con filas afectadas; para "BOOM", un
//! ErrorResponse. Oráculo conductual en ambos motores (mismo stdout).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

// server-first/server-final PRECOMPUTADOS para user=raylang, pw=secret, nonce=clientnonce123456, i=64.
const SERVER_FIRST: &[u8] = &[
    114, 61, 99, 108, 105, 101, 110, 116, 110, 111, 110, 99, 101, 49, 50, 51, 52, 53, 54, 115, 101,
    114, 118, 101, 114, 110, 111, 110, 99, 101, 55, 56, 57, 44, 115, 61, 65, 81, 73, 68, 66, 65, 85,
    71, 66, 119, 103, 74, 67, 103, 115, 77, 68, 81, 52, 80, 69, 65, 61, 61, 44, 105, 61, 54, 52,
];
const SERVER_FINAL: &[u8] = &[
    118, 61, 80, 87, 88, 53, 71, 72, 119, 87, 43, 66, 120, 108, 101, 99, 70, 122, 79, 56, 116, 105,
    79, 81, 69, 86, 111, 98, 86, 117, 43, 54, 80, 66, 103, 56, 55, 49, 110, 109, 121, 101, 81, 51, 52, 61,
];

/// Un mensaje con octeto de tipo: `[tipo][longitud=4+len][carga]`.
fn msg(t: u8, payload: &[u8]) -> Vec<u8> {
    let n = 4 + payload.len();
    let mut m = vec![t, (n >> 24) as u8, (n >> 16) as u8, (n >> 8) as u8, n as u8];
    m.extend_from_slice(payload);
    m
}

fn read_startup<S: Read + Write>(s: &mut S) {
    let mut hdr = [0u8; 4];
    s.read_exact(&mut hdr).unwrap();
    let len = u32::from_be_bytes(hdr) as usize;
    let mut rest = vec![0u8; len - 4];
    s.read_exact(&mut rest).unwrap();
}

/// Lee un mensaje con tipo: devuelve (tipo, carga).
fn read_typed<S: Read + Write>(s: &mut S) -> (u8, Vec<u8>) {
    let mut hdr = [0u8; 5];
    s.read_exact(&mut hdr).unwrap();
    let len = u32::from_be_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]) as usize;
    let mut payload = vec![0u8; len - 4];
    s.read_exact(&mut payload).unwrap();
    (hdr[0], payload)
}

/// Extrae (query, params) de un ciclo Parse/Bind/…/Sync. La query viene del Parse ('P'), los
/// parámetros del Bind ('B'). Devuelve al ver Sync ('S'); None si el cliente terminó ('X').
fn leer_ciclo<S: Read + Write>(s: &mut S) -> Option<(String, Vec<String>)> {
    let mut query = String::new();
    let mut params: Vec<String> = Vec::new();
    loop {
        let (t, p) = read_typed(s);
        match t {
            b'P' => {
                // Parse: [stmt NUL][query NUL][int16 ntypes]…
                let mut i = 0;
                while p[i] != 0 {
                    i += 1;
                }
                i += 1;
                let start = i;
                while p[i] != 0 {
                    i += 1;
                }
                query = String::from_utf8_lossy(&p[start..i]).into_owned();
            }
            b'B' => {
                // Bind: [portal NUL][stmt NUL][int16 nfmt][fmts][int16 nparams][params]…
                let mut i = 0;
                while p[i] != 0 {
                    i += 1;
                }
                i += 1;
                while p[i] != 0 {
                    i += 1;
                }
                i += 1;
                let nfmt = u16::from_be_bytes([p[i], p[i + 1]]) as usize;
                i += 2 + nfmt * 2;
                let nparams = u16::from_be_bytes([p[i], p[i + 1]]) as usize;
                i += 2;
                for _ in 0..nparams {
                    let len = i32::from_be_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]);
                    i += 4;
                    if len < 0 {
                        params.push(String::new());
                    } else {
                        let l = len as usize;
                        params.push(String::from_utf8_lossy(&p[i..i + l]).into_owned());
                        i += l;
                    }
                }
            }
            b'S' => return Some((query, params)),
            b'X' => return None,
            _ => {} // Describe 'D', Execute 'E': se ignoran
        }
    }
}

/// RowDescription ('T') con `ncols` columnas de nombre `colN` (metadatos a cero).
fn row_description(ncols: usize) -> Vec<u8> {
    let mut td = vec![(ncols >> 8) as u8, ncols as u8];
    for k in 0..ncols {
        td.extend_from_slice(format!("col{k}").as_bytes());
        td.push(0);
        td.extend_from_slice(&[0u8; 18]);
    }
    td
}

/// DataRow ('D') con columnas de texto.
fn data_row(cols: &[String]) -> Vec<u8> {
    let mut dr = vec![(cols.len() >> 8) as u8, cols.len() as u8];
    for c in cols {
        dr.extend_from_slice(&(c.len() as u32).to_be_bytes());
        dr.extend_from_slice(c.as_bytes());
    }
    dr
}

/// DataRow ('D') con columnas que pueden ser NULL (`None` → longitud -1 = 0xFFFFFFFF).
fn data_row_opt(cols: &[Option<String>]) -> Vec<u8> {
    let mut dr = vec![(cols.len() >> 8) as u8, cols.len() as u8];
    for c in cols {
        match c {
            Some(v) => {
                dr.extend_from_slice(&(v.len() as u32).to_be_bytes());
                dr.extend_from_slice(v.as_bytes());
            }
            None => dr.extend_from_slice(&(-1i32).to_be_bytes()), // marcador NULL
        }
    }
    dr
}

fn command_complete(tag: &str) -> Vec<u8> {
    let mut t = tag.as_bytes().to_vec();
    t.push(0);
    t
}

/// Atiende una sesión: startup, SCRAM (precomputado), y el protocolo extendido por ciclo.
fn handle(mut s: TcpStream) {
    handle_stream(&mut s);
}

/// La sesión en sí, genérica sobre el flujo (TCP plano o rustls::Stream → sirve para el test TLS).
fn handle_stream<S: Read + Write>(s: &mut S) {
    read_startup(s);
    let mut sasl = vec![0u8, 0, 0, 10];
    sasl.extend_from_slice(b"SCRAM-SHA-256\0");
    sasl.push(0);
    s.write_all(&msg(b'R', &sasl)).unwrap();
    read_typed(&mut *s); // SASLInitialResponse
    let mut cont = vec![0u8, 0, 0, 11];
    cont.extend_from_slice(SERVER_FIRST);
    s.write_all(&msg(b'R', &cont)).unwrap();
    read_typed(&mut *s); // SASLResponse
    let mut fin = vec![0u8, 0, 0, 12];
    fin.extend_from_slice(SERVER_FINAL);
    s.write_all(&msg(b'R', &fin)).unwrap();
    s.write_all(&msg(b'R', &[0, 0, 0, 0])).unwrap(); // AuthenticationOk
    s.write_all(&msg(b'Z', b"I")).unwrap(); // ReadyForQuery

    while let Some((query, params)) = leer_ciclo(&mut *s) {
        s.write_all(&msg(b'1', &[])).unwrap(); // ParseComplete
        s.write_all(&msg(b'2', &[])).unwrap(); // BindComplete
        if query.starts_with("SELECT") {
            // Primera fila = los parámetros (prueba el binding); segunda fila fija.
            let ncols = params.len().max(1);
            s.write_all(&msg(b'T', &row_description(ncols))).unwrap();
            let mut fila1 = params.clone();
            if fila1.is_empty() {
                fila1.push("sin-params".to_string());
            }
            s.write_all(&msg(b'D', &data_row(&fila1))).unwrap();
            let fixes: Vec<String> = (0..ncols).map(|k| format!("fixes{k}")).collect();
            s.write_all(&msg(b'D', &data_row(&fixes))).unwrap();
            s.write_all(&msg(b'C', &command_complete("SELECT 2"))).unwrap();
        } else if query.starts_with("NULLTEST") {
            // Fila con una columna NULL (longitud -1): antes reventaba el cliente (be32 sin signo).
            s.write_all(&msg(b'T', &row_description(2))).unwrap();
            s.write_all(&msg(b'D', &data_row_opt(&[Some("hello".to_string()), None]))).unwrap();
            s.write_all(&msg(b'C', &command_complete("SELECT 1"))).unwrap();
        } else if query.starts_with("BOOM") {
            // ErrorResponse: campos [tipo][valor NUL]… 'M' = mensaje.
            let mut e = Vec::new();
            e.push(b'M');
            e.extend_from_slice(b"relacion nonexistent\0");
            e.push(0);
            s.write_all(&msg(b'E', &e)).unwrap();
        } else if query.starts_with("INSERT") {
            s.write_all(&msg(b'C', &command_complete("INSERT 0 5"))).unwrap();
        } else {
            // BEGIN/COMMIT/otros: tag sin número → 0 afectadas.
            s.write_all(&msg(b'C', &command_complete(&query))).unwrap();
        }
        s.write_all(&msg(b'Z', b"I")).unwrap(); // ReadyForQuery
        let _ = s.flush();
    }
}

fn launch_servidor() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for s in listener.incoming().flatten() {
            handle(s);
        }
    });
    port
}

/// Variante TLS: espera el **SSLRequest** del protocolo (8 octetos, código 80877103), responde
/// 'S', hace el handshake TLS (cert autofirmado de `tests/fixtures/`) y atiende LA MISMA sesión
/// (startup + SCRAM + extendido) sobre el canal cifrado.
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
            // Fase en claro: el SSLRequest exacto.
            let mut req = [0u8; 8];
            s.read_exact(&mut req).expect("lee SSLRequest");
            assert_eq!(u32::from_be_bytes([req[0], req[1], req[2], req[3]]), 8, "length del SSLRequest");
            assert_eq!(u32::from_be_bytes([req[4], req[5], req[6], req[7]]), 80877103, "código del SSLRequest");
            s.write_all(b"S").expect("responde S");
            // Fase TLS: la misma sesión de siempre, sobre el canal cifrado.
            let mut conn = rustls::ServerConnection::new(config.clone()).expect("server conn");
            let mut tls = rustls::Stream::new(&mut conn, &mut s);
            handle_stream(&mut tls);
            conn.send_close_notify();
            let _ = conn.complete_io(&mut s);
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
        r#"import db/postgres;

fn main() -> int {{
    var c = match (postgres.connect("127.0.0.1", {port}, "raylang", "secret", "test", "clientnonce123456")) {{
        Result.Ok(conn) => conn,
        Result.Err(e) => {{ print(e); return 1; }},
    }};
    // query con parámetros: el servidor los devuelve como primera fila.
    match (postgres.query(c, "SELECT * FROM t WHERE a = $1 AND b = $2", ["ada", "36"])) {{
        Result.Ok(rows) => {{
            var i = 0;
            while (i < rows.len()) {{
                print(rows[i].join("|"));
                i = i + 1;
            }}
        }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    // transacción + exec con parámetro.
    match (postgres.exec(c, "BEGIN", [])) {{
        Result.Ok(n) => {{ print("begin: " + to_string(n)); }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    match (postgres.exec(c, "INSERT INTO t VALUES ($1)", ["z"])) {{
        Result.Ok(n) => {{ print("insert: " + to_string(n)); }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    // fila con columna NULL: llega como "" (antes reventaba el cliente).
    match (postgres.query(c, "NULLTEST", [])) {{
        Result.Ok(rows) => {{ print("null: [" + rows[0].join("|") + "]"); }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    // error del servidor.
    match (postgres.query(c, "BOOM", [])) {{
        Result.Ok(_) => {{ print("no debería"); }},
        Result.Err(e) => {{ print(e); }},
    }}
    postgres.disconnect(c);
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

const ESPERADO: &str = "ada|36\nfixes0|fixes1\nbegin: 0\ninsert: 5\nnull: [hello|]\npostgres: relacion nonexistent\n";

#[test]
fn postgres_v2_extendido_params_y_transaccion() {
    let base = std::env::temp_dir().join("ray_pg_v2_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let port = launch_servidor();
    let app = project(&base, port);

    // VM (motor de producto) e intérprete (oráculo): mismo stdout exacto.
    assert_eq!(run(&app, &[]), ESPERADO, "VM");
    assert_eq!(run(&app, &["--interp"]), ESPERADO, "intérprete");
}

// --- TLS: connect_tls (sslRequest → 'S' → tls_upgrade → misma sesión cifrada) ---

/// El mismo programa de prueba pero con `connect_tls` contra "localhost" (el nombre del cert).
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
        r#"import db/postgres;

fn main() -> int {{
    var c = match (postgres.connect_tls("localhost", {port}, "raylang", "secret", "test", "clientnonce123456")) {{
        Result.Ok(conn) => conn,
        Result.Err(e) => {{ print(e); return 1; }},
    }};
    match (postgres.query(c, "SELECT * FROM t WHERE a = $1", ["segura"])) {{
        Result.Ok(rows) => {{
            var i = 0;
            while (i < rows.len()) {{
                print(rows[i].join("|"));
                i = i + 1;
            }}
        }},
        Result.Err(e) => {{ print(e); return 1; }},
    }}
    postgres.disconnect(c);
    0
}}
"#
    );
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

/// Como `correr` pero confiando en la CA de prueba (que firmó el cert de "localhost").
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

const ESPERADO_TLS: &str = "segura\nfixes0\n";

#[test]
fn postgres_tls_sslrequest_y_sesion_cifrada() {
    let base = std::env::temp_dir().join("ray_pg_v2_cli_tls");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let port = launch_servidor_tls();
    let app = project_tls(&base, port);

    assert_eq!(run_tls(&app, &[]), ESPERADO_TLS, "VM");
    assert_eq!(run_tls(&app, &["--interp"]), ESPERADO_TLS, "intérprete");
}
