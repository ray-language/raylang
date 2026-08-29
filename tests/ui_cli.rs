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
fn headless_battery_matches_on_all_three_engines() {
    let base = tmp("battery");
    std::fs::write(base.join("prog.ray"), PROG).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, code) =
            run_headless(Command::new(env!("CARGO_BIN_EXE_ray")).args([engine, "prog.ray"]).current_dir(&base));
        assert_eq!(code, 0, "{engine}: exit 0");
        assert_eq!(out, WANT, "{engine}: salida exacta");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin");
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let (out, code) = run_headless(&mut Command::new(&bin));
        assert_eq!(code, 0, "nativo: exit 0");
        assert_eq!(out, WANT, "nativo ≡ VM");
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
    const WANT_PARK: &str = "parked event: closed, waited: true\nchannel event: closed, same window: true\n";
    let base = tmp("park");
    std::fs::write(base.join("prog.ray"), PARK_PROG).unwrap();
    let (out, code) =
        run_headless(Command::new(env!("CARGO_BIN_EXE_ray")).args(["--vm", "prog.ray"]).current_dir(&base));
    assert_eq!(code, 0, "exit 0");
    assert_eq!(out, WANT_PARK, "salida exacta");
    // El nativo con el mismo programa: el aparcado por el fd (fibras) y la fibra-bomba.
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin");
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let (out, code) = run_headless(&mut Command::new(&bin));
        assert_eq!(code, 0, "nativo: exit 0");
        assert_eq!(out, WANT_PARK, "nativo ≡ VM");
    }
}

// M147d — el negativo de Linux (corre en CI-ubuntu, sin display ni WebKitGTK): `ui.open` SIN
// headless debe fallar LIMPIO y rápido (lib ausente o sin sesión gráfica — el plazo del gate es
// 5 s), jamás colgarse. Es la única aserción barata del backend GTK sin un desktop real.
#[cfg(target_os = "linux")]
#[test]
fn on_linux_without_a_display_open_fails_clean_and_fast() {
    use std::time::Instant;
    let base = tmp("gtk_negative");
    std::fs::write(
        base.join("prog.ray"),
        "import std/ui;\n\nfn main() {\n    match (ui.open(\"X\", \"http://127.0.0.1:1/\", 320, 200)) {\n        Result.Ok(_) => print(\"bad: opened\"),\n        Result.Err(e) => print(\"clean err: \" + to_string(e.starts_with(\"ui:\"))),\n    }\n}\n",
    )
    .unwrap();
    let t0 = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["--vm", "prog.ray"])
        .env_remove("RAY_UI_BACKEND")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .current_dir(&base)
        .output()
        .expect("corre");
    let secs = t0.elapsed().as_secs();
    assert_eq!(out.status.code(), Some(0), "exit 0 (error como valor)");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "clean err: true\n", "Err limpio");
    assert!(secs < 15, "sin cuelgue (tardó {secs}s)");
}

// M148 — menús + diálogos de archivo, headless en los tres motores: menu() valida y no-opa;
// los diálogos se conducen con RAY_UI_PICK (set → Some, sin variable → None); el evento lleva
// el campo tag nuevo (vacío en closed).
const MENU_PROG: &str = r#"import std/ui;

fn main() {
    let items = [
        ui.MenuItem { tag: "new", title: "New", shortcut: "n" },
        ui.MenuItem { tag: "quit", title: "Quit Game", shortcut: "" },
    ];
    print("menu ok: " + to_string(ui.menu("Game", items).is_ok()));
    match (ui.menu("Bad", [ui.MenuItem { tag: "", title: "x", shortcut: "" }])) {
        Result.Ok(_) => print("bad: empty tag accepted"),
        Result.Err(e) => print("empty tag rejected: " + to_string(e.contains("non-empty tag"))),
    }
    match (ui.pick_file()) {
        Result.Ok(o) => match (o) {
            Option.Some(p) => print("picked: " + p),
            Option.None => print("pick cancelled"),
        },
        Result.Err(e) => print("err: " + e),
    }
    match (ui.save_file("draft.txt")) {
        Result.Ok(o) => match (o) {
            Option.Some(p) => print("save to: " + p),
            Option.None => print("save cancelled"),
        },
        Result.Err(e) => print("err: " + e),
    }
    match (ui.open("T", "http://127.0.0.1:1/", 320, 200)) {
        Result.Err(e) => print("open failed: " + e),
        Result.Ok(h) => {
            let _ = close(h);
            match (ui.next_event()) {
                Result.Ok(e) => print("event: " + e.kind + ", tag empty: " + to_string(e.tag == "")),
                Result.Err(e) => print("err: " + e),
            }
        },
    }
}
"#;

#[test]
fn menus_and_dialogs_match_on_all_three_engines() {
    const WANT_NONE: &str = "menu ok: true\nempty tag rejected: true\npick cancelled\n\
save cancelled\nevent: closed, tag empty: true\n";
    const WANT_PICK: &str = "menu ok: true\nempty tag rejected: true\npicked: /tmp/x.txt\n\
save to: /tmp/x.txt\nevent: closed, tag empty: true\n";
    let base = tmp("menus");
    std::fs::write(base.join("prog.ray"), MENU_PROG).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, code) = run_headless(
            Command::new(env!("CARGO_BIN_EXE_ray"))
                .args([engine, "prog.ray"])
                .env_remove("RAY_UI_PICK")
                .current_dir(&base),
        );
        assert_eq!(code, 0, "{engine}: exit 0");
        assert_eq!(out, WANT_NONE, "{engine}: sin RAY_UI_PICK");
        let (out, code) = run_headless(
            Command::new(env!("CARGO_BIN_EXE_ray"))
                .args([engine, "prog.ray"])
                .env("RAY_UI_PICK", "/tmp/x.txt")
                .current_dir(&base),
        );
        assert_eq!(code, 0, "{engine}: exit 0 (pick)");
        assert_eq!(out, WANT_PICK, "{engine}: con RAY_UI_PICK");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin");
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let (out, code) = run_headless(Command::new(&bin).env("RAY_UI_PICK", "/tmp/x.txt"));
        assert_eq!(code, 0, "nativo: exit 0");
        assert_eq!(out, WANT_PICK, "nativo ≡ VM");
    }
}
