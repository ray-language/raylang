//! M295 (IDEAS §96 #9): el binario nativo corta la recursión demasiado profunda con el MISMO error
//! que la VM (`stack overflow (recursion too deep: N frames; RAYLANG_MAX_DEPTH raises the limit)`,
//! exit 70) en vez de reventar la pila (hilo principal: aborto con el mensaje de Rust; fibra: SIGBUS
//! mudo por debajo de los 1024 marcos que la VM permite). Un solo programa con modos: recursión
//! directa, dentro de una fibra, mutua, y a través de un valor `fn`; se comparan stdout y código de
//! salida VM ↔ nativo, también con `RAYLANG_MAX_DEPTH`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

const PROGRAM: &str = r#"
fn depth(n: int) -> int {
    if (n == 0) { 0 } else {
        let s = to_string(n);
        depth(n - 1) + s.len()
    }
}

fn even(n: int) -> bool { if (n == 0) { true } else { odd(n - 1) } }
fn odd(n: int) -> bool { if (n == 0) { false } else { even(n - 1) } }

fn apply(g: fn(int) -> int, n: int) -> int { if (n == 0) { 0 } else { g(n - 1) + 1 } }
fn via_value(n: int) -> int { apply(via_value, n) }

fn main() -> int {
    let a = args();
    let mode = a[0];
    let n = match (a[1].parse_int()) { Option.Some(v) => v, Option.None => 10 };
    print("start");
    if (mode == "direct") {
        print("direct " + to_string(depth(n)));
    } else if (mode == "fiber") {
        let t = spawn(fn() -> int { depth(n) });
        print("fiber " + to_string(join(t)));
    } else if (mode == "mutual") {
        print("mutual " + to_string(even(n)));
    } else {
        print("value " + to_string(via_value(n)));
    }
    print("end");
    0
}
"#;

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn run(cmd: &mut Command, envs: &[(&str, &str)]) -> (String, i32, String) {
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("lanza");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn native_recursion_limit_matches_the_vm() {
    if !has_rustc() {
        eprintln!("saltando: rustc no disponible");
        return;
    }
    let base = std::env::temp_dir().join(format!("ray_native_depth_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let src = base.join("depth.ray");
    std::fs::write(&src, PROGRAM).unwrap();
    let bin = base.join(format!("depth_native{}", std::env::consts::EXE_SUFFIX));
    let out = Command::new(BIN).args(["build", "--native", src.to_str().unwrap(), "-o", bin.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "compila el nativo: {}", String::from_utf8_lossy(&out.stderr));

    // (modo, n, env): por debajo del límite pasa; por encima, el mismo error y exit 70 en ambos.
    let cases: &[(&str, &str, &[(&str, &str)])] = &[
        ("direct", "1022", &[]),
        ("direct", "1023", &[]),   // main + 1023 marcos = 1024 → corta (como la VM)
        ("direct", "100000", &[]),
        ("fiber", "1023", &[]),    // la fibra no cuenta el main: pasa
        ("fiber", "100000", &[]),  // antes: SIGBUS mudo en nativo
        ("mutual", "3000", &[]),
        ("value", "3000", &[]),
        ("direct", "100", &[("RAYLANG_MAX_DEPTH", "64")]),
        ("direct", "40", &[("RAYLANG_MAX_DEPTH", "64")]),
    ];
    for (mode, n, envs) in cases {
        let (vm_out, vm_code, vm_err) = run(Command::new(BIN).args(["run", src.to_str().unwrap(), mode, n]), envs);
        let (nt_out, nt_code, nt_err) = run(Command::new(&bin).args([mode, n]), envs);
        assert_eq!(vm_out, nt_out, "stdout {mode} {n} {envs:?}\nvm stderr: {vm_err}\nnative stderr: {nt_err}");
        assert_eq!(vm_code, nt_code, "exit {mode} {n} {envs:?}\nvm stderr: {vm_err}\nnative stderr: {nt_err}");
        if vm_code != 0 {
            assert!(vm_err.contains("stack overflow (recursion too deep:"), "vm: {vm_err}");
            assert!(nt_err.contains("stack overflow (recursion too deep:"), "native: {nt_err}");
            assert!(!nt_err.contains("has overflowed its stack"), "native no revienta la pila: {nt_err}");
        }
    }
    let _ = std::fs::remove_dir_all(&base);
}
