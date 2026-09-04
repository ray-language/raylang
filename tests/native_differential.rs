//! M120 — Harness DIFERENCIAL de motores: genera programas raylang bien tipados y exige que
//! intérprete, VM y binario nativo produzcan **exactamente** el mismo stdout y código de salida.
//!
//! Por qué existe: el dogfood de 14 apps (IDEAS §§63-72) destapó 4 bugs de la misma clase —
//! "código válido en la VM que no compila o diverge en nativo" (sort-float §63, close-con-lector
//! §64, RefCell-en-variante §64, return-en-spawn §68) — y todos aparecieron en interacciones de
//! features que el corpus de ejemplos (caminos felices idiomáticos) nunca ejercita. Este harness
//! ataca la clase, no los casos: una biblioteca de GENERADORES de sondas, uno por interacción
//! (builtin×tipo, mutación-en-constructor, return-en-closure, valores cruzando fibras…), con los
//! valores sembrados por un PRNG determinista.
//!
//! Diseño (las tres palancas de coste):
//! - **Empaquetado**: N sondas → UN programa (`fn probe_i()` + un `main` que las llama con
//!   separadores). Un solo build nativo (~1 s por la vía rustc-pelado) amortiza N sondas.
//! - **Errores como datos**: los caminos de error de ejecución (división por cero, índice fuera
//!   de rango, overflow) se sondean vía `try_call` — el mensaje se IMPRIME y entra a la
//!   comparación byte a byte (paridad de errores H6) sin abortar el batch.
//! - **Bisección automática**: ante una divergencia (o un build nativo roto), el harness parte el
//!   batch por mitades hasta aislar la(s) sonda(s) culpable(s), escribe el repro mínimo a un
//!   archivo y falla nombrando la semilla y la ruta.
//!
//! Reproducir: `RAYLANG_DIFF_SEED=<n>` re-ejecuta ese batch exacto. `RAYLANG_DIFF_BATCHES=<n>`
//! sube el presupuesto (la campaña nocturna corre muchos más que el push de CI).
//!
//! Los batches SECUENCIALES comparan los 3 motores (nativo por la vía rustc-pelado, `--without`
//! todo). Los batches de CONCURRENCIA (spawn/canales/scope, sincronizados por join → salida
//! determinista bajo cualquier scheduler) comparan VM ↔ nativo con fibras (el default del
//! producto); el intérprete no tiene concurrencia. Lo que este harness NO cubre a propósito: los
//! entrelazados de E/S real (la clase close-con-lector-aparcado se prueba con tests E2E dirigidos
//! en cli_cli.rs — un socket real no se genera desde una plantilla determinista).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

// ---------------------------------------------------------------------------------------------
// PRNG determinista (SplitMix64, como tests/fuzz_frontend.rs) — misma semilla, mismo batch.
// ---------------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Entero en `[0, n)` (n > 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Entero en `[lo, hi]`.
    fn int_in(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next_u64() % ((hi - lo + 1) as u64)) as i64
    }

    /// Una palabra corta (para strings de sonda; incluye no-ASCII a propósito).
    fn word(&mut self) -> &'static str {
        const WORDS: &[&str] = &["sol", "ñu", "kai", "早い", "zeta", "mar", "🌊ola", "ácido", "rex"];
        WORDS[self.below(WORDS.len())]
    }
}

// ---------------------------------------------------------------------------------------------
// Sondas y generadores
// ---------------------------------------------------------------------------------------------

/// Una sonda: declaraciones top-level (tipos/funciones auxiliares con nombres manglados por
/// índice) + la función `probe_{i}` que imprime resultados deterministas.
struct Probe {
    /// Módulos `import` que la sonda necesita (el batch los dedup-a en cabecera).
    imports: Vec<&'static str>,
    /// Código top-level completo, incluida `fn probe_{i}()`.
    top: String,
    /// Índice (para el separador y la etiqueta de bisección).
    idx: usize,
    /// Nombre del generador (diagnóstico).
    gen_name: &'static str,
}

type GenFn = fn(&mut Rng, usize) -> (Vec<&'static str>, String);

/// Generadores SECUENCIALES: cada uno cubre una clase de interacción (ver el doc de arriba).
const SEQ_GENS: &[(&str, GenFn)] = &[
    ("sort_floats", gen_sort_floats),
    ("sort_strings_bools", gen_sort_strings_bools),
    ("iterator_chain", gen_iterator_chain),
    ("string_unicode", gen_string_unicode),
    ("string_methods", gen_string_methods),
    ("runtime_errors_as_values", gen_runtime_errors),
    ("unsigned_wrapping", gen_unsigned_wrapping),
    ("float_int_casts", gen_float_int_casts),
    ("mutation_in_constructor_args", gen_mutation_in_ctor),
    ("closure_captures_var", gen_closure_captures),
    ("closure_in_struct_field", gen_closure_field),
    ("return_inside_closures", gen_return_in_closures),
    ("generic_multi_instantiation", gen_generics),
    ("trait_dispatch_dyn", gen_trait_dyn),
    ("enums_nested_match", gen_enums_nested),
    ("option_result_chains", gen_option_result),
    ("map_key_types", gen_map_keys),
    ("bytes_ops", gen_bytes_ops),
    ("tuples", gen_tuples),
    ("for_ranges", gen_for_ranges),
    ("concat_interpolation", gen_concat_interp),
    ("recursion", gen_recursion),
];

/// Generadores de CONCURRENCIA (deterministas por sincronización de join/reply).
const CONC_GENS: &[(&str, GenFn)] = &[
    ("spawn_join_each_type", gen_spawn_join_types),
    ("channel_lockstep", gen_channel_lockstep),
    ("channel_in_variant_actor", gen_actor),
    ("return_in_spawn", gen_return_in_spawn),
    ("scope_children_try_join", gen_scope_children),
    ("try_recv_select_timeout", gen_try_recv_select),
];

fn gen_sort_floats(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let (a, b, c) = (r.int_in(1, 9), r.int_in(10, 99), r.int_in(1, 9));
    let top = format!(
        "fn probe_{i}() {{\n    let xs = [{a}.5, {b}.25, -{c}.75, 0.0, {a}.5];\n    let ys = sort(xs);\n    print(ys);\n    print(ys.contains({a}.5));\n    print(ys.position(0.0));\n    print(sort([{b}, {a}, -{c}, 0, {a}]));\n}}\n"
    );
    (vec![], top)
}

fn gen_sort_strings_bools(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let (w1, w2, w3) = (r.word(), r.word(), r.word());
    let top = format!(
        "fn probe_{i}() {{\n    let ss = sort([\"{w1}\", \"{w2}\", \"{w3}\", \"\"]);\n    print(ss.join(\"|\"));\n    print(sort([true, false, true]));\n    var zs = [\"a\", \"{w1}\", \"z\"];\n    zs.reverse();\n    print(zs);\n    print(zs.pop());\n    print(zs);\n}}\n"
    );
    (vec![], top)
}

fn gen_iterator_chain(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let (n, k) = (r.int_in(5, 12), r.int_in(2, 4));
    let top = format!(
        "fn probe_{i}() {{\n    var xs: [int] = [];\n    for v in 0..{n} {{ xs.push(v); }}\n    let out = xs.map(fn(x: int) -> int {{ x * {k} }}).filter(fn(x: int) -> bool {{ x % 2 == 0 }}).fold(0, fn(acc: int, x: int) -> int {{ acc + x }});\n    print(out);\n    print(xs.map(fn(x: int) -> string {{ \"n${{x}}\" }}).join(\",\"));\n}}\n"
    );
    (vec![], top)
}

fn gen_string_unicode(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let w = r.word();
    let top = format!(
        "fn probe_{i}() {{\n    let s = \"a{w}\\u{{1F680}}é早\";\n    print(s.len());\n    print(s.to_bytes().len());\n    print(s[1]);\n    var codes = 0;\n    for c in s.chars() {{ codes = codes + char_code(c); }}\n    print(codes);\n    print(s.substring(1, 3));\n    print(s.to_upper());\n    print(\"v=${{s}}|${{s.len()}}\");\n}}\n"
    );
    (vec![], top)
}

fn gen_string_methods(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let (w1, w2) = (r.word(), r.word());
    let top = format!(
        "fn probe_{i}() {{\n    let s = \"{w1},{w2},{w1}\";\n    print(s.split(\",\").len());\n    print(s.replace(\"{w1}\", \"X\"));\n    print(s.starts_with(\"{w1}\"));\n    print(s.ends_with(\"{w1}\"));\n    print(s.index_of(\"{w2}\"));\n    print(\"  pad  \".trim());\n    print(\"ab\".repeat(3));\n    print(s.contains(\"{w2}\"));\n}}\n"
    );
    (vec![], top)
}

fn gen_runtime_errors(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    // La paridad de MENSAJES de error de ejecución (H6) como datos, vía try_call. EXCEPCIÓN
    // documentada (transpile/runtime.rs, cabecera de __ray_in_try): el TEXTO de índice-fuera-de-
    // rango difiere a propósito (el indexado nativo no paga bounds-check propio: panica el de
    // Rust) — el FLUJO (ambos Err) sí es idéntico, así que esa sonda clasifica sin el mensaje.
    let n = r.int_in(2, 6);
    let top = format!(
        "fn probe_{i}() {{\n    match (try_call(fn() -> int {{ let xs = [1, 2, {n}]; xs[9] }})) {{\n        Result.Ok(v) => print(v),\n        Result.Err(_e) => print(\"err:oob\"),\n    }}\n    match (try_call(fn() -> int {{ {n} / ({n} - {n}) }})) {{\n        Result.Ok(v) => print(v),\n        Result.Err(e) => print(\"err:${{e}}\"),\n    }}\n    match (try_call(fn() -> int {{ 9223372036854775807 + {n} }})) {{\n        Result.Ok(v) => print(v),\n        Result.Err(e) => print(\"err:${{e}}\"),\n    }}\n    match (try_call(fn() -> string {{ panic(\"boom{i}\"); \"no\" }})) {{\n        Result.Ok(v) => print(v),\n        Result.Err(e) => print(\"err:${{e}}\"),\n    }}\n}}\n"
    );
    (vec![], top)
}

fn gen_unsigned_wrapping(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let k = r.int_in(3, 60);
    let top = format!(
        "fn probe_{i}() {{\n    let a: u8 = 250;\n    print(a + {k});\n    let b: u32 = 4294967290;\n    print(b + {k});\n    let c: u64 = 1;\n    print(c << 63);\n    print((5 & 3) == 1);\n    print(5 | 3);\n    print(5 ^ 3);\n    print(1 << {sh});\n    print(1024 >> 3);\n}}\n",
        sh = r.int_in(1, 30)
    );
    (vec![], top)
}

fn gen_float_int_casts(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let (a, b) = (r.int_in(1, 99), r.int_in(1, 9));
    let top = format!(
        "fn probe_{i}() {{\n    print({a}.75 as int);\n    print(-{a}.75 as int);\n    print({b} as float);\n    print(math.floor({a}.6));\n    print(math.ceil({a}.2));\n    print(math.round(-{b}.5));\n    print(math.sqrt(2.0));\n    print(0.1 + 0.2);\n    print({a}.0 / {b}.0);\n}}\n"
    );
    (vec!["std/math"], top)
}

fn gen_mutation_in_ctor(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    // La clase RefCell-en-constructor (§64): args de literales compuestos que MUTAN el receptor.
    let k = r.int_in(1, 9);
    let top = format!(
        "struct BoxP{i} {{ n: int, }}\n\
         enum PairP{i} {{ Two(int, int, int), }}\n\
         struct WrapP{i} {{ a: int, b: int, }}\n\
         fn bump_{i}(b: BoxP{i}) -> int {{\n    b.n = b.n + {k};\n    b.n\n}}\n\
         fn probe_{i}() {{\n    var b = BoxP{i} {{ n: 1 }};\n    let p = PairP{i}.Two(b.n, bump_{i}(b), b.n);\n    match (p) {{ PairP{i}.Two(x, y, z) => print(\"${{x}},${{y}},${{z}}\"), }}\n    var c = BoxP{i} {{ n: 10 }};\n    let w = WrapP{i} {{ a: c.n, b: bump_{i}(c) }};\n    print(\"${{w.a}}/${{w.b}}\");\n    var d = BoxP{i} {{ n: 100 }};\n    let arr = [d.n, bump_{i}(d), d.n];\n    print(arr);\n    var e = BoxP{i} {{ n: 7 }};\n    let t = (e.n, bump_{i}(e), e.n);\n    print(\"${{t.0}}:${{t.1}}:${{t.2}}\");\n}}\n"
    );
    (vec![], top)
}

fn gen_closure_captures(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let n = r.int_in(3, 7);
    let top = format!(
        "fn probe_{i}() {{\n    var total = 0;\n    let add = fn(x: int) {{ total = total + x; }};\n    for v in 1..{n} {{ add(v); }}\n    print(total);\n    var hits = 0;\n    let outer = fn() {{\n        let inner = fn() {{ hits = hits + 1; }};\n        inner();\n        inner();\n    }};\n    outer();\n    print(hits);\n}}\n"
    );
    (vec![], top)
}

fn gen_closure_field(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let k = r.int_in(2, 9);
    let top = format!(
        "struct OpsP{i} {{ scale: fn(int) -> int, label: string, }}\n\
         fn make_ops_{i}() -> OpsP{i} {{\n    OpsP{i} {{ scale: fn(x: int) -> int {{ x * {k} }}, label: \"x{k}\" }}\n}}\n\
         fn probe_{i}() {{\n    let ops = make_ops_{i}();\n    let f = ops.scale;\n    print(f(6));\n    print(ops.label);\n    let g = fn(h: fn(int) -> int, v: int) -> int {{ h(v) + 1 }};\n    print(g(f, 10));\n}}\n"
    );
    (vec![], top)
}

fn gen_return_in_closures(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    // La clase return-en-cuerpo-literal (§68), en secuencial: return temprano dentro de
    // match dentro de while dentro de una función anónima.
    let stop = r.int_in(3, 6);
    let top = format!(
        "fn probe_{i}() {{\n    let scan = fn(xs: [int]) -> int {{\n        var j = 0;\n        while (j < xs.len()) {{\n            if (xs[j] == {stop}) {{ return j; }}\n            j = j + 1;\n        }}\n        return -1;\n    }};\n    print(scan([1, {stop}, 9]));\n    print(scan([7, 8]));\n    let pick = fn(o: Option<int>) -> int {{\n        if let Option.Some(v) = o {{ return v * 2; }}\n        0\n    }};\n    print(pick(Option.Some({stop})));\n    print(pick(Option.None));\n}}\n"
    );
    (vec![], top)
}

fn gen_generics(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let w = r.word();
    let top = format!(
        "fn dup_{i}<T>(x: T) -> (T, T) {{\n    (x, x)\n}}\n\
         fn largest_{i}<T: Ord>(a: T, b: T) -> T {{\n    if (a.less(b)) {{ b }} else {{ a }}\n}}\n\
         fn probe_{i}() {{\n    let a = dup_{i}(4);\n    print(a.0 + a.1);\n    let s = dup_{i}(\"{w}\");\n    print(s.0 + s.1);\n    print(largest_{i}(3, 9));\n    print(largest_{i}(2.5, 1.5));\n    print(largest_{i}(\"a\", \"{w}\"));\n}}\n"
    );
    (vec![], top)
}

fn gen_trait_dyn(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let (a, b) = (r.int_in(2, 6), r.int_in(2, 6));
    let top = format!(
        "trait ShapeP{i} {{ fn area(self) -> int; fn tag(self) -> string; }}\n\
         struct SqP{i} {{ side: int, }}\n\
         struct RcP{i} {{ w: int, h: int, }}\n\
         impl ShapeP{i} for SqP{i} {{\n    fn area(self) -> int {{ self.side * self.side }}\n    fn tag(self) -> string {{ \"sq\" }}\n}}\n\
         impl ShapeP{i} for RcP{i} {{\n    fn area(self) -> int {{ self.w * self.h }}\n    fn tag(self) -> string {{ \"rc\" }}\n}}\n\
         fn probe_{i}() {{\n    let shapes: [dyn ShapeP{i}] = [SqP{i} {{ side: {a} }}, RcP{i} {{ w: {a}, h: {b} }}];\n    var total = 0;\n    for s in shapes {{\n        total = total + s.area();\n        print(s.tag());\n    }}\n    print(total);\n}}\n"
    );
    (vec![], top)
}

fn gen_enums_nested(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let k = r.int_in(1, 9);
    let top = format!(
        "enum InnerP{i} {{ Leaf(int), Nil, }}\n\
         enum OuterP{i} {{ Wrap(InnerP{i}), Tag(string), }}\n\
         fn describe_{i}(o: OuterP{i}) -> string {{\n    match (o) {{\n        OuterP{i}.Wrap(inner) => match (inner) {{\n            InnerP{i}.Leaf(v) => \"leaf:${{v}}\",\n            InnerP{i}.Nil => \"nil\",\n        }},\n        OuterP{i}.Tag(s) => \"tag:${{s}}\",\n    }}\n}}\n\
         fn probe_{i}() {{\n    print(describe_{i}(OuterP{i}.Wrap(InnerP{i}.Leaf({k}))));\n    print(describe_{i}(OuterP{i}.Wrap(InnerP{i}.Nil)));\n    print(describe_{i}(OuterP{i}.Tag(\"t{k}\")));\n    let all = [OuterP{i}.Tag(\"a\"), OuterP{i}.Wrap(InnerP{i}.Leaf(2))];\n    print(all.len());\n}}\n"
    );
    (vec![], top)
}

fn gen_option_result(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let n = r.int_in(10, 99);
    let top = format!(
        "fn as_res_{i}(o: Option<int>, msg: string) -> Result<int, string> {{\n    match (o) {{ Option.Some(v) => Result.Ok(v), Option.None => Result.Err(msg), }}\n}}\n\
         fn parse_pair_{i}(a: string, b: string) -> Result<int, string> {{\n    let x = as_res_{i}(parse_int(a), \"bad:${{a}}\")?;\n    let y = as_res_{i}(parse_int(b), \"bad:${{b}}\")?;\n    Result.Ok(x + y)\n}}\n\
         fn probe_{i}() {{\n    match (parse_pair_{i}(\"{n}\", \"1\")) {{ Result.Ok(v) => print(v), Result.Err(e) => print(e), }}\n    match (parse_pair_{i}(\"{n}\", \"zz\")) {{ Result.Ok(v) => print(v), Result.Err(e) => print(e), }}\n    print(parse_int(\"-{n}\"));\n    print(parse_float(\"{n}.5\"));\n}}\n"
    );
    (vec![], top)
}

fn gen_map_keys(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let (w, k) = (r.word(), r.int_in(1, 9));
    let top = format!(
        "fn probe_{i}() {{\n    var ms: Map<string, int> = Map.new();\n    ms.insert(\"{w}\", {k});\n    ms.insert(\"a\", 1);\n    ms.insert(\"{w}\", {k} + 1);\n    print(get(ms, \"{w}\"));\n    print(ms.keys());\n    print(ms.contains_key(\"nope\"));\n    ms.remove(\"a\");\n    print(ms.keys().len());\n    var mi: Map<int, string> = Map.new();\n    mi.insert({k}, \"v\");\n    mi.insert(-1, \"neg\");\n    print(mi.keys());\n    for key in ms.keys() {{ print(\"k=${{key}}\"); }}\n}}\n"
    );
    (vec![], top)
}

fn gen_bytes_ops(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let k = r.int_in(0, 255);
    let top = format!(
        "fn probe_{i}() {{\n    let b = b\"ok\\x00\\xff\" + bytes_of([{k}, 65]);\n    print(b.len());\n    print(b[0]);\n    print(b[b.len() - 1]);\n    print(b.sub_bytes(1, 3));\n    match (from_utf8(b\"hola\")) {{ Result.Ok(s) => print(s), Result.Err(e) => print(e), }}\n    match (from_utf8(b\"\\xff\\xfe\")) {{ Result.Ok(s) => print(s), Result.Err(e) => print(\"inv\"), }}\n    print(\"ñu\".to_bytes());\n}}\n"
    );
    (vec![], top)
}

fn gen_tuples(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let (a, b) = (r.int_in(1, 9), r.word());
    let top = format!(
        "fn split_{i}(n: int) -> (int, int, string) {{\n    (n / 2, n % 2, \"r{b}\")\n}}\n\
         fn probe_{i}() {{\n    let t = split_{i}({a}1);\n    print(t.0);\n    print(t.1);\n    print(t.2);\n    let (q, r, s) = split_{i}(9);\n    print(\"${{q}}-${{r}}-${{s}}\");\n    let pair = ((1, 2), ({a}, \"{b}\"));\n    let second = pair.1;\n    print(second.1);\n    let first = pair.0;\n    print(first.0 + first.1);\n}}\n"
    );
    (vec![], top)
}

fn gen_for_ranges(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let n = r.int_in(3, 6);
    let top = format!(
        "fn probe_{i}() {{\n    var acc = 0;\n    for x in 0..{n} {{\n        for y in 0..x {{ acc = acc + y; }}\n    }}\n    print(acc);\n    for _e in 5..5 {{ acc = acc + 100; }}\n    print(acc);\n    let xs = [\"{w}\", \"b\"];\n    var joined = \"\";\n    for s in xs {{ joined = joined + s; }}\n    print(joined);\n    for c in \"ab早\".chars() {{ print(char_code(c)); }}\n}}\n",
        w = r.word()
    );
    (vec![], top)
}

fn gen_concat_interp(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let (a, w) = (r.int_in(1, 99), r.word());
    let top = format!(
        "fn probe_{i}() {{\n    let n = {a};\n    let f = {a}.5;\n    let s = \"{w}\";\n    print(\"n=${{n}} f=${{f}} s=${{s}} b=${{n > 10}}\");\n    print(\"a\" + \"b\" + to_string(n) + s + to_string(f));\n    print([n, n + 1]);\n    print(`tpl \"${{s}}\" ok`);\n}}\n"
    );
    (vec![], top)
}

fn gen_recursion(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let n = r.int_in(8, 14);
    let top = format!(
        "fn fib_{i}(n: int) -> int {{\n    if (n < 2) {{ n }} else {{ fib_{i}(n - 1) + fib_{i}(n - 2) }}\n}}\n\
         fn probe_{i}() {{\n    print(fib_{i}({n}));\n    print(fib_{i}(0));\n}}\n"
    );
    (vec![], top)
}

// ---- Concurrencia (VM ↔ nativo con fibras; determinista por joins/replies) ----

fn gen_spawn_join_types(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    // Cada TIPO cruzando la frontera de fibras (la clase send-repr del nativo).
    let k = r.int_in(1, 9);
    let top = format!(
        "struct PtP{i} {{ x: int, y: int, }}\n\
         enum MsgP{i} {{ N(int), S(string), }}\n\
         fn probe_{i}() {{\n    let t1 = spawn(fn() -> [int] {{ [{k}, {k} + 1] }});\n    print(join(t1));\n    let t2 = spawn(fn() -> string {{ \"s${{{k} * 2}}\" }});\n    print(join(t2));\n    let t3 = spawn(fn() -> PtP{i} {{ PtP{i} {{ x: {k}, y: 4 }} }});\n    let p = join(t3);\n    print(p.x + p.y);\n    let t4 = spawn(fn() -> MsgP{i} {{ MsgP{i}.S(\"m{k}\") }});\n    match (join(t4)) {{ MsgP{i}.N(v) => print(v), MsgP{i}.S(s) => print(s), }}\n    let t5 = spawn(fn() -> Map<string, int> {{\n        var m: Map<string, int> = Map.new();\n        m.insert(\"k\", {k});\n        m\n    }});\n    print(get(join(t5), \"k\"));\n    let t6 = spawn(fn() -> bytes {{ b\"\\x01\\x02\" }});\n    print(join(t6));\n    let t7 = spawn(fn() -> float {{ {k}.5 }});\n    print(join(t7));\n}}\n"
    );
    (vec![], top)
}

fn gen_channel_lockstep(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let k = r.int_in(1, 9);
    let top = format!(
        "fn probe_{i}() {{\n    let data: Channel<[int]> = Channel.bounded(1);\n    let t = spawn(fn() -> int {{\n        send(data, [{k}, {k} + 1]);\n        send(data, [{k} * 10]);\n        close(data);\n        {k}\n    }});\n    if let Option.Some(v) = recv(data) {{ print(v); }}\n    if let Option.Some(v) = recv(data) {{ print(v); }}\n    match (recv(data)) {{ Option.Some(v) => print(v), Option.None => print(\"closed\"), }}\n    print(join(t));\n}}\n"
    );
    (vec![], top)
}

fn gen_actor(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    // Canales DENTRO de variantes cruzando fibras (el patrón rayrelay, §64).
    let k = r.int_in(1, 9);
    let top = format!(
        "enum ReqP{i} {{ Put(string, int), Get(string, Channel<int>), Stop, }}\n\
         fn probe_{i}() {{\n    let inbox: Channel<ReqP{i}> = Channel.bounded(8);\n    let t = spawn(fn() -> int {{\n        var store: Map<string, int> = Map.new();\n        var served = 0;\n        var run = true;\n        while (run) {{\n            match (recv(inbox)) {{\n                Option.Some(m) => match (m) {{\n                    ReqP{i}.Put(key, v) => {{ store.insert(key, v); }},\n                    ReqP{i}.Get(key, reply) => {{\n                        match (get(store, key)) {{\n                            Option.Some(v) => send(reply, v),\n                            Option.None => send(reply, -1),\n                        }}\n                        served = served + 1;\n                    }},\n                    ReqP{i}.Stop => {{ run = false; }},\n                }},\n                Option.None => {{ run = false; }},\n            }}\n        }}\n        served\n    }});\n    send(inbox, ReqP{i}.Put(\"a\", {k}));\n    let r1: Channel<int> = Channel.bounded(1);\n    send(inbox, ReqP{i}.Get(\"a\", r1));\n    print(recv(r1));\n    let r2: Channel<int> = Channel.bounded(1);\n    send(inbox, ReqP{i}.Get(\"zz\", r2));\n    print(recv(r2));\n    send(inbox, ReqP{i}.Stop);\n    print(join(t));\n}}\n"
    );
    (vec![], top)
}

fn gen_return_in_spawn(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    // La clase EXACTA del bug de raykv (§68): `return;` en un match dentro de while dentro
    // del cuerpo literal de spawn.
    let k = r.int_in(2, 5);
    let top = format!(
        "fn probe_{i}() {{\n    let go: Channel<int> = Channel.bounded(4);\n    let out: Channel<string> = Channel.bounded(4);\n    let t = spawn(fn() {{\n        while (true) {{\n            match (recv(go)) {{\n                Option.Some(v) => {{\n                    if (v == 0) {{ return; }}\n                    send(out, \"tick${{v}}\");\n                }},\n                Option.None => {{ return; }},\n            }}\n        }}\n    }});\n    send(go, {k});\n    send(go, 1);\n    send(go, 0);\n    join(t);\n    print(recv(out));\n    print(recv(out));\n    print(\"stopped\");\n}}\n"
    );
    (vec![], top)
}

fn gen_scope_children(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let (a, b) = (r.int_in(1, 9), r.int_in(10, 90));
    let top = format!(
        "fn probe_{i}() {{\n    let total = scope(fn() -> int {{\n        let x = spawn(fn() -> int {{ {a} }});\n        let y = spawn(fn() -> int {{ {b} }});\n        let z = spawn(fn() -> string {{ \"side\" }});\n        print(join(z));\n        join(x) + join(y)\n    }});\n    print(total);\n    let outcome = scope(fn() -> string {{\n        let bad = spawn(fn() -> int {{\n            panic(\"boom{i}\");\n            0\n        }});\n        match (try_join(bad)) {{ Result.Ok(v) => \"ok${{v}}\", Result.Err(e) => \"caught:${{e}}\", }}\n    }});\n    print(outcome);\n}}\n"
    );
    (vec![], top)
}

fn gen_try_recv_select(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let k = r.int_in(1, 9);
    let top = format!(
        "fn probe_{i}() {{\n    let ch: Channel<int> = Channel.bounded(2);\n    match (try_recv(ch)) {{ Received.Got(v) => print(v), Received.Empty => print(\"empty\"), Received.Closed => print(\"closed\"), }}\n    send(ch, {k});\n    match (try_recv(ch)) {{ Received.Got(v) => print(v), Received.Empty => print(\"empty\"), Received.Closed => print(\"closed\"), }}\n    close(ch);\n    match (try_recv(ch)) {{ Received.Got(v) => print(v), Received.Empty => print(\"empty\"), Received.Closed => print(\"closed\"), }}\n    let a: Channel<int> = Channel.bounded(1);\n    let b: Channel<int> = Channel.bounded(1);\n    send(b, {k});\n    match (select_timeout([a, b], 200)) {{ Option.Some(idx) => print(idx), Option.None => print(\"timeout\"), }}\n    match (select_timeout([a], 1)) {{ Option.Some(idx) => print(idx), Option.None => print(\"timeout\"), }}\n}}\n"
    );
    (vec![], top)
}

// ---------------------------------------------------------------------------------------------
// Ensamblado del batch y ejecución de los motores
// ---------------------------------------------------------------------------------------------

/// Ensambla las sondas en UN programa: imports dedup-ados + top-levels + un main que llama a
/// cada `probe_{i}` con un separador impreso (para leer la salida por sonda al divergir).
fn assemble(probes: &[Probe]) -> String {
    let mut imports: BTreeSet<&'static str> = BTreeSet::new();
    for p in probes {
        imports.extend(p.imports.iter().copied());
    }
    let mut src = String::new();
    for m in &imports {
        src.push_str(&format!("import {m};\n"));
    }
    src.push('\n');
    for p in probes {
        src.push_str(&p.top);
        src.push('\n');
    }
    src.push_str("fn main() {\n");
    for p in probes {
        src.push_str(&format!("    print(\"== p{}:{}\");\n", p.idx, p.gen_name));
        src.push_str(&format!("    probe_{}();\n", p.idx));
    }
    src.push_str("}\n");
    src
}

/// Corre un comando con TIMEOUT (un deadlock de una sonda concurrente no debe colgar el harness).
/// Devuelve (stdout, stderr, exit code) o Err si venció el plazo.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<(String, String, Option<i32>), String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("no se pudo lanzar: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("timeout tras {}s", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(format!("wait falló: {e}")),
        }
    }
    // La salida cabe en el buffer del pipe (las sondas imprimen poco) → leer tras la espera es seguro.
    let out = child.wait_with_output().map_err(|e| format!("output falló: {e}"))?;
    Ok((
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    ))
}

/// El sabor del batch: qué motores se comparan y con qué flags se construye el nativo.
#[derive(Clone, Copy, PartialEq)]
enum Flavor {
    /// Secuencial: 3 motores; nativo por la vía rustc-pelado (`--without` todo) — build ~1 s.
    Sequential,
    /// Concurrente: VM ↔ nativo con FIBRAS (el default del producto); el intérprete no aplica.
    Concurrent,
}

/// El resultado de comparar un conjunto de sondas en todos los motores del sabor.
/// `Ok(())` = byte-idénticos. `Err(descripción)` = divergencia o build roto.
fn check(probes: &[Probe], flavor: Flavor, dir: &Path, tag: &str) -> Result<(), String> {
    let src_path = dir.join(format!("diff_{tag}.ray"));
    let bin_path = dir.join(format!("diff_{tag}_bin{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&src_path, assemble(probes)).map_err(|e| format!("write: {e}"))?;
    let src = src_path.to_str().unwrap();

    // (1) La VM — el motor de referencia. Un fallo AQUÍ es un bug del harness (sonda inválida).
    let mut vm_cmd = Command::new(BIN);
    vm_cmd.args(["run", src]);
    let (vm_out, vm_err, vm_code) = run_with_timeout(vm_cmd, Duration::from_secs(60))
        .map_err(|e| format!("VM: {e}"))?;
    if vm_code != Some(0) {
        // Sonda que no compila o revienta: es un defecto del GENERADOR, no una divergencia.
        panic!(
            "el harness generó un programa que la VM rechaza (bug del generador, no divergencia).\n\
             programa: {}\nexit: {vm_code:?}\nstderr:\n{vm_err}",
            src_path.display()
        );
    }

    // (2) El intérprete (solo secuencial): el oráculo de desarrollo.
    if flavor == Flavor::Sequential {
        let mut it_cmd = Command::new(BIN);
        it_cmd.args(["--interp", src]);
        let (it_out, it_err, it_code) = run_with_timeout(it_cmd, Duration::from_secs(60))
            .map_err(|e| format!("interp: {e}"))?;
        if it_out != vm_out || it_code != vm_code {
            return Err(format!(
                "interp ≠ VM\n  exit: interp={it_code:?} vm={vm_code:?}\n  stderr interp: {}\n{}",
                it_err.trim(),
                first_diff(&it_out, &vm_out, "interp", "vm")
            ));
        }
    }

    // (3) El binario nativo.
    let mut args: Vec<&str> = vec!["build", src, "--native", "-o", bin_path.to_str().unwrap()];
    if flavor == Flavor::Sequential {
        // rustc-pelado: sin crates y con hilo-por-tarea — el build baja de ~5 s a ~1 s.
        args.extend_from_slice(&["--without", "crypto,tls,sqlite,mimalloc,ahash,regex,fibers,process,watch"]);
    }
    let mut build_cmd = Command::new(BIN);
    build_cmd.args(&args);
    let (_, build_err, build_code) = run_with_timeout(build_cmd, Duration::from_secs(300))
        .map_err(|e| format!("build nativo: {e}"))?;
    if build_code != Some(0) {
        return Err(format!("build nativo FALLÓ (clase sort-float/return-en-spawn):\n{}", build_err.trim()));
    }
    let (nat_out, nat_err, nat_code) = run_with_timeout(Command::new(&bin_path), Duration::from_secs(60))
        .map_err(|e| format!("binario nativo: {e}"))?;
    let _ = std::fs::remove_file(&bin_path);
    if nat_out != vm_out || nat_code != vm_code {
        return Err(format!(
            "nativo ≠ VM\n  exit: nativo={nat_code:?} vm={vm_code:?}\n  stderr nativo: {}\n{}",
            nat_err.trim(),
            first_diff(&nat_out, &vm_out, "nativo", "vm")
        ));
    }
    Ok(())
}

/// La primera línea donde dos salidas divergen (diagnóstico compacto; la salida entera puede ser larga).
fn first_diff(a: &str, b: &str, la: &str, lb: &str) -> String {
    for (n, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            return format!("  primera divergencia (línea {}):\n    {la}: {x:?}\n    {lb}: {y:?}", n + 1);
        }
    }
    format!(
        "  longitudes distintas: {la}={} líneas, {lb}={} líneas\n  cola {la}: {:?}\n  cola {lb}: {:?}",
        a.lines().count(),
        b.lines().count(),
        a.lines().last().unwrap_or(""),
        b.lines().last().unwrap_or("")
    )
}

/// Bisección: aísla un subconjunto mínimo (por mitades) de sondas que reproduce el fallo.
/// Si ninguna mitad falla por separado (fallo de interacción), se queda con el conjunto actual.
fn bisect(mut probes: Vec<Probe>, flavor: Flavor, dir: &Path) -> (Vec<Probe>, String) {
    let mut last_err = check(&probes, flavor, dir, "bisect").unwrap_err();
    while probes.len() > 1 {
        let mid = probes.len() / 2;
        let (first, second) = (probes[..mid].to_vec(), probes[mid..].to_vec());
        if let Err(e) = check(&first, flavor, dir, "bisect") {
            probes = first;
            last_err = e;
        } else if let Err(e) = check(&second, flavor, dir, "bisect") {
            probes = second;
            last_err = e;
        } else {
            break; // interacción entre mitades: el conjunto actual es el mínimo alcanzable así
        }
    }
    (probes, last_err)
}

impl Clone for Probe {
    fn clone(&self) -> Self {
        Probe { imports: self.imports.clone(), top: self.top.clone(), idx: self.idx, gen_name: self.gen_name }
    }
}

/// Genera un batch: una sonda por generador de la lista, con los valores sembrados por `seed`.
fn make_batch(gens: &[(&'static str, GenFn)], seed: u64) -> Vec<Probe> {
    let mut rng = Rng(seed);
    gens.iter()
        .enumerate()
        .map(|(idx, (name, g))| {
            let (imports, top) = g(&mut rng, idx);
            Probe { imports, top, idx, gen_name: name }
        })
        .collect()
}

/// Corre un batch completo con bisección al fallar. `label` para el mensaje; `seed` reproducible.
fn run_batch(gens: &[(&'static str, GenFn)], flavor: Flavor, seed: u64, label: &str) {
    let dir = std::env::temp_dir().join(format!("ray_differential_{}_{seed}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("crea el dir temporal");
    let probes = make_batch(gens, seed);
    let result = check(&probes, flavor, &dir, "full");
    if let Err(first_err) = result {
        let (minimal, err) = bisect(probes, flavor, &dir);
        let reproduction = std::env::temp_dir().join(format!("ray_diff_repro_{seed}.ray"));
        let _ = std::fs::write(&reproduction, assemble(&minimal));
        let culprits: Vec<&str> = minimal.iter().map(|p| p.gen_name).collect();
        panic!(
            "DIVERGENCIA entre motores ({label}, semilla {seed}).\n\
             sondas mínimas: {culprits:?}\n\
             repro escrito en: {}\n\
             re-ejecutar solo este batch: RAYLANG_DIFF_SEED={seed} cargo test --test native_differential -- --ignored\n\
             --- fallo del batch completo ---\n{first_err}\n\
             --- fallo del repro mínimo ---\n{err}",
            reproduction.display()
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Como en native_corpus: en local sin rustc el skip es honesto; bajo CI sería un falso verde.
fn require_rustc() -> bool {
    if has_rustc() {
        return true;
    }
    assert!(
        std::env::var_os("CI").is_none(),
        "rustc no disponible bajo CI: el harness diferencial daría un falso verde"
    );
    eprintln!("saltando native_differential: rustc no disponible");
    false
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

/// Semilla base ("RAYDIFF" no cabe: usa el patrón de fuzz_frontend con otra constante).
const BASE_SEED: u64 = 0x5241_5944_4946_4632; // "RAYDIFF2" en ASCII

// ---------------------------------------------------------------------------------------------
// Entradas
// ---------------------------------------------------------------------------------------------

/// Humo en CADA `cargo test`: un batch secuencial (3 motores; build nativo rustc-pelado ~1-2 s).
/// La cobertura ancha vive en la campaña de abajo; esto garantiza que el harness mismo no se
/// pudra y que la clase más barata de divergencia se cace en cada push.
#[test]
fn differential_smoke_one_sequential_batch() {
    if !require_rustc() {
        return;
    }
    run_batch(SEQ_GENS, Flavor::Sequential, BASE_SEED, "humo secuencial");
}

/// La campaña: varios batches secuenciales con semillas rotadas + los batches de concurrencia
/// (build nativo con fibras, la vía Cargo — más lento). `#[ignore]` en local; CI la corre en
/// cada push (como native_corpus) y la nocturna con `RAYLANG_DIFF_BATCHES` alto.
#[test]
#[ignore = "compila varios binarios nativos (~1-3 min); correr con -- --ignored"]
fn differential_campaign() {
    if !require_rustc() {
        return;
    }
    let batches = env_u64("RAYLANG_DIFF_BATCHES", 4);
    // Modo reproducción: `RAYLANG_DIFF_SEED=<n>` corre SOLO ese batch en ambos sabores.
    if let Some(seed) = std::env::var("RAYLANG_DIFF_SEED").ok().and_then(|s| s.parse::<u64>().ok()) {
        run_batch(SEQ_GENS, Flavor::Sequential, seed, "repro secuencial");
        run_batch(CONC_GENS, Flavor::Concurrent, seed, "repro concurrente");
        return;
    }
    for b in 0..batches {
        let seed = BASE_SEED.wrapping_add((b + 1).wrapping_mul(0x9E37_79B9));
        run_batch(SEQ_GENS, Flavor::Sequential, seed, "campaña secuencial");
    }
    // Concurrencia: menos batches (el build con fibras va por la vía Cargo, ~5-10 s cada uno).
    let conc_batches = (batches / 2).max(1);
    for b in 0..conc_batches {
        let seed = BASE_SEED.wrapping_add((b + 1).wrapping_mul(0x51_7C_C1_B7));
        run_batch(CONC_GENS, Flavor::Concurrent, seed, "campaña concurrente");
    }
}
