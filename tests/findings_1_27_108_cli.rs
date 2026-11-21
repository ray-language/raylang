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

/// Revisión de 1.27.109: (1) la colisión de #456 sobrevivía con un pipeline dentro de la
/// interpolación (la llamada está en la posición del lado derecho y su primer argumento en la de
/// la envoltura); (2) `q.to_string()` sobre un tipo con `Show` usa su `show`, como `to_string(q)`;
/// (3) `to_string` como valor sobre un tipo con `Show` se rechaza en vez de ignorar el `show`.
#[test]
fn to_string_follows_the_user_show_in_every_form() {
    let d = tmp("to_string_forms");
    let src = "struct Q {\n    n: int,\n}\n\nimpl Show for Q {\n    fn show(self) -> string {\n        \"Q!${self.n}\"\n    }\n}\n\nfn wrap(s: string) -> string {\n    \"<\" + s + \">\"\n}\n\nfn mk(s: string) -> Q {\n    Q { n: s.len() }\n}\n\nfn main() -> int {\n    let q = Q { n: 9 };\n    print(\"${to_string(q) |> wrap}\");\n    print(\"${to_string(q) |> mk}\");\n    print(q.to_string());\n    print(\"${q.to_string()}\");\n    0\n}\n";
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let expected = "<Q!9>\nQ!3\nQ!9\nQ!9\n";
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, expected, "{args:?}");
    }
    let (out, err, code) = ray(&d, &["build", "--native", "prog.ray", "-o", "prog"]);
    assert_eq!(code, 0, "{out}{err}");
    let native = Command::new(d.join("prog")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&native.stdout), expected);

    let bad = "struct Q {\n    n: int,\n}\n\nimpl Show for Q {\n    fn show(self) -> string {\n        \"Q\"\n    }\n}\n\nfn main() -> int {\n    let qs = [Q { n: 1 }];\n    print(qs.map(to_string));\n    0\n}\n";
    std::fs::write(d.join("bad.ray"), bad).unwrap();
    let (_, err, code) = ray(&d, &["run", "bad.ray"]);
    assert_ne!(code, 0);
    assert!(err.contains("Q has its own 'show'; pass a function that calls it"), "{err}");
}

/// Revisión de 1.27.109: el aviso de variádicas es solo para libc; una función homónima de otra
/// librería no avisa.
#[test]
fn a_same_named_function_of_another_library_is_not_a_libc_variadic() {
    let d = tmp("variadic_other_lib");
    let src = "extern \"mylib\" {\n    fn open(a: int, b: int, c: int) -> int;\n}\n\nfn main() -> int {\n    0\n}\n";
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let (_, err, code) = ray(&d, &["check", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(!err.contains("variadic"), "{err}");
}

/// `ray check packages/db/bson.ray`: un módulo suelto con una función de nombre de PRELUDE
/// (`fn get`) se comprobaba como raíz, su `get` redefinía la del prelude y `m.get(k)` sobre un
/// `Map` ligaba a ella. Ahora da el mismo veredicto que dentro del proyecto, y la llamada directa
/// `get(xs, …)` del propio módulo sigue yendo a la suya.
#[test]
fn a_loose_module_keeps_the_prelude_function_it_shares_a_name_with() {
    let d = tmp("loose_prelude_name");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"loose\"\nversion = \"0.1.0\"\n").unwrap();
    let module = "pub struct Field {\n    name: string,\n    value: int,\n}\n\npub fn get(fields: [Field], name: string) -> int {\n    for f in fields {\n        if (f.name == name) {\n            return f.value;\n        }\n    }\n    0\n}\n\npub fn total(m: Map<string, int>, fields: [Field]) -> int {\n    m.get(\"a\").unwrap_or(0) + get(fields, \"b\") + fields.get(\"b\") - 2\n}\n";
    std::fs::write(d.join("src/store.ray"), module).unwrap();
    std::fs::write(
        d.join("src/main.ray"),
        "import store;\n\nfn main() -> int {\n    let m: Map<string, int> = Map.new();\n    m.insert(\"a\", 40);\n    print(store.total(m, [store.Field { name: \"b\", value: 2 }]));\n    0\n}\n",
    )
    .unwrap();
    let (out, err, code) = ray(&d, &["check", "src/store.ray"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("compiles (module without main)"), "{out}{err}");
    let (out, err, code) = ray(&d, &["run"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "42\n");
    // Revisión de 1.27.110: una suite (entrada sin `main`) con pruebas de nombre de prelude las
    // sigue corriendo todas; el renombrado de arriba no las toca.
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(
        d.join("tests/names_test.ray"),
        "@test\nfn filter() {\n    assert(false);\n}\n\n@test\nfn sort() {\n    assert(true);\n}\n\n@test\nfn plain() {\n    assert(true);\n}\n",
    )
    .unwrap();
    let (out, err, code) = ray(&d, &["test"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("1 of 3 test(s) failed"), "{out}{err}");
}

/// Una función de la raíz con el nombre de una del prelude que es raylang puro (`try_call`): los
/// módulos siguen llamando a la del prelude (el alias `try_call#prelude`). En nativo el alias se
/// trataba como el nombre pelado, su tipo salía el de la función de la raíz y la función del
/// módulo que lo llamaba se emitía como un stub que fallaba al ejecutarse (así dejó de correr el
/// handler de `net/grpc_server` en un programa que definía su propio `try_call`).
#[test]
fn a_root_function_named_like_a_prelude_one_does_not_break_modules_natively() {
    let d = tmp("root_try_call");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"tc\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(
        d.join("src/guard.ray"),
        "pub fn safe(f: fn() -> int) -> string {\n    match (try_call(f)) {\n        Result.Ok(v) => \"ok ${v}\",\n        Result.Err(e) => \"caught: ${e}\",\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(
        d.join("src/main.ray"),
        "import guard;\n\nfn try_call(label: string, n: int) {\n    print(\"mine: ${label} ${n}\");\n}\n\nfn main() -> int {\n    try_call(\"root\", 1);\n    print(guard.safe(fn() -> int { 41 + 1 }));\n    print(guard.safe(fn() -> int {\n        panic(\"boom\");\n        0\n    }));\n    0\n}\n",
    )
    .unwrap();
    let expected = "mine: root 1\nok 42\ncaught: boom\n";
    let (out, err, code) = ray(&d, &["run"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, expected);
    let (out, err, code) = ray(&d, &["build", "--native", "-o", "tc"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(!err.contains("emitted as stubs") && !out.contains("emitted as stubs"), "{out}{err}");
    let native = Command::new(d.join("tc")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&native.stdout), expected, "{}", String::from_utf8_lossy(&native.stderr));
}
