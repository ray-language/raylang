//! Ronda de hallazgos pequeños re-medidos sobre 1.27.104 (raysolve #361, raycrawl #369,
//! rayfocus #375, ray-sublime #411), cada uno con su repro mínimo.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_f105_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// raysolve #361: `ray test tests/x.ray` en un paquete-librería (sin `src/main.ray`) encuentra
/// los módulos de `src/`, igual que `ray test` a secas.
#[test]
fn an_explicit_suite_of_a_library_package_sees_src() {
    let d = tmp("lib_suite");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"libsuite\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(d.join("src/util.ray"), "pub fn inc(x: int) -> int { x + 1 }\n").unwrap();
    std::fs::write(d.join("tests/util_test.ray"), "import util;\n@test\nfn increments() { assert_eq(util.inc(1), 2); }\n").unwrap();
    for args in [&["test"][..], &["test", "tests/util_test.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {out}{err}");
        assert!(out.contains("1 test(s), all passed"), "{args:?}: {out}{err}");
    }
}

/// raycrawl #369: aceptar sobre un listener de otro dominio da el mismo error en la VM y en
/// nativo (`invalid handle`; antes el nativo decía «is not a listener»).
#[test]
fn a_listener_from_another_domain_is_an_invalid_handle_in_both_engines() {
    let d = tmp("domain");
    std::fs::write(
        d.join("prog.ray"),
        "import std/net;\nfn main() {\n    let l = net.tcp_listen(\"127.0.0.1\", 0).unwrap();\n    let t = spawn_isolated(fn() -> string {\n        match (net.tcp_accept(l)) { Result.Ok(_) => \"ok\", Result.Err(e) => e, }\n    });\n    print(try_join(t).unwrap().starts_with(\"invalid handle: \"));\n}\n",
    )
    .unwrap();
    let (out, err, _) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(out, "true\n", "vm: {err}");
    if has_rustc() {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        assert_eq!(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout), "true\n");
    }
}

/// rayfocus #375: `save` crea la carpeta del archivo si falta, y un fallo dice la ruta.
#[test]
fn kv_save_creates_its_folder_and_names_the_path_on_failure() {
    let d = tmp("kv");
    std::fs::write(
        d.join("prog.ray"),
        "import std/kv;\nfn main() -> int {\n    let s = kv.open(\"a/b/state.kv\").unwrap();\n    s.set_string(\"k\", \"v\");\n    print(s.save().is_ok());\n    print(kv.open(\"a/b/state.kv\").unwrap().get_string(\"k\"));\n    let _ = kv.open(\"plain.kv\").unwrap().save();\n    let bad = kv.open(\"plain.kv/x/state.kv\").unwrap();\n    match (bad.save()) { Result.Ok(_) => print(\"saved\"), Result.Err(e) => print(e.contains(\"plain.kv/x/state.kv\")), }\n    0\n}\n",
    )
    .unwrap();
    let (out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "true\nOption.Some(v)\ntrue\n", "{err}");
    assert!(d.join("a/b/state.kv").is_file());
}

/// ray-sublime #411: `ray doc sleep` no presenta como global una función que solo existe
/// dentro de `std/time`; un builtin de verdad sigue saliendo pelado.
#[test]
fn ray_doc_names_the_module_of_a_function_that_is_not_a_global() {
    let d = tmp("doc");
    let (out, _e, _c) = ray(&d, &["doc", "sleep"]);
    assert!(out.starts_with("time.sleep(ms: int) -> unit\n"), "{out}");
    assert!(out.contains("import std/time;"), "{out}");
    let (out, _e, _c) = ray(&d, &["doc", "floor"]);
    assert!(out.starts_with("math.floor(") && out.contains("import std/math;"), "{out}");
    let (out, _e, _c) = ray(&d, &["doc", "len"]);
    assert!(out.starts_with("len(c) -> int\n") && !out.contains("Not a global"), "{out}");
}

/// raysolve #358: el contexto baja a lo que no se puede tipar solo. Una variante sin datos
/// dentro de un struct genérico, en una función genérica, toma su parámetro del campo; y un
/// `[]` o un `Option.None` dentro de un array toman el tipo de sus hermanos, vayan donde vayan.
#[test]
fn context_reaches_unit_variants_and_empty_arrays() {
    let d = tmp("infer");
    std::fs::write(
        d.join("prog.ray"),
        "enum Bound<V> { Incl(V), Unbounded, }\nstruct Seg<V> { lo: Bound<V>, hi: Bound<V>, }\nfn c<V>(v: V) -> Seg<V> { Seg { lo: Bound.Incl(v), hi: Bound.Unbounded } }\nfn open<V>(s: Seg<V>) -> bool { match (s.hi) { Bound.Unbounded => true, Bound.Incl(_) => false, } }\nfn main() -> int {\n    print(open(c(3)));\n    let xs = [[1, 2, 3], [3], []];\n    let ys = [[], [1]];\n    let zs = [Option.None, Option.Some(\"a\")];\n    print(xs);\n    print(ys);\n    print(zs);\n    0\n}\n",
    )
    .unwrap();
    let want = "true\n[[1, 2, 3], [3], []]\n[[], [1]]\n[Option.None, Option.Some(a)]\n";
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, want, "{args:?}");
    }
    if has_rustc() {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        assert_eq!(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout), want);
    }
    // Sin ningún elemento que se tipe solo, el error sigue pidiendo la anotación.
    std::fs::write(d.join("bad.ray"), "fn main() -> int { let a = [[], []]; 0 }\n").unwrap();
    let (_o, err, code) = ray(&d, &["check", "bad.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("cannot infer the type of [] here"), "{err}");
}

/// raysolve #357: `while let` (con `continue`, `break` y etiqueta) y supertraits (transitivos,
/// con un método por defecto que llama al supertrait), en los tres motores; `ray fmt` los deja
/// como se escribieron.
#[test]
fn while_let_and_supertraits_work_on_the_three_engines() {
    let d = tmp("while_let");
    let src = r#"trait Version: Ord + Show {
    fn bump(self) -> int;
    fn label(self) -> string {
        self.show() + "+"
    }
}

trait Named: Version {
    fn name(self) -> string;
}

struct Semver {
    major: int,
}

impl Ord for Semver {
    fn less(self, other: Semver) -> bool {
        self.major < other.major
    }
}

impl Show for Semver {
    fn show(self) -> string {
        "v${self.major}"
    }
}

impl Version for Semver {
    fn bump(self) -> int {
        self.major + 1
    }
}

impl Named for Semver {
    fn name(self) -> string {
        "semver"
    }
}

fn newest<N: Named>(a: N, b: N) -> string {
    let top = if (a.less(b)) { b } else { a };
    "${top.name()} ${top.label()} ${top.bump()}"
}

fn pop(xs: [int]) -> Option<int> {
    if (xs.len() == 0) { Option.None } else { Option.Some(remove_at(xs, xs.len() - 1)) }
}

fn main() -> int {
    print(newest(Semver { major: 1 }, Semver { major: 4 }));
    let it = [1, 2, 3].iter();
    while let Option.Some(x) = it.next() {
        print(x);
    }
    var stack = [10, 20, 30, 40];
    var sum = 0;
    while let Option.Some(v) = pop(stack) {
        if (v == 30) {
            continue;
        }
        if (v == 10) {
            break;
        }
        sum = sum + v;
    }
    print(sum);
    var rows = [1, 2];
    outer: while let Option.Some(r) = pop(rows) {
        var cols = [7, 8];
        while let Option.Some(c) = pop(cols) {
            if (c == 7) {
                continue outer;
            }
            print("${r}:${c}");
        }
    }
    0
}
"#;
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let want = "semver v4+ 5\n1\n2\n3\n60\n2:8\n1:8\n";
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, want, "{args:?}");
    }
    if has_rustc() {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        assert_eq!(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout), want);
    }
    let (out, err, code) = ray(&d, &["fmt", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, src, "ray fmt keeps `while let` and the supertraits as written");
}

/// raysolve #357: los errores de los supertraits y de lo que aún no se soporta.
#[test]
fn supertrait_and_generic_trait_errors_say_what_is_wrong() {
    let d = tmp("supertrait_errors");
    let check = |name: &str, src: &str| {
        std::fs::write(d.join(name), src).unwrap();
        let (_o, err, code) = ray(&d, &["check", name]);
        assert_eq!(code, 65, "{name}: {err}");
        err
    };
    let err = check("missing.ray", "trait A: Show { fn a(self) -> int; }\nstruct S { n: int, }\nimpl A for S { fn a(self) -> int { 1 } }\nfn main() -> int { 0 }\n");
    assert!(err.contains("the impl of 'A' for S requires an impl of its supertrait 'Show'"), "{err}");
    let err = check("unknown.ray", "trait A: Nope { fn a(self) -> int; }\nfn main() -> int { 0 }\n");
    assert!(err.contains("the supertrait 'Nope' of 'A' is not a declared trait"), "{err}");
    let err = check("bound.ray", "trait Source<V> { fn get(self) -> V; }\nfn f<V, S: Source<V>>(s: S) -> int { 0 }\nfn main() -> int { 0 }\n");
    assert!(err.contains("a bound cannot take type arguments yet ('S: Source<…>')"), "{err}");
    let err = check("dyn.ray", "trait Source<V> { fn get(self) -> V; }\nfn f(s: dyn Source<int>) -> int { 0 }\nfn main() -> int { 0 }\n");
    assert!(err.contains("'dyn Source<…>' is not supported yet"), "{err}");
}
