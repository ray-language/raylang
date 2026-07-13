//! Prueba del micro-framework web (`examples/web/framework.ray`): enrutado, parámetros de ruta, middleware
//! y respuestas. El servidor es concurrente (cede fibras) → **solo VM** y no determinista para el
//! oráculo, así que se prueba por subproceso + un cliente HTTP en Rust (como `webserver_cli`). Se copia
//! el framework (con puerto efímero) y su dependencia `webserver.ray` a un temporal y se lanza con `--vm`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Copia `framework.ray` (puerto 8080 → 0, efímero) y `webserver.ray` a un temporal, lo lanza con `--vm`
/// y devuelve el proceso + el puerto que imprime ("escuchando en el puerto N").
fn launch() -> (Child, u16) {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut dir = std::env::temp_dir();
    dir.push("ray_framework");
    std::fs::create_dir_all(&dir).expect("crea dir");
    // El framework es una librería (sin `main`): se prueba a través de su demo, que lo IMPORTA. Se
    // copian los tres archivos (webserver ← framework ← framework_demo) al temporal.
    std::fs::copy(format!("{root}/examples/web/webserver.ray"), dir.join("webserver.ray")).expect("copia webserver");
    std::fs::copy(format!("{root}/examples/web/framework.ray"), dir.join("framework.ray")).expect("copia framework");
    let demo = std::fs::read_to_string(format!("{root}/examples/web/framework_demo.ray")).expect("lee demo");
    std::fs::write(dir.join("framework_demo.ray"), demo.replace("8080", "0")).expect("escribe demo");

    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm").arg(dir.join("framework_demo.ray"))
        .stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("lanza framework");

    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee port");
    // "escuchando en el puerto N" → el último token es el puerto.
    let port: u16 = linea.trim().rsplit(' ').next().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no se pudo leer el port de: {linea:?}"));

    // El framework loguea cada petición a stdout (middleware). Hay que DRENAR ese stdout en un hilo: si
    // no, al cerrarse el read-end del pipe el `println` del servidor falla con "broken pipe" y aborta.
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

    // Ruta raíz.
    let r = ask(port, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("200 OK"), "GET /: {r}");
    assert!(r.contains("micro-framework"), "GET / body: {r}");

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

    // Estado a medida encadenado.
    let r = ask(port, "GET /teapot HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("418"), "GET /teapot estado: {r}");
    assert!(r.contains("tetera"), "GET /teapot body: {r}");

    // Ruta inexistente → 404.
    let r = ask(port, "GET /nope HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("404"), "GET /nope: {r}");

    let _ = child.kill();
    let _ = child.wait();
}
