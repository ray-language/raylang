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

/// M210 (feedback 27 de ray-remote): `ui.open_with` con `WindowOptions` — mínimo, redimensionable,
/// centrado y autosave. En headless las opciones se validan y la ventana es una fila en memoria.
/// M224: `titlebar_color` acepta `#rrggbb` (vacío = sistema) y rechaza cualquier otra forma.
const OPEN_WITH_PROGRAM: &str = r##"
import std/ui;

fn main() {
    var o = ui.options(1024, 720);
    o.min_width = 640;
    o.min_height = 480;
    o.resizable = false;
    o.autosave = "main";
    o.titlebar_color = "#1f2430";
    o.minimizable = false;
    match (ui.open_with("Opts", "http://127.0.0.1:1/", o)) {
        Result.Ok(h) => { print("opened: " + to_string(h > 0)); print("focus: " + to_string(ui.focus(h).is_ok())); let _ = close(h); print("focus closed: " + to_string(ui.focus(h).is_err())); },
        Result.Err(e) => print("open failed: " + e),
    }
    var bad = ui.options(400, 300);
    bad.min_width = 800;
    match (ui.open_with("Bad", "http://127.0.0.1:1/", bad)) {
        Result.Ok(_) => print("bad: minimum larger than the window accepted"),
        Result.Err(e) => print("min rejected: " + to_string(e.contains("unsupported minimum size"))),
    }
    var ugly = ui.options(400, 300);
    ugly.titlebar_color = "red";
    match (ui.open_with("Ugly", "http://127.0.0.1:1/", ugly)) {
        Result.Ok(_) => print("ugly: a non-#rrggbb color accepted"),
        Result.Err(e) => print("color rejected: " + to_string(e.contains("unsupported titlebar color 'red'"))),
    }
}
"##;

#[test]
fn open_with_validates_its_options_on_both_engines() {
    let path = tmp("open_with").join("main.ray");
    std::fs::write(&path, OPEN_WITH_PROGRAM).unwrap();
    for interp in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
        if interp {
            cmd.arg("--interp");
        }
        let (out, code) = run_headless(cmd.arg(&path));
        assert_eq!(code, 0, "{out}");
        assert_eq!(out, "opened: true\nfocus: true\nfocus closed: true\nmin rejected: true\ncolor rejected: true\n", "interp={interp}\n{out}");
    }
}

/// M217 (ray-sublime #8, #9): con `RAY_UI_TRACE=1` el headless escribe en stderr cada open y eval_js
/// (y por tanto cada `ui.reply`), y con `RAY_UI_EXIT_AFTER_MS=N` el proceso termina solo (salida 0)
/// tras N ms sin eventos — sin `perl -e alarm` en CI. La fibra estaba aparcada en `next_event()`.
#[test]
fn headless_trace_and_exit_after_idle() {
    let path = tmp("trace_exit").join("main.ray");
    std::fs::write(&path, "import std/ui;\nfn main() {\n    match (ui.open(\"T\", \"http://127.0.0.1:1/\", 320, 200)) {\n        Result.Ok(h) => { let _ = ui.eval_js(h, \"reply(1)\"); print(\"opened\"); let _ = ui.next_event(); print(\"never\"); },\n        Result.Err(e) => print(e),\n    }\n}\n").unwrap();
    let started = std::time::Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(&path)
        .env("RAY_UI_BACKEND", "headless")
        .env("RAY_UI_TRACE", "1")
        .env("RAY_UI_EXIT_AFTER_MS", "300")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(15), "termina solo");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let err = String::from_utf8_lossy(&out.stderr);
    let so = String::from_utf8_lossy(&out.stdout);
    assert!(err.contains("[ui] open 1 T http://127.0.0.1:1/") && err.contains("[ui] eval 1 reply(1)") && err.contains("[ui] exit"), "{err}");
    assert_eq!(so, "opened\n", "la salida anterior al cierre se conserva; 'never' no llega");
}

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
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
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
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
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
        ui.item("new", "New", "n"),
        ui.item("quit", "Quit Game", ""),
    ];
    print("menu ok: " + to_string(ui.menu("Game", items).is_ok()));
    print("about ok: " + to_string(ui.set_about("Demo", "Version 1.0", "A demo app", "(c) 2026 Demo").is_ok()));
    let app_items = [
        ui.item("role:about", "About Demo", ""),
        ui.item("settings", "Settings...", ","),
    ];
    print("app_menu ok: " + to_string(ui.app_menu("Demo", app_items).is_ok()));
    match (ui.app_menu("Demo", [ui.item("", "x", "")])) {
        Result.Ok(_) => print("bad: app_menu empty tag accepted"),
        Result.Err(e) => print("app_menu empty tag rejected: " + to_string(e.contains("non-empty tag"))),
    }
    match (ui.menu("Bad", [ui.item("", "x", "")])) {
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
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
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
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
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
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
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

/// M159 — `split_events()`: el fan-out puro raylang — los "message" por un canal, el resto
/// ("closed") por el otro. VM (+ nativo): usa spawn, como events().
const SPLIT_PROG: &str = r#"import std/ui;

fn main() {
    let pair = ui.split_events();
    let (msgs, other) = pair;
    match (ui.open("C", "http://127.0.0.1:1/", 320, 200)) {
        Result.Err(e) => print("open failed: " + e),
        Result.Ok(h) => {
            if let Option.Some(e) = recv(msgs) {
                print("msgs: " + e.kind + " tag=" + e.tag + " same window=" + to_string(e.window == h));
            }
            let _ = close(h);
            if let Option.Some(e2) = recv(other) {
                print("other: " + e2.kind);
            }
        },
    }
}
"#;

#[test]
fn split_events_routes_messages_and_the_rest() {
    const WANT: &str = "msgs: message tag=ping same window=true\nother: closed\n";
    let base = tmp("split");
    std::fs::write(base.join("prog.ray"), SPLIT_PROG).unwrap();
    let (out, code) = run_headless(
        Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["--vm", "prog.ray"])
            .env("RAY_UI_MSG", "ping")
            .current_dir(&base),
    );
    assert_eq!(code, 0, "vm: exit 0");
    assert_eq!(out, WANT, "vm: exact output");
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
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

/// M231 — devtools del webview en la toolchain: apagadas por defecto, encendidas con `ray run
/// --devtools` o bajo `ray dev` (RAY_DEV_RELOAD). Headless lo deja en la traza. (En nativo lo
/// decide el build: ver el test del transpilador.)
#[test]
fn devtools_follow_the_run_flag_and_ray_dev() {
    let path = tmp("devtools").join("main.ray");
    std::fs::write(&path, "import std/ui;\nfn main() {\n    match (ui.open(\"D\", \"http://127.0.0.1:1/\", 320, 200)) {\n        Result.Ok(h) => { print(\"opened\"); let _ = close(h); },\n        Result.Err(e) => print(e),\n    }\n}\n").unwrap();
    let run = |flag: bool, dev: bool| -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ray"));
        cmd.arg("run");
        if flag {
            cmd.arg("--devtools");
        }
        cmd.arg(&path).env("RAY_UI_BACKEND", "headless").env("RAY_UI_TRACE", "1").env_remove("RAY_DEV_RELOAD");
        if dev {
            cmd.env("RAY_DEV_RELOAD", "1");
        }
        let out = cmd.stdin(std::process::Stdio::null()).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "opened\n");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    assert!(!run(false, false).contains("[ui] devtools"), "por defecto: apagadas");
    assert!(run(true, false).contains("[ui] devtools 1 on"), "--devtools");
    assert!(run(false, true).contains("[ui] devtools 1 on"), "bajo ray dev");
}

/// M226 — el esquema `ray://app/…`: los montajes se validan sin ventana (directorio inexistente y
/// prefijo con `..` → Err; bytes en memoria → Ok) en ambos motores. El servicio real (Range, ETag,
/// MIME, traversal) lo cubren los tests unitarios de `ray_runtime::ui::scheme`.
#[test]
fn ray_scheme_mounts_validate_on_both_engines() {
    let base = tmp("scheme_mounts");
    std::fs::write(base.join("index.html"), "<p>hi</p>").unwrap();
    let path = base.join("main.ray");
    std::fs::write(
        &path,
        format!(
            "import std/ui;\nfn main() {{\n    print(to_string(ui.mount_dir(\"site\", \"{dir}\").is_ok()));\n    print(to_string(ui.mount_dir(\"bad\", \"{dir}/missing\").is_err()));\n    match (ui.mount_dir(\"../up\", \"{dir}\")) {{ Result.Ok(_) => print(\"escaped\"), Result.Err(e) => print(e) }}\n    print(to_string(ui.mount_bytes(\"mem/a.txt\", b\"abc\").is_ok()));\n    print(to_string(ui.mount_bytes(\"\", b\"abc\").is_err()));\n}}\n",
            // Windows: la ruta con `\\` dentro de un literal raylang no compila (escapes) → `/`.
            dir = base.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, code) = run_headless(Command::new(env!("CARGO_BIN_EXE_raylang")).args([engine]).arg(&path));
        assert_eq!(code, 0, "{engine}: {out}");
        assert_eq!(out, "true\ntrue\nui: path escapes the mount: '../up'\ntrue\ntrue\n", "{engine}\n{out}");
    }
}

/// M225 — el literal JS de `reply` se escapa en el runtime (una pasada, lineal) y `reply_json`
/// entrega por `_deliver_json` (la página recibe `JSON.parse`). El texto exacto del eval sale por
/// `RAY_UI_TRACE=1` en headless; cubre `\\`, `"`, `\n`, `\r`, NUL y U+2028.
#[test]
fn reply_escapes_natively_and_reply_json_uses_deliver_json() {
    let path = tmp("reply_escape").join("main.ray");
    std::fs::write(
        &path,
        "import std/ui;\nfn main() {\n    match (ui.open(\"R\", \"http://127.0.0.1:1/\", 320, 200)) {\n        Result.Ok(h) => {\n            let _ = ui.reply(h, 7, \"a\\\\b\\\"c\\nd\\re\\u{0}f\\u{2028}g\");\n            let _ = ui.reply_json(h, 8, \"{\\\"k\\\": [1, 2]}\");\n            print(\"sent\");\n            let _ = close(h);\n        },\n        Result.Err(e) => print(e),\n    }\n}\n",
    )
    .unwrap();
    for engine in ["--vm", "--interp"] {
        let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
            .args([engine])
            .arg(&path)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "sent\n", "{engine}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("[ui] eval 1 window.ray._deliver(7,\"a\\\\b\\\"c\\nd\\re\\u0000f\\u2028g\")"), "{engine}: reply escapado:\n{err}");
        assert!(err.contains("[ui] eval 1 window.ray._deliver_json(8,\"{\\\"k\\\": [1, 2]}\")"), "{engine}: reply_json:\n{err}");
    }
}

/// M157 — request/reply del puente IPC: el sobre `\u{1}q\u{1}id\u{1}payload` se decodifica
/// con as_request, un send plano no, y reply (eval_js) es Ok. 3 motores, headless (el sobre
/// entra por RAY_UI_MSG).
const REQUEST_PROG: &str = "import std/ui;\n\nfn main() {\n    match (ui.open(\"A\", \"http://127.0.0.1:1/\", 320, 200)) {\n        Result.Err(e) => print(\"open failed: \" + e),\n        Result.Ok(h) => {\n            match (ui.next_event()) {\n                Result.Ok(e) => {\n                    match (ui.as_request(e)) {\n                        Option.Some(pair) => {\n                            let (id, body) = pair;\n                            print(\"request \" + to_string(id) + \": \" + body);\n                            print(\"reply ok: \" + to_string(ui.reply(h, id, \"pong\").is_ok()));\n                        },\n                        Option.None => print(\"plain: \" + e.tag),\n                    }\n                },\n                Result.Err(e) => print(\"err: \" + e),\n            }\n            let _ = close(h);\n        },\n    }\n}\n";

#[test]
fn requests_decode_and_reply_on_all_three_engines() {
    const WANT: &str = "request 7: {\"op\":\"sum\"}\nreply ok: true\n";
    let envelope = "\u{1}q\u{1}7\u{1}{\"op\":\"sum\"}";
    let base = tmp("req");
    std::fs::write(base.join("prog.ray"), REQUEST_PROG).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, code) = run_headless(
            Command::new(env!("CARGO_BIN_EXE_ray"))
                .args([engine, "prog.ray"])
                .env("RAY_UI_MSG", envelope)
                .current_dir(&base),
        );
        assert_eq!(code, 0, "{engine}: exit 0");
        assert_eq!(out, WANT, "{engine}: exact output");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("native build");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let (out, code) = run_headless(Command::new(&bin).env("RAY_UI_MSG", envelope));
        assert_eq!(code, 0, "native: exit 0");
        assert_eq!(out, WANT, "native ≡ VM");
    }
}

/// M157 — el CICLO REAL request→reply en una ventana de verdad (macOS): la página lanza
/// window.ray.request('ping') y manda el resultado de vuelta con send — el programa
/// responde 'pong' y debe ver 'got:pong'. `#[ignore]`: cargo test -- --ignored en un mac.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "opens a real window; run by hand on macOS: cargo test --test ui_cli -- --ignored"]
fn a_real_request_reply_roundtrip_completes() {
    let base = tmp("req_real");
    let prog = "import std/time;\nimport std/ui;\n\nfn main() {\n    match (ui.open(\"rr\", \"about:blank\", 320, 200)) {\n        Result.Err(e) => print(\"open failed: \" + e),\n        Result.Ok(h) => {\n            var got = \"\";\n            var tries = 0;\n            while (got == \"\" && tries < 50) {\n                let _ = ui.eval_js(h, \"window.__armed||(window.__armed=1,window.ray.request('ping').then(function(r){window.ray.send('got:'+r)}))\");\n                match (ui.next_event_timeout(100)) {\n                    Result.Ok(o) => {\n                        if let Option.Some(e) = o {\n                            match (ui.as_request(e)) {\n                                Option.Some(pair) => {\n                                    let (id, body) = pair;\n                                    let _ = ui.reply(h, id, \"pong-\" + body);\n                                },\n                                Option.None => {\n                                    if (e.kind == \"message\") {\n                                        got = e.tag;\n                                    }\n                                },\n                            }\n                        }\n                    },\n                    Result.Err(_) => {},\n                }\n                tries = tries + 1;\n            }\n            print(\"got: \" + got);\n            let _ = close(h);\n        },\n    }\n}\n";
    std::fs::write(base.join("prog.ray"), prog).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["--vm", "prog.ray"])
        .env_remove("RAY_UI_BACKEND")
        .current_dir(&base)
        .output()
        .expect("run with a real window");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("got: got:pong-ping"),
        "the real request/reply roundtrip completes:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// M179 (W7c): una ventana REAL en Windows — Win32 + WebView2. Abre `about:blank`, evalúa JS,
/// cierra y espera el evento `closed` (desde M252 la ventana real emite antes `focused` al
/// activarse — WM_ACTIVATE — y el programa lo salta). Exige el WebView2 Runtime (viene con Windows 11 / Edge) y
/// una sesión de escritorio (el runner de GitHub la tiene); sin runtime, `open` devuelve un `Err`
/// que nombra el WebView2 Runtime y el test lo reporta como salto explícito.
#[cfg(windows)]
#[test]
fn on_windows_a_real_window_opens_evaluates_and_closes() {
    let base = tmp("win_real");
    std::fs::write(
        base.join("prog.ray"),
        "import std/ui;\n\nfn main() {\n    match (ui.open(\"ray test\", \"about:blank\", 320, 200)) {\n        Result.Err(e) => print(\"open err: \" + e),\n        Result.Ok(h) => {\n            print(\"eval ok: \" + to_string(ui.eval_js(h, \"document.title = 'x'\").is_ok()));\n            let _ = close(h);\n            var kind = \"focused\";\n            var tries = 0;\n            while (kind == \"focused\" && tries < 5) {\n                match (ui.next_event_timeout(5000)) {\n                    Result.Ok(o) => match (o) {\n                        Option.Some(e) => { kind = e.kind; },\n                        Option.None => { kind = \"none\"; },\n                    },\n                    Result.Err(e) => { kind = \"err: \" + e; },\n                }\n                tries = tries + 1;\n            }\n            print(\"event: \" + kind);\n        },\n    }\n}\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["--vm", "prog.ray"])
        .env_remove("RAY_UI_BACKEND")
        .current_dir(&base)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("corre");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if text.contains("WebView2 Runtime") {
        eprintln!("saltando: sin WebView2 Runtime en esta máquina\n{text}");
        return;
    }
    assert_eq!(out.status.code(), Some(0), "exit 0\n{text}\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(text, "eval ok: true\nevent: closed\n", "la ventana real abre, evalúa y cierra");
}

/// M183 (A1 de RayDesk en Windows): `ui.next_event()` BLOQUEANTE debe volver aunque otra fibra
/// tenga un socket aparcado (el webserver de una app de escritorio). Antes, en Windows, el
/// poller esperaba sin plazo por el socket y la cola de eventos de UI — sin fd — no lo despertaba:
/// la app nunca veía `message`/`menu`/`closed` (solo `next_event_timeout` "funcionaba", al vencer).
const PARKED_SOCKET_PROG: &str = r#"import std/ui;
import std/net;
import std/time;

fn main() -> int {
    let srv = match (net.tcp_listen("127.0.0.1", 0)) {
        Result.Ok(s) => s,
        Result.Err(e) => { print("listen: " + e); return 1; },
    };
    let acceptor = spawn(fn() {
        match (net.tcp_accept(srv)) {
            Result.Ok(c) => { close(c); },
            Result.Err(_) => {},
        }
    });
    let h = match (ui.open("probe", "about:blank", 320, 200)) {
        Result.Ok(h) => h,
        Result.Err(e) => { print("open err: " + e); return 1; },
    };
    let closer = spawn(fn() { time.sleep(300); let _ = close(h); });
    match (ui.next_event()) {
        Result.Ok(ev) => print("event: " + ev.kind),
        Result.Err(e) => print("err: " + e),
    }
    join(closer);
    let port = net.local_port(srv);
    match (net.tcp_connect("127.0.0.1", port)) {
        Result.Ok(c) => { close(c); },
        Result.Err(_) => {},
    }
    join(acceptor);
    close(srv);
    print("done");
    0
}
"#;

#[test]
fn blocking_next_event_wakes_while_a_socket_is_parked() {
    let base = tmp("parked_socket");
    std::fs::write(base.join("prog.ray"), PARKED_SOCKET_PROG).unwrap();
    // Solo la VM: el programa usa `spawn` y el intérprete es el oráculo secuencial.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ray"));
    cmd.args(["--vm", "prog.ray"]).current_dir(&base);
    let (out, code) = run_headless(&mut cmd);
    assert_eq!(code, 0, "exit 0\n{out}");
    assert_eq!(out, "event: closed\ndone\n", "el next_event bloqueante despierta con un socket aparcado");
}

// ---------------------------------------------------------------------------
// M234 — live-reload para apps de `std/ui` sin servidor HTTP: el runtime se suscribe al hub de
// `ray dev` (puerto que fija la toolchain) y recarga sus ventanas; `mount_embed` monta el
// directorio en vivo bajo la toolchain y los bytes horneados en el nativo.
// ---------------------------------------------------------------------------

/// Un hub SSE falso como el de `ray dev`: `GET /ui` → 204; cualquier otra petición recibe las
/// cabeceras SSE y un `reload` al momento. Devuelve el puerto.
fn fake_reload_hub() -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(300)));
            let mut buf = [0u8; 256];
            let n = s.read(&mut buf).unwrap_or(0);
            if buf[..n].starts_with(b"GET /ui") {
                let _ = s.write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
                continue;
            }
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n: connected\n\n";
            let _ = s.write_all(head.as_bytes());
            let _ = s.flush();
            // El runtime se suscribe al abrir la ventana, ANTES de registrarla: como haría `ray dev`
            // (que solo emite al cambiar un archivo), el `reload` llega un poco después.
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(400));
                let _ = s.write_all(b"data: reload\n\n");
                let _ = s.flush();
                std::thread::sleep(std::time::Duration::from_secs(3));
                drop(s);
            });
        }
    });
    port
}

#[test]
fn dev_reload_hub_reloads_ui_windows_without_a_webserver() {
    let base = tmp("dev_reload");
    let port = fake_reload_hub();
    std::fs::write(
        base.join("prog.ray"),
        "import std/ui;\nfn main() {\n    match (ui.open(\"T\", \"ray://app/index.html\", 320, 200)) {\n        Result.Ok(_) => { print(\"opened\"); let _ = ui.next_event(); print(\"never\"); },\n        Result.Err(e) => print(e),\n    }\n}\n",
    )
    .unwrap();
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .env("RAY_UI_EXIT_AFTER_MS", "1500")
            .env("RAY_DEV_RELOAD", port.to_string())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "opened\n", "{engine:?}\n{err}");
        assert!(err.contains("[ui] eval 1 location.reload()"), "{engine:?}: la ventana se recarga\n{err}");
        assert!(err.contains("[ui] dev reload 1 windows"), "{engine:?}\n{err}");
    }
}

#[test]
fn mount_embed_serves_the_directory_live_under_the_toolchain_and_baked_natively() {
    let base = tmp("mount_embed_live");
    std::fs::create_dir_all(base.join("assets")).unwrap();
    std::fs::write(base.join("assets/a.txt"), "hola").unwrap();
    std::fs::write(base.join("ray.toml"), "[package]\nname = \"embedlive\"\nversion = \"0.1.0\"\n\n[native]\nembed = [\"assets\"]\n").unwrap();
    std::fs::write(
        base.join("prog.ray"),
        "import std/ui;\nfn main() {\n    match (ui.mount_embed(\"\", \"assets\")) { Result.Ok(n) => print(\"ok ${n}\"), Result.Err(e) => print(e) }\n    match (ui.mount_embed(\"static\", \"assets\")) { Result.Ok(n) => print(\"ok ${n}\"), Result.Err(e) => print(e) }\n}\n",
    )
    .unwrap();
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ok 1\nok 1\n", "{engine:?}\n{err}");
        assert!(err.contains("[ui] mount dir assets "), "{engine:?}: montaje en vivo del directorio\n{err}");
        assert!(err.contains("[ui] mount dir static/assets "), "{engine:?}\n{err}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).env("RAY_UI_BACKEND", "headless").env("RAY_UI_TRACE", "1").output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ok 1\nok 1\n", "nativo\n{err}");
        assert!(!err.contains("[ui] mount dir"), "nativo: horneado, sin directorio en vivo\n{err}");
    }
}

// ---------------------------------------------------------------------------
// M235 (ray-sublime #69) — escritorio y portapapeles: `ui.open_path`/`ui.reveal` validan la ruta y
// en headless dejan traza; el portapapeles headless es un buffer en proceso (ida y vuelta).
// ---------------------------------------------------------------------------
#[test]
fn desktop_actions_and_clipboard_on_all_three_engines() {
    let base = tmp("desktop_clip");
    std::fs::write(base.join("note.txt"), "x").unwrap();
    std::fs::write(
        base.join("prog.ray"),
        "import std/ui;\nfn main() {\n    match (ui.reveal(\"note.txt\")) { Result.Ok(_) => print(\"revealed\"), Result.Err(e) => print(e) }\n    match (ui.open_path(\"note.txt\")) { Result.Ok(_) => print(\"opened\"), Result.Err(e) => print(e) }\n    match (ui.open_path(\"missing.txt\")) { Result.Ok(_) => print(\"bad\"), Result.Err(e) => print(e) }\n    match (ui.reveal(\"\")) { Result.Ok(_) => print(\"bad\"), Result.Err(e) => print(e) }\n    match (ui.clipboard_write(\"hola ✓\")) { Result.Ok(_) => print(\"wrote\"), Result.Err(e) => print(e) }\n    match (ui.clipboard_read()) { Result.Ok(s) => print(\"read: \" + s), Result.Err(e) => print(e) }\n}\n",
    )
    .unwrap();
    const WANT: &str = "revealed\nopened\nui: no such path 'missing.txt'\nui: no such path ''\nwrote\nread: hola ✓\n";
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}\n{err}");
        assert!(err.contains("[ui] reveal note.txt") && err.contains("[ui] open_path note.txt"), "{engine:?}\n{err}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).current_dir(&base).env("RAY_UI_BACKEND", "headless").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

// ---------------------------------------------------------------------------
// M236 (ray-sublime #70/#68) — menús: chords en `shortcut`, separadores, estado por tag y
// posición. Headless valida igual que un backend real (chords, tags) y deja traza.
// ---------------------------------------------------------------------------
#[test]
fn menu_chords_separators_state_and_position_on_all_three_engines() {
    let base = tmp("menus_m236");
    std::fs::write(
        base.join("prog.ray"),
        r##"import std/ui;
fn main() {
    var save = ui.item("save", "Save", "cmd+s");
    var save_all = ui.item("save_all", "Save All", "cmd+alt+s");
    save_all.icon = "sf:square.and.arrow.down.on.square";
    var wrap = ui.item("wrap", "Word Wrap", "");
    wrap.checked = true;
    var close = ui.item("close", "Close File", "cmd+w");
    close.enabled = false;
    let items = [save, save_all, ui.separator(), ui.item("reopen", "Reopen Closed File", "cmd+shift+t"), ui.item("go", "Go to Line", "ctrl+g"), ui.item("run", "Run", "f5"), wrap, close];
    match (ui.menu_at(0, "File", items)) { Result.Ok(_) => print("file ok"), Result.Err(e) => print(e) }
    match (ui.menu("Bad", [ui.item("x", "X", "cmd+bogus")])) { Result.Ok(_) => print("bad accepted"), Result.Err(e) => print(e) }
    match (ui.menu("Bad", [ui.item("x", "X", "hyper+s")])) { Result.Ok(_) => print("bad accepted"), Result.Err(e) => print(e) }
    match (ui.set_menu_item("save", false, false)) { Result.Ok(_) => print("save disabled"), Result.Err(e) => print(e) }
    match (ui.set_menu_item("wrap", true, false)) { Result.Ok(_) => print("wrap unchecked"), Result.Err(e) => print(e) }
    match (ui.set_menu_item("nope", true, false)) { Result.Ok(_) => print("bad"), Result.Err(e) => print(e) }
    match (ui.set_menu_item("", true, false)) { Result.Ok(_) => print("bad"), Result.Err(e) => print(e) }
    match (ui.menu("Legacy", [ui.item("n", "New", "n"), ui.item("s", "Shifted", "S")])) { Result.Ok(_) => print("legacy ok"), Result.Err(e) => print(e) }
}
"##,
    )
    .unwrap();
    const WANT: &str = "file ok\nui: unsupported menu shortcut 'cmd+bogus'\nui: unsupported menu shortcut 'hyper+s'\nsave disabled\nwrap unchecked\nui: no menu item with tag 'nope'\nui: set_menu_item needs a tag\nlegacy ok\n";
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}\n{err}");
        assert!(err.contains("[ui] menu File at 0 items 8"), "{engine:?}\n{err}");
        assert!(err.contains("[ui] menu item save enabled=false checked=false"), "{engine:?}\n{err}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).current_dir(&base).env("RAY_UI_BACKEND", "headless").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

// ---------------------------------------------------------------------------
// M239 — `ui.set_titlebar_color` en caliente: valida el formato y el handle igual que un
// backend real; headless deja traza. Tres motores.
// ---------------------------------------------------------------------------
#[test]
fn titlebar_color_changes_on_an_open_window_on_all_three_engines() {
    let base = tmp("titlebar_setter");
    std::fs::write(
        base.join("prog.ray"),
        "import std/ui;\nfn main() {\n    let h = ui.open(\"T\", \"ray://app/index.html\", 320, 200).unwrap();\n    match (ui.set_titlebar_color(h, \"#1e1e2e\")) { Result.Ok(_) => print(\"tinted\"), Result.Err(e) => print(e) }\n    match (ui.set_titlebar_color(h, \"\")) { Result.Ok(_) => print(\"system\"), Result.Err(e) => print(e) }\n    match (ui.set_titlebar_color(h, \"red\")) { Result.Ok(_) => print(\"bad\"), Result.Err(e) => print(e) }\n    match (ui.set_titlebar_color(99, \"#000000\")) { Result.Ok(_) => print(\"bad\"), Result.Err(e) => print(e) }\n    close(h);\n}\n",
    )
    .unwrap();
    const WANT: &str = "tinted\nsystem\nui: unsupported titlebar color 'red' (expected #rrggbb)\nui: not an open window\n";
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}\n{err}");
        assert!(err.contains("[ui] titlebar 1 #1e1e2e") && err.contains("[ui] titlebar 1 system"), "{engine:?}\n{err}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).current_dir(&base).env("RAY_UI_BACKEND", "headless").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

// ---------------------------------------------------------------------------
// M250 (ray-sublime) — `ui.replace_menu`: el contenido de un menú existente cambia después de
// creado (títulos, atajos, ítems); los tags viejos desaparecen, los nuevos existen; un título
// desconocido es `Err`. Headless deja traza; VM, intérprete y nativo.
// ---------------------------------------------------------------------------
#[test]
fn replace_menu_swaps_the_items_of_an_existing_menu_on_all_three_engines() {
    let base = tmp("menus_m250");
    std::fs::write(
        base.join("prog.ray"),
        r##"import std/ui;
fn main() {
    match (ui.menu_at(0, "View", [ui.item("sidebar", "Side Bar", "cmd+k"), ui.item("panel", "Panel", "cmd+j")])) { Result.Ok(_) => print("view ok"), Result.Err(e) => print(e) }
    match (ui.set_menu_item("panel", true, true)) { Result.Ok(_) => print("panel checked"), Result.Err(e) => print(e) }
    match (ui.replace_menu("View", [ui.item("sidebar", "Side Bar   ⌘K ⌘B", ""), ui.separator(), ui.item("terminal", "Terminal", "ctrl+`")])) { Result.Ok(_) => print("view replaced"), Result.Err(e) => print(e) }
    match (ui.set_menu_item("terminal", true, false)) { Result.Ok(_) => print("terminal live"), Result.Err(e) => print(e) }
    match (ui.replace_menu("Nope", [ui.item("x", "X", "")])) { Result.Ok(_) => print("bad"), Result.Err(e) => print(e) }
    match (ui.replace_menu("View", [ui.item("x", "X", "cmd+bogus")])) { Result.Ok(_) => print("bad"), Result.Err(e) => print(e) }
}
"##,
    )
    .unwrap();
    const WANT: &str = "view ok\npanel checked\nview replaced\nterminal live\nui: no menu titled 'Nope'\nui: unsupported menu shortcut 'cmd+bogus'\n";
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}\n{err}");
        assert!(err.contains("[ui] replace menu View items 3"), "traza headless: {err}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).current_dir(&base).env("RAY_UI_BACKEND", "headless").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

// ---------------------------------------------------------------------------
// M260 — tipos de ventana (`kind`), `always_on_top`, `parent` y las operaciones de geometría en
// caliente (`set_fullscreen`, `set_always_on_top`, `set_size`, `set_position`, `center`, `minimize`,
// `maximize`): en headless se validan y trazan; `Err` con kind desconocido, dueña no abierta,
// tamaño fuera de rango o ventana cerrada. VM, intérprete y nativo.
// ---------------------------------------------------------------------------
#[test]
fn window_kinds_and_geometry_ops_on_all_three_engines() {
    let base = tmp("window_kinds_m260");
    std::fs::write(
        base.join("prog.ray"),
        r##"import std/ui;
fn show(r: Result<int, string>, ok: string) { match (r) { Result.Ok(_) => print(ok), Result.Err(e) => print(e) } }
fn main() {
    let main_w = ui.open("Editor", "http://127.0.0.1:1/", 800, 600).unwrap();
    var p = ui.options(320, 200);
    p.kind = "panel";
    p.always_on_top = true;
    p.parent = main_w;
    match (ui.open_with("Find", "http://127.0.0.1:1/", p)) { Result.Ok(h) => print("panel " + to_string(h)), Result.Err(e) => print(e) }
    var b = ui.options(300, 300);
    b.kind = "borderless";
    match (ui.open_with("Splash", "http://127.0.0.1:1/", b)) { Result.Ok(h) => print("borderless " + to_string(h)), Result.Err(e) => print(e) }
    var bad = ui.options(300, 300);
    bad.kind = "hud";
    match (ui.open_with("X", "http://127.0.0.1:1/", bad)) { Result.Ok(_) => print("bad"), Result.Err(e) => print(e) }
    var orphan = ui.options(300, 300);
    orphan.parent = 99;
    match (ui.open_with("X", "http://127.0.0.1:1/", orphan)) { Result.Ok(_) => print("bad"), Result.Err(e) => print(e) }
    show(ui.set_fullscreen(main_w, true), "fullscreen on");
    show(ui.set_fullscreen(main_w, false), "fullscreen off");
    show(ui.set_always_on_top(main_w, true), "on top");
    show(ui.set_size(main_w, 1024, 720), "resized");
    show(ui.set_size(main_w, 0, 720), "bad");
    show(ui.set_position(main_w, 40, 60), "moved");
    show(ui.center(main_w), "centered");
    show(ui.minimize(main_w), "minimized");
    show(ui.maximize(main_w), "maximized");
    close(main_w);
    show(ui.center(main_w), "bad");
}
"##,
    )
    .unwrap();
    const WANT: &str = "panel 2\nborderless 3\nui: unsupported window kind 'hud' (document, panel, borderless)\nui: the parent is not an open window\nfullscreen on\nfullscreen off\non top\nresized\nui: unsupported window size 0x720\nmoved\ncentered\nminimized\nmaximized\nui: not an open window\n";
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}\n{err}");
        for line in [
            "[ui] window 2 kind panel always_on_top true parent 1",
            "[ui] window 3 kind borderless always_on_top false parent 0",
            "[ui] window 1 Fullscreen(true)",
            "[ui] window 1 Size(1024, 720)",
            "[ui] window 1 Position(40, 60)",
            "[ui] window 1 Center",
            "[ui] window 1 Maximize",
        ] {
            assert!(err.contains(line), "traza headless '{line}': {err}");
        }
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).current_dir(&base).env("RAY_UI_BACKEND", "headless").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

// ---------------------------------------------------------------------------
// M259 (ray-sublime) — `ui.popup_menu(h, items)`: menú contextual. En headless registra los tags
// (para `set_menu_item`) y deja traza; `Err` sobre ventana cerrada/desconocida o item inválido.
// VM, intérprete y nativo.
// ---------------------------------------------------------------------------
#[test]
fn popup_menu_registers_its_items_on_all_three_engines() {
    let base = tmp("popup_m259");
    std::fs::write(
        base.join("prog.ray"),
        r##"import std/ui;
fn show(r: Result<int, string>, ok: string) { match (r) { Result.Ok(_) => print(ok), Result.Err(e) => print(e) } }
fn main() {
    let w = ui.open("Editor", "http://127.0.0.1:1/", 400, 300).unwrap();
    show(ui.popup_menu(w, [ui.item("rename", "Rename...", ""), ui.separator(), ui.item("role:copy", "", ""), ui.item("delete", "Delete", "del")]), "popup ok");
    show(ui.set_menu_item("delete", false, false), "delete greyed");
    show(ui.popup_menu(w, [ui.item("x", "X", "cmd+bogus")]), "bad");
    show(ui.popup_menu(99, [ui.item("x", "X", "")]), "bad");
    close(w);
    show(ui.popup_menu(w, [ui.item("x", "X", "")]), "bad");
}
"##,
    )
    .unwrap();
    const WANT: &str = "popup ok\ndelete greyed\nui: unsupported menu shortcut 'cmd+bogus'\nui: not an open window\nui: not an open window\n";
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}\n{err}");
        assert!(err.contains("[ui] popup 1 items 4"), "traza headless: {err}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).current_dir(&base).env("RAY_UI_BACKEND", "headless").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

// ---------------------------------------------------------------------------
// M258 (ray-sublime) — `ui.message`/`alert`/`confirm` y los diálogos de archivo con opciones
// (`pick_file_with`, `pick_files`, `save_file_with`): en headless `RAY_UI_ANSWER` conduce el
// botón y `RAY_UI_PICK` (rutas separadas por \n) el diálogo; la traza muestra estilo, botones,
// filtros y multiple. Validación: 1..3 botones, estilo conocido. VM, intérprete y nativo.
// ---------------------------------------------------------------------------
#[test]
fn message_dialogs_and_file_dialog_options_on_all_three_engines() {
    let base = tmp("dialogs_m258");
    std::fs::write(
        base.join("prog.ray"),
        r##"import std/ui;
fn show_int(r: Result<int, string>) { match (r) { Result.Ok(i) => print("button " + to_string(i)), Result.Err(e) => print(e) } }
fn show_opt(r: Result<Option<string>, string>) { match (r) { Result.Ok(o) => match (o) { Option.Some(p) => print("path " + p), Option.None => print("none") }, Result.Err(e) => print(e) } }
fn main() {
    show_int(ui.message("Save changes?", "Your edits will be lost.", ["Save", "Don't Save", "Cancel"]));
    show_int(ui.message_styled("Disk full", "", "error", ["OK"]));
    show_int(ui.alert("Done", "Exported."));
    match (ui.confirm("Delete?", "This cannot be undone.", "Delete", "Cancel")) { Result.Ok(b) => print("confirm " + to_string(b)), Result.Err(e) => print(e) }
    show_int(ui.message("x", "y", []));
    show_int(ui.message_styled("x", "y", "fancy", ["OK"]));
    var o = ui.file_options();
    o.title = "Open a source file";
    o.directory = "/tmp";
    o.filters = [ui.filter("Ray sources", ["ray", "toml"]), ui.filter("Text", ["txt"])];
    show_opt(ui.pick_file_with(o));
    match (ui.pick_files(o)) { Result.Ok(ps) => print("files " + to_string(ps.len()) + " " + ps.join("+")), Result.Err(e) => print(e) }
    o.suggested = "untitled.ray";
    show_opt(ui.save_file_with(o));
}
"##,
    )
    .unwrap();
    const WANT: &str = "button 1\nbutton 0\nbutton 0\nconfirm false\nui: a message dialog needs 1 to 3 buttons\nui: unknown message style 'fancy' (info, warning, error)\npath /a/x.ray\nfiles 2 /a/x.ray+/a/y.ray\npath /a/x.ray\n";
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .env("RAY_UI_ANSWER", "1")
            .env("RAY_UI_PICK", "/a/x.ray\n/a/y.ray")
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}\n{err}");
        for line in [
            "[ui] message info 'Save changes?' [Save|Don't Save|Cancel]",
            "[ui] message error 'Disk full' [OK]",
            "[ui] dialog open_file title 'Open a source file' dir '/tmp' filters [Ray sources:ray,toml Text:txt] multiple false",
            "[ui] dialog open_file title 'Open a source file' dir '/tmp' filters [Ray sources:ray,toml Text:txt] multiple true",
            "[ui] dialog save_file title 'Open a source file'",
        ] {
            assert!(err.contains(line), "traza headless '{line}': {err}");
        }
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_ANSWER", "1")
            .env("RAY_UI_PICK", "/a/x.ray\n/a/y.ray")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

// ---------------------------------------------------------------------------
// M257 (ray-sublime) — operaciones de ventana por nombre: `set_title`, `set_edited`,
// `intercept_close`, `intercept_quit`. En headless: `Ok` con traza, `Err` sobre una ventana
// cerrada/desconocida, y `close(h)` sigue cerrando aunque el cierre esté interceptado (emite
// `closed`). El cierre del USUARIO (botón/⌘W/WM/WM_CLOSE) no existe en headless: la
// interceptación real queda para los backends. VM, intérprete y nativo.
// ---------------------------------------------------------------------------
#[test]
fn window_title_edited_and_close_interception_on_all_three_engines() {
    let base = tmp("window_ops_m257");
    std::fs::write(
        base.join("prog.ray"),
        r##"import std/ui;
fn show(r: Result<int, string>, ok: string) { match (r) { Result.Ok(_) => print(ok), Result.Err(e) => print(e) } }
fn main() {
    let w = ui.open("Editor", "http://127.0.0.1:1/", 400, 300).unwrap();
    show(ui.set_title(w, "main.ray — Editor"), "title ok");
    show(ui.set_edited(w, true), "edited ok");
    show(ui.intercept_close(w, true), "intercept ok");
    show(ui.intercept_quit(true), "quit intercept ok");
    close(w);
    match (ui.next_event_timeout(500)) {
        Result.Ok(o) => match (o) { Option.Some(e) => print(e.kind + " " + to_string(e.window)), Option.None => print("no event") },
        Result.Err(e) => print(e),
    }
    show(ui.set_title(w, "after close"), "bad");
    show(ui.intercept_close(99, true), "bad");
}
"##,
    )
    .unwrap();
    const WANT: &str = "title ok\nedited ok\nintercept ok\nquit intercept ok\nclosed 1\nui: not an open window\nui: not an open window\n";
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}\n{err}");
        for line in ["[ui] title 1 main.ray — Editor", "[ui] edited 1 true", "[ui] intercept close 1 on", "[ui] intercept quit on"] {
            assert!(err.contains(line), "traza headless '{line}': {err}");
        }
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).current_dir(&base).env("RAY_UI_BACKEND", "headless").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

// ---------------------------------------------------------------------------
// M255 (ray-sublime) — roles estándar de edición (`role:undo`, `role:paste`, …) y `edit_menu`:
// título/atajo estándar si vienen vacíos, `set_menu_item` por tag, y el Edit se crea la primera
// vez y se reemplaza después (en headless no hay Edit estándar, como en Linux/Windows). VM,
// intérprete y nativo (el comportamiento nativo del rol — portapapeles real — no se prueba
// sin display; aquí, la superficie y el borde de decodificación).
// ---------------------------------------------------------------------------
#[test]
fn edit_roles_and_edit_menu_on_all_three_engines() {
    let base = tmp("edit_roles_m255");
    std::fs::write(
        base.join("prog.ray"),
        r##"import std/ui;
fn main() {
    let std_items = [ui.item("role:undo", "", ""), ui.item("role:redo", "", ""), ui.separator(), ui.item("role:cut", "", ""), ui.item("role:copy", "", ""), ui.item("role:paste", "", ""), ui.separator(), ui.item("find", "Find...", "cmd+f"), ui.item("role:select_all", "", "")];
    match (ui.edit_menu(std_items)) { Result.Ok(_) => print("edit ok"), Result.Err(e) => print(e) }
    match (ui.set_menu_item("role:paste", false, false)) { Result.Ok(_) => print("paste greyed"), Result.Err(e) => print(e) }
    match (ui.edit_menu([ui.item("role:copy", "Copiar", "cmd+shift+c"), ui.item("role:close", "", "")])) { Result.Ok(_) => print("edit replaced"), Result.Err(e) => print(e) }
    match (ui.edit_menu([ui.item("role:paste", "", "cmd+bogus")])) { Result.Ok(_) => print("bad"), Result.Err(e) => print(e) }
    match (ui.menu("Tools", [ui.item("role:undo", "", "")])) { Result.Ok(_) => print("tools ok"), Result.Err(e) => print(e) }
}
"##,
    )
    .unwrap();
    const WANT: &str = "edit ok\npaste greyed\nedit replaced\nui: unsupported menu shortcut 'cmd+bogus'\ntools ok\n";
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(engine)
            .current_dir(&base)
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}\n{err}");
        assert!(err.contains("[ui] menu Edit at -1 items 9"), "el primer edit_menu crea el Edit: {err}");
        assert!(err.contains("[ui] replace menu Edit items 2"), "el segundo lo reemplaza: {err}");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).current_dir(&base).env("RAY_UI_BACKEND", "headless").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

// ---------------------------------------------------------------------------
// M252 (ray-sublime #76) — el evento `focused`: `focus(h)` lo emite con el handle (abrir no
// produce eventos en headless: la cola queda en silencio, ver las pruebas del aparcado).
// VM, intérprete y nativo.
// ---------------------------------------------------------------------------
#[test]
fn focused_events_follow_open_and_focus_on_all_three_engines() {
    let base = tmp("focused_m252");
    std::fs::write(
        base.join("prog.ray"),
        r##"import std/ui;
fn show(e: ui.UiEvent) {
    print(e.kind + " " + to_string(e.window) + " [" + e.tag + "]");
}
fn main() {
    let a = ui.open("A", "http://127.0.0.1:1/", 400, 300).unwrap();
    let b = ui.open("B", "http://127.0.0.1:1/", 400, 300).unwrap();
    let _ = ui.focus(b);
    show(ui.next_event().unwrap());
    let _ = ui.focus(a);
    show(ui.next_event().unwrap());
    print(to_string(a) + " " + to_string(b));
    close(b);
    show(ui.next_event().unwrap());
}
"##,
    )
    .unwrap();
    const WANT: &str = "focused 2 []\nfocused 1 []\n1 2\nclosed 2 []\n";
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(engine).current_dir(&base).env("RAY_UI_BACKEND", "headless").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{engine:?}\n{}", String::from_utf8_lossy(&out.stderr));
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()])
            .current_dir(&base)
            .output()
            .expect("build nativo");
        assert!(st.status.success(), "build --native ok\n{}", String::from_utf8_lossy(&st.stderr));
        let out = Command::new(&bin).current_dir(&base).env("RAY_UI_BACKEND", "headless").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}

/// M299 (findings 1.27.11 #12): `ui.MenuItem` ganó campos (`icon/enabled/checked`, 1.15) y las apps
/// que lo construían a mano dejaron de compilar sin pista. El error de campo ausente sugiere el
/// constructor público del módulo que devuelve el struct (rellena los defaults).
#[test]
fn missing_field_of_a_module_struct_suggests_its_constructor() {
    let base = tmp("ctor_hint");
    std::fs::write(
        base.join("prog.ray"),
        "import std/ui;\nfn main() { let m = ui.MenuItem { tag: \"a\", title: \"A\", shortcut: \"\" }; print(m.title); }\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "prog.ray"]).current_dir(&base).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("missing field 'icon' in the literal of 'std::ui::MenuItem' (use the constructor ui.item(string, string, string) — it fills the other fields with their defaults)"),
        "{err}"
    );
}
