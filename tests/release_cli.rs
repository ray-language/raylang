//! M248 (IDEAS §89): `ray keygen` + `ray release` — el lado del publicador de std/update. Un
//! proyecto mínimo con `[app] id`: keygen crea la semilla en RAY_KEYS_DIR y escribe la pública en
//! ray.toml; release empaqueta (ray bundle), comprime el bundle en un zip con un solo directorio
//! raíz, escribe `update.json` y su firma, y conserva los artefactos de otras plataformas de la
//! misma versión. Lo que sale se verifica con el propio `std/update` (firma, manifiesto, sha256)
//! y con `unzip -t` donde exista.

use std::path::{Path, PathBuf};
use std::process::Command;

fn ray(dir: &Path, keys: &Path, args: &[&str]) -> (String, String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).env("RAY_KEYS_DIR", keys).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string(), out.status.success())
}

fn project() -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("raylang_test_release_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(app.join("ray.toml"), "[package]\nname = \"hello\"\nversion = \"1.2.3\"\nentry = \"src/main.ray\"\n\n[app]\nid = \"dev.rayala.hello\"\n").unwrap();
    std::fs::write(app.join("src/main.ray"), "fn main() -> int {\n    print(\"hola\");\n    0\n}\n").unwrap();
    std::fs::write(
        app.join("verify.ray"),
        r#"import std/update;
import std/fs;
import std/zip;
fn main() -> int {
    let pk = args()[0];
    let m = fs.read_file_bytes("dist/update.json").unwrap();
    let sig = fs.read_file("dist/update.json.sig").unwrap();
    print(update.verify_signature(m, sig, pk));
    print(update.verify_signature(m, sig, "00" + pk.substring(2, pk.len())));
    let r = update.parse_manifest(from_utf8(m).unwrap(), update.platform_key()).unwrap();
    print(r.version + " " + r.url + " " + r.notes + " " + r.min_version);
    let data = fs.read_file_bytes("dist/" + r.url).unwrap();
    print(update.verify_package_bytes(r, data).is_ok());
    let a = zip.open(data).unwrap();
    var top = "";
    var files = 0;
    for e in zip.entries(a) {
        let first = e.name.substring(0, e.name.index_of("/").unwrap_or(e.name.len()));
        if (top == "") { top = first; } else if (top != first) { print("two roots: " + top + " " + first); }
        if (!e.name.ends_with("/")) { files = files + 1; }
    }
    print(top);
    print(files > 0);
    0
}
"#,
    )
    .unwrap();
    (app, base.join("keys"))
}

#[test]
fn keygen_then_release_produces_a_signed_manifest_the_app_verifies() {
    let (app, keys) = project();
    // keygen: semilla protegida + pública en ray.toml; una segunda vez sin --force se niega.
    let (out, err, ok) = ray(&app, &keys, &["keygen"]);
    assert!(ok, "keygen: {out}{err}");
    assert!(out.contains("ray.toml: [app] public_key updated"), "{out}");
    let toml = std::fs::read_to_string(app.join("ray.toml")).unwrap();
    let pk = toml.lines().find_map(|l| l.trim().strip_prefix("public_key = \"")).map(|v| v.trim_end_matches('"').to_string()).expect("public_key en ray.toml");
    assert_eq!(pk.len(), 64, "clave pública hex de 32 bytes: {pk}");
    assert!(keys.join("dev.rayala.hello.key").is_file());
    let (_, err, ok) = ray(&app, &keys, &["keygen"]);
    assert!(!ok && err.contains("already exists"), "segundo keygen: {err}");

    // release: bundle + zip + manifiesto + firma.
    let (out, err, ok) = ray(&app, &keys, &["release", "-o", "dist", "--notes", "https://example.dev/notes", "--min-version", "1.0.0"]);
    assert!(ok, "release: {out}{err}");
    let key = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let zip = app.join("dist").join(format!("hello-1.2.3-{key}.zip"));
    assert!(zip.is_file(), "zip en dist: {out}");
    assert!(app.join("dist/update.json").is_file() && app.join("dist/update.json.sig").is_file());
    let manifest = std::fs::read_to_string(app.join("dist/update.json")).unwrap();
    assert!(manifest.contains("\"app\": \"dev.rayala.hello\"") && manifest.contains("\"version\": \"1.2.3\""), "{manifest}");
    assert!(out.contains("artifact URLs are relative"), "{out}");

    // La app lo verifica con std/update: firma (y su rechazo con otra clave), manifiesto, sha256,
    // y el zip tiene un único directorio raíz (el bundle).
    let (vout, verr, vok) = ray(&app, &keys, &["run", "verify.ray", &pk]);
    assert!(vok, "verify: {vout}{verr}");
    let top = if cfg!(target_os = "macos") { "hello.app" } else { "hello" };
    assert_eq!(vout, format!("true\nfalse\n1.2.3 hello-1.2.3-{key}.zip https://example.dev/notes 1.0.0\ntrue\n{top}\ntrue\n"));

    // Un unzip del sistema también lo acepta (formato estándar: central directory, deflate).
    if let Ok(o) = Command::new("unzip").arg("-t").arg(&zip).output() {
        assert!(o.status.success(), "unzip -t: {}", String::from_utf8_lossy(&o.stdout));
    }

    // Segunda ejecución con un artefacto ajeno de la misma versión en el manifiesto: se conserva.
    let with_other = manifest.replace(
        "\"artifacts\": {\n",
        "\"artifacts\": {\n    \"plan9-mips\": {\"url\": \"hello-1.2.3-plan9-mips.zip\", \"sha256\": \"00\", \"size\": 1},\n",
    );
    std::fs::write(app.join("dist/update.json"), with_other).unwrap();
    let (out, err, ok) = ray(&app, &keys, &["release", "-o", "dist"]);
    assert!(ok, "release 2: {out}{err}");
    assert!(out.contains("2 artifact(s)"), "{out}");
    let manifest2 = std::fs::read_to_string(app.join("dist/update.json")).unwrap();
    assert!(manifest2.contains("plan9-mips") && manifest2.contains(&key), "{manifest2}");
    // …y la firma nueva sigue verificando.
    let (vout, verr, vok) = ray(&app, &keys, &["run", "verify.ray", &pk]);
    assert!(vok, "verify 2: {vout}{verr}");
    assert!(vout.starts_with("true\nfalse\n"), "{vout}");
}

#[test]
fn release_refuses_a_key_that_does_not_match_the_baked_public_key() {
    let (app, keys) = project();
    let (_, _, ok) = ray(&app, &keys, &["keygen"]);
    assert!(ok);
    let other = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    let (out, err, ok) = ray(&app, &keys, &["release", "-o", "dist", "--key", other]);
    assert!(!ok, "debería negarse: {out}");
    assert!(err.contains("does not match [app] public_key"), "{err}");
}
