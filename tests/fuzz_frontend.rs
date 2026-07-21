//! Fuzzing determinista del front-end (M42.3) — **sin dependencias de Cargo** (invariante del proyecto).
//!
//! Invariante que verifica: el front-end completo (`lex → parse → check → compile`) **nunca debe hacer
//! *panic* de Rust** ante una entrada arbitraria. Una entrada malformada es un **error de usuario** (`Err`
//! limpio), no un bug del compilador. Un panic aquí sería un **ICE** (la red de `lib::with_big_stack_or_ice`,
//! exit 101, "reporta este bug"): correcto para invariantes rotas, pero jamás debe dispararlo *input del
//! usuario*. Este fuzzer busca exactamente esos casos. **No ejecuta la VM** a propósito: el front-end es puro
//! y determinista, mientras que ejecutar código mutado dispararía I/O real y bloqueos que no se pueden acotar
//! (un ejemplo de servidor mutado se queda en `accept`); la robustez de la ejecución es otra cosa (M42.1/2).
//!
//! **Por qué propio y no cargo-fuzz**: cargo-fuzz exige nightly + `libfuzzer-sys`/`arbitrary` (deps de
//! Cargo) y un crate aparte. Un fuzzer determinista sembrado, escrito a mano (como el SHA-256 de M39c-2b o
//! el JSON del LSP), respeta la invariante cero-deps, corre en **stable** dentro de `cargo test` (→ fuzzing
//! **continuo**: cada corrida de tests/CI fuzzea) y es **reproducible** (semilla fija → misma secuencia).
//!
//! **Reproducir un fallo**: el mensaje de pánico imprime la semilla de la iteración culpable y escribe la
//! entrada exacta a un archivo; `RAYLANG_FUZZ_SEED=<n>` re-ejecuta esa sola entrada. `RAYLANG_FUZZ_ITERS=<n>`
//! sube el presupuesto (CI corre más iteraciones que el `cargo test` local).

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use raylang::{checker, compiler, lexer, parser};

/// PRNG **SplitMix64** (dominio público, de Steele/Lea) — un generador diminuto y determinista, sin deps.
/// Sembrado por iteración: un fallo se reproduce con su semilla exacta. No es criptográfico (no hace falta:
/// solo queremos una secuencia de mutaciones barajada y reproducible).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        // Avance del estado + mezcla de avalancha (constantes canónicas de SplitMix64).
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Entero en `[0, n)` (n > 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }
}

/// Recolecta recursivamente los `.ray` bajo `dir` (corpus de semillas para la mutación). Sin `walkdir`:
/// un recorrido a mano con `std::fs`, como el resto del proyecto.
fn collect_ray(dir: &Path, acc: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_ray(&p, acc);
        } else if p.extension().is_some_and(|x| x == "ray") {
            acc.push(p);
        }
    }
}

/// Carga el corpus: los fuentes reales del repo (ejemplos + self-hosting + std). Son entradas **válidas y
/// variadas** — mutarlas explora el vecindario de lo real (donde viven los bugs interesantes), mucho más
/// fértil que solo bytes al azar. Trunca cada semilla a `MAX_SEMILLA` para acotar la profundidad de anidación
/// (y con ella la recursión del parser: una entrada de N bytes anida a lo sumo ~N niveles).
fn load_corpus() -> Vec<String> {
    const MAX_SEMILLA: usize = 4096;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    for sub in ["examples", "selfhost", "std"] {
        collect_ray(&root.join(sub), &mut paths);
    }
    let mut corpus: Vec<String> = paths
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        // Truncar por CARACTERES, no por bytes: `String::truncate` en un byte a mitad de un carácter
        // multibyte (los acentos de los comentarios en español) panicaría. `take` por char es seguro.
        .map(|s| s.chars().take(MAX_SEMILLA).collect())
        .collect();
    // Unos cuantos snippets mínimos garantizan variedad de tokens aunque el corpus del disco falte.
    for s in [
        "fn main() -> int { 0 }",
        "fn f<T: Show>(x: T) -> string { x.show() }",
        "enum E { A, B(int) } fn main() -> int { match (E.A) { E.A => 0, E.B(n) => n } }",
        "struct P { x: int, y: int }",
        "extern \"m\" { fn sqrt(x: float) -> float; }",
    ] {
        corpus.push(s.to_string());
    }
    corpus
}

/// Fabrica una entrada de fuzz a partir del corpus, con una de varias estrategias barajadas. Todas producen
/// entradas **acotadas en tamaño** (→ profundidad de anidación acotada → la recursión del parser no desborda
/// ni siquiera la pila normal, y menos la de 256 MiB en la que corremos).
fn generate(rng: &mut Rng, corpus: &[String]) -> Vec<u8> {
    let base = corpus[rng.below(corpus.len())].as_bytes().to_vec();
    match rng.below(6) {
        // (0) Bytes puros al azar: el caso extremo (casi nunca lexéa, estresa el lexer).
        0 => {
            let n = 1 + rng.below(256);
            (0..n).map(|_| rng.byte()).collect()
        }
        // (1) Mutación de bytes: voltea/reemplaza unas pocas posiciones de una semilla real.
        1 => {
            let mut v = base;
            if !v.is_empty() {
                let edits = 1 + rng.below(8);
                for _ in 0..edits {
                    let i = rng.below(v.len());
                    v[i] = rng.byte();
                }
            }
            v
        }
        // (2) Inserción: mete bytes al azar en posiciones al azar (rompe el balance de llaves/paréntesis).
        2 => {
            let mut v = base;
            let ins = 1 + rng.below(8);
            for _ in 0..ins {
                let i = if v.is_empty() { 0 } else { rng.below(v.len() + 1) };
                v.insert(i, rng.byte());
            }
            v
        }
        // (3) Borrado: quita tramos (deja el fuente truncado a media construcción).
        3 => {
            let mut v = base;
            let deletions = 1 + rng.below(8);
            for _ in 0..deletions {
                if v.is_empty() {
                    break;
                }
                let i = rng.below(v.len());
                v.remove(i);
            }
            v
        }
        // (4) Truncado brusco: corta la entrada en un punto al azar (EOF en medio de un token/bloque).
        4 => {
            let mut v = base;
            if !v.is_empty() {
                v.truncate(rng.below(v.len()));
            }
            v
        }
        // (5) Duplicar-y-empalmar dos semillas (mezcla declaraciones de fuentes distintos).
        _ => {
            let other = corpus[rng.below(corpus.len())].as_bytes();
            let cut_a = if base.is_empty() { 0 } else { rng.below(base.len()) };
            let cut_b = if other.is_empty() { 0 } else { rng.below(other.len()) };
            let mut v = base[..cut_a].to_vec();
            v.extend_from_slice(&other[cut_b..]);
            v.truncate(4096);
            v
        }
    }
}

/// Corre una entrada por el **front-end completo** (`lex → parse → check → compile`), capturando cualquier
/// *panic* de Rust. Devuelve `Err(mensaje)` si panicó (el fallo que buscamos); `Ok(())` si el pipeline
/// terminó normal (con éxito o con un `Err` limpio, que es lo esperado ante basura). El lexer rechaza los
/// bytes no-UTF-8, así que probamos solo entradas UTF-8 válidas (una entrada binaria pura es un error de
/// I/O del CLI, no del compilador).
///
/// **Deliberadamente NO ejecuta la VM.** El front-end es **puro y determinista** (sin I/O), justo lo que
/// queremos endurecer: el *compilador* jamás debe panicar ante input arbitrario. Ejecutar código mutado, en
/// cambio, dispara **efectos y bloqueos reales** que ningún límite del fuzzer acota: un ejemplo de servidor
/// del corpus, tras mutar, compila a un programa válido que hace `listen`/`accept` y **se bloquea en el
/// syscall** (el fuel cuenta instrucciones, no esperas de I/O), además de abrir sockets/archivos y ser
/// no-determinista. La robustez de la *ejecución* de código no confiable es otra cosa (y sus recursos —fuel,
/// tope de heap— se prueban en M42.1/42.2, sobre programas propios, no aquí).
fn exercise(input: &str) -> Result<(), String> {
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let Ok(tokens) = lexer::lex(input) else {
            return;
        };
        let Ok(mut program) = parser::parse(tokens) else {
            return;
        };
        if checker::check(&mut program).is_err() {
            return;
        }
        // Bajar a bytecode es la última fase pura; NO se ejecuta (ver el doc de la función).
        let _ = compiler::compile_program(&program);
    }));
    result.map_err(|payload| {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "pánico sin mensaje".to_string())
    })
}

/// Lee una variable de entorno como `usize`, con default.
fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

/// El fuzzer determinista. Corre en el hilo de pila grande (`with_big_stack`) por margen frente a entradas
/// muy anidadas, y con el *panic hook* silenciado (si no, cada `catch_unwind` volcaría una traza al stderr;
/// nosotros ya reportamos lo necesario). Semilla base fija → secuencia reproducible; cada iteración deriva su
/// propia semilla, así el mensaje de fallo apunta a la entrada exacta.
#[test]
fn fuzz_el_frontend_no_panica() {
    raylang::with_big_stack(|| {
        const SEMILLA_BASE: u64 = 0x5241_594C_414E_47; // "RAYLANG" en ASCII (determinismo).
        let corpus = load_corpus();
        assert!(!corpus.is_empty(), "el corpus de fuzz no debería estar vacío");

        // Modo reproducción: `RAYLANG_FUZZ_SEED=<n>` ejercita una sola entrada (la de esa semilla) y para.
        let iters = env_usize("RAYLANG_FUZZ_ITERS", 3000);
        let seed_fixes = std::env::var("RAYLANG_FUZZ_SEED").ok().and_then(|s| s.parse::<u64>().ok());

        // Silenciamos el hook de pánico durante el fuzzing y lo restauramos al terminar.
        let previous_hook = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));

        let mut failure: Option<(u64, Vec<u8>, String)> = None;
        let range: Vec<u64> = match seed_fixes {
            Some(s) => vec![s],
            None => (0..iters as u64).map(|i| SEMILLA_BASE.wrapping_add(i.wrapping_mul(0x9E37_79B9))).collect(),
        };

        for seed in range {
            let mut rng = Rng(seed);
            let bytes = generate(&mut rng, &corpus);
            // Solo UTF-8 válido llega al compilador (el CLI lee el archivo como texto); el resto se descarta.
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            if let Err(msg) = exercise(text) {
                failure = Some((seed, bytes.clone(), msg));
                break;
            }
        }

        panic::set_hook(previous_hook);

        if let Some((seed, bytes, msg)) = failure {
            // Escribe la entrada culpable para inspección/repro y falla con instrucciones claras.
            let crash_input_path = std::env::temp_dir().join(format!("raylang_fuzz_repro_{seed}.ray"));
            let _ = std::fs::write(&crash_input_path, &bytes);
            panic!(
                "el front-end PANICÓ ante one entry de fuzz (esto es un ICE what input de user nunca must \
                 causar).\n  seed: {seed}\n  pánico: {msg}\n  entry escrita en: {}\n  reproducir: \
                 RAYLANG_FUZZ_SEED={seed} cargo test --test fuzz_frontend",
                crash_input_path.display()
            );
        }
    });
}
