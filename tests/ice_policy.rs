//! La política de ICEs (M33b) se auto-defiende: el front-end y sus clientes de
//! compilación no pueden hacer panic "a pelo" — toda invariante interna pasa por la
//! macro `ice!` (mensaje estandarizado + la red central de `with_big_stack_or_ice`)
//! y todo error de usuario es un `LexError`/`ParseError`/`TypeError` con posición.
//!
//! Este test lee los fuentes y **falla si reaparece** un `panic!`/`unwrap()`/
//! `expect("…")`/`unreachable!`/`todo!`/`unimplemented!` fuera de los tests. Las
//! excepciones deliberadas se marcan con `// ice-ok` en la misma línea (hoy: la
//! definición de la propia macro y el re-lanzado de `with_big_stack` para tests).
//!
//! Fuera del alcance (a propósito): el runtime (`vm`/`interpreter`/`compiler`/
//! `bytecode`/`gc`/`builtins`) — sus pánicos son del dominio de ejecución y su
//! auditoría va con el arco del motor único (M35/M36); mientras tanto la red
//! central ya los presenta como ICE.

/// Los archivos bajo la política: el front-end + los clientes de compilación.
const EN_POLITICA: &[&str] = &[
    "src/token.rs",
    "src/lexer.rs",
    "src/parser.rs",
    "src/ast.rs",
    "src/checker.rs",
    "src/loader.rs",
    "src/prelude.rs",
    "src/diagnostic.rs",
    "src/fmt.rs",
    "src/lsp.rs",
    "src/repl.rs",
    "src/test_runner.rs",
    "src/main.rs",
    "src/lib.rs",
];

/// Patrones prohibidos fuera de tests. `.expect("` casa solo el `expect` de
/// `Option`/`Result` de Rust (con literal), no el método `expect` del parser
/// (que recibe un `&TokenKind` y devuelve `Result` — ese es el camino bueno).
const PROHIBIDOS: &[&str] = &[
    "panic!(",
    "unreachable!(",
    "todo!(",
    "unimplemented!(",
    ".unwrap()",
    ".expect(\"",
];

#[test]
fn el_front_end_no_panica_a_pelo() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violaciones = Vec::new();
    for file in EN_POLITICA {
        let source = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|e| panic!("no se pudo leer {file}: {e}"));
        for (i, linea) in source.lines().enumerate() {
            // Todo lo que sigue al primer bloque de tests no cuenta (convención del
            // proyecto: los tests van al final del archivo).
            if linea.trim_start().starts_with("#[cfg(test)]") {
                break;
            }
            if linea.contains("ice-ok") {
                continue; // excepción deliberada, documentada en el propio sitio
            }
            for patron in PROHIBIDOS {
                if linea.contains(patron) {
                    violaciones.push(format!("{file}:{}: {}", i + 1, linea.trim()));
                }
            }
        }
    }
    assert!(
        violaciones.is_empty(),
        "el front-end must usar `ice!` (invariantes) o errors con posición (user), \
         no pánicos a pelo (M33b). Violaciones:\n{}",
        violaciones.join("\n")
    );
}

/// La UX de la red central (M33b), de punta a punta: un pánico dentro de
/// `with_big_stack_or_ice` produce el banner de ICE en stderr y el código 101.
/// Truco: el test se re-lanza a sí mismo como subproceso (variable de entorno
/// `RAYLANG_ICE_CHILD`) porque `process::exit` no puede probarse en el mismo proceso.
#[test]
fn la_red_central_presenta_el_ice() {
    if std::env::var("RAYLANG_ICE_CHILD").is_ok() {
        // Rama hija: panica dentro de la red. No retorna.
        raylang::with_big_stack_or_ice(|| panic!("invariante rota de prueba"));
        return;
    }
    let exe = std::env::current_exe().expect("path del test");
    let out = std::process::Command::new(exe)
        .arg("la_red_central_presenta_el_ice")
        .arg("--nocapture") // sin esto el arnés captura el stderr y el exit() lo pierde
        .env("RAYLANG_ICE_CHILD", "1")
        .output()
        .expect("ejecuta el subproceso");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(101), "código 101 (convención Rust)\n{stderr}");
    assert!(stderr.contains("internal compiler error (ICE)"), "{stderr}");
    assert!(stderr.contains("invariante rota de prueba"), "el payload del pánico\n{stderr}");
    assert!(stderr.contains("report it"), "asks el reporte\n{stderr}");
}
