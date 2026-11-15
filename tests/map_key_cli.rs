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
        "type error at 2:1: 'MapKey' cannot be implemented by hand: it is granted to int/string/char/bool/bytes and to types with @derive(Eq, Hash) whose fields are Map keys",
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
    let head = "a Map key must be int/string/char/bool/bytes or a type with @derive(Eq, Hash) whose fields are Map keys";
    // Un `Eq`/`Hash` a mano no cuenta: el `Map` compara por estructura, no con ese código.
    rejected(
        "by_hand_impls",
        "struct P { a: int, }\nimpl Eq for P { fn eq(self, o: P) -> bool { self.a == o.a } }\nimpl Hash for P { fn hash(self) -> int { self.a } }\nfn main() -> int {\n    let m: Map<P, int> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 5:5: {head}, not P (add @derive(Eq, Hash) to 'P'; an impl written by hand does not count)"),
    );
    rejected(
        "nested_field",
        "@derive(Eq, Hash)\nstruct In { a: Option<int>, }\n@derive(Eq, Hash)\nstruct P { i: In, }\nfn main() -> int {\n    let m: Map<P, int> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 6:5: {head}, not P (field 'i' of 'P' has type In, which is not a Map key)"),
    );
    rejected(
        "variant",
        "@derive(Eq, Hash)\nenum E { A, B(Option<int>), }\nstruct W { m: Map<E, int>, }\nfn main() -> int { 0 }\n",
        &format!("type error at 3:15: {head}, not E (variant 'B' of 'E' has type Option<int>, which is not a Map key)"),
    );
    rejected(
        "tuple",
        "fn main() -> int {\n    let m: Map<(int, int), int> = Map.new();\n    m.len()\n}\n",
        &format!("type error at 2:5: {head}, not (int, int)"),
    );
}
