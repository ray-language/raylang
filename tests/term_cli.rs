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

#[test]
fn size_px_and_cell_px_are_none_without_a_terminal() {
    // M143 (IDEAS §78): sin tty (stdout canalizado) el área en píxeles es None en los TRES
    // motores — el camino determinista de CI; el camino Some depende del terminal real.
    assert_on_all_engines(
        "size_px",
        "import std/term;\n\nfn main() {\n    match (term.size_px()) {\n        Option.Some(p) => print(\"px \" + to_string(p.0) + \"x\" + to_string(p.1)),\n        Option.None => print(\"no px\"),\n    }\n    match (term.cell_px()) {\n        Option.Some(c) => print(\"cell \" + to_string(c.0) + \"x\" + to_string(c.1)),\n        Option.None => print(\"no cell\"),\n    }\n}\n",
        "no px\nno cell\n",
    );
}

#[test]
fn capabilities_from_env_match_on_all_three_engines() {
    // M143b (IDEAS §78): con el env CONTROLADO y sin tty (stdout canalizado → sin query DA1),
    // capabilities() es determinista; parse_device_attributes es pura. Byte-idéntico en los 3.
    let src = "import std/term;\n\nfn main() {\n    print(term.parse_device_attributes(b\"\\x1b[?64;1;4c\"));\n    print(term.parse_device_attributes(b\"\\x1b[?1;2c\"));\n    print(term.parse_device_attributes(b\"garbage\"));\n    print(term.parse_device_attributes(b\"\\x1b[?64;1;4\"));\n    let c = term.capabilities();\n    print(\"tc=\" + to_string(c.truecolor) + \" c256=\" + to_string(c.colors_256) + \" sixel=\" + to_string(c.sixel) + \" kitty=\" + to_string(c.kitty_graphics));\n}\n";
    let want = "[64, 1, 4]\n[1, 2]\n[]\n[]\ntc=true c256=true sixel=false kitty=false\n";
    let env = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor"), ("KITTY_WINDOW_ID", "")];
    let base = tmp("caps_env");
    std::fs::write(base.join("prog.ray"), src).unwrap();
    let run = |cmd: &mut Command| {
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.current_dir(&base).stdin(std::process::Stdio::null()).output().expect("corre");
        (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
    };
    for engine in ["--vm", "--interp"] {
        let (out, code) = run(Command::new(env!("CARGO_BIN_EXE_ray")).args([engine, "prog.ray"]));
        assert_eq!(code, 0, "{engine}: exit 0");
        assert_eq!(out, want, "{engine}: salida exacta");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin");
        let (_o, _e, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(bcode, 0, "build --native ok");
        let (out, code) = run(&mut Command::new(&bin));
        assert_eq!(code, 0, "nativo: exit 0");
        assert_eq!(out, want, "nativo ≡ VM");
    }
}

/// M143c (hallazgo de rallyx): `capabilities()` DENTRO de `term.raw` dejaba el terminal cocinado
/// — `raw()` no era reentrante y el `raw_off` interno de la query DA1 restauraba a mitad de la
/// sesión exterior (teclas muertas). El repro: dentro de raw, capabilities() y luego leer UNA
/// tecla sin Enter — en cooked el byte se queda en el buffer canónico y el read_timeout vence.
#[cfg(unix)]
#[test]
fn capabilities_inside_raw_keeps_the_terminal_raw() {
    // Necesita un pty: script(1). Si no está, el test se salta con aviso (no hay pty portable).
    if Command::new("script").arg("--version").output().is_err()
        && !std::path::Path::new("/usr/bin/script").exists()
    {
        eprintln!("skipping: script(1) not available");
        return;
    }
    let base = tmp("raw_reentrant");
    let prog = "import std/term;\nimport std/io;\n\nfn main() {\n    let r = term.raw(fn() -> string {\n        let c = term.capabilities();\n        let _ = c;\n        match (io.read_timeout(1, 3000)) {\n            io.ReadResult.Data(b) => \"alive \" + to_string(b[0]),\n            io.ReadResult.Eof => \"eof\",\n            io.ReadResult.TimedOut => \"dead keys\",\n        }\n    });\n    match (r) {\n        Result.Ok(s) => print(s),\n        Result.Err(e) => print(\"err \" + e),\n    }\n}\n";
    std::fs::write(base.join("prog.ray"), prog).unwrap();
    let run_in_pty = |cmdline: &str| -> String {
        // La 'x' va SIN Enter tras un respiro (raw entra y la query DA1 vence su plazo).
        let feeder = format!("(sleep 1; printf x; sleep 2) | {}", pty_wrap(cmdline));
        let out = Command::new("sh").args(["-c", &feeder]).current_dir(&base).output().expect("pty");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    for engine in ["--vm", "--interp"] {
        let out = run_in_pty(&format!("{} {engine} prog.ray", env!("CARGO_BIN_EXE_ray")));
        assert!(
            out.contains("alive 120"),
            "{engine}: la tecla debe llegar cruda tras capabilities() (salida:\n{out})"
        );
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin");
        let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(bcode, 0, "build --native ok\n{berr}");
        let out = run_in_pty(bin.to_str().unwrap());
        assert!(out.contains("alive 120"), "nativo: tecla cruda tras capabilities():\n{out}");
    }
}

/// Envuelve un comando en script(1) — la sintaxis difiere entre macOS/BSD y util-linux.
#[cfg(unix)]
fn pty_wrap(cmdline: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("script -q /dev/null {cmdline}")
    } else {
        format!("script -qec \"{cmdline}\" /dev/null")
    }
}

/// M143d (hallazgo de raycode): en el binario NATIVO, print encola en el hilo escritor (M96f) y
/// `term.raw` cambiaba el termios sin drenar — la salida encolada en modo cocido se escribía ya
/// dentro de la SIGUIENTE sesión raw (`\n` sin `\r`: escalera intermitente, el primer /help de
/// raycode). El repro que la caza es la ALTERNANCIA raw→prints→raw en varias rondas (medido sin
/// el fix: 6 de 18 líneas con `\r`); con el drenado en `__ray_term_raw`, 18 de 18.
#[cfg(unix)]
#[test]
fn native_prints_between_raw_sessions_carry_the_carriage_return() {
    if Command::new("script").arg("--version").output().is_err()
        && !std::path::Path::new("/usr/bin/script").exists()
    {
        eprintln!("skipping: script(1) not available");
        return;
    }
    if !Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skipping: rustc not available");
        return;
    }
    let base = tmp("raw_flush");
    let prog = "import std/term;\nimport std/io;\n\nfn round(i: int) {\n    let r = term.raw(fn() -> string {\n        match (io.read_timeout(1, 30)) {\n            io.ReadResult.Data(_) => \"d\",\n            io.ReadResult.Eof => \"e\",\n            io.ReadResult.TimedOut => \"t\",\n        }\n    });\n    let _ = r;\n    print(\"r\" + to_string(i) + \"a\");\n    print(\"r\" + to_string(i) + \"b\");\n    print(\"r\" + to_string(i) + \"c\");\n}\n\nfn main() {\n    var i = 0;\n    while (i < 6) {\n        round(i);\n        i = i + 1;\n    }\n}\n";
    std::fs::write(base.join("prog.ray"), prog).unwrap();
    let bin = base.join("prog_bin");
    let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(bcode, 0, "build --native ok\n{berr}");
    let feeder = format!("(sleep 2) | {}", pty_wrap(bin.to_str().unwrap()));
    let out = Command::new("sh").args(["-c", &feeder]).current_dir(&base).output().expect("pty");
    let bytes = out.stdout;
    // TODAS las líneas impresas en modo cocido deben llevar el \r de ONLCR: cada una sin él es
    // salida que el escritor soltó ya dentro del raw de la ronda siguiente.
    let crlf = bytes.windows(2).filter(|w| w == b"\r\n").count();
    let lf = bytes.iter().filter(|&&b| b == b'\n').count();
    assert_eq!(crlf, lf, "lineas en escalera: {crlf} de {lf} con retorno de carro\n{:?}", String::from_utf8_lossy(&bytes));
    assert_eq!(lf, 18, "las 18 lineas del programa\n{:?}", String::from_utf8_lossy(&bytes));
}

/// Regresión (dogfood raydesk, 31 ago 2026): `atexit` no corre ante una señal fatal — un
/// SIGTERM (p. ej. `ray dev` relanzando la app al guardar) mataba una TUI en crudo y el
/// terminal quedaba envenenado (escalera); el hijo siguiente guardaba ese termios crudo como
/// "original" y lo perpetuaba. Ahora `raw_on` arma un handler que restaura y re-lanza: tras
/// matar con SIGTERM a un programa en raw, el termios del pty debe volver a cooked (icanon).
#[cfg(unix)]
#[test]
fn sigterm_during_raw_restores_the_terminal() {
    if Command::new("script").arg("--version").output().is_err()
        && !std::path::Path::new("/usr/bin/script").exists()
    {
        eprintln!("skipping: script(1) not available");
        return;
    }
    let base = tmp("raw_sigterm");
    let prog = "import std/term;\nimport std/time;\n\nfn main() {\n    let _ = term.raw(fn() -> int {\n        print(\"in-raw\");\n        time.sleep(10000);\n        0\n    });\n}\n";
    std::fs::write(base.join("prog.ray"), prog).unwrap();
    // El runner corre DENTRO del pty: lanza el programa, lo mata en pleno raw y vuelca el
    // termios resultante del pty a un archivo (la salida del pty en sí no es fiable). OJO:
    // un job `&` de sh hereda stdin de /dev/null — sin el `< /dev/tty` el programa no vería
    // terminal y el test no probaría nada (el print "in-raw" asevera que SÍ entró).
    let runner = format!(
        "'{ray}' --vm prog.ray < /dev/tty &\npid=$!\nsleep 1\nkill -TERM $pid\nwait $pid 2>/dev/null\nstty -a > result.txt 2>&1\n",
        ray = env!("CARGO_BIN_EXE_ray"),
    );
    std::fs::write(base.join("runner.sh"), runner).unwrap();
    let cmd = pty_wrap("sh runner.sh");
    let out = Command::new("sh").args(["-c", &cmd]).current_dir(&base).output().expect("pty");
    assert!(out.status.success(), "pty run ok: {:?}", String::from_utf8_lossy(&out.stderr));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("in-raw"),
        "el programa debió entrar en raw dentro del pty:\n{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stty = std::fs::read_to_string(base.join("result.txt")).unwrap_or_default();
    assert!(stty.contains("icanon"), "stty reporta el termios del pty:\n{stty}");
    assert!(
        !stty.contains("-icanon"),
        "el terminal quedó en crudo tras el SIGTERM (icanon apagado):\n{stty}"
    );
}
