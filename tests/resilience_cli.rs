//! M88.2 — `std/resilience`: retry con backoff+jitter, circuit breaker y deadline. Raylang
//! puro; golden determinista sembrando el PRNG (`random.seed`) para que el jitter no varíe,
//! y contando intentos con un contador mutable (struct por referencia). Ambos motores.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

fn run(flags: &[&str], main: &str) -> (String, i32) {
    let dir = std::env::temp_dir().join("ray_resilience");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("m.ray");
    std::fs::write(&path, main).unwrap();
    let out = Command::new(BIN).args(flags).arg(&path).output().expect("ejecuta");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

const PROG: &str = r#"import std/resilience;
import std/random;

struct Contador { n: int }

// Falla las primeras `fallar` veces (Err int), luego Ok.
fn intento(c: Contador, fallar: int) -> Result<int, int> {
    c.n = c.n + 1;
    if (c.n <= fallar) {
        Result.Err(c.n)
    } else {
        Result.Ok(c.n)
    }
}

fn main() -> int {
    random.seed(42);
    // Retry: 5 intentos, falla 2 → Ok al 3.º. base 1ms (dormir mínimo).
    let p = resilience.policy(5, 1, 10);
    let c1 = Contador { n: 0 };
    match (resilience.retry(p, fn() -> Result<int, int> { intento(c1, 2) })) {
        Result.Ok(v) => print("retry ok after " + to_string(v) + " intentos"),
        Result.Err(e) => print("retry err " + to_string(e)),
    }
    // Retry agotado: falla siempre → devuelve el último Err.
    let c2 = Contador { n: 0 };
    let p2 = resilience.policy(3, 1, 10);
    match (resilience.retry(p2, fn() -> Result<int, int> { intento(c2, 99) })) {
        Result.Ok(v) => print("no debería"),
        Result.Err(e) => print("retry agotado en " + to_string(e)),
    }
    // Breaker: threshold 2. Dos fallos lo ABREN → la 3.ª llamada NO ejecuta (fail-fast).
    let c3 = Contador { n: 0 };
    let b = resilience.breaker(2, 100000);
    var i = 0;
    while (i < 4) {
        let r = resilience.guard(b, 0 - 1, fn() -> Result<int, int> { intento(c3, 99) });
        match (r) {
            Result.Ok(v) => print("guard ok"),
            Result.Err(e) => print("guard err " + to_string(e)),
        }
        i = i + 1;
    }
    // c3.n == 2: solo se ejecutó f dos veces; los intentos 3 y 4 fueron fail-fast (err -1).
    print("f corrió " + to_string(c3.n) + " veces; abierto=" + to_string(resilience.is_open(b)));
    // Deadline: un presupuesto de 100s no expira de inmediato.
    let d = resilience.deadline(100000);
    print("deadline expirado=" + to_string(resilience.expired(d)));
    0
}
"#;

const ESPERADO: &str = "retry ok after 3 intentos\n\
retry agotado en 3\n\
guard err 1\n\
guard err 2\n\
guard err -1\n\
guard err -1\n\
f corrió 2 veces; abierto=true\n\
deadline expirado=false\n";

#[test]
fn resilience_ambos_engines() {
    let (o_in, c_in) = run(&["--interp"], PROG);
    let (o_vm, c_vm) = run(&["--vm"], PROG);
    assert_eq!(c_in, 0, "intérprete 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm 0\n{o_vm}");
    assert_eq!(o_in, o_vm, "ambos engines match");
    assert_eq!(o_in, ESPERADO, "output expected_val");
}
