//! Cápsulas (`dir/mod.ray`) como frontera de módulo: la ronda de rayhotel sobre 1.27.111
//! (#481–#490). Un monolito modular las usa para que lo interno de un módulo no se alcance desde
//! fuera; estos tests fijan que la frontera cierra (UFCS, imports por otra ruta), que las
//! herramientas la entienden (`ray check`, `ray test`, `ray doc`, el LSP) y que un paquete con
//! cápsulas se puede comprobar y consumir.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_ray");

/// Un directorio de trabajo limpio con los archivos dados (rutas relativas).
fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let base = std::env::temp_dir().join(format!("ray_capsules_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for (path, text) in files {
        let file = base.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    base
}

fn ray(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

const APP_TOML: &str = "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n";
const MODEL: &str = "pub struct Invoice { id: int, }\n\
/// Builds an invoice.\n\
pub fn make(id: int) -> Invoice { Invoice { id: id } }\n\
pub fn secret_wipe(i: Invoice) { i.id = -1; }\n\
@test\nfn inner_works() { assert_eq(make(2).id, 2); }\n";
const FACADE: &str = "pub from shop/model import Invoice, make;\npub fn area(i: Invoice) -> int { i.id * 2 }\n";

fn shop_app(name: &str, main: &str) -> PathBuf {
    tree(name, &[("ray.toml", APP_TOML), ("src/main.ray", main), ("src/shop/mod.ray", FACADE), ("src/shop/model.ray", MODEL)])
}

/// #481: desde fuera de la cápsula, una llamada como método alcanza lo que la FACHADA exporta —
/// lo que declara ella misma incluido — y no los `pub` del módulo interno que declara el tipo.
#[test]
fn method_calls_from_outside_see_the_facade_not_the_internals() {
    let leak = shop_app("ufcs_leak", "import shop;\nfn main() {\n    let i = shop.make(3);\n    i.secret_wipe();\n    print(i.id);\n}\n");
    let (out, err, code) = ray(&leak, &["run"]);
    assert_ne!(code, 0, "{out}{err}");
    assert!(err.contains("no field or function 'secret_wipe' applicable to shop::model::Invoice"), "{err}");

    let contract = shop_app("ufcs_contract", "import shop;\nfn main() {\n    print(shop.make(4).area());\n}\n");
    let (out, err, code) = ray(&contract, &["run"]);
    assert_eq!((out.as_str(), code), ("8\n", 0), "{err}");
    let _ = std::fs::remove_dir_all(leak);
    let _ = std::fs::remove_dir_all(contract);
}

/// Dentro de la cápsula nada cambia: un interno llama como método a lo `pub` de su hermano.
#[test]
fn inside_the_capsule_internals_still_reach_each_other() {
    let d = tree(
        "inside",
        &[
            ("ray.toml", APP_TOML),
            ("src/main.ray", "import shop;\nfn main() {\n    print(shop.total(3));\n}\n"),
            ("src/shop/mod.ray", "import shop/billing;\npub fn total(id: int) -> int { billing.wiped(id) }\n"),
            ("src/shop/model.ray", MODEL),
            ("src/shop/billing.ray", "import shop/model;\npub fn wiped(id: int) -> int {\n    let i = model.make(id);\n    i.secret_wipe();\n    i.id\n}\n"),
        ],
    );
    let (out, err, code) = ray(&d, &["run"]);
    assert_eq!((out.as_str(), code), ("-1\n", 0), "{err}");
    let _ = std::fs::remove_dir_all(d);
}

/// #482: el módulo interno nombrado por OTRA ruta (`src/…`, que resuelve porque el directorio del
/// proyecto también es una raíz) es el mismo archivo: la regla va sobre la ruta.
#[test]
fn an_internal_module_cannot_be_imported_by_another_path() {
    let d = shop_app("detour", "import src/shop/model;\nfn main() {\n    print(model.make(3).id);\n}\n");
    let (out, err, code) = ray(&d, &["run"]);
    assert_ne!(code, 0, "{out}{err}");
    assert!(err.contains("module 'src/shop/model' is internal to capsule 'shop'; import it with 'import shop;'"), "{err}");
    let _ = std::fs::remove_dir_all(d);
}

/// #483: `ray check` sobre la fachada o sobre un interno los comprueba con su identidad real.
#[test]
fn ray_check_accepts_a_facade_and_an_internal_module_as_files() {
    let d = shop_app("check_files", "import shop;\nfn main() {\n    print(shop.make(4).area());\n}\n");
    for file in ["src/shop/mod.ray", "src/shop/model.ray"] {
        let (out, err, code) = ray(&d, &["check", file]);
        assert_eq!(code, 0, "{file}: {out}{err}");
    }
    let _ = std::fs::remove_dir_all(d);
}

/// #488: las pruebas de caja blanca viven en un `*_test.ray` dentro de la cápsula; `ray test` lo
/// descubre aunque nadie lo importe.
#[test]
fn ray_test_discovers_white_box_tests_inside_a_capsule() {
    let d = shop_app("whitebox", "import shop;\nfn main() {\n    print(shop.make(4).area());\n}\n");
    std::fs::write(
        d.join("src/shop/model_test.ray"),
        "import shop/model;\n@test\nfn whitebox_sees_the_internals() {\n    let i = model.make(1);\n    model.secret_wipe(i);\n    assert_eq(i.id, -1);\n}\n",
    )
    .unwrap();
    let (out, err, code) = ray(&d, &["test"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("ok    whitebox_sees_the_internals"), "{out}");
    assert!(out.contains("inner_works"), "{out}");
    assert!(out.contains("result: 2 test(s), all passed"), "{out}");
    let (out, err, code) = ray(&d, &["test", "src/shop/model_test.ray"]);
    assert_eq!(code, 0, "{out}{err}");
    let _ = std::fs::remove_dir_all(d);
}

const LIB_FILES: &[(&str, &str)] = &[
    ("lib3/ray.toml", "[package]\nname = \"kit\"\nversion = \"0.1.0\"\n"),
    ("lib3/kernel/inner.ray", "/// Doubles a number.\npub fn twice(n: int) -> int { n * 2 }\n@test\nfn inner_twice() { assert_eq(twice(2), 4); }\n"),
    ("lib3/kernel/mod.ray", "pub from kit/kernel/inner import twice;\npub fn enter() -> int { twice(21) }\n"),
    ("lib3/b.ray", "import kit/kernel;\npub fn answer() -> int { kernel.enter() }\n"),
    ("use/ray.toml", "[package]\nname = \"use\"\nversion = \"0.1.0\"\n\n[dependencies]\nk = \"path:../lib3\"\n"),
    ("use/src/main.ray", "import k/b;\nimport k/kernel;\nfn main() {\n    print(b.answer());\n    print(kernel.enter());\n}\n"),
];

/// #483: un paquete-librería (sin entry) con cápsulas pasa `ray check` y `ray test`.
#[test]
fn a_library_package_with_capsules_passes_check_and_test() {
    let d = tree("library", LIB_FILES);
    let lib = d.join("lib3");
    let (out, err, code) = ray(&lib, &["check"]);
    assert_eq!(code, 0, "{out}{err}");
    let (out, err, code) = ray(&lib, &["test"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("ok    inner_twice"), "{out}");
    assert!(!out.contains("failed to compile"), "{out}");
    let _ = std::fs::remove_dir_all(d);
}

/// #490: los módulos del paquete se importan entre sí por su `[package] name` (`kit/…`) aunque el
/// consumidor lo declare con otra clave (`k`).
#[test]
fn a_package_imports_itself_by_name_under_any_dependency_key() {
    let d = tree("own_name", LIB_FILES);
    let (out, err, code) = ray(&d.join("use"), &["run"]);
    assert_eq!((out.as_str(), code), ("42\n42\n", 0), "{err}");
    let _ = std::fs::remove_dir_all(d);
}

/// #485: `ray doc` de una fachada muestra su contrato — lo re-exportado con `pub from` incluido —,
/// la titula con el nombre de la cápsula, y resuelve la cápsula y sus símbolos por su nombre.
#[test]
fn ray_doc_shows_the_contract_of_a_capsule() {
    let d = shop_app("doc", "import shop;\nfn main() {\n    print(shop.make(4).area());\n}\n");
    let (out, err, code) = ray(&d, &["doc", "src/shop/mod.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("# shop\n"), "{out}");
    assert!(out.contains("## Functions\n\n### `fn area(i: Invoice) -> int`"), "{out}");
    assert!(out.contains("## Re-exports"), "{out}");
    assert!(out.contains("### `fn make(id: int) -> Invoice`\n\nFrom `shop/model`.\n\nBuilds an invoice."), "{out}");
    assert!(out.contains("### `struct Invoice { id: int }`"), "{out}");

    let (out, err, code) = ray(&d, &["doc", "shop"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("make(id: int) -> Invoice  [re-exported from shop/model]"), "{out}");
    assert!(out.contains("area(i: Invoice) -> int"), "{out}");

    let (out, err, code) = ray(&d, &["doc", "shop.make"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("make(id: int) -> Invoice") && out.contains("Builds an invoice."), "{out}");
    let _ = std::fs::remove_dir_all(d);

    // Una dependencia `path:` con otra clave: por la clave y por el símbolo.
    let d = tree("doc_dep", LIB_FILES);
    let (out, err, code) = ray(&d.join("use"), &["doc", "k/kernel"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("twice(n: int) -> int  [re-exported from kit/kernel/inner]"), "{out}");
    let (out, err, code) = ray(&d.join("use"), &["doc", "kernel.twice"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("twice(n: int) -> int") && out.contains("Doubles a number."), "{out}");
    let _ = std::fs::remove_dir_all(d);
}

/// Un módulo que define un tipo con nombre de tipo builtin (`struct Task`) compila dentro del
/// proyecto (va namespacado); comprobado como archivo —lo que hace el editor al abrirlo— daba
/// «'Task' is a builtin type…». Visto en rayhotel al barrer sus archivos por el LSP.
#[test]
fn a_module_with_a_builtin_named_type_checks_as_a_file() {
    let d = tree(
        "builtin_type",
        &[
            ("ray.toml", APP_TOML),
            ("src/main.ray", "import jobs/tasks;\nfn main() {\n    print(tasks.first().id);\n}\n"),
            ("src/jobs/tasks.ray", "pub struct Task { id: int, }\npub fn first() -> Task { Task { id: 7 } }\npub fn all() -> [Task] { [first()] }\n"),
        ],
    );
    let (out, err, code) = ray(&d, &["run"]);
    assert_eq!((out.as_str(), code), ("7\n", 0), "{err}");
    let (out, err, code) = ray(&d, &["check", "src/jobs/tasks.ray"]);
    assert_eq!(code, 0, "{out}{err}");
    let out = lsp_open(&d.join("src/jobs/tasks.ray"));
    assert!(out.contains("\"diagnostics\":[]"), "{out}");
    let _ = std::fs::remove_dir_all(d);
}

// ---------------------------------------------------------------------------------------------
// LSP
// ---------------------------------------------------------------------------------------------

fn file_uri(path: &Path) -> String {
    let s = path.display().to_string().replace('\\', "/");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

/// Abre `file` en el servidor LSP con el texto que tiene en disco y devuelve todo su stdout.
fn lsp_open(file: &Path) -> String {
    let text = std::fs::read_to_string(file).unwrap().replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    let open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}","text":"{text}"}}}}}}"#,
        file_uri(file)
    );
    let input = frame(&open) + &frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    let mut child = Command::new(BIN).arg("--lsp").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("lanza el LSP");
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned()
}

/// El LSP diagnostica los archivos de una cápsula con su identidad real: ni la fachada de una
/// librería (sin `main.ray` por encima) es ajena a su cápsula, ni un interno pierde el alcance
/// de sus hermanos. Lo de fuera sí recibe el error.
#[test]
fn the_lsp_checks_capsule_files_with_their_real_identity() {
    let d = tree("lsp_lib", LIB_FILES);
    for file in ["lib3/kernel/mod.ray", "lib3/kernel/inner.ray", "lib3/b.ray"] {
        let out = lsp_open(&d.join(file));
        assert!(out.contains("\"diagnostics\":[]"), "{file} debe abrir limpio\n{out}");
    }
    let _ = std::fs::remove_dir_all(d);

    let d = tree(
        "lsp_app",
        &[
            ("ray.toml", APP_TOML),
            ("src/main.ray", "import shop;\nfn main() {\n    let i = shop.make(3);\n    i.secret_wipe();\n}\n"),
            ("src/shop/mod.ray", "import shop/billing;\npub from shop/model import Invoice, make;\npub fn total(id: int) -> int { billing.wiped(id) }\n"),
            ("src/shop/model.ray", MODEL),
            ("src/shop/billing.ray", "import shop/model;\npub fn wiped(id: int) -> int {\n    let i = model.make(id);\n    i.secret_wipe();\n    i.id\n}\n"),
        ],
    );
    for file in ["src/shop/mod.ray", "src/shop/billing.ray", "src/shop/model.ray"] {
        let out = lsp_open(&d.join(file));
        assert!(out.contains("\"diagnostics\":[]"), "{file} debe abrir limpio\n{out}");
    }
    let out = lsp_open(&d.join("src/main.ray"));
    assert!(out.contains("no field or function 'secret_wipe'"), "main.ray está fuera de la cápsula\n{out}");
    let _ = std::fs::remove_dir_all(d);
}
