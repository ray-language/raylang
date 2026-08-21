//! Tests de `lsp` (movimiento puro; usar `git log --follow`).

use super::*;

#[test]
fn analyze_all_public_various_errors() {
    // M33c: dos errores de tipos → dos diagnósticos.
    let ds = analyze_all("fn f() -> int { 1 + true }\nfn g() -> int { \"x\" * 2 }\nfn main() -> int { 0 }");
    assert_eq!(ds.len(), 2, "{:?}", ds.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert_eq!((ds[0].line, ds[1].line), (1, 2));
    // Dos errores de sintaxis → dos diagnósticos (recuperación del parser)…
    let ds = analyze_all("fn f() -> int { let = 1; 0 }\nfn g() -> int { 2 + }\nfn main() -> int { 0 }");
    assert!(ds.len() >= 2, "{:?}", ds.iter().map(|d| &d.message).collect::<Vec<_>>());
    // …pero un parse sucio NO llega al checker (sería cascada sobre un AST parcial).
    assert!(ds.iter().all(|d| d.message.contains("syntax")), "{:?}",
        ds.iter().map(|d| &d.message).collect::<Vec<_>>());
    // Sin errores → lista vacía (borra los diagnósticos previos del editor).
    assert!(analyze_all("fn main() -> int { 0 }").is_empty());
}

    #[test]
fn analyzes_valid_program_without_errors() {
    assert!(analyze("fn main() -> int { 1 + 2 }").is_none());
}

#[test]
fn diagnostics_con_modules() {
    // Un proyecto de dos archivos: `geo.ray` (en disco) y la entrada `main.ray` (en el buffer).
    let dir = std::env::temp_dir().join("ray_lsp_mod");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("geo.ray"), "pub fn duplicate(x: int) -> int { x * 2 }\n").unwrap();
    let entry = dir.join("main.ray");
    let uri = format!("file://{}", entry.display());

    // (a) Un import válido NO produce diagnósticos (antes: "función 'duplicar' no declarada").
    let src = "from geo import duplicate;\nfn main() -> int { duplicate(21) }\n";
    let ds = analyze_modular(&uri, src).expect("mode modular (es un file)");
    assert!(ds.is_empty(), "un import válido no must dar errors: {:?}",
        ds.iter().map(|d| &d.message).collect::<Vec<_>>());

    // (b) Un error de tipos EN LA ENTRADA sí se reporta, con la línea local.
    let src = "from geo import duplicate;\nfn main() -> int { duplicate(true) }\n";
    let ds = analyze_modular(&uri, src).expect("modular");
    assert_eq!(ds.len(), 1, "{:?}", ds.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert_eq!(ds[0].line, 2, "línea local de la entry, no la global del program fusionado");

    // (c) Un import a un módulo inexistente se reporta (la entrada parsea → error del loader).
    let src = "from noexiste import cosa;\nfn main() -> int { 0 }\n";
    let ds = analyze_modular(&uri, src).expect("modular");
    assert_eq!(ds.len(), 1);
    assert!(ds[0].message.contains("noexiste"), "{}", ds[0].message);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diagnostics_test_file_resolves_project_modules() {
    // M113b: un `tests/*.ray` de un proyecto con ray.toml importa los módulos de `src/` — el
    // editor debe verlo IGUAL que `ray test` (que añade la raíz de la entrada como raíz extra).
    // Antes: "module 'fileutil' not found" sobre un test que corría en verde.
    let dir = std::env::temp_dir().join("ray_lsp_tests_dir");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(dir.join("ray.toml"), "[package]\nname = \"proj\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(dir.join("src/main.ray"), "fn main() -> int { 0 }\n").unwrap();
    std::fs::write(dir.join("src/fileutil.ray"), "pub fn double(x: int) -> int { x * 2 }\n").unwrap();
    let entry = dir.join("tests/t.ray");
    let uri = format!("file://{}", entry.display());

    // (a) El import del módulo del proyecto resuelve → sin diagnósticos.
    let src = "import fileutil;\n@test\nfn doubles() { assert_eq(fileutil.double(2), 4); }\n";
    let ds = analyze_modular(&uri, src).expect("modular");
    assert!(ds.is_empty(), "el test importa src/ sin errores: {:?}",
        ds.iter().map(|d| &d.message).collect::<Vec<_>>());

    // (b) Los errores reales se siguen reportando (no se volvió lenidad).
    let src = "import fileutil;\n@test\nfn bad() { assert_eq(fileutil.double(true), 4); }\n";
    let ds = analyze_modular(&uri, src).expect("modular");
    assert_eq!(ds.len(), 1, "{:?}", ds.iter().map(|d| &d.message).collect::<Vec<_>>());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diagnostics_submodule_without_main_and_import_by_path() {
    // Reproduce dos problemas al abrir un ARCHIVO DE MÓDULO (no la entrada) en el editor:
    //   1. Un submódulo `pub` sin `main` marcaba "falta la función de entrada 'main'".
    //   2. Un `import geo/util;` (ruta absoluta desde la raíz) no resolvía, porque el loader
    //      tomaba como raíz la carpeta del propio submódulo, no la raíz del proyecto.
    // Estructura (igual a examples/proyecto):
    //   root/main.ray                 (entrada, con main)
    //   root/geo/util.ray             (pub fn, sin main)
    //   root/geo/formas/circulo.ray   (import geo/util; pub struct; pub fn area, sin main)
    let dir = std::env::temp_dir().join("ray_lsp_submod");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("geo/formas")).unwrap();
    std::fs::write(dir.join("main.ray"),
        "import geo/formas/circle;\nfn main() -> int { circle.area(circle.Circle { radio: 4 }) }\n").unwrap();
    let util_src = "pub fn square(n: int) -> int { n * n }\n";
    std::fs::write(dir.join("geo/util.ray"), util_src).unwrap();
    let circle_src = "import geo/util;\npub struct Circle { radio: int }\npub fn area(c: Circle) -> int { 3 * util.square(c.radio) }\n";
    std::fs::write(dir.join("geo/formas/circle.ray"), circle_src).unwrap();

    // La raíz del proyecto se detecta como el ancestro con `main.ray`, desde un submódulo profundo.
    let root = project_root_for(&dir.join("geo/formas/circle.ray")).expect("raíz");
    assert_eq!(root, dir, "la raíz es el directory con main.ray, no la del submódulo");

    // (1) util.ray: submódulo sin `main` → SIN diagnósticos (antes: "falta … 'main'").
    let uri_util = format!("file://{}", dir.join("geo/util.ray").display());
    let ds = analyze_modular(&uri_util, util_src).expect("modular");
    assert!(ds.is_empty(), "un submódulo sin main no must dar errors: {:?}",
        ds.iter().map(|d| &d.message).collect::<Vec<_>>());

    // (2) circulo.ray: `import geo/util;` resuelve desde la raíz + sin `main` → SIN diagnósticos.
    let uri_circ = format!("file://{}", dir.join("geo/formas/circle.ray").display());
    let ds = analyze_modular(&uri_circ, circle_src).expect("modular");
    assert!(ds.is_empty(), "el import por path absoluta must resolver: {:?}",
        ds.iter().map(|d| &d.message).collect::<Vec<_>>());

    // (3) Un error de tipos REAL en el submódulo sí se reporta (no se traga por el modo módulo).
    let circle_malo = "import geo/util;\npub struct Circle { radio: int }\npub fn area(c: Circle) -> int { 3 * util.square(c) }\n";
    let ds = analyze_modular(&uri_circ, circle_malo).expect("modular");
    assert_eq!(ds.len(), 1, "el error real del body must verse: {:?}",
        ds.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert_eq!(ds[0].line, 3, "línea local del submódulo");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diagnostics_internal_submodule_of_capsule() {
    // Un submódulo DENTRO de una cápsula que importa a un vecino interno de la MISMA cápsula.
    // `geo/mod.ray` hace de `geo` una cápsula; `geo/util` es interno; `geo/formas/circulo.ray`
    // (también dentro de `geo/`) hace `import geo/util;` → legítimo (el importador vive bajo la
    // cápsula). Antes, al abrir el submódulo, el loader lo identificaba por su stem ("circulo")
    // y el enforcement lo trataba como externo: "el módulo 'geo/util' es interno a la cápsula 'geo'".
    let dir = std::env::temp_dir().join("ray_lsp_capsule");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("geo/formas")).unwrap();
    std::fs::write(dir.join("main.ray"),
        "import geo;\nfn main() -> int { geo.area(geo.Circle { radio: 4 }) }\n").unwrap();
    std::fs::write(dir.join("geo/mod.ray"),
        "pub from geo/formas/circle import Circle, area;\n").unwrap();
    std::fs::write(dir.join("geo/util.ray"), "pub fn square(n: int) -> int { n * n }\n").unwrap();
    let circle_src = "import geo/util;\npub struct Circle { radio: int }\npub fn area(c: Circle) -> int { 3 * util.square(c.radio) }\n";
    std::fs::write(dir.join("geo/formas/circle.ray"), circle_src).unwrap();

    // La identidad real del submódulo es "geo/formas/circulo" → está bajo la cápsula "geo".
    let root = project_root_for(&dir.join("geo/formas/circle.ray")).expect("raíz");
    assert_eq!(root, dir);

    // Abrir el submódulo interno: SIN diagnósticos (import a un vecino de la cápsula, y sin main).
    let uri = format!("file://{}", dir.join("geo/formas/circle.ray").display());
    let ds = analyze_modular(&uri, circle_src).expect("modular");
    assert!(ds.is_empty(), "importar a un vecino internal de la propia cápsula es legítimo: {:?}",
        ds.iter().map(|d| &d.message).collect::<Vec<_>>());

    // Hover DENTRO del submódulo (sin `main`): antes no daba nada porque el chequeo de main
    // cortaba antes de recorrer los cuerpos. `circulo_src` línea 3 (0-based 2):
    //   `pub fn area(c: Circle) -> int { 3 * util.square(c.radio) }`  — uso de `c` en col 48.
    let (t, _, _) = hover_at(Some(&uri), circle_src, 2, 48).expect("hover about use de 'c'");
    assert_eq!(t, "c: Circle", "hover de un use inside de un submódulo sin main");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn formatting_replaces_the_document() {
    let mut docs = HashMap::new();
    let uri = "file:///t.ray".to_string();
    // Código mal formateado (espaciado irregular) → un edit que cubre todo el buffer.
    docs.insert(uri.clone(), "fn  main( )->int{1+2}\n".to_string());
    let msg = obj(vec![("params", obj(vec![("textDocument", obj(vec![("uri", text(&uri))]))]))]);
    let r = formatting_result(&msg, &docs);
    let edits = r.as_array().expect("array de edits");
    assert_eq!(edits.len(), 1, "un único edit de documento complete");
    let new = edits[0].get("newText").and_then(Json::as_str).unwrap();
    // El resultado es el mismo que `ray fmt` (el formateador compartido).
    assert_eq!(new, crate::fmt::format_source("fn  main( )->int{1+2}\n").unwrap());
    assert!(new.contains("fn main()"), "se normalizó el espaciado: {new:?}");
    // El rango arranca en (0,0).
    let start = edits[0].get("range").unwrap().get("start").unwrap();
    assert_eq!(start.get("line"), Some(&Json::Num(0.0)));
    assert_eq!(start.get("character"), Some(&Json::Num(0.0)));

    // Ya formateado → lista vacía (nada que cambiar).
    let already = crate::fmt::format_source("fn  main( )->int{1+2}\n").unwrap();
    docs.insert(uri.clone(), already);
    assert!(formatting_result(&msg, &docs).as_array().unwrap().is_empty());

    // Código que no parsea → lista vacía (no se formatea código inválido).
    docs.insert(uri.clone(), "fn main( { ".to_string());
    assert!(formatting_result(&msg, &docs).as_array().unwrap().is_empty());
}

#[test]
fn document_symbol_list_el_outline() {
    let mut docs = HashMap::new();
    let uri = "file:///t.ray".to_string();
    let src = "const K: int = 3;\nstruct Point { x: int, y: int }\nenum Color { Rojo, Verde }\ntrait Show { fn show(self) -> string; }\nimpl Show for Point { fn show(self) -> string { \"p\" } }\nfn main() -> int { 0 }\n";
    docs.insert(uri.clone(), src.to_string());
    let msg = obj(vec![("params", obj(vec![("textDocument", obj(vec![("uri", text(&uri))]))]))]);
    let syms = document_symbol_result(&msg, &docs);
    let arr = syms.as_array().expect("array");
    let names: Vec<&str> = arr.iter().filter_map(|s| s.get("name").and_then(Json::as_str)).collect();
    // Todos los ítems de nivel superior, en orden de archivo.
    assert_eq!(names, vec!["K", "Point", "Color", "Show", "impl Show for Point", "main"], "{names:?}");
    // El enum lleva sus variantes como hijos.
    let color = arr.iter().find(|s| s.get("name").and_then(Json::as_str) == Some("Color")).unwrap();
    let vars: Vec<&str> = color.get("children").unwrap().as_array().unwrap().iter()
        .filter_map(|c| c.get("name").and_then(Json::as_str)).collect();
    assert_eq!(vars, vec!["Rojo", "Verde"]);
    // El trait lleva su método como hijo; el kind del struct es 23 (Struct).
    let show = arr.iter().find(|s| s.get("name").and_then(Json::as_str) == Some("Show")).unwrap();
    assert_eq!(show.get("children").unwrap().as_array().unwrap().len(), 1);
    let point = arr.iter().find(|s| s.get("name").and_then(Json::as_str) == Some("Point")).unwrap();
    assert_eq!(point.get("kind"), Some(&Json::Num(23.0)));
    // El selectionRange del struct apunta a su NOMBRE (col 7 0-based en "struct Point").
    let sel = point.get("selectionRange").unwrap().get("start").unwrap();
    assert_eq!(sel.get("line"), Some(&Json::Num(1.0)));
    assert_eq!(sel.get("character"), Some(&Json::Num(7.0)));
}

#[test]
fn hover_and_signature_of_builtins() {
    // M10.2i: los builtins (print/char_code/…) no viven en la fuente; aun así el hover muestra su
    // firma (con los tipos de la llamada) y el signature help su firma fija. (M49: sqrt/pow/abs/… se
    // movieron a `std/math`, ya no son builtins → se prueba con `char_code`, que sí lo sigue siendo.)
    let src = "fn main() -> int {\n  let x = char_code('a');\n  print(x);\n  0\n}\n";
    // `char_code('a')` → int.
    let cc = src.lines().nth(1).unwrap().find("char_code").unwrap();
    let (ta, _, _) = hover_at(None, src, 1, cc).expect("hover de char_code");
    assert_eq!(ta, "char_code: fn(char) -> int");
    // `print(x)` → unit.
    let cp = src.lines().nth(2).unwrap().find("print").unwrap();
    let (tp, _, _) = hover_at(None, src, 2, cp).expect("hover de print");
    assert_eq!(tp, "print: fn(int) -> unit");
    // Signature help por firma fija de la tabla (`signature`): sigue sirviendo a `math.pow(` (el
    // envoltorio de `std/math`) aunque `pow` ya no sea builtin.
    let (ps, ret) = crate::builtins::signature("pow").expect("signature de pow");
    assert_eq!((ps, ret), (vec!["base: float", "exp: float"], "float"));
    // M46a: los builtins-método también llevan firma (para el detalle del popup).
    assert_eq!(crate::builtins::signature("len"), Some((vec!["c"], "int")));
    assert!(crate::builtins::signature("print").is_none(), "print es variádico ad-hoc, sin signature fixes");
}

#[test]
fn hover_shows_documentacion() {
    // Una función con `///` encima: el hover trae la firma + la doc, como Markdown.
    let src = "/// Duplica un número.\n/// Segunda línea.\nfn duplicate(x: int) -> int { x * 2 }\nfn main() -> int {\n  duplicate(21)\n}\n";
    // Uso de `duplicar` en la línea 5 (0-based 4), col 2.
    let (info, _, _) = hover_at(None, src, 4, 2).expect("hover");
    assert!(info.starts_with("duplicate: fn(int) -> int"), "{info}");
    // La doc se localiza escaneando los `///` encima de la declaración.
    let mut docs = HashMap::new();
    let uri = format!("file://{}", std::env::temp_dir().join("ray_hover_doc.ray").display());
    docs.insert(uri.clone(), src.to_string());
    let d = doc_of_symbol(&uri, src, 4, 2, &docs).expect("documentación");
    assert_eq!(d, "Duplica un número.\nSegunda línea.");
    // El result de hover la mete en un bloque Markdown con la firma.
    let msg = obj(vec![("params", obj(vec![
        ("textDocument", obj(vec![("uri", text(&uri))])),
        ("position", obj(vec![("line", num(4)), ("character", num(2))])),
    ]))]);
    let r = hover_result(&msg, &docs);
    let val = r.get("contents").unwrap().get("value").and_then(Json::as_str).unwrap();
    assert!(val.contains("```raylang"), "signature en block de código: {val}");
    assert!(val.contains("Duplica un número."), "incluye la doc: {val}");
    assert_eq!(r.get("contents").unwrap().get("kind"), Some(&Json::Str("markdown".into())));

    // Un MÉTODO documentado con `///` también muestra su doc (M10.2h: los métodos se indexan).
    let src_m = "trait Show { fn show(self) -> string; }\nstruct P { v: int }\nimpl Show for P {\n  /// Muestra el valor.\n  fn show(self) -> string { \"p\" }\n}\nfn main() -> int {\n  let p = P { v: 1 };\n  print(p.show());\n  0\n}\n";
    docs.insert(uri.clone(), src_m.to_string());
    // `p.show()` en la línea 9 (0-based 8); `mostrar` tras el punto.
    let col_m = src_m.lines().nth(8).unwrap().find("show").unwrap();
    let d = doc_of_symbol(&uri, src_m, 8, col_m, &docs).expect("doc del método");
    assert_eq!(d, "Muestra el valor.", "hover-doc de método");

    // Un símbolo SIN doc → hover en texto plano, sin bloque Markdown.
    let src2 = "fn triple(x: int) -> int { x * 3 }\nfn main() -> int { triple(1) }\n";
    docs.insert(uri.clone(), src2.to_string());
    let msg2 = obj(vec![("params", obj(vec![
        ("textDocument", obj(vec![("uri", text(&uri))])),
        ("position", obj(vec![("line", num(1)), ("character", num(19))])),
    ]))]);
    let r2 = hover_result(&msg2, &docs);
    assert_eq!(r2.get("contents").unwrap().get("kind"), Some(&Json::Str("plaintext".into())));
}

#[test]
fn hover_doc_of_builtins_and_prelude() {
    let mut docs = HashMap::new();
    let uri = format!("file://{}", std::env::temp_dir().join("ray_doc_builtin.ray").display());
    // Un builtin (`pow`) no tiene declaración en el archivo: la doc sale de la tabla
    // (`builtins::doc`, en inglés).
    let src = "fn main() -> int {\n  print(pow(2.0, 10.0));\n  0\n}\n";
    docs.insert(uri.clone(), src.to_string());
    let col = src.lines().nth(1).unwrap().find("pow").unwrap();
    let d = doc_of_symbol(&uri, src, 1, col, &docs).expect("doc de pow");
    assert!(d.contains("Raises `base` to the power"), "{d}");
    // Un símbolo del PRELUDE (`sort`): su declaración vive en la fuente inyectada, no en el
    // buffer → la doc se busca por nombre en `prelude::SOURCE`.
    let src2 = "fn main() -> int {\n  let xs = sort([3, 1, 2]);\n  xs[0]\n}\n";
    docs.insert(uri.clone(), src2.to_string());
    let col2 = src2.lines().nth(1).unwrap().find("sort").unwrap();
    let d2 = doc_of_symbol(&uri, src2, 1, col2, &docs).expect("doc de sort");
    assert!(!d2.is_empty(), "doc del prelude para sort: {d2}");
    // Una variable local que TAPA un nombre de builtin no hereda su doc: `min` local.
    let src3 = "fn main() -> int {\n  let min = 5;\n  min\n}\n";
    docs.insert(uri.clone(), src3.to_string());
    let col3 = src3.lines().nth(2).unwrap().find("min").unwrap();
    assert_eq!(doc_of_symbol(&uri, src3, 2, col3, &docs), None, "local sin doc, aunque exista el builtin min");
    // El completion adjunta la doc del builtin como Markdown.
    let msg = obj(vec![("params", obj(vec![
        ("textDocument", obj(vec![("uri", text(&uri))])),
        ("position", obj(vec![("line", num(1)), ("character", num(2))])),
    ]))]);
    docs.insert(uri.clone(), src.to_string());
    let r = completion_result(&msg, &docs);
    let Json::Arr(items) = &r else { panic!("completion no es list") };
    // M49: `pow`/`sqrt`/`abs`/… se movieron a `std/math` (ya no en el completion global de builtins);
    // se prueba con `char_code`, que sigue siendo builtin.
    let cc = items.iter().find(|i| i.get("label") == Some(&Json::Str("char_code".into()))).expect("char_code en completion");
    let doc_val = cc.get("documentation").and_then(|d| d.get("value")).and_then(Json::as_str).expect("documentation de char_code");
    assert!(doc_val.contains("Unicode"), "{doc_val}");
}

#[test]
fn document_highlight_highlights_occurrences() {
    let mut docs = HashMap::new();
    let uri = "file:///t.ray".to_string();
    // `x` se declara y se usa dos veces.
    let src = "fn main() -> int {\n  let x = 3;\n  x + x\n}\n";
    docs.insert(uri.clone(), src.to_string());
    // Cursor sobre el primer uso de `x` (línea 3 0-based 2, col 2).
    let msg = obj(vec![
        ("params", obj(vec![
            ("textDocument", obj(vec![("uri", text(&uri))])),
            ("position", obj(vec![("line", num(2)), ("character", num(2))])),
        ])),
    ]);
    let hl = document_highlight_result(&msg, &docs);
    let arr = hl.as_array().expect("array");
    // Declaración (let x) + dos usos = 3 rangos, sin duplicados.
    assert_eq!(arr.len(), 3, "decl + 2 usos: {arr:?}");
    // Exactamente uno es Write (kind 3, la declaración); el resto Text (kind 1).
    let writes = arr.iter().filter(|h| h.get("kind") == Some(&Json::Num(3.0))).count();
    assert_eq!(writes, 1, "la declaración es el único Write");
}

#[test]
fn hover_con_modules() {
    // Antes: un archivo con `import` no daba NINGÚN hover (el checker fallaba por el import).
    let dir = std::env::temp_dir().join("ray_lsp_hover_mod");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("geo.ray"), "pub fn duplicate(x: int) -> int { x * 2 }\n").unwrap();
    let uri = format!("file://{}", dir.join("main.ray").display());
    let src = "from geo import duplicate;\nfn main() -> int {\n  let y = 5;\n  duplicate(y)\n}\n";

    // Variable LOCAL: hover funciona pese al import (índice sobre el programa fusionado).
    let (t, _, _) = hover_at(Some(&uri), src, 3, 12).expect("hover about 'y'");
    assert_eq!(t, "y: int");
    // Función IMPORTADA de otro módulo: muestra su tipo en forma de fachada (`geo.duplicar`,
    // no el `geo::duplicar` interno).
    let (t, _, _) = hover_at(Some(&uri), src, 3, 2).expect("hover about 'duplicate'");
    assert_eq!(t, "geo.duplicate: fn(int) -> int", "forma de fachada, sin '::'");

    // Rename de una variable local en un archivo multi-módulo: seguro (vive entera aquí).
    let (_, _, _, is_local) = symbol_occurrences(Some(&uri), src, 3, 12).expect("símbolo");
    assert!(is_local, "'y' es local → renombrable");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hover_of_fields_and_methods() {
    // Campo de struct: `p.x` → el tipo del campo, en la posición del nombre tras el `.`.
    let src = "struct Point { x: int, y: int }\nfn main() -> int {\n  let p = Point { x: 3, y: 4 };\n  p.x + p.y\n}\n";
    let (t, _, _) = hover_at(None, src, 3, 4).expect("hover del campo x");
    assert_eq!(t, "x: int");

    // Método de trait: `n.doblar()` → la firma del método (incluye el receptor).
    let src = "trait D { fn fold(self) -> int; }\nstruct N { v: int }\nimpl D for N { fn fold(self) -> int { self.v * 2 } }\nfn main() -> int {\n  let n = N { v: 21 };\n  n.fold()\n}\n";
    let (t, _, _) = hover_at(None, src, 5, 4).expect("hover del método fold");
    assert_eq!(t, "fold: fn(N) -> int");

    // Método por UFCS a una función del prelude: `xs.map(f)`.
    let src = "fn main() -> int {\n  let xs = [1, 2, 3];\n  xs.map(fn(a: int) -> int { a + 1 });\n  0\n}\n";
    let (t, _, _) = hover_at(None, src, 2, 5).expect("hover del método map");
    assert!(t.starts_with("map: fn("), "{t}");
}

#[test]
fn hover_inside_interpolation() {
    // Las expresiones de `${…}` se re-lexan con posiciones reales → hover funciona dentro.
    let src = "fn main() -> int {\n  let x = 7;\n  print(\"n=${x} y ${x + 1}\");\n  0\n}\n";
    // línea 3 (0-based 2): `  print("n=${x} y ${x + 1}");`
    //   `x` de `${x}` en col 13; `x` de `${x + 1}` en col 20.
    let (t, _, _) = hover_at(None, src, 2, 13).expect("hover 'x' en ${x}");
    assert_eq!(t, "x: int");
    let (t, _, _) = hover_at(None, src, 2, 20).expect("hover 'x' en ${x + 1}");
    assert_eq!(t, "x: int");
}

#[test]
fn facade_name_collapses_namespaces() {
    // Sin imports conocidos → fallback `primer.último` (respeta la cápsula, sin `::`).
    assert_eq!(facade_name("geo::formas::circle::Circulo", &[]), "geo.Circulo");
    assert_eq!(facade_name("geo::area: fn(geo::formas::circle::Circulo) -> int", &[]),
        "geo.area: fn(geo.Circulo) -> int");
    // Nombres sin namespacing (locales, primitivos) intactos.
    assert_eq!(facade_name("c: Punto", &[]), "c: Punto");
    assert_eq!(facade_name("n: int", &[]), "n: int");
    assert_eq!(facade_name("f: fn(int, bool) -> string", &[]), "f: fn(int, bool) -> string");
    // M49: con el import `std/math` (leaf `math`, ns_prefix `std::math`), la fachada usa el LEAF
    // con el que el usuario accede: `math.sqrt`, no `std.sqrt`. Una cápsula `geo` (ns_prefix `geo`)
    // sigue mostrando su raíz (`geo.Circulo`) porque su ns_prefix también es prefijo.
    let imp = vec![("math".to_string(), "std::math".to_string()), ("geo".to_string(), "geo".to_string())];
    assert_eq!(facade_name("std::math::sqrt: fn(float) -> float", &imp), "math.sqrt: fn(float) -> float");
    assert_eq!(facade_name("std::math::PI: float", &imp), "math.PI: float");
    assert_eq!(facade_name("geo::formas::circle::Circulo", &imp), "geo.Circulo");
}

#[test]
fn definition_cross_modules() {
    let dir = std::env::temp_dir().join("ray_lsp_def_mod");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("geo.ray"), "pub fn duplicate(x: int) -> int { x * 2 }\n").unwrap();
    let uri = format!("file://{}", dir.join("main.ray").display());
    let src = "from geo import duplicate;\nfn main() -> int {\n  let r = duplicate(21);\n  r\n}\n";

    // Ir-a-definición de la función IMPORTADA → salta al otro archivo (geo.ray, línea 0).
    let (turi, line, _, _) = definition_at(&uri, src, 2, 10).expect("def de duplicate");
    assert!(turi.ends_with("geo.ray"), "el target es el file del módulo: {turi}");
    assert_eq!(line, 0);

    // Ir-a-definición de una variable LOCAL → se queda en este archivo.
    let (turi, line, _, _) = definition_at(&uri, src, 3, 2).expect("def de r");
    assert!(turi.ends_with("main.ray"), "{turi}");
    assert_eq!(line, 2, "la declaración de 'r' está en la línea 3 (0-based 2)");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn references_cross_modules() {
    let dir = std::env::temp_dir().join("ray_lsp_refs_mod");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // geo.ray: define `duplicar` y lo usa una vez internamente.
    std::fs::write(dir.join("geo.ray"),
        "pub fn duplicate(x: int) -> int { x * 2 }\npub fn cuad(x: int) -> int { duplicate(x) }\n").unwrap();
    let uri = format!("file://{}", dir.join("main.ray").display());
    let src = "from geo import duplicate;\nfn main() -> int {\n  duplicate(21)\n}\n";

    // Find-references desde el uso en main.ray → apariciones en AMBOS archivos.
    let locs = references_cross(&uri, src, 2, 2, true).expect("references");
    let files: std::collections::HashSet<&str> = locs.iter()
        .map(|(u, _)| u.rsplit('/').next().unwrap())
        .collect();
    assert!(files.contains("geo.ray"), "incluye la declaración y el use internal: {locs:?}");
    assert!(files.contains("main.ray"), "incluye el use del file abierto: {locs:?}");
    // 3 apariciones: declaración + uso interno en geo.ray, uso en main.ray.
    assert_eq!(locs.len(), 3, "{locs:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rename_cross_modules() {
    let dir = std::env::temp_dir().join("ray_lsp_ren_mod");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("geo.ray"),
        "pub fn duplicate(x: int) -> int { x * 2 }\npub fn cuad(x: int) -> int { duplicate(x) }\n").unwrap();
    let uri = format!("file://{}", dir.join("main.ray").display());

    // Rename de una función importada → toca AMBOS archivos, INCLUIDA la línea del import.
    let src = "from geo import duplicate;\nfn main() -> int {\n  duplicate(21)\n}\n";
    let pos = rename_cross(&uri, src, 2, 2).expect("rename cross-módulo");
    let files: std::collections::HashSet<&str> =
        pos.iter().map(|(u, _)| u.rsplit('/').next().unwrap()).collect();
    assert!(files.contains("geo.ray") && files.contains("main.ray"), "{pos:?}");
    // 4 posiciones: decl + uso interno (geo.ray), especificador de import + uso (main.ray).
    assert_eq!(pos.len(), 4, "{pos:?}");
    // El especificador del import (main.ray línea 0) está entre las posiciones.
    assert!(pos.iter().any(|(u, (l, _, _))| u.ends_with("main.ray") && *l == 0), "falta el import: {pos:?}");

    // Con `as alias`, el rename se NIEGA (los usos van por el alias → incompleto e inseguro).
    let src = "from geo import duplicate as d;\nfn main() -> int { d(21) }\n";
    assert!(rename_cross(&uri, src, 1, 19).is_none(), "rename con alias must rechazarse");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn uri_to_path_decodes() {
    assert_eq!(uri_to_path("file:///a/b/c.ray"), Some(PathBuf::from("/a/b/c.ray")));
    assert_eq!(uri_to_path("file:///a/mi%20carpeta/x.ray"), Some(PathBuf::from("/a/mi carpeta/x.ray")));
    assert_eq!(uri_to_path("untitled:Untitled-1"), None); // buffer sin archivo → single-file
}

#[test]
fn references_of_local_variable() {
    // `let x = 1; x + x` → declaración + 2 usos.
    let src = "fn main() -> int {\n  let x = 1;\n  x + x\n}\n";
    // Cursor sobre el primer uso de `x` (línea 3 → 0-based 2, col 2).
    let (name, decl, uses, is_local) = symbol_occurrences(None, src, 2, 2).expect("hay símbolo");
    assert_eq!(name, "x");
    assert_eq!(decl, Some((1, 6, 1)), "la declaración apunta al NOMBRE x, no al 'let'");
    assert_eq!(uses.len(), 2, "x + x son dos usos");
    assert!(is_local, "one variable local vive entera en este file");
    // Y desde el nombre de la declaración (línea 2 → 0-based 1, col 6) da lo mismo.
    let (n2, d2, u2, _) = symbol_occurrences(None, src, 1, 6).expect("símbolo from la declaración");
    assert_eq!((n2, d2, u2.len()), ("x".to_string(), Some((1, 6, 1)), 2));
}

#[test]
fn references_distinguish_scopes() {
    // Dos `x` en funciones distintas no se mezclan (claves de declaración distintas).
    let src = "fn f(a: int) -> int {\n  let x = a;\n  x + x\n}\nfn main() -> int {\n  let x = 9;\n  x\n}\n";
    // El `x` de `f` (línea 3 → 0-based 2): 2 usos.
    let (_, _, uf, _) = symbol_occurrences(None, src, 2, 2).unwrap();
    assert_eq!(uf.len(), 2);
    // El `x` de `main` (línea 7 → 0-based 6): 1 uso.
    let (_, _, um, _) = symbol_occurrences(None, src, 6, 2).unwrap();
    assert_eq!(um.len(), 1);
}

#[test]
fn references_of_function() {
    // Una función llamada dos veces: declaración + 2 usos.
    let src = "fn double(n: int) -> int { n + n }\nfn main() -> int {\n  double(1) + double(2)\n}\n";
    // Cursor sobre la primera llamada `double` (línea 3 → 0-based 2, col 2).
    let (name, decl, uses, _) = symbol_occurrences(None, src, 2, 2).expect("hay símbolo");
    assert_eq!(name, "double");
    assert_eq!(decl, Some((0, 3, 6)), "la declaración apunta al name 'double' tras 'fn '");
    assert_eq!(uses.len(), 2);
}

#[test]
fn rename_produces_workspace_edit() {
    let src = "fn main() -> int {\n  let x = 1;\n  x + x\n}\n";
    let msg = json::parse(
        r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":2,"character":2},"newName":"y"}}"#
    ).unwrap();
    let mut docs = HashMap::new();
    docs.insert("file:///t.ray".to_string(), src.to_string());
    let res = rename_result(&msg, &docs);
    let edits = res.get("changes").unwrap().get("file:///t.ray").unwrap().as_array().unwrap();
    assert_eq!(edits.len(), 3, "declaración + 2 usos");
    assert_eq!(edits[0].get("newText"), Some(&Json::Str("y".to_string())));
}

#[test]
fn completion_offers_symbols_builtins_y_keywords() {
    let src = "struct Point { x: int }\nfn double(n: int) -> int { n + n }\nfn main() -> int { 0 }\n";
    let msg = json::parse(
        r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":2,"character":0}}}"#
    ).unwrap();
    let mut docs = HashMap::new();
    docs.insert("file:///t.ray".to_string(), src.to_string());
    let res = completion_result(&msg, &docs);
    let items = res.as_array().unwrap();
    let labels: Vec<&str> = items.iter().filter_map(|i| i.get("label").and_then(|l| l.as_str())).collect();
    assert!(labels.contains(&"double"), "función propia\n{labels:?}");
    assert!(labels.contains(&"Point"), "type propio");
    assert!(labels.contains(&"print"), "builtin");
    assert!(labels.contains(&"map"), "función del prelude");
    assert!(labels.contains(&"while"), "palabra clave");
    // No expone nombres sintéticos (manglados, internos).
    assert!(!labels.iter().any(|l| l.contains('#') || l.starts_with("__")), "sin names sintéticos");
}

#[test]
fn completion_offers_closure_snippet_for_spawn_and_scope() {
    // `spawn`/`scope` toman una función anónima → un ítem-extra inserta `name(fn() { … });`.
    let src = "fn main() -> int { 0 }\n";
    let msg = json::parse(
        r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":0,"character":19}}}"#
    ).unwrap();
    let mut docs = HashMap::new();
    docs.insert("file:///t.ray".to_string(), src.to_string());
    let res = completion_result(&msg, &docs);
    let items = res.as_array().unwrap();
    for name in ["spawn", "scope"] {
        let snippet = items.iter().find(|i|
            i.get("label").and_then(|l| l.as_str()) == Some(&format!("{name}(fn() {{…}})")));
        let snippet = snippet.unwrap_or_else(|| panic!("falta el ítem de closure para {name}"));
        assert_eq!(snippet.get("insertTextFormat"), Some(&Json::Num(2.0)), "{name}: es snippet");
        assert_eq!(
            snippet.get("insertText").and_then(|t| t.as_str()),
            Some(format!("{name}(fn() {{\n\t$0\n}});").as_str()),
            "{name}: inserta el body de la función anónima"
        );
        // El builtin pelado sigue ofreciéndose aparte.
        assert!(items.iter().any(|i| i.get("label").and_then(|l| l.as_str()) == Some(name)),
            "{name}: builtin pelado también presente");
    }
}

#[test]
fn completion_offers_language_construct_snippets() {
    // Snippets de construcciones del lenguaje: teclear la keyword ofrece el bloque completo
    // con placeholders (además de la keyword pelada, que sigue para las demás posiciones).
    let src = "fn main() -> int { 0 }\n";
    let msg = json::parse(
        r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":0,"character":19}}}"#
    ).unwrap();
    let mut docs = HashMap::new();
    docs.insert("file:///t.ray".to_string(), src.to_string());
    let res = completion_result(&msg, &docs);
    let items = res.as_array().unwrap();
    // (label, filterText, fragmento que el insertText debe contener). Placeholders en INGLÉS
    // (convención: todo lo que el lenguaje entrega al usuario va en inglés; la política de
    // identificadores del código la vigila tests/naming_policy.rs, no este test).
    let expected: &[(&str, &str, &str)] = &[
        ("fn …() { }", "fn", "fn ${1:name}("),
        ("fn main() { }", "main", "fn main() -> int {"),
        ("let … = …;", "let", "let ${1:name} = ${2:expr};"),
        ("var … = …;", "var", "var ${1:name} = ${2:expr};"),
        ("if (…) { }", "if", "if (${1:condition}) {"),
        ("if (…) { } else { }", "if", "} else {"),
        ("while (…) { }", "while", "while (${1:condition}) {"),
        ("for … in … { }", "for", "for ${1:elem} in ${2:collection} {"),
        ("for … in a..b { }", "for", "..${3:n}"),
        ("match (…) { … => … }", "match", "match (${1:expr}) {"),
        // Etapa 2 — datos y tipos.
        ("if let … = … { }", "if", "if let ${1:pattern} = ${2:expr} {"),
        ("struct … { }", "struct", "${2:field}: ${3:type},"),
        ("enum … { }", "enum", "${3:Variant}(${4:type}),"),
        ("trait … { }", "trait", "fn ${2:method}(self)"),
        ("impl … for … { }", "impl", "impl ${1:Trait} for ${2:Type} {"),
        ("const … = …;", "const", "const ${1:NAME}: ${2:type} = ${3:value};"),
        ("fn(…) { } (anonymous)", "fn", "fn(${1:params}) {"),
        ("@test fn … { }", "test", "@test\nfn ${1:name}() {"),
        ("@derive(…) struct … { }", "derive", "@derive(${1:Eq, Show})"),
        // Etapa 3 — los no-obvios: variantes calificadas, channel anotado, ?, import, extern.
        ("match Option { Some/None }", "match", "Option.Some(${2:v}) => $3,\n\tOption.None => $0,"),
        ("match Result { Ok/Err }", "match", "Result.Ok(${2:v}) => $3,\n\tResult.Err(${4:e}) => $0,"),
        ("fn … -> Result … ? …", "fn", "-> Result<${3:int}, string> {"),
        ("import …;", "import", "import ${1:module};"),
        ("from … import …;", "from", "from ${1:module} import ${2:name};"),
        ("channel + spawn + send + recv", "channel", "let ${1:ch}: Channel<${2:int}> = Channel.new();"),
        ("extern \"lib\" { fn …; }", "extern", "extern \"${1:lib}\" {"),
    ];
    for (label, filter, frag) in expected {
        let it = items.iter().find(|i| i.get("label").and_then(|l| l.as_str()) == Some(label))
            .unwrap_or_else(|| panic!("falta el snippet {label}"));
        assert_eq!(it.get("insertTextFormat"), Some(&Json::Num(2.0)), "{label}: es snippet");
        assert_eq!(it.get("filterText").and_then(|f| f.as_str()), Some(*filter), "{label}: filterText");
        let insert = it.get("insertText").and_then(|t| t.as_str()).unwrap();
        assert!(insert.contains(frag), "{label}: insertText contiene {frag:?}: {insert}");
    }
    // La gramática no-obvia: la CONDICIÓN va entre paréntesis en if/while/match… pero el
    // escrutinio del `if let` va sin paréntesis (hasta el `{`, SPEC §6.2).
    for label in ["if (…) { }", "if (…) { } else { }", "while (…) { }", "match (…) { … => … }"] {
        let it = items.iter().find(|i| i.get("label").and_then(|l| l.as_str()) == Some(label)).unwrap();
        assert!(it.get("insertText").and_then(|t| t.as_str()).unwrap().contains('('),
            "{label}: condición entre paréntesis");
    }
    let iflet = items.iter().find(|i| i.get("label").and_then(|l| l.as_str()) == Some("if let … = … { }")).unwrap();
    assert!(!iflet.get("insertText").and_then(|t| t.as_str()).unwrap().starts_with("if let ("),
        "if let: escrutinio SIN paréntesis");
    // La keyword pelada sigue ofreciéndose (posiciones donde el bloque no aplica).
    for kw in ["fn", "if", "while", "for", "match", "let", "var", "struct", "enum", "trait", "impl", "const"] {
        assert!(items.iter().any(|i| i.get("label").and_then(|l| l.as_str()) == Some(kw)),
            "{kw}: keyword pelada también presente");
    }
}

/// Helper: labels de la completion en `(line, character)` (0-basados) sobre `src`.
fn completion_labels(src: &str, line: usize, character: usize) -> Vec<String> {
    let msg = json::parse(&format!(
        r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{character}}}}}}}"#
    )).unwrap();
    let mut docs = HashMap::new();
    docs.insert("file:///t.ray".to_string(), src.to_string());
    completion_result(&msg, &docs).as_array().unwrap().iter()
        .filter_map(|i| i.get("label").and_then(|l| l.as_str()).map(|s| s.to_string()))
        .collect()
}

#[test]
fn completion_of_from_import_symbols_of_embedded_std() {
    // IDEAS §56: tras `from std/M import ` se ofrecen los `pub` del módulo aunque sea EMBEBIDO
    // (sin archivo en disco — antes la resolución iba solo a disco y devolvía []).
    let src = "from std/units import \nfn main() { print(1) }\n";
    let labels = completion_labels(src, 0, 22);
    for expected in ["kb", "mb", "gb"] {
        assert!(labels.contains(&expected.to_string()), "ofrece {expected}: {labels:?}");
    }
    let src = "from std/time import \nfn main() { print(1) }\n";
    let labels = completion_labels(src, 0, 21);
    for expected in ["seconds", "minutes", "sleep"] {
        assert!(labels.contains(&expected.to_string()), "ofrece {expected}: {labels:?}");
    }
}

/// M104 — `ray fmt` reparte un `from … import` largo en varias líneas, así que el cursor puede estar
/// en una línea de CONTINUACIÓN (`    seconds,`), cuyo prefijo no dice nada: el contexto de import se
/// reconstruye desde el inicio de la sentencia, no desde el inicio de la línea.
#[test]
fn completion_of_from_import_symbols_on_a_wrapped_import() {
    let src = "from std/time import\n    now,\n    \nfn main() { print(1) }\n";
    let labels = completion_labels(src, 2, 4);
    for expected in ["seconds", "minutes", "sleep"] {
        assert!(labels.contains(&expected.to_string()), "ofrece {expected} en continuacion: {labels:?}");
    }
    // La primera línea (`from std/time import`) sigue funcionando.
    let labels = completion_labels(src, 0, 20);
    assert!(labels.contains(&"sleep".to_string()), "cabecera del envuelto: {labels:?}");
    // Cerrada la sentencia con `;`, la línea siguiente ya NO es contexto de import.
    let closed = "from std/time import\n    now;\n\nfn main() { print(1) }\n";
    let labels = completion_labels(closed, 2, 0);
    assert!(!labels.contains(&"minutes".to_string()), "tras el ';' no es import: {labels:?}");
}

#[test]
fn completion_of_module_path_includes_embedded_std() {
    // IDEAS §56: en posición de RUTA (`from <cursor>`) la stdlib embebida se ofrece aunque no haya
    // raíces de proyecto (un buffer suelto sin main.ray ancestro).
    let src = "from std/uni\nfn main() { print(1) }\n";
    let labels = completion_labels(src, 0, 12);
    assert!(labels.contains(&"std/units".to_string()), "ofrece std/units: {labels:?}");
    assert!(labels.contains(&"std/time".to_string()), "ofrece std/time: {labels:?}");
}

#[test]
fn completion_of_struct_members() {
    // M45: `p.` sobre un struct ofrece sus campos y sus métodos de trait, no los símbolos de archivo.
    let src = "struct P { x: int, y: int }\ntrait Ver { fn see(self) -> int; }\nimpl Ver for P { fn see(self) -> int { self.x } }\nfn sum(p: P) -> int { p.x + p.y }\nfn main() -> int {\n    let p = P { x: 1, y: 2 };\n    p.\n    0\n}\n";
    let labels = completion_labels(src, 6, 6); // línea "    p." (0-basada), tras el punto
    assert!(labels.contains(&"x".to_string()) && labels.contains(&"y".to_string()), "fields: {labels:?}");
    assert!(labels.contains(&"see".to_string()), "método de trait: {labels:?}");
    assert!(labels.contains(&"sum".to_string()), "UFCS del user: {labels:?}");
    // NO ofrece los símbolos de archivo (no es una completion de archivo tras el punto).
    assert!(!labels.contains(&"print".to_string()), "sin builtins globales: {labels:?}");
    assert!(!labels.contains(&"while".to_string()), "sin words clave: {labels:?}");
}

#[test]
fn completion_of_string_and_array_members() {
    // string: builtins de string + métodos de trait; NADA de funciones de E/S que toman una ruta.
    let s = "fn main() -> int {\n    let s = \"h\";\n    s.\n    0\n}\n";
    let ls = completion_labels(s, 2, 6);
    assert!(ls.contains(&"trim".to_string()) && ls.contains(&"split".to_string()) && ls.contains(&"len".to_string()), "string builtins: {ls:?}");
    assert!(!ls.contains(&"read_file".to_string()) && !ls.contains(&"env".to_string()), "sin E/S about string: {ls:?}");
    // array: builtins + orden superior del prelude por UFCS.
    let a = "fn main() -> int {\n    let xs = [1, 2, 3];\n    xs.\n    0\n}\n";
    let la = completion_labels(a, 2, 7);
    for m in ["len", "push", "reverse", "map", "filter", "fold", "sort"] {
        assert!(la.contains(&m.to_string()), "array must ofrecer '{m}': {la:?}");
    }
}

#[test]
fn completion_of_members_in_expression_context() {
    // M45b: `x.` como argumento de una llamada NO debe romper el parseo (bug del `;` dentro
    // del paréntesis). `sum(x.)` ofrece los miembros del array.
    let src = "fn sum(xs: [int]) -> int { 0 }\nfn main() -> int {\n    let x = [1, 2];\n    let y = sum(x.);\n    0\n}\n";
    let labels = completion_labels(src, 3, 18); // tras "sum(x." (el punto está en col 17, cursor 18)
    assert!(labels.contains(&"len".to_string()) && labels.contains(&"map".to_string()), "en expresión: {labels:?}");
}

#[test]
fn completion_of_members_snippet_and_doc() {
    // M45b/M46c: un método con args → snippet con placeholders por parámetro, sin el receptor
    // (`doblar(${1:k})`); un campo no inserta `()`; los métodos del usuario traen su doc `///`.
    let src = "struct P { x: int }\n/// Duplica x.\nfn fold(p: P, k: int) -> int { p.x * k }\nfn main() -> int {\n    let p = P { x: 1 };\n    p.\n    0\n}\n";
    let msg = json::parse(
        r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":5,"character":6}}}"#
    ).unwrap();
    let mut docs = HashMap::new();
    docs.insert("file:///t.ray".to_string(), src.to_string());
    let items = completion_result(&msg, &docs);
    let arr = items.as_array().unwrap();
    let fold = arr.iter().find(|i| i.get("label").and_then(|l| l.as_str()) == Some("fold")).expect("fold");
    assert_eq!(fold.get("insertText").and_then(|t| t.as_str()), Some("fold(${1:k})"), "snippet con placeholder, sin receptor");
    assert!(fold.get("command").is_some(), "dispara signature help");
    let doc = fold.get("documentation").and_then(|d| d.get("value")).and_then(|v| v.as_str()).unwrap_or("");
    assert!(doc.contains("Duplica x"), "doc /// del método: {doc}");
    // el campo x no es invocable → sin insertText de llamada.
    let x = arr.iter().find(|i| i.get("label").and_then(|l| l.as_str()) == Some("x")).expect("x");
    assert!(x.get("insertText").is_none(), "un campo no inserta ()");
}

#[test]
fn completion_after_pipe_is_type_aware() {
    // La completion tras `|>` ofrece las funciones aplicables al tipo del operando izquierdo
    // (`x |> f` ≡ `f(x)`), como el acceso a miembro: `duplicar(int)` sí, `saludar(string)` no.
    let src = "fn duplicate(n: int) -> int { n * 2 }\nfn greet(s: string) -> string { s }\nfn main() -> int {\n    let x = 5;\n    x |> d\n    0\n}\n";
    // Línea 4 = "    x |> d"; el cursor va tras la `d` (columna 10).
    let labels = completion_labels(src, 4, 10);
    assert!(labels.contains(&"duplicate".to_string()), "offers la función de int: {labels:?}");
    assert!(!labels.contains(&"greet".to_string()), "NO offers la función de string: {labels:?}");
    // También un builtin aplicable a int (to_string) — enumera builtins-como-método.
    assert!(labels.contains(&"to_string".to_string()), "offers builtins de int: {labels:?}");
    // Un método de trait (show) NO es pipeable: `n |> show` sería `show(n)`, y no hay `show` libre.
    assert!(!labels.contains(&"show".to_string()), "NO offers méall de trait: {labels:?}");

    // Segundo pipe SIN prefijo, cursor justo tras `|>` (lo dispara el trigger char `>`): sobre
    // `v |> duplicar() |>` el operando izquierdo sigue siendo int → ofrece las mismas funciones.
    let src2 = "fn duplicate(n: int) -> int { n * 2 }\nfn main() -> int {\n    let v = 5;\n    v |> duplicate() |>\n    0\n}\n";
    let line = "    v |> duplicate() |>"; // cursor al final (tras el segundo `|>`)
    let labels2 = completion_labels(src2, 3, line.chars().count());
    assert!(labels2.contains(&"duplicate".to_string()), "segundo pipe sin prefijo: {labels2:?}");
}

#[test]
fn completion_after_pipe_inserts_with_space_and_space_triggers() {
    let src = "fn duplicate(n: int) -> int { n * 2 }\nfn main() -> int {\n    let v = 5;\n    v |>\n    0\n}\n";
    // (a) Pegado al `|>` (cursor tras `>`): el insertText lleva un espacio inicial → `|> duplicar()`.
    let msg = json::parse(
        r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":3,"character":8}}}"#
    ).unwrap();
    let mut docs = HashMap::new();
    docs.insert("file:///t.ray".to_string(), src.to_string());
    let arr = completion_result(&msg, &docs);
    let dup = arr.as_array().unwrap().iter()
        .find(|i| i.get("label").and_then(|l| l.as_str()) == Some("duplicate")).expect("duplicate");
    assert_eq!(dup.get("insertText").and_then(|t| t.as_str()), Some(" duplicate()"), "pegado → espacio inicial");

    // (b) El espacio como trigger dispara el pipeline: `v |> ` (con espacio) sigue ofreciendo.
    let src_sp = "fn duplicate(n: int) -> int { n * 2 }\nfn main() -> int {\n    let v = 5;\n    v |> \n    0\n}\n";
    let msg_sp = json::parse(
        r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":3,"character":9},"context":{"triggerKind":2,"triggerCharacter":" "}}}"#
    ).unwrap();
    docs.insert("file:///t.ray".to_string(), src_sp.to_string());
    let arr_sp = completion_result(&msg_sp, &docs);
    let has = arr_sp.as_array().unwrap().iter()
        .any(|i| i.get("label").and_then(|l| l.as_str()) == Some("duplicate"));
    assert!(has, "el espacio-trigger follows ofreciendo en pipeline");
    // Y su insertText NO duplica el espacio (ya hay uno antes del cursor).
    let dup_sp = arr_sp.as_array().unwrap().iter()
        .find(|i| i.get("label").and_then(|l| l.as_str()) == Some("duplicate")).unwrap();
    assert_eq!(dup_sp.get("insertText").and_then(|t| t.as_str()), Some("duplicate()"), "con espacio previo → sin duplicate");

    // (c) El espacio como trigger FUERA de un pipeline no ofrece nada (no inunda el archivo).
    let src_no = "fn main() -> int {\n    let w = \n    0\n}\n";
    let msg_no = json::parse(
        r#"{"params":{"textDocument":{"uri":"file:///t.ray"},"position":{"line":1,"character":12},"context":{"triggerKind":2,"triggerCharacter":" "}}}"#
    ).unwrap();
    docs.insert("file:///t.ray".to_string(), src_no.to_string());
    let arr_no = completion_result(&msg_no, &docs);
    assert!(arr_no.as_array().unwrap().is_empty(), "espacio outside de pipeline → vacío");
}

#[test]
fn completion_of_members_in_interpolation() {
    // M45b: dentro de `${x.}` el LSP ofrece los miembros (el reparado no rompe la cadena).
    let src = "fn main() -> int {\n    let x = [1, 2];\n    let y = \"n ${x.}\";\n    0\n}\n";
    let line = "    let y = \"n ${x.}\";";
    let col = line.find("x.").unwrap() + 2; // tras el punto
    let labels = completion_labels(src, 2, col);
    assert!(labels.contains(&"len".to_string()) && labels.contains(&"push".to_string()), "en interpolación: {labels:?}");
}

#[test]
fn completion_of_import_symbols_and_paths() {
    // M45c: crea un proyecto temporal en disco y comprueba el completion de import.
    let base = std::env::temp_dir().join("ray_lsp_import_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("geo/formas")).unwrap();
    std::fs::create_dir_all(base.join("util")).unwrap();
    let w = |rel: &str, txt: &str| std::fs::write(base.join(rel), txt).unwrap();
    w("main.ray", "fn main() -> int { 0 }\n");
    w("geo.ray", "pub struct Circle { r: int }\npub fn area(c: Circle) -> int { c.r }\nfn internal() -> int { 0 }\n");
    w("geo/formas/circle.ray", "pub fn dibujar() -> int { 0 }\n");
    w("util/mod.ray", "pub fn publico() -> int { 0 }\n"); // cápsula
    w("util/internal.ray", "pub fn oculto() -> int { 0 }\n"); // interno a la cápsula

    let uri = format!("file://{}/main.ray", base.display());
    let labels = |line: &str, ch: usize| -> Vec<String> {
        let src = format!("{line}\nfn main() -> int {{ 0 }}\n");
        let mut docs = HashMap::new();
        docs.insert(uri.clone(), src);
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":0,"character":{ch}}}}}}}"#
        )).unwrap();
        completion_result(&msg, &docs).as_array().unwrap().iter()
            .filter_map(|i| i.get("label").and_then(|l| l.as_str()).map(|s| s.to_string())).collect()
    };

    // from geo import <cursor> → símbolos `pub` (no `interno`).
    let syms = labels("from geo import ", 16);
    assert!(syms.contains(&"Circle".to_string()) && syms.contains(&"area".to_string()), "pub de geo: {syms:?}");
    assert!(!syms.contains(&"internal".to_string()), "no expone lo private: {syms:?}");
    assert!(!syms.contains(&"print".to_string()), "no cae al completion de file: {syms:?}");

    // import <cursor> → rutas de módulo; la cápsula `util` sí, su interno NO.
    let paths = labels("import ", 7);
    assert!(paths.contains(&"geo".to_string()), "módulo geo: {paths:?}");
    assert!(paths.contains(&"geo/formas/circle".to_string()), "path de directories: {paths:?}");
    assert!(paths.contains(&"util".to_string()), "la cápsula util: {paths:?}");
    assert!(!paths.contains(&"util/internal".to_string()), "el internal de la cápsula queda oculto: {paths:?}");

    // M45c-3: acceso calificado `u.` (alias) / `circulo.` (leaf) → símbolos `pub` del módulo.
    let qualified = |body: &str, line: usize, ch: usize| -> Vec<String> {
        let src = format!("import geo/formas/circle;\nimport geo as u;\nfn main() -> int {{\n{body}\n0\n}}\n");
        let mut docs = HashMap::new();
        docs.insert(uri.clone(), src);
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
        )).unwrap();
        completion_result(&msg, &docs).as_array().unwrap().iter()
            .filter_map(|i| i.get("label").and_then(|l| l.as_str()).map(|s| s.to_string())).collect()
    };
    let by_alias = qualified("    u.", 3, 6); // `import geo as u` → símbolos pub de geo
    assert!(by_alias.contains(&"Circle".to_string()) && by_alias.contains(&"area".to_string()), "alias u.: {by_alias:?}");
    assert!(!by_alias.contains(&"internal".to_string()), "no expone lo private del módulo: {by_alias:?}");
    let by_leaf = qualified("    circle.", 3, 12); // leaf `circulo` (geo/formas/circulo.ray)
    assert!(by_leaf.contains(&"dibujar".to_string()), "leaf circle.: {by_leaf:?}");

    // M46c: en el acceso calificado, las funciones traen firma + snippet con placeholders, y las
    // firmas de un RE-EXPORT de cápsula se resuelven en el módulo origen (no en `mod.ray`).
    std::fs::write(base.join("util/mod.ray"),
        "pub from util/internal import greet;\npub fn publico() -> int { 0 }\n").unwrap();
    std::fs::write(base.join("util/internal.ray"),
        "pub fn greet(name: string, times: int) -> string { name }\n").unwrap();
    let items_at = |body: &str, line: usize, ch: usize| -> Json {
        let src = format!("import util;\nfn main() -> int {{\n{body}\n0\n}}\n");
        let mut docs = HashMap::new();
        docs.insert(uri.clone(), src);
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
        )).unwrap();
        completion_result(&msg, &docs)
    };
    let it = |arr: &Json, label: &str| arr.as_array().unwrap().iter()
        .find(|i| i.get("label").and_then(|l| l.as_str()) == Some(label)).cloned();
    let items = items_at("    util.", 2, 9);
    let greet = it(&items, "greet").expect("re-export greet");
    assert_eq!(greet.get("insertText").and_then(Json::as_str), Some("greet(${1:name}, ${2:times})"),
               "signature del re-export resuelta en el módulo origen + placeholders");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn completion_imported_symbols_and_enum_variants() {
    // Proyecto: un módulo `figuras` con una función `pub`, un struct y un enum; el archivo de
    // entrada los importa.
    let base = std::env::temp_dir().join("ray_lsp_import_syms_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("main.ray"), "fn main() -> int { 0 }\n").unwrap();
    std::fs::write(base.join("figuras.ray"),
        "pub fn area(a: int, b: int) -> int { a * b }\npub struct Rect { ancho: int }\npub enum Orientation { Horizontal, Vertical }\n").unwrap();
    let uri = format!("file://{}/main.ray", base.display());
    let items = |body: &str, line: usize, ch: usize| -> Vec<(String, i64)> {
        let src = format!("import figuras;\nfrom figuras import Orientation, area;\nfn main() -> int {{\n{body}\n0\n}}\n");
        let mut docs = HashMap::new();
        docs.insert(uri.clone(), src);
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
        )).unwrap();
        completion_result(&msg, &docs).as_array().unwrap().iter()
            .filter_map(|i| Some((i.get("label")?.as_str()?.to_string(), as_usize(i.get("kind")?)? as i64)))
            .collect()
    };
    // Completion de archivo: el nombre de módulo `figuras` (kind 9) y los from-imports.
    let file_items = items("    x", 3, 5);
    assert!(file_items.iter().any(|(l, k)| l == "figuras" && *k == 9), "módulo figuras (kind Module): {file_items:?}");
    assert!(file_items.iter().any(|(l, _)| l == "Orientation"), "from-import Orientation: {file_items:?}");
    assert!(file_items.iter().any(|(l, _)| l == "area"), "from-import area: {file_items:?}");
    // `Orientation.` → sus variantes (kind 20 = EnumMember).
    let vars = items("    Orientation.", 3, 16);
    assert!(vars.iter().any(|(l, k)| l == "Horizontal" && *k == 20), "variant Horizontal: {vars:?}");
    assert!(vars.iter().any(|(l, _)| l == "Vertical"), "variant Vertical: {vars:?}");
    assert!(!vars.iter().any(|(l, _)| l == "figuras"), "after el punto NO sale la completion de file: {vars:?}");

    // Una variante con payload muestra los tipos en el popup (`labelDetails.detail`).
    let src = "enum Shape { Circulo(float), Rect(float, float), Punto }\nfn main() -> int {\n    Shape.\n0\n}\n";
    let mut docs = HashMap::new();
    docs.insert(uri.clone(), src.to_string());
    let msg = json::parse(&format!(
        r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":2,"character":10}}}}}}"#
    )).unwrap();
    let vs = completion_result(&msg, &docs);
    let rect = vs.as_array().unwrap().iter()
        .find(|i| i.get("label").and_then(Json::as_str) == Some("Rect")).unwrap();
    let det = rect.get("labelDetails").and_then(|d| d.get("detail")).and_then(Json::as_str);
    assert_eq!(det, Some("(float, float)"), "types del payload en el popup: {rect:?}");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn completion_and_signature_of_associated_functions() {
    // M48.1: `Channel.` completa `new`/`bounded` (kind 3); el sig help de `Channel.bounded(` sale
    // del registro de asociadas.
    let uri = "file:///t.ray";
    let src = "fn main() -> int {\n    let c: Channel<int> = Channel.\n    0\n}\n";
    let mut docs = HashMap::new();
    docs.insert(uri.to_string(), src.to_string());
    let msg = json::parse(&format!(
        r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":1,"character":34}}}}}}"#
    )).unwrap();
    let vs = completion_result(&msg, &docs);
    let labels: Vec<&str> = vs.as_array().unwrap().iter()
        .filter_map(|i| i.get("label").and_then(Json::as_str)).collect();
    assert!(labels.contains(&"new") && labels.contains(&"bounded"), "asociadas de Channel: {labels:?}");
    // Signature help dentro de `Channel.bounded(`.
    let src2 = "fn main() -> int {\n    let c: Channel<int> = Channel.bounded(\n    0\n}\n";
    docs.insert(uri.to_string(), src2.to_string());
    let msg2 = json::parse(&format!(
        r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":1,"character":42}}}}}}"#
    )).unwrap();
    let r = signature_help_result(&msg2, &docs);
    let label = r.get("signatures").and_then(|s| s.as_array()).and_then(|a| a.first())
        .and_then(|s| s.get("label")).and_then(Json::as_str).unwrap_or("");
    assert_eq!(label, "Channel.bounded(n: int) -> Channel<T>", "sig de Channel.bounded: {r:?}");
}

#[test]
fn signature_help_of_embedded_std_functions() {
    // IDEAS §56 (2ª tanda): el BFS de SigCtx resolvía imports solo por disco → sin firma para las
    // funciones de la stdlib EMBEBIDA. Cubre la calificada (`units.kb(`) y la UFCS (`64.kb(`,
    // receptor-valor: se recorta el primer parámetro).
    let sig = |src: &str, line: usize, ch: usize| -> Option<String> {
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
        )).unwrap();
        let r = signature_help_result(&msg, &docs);
        r.get("signatures").and_then(|s| s.as_array()).and_then(|a| a.first())
            .and_then(|s| s.get("label")).and_then(Json::as_str).map(|s| s.to_string())
    };
    let src = "import std/units;\nfn main() {\n    print(units.kb(\n}\n";
    assert_eq!(sig(src, 2, 19).as_deref(), Some("fn kb(n: int) -> int"), "calificada units.kb(");
    let src = "from std/units import kb;\nfn main() {\n    print(64.kb(\n}\n";
    assert_eq!(sig(src, 2, 16).as_deref(), Some("fn kb() -> int"), "UFCS 64.kb( (receptor recortado)");
    let src = "from std/time import seconds;\nfn main() {\n    print(30.seconds(\n}\n";
    assert_eq!(sig(src, 2, 21).as_deref(), Some("fn seconds() -> int"), "UFCS 30.seconds(");
}

#[test]
fn signature_help_methods_y_prelude() {
    // M46b: el signature help resuelve funciones del prelude y recorta el receptor en un método,
    // pero NO en una llamada calificada de módulo (esa parte cross-módulo se cubre en el CLI).
    let sig = |src: &str, line: usize, ch: usize| -> Option<(String, usize)> {
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
        )).unwrap();
        let r = signature_help_result(&msg, &docs);
        if r == Json::Null { return None; }
        let label = r.get("signatures").and_then(|s| s.as_array()).and_then(|a| a.first())
            .and_then(|s| s.get("label")).and_then(Json::as_str)?.to_string();
        let active = r.get("activeParameter").and_then(as_usize).unwrap_or(0);
        Some((label, active))
    };
    // Prelude: sort(.
    assert_eq!(sig("fn main() -> int {\n    let xs = [3,1];\n    sort(\n}\n", 2, 9),
               Some(("fn sort(a: [T]) -> [T]".into(), 0)));
    // Método: p.doblar( → receptor recortado, `(k: int)`.
    let m = "struct P { x: int }\nfn fold(p: P, k: int) -> int { p.x }\nfn main() -> int {\n    let p = P { x: 1 };\n    p.fold(\n}\n";
    assert_eq!(sig(m, 4, 13), Some(("fn fold(k: int) -> int".into(), 0)));
    // Función libre con una coma: doblar(1, → firma completa, param activo 1.
    let free_call = "fn fold(p: int, k: int) -> int { p }\nfn main() -> int {\n    fold(1, \n}\n";
    assert_eq!(sig(free_call, 2, 13), Some(("fn fold(p: int, k: int) -> int".into(), 1)));
    // Builtin: pow(.
    assert_eq!(sig("fn main() -> int {\n    let x = pow(\n    0\n}\n", 1, 16),
               Some(("fn pow(base: float, exp: float) -> float".into(), 0)));
    // Construcción de variante de enum: `Shape.Rect(1.0, ` → firma con los tipos del payload,
    // param activo 1. No es una `fn`, pero el receptor es un enum con esa variante.
    let e = "enum Shape { Circulo(float), Rect(float, float) }\nfn main() -> int {\n    let r: Shape = Shape.Rect(1.0, \n    0\n}\n";
    assert_eq!(sig(e, 2, 34), Some(("Shape.Rect(float, float)".into(), 1)));
}

#[test]
fn completion_snippet_with_placeholders_per_parameter() {
    // M46c: el insertText usa un placeholder por parámetro (nombre), navegable con Tab.
    let insert = |src: &str, line: usize, ch: usize, label: &str| -> Option<String> {
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
        )).unwrap();
        completion_result(&msg, &docs).as_array().unwrap().iter()
            .find(|i| i.get("label").and_then(|l| l.as_str()) == Some(label))
            .and_then(|i| i.get("insertText")).and_then(Json::as_str).map(|s| s.to_string())
    };
    // Función de archivo con dos params.
    let f = "fn fold(p: int, k: int) -> int { p }\nfn main() -> int {\n    dob\n    0\n}\n";
    assert_eq!(insert(f, 2, 7, "fold").as_deref(), Some("fold(${1:p}, ${2:k})"));
    // Método: receptor recortado → solo el resto.
    let m = "struct P { x: int }\nfn tri(p: P, k: int) -> int { p.x }\nfn main() -> int {\n    let p = P { x: 1 };\n    p.\n    0\n}\n";
    assert_eq!(insert(m, 4, 6, "tri").as_deref(), Some("tri(${1:k})"));
    // Builtin sin argumentos → `()`.
    let a = "fn main() -> int {\n    let xs = [1];\n    xs.\n    0\n}\n";
    assert_eq!(insert(a, 2, 7, "len").as_deref(), Some("len()"));
}

#[test]
fn completion_of_struct_literal_fields() {
    // M47a: dentro de `Nombre { … }` (posición de nombre de campo), los campos del struct.
    let labels = |body: &str, line: usize, ch: usize| -> Vec<(String, i64, Option<String>)> {
        let src = format!("struct Point {{ x: int, y: int }}\nfn dobla(n: int) -> int {{ n }}\nfn main() -> int {{\n{body}\n0\n}}\n");
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src);
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
        )).unwrap();
        completion_result(&msg, &docs).as_array().unwrap().iter()
            .map(|i| (i.get("label").unwrap().as_str().unwrap().to_string(),
                      as_usize(i.get("kind").unwrap()).unwrap() as i64,
                      i.get("insertText").and_then(Json::as_str).map(|s| s.to_string())))
            .collect()
    };
    // `Point { |` → ambos campos, kind Field (5), insertText `campo: `.
    let empty = labels("    let p = Point { ", 3, 20);
    assert!(empty.iter().any(|(l, k, ins)| l == "x" && *k == 5 && ins.as_deref() == Some("x: ")), "campo x: {empty:?}");
    assert!(empty.iter().any(|(l, _, _)| l == "y"), "campo y: {empty:?}");
    assert!(!empty.iter().any(|(l, _, _)| l == "print"), "no cae a la completion de file: {empty:?}");
    // `Point { x: 1, |` → solo el campo que falta.
    let one = labels("    let p = Point { x: 1, ", 3, 26);
    assert!(one.iter().any(|(l, _, _)| l == "y") && !one.iter().any(|(l, _, _)| l == "x"),
            "excluye el campo ya escrito: {one:?}");
    // `Point { x: dob|` (posición de VALOR) → cae a la completion de archivo (dobla), no campos.
    let value_labels = labels("    let p = Point { x: dob", 3, 26);
    assert!(value_labels.iter().any(|(l, _, _)| l == "dobla"), "en posición de valor, completion de file: {value_labels:?}");

    // M47b: al teclear el TIPO, un ítem extra `Point {…}` que inserta el literal con placeholders,
    // aparte del tipo pelado `Point`.
    let type_labels = labels("    let p = Poi", 3, 15);
    assert!(type_labels.iter().any(|(l, k, _)| l == "Point" && *k == 22), "el type pelado follows: {type_labels:?}");
    assert!(type_labels.iter().any(|(l, k, ins)| l == "Point {…}" && *k == 15
        && ins.as_deref() == Some("Point { x: ${1:int}, y: ${2:int} }")), "el literal-snippet: {type_labels:?}");
}

#[test]
fn hover_of_associated_function() {
    // M48.1: hover sobre el nombre asociado (`Channel.new`) → su firma del registro de asociadas.
    let src = "fn main() -> int {\n    let ch: Channel<int> = Channel.new();\n    0\n}\n";
    let mut docs = HashMap::new();
    docs.insert("file:///t.ray".to_string(), src.to_string());
    // Posición sobre `new` (tras `Channel.`).
    let cn = src.lines().nth(1).unwrap().find("Channel.new").unwrap() + "Channel.".len() + 1;
    let msg = json::parse(&format!(
        r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":1,"character":{cn}}}}}}}"#
    )).unwrap();
    let r = hover_result(&msg, &docs);
    let v = r.get("contents").and_then(|c| c.get("value")).and_then(Json::as_str).unwrap_or("");
    assert_eq!(v, "Channel.new() -> Channel<T>", "hover de Channel.new: {v}");
}

#[test]
fn hover_of_const_and_builtin_type() {
    let hover_of = |src: &str, line: usize, ch: usize| -> String {
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
        )).unwrap();
        let r = hover_result(&msg, &docs);
        r.get("contents").and_then(|c| c.get("value")).and_then(Json::as_str).unwrap_or("").to_string()
    };
    // Uso de una constante → su tipo (como una variable).
    let c = hover_of("const MAXIMO: int = 100;\nfn main() -> int {\n    let x = MAXIMO;\n    x\n}\n", 2, 13);
    assert!(c.contains("MAXIMO: int"), "hover de const en use: {c}");
    // Tipo incorporado (Channel) → descripción breve.
    let t = hover_of("fn main() -> int {\n    let ch: Channel<int> = channel();\n    0\n}\n", 1, 13);
    assert!(t.contains("Channel<T>"), "hover de type Channel: {t}");
}

#[test]
fn completion_offers_builtin_consts_and_types() {
    let comp = |src: &str, line: usize, ch: usize| -> Vec<String> {
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
        )).unwrap();
        completion_result(&msg, &docs).as_array().unwrap().iter()
            .filter_map(|i| i.get("label").and_then(Json::as_str).map(|s| s.to_string())).collect()
    };
    // Constante de nivel superior.
    let c = comp("const MAXIMO: int = 100;\nfn main() -> int {\n    x\n    0\n}\n", 2, 5);
    assert!(c.contains(&"MAXIMO".to_string()), "const MAXIMO: {c:?}");
    // Tipos genéricos incorporados / del prelude.
    for t in ["Channel", "Task", "Map", "Option", "Result"] {
        assert!(c.contains(&t.to_string()), "type {t}: {c:?}");
    }
}

#[test]
fn completion_shows_signature_in_detail() {
    // M46a: los ítems invocables llevan `labelDetails` con params y retorno.
    let detail_of = |src: &str, line: usize, ch: usize, label: &str| -> Option<(String, String)> {
        let mut docs = HashMap::new();
        docs.insert("file:///t.ray".to_string(), src.to_string());
        let msg = json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"file:///t.ray"}},"position":{{"line":{line},"character":{ch}}}}}}}"#
        )).unwrap();
        completion_result(&msg, &docs).as_array().unwrap().iter()
            .find(|i| i.get("label").and_then(|l| l.as_str()) == Some(label))
            .and_then(|i| i.get("labelDetails"))
            .map(|ld| (
                ld.get("detail").and_then(Json::as_str).unwrap_or("").to_string(),
                ld.get("description").and_then(Json::as_str).unwrap_or("").to_string(),
            ))
    };
    // Función de archivo (incompleta: `parse_all` la recupera) → firma completa.
    let src = "fn fold(p: int, k: int) -> int { p * k }\nfn main() -> int {\n    dob\n    0\n}\n";
    assert_eq!(detail_of(src, 2, 7, "fold"), Some(("(p: int, k: int)".into(), "int".into())));
    // Método → sin el receptor.
    let m = "struct P { x: int }\nfn tri(p: P, k: int) -> int { p.x }\nfn main() -> int {\n    let p = P { x: 1 };\n    p.\n    0\n}\n";
    assert_eq!(detail_of(m, 4, 6, "tri"), Some(("(k: int)".into(), "int".into())));
    // Builtin-método → firma sin receptor.
    let a = "fn main() -> int {\n    let xs = [1, 2];\n    xs.\n    0\n}\n";
    assert_eq!(detail_of(a, 2, 7, "push"), Some(("(value: T)".into(), "unit".into())));
}

#[test]
fn completion_without_dot_stays_file_completion() {
    // Sin `.` delante, la completion es la de archivo (regresión de M10.2e).
    let src = "fn double(n: int) -> int { n + n }\nfn main() -> int { 0 }\n";
    let labels = completion_labels(src, 1, 19);
    assert!(labels.contains(&"double".to_string()) && labels.contains(&"print".to_string()), "{labels:?}");
}

#[test]
fn analyzes_type_error() {
    let d = analyze("fn main() -> int { 1 + true }").expect("debería haber error");
    assert_eq!(d.line, 1);
    assert!(d.col >= 1);
    assert!(!d.message.is_empty());
}

#[test]
fn diagnostic_uses_0_based_coordinates() {
    // Error en la línea 2: la fase reporta 1-basado; LSP debe verlo 0-basado.
    let src = "fn main() -> int {\n    1 + true\n}";
    let d = analyze(src).unwrap();
    assert_eq!(d.line, 2); // 1-basado
    let dj = diagnostic_json(src, &d);
    let start = dj.get("range").unwrap().get("start").unwrap();
    assert_eq!(start.get("line"), Some(&Json::Num(1.0))); // 0-basado
    assert_eq!(dj.get("severity"), Some(&Json::Num(1.0))); // Error
}

/// Enmarca un cuerpo JSON con su cabecera `Content-Length`, como un cliente real.
fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

#[test]
fn serve_responde_initialize_y_public_diagnostics() {
    let mut input = String::new();
    input.push_str(&frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#));
    // didOpen de un programa con error de tipos.
    input.push_str(&frame(
        r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.ray","text":"fn main() -> int { 1 + true }"}}}"#,
    ));
    input.push_str(&frame(r#"{"jsonrpc":"2.0","method":"exit"}"#));

    let mut reader = io::Cursor::new(input.into_bytes());
    let mut output: Vec<u8> = Vec::new();
    serve(&mut reader, &mut output);
    let out = String::from_utf8(output).unwrap();

    assert!(out.contains("\"id\":1"));
    assert!(out.contains("\"capabilities\""));
    // Las capacidades nuevas se anuncian (el cliente las descubre aquí).
    assert!(out.contains("documentFormattingProvider"));
    assert!(out.contains("documentSymbolProvider"));
    assert!(out.contains("documentHighlightProvider"));
    assert!(out.contains("textDocument/publishDiagnostics"));
    assert!(out.contains("\"severity\":1"));
}

#[test]
fn serve_program_valid_public_list_empty() {
    let body = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///ok.ray","text":"fn main() -> int { 42 }"}}}"#;
    let mut input = frame(body);
    input.push_str(&frame(r#"{"jsonrpc":"2.0","method":"exit"}"#));

    let mut reader = io::Cursor::new(input.into_bytes());
    let mut output: Vec<u8> = Vec::new();
    serve(&mut reader, &mut output);
    let out = String::from_utf8(output).unwrap();
    assert!(out.contains("\"diagnostics\":[]"));
}

#[test]
fn completion_in_a_template_offers_for_params_and_vars() {
    // M55: dentro de `{{ }}` se ofrecen los params tipados de la cabecera; dentro de un
    // `{% for %}` abierto, también la variable de bucle (con su tipo inferido del `[T]`);
    // en contexto de etiqueta `{%`, además las keywords; fuera de los delimitadores, nada.
    let tpl = "{% params titulo: string, rows: [string], total: int %}\n\
               <h1>{{ ti }}</h1>\n\
               {% for row in rows %}<li>{{ f }}</li>{% endfor %}\n\
               {% if total > 0 %}<p>hay</p>{% endif %}\n\
               <p>outside</p>\n";
    let labels = |line0: usize, char0: usize| -> Vec<(String, Option<String>)> {
        template_completion_items(tpl, line0, char0).get("items").unwrap().as_array().unwrap().iter()
            .map(|i| (
                i.get("label").and_then(Json::as_str).unwrap().to_string(),
                i.get("detail").and_then(Json::as_str).map(|s| s.to_string()),
            ))
            .collect()
    };
    // Dentro de `{{ ti| }}` (línea 2): los tres params con su tipo; sin keywords de etiqueta.
    let en_expr = labels(1, 10);
    assert!(en_expr.contains(&("titulo".into(), Some("string".into()))), "{en_expr:?}");
    assert!(en_expr.contains(&("rows".into(), Some("[string]".into()))), "{en_expr:?}");
    assert!(en_expr.contains(&("total".into(), Some("int".into()))), "{en_expr:?}");
    assert!(!en_expr.iter().any(|(l, _)| l == "endif"), "sin keywords de tag en {{{{ }}}}: {en_expr:?}");
    assert!(!en_expr.iter().any(|(l, _)| l == "row"), "el for aún no está abierto: {en_expr:?}");
    // Dentro del for (línea 3, `{{ f| }}`): la variable de bucle con el tipo del elemento.
    let en_for = labels(2, 30);
    assert!(en_for.contains(&("row".into(), Some("string".into()))), "{en_for:?}");
    // En una etiqueta `{% if |` (línea 4): params + keywords.
    let en_tag = labels(3, 6);
    assert!(en_tag.iter().any(|(l, _)| l == "total"), "{en_tag:?}");
    assert!(en_tag.iter().any(|(l, _)| l == "endif"), "{en_tag:?}");
    // Fuera de los delimitadores (línea 5): NADA de variables (el HTML no es nuestro), solo
    // los snippets de bloque (`{% for %}`/`{% if %}`/…) para insertar un bloque entero.
    let en_html = labels(4, 3);
    assert!(!en_html.iter().any(|(l, _)| l == "titulo"), "{en_html:?}");
    assert!(en_html.iter().any(|(l, _)| l == "{% for %}"), "{en_html:?}");

    // La `}` huérfana del auto-close: en `{% f|}` el snippet de bloque la ELIMINA con un
    // additionalTextEdit (si no, su cierre propio la duplicaría). Sin llave huérfana, no hay
    // additionalTextEdits.
    let for_item_of = |items: &Json| items.get("items").unwrap().as_array().unwrap().iter()
        .find(|i| i.get("label").and_then(Json::as_str) == Some("{% for %}")).cloned().unwrap();
    let edit_of = |item: &Json| {
        let e = item.get("textEdit").expect("textEdit explícito (sin él cada client adivina)");
        let r = e.get("range").unwrap().serialize();
        (r, e.get("newText").and_then(Json::as_str).unwrap().to_string())
    };
    // `{% f|}`: el textEdit reemplaza la palabra parcial `f` Y la `}` huérfana del auto-close
    // (cols 3..5) con el bloque entero — sin llave duplicada, en cualquier cliente.
    let tpl2 = "{% params t: string %}\n{% f}\n";
    let items = template_completion_items(tpl2, 1, 4);
    assert!(items.serialize().contains("\"isIncomplete\":true"), "re-query por tecla");
    let (r, txt) = edit_of(&for_item_of(&items));
    assert!(r.contains("\"character\":3") && r.contains("\"character\":5"), "{r}");
    assert!(txt.starts_with("for ") && txt.ends_with("{% endfor %}"), "{txt}");
    // Sin llave huérfana: el rango cubre solo la palabra (3..4).
    let tpl3 = "{% params t: string %}\n{% f\n";
    let (r, _) = edit_of(&for_item_of(&template_completion_items(tpl3, 1, 4)));
    assert!(r.contains("\"character\":3") && r.contains("\"character\":4"), "{r}");
    // `{%f|}` (sin espacio tras el delimitador): antepone el espacio Y come la llave (2..4).
    let tpl4 = "{% params t: string %}\n{%f}\n";
    let (r, txt) = edit_of(&for_item_of(&template_completion_items(tpl4, 1, 3)));
    assert!(txt.starts_with(" for "), "{txt}");
    assert!(r.contains("\"character\":2") && r.contains("\"character\":4"), "{r}");

    // Y el enrutado: un completion sobre una URI `.ray.html` pasa por este camino.
    let uri = "file:///tmp/vista.ray.html";
    let mut docs = HashMap::new();
    docs.insert(uri.to_string(), tpl.to_string());
    let msg = json::parse(&format!(
        r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":1,"character":10}}}}}}"#
    )).unwrap();
    let items = completion_result(&msg, &docs);
    assert!(items.get("items").unwrap().as_array().unwrap().iter()
        .any(|i| i.get("label").and_then(Json::as_str) == Some("titulo")));

    // Hover: sobre `ti` de `{{ ti }}`... no hay símbolo; sobre `rows` del for (línea 3,
    // "{% for row in rows %}" → `rows` empieza en col 15) → `rows: [string]`; sobre la
    // variable de bucle usada dentro (no cubierta aquí: `f` no es `row`); y sobre HTML, nada.
    assert_eq!(template_hover_at(tpl, 2, 16), Some(("rows: [string]".into(), 14, 18)));
    assert_eq!(template_hover_at(tpl, 3, 7), Some(("total: int".into(), 6, 11))); // {% if total
    assert!(template_hover_at(tpl, 4, 4).is_none(), "about el HTML no hay hover");
    // Y el enrutado por hover_result con URI .ray.html.
    let hmsg = json::parse(&format!(
        r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":3,"character":7}}}}}}"#
    )).unwrap();
    let h = hover_result(&hmsg, &docs);
    assert!(h.serialize().contains("total: int"), "{h:?}");
}

#[test]
fn semantic_intelligence_in_templates() {
    // M55: hover con tipos REALES, completion de miembros tras `.`, ir-a-definición y
    // signature help DENTRO de las expresiones del template — todo vía el módulo generado +
    // el line map (la posición del cursor se traduce al generado y de vuelta).
    let base = std::env::temp_dir().join("ray_lsp_tpl_sem_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("std")).unwrap();
    // El generado importa `from std/template import escape_html` → resoluble bajo la base.
    std::fs::write(base.join("std/template.ray"),
        "pub fn escape_html(s: string) -> string { s }\n").unwrap();
    let uri = format!("file://{}/vista.ray.html", base.display());
    let tpl = "{% params titulo: string, rows: [string] %}\n\
               <h1>{{ titulo }}</h1>\n\
               {% for row in rows %}<li>{{ row.trim() }}</li>{% endfor %}\n";

    // Hover semántico: sobre `row` dentro de `{{ row.trim() }}` → su tipo REAL (string,
    // inferido por el checker del `for` sobre `[string]`); el rango es el del template.
    let l2 = tpl.lines().nth(2).unwrap();
    let col_row = l2.rfind("row.trim").unwrap() + 1; // dentro de `row` (ASCII: byte == char)
    let (info, start, end) = template_semantic_hover(&uri, tpl, 2, col_row).expect("hover de row");
    assert!(info.contains("string"), "{info}");
    assert_eq!((start, end), (l2.rfind("row.trim").unwrap(), l2.rfind("row.trim").unwrap() + 3));

    // Ir-a-definición: sobre `titulo` en `{{ titulo }}` → la línea del `{% params %}` del
    // PROPIO template (la declaración vive en la firma del generado, que mapea a la línea 1).
    let l1 = tpl.lines().nth(1).unwrap();
    let d = template_definition(&uri, tpl, 1, l1.find("titulo").unwrap() + 2).expect("def de titulo");
    let ser = d.serialize();
    assert!(ser.contains(&uri), "la def vuelve al template: {ser}");
    assert!(ser.contains("\"line\":0"), "línea del params: {ser}");
    assert!(ser.contains(&format!("\"character\":{}", tpl.lines().next().unwrap().find("titulo").unwrap())), "{ser}");

    // Completion de miembros: `{{ titulo. }}` ofrece los builtins de string (len/trim/…).
    let tpl2 = "{% params titulo: string %}\n<p>{{ titulo. }}</p>\n";
    let (code, map, gen_uri) = template_generated(&uri, tpl2).unwrap();
    let l = "<p>{{ titulo. }}</p>";
    let (gl, gc) = template_pos_to_generated(tpl2, &code, &map, 1, l.find('.').unwrap() + 1).expect("mapea after el punto");
    let docs = HashMap::new();
    // Como en completion_result: stub local en vez del import (member_completion es de un buffer).
    let code_sb = code.replacen("from std/template import escape_html;",
        "fn escape_html(s: string) -> string { s }", 1);
    let items = member_completion_items(Some(&gen_uri), &code_sb, gl, gc, &docs).expect("members de string");
    let labels: Vec<String> = items.as_array().unwrap().iter()
        .filter_map(|i| i.get("label").and_then(Json::as_str).map(|s| s.to_string())).collect();
    assert!(labels.iter().any(|l| l == "len"), "{labels:?}");
    assert!(labels.iter().any(|l| l == "trim"), "{labels:?}");

    // Signature help: `{{ titulo.substring( }}` muestra la firma del builtin.
    let tpl3 = "{% params titulo: string %}\n<p>{{ titulo.substring( }}</p>\n";
    let (code3, map3, gen_uri3) = template_generated(&uri, tpl3).unwrap();
    let l3 = "<p>{{ titulo.substring( }}</p>";
    let (gl3, gc3) = template_pos_to_generated(tpl3, &code3, &map3, 1, l3.find('(').unwrap() + 1).expect("mapea after el paréntesis");
    let sh = signature_help_at(&gen_uri3, &code3, gl3, gc3);
    assert!(sh.serialize().contains("substring"), "{sh:?}");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn rename_references_highlight_y_outline_en_templates() {
    // M55: el escáner de ocurrencias del template resuelve cada ident de los delimitadores a su
    // binding (param / var de for, con shadowing) y sobre él corren references / rename /
    // highlight / outline. El HTML de fuera de los delimitadores NUNCA se toca.
    let uri = "file:///tmp/list.ray.html".to_string();
    let tpl = "{% params row: string, rows: [string] %}\n\
               <p>row {{ row }}</p>\n\
               {% for row in rows %}<li>{{ row.trim() }}</li>{% endfor %}\n\
               <i>{{ row }}</i>\n";
    let occs = template_occurrences(tpl);
    // El `fila` del texto HTML (línea 2, "fila " literal) NO es ocurrencia.
    assert!(!occs.iter().any(|(l, c, _, _, _)| *l == 1 && *c == 3), "{occs:?}");
    // `trim` (miembro tras `.`) no liga.
    assert!(!occs.iter().any(|(l, c, _, _, _)| *l == 2 && tpl.lines().nth(2).unwrap().chars().skip(*c).take(4).collect::<String>() == "trim"), "{occs:?}");
    // El `fila` DENTRO del for liga a la var del for (shadowing del param).
    let l2 = tpl.lines().nth(2).unwrap();
    let col_usage = l2.rfind("row.trim").unwrap(); // ASCII: byte == char
    let usage = template_occurrence_at(&occs, 2, col_usage + 1).expect("use de row en el for");
    assert!(usage.3.starts_with("f:"), "liga al for, no al param: {usage:?}");
    // Los `fila` de FUERA del for ligan al param (línea 2 y línea 4 + su decl en la cabecera).
    let param_occs: Vec<_> = occs.iter().filter(|o| o.3 == "p:row").collect();
    assert_eq!(param_occs.len(), 3, "decl + 2 usos: {param_occs:?}");

    let mut docs = HashMap::new();
    docs.insert(uri.clone(), tpl.to_string());
    let at = |line: usize, ch: usize, extra: &str| -> Json {
        json::parse(&format!(
            r#"{{"params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{ch}}}{extra}}}}}"#
        )).unwrap()
    };
    // Rename del param `fila` → 3 ediciones (cabecera + 2 usos), sin tocar el for ni el HTML.
    let col_p = tpl.lines().nth(1).unwrap().rfind("row").unwrap();
    let w = rename_result(&at(1, col_p + 1, r#","newName":"registro""#), &docs);
    let ser = w.serialize();
    assert_eq!(ser.matches("registro").count(), 3, "{ser}");
    // Un nombre nuevo inválido → null.
    assert!(matches!(rename_result(&at(1, col_p + 1, r#","newName":"1x""#), &docs), Json::Null));
    // References sobre la var del for → decl + uso (2), no los del param.
    let refs = references_result(&at(2, col_usage + 1, ""), &docs);
    assert_eq!(refs.as_array().unwrap().len(), 2, "{}", refs.serialize());
    // Highlight: mismas 2, la decl como Write (kind 3).
    let hl = document_highlight_result(&at(2, col_usage + 1, ""), &docs);
    assert_eq!(hl.as_array().unwrap().len(), 2);
    assert!(hl.serialize().contains("\"kind\":3"), "{}", hl.serialize());
    // Outline: raíz `render` (M103: nombre fijo) con las decls como hijas (2 params + 1 var de for).
    let syms = document_symbol_result(&at(0, 0, ""), &docs);
    let ser = syms.serialize();
    assert!(ser.contains("\"name\":\"render\""), "{ser}");
    assert_eq!(ser.matches("\"kind\":13").count(), 3, "{ser}");
}

#[test]
fn serve_method_unknown_con_id_da_error() {
    let body = r#"{"jsonrpc":"2.0","id":9,"method":"textDocument/nonexistent","params":{}}"#;
    let mut input = frame(body);
    input.push_str(&frame(r#"{"jsonrpc":"2.0","method":"exit"}"#));

    let mut reader = io::Cursor::new(input.into_bytes());
    let mut output: Vec<u8> = Vec::new();
    serve(&mut reader, &mut output);
    let out = String::from_utf8(output).unwrap();
    assert!(out.contains("\"id\":9"));
    assert!(out.contains("-32601"));
}
