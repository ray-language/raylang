//! #356 (raysolve): las claves de `Map`. `Map<P, V>` con un parámetro de tipo exige el bound
//! `MapKey`, que se verifica en cada instanciación; lo cumplen los primitivos y los tipos de
//! usuario con `@derive(Eq, Hash)` cuyos campos son claves. Lo que compila da lo mismo en la VM,
//! en el intérprete y en el binario nativo, y lo que no es clave es un error de tipos.
use std::process::Command;

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn workdir(name: &str, src: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_map_key_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), src).unwrap();
    d
}

/// VM, intérprete y (si hay rustc) el binario nativo dan `want` sin nada en stderr.
fn same_everywhere(name: &str, src: &str, want: &str) {
    let d = workdir(name, src);
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(&d).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stderr), "", "{name} {args:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{name} {args:?}");
    }
    if has_rustc() {
        let bin = d.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        assert!(b.status.success(), "{name} build --native: {}", String::from_utf8_lossy(&b.stderr));
        let o = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{name} nativo");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// El programa no compila: el primer renglón de stderr es `want` y no se ejecuta nada.
fn rejected(name: &str, src: &str, want: &str) {
    let d = workdir(name, src);
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(err.lines().next().unwrap_or(""), want, "{name}: {err}");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "", "{name}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_type_parameter_bounded_by_map_key_is_a_key_in_the_three_engines() {
    same_everywhere(
        "generic",
        r#"struct Cache<K: MapKey> { hits: Map<K, int>, }
type Counts<K> = Map<K, int>;
fn count<P: MapKey>(xs: [P]) -> int {
    var seen: Map<P, int> = Map.new();
    for x in xs { seen.insert(x, 1); }
    seen.len()
}
fn group<T, K: MapKey>(xs: [T], key: fn(T) -> K) -> Counts<K> {
    var m: Counts<K> = Map.new();
    for x in xs {
        let k = key(x);
        m.insert(k, m.get(k).unwrap_or(0) + 1);
    }
    m
}
fn distinct<Q: MapKey>(xs: [Q]) -> int { group(xs, fn(q: Q) -> Q { q }).len() }
fn main() -> int {
    print(count([1, 2, 2]));
    print(count(["a", "b"]));
    print(count(['x', 'x']));
    print(count([b"k", b"k", b"j"]));
    let parity = group([1, 2, 3, 4, 5], fn(n: int) -> bool { n % 2 == 0 });
    print("${parity.get(true).unwrap_or(0)} ${parity.get(false).unwrap_or(0)}");
    print(distinct(["a", "a", "b"]));
    let c = Cache { hits: ["a": 1, "b": 2] };
    print(c.hits.keys());
    0
}
"#,
        "2\n2\n1\n2\n2 3\n2\n[a, b]\n",
    );
}

#[test]
fn a_key_that_is_not_a_map_key_is_a_type_error() {
    // Antes `Map<P, V>` pasaba el checker con cualquier `P`: la VM moría con un error interno
    // al instanciarlo con un struct y el nativo no compilaba ni con un int.
    rejected(
        "unbounded",
        "fn count<P>(xs: [P]) -> int {\n    var seen: Map<P, int> = Map.new();\n    seen.len()\n}\nfn main() -> int { 0 }\n",
        "type error at 2:5: the type parameter 'P' is used as a Map key but is not bounded by 'MapKey' (declare it as 'P: MapKey')",
    );
    rejected(
        "literal",
        "fn one<T>(x: T) -> int {\n    let m = [x: 1];\n    m.len()\n}\nfn main() -> int { 0 }\n",
        "type error at 2:13: the type parameter 'T' is used as a Map key but is not bounded by 'MapKey' (declare it as 'T: MapKey')",
    );
    rejected(
        "field",
        "struct Cache<K> { hits: Map<K, int>, }\nfn main() -> int { 0 }\n",
        "type error at 1:25: the type parameter 'K' is used as a Map key but is not bounded by 'MapKey' (declare it as 'K: MapKey')",
    );
    rejected(
        "alias",
        "type Counts<K> = Map<K, int>;\nfn size<T>(m: Counts<T>) -> int { m.len() }\nfn main() -> int { 0 }\n",
        "type error at 2:12: the type parameter 'T' is used as a Map key but is not bounded by 'MapKey' (declare it as 'T: MapKey')",
    );
    rejected(
        "instance",
        "struct Pos { x: int, }\nfn count<P: MapKey>(xs: [P]) -> int { xs.len() }\nfn main() -> int { count([Pos { x: 1 }]) }\n",
        "type error at 3:20: Pos does not implement 'MapKey' (required by the call)",
    );
    rejected(
        "forward",
        "fn count<P: MapKey>(xs: [P]) -> int { xs.len() }\nfn relay<T>(xs: [T]) -> int { count(xs) }\nfn main() -> int { 0 }\n",
        "type error at 2:31: the type parameter 'T' is not bounded by 'MapKey' (required by the call)",
    );
    rejected(
        "by_hand",
        "struct Pos { x: int, }\nimpl MapKey for Pos {}\nfn main() -> int { 0 }\n",
        "type error at 2:1: 'MapKey' cannot be implemented by hand: it is granted to int/string/char/bool/bytes, to tuples and Options of keys, and to types with @derive(Eq, Hash) whose fields are Map keys",
    );
}

#[test]
fn a_derived_struct_or_enum_is_a_key_in_the_three_engines() {
    // M461: la clave es una COPIA estructural (mutar el original o lo que devuelve `keys()` no
    // toca el mapa) y el orden es el de los campos, y el de declaración de las variantes.
    same_everywhere(
        "derived",
        r#"@derive(Eq, Hash, Show)
struct SemVer { major: int, minor: int, pre: string, }
@derive(Eq, Hash, Show)
struct PkgVer { name: string, version: SemVer, }
@derive(Eq, Hash, Show)
enum Shape { Dot, Circle(int), Rect(int, int), Tagged(SemVer), }
@derive(Eq, Hash, Show)
enum Dir { North, South, East, West, }
@derive(Eq, Hash, Show)
enum Tree { Leaf(int), Node(Tree, Tree), }
struct Grid { cells: Map<SemVer, Dir>, }
struct Index<K: MapKey> { hits: Map<K, int>, }
fn v(a: int, b: int) -> SemVer { SemVer { major: a, minor: b, pre: "" } }
fn count<P: MapKey>(xs: [P]) -> int {
    var seen: Map<P, int> = Map.new();
    for x in xs { seen.insert(x, seen.get(x).unwrap_or(0) + 1); }
    seen.len()
}
fn group<T, K: MapKey>(xs: [T], key: fn(T) -> K) -> Map<K, int> {
    var m: Map<K, int> = Map.new();
    for x in xs { m.add_to(key(x), 1); }
    m
}
fn first_key<K: MapKey>(m: Map<K, int>) -> K { m.keys()[0] }
fn main() -> int {
    var m: Map<PkgVer, int> = Map.new();
    let k = PkgVer { name: "net", version: v(1, 2) };
    m.insert(k, 1);
    m.insert(PkgVer { name: "net", version: v(1, 2) }, 2);
    m.insert(PkgVer { name: "db", version: v(0, 8) }, 3);
    m.insert(PkgVer { name: "net", version: v(1, 10) }, 4);
    print(m.len());
    print(m.get(PkgVer { name: "net", version: v(1, 2) }));
    print(m.contains_key(PkgVer { name: "net", version: v(9, 9) }));
    k.name = "changed";
    print(m.contains_key(PkgVer { name: "net", version: v(1, 2) }));
    for (key, val) in m { print("${key.name} ${key.version.minor} -> ${val}"); }
    let ks = m.keys();
    ks[0].name = "mutated";
    print(m.contains_key(PkgVer { name: "db", version: v(0, 8) }));
    print(m.remove(PkgVer { name: "db", version: v(0, 8) }));
    print(m.len());
    var s: Map<Shape, string> = [Shape.Rect(1, 2): "r", Shape.Dot: "d", Shape.Circle(3): "c", Shape.Tagged(v(1, 0)): "t", Shape.Circle(1): "c1"];
    print(s.keys());
    print(s.get_or(Shape.Rect(1, 2), "?"));
    var trees: Map<Tree, string> = Map.new();
    trees.insert(Tree.Node(Tree.Leaf(1), Tree.Leaf(2)), "a");
    trees.insert(Tree.Leaf(7), "b");
    trees.insert(Tree.Node(Tree.Leaf(1), Tree.Leaf(1)), "c");
    print(trees.keys());
    print(count([v(1, 0), v(1, 0), v(2, 0)]));
    print(count([Shape.Dot, Shape.Dot]));
    let by_dir = group([Dir.West, Dir.North, Dir.West], fn(d: Dir) -> Dir { d });
    print(by_dir.keys());
    print(first_key(by_dir));
    let by_ver = group([1, 2, 3, 4, 5, 6], fn(n: int) -> SemVer { v(n % 2, n % 3) });
    print(by_ver.len());
    let g = Grid { cells: [v(1, 1): Dir.East] };
    print(g.cells.get_or(v(1, 1), Dir.North));
    let ix = Index { hits: by_ver };
    print(ix.hits.values());
    0
}
"#,
        concat!(
            "3\n",
            "Option.Some(2)\n",
            "false\n",
            "true\n",
            "db 8 -> 3\n",
            "net 2 -> 2\n",
            "net 10 -> 4\n",
            "true\n",
            "Option.Some(3)\n",
            "2\n",
            "[Shape.Dot, Shape.Circle(1), Shape.Circle(3), Shape.Rect(1, 2), Shape.Tagged(SemVer { major: 1, minor: 0, pre:  })]\n",
            "r\n",
            "[Tree.Leaf(7), Tree.Node(Tree.Leaf(1), Tree.Leaf(1)), Tree.Node(Tree.Leaf(1), Tree.Leaf(2))]\n",
            "2\n",
            "1\n",
            "[Dir.North, Dir.West]\n",
            "Dir.North\n",
            "6\n",
            "Dir.East\n",
            "[1, 1, 1, 1, 1, 1]\n",
        ),
    );
}

#[test]
fn a_map_with_derived_keys_crosses_fibers() {
    // Lo que cruza un canal se copia: la clave compuesta viaja con el mapa (VM y nativo; el
    // intérprete no tiene concurrencia).
    let src = r#"@derive(Eq, Hash, Show)
struct Pos { x: int, y: int, }
fn worker(m: Map<Pos, int>, out: Channel<Map<Pos, int>>) {
    m.insert(Pos { x: 9, y: 9 }, 99);
    out.send(m);
}
fn main() -> int {
    let ch: Channel<Map<Pos, int>> = Channel.new();
    var shared: Map<Pos, int> = [Pos { x: 0, y: 0 }: 1];
    let t = spawn(fn() { worker(shared, ch); });
    let back = ch.recv().unwrap();
    t.join();
    print(back.keys());
    print(shared.len());
    print(back.remove(Pos { x: 9, y: 9 }));
    0
}
"#;
    let want = "[Pos { x: 0, y: 0 }, Pos { x: 9, y: 9 }]\n1\nOption.Some(99)\n";
    let d = workdir("fibers", src);
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stderr), "");
    assert_eq!(String::from_utf8_lossy(&o.stdout), want);
    if has_rustc() {
        let bin = d.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
        assert_eq!(String::from_utf8_lossy(&Command::new(&bin).output().unwrap().stdout), want);
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_user_type_that_is_not_a_key_says_why() {
    let head = "a Map key must be int/string/char/bool/bytes, a tuple or Option of keys, or a type with @derive(Eq, Hash) whose fields are Map keys";
    // Un `Eq`/`Hash` a mano no cuenta: el `Map` compara por estructura, no con ese código.
    rejected(
        "by_hand_impls",
        "struct P { a: int, }\nimpl Eq for P { fn eq(self, o: P) -> bool { self.a == o.a } }\nimpl Hash for P { fn hash(self) -> int { self.a } }\nfn main() -> int {\n    let m: Map<P, int> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 5:5: {head}, not P (add @derive(Eq, Hash) to 'P'; an impl written by hand does not count)"),
    );
    rejected(
        "nested_field",
        "@derive(Eq, Hash)\nstruct In { a: u8, }\n@derive(Eq, Hash)\nstruct P { i: In, }\nfn main() -> int {\n    let m: Map<P, int> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 6:5: {head}, not P (field 'i' of 'P' has type In, which is not a Map key)"),
    );
    rejected(
        "variant",
        "@derive(Eq, Hash)\nenum E { A, B(u8), }\nstruct W { m: Map<E, int>, }\nfn main() -> int { 0 }\n",
        &format!("type error at 3:15: {head}, not E (variant 'B' of 'E' has type u8, which is not a Map key)"),
    );
    rejected(
        "tuple",
        "fn main() -> int {\n    let m: Map<(int, float), int> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 2:5: {head}, not (int, float)"),
    );
    rejected(
        "array",
        "fn main() -> int {\n    let m: Map<[int], int> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 2:5: {head}, not [int]"),
    );
}

#[test]
fn the_marker_bound_is_checked_on_every_path() {
    // Revisión de 1.27.100: tres caminos por los que un tipo que no es clave llegaba al `Map`
    // (pánico interno en la VM, error de rustc en nativo).
    let head = "a Map key must be int/string/char/bool/bytes, a tuple or Option of keys, or a type with @derive(Eq, Hash) whose fields are Map keys";
    // Los `impl Eq`/`Hash` de `u8`/`u32`/`u64` del prelude van marcados como derivados.
    rejected(
        "unsigned",
        "fn main() -> int {\n    let m: Map<u8, string> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 2:5: {head}, not u8"),
    );
    // Un impl genérico con bound marcador, elegido a través de otro genérico o de un `dyn`.
    let tally = "trait Tally { fn tally(self) -> int; }\nstruct Wrap<T> { v: T, }\nimpl<T: MapKey> Tally for Wrap<T> { fn tally(self) -> int { 1 } }\n";
    rejected(
        "impl_via_generic",
        &format!("{tally}fn use_it<X: Tally>(x: X) -> int {{ x.tally() }}\nfn main() -> int {{ use_it(Wrap {{ v: 1.5 }}) }}\n"),
        "type error at 5:20: the impl for Wrap<float> requires that 'T' be 'MapKey', but float does not implement it",
    );
    rejected(
        "impl_via_dyn",
        &format!("{tally}fn main() -> int {{\n    let d: dyn Tally = Wrap {{ v: [1, 2] }};\n    d.tally()\n}}\n"),
        "type error at 5:24: the impl for Wrap<[int]> requires that 'T' be 'MapKey', but [int] does not implement it",
    );
    // La mención de un tipo genérico con parámetro `MapKey`.
    let idx = "struct Idx<K: MapKey> { m: Map<K, int>, }\n";
    rejected(
        "mention_param",
        &format!("{idx}fn size<T>(x: Idx<T>) -> int {{ x.m.len() }}\nfn main() -> int {{ 0 }}\n"),
        "type error at 2:12: the type parameter 'T' is used as a Map key (by 'Idx') but is not bounded by 'MapKey' (declare it as 'T: MapKey')",
    );
    rejected(
        "mention_field",
        &format!("{idx}struct Outer<T> {{ i: Idx<T>, }}\nfn main() -> int {{ 0 }}\n"),
        "type error at 2:22: the type parameter 'T' is used as a Map key (by 'Idx') but is not bounded by 'MapKey' (declare it as 'T: MapKey')",
    );
    rejected(
        "mention_concrete",
        &format!("{idx}fn size(x: Idx<float>) -> int {{ x.m.len() }}\nfn main() -> int {{ 0 }}\n"),
        "type error at 2:9: float does not implement 'MapKey' (required by 'Idx')",
    );
    // El consejo del derive solo donde sirve.
    rejected(
        "option",
        "fn main() -> int {\n    let m: Map<Option<float>, int> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 2:5: {head}, not Option<float> (an Option is a Map key when its content is)"),
    );
    rejected(
        "result",
        "fn main() -> int {\n    let m: Map<Result<int, string>, int> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 2:5: {head}, not Result<int, string>"),
    );
    rejected(
        "generic_type",
        "struct Pair<T> { v: T, }\nfn main() -> int {\n    let m: Map<Pair<int>, int> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 3:5: {head}, not Pair<int> (a generic type such as 'Pair' cannot be a Map key)"),
    );
}

#[test]
fn bytes_fields_and_marker_bounded_impls_work_in_the_three_engines() {
    same_everywhere(
        "bytes_field",
        r#"@derive(Eq, Hash)
struct P { id: int, tag: bytes, }
trait Tally { fn tally(self) -> int; }
struct Wrap<T> { v: T, }
impl<T: MapKey> Tally for Wrap<T> {
    fn tally(self) -> int {
        let m: Map<T, int> = Map.new();
        m.insert(self.v, 1);
        m.len()
    }
}
fn use_it<X: Tally>(x: X) -> int { x.tally() }
struct Idx<K: MapKey> { m: Map<K, int>, }
fn size<T: MapKey>(x: Idx<T>) -> int { x.m.len() }
fn main() -> int {
    var m: Map<P, int> = [P { id: 1, tag: b"a" }: 1, P { id: 1, tag: b"b" }: 2];
    print(m.keys().len());
    print(m.get(P { id: 1, tag: b"b" }));
    print(use_it(Wrap { v: "s" }));
    let d: dyn Tally = Wrap { v: P { id: 2, tag: b"" } };
    print(d.tally());
    print(size(Idx { m: ["a": 1] }));
    0
}
"#,
        "2\nOption.Some(2)\n1\n1\n1\n",
    );
}

#[test]
fn tuples_and_options_are_keys_in_the_three_engines() {
    // M462: una tupla y un `Option` de claves son clave, sueltos, anidados, como campo de un
    // tipo derivado y en código genérico. Orden: tuplas elemento a elemento; `None` antes que
    // `Some`.
    same_everywhere(
        "tuples_options",
        r#"@derive(Eq, Hash, Show)
struct Pos { x: int, y: int, }
@derive(Eq, Hash)
struct Cell { at: (int, int), owner: Option<string>, tag: Option<(int, Pos)>, }
fn count<P: MapKey>(xs: [P]) -> int {
    var seen: Map<P, int> = Map.new();
    for x in xs { seen.add_to(x, 1); }
    seen.len()
}
fn pairs<A: MapKey, B: MapKey>(xs: [A], ys: [B]) -> Map<(A, B), int> {
    var m: Map<(A, B), int> = Map.new();
    for x in xs { for y in ys { m.insert((x, y), 1); } }
    m
}
fn main() -> int {
    var grid: Map<(int, int), string> = Map.new();
    grid.insert((2, 1), "c");
    grid.insert((1, 9), "b");
    grid.insert((1, 2), "a");
    grid.insert((1, 2), "A");
    print(grid.len());
    print(grid.keys());
    print(grid.get((1, 2)));
    print(grid.contains_key((9, 9)));
    for (rc, v) in grid { print("${rc.0},${rc.1}=${v}"); }
    var mixed: Map<(string, Pos), int> = [("a", Pos { x: 1, y: 1 }): 1, ("a", Pos { x: 0, y: 5 }): 2];
    print(mixed.keys());
    var opt: Map<Option<int>, string> = [Option.Some(3): "three", Option.None: "none", Option.Some(1): "one"];
    print(opt.keys());
    print(opt.get(Option.None));
    print(opt.get_or(Option.Some(3), "?"));
    print(opt.remove(Option.None));
    var cells: Map<Cell, int> = Map.new();
    cells.insert(Cell { at: (1, 1), owner: Option.None, tag: Option.Some((7, Pos { x: 0, y: 0 })) }, 1);
    cells.insert(Cell { at: (1, 1), owner: Option.Some("x"), tag: Option.None }, 2);
    cells.insert(Cell { at: (0, 9), owner: Option.Some("x"), tag: Option.None }, 3);
    print(cells.get(Cell { at: (1, 1), owner: Option.Some("x"), tag: Option.None }));
    for (c, n) in cells { print("${c.at.0},${c.at.1} ${n}"); }
    print(count([(1, "a"), (1, "a"), (2, "a")]));
    let os: [Option<int>] = [Option.Some(1), Option.None, Option.None];
    print(count(os));
    let ps = pairs([1, 2], ["x", "y"]);
    print(ps.keys());
    let nested: Map<((int, int), bool), int> = [((1, 2), true): 1];
    print(nested.get(((1, 2), true)));
    0
}
"#,
        concat!(
            "3\n",
            "[(1, 2), (1, 9), (2, 1)]\n",
            "Option.Some(A)\n",
            "false\n",
            "1,2=A\n",
            "1,9=b\n",
            "2,1=c\n",
            "[(a, Pos { x: 0, y: 5 }), (a, Pos { x: 1, y: 1 })]\n",
            "[Option.None, Option.Some(1), Option.Some(3)]\n",
            "Option.Some(none)\n",
            "three\n",
            "Option.Some(none)\n",
            "Option.Some(2)\n",
            "0,9 3\n",
            "1,1 1\n",
            "1,1 2\n",
            "2\n",
            "2\n",
            "[(1, x), (1, y), (2, x), (2, y)]\n",
            "Option.Some(1)\n",
        ),
    );
}

#[test]
fn nested_map_literals_and_generic_key_structs_build_natively() {
    // Revisión de 1.27.101: el tipo esperado de un literal de `Map` se colaba en el literal
    // anidado; un `[Option.None: 1]` como retorno, argumento o payload no nombraba su clave; y un
    // struct genérico cuyo parámetro solo vive en una clave tupla/Option no lo usaba en Rust.
    // De paso, el checker comprobaba el campo `Map<(A, int), V>` contra `(A, int)` sin resolver.
    same_everywhere(
        "nested",
        r#"struct Grid<A: MapKey> { m: Map<(A, int), int>, o: Map<Option<A>, int> }
enum E { A(Map<Option<int>, int>) }
fn make() -> Map<Option<int>, int> { [Option.None: 1] }
fn size(m: Map<Option<int>, int>) -> int { m.len() }
fn main() -> int {
    let a: Map<string, Map<(int, int), int>> = ["x": [(1, 2): 3]];
    print(a.get("x").unwrap().get((1, 2)));
    let b: Map<(int, int), Map<string, int>> = [(1, 2): ["k": 3]];
    print(b.get((1, 2)).unwrap().get("k"));
    print(make().len());
    print(size([Option.None: 1]));
    let e = E.A([Option.None: 1]);
    match (e) { E.A(m) => print(m.len()) }
    let o: Option<Map<Option<int>, int>> = Option.Some([Option.None: 5]);
    print(o.unwrap().get(Option.None));
    let g = Grid { m: [("a", 1): 2], o: [Option.Some("z"): 1] };
    print(g.m.get(("a", 1)));
    print(g.o.keys());
    0
}
"#,
        "Option.Some(3)\nOption.Some(3)\n1\n1\n1\nOption.Some(5)\nOption.Some(2)\n[Option.Some(z)]\n",
    );
}
