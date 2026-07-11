//! Pruebas de `std/resilience` (M88.2) sobre el binario. El kit usa el reloj y el RNG (jitter)
//! → no determinista, así que (como `time_random_cli`) se comprueban **propiedades** (conteo de
//! llamadas, estado del breaker, monotonía del deadline) por subproceso, en ambos motores.

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

/// `retry` devuelve el primer Ok y no sigue reintentando tras acertar.
#[test]
fn retry_devuelve_el_primer_ok() {
    let src = r#"
import std/resilience;
fn main() -> int {
    var tries = [0];
    let r = resilience.retry(resilience.policy(5, 1, 4), fn() -> Result<int, string> {
        tries[0] = tries[0] + 1;
        if (tries[0] < 3) { Result.Err("boom") } else { Result.Ok(tries[0]) }
    });
    match (r) {
        Result.Ok(n) => print("ok " + to_string(n)),
        Result.Err(e) => print("err " + e),
    }
    print(tries[0]);
    0
}
"#;
    for vm in [false, true] {
        let (out, code) = run("ray_retry_ok", src, vm);
        assert_eq!(out, "ok 3\n3\n", "acierta al tercer intento y para (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

/// `retry` agotado devuelve el ÚLTIMO Err y llama a `f` exactamente `attempts` veces.
#[test]
fn retry_agotado_devuelve_el_ultimo_err() {
    let src = r#"
import std/resilience;
fn main() -> int {
    var tries = [0];
    let r = resilience.retry(resilience.policy(3, 1, 2), fn() -> Result<int, string> {
        tries[0] = tries[0] + 1;
        Result.Err("fallo " + to_string(tries[0]))
    });
    match (r) {
        Result.Ok(n) => print("ok"),
        Result.Err(e) => print(e),
    }
    print(tries[0]);
    0
}
"#;
    for vm in [false, true] {
        let (out, code) = run("ray_retry_err", src, vm);
        assert_eq!(out, "fallo 3\n3\n", "3 intentos y el último Err (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

/// El backoff duerme de verdad: con base 30 ms y 3 intentos pasan al menos ~90 ms
/// (30 + 60; el jitter solo suma). Holgura amplia para no ser flaky.
#[test]
fn retry_espera_el_backoff_entre_intentos() {
    let src = r#"
import std/resilience;
import std/time;
fn main() -> int {
    let t0 = time.monotonic();
    let r = resilience.retry(resilience.policy(3, 30, 200), fn() -> Result<int, string> {
        Result.Err("no")
    });
    let dt = time.monotonic() - t0;
    if (dt >= 70) { print("durmio") } else { print("corto " + to_string(dt)) }
    0
}
"#;
    for vm in [false, true] {
        let (out, code) = run("ray_retry_backoff", src, vm);
        assert!(out.contains("durmio"), "el backoff durmió entre intentos (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

/// El breaker abre tras `threshold` fallos seguidos y entonces falla en seco SIN llamar a `f`;
/// un éxito tras el cooldown lo cierra.
#[test]
fn breaker_abre_falla_en_seco_y_se_recupera() {
    let src = r#"
import std/resilience;
import std/time;
fn main() -> int {
    let b = resilience.breaker(2, 50);
    var calls = [0];
    let mal = fn() -> Result<int, string> { calls[0] = calls[0] + 1; Result.Err("down") };
    let bien = fn() -> Result<int, string> { calls[0] = calls[0] + 1; Result.Ok(1) };

    let a1 = resilience.guard(b, "abierto", mal);
    let a2 = resilience.guard(b, "abierto", mal);
    print(resilience.is_open(b));            // true: 2 fallos = threshold
    let a3 = resilience.guard(b, "abierto", mal);
    match (a3) {
        Result.Ok(n) => print("?"),
        Result.Err(e) => print(e),           // "abierto": fail-fast
    }
    print(calls[0]);                          // 2: la tercera NO llamó a f

    time.sleep(80);                           // pasa el cooldown
    print(resilience.is_open(b));            // false: semiabierto (deja probar)
    let a4 = resilience.guard(b, "abierto", bien);
    match (a4) {
        Result.Ok(n) => print("cerrado"),
        Result.Err(e) => print("? " + e),
    }
    print(resilience.is_open(b));            // false: el éxito lo cerró
    0
}
"#;
    for vm in [false, true] {
        let (out, code) = run("ray_breaker", src, vm);
        assert_eq!(
            out, "true\nabierto\n2\nfalse\ncerrado\nfalse\n",
            "abre, falla en seco y se recupera (vm={vm}): {out}"
        );
        assert_eq!(code, 0);
    }
}

/// Un éxito resetea la cuenta de fallos: fallos NO consecutivos no abren el breaker.
#[test]
fn breaker_solo_cuenta_fallos_consecutivos() {
    let src = r#"
import std/resilience;
fn main() -> int {
    let b = resilience.breaker(2, 1000);
    let mal = fn() -> Result<int, string> { Result.Err("down") };
    let bien = fn() -> Result<int, string> { Result.Ok(1) };
    let a1 = resilience.guard(b, "abierto", mal);
    let a2 = resilience.guard(b, "abierto", bien);   // resetea
    let a3 = resilience.guard(b, "abierto", mal);
    print(resilience.is_open(b));                    // false: nunca hubo 2 seguidos
    0
}
"#;
    for vm in [false, true] {
        let (out, code) = run("ray_breaker_consec", src, vm);
        assert_eq!(out, "false\n", "fallo-éxito-fallo no abre (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

/// El deadline es un presupuesto monótono: no expira recién creado, expira tras dormir más
/// que el presupuesto, y `remaining` nunca es negativo.
#[test]
fn deadline_presupuesto_monotono() {
    let src = r#"
import std/resilience;
import std/time;
fn main() -> int {
    let d = resilience.deadline(60);
    print(resilience.expired(d));       // false
    let r0 = resilience.remaining(d);
    if (r0 > 0 && r0 <= 60) { print("presupuesto") } else { print("raro " + to_string(r0)) }
    time.sleep(90);
    print(resilience.expired(d));       // true
    print(resilience.remaining(d));     // 0 (nunca negativo)
    0
}
"#;
    for vm in [false, true] {
        let (out, code) = run("ray_deadline", src, vm);
        assert_eq!(out, "false\npresupuesto\ntrue\n0\n", "deadline como presupuesto (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}
