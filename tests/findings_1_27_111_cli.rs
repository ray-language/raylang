//! Ronda de hallazgos re-medidos sobre 1.27.111, cada uno con su repro mínimo.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_f111_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

/// raysolve #358: dentro de una función genérica, el parámetro de tipo del llamador (`V` de
/// `fn h<V>`) es un tipo rígido, no una incógnita: una vez que un argumento liga con él el
/// parámetro del llamado, da contexto al argumento hermano, a la otra rama del `if` y a los demás
/// brazos del `match`, igual que un tipo concreto. Antes solo funcionaba con `int`, `string`…
#[test]
fn a_rigid_type_parameter_gives_context_like_a_concrete_type() {
    let d = tmp("rigid_context");
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    let expected = "2\n1\n0\n1\n4\n2\n0\n7\n0\n2\n1\n";
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!((out.as_str(), code), (expected, 0), "{args:?}: {err}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        assert_eq!(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout), expected);
    }
    // Un parámetro que ningún argumento liga sigue siendo un error de inferencia.
    std::fs::write(
        d.join("bad.ray"),
        "fn empty<V>() -> [V] { [] }\nfn pair<A, B>(a: [A], b: [B]) -> int { a.len() + b.len() }\nfn h<V>(a: [V]) -> int { pair(a, empty()) }\nfn main() { print(h([1])); }\n",
    )
    .unwrap();
    let (_out, err, code) = ray(&d, &["check", "bad.ray"]);
    assert_eq!(code, 65);
    assert!(err.contains("could not infer the type parameter 'V' of 'empty'"), "{err}");
}

const PROG: &str = r#"// raysolve #358
struct T<P, V> { p: P, vs: [V], }
fn empty<V>() -> [V] { [] }
fn full<V>() -> [V] { [] }
fn at_least<V>(v: V) -> [V] { [v] }
fn none<V>() -> Option<V> { Option.None }
fn any<P, V>(p: P) -> T<P, V> { T { p: p, vs: [] } }
fn count<V>(a: [V], b: [V]) -> int { a.len() + b.len() }
fn tally<W>(a: [W], b: [W]) -> int { a.len() + b.len() }
fn sat<P, V>(t: T<P, V>, u: T<P, V>) -> int { t.vs.len() + u.vs.len() + 1 }
// Los tres casos de raysolve.
fn h1<V>(a: [V]) -> int { count(a, empty()) }
fn h2<P, V>(t: T<P, V>, p: P) -> int { sat(t, any(p)) }
fn h3<V>(c: bool, v: V) -> int {
    let low = if (c) { full() } else { at_least(v) };
    low.len()
}
// Con nombres distintos en llamador y llamado, y en cualquier orden.
fn h4<P>(a: [P]) -> int { tally(empty(), a) + tally(a, empty()) }
// Un brazo del match fija el tipo del otro.
fn h5<V>(o: Option<V>, v: V) -> int {
    let xs = match (o) { Option.Some(x) => [x, v], Option.None => empty() };
    xs.len()
}
// La rama `then` indeterminada toma el tipo del `else`.
fn h6<V>(c: bool, v: V) -> Option<V> {
    let r = if (c) { none() } else { Option.Some(v) };
    r
}
// Un builtin cuyo contenedor es genérico.
fn h7<V>(v: V) -> int {
    var out: [Option<V>] = [];
    out.push(Option.Some(v));
    out.push(none());
    out.len()
}
fn h8<V>(a: [V]) -> int { count(a, if (a.len() > 0) { empty() } else { a }) }
fn main() {
    print(h1([1, 2]));
    let t: T<int, string> = any(1);
    print(h2(t, 2));
    print(h3(true, 3));
    print(h3(false, "x"));
    print(h4([1, 2]));
    print(h5(Option.Some("a"), "b"));
    print(h5(none(), 1));
    print(h6(false, 7).unwrap_or(0));
    print(h6(true, 7).unwrap_or(0));
    print(h7(1.5));
    print(h8(["x"]));
}
"#;
