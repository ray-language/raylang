//! Hallazgos atendidos en 1.27.76: #228 `b[i] = v` sobre un `bytes` pasaba el checker (ICE en la
//! VM, el nativo no compilaba); #199 la doc de los campos de un struct (`Stat.mode` vale 0 en
//! Windows) no salía en `ray doc`.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ray_f1_27_76_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

/// rayffi #228: el checker rechaza la escritura de un octeto, con posición y la salida; en 1.27.75
/// `ray check` decía «compiles» y `ray run` caía en un ICE.
#[test]
fn assigning_an_octet_of_bytes_is_a_type_error() {
    let d = tmp("bytes_assign");
    std::fs::write(
        d.join("prog.ray"),
        "fn main() -> int {\n    let b = bytes_of([0, 0]);\n    b[0] = 1;\n    print(\"${b[0]}\");\n    0\n}\n",
    )
    .unwrap();
    for args in [&["check", "prog.ray"][..], &["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        let all = format!("{out}{err}");
        assert_ne!(code, 0, "{args:?}: {all}");
        assert!(all.contains("type error at 3:5"), "{args:?}: {all}");
        assert!(all.contains("cannot assign to an octet of bytes (bytes are immutable"), "{args:?}: {all}");
        assert!(!all.contains("internal compiler error"), "{args:?}: {all}");
    }
}

/// Leer un octeto y mutar un arreglo siguen valiendo (la guarda es solo para `bytes`).
#[test]
fn reading_octets_and_writing_arrays_still_work() {
    let d = tmp("bytes_read");
    std::fs::write(
        d.join("prog.ray"),
        "fn main() -> int {\n    let b = bytes_of([7, 9]);\n    let a = [0, 0];\n    a[1] = b[0] + b[1];\n    print(\"${a[1]}\");\n    0\n}\n",
    )
    .unwrap();
    let (out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(out, "16\n");
}

/// rayzip #199: `ray doc fs.Stat` lista la doc de cada campo, y la de `mode` dice lo de Windows.
#[test]
fn ray_doc_of_a_struct_lists_its_field_docs() {
    let d = tmp("field_docs");
    let (out, err, code) = ray(&d, &["doc", "fs.Stat"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("struct Stat { kind: string, mode: int, size: int, mtime_ms: int }"), "{out}");
    let mode = out.lines().find(|l| l.trim_start().starts_with("mode: int — ")).unwrap_or_else(|| panic!("{out}"));
    assert!(mode.contains("Always 0 on Windows"), "{mode}");
    assert!(out.lines().any(|l| l.trim_start().starts_with("kind: string — ")), "{out}");
    let (out, _, _) = ray(&d, &["doc", "fs.stat"]);
    assert!(out.contains("On Windows `mode` is always 0"), "{out}");
}

/// De paso (M422): las posiciones de los campos de un struct de OTRO módulo no se desplazaban a su
/// banda, así que el «unknown type» de un campo (M396) culpaba al módulo de entrada, sin fragmento.
#[test]
fn an_unknown_field_type_in_an_imported_module_points_at_that_module() {
    let d = tmp("field_band");
    std::fs::write(d.join("main.ray"), "import geo;\nfn main() -> int {\n    0\n}\n").unwrap();
    std::fs::write(d.join("geo.ray"), "// geo\n\npub struct S {\n    a: int,\n    b: Nope,\n}\n").unwrap();
    let (out, err, code) = ray(&d, &["check", "main.ray"]);
    let all = format!("{out}{err}");
    assert_ne!(code, 0, "{all}");
    assert!(all.contains("[geo] type error at 5:8: unknown type: 'Nope' not declared"), "{all}");
    assert!(all.contains("b: Nope,"), "con el fragmento de geo.ray: {all}");
}
