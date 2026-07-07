//! M49.1a — `std/math` importable. Verifica end-to-end el **envoltorio** `math.sqrt(...)` (import +
//! resolución de la std embebida en el binario + bajada al primitivo `__x`), en **ambos motores**. El
//! cálculo en sí (los opcodes) lo cubre el oráculo `matematicas_oraculo` de `vm.rs` sobre los `__x`;
//! aquí se cierra la parte de front-end (que `import std/math; math.sqrt(2.0)` compila y corre).

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

const PROG: &str = r#"import std/math;
fn main() -> int {
    print(math.sqrt(16.0));       // 4
    print(math.pow(2.0, 10.0));   // 1024
    print(math.floor(3.7));       // 3
    print(math.ceil(3.2));        // 4
    print(math.round(2.5));       // 3
    print(math.sin(0.0));         // 0
    print(math.cos(0.0));         // 1
    print(math.log10(1000.0));    // 3
    // M49.1b: abs/min/max genéricos (Signed/Ord) + pi/e nularias.
    print(math.abs(-7));          // 7   (int)
    print(math.abs(-2.5));        // 2.5 (float)
    print(math.min(3, 8));        // 3   (int)
    print(math.max(1.5, 9.0));    // 9   (float)
    print(math.min("b", "a"));    // a   (Ord: string)
    print(math.pi() > 3.14);      // true
    0
}
"#;

#[test]
fn std_math_envoltorios() {
    let esperado = "4\n1024\n3\n4\n3\n0\n1\n3\n7\n2.5\n3\n9\na\ntrue\n";
    let (o_in, c_in) = run("m49_math_in", PROG, false);
    let (o_vm, c_vm) = run("m49_math_vm", PROG, true);
    assert_eq!(c_in, 0, "intérprete sale 0");
    assert_eq!(c_vm, 0, "vm sale 0");
    assert_eq!(o_in, esperado, "salida del intérprete");
    assert_eq!(o_vm, esperado, "salida de la vm");
    assert_eq!(o_in, o_vm, "ambos motores coinciden");
}

#[test]
fn prefijo_global_ya_no_existe() {
    // M49.1a: la forma prefija global se retiró; `sqrt(x)` sin importar `std/math` es un error de tipos.
    let (_o, c) = run("m49_math_bad", "fn main() -> int { print(sqrt(16.0)); 0 }", true);
    assert_ne!(c, 0, "sqrt() global debe fallar (ya no es builtin)");
}
