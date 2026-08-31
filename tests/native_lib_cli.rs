//! §80b — `ray build --native --lib`: la LIBRERÍA estática con entrada C (`ray_start`). El
//! test es el SHELL en miniatura: compila el staticlib para el HOST, verifica con `nm` que los
//! exports sobreviven al LTO, linkea un driver C que hace exactamente lo que hará la app iOS
//! (registrar handlers → ray_start → recibir open/eval → empujar un evento) y lo corre.

use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

const PROG: &str = r#"import std/ui;

fn main() {
    match (ui.open("Shell App", "http://127.0.0.1:9999/", 375, 667)) {
        Result.Err(e) => print("open failed: " + e),
        Result.Ok(h) => {
            print("window: " + to_string(h));
            let _ = ui.eval_js(h, "console.log('hi')");
            match (ui.next_event()) {
                Result.Ok(e) => print("event: " + e.kind + " tag=" + e.tag),
                Result.Err(e) => print("err: " + e),
            }
        },
    }
    print("program done");
}
"#;

const DRIVER: &str = r#"
#include <stdio.h>
#include <unistd.h>

extern void ray_ui_set_handlers(void (*open)(const char*, const char*),
                                void (*eval)(const char*));
extern void ray_ui_push_event(const char* kind, long long window, const char* tag);
extern int ray_start(void);

static void on_open(const char* title, const char* url) {
    printf("SHELL OPEN title=%s url=%s\n", title, url);
    fflush(stdout);
}
static void on_eval(const char* js) {
    printf("SHELL EVAL %s\n", js);
    fflush(stdout);
}

int main(void) {
    ray_ui_set_handlers(on_open, on_eval);
    if (ray_start() != 0) return 1;
    usleep(500 * 1000);
    ray_ui_push_event("lifecycle", 0, "background");
    usleep(800 * 1000);
    return 0;
}
"#;

#[test]
#[cfg(target_os = "macos")] // el link del driver usa los frameworks del backend mac del .a
fn the_static_library_drives_a_c_shell_end_to_end() {
    if !have("rustc") || !have("cc") {
        return;
    }
    let base = std::env::temp_dir().join("ray_native_lib");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("prog.ray"), PROG).unwrap();
    std::fs::write(base.join("driver.c"), DRIVER).unwrap();

    let lib = base.join("libprog.a");
    let st = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "prog.ray", "--native", "--lib", "-o", lib.to_str().unwrap()])
        .current_dir(&base)
        .output()
        .expect("build --lib");
    assert!(st.status.success(), "build --native --lib ok\n{}", String::from_utf8_lossy(&st.stderr));

    // Los exports sobreviven al fat-LTO (la duda clásica de un staticlib con no_mangle).
    let nm = Command::new("nm").arg("-gU").arg(&lib).output().expect("nm");
    let syms = String::from_utf8_lossy(&nm.stdout);
    for sym in ["_ray_start", "_ray_ui_set_handlers", "_ray_ui_push_event"] {
        assert!(syms.contains(sym), "export {sym} presente en el .a");
    }

    // El shell en miniatura: mismo contrato que la app iOS generada.
    let exe = base.join("shell_driver");
    let cc = Command::new("cc")
        .arg(base.join("driver.c"))
        .arg(&lib)
        .arg("-o")
        .arg(&exe)
        .args(["-framework", "AppKit", "-framework", "WebKit", "-framework", "Foundation", "-lobjc"])
        .output()
        .expect("cc");
    assert!(cc.status.success(), "link del driver\n{}", String::from_utf8_lossy(&cc.stderr));

    let out = Command::new(&exe).output().expect("corre el driver");
    assert_eq!(out.status.code(), Some(0), "driver exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Orden causal completo: open por el handler → eval por el handler → el programa reporta
    // su ventana → recibe el evento que empujó el shell → termina (sin matar al proceso).
    let want = [
        "SHELL OPEN title=Shell App url=http://127.0.0.1:9999/",
        "SHELL EVAL console.log('hi')",
        "window: 1",
        "event: lifecycle tag=background",
        "program done",
    ];
    let mut at = 0;
    for needle in want {
        let pos = stdout[at..].find(needle);
        assert!(pos.is_some(), "'{needle}' en orden; salida:\n{stdout}");
        at += pos.unwrap();
    }
}
