//! IDEAS §103 — los hallazgos que raylb (el balanceador de ray-apps) documentó, cada uno con su
//! programa mínimo. M355: el `connect` no retiene al worker (L19/L14) y el cliente HTTP acota el dial (L5).

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_raylb_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Un proyecto consumidor con `net` del repo como dependencia por ruta.
fn project(name: &str) -> PathBuf {
    let d = tmp(name);
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        d.join("ray.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nentry = \"prog.ray\"\n\n[dependencies]\nnet = \"path:{root}/packages/net\"\n"),
    )
    .unwrap();
    d
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// `prog.ray` en la VM y (con rustc) en nativo, ambos con DOS hilos worker: imprimen `want`.
fn vm_and_native_two_threads(dir: &PathBuf, want: &str) {
    let ray = env!("CARGO_BIN_EXE_ray");
    // Ninguna ejecución puede esperar al plazo del SO (75 s en macOS): también la SALIDA del proceso.
    let t = std::time::Instant::now();
    let out = Command::new(ray).args(["run", "prog.ray"]).env("RAYLANG_THREADS", "2").current_dir(dir).output().unwrap();
    assert!(out.status.success(), "vm: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), want, "vm");
    assert!(t.elapsed().as_secs() < 30, "vm: the run waited for the OS connect timeout");
    if has_rustc() {
        let bin = dir.join("prog_bin");
        let b = Command::new(ray).args(["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]).current_dir(dir).output().unwrap();
        assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
        let t = std::time::Instant::now();
        let out = Command::new(&bin).env("RAYLANG_THREADS", "2").current_dir(dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
        assert!(t.elapsed().as_secs() < 30, "native: the run waited for the OS connect timeout");
    }
}

/// L19/L14/L5: cuatro fibras marcan a un host que descarta los SYN (10.255.255.1, no enrutable) con
/// dos hilos worker. Antes el `connect` bloqueaba al worker: los temporizadores de `main` se
/// congelaban hasta 75 s y el proceso no podía salir. Ahora `main` sigue a su ritmo, el cliente HTTP
/// falla dentro de su plazo y el programa termina con los diales aún pendientes. (Donde la red
/// rechaza el destino al instante, las mismas aserciones valen: solo se exige no esperar al SO.)
#[test]
fn a_dial_to_a_black_hole_does_not_freeze_the_scheduler_and_http_bounds_it() {
    let d = project("blackhole");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/net;
import std/time;
import net/http;

fn dialer() {
    let _ = net.tcp_connect("10.255.255.1", 81);
}

fn main() -> int {
    var i: int = 0;
    while (i < 4) {
        spawn(fn() { dialer(); });
        i = i + 1;
    }
    let t0 = time.monotonic();
    var ticks: int = 0;
    while (ticks < 5) {
        time.sleep(100);
        ticks = ticks + 1;
    }
    print("timely=" + (time.monotonic() - t0 < 5000).to_string());
    let t1 = time.monotonic();
    let r = http.request_bytes("GET", "http://10.255.255.1:81/", b"", Map.new(), 400);
    let failed = match (r) {
        Result.Ok(_) => false,
        Result.Err(m) => m.starts_with("could not connect: "),
    };
    print("request failed=" + failed.to_string() + " bounded=" + (time.monotonic() - t1 < 5000).to_string());
    let t2 = time.monotonic();
    let c = match (http.connect_timeout("http://10.255.255.1:81", 300)) {
        Result.Ok(_) => false,
        Result.Err(m) => m.starts_with("could not connect: "),
    };
    print("connect failed=" + c.to_string() + " bounded=" + (time.monotonic() - t2 < 5000).to_string());
    0
}
"#,
    )
    .unwrap();
    vm_and_native_two_threads(&d, "timely=true\nrequest failed=true bounded=true\nconnect failed=true bounded=true\n");
}
