//! M454: las instrucciones fundidas de la VM (operandos direccionados, campos por posición,
//! brazos de `match` sin pila) dan lo mismo que el intérprete — también por sus caminos lentos:
//! locales capturados (boxeados), tipos que no son int/float, y los errores con su posición.

use std::process::Command;

/// Corre `src` en la VM y en el intérprete: misma salida, mismo error, mismo código de salida.
/// Devuelve `(stdout, stderr)` de la VM.
fn same_on_both(name: &str, src: &str) -> (String, String) {
    let d = std::env::temp_dir().join(format!("ray_fused_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let run = |args: &[&str]| {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(&d).output().unwrap();
        (String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned(), o.status.code())
    };
    let vm = run(&["run", "prog.ray"]);
    let interp = run(&["run", "--interp", "prog.ray"]);
    assert_eq!(vm, interp, "{name}: la VM y el intérprete divergen");
    let _ = std::fs::remove_dir_all(&d);
    (vm.0, vm.1)
}

#[test]
fn addressed_arithmetic_matches_the_interpreter() {
    let (out, _) = same_on_both(
        "arith",
        r#"fn main() {
    var s = 0;
    var i = 0;
    while (i < 10) { s = s + i; i = i + 1; }
    let a = 7;
    let b = 3;
    var x = 1.5;
    let y = 0.25;
    x = x * y;
    x = x - 2.0;
    var t = a * b;
    t = t / 2;
    t = t % 4;
    let name = "ab";
    var text = name + name;
    text = text + "!";
    var xs = [1, 2];
    let ys = [3];
    xs = xs + ys;
    let u: u8 = 250 as u8;
    let w: u8 = 10 as u8;
    let z = u + w;
    print("${s} ${a - b} ${a * b + b} ${x} ${t} ${text} ${xs.len()} ${z} ${(a + b) * 2 - a} ${1.0 / y + x}");
}
"#,
    );
    assert_eq!(out, "45 4 24 -1.625 2 abab! 3 4 13 2.375\n");
}

#[test]
fn captured_locals_take_the_general_path() {
    let (out, _) = same_on_both(
        "boxed",
        r#"struct P { x: int, y: float, }
enum E { A(int), B, }
fn main() {
    var n = 1;
    var f = 0.5;
    var p = P { x: 2, y: 1.5 };
    var e = E.A(5);
    let bump = fn() -> int {
        n = n + 1;
        f = f * 2.0;
        p.x = p.x + n;
        e = E.B;
        n
    };
    n = n + 10;
    let k = bump() + n * 2;
    let m = match (e) {
        E.A(v) => v,
        E.B => 0 - 1,
    };
    if (f < 2.0) { print("lt") } else { print("ge") };
    if (p.y == 1.5) { print("eq") };
    print("${n} ${f} ${p.x} ${p.x + p.x} ${k} ${m}");
}
"#,
    );
    assert_eq!(out, "lt\neq\n12 1 14 28 36 -1\n");
}

#[test]
fn match_arms_and_field_reads_match_the_interpreter() {
    let (out, _) = same_on_both(
        "match",
        r#"struct A { v: int, name: string, }
struct B { name: string, v: int, }
enum Shape { Circle(float), Rect(float, float), Dot, Tagged(string, A), }
fn area(s: Shape) -> float {
    match (s) {
        Shape.Circle(r) => r * r * 3.0,
        Shape.Rect(w, h) => w * h,
        Shape.Dot => 0.0,
        Shape.Tagged(_, a) => a.v as float,
    }
}
fn first(o: Option<int>, r: Result<int, string>) -> int {
    let a = match (o) {
        Option.Some(v) => v,
        Option.None => 0,
    };
    match (r) {
        Result.Ok(v) => a + v,
        Result.Err(e) => a - e.len(),
    }
}
fn half(x: int) -> Result<int, string> {
    if (x % 2 == 0) { Result.Ok(x / 2) } else { Result.Err("odd") }
}
fn quarter(x: int) -> Result<int, string> {
    let h = half(x)?;
    half(h)
}
fn main() {
    let a = A { v: 1, name: "a" };
    let b = B { name: "b", v: 2 };
    var total = 0.0;
    for s in [Shape.Circle(1.0), Shape.Rect(2.0, 3.0), Shape.Dot, Shape.Tagged("t", a)] {
        match (s) {
            Shape.Dot => { },
            _ => total = total + area(s),
        }
    }
    print("${total} ${a.name}${b.name} ${a.v + b.v} ${first(Option.Some(4), Result.Ok(5))} ${first(Option.None, Result.Err("xyz"))}");
    match (quarter(12)) { Result.Ok(v) => print(v), Result.Err(e) => print(e), };
    match (quarter(6)) { Result.Ok(v) => print(v), Result.Err(e) => print(e), };
    var c = 0;
    let zero = 0.0;
    let nan = zero / zero;
    if (nan == nan) { c = c + 1; }
    if (nan != nan) { c = c + 10; }
    if (nan < 1.0) { c = c + 100; }
    if ("b" > a.name) { c = c + 1000; }
    print(c);
}
"#,
    );
    assert_eq!(out, "10 ab 3 9 -3\n3\nodd\n1010\n");
}

#[test]
fn errors_keep_their_message_and_position() {
    for (name, body, want) in [
        ("ovf", "var s = 9223372036854775807;\n    let one = 1;\n    s = s + one;\n    print(s);", "arithmetic overflow on int"),
        ("div", "let a = 4;\n    var z = 0;\n    z = a / z;\n    print(z);", "integer division by zero"),
        ("rem", "let a = 4;\n    let z = 0;\n    print(a % z + 1);", "modulo by zero"),
        ("mul", "var s = 4611686018427387904;\n    s = s * 2;\n    print(s);", "arithmetic overflow on int"),
        ("stack", "let a = [9223372036854775807];\n    let one = 1;\n    print(a[0] + one);", "arithmetic overflow on int"),
    ] {
        let (_, err) = same_on_both(name, &format!("fn main() {{\n    {body}\n}}\n"));
        assert!(err.contains(want), "{name}: {err}");
    }
}

#[test]
fn pattern_bindings_and_returns_from_locals_match_the_interpreter() {
    let (out, _) = same_on_both(
        "bind",
        r#"struct P { x: int, y: int, }
enum T { Leaf(int), Node(T, T), Pair((int, string)), Wrap(Option<P>), }
fn sum(t: T) -> int {
    match (t) {
        T.Leaf(v) => v,
        T.Node(T.Leaf(a), r) => a + sum(r),
        T.Node(l, r) => sum(l) + sum(r),
        T.Pair((n, s)) => n + s.len(),
        T.Wrap(Option.Some(P { x, y })) => x * y,
        T.Wrap(Option.None) => 0,
    }
}
fn shadow(o: Option<int>) -> int {
    let v = 100;
    let got = match (o) {
        Option.Some(v) => v + 1,
        Option.None => v,
    };
    got + v
}
fn counter() -> fn() -> int {
    var n = 0;
    fn() -> int {
        n = n + 1;
        n
    }
}
fn pick(a: [int], i: int) -> int {
    let v = a[i];
    v
}
fn keep(s: string) -> string {
    let t = s + "!";
    t
}
fn apply(f: fn(int, int) -> int, a: int, b: int) -> int { f(a, b) }
fn main() {
    let t = T.Node(T.Leaf(1), T.Node(T.Node(T.Leaf(2), T.Leaf(3)), T.Pair((4, "abc"))));
    print("${sum(t)} ${sum(T.Wrap(Option.Some(P { x: 3, y: 5 })))} ${sum(T.Wrap(Option.None))}");
    print("${shadow(Option.Some(1))} ${shadow(Option.None)}");
    let c = counter();
    let k = 10;
    let add = fn(a: int, b: int) -> int { a + b + k };
    print("${c()} ${c()} ${c()} ${pick([7, 8, 9], 1)} ${keep("a")} ${apply(add, 1, 2)} ${apply(fn(a: int, b: int) -> int { a * b }, 3, 4)}");
}
"#,
    );
    assert_eq!(out, "13 15 0\n102 200\n1 2 3 8 a! 13 12\n");
}
