//! M55 — `ray templ`: templates compilados. Un `.ray.html` con `{% params %}` tipados se compila a
//! un módulo raylang (`pub fn render_<stem>(…) -> string`) que se importa como cualquier módulo.
//! El test genera el módulo, corre un programa que lo usa (golden por AMBOS motores: autoescape,
//! elif, for sobre arreglo y sobre rango, expresiones `{{ n * 10 }}`, literales con `${`/comillas),
//! verifica que un typo en una variable es ERROR DE COMPILACIÓN (la promesa del diseño), y cubre
//! los errores del propio template (sin params, endif faltante, etiqueta desconocida).

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

const TPL: &str = r#"{% params titulo: string, admin: bool, filas: [string], total: int %}
<html><body>
<h1>{{ titulo }}</h1>
{% if admin %}<p>modo admin</p>{% elif total > 0 %}<p>{{ total }} filas</p>{% else %}<p>vacío</p>{% endif %}
<ul>{% for fila in filas %}<li>{{ fila }}</li>{% endfor %}</ul>
<p>precio: "${simbolo}" & <raro></p>
{% for i in 0..2 %}[{{ i * 10 }}]{% endfor %}
</body></html>
"#;

const MAIN: &str = r#"import vistas/lista;

fn main() -> int {
    print(lista.render_lista("Informe & datos", false, ["a<b", "c"], 2));
    0
}
"#;

const ESPERADO: &str = "<html><body>\n\
<h1>Informe &amp; datos</h1>\n\
<p>2 filas</p>\n\
<ul><li>a&lt;b</li><li>c</li></ul>\n\
<p>precio: \"${simbolo}\" & <raro></p>\n\
[0][10]\n\
</body></html>\n\n";

fn proyecto(base: &std::path::Path, tpl: &str, main: &str) -> std::path::PathBuf {
    let app = base.join("app");
    std::fs::create_dir_all(app.join("vistas")).unwrap();
    std::fs::write(app.join("vistas/lista.ray.html"), tpl).unwrap();
    std::fs::write(app.join("main.ray"), main).unwrap();
    app
}

fn templ(app: &std::path::Path, arg: &str) -> (String, String, i32) {
    let out = Command::new(BIN).args(["templ", arg]).current_dir(app).output().expect("lanza ray templ");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn genera_y_renderiza_en_ambos_motores() {
    let base = std::env::temp_dir().join("ray_templ_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let app = proyecto(&base, TPL, MAIN);

    let (stdout, stderr, code) = templ(&app, "vistas");
    assert_eq!(code, 0, "ray templ falla:\n{stderr}");
    assert!(stdout.contains("generado:"), "{stdout}");
    let generado = std::fs::read_to_string(app.join("vistas/lista.ray")).unwrap();
    assert!(generado.contains("pub fn render_lista(titulo: string, admin: bool, filas: [string], total: int) -> string"), "{generado}");
    assert!(generado.contains("GENERADO por `ray templ`"), "cabecera de no-editar\n{generado}");

    // El módulo generado corre idéntico por ambos motores.
    for flags in [&[][..], &["--interp"][..]] {
        let mut args = vec!["run"];
        args.extend_from_slice(flags);
        args.push("main.ray");
        let out = Command::new(BIN).args(&args).current_dir(&app).output().unwrap();
        assert!(out.status.success(), "run falla ({flags:?}): {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), ESPERADO, "({flags:?})");
    }
}

#[test]
fn un_typo_en_una_variable_es_error_de_compilacion() {
    // La promesa del diseño: `{{ titulo }}` mal escrito NO es un "" silencioso (como en el motor
    // runtime), sino un error de tipos del módulo generado al compilar el programa.
    let base = std::env::temp_dir().join("ray_templ_cli_typo");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let tpl = "{% params titulo: string %}<h1>{{ titluo }}</h1>\n"; // typo: titluo
    let main = "import vistas/lista;\n\nfn main() -> int {\n    print(lista.render_lista(\"x\"));\n    0\n}\n";
    let app = proyecto(&base, tpl, main);

    let (_, stderr, code) = templ(&app, "vistas");
    assert_eq!(code, 0, "el template ES sintácticamente válido:\n{stderr}");
    let out = Command::new(BIN).args(["run", "main.ray"]).current_dir(&app).output().unwrap();
    assert_eq!(out.status.code(), Some(65), "debe fallar la compilación");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("titluo"), "el error señala el typo:\n{err}");
}

#[test]
fn run_y_build_regeneran_templates_desactualizados() {
    // M55: `ray run`/`ray build` regeneran los `.ray.html` cuyo `.ray` falte o esté viejo — no hay
    // que acordarse de `ray templ`. El aviso va por stderr (stdout es del programa).
    let base = std::env::temp_dir().join("ray_templ_autoregen");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let tpl = "{% params t: string %}<h1>{{ t }}</h1>\n";
    let main = "import vistas/lista;\n\nfn main() -> int {\n    print(lista.render_lista(\"hola\"));\n    0\n}\n";
    let app = proyecto(&base, tpl, main);

    // 1) Sin `ray templ` previo: el generado FALTA → `ray run` lo genera y el programa corre.
    let out = Command::new(BIN).args(["run", "main.ray"]).current_dir(&app).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("<h1>hola</h1>"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("template regenerado"), "aviso por stderr");

    // 2) Editar el template → el generado queda VIEJO → `ray run` lo regenera solo.
    std::thread::sleep(std::time::Duration::from_millis(30)); // mtime estrictamente posterior
    std::fs::write(app.join("vistas/lista.ray.html"), "{% params t: string %}<h2>{{ t }}</h2>\n").unwrap();
    let out = Command::new(BIN).args(["run", "main.ray"]).current_dir(&app).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("<h2>hola</h2>"), "corre el template NUEVO");

    // 3) Sin cambios: nada que regenerar (ni aviso).
    let out = Command::new(BIN).args(["build", "main.ray"]).current_dir(&app).output().unwrap();
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("regenerado"), "al día → silencio");

    // 4) Un template ROTO aborta el build con 65 y el error del template (mejor señal que
    //    compilar el generado viejo).
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(app.join("vistas/lista.ray.html"), "{% params t: string %}{% if t %}sin cierre\n").unwrap();
    let out = Command::new(BIN).args(["build", "main.ray"]).current_dir(&app).output().unwrap();
    assert_eq!(out.status.code(), Some(65));
    assert!(String::from_utf8_lossy(&out.stderr).contains("endif"), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn include_e_import_componen_vistas_y_layout() {
    // Composición M55: una página {% import %}a un partial y lo {% include %}; un layout es un
    // template más con param `contenido: string` que envuelve lo ya renderizado.
    let base = std::env::temp_dir().join("ray_templ_include");
    let _ = std::fs::remove_dir_all(&base);
    let app = base.join("app");
    std::fs::create_dir_all(app.join("vistas")).unwrap();
    std::fs::write(app.join("vistas/tarjeta.ray.html"),
        "{% params nombre: string %}<div class=\"tarjeta\">{{ nombre }}</div>\n").unwrap();
    std::fs::write(app.join("vistas/layout.ray.html"),
        "{% params titulo: string, contenido: string %}<html><title>{{ titulo }}</title><body>{% include contenido %}</body></html>\n").unwrap();
    std::fs::write(app.join("vistas/pagina.ray.html"),
        "{% params nombres: [string] %}{% import vistas/tarjeta %}<ul>{% for n in nombres %}{% include tarjeta.render_tarjeta(n) %}{% endfor %}</ul>\n").unwrap();
    std::fs::write(app.join("main.ray"),
        "import vistas/pagina;\nimport vistas/layout;\n\nfn main() -> int {\n    print(layout.render_layout(\"Equipo\", pagina.render_pagina([\"Ada\", \"Lin<us\"])));\n    0\n}\n").unwrap();

    // Sin `ray templ` explícito: la regeneración automática de `ray run` compila los tres.
    let out = Command::new(BIN).args(["run", "main.ray"]).current_dir(&app).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let esperado = "<html><title>Equipo</title><body><ul>\
        <div class=\"tarjeta\">Ada</div>\n\
        <div class=\"tarjeta\">Lin&lt;us</div>\n\
        </ul>\n</body></html>\n\n";
    assert_eq!(stdout, esperado, "el include NO re-escapa el HTML del partial, pero el partial SÍ escapó su dato");
}

#[test]
fn errores_del_template() {
    let base = std::env::temp_dir().join("ray_templ_cli_err");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    let casos: &[(&str, &str)] = &[
        ("<h1>hola</h1>", "params"),                                    // sin firma
        ("{% params x: int %}{% if x > 0 %}abierto", "endif"),          // sin cerrar
        ("{% params x: int %}{% bloque %}", "desconocida"),             // etiqueta inválida
        ("{% params x %}hola", "mal formado"),                          // params sin tipo
    ];
    let mut k = 0;
    for (tpl, espera) in casos {
        let app = base.join(format!("caso{k}"));
        std::fs::create_dir_all(app.join("vistas")).unwrap();
        std::fs::write(app.join("vistas/v.ray.html"), tpl).unwrap();
        let (_, stderr, code) = templ(&app, "vistas");
        assert_eq!(code, 65, "caso {k} debía fallar:\n{stderr}");
        assert!(stderr.contains(espera), "caso {k}: esperaba '{espera}' en:\n{stderr}");
        k += 1;
    }
}
