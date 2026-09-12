//! M240 — `ray profile`: perfilador por función de la VM. El informe (tabla en stderr o JSON en
//! `--out`) trae llamadas, tiempo propio e inclusivo; `fib` recursivo da un conteo EXACTO de
//! llamadas (2·fib(n+1)−1 por raíz), que es lo que se afirma.

use std::process::Command;

const PROG: &str = "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }\nfn work(xs: [int]) -> int { var s = 0; for x in xs { s = s + fib(x); } s }\nfn apply(f: fn(int) -> int, v: int) -> int { let r = f(v); r + 0 }\nfn main() -> int {\n    let a = work([15, 16]);\n    let b = apply(fib, 12);\n    print(a + b);\n    0\n}\n";

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_profile_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    d
}

fn field(json: &str, name: &str, key: &str) -> u64 {
    let start = json.find(&format!("\"name\":\"{name}\"")).unwrap_or_else(|| panic!("{name} en {json}"));
    let rest = &json[start..];
    let k = rest.find(&format!("\"{key}\":")).unwrap() + key.len() + 3;
    rest[k..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap()
}

#[test]
fn json_report_counts_calls_exactly_and_orders_by_self_time() {
    let d = dir("json");
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["profile", "--json", "--out", "report.json", "prog.ray"])
        .current_dir(&d)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1741\n", "la salida del programa no cambia");
    let json = std::fs::read_to_string(d.join("report.json")).unwrap();
    // fib(15): 2·fib(16)−1 = 1973; fib(16): 2·fib(17)−1 = 3193; fib(12): 2·fib(13)−1 = 465.
    assert_eq!(field(&json, "fib", "calls"), 1973 + 3193 + 465, "{json}");
    assert_eq!(field(&json, "work", "calls"), 1);
    assert_eq!(field(&json, "apply", "calls"), 1);
    assert_eq!(field(&json, "main", "calls"), 1);
    assert!(field(&json, "work", "inclusive_ns") >= field(&json, "work", "self_ns"));
    assert!(field(&json, "main", "inclusive_ns") >= field(&json, "work", "inclusive_ns"), "main incluye a work: {json}");
    assert!(field(&json, "apply", "inclusive_ns") > field(&json, "apply", "self_ns"), "apply incluye la llamada indirecta a fib: {json}");
    assert!(json.starts_with("{\"wall_ns\":") && json.contains("\"fibers\":1,"), "{json}");
    let first = json.find("\"name\":\"").unwrap();
    assert!(json[first..].starts_with("\"name\":\"fib\""), "fib manda en tiempo propio: {json}");
}

#[test]
fn table_goes_to_stderr_and_exit_still_reports() {
    let d = dir("table");
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["profile", "--top", "2", "prog.ray"]).current_dir(&d).output().unwrap();
    assert!(out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("[profile] 4 function(s), 1 fiber(s)"), "{err}");
    assert!(err.contains(" fib\n") && err.contains("… 2 more (--top N)"), "{err}");
    // exit() en mitad del programa: el informe sale igual, con el código de salida del programa.
    std::fs::write(d.join("exit.ray"), "fn f() -> int { 1 }\nfn main() -> int {\n    let _ = f();\n    exit(3);\n    0\n}\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["profile", "exit.ray"]).current_dir(&d).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("[profile]"), "{}", String::from_utf8_lossy(&out.stderr));
    // Solo VM.
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["profile", "--interp", "prog.ray"]).current_dir(&d).output().unwrap();
    assert_eq!(out.status.code(), Some(64));
}
