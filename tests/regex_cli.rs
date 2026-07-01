//! Pruebas del motor de regex (`examples/stdlib/regex.ray`, M29.1). Es una librería raylang pura
//! (Thompson NFA / VM de regex de Russ Cox): cero cambios de runtime. El demo (`regex_demo.ray`)
//! ejercita `full_match` (anclado) y `search` (substring) sobre un batería de casos; el test exige
//! que la salida sea la esperada Y que **ambos motores coincidan** (intérprete ↔ VM).

use std::process::Command;

const ESPERADO: &[&str] = &[
    "full  /abc/ ~ \"abc\" = si",
    "full  /abc/ ~ \"abd\" = no",
    "full  /abc/ ~ \"ab\" = no",
    "full  /a.c/ ~ \"axc\" = si",
    "full  /a.c/ ~ \"ac\" = no",
    "full  /ab*c/ ~ \"ac\" = si",
    "full  /ab*c/ ~ \"abbbc\" = si",
    "full  /ab+c/ ~ \"ac\" = no",
    "full  /ab+c/ ~ \"abc\" = si",
    "full  /colou?r/ ~ \"color\" = si",
    "full  /colou?r/ ~ \"colour\" = si",
    "full  /gr(a|e)y/ ~ \"gray\" = si",
    "full  /gr(a|e)y/ ~ \"grey\" = si",
    "full  /gr(a|e)y/ ~ \"groy\" = no",
    "full  /(ab)+/ ~ \"ababab\" = si",
    "full  /(ab)+/ ~ \"aba\" = no",
    "full  /a(b|c)*d/ ~ \"ad\" = si",
    "full  /a(b|c)*d/ ~ \"abccbd\" = si",
    "full  /a(b|c)*d/ ~ \"abxd\" = no",
    "full  /a\\.b/ ~ \"a.b\" = si",
    "full  /a\\.b/ ~ \"axb\" = no",
    "search/cd/ ~ \"abcde\" = si",
    "search/xyz/ ~ \"abcde\" = no",
    "search/a+/ ~ \"bbbaaa\" = si",
    "search// ~ \"abc\" = si",
    "full  /[abc]+/ ~ \"cabba\" = si",
    "full  /[abc]+/ ~ \"cabxa\" = no",
    "full  /[a-z]+/ ~ \"hola\" = si",
    "full  /[a-z]+/ ~ \"Hola\" = no",
    "full  /[^0-9]+/ ~ \"abc\" = si",
    "full  /[^0-9]+/ ~ \"ab3\" = no",
    "full  /[A-Za-z0-9_]+/ ~ \"var_9\" = si",
    "full  /\\d+/ ~ \"2024\" = si",
    "full  /\\d+/ ~ \"20a4\" = no",
    "full  /\\w+/ ~ \"hola_99\" = si",
    "full  /a\\sb/ ~ \"a b\" = si",
    "full  /a\\sb/ ~ \"a_b\" = no",
    "full  /\\D+/ ~ \"abc\" = si",
    "full  /[\\d.]+/ ~ \"3.14\" = si",
    "search/^abc/ ~ \"abcdef\" = si",
    "search/^abc/ ~ \"xabcdef\" = no",
    "search/def$/ ~ \"abcdef\" = si",
    "search/def$/ ~ \"abcdefg\" = no",
    "search/^\\d+$/ ~ \"12345\" = si",
    "search/^\\d+$/ ~ \"123a5\" = no",
    "full  /\\w+@\\w+\\.\\w+/ ~ \"ana@rayala.org\" = si",
    "full  /\\w+@\\w+\\.\\w+/ ~ \"ana.rayala.org\" = no",
    "find  /\\d+/ ~ \"abc123def\" = \"123\"",
    "find  /\\d+/ ~ \"sin numeros\" = <none>",
    "find  /a+/ ~ \"xaaay\" = \"aaa\"",
    "all   /\\d+/ ~ \"a12b345c6\" = [12,345,6]",
    "all   /[a-z]+/ ~ \"Hola Mundo 42\" = [ola,undo]",
    "repl  /\\d+/ \"quedan 3 de 10\" -> \"quedan N de N\"",
    "repl  /\\s+/ \"hola   mundo  ya\" -> \"hola_mundo_ya\"",
    "repl  /a/ \"banana\" -> \"b-n-n-\"",
];

fn correr(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/stdlib/regex_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta regex_demo.ray");
    let lineas = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    (lineas, out.status.success())
}

#[test]
fn regex_interprete() {
    let (lineas, ok) = correr(&[]);
    assert!(ok, "regex_demo falló en el intérprete");
    assert_eq!(lineas, ESPERADO);
}

#[test]
fn regex_vm() {
    let (lineas, ok) = correr(&["--vm"]);
    assert!(ok, "regex_demo falló en la VM");
    assert_eq!(lineas, ESPERADO);
}

/// Oráculo conductual: intérprete y VM deben producir EXACTAMENTE la misma salida.
#[test]
fn regex_ambos_motores_coinciden() {
    let (interp, ok1) = correr(&[]);
    let (vm, ok2) = correr(&["--vm"]);
    assert!(ok1 && ok2, "regex_demo falló");
    assert_eq!(interp, vm, "el intérprete y la VM difieren en regex_demo");
}
