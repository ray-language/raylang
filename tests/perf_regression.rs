//! Gate de regresión de rendimiento (M35c), descubrible desde `cargo test`.
//!
//! Es un envoltorio delgado sobre `benchmarks/regress.py`: compila la release y comprueba
//! los tiempos de la VM contra el baseline commiteado (`benchmarks/baseline.json`), fallando
//! si algún caso es >5% más lento. `#[ignore]` porque es LENTO (~1 min: build + mejor-de-7) y
//! no determinista → no va en la suite normal. Córrelo a demanda o en CI:
//!
//!   cargo test --test perf_regression -- --ignored --nocapture
//!
//! El baseline se grabó en la máquina del mantenedor (Apple M3 Pro). Como los tiempos
//! absolutos dependen del hardware, el gate es **estricto** solo si la huella de máquina casa
//! y **informativo** (no falla) si no — así este test es seguro en cualquier máquina, y de
//! verdad protege en la del baseline. En CI (runner consistente) se graba el baseline propio
//! una vez (`regress.py --record`, cacheado) y a partir de ahí el gate es estricto. Ver
//! `benchmarks/README.md`.

use std::process::Command;

#[test]
#[ignore = "lento (~1 min) y no determinista; córrelo con --ignored"]
fn no_performance_regression() {
    let root = env!("CARGO_MANIFEST_DIR");

    // 1. Compila la release (el gate mide `target/release/raylang`).
    let build = Command::new("cargo")
        .args(["build", "--release", "--quiet"])
        .current_dir(root)
        .status()
        .expect("lanza cargo build --release");
    assert!(build.success(), "la compilación de release falló");

    // 2. Corre el gate (no `--strict`: informativo si la máquina no casa con el baseline).
    let gate = Command::new("python3")
        .arg("benchmarks/regress.py")
        .current_dir(root)
        .status()
        .expect("lanza python3 benchmarks/regress.py");
    assert!(
        gate.success(),
        "regresión de rendimiento detectada (o falta el baseline / la release); \
         revisa la output de benchmarks/regress.py"
    );
}
