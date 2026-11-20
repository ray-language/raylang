//! Ronda de hallazgos urgentes sobre 1.27.95 (#352, #353, #359, #388): cada caso da lo mismo
//! en la VM y en el intérprete, y lo que el hallazgo esperaba.

use std::process::Command;

/// Corre `src` en la VM y en el intérprete: misma salida, mismo error, mismo código de salida.
/// Devuelve `(stdout, stderr)` de la VM.
fn same_on_both(name: &str, src: &str) -> (String, String) {
    let d = std::env::temp_dir().join(format!("ray_round_{name}_{}", std::process::id()));
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
fn a_composite_expression_coerces_to_dyn_as_a_whole() {
    // #353: la coerción se registra por posición, y `f(x)`, `b.sq` y `xs[0]` la comparten con su
    // hijo izquierdo. Se envolvía también ese hijo: ICE con la llamada y el índice, y con el
    // campo un valor equivocado sin error (`square Square { side: 3 }`).
    let (out, err) = same_on_both(
        "dyn",
        r#"trait Shape { fn name(self) -> string; }
struct Square { side: int, }
impl Shape for Square { fn name(self) -> string { "square ${self.side}" } }
struct Holder { sq: Square, }
fn square(n: int) -> Square { Square { side: n } }
fn describe(s: dyn Shape) -> string { "<" + s.name() + ">" }
fn made() -> Result<Square, string> { Result.Ok(square(7)) }
fn tried() -> Result<string, string> {
    let s: dyn Shape = made()?;
    Result.Ok(s.name())
}
fn main() -> int {
    let s: dyn Shape = square(2);
    print(s.name());
    let h = Holder { sq: square(3) };
    let f: dyn Shape = h.sq;
    print(f.name());
    let arr = [square(4)];
    let g: dyn Shape = arr[0];
    print(g.name());
    print(describe(square(5)));
    print(describe(h.sq));
    let xs: [dyn Shape] = [square(6), h.sq, arr[0]];
    print(xs[0].name() + xs[1].name() + xs[2].name());
    print(tried().unwrap());
    0
}
"#,
    );
    assert_eq!(err, "");
    assert_eq!(out, "square 2\nsquare 3\nsquare 4\n<square 5>\n<square 3>\nsquare 6square 3square 4\nsquare 7\n");
}

#[test]
fn a_for_over_an_iterator_with_a_bounded_impl_gets_its_dictionaries() {
    // #352: el `next` de un `impl<V: Ord> Iterator<V>` lleva parámetros-diccionario, y el `for`
    // lo llamaba plano: ICE en la VM. Con `continue`, `break`, etiqueta, patrón de tupla y
    // dentro de una función genérica que reenvía su propio diccionario.
    let (out, err) = same_on_both(
        "bounded_iter",
        r#"struct Above<V> { xs: [V], i: int, min: V, }
impl<V: Ord> Iterator<V> for Above<V> {
    fn next(self) -> Option<V> {
        while (self.i < self.xs.len()) {
            let v = self.xs[self.i];
            self.i = self.i + 1;
            if (self.min.less(v)) { return Option.Some(v); }
        }
        Option.None
    }
}
struct Pairs<V> { xs: [V], i: int, }
impl<V: Show> Iterator<(int, string)> for Pairs<V> {
    fn next(self) -> Option<(int, string)> {
        if (self.i >= self.xs.len()) { return Option.None; }
        self.i = self.i + 1;
        Option.Some((self.i, self.xs[self.i - 1].show()))
    }
}
fn above<V: Ord>(xs: [V], min: V) -> int {
    var n = 0;
    let it = Above { xs: xs, i: 0, min: min };
    for v in it { n = n + 1; }
    n
}
fn main() -> int {
    let it = Above { xs: [1, 5, 2, 7, 9], i: 0, min: 3 };
    for v in it {
        if (v == 7) { continue; }
        print(v);
    }
    let words = Above { xs: ["a", "z", "m"], i: 0, min: "b" };
    outer: for w in words {
        for k in 0..3 {
            if (k == 1) { continue outer; }
            print("${w}${k}");
        }
    }
    let ps = Pairs { xs: [10, 20], i: 0 };
    for (i, s) in ps { print("${i}:${s}"); }
    let early = Above { xs: [4, 5, 6], i: 0, min: 0 };
    for v in early { if (v == 5) { break; } print(v); }
    print(above([1.5, 2.5, 0.5], 1.0));
    0
}
"#,
    );
    assert_eq!(err, "");
    assert_eq!(out, "5\n9\nz0\nm0\n1:10\n2:20\n4\n2\n");
}

#[test]
fn print_and_to_string_use_the_show_of_the_user() {
    // #359: `print` mostraba la forma por defecto (`Plain { n: 1 }`) aunque el tipo tuviera
    // `impl Show`, y `to_string(p)`/`"${p}"` rechazaban un `p: P` con `P: Show`. Un tipo sin
    // `impl Show` conserva la forma por defecto.
    let (out, err) = same_on_both(
        "show",
        r#"struct Plain { n: int, }
impl Show for Plain { fn show(self) -> string { "P${self.n}" } }
struct Boxed<T> { v: T, }
impl<T: Show> Show for Boxed<T> { fn show(self) -> string { "B(${self.v.show()})" } }
struct Raw { n: int, }
enum Color { Red, Rgb(int), }
impl Show for Color {
    fn show(self) -> string {
        match (self) { Color.Red => "red", Color.Rgb(n) => "rgb ${n}", }
    }
}
fn label<P: Show>(p: P) -> string { to_string(p) }
fn quoted<P: Show>(p: P) -> string { "<${p}>" }
fn shout<P: Show>(p: P) { print(p); }
fn main() -> int {
    print(Plain { n: 1 });
    print(Boxed { v: 2 });
    print(Boxed { v: Plain { n: 3 } });
    print([Plain { n: 4 }, Plain { n: 5 }]);
    print(Option.Some(Plain { n: 6 }));
    print(Color.Rgb(7));
    print(Raw { n: 8 });
    print([Raw { n: 9 }]);
    print(Option.Some(3));
    print(label(Plain { n: 10 }));
    print(label(11));
    print(quoted(Color.Red));
    print(quoted("s"));
    shout(Plain { n: 12 });
    shout(1.5);
    0
}
"#,
    );
    assert_eq!(err, "");
    assert_eq!(out, "P1\nB(2)\nB(P3)\n[P4, P5]\nOption.Some(P6)\nrgb 7\nRaw { n: 8 }\n[Raw { n: 9 }]\nOption.Some(3)\nP10\n11\n<red>\n<s>\nP12\n1.5\n");
}

#[test]
fn protobuf_parse_is_total_on_malformed_input() {
    // #388: un varint o un fixed cortado abortaba el programa (índice fuera de rango), un
    // length-delimited cortado daba `Ok` con menos bytes, el campo 0 se aceptaba y un varint
    // de 11 octetos daba `Ok(-1)` en la VM y un panic en nativo. Todo es `Err` con el byte.
    // #463: un varint de 10 octetos cuyo décimo octeto pasa de 1 no cabe en 64 bits (se
    // aceptaba descartando los bits de más), y el error del tipo wire sigue el formato del resto.
    let (out, err) = same_on_both(
        "protobuf",
        r#"import std/protobuf;
fn show(data: [int]) {
    match (protobuf.parse(bytes_of(data))) {
        Result.Ok(fs) => print("ok ${fs.len()} ${protobuf.get_int(fs, 1)} ${protobuf.get_bytes(fs, 1).len()}"),
        Result.Err(e) => print(e),
    }
}
fn main() -> int {
    show([0x08, 0x80]);
    show([0x0A, 0x05, 0x41]);
    show([0x00, 0x01]);
    show([0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
    show([0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
    show([0x09, 0x01, 0x02]);
    show([0x0D, 0x01]);
    show([0x0A, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x41]);
    show([0x0B]);
    show([0x08, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02]);
    show([0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
    show([0x80]);
    show([0x08, 0x96, 0x01]);
    show([0x0A, 0x02, 0x41, 0x42]);
    show([0x09, 1, 0, 0, 0, 0, 0, 0, 0]);
    show([0x0D, 2, 0, 0, 0]);
    show([]);
    let w = protobuf.writer();
    protobuf.write_varint(w, 1, 300);
    protobuf.write_string(w, 2, "hola");
    let fs = protobuf.parse(protobuf.finish(w)).unwrap();
    print("${protobuf.get_int(fs, 1)} ${protobuf.get_string(fs, 2)}");
    0
}
"#,
    );
    assert_eq!(err, "");
    assert_eq!(out, "protobuf: truncated varint of the value of field 1 at byte 1\nprotobuf: field 1 at byte 0 announces 5 bytes and 1 remain\nprotobuf: invalid field number 0 at byte 0\nprotobuf: the varint of the value of field 1 at byte 1 is longer than 10 bytes\nok 1 -1 0\nprotobuf: truncated fixed64 of field 1 at byte 1\nprotobuf: truncated fixed32 of field 1 at byte 1\nprotobuf: field 1 at byte 0 announces -1 bytes and 1 remain\nprotobuf: unsupported wire type 3 of field 1 at byte 0\nprotobuf: the varint of the value of field 1 at byte 1 does not fit in 64 bits\nprotobuf: the varint of the value of field 1 at byte 1 does not fit in 64 bits\nprotobuf: truncated varint of the tag at byte 0\nok 1 150 0\nok 1 0 2\nok 1 1 0\nok 1 2 0\nok 0 0 0\n300 hola\n");
}

#[test]
fn the_result_of_a_ufcs_call_or_an_operator_coerces_to_dyn() {
    // Secuela de #353 (revisión de 1.27.99): UFCS y los operadores sobrecargados bajan con el
    // receptor en `args[0]`, en la MISMA posición que la llamada. Se envolvía también el
    // receptor y la función recibía el trait object donde esperaba el tipo concreto (ICE).
    let (out, err) = same_on_both(
        "dyn_ufcs",
        r#"trait Shape { fn area(self) -> int; }
struct Sq { s: int, }
impl Shape for Sq { fn area(self) -> int { self.s * self.s } }
impl Add for Sq { fn add(self, o: Sq) -> Sq { Sq { s: self.s + o.s } } }
struct Foo { n: int, }
fn make(f: Foo) -> Sq { Sq { s: f.n } }
fn grow(q: Sq, n: int) -> Sq { Sq { s: q.s + n } }
fn total(s: dyn Shape) -> int { s.area() }
fn main() -> int {
    let foo = Foo { n: 3 };
    let q = Sq { s: 2 };
    let d: dyn Shape = foo.make();
    print(d.area());
    print(total(q.grow(1)));
    let e: dyn Shape = q + q;
    print(e.area());
    let xs: [dyn Shape] = [q.grow(2), q];
    print(xs[0].area() + xs[1].area());
    let c: dyn Shape = q.grow(1).grow(1);
    print(c.area());
    print(total(grow(q, 1)));
    0
}
"#,
    );
    assert_eq!(err, "");
    assert_eq!(out, "9\n9\n16\n20\n16\n9\n");
}

#[test]
fn to_string_on_a_type_parameter_only_follows_the_show_bound() {
    // Revisión de 1.27.99: `"${x}"` con `x: T` seguía CUALQUIER trait acotado con un método
    // `show` y tomaba su tipo de retorno: un `trait Labeled { fn show(self) -> int; }` daba un
    // error de `+` entre string e int en vez de decir que `T` no se convierte.
    let (out, err) = same_on_both(
        "show_bound",
        r#"trait Labeled { fn show(self) -> int; }
fn lab<T: Labeled>(x: T) -> string { "v=${x}" }
fn main() -> int { 0 }
"#,
    );
    assert_eq!(out, "");
    assert!(err.contains("to_string only converts") && err.contains("not T"), "{err}");
}

#[test]
fn a_derive_error_names_only_the_fields_at_fault() {
    // Revisión de 1.27.99: el campo se buscaba por subcadena y `ok: In` salía culpado porque
    // `In` está dentro de `Inner`.
    let (_, err) = same_on_both(
        "derive_fields",
        r#"struct Inner { n: int, }
@derive(Show)
struct In { n: int, }
@derive(Show)
struct Outer { a: Inner, ok: In, xs: [In], }
fn main() -> int { 0 }
"#,
    );
    assert!(err.contains("cannot derive Show for 'Outer' (field 'a'):"), "{err}");
}
