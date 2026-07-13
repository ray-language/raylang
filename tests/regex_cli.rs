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
    // M59.2 — errores como valores (compile) + la API compilada (métodos de Matcher).
    "comp  /gr(a|e/ = err: regex: falta ')'",
    "comp  /[a-z/ = err: regex: falta ']' para cerrar la clase",
    "comp  /abc\\/ = err: regex: '\\' al final del patrón",
    "comp  /ab)c/ = err: regex: carácter inesperado en el patrón (¿')' de más?)",
    "re    full 2024 = si",
    "re    search abc123 = si",
    "re    find_str = \"123\"",
    "re    find = (3,6)",
    "re    all = [12,345,6]",
    "re    repl = \"quedan N de N\"",
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/stdlib/regex_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta regex_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    (lines, out.status.success())
}

#[test]
fn regex_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "regex_demo falló en el intérprete");
    assert_eq!(lines, ESPERADO);
}

#[test]
fn regex_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "regex_demo falló en la VM");
    assert_eq!(lines, ESPERADO);
}

/// Oráculo conductual: intérprete y VM deben producir EXACTAMENTE la misma salida.
#[test]
fn regex_ambos_engines_matches() {
    let (interp, ok1) = run(&[]);
    let (vm, ok2) = run(&["--vm"]);
    assert!(ok1 && ok2, "regex_demo falló");
    assert_eq!(interp, vm, "el intérprete y la VM difieren en regex_demo");
}

// ---------------------------------------------------------------------------
// M81 — Pike VM: grupos de captura, {n,m} y cuantificadores lazy.
// ---------------------------------------------------------------------------
const ESPERADO_M81: &[&str] = &[
    "caps /(\\d+)-(\\d+)/ ~ \"tel 12-345 fin\" → [0]=12-345 [1]=12 [2]=345",
    "caps /(a+)(b*)/ ~ \"aab\" → [0]=aab [1]=aa [2]=b",
    "caps /((a)(b))c/ ~ \"abc\" → [0]=abc [1]=ab [2]=a [3]=b",
    "caps /(x)|(y)/ ~ \"y\" → [0]=y [1]=<none> [2]=y",
    "caps /(?:ab)+(c)/ ~ \"ababc\" → [0]=ababc [1]=c",
    "full  /a{3}/ ~ \"aaa\" = si",
    "full  /a{3}/ ~ \"aa\" = no",
    "full  /a{3}/ ~ \"aaaa\" = no",
    "full  /a{2,}/ ~ \"aaaa\" = si",
    "full  /a{2,}/ ~ \"a\" = no",
    "full  /a{2,3}/ ~ \"aaa\" = si",
    "full  /a{2,3}/ ~ \"aaaa\" = no",
    "full  /(ab){2}/ ~ \"abab\" = si",
    "full  /\\d{2,4}/ ~ \"123\" = si",
    "full  /a{x}/ ~ \"a{x}\" = si",
    "find  /<.+>/ ~ \"<a><b>\" = (0,6)",
    "find  /<.+?>/ ~ \"<a><b>\" = (0,3)",
    "find  /a+?/ ~ \"aaa\" = (0,1)",
    "caps /\"(.*?)\"/ ~ \"dice \"hola\" y \"adios\"\" → [0]=\"hola\" [1]=hola",
    "1,22,333",
    "a_b_c",
];

fn run_m81(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/stdlib/regex_captures_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta regex_captures_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    (lines, out.status.success())
}

#[test]
fn regex_capturas_ambos_engines() {
    let (interp, ok1) = run_m81(&[]);
    let (vm, ok2) = run_m81(&["--vm"]);
    assert!(ok1 && ok2, "regex_captures_demo falló");
    assert_eq!(interp, ESPERADO_M81, "intérprete vs golden");
    assert_eq!(vm, ESPERADO_M81, "VM vs golden");
}
