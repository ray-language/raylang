//! M266 — std/keychain: get/set/delete sobre el llavero del sistema. La batería corre contra el
//! BACKEND DE ARCHIVO (`RAY_KEYCHAIN_FILE`: el CI no tiene llavero ni sesión de usuario) en los
//! tres motores; el backend real se prueba a mano con `RAY_KEYCHAIN_REAL=1` (macOS/Linux/Windows).

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("ray_keychain_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

const PROG: &str = r#"import std/keychain;

// Un `Result<Option<string>, string>` a texto: "none", el secreto, o "err: …".
fn show(r: Result<Option<string>, string>) -> string {
    match (r) {
        Result.Ok(o) => match (o) {
            Option.Some(v) => v,
            Option.None => "none",
        },
        Result.Err(e) => "err: " + e,
    }
}

fn main() {
    let svc = "dev.raylang.test";
    print(show(keychain.get(svc, "alpha")));
    print(keychain.set(svc, "alpha", "s3cret").is_ok());
    print(show(keychain.get(svc, "alpha")));
    print(keychain.set(svc, "alpha", "changed").is_ok());
    print(show(keychain.get(svc, "alpha")));
    print(keychain.set(svc, "beta", "otro").is_ok());
    print(keychain.delete(svc, "alpha").unwrap_or(false));
    print(keychain.delete(svc, "alpha").unwrap_or(true));
    print(show(keychain.get(svc, "alpha")));
    print(show(keychain.get(svc, "beta")));
    print(keychain.delete(svc, "beta").unwrap_or(false));
    print(keychain.set("", "x", "y").is_err());
}
"#;

const WANT: &str = "none\ntrue\ns3cret\ntrue\nchanged\ntrue\ntrue\nfalse\nnone\notro\ntrue\ntrue\n";

fn run_engine(dir: &PathBuf, engine: &str, store: &PathBuf) -> String {
    let prog = dir.join("prog.ray");
    std::fs::write(&prog, PROG).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(engine)
        .arg(&prog)
        .env("RAY_KEYCHAIN_FILE", store)
        .output()
        .unwrap();
    assert!(out.status.success(), "{engine}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn keychain_round_trip_on_the_file_backend_both_engines() {
    for engine in ["--vm", "--interp"] {
        let d = tmp(&engine[2..]);
        let store = d.join("store");
        assert_eq!(run_engine(&d, engine, &store), WANT, "{engine}");
        // el archivo existe y es 0600 en unix
        assert!(store.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&store).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}

#[test]
fn keychain_round_trip_natively() {
    if !Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("(sin rustc: se omite el nativo)");
        return;
    }
    let d = tmp("native");
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    let bin = d.join("prog_bin");
    let st = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
        .current_dir(&d)
        .output()
        .expect("build nativo");
    assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
    let out = Command::new(&bin).env("RAY_KEYCHAIN_FILE", d.join("store")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo");
}

/// El llavero REAL de la máquina (Keychain / Secret Service / Credential Manager): solo a mano,
/// con `RAY_KEYCHAIN_REAL=1` (deja el llavero limpio: el programa borra lo que crea).
#[test]
fn keychain_round_trip_on_the_real_backend() {
    if std::env::var("RAY_KEYCHAIN_REAL").ok().as_deref() != Some("1") {
        eprintln!("(RAY_KEYCHAIN_REAL=1 para probar el llavero real)");
        return;
    }
    let d = tmp("real");
    let prog = d.join("prog.ray");
    std::fs::write(&prog, PROG).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang")).arg("--vm").arg(&prog).env_remove("RAY_KEYCHAIN_FILE").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "llavero real");
}
