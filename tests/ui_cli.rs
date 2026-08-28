//! M146 — std/ui: ventana + webview. La batería corre contra el backend HEADLESS
//! (`RAY_UI_BACKEND=headless` — CI no tiene sesión gráfica; las ventanas son filas en memoria y
//! `close(h)` sintetiza el evento `closed`) en VM e intérprete; el camino AppKit real se
//! verifica con dogfood manual en macOS. El aparcado de verdad (la fibra espera en el self-pipe
//! de la cola y otra fibra cierra la ventana) se cubre en el test de la VM.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("ray_ui_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn run_headless(cmd: &mut Command) -> (String, i32) {
    cmd.env("RAY_UI_BACKEND", "headless");
    let out = cmd.stdin(std::process::Stdio::null()).output().expect("corre");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

const PROG: &str = r#"import std/ui;

fn main() {
    match (ui.open("Test", "http://127.0.0.1:1/", 0, 600)) {
        Result.Ok(_) => print("bad: size 0 accepted"),
        Result.Err(e) => print("size rejected: " + to_string(e.contains("unsupported window size"))),
    }
    match (ui.open("Test", "http://127.0.0.1:1/", 800, 600)) {
        Result.Err(e) => print("open failed: " + e),
        Result.Ok(h) => {
            print("eval ok: " + to_string(ui.eval_js(h, "document.title").is_ok()));
            match (ui.next_event_timeout(50)) {
                Result.Ok(o) => match (o) {
                    Option.None => print("quiet queue: true"),
                    Option.Some(_) => print("bad: unexpected event"),
                },
                Result.Err(e) => print("err: " + e),
            }
            let _ = close(h);
            match (ui.next_event()) {
                Result.Ok(e) => print("event: " + e.kind + ", same window: " + to_string(e.window == h)),
                Result.Err(e) => print("err: " + e),
            }
            print("eval after close: " + to_string(ui.eval_js(h, "1").is_err()));
        },
    }
}
"#;

const WANT: &str = "size rejected: true\neval ok: true\nquiet queue: true\n\
event: closed, same window: true\neval after close: true\n";

#[test]
fn headless_battery_matches_on_vm_and_interpreter() {
    let base = tmp("battery");
    std::fs::write(base.join("prog.ray"), PROG).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, code) =
            run_headless(Command::new(env!("CARGO_BIN_EXE_ray")).args([engine, "prog.ray"]).current_dir(&base));
        assert_eq!(code, 0, "{engine}: exit 0");
        assert_eq!(out, WANT, "{engine}: salida exacta");
    }
}

// El aparcado real: la fibra principal espera en `next_event()` (cola vacía → se aparca en el
// self-pipe) y una fibra aparte cierra la ventana 100 ms después — el evento la despierta. Y el
// canal de `events()` entrega lo mismo vía la fibra-bomba.
const PARK_PROG: &str = r#"import std/ui;
import std/time;

fn main() {
    let h = match (ui.open("Park", "http://127.0.0.1:1/", 320, 200)) {
        Result.Ok(w) => w,
        Result.Err(e) => {
            print("open failed: " + e);
            return;
        },
    };
    spawn(fn() {
        time.sleep(100);
        let _ = close(h);
    });
    let t0 = time.monotonic();
    match (ui.next_event()) {
        Result.Ok(e) => {
            let waited = time.monotonic() - t0;
            print("parked event: " + e.kind + ", waited: " + to_string(waited >= 80));
        },
        Result.Err(e) => print("err: " + e),
    }

    let ch = ui.events();
    let h2 = match (ui.open("Park2", "http://127.0.0.1:1/", 320, 200)) {
        Result.Ok(w) => w,
        Result.Err(e) => {
            print("open failed: " + e);
            return;
        },
    };
    spawn(fn() {
        time.sleep(50);
        let _ = close(h2);
    });
    match (recv(ch)) {
        Option.Some(e) => {
            print("channel event: " + e.kind + ", same window: " + to_string(e.window == h2));
        },
        Option.None => print("bad: channel closed"),
    }
}
"#;

#[test]
fn the_fiber_parks_and_the_events_channel_pumps() {
    let base = tmp("park");
    std::fs::write(base.join("prog.ray"), PARK_PROG).unwrap();
    let (out, code) =
        run_headless(Command::new(env!("CARGO_BIN_EXE_ray")).args(["--vm", "prog.ray"]).current_dir(&base));
    assert_eq!(code, 0, "exit 0");
    assert_eq!(
        out,
        "parked event: closed, waited: true\nchannel event: closed, same window: true\n",
        "salida exacta"
    );
}
