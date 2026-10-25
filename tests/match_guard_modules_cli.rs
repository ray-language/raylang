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

/// Revisión de 1.27.80: los recorredores del backend nativo (`transpile/analysis.rs`) saltaban la
/// guarda, así que una variable capturada SOLO desde ella no entraba en `cell_vars` y el binario
/// veía el valor viejo (VM `false`, nativo `true`). Se compara la salida de los tres motores.
#[test]
fn a_variable_captured_only_from_a_guard_is_a_cell_in_the_native_backend() {
    let dir = std::env::temp_dir().join(format!("ray_guard_cell_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("prog.ray"),
        "fn main() {\n    var limit = 10;\n    let f = fn(x: Option<int>) -> bool {\n        match (x) {\n            Option.Some(n) if n > limit => true,\n            _ => false,\n        }\n    };\n    limit = 20;\n    print(f(Option.Some(15)));\n    limit = 12;\n    print(f(Option.Some(15)));\n}\n",
    )
    .unwrap();
    const WANT: &str = "false\ntrue\n";
    for flags in [&[][..], &["--interp"][..]] {
        let out = Command::new(BIN).arg("run").args(flags).arg("prog.ray").current_dir(&dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{flags:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = dir.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(BIN)
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo ≡ VM");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
