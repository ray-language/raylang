//! IDEAS §74 — `site/site.ray`: el generador del SITIO del lenguaje, escrito EN raylang
//! (dogfooding doble: std/markdown + los templates nativos `.ray.html` de `site/` con layout
//! heredado). Genera `index.html` (la landing), `spec.html` (SPEC.md renderizada empalmada
//! cruda con `{{& body }}`), los assets de marca con las fuentes EMBEBIDAS (autocontenido)
//! y el playground WASM. Salida determinista → ambos motores byte-idénticos.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

fn generate(out: &std::path::Path, interp: bool) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let tool = root.join("site/site.ray");
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

    // La landing: hero, instalación, muestra de código (tipeo + fallback) y enlace a la SPEC.
    let landing = std::fs::read_to_string(vm.join("index.html")).unwrap();
    assert!(landing.contains("producción real"), "hero\n{landing}");
    assert!(landing.contains("install.sh | sh"), "snippet de instalación\n{landing}");
    assert!(landing.contains("id=\"typed\""), "efecto de tipeo\n{landing}");
    assert!(landing.contains("<noscript>enum Tree"), "fallback sin JS\n{landing}");
    assert!(landing.contains("prefers-reduced-motion"), "tipeo accesible");
    assert!(landing.contains("href=\"spec.html\""), "enlace a la SPEC\n{landing}");
    // Los templates quedaron completamente resueltos (ni una directiva cruda en la salida).
    assert!(!landing.contains("{%"), "directivas sin resolver");

    // Las secciones nuevas: agentes LLM/MCP, playground embebido y ecosistema.
    assert!(landing.contains("id=\"agentes\""), "sección de agentes\n{landing}");
    assert!(landing.contains("claude mcp add raylang -- ray mcp"), "conexión MCP");
    assert!(landing.contains("llms.txt"), "mención de llms.txt");
    assert!(landing.contains("ray_check"), "tools MCP");
    assert!(
        landing.contains("iframe src=\"playground/index.html\""),
        "playground embebido\n{landing}"
    );
    assert!(landing.contains("id=\"ecosistema\""), "sección de ecosistema");
    assert!(
        landing.contains("github.com/ray-language/raycode") && landing.contains("raywatch"),
        "apps enlazadas\n{landing}"
    );
    assert!(landing.contains("ray-language/ray-index"), "enlace al registro");

    // La marca (assets/branding/raylang-brand.pdf): símbolo, mascota, paleta y fuentes.
    assert!(landing.contains("assets/symbol.svg"), "símbolo en la nav\n{landing}");
    assert!(landing.contains("assets/mascot.svg"), "Manta\n{landing}");
    assert!(landing.contains("#2b7ce0"), "azul raya (primario)\n{landing}");
    for asset in ["assets/symbol.svg", "assets/icon.svg", "assets/mascot.svg"] {
        let svg = std::fs::read_to_string(vm.join(asset)).unwrap_or_else(|_| panic!("falta {asset}"));
        assert!(svg.contains("<svg"), "asset SVG válido: {asset}");
    }

    // 100% autocontenido: fuentes EMBEBIDAS (woff2 byte-idénticos al origen), cero CDNs.
    assert!(landing.contains("@font-face"), "fuentes embebidas\n{landing}");
    assert!(
        !landing.contains("fonts.googleapis.com") && !landing.contains("https://cdn"),
        "sin recursos externos\n{landing}"
    );
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for font in ["space-grotesk.woff2", "jetbrains-mono.woff2"] {
        let copied = std::fs::read(vm.join("assets/fonts").join(font)).unwrap();
        let origin = std::fs::read(repo.join("playground/fonts").join(font)).unwrap();
        assert_eq!(copied, origin, "fuente copiada íntegra: {font}");
    }

    // El playground viaja completo con el sitio.
    let wasm = std::fs::read(vm.join("playground/raylang.wasm")).unwrap();
    assert!(wasm.starts_with(b"\0asm"), "wasm válido");
    let play = std::fs::read_to_string(vm.join("playground/index.html")).unwrap();
    assert!(play.contains("raylang"), "playground copiado");
    assert!(vm.join("playground/fonts/jetbrains-mono.woff2").exists(), "fuentes del playground");

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
