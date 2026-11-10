//! M445 — los hallazgos de lenguaje de pepa (ray-apps a 1.27.90, #321–#323 y #331), cada uno con
//! su programa mínimo en los motores que aplican. (#328 y #332 tienen sus tests en `bundle_ios`
//! y `devlink`.)
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_pepa_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(args)
        .current_dir(dir)
        .env("RAY_UI_BACKEND", "headless")
        .env("RAY_UI_TRACE", "1")
        .output()
        .unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Corre `prog.ray` de `dir` en VM, intérprete y (si hay rustc) nativo; los tres deben dar `want`
/// en stdout y contener `want_err` en stderr.
fn three_engines(dir: &PathBuf, want: &str, want_err: &str) {
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(dir, engine);
        assert_eq!(code, 0, "{engine:?}: {err}");
        assert_eq!(out, want, "{engine:?}");
        assert!(err.contains(want_err), "{engine:?}: {err}");
    }
    if has_rustc() {
        let bin = dir.join("prog_bin");
        let (_o, err, code) = ray(dir, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        assert!(!err.contains("not supported in the native subset"), "nativo con stubs: {err}");
        let out = Command::new(&bin).env("RAY_UI_BACKEND", "headless").env("RAY_UI_TRACE", "1").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo");
        assert!(String::from_utf8_lossy(&out.stderr).contains(want_err), "nativo: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// pepa #321: el resultado descartado de `sort`/`sort_by`/`trim` avisa en `ray check` y `ray run`
/// (stderr, sin cambiar el código de salida); una redefinición del usuario calla el aviso.
#[test]
fn a_discarded_sort_warns_without_failing() {
    let d = tmp("warn");
    std::fs::write(
        d.join("prog.ray"),
        "fn main() -> int {\n    var xs = [3, 1, 2];\n    sort(xs);\n    xs.sort();\n    let s = \" a \";\n    s.trim();\n    xs = sort(xs);\n    print(xs);\n    0\n}\n",
    )
    .unwrap();
    let (out, err, code) = ray(&d, &["check", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("compiles"), "{out}");
    assert!(err.contains("warning at 3:5: the result of `sort(…)` is discarded: `sort` returns a new array and leaves its argument unchanged (assign it: `x = sort(…)`)"), "{err}");
    assert!(err.contains("warning at 4:5: the result of `xs.sort()` is discarded"), "{err}");
    assert!(err.contains("warning at 6:5: the result of `s.trim()` is discarded: `trim` returns a new string"), "{err}");
    assert!(err.contains("  3 |     sort(xs);"), "render con la línea de fuente: {err}");
    let (out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!((out.as_str(), code), ("[1, 2, 3]\n", 0), "{err}");
    assert_eq!(err.matches("warning at").count(), 3, "{err}");
    // Un `fn sort` propio: ninguna queja (puede tener otra semántica).
    std::fs::write(d.join("prog.ray"), "fn sort(xs: [int]) -> [int] { xs }\nfn main() -> int { let xs = [1]; sort(xs); xs.sort(); 0 }\n").unwrap();
    let (_out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(!err.contains("warning"), "{err}");
}

/// pepa #322: `@derive(ToJson)` con campos `[T]`, `Option<T>` y `Map<string, T>`, anidados —
/// idéntico en los tres motores; una clave que no es string es error con el motivo.
#[test]
fn derive_tojson_serializes_arrays_options_and_maps() {
    let d = tmp("derive");
    std::fs::write(
        d.join("prog.ray"),
        r#"from std/json import ToJson;

@derive(ToJson)
struct Dose { at: string, mg: int }

@derive(ToJson)
struct Rule { days: [int], doses: [Dose], note: Option<string>, nested: [[int]], tags: Map<string, int>, extra: Map<string, [Option<Dose>]>, empty: [Dose] }

fn main() -> int {
    let tags: Map<string, int> = Map.new();
    tags.insert("b", 2);
    tags.insert("a", 1);
    let extra: Map<string, [Option<Dose>]> = Map.new();
    extra.insert("x", [Option.Some(Dose { at: "08:00", mg: 5 }), Option.None]);
    let r = Rule { days: [1, 2], doses: [Dose { at: "20:30", mg: 5 }], note: Option.None, nested: [[1], [], [2, 3]], tags: tags, extra: extra, empty: [] };
    print(r.to_json());
    print(Rule { days: [], doses: [], note: Option.Some("a\"b"), nested: [], tags: Map.new(), extra: Map.new(), empty: [] }.to_json());
    0
}
"#,
    )
    .unwrap();
    three_engines(
        &d,
        "{\"days\":[1,2],\"doses\":[{\"at\":\"20:30\",\"mg\":5}],\"note\":null,\"nested\":[[1],[],[2,3]],\"tags\":{\"a\":1,\"b\":2},\"extra\":{\"x\":[{\"at\":\"08:00\",\"mg\":5},null]},\"empty\":[]}\n{\"days\":[],\"doses\":[],\"note\":\"a\\\"b\",\"nested\":[],\"tags\":{},\"extra\":{},\"empty\":[]}\n",
        "",
    );
    std::fs::write(d.join("prog.ray"), "from std/json import ToJson;\n@derive(ToJson)\nstruct S { m: Map<int, int> }\nfn main() { print(S { m: Map.new() }.to_json()); }\n").unwrap();
    let (_out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("type error at 2:1: cannot derive ToJson for a field of type Map<int, int> (a JSON object needs string keys, not int)"), "{err}");
}

/// pepa #323: `m.clear()` vacía el mapa en el sitio, en los tres motores.
#[test]
fn map_clear_empties_in_place_on_all_engines() {
    let d = tmp("map_clear");
    std::fs::write(
        d.join("prog.ray"),
        "fn main() -> int {\n    let m: Map<string, int> = Map.new();\n    m.insert(\"a\", 1);\n    m.insert(\"b\", 2);\n    print(m.len());\n    m.clear();\n    print(m.len());\n    print(m.contains_key(\"a\"));\n    m.insert(\"c\", 3);\n    print(m.keys());\n    0\n}\n",
    )
    .unwrap();
    three_engines(&d, "2\n0\nfalse\n[c]\n", "");
}

/// pepa #331: `ui.haptic(kind)` — `Ok` con la traza headless en los tres motores; `Err` con un
/// tipo desconocido (sin tocar el sistema).
#[test]
fn ui_haptic_traces_headless_and_rejects_unknown_kinds() {
    let d = tmp("haptic");
    std::fs::write(
        d.join("prog.ray"),
        "import std/ui;\nfn main() -> int {\n    print(ui.haptic(\"success\"));\n    print(ui.haptic(\"light\"));\n    print(ui.haptic(\"buzz\"));\n    0\n}\n",
    )
    .unwrap();
    three_engines(
        &d,
        "Result.Ok(0)\nResult.Ok(0)\nResult.Err(ui: unknown haptic 'buzz' (light, medium, heavy, soft, rigid, selection, success, warning or error))\n",
        "[ui] haptic success\n[ui] haptic light\n",
    );
}

/// pepa #342: `ui.next_event_timeout(0)` no espera (antes, 0 = sin plazo y el bucle se colgaba);
/// `try_next_event` tampoco; un valor positivo sigue esperando su plazo.
#[test]
fn a_zero_event_timeout_does_not_wait() {
    let d = tmp("poll");
    std::fs::write(
        d.join("prog.ray"),
        "import std/ui;\nfn main() -> int {\n    let w = ui.open(\"t\", \"about:blank\", 300, 200).unwrap();\n    print(ui.next_event_timeout(0));\n    print(ui.try_next_event());\n    print(ui.next_event_timeout(20));\n    0\n}\n",
    )
    .unwrap();
    three_engines(&d, "Result.Ok(Option.None)\nOption.None\nResult.Ok(Option.None)\n", "");
}
