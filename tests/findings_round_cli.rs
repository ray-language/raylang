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
