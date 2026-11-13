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

/// Paso 3: enums de valor (datos escalares) junto a enums compartidos — derives, impl de trait,
/// `Ord` propio y `sort`, campo de struct, patrones anidados y de tupla, recursión, canales
/// (incluido un enum que lleva un canal) y `clone`.
#[test]
fn value_enums_behave_like_shared_ones() {
    engines(
        "enums",
        r#"
@derive(Eq, Show, Hash, Clone)
enum Color { Red, Green, Rgb(int, int, int), }
@derive(Eq, Show)
enum Op { Push(int), Add, Jmp(int, bool), Scale(float), }
enum Tree { Leaf(int), Node(Tree, Tree), }
enum Msg { Ask(int, Channel<int>), Stop, }
struct Pix { c: Color, n: int, }
trait Cost { fn cost(self) -> int; }
impl Cost for Op { fn cost(self) -> int { match (self) { Op.Push(v) => v, Op.Add => 1, Op.Jmp(t, c) => if (c) { t } else { 0 }, Op.Scale(f) => f as int, } } }
impl Ord for Color { fn less(self, other: Color) -> bool { rank(self) < rank(other) } }
fn rank(c: Color) -> int { match (c) { Color.Red => 0, Color.Green => 1, Color.Rgb(r, g, b) => 2 + r + g + b, } }
fn sum(t: Tree) -> int { match (t) { Tree.Leaf(v) => v, Tree.Node(l, r) => sum(l) + sum(r), } }
fn total<T: Cost>(xs: [T]) -> int { var t = 0; for x in xs { t = t + x.cost(); } t }
fn pick(n: int) -> Option<Color> { if (n > 0) { Option.Some(Color.Rgb(n, 0, 0)) } else { Option.None } }
fn actor(inbox: Channel<Msg>) {
    while (true) { match (recv(inbox)) { Option.Some(m) => match (m) { Msg.Ask(x, reply) => send(reply, x * 2), Msg.Stop => break, }, Option.None => break, } }
}
fn main() {
    let ops = [Op.Push(3), Op.Add, Op.Jmp(7, true), Op.Scale(2.5)];
    print(total(ops));
    print(ops[0] == Op.Push(3));
    print(ops);
    let p = Pix { c: Color.Rgb(1, 2, 3), n: 1 };
    p.c = Color.Green;
    print(p.c);
    print([Color.Rgb(9, 0, 0), Color.Red, Color.Green].sort());
    match (pick(4)) { Option.Some(Color.Rgb(r, _, _)) => print(r), Option.Some(_) => print(-1), Option.None => print(0), }
    match ((Color.Red, Op.Add)) { (Color.Red, Op.Add) => print("ra"), (_, _) => print("other"), }
    print(sum(Tree.Node(Tree.Leaf(1), Tree.Node(Tree.Leaf(2), Tree.Leaf(3)))));
    let ch: Channel<Op> = Channel.new();
    let t = spawn(fn() -> int { match (recv(ch)) { Option.Some(o) => o.cost(), Option.None => -1, } });
    send(ch, Op.Jmp(42, true));
    print(join(t));
    let inbox: Channel<Msg> = Channel.new();
    let a = spawn(fn() { actor(inbox); });
    let reply: Channel<int> = Channel.bounded(1);
    send(inbox, Msg.Ask(21, reply));
    print(recv(reply).unwrap_or(0));
    send(inbox, Msg.Stop);
    join(a);
    let c2 = Color.Rgb(5, 5, 5).clone();
    print(c2);
    var cur = Op.Add;
    cur = Op.Push(9);
    print(cur.cost());
}
"#,
        "13\ntrue\n[Op.Push(3), Op.Add, Op.Jmp(7, true), Op.Scale(2.5)]\nColor.Green\n[Color.Red, Color.Green, Color.Rgb(9, 0, 0)]\n4\nra\n6\n42\n42\nColor.Rgb(5, 5, 5)\n9\n",
        false,
    );
}

/// Paso 4: préstamos izados fuera del bucle — escritura y lectura del mismo arreglo, `if`/`else`,
/// `while` con `break`, dos nombres del MISMO arreglo, filas de un arreglo de arreglos (incluida
/// una fila repetida en la rejilla) y bucles de cero vueltas con un índice de fila inválido.
#[test]
fn hoisted_borrows_keep_aliasing_and_bounds_behaviour() {
    engines(
        "hoist",
        r#"fn fill(n: int, v: int) -> [int] { var xs: [int] = []; for i in 0..n { xs.push(v + i); } xs }
fn main() {
    // escritura + lectura del mismo arreglo, if/else, while anidado, break/continue
    var a = fill(10, 0);
    for i in 1..10 { if (a[i] % 2 == 0) { a[i] = a[i - 1] + 1; } else { continue; } }
    print(a);
    var i = 0;
    while (i < 10) { a[i] = a[i] * 2; if (a[i] > 12) { break; } i = i + 1; }
    print(a);
    // dos nombres del MISMO arreglo: escribir por uno y leer por el otro
    let b = a;
    for k in 0..5 { b[k] = a[9 - k] + k; }
    print(a);
    // arreglos distintos, uno escrito
    var c = fill(10, 100);
    for k in 0..10 { c[k] = a[k] + c[k]; }
    print(c);
    // filas: dp, la misma fila dos veces, y una fila repetida en la rejilla (alias)
    var g: [[int]] = [];
    for y in 0..4 { g.push(fill(5, y * 10)); }
    for y in 1..4 { for x in 1..5 { let up = g[y - 1][x]; let left = g[y][x - 1]; g[y][x] = g[y][x] + if (up < left) { up } else { left }; } }
    print(g);
    let shared = fill(3, 1);
    var h: [[int]] = [shared, shared, fill(3, 7)];
    for x in 0..3 { h[1][x] = h[0][x] + h[2][x]; }
    print(h);
    // floats y bools
    var fs: [float] = [1.5, 2.5, 3.5];
    var flags: [bool] = [false, false, false];
    for k in 0..3 { fs[k] = fs[k] * 2.0; flags[k] = fs[k] > 4.0; }
    print(fs);
    print(flags);
    // bucle de cero vueltas con un índice de fila inválido: no debe fallar
    let y0 = 0;
    for x in 0..0 { g[y0 - 1][x] = 1; }
    for x in 3..1 { g[99][x] = 1; }
    print("ok");
}
"#,
        "[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]\n[0, 2, 4, 6, 8, 10, 12, 14, 8, 9]\n[9, 9, 16, 15, 14, 10, 12, 14, 8, 9]\n[109, 110, 118, 118, 118, 115, 118, 121, 116, 118]\n[[0, 1, 2, 3, 4], [10, 12, 14, 16, 18], [20, 33, 36, 39, 42], [30, 61, 68, 72, 76]]\n[[8, 10, 12], [8, 10, 12], [7, 8, 9]]\n[3, 5, 7]\n[false, true, true]\nok\n",
        true,
    );
}

/// Revisión previa a 1.27.96: (1) una asignación dentro de un bloque con VALOR (el `if` de un
/// `let`, la condición de un `while`) cambia el índice de una fila — no se iza; (2) `xs.push(v)`
/// con un valor que reasigna `xs` conserva el orden de la VM (arreglo, luego valor); (3) comparar
/// una variable con un literal, en ambos órdenes.
#[test]
fn hoisting_and_push_respect_assignments_inside_value_blocks() {
    engines(
        "review_1_27_96",
        r#"fn main() {
    var g: [[int]] = [[1, 2, 3], [10, 20, 30], [100, 200, 300]];
    var k = 0;
    var total = 0;
    for i in 0..3 {
        let v = if (i > 0) { k = k + 1; 1 } else { 0 };
        total = total + g[k][i] + v;
    }
    print(total);
    k = 0;
    var i = 0;
    total = 0;
    while ({ k = (k + 1) % 3; i < 3 }) {
        total = total + g[k][i];
        i = i + 1;
    }
    print(total);
    var xs = [1];
    let old = xs;
    xs.push({ xs = [9]; 2 });
    print(old);
    print(xs);
    let line = "alpha";
    print(line == "alpha");
    print("beta" != line);
}
"#,
        "323\n213\n[1, 2]\n[9]\ntrue\ntrue\n",
        true,
    );
}

/// M452: la profundidad de recursión como parámetro oculto corta donde corta la VM — recursión
/// profunda bajo el límite, de cola (no cuenta marcos), mutua, a través de un valor función y de
/// una función que no cuenta, un desbordamiento atrapado por `try_call` (y la cuenta sigue bien
/// después) y closures que vuelven a entrar desde un builtin.
#[test]
fn recursion_depth_by_parameter_matches_the_vm() {
    engines(
        "depth_param",
        r#"fn down(n: int) -> int { if (n == 0) { 0 } else { 1 + down(n - 1) } }
fn tail(n: int, acc: int) -> int { if (n == 0) { acc } else { tail(n - 1, acc + 1) } }
fn even(n: int) -> bool { if (n == 0) { true } else { odd(n - 1) } }
fn odd(n: int) -> bool { if (n == 0) { false } else { even(n - 1) } }
fn via(f: fn(int) -> int, n: int) -> int { if (n == 0) { 0 } else { 1 + f(n - 1) } }
fn helper(n: int) -> int { down(n) + 1 }
fn main() {
    print(down(900));
    print(tail(1000000, 0));
    print(even(500));
    print(helper(900));
    print(via(fn(k: int) -> int { down(k) }, 800));
    let r = try_call(fn() { print(down(2000)); });
    print(r);
    print(down(1000));
    print([1, 2, 3].map(fn(x: int) -> int { down(x * 100) }));
}
"#,
        "900\n1000000\ntrue\n901\n800\nResult.Err(stack overflow (recursion too deep: 1024 frames; RAYLANG_MAX_DEPTH raises the limit))\n1000\n[100, 200, 300]\n",
        false,
    );
}
