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
