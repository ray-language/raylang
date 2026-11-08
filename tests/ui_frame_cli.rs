//! M441 (rayreq #312/#316): `ui.set_appearance`, `ui.screens`, el evento `frame` y
//! `ui.event_frame`. Headless (el backend real de macOS se verificó a mano; Linux y Windows se
//! comprueban en compilación cruzada): `set_size`/`set_position` emiten `frame` con el marco
//! nuevo, los `frame` consecutivos de una ventana se funden y `screens` da la pantalla fija.

use std::process::Command;

const PROG: &str = r##"import std/ui;
fn main() {
    let w = ui.open("Demo", "about:blank", 400, 300).unwrap();
    print(ui.set_appearance(w, "dark"));
    print(ui.set_appearance(0, "system"));
    print(ui.set_appearance(w, "sepia"));
    print(ui.screens());
    let _ = ui.set_size(w, 640, 480);
    let _ = ui.set_position(w, 10, 20);
    let e = ui.next_event().unwrap();
    print(e.kind + " " + e.tag);
    print(ui.event_frame(e));
    print(ui.event_frame(ui.UiEvent { kind: "closed", window: w, tag: "" }));
    print(ui.next_event_timeout(50).unwrap().is_none());
}
"##;

const WANT: &str = r##"Result.Ok(0)
Result.Ok(0)
Result.Err(ui: unknown appearance 'sepia' (system | light | dark))
Result.Ok([(0, 0, 1440, 900)])
frame 10,20,640,480
Option.Some((10, 20, 640, 480))
Option.None
true
"##;

#[test]
fn appearance_screens_and_the_frame_event_work_headless() {
    let dir = std::env::temp_dir().join(format!("ray_ui_frame_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ray"), PROG).unwrap();
    for flags in [&[][..], &["--interp"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .arg("run")
            .args(flags)
            .arg("a.ray")
            .env("RAY_UI_BACKEND", "headless")
            .current_dir(&dir)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{flags:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}
