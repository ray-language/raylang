//! Pruebas de **std/term** (M107.3). Sin terminal en CI, se prueba lo probable:
//!
//! - el DECODIFICADOR de teclas (`term.decode`), que es puro (bytes → tecla) a propósito: la
//!   batería (`tests/fixtures/term_decoder.ray`) cubre ASCII, controles, CSI (flechas/Home/End/
//!   `~`-teclas/F1..F12/modificadores), SS3, Shift-Tab, UTF-8 de 2/3/4 octetos y los prefijos
//!   INCOMPLETOS — en los tres motores, contra salida exacta;
//! - los caminos sin-tty: `is_tty` false, `size` None, y `raw` que falla LIMPIO sin correr `f`.
//!
//! El modo crudo real (termios) no se puede ejercitar bajo pipes: su smoke es manual
//! (`examples/term/keys.ray` en una terminal).

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_term_{name}"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("lanza ray");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Corre `src` en VM + intérprete + (si hay rustc) nativo, y asevera la salida EXACTA en los tres.
fn assert_on_all_engines(name: &str, src: &str, want: &str) {
    let base = tmp(name);
    std::fs::write(base.join("prog.ray"), src).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, err, code) = ray(&base, &[engine, "prog.ray"]);
        assert_eq!(code, 0, "{engine}: exit 0\n{err}");
        assert_eq!(out, want, "{engine}: salida exacta");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin");
        let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(bcode, 0, "build --native ok\n{berr}");
        let native = Command::new(&bin).stdin(std::process::Stdio::null()).output().expect("nativo");
        assert_eq!(String::from_utf8_lossy(&native.stdout), want, "nativo ≡ VM");
        assert_eq!(native.status.code(), Some(0));
    }
}

#[test]
fn decoder_battery_matches_on_all_three_engines() {
    assert_on_all_engines(
        "decoder",
        include_str!("fixtures/term_decoder.ray"),
        include_str!("fixtures/term_decoder.out"),
    );
}

#[test]
fn without_a_tty_everything_degrades_cleanly() {
    // Bajo pipes (stdin /dev/null, stdout capturado): nada es tty → is_tty false, size None, y
    // `raw` devuelve Err SIN correr f (una TUI exige terminal; si f corriera, saldría "CORRIO").
    assert_on_all_engines(
        "no_tty",
        include_str!("fixtures/term_no_tty.ray"),
        "false\nno-size\nraw-err\n",
    );
}

#[test]
fn cell_width_matches_on_all_three_engines() {
    // M117: term.width / char_width / fit / fit_right (wcwidth pragmático) — byte-idéntico en los
    // tres motores (es raylang puro, se transpila; sin opcode nuevo).
    assert_on_all_engines(
        "cell_width",
        include_str!("fixtures/term_width.ray"),
        include_str!("fixtures/term_width.out"),
    );
}

#[test]
fn hidden_input_core_matches_on_all_three_engines() {
    // M125: el núcleo PURO de la entrada oculta (hidden_feed: Enter/backspace-por-carácter/
    // Ctrl-C/controles ignorados) + la degradación sin tty de read_hidden.
    assert_on_all_engines(
        "hidden",
        include_str!("fixtures/term_hidden.ray"),
        include_str!("fixtures/term_hidden.out"),
    );
}

/// M125: el camino REAL — un pty vía `script -q` (la receta de las TUIs). Se ESPERA el prompt
/// antes de teclear (term.raw usa TCSAFLUSH, que descarta la entrada pendiente: teclear antes de
/// que el modo crudo esté puesto perdería los bytes y colgaría la lectura). Se teclea
/// "sécrX<backspace>eto<Enter>": el programa debe devolver "sécreto" (el backspace borra la X, la
/// "é" multibyte cruza intacta) y NADA de lo tecleado debe aparecer en la salida (sin eco).
/// Solo VM (la batería pura de arriba ya cubre la edición en los tres motores).
#[test]
fn read_hidden_under_a_real_pty() {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    if Command::new("script").arg("--version").output().is_err() && Command::new("script").output().is_err() {
        eprintln!("saltando read_hidden_under_a_real_pty: no hay `script`");
        return;
    }
    let dir = tmp("hidden_pty");
    let main = dir.join("main.ray");
    std::fs::write(
        &main,
        r#"import std/term;

fn main() {
    match (term.read_hidden("pass: ")) {
        Result.Ok(s) => print("got=" + s),
        Result.Err(e) => print("err=" + e),
    }
}
"#,
    )
    .unwrap();
    let bin = env!("CARGO_BIN_EXE_raylang");
    let mut cmd = if cfg!(target_os = "linux") {
        // util-linux: script -q -e -c "cmd" /dev/null
        let inner = format!("{} --vm {}", bin, main.display());
        let mut c = Command::new("script");
        c.args(["-q", "-e", "-c", &inner, "/dev/null"]);
        c
    } else {
        // BSD/macOS: script -q /dev/null cmd args...
        let mut c = Command::new("script");
        c.arg("-q").arg("/dev/null").arg(bin).arg("--vm").arg(&main);
        c
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanza script(pty)");
    // Hilo lector: acumula TODO el stdout del pty (prompt incluido — stderr del hijo también
    // desemboca en el pty).
    let mut stdout = child.stdout.take().expect("stdout");
    let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let sink = std::sync::Arc::clone(&collected);
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    // Espera el prompt (→ el modo crudo YA está puesto: el ewrite del prompt va antes de raw,
    // pero el margen extra de abajo cubre el tcsetattr inmediato) y entonces teclea.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if collected.lock().unwrap().windows(6).any(|w| w == b"pass: ") {
            break;
        }
        assert!(Instant::now() < deadline, "el prompt nunca llegó");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300)); // margen: prompt visto → raw ya aplicado
    let typed = b"s\xc3\xa9crX\x7feto\r";
    child.stdin.as_mut().unwrap().write_all(typed).expect("teclea");
    let _ = child.stdin.as_mut().unwrap().flush();
    drop(child.stdin.take());
    // Espera acotada a que el hijo termine.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let all = String::from_utf8_lossy(&collected.lock().unwrap()).into_owned();
                    panic!("el pty no terminó a tiempo; salida hasta ahora: {all:?}");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("wait: {e}"),
        }
    }
    let _ = reader.join();
    let all = String::from_utf8_lossy(&collected.lock().unwrap()).into_owned();
    assert!(all.contains("got=sécreto"), "el resultado editado debe llegar: {all:?}");
    assert!(!all.contains("sécrX"), "lo tecleado no debe tener eco: {all:?}");
}
