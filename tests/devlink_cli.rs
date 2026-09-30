//! M330 D2 (hot reload móvil): el enlace de desarrollo de extremo a extremo, en escritorio —
//! `ray dev --device` como anfitrión y `ray dev-client` como el dispositivo (lo que la librería
//! de desarrollo hace en el teléfono), con la UI en headless. Un cambio en `main.ray` debe llegar
//! al cliente, parar el programa viejo (D1) y arrancar el nuevo en el mismo proceso.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_ray");

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ray-devlink-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Lee un flujo línea a línea a un búfer compartido (para esperar por un texto sin bloquear).
fn tail<R: std::io::Read + Send + 'static>(r: R) -> Arc<Mutex<String>> {
    let buf = Arc::new(Mutex::new(String::new()));
    let out = buf.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines().map_while(Result::ok) {
            out.lock().unwrap().push_str(&line);
            out.lock().unwrap().push('\n');
        }
    });
    buf
}

fn wait_for(buf: &Arc<Mutex<String>>, needle: &str, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if buf.lock().unwrap().contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        // Los dos procesos son hijos lanzados por este test: se terminan por su handle.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn write_main(project: &Path, greeting: &str) {
    std::fs::write(
        project.join("src/main.ray"),
        format!("import std/time;\nfn main() {{\n    print(\"{greeting}\");\n    time.sleep(600000);\n}}\n"),
    )
    .unwrap();
}

#[test]
fn a_change_on_the_host_restarts_the_program_on_the_linked_device() {
    let project = scratch("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("ray.toml"), "[package]\nname = \"linked\"\nversion = \"0.1.0\"\n").unwrap();
    write_main(&project, "hello v1");

    // El anfitrión: imprime la URL del enlace y publica el primer snapshot.
    let mut host = Command::new(BIN)
        .args(["dev", "--device"])
        .current_dir(&project)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ray dev --device");
    let host_err = tail(host.stderr.take().unwrap());
    let _host = Guard(host);
    assert!(wait_for(&host_err, "device link: ray-dev://", 30), "no link URL:\n{}", host_err.lock().unwrap());
    let url = {
        let text = host_err.lock().unwrap();
        let line = text.lines().find(|l| l.contains("device link: ")).unwrap();
        line.split("device link: ").nth(1).unwrap().trim().to_string()
    };
    // El teléfono llega por la LAN; el cliente de prueba entra por loopback al mismo puerto.
    let (_, rest) = url.split_once("://").unwrap();
    let (_, port_and_token) = rest.split_once(':').unwrap();
    let local_url = format!("ray-dev://127.0.0.1:{port_and_token}");

    // El dispositivo: headless, con su sandbox propio.
    let sandbox = scratch("sandbox");
    let mut client = Command::new(BIN)
        .args(["dev-client", &local_url, sandbox.to_str().unwrap()])
        .env("RAY_UI_BACKEND", "headless")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ray dev-client");
    let client_out = tail(client.stdout.take().unwrap());
    let client_err = tail(client.stderr.take().unwrap());
    let _client = Guard(client);

    assert!(wait_for(&client_out, "hello v1", 30), "v1 never ran on the device:\nstdout:\n{}\nstderr:\n{}\nhost:\n{}", client_out.lock().unwrap(), client_err.lock().unwrap(), host_err.lock().unwrap());
    assert!(wait_for(&host_err, "running", 10), "the host did not hear the device's status:\n{}", host_err.lock().unwrap());
    // El snapshot vive en la sandbox del dispositivo, no se corre desde el proyecto del Mac.
    assert!(sandbox.join("project/src/main.ray").is_file());

    // Un cambio en el Mac: el anfitrión comprueba, publica; el dispositivo para v1 y arranca v2.
    std::thread::sleep(Duration::from_millis(300)); // que el mtime nuevo sea distinguible
    write_main(&project, "hello v2");
    assert!(wait_for(&client_out, "hello v2", 30), "v2 never ran on the device:\nstdout:\n{}\nstderr:\n{}\nhost:\n{}", client_out.lock().unwrap(), client_err.lock().unwrap(), host_err.lock().unwrap());
    assert!(wait_for(&host_err, "stopped", 10), "the host did not hear v1 stop:\n{}", host_err.lock().unwrap());

    // Un cambio que no compila NO llega al dispositivo: v2 sigue corriendo.
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(project.join("src/main.ray"), "fn main() { let x: int = \"no\"; }\n").unwrap();
    assert!(wait_for(&host_err, "does not compile", 30), "the host did not reject the broken program:\n{}", host_err.lock().unwrap());
    let snapshots_before = host_err.lock().unwrap().matches("snapshot of").count();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(host_err.lock().unwrap().matches("snapshot of").count(), snapshots_before, "a broken program was published");
    assert_eq!(client_out.lock().unwrap().matches("hello v2").count(), 1);

    let _ = std::fs::remove_dir_all(&project);
    let _ = std::fs::remove_dir_all(&sandbox);
}
