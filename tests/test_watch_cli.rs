//! M140 — `ray test --watch`: el bucle de dev aplicado al runner. Se prueba el ciclo completo
//! por subproceso (sin tty: cubre el camino pipe/CI, sin limpieza de pantalla ni modo crudo):
//! corre la suite, un cambio la re-corre con el resultado nuevo, y un guardado con los mismos
//! bytes ni re-corre ni sale del estado de espera.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Termina el supervisor con SIGTERM (su handler mata al hijo en curso; `Child::kill` es SIGKILL
/// y lo saltaría). Mismo helper que en dev_cli.
fn stop_watch(watch: &mut std::process::Child) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        unsafe {
            kill(watch.id() as i32, SIGTERM);
        }
        for _ in 0..30 {
            if let Ok(Some(_)) = watch.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = watch.kill();
    let _ = watch.wait();
}

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
fn test_watch_reruns_on_changes_and_ignores_identical_saves() {
    let base = std::env::temp_dir().join("ray_test_watch_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("ray.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\n").unwrap();
    let passing = "fn main() {}\n\n@test\nfn adds() -> bool {\n    1 + 1 == 2\n}\n";
    let failing = "fn main() {}\n\n@test\nfn adds() -> bool {\n    1 + 1 == 3\n}\n";
    std::fs::write(base.join("src/main.ray"), passing).unwrap();

    let out_path = base.join("output.txt");
    let out_file = std::fs::File::create(&out_path).unwrap();
    let err_file = out_file.try_clone().unwrap();
    let mut watch = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["test", "--watch"])
        .current_dir(&base)
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("lanza ray test --watch");

    // 1) La suite corre al arrancar y el supervisor queda a la espera.
    wait_for_content(&out_path, "all passed", 10);
    wait_for_content(&out_path, "waiting for changes", 10);

    // 2) Romper el test → re-corre y el resultado nuevo es el fallo.
    std::thread::sleep(Duration::from_millis(50)); // mtime estrictamente posterior
    std::fs::write(base.join("src/main.ray"), failing).unwrap();
    let output = wait_for_content(&out_path, "failed", 10);
    assert!(output.contains("re-running"), "anuncia la re-corrida:\n{output}");

    // 3) Arreglarlo → verde de nuevo (segunda aparición de "all passed").
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(base.join("src/main.ray"), passing).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = std::fs::read_to_string(&out_path).unwrap_or_default();
        if s.matches("all passed").count() >= 2 {
            break;
        }
        assert!(Instant::now() < deadline, "no volvió a verde en 10s:\n{s}");
        std::thread::sleep(Duration::from_millis(100));
    }

    // 4) Guardado con los MISMOS bytes → no re-corre (lo anuncia y sigue esperando). Antes,
    // asegurar que el supervisor ya está EN ESPERA (la última línea es el hint): un cambio que
    // llegara con la corrida aún viva la cortaría y re-correría sin gate, por diseño.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = std::fs::read_to_string(&out_path).unwrap_or_default();
        if s.trim_end().ends_with("(q\u{23CE} or Ctrl-C exits)") {
            break;
        }
        assert!(Instant::now() < deadline, "no volvió al estado de espera en 10s:\n{s}");
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(50));
    let runs_before = std::fs::read_to_string(&out_path).unwrap().matches("result:").count();
    std::fs::write(base.join("src/main.ray"), passing).unwrap();
    let output = wait_for_content(&out_path, "contents unchanged", 10);
    let runs_after = output.matches("result:").count();
    assert_eq!(runs_after, runs_before, "no debió re-correr:\n{output}");

    stop_watch(&mut watch);
}

#[test]
fn test_watch_selects_only_the_affected_suites() {
    // M141: un cambio en un módulo re-corre SOLO las suites cuyo grafo de imports lo alcanza;
    // ray.toml re-corre todo. El proyecto: entrada sin imports + una suite que importa `util` +
    // una suite suelta.
    let base = std::env::temp_dir().join("ray_test_watch_selective");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::create_dir_all(base.join("tests")).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nentry = \"src/main.ray\"\n",
    )
    .unwrap();
    std::fs::write(base.join("src/main.ray"), "fn main() {}\n").unwrap();
    std::fs::write(base.join("src/util.ray"), "pub fn one() -> int {\n    1\n}\n").unwrap();
    std::fs::write(
        base.join("tests/a_util.ray"),
        "import util;\n\n@test\nfn util_ok() -> bool {\n    util.one() == 1\n}\n",
    )
    .unwrap();
    std::fs::write(base.join("tests/b_plain.ray"), "@test\nfn plain_ok() -> bool {\n    true\n}\n")
        .unwrap();

    let out_path = base.join("output.txt");
    let out_file = std::fs::File::create(&out_path).unwrap();
    let err_file = out_file.try_clone().unwrap();
    let mut watch = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["test", "--watch"])
        .current_dir(&base)
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("lanza ray test --watch");

    // 1) La corrida inicial es COMPLETA (las dos suites) y queda en espera.
    wait_for_content(&out_path, "all passed", 10);
    wait_for_content(&out_path, "waiting for changes", 10);
    let initial = std::fs::read_to_string(&out_path).unwrap();
    assert!(initial.contains("util_ok") && initial.contains("plain_ok"), "corrida inicial completa:\n{initial}");

    // 2) Cambiar `util.ray` → SOLO la suite que lo importa re-corre.
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(base.join("src/util.ray"), "pub fn one() -> int {\n    2 - 1\n}\n").unwrap();
    wait_for_content(&out_path, "re-running 1 of 3 suite(s)", 10);
    // La corrida selectiva (lo que sigue al anuncio) trae util_ok y NO plain_ok.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = std::fs::read_to_string(&out_path).unwrap_or_default();
        let after = &s[s.find("re-running 1 of 3").unwrap()..];
        if after.contains("result:") {
            assert!(after.contains("util_ok"), "la suite afectada corre:\n{after}");
            assert!(!after.contains("plain_ok"), "la suite ajena NO corre:\n{after}");
            break;
        }
        assert!(Instant::now() < deadline, "la corrida selectiva no terminó en 10s:\n{s}");
        std::thread::sleep(Duration::from_millis(100));
    }

    // 3) Cambiar `ray.toml` → re-corre TODO.
    std::thread::sleep(Duration::from_millis(50));
    let mut toml = std::fs::read_to_string(base.join("ray.toml")).unwrap();
    toml.push_str("# touch\n");
    std::fs::write(base.join("ray.toml"), toml).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = std::fs::read_to_string(&out_path).unwrap_or_default();
        if let Some(i) = s.rfind("re-running…")
            && s[i..].contains("plain_ok")
            && s[i..].contains("util_ok")
        {
            break;
        }
        assert!(Instant::now() < deadline, "ray.toml no re-corrió todo en 10s:\n{s}");
        std::thread::sleep(Duration::from_millis(100));
    }

    stop_watch(&mut watch);
}
