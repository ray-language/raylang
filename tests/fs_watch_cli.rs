//! Pruebas del watch de filesystem (M115.4): `fs.watch`/`fs.next_event`/`fs.next_event_timeout`
//! sobre eventos de KERNEL (crate notify: FSEvents/inotify). No determinista (disco + latencia
//! del kernel) → subproceso con HANDSHAKE: el programa imprime "ready" con el watch ya armado,
//! el test toca el archivo, y el programa reporta el evento. En ambos motores.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

#[test]
fn watch_reports_changes_and_timeout() {
    for vm in [false, true] {
        let base = std::env::temp_dir().join(format!("ray_watch_{}", if vm { "vm" } else { "in" }));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("mkdir");
        let root = base.to_string_lossy().into_owned();
        let src = format!(
            r#"
import std/fs;
fn main() -> int {{
    let h = match (fs.watch("{root}")) {{
        Result.Ok(h) => h,
        Result.Err(e) => {{ eprint(e); return 1; }},
    }};
    match (fs.next_event_timeout(h, 200)) {{
        Result.Ok(opt) => match (opt) {{
            Option.Some(ev) => print("unexpected early event"),
            Option.None => print("quiet"),
        }},
        Result.Err(e) => {{ eprint(e); return 1; }},
    }};
    print("ready");
    match (fs.next_event(h)) {{
        Result.Ok(ev) => {{
            if (ev.path.contains("touched.txt")) {{ print("event on touched.txt") }} else {{ print("event on " + ev.path) }}
        }},
        Result.Err(e) => {{ eprint(e); return 1; }},
    }};
    close(h);
    match (fs.next_event_timeout(h, 50)) {{
        Result.Ok(_) => print("MAL"),
        Result.Err(e) => print("closed -> err"),
    }};
    0
}}
"#
        );
        let mut prog = std::env::temp_dir();
        prog.push(format!("ray_watch_{}.ray", if vm { "vm" } else { "in" }));
        std::fs::File::create(&prog).expect("crea").write_all(src.as_bytes()).expect("escribe");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
        if vm {
            cmd.arg("--vm");
        } else {
            cmd.arg("--interp");
        }
        let mut child = cmd.arg(&prog).stdout(Stdio::piped()).spawn().expect("lanza raylang");
        let mut sout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        sout.read_line(&mut line).expect("primera línea");
        assert_eq!(line.trim(), "quiet", "sin cambios el plazo vence (vm={vm})");
        line.clear();
        sout.read_line(&mut line).expect("ready");
        assert_eq!(line.trim(), "ready", "handshake (vm={vm})");
        // El watch está armado: tocar el archivo dispara el evento y despierta al programa.
        std::fs::write(base.join("touched.txt"), b"hola").expect("toca");
        let mut rest = String::new();
        sout.read_to_string(&mut rest).expect("resto");
        assert!(rest.contains("event on touched.txt"), "reporta el evento (vm={vm}): {rest:?}");
        assert!(rest.contains("closed -> err"), "close invalida el watch (vm={vm}): {rest:?}");
        let status = child.wait().expect("termina");
        assert_eq!(status.code(), Some(0), "termina limpio (vm={vm})");
    }
}
