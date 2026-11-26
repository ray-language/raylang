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

/// De la revisión previa a publicar (nov 2026): el valor inicial de un `fold` rápido se evaluaba
/// con el arreglo ya prestado; una función global con nombre conserva el camino rápido de
/// `map`/`filter`/`fold`; y los argumentos de una llamada a una expresión llevan el tipo del
/// parámetro (un `[]` no se tipa solo).
#[test]
fn fold_initial_value_named_functions_and_typed_arguments_of_expression_calls() {
    same_everywhere(
        "review",
        r#"fn grow(xs: [int]) -> int {
    xs.push(10);
    xs.len()
}
fn double(x: int) -> int { x * 2 }
fn add(a: int, x: int) -> int { a + x }
fn is_big(x: int) -> bool { x > 2 }
fn total(xs: [int]) -> int { xs.len() }
fn pick() -> fn([int], int) -> int {
    fn(xs: [int], k: int) -> int { xs.len() + k }
}
fn main() {
    var xs = [1, 2, 3];
    print(xs.fold(grow(xs), fn(a: int, x: int) -> int { a + x }));
    print(xs.map(double));
    print(xs.filter(is_big));
    print(xs.fold(0, add));
    let fs = [total];
    print(fs[0]([]));
    print(pick()([], 4));
    print(pick()([1, 2], 4));
}
"#,
        true,
        "20\n[2, 4, 6, 20]\n[3, 10]\n16\n0\n4\n6\n",
    );
}

/// rayhotel #489: los patrones de struct (`Invoice { id, status: Status.Paid(n) }`) no compilaban
/// en nativo («struct destructuring pattern is not supported») aunque la VM los ejecuta: en el
/// escrutinio, anidados en un `Option`, con sub-patrones de enum, tupla y literal, con guarda y
/// sobre un struct genérico.
#[test]
fn struct_patterns_in_match() {
    same_everywhere(
        "struct_patterns",
        r#"enum Status { Open, Paid(int), Void(string), }
struct Line { qty: int, label: string, }
struct Invoice { id: int, status: Status, first: Option<Line>, tag: (int, string), }
struct Pair<T> { left: T, right: T, }

fn show(i: Invoice) -> string {
    match (i) {
        Invoice { id, status: Status.Paid(n), first: _, tag: _ } if n > 100 => "big ${id}:${n}",
        Invoice { id, status: Status.Paid(n), first: Option.Some(Line { qty, label }), tag: _ } => "paid ${id}:${n} ${qty}x${label}",
        Invoice { id: 7, status: _, first: _, tag: (k, "lucky") } => "seven ${k}",
        Invoice { id, status: Status.Void(why), first: Option.None, tag: (_, t) } => "void ${id} ${why} ${t}",
        Invoice { id, status, first, tag } => "other ${id} ${first.is_some()} ${tag.0}",
    }
}

fn opt(o: Option<Invoice>) -> int {
    match (o) {
        Option.Some(Invoice { id, status: Status.Open, first: _, tag: _ }) => id,
        Option.Some(Invoice { id, status: _, first: _, tag: _ }) => 0 - id,
        Option.None => 0,
    }
}

fn sum(p: Pair<int>) -> int {
    match (p) {
        Pair { left: 0, right } => right,
        Pair { left, right } => left + right,
    }
}

fn line(id: int, s: Status, first: Option<Line>, tag: (int, string)) -> Invoice {
    Invoice { id: id, status: s, first: first, tag: tag }
}

fn main() {
    print(show(line(1, Status.Paid(500), Option.None, (1, "a"))));
    print(show(line(2, Status.Paid(5), Option.Some(Line { qty: 3, label: "pen" }), (1, "a"))));
    print(show(line(7, Status.Open, Option.None, (9, "lucky"))));
    print(show(line(4, Status.Void("dup"), Option.None, (1, "t"))));
    print(show(line(5, Status.Open, Option.Some(Line { qty: 1, label: "x" }), (8, "z"))));
    print(show(line(6, Status.Paid(5), Option.None, (3, "q"))));
    print(opt(Option.Some(line(9, Status.Open, Option.None, (0, "")))));
    print(opt(Option.Some(line(9, Status.Paid(1), Option.None, (0, "")))));
    print(opt(Option.None));
    print(sum(Pair { left: 0, right: 5 }));
    print(sum(Pair { left: 2, right: 5 }));
}
"#,
        true,
        "big 1:500\npaid 2:5 3xpen\nseven 9\nvoid 4 dup t\nother 5 true 8\nother 6 false 3\n9\n-9\n0\n5\n7\n",
    );
}

/// Revisión previa a 1.27.113: `map`/`filter`/`fold` evalúan el receptor ANTES que sus otros
/// argumentos, como la VM. El arreglo del valor inicial de `fold` lo había dejado delante.
#[test]
fn higher_order_functions_evaluate_the_receiver_first() {
    same_everywhere(
        "hof_order",
        r#"fn nums() -> [int] {
    print("receiver");
    [1, 2, 3]
}
fn start() -> int {
    print("init");
    10
}
fn add(a: int, x: int) -> int { a + x }
fn double(x: int) -> int { x * 2 }
fn main() {
    print(nums().fold(start(), add));
    print(nums().fold(start(), fn(a: int, x: int) -> int { a + x }));
    print(nums().map(double));
}
"#,
        true,
        "receiver\ninit\n16\nreceiver\ninit\n16\nreceiver\n[2, 4, 6]\n",
    );
}

/// rayhotel #496: dos `let _ = spawn(…)` en un bloque. La primera dejaba en ámbito una Task
/// llamada `_`, y la segunda la pre-clonaba como captura (`let _ = _.clone();`): Rust no compila.
#[test]
fn two_discarded_spawns_in_one_block() {
    same_everywhere(
        "discarded_spawns",
        r#"fn f() -> int { 1 }
fn main() {
    let _ = spawn(fn() -> int { f() });
    let _ = spawn(fn() -> int { f() });
    let r = scope(fn() -> int {
        let _ = spawn(fn() -> int { f() });
        let _ = spawn(fn() -> int { f() });
        2
    });
    print(r);
}
"#,
        false,
        "2\n",
    );
}
