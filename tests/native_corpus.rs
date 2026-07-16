//! H10 — Corpus automatizado del backend nativo (`ray build --native`). El claim "los ejemplos
//! transpilan byte-idénticos a la VM" (docs/transpilador-nativo.md §2.3) dejaba de estar cubierto por
//! ningún test: era una afirmación manual que se descompasaba en silencio. Este test lo AUTOMATIZA:
//! itera los ejemplos DETERMINISTAS (`examples/{basics,data,types,stdlib}`) y, para cada uno, compila el
//! binario nativo, lo ejecuta y exige que su stdout + código de salida coincidan con la VM.
//!
//! Es un GUARDIA de regresión: si un cambio del transpilador rompe un ejemplo (o uno nuevo usa algo fuera
//! del subconjunto), el test falla. Los ejemplos que el backend nativo NO soporta o que no son
//! deterministas están en `EXCLUIDOS` con su motivo.
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
const EXCLUIDOS: &[(&str, &str)] = &[
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

fn tiene_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn ejemplos_de(dir: &str) -> Vec<PathBuf> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(dir);
    let mut v: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap_or_else(|e| panic!("no se pudo leer {}: {e}", base.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map_or(false, |x| x == "ray"))
        .collect();
    v.sort(); // orden estable → salida reproducible
    v
}

fn excluido(p: &Path) -> Option<&'static str> {
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
    EXCLUIDOS.iter().find(|(n, _)| *n == name).map(|(_, motivo)| *motivo)
}

#[test]
#[ignore = "compila ~50 binarios nativos con rustc (~2-3 min); correr con -- --ignored"]
fn los_ejemplos_deterministas_transpilan_identicos_a_la_vm() {
    if !tiene_rustc() {
        eprintln!("saltando native_corpus: rustc no disponible");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("ray_corpus_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("crea el dir temporal");

    let mut cubiertos = 0usize;
    let mut saltados = 0usize;
    let mut fallos: Vec<String> = Vec::new();

    for dir in DIRS {
        for ejemplo in ejemplos_de(dir) {
            if let Some(motivo) = excluido(&ejemplo) {
                eprintln!("· saltado {}/{}: {motivo}", dir, ejemplo.file_name().unwrap().to_string_lossy());
                saltados += 1;
                continue;
            }
            let etiqueta = format!("{}/{}", dir, ejemplo.file_name().unwrap().to_string_lossy());
            let src = ejemplo.to_str().unwrap();
            let bin = tmp.join(ejemplo.file_stem().unwrap());

            // (1) La VM: oráculo de referencia.
            let vm = Command::new(BIN).args(["run", src]).output().expect("corre la VM");
            let vm_out = String::from_utf8_lossy(&vm.stdout).into_owned();
            let vm_code = vm.status.code();

            // (2) El binario nativo debe COMPILAR (si un ejemplo nuevo usa algo fuera del subconjunto, esto
            //     falla → hay que soportarlo o añadirlo a EXCLUIDOS con su motivo).
            let build = Command::new(BIN)
                .args(["build", src, "--native", "-o", bin.to_str().unwrap()])
                .output()
                .expect("lanza el build --native");
            if !build.status.success() {
                fallos.push(format!(
                    "{etiqueta}: build --native falló\n  {}",
                    String::from_utf8_lossy(&build.stderr).trim()
                ));
                continue;
            }
            // (3) El binario nativo ≡ VM (stdout + código de salida).
            let nat = Command::new(&bin).output().expect("corre el binario nativo");
            let nat_out = String::from_utf8_lossy(&nat.stdout).into_owned();
            if nat_out != vm_out {
                fallos.push(format!("{etiqueta}: stdout diverge\n  VM: {vm_out:?}\n  nativo: {nat_out:?}"));
            } else if nat.status.code() != vm_code {
                fallos.push(format!(
                    "{etiqueta}: código de salida diverge (VM={vm_code:?}, nativo={:?})",
                    nat.status.code()
                ));
            } else {
                cubiertos += 1;
            }
            let _ = std::fs::remove_file(&bin);
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);

    eprintln!("native_corpus: {cubiertos} ejemplos ≡ VM, {saltados} saltados (excluidos)");
    assert!(
        fallos.is_empty(),
        "el backend nativo diverge de la VM en {} ejemplo(s):\n{}",
        fallos.len(),
        fallos.join("\n")
    );
    // Salvaguarda: si el corpus cae a ~0 (p. ej. un cambio rompe TODOS los builds), es señal de alarma.
    assert!(cubiertos >= 40, "el corpus cubrió solo {cubiertos} ejemplos (¿regresión masiva del backend?)");
}
