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
//! Dos clases de generador (nov 2026): los de PLANTILLA FIJA, donde la semilla solo cambia los
//! valores, y los ESTRUCTURALES, que fijan la forma y sortean el TIPO sobre el que opera (primitivo,
//! tupla, struct o enum propio, y sus envoltorios `[T]`/`Option<T>`/`[[T]]`). Los segundos nacieron
//! del repaso de los hallazgos históricos: casi todos los «el checker acepta y el nativo no
//! compila» salen del cruce forma × tipo, que una plantilla fija nunca recorre.
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
    // M287 (ray-sublime §102): PATRONES ANIDADOS de verdad (`Wrap(Leaf(v))`, y dentro de `Option`)
    // y una guarda `if` con binding — antes la sonda generaba el rodeo (`Wrap(inner) => match
    // (inner)`), que es justo lo único que compilaba en nativo, y el bug pasó de largo.
    let k = r.int_in(1, 9);
    let top = format!(
        "enum InnerP{i} {{ Leaf(int), Nil, }}\n\
         enum OuterP{i} {{ Wrap(InnerP{i}), Tag(string), }}\n\
         fn describe_{i}(o: OuterP{i}) -> string {{\n    match (o) {{\n        OuterP{i}.Wrap(InnerP{i}.Leaf(v)) if v > 4 => \"big:${{v}}\",\n        OuterP{i}.Wrap(InnerP{i}.Leaf(v)) => \"leaf:${{v}}\",\n        OuterP{i}.Wrap(_) => \"nil\",\n        OuterP{i}.Tag(s) => \"tag:${{s}}\",\n    }}\n}}\n\
         fn opt_{i}(o: Option<OuterP{i}>) -> string {{\n    match (o) {{\n        Option.Some(OuterP{i}.Wrap(InnerP{i}.Leaf(v))) => \"some:${{v}}\",\n        Option.Some(OuterP{i}.Tag(t)) if t == \"z\" => \"zeta\",\n        Option.Some(_) => \"some\",\n        Option.None => \"none\",\n    }}\n}}\n\
         fn probe_{i}() {{\n    print(describe_{i}(OuterP{i}.Wrap(InnerP{i}.Leaf({k}))));\n    print(describe_{i}(OuterP{i}.Wrap(InnerP{i}.Leaf(3))));\n    print(describe_{i}(OuterP{i}.Wrap(InnerP{i}.Nil)));\n    print(describe_{i}(OuterP{i}.Tag(\"t{k}\")));\n    print(opt_{i}(Option.Some(OuterP{i}.Wrap(InnerP{i}.Leaf({k})))));\n    print(opt_{i}(Option.Some(OuterP{i}.Tag(\"z\"))));\n    print(opt_{i}(Option.Some(OuterP{i}.Wrap(InnerP{i}.Nil))));\n    print(opt_{i}(Option.None));\n    let all = [OuterP{i}.Tag(\"a\"), OuterP{i}.Wrap(InnerP{i}.Leaf(2))];\n    print(all.len());\n}}\n"
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
        "fn probe_{i}() {{\n    let b = b\"ok\\x00\\xff\" + bytes_of([{k}, 65]);\n    print(b.len());\n    print(b[0]);\n    print(b[b.len() - 1]);\n    print(b.sub_bytes(1, 3));\n    match (from_utf8(b\"hola\")) {{ Result.Ok(s) => print(s), Result.Err(e) => print(e), }}\n    match (from_utf8(b\"\\xff\\xfe\")) {{ Result.Ok(s) => print(s), Result.Err(e) => print(\"inv\"), }}\n    print(\"ñu\".to_bytes());\n    let parts: [bytes] = [b\"ok\", bytes_of([{k}]), b\"\", b.sub_bytes(0, 2)];\n    print(bytes_concat(parts));\n    print(bytes_concat([]).len());\n    print(__base64_decode_bytes(b\"x:QUJD\", 2, 99, false)[1]);\n}}\n"
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
        "fn probe_{i}() {{\n    let ch: Channel<int> = Channel.bounded(2);\n    match (try_recv(ch)) {{ Received.Got(v) => print(v), Received.Empty => print(\"empty\"), Received.Closed => print(\"closed\"), }}\n    send(ch, {k});\n    print(try_send(ch, {k}));\n    print(try_send(ch, {k}));\n    match (try_recv(ch)) {{ Received.Got(v) => print(v), Received.Empty => print(\"empty\"), Received.Closed => print(\"closed\"), }}\n    close(ch);\n    print(try_send(ch, {k}));\n    match (try_recv(ch)) {{ Received.Got(v) => print(v), Received.Empty => print(\"empty\"), Received.Closed => print(\"closed\"), }}\n    let a: Channel<int> = Channel.bounded(1);\n    let b: Channel<int> = Channel.bounded(1);\n    send(b, {k});\n    match (select_timeout([a, b], 200)) {{ Option.Some(idx) => print(idx), Option.None => print(\"timeout\"), }}\n    match (select_timeout([a], 1)) {{ Option.Some(idx) => print(idx), Option.None => print(\"timeout\"), }}\n}}\n"
    );
    (vec![], top)
}

// ---------------------------------------------------------------------------------------------
// Variación ESTRUCTURAL (arco de huecos, nov 2026)
// ---------------------------------------------------------------------------------------------
//
// Los generadores de arriba son plantillas FIJAS: la semilla cambia los valores, no la forma. El
// repaso de los hallazgos históricos (24 de «el checker acepta y el nativo no compila», 11 apps)
// mostró que casi todos nacen del cruce FORMA × TIPO: el mismo código que funciona con `int` falla
// con una tupla, un struct propio o un `Option` de un enum. Estos generadores fijan la forma (una
// familia de hallazgos cada uno) y sortean el TIPO sobre el que opera, con sus declaraciones.

/// Un tipo sorteado: su expresión de tipo, dos valores DISTINTOS (expresiones sin contexto: se
/// pueden escribir en cualquier posición) y las declaraciones top-level que necesita.
#[derive(Clone)]
struct Ty {
    ty: String,
    a: String,
    b: String,
    /// Sirve de clave de `Map` (`MapKey`): ni `float` ni arrays, ni nada que los contenga.
    key: bool,
    decls: String,
}

fn prim_ty(r: &mut Rng) -> Ty {
    let (ty, a, b, key) = match r.below(4) {
        0 => {
            let k = r.int_in(-9, 90);
            ("int", format!("{k}"), format!("{}", k + 1), true)
        }
        1 => {
            let w = r.word();
            ("string", format!("\"{w}\""), format!("\"{w}x\""), true)
        }
        2 => ("bool", "true".to_string(), "false".to_string(), true),
        _ => {
            let k = r.int_in(1, 90);
            ("float", format!("{k}.5"), format!("{k}.25"), false)
        }
    };
    Ty { ty: ty.to_string(), a, b, key, decls: String::new() }
}

/// Un tipo «elemento»: primitivo, struct propio o enum propio (con payloads primitivos — el
/// `@derive` no admite tuplas ni `Option` en los campos).
fn elem_ty(r: &mut Rng, tag: &str) -> Ty {
    match r.below(5) {
        0 | 1 => prim_ty(r),
        2 | 3 => {
            let (p, q) = (prim_ty(r), prim_ty(r));
            let key = p.key && q.key;
            let name = format!("St{tag}");
            let derive = if key { "@derive(Eq, Hash, Show)" } else { "@derive(Eq, Show)" };
            Ty {
                ty: name.clone(),
                a: format!("{name} {{ p: {}, q: {} }}", p.a, q.a),
                b: format!("{name} {{ p: {}, q: {} }}", p.b, q.a),
                key,
                decls: format!("{derive}\nstruct {name} {{ p: {}, q: {}, }}\n", p.ty, q.ty),
            }
        }
        _ => {
            let p = prim_ty(r);
            let name = format!("En{tag}");
            let derive = if p.key { "@derive(Eq, Hash, Show)" } else { "@derive(Eq, Show)" };
            let (a, b) = match r.below(3) {
                0 => (format!("{name}.One({})", p.a), format!("{name}.Zero")),
                1 => (format!("{name}.Zero"), format!("{name}.Two(3, {})", p.b)),
                _ => (format!("{name}.Two(1, {})", p.a), format!("{name}.One({})", p.a)),
            };
            Ty {
                ty: name.clone(),
                a,
                b,
                key: p.key,
                decls: format!("{derive}\nenum {name} {{ One({}), Zero, Two(int, {}), }}\n", p.ty, p.ty),
            }
        }
    }
}

/// Un tipo cualquiera del universo de sondas: un elemento, o un elemento envuelto.
fn any_ty(r: &mut Rng, i: usize) -> Ty {
    let tag = format!("P{i}");
    match r.below(9) {
        0 | 1 | 2 => elem_ty(r, &tag),
        3 | 4 => {
            let e = elem_ty(r, &tag);
            Ty { ty: format!("[{}]", e.ty), a: format!("[{}, {}]", e.a, e.b), b: format!("[{}]", e.b), key: false, decls: e.decls }
        }
        5 | 6 => {
            let e = elem_ty(r, &tag);
            Ty {
                ty: format!("Option<{}>", e.ty),
                a: format!("Option.Some({})", e.a),
                b: format!("Option.Some({})", e.b),
                key: e.key,
                decls: e.decls,
            }
        }
        7 => {
            // Las tuplas, solo de primitivos: `print` no imprime una tupla con un tipo propio
            // (ni una tupla suelta: las sondas la imprimen dentro de un array o un `Option`).
            let (p, q) = (prim_ty(r), prim_ty(r));
            Ty {
                ty: format!("({}, {})", p.ty, q.ty),
                a: format!("({}, {})", p.a, q.a),
                b: format!("({}, {})", p.b, q.a),
                key: p.key && q.key,
                decls: String::new(),
            }
        }
        _ => {
            let e = elem_ty(r, &tag);
            Ty {
                ty: format!("[[{}]]", e.ty),
                a: format!("[[{}], [{}, {}]]", e.a, e.b, e.a),
                b: format!("[[{}]]", e.b),
                key: false,
                decls: e.decls,
            }
        }
    }
}

/// Rellena una plantilla: `@I@` = índice de la sonda, `@T@` = el tipo, `@A@`/`@B@` = sus dos
/// valores, `@K@` = un entero sorteado (1..9). Las plantillas van en raw strings: código raylang
/// legible, sin el doble escape de `format!`.
fn fill(template: &str, i: usize, t: &Ty, k: i64) -> String {
    let body = template
        .replace("@I@", &i.to_string())
        .replace("@T@", &t.ty)
        .replace("@A@", &t.a)
        .replace("@B@", &t.b)
        .replace("@K@", &k.to_string());
    format!("{}{}", t.decls, body)
}

/// Generadores ESTRUCTURALES secuenciales (3 motores).
const SEQ_STRUCT_GENS: &[(&str, GenFn)] = &[
    ("generic_bounds_over_type", gen_generic_bounds),
    ("empties_typed_by_context", gen_empties),
    ("closures_stored_and_returned", gen_closures_stored),
    ("option_result_of_type", gen_option_result_of),
    ("aliasing_mutation", gen_aliasing),
    ("match_guards_and_captures", gen_match_guards),
    ("struct_patterns_over_type", gen_struct_patterns),
];

/// Generadores ESTRUCTURALES de concurrencia (VM ↔ nativo con fibras).
const CONC_STRUCT_GENS: &[(&str, GenFn)] = &[
    ("channels_in_containers", gen_channels_in_containers),
    ("values_cross_spawn", gen_values_cross_spawn),
];

/// Genéricos con bounds sobre un tipo sorteado (raysolve #352–#356, #359): `Eq` sobre arrays,
/// `MapKey` como clave, un campo `fn` de un struct genérico, closures que producen `P`.
fn gen_generic_bounds(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let t = any_ty(r, i);
    let mut tpl = String::from(
        r#"fn has_@I@<P: Eq>(xs: [P], x: P) -> bool { xs.contains(x) }
fn at_@I@<P: Eq>(xs: [P], x: P) -> Option<int> { xs.position(x) }
fn same_@I@<P: Eq>(a: P, b: P) -> bool { a == b }
fn first_or_@I@<P>(xs: [P], d: P) -> P { if (xs.len() > 0) { xs[0] } else { d } }
fn wrap_@I@<P>(x: P) -> Option<P> { Option.Some(x) }
fn pair_@I@<A, B>(a: A, b: B) -> (A, B) { (a, b) }
fn twice_@I@<P>(x: P, f: fn(P) -> P) -> P { f(f(x)) }
fn collect_@I@<P>(n: int, mk: fn(int) -> P) -> [P] {
    var out: [P] = [];
    for j in 0..n { out.push(mk(j)); }
    out
}
struct Prov_@I@<P> { deps: fn(P) -> [P], seed: P, }
fn expand_@I@<P>(p: Prov_@I@<P>) -> [P] {
    let f = p.deps;
    f(p.seed)
}
"#,
    );
    if t.key {
        tpl.push_str(
            r#"fn tally_@I@<P: MapKey>(xs: [P]) -> int {
    var m: Map<P, int> = Map.new();
    for x in xs {
        match (get(m, x)) {
            Option.Some(n) => { m.insert(x, n + 1); },
            Option.None => { m.insert(x, 1); },
        }
    }
    m.keys().len()
}
fn uniq_@I@<P: MapKey>(xs: [P]) -> [P] {
    var seen: Map<P, bool> = Map.new();
    var out: [P] = [];
    for x in xs {
        if (!seen.contains_key(x)) {
            seen.insert(x, true);
            out.push(x);
        }
    }
    out
}
"#,
        );
    }
    tpl.push_str(
        r#"fn probe_@I@() {
    let a: @T@ = @A@;
    let b: @T@ = @B@;
    let xs = [a, b, a];
    print(wrap_@I@(a));
    print(xs);
    print(has_@I@(xs, b));
    print(at_@I@(xs, b));
    print(same_@I@(a, b));
    print(same_@I@(xs[0], xs[2]));
    print(first_or_@I@(xs, b) == a);
    let none: [@T@] = [];
    print(first_or_@I@(none, b) == b);
    print(wrap_@I@(a) == Option.Some(a));
    let p = pair_@I@(a, @K@);
    print(p.1);
    print(p.0 == a);
    print(twice_@I@(a, fn(x: @T@) -> @T@ { x }) == a);
    let made = collect_@I@(3, fn(j: int) -> @T@ { if (j == 1) { @B@ } else { @A@ } });
    print(made.len());
    print(at_@I@(made, b));
    let prov = Prov_@I@ { deps: fn(x: @T@) -> [@T@] { [x, @B@] }, seed: a };
    let ex = expand_@I@(prov);
    print(ex.len());
    print(ex[1] == b);
    print(has_@I@(ex, a));
"#,
    );
    if t.key {
        tpl.push_str(
            r#"    print(tally_@I@(xs));
    print(uniq_@I@(xs).len());
    var idx: Map<@T@, int> = Map.new();
    idx.insert(a, 1);
    idx.insert(b, 2);
    idx.insert(a, 3);
    print(get(idx, a));
    print(idx.contains_key(b));
    print(idx.keys().len());
"#,
        );
    }
    tpl.push_str("}\n");
    (vec![], fill(&tpl, i, &t, r.int_in(1, 9)))
}

/// Literales vacíos y `Option.None` que toman su tipo del contexto (#6, #78, #86, #133, #137,
/// #147): la rama gemela de un `if`/`match`, un argumento, un campo, un `return`, un operando.
fn gen_empties(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let t = any_ty(r, i);
    let tpl = r#"struct Hold_@I@ { items: [@T@], pick: Option<@T@>, }
fn take_arr_@I@(xs: [@T@]) -> int { xs.len() }
fn take_opt_@I@(o: Option<@T@>) -> bool { o.is_some() }
fn glen_@I@<P>(xs: [P], d: P) -> int { xs.len() }
fn gopt_@I@<P>(o: Option<P>, d: P) -> P { o.unwrap_or(d) }
fn ret_arr_@I@(c: bool) -> [@T@] {
    if (c) { return []; }
    [@A@]
}
fn ret_res_@I@(n: int) -> Result<@T@, string> {
    match (n) {
        0 => Result.Ok(@A@),
        1 => Result.Err("one"),
        _ => Result.Ok(@B@),
    }
}
fn ret_opt_@I@(n: int) -> Option<@T@> {
    if (n > @K@) { Option.Some(@A@) } else { Option.None }
}
fn probe_@I@() {
    let c = ret_opt_@I@(0).is_some();
    let e1: [@T@] = if (c) { [] } else { [@A@] };
    let e2: [@T@] = if (c) { [@A@] } else { [] };
    print(e1.len() + e2.len());
    let e3 = if (c) { [] } else { [@A@, @B@] };
    print(e3.len());
    let o1: Option<@T@> = match (@K@) { 0 => Option.None, _ => Option.Some(@A@), };
    let o2 = if (c) { Option.Some(@B@) } else { Option.None };
    print(o1.is_some());
    print(o2.is_none());
    print(o1 == Option.None);
    print(Option.None == o2);
    print(take_arr_@I@([]));
    print(take_opt_@I@(Option.None));
    print(glen_@I@([], @A@));
    print(gopt_@I@(Option.None, @B@) == @B@);
    let h = Hold_@I@ { items: [], pick: Option.None };
    print(h.items.len());
    print(h.pick.is_none());
    var grow: [@T@] = [] + e1;
    grow = grow + [];
    grow.push(@B@);
    print(grow.len());
    var nest: [[@T@]] = [];
    nest.push([]);
    nest.push([@A@]);
    print(nest[0].len() + nest[1].len());
    var m: Map<string, [@T@]> = Map.new();
    m.insert("k", []);
    print(get(m, "k").unwrap_or([@A@]).len());
    print(ret_arr_@I@(true).len() + ret_arr_@I@(false).len());
    let r0 = ret_res_@I@(0);
    print(r0.is_ok());
    match (ret_res_@I@(1)) {
        Result.Ok(_v) => print("ok"),
        Result.Err(e) => print(e),
    }
    print(ret_opt_@I@(0).is_none());
    let t = match (@K@ % 2) { 0 => (Option.None, 1), _ => (Option.Some(@A@), 2), };
    print(t.1);
    let ch = if (c) { e3 } else { [] };
    print(ch.len());
}
"#;
    (vec![], fill(tpl, i, &t, r.int_in(1, 9)))
}

/// Closures guardados y devueltos (#60, #77 en su parte secuencial): en un `Map`, en un array,
/// en un campo, como resultado de una función, elegidos por un `if`.
fn gen_closures_stored(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let t = any_ty(r, i);
    let tpl = r#"struct Ops_@I@ { f: fn(@T@) -> @T@, g: fn(@T@) -> [@T@], tag: string, }
fn make_@I@(d: @T@) -> fn(@T@) -> @T@ {
    fn(x: @T@) -> @T@ { if (x == d) { @B@ } else { d } }
}
fn register_@I@(m: Map<string, fn(@T@) -> @T@>, name: string, f: fn(@T@) -> @T@) {
    m.insert(name, f);
}
fn apply_all_@I@(fs: [fn(@T@) -> @T@], x: @T@) -> @T@ {
    var cur = x;
    for f in fs { cur = f(cur); }
    cur
}
fn probe_@I@() {
    var table: Map<string, fn(@T@) -> @T@> = Map.new();
    register_@I@(table, "id", fn(x: @T@) -> @T@ { x });
    register_@I@(table, "mk", make_@I@(@A@));
    table.insert("const", fn(_x: @T@) -> @T@ { @B@ });
    print(table.keys().len());
    match (get(table, "mk")) {
        Option.Some(f) => print(f(@A@) == @B@),
        Option.None => print("missing"),
    }
    let idf = get(table, "id").unwrap();
    print(idf(@A@) == @A@);
    let fs = [idf, make_@I@(@B@), fn(x: @T@) -> @T@ { x }];
    print(apply_all_@I@(fs, @A@) == @B@);
    let ops = Ops_@I@ { f: make_@I@(@A@), g: fn(x: @T@) -> [@T@] { [x, x] }, tag: "t@K@" };
    let run = fn(o: Ops_@I@, x: @T@) -> int {
        let g = o.g;
        let f = o.f;
        g(f(x)).len()
    };
    print(run(ops, @B@));
    print(ops.tag);
    var count = 0;
    let counted = fn(x: @T@) -> @T@ {
        count = count + 1;
        x
    };
    let _u = apply_all_@I@([counted, counted], @A@);
    print(count);
    let pick = if (@K@ > 4) { make_@I@(@A@) } else { idf };
    print(pick(@B@) == @B@);
    print(get(table, "const").unwrap()(@A@) == @B@);
    print(make_@I@(@A@)(@A@) == @B@);
    print(fs[1](@B@) == @B@);
    print(ops.g(@A@).len());
    let viamatch = match (count) { 0 => idf, _ => counted, };
    print(viamatch(@A@) == @A@);
    let some: Option<fn(@T@) -> @T@> = Option.Some(counted);
    match (some) {
        Option.Some(f) => print(f(@B@) == @B@),
        Option.None => print("none"),
    }
    let tup = (counted, @K@);
    let first = tup.0;
    print(first(@A@) == @A@);
    print(count);
    var grown: [fn(@T@) -> @T@] = [];
    grown.push(idf);
    grown.push(counted);
    grown.push(make_@I@(@B@));
    print(apply_all_@I@(grown, @B@) == @B@);
    let nested = [[idf], [counted, idf]];
    print(apply_all_@I@(nested[1], @A@) == @A@);
    print(count);
}
"#;
    (vec![], fill(tpl, i, &t, r.int_in(1, 9)))
}

/// `Option`/`Result` de un tipo sorteado y con un error propio (#218, #281): `unwrap`, `?`,
/// `==`, `map`, tuplas de cuatro dentro de un `Option`, arrays de `Result`.
fn gen_option_result_of(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let t = any_ty(r, i);
    let tpl = r#"struct Fail_@I@ { code: int, what: string, }
fn find_@I@(n: int) -> Result<@T@, Fail_@I@> {
    if (n == 0) {
        Result.Err(Fail_@I@ { code: @K@, what: "none" })
    } else if (n == 1) {
        Result.Ok(@A@)
    } else {
        Result.Ok(@B@)
    }
}
fn both_@I@(n: int) -> Result<[@T@], Fail_@I@> {
    let x = find_@I@(n)?;
    let y = find_@I@(n + 1)?;
    Result.Ok([x, y])
}
fn maybe_@I@(n: int) -> Option<@T@> {
    if (n > 0) { Option.Some(@A@) } else { Option.None }
}
fn probe_@I@() {
    print(find_@I@(1).unwrap() == @A@);
    print(find_@I@(2).is_ok());
    match (find_@I@(0)) {
        Result.Ok(_v) => print("ok"),
        Result.Err(e) => print("${e.code}:${e.what}"),
    }
    print(both_@I@(1).unwrap().len());
    print(both_@I@(0).is_err());
    let o = maybe_@I@(1);
    print(o.is_some());
    print(o.unwrap() == @A@);
    print(maybe_@I@(0).unwrap_or(@B@) == @B@);
    print(maybe_@I@(0).is_none());
    print(o == Option.Some(@A@));
    print(o.map(fn(_x: @T@) -> int { @K@ }).unwrap_or(0));
    print(o);
    let pairs: Option<(@T@, int)> = Option.Some((@A@, @K@));
    print(pairs.is_some());
    match (pairs) {
        Option.Some(pq) => print(pq.1),
        Option.None => print("none"),
    }
    let quad: Option<(int, string, bool, @T@)> = Option.Some((1, "q", true, @B@));
    print(quad.is_none());
    let rs: [Result<@T@, string>] = [Result.Ok(@A@), Result.Err("bad"), Result.Ok(@B@)];
    var oks = 0;
    for res in rs {
        if let Result.Ok(_v) = res { oks = oks + 1; }
    }
    print(oks);
    match (try_call(fn() -> bool { find_@I@(0).unwrap() == @A@ })) {
        Result.Ok(v) => print(v),
        Result.Err(_e) => print("err:unwrap"),
    }
}
"#;
    (vec![], fill(tpl, i, &t, r.int_in(1, 9)))
}

/// Mutar un contenedor mientras se lee de él (#156, #315): el `RefCell already borrowed` del
/// nativo. Índices que leen y escriben el mismo array, un argumento que muta al receptor.
fn gen_aliasing(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let t = any_ty(r, i);
    let tpl = r#"struct Bag_@I@ { items: [@T@], n: int, }
fn bump_@I@(b: Bag_@I@) -> int {
    b.n = b.n + 1;
    b.items.push(@B@);
    b.n
}
fn grow_@I@(xs: [@T@]) -> int {
    xs.push(@A@);
    xs.len()
}
fn put_@I@(m: Map<string, [@T@]>, k: string) -> bool {
    m.insert(k, [@B@]);
    true
}
fn probe_@I@() {
    var xs: [@T@] = [@A@, @B@, @A@];
    for j in 1..xs.len() {
        xs[j] = xs[j - 1];
        if (j == 1) { xs.push(@B@); }
    }
    print(xs.len());
    print(xs[2] == @A@);
    xs.push(xs[0]);
    xs[0] = xs[xs.len() - 1];
    print(xs.len());
    var b = Bag_@I@ { items: [@A@], n: @K@ };
    print("abcdefghijklmnop".substring(b.n % 3, bump_@I@(b) % 3 + 4));
    print(to_string(b.n) + to_string(bump_@I@(b)) + to_string(b.n));
    b.items.push(b.items[0]);
    b.items[0] = b.items[b.items.len() - 1];
    print(b.items.len());
    print(b.items.len() + bump_@I@(b) + b.items.len());
    var m: Map<string, [@T@]> = Map.new();
    m.insert("a", [@A@]);
    m.insert("b", get(m, "a").unwrap_or([]));
    m.insert("a", get(m, "a").unwrap_or([]) + get(m, "b").unwrap_or([]));
    print(get(m, "a").unwrap_or([]).len());
    var grid: [[@T@]] = [[@A@], [@B@]];
    grid[0].push(grid[1][0]);
    grid[1] = grid[0];
    grid[1].push(@A@);
    print(grid[0].len());
    print(grid[1].len());
    let snap = xs;
    xs.push(@B@);
    print(snap.len() == xs.len());
    print(xs.len() + grow_@I@(xs));
    print(grow_@I@(xs) + xs.len());
    print(xs.reverse().len() + grow_@I@(xs));
    print(xs.contains(@B@) && grow_@I@(xs) > 0);
    print(xs.position(@B@).unwrap_or(0) + grow_@I@(xs));
    print(xs[0] == @A@ && grow_@I@(xs) > 0);
    let before = xs.len();
    let ys = xs.map(fn(x: @T@) -> @T@ {
        if (xs.len() == before) { xs.push(@B@); }
        x
    });
    print(ys.len());
    let zs = xs.filter(fn(x: @T@) -> bool {
        if (xs.len() < before + 3) { xs.push(@A@); }
        x == @A@
    });
    print(zs.len());
    print(xs.fold(0, fn(acc: int, _x: @T@) -> int {
        if (acc == 1) { xs.push(@B@); }
        acc + 1
    }));
    print(xs.len());
    for x in xs {
        if (x == @B@ && xs.len() < 40) { xs.push(@A@); }
    }
    print(xs.len());
    print(m.contains_key("zz") || put_@I@(m, "zz"));
    print(m.len() + (if (put_@I@(m, "yy")) { 1 } else { 0 }));
    for k in m.keys() { m.insert(k + "!", [@A@]); }
    print(m.len());
}
"#;
    (vec![], fill(tpl, i, &t, r.int_in(1, 9)))
}

/// Guardas de `match` y variables por brazo capturadas por closures (#1, #291): el mismo nombre
/// como `var` en un brazo y como binding capturado en otro; una guarda en una función que
/// devuelve un struct.
fn gen_match_guards(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let t = any_ty(r, i);
    let tpl = r#"struct Resp_@I@ { code: int, body: string, }
fn route_@I@(x: @T@, n: int) -> Resp_@I@ {
    match (n) {
        0 if x == @A@ => Resp_@I@ { code: 200, body: "a" },
        0 => Resp_@I@ { code: 201, body: "not-a" },
        k if k > @K@ => {
            let f = fn() -> string { "big${k}" };
            Resp_@I@ { code: k, body: f() }
        },
        _ => Resp_@I@ { code: 404, body: "nf" },
    }
}
fn classify_@I@(o: Option<@T@>, lim: int) -> string {
    match (o) {
        Option.Some(v) if v == @A@ && lim > 0 => "first",
        Option.Some(v) if v == @B@ => {
            var hits = lim;
            let inc = fn() { hits = hits + 1; };
            inc();
            "second${hits}"
        },
        Option.Some(_v) => "other",
        Option.None => {
            let hits = "none";
            let show = fn() -> string { hits };
            show()
        },
    }
}
fn probe_@I@() {
    let r1 = route_@I@(@A@, 0);
    print("${r1.code}${r1.body}");
    let r2 = route_@I@(@B@, 0);
    print("${r2.code}${r2.body}");
    let r3 = route_@I@(@A@, @K@ + 5);
    print("${r3.code}${r3.body}");
    let r4 = route_@I@(@B@, 1);
    print("${r4.code}${r4.body}");
    print(classify_@I@(Option.Some(@A@), 1));
    print(classify_@I@(Option.Some(@A@), 0));
    print(classify_@I@(Option.Some(@B@), 3));
    print(classify_@I@(Option.None, 3));
    var acc: [string] = [];
    for n in 0..4 {
        match (n) {
            0 => {
                var x = "zero";
                x = x + "!";
                acc.push(x);
            },
            1 => {
                let x = @K@;
                let f = fn() -> int { x + n };
                acc.push(to_string(f()));
            },
            m if m % 2 == 0 => { acc.push("even"); },
            _ => {
                let x = [n];
                let g = fn() -> int { x.len() };
                acc.push(to_string(g()));
            },
        }
    }
    print(acc.join(","));
}
"#;
    (vec![], fill(tpl, i, &t, r.int_in(1, 9)))
}

/// Patrones de struct con un campo del tipo sorteado (rayhotel #489): en el escrutinio, dentro
/// de un `Option`, con un literal, con un sub-patrón de `Option` y con guarda.
fn gen_struct_patterns(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let t = any_ty(r, i);
    let tpl = r#"struct Rec_@I@ { v: @T@, n: int, o: Option<@T@>, }
fn pick_@I@(r: Rec_@I@) -> int {
    match (r) {
        Rec_@I@ { v, n: 0, o: _ } => if (v == @A@) { 1 } else { 2 },
        Rec_@I@ { v: _, n, o: Option.Some(x) } if x == @B@ => n + 10,
        Rec_@I@ { v, n, o } => if (v == @B@ && o.is_none()) { n + 20 } else { n + 30 },
    }
}
fn inside_@I@(o: Option<Rec_@I@>) -> int {
    match (o) {
        Option.Some(Rec_@I@ { v, n: @K@, o: _ }) => if (v == @A@) { 100 } else { 200 },
        Option.Some(Rec_@I@ { v: _, n, o: _ }) => n,
        Option.None => 0 - 1,
    }
}
fn probe_@I@() {
    print(pick_@I@(Rec_@I@ { v: @A@, n: 0, o: Option.None }));
    print(pick_@I@(Rec_@I@ { v: @B@, n: 0, o: Option.None }));
    print(pick_@I@(Rec_@I@ { v: @A@, n: 3, o: Option.Some(@B@) }));
    print(pick_@I@(Rec_@I@ { v: @A@, n: 3, o: Option.Some(@A@) }));
    print(pick_@I@(Rec_@I@ { v: @B@, n: 4, o: Option.None }));
    print(inside_@I@(Option.Some(Rec_@I@ { v: @A@, n: @K@, o: Option.None })));
    print(inside_@I@(Option.Some(Rec_@I@ { v: @B@, n: @K@, o: Option.None })));
    print(inside_@I@(Option.Some(Rec_@I@ { v: @A@, n: 77, o: Option.None })));
    print(inside_@I@(Option.None));
}
"#;
    (vec![], fill(tpl, i, &t, r.int_in(1, 9)))
}

/// Canales dentro de tuplas, structs, mapas y enums genéricos (#71, #76, #280, #292).
fn gen_channels_in_containers(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let t = any_ty(r, i);
    let tpl = r#"enum Msg_@I@<P> { Item(P), Done, }
struct Hub_@I@ { outs: [Channel<@T@>], name: string, }
fn drain_@I@<P>(ch: Channel<Msg_@I@<P>>) -> int {
    var n = 0;
    var run = true;
    while (run) {
        match (recv(ch)) {
            Option.Some(m) => match (m) {
                Msg_@I@.Item(_p) => { n = n + 1; },
                Msg_@I@.Done => { run = false; },
            },
            Option.None => { run = false; },
        }
    }
    n
}
fn probe_@I@() {
    let pair: (Channel<@T@>, int) = (Channel.bounded(2), @K@);
    send(pair.0, @A@);
    close(pair.0);
    print(recv(pair.0) == Option.Some(@A@));
    print(recv(pair.0).is_none());
    print(pair.1);
    let hub = Hub_@I@ { outs: [Channel.bounded(1), Channel.bounded(1)], name: "h@K@" };
    let t = spawn(fn() -> int {
        send(hub.outs[0], @A@);
        send(hub.outs[1], @B@);
        2
    });
    print(recv(hub.outs[0]) == Option.Some(@A@));
    print(recv(hub.outs[1]) == Option.Some(@B@));
    print(join(t));
    print(hub.name);
    let res: Channel<Result<@T@, string>> = Channel.bounded(2);
    send(res, Result.Ok(@A@));
    send(res, Result.Err("e@K@"));
    match (recv(res)) {
        Option.Some(got) => print(got.is_ok()),
        Option.None => print("closed"),
    }
    match (recv(res)) {
        Option.Some(Result.Err(e)) => print(e),
        _ => print("other"),
    }
    let msgs: Channel<Msg_@I@<@T@>> = Channel.bounded(4);
    let w = spawn(fn() -> int { drain_@I@(msgs) });
    send(msgs, Msg_@I@.Item(@A@));
    send(msgs, Msg_@I@.Item(@B@));
    send(msgs, Msg_@I@.Done);
    print(join(w));
    let opt: Channel<Option<@T@>> = Channel.bounded(1);
    send(opt, Option.None);
    match (recv(opt)) {
        Option.Some(inner) => print(inner.is_none()),
        Option.None => print("closed"),
    }
    var chans: Map<string, Channel<@T@>> = Map.new();
    chans.insert("a", Channel.bounded(1));
    send(get(chans, "a").unwrap(), @B@);
    print(recv(get(chans, "a").unwrap()) == Option.Some(@B@));
}
"#;
    (vec![], fill(tpl, i, &t, r.int_in(1, 9)))
}

/// Valores de un tipo sorteado cruzando a `spawn`, `join`, `scope` y canales. Un CLOSURE
/// capturado por `spawn` queda fuera a propósito: es un límite declarado del nativo («a function
/// value captured by 'spawn' cannot cross threads in the native backend yet», #59/#77).
fn gen_values_cross_spawn(r: &mut Rng, i: usize) -> (Vec<&'static str>, String) {
    let t = any_ty(r, i);
    let tpl = r#"struct Job_@I@ { input: @T@, extra: [@T@], label: string, }
fn work_@I@(j: Job_@I@) -> [@T@] {
    var out = j.extra;
    out.push(j.input);
    out
}
fn probe_@I@() {
    let a: @T@ = @A@;
    let t1 = spawn(fn() -> bool { a == @A@ });
    print(join(t1));
    let job = Job_@I@ { input: @B@, extra: [@A@], label: "k@K@" };
    let t3 = spawn(fn() -> string {
        if (work_@I@(job).len() == 2) { job.label } else { "no" }
    });
    print(join(t3));
    let t4 = spawn(fn() -> @T@ { @A@ });
    print(join(t4) == @A@);
    let t5 = spawn(fn() -> [@T@] { [@A@, @B@] });
    print(join(t5).len());
    let t6 = spawn(fn() -> Option<@T@> { Option.Some(@B@) });
    print(join(t6) == Option.Some(@B@));
    let t7 = spawn(fn() -> Result<@T@, string> { Result.Err("x") });
    print(join(t7).is_err());
    let t9 = spawn(fn() -> (@T@, int) { (@B@, @K@) });
    let got = join(t9);
    print(got.1);
    print(got.0 == @B@);
    let out: Channel<@T@> = Channel.bounded(2);
    let items = [@B@, @A@];
    let t8 = spawn(fn() {
        for x in items { send(out, x); }
    });
    join(t8);
    print(recv(out) == Option.Some(@B@));
    print(recv(out) == Option.Some(@A@));
    let total = scope(fn() -> int {
        let child = spawn(fn() -> int { if (a == @B@) { 1 } else { 0 } });
        let other = spawn(fn() -> [@T@] { work_@I@(job) });
        join(child) + join(other).len() + @K@
    });
    print(total);
    match (try_join(spawn(fn() -> @T@ {
        if (@K@ > 0) { panic("boom@I@"); }
        @A@
    }))) {
        Result.Ok(_v) => print("ok"),
        Result.Err(e) => print("caught:${e}"),
    }
}
"#;
    (vec![], fill(tpl, i, &t, r.int_in(1, 9)))
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
    // La etiqueta entra en el nombre: dos batches con la MISMA semilla (los dos humos, que cargo
    // corre en paralelo) no deben compartir directorio ni pisarse el programa.
    let slug: String = label.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let dir = std::env::temp_dir().join(format!("ray_differential_{}_{seed}_{slug}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("crea el dir temporal");
    let probes = make_batch(gens, seed);
    let result = check(&probes, flavor, &dir, "full");
    if let Err(first_err) = result {
        let (minimal, err) = bisect(probes, flavor, &dir);
        let reproduction = std::env::temp_dir().join(format!("ray_diff_repro_{seed}_{slug}.ray"));
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

/// Humo ESTRUCTURAL en cada `cargo test`: un batch con un tipo sorteado por generador.
#[test]
fn differential_smoke_one_structural_batch() {
    if !require_rustc() {
        return;
    }
    run_batch(SEQ_STRUCT_GENS, Flavor::Sequential, BASE_SEED, "humo estructural");
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
        run_batch(SEQ_STRUCT_GENS, Flavor::Sequential, seed, "repro estructural");
        run_batch(CONC_GENS, Flavor::Concurrent, seed, "repro concurrente");
        run_batch(CONC_STRUCT_GENS, Flavor::Concurrent, seed, "repro estructural concurrente");
        return;
    }
    for b in 0..batches {
        let seed = BASE_SEED.wrapping_add((b + 1).wrapping_mul(0x9E37_79B9));
        run_batch(SEQ_GENS, Flavor::Sequential, seed, "campaña secuencial");
        // Los estructurales rinden por SEMILLA (cada una sortea otros tipos): el doble de batches.
        run_batch(SEQ_STRUCT_GENS, Flavor::Sequential, seed, "campaña estructural");
        run_batch(SEQ_STRUCT_GENS, Flavor::Sequential, seed ^ 0x5DEE_CE66, "campaña estructural");
    }
    // Concurrencia: menos batches (el build con fibras va por la vía Cargo, ~5-10 s cada uno).
    let conc_batches = (batches / 2).max(1);
    for b in 0..conc_batches {
        let seed = BASE_SEED.wrapping_add((b + 1).wrapping_mul(0x51_7C_C1_B7));
        run_batch(CONC_GENS, Flavor::Concurrent, seed, "campaña concurrente");
        run_batch(CONC_STRUCT_GENS, Flavor::Concurrent, seed, "campaña estructural concurrente");
    }
}

/// SONDEO estructural (herramienta de desarrollo, `#[ignore]`): cada generador estructural, de
/// uno en uno y con `RAYLANG_DIFF_BATCHES` semillas (por defecto 3), SIN parar en el primer fallo.
/// Lista todo lo que diverge o no compila, con su repro en disco, y falla al final si hubo algo.
/// `RAYLANG_DIFF_ONLY=<texto>` filtra los generadores por nombre.
#[test]
#[ignore = "sondeo largo: un build nativo por sonda; correr con -- --ignored"]
fn differential_structural_survey() {
    if !require_rustc() {
        return;
    }
    let seeds = env_u64("RAYLANG_DIFF_BATCHES", 3);
    let only = std::env::var("RAYLANG_DIFF_ONLY").unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("ray_differential_survey_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("crea el dir temporal");
    let mut failures: Vec<String> = Vec::new();
    let lists: [(&[(&'static str, GenFn)], Flavor); 2] =
        [(SEQ_STRUCT_GENS, Flavor::Sequential), (CONC_STRUCT_GENS, Flavor::Concurrent)];
    for (gens, flavor) in lists {
        for (idx, (name, g)) in gens.iter().enumerate() {
            if !name.contains(only.as_str()) {
                continue;
            }
            for s in 0..seeds {
                let seed = BASE_SEED.wrapping_add((s + 1).wrapping_mul(0x9E37_79B9)).wrapping_add(idx as u64);
                let mut rng = Rng(seed);
                let (imports, top) = g(&mut rng, idx);
                let probe = Probe { imports, top, idx, gen_name: name };
                let tag = format!("{name}_{s}");
                let outcome = std::panic::catch_unwind(|| check(std::slice::from_ref(&probe), flavor, &dir, &tag));
                let err = match outcome {
                    Ok(Ok(())) => continue,
                    Ok(Err(e)) => e,
                    Err(p) => p.downcast_ref::<String>().cloned().unwrap_or_else(|| "pánico del harness".to_string()),
                };
                let head: String = err.lines().filter(|l| !l.trim().is_empty()).take(7).collect::<Vec<_>>().join("\n    ");
                failures.push(format!("{name} (semilla {s}) → {}\n    {head}", dir.join(format!("diff_{tag}.ray")).display()));
            }
        }
    }
    if failures.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    panic!("{} sonda(s) estructurales fallan:\n\n{}", failures.len(), failures.join("\n\n"));
}
