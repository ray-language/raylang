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
    print("about ok: " + to_string(ui.set_about("Demo", "Version 1.0", "A demo app", "(c) 2026 Demo").is_ok()));
    let app_items = [
        ui.MenuItem { tag: "role:about", title: "About Demo", shortcut: "" },
        ui.MenuItem { tag: "settings", title: "Settings...", shortcut: "," },
    ];
    print("app_menu ok: " + to_string(ui.app_menu("Demo", app_items).is_ok()));
    match (ui.app_menu("Demo", [ui.MenuItem { tag: "", title: "x", shortcut: "" }])) {
        Result.Ok(_) => print("bad: app_menu empty tag accepted"),
        Result.Err(e) => print("app_menu empty tag rejected: " + to_string(e.contains("non-empty tag"))),
    }
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
    const WANT_NONE: &str = "menu ok: true\nabout ok: true\napp_menu ok: true\n\
app_menu empty tag rejected: true\nempty tag rejected: true\npick cancelled\n\
save cancelled\nevent: closed, tag empty: true\n";
    const WANT_PICK: &str = "menu ok: true\nabout ok: true\napp_menu ok: true\n\
app_menu empty tag rejected: true\nempty tag rejected: true\npicked: /tmp/x.txt\n\
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

/// M152 (puente IPC): el kind "message" viaja por el MISMO stream que closed/menu. En
/// headless lo inyecta RAY_UI_MSG (uno por ventana abierta) — batería de salida exacta.
const MSG_PROG: &str = r#"import std/ui;

fn main() {
    match (ui.open("A", "http://127.0.0.1:1/", 320, 200)) {
        Result.Err(e) => print("open failed: " + e),
        Result.Ok(h) => {
            match (ui.next_event()) {
                Result.Ok(e) => print("message: kind=" + e.kind + " same window=" + to_string(e.window == h) + " tag=" + e.tag),
                Result.Err(e) => print("err: " + e),
            }
            let _ = close(h);
            match (ui.next_event()) {
                Result.Ok(e) => print("then: " + e.kind),
                Result.Err(e) => print("err: " + e),
            }
        },
    }
}
"#;

#[test]
fn injected_messages_match_on_all_three_engines() {
    const WANT: &str = "message: kind=message same window=true tag=hello ipc\nthen: closed\n";
    let base = tmp("msg");
    std::fs::write(base.join("prog.ray"), MSG_PROG).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, code) = run_headless(
            Command::new(env!("CARGO_BIN_EXE_ray"))
                .args([engine, "prog.ray"])
                .env("RAY_UI_MSG", "hello ipc")
                .current_dir(&base),
        );
        assert_eq!(code, 0, "{engine}: exit 0");
        assert_eq!(out, WANT, "{engine}: exact output");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin");
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("native build");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let (out, code) = run_headless(Command::new(&bin).env("RAY_UI_MSG", "hello ipc"));
        assert_eq!(code, 0, "native: exit 0");
        assert_eq!(out, WANT, "native ≡ VM");
    }
}

/// El argumento central de D1 (DESIGN §M152): los mensajes llegan TAMBIÉN por `events()` —
/// es el mismo stream, no un canal aparte que robaría eventos. VM (+ nativo), como el test
/// del park: `events()` usa spawn y el intérprete no tiene fibras.
const MSG_CHANNEL_PROG: &str = r#"import std/ui;

fn main() {
    let ch = ui.events();
    match (ui.open("B", "http://127.0.0.1:1/", 320, 200)) {
        Result.Err(e) => print("open failed: " + e),
        Result.Ok(h) => {
            if let Option.Some(e) = recv(ch) {
                print("via events: " + e.kind + " tag=" + e.tag + " same window=" + to_string(e.window == h));
            }
            let _ = close(h);
            if let Option.Some(e2) = recv(ch) {
                print("then: " + e2.kind);
            }
        },
    }
}
"#;

#[test]
fn messages_flow_through_the_events_channel_too() {
    const WANT: &str = "via events: message tag=ping same window=true\nthen: closed\n";
    let base = tmp("msg_chan");
    std::fs::write(base.join("prog.ray"), MSG_CHANNEL_PROG).unwrap();
    let (out, code) = run_headless(
        Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["--vm", "prog.ray"])
            .env("RAY_UI_MSG", "ping")
            .current_dir(&base),
    );
    assert_eq!(code, 0, "vm: exit 0");
    assert_eq!(out, WANT, "vm: exact output");
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join("prog_bin");
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("native build");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let (out, code) = run_headless(Command::new(&bin).env("RAY_UI_MSG", "ping"));
        assert_eq!(code, 0, "native: exit 0");
        assert_eq!(out, WANT, "native ≡ VM");
    }
}

/// M152 — el E2E REAL del puente (macOS, ventana de verdad): la página llama
/// `window.ray.send("ping")` (disparado con eval_js — el retry absorbe la carrera
/// eval-antes-de-load) y el programa lo ve por `next_event_timeout`. `#[ignore]` porque
/// `ray test`/la batería fuerzan headless y CI no tiene sesión gráfica:
/// `cargo test --test ui_cli -- --ignored` en un mac local.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "opens a real window; run by hand on macOS: cargo test --test ui_cli -- --ignored"]
fn a_real_webview_message_reaches_the_program() {
    let base = tmp("msg_real");
    let prog = r#"import std/ui;
import std/time;

fn main() {
    match (ui.open("ipc", "about:blank", 320, 200)) {
        Result.Err(e) => print("open failed: " + e),
        Result.Ok(h) => {
            var got = "";
            var tries = 0;
            while (got == "" && tries < 50) {
                let _ = ui.eval_js(h, "window.ray && window.ray.send('ping')");
                match (ui.next_event_timeout(100)) {
                    Result.Ok(o) => match (o) {
                        Option.Some(e) => {
                            if (e.kind == "message") {
                                got = e.tag + " same window=" + to_string(e.window == h);
                            }
                        },
                        Option.None => {},
                    },
                    Result.Err(_) => {},
                }
                tries = tries + 1;
            }
            print("got: " + got);
            let _ = close(h);
        },
    }
}
"#;
    std::fs::write(base.join("prog.ray"), prog).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["--vm", "prog.ray"])
        .env_remove("RAY_UI_BACKEND")
        .current_dir(&base)
        .output()
        .expect("run with a real window");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("got: ping same window=true"),
        "the real bridge delivers the message:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
