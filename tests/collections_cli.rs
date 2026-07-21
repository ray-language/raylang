//! M50.2 — `std/collections/{set,deque,stringbuilder}` importables. Las colecciones (puras en raylang,
//! sin primitivos `__x`) salieron del prelude a submódulos opt-in con leaf-binding (`import
//! std/collections/set;` → `set.new()`). Como el uso pasa por el **loader** (resuelve el import), el
//! oráculo VM↔intérprete de `vm.rs` (que es pre-loader) ya no aplica: aquí se corre cada ejemplo por
//! **ambos motores** en subproceso y se exige que coincidan y den la salida esperada.

use std::process::Command;

/// Corre un archivo `.ray` por el motor elegido; devuelve (stdout, código de salida).
fn run_file(path: &str, vm: bool) -> (String, i32) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.arg(if vm { "--vm" } else { "--interp" });
    let out = cmd.arg(path).output().expect("ejecuta raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

/// Corre `src` (fuente inline en un temporal) por la VM; devuelve el código de salida.
fn run_src(name: &str, src: &str) -> i32 {
    let path = std::env::temp_dir().join(format!("{name}.ray"));
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&path)
        .output()
        .expect("ejecuta raylang");
    out.status.code().unwrap_or(-1)
}

/// Oráculo por subproceso: los dos motores coinciden (stdout + código) sobre el ejemplo, y la salida
/// es la esperada.
fn ambos_engines_matches(path: &str, expected: &str) {
    let (o_in, c_in) = run_file(path, false);
    let (o_vm, c_vm) = run_file(path, true);
    assert_eq!(c_in, 0, "intérprete sale 0 ({path})\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0 ({path})\n{o_vm}");
    assert_eq!(o_in, o_vm, "ambos engines match ({path})");
    assert_eq!(o_in, expected, "output expected_val ({path})");
}

/// M61.1 — el hash NO desborda: el `int` es checked (trap) y el polinomio h*31+c crecía sin
/// cota → `.hash()` de un string ≥ ~12 chars (o un derive con un campo de valor grande)
/// reventaba con "desbordamiento aritmético", matando `Set<string>` con claves reales.
#[test]
fn hash_without_overflow_both_engines() {
    let src = "import std/collections/set;\n\
        @derive(Hash, Eq)\n\
        struct Q { a: int, b: int, name: string }\n\
        fn main() {\n\
        \x20 let largo = \"abcdefghijklmnopqrstuvwxyz\".repeat(40);\n\
        \x20 print(to_string(largo.hash()));\n\
        \x20 let s: set.Set<string> = set.new();\n\
        \x20 set.add(s, \"one clave de length completamente normal\");\n\
        \x20 set.add(s, largo);\n\
        \x20 print(to_string(set.has(s, \"one clave de length completamente normal\")));\n\
        \x20 print(to_string(set.has(s, largo)));\n\
        \x20 print(to_string(set.has(s, \"ausente\")));\n\
        \x20 let q = Q { a: 400000000000000000, b: 7, name: largo };\n\
        \x20 print(to_string(q.hash()));\n\
        \x20 print(to_string(q.hash() == Q { a: 400000000000000000, b: 7, name: largo }.hash()));\n\
        }\n";
    let path = std::env::temp_dir().join("m61_hash_overflow.ray");
    std::fs::write(&path, src).unwrap();
    let (o_in, c_in) = run_file(path.to_str().unwrap(), false);
    let (o_vm, c_vm) = run_file(path.to_str().unwrap(), true);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, o_vm, "ambos engines match");
    let lines: Vec<&str> = o_in.lines().collect();
    assert_eq!(lines[1], "true");
    assert_eq!(lines[2], "true");
    assert_eq!(lines[3], "false");
    assert_eq!(lines[5], "true", "hash determinista del struct derivado");
}

/// M61.2 — `sort` pasó de insertion sort O(n²) a merge sort bottom-up O(n log n). Debe seguir
/// siendo ESTABLE (claves iguales conservan su orden relativo) y cubrir los bordes.
#[test]
fn stable_merge_sort_both_engines() {
    let src = "import std/sort as su;\n\
        struct Par { k: int, tag: string }\n\
        impl Ord for Par {\n\
        \x20 fn less(self, other: Par) -> bool { self.k < other.k }\n\
        }\n\
        fn main() {\n\
        \x20 let v: [int] = [];\n\
        \x20 print(to_string(sort(v).len()));\n\
        \x20 print(sort([3, 1, 2, 3, 1]).map(fn(x: int) -> string { to_string(x) }).join(\",\"));\n\
        \x20 print(sort([5, 4, 3, 2, 1]).map(fn(x: int) -> string { to_string(x) }).join(\",\"));\n\
        \x20 print(sort([\"pera\", \"kiwi\", \"uva\"]).join(\",\"));\n\
        \x20 print(su.is_sorted(sort([9, 8, 1, 5, 5, 2])).show());\n\
        \x20 var ps: [Par] = [];\n\
        \x20 ps.push(Par { k: 2, tag: \"a\" });\n\
        \x20 ps.push(Par { k: 1, tag: \"b\" });\n\
        \x20 ps.push(Par { k: 2, tag: \"c\" });\n\
        \x20 ps.push(Par { k: 1, tag: \"d\" });\n\
        \x20 ps.push(Par { k: 2, tag: \"e\" });\n\
        \x20 let s = sort(ps);\n\
        \x20 var out = \"\";\n\
        \x20 var i = 0;\n\
        \x20 while (i < s.len()) {\n\
        \x20\x20  out = out + to_string(s[i].k) + s[i].tag;\n\
        \x20\x20  i = i + 1;\n\
        \x20 }\n\
        \x20 print(out);\n\
        }\n";
    let path = std::env::temp_dir().join("m61_sort_estable.ray");
    std::fs::write(&path, src).unwrap();
    let expected = "0\n1,1,2,3,3\n1,2,3,4,5\nkiwi,pera,uva\ntrue\n1b1d2a2c2e\n";
    let (o_in, c_in) = run_file(path.to_str().unwrap(), false);
    let (o_vm, c_vm) = run_file(path.to_str().unwrap(), true);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, o_vm, "ambos engines match");
    assert_eq!(o_in, expected, "sort correcto y ESTABLE");
}

/// Revisión de collections (post-M64): el Set CRECE — rehash al doble de buckets cuando la carga
/// media supera 2 elementos/bucket. Antes los 16 buckets eran FIJOS y un Set de n elementos
/// degeneraba a búsqueda lineal O(n/16) (medido: 20k adds 2235→38 ms, 59×). Corrección bajo
/// varios rehash: size/has/ausencia/remove intactos, por ambos motores.
#[test]
fn set_crece_con_rehash_ambos_engines() {
    let src = "import std/collections/set;\n\
        fn main() {\n\
        \x20 let s: set.Set<int> = set.new();\n\
        \x20 var i = 0;\n\
        \x20 while (i < 2000) { set.add(s, i * 7); i = i + 1; }\n\
        \x20 print(to_string(set.size(s)));\n\
        \x20 var ok = true;\n\
        \x20 i = 0;\n\
        \x20 while (i < 2000) { if (!set.has(s, i * 7)) { ok = false; } i = i + 1; }\n\
        \x20 print(to_string(ok));\n\
        \x20 print(to_string(set.has(s, 3)));\n\
        \x20 i = 0;\n\
        \x20 while (i < 2000) { set.remove(s, i * 7); i = i + 2; }\n\
        \x20 print(to_string(set.size(s)));\n\
        \x20 print(to_string(set.has(s, 0)) + \",\" + to_string(set.has(s, 7)));\n\
        }\n";
    let path = std::env::temp_dir().join("set_rehash.ray");
    std::fs::write(&path, src).unwrap();
    let expected = "2000\ntrue\nfalse\n1000\nfalse,true\n";
    let (o_in, c_in) = run_file(path.to_str().unwrap(), false);
    let (o_vm, c_vm) = run_file(path.to_str().unwrap(), true);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, o_vm, "ambos engines match");
    assert_eq!(o_in, expected, "el Set con rehash es correcto");
}

#[test]
fn set_both_engines() {
    ambos_engines_matches(
        "examples/stdlib/conjunto.ray",
        "7\ntrue\nfalse\nfalse\n6\n2\ntrue\n26\n",
    );
}

#[test]
fn stringbuilder_y_deque_ambos_engines() {
    ambos_engines_matches(
        "examples/stdlib/builder_deque.ray",
        "fila1\nfila2\nfila3\nfila4\nfila5\n\n15\n10\n20\nb\n3\n0\n",
    );
}

#[test]
fn global_names_no_longer_exist() {
    // M50.2: la forma global del prelude se retiró; usar los nombres sin importar el submódulo es error.
    assert_ne!(
        run_src("m50_set_bad", "fn main() -> int { let s: Set<int> = set_new(); set_size(s) }"),
        0,
        "set_new/Set global must fallar (ya no está en el prelude)"
    );
    assert_ne!(
        run_src("m50_deque_bad", "fn main() -> int { let d: Deque<int> = deque_new(); deque_len(d) }"),
        0,
        "deque_new/Deque global must fallar"
    );
    assert_ne!(
        run_src("m50_sb_bad", "fn main() -> int { let b = sb_new(); sb_count(b) }"),
        0,
        "sb_new global must fallar"
    );
}

#[test]
fn qualified_import_from_submodule() {
    // El leaf-binding liga el último segmento: `import std/collections/set;` → `set.new()`.
    let src = "import std/collections/set;\n\
               fn main() -> int {\n\
               \x20 let s: set.Set<int> = set.new();\n\
               \x20 set.add(s, 1); set.add(s, 1); set.add(s, 2);\n\
               \x20 set.size(s)\n\
               }";
    assert_eq!(run_src("m50_set_ok", src), 2, "set con leaf-binding dedup → size 2");
}

/// M82 — `std/collections/dict`: mapa hash GENÉRICO (claves de USUARIO vía Hash+Eq),
/// hermano del Set. Claves struct con @derive(Hash, Eq), reemplazo (last wins), remove
/// que devuelve el valor, crecimiento con rehash (200 claves) y keys/values alineados.
#[test]
fn dict_user_keys_both_engines() {
    let src = r#"import std/collections/dict;

@derive(Hash, Eq, Show)
struct Punto {
    x: int,
    y: int,
}

fn main() -> int {
    // Claves de USUARIO (struct derivado) — el diferido de M13.
    let d: dict.Dict<Punto, string> = dict.new();
    dict.insert(d, Punto { x: 1, y: 2 }, "a");
    dict.insert(d, Punto { x: 3, y: 4 }, "b");
    match (dict.get(d, Punto { x: 1, y: 2 })) {
        Option.Some(v) => print("get(1,2) = " + v),
        Option.None => print("get(1,2) = <none>"),
    }
    // Reemplazo (last wins) y size.
    dict.insert(d, Punto { x: 1, y: 2 }, "A");
    match (dict.get(d, Punto { x: 1, y: 2 })) {
        Option.Some(v) => print("after reemplazo = " + v),
        Option.None => print("<none>"),
    }
    print("size = " + to_string(dict.size(d)));
    // remove devuelve el valor.
    match (dict.remove(d, Punto { x: 3, y: 4 })) {
        Option.Some(v) => print("removed = " + v),
        Option.None => print("removed = <none>"),
    }
    print("has(3,4) = " + to_string(dict.has(d, Punto { x: 3, y: 4 })));
    print("size = " + to_string(dict.size(d)));
    // Crecimiento: 200 claves int (rehash x varias) + verificación total.
    let n: dict.Dict<int, int> = dict.new();
    var i = 0;
    while (i < 200) {
        dict.insert(n, i, i * i);
        i = i + 1;
    }
    var ok = true;
    i = 0;
    while (i < 200) {
        match (dict.get(n, i)) {
            Option.Some(v) => { if (v != i * i) { ok = false; } },
            Option.None => { ok = false; },
        }
        i = i + 1;
    }
    print("crecimiento 200 ok = " + to_string(ok) + ", size = " + to_string(dict.size(n)));
    print("keys+values casan = " + to_string(dict.keys(n).len() == dict.values(n).len()));
    0
}
"#;
    let path = std::env::temp_dir().join("m82_dict.ray");
    std::fs::write(&path, src).unwrap();
    let (o_in, c_in) = run_file(path.to_str().unwrap(), false);
    let (o_vm, c_vm) = run_file(path.to_str().unwrap(), true);
    assert_eq!(c_in, 0, "intérprete sale 0
{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0
{o_vm}");
    assert_eq!(o_in, o_vm, "ambos engines match");
    let expected = "get(1,2) = a
after reemplazo = A
size = 2
removed = b
has(3,4) = false
size = 1
crecimiento 200 ok = true, size = 200
keys+values casan = true
";
    assert_eq!(o_in, expected, "output expected_val");
}
