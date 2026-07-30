//! IDEAS §55 — `std/units` importable: constructores de tamaño en bytes (convención binaria
//! 1024ⁿ). Verifica end-to-end el import de la std embebida + la forma UFCS (`64.kb()`, que exige
//! el import SIN calificar) y la calificada (`units.kb(64)`), en **ambos motores**.

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

const PROG: &str = r#"import std/units;
from std/units import kb, mb, gb;
fn main() -> int {
    print(64.kb());        // 65536
    print(16.mb());        // 16777216
    print(2.gb());         // 2147483648
    print(units.kb(64));   // 65536 (forma calificada)
    print(4.kb() + 512);   // 4608 (compone como int normal)
    0
}
"#;

#[test]
fn std_units_size_constructors() {
    let expected = "65536\n16777216\n2147483648\n65536\n4608\n";
    let (o_in, c_in) = run("units_size_in", PROG, false);
    let (o_vm, c_vm) = run("units_size_vm", PROG, true);
    assert_eq!(c_in, 0, "intérprete sale 0");
    assert_eq!(c_vm, 0, "vm sale 0");
    assert_eq!(o_in, expected, "output del intérprete");
    assert_eq!(o_vm, expected, "output de la vm");
}
