//! M192 (lote B de ray-remote): literales con sufijo y amplios (`u64`), cuenta de desplazamiento
//! `int`, y `from` como identificador — misma salida en VM, intérprete y binario nativo.

use std::io::Write;
use std::process::Command;

const PROGRAM: &str = r#"
fn rotl(v: u32, n: int) -> u32 { (v << n) | (v >> (32 - n)) }
fn slice(bits: [int], from: int, to: int) -> [int] {
    var out: [int] = [];
    var i = from;
    while (i < to) { out.push(bits[i]); i = i + 1; }
    out
}
fn main() -> int {
    let all: u64 = 0xFFFFFFFFFFFFFFFF;
    let k0: u64 = 0x428A2F98D728AE22;
    let big = 18446744073709551615;
    let b = 255u8;
    let m = 0xFFu32;
    print(all); print(k0); print(big); print(b); print(m);
    print(all >> 60); print(rotl(1 as u32, 31)); print(rotl(0x80000000u32, 1));
    let n = 4;
    let w: u64 = (all >> (n + 56)) + 1;
    print(w);
    let from = 2;
    print(slice([1, 2, 3, 4, 5], from, 4));
    print(all == big);
    0
}
"#;

const WANT: &str = "18446744073709551615\n4794697086780616226\n18446744073709551615\n255\n255\n15\n2147483648\n1\n16\n[3, 4]\ntrue\n";

fn write_program(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_uint_literals_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crea el directorio temporal");
    let path = dir.join("prog.ray");
    std::fs::File::create(&path).expect("crea").write_all(PROGRAM.as_bytes()).expect("escribe");
    path
}

fn run(path: &std::path::Path, flag: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang")).arg(flag).arg(path).output().expect("lanza raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn vm_and_interpreter_agree() {
    let path = write_program("engines");
    let (vm, code) = run(&path, "--vm");
    assert_eq!(vm, WANT, "VM");
    assert_eq!(code, 0);
    let (interp, code) = run(&path, "--interp");
    assert_eq!(interp, WANT, "intérprete");
    assert_eq!(code, 0);
}

#[test]
fn native_agrees() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando native uint literals: rustc no disponible");
        return;
    }
    let path = write_program("native");
    let bin = path.with_file_name(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "--native", path.to_str().unwrap(), "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza ray build");
    assert!(build.status.success(), "build --native\n{}", String::from_utf8_lossy(&build.stderr));
    let out = Command::new(&bin).output().expect("corre el binario nativo");
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo");
    assert_eq!(out.status.code(), Some(0));
}
