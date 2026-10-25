//! rayreq/ray-sublime #245: `WindowOptions.place` abre una ventana en `x`/`y` (`place_at`) o
//! escalonada respecto a la anterior (`cascade`), y `ui.frame(h)` dice dónde está. Headless (el
//! backend real de macOS se verificó a mano; Linux y Windows comparten el código de colocación).

use std::process::Command;

const PROG: &str = r##"import std/ui;
fn show(r: Result<(int, int, int, int), string>) -> string {
    match (r) {
        Result.Ok(f) => "${f.0},${f.1},${f.2},${f.3}",
        Result.Err(e) => "err " + e,
    }
}
fn main() {
    let a = ui.open_with("A", "about:blank", ui.place_at(ui.options(600, 400), 100, 200)).unwrap();
    print("a " + show(ui.frame(a)));
    let b = ui.open_with("B", "about:blank", ui.cascade(ui.options(500, 300))).unwrap();
    print("b " + show(ui.frame(b)));
    let _ = ui.set_position(b, 10, 20);
    let _ = ui.set_size(b, 640, 480);
    print("b moved " + show(ui.frame(b)));
    var o = ui.options(300, 200);
    o.place = "nowhere";
    print("bad " + match (ui.open_with("C", "about:blank", o)) { Result.Ok(_) => "ok", Result.Err(e) => e });
    close(a);
    print("closed " + show(ui.frame(a)));
}
"##;

const WANT: &str = r##"a 100,200,600,400
b 122,222,500,300
b moved 10,20,640,480
bad ui: unsupported window placement 'nowhere' ("", at, cascade)
closed err ui: not an open window
"##;

#[test]
fn windows_open_where_asked_and_report_their_frame() {
    let dir = std::env::temp_dir().join(format!("ray_ui_place_{}", std::process::id()));
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
