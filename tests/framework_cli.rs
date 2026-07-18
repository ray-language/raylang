//! Prueba del framework web `packages/web/framework.ray` (M93, promovido de examples): enrutado,
//! parámetros de ruta, middleware/logging, estáticos con ETag/304, redirect, headers y 404 custom.
//! El servidor es concurrente (cede fibras) → **solo VM** y no determinista para el oráculo, así
//! que se prueba por subproceso + un cliente HTTP en Rust (como `webserver_cli`). El test monta un
//! PROYECTO CONSUMIDOR real en un temporal (ray.toml con path-deps a packages/web y packages/net,
//! como haría un usuario) a partir del demo `examples/web/framework/`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Copia el proyecto demo (`examples/web/framework/`) a un temporal con el puerto 8080 → 0
/// (efímero) y ray.toml apuntando a los paquetes del repo (rutas absolutas), lo lanza con
/// `ray run` y devuelve el proceso + el puerto que imprime ("listening on port N").
fn launch() -> (Child, u16) {
    let root = env!("CARGO_MANIFEST_DIR");
    let dir = std::env::temp_dir().join("ray_framework_pkg");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("static")).expect("crea dir");
    let demo = std::fs::read_to_string(format!("{root}/examples/web/framework/main.ray")).expect("lee demo");
    std::fs::write(dir.join("main.ray"), demo.replace("8080", "0")).expect("escribe main");
    std::fs::copy(
        format!("{root}/examples/web/framework/static/style.css"),
        dir.join("static/style.css"),
    ).expect("copia css");
    std::fs::write(
        dir.join("ray.toml"),
        format!(
            "[package]\nname = \"framework-test\"\nversion = \"0.1.0\"\nentry = \"main.ray\"\n\n\
             [dependencies]\nweb = \"path:{root}/packages/web\"\nnet = \"path:{root}/packages/net\"\n"
        ),
    ).expect("escribe ray.toml");

    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["run", "main.ray"])
        .current_dir(&dir)
        .stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("lanza framework");

    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).expect("lee port");
    // "listening on port N" → el último token es el puerto.
    let port: u16 = line.trim().rsplit(' ').next().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no se pudo leer el port de: {line:?}"));

    // El framework loguea cada petición a stdout (log_requests). Hay que DRENAR ese stdout en un
    // hilo: si no, al cerrarse el read-end del pipe el `print` del servidor aborta con broken pipe.
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = reader.read_to_end(&mut sink);
    });
    (child, port)
}

/// Envía una petición HTTP cruda y devuelve la respuesta completa (hasta que el servidor cierra).
fn ask(port: u16, req: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(req.as_bytes()).expect("envía");
    let mut bytes = Vec::new();
    let _ = s.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn framework_enruta_params_middleware_y_404() {
    let (mut child, port) = launch();

    // Ruta raíz (HTML).
    let r = ask(port, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("200 OK"), "GET /: {r}");
    assert!(r.contains("framework web de raylang"), "GET / body: {r}");
    assert!(r.contains("Content-Type: text/html"), "html(): {r}");

    // Parámetro de ruta + JSON.
    let r = ask(port, "GET /users/42 HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("application/json"), "type JSON: {r}");
    assert!(r.contains("\"id\": \"42\""), "param id capturado: {r}");
    assert!(r.contains("usuario-42"), "param interpolado: {r}");

    // POST con cuerpo (eco).
    let r = ask(port, "POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 8\r\nConnection: close\r\n\r\neco esto");
    assert!(r.contains("200 OK"), "POST /echo estado: {r}");
    assert!(r.ends_with("eco esto"), "POST /echo eco: {r}");

    // M56.2: la query NO forma parte del path → la ruta con :id sigue casando con ?x=1.
    let r = ask(port, "GET /users/42?x=1 HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("\"id\": \"42\""), "path con query must casar: {r}");

    // M56.2: c.query("nombre") lee la query string (decodificada: + = espacio).
    let r = ask(port, "GET /saluda?nombre=Ada+Lovelace HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("hola, Ada Lovelace"), "query param: {r}");
    let r = ask(port, "GET /saluda HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("hola, mundo"), "query param ausente → default: {r}");

    // M56.7: dos cookies = dos líneas Set-Cookie en la respuesta.
    let r = ask(port, "GET /entra HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("Set-Cookie: sesion=abc123; Path=/; HttpOnly"), "1ª cookie: {r}");
    assert!(r.contains("Set-Cookie: flash=hola"), "2ª cookie: {r}");

    // Estado a medida + header custom, encadenados.
    let r = ask(port, "GET /teapot HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("418"), "GET /teapot estado: {r}");
    assert!(r.contains("X-Tetera: si"), "header custom: {r}");
    assert!(r.contains("tetera"), "GET /teapot body: {r}");

    // Ruta inexistente → el 404 PERSONALIZADO (JSON con la ruta).
    let r = ask(port, "GET /nope HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("404"), "GET /nope: {r}");
    assert!(r.contains("\"path\": \"/nope\""), "404 custom con la ruta: {r}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn framework_step_locals_after_y_middleware_por_ruta() {
    // M93.2a: Step.Next/Done, use_on (prefijo), with_mw (por ruta), locals y hooks after.
    let (mut child, port) = launch();

    // use_on("/privado/") corta con Step.Done sin token…
    let r = ask(port, "GET /privado/perfil HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("401"), "sin token → 401: {r}");
    // …y con token sigue (Step.Next); el handler lee el local que dejó el middleware.
    let r = ask(port, "GET /privado/perfil?token=secreto HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("perfil de ada"), "local user visible en el handler: {r}");

    // with_mw: la misma auth como cadena POR RUTA.
    let r = ask(port, "GET /admin HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("401"), "with_mw corta sin token: {r}");
    let r = ask(port, "GET /admin?token=secreto HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("panel de ada"), "with_mw deja pasar con token: {r}");

    // after: la cabecera común está en una ruta normal, en un corte de middleware y en el 404.
    let r = ask(port, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("X-Framework: raylang"), "after en ruta normal: {r}");
    let r = ask(port, "GET /privado/perfil HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("X-Framework: raylang"), "after con cadena cortada: {r}");
    let r = ask(port, "GET /nope HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("X-Framework: raylang"), "after en el 404: {r}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn framework_estaticos_redirect_y_metodos() {
    let (mut child, port) = launch();

    // Estáticos (M56.9) montados bajo /assets/ desde static/: 200 + mime + ETag.
    let r = ask(port, "GET /assets/style.css HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("200 OK") && r.contains("sans-serif"), "css: {r}");
    assert!(r.contains("Content-Type: text/css"), "mime css: {r}");
    let etag = r.lines().find_map(|l| l.strip_prefix("ETag: "))
        .unwrap_or_else(|| panic!("sin ETag: {r}")).trim().to_string();

    // Revalidación: If-None-Match que casa → 304 sin cuerpo.
    let c = ask(port, &format!("GET /assets/style.css HTTP/1.1\r\nHost: x\r\nIf-None-Match: {etag}\r\nConnection: close\r\n\r\n"));
    assert!(c.contains("304 Not Modified"), "304: {c}");
    assert!(!c.contains("sans-serif"), "el 304 no lleva cuerpo: {c}");

    // Traversal bajo el mount → 404 (saneo de M56.9).
    let t = ask(port, "GET /assets/../main.ray HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(t.contains("404"), "traversal: {t}");

    // Redirect: 302 + Location.
    let r = ask(port, "GET /antigua HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("302"), "redirect estado: {r}");
    assert!(r.contains("Location: /"), "redirect Location: {r}");

    // M93.2b: un POST a una ruta que existe como GET → 405 + Allow (RFC 9110; antes era 404).
    let r = ask(port, "POST /teapot HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    assert!(r.contains("405"), "POST a ruta GET → 405: {r}");
    assert!(r.contains("Allow: GET, HEAD"), "Allow con los métodos: {r}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn framework_catchall_all_regex_y_mount() {
    // M93.2b: `*resto`, ALL (método comodín), rutas regex compiladas y sub-apps con mount.
    let (mut child, port) = launch();

    // Catch-all: captura el resto del path, con las "/".
    let r = ask(port, "GET /files/docs/a.txt HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("archivo: docs/a.txt"), "catch-all multi-segmento: {r}");
    let r = ask(port, "GET /files/solo.txt HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("archivo: solo.txt"), "catch-all un segmento: {r}");

    // ALL: la misma ruta atiende GET y POST.
    let r = ask(port, "GET /ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("pong GET"), "ALL con GET: {r}");
    let r = ask(port, "POST /ping HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    assert!(r.contains("pong POST"), "ALL con POST: {r}");

    // Regex: captura numerada; un path que no casa la regex cae al 404.
    let r = ask(port, "GET /v2/estado HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("\"version\": 2"), "captura regex: {r}");
    let r = ask(port, "GET /vx/estado HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("404"), "regex no casa letras: {r}");

    // Mount: la ruta del sub-app queda bajo el prefijo, con los middlewares del grupo (auth).
    let r = ask(port, "GET /api/users/7/rol HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("401"), "mount aplica el middleware del grupo: {r}");
    let r = ask(port, "GET /api/users/7/rol?token=secreto HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("\"id\": \"7\""), "mount re-prefija y captura params: {r}");

    let _ = child.kill();
    let _ = child.wait();
}
