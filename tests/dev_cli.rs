//! M92.1 — `ray dev`: modo desarrollo (watcher + reinicio). Se prueba el ciclo completo por
//! subproceso: arranca un proyecto que imprime y termina, se edita el fuente y el supervisor
//! relanza el programa con el código nuevo. La salida del hijo va a un archivo que el test
//! sondea con plazo (el watcher es polling de ~200 ms; los tiempos son holgados para CI).

use std::io::Read;
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
    esperar_contenido(&out_path, "esperando cambios", 10);

    // 2) Editar el fuente → el watcher lo ve y relanza con el código NUEVO.
    std::thread::sleep(Duration::from_millis(50)); // mtime estrictamente posterior
    std::fs::write(base.join("src/main.ray"), "fn main() -> int { print(\"v2\"); 0 }\n").unwrap();
    let output = esperar_contenido(&out_path, "v2", 10);
    assert!(output.contains("reiniciando"), "anuncia el reinicio:\n{output}");

    let _ = dev.kill();
    let _ = dev.wait();
}
