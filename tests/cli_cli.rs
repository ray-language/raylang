//! Pruebas del CLI de subcomandos (M39a) sobre el binario: `new`, `run`, `build`, `test`,
//! `help`, `version`, y la compatibilidad con la interfaz legada por flags.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Ejecuta el binario con `args` y `cwd`, devuelve (stdout, stderr, código).
fn ray(cwd: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN).args(args).current_dir(cwd).output().expect("lanza el binario");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Un directorio temporal único por prueba (evita choques entre tests paralelos).
fn tmp(nombre: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_cli_{nombre}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("crea el dir temporal");
    d
}

#[test]
fn new_crea_el_esqueleto_y_run_lo_ejecuta() {
    let base = tmp("new");
    // `ray new proj` crea ray.toml + src/main.ray + .gitignore.
    let (out, _err, code) = ray(&base, &["new", "proj"]);
    assert_eq!(code, 0, "new debe salir 0\n{out}");
    let proj = base.join("proj");
    assert!(proj.join("ray.toml").is_file(), "falta ray.toml");
    assert!(proj.join("src/main.ray").is_file(), "falta src/main.ray");
    assert!(proj.join(".gitignore").is_file(), "falta .gitignore");
    let manifiesto = std::fs::read_to_string(proj.join("ray.toml")).unwrap();
    assert!(manifiesto.contains("name = \"proj\""), "el manifiesto nombra el proyecto\n{manifiesto}");

    // `ray run` sin archivo usa src/main.ray (convención de proyecto).
    let (out, _err, code) = ray(&proj, &["run"]);
    assert!(out.contains("hola desde proj"), "run ejecuta el hola-mundo\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn new_falla_si_el_destino_existe() {
    let base = tmp("new_dup");
    assert_eq!(ray(&base, &["new", "dup"]).2, 0);
    let (_o, err, code) = ray(&base, &["new", "dup"]);
    assert_ne!(code, 0, "no debe sobrescribir un directorio existente");
    assert!(err.contains("ya existe"), "{err}");
}

#[test]
fn run_pasa_los_args_del_programa() {
    let base = tmp("run_args");
    std::fs::write(
        base.join("prog.ray"),
        "fn main() -> int { print(len(args())); 0 }\n",
    )
    .unwrap();
    // Los argumentos tras el archivo llegan a `args()`.
    let (out, _err, _code) = ray(&base, &["run", "prog.ray", "uno", "dos", "tres"]);
    assert!(out.contains("3"), "args() ve los 3 argumentos\n{out}");
}

#[test]
fn build_compila_ok_y_reporta_errores() {
    let base = tmp("build");
    // Programa válido: build sale 0.
    std::fs::write(base.join("ok.ray"), "fn main() -> int { 1 + 2 }\n").unwrap();
    let (out, _err, code) = ray(&base, &["build", "ok.ray"]);
    assert_eq!(code, 0, "build de un programa válido sale 0\n{out}");
    assert!(out.contains("compila"), "{out}");
    // build NO ejecuta: un programa que devolvería 42 igual sale 0 (solo compiló).
    std::fs::write(base.join("cuarenta.ray"), "fn main() -> int { 42 }\n").unwrap();
    assert_eq!(ray(&base, &["build", "cuarenta.ray"]).2, 0, "build no corre el programa");
    // Programa con error de tipos: build sale 65 y no dice 'compila'.
    std::fs::write(base.join("mal.ray"), "fn main() -> int { 1 + true }\n").unwrap();
    let (_o, err, code) = ray(&base, &["build", "mal.ray"]);
    assert_eq!(code, 65, "build de un programa con error sale 65\n{err}");
    assert!(err.contains("error de tipos"), "{err}");
}

#[test]
fn test_subcomando_corre_las_pruebas() {
    let base = tmp("test");
    std::fs::write(
        base.join("suite.ray"),
        "@test\nfn pasa() -> bool { true }\n@test\nfn falla() -> bool { false }\nfn main() -> int { 0 }\n",
    )
    .unwrap();
    let (out, _err, code) = ray(&base, &["test", "suite.ray"]);
    assert!(out.contains("pasa") && out.contains("falla"), "informa ambas pruebas\n{out}");
    assert_eq!(code, 1, "el código de salida es el número de fallos (1)");
    // Filtro por subcadena: solo la que pasa.
    let (out, _err, code) = ray(&base, &["test", "suite.ray", "pasa"]);
    assert!(out.contains("pasa") && !out.contains("FALLO"), "el filtro deja solo 'pasa'\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn help_y_version() {
    let cwd = std::env::temp_dir();
    let (out, _err, code) = ray(&cwd, &["help"]);
    assert_eq!(code, 0);
    assert!(out.contains("Uso: ray") && out.contains("new") && out.contains("build"), "{out}");
    let (out, _err, code) = ray(&cwd, &["version"]);
    assert_eq!(code, 0);
    assert!(out.contains("raylang 1."), "la versión del lenguaje\n{out}");
}

#[test]
fn compat_flags_legadas() {
    let base = tmp("legacy");
    std::fs::write(base.join("p.ray"), "fn main() -> int { 7 }\n").unwrap();
    // La interfaz previa por flags sigue funcionando (un `<archivo>` directo, y --vm).
    assert_eq!(ray(&base, &["p.ray"]).2, 7, "raylang <archivo> directo");
    assert_eq!(ray(&base, &["--vm", "p.ray"]).2, 7, "raylang --vm <archivo>");
}

// ── M39b: el manifiesto ray.toml dirige build/run/test ───────────────────────────────

/// Crea un proyecto con un `ray.toml` a medida y un archivo de entrada.
fn proyecto(nombre: &str, manifiesto: &str, entry_rel: &str, entry_src: &str) -> std::path::PathBuf {
    let raiz = tmp(nombre);
    std::fs::write(raiz.join("ray.toml"), manifiesto).unwrap();
    let entry = raiz.join(entry_rel);
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(entry, entry_src).unwrap();
    raiz
}

#[test]
fn build_y_run_usan_la_entry_del_manifiesto() {
    let raiz = proyecto(
        "manifest_entry",
        "[package]\nname = \"miapp\"\nversion = \"2.0.0\"\nentry = \"src/arranque.ray\"\n",
        "src/arranque.ray",
        "fn main() -> int { print(\"arranque\"); 5 }\n",
    );
    // build: banner con nombre+versión (a stderr) y compila la entry del manifiesto.
    let (out, err, code) = ray(&raiz, &["build"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("compilando miapp v2.0.0"), "banner del proyecto\n{err}");
    assert!(out.contains("arranque.ray") && out.contains("compila"), "{out}");
    // run: ejecuta la entry del manifiesto (sin pasar archivo).
    let (out, _err, code) = ray(&raiz, &["run"]);
    assert!(out.contains("arranque"), "{out}");
    assert_eq!(code, 5, "el exit es el int de main");
}

#[test]
fn run_sube_a_la_raiz_desde_un_subdirectorio() {
    let raiz = proyecto(
        "manifest_subdir",
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n",
        "src/main.ray",
        "fn main() -> int { print(\"raiz\"); 0 }\n",
    );
    // Ejecutar desde src/: el CLI sube buscando ray.toml (como cargo/git).
    let (out, _err, code) = ray(&raiz.join("src"), &["run"]);
    assert!(out.contains("raiz"), "encuentra el proyecto subiendo\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn dependencia_inalcanzable_falla_al_descargar() {
    // M39c-2a: una dependencia declarada se descarga en `run`/`build`; si no se puede clonar
    // (aquí, una ruta local inexistente → fallo rápido y offline), es error de compilación.
    let raiz = proyecto(
        "manifest_deps",
        "[package]\nname = \"condeps\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"git+file:///no/existe/geo@v1\"\n",
        "src/main.ray",
        "fn main() -> int { 0 }\n",
    );
    let (_out, err, code) = ray(&raiz, &["run"]);
    assert_eq!(code, 65, "una dependencia inalcanzable aborta\n{err}");
    assert!(err.contains("geo") && err.contains("clonar"), "error claro de descarga\n{err}");
}

#[test]
fn manifiesto_mal_formado_falla_claro() {
    let raiz = tmp("manifest_bad");
    std::fs::write(raiz.join("ray.toml"), "[package]\nname = sinComillas\n").unwrap();
    std::fs::create_dir_all(raiz.join("src")).unwrap();
    std::fs::write(raiz.join("src/main.ray"), "fn main() -> int { 0 }\n").unwrap();
    let (_out, err, code) = ray(&raiz, &["build"]);
    assert_eq!(code, 65, "un ray.toml mal formado es error de compilación\n{err}");
    assert!(err.contains("ray.toml:2"), "el error trae la línea\n{err}");
}

// ── M39c-1: la caché `.ray-deps/` es raíz de módulos (un paquete = una cápsula) ──────

/// Escribe un paquete `nombre` en la caché `.ray-deps/` del proyecto `raiz`, con su `mod.ray`.
fn dep(raiz: &std::path::Path, nombre: &str, mod_ray: &str) {
    let d = raiz.join(".ray-deps").join(nombre);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("mod.ray"), mod_ray).unwrap();
}

#[test]
fn dependencia_de_ray_deps_es_importable() {
    let raiz = proyecto(
        "dep_import",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"git+https://x/geo@v1\"\n",
        "src/main.ray",
        "from geo import duplicar;\nfn main() -> int { print(duplicar(21)); 0 }\n",
    );
    dep(&raiz, "geo", "pub fn duplicar(x: int) -> int { x * 2 }\n");
    // La dependencia está en la caché → el loader la encuentra y `from geo import` funciona.
    let (out, err, code) = ray(&raiz, &["run"]);
    assert!(out.contains("42"), "usa la función de la dependencia\n{out}\n{err}");
    assert_eq!(code, 0);
    // Y como está presente, NO se avisa de dependencia sin descargar.
    assert!(!err.contains("sin descargar"), "no debe avisar de una dep presente\n{err}");
}

#[test]
fn dependencia_calificada_y_capsula_protege_internos() {
    let raiz = proyecto(
        "dep_capsula",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        "src/main.ray",
        "import geo;\nfn main() -> int { print(geo.triplicar(10)); 0 }\n",
    );
    // El paquete geo es una cápsula que usa su propio submódulo interno.
    dep(
        &raiz,
        "geo",
        "from geo/interno import triple;\npub fn triplicar(x: int) -> int { triple(x) }\n",
    );
    std::fs::write(
        raiz.join(".ray-deps/geo/interno.ray"),
        "pub fn triple(x: int) -> int { x * 3 }\n",
    )
    .unwrap();
    // Acceso calificado a la cara pública del paquete.
    let (out, err, code) = ray(&raiz, &["run"]);
    assert!(out.contains("30"), "geo.triplicar via su interno\n{out}\n{err}");
    assert_eq!(code, 0);

    // La app NO puede alcanzar el submódulo interno del paquete (enforcement de cápsula, M11.6b).
    std::fs::write(
        raiz.join("src/main.ray"),
        "import geo/interno;\nfn main() -> int { print(geo.interno.triple(5)); 0 }\n",
    )
    .unwrap();
    let (_o, err, code) = ray(&raiz, &["run"]);
    assert_eq!(code, 65, "alcanzar el interno de una dependencia es error\n{err}");
    assert!(err.contains("interno a la cápsula 'geo'"), "{err}");
}

#[test]
fn lo_local_tapa_a_la_dependencia_del_mismo_nombre() {
    let raiz = proyecto(
        "dep_shadow",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        "src/main.ray",
        "from util import saludo;\nfn main() -> int { print(saludo()); 0 }\n",
    );
    // Módulo local `util` en el proyecto...
    std::fs::write(raiz.join("src/util.ray"), "pub fn saludo() -> int { 1 }\n").unwrap();
    // ...y una dependencia `util` homónima en la caché.
    dep(&raiz, "util", "pub fn saludo() -> int { 999 }\n");
    // El proyecto se busca antes que la caché: gana el módulo local.
    let (out, err, code) = ray(&raiz, &["run"]);
    assert!(out.contains("1") && !out.contains("999"), "lo local tapa a la dependencia\n{out}\n{err}");
    assert_eq!(code, 0);
}

#[test]
fn doc_genera_markdown_de_la_superficie_publica() {
    let base = tmp("doc");
    let archivo = base.join("lib.ray");
    std::fs::write(
        &archivo,
        "/// Suma dos enteros.\npub fn suma(a: int, b: int) -> int { a + b }\n\nfn interna() -> int { 0 }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["doc", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "doc debe salir 0\n{err}");
    assert!(out.contains("# lib.ray"), "encabezado con el nombre\n{out}");
    assert!(out.contains("### `fn suma(a: int, b: int) -> int`"), "firma\n{out}");
    assert!(out.contains("Suma dos enteros."), "el comentario /// se documenta\n{out}");
    assert!(!out.contains("interna"), "los ítems privados no se documentan\n{out}");
}

#[test]
fn importa_la_stdlib_del_sistema() {
    // Un programa fuera del repo (dir temporal) puede `import std/math;` — la stdlib va EMBEBIDA en el
    // binario (M40.5), así que resuelve sin que `std/` exista en disco junto al programa.
    let base = tmp("std_import");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "import std/math;\nfn main() -> int { print(math.gcd(48, 36)); print(math.is_prime(13)); 0 }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con import std/math debe salir 0\n{err}");
    assert!(out.contains("12"), "gcd(48,36)=12\n{out}");
    assert!(out.contains("true"), "is_prime(13)\n{out}");
}

#[test]
fn stdlib_text_capitaliza_e_invierte() {
    let base = tmp("std_text");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "import std/text;\nfn main() -> int { print(text.capitalize(\"hola\")); print(text.reverse(\"abc\")); print(text.count(\"aaaa\", \"aa\")); 0 }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con import std/text debe salir 0\n{err}");
    assert!(out.contains("Hola"), "capitalize\n{out}");
    assert!(out.contains("cba"), "reverse\n{out}");
    assert!(out.contains("2"), "count no solapado\n{out}");
}

#[test]
fn stdlib_sort_busca_y_deduplica() {
    let base = tmp("std_sort");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "import std/sort;\n\
         fn main() -> int {\n\
             print(sort.dedup([5, 2, 8, 2, 1, 8]));\n\
             print(sort.binary_search([1, 3, 5, 7, 9], 7));\n\
             print(sort.merge([1, 4], [2, 3, 5]));\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con import std/sort debe salir 0\n{err}");
    assert!(out.contains("[1, 2, 5, 8]"), "dedup ordena y quita repetidos\n{out}");
    assert!(out.contains("Option.Some(3)"), "binary_search halla el índice\n{out}");
    assert!(out.contains("[1, 2, 3, 4, 5]"), "merge fusiona ordenado\n{out}");
}

#[test]
fn stdlib_encoding_hex_base64_url_json() {
    // M40.7a: librerías de encoding promovidas de examples/web/ a std/ (embebidas, fuente única).
    let base = tmp("std_enc");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "import std/hex;\n\
         import std/base64;\n\
         import std/url;\n\
         import std/json;\n\
         fn main() -> int {\n\
             print(hex.hex_encode([255, 0, 171]));\n\
             print(base64.base64([104, 105]));\n\
             print(url.url_encode(\"a b&c\"));\n\
             match (json.parse(\"{\\\"n\\\": 42}\")) {\n\
                 Result.Ok(j) => { print(json.stringify(j)); },\n\
                 Result.Err(e) => { print(e); },\n\
             }\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con imports de std encoding debe salir 0\n{err}");
    assert!(out.contains("ff00ab"), "hex_encode\n{out}");
    assert!(out.contains("aGk="), "base64 de \"hi\"\n{out}");
    assert!(out.contains("a%20b%26c"), "url_encode\n{out}");
    assert!(out.contains("{\"n\":42}"), "json parse+stringify\n{out}");
}

#[test]
fn stdlib_hashing_vectores_conocidos() {
    // M40.7b: hashing promovido a std/. sha512/hmac no son hojas → sus imports se namespacaron a std/,
    // que la resolución embebida satisface (el temporal no tiene hex.ray/sha256.ray al lado).
    let base = tmp("std_hash");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "import std/sha256;\n\
         import std/sha512;\n\
         import std/hmac;\n\
         import std/sha1;\n\
         fn main() -> int {\n\
             print(sha256.sha256_hex(to_bytes(\"abc\")));\n\
             print(sha512.sha512_hex([]));\n\
             print(hmac.hmac_sha256_hex(to_bytes(\"\"), to_bytes(\"\")));\n\
             print(sha1.sha1_hex(to_bytes(\"abc\")));\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con imports de std hashing debe salir 0\n{err}");
    assert!(out.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"), "sha256(abc)\n{out}");
    assert!(out.starts_with("ba7816bf") || out.contains("cf83e1357eefb8bd"), "sha512(\"\")\n{out}");
    assert!(out.contains("b613679a0814d9ec772f95d778c35fc5ff1697c493715653c6c712144292c5ad"), "hmac_sha256(\"\",\"\")\n{out}");
    assert!(out.contains("a9993e364706816aba3e25717850c26c9cd0d89d"), "sha1(abc)\n{out}");
}

#[test]
fn stdlib_compresion_roundtrip() {
    // M40.7c: compresión promovida a std/. deflate → std/inflate (namespacado en el ejemplo).
    let base = tmp("std_comp");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "import std/inflate;\n\
         import std/deflate;\n\
         import std/huffman;\n\
         fn main() -> int {\n\
             let comp = deflate.deflate_raw(to_bytes(\"raylang raylang raylang comprime\"));\n\
             match (inflate.inflate_raw(comp)) {\n\
                 Result.Ok(back) => { match (from_utf8(back)) {\n\
                     Result.Ok(s) => { print(s); }, Result.Err(e) => { print(e); },\n\
                 } }, Result.Err(e) => { print(e); },\n\
             }\n\
             match (huffman.huffman_decode(huffman.huffman_encode([65, 65, 66, 67]))) {\n\
                 Result.Ok(d) => { print(d); }, Result.Err(e) => { print(e); },\n\
             }\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con imports de std compresión debe salir 0\n{err}");
    assert!(out.contains("raylang raylang raylang comprime"), "deflate→inflate roundtrip\n{out}");
    assert!(out.contains("[65, 65, 66, 67]"), "huffman roundtrip\n{out}");
}

#[test]
fn stdlib_texto_regex_csv_toml() {
    // M40.7d: procesamiento de texto/datos (librerías puras de examples/stdlib/, hojas).
    let base = tmp("std_txt");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "import std/regex;\n\
         import std/csv;\n\
         import std/toml;\n\
         fn main() -> int {\n\
             print(regex.find_all(\"[0-9]+\", \"a12b345c\"));\n\
             match (csv.parse_csv(\"a,b\\n1,2\")) {\n\
                 Result.Ok(rows) => { print(rows); }, Result.Err(e) => { print(e); },\n\
             }\n\
             match (toml.parse_toml(\"port = 8080\")) {\n\
                 Result.Ok(es) => { match (toml.toml_get(es, \"port\")) {\n\
                     Option.Some(v) => { print(toml.toml_show(v)); }, Option.None => { print(\"?\"); },\n\
                 } }, Result.Err(e) => { print(e); },\n\
             }\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con imports de std texto debe salir 0\n{err}");
    assert!(out.contains("[12, 345]"), "regex find_all\n{out}");
    assert!(out.contains("[[a, b], [1, 2]]"), "csv parse\n{out}");
    assert!(out.contains("8080"), "toml get\n{out}");
}

#[test]
fn stdlib_cripto_aead_y_protobuf() {
    // M40.7e: primitivas cripto + protobuf. AEAD (chacha20-poly1305) seal→open y protobuf varint.
    // aead depende de std/chacha20 + std/poly1305; se namespacaron en el ejemplo (resuelven embebidas).
    let base = tmp("std_crypto");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "import std/chacha20poly1305;\n\
         import std/protobuf;\n\
         fn main() -> int {\n\
             let key: [int] = []; var i = 0; while (i < 32) { push(key, i); i = i + 1; }\n\
             let nonce: [int] = []; var j = 0; while (j < 12) { push(nonce, 0); j = j + 1; }\n\
             let s = chacha20poly1305.aead_seal(key, nonce, [], [72, 105]);\n\
             match (chacha20poly1305.aead_open(key, nonce, [], s.ciphertext, s.tag)) {\n\
                 Option.Some(pt) => { print(pt); }, Option.None => { print(\"auth\"); },\n\
             }\n\
             let w = protobuf.writer();\n\
             protobuf.write_varint(w, 1, 150);\n\
             print(to_string(protobuf.finish(w)));\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con imports de std cripto debe salir 0\n{err}");
    assert!(out.contains("[72, 105]"), "aead seal→open roundtrip\n{out}");
    assert!(out.contains("089601"), "protobuf varint field1=150\n{out}");
}

#[test]
fn stdlib_uuid_genera_y_valida() {
    // M40.7f: uuid_v4 usa random_int (no determinista); se valida el ROUNDTRIP (is_uuid_v4 es determinista).
    let base = tmp("std_uuid");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "import std/uuid;\n\
         fn main() -> int {\n\
             print(uuid.is_uuid_v4(uuid.uuid_v4()));\n\
             print(uuid.is_uuid_v4(\"not-a-uuid\"));\n\
             print(len(uuid.uuid_v4()));\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con import std/uuid debe salir 0\n{err}");
    assert!(out.contains("true"), "is_uuid_v4(uuid_v4()) roundtrip\n{out}");
    assert!(out.contains("false"), "is_uuid_v4 rechaza basura\n{out}");
    assert!(out.contains("36"), "un uuid mide 36 chars\n{out}");
}

#[test]
fn ffi_llama_a_libm() {
    // M41.1: FFI. Un `extern "m" { … }` declara funciones de libm y se llaman como cualquier función.
    // Determinista (libm) → end-to-end por subproceso, motor de producto (VM).
    let base = tmp("ffi_libm");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "extern \"m\" {\n\
         \x20 fn sqrt(x: float) -> float;\n\
         \x20 fn pow(base: float, exp: float) -> float;\n\
         }\n\
         fn main() -> int {\n\
         \x20 if (sqrt(16.0) == 4.0 && pow(2.0, 10.0) == 1024.0) { print(\"ffi ok\"); } else { print(\"ffi mal\"); }\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con extern debe salir 0\n{err}");
    assert!(out.contains("ffi ok"), "sqrt/pow de libm por FFI\n{out}");
}

#[test]
fn ffi_marshala_strings_a_char_ptr() {
    // M41.2: un `string` de raylang se pasa como `char*` (NUL-terminado) a una función C.
    let base = tmp("ffi_str");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "extern \"c\" { fn strlen(s: string) -> int; fn atoi(s: string) -> int; }\n\
         fn main() -> int {\n\
         \x20 print(strlen(\"hola mundo\"));\n\
         \x20 print(atoi(\"42\"));\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con string FFI debe salir 0\n{err}");
    assert!(out.contains("10"), "strlen(\"hola mundo\")\n{out}");
    assert!(out.contains("42"), "atoi(\"42\")\n{out}");
}

#[test]
fn ffi_retorno_char_ptr_como_option() {
    // M41.3: un char* de retorno → Option<string> (None si NULL). strstr es determinista.
    let base = tmp("ffi_ret");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        "extern \"c\" { fn strstr(h: string, n: string) -> Option<string>; }\n\
         fn main() -> int {\n\
         \x20 match (strstr(\"hola mundo\", \"mundo\")) {\n\
         \x20   Option.Some(s) => { print(s); }, Option.None => { print(\"none\"); },\n\
         \x20 }\n\
         \x20 match (strstr(\"hola\", \"zzz\")) {\n\
         \x20   Option.Some(s) => { print(s); }, Option.None => { print(\"none\"); },\n\
         \x20 }\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con retorno char* debe salir 0\n{err}");
    assert!(out.contains("mundo"), "strstr encontró 'mundo'\n{out}");
    assert!(out.contains("none"), "strstr no encontrado → None\n{out}");
}

#[test]
fn ffi_anchura_int_y_puntero_opaco_como_u64() {
    // M41.4a: int → C int (32-bit, EOF=-1 corta el bucle); u64 → C long/size_t (64-bit); un FILE*
    // (puntero) se pasa como u64 (opaco). fopen/fgetc/fclose sobre un archivo con contenido conocido.
    let base = tmp("ffi_width");
    std::fs::write(base.join("data.txt"), "Hi!").unwrap();
    let datos = base.join("data.txt");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        format!(
            "extern \"c\" {{\n\
             \x20 fn fopen(path: string, mode: string) -> u64;\n\
             \x20 fn fgetc(stream: u64) -> int;\n\
             \x20 fn fclose(stream: u64) -> int;\n\
             \x20 fn strlen(s: string) -> u64;\n\
             }}\n\
             fn main() -> int {{\n\
             \x20 print(strlen(\"hola mundo\") as int);\n\
             \x20 let h = fopen(\"{}\", \"r\");\n\
             \x20 if (h == 0) {{ print(\"no abrió\"); return 1; }}\n\
             \x20 var n = 0;\n\
             \x20 var c = fgetc(h);\n\
             \x20 while (c >= 0) {{ n = n + 1; c = fgetc(h); }}\n\
             \x20 fclose(h);\n\
             \x20 print(n);\n\
             \x20 0\n\
             }}\n",
            datos.to_str().unwrap()
        ),
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con u64/int FFI debe salir 0\n{err}");
    assert!(out.contains("10"), "strlen size_t (u64)\n{out}");
    assert!(out.contains("3"), "fgetc leyó 3 bytes y EOF (-1) cortó el bucle\n{out}");
}

#[test]
fn ffi_ptr_opaco_y_option_ptr() {
    // M41.4b: tipo `ptr` opaco + Option<ptr> fallible. fopen(existe)→Some, fopen(no existe)→None.
    let base = tmp("ffi_ptr");
    std::fs::write(base.join("data.txt"), "Hi!").unwrap();
    let datos = base.join("data.txt");
    let archivo = base.join("main.ray");
    std::fs::write(
        &archivo,
        format!(
            "extern \"c\" {{\n\
             \x20 fn fopen(path: string, mode: string) -> Option<ptr>;\n\
             \x20 fn fgetc(stream: ptr) -> int;\n\
             \x20 fn fclose(stream: ptr) -> int;\n\
             }}\n\
             fn leer(h: ptr) -> int {{\n\
             \x20 var n = 0; var c = fgetc(h);\n\
             \x20 while (c >= 0) {{ n = n + 1; c = fgetc(h); }}\n\
             \x20 fclose(h); n\n\
             }}\n\
             fn main() -> int {{\n\
             \x20 match (fopen(\"{}\", \"r\")) {{\n\
             \x20   Option.Some(h) => {{ print(leer(h)); }}, Option.None => {{ print(\"no\"); }},\n\
             \x20 }}\n\
             \x20 match (fopen(\"{}/no_existe\", \"r\")) {{\n\
             \x20   Option.Some(h) => {{ fclose(h); print(\"abrió?!\"); }}, Option.None => {{ print(\"None ok\"); }},\n\
             \x20 }}\n\
             \x20 0\n\
             }}\n",
            datos.to_str().unwrap(),
            base.to_str().unwrap()
        ),
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", archivo.to_str().unwrap()]);
    assert_eq!(code, 0, "run con ptr/Option<ptr> debe salir 0\n{err}");
    assert!(out.contains("3"), "leyó 3 bytes por el handle ptr\n{out}");
    assert!(out.contains("None ok"), "fopen de archivo inexistente → None\n{out}");
}

#[test]
fn dependencia_por_ruta_local() {
    // M40.8a: `nombre = "path:<dir>"` consume un paquete-cápsula LOCAL sin git ni descarga (un paquete
    // adicional que no va en el binario). El paquete vive fuera del proyecto que lo importa.
    let base = tmp("pathdep");
    // El paquete-cápsula `saludo` (con mod.ray).
    std::fs::create_dir_all(base.join("pkgs/saludo")).unwrap();
    std::fs::write(
        base.join("pkgs/saludo/mod.ray"),
        "pub fn hola(n: string) -> string { \"hola, \" + n + \"!\" }\n",
    )
    .unwrap();
    // El proyecto que lo consume por ruta.
    std::fs::create_dir_all(base.join("app/src")).unwrap();
    std::fs::write(
        base.join("app/ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nsaludo = \"path:../pkgs/saludo\"\n",
    )
    .unwrap();
    std::fs::write(
        base.join("app/src/main.ray"),
        "import saludo;\nfn main() -> int { print(saludo.hola(\"mundo\")); 0 }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base.join("app"), &["run"]);
    assert_eq!(code, 0, "run con path-dep debe salir 0\n{err}");
    assert!(out.contains("hola, mundo!"), "usó la función del paquete local\n{out}");
    // La path-dep NO se descarga: no debe crear `.ray-deps`.
    assert!(!base.join("app/.ray-deps").exists(), "una path-dep no se clona");
}
