//! M196 (plan ray-remote D2): un módulo puede definir una función con el nombre de un builtin
//! (`pub fn close(c: Conn)`), se consume calificada (`proto.close(c)`), y dentro del módulo el
//! builtin sigue alcanzable por el pseudo-módulo `builtin` (`builtin.close(c.sock)`). Misma salida en
//! VM, intérprete y nativo. Además: el módulo chequeado suelto ya no es error; `from M import close;`
//! sin calificar se rechaza con mensaje; `builtin.nope` se rechaza.

use std::process::Command;

const PROTO: &str = r#"
pub struct Conn { sock: int, live: bool }
pub fn open(sock: int) -> Conn { Conn { sock: sock, live: true } }
pub fn close(c: Conn) { c.live = false; builtin.close(c.sock); }
pub fn len(c: Conn) -> int { "siete!!".len() }
"#;

const MAIN: &str = r#"
import rfb/proto;
fn main() -> int {
    let c = proto.open(0);
    print(proto.len(c));
    proto.close(c);
    print(c.live);
    print([1, 2, 3].len());
    0
}
"#;

const WANT: &str = "7\nfalse\n3\n";

fn project(name: &str, main: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_builtin_shadow_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/rfb")).expect("crea el proyecto");
    std::fs::write(dir.join("ray.toml"), "[package]\nname = \"shadow\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(dir.join("src/rfb/proto.ray"), PROTO).unwrap();
    std::fs::write(dir.join("src/main.ray"), main).unwrap();
    dir
}

fn ray(dir: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().expect("lanza ray");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn module_function_named_like_a_builtin_with_the_escape() {
    let dir = project("engines", MAIN);
    let (out, err, code) = ray(&dir, &["run", "src/main.ray"]);
    assert_eq!(out, WANT, "VM\n{err}");
    assert_eq!(code, 0);
    let (out, err, code) = ray(&dir, &["run", "--interp", "src/main.ray"]);
    assert_eq!(out, WANT, "intérprete\n{err}");
    assert_eq!(code, 0);
    // El módulo chequeado SUELTO (sin main) ya no dispara "cannot be redefined".
    let (_o, err, _c) = ray(&dir, &["build", "src/rfb/proto.ray"]);
    assert!(!err.contains("cannot be redefined"), "módulo suelto\n{err}");
    // La entrada sigue sin poder redefinir un builtin con nombre pelado.
    std::fs::write(dir.join("src/redef.ray"), "fn to_string(x: int) -> string { \"USER\" }\nfn main() -> int { 0 }\n").unwrap();
    let (_o, err, code) = ray(&dir, &["run", "src/redef.ray"]);
    assert_eq!(code, 65, "{err}");
    assert!(err.contains("is a language builtin and cannot be redefined"), "{err}");
}

#[test]
fn unqualified_import_of_a_builtin_name_and_unknown_builtin_are_rejected() {
    let dir = project("errors", "from rfb/proto import close;\nfn main() -> int { 0 }\n");
    let (_o, err, code) = ray(&dir, &["run", "src/main.ray"]);
    assert_ne!(code, 0);
    assert!(err.contains("cannot import 'close' unqualified: it is a language builtin"), "{err}");
    std::fs::write(dir.join("src/main.ray"), "fn main() -> int { builtin.nope(1); 0 }\n").unwrap();
    let (_o, err, code) = ray(&dir, &["run", "src/main.ray"]);
    assert_ne!(code, 0);
    assert!(err.contains("'nope' not declared") || err.contains("nope"), "{err}");
}

#[test]
fn native_agrees() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando native builtin shadow: rustc no disponible");
        return;
    }
    let dir = project("native", MAIN);
    let bin = dir.join(format!("shadow_bin{}", std::env::consts::EXE_SUFFIX));
    let (_o, err, code) = ray(&dir, &["build", "src/main.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native\n{err}");
    let out = Command::new(&bin).output().expect("corre el binario nativo");
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
}
