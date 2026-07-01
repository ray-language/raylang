//! Fuzzing del front-end (M33d, DESIGN §38.4): bombardear lex→parse→check (+ rayfmt) con
//! mutaciones de un corpus real y comprobar que NINGUNA entrada tumba el proceso — todo
//! debe acabar en un diagnóstico (o, si hay bug, en un pánico que este arnés reporta).
//!
//! **Arnés propio, cero dependencias** (decisión de §38.4): mutador + PRNG SplitMix64
//! propios, en vez de `cargo-fuzz` (nightly + libFuzzer; el coverage-guided queda
//! diferido). El corpus semilla son los `.ray` reales del repo (`examples/` + `selfhost/`).
//!
//! Objetivo: `lsp::analizar` (corre el front-end SIN ejecutar — fuzzeamos el compilador,
//! no el programa del usuario) y `fmt::format_source`. Cada caso corre en un hilo con
//! pila grande; un `join` con `Err` = pánico = hallazgo: la entrada se guarda en
//! `target/fuzz/` y el test falla con la ruta y la semilla (reproducible).
//!
//! Dos marchas: el *smoke* determinista corre en la suite (semilla fija, casos acotados);
//! la campaña larga es `#[ignore]` y se gradúa con `RAYLANG_FUZZ_ITERS`:
//!   RAYLANG_FUZZ_ITERS=200000 cargo test --test fuzz_front -- --ignored

use std::path::{Path, PathBuf};

// ── PRNG: SplitMix64 (el mismo esquema que el `random` del runtime, M15.1b) ──────────

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// Un entero uniforme en `[0, n)` (`n > 0`).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

// ── Corpus ───────────────────────────────────────────────────────────────────────────

/// Recoge recursivamente los `.ray` de `examples/` y `selfhost/` como bytes.
fn corpus() -> Vec<Vec<u8>> {
    let raiz = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut archivos = Vec::new();
    for dir in ["examples", "selfhost"] {
        recoger(&raiz.join(dir), &mut archivos);
    }
    assert!(archivos.len() > 50, "el corpus debe tener los .ray del repo");
    archivos
}

fn recoger(dir: &Path, sink: &mut Vec<Vec<u8>>) {
    let Ok(entradas) = std::fs::read_dir(dir) else { return };
    for e in entradas.flatten() {
        let p = e.path();
        if p.is_dir() {
            recoger(&p, sink);
        } else if p.extension().is_some_and(|x| x == "ray") {
            if let Ok(bytes) = std::fs::read(&p) {
                sink.push(bytes);
            }
        }
    }
}

// ── Mutador ──────────────────────────────────────────────────────────────────────────

/// Produce un caso: un archivo del corpus con 1–4 mutaciones aplicadas.
fn mutar(rng: &mut Rng, corpus: &[Vec<u8>]) -> Vec<u8> {
    let mut caso = corpus[rng.below(corpus.len())].clone();
    for _ in 0..(1 + rng.below(4)) {
        if caso.is_empty() {
            caso = corpus[rng.below(corpus.len())].clone();
        }
        match rng.below(7) {
            // Flip de un byte.
            0 => {
                let i = rng.below(caso.len());
                caso[i] ^= 1 << rng.below(8);
            }
            // Truncar por un punto arbitrario.
            1 => caso.truncate(rng.below(caso.len())),
            // Borrar un span.
            2 => {
                let ini = rng.below(caso.len());
                let fin = (ini + 1 + rng.below(64)).min(caso.len());
                caso.drain(ini..fin);
            }
            // Duplicar un span (crea anidamientos/repeticiones).
            3 => {
                let ini = rng.below(caso.len());
                let fin = (ini + 1 + rng.below(64)).min(caso.len());
                let trozo: Vec<u8> = caso[ini..fin].to_vec();
                let destino = rng.below(caso.len());
                for (k, b) in trozo.into_iter().enumerate() {
                    caso.insert(destino + k, b);
                }
            }
            // Insertar basura (ASCII imprimible, tokens raros y UTF-8 multibyte).
            4 => {
                let basura: &[&[u8]] = &[
                    b"(((((", b"{{{", b"[[[", b"\"", b"f\"", b"b\"", b"'", b"\\x", b"..",
                    b"|>", b"=>", b"->", b"::", "é😀\u{7f}".as_bytes(), b"\x00\xff\xfe",
                ];
                let g = basura[rng.below(basura.len())];
                let destino = rng.below(caso.len() + 1);
                for (k, b) in g.iter().enumerate() {
                    caso.insert(destino + k, *b);
                }
            }
            // Amplificar un carácter (la clase que encontró el anidamiento profundo).
            5 => {
                let i = rng.below(caso.len());
                let b = caso[i];
                let n = 1 << (3 + rng.below(9)); // 8..=2048 repeticiones
                for _ in 0..n {
                    caso.insert(i, b);
                }
            }
            // Empalmar con el prefijo de otro archivo del corpus.
            _ => {
                let otro = &corpus[rng.below(corpus.len())];
                let corte = rng.below(caso.len());
                let toma = rng.below(otro.len().max(1));
                caso.truncate(corte);
                caso.extend_from_slice(&otro[..toma]);
            }
        }
    }
    caso.truncate(64 * 1024); // acotar el caso: el valor está en la forma, no el tamaño
    caso
}

// ── Ejecución de un caso ─────────────────────────────────────────────────────────────

/// Corre el front-end completo sobre el caso, en un hilo con pila grande.
/// Devuelve `Some(mensaje_del_pánico)` si el compilador panicó (hallazgo).
fn correr(caso: &[u8]) -> Option<String> {
    // El binario lee con `read_to_string` (rechaza UTF-8 inválido con un error de I/O),
    // así que el front-end solo ve UTF-8 válido: `lossy` reproduce esa frontera.
    let src = String::from_utf8_lossy(caso).into_owned();
    let src2 = src.clone();
    let h = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let _ = raylang::lsp::analizar(&src2); // lex → parse → check, sin ejecutar
            let _ = raylang::fmt::format_source(&src2); // rayfmt: lex → parse → pretty-print
        })
        .expect("hilo del caso");
    h.join().err().map(|p| {
        p.downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "pánico sin mensaje".into())
    })
}

/// El bucle: `iters` casos desde `semilla`; el primer pánico guarda la entrada y falla.
fn fuzz(semilla: u64, iters: usize) {
    let corpus = corpus();
    let mut rng = Rng(semilla);
    for i in 0..iters {
        let caso = mutar(&mut rng, &corpus);
        if let Some(msg) = correr(&caso) {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/fuzz");
            std::fs::create_dir_all(&dir).expect("crea target/fuzz");
            let ruta: PathBuf = dir.join(format!("hallazgo_{semilla}_{i}.ray"));
            std::fs::write(&ruta, &caso).expect("guarda el hallazgo");
            panic!(
                "el front-end panicó en el caso {i} (semilla {semilla}):\n  {msg}\n\
                 entrada guardada en {}",
                ruta.display()
            );
        }
    }
}

// ── Las dos marchas ──────────────────────────────────────────────────────────────────

/// Smoke determinista: corre SIEMPRE en la suite. Semilla fija → mismos casos en cada
/// corrida; es la regresión de "el front-end no panica", no una búsqueda.
#[test]
fn fuzz_smoke_determinista() {
    fuzz(0xC0FFEE, 400);
}

/// La campaña: iteraciones a demanda (por defecto 50k). Lenta → fuera de la suite.
///   RAYLANG_FUZZ_ITERS=200000 cargo test --test fuzz_front -- --ignored
#[test]
#[ignore]
fn fuzz_campana() {
    let iters = std::env::var("RAYLANG_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    // Semilla del reloj: cada campaña explora casos distintos (se imprime para reproducir).
    let semilla = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("reloj")
        .as_secs();
    eprintln!("campaña de fuzzing: semilla {semilla}, {iters} iteraciones");
    fuzz(semilla, iters);
}
