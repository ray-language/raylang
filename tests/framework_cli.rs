//! Prueba del micro-framework web (`examples/framework.ray`): enrutado, parámetros de ruta, middleware
//! y respuestas. El servidor es concurrente (cede fibras) → **solo VM** y no determinista para el
//! oráculo, así que se prueba por subproceso + un cliente HTTP en Rust (como `webserver_cli`). Se copia
//! el framework (con puerto efímero) y su dependencia `webserver.ray` a un temporal y se lanza con `--vm`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Copia `framework.ray` (puerto 8080 → 0, efímero) y `webserver.ray` a un temporal, lo lanza con `--vm`
/// y devuelve el proceso + el puerto que imprime ("escuchando en el puerto N").
fn lanzar() -> (Child, u16) {
    let raiz = env!("CARGO_MANIFEST_DIR");
    let mut dir = std::env::temp_dir();
    dir.push("ray_framework");
    std::fs::create_dir_all(&dir).expect("crea dir");
    std::fs::copy(format!("{raiz}/examples/webserver.ray"), dir.join("webserver.ray")).expect("copia webserver");
    let fw = std::fs::read_to_string(format!("{raiz}/examples/framework.ray")).expect("lee framework");
    std::fs::write(dir.join("framework.ray"), fw.replace("8080", "0")).expect("escribe framework");

    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm").arg(dir.join("framework.ray"))
        .stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("lanza framework");

    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee puerto");
    // "escuchando en el puerto N" → el último token es el puerto.
    let port: u16 = linea.trim().rsplit(' ').next().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no se pudo leer el puerto de: {linea:?}"));

    // El framework loguea cada petición a stdout (middleware). Hay que DRENAR ese stdout en un hilo: si
    // no, al cerrarse el read-end del pipe el `println` del servidor falla con "broken pipe" y aborta.
    std::thread::spawn(move || {
        let mut sumidero = Vec::new();
        let _ = reader.read_to_end(&mut sumidero);
    });
    (child, port)
}

/// Envía una petición HTTP cruda y devuelve la respuesta completa (hasta que el servidor cierra).
fn pedir(port: u16, req: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(req.as_bytes()).expect("envía");
    let mut bytes = Vec::new();
    let _ = s.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn framework_enruta_params_middleware_y_404() {
    let (mut child, port) = lanzar();

    // Ruta raíz.
    let r = pedir(port, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("200 OK"), "GET /: {r}");
    assert!(r.contains("micro-framework"), "GET / cuerpo: {r}");

    // Parámetro de ruta + JSON.
    let r = pedir(port, "GET /users/42 HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("application/json"), "tipo JSON: {r}");
    assert!(r.contains("\"id\": \"42\""), "param id capturado: {r}");
    assert!(r.contains("usuario-42"), "param interpolado: {r}");

    // POST con cuerpo (eco).
    let r = pedir(port, "POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 8\r\nConnection: close\r\n\r\neco esto");
    assert!(r.contains("200 OK"), "POST /echo estado: {r}");
    assert!(r.ends_with("eco esto"), "POST /echo eco: {r}");

    // Estado a medida encadenado.
    let r = pedir(port, "GET /teapot HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("418"), "GET /teapot estado: {r}");
    assert!(r.contains("tetera"), "GET /teapot cuerpo: {r}");

    // Ruta inexistente → 404.
    let r = pedir(port, "GET /nope HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(r.contains("404"), "GET /nope: {r}");

    let _ = child.kill();
    let _ = child.wait();
}
