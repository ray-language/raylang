//! M191: `break` / `continue` en los tres motores (VM, intérprete, nativo) con la misma salida.
//! Cubre `while`, las cuatro formas de `for` (rango, arreglo, string, Map e `Iterator`), bucles
//! anidados (solo sale el interno), `break` como brazo de `match` que cede el tipo, y `continue`
//! como rama de un `if` en un `let`.

use std::io::Write;
use std::process::Command;

const PROGRAM: &str = r#"
struct Counter { n: int, limit: int }
impl Iterator<int> for Counter {
    fn next(self) -> Option<int> { if (self.n >= self.limit) { Option.None } else { self.n = self.n + 1; Option.Some(self.n) } }
}
fn main() -> int {
    var i = 0;
    var acc: [int] = [];
    while (true) {
        i = i + 1;
        if (i % 2 == 0) { continue; }
        if (i > 7) { break; }
        acc.push(i);
    }
    print(acc);
    var s = 0;
    for k in 0..100 { if (k == 5) { break; } if (k % 2 == 1) { continue; } s = s + k; }
    print(s);
    var seen = "";
    for c in "abcdef" { if (c == 'c') { continue; } if (c == 'e') { break; } seen = seen + to_string(c); }
    print(seen);
    let m: Map<string, int> = ["a": 1, "b": 2, "c": 3];
    var ks = "";
    for (k, v) in m { if (v == 2) { continue; } ks = ks + k; }
    print(ks);
    var it_sum = 0;
    let ctr = Counter { n: 0, limit: 10 };
    for x in ctr { if (x > 4) { break; } it_sum = it_sum + x; }
    print(it_sum);
    var pairs = 0;
    for a in 0..3 { for b in 0..3 { if (b == 1) { break; } pairs = pairs + 1; } }
    print(pairs);
    let xs: [Option<int>] = [Option.Some(1), Option.Some(2), Option.None, Option.Some(4)];
    var total = 0;
    for o in xs {
        let v = match (o) { Option.Some(n) => n, Option.None => { break; } };
        total = total + v;
    }
    print(total);
    var j = 0;
    while (j < 3) { j = j + 1; let w = if (j == 2) { continue; } else { j * 10 }; total = total + w; }
    print(total);
    0
}
"#;

const WANT: &str = "[1, 3, 5, 7]\n6\nabd\nac\n10\n3\n3\n43\n";

fn write_program(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_break_continue_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crea el directorio temporal");
    let path = dir.join("prog.ray");
    std::fs::File::create(&path).expect("crea").write_all(PROGRAM.as_bytes()).expect("escribe");
    path
}

fn run(path: &std::path::Path, flag: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(flag)
        .arg(path)
        .output()
        .expect("lanza raylang");
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
        eprintln!("saltando native break/continue: rustc no disponible");
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
