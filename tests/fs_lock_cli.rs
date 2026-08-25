//! Pruebas de los candados consultivos de archivo (M115.2): `fs.try_lock`/`fs.unlock` (flock).
//! Dos handles del MISMO archivo son open file descriptions distintas → conflictúan incluso en
//! el mismo proceso, lo que permite probar el patrón LOCK-file sin lanzar otro proceso.
//! No determinista (toca disco) → subproceso, en ambos motores.

use std::io::Write;
use std::process::Command;

fn run(name: &str, src: &str, vm: bool) -> (String, i32) {
    let mut path = std::env::temp_dir();
    path.push(format!("{name}.ray"));
    std::fs::File::create(&path).expect("crea").write_all(src.as_bytes()).expect("escribe");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let out = cmd.arg(&path).output().expect("lanza raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn try_lock_excludes_and_unlock_releases() {
    for vm in [false, true] {
        let dat = std::env::temp_dir().join(format!("ray_lock_{}.lock", if vm { "vm" } else { "in" }));
        let src = format!(
            r#"
import std/fs;
fn take(path: string) -> int {{
    match (fs.open(path, "w")) {{
        Result.Ok(h) => h,
        Result.Err(e) => {{ eprint(e); panic("open"); }},
    }}
}}
fn main() -> int {{
    let a = take("{path}");
    let b = take("{path}");
    match (fs.try_lock(a)) {{
        Result.Ok(got) => print(if (got) {{ "a: acquired" }} else {{ "a: busy" }}),
        Result.Err(e) => print("a err: " + e),
    }};
    match (fs.try_lock(b)) {{
        Result.Ok(got) => print(if (got) {{ "b: acquired" }} else {{ "b: busy" }}),
        Result.Err(e) => print("b err: " + e),
    }};
    match (fs.unlock(a)) {{
        Result.Ok(_) => print("a: unlocked"),
        Result.Err(e) => print("unlock err: " + e),
    }};
    match (fs.try_lock(b)) {{
        Result.Ok(got) => print(if (got) {{ "b: acquired" }} else {{ "b: busy" }}),
        Result.Err(e) => print("b err: " + e),
    }};
    close(b);
    // close suelta el candado: a puede volver a tomarlo
    match (fs.try_lock(a)) {{
        Result.Ok(got) => print(if (got) {{ "a: acquired" }} else {{ "a: busy" }}),
        Result.Err(e) => print("a err: " + e),
    }};
    close(a);
    // errores limpios: handle inválido
    match (fs.try_lock(99999)) {{
        Result.Ok(_) => print("MAL"),
        Result.Err(e) => print(e),
    }};
    match (fs.unlock(99999)) {{
        Result.Ok(_) => print("MAL"),
        Result.Err(e) => print(e),
    }};
    0
}}
"#,
            path = dat.to_string_lossy()
        );
        let (out, code) = run("ray_lock", &src, vm);
        assert_eq!(
            out,
            "a: acquired\nb: busy\na: unlocked\nb: acquired\na: acquired\n\
             invalid file handle: 99999\ninvalid file handle: 99999\n",
            "try_lock/unlock (vm={vm}): {out}"
        );
        assert_eq!(code, 0);
    }
}
