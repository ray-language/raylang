//! M168 — `signals()` existe en TODAS las plataformas de escritorio: unix (self-pipe, M88.1) y
//! Windows (`SetConsoleCtrlHandler`, docs/windows.md W1). Este test corre en las tres: el canal
//! se crea sin error en la VM y en el binario nativo, y en Windows el handler entrega 2/15 al
//! simular los eventos de consola (no hay forma portable de generar un Ctrl-C real en CI).

use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_ray");

const PROGRAM: &str = "fn main() -> int {\n    let s = signals();\n    print(\"signals installed\");\n    0\n}\n";

fn tmp(name: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("ray_signals_portable_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn signals_channel_is_created_on_every_platform_in_the_vm() {
    let base = tmp("vm");
    std::fs::write(base.join("prog.ray"), PROGRAM).unwrap();
    let out = Command::new(BIN)
        .args(["run", "prog.ray"])
        .current_dir(&base)
        .stdin(Stdio::null())
        .output()
        .expect("lanza ray");
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "signals installed");
}

#[test]
fn signals_channel_is_created_on_every_platform_in_the_native_binary() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando: rustc no disponible");
        return;
    }
    let base = tmp("native");
    std::fs::write(base.join("prog.ray"), PROGRAM).unwrap();
    let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new(BIN)
        .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
        .current_dir(&base)
        .output()
        .expect("lanza el build");
    assert!(build.status.success(), "build --native: {}", String::from_utf8_lossy(&build.stderr));
    let out = Command::new(&bin).stdin(Stdio::null()).output().expect("corre el nativo");
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "signals installed");
}

/// Windows: el handler de consola encola 2 (Ctrl-C/Break) y 15 (cierre/logoff/apagado), ignora
/// eventos ajenos, y la bandera + cola se drenan como el self-pipe de unix.
#[cfg(windows)]
#[test]
fn console_control_events_map_to_posix_numbers() {
    let fd = raylang::builtins::signals_install().expect("install");
    assert_eq!(fd, -1, "sin fd: la fuente es el handler de consola");
    assert_eq!(raylang::builtins::signals_simulate_console_event(0), 1, "CTRL_C manejado");
    assert_eq!(raylang::builtins::signals_simulate_console_event(1), 1, "CTRL_BREAK manejado");
    assert_eq!(raylang::builtins::signals_simulate_console_event(99), 0, "evento ajeno: no manejado");
    assert!(raylang::builtins::signals_pending(), "bandera pendiente");
    assert_eq!(raylang::builtins::signals_read_one(fd), Some(2));
    assert_eq!(raylang::builtins::signals_read_one(fd), Some(2));
    assert_eq!(raylang::builtins::signals_read_one(fd), None, "cola drenada");
}
