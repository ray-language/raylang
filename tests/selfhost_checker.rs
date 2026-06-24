//! Oráculo del **checker auto-alojado** (M14.3, self-hosting).
//!
//! El checker escrito en raylang (`selfhost/checker.ray`, vía `selfhost/check_dump.ray`) debe dar el
//! mismo VEREDICTO que el checker de Rust (`src/checker.rs`): `ok` para programas válidos, o
//! `error de tipos en L:C: msg` byte-idéntico para los inválidos. Es un VALIDADOR (no reproduce el
//! lowering de M9; ver DESIGN §23.4): solo se compara el veredicto.
//!
//! Estrategia: la misma fuente por ambos pipelines (Rust: lex→parse→check; raylang: self-lex→
//! self-parse→self-check). El lado raylang lo imprime el driver; el lado Rust lo reconstruye
//! `canonical`.
//!
//! Cobertura M14.3a: núcleo monomórfico (operadores, variables, llamadas, if/while/return, builtin
//! print). El corpus evita estructuras/enums/genéricos/traits (→ M14.3b–d).

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// El veredicto del front-end de Rust (el oráculo) para una fuente: `ok` o el `Display` del primer
/// error (léxico, sintáctico o de tipos), exactamente lo que imprime `check_dump.ray`.
fn canonical(src: &str) -> String {
    let tokens = match raylang::lexer::lex(src) {
        Ok(t) => t,
        Err(e) => return format!("{e}"),
    };
    let mut prog = match raylang::parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => return format!("{e}"),
    };
    match raylang::checker::check(&mut prog) {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("{e}"),
    }
}

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Ejecuta el checker auto-alojado sobre `src`: lo escribe a un temporal y corre
/// `raylang selfhost/check_dump.ray <temporal>`. Devuelve su stdout (sin el salto final).
fn check_dump(src: &str, nombre_tmp: &str) -> String {
    let mut tmp = std::env::temp_dir();
    tmp.push(nombre_tmp);
    let mut f = std::fs::File::create(&tmp).expect("crea el temporal");
    f.write_all(src.as_bytes()).expect("escribe el temporal");
    drop(f);

    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(repo_path("selfhost/check_dump.ray"))
        .arg(&tmp)
        .output()
        .expect("ejecuta el checker auto-alojado");
    assert!(
        out.status.success(),
        "check_dump falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// Compara el checker auto-alojado con el oráculo para una fuente concreta.
fn comparar(src: &str, nombre_tmp: &str) {
    let esperado = canonical(src);
    let obtenido = check_dump(src, nombre_tmp);
    assert_eq!(obtenido, esperado, "el checker auto-alojado difiere del oráculo para:\n{src}");
}

#[test]
fn programas_validos() {
    comparar("fn main() -> int { 0 }", "sc_min.ray");
    comparar("fn main() { }", "sc_unit.ray");
    comparar("fn main() -> int { let x = 3; let y = x + 1; y }", "sc_let.ray");
    comparar("fn main() -> int { var i = 0; while (i < 10) { i = i + 1; } i }", "sc_while.ray");
    comparar("fn main() -> int { if (1 < 2) { 1 } else { 2 } }", "sc_if.ray");
    comparar("fn doble(x: int) -> int { x * 2 } fn main() -> int { doble(21) }", "sc_call.ray");
    comparar("fn main() -> int { print(\"hola\"); print(42); 0 }", "sc_print.ray");
    comparar("fn main() -> int { let b = true && false || 1 == 2; if (b) { 1 } else { 0 } }", "sc_logic.ray");
}

#[test]
fn errores_de_tipo() {
    // Operadores.
    comparar("fn main() -> int { 1 + true }", "sce_add.ray");
    comparar("fn main() -> int { if (1) { 1 } else { 2 } }", "sce_cond.ray");
    comparar("fn main() -> bool { 1 < true }", "sce_order.ray");
    comparar("fn main() -> int { -true }", "sce_neg.ray");
    comparar("fn main() -> int { if (!1) { 0 } else { 1 } }", "sce_not.ray");
    // Variables.
    comparar("fn main() -> int { x }", "sce_undecl.ray");
    comparar("fn main() -> int { let x = 3; x = 4; 0 }", "sce_immut.ray");
    comparar("fn main() -> int { let x: int = true; x }", "sce_annot.ray");
    // Retorno.
    comparar("fn main() -> int { true }", "sce_ret.ray");
    comparar("fn f() -> int { } fn main() -> int { 0 }", "sce_ret_unit.ray");
    // Llamadas.
    comparar("fn g(x: int) -> int { x } fn main() -> int { g() }", "sce_arity.ray");
    comparar("fn g(x: int) -> int { x } fn main() -> int { g(true) }", "sce_argty.ray");
    comparar("fn main() -> int { h(1) }", "sce_nofn.ray");
    comparar("fn main() -> int { print(1, 2) }", "sce_print_arity.ray");
    // if sin else con valor.
    comparar("fn main() -> int { if (true) { 1 } 0 }", "sce_if_noelse.ray");
    // main mal.
    comparar("fn main(x: int) -> int { 0 }", "sce_main_params.ray");
    comparar("fn main() -> bool { true }", "sce_main_ret.ray");
    comparar("fn f() -> int { 1 }", "sce_no_main.ray");
    // tipo desconocido.
    comparar("fn main() -> Foo { 0 }", "sce_unknown_type.ray");
}

/// El test fuerte: los ejemplos reales monomórficos deben dar el mismo veredicto (`ok`) que Rust.
#[test]
fn ejemplos_reales_validos() {
    let archivos = ["examples/fib.ray", "examples/fizzbuzz.ray", "examples/gcd.ray", "examples/primes.ray"];
    for rel in archivos {
        let src = std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("lee {rel}: {e}"));
        let esperado = canonical(&src);
        let nombre_tmp = format!("sc_real_{}.ray", rel.replace('/', "_"));
        let obtenido = check_dump(&src, &nombre_tmp);
        assert_eq!(obtenido, esperado, "el checker auto-alojado difiere del oráculo en {rel}");
    }
}
