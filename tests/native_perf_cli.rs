//! Arco de rendimiento del nativo (nov 2026): los programas mínimos de cada paso, idénticos en
//! VM, intérprete y nativo. Las mediciones viven en `benchmarks/probe/`; aquí solo se asevera que
//! cada optimización conserva el resultado.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_native_perf_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Corre `src` en VM, intérprete (salvo `interp = false`: fibras) y nativo; los tres dan `want`.
fn engines(name: &str, src: &str, want: &str, interp: bool) {
    let d = tmp(name);
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let mut runs: Vec<&[&str]> = vec![&["run", "prog.ray"]];
    if interp {
        runs.push(&["run", "--interp", "prog.ray"]);
    }
    for args in runs {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(&d).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{args:?}");
    }
    if has_rustc() {
        let bin = d.join("prog_bin");
        let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        let err = String::from_utf8_lossy(&b.stderr);
        assert!(b.status.success(), "build --native: {err}");
        assert!(!err.contains("not supported in the native subset"), "nativo con stubs: {err}");
        let o = Command::new(&bin).output().unwrap();
        assert!(o.status.success(), "nativo: {}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "nativo");
    }
}

/// Un tipo de usuario con el nombre de un tipo de Rust que el código generado usa sin calificar
/// (`Rc`, `Vec`, `String`, `Box`, `Send`…) compila a nativo y se muestra con su nombre original.
#[test]
fn user_types_named_like_rust_types_build_natively() {
    engines(
        "names",
        r#"@derive(Eq, Show)
struct Rc { w: int, }
@derive(Show)
struct Vec { n: int, }
struct String { s: string, }
struct Box { v: int, }
enum Arc { One(int), Two, }
struct RefCell { c: int, }
struct HashMap { k: int, }
struct Cell { k: int, }
trait Send { fn go(self) -> int; }
impl Send for Box { fn go(self) -> int { self.v } }
fn total<T: Send>(x: T) -> int { x.go() }
fn main() {
    let a = Rc { w: 1 };
    let v = Vec { n: 2 };
    let s = String { s: "x" };
    let b = Box { v: 3 };
    let e = Arc.One(4);
    let r = RefCell { c: 5 };
    let h = HashMap { k: 6 };
    let c = Cell { k: 7 };
    let xs = [a, Rc { w: 9 }];
    let m: Map<string, Rc> = Map.new();
    m.insert("k", a);
    print(a);
    print(v);
    print(xs.len() + m.len() + v.n + s.s.len() + total(b) + r.c + h.k + c.k);
    match (e) { Arc.One(x) => print(x), Arc.Two => print(0), }
    print(a == Rc { w: 1 });
}
"#,
        "Rc { w: 1 }\nVec { n: 2 }\n27\n4\ntrue\n",
        true,
    );
}

/// Paso 1: los literales de string en todos sus contextos (argumento, comparación, `contains`,
/// clave de Map, campo, elemento, payload, patrón, retorno) — de la tabla por hilo o como texto.
#[test]
fn string_literals_keep_their_meaning_in_every_context() {
    engines(
        "literals",
        r#"struct U { name: string, tag: string, }
enum E { A(string), B, }
fn takes(s: string) -> int { s.len() }
fn r(x: int) -> Result<int, string> { if (x > 0) { Result.Ok(x) } else { Result.Err("neg") } }
fn pick(x: int) -> string { if (x > 0) { "pos" } else { "neg" } }
fn main() {
    let line = "alpha,beta";
    var s = 0;
    s = s + takes("delta");
    if (line == "alpha") { s = s + 1; }
    if (line.contains("beta")) { s = s + 1; }
    if (line.starts_with("al")) { s = s + 1; }
    let u = U { name: "bob", tag: "x" };
    let xs = ["a", "b"];
    let m: Map<string, int> = Map.new();
    m.insert("k", 1);
    s = s + m.get("k").unwrap_or(0);
    let e = E.A("payload");
    var acc = "start";
    acc = "reset";
    match (pick(s)) { "pos" => s = s + 1, _ => {}, }
    let parts = line.split(",");
    let j = xs.join("-");
    let rep = line.replace("a", "b");
    print(s + u.name.len() + xs.len() + acc.len() + parts.len() + j.len() + rep.len());
    print(r(0));
}
"#,
        "34\nResult.Err(neg)\n",
        true,
    );
}

/// Paso 2: los diccionarios de un bound monomorfizados — reenvío entre genéricas, closures que
/// los capturan, recursión, `sort` genérico, bounds del prelude (Ord/Eq/Show) y una tarea.
#[test]
fn bound_dictionaries_behave_the_same_when_monomorphized() {
    engines(
        "dicts",
        r#"trait Sh { fn area(self) -> int; fn name(self) -> string; }
struct Sq { s: int, }
struct Re { w: int, h: int, }
impl Sh for Sq { fn area(self) -> int { self.s * self.s } fn name(self) -> string { "sq" } }
impl Sh for Re { fn area(self) -> int { self.w * self.h } fn name(self) -> string { "re" } }

@derive(Eq, Show)
struct Wrap { v: int, }

fn one<T: Sh>(x: T) -> int { x.area() }
fn twice<T: Sh>(x: T) -> int { one(x) + one(x) }
fn total<T: Sh>(xs: [T]) -> int { var t = 0; for x in xs { t = t + twice(x); } t }
fn names<T: Sh>(xs: [T]) -> [string] { xs.map(fn(x: T) -> string { x.name() + ":" + to_string(x.area()) }) }
fn biggest<T: Ord>(xs: [T], cur: T, i: int) -> T {
    if (i >= xs.len()) { return cur; }
    biggest(xs, if (cur.less(xs[i])) { xs[i] } else { cur }, i + 1)
}
fn same<T: Eq>(a: T, b: T) -> bool { a == b }
fn describe<T: Show>(x: T) -> string { "<" + x.show() + ">" }
fn apply(f: fn(Sq) -> int, x: Sq) -> int { f(x) }

fn main() {
    let sq = [Sq { s: 2 }, Sq { s: 3 }];
    let re = [Re { w: 2, h: 5 }];
    print(total(sq) + total(re));
    print(names(sq).join(","));
    print(names(re).join(","));
    print(biggest([3, 9, 4], 0, 0));
    print(biggest(["b", "z", "a"], "", 0));
    print(same(Wrap { v: 1 }, Wrap { v: 1 }));
    print(describe(Wrap { v: 8 }));
    print(describe([Wrap { v: 1 }, Wrap { v: 2 }]));
    print(apply(fn(q: Sq) -> int { one(q) }, Sq { s: 4 }));
    print([3, 1, 2].sort());
    print(["b", "a"].sort().contains("a"));
    let t = spawn(fn() -> int { total([Sq { s: 5 }]) });
    print(join(t));
}
"#,
        "46\nsq:4,sq:9\nre:10\n9\nz\ntrue\n<Wrap { v: 8 }>\n<[Wrap { v: 1 }, Wrap { v: 2 }]>\n16\n[1, 2, 3]\ntrue\n50\n",
        false,
    );
}
