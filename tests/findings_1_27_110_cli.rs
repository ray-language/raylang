//! Ronda de hallazgos re-medidos sobre 1.27.110, cada uno con su repro mínimo.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_f110_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

/// rayserial #473: el aviso de `extern` variádico sale también cuando la declaración está en un
/// módulo importado (lo normal es agrupar los `extern` en uno aparte), con el nombre del módulo.
#[test]
fn a_variadic_extern_in_an_imported_module_warns() {
    let d = tmp("variadic_module");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("ray.toml"), "[package]\nname = \"v\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(
        d.join("src/sys.ray"),
        "extern \"c\" {\n    fn fcntl(fd: int, cmd: int, arg: int) -> int;\n    fn getpid() -> int;\n}\n\npub fn pid() -> int {\n    getpid()\n}\n",
    )
    .unwrap();
    std::fs::write(d.join("src/main.ray"), "import sys;\n\nfn main() -> int {\n    print(sys.pid() > 0);\n    0\n}\n").unwrap();
    for args in [&["check", "."][..], &["run"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {out}{err}");
        assert!(err.contains("[sys] warning at 2:5: `fcntl` is variadic in C"), "{args:?}: {err}");
    }
}

/// raycrawl #469: `url.escape_unsafe` limpia un `href` como el parser de URL del navegador: quita
/// espacios y controles de los extremos y los tabuladores y saltos de línea de dentro. Antes los
/// codificaba y un `href` absoluto indentado se resolvía como relativo.
#[test]
fn escape_unsafe_cleans_an_href_like_a_browser() {
    let d = tmp("escape_unsafe");
    let src = "import std/url;\n\nfn main() -> int {\n    print(url.escape_unsafe(\"  /a b\\n \"));\n    print(url.escape_unsafe(\"/p\\tq\"));\n    print(url.resolve(\"http://h/d/\", url.escape_unsafe(\"  /sp\\tace\\n\")));\n    print(url.escape_unsafe(\"/a b/é?q=x y\"));\n    print(url.escape_unsafe(\" \\n \").len());\n    0\n}\n";
    std::fs::write(d.join("prog.ray"), src).unwrap();
    let expected = "/a%20b\n/pq\nResult.Ok(http://h/space)\n/a%20b/%C3%A9?q=x%20y\n0\n";
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(&d, args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, expected, "{args:?}");
    }
}

/// ray-daw #465: con `RAY_AUDIO_SINK=null`, `played_ms` avanza al ritmo del reloj de pared
/// mientras se escribe. Antes iba a 0,82–0,85× (el sumidero dormía la duración de cada trozo).
/// Márgenes anchos, por la carga del CI: lo que se vigila es que no vuelva a ir un 15 % lento.
#[test]
fn the_null_audio_sink_keeps_real_time() {
    let d = tmp("null_sink_clock");
    let src = "import std/audio;\nimport std/time;\nimport std/collections/bytesbuilder;\n\nfn main() -> int {\n    let h = audio.open_format(48000, 2, \"s32le\", 20).unwrap();\n    var bb = bytesbuilder.new();\n    for _ in 0..(480 * 2 * 4) {\n        bb.push_byte(0);\n    }\n    let block = bb.build();\n    let t0 = time.monotonic();\n    let p0 = audio.played_ms(h).unwrap();\n    while (time.monotonic() - t0 < 2000) {\n        let _ = audio.write(h, block);\n    }\n    let wall = time.monotonic() - t0;\n    let played = audio.played_ms(h).unwrap() - p0;\n    print(played * 1000 / wall);\n    0\n}\n";
    std::fs::write(d.join("prog.ray"), src).unwrap();
    // Un hueco de más de 50 ms sin escribir no cuenta como reproducido (el reloj se re-ancla), así
    // que un runner cargado puede bajar la razón: se reintenta, y basta UNA medida en margen.
    let mut seen = Vec::new();
    for _ in 0..3 {
        let out = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["run", "prog.ray"])
            .env("RAY_AUDIO_SINK", "null")
            .current_dir(&d)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let permille: i64 = String::from_utf8_lossy(&out.stdout).trim().parse().expect("la razón en milésimas");
        if (930..=1040).contains(&permille) {
            return;
        }
        seen.push(permille);
    }
    panic!("played_ms / reloj de pared = {seen:?} por mil en tres intentos");
}
