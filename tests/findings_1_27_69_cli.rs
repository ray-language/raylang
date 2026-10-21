//! Hallazgos atendidos en 1.27.69 (IDEAS §113 #7 y #8, oct 2026): `ray test --native` parado se
//! lleva a su binario de prueba; la idempotencia del formateador se prueba en `src/fmt.rs`.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ray_f1_27_69_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Los procesos vivos cuya línea de comando contiene `needle` (unix: `ps`).
#[cfg(unix)]
fn processes_matching(needle: &str) -> Vec<String> {
    let out = Command::new("ps").args(["-axo", "pid=,command="]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains(needle)).map(str::to_string).collect()
}

/// IDEAS §113 #7 (raystat #207, nota): parar `ray test --native` por su pid (SIGTERM) termina
/// también el binario de prueba que estaba corriendo — antes quedaba huérfano, colgado para siempre.
#[cfg(unix)]
#[test]
fn stopping_ray_test_native_takes_its_test_binary_with_it() {
    if !has_rustc() {
        return;
    }
    let d = tmp("orphan");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"orphan\"\nversion = \"0.1.0\"\nentry = \"src/main.ray\"\n").unwrap();
    std::fs::write(
        d.join("src/main.ray"),
        "import std/time;\n@test\nfn hangs_forever() -> bool {\n    while (true) {\n        time.sleep(50);\n    }\n    true\n}\nfn main() -> int { 0 }\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["test", "--native"])
        .current_dir(&d)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let marker = format!("ray_test_native_{}", child.id());
    // Esperar a que el binario de la suite esté compilado y corriendo (el build nativo tarda).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        if !processes_matching(&marker).is_empty() {
            break;
        }
        if let Ok(Some(st)) = child.try_wait() {
            panic!("ray test terminó antes de lanzar la prueba: {st}");
        }
        assert!(std::time::Instant::now() < deadline, "el binario de prueba no llegó a correr");
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    // SAFETY: SIGTERM a un hijo nuestro, aún sin recoger.
    assert_eq!(unsafe { kill(child.id() as i32, 15) }, 0);
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130), "{status}");
    // El handler reenvía SIGTERM al binario de prueba; debe desaparecer enseguida.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let alive = processes_matching(&marker);
        if alive.is_empty() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "el binario de prueba quedó huérfano: {alive:?}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(&marker));
}
