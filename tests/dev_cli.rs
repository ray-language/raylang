//! M92.1 — `ray dev`: modo desarrollo (watcher + reinicio). Se prueba el ciclo completo por
//! subproceso: arranca un proyecto que imprime y termina, se edita el fuente y el supervisor
//! relanza el programa con el código nuevo. La salida del hijo va a un archivo que el test
//! sondea con plazo (el watcher es polling de ~200 ms; los tiempos son holgados para CI).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Sondea `path` hasta que su contenido contenga `needle` (plazo `secs`); devuelve el contenido.
fn wait_for_content(path: &std::path::Path, needle: &str, secs: u64) -> String {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let mut s = String::new();
        if let Ok(mut f) = std::fs::File::open(path) {
            let _ = f.read_to_string(&mut s);
            if s.contains(needle) {
                return s;
            }
        }
        assert!(Instant::now() < deadline, "'{needle}' no apareció en {secs}s; output:\n{s}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn dev_restarts_on_changes() {
    let base = std::env::temp_dir().join("ray_dev_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("ray.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(base.join("src/main.ray"), "fn main() -> int { print(\"v1\"); 0 }\n").unwrap();

    let out_path = base.join("output.txt");
    let out_file = std::fs::File::create(&out_path).unwrap();
    let err_file = out_file.try_clone().unwrap();
    let mut dev = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("dev")
        .current_dir(&base)
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("lanza ray dev");

    // 1) El programa corre al arrancar y, al terminar solo, el supervisor queda a la espera.
    wait_for_content(&out_path, "v1", 10);
    wait_for_content(&out_path, "waiting for changes", 10);

    // 2) Editar el fuente → el watcher lo ve y relanza con el código NUEVO.
    std::thread::sleep(Duration::from_millis(50)); // mtime estrictamente posterior
    std::fs::write(base.join("src/main.ray"), "fn main() -> int { print(\"v2\"); 0 }\n").unwrap();
    let output = wait_for_content(&out_path, "v2", 10);
    assert!(output.contains("restarting"), "anuncia el reinicio:\n{output}");

    let _ = dev.kill();
    let _ = dev.wait();
}

/// Conecta a `127.0.0.1:port`, envía un ping y devuelve la respuesta (reintenta hasta `secs`). El
/// socket lo retiene el supervisor, así que una conexión temprana se ENCOLA en el backlog (nunca
/// rechazada) hasta que un hijo la acepta — justo la propiedad que probamos.
#[cfg(unix)]
fn connect_and_read(port: u16, secs: u64) -> String {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = s.write_all(b"ping");
            let mut buf = String::new();
            if s.read_to_string(&mut buf).is_ok() && !buf.is_empty() {
                return buf;
            }
        }
        assert!(Instant::now() < deadline, "sin respuesta del servidor en {port} tras {secs}s");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(unix)]
#[test]
fn dev_socket_activation_retains_listener_across_restarts() {
    // M92.3: con `--port P`, el supervisor pre-abre y RETIENE `127.0.0.1:P`; cada hijo lo ADOPTA (fd
    // heredado) en vez de re-bind. Prueba rigurosa: si el segundo hijo NO adoptara, su `bind(P)` chocaría
    // con el socket que el supervisor sigue reteniendo (EADDRINUSE) → el servidor v2 no arrancaría. Que la
    // conexión post-reinicio reciba "v2" demuestra que re-adoptó el socket retenido.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let base = std::env::temp_dir().join("ray_dev_sockact");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("ray.toml"), "[package]\nname = \"srv\"\nversion = \"0.1.0\"\n").unwrap();
    // Servidor de un tiro: acepta una conexión, la lee, responde `<v>` y sale.
    let src = |v: &str| {
        format!(
            "import std/net;\n\
             fn main() -> int {{\n\
               match (net.tcp_listen(\"127.0.0.1\", {port})) {{\n\
                 Result.Ok(srv) => {{ match (net.tcp_accept(srv)) {{\n\
                   Result.Ok(c) => {{ match (net.socket_read(c)) {{ Result.Ok(_) => {{ let _ = net.socket_write(c, \"{v}\"); }}, Result.Err(e) => eprint(e), }} close(c); }},\n\
                   Result.Err(e) => eprint(e), }} 0 }},\n\
                 Result.Err(e) => {{ eprint(\"listen: \" + e); 1 }},\n\
               }}\n\
             }}\n"
        )
    };
    std::fs::write(base.join("src/main.ray"), src("v1")).unwrap();

    let out_path = base.join("output.txt");
    let out_file = std::fs::File::create(&out_path).unwrap();
    let err_file = out_file.try_clone().unwrap();
    let mut dev = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("dev")
        .arg("--port")
        .arg(port.to_string())
        .current_dir(&base)
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("lanza ray dev --port");

    wait_for_content(&out_path, "socket-activation", 10);
    // 1) El primer hijo adopta el socket retenido; una conexión recibe "v1" (si no adoptara, EADDRINUSE).
    assert_eq!(connect_and_read(port, 10), "v1", "el primer hijo adoptó el socket retenido");
    wait_for_content(&out_path, "waiting for changes", 10);
    // 2) Editar → reinicio → el segundo hijo RE-adopta el MISMO socket (el supervisor lo retuvo).
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(base.join("src/main.ray"), src("v2")).unwrap();
    wait_for_content(&out_path, "restarting", 10);
    assert_eq!(connect_and_read(port, 10), "v2", "el segundo hijo re-adoptó el socket entre reinicios");

    let _ = dev.kill();
    let _ = dev.wait();
}

/// Lee el puerto del hub de live-reload del log de `ray dev` (`live-reload on http://127.0.0.1:<port>`).
#[cfg(unix)]
fn read_hub_port(path: &std::path::Path, secs: u64) -> u16 {
    let s = wait_for_content(path, "live-reload on http://127.0.0.1:", secs);
    let marker = "live-reload on http://127.0.0.1:";
    let i = s.find(marker).unwrap() + marker.len();
    s[i..].split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap()
}

/// GET `/` a `127.0.0.1:port` (reintenta hasta `secs`; el socket lo retiene el supervisor → se encola).
#[cfg(unix)]
fn http_get(port: u16, secs: u64) -> String {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
            let _ = s.write_all(b"GET / HTTP/1.0\r\nHost: x\r\n\r\n");
            let mut buf = String::new();
            if s.read_to_string(&mut buf).is_ok() && buf.contains("200") {
                return buf;
            }
        }
        assert!(Instant::now() < deadline, "sin respuesta HTTP en {port} tras {secs}s");
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(unix)]
#[test]
fn dev_live_reload_injects_snippet_and_emits_reload() {
    // M92.4: bajo `ray dev --port P` con un webserver, (1) las respuestas HTML llevan inyectado un
    // `EventSource` al hub SSE del supervisor, y (2) el hub emite `data: reload` en cada reinicio → la
    // página se refresca sola. El hub es un canal SOLO-de-dev en un puerto lateral (no el SSE de la app).
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let base = std::env::temp_dir().join("ray_dev_livereload");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("net")).unwrap();
    // Usa el PAQUETE real (`packages/net/webserver.ray`, donde vive la inyección) + su dep `net/trace`
    // (un solo archivo, que solo importa std). Así el test ejercita el código publicado, sin duplicar la
    // inyección en la copia standalone `examples/web/webserver.ray`.
    let pkg = format!("{}/packages/net", env!("CARGO_MANIFEST_DIR"));
    std::fs::copy(format!("{pkg}/webserver.ray"), base.join("webserver.ray")).unwrap();
    std::fs::copy(format!("{pkg}/trace.ray"), base.join("net/trace.ray")).unwrap();
    let driver = |v: &str| {
        format!(
            "from webserver import serve, html_response, Request, Response;\n\
             fn h(req: Request) -> Response {{ html_response(200, \"<html><body>{v}</body></html>\") }}\n\
             fn main() -> int {{ let _ = serve(\"127.0.0.1\", {port}, h); 0 }}\n"
        )
    };
    std::fs::write(base.join("main.ray"), driver("v1")).unwrap();

    let out_path = base.join("output.txt");
    let out_file = std::fs::File::create(&out_path).unwrap();
    let err_file = out_file.try_clone().unwrap();
    let mut dev = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("dev")
        .arg("--port")
        .arg(port.to_string())
        .arg("main.ray")
        .current_dir(&base)
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("lanza ray dev --port");

    let hub = read_hub_port(&out_path, 10);
    // 1) La respuesta HTML lleva el snippet de live-reload apuntando al hub.
    let resp = http_get(port, 15);
    assert!(resp.contains("EventSource"), "el HTML lleva el snippet inyectado:\n{resp}");
    assert!(resp.contains(&format!(":{hub}/")), "el snippet apunta al hub {hub}:\n{resp}");

    // 2) Un navegador conectado al hub recibe `data: reload` cuando un cambio VÁLIDO reinicia.
    let mut sse = TcpStream::connect(("127.0.0.1", hub)).expect("conecta al hub SSE");
    sse.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    sse.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    // Espera al ": connected" inicial para asegurar el registro antes de editar.
    let mut got = String::new();
    let mut probe = [0u8; 256];
    loop {
        let n = sse.read(&mut probe).expect("lee del hub");
        got.push_str(&String::from_utf8_lossy(&probe[..n]));
        if got.contains(": connected") {
            break;
        }
    }
    std::thread::sleep(Duration::from_millis(100));
    std::fs::write(base.join("main.ray"), driver("v2")).unwrap();
    // Lee hasta el evento reload.
    loop {
        if got.contains("data: reload") {
            break;
        }
        let n = sse.read(&mut probe).expect("lee el evento reload del hub");
        assert!(n > 0, "el hub cerró antes del reload; recibido:\n{got}");
        got.push_str(&String::from_utf8_lossy(&probe[..n]));
    }

    let _ = dev.kill();
    let _ = dev.wait();
}

#[cfg(unix)]
#[test]
fn dev_live_reload_without_port_also_injects() {
    // El hub arranca SIEMPRE (no solo con `--port`): "es una app web" lo decide el webserver al servir
    // HTML, no el supervisor. Sin socket-activation el snippet reintenta el fetch hasta que el hijo
    // re-binde (aquí solo se verifica la inyección; el reinicio lo cubre el test con `--port`).
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let base = std::env::temp_dir().join("ray_dev_livereload_noport");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("net")).unwrap();
    let pkg = format!("{}/packages/net", env!("CARGO_MANIFEST_DIR"));
    std::fs::copy(format!("{pkg}/webserver.ray"), base.join("webserver.ray")).unwrap();
    std::fs::copy(format!("{pkg}/trace.ray"), base.join("net/trace.ray")).unwrap();
    std::fs::write(
        base.join("main.ray"),
        format!(
            "from webserver import serve, html_response, Request, Response;\n\
             fn h(req: Request) -> Response {{ html_response(200, \"<html><body>solo</body></html>\") }}\n\
             fn main() -> int {{ let _ = serve(\"127.0.0.1\", {port}, h); 0 }}\n"
        ),
    )
    .unwrap();

    let out_path = base.join("output.txt");
    let out_file = std::fs::File::create(&out_path).unwrap();
    let err_file = out_file.try_clone().unwrap();
    let mut dev = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("dev")
        .arg("main.ray")
        .current_dir(&base)
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("lanza ray dev sin --port");

    let hub = read_hub_port(&out_path, 10);
    let resp = http_get(port, 15);
    assert!(resp.contains("EventSource"), "el HTML lleva el snippet inyectado sin --port:\n{resp}");
    assert!(resp.contains(&format!(":{hub}/")), "el snippet apunta al hub {hub}:\n{resp}");

    let _ = dev.kill();
    let _ = dev.wait();
}

#[test]
fn dev_does_not_restart_if_change_fails_to_compile() {
    // Check-before-restart (M92.2): un cambio que NO compila NO debe reiniciar el programa; el
    // supervisor imprime el diagnóstico y mantiene lo que había. Un cambio verde posterior sí reinicia.
    let base = std::env::temp_dir().join("ray_dev_check");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("ray.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(base.join("src/main.ray"), "fn main() -> int { print(\"v1\"); 0 }\n").unwrap();

    let out_path = base.join("output.txt");
    let out_file = std::fs::File::create(&out_path).unwrap();
    let err_file = out_file.try_clone().unwrap();
    let mut dev = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("dev")
        .current_dir(&base)
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("lanza ray dev");

    wait_for_content(&out_path, "v1", 10);
    wait_for_content(&out_path, "waiting for changes", 10);

    // 1) Un cambio que NO compila (falta cerrar la llave): el supervisor lo rechaza, no reinicia.
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(base.join("src/main.ray"), "fn main() -> int { print(\"roto\"); 0 \n").unwrap();
    wait_for_content(&out_path, "does not compile", 10);

    // 2) Un cambio verde posterior SÍ reinicia con el código nuevo.
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(base.join("src/main.ray"), "fn main() -> int { print(\"v2\"); 0 }\n").unwrap();
    let output = wait_for_content(&out_path, "v2", 10);
    assert!(output.contains("does not compile"), "rechazó el cambio roto:\n{output}");
    assert!(output.contains("restarting"), "reinició con el cambio verde:\n{output}");
    // El código roto nunca corrió (no imprimió "roto").
    assert!(!output.contains("roto"), "el cambio roto no llegó a ejecutarse:\n{output}");

    let _ = dev.kill();
    let _ = dev.wait();
}

#[test]
fn dev_ignores_a_save_with_no_changes() {
    // Confirmación por contenido: un mtime tocado con los MISMOS bytes (guardado sin editar, un
    // formateador idempotente, `touch`) no debe reiniciar el programa ni emitir reload. Solo una
    // edición real (otros bytes) reinicia.
    let base = std::env::temp_dir().join("ray_dev_touch");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("ray.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\n").unwrap();
    let v1 = "fn main() -> int { print(\"v1\"); 0 }\n";
    std::fs::write(base.join("src/main.ray"), v1).unwrap();

    let out_path = base.join("output.txt");
    let out_file = std::fs::File::create(&out_path).unwrap();
    let err_file = out_file.try_clone().unwrap();
    let mut dev = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("dev")
        .current_dir(&base)
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("lanza ray dev");

    wait_for_content(&out_path, "v1", 10);
    wait_for_content(&out_path, "waiting for changes", 10);

    // 1) Reescribir el MISMO contenido (bump de mtime, cero bytes cambiados): se ignora sin reiniciar.
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(base.join("src/main.ray"), v1).unwrap();
    let output = wait_for_content(&out_path, "contents unchanged", 10);
    assert!(!output.contains("restarting"), "un guardado sin cambios no reinicia:\n{output}");

    // 2) Una edición real posterior SÍ reinicia.
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(base.join("src/main.ray"), "fn main() -> int { print(\"v2\"); 0 }\n").unwrap();
    let output = wait_for_content(&out_path, "v2", 10);
    assert!(output.contains("restarting"), "la edición real reinició:\n{output}");

    let _ = dev.kill();
    let _ = dev.wait();
}
