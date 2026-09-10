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
    "comp  /gr(a|e/ = err: regex: missing ')'",
    "comp  /[a-z/ = err: regex: missing ']' to close the class",
    "comp  /abc\\/ = err: regex: trailing '\\' at end of pattern",
    "comp  /ab)c/ = err: regex: unexpected character in pattern (stray ')'?)",
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
    // M128 — grupos con nombre: (?P<name>) / (?<name>), captures_map y errores de nombre.
    "names [,y,m,]",
    "cap m=08",
    "cap y=2026",
    "cap <none>",
    "comp err: regex: duplicate group name 'a'",
    "comp err: regex: group name cannot start with a digit",
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

/// M211 (ray-sublime #1–#3): lo que el motor no implementa se rechaza en `compile` con un `Err` con
/// nombre — antes look-around y `\p{…}` "compilaban" y reventaban (ICE) en la primera búsqueda por
/// la vía acelerada, y `\1`/`\G` casaban el carácter literal en silencio. Idéntico en ambos motores.
#[test]
fn unsupported_constructs_are_rejected_at_compile() {
    let cases: &[(&str, &str)] = &[
        ("(?=foo)", "look-ahead"),
        ("(?!foo)", "look-ahead"),
        ("(?<=a)b", "look-behind"),
        ("(?<!a)b", "look-behind"),
        ("(?>ab)", "atomic"),
        ("(?i)ab", "inline flags"),
        ("\\p{Lu}+", "Unicode classes"),
        ("(a)\\1", "backreferences"),
        ("(?P<n>a)\\k<n>", "backreferences"),
        ("\\Gab", "anchor escape"),
        ("\\Aab", "anchor escape"),
        ("a\\h", "not supported"),
        ("a*+", "possessive"),
        ("a++", "possessive"),
        ("a?+", "possessive"),
        ("[[:alpha:]]", "POSIX classes"),
        ("[a-z&&[^m]]", "intersection"),
        ("[\\p{L}]", "Unicode classes"),
    ];
    let mut src = String::from("import std/regex;\nfn main() -> int {\n");
    for (pat, _) in cases {
        let lit = pat.replace('\\', "\\\\").replace('"', "\\\"");
        src.push_str(&format!(
            "    match (regex.compile(\"{lit}\")) {{ Result.Ok(_) => print(\"ok\"), Result.Err(e) => print(e) }}\n"
        ));
    }
    // Lo que SÍ se soporta sigue compilando (grupos con nombre, no captura, lazy, bounds, clases).
    src.push_str("    match (regex.compile(\"(?P<w>a+?)(?:b|c)*[^x]{2,3}\\\\d\")) { Result.Ok(_) => print(\"ok\"), Result.Err(e) => print(e) }\n    0\n}\n");
    let path = std::env::temp_dir().join("ray_regex_unsupported.ray");
    std::fs::write(&path, &src).unwrap();
    for flags in [&[][..], &["--vm"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_raylang")).args(flags).arg(&path).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let lines: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
        assert_eq!(lines.len(), cases.len() + 1, "{lines:?}");
        for ((pat, needle), line) in cases.iter().zip(&lines) {
            assert!(line.starts_with("regex: ") && line.contains(needle), "{pat}: {line}");
        }
        assert_eq!(lines[cases.len()], "ok", "{flags:?}");
    }
}

// ---------------------------------------------------------------------------
// M232 — `regex.onig`: dialecto Oniguruma en ray_runtime (fancy-regex). Sin espejo raylang, la
// paridad es por construcción: el mismo programa en intérprete, VM y nativo, byte a byte.
// ---------------------------------------------------------------------------
const ONIG_PROG: &str = r#"import std/regex;

fn show(m: Option<regex.Match>) {
    match (m) {
        Option.Some(x) => print("${x.start}-${x.end} groups=${x.groups.len()}"),
        Option.None => print("none"),
    }
}

fn main() -> int {
    let t = "  x = 1; total = 42";
    let re = regex.onig("(?<key>\\w+)\\s*=\\s*(?=\\d)(\\d+)").unwrap();
    print(re.names.join(","));
    show(re.search_from(t, 5));
    match (re.search_from(t, 5)) {
        Option.Some(m) => {
            print(regex.group_str(m, t, 1).unwrap_or("?"));
            print(regex.group_str(m, t, 2).unwrap_or("?"));
            print(regex.group_str(m, t, 7).unwrap_or("out"));
        },
        Option.None => print("none"),
    }
    print(regex.group_index(re, "key").unwrap_or(-1));
    print(regex.group_index(re, "nope").unwrap_or(-1));
    show(re.match_at("ab = 7", 1));
    show(re.match_at(" ab = 7", 0));
    let g = regex.onig("\\Gab").unwrap();
    show(g.search_from("xxab ab", 2));
    show(g.search_from("xxab ab", 0));
    let br = regex.onig("(\\w)\\1").unwrap();
    print(br.is_match("abccd"));
    print(br.is_match("abcd"));
    show(regex.onig("(?<=\\$)\\w+").unwrap().search_from("pay $amount now", 0));
    show(regex.onig("^b$").unwrap().search_from("a\nb\nc", 0));
    show(regex.onig("\\h+").unwrap().search_from("zz1fG", 0));
    show(regex.onig("a++a").unwrap().search_from("aaa", 0));
    show(regex.onig("(?i:ab)c").unwrap().search_from("ABc", 0));
    show(regex.onig("(?>a+)b").unwrap().search_from("aaab", 0));
    show(regex.onig("\\p{Lu}+").unwrap().search_from("abcDEFg", 0));
    show(regex.onig("[a-z&&[^m]]+").unwrap().search_from("almo", 0));
    show(regex.onig("ñ+").unwrap().search_from("añññb", 0));
    show(regex.onig("b").unwrap().match_at("añññb", 4));
    show(regex.onig("b").unwrap().search_from("añññb", 9));
    match (regex.onig("(a)|(b)").unwrap().search_from("b", 0)) {
        Option.Some(m) => { print(m.groups[1].is_none()); print(m.groups[2].is_some()); },
        Option.None => print("none"),
    }
    match (regex.onig("(?<=a+")) { Result.Ok(_) => print("ok"), Result.Err(e) => print(e) }
    match (regex.onig("(a)\\2")) { Result.Ok(_) => print("ok"), Result.Err(e) => print(e) }
    match (regex.onig("[abc")) { Result.Ok(_) => print("ok"), Result.Err(e) => print(e) }
    0
}
"#;

const ONIG_WANT: &[&str] = &[
    ",key,",
    "9-19 groups=3",
    "total",
    "42",
    "out",
    "1",
    "-1",
    "1-6 groups=3",
    "none",
    "2-4 groups=1",
    "none",
    "true",
    "false",
    "5-11 groups=1",
    "2-3 groups=1",
    "2-4 groups=1",
    "none",
    "0-3 groups=1",
    "0-4 groups=1",
    "3-6 groups=1",
    "0-2 groups=1",
    "1-4 groups=1",
    "4-5 groups=1",
    "none",
    "true",
    "true",
    "regex: Parsing error at position 6: Opening parenthesis without closing parenthesis",
    "regex: Invalid back reference to group 2",
    "regex: Parsing error at position 4: Invalid character class",
];

fn onig_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("raylang_test_regex_onig");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("prog.ray"), ONIG_PROG).unwrap();
    dir
}

#[test]
fn onig_dialect_matches_on_interpreter_and_vm() {
    let dir = onig_dir();
    for flags in [&[][..], &["--vm"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_raylang")).args(flags).arg(dir.join("prog.ray")).output().unwrap();
        assert!(out.status.success(), "{flags:?}: {}", String::from_utf8_lossy(&out.stderr));
        let lines: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
        assert_eq!(lines, ONIG_WANT, "{flags:?}");
    }
}

/// El nativo llama a la MISMA tabla de handles de ray_runtime: salida byte-idéntica a la VM.
#[test]
fn onig_dialect_matches_natively() {
    if !Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("(sin rustc: se omite el nativo)");
        return;
    }
    let dir = onig_dir();
    let bin = dir.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
    let st = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .expect("build nativo");
    assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
    let out = Command::new(&bin).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    assert_eq!(lines, ONIG_WANT, "nativo");
}

/// Un patrón catastrófico no cuelga: el motor abandona por el límite de backtracking y
/// `std/regex` lo convierte en un pánico con nombre (igual en ambos motores).
#[test]
fn onig_backtrack_limit_panics_with_a_named_message() {
    let dir = std::env::temp_dir().join("raylang_test_regex_onig_limit");
    std::fs::create_dir_all(&dir).unwrap();
    let prog = dir.join("prog.ray");
    std::fs::write(
        &prog,
        "import std/regex;\nfn main() -> int {\n    let re = regex.onig(\"^(a*)*\\\\1$\").unwrap();\n    print(re.is_match(\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaac\"));\n    0\n}\n",
    )
    .unwrap();
    for flags in [&[][..], &["--vm"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_raylang")).args(flags).arg(&prog).output().unwrap();
        assert!(!out.status.success(), "{flags:?}: debe fallar");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("regex: backtrack limit exceeded for pattern ^(a*)*\\1$"), "{flags:?}: {err}");
    }
}
