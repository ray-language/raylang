//! rayreq #262: una guarda de `match` ve los módulos importados (de la stdlib y del proyecto),
//! igual que el cuerpo del brazo. Antes el loader no resolvía la guarda y `math.abs(n)` daba
//! «name 'math' not declared». Se comprueba en la VM y en el intérprete.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

#[test]
fn a_match_guard_sees_imported_modules() {
    let app = std::env::temp_dir().join(format!("ray_guard_modules_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(app.join("ray.toml"), "[package]\nname = \"p\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(app.join("src/helper.ray"), "pub fn big(n: int) -> bool {\n    n > 10\n}\n").unwrap();
    std::fs::write(
        app.join("src/main.ray"),
        "import std/math;\nimport helper;\n\nfn main() {\n    let x: Option<int> = Option.Some(42);\n    match (x) {\n        Option.Some(n) if helper.big(n) => print(\"big\"),\n        _ => print(\"small\"),\n    }\n    match (x) {\n        Option.Some(n) if math.abs(0 - n) > 1 => print(\"abs\"),\n        _ => print(\"no\"),\n    }\n}\n",
    )
    .unwrap();
    for flags in [&[][..], &["--interp"][..]] {
        let out = Command::new(BIN).arg("run").args(flags).arg("src/main.ray").current_dir(&app).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(stdout, "big\nabs\n", "{flags:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}
