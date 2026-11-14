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
