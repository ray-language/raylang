//! Prueba del endpoint `/metrics` real (M21.3, `examples/web/metrics_server_demo.ray`): `metrics.ray`
//! montado sobre `webserver.ray`. El servidor es concurrente (cede fibras) → **solo VM**, no
//! determinista para el oráculo → subproceso + cliente HTTP en Rust. Se hacen varias peticiones para
//! generar métricas y luego se escrapea `/metrics`, validando el formato de exposición de Prometheus.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Copia webserver.ray + metrics.ray + el demo (puerto 8080 → 0 efímero) a un temporal, lo lanza con
/// `--vm` y devuelve el proceso + el puerto.
fn lanzar() -> (Child, u16) {
    let raiz = env!("CARGO_MANIFEST_DIR");
    let mut dir = std::env::temp_dir();
    dir.push("ray_metrics_server");
    std::fs::create_dir_all(&dir).expect("crea dir");
    for lib in ["webserver.ray", "metrics.ray"] {
        std::fs::copy(format!("{raiz}/examples/web/{lib}"), dir.join(lib)).unwrap_or_else(|_| panic!("copia {lib}"));
    }
    let demo = std::fs::read_to_string(format!("{raiz}/examples/web/metrics_server_demo.ray")).expect("lee demo");
    std::fs::write(dir.join("metrics_server_demo.ray"), demo.replace("8080", "0")).expect("escribe demo");

    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(dir.join("metrics_server_demo.ray"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("lanza servidor de métricas");

    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut linea = String::new();
    reader.read_line(&mut linea).expect("lee puerto");
    let port: u16 = linea.trim().rsplit(' ').next().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no se pudo leer el puerto de: {linea:?}"));
    // Drena el resto del stdout en un hilo (por si el servidor imprimiera algo más).
    std::thread::spawn(move || {
        let mut sumidero = Vec::new();
        let _ = reader.read_to_end(&mut sumidero);
    });
    (child, port)
}

/// Envía una petición HTTP cruda y devuelve la respuesta completa.
fn pedir(port: u16, req: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(req.as_bytes()).expect("envía");
    let mut bytes = Vec::new();
    let _ = s.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn get(port: u16, path: &str) -> String {
    pedir(port, &format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
}

#[test]
fn endpoint_metrics_expone_counters_e_histograma() {
    let (mut child, port) = lanzar();

    // Genera tráfico: dos GET / y un GET /foo (404).
    assert!(get(port, "/").contains("hola desde raylang"), "GET /");
    let _ = get(port, "/");
    assert!(get(port, "/foo").contains("404"), "GET /foo debe ser 404");

    // Escrapea /metrics.
    let m = get(port, "/metrics");
    assert!(m.contains("Content-Type: text/plain"), "tipo de contenido Prometheus: {m}");
    // Counter etiquetado por (método, ruta).
    assert!(m.contains(r#"http_requests_total{method="GET",path="/"} 2"#), "counter / = 2: {m}");
    assert!(m.contains(r#"http_requests_total{method="GET",path="/foo"} 1"#), "counter /foo = 1: {m}");
    assert!(m.contains("# TYPE http_requests_total counter"), "TYPE counter: {m}");
    // Histograma: 3 observaciones de 0.008 (caen en le≥0.01).
    assert!(m.contains(r#"http_request_duracion_segundos_bucket{le="0.005"} 0"#), "bucket 0.005=0: {m}");
    assert!(m.contains(r#"http_request_duracion_segundos_bucket{le="0.01"} 3"#), "bucket 0.01=3: {m}");
    assert!(m.contains("http_request_duracion_segundos_count 3"), "count=3: {m}");
    assert!(m.contains("http_request_duracion_segundos_sum 0.024"), "sum=0.024: {m}");

    let _ = child.kill();
    let _ = child.wait();
}
