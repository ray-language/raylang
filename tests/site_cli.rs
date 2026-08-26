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
    // El "fantasma" con el código final dimensiona el contenedor (sin saltos) y es a la
    // vez el fallback resaltado sin JS / con movimiento reducido.
    assert!(landing.contains("id=\"ghost\""), "fantasma dimensionador\n{landing}");
    assert!(landing.contains("pre.typing #typed { position: absolute"), "tipeo superpuesto");
    assert!(landing.contains("prefers-reduced-motion"), "tipeo accesible");
    assert!(landing.contains("href=\"spec.html\""), "enlace a la SPEC\n{landing}");
    // Los templates quedaron completamente resueltos (ni una directiva cruda en la salida).
    assert!(!landing.contains("{%"), "directivas sin resolver");

    // La nav: LLMs primero (orden del contenido), anclas same-page con scrollspy en la
    // landing y absolutas en la SPEC (que además marca su propio ítem activo), icono de
    // GitHub y selector de tema claro/sistema/oscuro persistido.
    let llms = landing.find("data-spy=\"agentes\"").expect("spy de LLMs");
    let play_link = landing.find("data-spy=\"playground\"").expect("spy del playground");
    assert!(llms < play_link, "LLMs va primero en la nav");
    assert!(landing.contains("href=\"#agentes\""), "ancla same-page en la landing");
    assert!(landing.contains("aria-label=\"GitHub\""), "icono de GitHub\n{landing}");
    assert!(!landing.contains(">GitHub</a>"), "el texto GitHub se fue de la nav");
    for mode in ["light", "system", "dark"] {
        assert!(landing.contains(&format!("data-mode=\"{mode}\"")), "modo de tema {mode}");
    }
    assert!(landing.contains("data-theme"), "aplicación del tema");
    assert!(landing.contains("scroll-margin-top"), "anclas bajo la nav sticky");

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

    // El playground viaja completo con el sitio; el wasm y el bundle del editor son
    // artefactos de `playground/build.sh` (no versionados) → se aseveran solo si están.
    let play = std::fs::read_to_string(vm.join("playground/index.html")).unwrap();
    assert!(play.contains("editor.bundle.js"), "el playground usa el editor real");
    assert!(play.contains("wasm.lsp"), "el playground habla con el LSP embebido");
    assert!(vm.join("playground/fonts/jetbrains-mono.woff2").exists(), "fuentes del playground");
    if repo.join("playground/raylang.wasm").exists() {
        let wasm = std::fs::read(vm.join("playground/raylang.wasm")).unwrap();
        assert!(wasm.starts_with(b"\0asm"), "wasm válido");
    }
    if repo.join("playground/editor.bundle.js").exists() {
        assert!(vm.join("playground/editor.bundle.js").exists(), "bundle del editor copiado");
    }

    // La SPEC renderizada: título y estructura reales de SPEC.md, pasados por std/markdown.
    let spec = std::fs::read_to_string(vm.join("spec.html")).unwrap();
    assert!(
        spec.contains("Especificación del lenguaje raylang"),
        "título de la SPEC\n… {} …",
        &spec[..400]
    );
    assert!(spec.contains("<h2>"), "secciones renderizadas");
    assert!(spec.contains("normativo"), "nota de normatividad");
    assert!(spec.contains("class=\"link active\""), "Especificación activa en su nav");
    assert!(spec.contains("index.html#agentes"), "anclas absolutas desde la SPEC");
}
