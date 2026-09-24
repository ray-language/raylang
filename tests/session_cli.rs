//! G.2 (IDEAS §48 G-rev) — sesiones del framework web (`sessions`/`session_of`/`session_get`/
//! `session_put`): cookie `ray_session` + store compartido por actor (std/kv). El diferenciador
//! de DX: bajo `ray dev` (RAY_DEV_RELOAD) la sesión SOBREVIVE al restart del servidor; en
//! producción vive en memoria y nunca toca disco. Servidor concurrente → solo VM, por
//! subproceso con un proyecto consumidor real (patrón `framework_cli`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// El main.ray del proyecto consumidor: /put guarda `v` en la sesión, /get la lee.
const MAIN: &str = r#"from web/framework import new_app, GET, listen, text, App, Ctx, Res, sessions, session_get, session_put, Sessions, query;

fn main() -> int {
    let sess = match (sessions("dev-sessions.rkv")) {
        Result.Ok(s) => s,
        Result.Err(e) => {
            print("sessions: " + e);
            return 1;
        },
    };
    let r = listen(fn() -> App {
        var app = new_app();
        app.GET("/put", fn(c: Ctx, r: Res) {
            session_put(sess, c, r, "name", query(c, "v"));
            r.text("stored");
        });
        app.GET("/get", fn(c: Ctx, r: Res) {
            r.text("name=" + session_get(sess, c, r, "name"));
        });
        app
    }, "127.0.0.1", 0);
    match (r) {
        Result.Ok(n) => 0,
        Result.Err(e) => {
            print("listen: " + e);
            1
        },
    }
}
"#;

/// Monta (una vez) el proyecto consumidor en `dir` y lanza el servidor con las env extra dadas.
/// Devuelve el proceso + el puerto ("listening on port N").
fn launch(dir: &std::path::Path, envs: &[(&str, &str)]) -> (Child, u16) {
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::create_dir_all(dir).expect("crea dir");
    std::fs::write(dir.join("main.ray"), MAIN).expect("escribe main");
    std::fs::write(
        dir.join("ray.toml"),
        format!(
            "[package]\nname = \"session-test\"\nversion = \"0.1.0\"\nentry = \"main.ray\"\n\n\
             [dependencies]\nweb = \"path:{root}/packages/web\"\nnet = \"path:{root}/packages/net\"\n"
        ),
    )
    .expect("escribe ray.toml");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.args(["run", "main.ray"]).current_dir(dir).stdout(Stdio::piped()).stderr(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("lanza servidor");

    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).expect("lee port");
    let port: u16 = line
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no se pudo leer el port de: {line:?}"));
    // Drenar stdout en un hilo (broken pipe si el read-end se cierra).
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = reader.read_to_end(&mut sink);
    });
    (child, port)
}

/// Petición HTTP cruda; devuelve la respuesta completa.
fn ask(port: u16, req: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(req.as_bytes()).expect("envía");
    let mut bytes = Vec::new();
    let _ = s.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Extrae el par `ray_session=<id>` de la línea Set-Cookie de una respuesta.
fn session_cookie(resp: &str) -> String {
    let line = resp
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("set-cookie: ray_session="))
        .unwrap_or_else(|| panic!("sin Set-Cookie de sesión: {resp}"));
    let v = &line["set-cookie: ".len()..];
    v.split(';').next().unwrap().trim().to_string()
}

/// Directorio único por test (los tests corren en paralelo).
fn test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_session_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Modo dev (RAY_DEV_RELOAD): la primera respuesta pone la cookie, la sesión se lee con
/// ella, y — el diferenciador — SOBREVIVE al restart del servidor (archivo RKV1).
#[test]
fn session_survives_reload_in_dev() {
    let dir = test_dir("dev");
    let (mut child, port) = launch(&dir, &[("RAY_DEV_RELOAD", "1")]);

    // put: guarda y estrena la cookie de sesión.
    let r = ask(port, "GET /put?v=roberto HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("200 OK") && r.contains("stored"), "put: {r}");
    let cookie = session_cookie(&r);
    assert!(r.contains("HttpOnly"), "cookie HttpOnly: {r}");

    // get con la cookie ve el valor; sin cookie no (y estrena otra sesión).
    let g = ask(port, &format!("GET /get HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"));
    assert!(g.contains("name=roberto"), "get con cookie: {g}");
    let anon = ask(port, "GET /get HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(anon.contains("name=\r\n") || anon.ends_with("name="), "get sin cookie vacío: {anon}");

    // El "reload": matar el servidor y relanzar sobre el MISMO directorio.
    child.kill().ok();
    child.wait().ok();
    assert!(dir.join("dev-sessions.rkv").exists(), "el estado se persistió");
    let (mut child2, port2) = launch(&dir, &[("RAY_DEV_RELOAD", "1")]);
    let g2 = ask(port2, &format!("GET /get HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"));
    assert!(g2.contains("name=roberto"), "la sesión sobrevive al reload: {g2}");

    child2.kill().ok();
    child2.wait().ok();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Modo producción (sin RAY_DEV_RELOAD): la sesión funciona en memoria y NUNCA toca disco.
#[test]
fn session_in_memory_in_production() {
    let dir = test_dir("prod");
    let (mut child, port) = launch(&dir, &[]);

    let r = ask(port, "GET /put?v=x HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let cookie = session_cookie(&r);
    let g = ask(port, &format!("GET /get HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"));
    assert!(g.contains("name=x"), "get con cookie: {g}");
    assert!(!dir.join("dev-sessions.rkv").exists(), "producción no escribe a disco");

    // M289: el id son 32 hex (128 bits del CSPRNG), la cookie lleva SameSite=Lax y, sin proxy
    // HTTPS, NO lleva Secure.
    let sid = &cookie["ray_session=".len()..];
    assert_eq!(sid.len(), 32, "id de 32 hex: {sid}");
    assert!(sid.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "hex minúsculas: {sid}");
    let set_cookie = r.lines().find(|l| l.to_ascii_lowercase().starts_with("set-cookie: ray_session=")).unwrap();
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Lax"), "flags: {set_cookie}");
    assert!(!set_cookie.contains("Secure"), "sin proxy https no hay Secure: {set_cookie}");

    child.kill().ok();
    child.wait().ok();
    let _ = std::fs::remove_dir_all(&dir);
}

/// M289: una cookie con un id que NO emitió el framework (session fixation) se ignora — el
/// servidor estrena otra sesión y el valor que el atacante eligió no ve nada. Y detrás de un
/// proxy que anuncia HTTPS (`X-Forwarded-Proto`), la cookie sale con `Secure`.
#[test]
fn forged_session_cookie_is_ignored_and_secure_behind_https_proxy() {
    let dir = test_dir("forged");
    let (mut child, port) = launch(&dir, &[]);

    // Fijación: el cliente trae `ray_session=evil` y guarda algo.
    let r = ask(port, "GET /put?v=fixed HTTP/1.1\r\nHost: x\r\nCookie: ray_session=evil\r\nConnection: close\r\n\r\n");
    let issued = session_cookie(&r);
    assert_ne!(issued, "ray_session=evil", "el id forjado no se adopta: {r}");
    // Con el id forjado no hay nada; con el emitido, sí.
    let g = ask(port, "GET /get HTTP/1.1\r\nHost: x\r\nCookie: ray_session=evil\r\nConnection: close\r\n\r\n");
    assert!(g.contains("name=\r\n") || g.ends_with("name="), "el id forjado no ve la sesión: {g}");
    let g = ask(port, &format!("GET /get HTTP/1.1\r\nHost: x\r\nCookie: {issued}\r\nConnection: close\r\n\r\n"));
    assert!(g.contains("name=fixed"), "el id emitido sí: {g}");
    // Un id con la forma correcta pero mayúsculas tampoco se adopta (forma canónica estricta).
    let upper = issued.to_ascii_uppercase();
    let r = ask(port, &format!("GET /put?v=z HTTP/1.1\r\nHost: x\r\nCookie: {upper}\r\nConnection: close\r\n\r\n"));
    assert_ne!(session_cookie(&r), upper, "mayúsculas no es un id nuestro");

    // Secure detrás de un proxy HTTPS.
    let r = ask(port, "GET /put?v=s HTTP/1.1\r\nHost: x\r\nX-Forwarded-Proto: https\r\nConnection: close\r\n\r\n");
    let set_cookie = r.lines().find(|l| l.to_ascii_lowercase().starts_with("set-cookie: ray_session=")).unwrap();
    assert!(set_cookie.contains("; Secure"), "Secure tras proxy https: {set_cookie}");

    child.kill().ok();
    child.wait().ok();
    let _ = std::fs::remove_dir_all(&dir);
}
