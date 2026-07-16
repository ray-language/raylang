//! Pruebas del reloj y la aleatoriedad (M15.1b) sobre el binario. Estos builtins son **no
//! deterministas** (dependen del reloj o del RNG) → no entran al oráculo VM↔intérprete. Se
//! comprueban **propiedades** (rangos, monotonía) por subproceso, en ambos motores, como el I/O.

use std::io::Write;
use std::process::Command;

/// Escribe `src` en un `.ray` temporal, ejecuta el binario (con `--vm` opcional) y devuelve
/// `(stdout, código)`.
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

/// `sleep` hace pasar al menos el tiempo pedido en el reloj monótono.
#[test]
fn monotonic_y_sleep_miden_intervalos() {
    let src = r#"
import std/time;
fn main() -> int {
    let t0: int = time.monotonic();
    time.sleep(50);
    let dt: int = time.monotonic() - t0;
    // Holgura amplia para no ser flaky: basta con que haya dormido "casi" lo pedido.
    if (dt >= 40) { print("ok") } else { print("corto") }
    0
}
"#;
    for vm in [false, true] {
        let (out, code) = run("ray_mono", src, vm);
        assert!(out.contains("ok"), "sleep durmió ~50ms (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

/// `now` devuelve un epoch en ms posterior a 2023 (cordura del reloj de pared).
#[test]
fn now_es_un_epoch_razonable() {
    let src = r#"
import std/time;
fn main() -> int {
    if (time.now() > 1700000000000) { print("ok") } else { print("mal") }
    0
}
"#;
    for vm in [false, true] {
        let (out, _) = run("ray_now", src, vm);
        assert!(out.contains("ok"), "now() es un epoch en ms razonable (vm={vm}): {out}");
    }
}

/// `random.next()` siempre cae en `[0, 1)`; `random.below(n)` en `[0, n)`; los casos de borde son totales.
#[test]
fn random_respects_sus_rangos() {
    let src = r#"
import std/random;
fn main() -> int {
    var i: int = 0;
    var outside: int = 0;
    while (i < 2000) {
        let r: float = random.next();
        if (r < 0.0 || r >= 1.0) { outside = outside + 1; }
        let x: int = random.below(6);
        if (x < 0 || x >= 6) { outside = outside + 1; }
        i = i + 1;
    }
    // Casos de borde: random.below(1) siempre 0; n<=0 → 0 (sin error).
    if (random.below(1) != 0) { outside = outside + 1; }
    if (random.below(0) != 0) { outside = outside + 1; }
    print(outside);
    0
}
"#;
    for vm in [false, true] {
        let (out, code) = run("ray_rand", src, vm);
        assert_eq!(out.trim(), "0", "random/random_int inside de range (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

/// El RNG produce variedad: 2000 tiradas de un dado tocan los 6 valores (probabilidad de fallo
/// despreciable: ~ (5/6)^2000). Detecta un generador "pegado".
#[test]
fn random_int_has_variedad() {
    let src = r#"
import std/random;
fn main() -> int {
    var caras: [int] = [0, 0, 0, 0, 0, 0];
    var i: int = 0;
    while (i < 2000) {
        let x: int = random.below(6);
        caras[x] = caras[x] + 1;
        i = i + 1;
    }
    var distintas: int = 0;
    var c: int = 0;
    while (c < 6) {
        if (caras[c] > 0) { distintas = distintas + 1; }
        c = c + 1;
    }
    print(distintas);
    0
}
"#;
    for vm in [false, true] {
        let (out, _) = run("ray_variedad", src, vm);
        assert_eq!(out.trim(), "6", "las 6 caras salieron al menos one vez (vm={vm}): {out}");
    }
}

/// M68.1 — `seed` vuelve el PRNG reproducible (SplitMix64 es aritmética entera pura → la
/// secuencia es estable entre plataformas y motores) y habilita el kit between/choice/shuffle
/// con golden DETERMINISTA. Re-sembrar reproduce la secuencia exacta.
#[test]
fn seed_reproducible_y_kit() {
    let src = r#"import std/random;
fn join_ints(xs: [int]) -> string {
    var out: [string] = [];
    var i = 0;
    while (i < xs.len()) { out.push(to_string(xs[i])); i = i + 1; }
    join(out, ",")
}
fn main() -> int {
    random.seed(42);
    print(random.below(100));
    print(random.below(100));
    print(random.between(1, 6));
    print(random.between(5, 5));
    print(random.between(9, 3));
    let xs = [1, 2, 3, 4, 5, 6, 7, 8];
    random.shuffle(xs);
    print(join_ints(xs));
    var empty: [int] = [];
    match (random.choice(empty)) {
        Option.Some(v) => print(v),
        Option.None => print("None ok"),
    }
    random.seed(42);
    print(random.below(100));
    0
}
"#;
    let expected = "13\n91\n1\n5\n9\n1,7,8,6,4,2,5,3\nNone ok\n13\n";
    let (o_in, c_in) = run("m68_seed_in", src, false);
    let (o_vm, c_vm) = run("m68_seed_vm", src, true);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, expected, "secuencia determinista con seed(42)");
    assert_eq!(o_in, o_vm, "ambos engines match");
}
