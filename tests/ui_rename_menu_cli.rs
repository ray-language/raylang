//! rayreq #252: `ui.rename_menu(old, new)` cambia el título de un menú de nivel superior sin tocar
//! sus ítems; después `replace_menu` lo encuentra por el título nuevo. Headless, VM e intérprete
//! (la barra real de macOS se verificó a mano con el mismo programa).

use std::process::Command;

const PROG: &str = r##"import std/ui;
fn show(r: Result<int, string>) -> string {
    match (r) { Result.Ok(_) => "ok", Result.Err(e) => "err " + e }
}
fn main() {
    print("menu " + show(ui.menu("File", [ui.item("new", "New", "cmd+n")])));
    print("menu " + show(ui.menu("Help", [ui.item("docs", "Docs", "")])));
    print("rename " + show(ui.rename_menu("File", "Archivo")));
    print("replace old " + show(ui.replace_menu("File", [ui.item("x", "X", "")])));
    print("replace new " + show(ui.replace_menu("Archivo", [ui.item("nuevo", "Nuevo", "cmd+n")])));
    print("taken " + show(ui.rename_menu("Archivo", "Help")));
    print("missing " + show(ui.rename_menu("Nope", "Otro")));
    print("empty " + show(ui.rename_menu("Archivo", "")));
}
"##;

const WANT: &str = r##"menu ok
menu ok
rename ok
replace old err ui: no menu titled 'File'
replace new ok
taken err ui: there is already a menu titled 'Help'
missing err ui: no menu titled 'Nope'
empty err ui: a menu title cannot be empty
"##;

#[test]
fn a_menu_can_be_renamed() {
    let dir = std::env::temp_dir().join(format!("ray_ui_rename_menu_{}", std::process::id()));
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

/// ray-daw #434: `ui.set_menu_title(tag, title)` cambia el texto de un ítem por su tag.
const TITLE_PROG: &str = r##"import std/ui;
fn show(r: Result<int, string>) -> string {
    match (r) { Result.Ok(_) => "ok", Result.Err(e) => "err " + e }
}
fn main() {
    print("menu " + show(ui.edit_menu([ui.item("page:undo", "Undo", "cmd+z")])));
    print("title " + show(ui.set_menu_title("page:undo", "Undo Move clip")));
    print("state " + show(ui.set_menu_item("page:undo", true, false)));
    print("missing " + show(ui.set_menu_title("nope", "X")));
    print("empty " + show(ui.set_menu_title("page:undo", "")));
    print("no tag " + show(ui.set_menu_title("", "X")));
}
"##;

const TITLE_WANT: &str = r##"menu ok
title ok
state ok
missing err ui: no menu item with tag 'nope'
empty err ui: a menu item title cannot be empty
no tag err ui: set_menu_title needs a tag
"##;

#[test]
fn a_menu_item_can_be_retitled() {
    let dir = std::env::temp_dir().join(format!("ray_ui_menu_title_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ray"), TITLE_PROG).unwrap();
    for flags in [&[][..], &["--interp"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .arg("run")
            .args(flags)
            .arg("a.ray")
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_TRACE", "1")
            .current_dir(&dir)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), TITLE_WANT, "{flags:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// ray-sublime #436: `accept_drops` entrega los archivos soltados como evento `drop`, con las
/// rutas y el punto (headless: la suelta de prueba de `RAY_UI_DROP`).
const DROP_PROG: &str = r##"import std/ui;
fn main() {
    let h = ui.open("t", "about:blank", 300, 200).unwrap();
    print("accept " + to_string(ui.accept_drops(h, true).is_ok()));
    print("closed window " + to_string(ui.accept_drops(999, true).is_err()));
    loop_events(h);
}
fn loop_events(h: int) {
    var n = 0;
    while (n < 5) {
        n = n + 1;
        match (ui.next_event_timeout(1000)) {
            Result.Ok(Option.Some(e)) => {
                if (e.kind == "drop") {
                    let p = ui.drop_point(e);
                    print("drop " + to_string(e.window == h) + " at " + to_string(p.0) + "," + to_string(p.1));
                    for path in ui.drop_paths(e) { print(path); }
                    return;
                }
            },
            _ => {},
        }
    }
    print("no drop");
}
"##;

#[test]
fn dropped_files_arrive_as_an_event() {
    let dir = std::env::temp_dir().join(format!("ray_ui_drop_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ray"), DROP_PROG).unwrap();
    for flags in [&[][..], &["--interp"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .arg("run")
            .args(flags)
            .arg("a.ray")
            .env("RAY_UI_BACKEND", "headless")
            .env("RAY_UI_DROP", "/tmp/a b.png:/tmp/c.ray")
            .current_dir(&dir)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "accept true\nclosed window true\ndrop true at 10,20\n/tmp/a b.png\n/tmp/c.ray\n",
            "{flags:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
