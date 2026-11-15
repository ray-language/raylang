//! #356 (raysolve): las claves de `Map` en código genérico. `Map<P, V>` con un parámetro de tipo
//! exige el bound `MapKey`, que se verifica en cada instanciación: lo que compila da lo mismo en
//! la VM, en el intérprete y en el binario nativo, y lo que no es clave es un error de tipos.
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
        "type error at 2:1: 'MapKey' cannot be implemented by hand: it is granted to int/string/char/bool/bytes",
    );
}
