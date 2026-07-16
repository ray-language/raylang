//! M92.1 — `ray dev`: modo desarrollo (watcher + reinicio). Se prueba el ciclo completo por
//! subproceso: arranca un proyecto que imprime y termina, se edita el fuente y el supervisor
//! relanza el programa con el código nuevo. La salida del hijo va a un archivo que el test
//! sondea con plazo (el watcher es polling de ~200 ms; los tiempos son holgados para CI).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Sondea `path` hasta que su contenido contenga `needle` (plazo `secs`); devuelve el contenido.
fn esperar_contenido(path: &std::path::Path, needle: &str, secs: u64) -> String {
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
fn dev_reinicia_ante_cambios() {
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
    esperar_contenido(&out_path, "v1", 10);
    esperar_contenido(&out_path, "waiting for changes", 10);

    // 2) Editar el fuente → el watcher lo ve y relanza con el código NUEVO.
    std::thread::sleep(Duration::from_millis(50)); // mtime estrictamente posterior
    std::fs::write(base.join("src/main.ray"), "fn main() -> int { print(\"v2\"); 0 }\n").unwrap();
    let output = esperar_contenido(&out_path, "v2", 10);
    assert!(output.contains("restarting"), "anuncia el reinicio:\n{output}");

    let _ = dev.kill();
    let _ = dev.wait();
}

/// Conecta a `127.0.0.1:port`, envía un ping y devuelve la respuesta (reintenta hasta `secs`). El
/// socket lo retiene el supervisor, así que una conexión temprana se ENCOLA en el backlog (nunca
/// rechazada) hasta que un hijo la acepta — justo la propiedad que probamos.
#[cfg(unix)]
fn conectar_y_leer(port: u16, secs: u64) -> String {
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
fn dev_socket_activation_retiene_el_listener_entre_reinicios() {
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

    esperar_contenido(&out_path, "socket-activation", 10);
    // 1) El primer hijo adopta el socket retenido; una conexión recibe "v1" (si no adoptara, EADDRINUSE).
    assert_eq!(conectar_y_leer(port, 10), "v1", "el primer hijo adoptó el socket retenido");
    esperar_contenido(&out_path, "waiting for changes", 10);
    // 2) Editar → reinicio → el segundo hijo RE-adopta el MISMO socket (el supervisor lo retuvo).
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(base.join("src/main.ray"), src("v2")).unwrap();
    esperar_contenido(&out_path, "restarting", 10);
    assert_eq!(conectar_y_leer(port, 10), "v2", "el segundo hijo re-adoptó el socket entre reinicios");

    let _ = dev.kill();
    let _ = dev.wait();
}

#[test]
fn dev_no_reinicia_si_el_cambio_no_compila() {
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

    esperar_contenido(&out_path, "v1", 10);
    esperar_contenido(&out_path, "waiting for changes", 10);

    // 1) Un cambio que NO compila (falta cerrar la llave): el supervisor lo rechaza, no reinicia.
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(base.join("src/main.ray"), "fn main() -> int { print(\"roto\"); 0 \n").unwrap();
    esperar_contenido(&out_path, "does not compile", 10);

    // 2) Un cambio verde posterior SÍ reinicia con el código nuevo.
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(base.join("src/main.ray"), "fn main() -> int { print(\"v2\"); 0 }\n").unwrap();
    let output = esperar_contenido(&out_path, "v2", 10);
    assert!(output.contains("does not compile"), "rechazó el cambio roto:\n{output}");
    assert!(output.contains("restarting"), "reinició con el cambio verde:\n{output}");
    // El código roto nunca corrió (no imprimió "roto").
    assert!(!output.contains("roto"), "el cambio roto no llegó a ejecutarse:\n{output}");

    let _ = dev.kill();
    let _ = dev.wait();
}
