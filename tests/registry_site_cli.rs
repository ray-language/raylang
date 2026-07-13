//! M84 — `tools/registry_site.ray`: el generador del sitio ESTÁTICO del registro, escrito
//! EN raylang (dogfooding: std/fs + StringBuilder + el mini-formato del índice). Genera
//! `index.html` (lista + búsqueda client-side) y `p/<nombre>.html` (versiones con insignias
//! retirada/firmada + dueño). Salida determinista → ambos motores byte-idénticos.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

fn fixture(base: &std::path::Path) -> std::path::PathBuf {
    let idx = base.join("idx");
    std::fs::create_dir_all(&idx).unwrap();
    std::fs::write(
        idx.join("mate.toml"),
        "# índice de mate\n[1.0.0]\ngit = \"git+https://ejemplo/mate@v1.0.0\"\nhash = \"sha256:aa\"\nsig = \"ed25519:bb\"\n\n[1.1.0]\ngit = \"git+https://ejemplo/mate@v1.1.0\"\nhash = \"sha256:cc\"\nyanked = \"true\"\n",
    )
    .unwrap();
    std::fs::write(idx.join("mate.owners.toml"), "owner = \"roberto\"\npubkey = \"ed25519:dd\"\n").unwrap();
    std::fs::write(idx.join("textutils.toml"), "[0.1.0]\ngit = \"git+https://ejemplo/textutils@v0.1.0\"\n").unwrap();
    idx
}

fn genera(idx: &std::path::Path, out: &std::path::Path, interp: bool) {
    let tool = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/registry_site.ray");
    let mut cmd = Command::new(BIN);
    if interp {
        cmd.arg("--interp");
    }
    let r = cmd.arg(&tool).arg(idx).arg(out).output().expect("ejecuta el generador");
    assert!(
        r.status.success(),
        "generador OK\n{}",
        String::from_utf8_lossy(&r.stderr)
    );
}

#[test]
fn genera_el_sitio_y_ambos_engines_matches() {
    let base = std::env::temp_dir().join("ray_registry_site");
    let _ = std::fs::remove_dir_all(&base);
    let idx = fixture(&base);
    let (vm, interp) = (base.join("site_vm"), base.join("site_in"));
    genera(&idx, &vm, false);
    genera(&idx, &interp, true);

    // Byte-idénticos por ambos motores.
    for rel in ["index.html", "p/mate.html", "p/textutils.html"] {
        let a = std::fs::read_to_string(vm.join(rel)).unwrap_or_else(|_| panic!("falta {rel}"));
        let b = std::fs::read_to_string(interp.join(rel)).unwrap();
        assert_eq!(a, b, "ambos engines idénticos en {rel}");
    }

    // Contenido clave.
    let portada = std::fs::read_to_string(vm.join("index.html")).unwrap();
    assert!(portada.contains("p/mate.html") && portada.contains("p/textutils.html"), "{portada}");
    assert!(portada.contains("<input id=\"q\""), "búsqueda client-side\n{portada}");
    let mate = std::fs::read_to_string(vm.join("p/mate.html")).unwrap();
    assert!(mate.contains("dueño: roberto"), "{mate}");
    assert!(mate.contains("nombre reclamado"), "{mate}");
    assert!(mate.contains(">retirada<") && mate.contains(">firmada<"), "insignias\n{mate}");
    assert!(mate.contains("ray add mate"), "{mate}");
    // La 1.1.0 (más nueva) se lista antes que la 1.0.0.
    let p110 = mate.find("1.1.0").unwrap();
    let p100 = mate.find("1.0.0").unwrap();
    assert!(p110 < p100, "las versiones van de new a vieja");
    // El paquete sin reclamar no muestra dueño.
    let tx = std::fs::read_to_string(vm.join("p/textutils.html")).unwrap();
    assert!(!tx.contains("reclamado"), "{tx}");
}
