//! M458 (pepa #418) — `std/locale`: los idiomas del sistema como etiquetas BCP 47. Con
//! `RAY_LANGUAGES` el resultado es el mismo en la VM y en el intérprete, sea cual sea la máquina.

use std::process::Command;

const PROG: &str = r#"import std/locale;
fn main() -> int {
    print(locale.preferred_languages().join(","));
    print(locale.pick(["en", "es"], "en"));
    print(locale.pick(["en", "ES-cl"], "en"));
    print(locale.pick(["de", "fr"], "en"));
    0
}
"#;

fn run(flags: &[&str], languages: &str) -> String {
    let d = std::env::temp_dir().join(format!("ray_locale_{}_{}", std::process::id(), flags.len()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    args.push("prog.ray");
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(&args).env("RAY_LANGUAGES", languages).current_dir(&d).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let _ = std::fs::remove_dir_all(&d);
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn preferred_languages_are_normalized_and_picked_in_order() {
    // Forma POSIX y BCP 47 mezcladas, con un repetido y un `C` que no es un idioma.
    let want = "pt-BR,es-CL,zh-Hans-CN\nes\nES-cl\nen\n";
    assert_eq!(run(&[], "pt_BR.UTF-8,es-cl,C,es-CL,zh-hans-cn"), want, "VM");
    assert_eq!(run(&["--interp"], "pt_BR.UTF-8,es-cl,C,es-CL,zh-hans-cn"), want, "intérprete");
}

#[test]
fn the_system_answer_never_fails() {
    // Sin la variable manda el sistema: sea lo que sea, el programa corre y cada etiqueta es
    // BCP 47 (letras, dígitos y guiones).
    let d = std::env::temp_dir().join(format!("ray_locale_sys_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).env_remove("RAY_LANGUAGES").current_dir(&d).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    let first = out.lines().next().unwrap_or("");
    assert!(first.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ','), "{first:?}");
    let _ = std::fs::remove_dir_all(&d);
}
