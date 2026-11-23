//! Huecos del backend nativo que destapó el harness diferencial al sortear el TIPO de cada sonda
//! (nov 2026, `tests/native_differential.rs`, generadores estructurales): programas que el checker
//! acepta y la VM ejecuta, y que `ray build --native` no compilaba o hacía morir en ejecución.
//! Aquí quedan los casos exactos, con su salida fija; la clase la vigila la campaña diferencial.
use std::process::Command;

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// La VM y (si hay rustc) el binario nativo dan `want`; con `interp`, también el intérprete.
fn same_everywhere(name: &str, source: &str, interp: bool, want: &str) {
    let d = std::env::temp_dir().join(format!("raylang_test_gaps_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let src = d.join("main.ray");
    std::fs::write(&src, source).unwrap();
    let src = src.to_str().unwrap();
    let mut engines: Vec<&[&str]> = vec![&["run"]];
    if interp {
        engines.push(&["--interp"]);
    }
    for args in engines {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).arg(src).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{args:?}");
    }
    if has_rustc() {
        let bin = d.join("prog_bin");
        let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", src, "--native", "-o", bin.to_str().unwrap()]).output().unwrap();
        let err = String::from_utf8_lossy(&b.stderr);
        assert!(b.status.success(), "build --native: {err}");
        assert!(!err.contains("not supported in the native subset"), "el nativo dejó una función como stub: {err}");
        let o = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "nativo: {}", String::from_utf8_lossy(&o.stderr));
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// Leer un contenedor y mutarlo en la MISMA expresión. El guard del `.borrow()` de `xs.len()`,
/// `m.contains_key(k)` o `xs.reverse()` era un temporal que vivía hasta el final de la sentencia:
/// la llamada siguiente hacía `borrow_mut` y el binario moría con «RefCell already borrowed».
#[test]
fn reading_and_mutating_a_container_in_one_expression() {
    same_everywhere(
        "aliasing",
        r#"struct Bag { items: [int], n: int, tags: Map<string, int>, }
fn bump(b: Bag) -> int {
    b.n = b.n + 1;
    b.items.push(7);
    b.tags.insert("k${b.n}", b.n);
    b.n
}
fn grow(xs: [int]) -> int {
    xs.push(xs.len());
    xs.len()
}
fn put(m: Map<string, int>, k: string) -> bool {
    m.insert(k, 1);
    true
}
fn main() {
    var b = Bag { items: [1], n: 3, tags: Map.new() };
    print(b.items.len() + bump(b));
    print(b.items.len() + bump(b) + b.items.len());
    print(b.tags.len() + bump(b));
    print(b.tags.contains_key("k4") && bump(b) > 0);
    var xs = [1, 2, 3];
    print(xs.len() + grow(xs));
    print(xs.reverse().len() + grow(xs));
    print(xs.contains(3) && grow(xs) > 0);
    var m: Map<string, int> = Map.new();
    print(m.contains_key("a") || put(m, "a"));
    print(m.len() + (if (put(m, "b")) { 1 } else { 0 }));
}
"#,
        true,
        "5\n10\n8\ntrue\n7\n9\ntrue\ntrue\n2\n",
    );
}

/// `map`/`filter`/`fold` con un closure que toca el arreglo que recorren: van por índice sobre el
/// arreglo vivo, como el prelude (ven lo añadido), en vez de tenerlo prestado toda la pasada.
#[test]
fn higher_order_functions_whose_closure_touches_the_array() {
    same_everywhere(
        "hof",
        r#"fn main() {
    var xs = [1, 2, 3];
    let ys = xs.map(fn(x: int) -> int {
        if (x == 1) { xs.push(100); }
        x * 2
    });
    print(ys);
    let zs = xs.filter(fn(x: int) -> bool {
        if (x == 2) { xs.push(200); }
        x > 2
    });
    print(zs);
    print(xs.fold(0, fn(acc: int, x: int) -> int {
        if (x == 3) { xs.push(300); }
        acc + 1
    }));
    print(xs);
    let k = 3;
    print(xs.map(fn(x: int) -> int { x * k + 1 }).filter(fn(x: int) -> bool { x % 2 == 0 }).fold(0, fn(a: int, x: int) -> int { a + x }));
}
"#,
        true,
        "[2, 4, 6, 200]\n[3, 100, 200]\n6\n[1, 2, 3, 100, 200, 300]\n14\n",
    );
}

/// Un closure local usado como VALOR dentro de un array, una tupla, un `Option` o las ramas de un
/// `if`/`match` (dos closures son dos tipos de Rust: E0308), y la llamada a una EXPRESIÓN de tipo
/// función («call to an expression (neither a name nor a method) is not supported»).
#[test]
fn local_closures_as_values_and_calls_on_expressions() {
    same_everywhere(
        "closures",
        r#"fn apply_all(fs: [fn(int) -> int], x: int) -> int {
    var cur = x;
    for f in fs { cur = f(cur); }
    cur
}
fn pick(o: Option<fn(int) -> int>, x: int) -> int {
    match (o) { Option.Some(f) => f(x), Option.None => x, }
}
fn pair(p: (fn(int) -> int, int)) -> int {
    let f = p.0;
    f(p.1)
}
fn make(d: int) -> fn(int) -> int {
    fn(x: int) -> int { x + d }
}
fn main() {
    var count = 0;
    let counted = fn(x: int) -> int {
        count = count + 1;
        x + 1
    };
    let plain = fn(x: int) -> int { x * 2 };
    print(apply_all([counted, counted], 1));
    print(apply_all([plain, counted], 1));
    print(pick(Option.Some(counted), 4));
    print(pair((counted, 5)));
    let nested = [[plain], [counted, plain]];
    print(apply_all(nested[1], 2));
    let chosen = if (count > 100) { plain } else { counted };
    print(chosen(1));
    let viamatch = match (count) { 0 => plain, _ => counted, };
    print(viamatch(1));
    print(count);
    var m: Map<string, fn(int) -> int> = Map.new();
    m.insert("a", make(1));
    print(get(m, "a").unwrap()(7));
    print(make(3)(4));
    let fs = [make(1), make(10)];
    print(fs[0](fs[1](1)));
    print((if (fs.len() > 1) { fs[0] } else { fs[1] })(9));
    print((fn(x: int) -> int { x * x })(6));
    let curry = fn(a: int) -> fn(int) -> int { fn(b: int) -> int { a * b } };
    print(curry(3)(4));
}
"#,
        true,
        "3\n3\n5\n6\n6\n2\n2\n8\n8\n7\n12\n10\n36\n12\n",
    );
}

/// Un `[]` que toma su tipo del contexto en dos sitios que el nativo no cubría: como argumento de
/// una función GENÉRICA (la función entera quedaba como stub) y a la derecha de un `+` (E0283).
#[test]
fn empty_array_literals_typed_by_a_generic_parameter_or_the_other_operand() {
    same_everywhere(
        "empties",
        r#"fn glen<P>(xs: [P], d: P) -> int { xs.len() }
fn first_or<P>(xs: [P], d: P) -> P { if (xs.len() > 0) { xs[0] } else { d } }
fn main() {
    print(glen([], 2.5));
    print(first_or([], "d"));
    print(first_or([], (1, "t")).1);
    var grow: [string] = ["a"];
    grow = grow + [];
    grow = [] + grow;
    grow.push("b");
    print(grow);
}
"#,
        true,
        "0\nd\nt\n[a, b]\n",
    );
}

/// Una función genérica cuyo parámetro de tipo solo aparece dentro de un `Channel<…>`: el canal
/// viaja borrado en la firma de Rust y rustc no podía inferir `P` (E0283).
#[test]
fn generic_functions_over_a_channel_of_a_generic_enum() {
    same_everywhere(
        "channels",
        r#"enum Msg<P> { Item(P), Done, }
fn drain<P>(ch: Channel<Msg<P>>) -> int {
    var n = 0;
    var run = true;
    while (run) {
        match (recv(ch)) {
            Option.Some(m) => match (m) {
                Msg.Item(_p) => { n = n + 1; },
                Msg.Done => { run = false; },
            },
            Option.None => { run = false; },
        }
    }
    n
}
fn count<P>(ch: Channel<P>) -> int {
    var n = 0;
    while (recv(ch).is_some()) { n = n + 1; }
    n
}
fn main() {
    let msgs: Channel<Msg<string>> = Channel.bounded(4);
    send(msgs, Msg.Item("a"));
    send(msgs, Msg.Done);
    print(drain(msgs));
    let w = spawn(fn() -> int { drain(msgs) });
    send(msgs, Msg.Item("b"));
    send(msgs, Msg.Item("c"));
    send(msgs, Msg.Done);
    print(join(w));
    let plain: Channel<(int, string)> = Channel.bounded(2);
    send(plain, (1, "x"));
    close(plain);
    print(count(plain));
}
"#,
        false,
        "1\n2\n1\n",
    );
}
