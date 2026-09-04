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
/// El esperado se normaliza a LF: los fixtures `.out` llegan con CRLF en un checkout de Windows
/// con `core.autocrlf=true` (el runner de CI, M166) y `include_str!` los embebe tal cual.
fn assert_on_all_engines(name: &str, src: &str, want: &str) {
    let want: &str = &want.replace("\r\n", "\n");
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

// M161 — gráficos kitty: el núcleo puro (chunking + parser de la respuesta APC) es
// determinista y corre byte-idéntico en los 3 motores, sin terminal.
#[test]
fn kitty_graphics_pure_core_matches_on_all_three_engines() {
    let src = r#"import std/term;

// Ocurrencias de "ESC _ G" (los APC emitidos): split cuenta separadores.
fn apcs(s: string) -> int {
    s.split("\u{1B}_G").len() - 1
}

fn main() {
    // Sin payload: un solo APC de control, sin ';' ni 'm='.
    let d = term.kitty_chunks("a=d,d=a,q=2", b"");
    print("delete: " + to_string(d == "\u{1B}_Ga=d,d=a,q=2\u{1B}\\"));
    // Payload corto: un APC con ';' y el base64 completo.
    let one = term.kitty_chunks("a=t,i=1,q=2", b"\x00\x00\x00");
    print("one: " + to_string(one == "\u{1B}_Ga=t,i=1,q=2;AAAA\u{1B}\\"));
    // 3073 octetos -> 2 chunks: el 1º con el control entero + m=1, el 2º SOLO m=0.
    var big: [int] = [];
    var i = 0;
    while (i < 3073) {
        big.push(65);
        i = i + 1;
    }
    let two = term.kitty_chunks("a=t,i=2,q=2", bytes_of(big));
    print("apcs: " + to_string(apcs(two)));
    print("head: " + to_string(two.contains("\u{1B}_Ga=t,i=2,q=2,m=1;")));
    print("tail: " + to_string(two.contains("\u{1B}_Gm=0;")));
    // 6145 octetos -> 3 chunks; el intermedio SOLO m=1.
    var bigger: [int] = [];
    var j = 0;
    while (j < 6145) {
        bigger.push(0);
        j = j + 1;
    }
    let three = term.kitty_chunks("a=t,i=3,q=2", bytes_of(bigger));
    print("apcs3: " + to_string(apcs(three)));
    print("mid: " + to_string(three.contains("\u{1B}_Gm=1;")));
    // El parser de la respuesta de la sonda: OK / error / basura / DA1 sola.
    print("ok: " + to_string(term.parse_graphics_reply(b"\x1b_Gi=31;OK\x1b\\\x1b[?64;4c")));
    print("err: " + to_string(term.parse_graphics_reply(b"\x1b_Gi=31;EBADF:oops\x1b\\\x1b[?64c")));
    print("empty: " + to_string(term.parse_graphics_reply(b"")));
    print("da1: " + to_string(term.parse_graphics_reply(b"\x1b[?64;1;4c")));
}
"#;
    let want = "delete: true\none: true\napcs: 2\nhead: true\ntail: true\napcs3: 3\nmid: true\nok: true\nerr: false\nempty: false\nda1: false\n";
    assert_on_all_engines("kitty_pure", src, want);
}

// M161 — los BYTES exactos que draw_image/clear_* ponen en el cable (patrón io_cli.rs:
// comparar out.stdout crudo). Sin tty se emite igual — es el contrato (captura/replay) y lo
// que hace posible este test. Congela el ORDEN de claves del control a propósito.
#[test]
fn draw_commands_emit_the_exact_escape_bytes() {
    let base = tmp("kitty_bytes");
    let src = r#"import std/term;
import std/image;

fn main() {
    let img = image.Image { width: 1, height: 1, pixels: bytes_of([255, 0, 0, 255]) };
    let _ = term.draw_image(7, 5, 3, img);
    let _ = term.clear_image(7);
    let _ = term.clear_images();
}
"#;
    std::fs::write(base.join("prog.ray"), src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["--vm", "prog.ray"])
        .current_dir(&base)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("corre");
    assert_eq!(out.status.code(), Some(0), "exit 0");
    let want: &[u8] = b"\x1b7\x1b[3;5H\x1b_Ga=T,i=7,t=d,f=32,s=1,v=1,C=1,q=2;/wAA/w==\x1b\\\x1b8\
\x1b_Ga=d,d=i,i=7,q=2\x1b\\\
\x1b_Ga=d,d=a,q=2\x1b\\";
    assert_eq!(out.stdout, want, "la secuencia exacta en el cable");
}

// M161 — las validaciones fallan como VALOR (Err con mensaje exacto), en los 3 motores.
#[test]
fn kitty_graphics_validation_errors_match_on_all_three_engines() {
    let src = r#"import std/term;
import std/image;

fn show(r: Result<int, string>) {
    match (r) {
        Result.Ok(_) => print("bad: ok"),
        Result.Err(e) => print("err: " + e),
    }
}

fn main() {
    let img = image.Image { width: 1, height: 1, pixels: bytes_of([255, 0, 0, 255]) };
    show(term.draw_image(0, 1, 1, img));
    let broken = image.Image { width: 2, height: 2, pixels: bytes_of([0]) };
    show(term.transmit_image(1, broken));
    show(term.place_image(-3, 1, 1, 0, 0));
    show(term.draw_png(1, 1, 1, b"not a png at all"));
    show(term.clear_image(0));
}
"#;
    let want = "err: image id must be positive\nerr: pixel buffer does not match width*height*4\nerr: image id must be positive\nerr: not a PNG (bad signature)\nerr: image id must be positive\n";
    assert_on_all_engines("kitty_validation", src, want);
}

/// M173 (W4, Windows): con una CONSOLA de verdad, `std/term` responde como en unix. El proceso de
/// `cargo test` tiene pipes por stdio (y en CI quizá ninguna consola), y `Command` de std pasa
/// SIEMPRE sus handles al hijo (`STARTF_USESTDHANDLES`), así que ninguna flag de creación
/// (`CREATE_NEW_CONSOLE`/`CREATE_NO_WINDOW`) le da al hijo los handles de una consola. Se lanza
/// vía `cmd /c start /wait /min`: `start` crea la consola nueva (minimizada: parpadea un instante
/// en local) y el programa nace con los handles de ESA consola. Deja sus hallazgos en un
/// archivo. Se prueba: `is_tty` true en 0/1/2, `size`
/// con columnas y filas positivas, `raw` que entra y sale (con `is_tty` aún true dentro), y un
/// `io.read_timeout` dentro del modo crudo que VENCE (antes, en Windows, ignoraba el plazo y
/// bloqueaba para siempre): el test acota el tiempo total. VM y, si hay rustc, binario nativo.
#[cfg(windows)]
#[test]
fn a_real_console_reports_a_terminal_and_read_timeout_expires() {
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};
    let base = tmp("console_win");
    let src = "import std/term;\n\
import std/io;\n\
import std/fs;\n\
\n\
fn main() -> int {\n\
    var out = \"tty \" + to_string(term.is_tty(0)) + \" \" + to_string(term.is_tty(1)) + \" \" + to_string(term.is_tty(2)) + \"\\n\";\n\
    match (term.size()) {\n\
        Option.Some(wh) => { out = out + \"size \" + to_string(wh.0 > 0 && wh.1 > 0) + \"\\n\"; },\n\
        Option.None => { out = out + \"no-size\\n\"; },\n\
    }\n\
    match (term.raw(fn() -> string {\n\
        let inside = to_string(term.is_tty(0));\n\
        match (io.read_timeout(8, 100)) {\n\
            io.ReadResult.Data(_) => \"data \" + inside,\n\
            io.ReadResult.Eof => \"eof \" + inside,\n\
            io.ReadResult.TimedOut => \"timeout \" + inside,\n\
        }\n\
    })) {\n\
        Result.Ok(r) => { out = out + \"raw \" + r + \"\\n\"; },\n\
        Result.Err(e) => { out = out + \"raw-err \" + e + \"\\n\"; },\n\
    }\n\
    let _ = fs.write_file(\"result.txt\", out);\n\
    0\n\
}\n";
    std::fs::write(base.join("prog.ray"), src).unwrap();
    let want = "tty true true true\nsize true\nraw timeout true\n";

    // (etiqueta, línea de comandos para `start`): el ejecutable entre comillas y sus args.
    let mut cmds: Vec<(String, String)> = Vec::new();
    cmds.push(("VM".into(), format!("\"{}\" run prog.ray", env!("CARGO_BIN_EXE_ray"))));
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin.exe");
        let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(bcode, 0, "build --native ok\n{berr}");
        cmds.push(("nativo".into(), format!("\"{}\"", bin.display())));
    }
    for (label, cmdline) in cmds {
        let _ = std::fs::remove_file(base.join("result.txt"));
        let started = Instant::now();
        // `start "" /wait /min <exe> args`: el primer par de comillas es el título de la ventana
        // (obligatorio cuando el ejecutable va entre comillas); `/wait` propaga el exit code a cmd.
        let status = Command::new("cmd")
            .current_dir(&base)
            .raw_arg(format!("/c start \"\" /wait /min {cmdline}"))
            .status()
            .expect("lanza cmd /c start");
        let elapsed = started.elapsed();
        assert!(status.success(), "{label}: exit 0 ({status})");
        assert!(elapsed < Duration::from_secs(20), "{label}: el read_timeout de 100 ms no venció (tardó {elapsed:?})");
        let got = std::fs::read_to_string(base.join("result.txt")).unwrap_or_default();
        assert_eq!(got, want, "{label}: la consola oculta es un terminal completo");
    }
}
