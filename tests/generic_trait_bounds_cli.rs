//! raysolve #357: bounds sobre traits GENÉRICOS (`S: Source<V>`). El trait ya se podía declarar e
//! implementar; ahora acota un parámetro de tipo de una función, un struct o un impl. El tipo
//! determina los argumentos del trait (hay un impl por tipo), así que `V` se infiere del impl.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_gtb_{name}"));
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

const TRAITS: &str = "trait Source<V> {\n    fn get(self) -> V;\n    fn put(self, v: V) -> int;\n}\n\nstruct Seven { }\n\nimpl Source<int> for Seven {\n    fn get(self) -> int { 7 }\n    fn put(self, v: int) -> int { v }\n}\n\n";

const PROGRAM: &str = r#"trait Source<V> {
    fn get(self) -> V;
    fn put(self, v: V) -> int;
}

trait Pair<A, B> {
    fn left(self) -> A;
    fn right(self) -> B;
}

trait Peek {
    fn peek(self) -> string;
}

struct Fixed<V> {
    v: V,
}

impl<V> Source<V> for Fixed<V> {
    fn get(self) -> V { self.v }

    fn put(self, v: V) -> int { 1 }
}

struct Seven { }

impl Source<int> for Seven {
    fn get(self) -> int { 7 }

    fn put(self, v: int) -> int { v }
}

struct Boxed<T> {
    v: T,
}

impl<T: Show> Source<string> for Boxed<T> {
    fn get(self) -> string { "<" + self.v.show() + ">" }

    fn put(self, v: string) -> int { v.len() }
}

struct P { }

impl Pair<int, string> for P {
    fn left(self) -> int { 1 }

    fn right(self) -> string { "r" }
}

struct Solver<V, S: Source<V>> {
    src: S,
    seen: [V],
}

impl<V: Show, S: Source<V>> Peek for Solver<V, S> {
    fn peek(self) -> string { self.src.get().show() }
}

fn first<W, S: Source<W>>(s: S) -> W { s.get() }

fn store<V, S: Source<V>>(s: S, v: V) -> int { s.put(v) }

fn twice<S: Source<int>>(s: S) -> int { first(s) * 2 }

fn relay<A, T: Source<A>>(t: T) -> A { first(t) }

fn listed<V, S: Source<[V]>>(s: S) -> int { s.get().len() }

fn both<A: Show, B: Show, T: Pair<A, B>>(t: T) -> string { t.left().show() + t.right().show() }

fn run<V, S: Source<V>>(sv: Solver<V, S>) -> int {
    sv.seen.push(sv.src.get());
    sv.seen.len()
}

fn main() {
    print(first(Fixed { v: "a" }));
    print(first(Seven { }) + 1);
    print(store(Seven { }, 5));
    print(twice(Seven { }));
    print(relay(Fixed { v: 2.5 }));
    print(listed(Fixed { v: [1, 2, 3] }));
    print(first(Boxed { v: 4 }));
    print(both(P { }));
    print(run(Solver { src: Fixed { v: 3 }, seen: [1] }));
    print(run(Solver { src: Seven { }, seen: [] }));
    print(Solver { src: Boxed { v: 9 }, seen: ["x"] }.peek());
}
"#;

const EXPECTED: &str = "a\n8\n5\n14\n2.5\n3\n<4>\n1r\n2\n1\n<9>\n";

/// El parámetro de tipo del bound no tiene que llamarse como el del trait, se infiere del impl
/// (concreto, genérico o genérico acotado) y un literal de struct lo usa para sus campos. Igual
/// en la VM, el intérprete y nativo; y `ray fmt` lo deja como está.
#[test]
fn a_generic_trait_bounds_functions_structs_and_impls_on_every_engine() {
    let d = tmp("engines");
    std::fs::write(d.join("prog.ray"), PROGRAM).unwrap();
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, EXPECTED, "{args:?}");
    }
    if has_rustc() {
        let bin = d.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        assert_eq!(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout), EXPECTED);
    }
    let (out, err, code) = ray(&d, &["fmt", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, PROGRAM, "ray fmt keeps the bounds with their type arguments");
    // Un `[]` como ÚLTIMO argumento también espera a lo que fija el bound. Solo VM e intérprete:
    // en nativo un `[]` suelto pasado a un parámetro genérico sigue pidiendo anotación.
    std::fs::write(d.join("last.ray"), format!("{TRAITS}struct Fixed<V> {{ v: V }}\nimpl<V> Source<V> for Fixed<V> {{ fn get(self) -> V {{ self.v }} fn put(self, v: V) -> int {{ 1 }} }}\nfn store<V, S: Source<V>>(s: S, v: V) -> int {{ s.put(v) }}\nfn main() {{ print(store(Fixed {{ v: [1] }}, [])); }}\n")).unwrap();
    for args in [&["run", "last.ray"][..], &["run", "--interp", "last.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, "1\n", "{args:?}");
    }
}

/// Un trait de otro módulo acota igual (`S: ports.Source<ports.Item>`), y `ray doc` enseña el
/// bound con sus argumentos.
#[test]
fn a_generic_trait_of_another_module_can_bound() {
    let d = tmp("modules");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"gtb\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(d.join("src/ports.ray"), "pub struct Item { n: int }\npub trait Source<V> { fn get(self) -> V; }\npub fn first<V, S: Source<V>>(s: S) -> V { s.get() }\n").unwrap();
    std::fs::write(d.join("src/main.ray"), "import ports;\nstruct Mem { }\nimpl ports.Source<ports.Item> for Mem { fn get(self) -> ports.Item { ports.Item { n: 3 } } }\nfn mine<S: ports.Source<ports.Item>>(s: S) -> int { s.get().n + ports.first(s).n }\nfn main() { print(mine(Mem { })); }\n").unwrap();
    let (out, err, code) = ray(&d, &["run"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "6\n");
    let (out, err, code) = ray(&d, &["doc", "src/ports.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("fn first<V, S: Source<V>>(s: S) -> V"), "{out}");
}

/// Los errores: el argumento del bound no casa con el del impl (en una llamada, en un struct y
/// al reenviar un parámetro acotado), la aridad del bound y del impl, y un tipo que no existe.
#[test]
fn generic_bound_errors_say_what_does_not_match() {
    let d = tmp("errors");
    let check = |name: &str, body: &str| {
        std::fs::write(d.join(name), format!("{TRAITS}fn first<V, S: Source<V>>(s: S) -> V {{ s.get() }}\n\nfn store<V, S: Source<V>>(s: S, v: V) -> int {{ s.put(v) }}\n\n{body}\n")).unwrap();
        let (_o, err, code) = ray(&d, &["check", name]);
        assert_eq!(code, 65, "{name}: {err}");
        err
    };
    let err = check("ret.ray", "fn main() { let a: string = first(Seven { }); }");
    assert!(err.contains("'a' is declared as string but initialized with int"), "{err}");
    let err = check("arg.ray", "fn main() { print(store(Seven { }, \"x\")); }");
    assert!(err.contains("Seven implements 'Source<int>', but 'store' requires 'S: Source<string>'"), "{err}");
    let err = check("struct.ray", "struct W<S: Source<string>> { s: S }\nfn main() { let w = W { s: Seven { } }; }");
    assert!(err.contains("Seven implements 'Source<int>', but 'W' requires 'S: Source<string>'"), "{err}");
    let err = check("forward.ray", "fn h<S: Source<string>>(s: S) -> string { s.get() }\nfn k<T: Source<int>>(t: T) -> string { h(t) }\nfn main() { }");
    assert!(err.contains("T implements 'Source<int>', but 'h' requires 'S: Source<string>'"), "{err}");
    let err = check("bare.ray", "fn g<S: Source>(s: S) { }\nfn main() { }");
    assert!(err.contains("the trait 'Source' takes 1 type parameter(s): write the bound as 'S: Source<…>'"), "{err}");
    let err = check("extra.ray", "fn g<S: Eq<int>>(s: S) { }\nfn main() { }");
    assert!(err.contains("the trait 'Eq' takes 0 type parameter(s), but the bound 'S: Eq<…>' passes 1"), "{err}");
    let err = check("impl.ray", "struct B { }\nimpl Source for B { fn get(self) -> int { 1 } fn put(self, v: int) -> int { v } }\nfn main() { }");
    assert!(err.contains("the trait 'Source' takes 1 type parameter(s), but the impl passes 0"), "{err}");
    let err = check("unknown.ray", "fn g<S: Source<Nope>>(s: S) { }\nfn main() { }");
    assert!(err.contains("'Nope' not declared"), "{err}");
    let err = check("dyn.ray", "fn f(s: dyn Source<int>) -> int { 0 }\nfn main() { }");
    assert!(err.contains("'dyn Source<…>' is not supported yet"), "{err}");
}

/// Un bound sobre un trait cuyos métodos tienen parámetros de tipo propios (`Iterator<T>` y su
/// `map<U>`) corre en la VM; el build nativo lo dice en vez de fallar en `rustc`.
#[test]
fn a_bound_on_a_trait_with_generic_methods_runs_on_the_vm_and_is_named_by_the_native_build() {
    let d = tmp("iterator");
    std::fs::write(d.join("prog.ray"), "struct Count { n: int }\nimpl Iterator<int> for Count {\n    fn next(self) -> Option<int> {\n        if (self.n <= 0) { Option.None } else { self.n = self.n - 1; Option.Some(self.n + 1) }\n    }\n}\nfn total<I: Iterator<int>>(it: I) -> int {\n    var t = 0;\n    while let Option.Some(x) = it.next() { t = t + x; }\n    t\n}\nfn main() { print(total(Count { n: 4 })); }\n").unwrap();
    let (out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "10\n");
    if has_rustc() {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_ne!(code, 0);
        assert!(err.contains("'total' is bounded by the trait 'Iterator', whose method 'map' has type parameters of its own"), "{err}");
    }
}

/// Un parámetro del impl que se llama como el del trait lo tapa: `impl<V: Show> Source<string>
/// for Boxed<V>` (antes: «is Boxed<V>, but the trait requires Boxed<string>»). El método por
/// defecto, escrito en términos del trait, recibe el argumento del impl aunque mencione ese
/// mismo nombre (`Source<[V]>` → `twice` devuelve `[[V]]`).
#[test]
fn an_impl_type_parameter_may_share_the_name_of_the_trait_parameter() {
    let d = tmp("shadow");
    std::fs::write(
        d.join("prog.ray"),
        "trait Source<V> {\n    fn get(self) -> V;\n    fn twice(self) -> [V] { [self.get(), self.get()] }\n}\nstruct Boxed<V> { v: V }\nimpl<V: Show> Source<string> for Boxed<V> {\n    fn get(self) -> string {\n        let inner: V = self.v;\n        \"<\" + inner.show() + \">\"\n    }\n}\nstruct Pairs<V> { a: V }\nimpl<V> Source<[V]> for Pairs<V> { fn get(self) -> [V] { [self.a, self.a] } }\nstruct Same<V> { v: V }\nimpl<V> Source<V> for Same<V> { fn get(self) -> V { self.v } }\nfn first<V, S: Source<V>>(s: S) -> V { s.get() }\nfn main() {\n    print(Boxed { v: 4 }.get());\n    print(Boxed { v: 4 }.twice());\n    print(first(Boxed { v: 2.5 }));\n    print(first(Pairs { a: 1 }));\n    print(Pairs { a: \"x\" }.twice());\n    print(first(Same { v: 9 }));\n    print(Same { v: 9 }.twice());\n}\n",
    )
    .unwrap();
    let expected = "<4>\n[<4>, <4>]\n<2.5>\n[1, 1]\n[[x, x], [x, x]]\n9\n[9, 9]\n";
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, expected, "{args:?}");
    }
    if has_rustc() {
        let bin = d.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        assert_eq!(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout), expected);
    }
}

/// De la revisión previa a 1.27.108. Tres sitios donde el argumento del bound no se comprobaba o
/// no se sustituía: el bound de un impl genérico elegido por diccionario, un parámetro acotado
/// que sale del tipo ESPERADO, y el bound de un método que menciona el parámetro del trait.
#[test]
fn bound_arguments_are_checked_through_dictionaries_expected_types_and_method_bounds() {
    let d = tmp("review");
    let run = |name: &str, body: &str| {
        std::fs::write(d.join(name), format!("{TRAITS}fn first<V, S: Source<V>>(s: S) -> V {{ s.get() }}\n\n{body}\n")).unwrap();
        ray(&d, &["run", name])
    };
    let pair = "struct Pair<V, S> { a: V, b: S }\nimpl<V, S: Source<V>> Source<[V]> for Pair<V, S> {\n    fn get(self) -> [V] { [self.a, self.b.get()] }\n    fn put(self, v: [V]) -> int { 0 }\n}\n";
    let (out, err, code) = run("dict_ok.ray", &format!("{pair}fn main() {{ print(first(Pair {{ a: 1, b: Seven {{ }} }})); }}"));
    assert_eq!((out.as_str(), code), ("[1, 7]\n", 0), "{err}");
    // Antes: pasaba el checker y la VM moría con un ICE al indexar un int como string.
    let (_o, err, code) = run("dict_bad.ray", &format!("{pair}fn main() {{ let xs: [string] = first(Pair {{ a: \"s\", b: Seven {{ }} }}); print(xs[1].len()); }}"));
    assert_eq!(code, 65, "{err}");
    assert!(err.contains("Seven implements 'Source<int>', but the impl for Pair<string, Seven> requires 'S: Source<string>'"), "{err}");
    let drain = "fn drain<V, S: Source<V>>(v: V) -> fn(S) -> int { fn(s: S) -> int { s.put(v) } }\n";
    let (out, err, code) = run("expected_ok.ray", &format!("{drain}fn main() {{ let f: fn(Seven) -> int = drain(5); print(f(Seven {{ }})); }}"));
    assert_eq!((out.as_str(), code), ("5\n", 0), "{err}");
    let (_o, err, code) = run("expected_bad.ray", &format!("{drain}fn main() {{ let f: fn(Seven) -> int = drain(\"str\"); print(f(Seven {{ }})); }}"));
    assert_eq!(code, 65, "{err}");
    assert!(err.contains("Seven implements 'Source<int>', but 'drain' requires 'S: Source<string>'"), "{err}");
    std::fs::write(d.join("method.ray"), "trait Sink<V> { fn take(self, v: V) -> int; }\ntrait Feed<V> {\n    fn get(self) -> V;\n    fn feed<K: Sink<V>>(self, k: K) -> int { k.take(self.get()) }\n}\nstruct Seven { }\nimpl Feed<int> for Seven { fn get(self) -> int { 7 } }\nstruct Acc { }\nimpl Sink<int> for Acc { fn take(self, v: int) -> int { v + 1 } }\nfn main() { print(Seven { }.feed(Acc { })); }\n").unwrap();
    let (out, err, code) = ray(&d, &["run", "method.ray"]);
    assert_eq!((out.as_str(), code), ("8\n", 0), "{err}");
}
