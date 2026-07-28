//! H10 — Corpus automatizado del backend nativo (`ray build --native`). El claim "los ejemplos
//! transpilan byte-idénticos a la VM" (docs/transpilador-nativo.md §2.3) dejaba de estar cubierto por
//! ningún test: era una afirmación manual que se descompasaba en silencio. Este test lo AUTOMATIZA:
//! itera los ejemplos DETERMINISTAS (`examples/{basics,data,types,stdlib}` + los EXTRAS multi-archivo y
//! el camino Cargo) y, para cada uno, compila el binario nativo, lo ejecuta y exige que su stdout +
//! código de salida coincidan con la VM.
//!
//! Es un GUARDIA de regresión: si un cambio del transpilador rompe un ejemplo (o uno nuevo usa algo fuera
//! del subconjunto), el test falla. Los ejemplos que el backend nativo NO soporta o que no son
//! deterministas están en `EXCLUDED` con su motivo.
//!
//! `#[ignore]` porque compila ~50 binarios con rustc (lento, ~2-3 min). Correr con:
//!   cargo test --test native_corpus -- --ignored
//! La concurrencia (`examples/concurrency`) queda fuera: tiene tests nativos DEDICADOS en `cli_cli.rs`
//! (CSP, rendezvous, spawn) y algunos ejemplos no terminan sin señal externa (`senales.ray`).

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Ejemplos NO cubiertos, con su motivo (se saltan al iterar). Si el backend nativo llega a soportarlos,
/// quitarlos de aquí y el corpus los cubrirá automáticamente.
const EXCLUDED: &[(&str, &str)] = &[
    // Módulos-librería: sin `main` ejecutable (se importan desde su `*_demo.ray`). `ray run`/`--native`
    // sobre ellos no produce un programa.
    ("csv.ray", "módulo-librería sin main (se usa vía csv_demo.ray)"),
    ("regex.ray", "módulo-librería sin main (se usa vía regex_demo.ray)"),
    ("template.ray", "módulo-librería sin main (se usa vía template_demo.ray)"),
    ("toml.ray", "módulo-librería sin main (se usa vía toml_demo.ray)"),
    // Usa `std::collections::deque::pop_back`, aún no soportado en el backend nativo (stub que panica).
    ("builder_deque.ray", "usa deque::pop_back, no soportado en nativo (stub)"),
];

/// Directorios de ejemplos DETERMINISTAS que el corpus cubre.
const DIRS: &[&str] = &["basics", "data", "types", "stdlib"];

/// Entradas EXTRA fuera de los directorios planos (revisión post-H10): los programas MULTI-ARCHIVO
/// (ejercitan la dimensión loader→transpile: bandas de líneas, namespacing `::`, cápsulas) y
/// `sqlite_demo` (el ÚNICO del corpus por el camino Cargo/ray-runtime, que los 50 planos nunca tocan;
/// determinista: SQLite en `:memory:`). Cada una corre con cwd en su directorio (resuelve sus imports).
const EXTRAS: &[&str] = &["modulos/main.ray", "capsula/main.ray", "proyecto/main.ray", "db/sqlite_demo.ray"];

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn examples_in(dir: &str) -> Vec<PathBuf> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(dir);
    let mut v: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap_or_else(|e| panic!("no se pudo leer {}: {e}", base.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map_or(false, |x| x == "ray"))
        .collect();
    v.sort(); // orden estable → salida reproducible
    v
}

fn excluded(p: &Path) -> Option<&'static str> {
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
    EXCLUDED.iter().find(|(n, _)| *n == name).map(|(_, motivo)| *motivo)
}

#[test]
#[ignore = "compila ~50 binarios nativos con rustc (~2-3 min); correr con -- --ignored"]
fn the_deterministic_examples_transpile_identically_to_the_vm() {
    run_corpus(&[]);
}

/// F2 (arco de concurrencia nativa): el MISMO corpus con `--fibers` — la concurrencia sobre el
/// scheduler M:N de ray_runtime::fibers debe seguir siendo byte-idéntica a la VM. Todo va por la
/// vía Cargo (corosensei), así que tarda más que el corpus plano.
#[test]
#[ignore = "compila ~50 binarios nativos vía cargo (--fibers, lento); correr con -- --ignored"]
fn the_deterministic_examples_transpile_identically_with_fibers() {
    run_corpus(&["--fibers"]);
}

/// Los dos corpus (plano y --fibers) compilan LOS MISMOS programas: el pkg de la caché Cargo
/// compartida (H14) sale de la ruta canónica del fuente, así que dos builds concurrentes del mismo
/// ejemplo pisan el mismo artefacto (cazado: "could not copy the binary" al correr ambos tests a
/// la vez). Se serializan aquí. La carrera de fondo (dos `ray build --native` concurrentes del
/// mismo fuente en una máquina) es preexistente y queda anotada en IDEAS.
static CORPUS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn run_corpus(extra_flags: &[&str]) {
    let _serial = CORPUS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !has_rustc() {
        // En LOCAL el skip es honesto (no todo el mundo tiene rustc a mano). Bajo CI sería un falso
        // verde silencioso — exactamente lo que este corpus existe para impedir → fallo duro.
        assert!(
            std::env::var_os("CI").is_none(),
            "rustc no disponible bajo CI: el corpus nativo daría un falso verde"
        );
        eprintln!("saltando native_corpus: rustc no disponible");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("ray_corpus_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("crea el dir temporal");

    let mut covered = 0usize;
    let mut skipped = 0usize;
    let mut failures: Vec<String> = Vec::new();

    // Cubre UN ejemplo: VM (oráculo) → build --native → binario ≡ VM (stdout + código de salida).
    // `cwd` fija el directorio de trabajo (los EXTRAS resuelven sus imports/manifiesto relativo a él).
    let mut cover = |label: &str, src: &str, cwd: &Path, bin: &Path,
                      covered: &mut usize, failures: &mut Vec<String>| {
        // (1) La VM: oráculo de referencia.
        let vm = Command::new(BIN).args(["run", src]).current_dir(cwd).output().expect("corre la VM");
        let vm_out = String::from_utf8_lossy(&vm.stdout).into_owned();
        let vm_code = vm.status.code();

        // (2) El binario nativo debe COMPILAR (si un ejemplo nuevo usa algo fuera del subconjunto, esto
        //     falla → hay que soportarlo o añadirlo a EXCLUDED con su motivo).
        let mut args: Vec<&str> = vec!["build", src, "--native", "-o", bin.to_str().unwrap()];
        args.extend_from_slice(extra_flags);
        let build = Command::new(BIN)
            .args(&args)
            .current_dir(cwd)
            .output()
            .expect("lanza el build --native");
        if !build.status.success() {
            failures.push(format!(
                "{label}: build --native falló\n  {}",
                String::from_utf8_lossy(&build.stderr).trim()
            ));
            return;
        }
        // (3) El binario nativo ≡ VM (stdout + código de salida).
        let nat = Command::new(bin).output().expect("corre el binario nativo");
        let nat_out = String::from_utf8_lossy(&nat.stdout).into_owned();
        if nat_out != vm_out {
            failures.push(format!("{label}: stdout diverge\n  VM: {vm_out:?}\n  nativo: {nat_out:?}"));
        } else if nat.status.code() != vm_code {
            failures.push(format!(
                "{label}: código de salida diverge (VM={vm_code:?}, nativo={:?})",
                nat.status.code()
            ));
        } else {
            *covered += 1;
        }
        let _ = std::fs::remove_file(bin);
    };

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    for dir in DIRS {
        for ejemplo in examples_in(dir) {
            if let Some(motivo) = excluded(&ejemplo) {
                eprintln!("· saltado {}/{}: {motivo}", dir, ejemplo.file_name().unwrap().to_string_lossy());
                skipped += 1;
                continue;
            }
            let label = format!("{}/{}", dir, ejemplo.file_name().unwrap().to_string_lossy());
            let bin = tmp.join(ejemplo.file_stem().unwrap());
            cover(&label, ejemplo.to_str().unwrap(), &root, &bin, &mut covered, &mut failures);
        }
    }
    // Los EXTRAS corren con cwd en SU directorio y un nombre de binario propio (dos se llaman main.ray).
    for extra in EXTRAS {
        let path = root.join("examples").join(extra);
        let cwd = path.parent().unwrap().to_path_buf();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let bin = tmp.join(extra.replace('/', "_").replace(".ray", ""));
        cover(extra, &name, &cwd, &bin, &mut covered, &mut failures);
    }
    let _ = std::fs::remove_dir_all(&tmp);

    eprintln!("native_corpus: {covered} ejemplos ≡ VM, {skipped} saltados (excluidos)");
    assert!(
        failures.is_empty(),
        "el backend nativo diverge de la VM en {} ejemplo(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
    // Salvaguarda: si el corpus cae a ~0 (p. ej. un cambio rompe TODOS los builds), es señal de alarma.
    assert!(covered >= 40, "el corpus cubrió solo {covered} ejemplos (¿regresión masiva del backend?)");
}
