//! M274 — la cola de raystream ([15] constantes arreglo con semántica de literal inyectado,
//! [16] `@derive(Show)` con campos arreglo). Los tres motores.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_raystream_tail_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn three_engines(dir: &PathBuf, want: &str) {
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(dir, engine);
        assert_eq!(code, 0, "{engine:?}: {err}");
        assert_eq!(out, want, "{engine:?}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = dir.join("prog_bin");
        let (_o, err, code) = ray(dir, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        let out = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo");
    }
}

/// [15] Una constante arreglo (anidada) se usa como tabla; cada uso es un arreglo FRESCO: mutar un
/// alias no toca a los demás usos. Un valor no literal sigue siendo error.
#[test]
fn array_constants_are_injected_literals() {
    let d = tmp("const_arrays");
    std::fs::write(
        d.join("prog.ray"),
        r#"const EXTENSIONS: [string] = ["srt", "vtt"];
const BITRATES: [[int]] = [[32, 64, 96], [128, 192, 256]];
const OFFSETS: [int] = [-1, 0, 1];

fn is_subtitle(ext: string) -> bool { EXTENSIONS.contains(ext) }

fn main() -> int {
    print(is_subtitle("vtt"));
    print(BITRATES[1][2]);
    print(OFFSETS[0]);
    let alias = EXTENSIONS;
    alias.push("ass");
    print(alias.len());
    print(EXTENSIONS.len());
    var total = 0;
    for row in BITRATES { for b in row { total = total + b; } }
    print(total);
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "true\n256\n-1\n3\n2\n768\n");
    std::fs::write(d.join("bad.ray"), "const XS: [int] = [1, 2 + 3];\nfn main() -> int { 0 }\n").unwrap();
    let (_o, err, code) = ray(&d, &["run", "bad.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("the value of constant 'XS' must be a literal"), "{err}");
}

/// [16] `@derive(Show)` con campos `[T]` (y anidados) en los tres motores.
#[test]
fn derive_show_renders_array_fields() {
    let d = tmp("derive_show_arrays");
    std::fs::write(
        d.join("prog.ray"),
        r#"@derive(Show)
struct Track { id: int, tags: [string], }

@derive(Show)
struct Album { title: string, tracks: [Track], grid: [[int]], }

fn main() -> int {
    let a = Album { title: "x", tracks: [Track { id: 1, tags: ["a", "b"] }], grid: [[1, 2], []] };
    print(a.show());
    print(to_string(a) == a.show());
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "Album { title: x, tracks: [Track { id: 1, tags: [a, b] }], grid: [[1, 2], []] }\ntrue\n");
}
