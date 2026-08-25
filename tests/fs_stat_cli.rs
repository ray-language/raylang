//! Pruebas de metadatos y permisos (M115.3): `fs.stat` (lstat: SIN seguir symlinks) y
//! `fs.chmod`. No determinista (toca disco) → subproceso, en ambos motores. Se asevera lo
//! estable: kinds, el tamaño del symlink (la longitud del propio enlace), el roundtrip
//! chmod→mode; nunca tamaños de directorio ni mtimes (dependen de la plataforma).

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
#[cfg(unix)]
fn stat_detects_kinds_and_chmod_round_trips() {
    for vm in [false, true] {
        let base = std::env::temp_dir().join(format!("ray_stat_{}", if vm { "vm" } else { "in" }));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("mkdir");
        std::fs::write(base.join("f.txt"), b"hello").expect("archivo");
        std::os::unix::fs::symlink("f.txt", base.join("lnk")).expect("symlink");
        let root = base.to_string_lossy().into_owned();
        let src = format!(
            r#"
import std/fs;
fn kind_of(path: string) -> string {{
    match (fs.stat(path)) {{
        Result.Ok(st) => st.kind,
        Result.Err(e) => "err",
    }}
}}
fn main() -> int {{
    print(kind_of("{root}/f.txt"));
    print(kind_of("{root}"));
    print(kind_of("{root}/lnk"));
    print(kind_of("{root}/nope"));
    // el symlink reporta SU PROPIO tamaño (la longitud de "f.txt" = 5), no el del destino
    match (fs.stat("{root}/lnk")) {{
        Result.Ok(st) => print("lnk size=" + to_string(st.size)),
        Result.Err(e) => print("err"),
    }};
    // chmod 384 (= 0o600) y releer el modo por stat
    match (fs.chmod("{root}/f.txt", 384)) {{
        Result.Ok(_) => print("chmod ok"),
        Result.Err(e) => print("chmod err"),
    }};
    match (fs.stat("{root}/f.txt")) {{
        Result.Ok(st) => print("mode=" + to_string(st.mode)),
        Result.Err(e) => print("err"),
    }};
    match (fs.chmod("{root}/nope", 384)) {{
        Result.Ok(_) => print("MAL"),
        Result.Err(e) => print("chmod missing -> err"),
    }};
    0
}}
"#
        );
        let (out, code) = run("ray_stat", &src, vm);
        assert_eq!(
            out,
            "file\ndir\nsymlink\nerr\nlnk size=5\nchmod ok\nmode=384\nchmod missing -> err\n",
            "stat/chmod (vm={vm}): {out}"
        );
        assert_eq!(code, 0);
    }
}
