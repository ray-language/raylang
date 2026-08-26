//! IDEAS §74 — `tools/site.ray`: el generador del SITIO del lenguaje, escrito EN raylang
//! (dogfooding: std/markdown + std/fs + StringBuilder). Genera `index.html` (la landing)
//! y `spec.html` (SPEC.md renderizada). Salida determinista → ambos motores byte-idénticos.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

fn generate(out: &std::path::Path, interp: bool) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let tool = root.join("tools/site.ray");
    let mut cmd = Command::new(BIN);
    if interp {
        cmd.arg("--interp");
    }
    let r = cmd.arg(&tool).arg(root).arg(out).output().expect("ejecuta el generador");
    assert!(
        r.status.success(),
        "generador OK\n{}",
        String::from_utf8_lossy(&r.stderr)
    );
}

#[test]
fn generates_the_site_and_both_engines_match() {
    let base = std::env::temp_dir().join("ray_lang_site");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let (vm, interp) = (base.join("site_vm"), base.join("site_in"));
    generate(&vm, false);
    generate(&interp, true);

    // Byte-idénticos por ambos motores.
    for rel in ["index.html", "spec.html"] {
        let a = std::fs::read_to_string(vm.join(rel)).unwrap_or_else(|_| panic!("falta {rel}"));
        let b = std::fs::read_to_string(interp.join(rel)).unwrap();
        assert_eq!(a, b, "ambos engines idénticos en {rel}");
    }

    // La landing: hero, instalación, muestra de código y enlace a la SPEC.
    let landing = std::fs::read_to_string(vm.join("index.html")).unwrap();
    assert!(landing.contains("<h1>raylang</h1>"), "{landing}");
    assert!(landing.contains("install.sh | sh"), "snippet de instalación\n{landing}");
    assert!(landing.contains("enum Tree"), "muestra de código\n{landing}");
    assert!(landing.contains("href=\"spec.html\""), "enlace a la SPEC\n{landing}");

    // La SPEC renderizada: título y estructura reales de SPEC.md, pasados por std/markdown.
    let spec = std::fs::read_to_string(vm.join("spec.html")).unwrap();
    assert!(
        spec.contains("Especificación del lenguaje raylang"),
        "título de la SPEC\n… {} …",
        &spec[..400]
    );
    assert!(spec.contains("<h2>"), "secciones renderizadas");
    assert!(spec.contains("normativo"), "nota de normatividad");
}
