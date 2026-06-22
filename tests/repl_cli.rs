//! Pruebas del REPL (M8.2) como **cliente externo**: lanzan el binario `raylang --repl`,
//! le pasan entrada por stdin y comprueban lo que imprime. Verifican que el REPL funciona
//! usando solo la interfaz pública (no toca el checker ni el intérprete por dentro).

use std::io::Write;
use std::process::{Command, Stdio};

/// Ejecuta el REPL con `input` en stdin y devuelve su stdout.
fn repl(input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza el binario raylang");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("escribe en stdin");
    let out = child.wait_with_output().expect("espera al proceso");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn imprime_valores_y_persiste_estado() {
    let out = repl(
        "1 + 2\n\
         let x = 10\n\
         x * x\n\
         [1, 2, 3]\n\
         fn doble(n: int) -> int { n * 2 }\n\
         doble(x)\n\
         :quit\n",
    );
    assert!(out.contains("> 3"), "1+2 -> 3\n{out}");
    assert!(out.contains("> 10"), "let x = 10 imprime 10\n{out}");
    assert!(out.contains("> 100"), "x*x -> 100\n{out}");
    assert!(out.contains("[1, 2, 3]"), "literal de arreglo\n{out}");
    assert!(out.contains("definida 'doble'"), "definición\n{out}");
    assert!(out.contains("> 20"), "doble(x) -> 20\n{out}");
}

#[test]
fn ufcs_pipelines_y_structs_en_el_repl() {
    let out = repl(
        "struct Punto { x: int, y: int }\n\
         let p = Punto { x: 7, y: 6 }\n\
         p.x + p.y\n\
         fn doble(n: int) -> int { n * 2 }\n\
         5.doble()\n\
         5 |> doble |> doble\n\
         :quit\n",
    );
    assert!(out.contains("definida 'Punto'"), "{out}");
    assert!(out.contains("> 13"), "p.x + p.y -> 13\n{out}");
    assert!(out.contains("> 10"), "UFCS 5.doble() -> 10\n{out}");
    assert!(out.contains("> 20"), "pipeline 5|>doble|>doble -> 20\n{out}");
}

#[test]
fn un_error_no_tumba_el_repl_ni_pierde_estado() {
    let out = repl(
        "let x = 3\n\
         1 + true\n\
         x + 1\n\
         :quit\n",
    );
    // El error sale por stderr; el REPL sigue vivo y 'x' persiste -> imprime 4.
    assert!(out.contains("> 3"), "let x = 3 imprime 3\n{out}");
    assert!(out.contains("> 4"), "tras el error, x+1 -> 4\n{out}");
}
