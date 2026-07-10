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
    print(math.PI > 3.14);      // true
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

/// M65.3 — `clamp` genérica sobre `Ord` (antes solo-int). Retrocompatible (int infiere T=int)
/// y funciona con float/string/tipo de usuario, como min/max.
#[test]
fn clamp_generica() {
    let src = r#"import std/math;
fn main() -> int {
    print(math.clamp(5, 1, 10));       // 5  (int, dentro)
    print(math.clamp(-3, 1, 10));      // 1  (por debajo)
    print(math.clamp(99, 1, 10));      // 10 (por encima)
    print(math.clamp(2.5, 0.0, 1.0));  // 1  (float)
    print(math.clamp("m", "a", "f"));  // f  (string, Ord lexicográfico)
    0
}
"#;
    let esperado = "5\n1\n10\n1\nf\n";
    let (o_in, c_in) = run("m65_clamp_in", src, false);
    let (o_vm, c_vm) = run("m65_clamp_vm", src, true);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, esperado, "salida del intérprete");
    assert_eq!(o_in, o_vm, "ambos motores coinciden");
}

/// M65.2 — trig inversa y compañía: los envoltorios `math.asin/acos/atan/atan2/log2/trunc`
/// compilan y corren por ambos motores (el cálculo lo cubre el oráculo de `vm.rs`).
#[test]
fn trig_inversa_y_compania() {
    let src = r#"import std/math;
fn main() -> int {
    print(math.atan2(1.0, 0.0) == math.PI / 2.0);  // true (ángulo de (0,1) = π/2)
    print(math.asin(1.0) == math.PI / 2.0);        // true
    print(math.acos(1.0));                         // 0
    print(math.atan(0.0));                         // 0
    print(math.log2(1024.0));                      // 10
    print(math.trunc(3.7));                        // 3
    print(math.trunc(0.0 - 3.7));                  // -3 (hacia cero, no floor)
    0
}
"#;
    let esperado = "true\ntrue\n0\n0\n10\n3\n-3\n";
    let (o_in, c_in) = run("m65_trig_in", src, false);
    let (o_vm, c_vm) = run("m65_trig_vm", src, true);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, esperado, "salida del intérprete");
    assert_eq!(o_in, o_vm, "ambos motores coinciden");
}

/// M65.1 — dos fixes de corrección: (a) `ipow` ya no trap-ea con resultados que caben (el
/// cuadrado final innecesario desbordaba el int checked: ipow(2,40) reventaba por 2^64);
/// (b) los empates de `min`/`max` devuelven `a`, como promete la doc (antes `b`; observable
/// con un `impl Ord` de usuario).
#[test]
fn ipow_sin_trap_y_empates_min_max() {
    let src = r#"import std/math;
@derive(Eq, Show)
struct P { orden: int, tag: string }
impl Ord for P {
    fn less(self, otro: P) -> bool { self.orden < otro.orden }
}
fn main() -> int {
    print(math.ipow(2, 40));      // 1099511627776 (antes: trap)
    print(math.ipow(2, 62));      // 4611686018427387904 (el mayor 2^n que cabe)
    print(math.ipow(3, 5));       // 243
    print(math.ipow(7, 0));       // 1
    print(math.ipow(5, -1));      // 0 (contrato: exp < 0 → 0)
    let a = P { orden: 1, tag: "a" };
    let b = P { orden: 1, tag: "b" };
    print(math.min(a, b).tag);    // a (empate → a)
    print(math.max(a, b).tag);    // a (empate → a)
    print(math.min(3, 8));        // 3 (no-empate intacto)
    print(math.max(1.5, 9.0));    // 9
    0
}
"#;
    let esperado = "1099511627776\n4611686018427387904\n243\n1\n0\na\na\n3\n9\n";
    let (o_in, c_in) = run("m65_ipow_in", src, false);
    let (o_vm, c_vm) = run("m65_ipow_vm", src, true);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, esperado, "salida del intérprete");
    assert_eq!(o_in, o_vm, "ambos motores coinciden");
}

#[test]
fn prefijo_global_ya_no_existe() {
    // M49.1a: la forma prefija global se retiró; `sqrt(x)` sin importar `std/math` es un error de tipos.
    let (_o, c) = run("m49_math_bad", "fn main() -> int { print(sqrt(16.0)); 0 }", true);
    assert_ne!(c, 0, "sqrt() global debe fallar (ya no es builtin)");
}
