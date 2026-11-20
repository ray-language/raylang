//! Ronda de hallazgos re-medidos sobre 1.27.108, cada uno con su repro mínimo.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_f108_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

/// rayserial #460: declarar con aridad fija una variádica de libc (`fcntl`, `ioctl`) compila, pero
/// en Apple arm64 la llamada no hace nada o deja basura; el checker avisa. `open` con dos
/// argumentos no usa la parte variádica y no avisa.
#[test]
fn an_extern_that_declares_a_variadic_libc_function_warns() {
    let d = tmp("variadic_extern");
    let src = "extern \"c\" {\n    fn fcntl(fd: int, cmd: int, arg: int) -> int;\n    fn open(path: string, flags: int) -> int;\n    fn getpid() -> int;\n}\n\nfn main() -> int {\n    print(getpid() > 0);\n    0\n}\n";
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let (out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "true\n");
    assert!(err.contains("warning at 2:5: `fcntl` is variadic in C after its first 2 parameter(s) and `extern` cannot pass variadic arguments"), "{err}");
    assert!(!err.contains("`open`"), "{err}");
}

/// rayfocus #459: un enum o un struct sin `Show` se interpola con la forma por defecto, la misma
/// que muestra `print`. Antes `"${kind}"` era un error de tipo.
#[test]
fn a_struct_or_an_enum_without_show_interpolates_like_print() {
    let d = tmp("interp_default_form");
    let src = "enum K {\n    A,\n    B(int),\n}\n\nstruct P {\n    x: int,\n    k: K,\n}\n\nfn main() -> int {\n    print(\"k: ${K.A} ${K.B(3)}\");\n    let p = P { x: 1, k: K.A };\n    print(\"${p}\");\n    print(p);\n    print(to_string(K.A).len());\n    0\n}\n";
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let expected = "k: K.A K.B(3)\nP { x: 1, k: K.A }\nP { x: 1, k: K.A }\n3\n";
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, expected, "{args:?}");
    }
    let (out, err, code) = ray(&d, &["build", "--native", "prog.ray", "-o", "prog"]);
    assert_eq!(code, 0, "{out}{err}");
    let native = Command::new(d.join("prog")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&native.stdout), expected);
}

/// ray-daw #455: `ray release --help` y `ray keygen --help` explican sus opciones y salen con 0
/// (antes: la línea de uso y 64). Un flag desconocido sigue siendo error de uso.
#[test]
fn release_and_keygen_help_explain_their_options() {
    let d = tmp("release_help");
    let (out, err, code) = ray(&d, &["release", "--help"]);
    assert_eq!(code, 0, "{err}");
    for needle in ["usage: ray release", "--publish", "--repo OWNER/NAME", "--no-sign", "ray release seal <dist>", "private"] {
        assert!(out.contains(needle), "falta {needle}: {out}");
    }
    let (out, err, code) = ray(&d, &["keygen", "--help"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("usage: ray keygen") && out.contains("--force") && out.contains("[app] public_key"), "{out}");
    let (out, err, code) = ray(&d, &["release", "--bogus"]);
    assert_eq!(code, 64, "{out}");
    assert!(err.starts_with("usage: ray release") && err.lines().count() == 1, "{err}");
}

/// rayfocus #458: en headless, con el permiso sin decidir, lo programado espera la respuesta (no
/// sale en `pending()`), como en iOS y macOS; `RAY_UI_NOTIFY_PERMISSION_ANSWER` simula la respuesta
/// al pedir el permiso.
#[test]
fn a_headless_notification_waits_while_the_permission_is_undecided() {
    let d = tmp("notify_undecided");
    let src = "import std/notify;\n\nfn main() -> int {\n    let id = notify.schedule(notify.message(\"t\", \"b\"), notify.Trigger.After(3600, false));\n    print(id.is_ok());\n    print(notify.pending().unwrap().len());\n    print(notify.request_permission());\n    print(notify.pending().unwrap().len());\n    0\n}\n";
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let run = |answer: Option<&str>| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ray"));
        c.args(["run", "prog.ray"]).current_dir(&d).env("RAY_UI_BACKEND", "headless").env("RAY_UI_NOTIFY_PERMISSION", "not_determined");
        match answer {
            Some(a) => c.env("RAY_UI_NOTIFY_PERMISSION_ANSWER", a),
            None => c.env_remove("RAY_UI_NOTIFY_PERMISSION_ANSWER"),
        };
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    assert_eq!(run(None), "true\n0\nResult.Ok(not_determined)\n0\n");
    assert_eq!(run(Some("granted")), "true\n0\nResult.Ok(granted)\n1\n");
    assert_eq!(run(Some("denied")), "true\n0\nResult.Ok(denied)\n0\n");
}
