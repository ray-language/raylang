//! Pruebas del motor de regex (`examples/stdlib/regex.ray`, M29.1). Es una librería raylang pura
//! (Thompson NFA / VM de regex de Russ Cox): cero cambios de runtime. El demo (`regex_demo.ray`)
//! ejercita `full_match` (anclado) y `search` (substring) sobre un batería de casos; el test exige
//! que la salida sea la esperada Y que **ambos motores coincidan** (intérprete ↔ VM).

use std::process::Command;

const EXPECTED: &[&str] = &[
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
    assert_eq!(lines, EXPECTED);
}

#[test]
fn regex_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "regex_demo falló en la VM");
    assert_eq!(lines, EXPECTED);
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
const EXPECTED_M81: &[&str] = &[
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
fn regex_captures_both_engines() {
    let (interp, ok1) = run_m81(&[]);
    let (vm, ok2) = run_m81(&["--vm"]);
    assert!(ok1 && ok2, "regex_captures_demo falló");
    assert_eq!(interp, EXPECTED_M81, "intérprete vs golden");
    assert_eq!(vm, EXPECTED_M81, "VM vs golden");
}

// ---------------------------------------------------------------------------
// R7 — la VM despacha las `run_*` de std/regex al crate `regex` (feature `regex`).
// ---------------------------------------------------------------------------

/// R7: con la feature `regex`, la VM ejecuta std/regex con el crate de Rust vía ray-runtime (el
/// MISMO borde que el binario nativo desde R5); el intérprete (`--interp`) conserva la Pike VM
/// raylang. Este test es el ORÁCULO CONTINUO del dialecto entre ambos motores —clases ASCII
/// fijas, escapes literales (`\b` es la letra b), `.` que casa '\n', índices por CARÁCTER,
/// matches vacíos estilo std, grupos que no participan, errores como valores— y cubre además el
/// escape `RAYLANG_REGEX_PIKE=1` (fuerza la Pike VM interpretada en la VM: byte-idéntico).
#[test]
fn regex_native_vm_matches_pike_interp() {
    let dir = std::env::temp_dir().join("raylang_test_regex_r7");
    std::fs::create_dir_all(&dir).unwrap();
    let prog = dir.join("torture.ray");
    std::fs::write(
        &prog,
        r#"import std/regex;

fn main() {
    print(regex.find_all("a*", "baa").join("|"));
    print(regex.replace_all("x*", "ab", "-"));
    print(regex.replace_all("a+", "banana", "[$0]"));
    print(regex.find_str("h.la", "linea1\nh\nla fin").unwrap_or("no"));
    print(regex.find_str("[\\d]+", "abc 123 def").unwrap_or("no"));
    print(regex.find_str("\\bcd", "abcd").unwrap_or("no"));
    print(regex.find_str("añ.€", "x añô€ y").unwrap_or("no"));
    match (regex.find("ñ+", "añññb")) {
        Option.Some(par) => { print(par.0); print(par.1); },
        Option.None => { print(0 - 1); },
    }
    print(regex.find_all("[0-9]+?", "a123b45").join("|"));
    var vacio: [Option<string>] = [];
    let caps = regex.captures_str("(\\w+)@(\\w+)", "mail: ana@example fin").unwrap_or(vacio);
    var i = 0;
    while (i < caps.len()) { print(caps[i].unwrap_or("<none>")); i = i + 1; }
    print(regex.full_match("us.r\\d+", "user42"));
    print(regex.full_match("us.r\\d+", "user42x"));
    print(regex.search("^ab|cd$", "zzcd"));
    print(regex.replace_all("[aeiou]", "murciélago", "_"));
    let rx = regex.compile("(\\d+)-(\\d+)").unwrap();
    print(rx.find_all("1-2 33-44 5").join(","));
    match (rx.captures("z 7-89 w")) {
        Option.Some(gs) => {
            var g = 0;
            while (g < gs.len()) {
                match (gs[g]) {
                    Option.Some(par) => { print(`${par.0}..${par.1}`); },
                    Option.None => { print("<none>"); },
                }
                g = g + 1;
            }
        },
        Option.None => { print("sin match"); },
    }
    match (regex.captures("(a)|(b)", "zb")) {
        Option.Some(gs) => { print(gs.len()); print(gs[1].is_none()); },
        Option.None => { print("sin match"); },
    }
    print(regex.compile("(").is_err());
    print(regex.compile("a{3,1}").is_err());
}
"#,
    )
    .unwrap();
    let exec = |flags: &[&str], pike_env: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
        cmd.args(flags).arg(&prog);
        if pike_env {
            cmd.env("RAYLANG_REGEX_PIKE", "1");
        }
        let out = cmd.output().expect("ejecuta torture.ray");
        assert!(out.status.success(), "torture.ray falló: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    let vm = exec(&["--vm"], false);
    let interp = exec(&["--interp"], false);
    let pike = exec(&["--vm"], true);
    assert!(!vm.is_empty(), "el programa de tortura imprime");
    assert_eq!(vm, interp, "VM (crate regex) ≡ intérprete (Pike VM), byte a byte");
    assert_eq!(vm, pike, "VM (crate regex) ≡ VM con RAYLANG_REGEX_PIKE=1 (Pike VM)");
}
