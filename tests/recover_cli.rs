//! Pruebas de `try_call` (M97.2): recuperación de un fallo fatal como VALOR, en la MISMA fibra.
//!
//! A diferencia de `try_join` (solo-VM, porque `spawn` no corre en el intérprete), `try_call`
//! funciona en los **tres** motores — así que casi todas estas pruebas son de **oráculo**:
//! comparan intérprete ≡ VM sobre el mismo fuente, y las que llegan al nativo comparan nativo ≡ VM.
//! Ese oráculo cruzado es precisamente lo que el camino `spawn`+`try_join` no podía tener.

use std::process::Command;

/// Escribe `src` y devuelve la salida de cada motor: `(intérprete, vm)`.
fn both(name: &str, src: &str) -> (String, String) {
    let mut path = std::env::temp_dir();
    path.push(format!("recover_{name}.ray"));
    std::fs::write(&path, src).expect("escribe el fuente");

    let run = |flag: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
            .arg(flag)
            .arg(&path)
            .output()
            .expect("lanza raylang");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    (run("--interp"), run("--vm"))
}

/// Como `both`, pero además compila el nativo y devuelve su salida. `None` si no hay `rustc`
/// (entorno sin toolchain: se salta, no se falla).
fn all_three(name: &str, src: &str) -> (String, String, Option<String>) {
    let (interp, vm) = both(name, src);
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando el nativo de {name}: rustc no disponible");
        return (interp, vm, None);
    }
    let mut src_path = std::env::temp_dir();
    src_path.push(format!("recover_{name}.ray"));
    let mut bin = std::env::temp_dir();
    bin.push(format!("recover_{name}_bin{}", std::env::consts::EXE_SUFFIX));
    let built = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", src_path.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza ray build --native");
    assert!(
        built.status.success(),
        "build --native falla: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = Command::new(&bin).output().expect("corre el binario nativo");
    (interp, vm, Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

#[test]
fn recovers_an_explicit_panic_as_a_value_in_the_three_engines() {
    // El caso central: un `panic` dentro de `try_call` NO tumba el programa, se observa como
    // `Result.Err` con el mensaje, y la ejecución sigue. Los tres motores, byte a byte.
    let src = r#"
fn risky(n: int) -> int {
    if (n > 2) { panic("too big: " + to_string(n)); }
    n * 10
}
fn main() -> int {
    match (try_call(fn() -> int { risky(2) })) {
        Result.Ok(v) => print("ok " + to_string(v)),
        Result.Err(e) => print("err " + e),
    }
    match (try_call(fn() -> int { risky(5) })) {
        Result.Ok(v) => print("ok " + to_string(v)),
        Result.Err(e) => print("err " + e),
    }
    print("alive");
    0
}
"#;
    let (interp, vm, native) = all_three("panic", src);
    assert_eq!(interp, vm, "intérprete ≡ VM");
    assert_eq!(vm, "ok 20\nerr too big: 5\nalive\n", "recupera y sigue\n{vm}");
    if let Some(n) = native {
        assert_eq!(n, vm, "nativo ≡ VM");
    }
}

#[test]
fn try_calls_nest_and_the_inner_failure_does_not_escape_the_outer() {
    // Los marcadores anidan: el `try_call` de dentro recupera lo suyo, y un fallo POSTERIOR en el
    // cuerpo de fuera lo recupera el de fuera. Es la prueba de que la pila de marcadores se
    // desapila en el orden correcto (un solo marcador global daría "err inner" aquí).
    let src = r#"
fn main() -> int {
    let r = try_call(fn() -> int {
        match (try_call(fn() -> int { panic("inner") })) {
            Result.Ok(v) => v,
            Result.Err(_) => panic("outer"),
        }
    });
    match (r) {
        Result.Ok(v) => print("ok " + to_string(v)),
        Result.Err(e) => print("err " + e),
    }
    0
}
"#;
    let (interp, vm, native) = all_three("nested", src);
    assert_eq!(interp, vm, "intérprete ≡ VM");
    assert_eq!(vm, "err outer\n", "el de fuera gana el fallo posterior\n{vm}");
    if let Some(n) = native {
        assert_eq!(n, vm, "nativo ≡ VM");
    }
}

#[test]
fn recovers_a_runtime_error_not_only_an_explicit_panic() {
    // No solo `panic`: un índice fuera de rango también se recupera. **Solo intérprete ≡ VM**: el
    // nativo recupera igual (mismo flujo de control) pero el TEXTO difiere, porque allí el
    // indexado se apoya en el bounds check de Rust en vez de emitir su propia comprobación. Esa
    // divergencia de mensaje es preexistente (se ve igual en un fallo sin capturar) y está
    // documentada en `docs/investigacion-p999-webserver-nativo.md`; lo que este test fija es que
    // la RECUPERACIÓN ocurre, y que los dos motores del oráculo dicen lo mismo.
    let src = r#"
fn main() -> int {
    let xs: [int] = [1, 2];
    match (try_call(fn() -> int { xs[7] })) {
        Result.Ok(v) => print("ok " + to_string(v)),
        Result.Err(e) => print("err " + e),
    }
    match (try_call(fn() -> int { 1 / 0 })) {
        Result.Ok(v) => print("ok " + to_string(v)),
        Result.Err(e) => print("err " + e),
    }
    print("alive");
    0
}
"#;
    let (interp, vm) = both("runtime_error", src);
    assert_eq!(interp, vm, "intérprete ≡ VM");
    assert!(vm.contains("err index 7 out of range"), "recupera el índice\n{vm}");
    assert!(vm.contains("division by zero"), "recupera la división\n{vm}");
    assert!(vm.ends_with("alive\n"), "sigue vivo tras recuperar\n{vm}");
}

#[test]
fn state_mutated_before_the_failure_stays_mutated() {
    // El sharp edge documentado: `try_call` recupera en la MISMA fibra, así que lo que el cuerpo
    // mutó ANTES de fallar sigue mutado (a diferencia de `spawn`+`try_join`, que aísla de verdad).
    // Se fija como comportamiento observable para que nadie lo "arregle" sin decidirlo.
    let src = r#"
fn main() -> int {
    var xs: [int] = [];
    match (try_call(fn() { xs.push(1); xs.push(2); panic("stop"); })) {
        Result.Ok(_) => print("ok"),
        Result.Err(e) => print("err " + e),
    }
    print(xs.len());
    0
}
"#;
    let (interp, vm, native) = all_three("mutation", src);
    assert_eq!(interp, vm, "intérprete ≡ VM");
    assert_eq!(vm, "err stop\n2\n", "lo mutado antes del fallo persiste\n{vm}");
    if let Some(n) = native {
        assert_eq!(n, vm, "nativo ≡ VM");
    }
}

#[test]
fn a_successful_body_returns_its_value_and_leaves_no_residue_on_the_stack() {
    // Camino bueno en bucle: el `Return` que cierra el marcador entrega `[]` y el envoltorio saca
    // el valor del array capturado. Repetirlo N veces destaparía cualquier desalineación de la
    // pila de operandos de la VM (un valor de más por iteración se acumularía y reventaría).
    let src = r#"
fn main() -> int {
    var total: int = 0;
    var i: int = 0;
    while (i < 200) {
        match (try_call(fn() -> int { i * 2 })) {
            Result.Ok(v) => { total = total + v; },
            Result.Err(_) => print("should not fail"),
        }
        i = i + 1;
    }
    print(total);
    0
}
"#;
    let (interp, vm, native) = all_three("loop", src);
    assert_eq!(interp, vm, "intérprete ≡ VM");
    assert_eq!(vm, "39800\n", "suma de 2i para i en 0..200\n{vm}");
    if let Some(n) = native {
        assert_eq!(n, vm, "nativo ≡ VM");
    }
}

#[test]
fn recovers_inside_main_instead_of_aborting_the_program() {
    // En la VM, un fallo en `main` aborta el programa (`return Err`). El marcador de `try_call`
    // tiene que ganarle a ese camino, o `try_call` no serviría justo donde más se usa. El código
    // de salida 0 (no 70) es la aserción real.
    let src = r#"
fn main() -> int {
    match (try_call(fn() { panic("in main"); })) {
        Result.Ok(_) => print("ok"),
        Result.Err(e) => print("err " + e),
    }
    0
}
"#;
    let mut path = std::env::temp_dir();
    path.push("recover_in_main.ray");
    std::fs::write(&path, src).expect("escribe el fuente");
    for flag in ["--interp", "--vm"] {
        let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
            .arg(flag)
            .arg(&path)
            .output()
            .expect("lanza raylang");
        assert_eq!(out.status.code(), Some(0), "{flag}: sale 0, no 70 (el fallo se recuperó)");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "err in main\n",
            "{flag}: recupera dentro de main"
        );
    }
}
