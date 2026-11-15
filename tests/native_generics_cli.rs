//! Genéricos en el backend nativo (hallazgos de raysolve sobre 1.27.95): lo que la VM aceptaba
//! y `ray build --native` rechazaba con un error de rustc o de tipado.
use std::process::Command;

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn project(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_generics_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"g\"\nversion = \"0.1.0\"\n").unwrap();
    for (path, text) in files {
        std::fs::write(d.join(path), text).unwrap();
    }
    d
}

/// VM, intérprete y (si hay rustc) el binario nativo dan `want`.
fn same_everywhere(d: &std::path::Path, want: &str) {
    for args in [&["run"][..], &["run", "--interp"][..]] {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(d).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{args:?}");
    }
    if has_rustc() {
        let bin = d.join("prog_bin");
        let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "--native", "-o", bin.to_str().unwrap()]).current_dir(d).output().unwrap();
        assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
        let o = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "nativo");
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn generic_arrays_fn_fields_and_show_build_natively() {
    // #354: `contains`/`position` sobre un `[P: Eq]` comparaban con `==` (E0369), y un arreglo
    // como `P` pedía `[]#eq`. #355: el retorno de un campo `fn` de un struct genérico quedaba
    // en `P`. #359: `print` de un arreglo ignoraba el `Show` del elemento.
    let d = project("native", &[("src/main.ray", r#"@derive(Eq)
struct Pt { x: int, y: int, }
fn has<P: Eq>(xs: [P], x: P) -> bool { xs.contains(x) }
fn at<P: Eq>(xs: [P], x: P) -> Option<int> { xs.position(x) }
fn nested<P: Eq>(xs: [P], x: P) -> bool { has(xs, x) }
fn eq_cases() -> int {
    print(has(["a", "b"], "b"));
    print(has([1.5, 2.5], 3.5));
    print(has([Pt { x: 1, y: 2 }], Pt { x: 1, y: 2 }));
    print(at([10, 20, 30], 30));
    print(at(["a"], "z"));
    print(nested([[1, 2], [3]], [3]));
    print([1, 2].contains(2));
    0
}
struct Provider<P> { deps: fn(P) -> [(P, int)], name: fn(P) -> P, }
fn first<P>(prov: Provider<P>, p: P) -> P { prov.deps(p)[0].0 }
fn field_cases() -> int {
    let prov = Provider {
        deps: fn(p: string) -> [(string, int)] { [("net", 14), (p, 1)] },
        name: fn(p: string) -> string { p + "!" },
    };
    for d in prov.deps("x") { print("dep ${d.0} ${d.1 + 1}"); }
    let n = prov.name("db");
    print(n.len());
    print(first(prov, "y"));
    0
}
struct Plain { n: int, }
impl Show for Plain { fn show(self) -> string { "P${self.n}" } }
fn main() -> int {
    let _ = eq_cases();
    let _ = field_cases();
    print([Plain { n: 4 }, Plain { n: 5 }]);
    0
}
"#)]);
    same_everywhere(&d, "true\nfalse\ntrue\nOption.Some(2)\nOption.None\ntrue\ntrue\ndep net 15\ndep x 2\n3\nnet\n[P4, P5]\n");
}

#[test]
fn a_module_qualified_trait_is_a_bound() {
    // #391: `<T: m.Codec>` era un error de sintaxis; `impl m.Codec for A` ya valía.
    let d = project(
        "qualified",
        &[
            ("src/m.ray", "pub trait Codec { fn size(self) -> int; }\npub trait Named { fn name(self) -> string; }\n"),
            (
                "src/main.ray",
                r#"import m;
struct A { n: int, }
impl m.Codec for A { fn size(self) -> int { self.n } }
impl m.Named for A { fn name(self) -> string { "a" } }
struct W<T> { v: T, }
impl<T: m.Codec> m.Codec for W<T> { fn size(self) -> int { self.v.size() + 1 } }
fn twice<T: m.Codec>(v: T) -> int { v.size() * 2 }
fn both<T: m.Codec + m.Named>(v: T) -> string { "${v.name()}${v.size()}" }
fn main() -> int {
    print(twice(A { n: 3 }));
    print(both(A { n: 4 }));
    print(twice(W { v: A { n: 5 } }));
    0
}
"#,
            ),
        ],
    );
    same_everywhere(&d, "6\na4\n12\n");
}

#[test]
fn a_derive_that_cannot_be_generated_points_at_the_annotation() {
    // #360/#432: el error salía en una línea sintética (`50999995:1`) y, entre módulos,
    // atribuido a otro. Ahora señala el `@derive` y nombra el trait, el tipo y el campo.
    let d = project(
        "derive",
        &[
            ("src/version.ray", "// versión\n\n@derive(Eq, Hash)\npub struct V {\n    major: int,\n    pre: [string],\n}\n"),
            ("src/main.ray", "import version;\nimport std/collections/set;\nfn main() -> int {\n    var s: set.Set<version.V> = set.new();\n    0\n}\n"),
        ],
    );
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run"]).current_dir(&d).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("[version] type error at 3:1: cannot derive Hash for 'version::V' (field 'pre'): no field or function 'hash' applicable to [string]"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&d);

    let d = project(
        "derive_eq",
        &[("src/main.ray", "@derive(Eq)\nstruct Outer { name: string, inner: Map<string, int>, }\nfn main() -> int { 0 }\n")],
    );
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run"]).current_dir(&d).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("type error at 1:1: cannot derive Eq for 'Outer' (field 'inner'): "), "{err}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_local_closure_shadows_a_generic_function_of_the_same_name() {
    // Una función genérica `f` del programa hacía que la llamada a un PARÁMETRO `f: fn() -> …`
    // saliera con los argumentos de tipo de la global (`f::<…>()`: E0109 de rustc). La local
    // tapa a la global, como en el checker y en la VM.
    let d = project(
        "shadow",
        &[(
            "src/main.ray",
            r#"fn f<T: Show>(x: T) -> string { "<${x}>" }
fn g<T>(x: T) -> T { x }
fn apply(f: fn() -> [string]) -> [string] { f() }
fn twice(g: fn(int) -> int, n: int) -> int { g(g(n)) }
fn main() -> int {
    print(f(1));
    var out: [[string]] = [];
    out.push(apply(fn() -> [string] { ["a"] }));
    print(out);
    print(twice(fn(n: int) -> int { n * 3 }, 2));
    let f = fn(s: string) -> string { s + "!" };
    print(f("hi"));
    print(g("z"));
    0
}
"#,
        )],
    );
    same_everywhere(&d, "<1>\n[[a]]\n18\nhi!\nz\n");
}
