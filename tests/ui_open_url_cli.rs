//! rayreq #263: `ui.open_url` acepta solo `http`/`https`/`mailto` y rechaza un espacio o un
//! carácter de control, sin abrir nada en modo headless. VM e intérprete.

use std::process::Command;

const PROG: &str = r#"import std/ui;
fn show(r: Result<int, string>) -> string {
    match (r) { Result.Ok(_) => "ok", Result.Err(e) => "err " + e }
}
fn main() {
    print(show(ui.open_url("https://example.com/auth?x=1")));
    print(show(ui.open_url("mailto:a@b.c")));
    print(show(ui.open_url("file:///etc/passwd")));
    print(show(ui.open_url("https://e.com/a b")));
}
"#;

#[test]
fn open_url_takes_only_web_and_mail_urls() {
    let dir = std::env::temp_dir().join(format!("ray_open_url_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ray"), PROG).unwrap();
    let want = "ok\nok\nerr ui: open_url takes an http, https or mailto URL, not 'file:///etc/passwd'\nerr ui: open_url: the URL has a space or a control character (percent-encode it)\n";
    for flags in [&[][..], &["--interp"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .arg("run")
            .args(flags)
            .arg("a.ray")
            .env("RAY_UI_BACKEND", "headless")
            .current_dir(&dir)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "{flags:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}
