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

/// rayhotel #481, el resto: un trait interno que la fachada no exporta tampoco da métodos fuera
/// de la cápsula (antes `shop.make(4).peek()` compilaba). Uno que la fachada sí exporta se llama
/// como siempre, y dentro de la cápsula el interno sigue valiendo.
#[test]
fn methods_of_a_trait_the_facade_does_not_export_stay_inside() {
    const TRAITS: &str = "pub struct Invoice { id: int, }\n\
pub fn make(id: int) -> Invoice { Invoice { id: id } }\n\
pub trait Hidden { fn peek(self) -> int; }\n\
impl Hidden for Invoice { fn peek(self) -> int { self.id } }\n\
pub trait Shown { fn total(self) -> int; }\n\
impl Shown for Invoice { fn total(self) -> int { self.id * 10 } }\n\
pub fn inside(i: Invoice) -> int { i.peek() }\n";
    let app = |name: &str, main: &str| {
        tree(name, &[
            ("ray.toml", APP_TOML),
            ("src/main.ray", main),
            ("src/shop/mod.ray", "pub from shop/model import Invoice, make, Shown, inside;\n"),
            ("src/shop/model.ray", TRAITS),
        ])
    };
    let ok = app("trait_contract", "import shop;\nfn main() {\n    let i = shop.make(4);\n    print(i.total());\n    print(shop.inside(i));\n}\n");
    for engine in [&["run"][..], &["--interp", "src/main.ray"][..]] {
        let (out, err, code) = ray(&ok, engine);
        assert_eq!((out.as_str(), code), ("40\n4\n", 0), "{engine:?}: {err}");
    }
    let leak = app("trait_leak", "import shop;\nfn main() {\n    print(shop.make(4).peek());\n}\n");
    let (out, err, code) = ray(&leak, &["run"]);
    assert_ne!(code, 0, "{out}{err}");
    assert!(err.contains("no field or function 'peek' applicable to shop::model::Invoice"), "{err}");
    let _ = std::fs::remove_dir_all(ok);
    let _ = std::fs::remove_dir_all(leak);

    // Un `to_string` de un trait interno no tapa fuera el `Show` del tipo.
    let shown = tree("trait_show", &[
        ("ray.toml", APP_TOML),
        ("src/main.ray", "import shop;\nfn main() {\n    print(shop.make(4).to_string());\n    print(shop.inside(shop.make(4)));\n}\n"),
        ("src/shop/mod.ray", "pub from shop/model import Invoice, make, inside;\n"),
        (
            "src/shop/model.ray",
            "pub struct Invoice { id: int, }\npub fn make(id: int) -> Invoice { Invoice { id: id } }\n\
impl Show for Invoice { fn show(self) -> string { \"INV-\" + to_string(self.id) } }\n\
pub trait Raw { fn to_string(self) -> string; }\nimpl Raw for Invoice { fn to_string(self) -> string { \"raw\" } }\n\
pub fn inside(i: Invoice) -> string { i.to_string() }\n",
        ),
    ]);
    let (out, err, code) = ray(&shown, &["run"]);
    assert_eq!((out.as_str(), code), ("INV-4\nraw\n", 0), "{err}");
    let _ = std::fs::remove_dir_all(shown);

    // El cuerpo de un método por defecto del trait interno es código de la cápsula, cargue quien
    // cargue después: su clon no tiene línea propia y antes se juzgaba por el último módulo.
    let default_body = tree("trait_default", &[
        ("ray.toml", APP_TOML),
        ("src/main.ray", "import zeta;\nimport shop;\nfn main() {\n    print(shop.inside(shop.make(4)));\n    print(zeta.far());\n}\n"),
        ("src/zeta.ray", "import omega/deep/far;\npub fn far() -> int { far.away() }\n"),
        ("src/omega/deep/far.ray", "pub fn away() -> int { 7 }\n"),
        ("src/shop/mod.ray", "pub from shop/model import Invoice, make, inside;\n"),
        (
            "src/shop/model.ray",
            "pub struct Invoice { id: int, }\npub fn make(id: int) -> Invoice { Invoice { id: id } }\n\
pub fn helper(i: Invoice) -> int { i.id }\n\
pub trait Hidden {\n    fn peek(self) -> int;\n    fn twice(self) -> int { self.peek() * 2 }\n}\n\
impl Hidden for Invoice { fn peek(self) -> int { self.helper() } }\n\
pub fn inside(i: Invoice) -> int { i.twice() }\n",
        ),
    ]);
    let (out, err, code) = ray(&default_body, &["run"]);
    assert_eq!((out.as_str(), code), ("8\n7\n", 0), "{err}");
    let _ = std::fs::remove_dir_all(default_body);

    // Ni un `show` de un trait interno pasa por `Show` fuera: las tres formas dan lo mismo.
    let raw_show = tree("trait_raw_show", &[
        ("ray.toml", APP_TOML),
        ("src/main.ray", "import shop;\nfn main() {\n    let i = shop.make(4);\n    print(i.to_string());\n    print(to_string(i));\n    print(\"${i}\");\n    print(shop.inside(i));\n}\n"),
        ("src/shop/mod.ray", "pub from shop/model import Invoice, make, inside;\n"),
        (
            "src/shop/model.ray",
            "pub struct Invoice { id: int, }\npub fn make(id: int) -> Invoice { Invoice { id: id } }\n\
pub trait Raw { fn show(self) -> string; }\nimpl Raw for Invoice { fn show(self) -> string { \"raw\" } }\n\
pub fn inside(i: Invoice) -> string { to_string(i) }\n",
        ),
    ]);
    let (out, err, code) = ray(&raw_show, &["run"]);
    let plain = "shop::model::Invoice { id: 4 }";
    assert_eq!((out.as_str(), code), (format!("{plain}\n{plain}\n{plain}\nraw\n").as_str(), 0), "{err}");
    let _ = std::fs::remove_dir_all(raw_show);
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

/// rayhotel #485, el resto: la firma de lo re-exportado nombra los tipos como el CONTRATO, no
/// como el módulo interno (`model.Booking` no se puede escribir fuera de la cápsula), y un trait
/// re-exportado lleva sus métodos. Lo que la fachada no exporta se deja como está.
#[test]
fn ray_doc_names_types_as_the_contract_does() {
    let d = tree(
        "doc_names",
        &[
            ("ray.toml", APP_TOML),
            ("src/main.ray", "import booking;\nfn main() { print(1); }\n"),
            (
                "src/booking/model.ray",
                "/// A booking.\npub struct Booking { id: int, }\npub struct Draft { id: int, }\n\
/// Things with a price.\npub trait Priced {\n    fn price(self) -> int;\n    fn same(self, other: Booking) -> bool;\n}\n\
impl Priced for Booking {\n    fn price(self) -> int { self.id }\n    fn same(self, other: Booking) -> bool { self.id == other.id }\n}\n",
            ),
            (
                "src/booking/service.ray",
                "import booking/model;\n\n/// Lists the bookings.\n\
pub fn bookings(limit: int) -> Result<[model.Booking], string> { Result.Ok([model.Booking { id: limit }]) }\n\
pub fn draft(b: model.Booking) -> model.Draft { model.Draft { id: b.id } }\n",
            ),
            ("src/booking/mod.ray", "pub from booking/model import Booking as Reservation, Priced;\npub from booking/service import bookings, draft;\n"),
        ],
    );
    let (out, err, code) = ray(&d, &["doc", "src/booking/mod.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("### `fn bookings(limit: int) -> Result<[Reservation], string>`"), "{out}");
    assert!(out.contains("### `fn draft(b: Reservation) -> model.Draft`"), "lo no exportado se queda:\n{out}");
    assert!(out.contains("### `trait Priced`\n\nFrom `booking/model`.\n\nThings with a price.\n\n- `fn price(self) -> int`\n- `fn same(self, other: Reservation) -> bool`\n"), "{out}");
    assert!(out.contains("### `struct Booking { id: int }`\n\nExported as `Reservation`."), "{out}");

    let (out, err, code) = ray(&d, &["doc", "booking"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("bookings(limit: int) -> Result<[Reservation], string>  [re-exported from booking/service]"), "{out}");
    let (out, err, code) = ray(&d, &["doc", "booking.bookings"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("bookings(limit: int) -> Result<[Reservation], string>") && out.contains("Lists the bookings."), "{out}");
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

const WALLET_CORE: &str = "from std/json import ToJson;\n\n\
@derive(Eq, Show, ToJson)\n\
pub struct Account { id: int, cents: int, }\n\
pub struct Hidden { n: int, }\n\
pub enum State { Open, Closed, }\n\
pub fn open(id: int) -> Account { Account { id: id, cents: 0 } }\n\
pub fn deposit(a: Account, n: int) { a.cents = a.cents + n; }\n\
pub fn balance(a: Account) -> int { a.cents }\n\
pub fn leak() -> Hidden { Hidden { n: 1 } }\n";
const WALLET_FACADE: &str = "import wallet/core;\n\
pub from wallet/core import opaque Account, open, deposit, balance, leak;\n\
pub type St = core.State;\n\
pub fn fresh() -> Account {\n    let a = core.open(9);\n    a.cents = 5;\n    a\n}\n";

fn wallet_app(name: &str, main: &str) -> PathBuf {
    tree(name, &[("ray.toml", APP_TOML), ("src/main.ray", main), ("src/wallet/mod.ray", WALLET_FACADE), ("src/wallet/core.ray", WALLET_CORE)])
}

/// #486: `pub from … import opaque T;` — fuera de la cápsula el tipo se nombra, se pasa, se
/// compara y se serializa (lo que su módulo derivó), y sus funciones se llaman como métodos.
/// Dentro sigue transparente: la fachada y el `@derive` tocan sus campos.
#[test]
fn an_opaque_type_is_named_passed_and_used_through_the_contract() {
    let d = wallet_app(
        "opaque_ok",
        "import wallet;\nfn keep(a: wallet.Account) -> wallet.Account { a }\nfn main() {\n    let a = keep(wallet.fresh());\n    a.deposit(7);\n    print(a.balance());\n    print(a == a);\n    print(a.to_json());\n}\n",
    );
    for engine in [&["run"][..], &["run", "--interp"][..]] {
        let (out, err, code) = ray(&d, engine);
        assert_eq!((out.as_str(), code), ("12\ntrue\n{\"id\":9,\"cents\":12}\n", 0), "{engine:?}: {err}");
    }
    let _ = std::fs::remove_dir_all(d);
}

/// #486: fuera de la cápsula no se construye, no se leen ni escriben sus campos y no se
/// desestructura.
#[test]
fn an_opaque_type_cannot_be_built_read_written_or_destructured_outside() {
    let cases = [
        ("build", "import wallet;\nfn main() {\n    let a = wallet.Account { id: -1, cents: 999 };\n    print(a.balance());\n}\n", "'Account' is opaque outside the capsule 'wallet': it cannot be constructed here"),
        ("read", "import wallet;\nfn main() {\n    print(wallet.fresh().cents);\n}\n", "'Account' is opaque outside the capsule 'wallet': its fields are not accessible here (field 'cents')"),
        ("write", "import wallet;\nfn main() {\n    let a = wallet.fresh();\n    a.cents = 0;\n}\n", "its fields are not accessible here (field 'cents')"),
        (
            "destructure",
            "import wallet;\nfrom wallet import Account;\nfn main() {\n    match (wallet.fresh()) {\n        Account { id, cents } => print(id + cents),\n    }\n}\n",
            "'Account' is opaque outside the capsule 'wallet': it cannot be destructured here",
        ),
    ];
    for (name, main, want) in cases {
        let d = wallet_app(&format!("opaque_{name}"), main);
        let (out, err, code) = ray(&d, &["run"]);
        assert_ne!(code, 0, "{name}: {out}{err}");
        assert!(err.contains(want), "{name}: {err}");
        let _ = std::fs::remove_dir_all(d);
    }
}

/// #486: `opaque` es solo para los reexports de una fachada, y solo de un struct.
#[test]
fn opaque_is_only_for_structs_re_exported_by_a_facade() {
    let d = wallet_app("opaque_misuse", "from wallet import opaque Account;\nfn main() {\n    print(1);\n}\n");
    let (_, err, code) = ray(&d, &["run"]);
    assert_ne!(code, 0);
    assert!(err.contains("'opaque' is only for the re-exports of a capsule facade"), "{err}");
    std::fs::write(d.join("src/wallet/mod.ray"), "pub from wallet/core import opaque State, open;\n").unwrap();
    std::fs::write(d.join("src/main.ray"), "import wallet;\nfn main() {\n    print(1);\n}\n").unwrap();
    let (_, err, code) = ray(&d, &["run"]);
    assert_ne!(code, 0);
    assert!(err.contains("'opaque State': only a struct can be re-exported as opaque"), "{err}");
    let _ = std::fs::remove_dir_all(d);
}

/// `ray fmt` conserva `opaque` (es parte del contrato de la fachada).
#[test]
fn ray_fmt_keeps_opaque() {
    let d = wallet_app("opaque_fmt", "import wallet;\nfn main() {\n    print(1);\n}\n");
    let (out, err, code) = ray(&d, &["fmt", "src/wallet/mod.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("pub from wallet/core import opaque Account, open, deposit, balance, leak;"), "{out}");
    let _ = std::fs::remove_dir_all(d);
}

/// #487: una función exportada que muestra un tipo que la cápsula no exporta recibe un aviso, y
/// un alias de fachada usado para construir o casar dice que hay que reexportar el tipo.
#[test]
fn a_facade_showing_types_it_does_not_export_gets_a_warning_and_alias_hints() {
    let d = wallet_app("facade_warning", "import wallet;\nfn main() {\n    print(wallet.fresh().balance());\n}\n");
    let (out, err, code) = ray(&d, &["check"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        err.contains("warning at 10:5: 'leak' is exported by the capsule 'wallet' and shows the type 'Hidden', which the capsule does not export"),
        "{err}"
    );
    // `Account` es opaco pero SÍ está exportado: `fresh`, `open`… no avisan.
    assert_eq!(err.matches("warning at").count(), 1, "{err}");
    let _ = std::fs::remove_dir_all(d);

    let d = wallet_app("alias_hint", "import wallet;\nfn main() {\n    let s: wallet.St = wallet.St.Open;\n    print(1);\n}\n");
    let (_, err, code) = ray(&d, &["run"]);
    assert_ne!(code, 0);
    assert!(err.contains("a `pub type` alias only names a type in signatures — re-export the type itself with `pub from … import St;`"), "{err}");
    let _ = std::fs::remove_dir_all(d);
}

/// Revisión previa a 1.27.113: la frontera de un tipo opaco la marca DÓNDE está escrito el
/// código, no de quién es el método. Un `impl` para el tipo opaco escrito fuera de la cápsula no
/// toca sus campos; un `impl` para un tipo ajeno escrito DENTRO sí construye el opaco.
#[test]
fn the_opaque_boundary_follows_where_an_impl_is_written() {
    let outside = wallet_app(
        "opaque_impl_outside",
        "import wallet;\ntrait Peek { fn peek(self) -> int; }\nimpl Peek for wallet.Account {\n    fn peek(self) -> int {\n        self.cents = 99;\n        self.cents\n    }\n}\nfn main() {\n    print(wallet.fresh().peek());\n}\n",
    );
    let (out, err, code) = ray(&outside, &["run"]);
    assert_ne!(code, 0, "{out}{err}");
    assert!(err.contains("'Account' is opaque outside the capsule 'wallet': its fields are not accessible here"), "{err}");
    let _ = std::fs::remove_dir_all(outside);

    let inside = tree(
        "opaque_impl_inside",
        &[
            ("ray.toml", APP_TOML),
            ("src/main.ray", "import wallet;\nimport money;\nfn main() {\n    print(wallet.from_money(money.Money { amount: 7 }).balance());\n}\n"),
            ("src/money.ray", "pub struct Money { amount: int, }\n"),
            ("src/wallet/core.ray", "import money;\npub struct Account { id: int, cents: int, }\npub trait Wrap { fn wrap(self) -> Account; }\nimpl Wrap for money.Money {\n    fn wrap(self) -> Account { Account { id: 1, cents: self.amount } }\n}\npub fn from_money(m: money.Money) -> Account { m.wrap() }\npub fn balance(a: Account) -> int { a.cents }\n"),
            ("src/wallet/mod.ray", "pub from wallet/core import opaque Account, from_money, balance;\n"),
        ],
    );
    let (out, err, code) = ray(&inside, &["run"]);
    assert_eq!((out.as_str(), code), ("7\n", 0), "{err}");
    let _ = std::fs::remove_dir_all(inside);
}

/// Revisión previa a 1.27.113: un alias `pub type` de la fachada deja nombrar el tipo, así que
/// no hay aviso; y `ray doc` muestra un reexport opaco sin sus campos.
#[test]
fn an_aliased_type_is_nameable_and_an_opaque_one_is_documented_without_its_shape() {
    let d = tree(
        "alias_names",
        &[
            ("ray.toml", APP_TOML),
            ("src/main.ray", "import shop;\nfn keep(i: shop.Bill) -> shop.Bill { i }\nfn main() {\n    print(shop.amount(keep(shop.make(3))));\n}\n"),
            ("src/shop/model.ray", "pub struct Invoice { total: int, }\npub fn make(n: int) -> Invoice { Invoice { total: n } }\npub fn amount(i: Invoice) -> int { i.total }\n"),
            ("src/shop/mod.ray", "import shop/model;\npub from shop/model import make, amount;\npub type Bill = model.Invoice;\n"),
        ],
    );
    let (out, err, code) = ray(&d, &["run"]);
    assert_eq!((out.as_str(), code), ("3\n", 0), "{err}");
    assert!(!err.contains("warning at"), "un tipo con alias en la fachada se puede nombrar: {err}");
    let _ = std::fs::remove_dir_all(d);

    let d = wallet_app("opaque_doc", "import wallet;\nfn main() {\n    print(1);\n}\n");
    let (out, err, code) = ray(&d, &["doc", "src/wallet/mod.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("### `struct Account (opaque)`"), "{out}");
    assert!(!out.contains("cents: int"), "la forma de un tipo opaco no es parte del contrato\n{out}");
    let (out, err, code) = ray(&d, &["doc", "wallet"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("struct Account (opaque"), "{out}");
    // Y por el símbolo, que es adonde manda el listado («Use ray_doc "wallet.<name>"»).
    let (out, err, code) = ray(&d, &["doc", "wallet.Account"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("struct Account (opaque)") && !out.contains("cents"), "{out}");
    let _ = std::fs::remove_dir_all(d);

    // Un alias de un tipo COMPUESTO deja nombrar el compuesto, no el tipo de dentro: sigue el aviso.
    let d = tree(
        "alias_composite",
        &[
            ("ray.toml", APP_TOML),
            ("src/main.ray", "import shop;\nfn main() {\n    print(shop.amount(shop.make(3)));\n}\n"),
            ("src/shop/model.ray", "pub struct Invoice { total: int, }\npub fn make(n: int) -> Invoice { Invoice { total: n } }\npub fn amount(i: Invoice) -> int { i.total }\n"),
            ("src/shop/mod.ray", "import shop/model;\npub from shop/model import make, amount;\npub type Bills = [model.Invoice];\n"),
        ],
    );
    let (_, err, code) = ray(&d, &["check"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("shows the type 'Invoice', which the capsule does not export"), "{err}");
    let _ = std::fs::remove_dir_all(d);
}

/// Revisión previa a 1.27.113: `ray doc <dir>` con la cápsula A LA VISTA desde el directorio
/// actual (un paquete-librería: `kernel/` está en la raíz) es la cápsula, no «un directorio».
#[test]
fn ray_doc_takes_a_visible_directory_as_its_capsule() {
    let d = tree("doc_dir", LIB_FILES);
    let (out, err, code) = ray(&d.join("lib3"), &["doc", "kernel"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("enter() -> int") && out.contains("twice(n: int) -> int  [re-exported from kit/kernel/inner]"), "{out}");
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

/// rayhotel #501: en una biblioteca (sin entrada) cada archivo es una suite. La primera traía
/// también las pruebas de los módulos que importa, que volvían a correr en sus propias suites: una
/// cadena de tres módulos con una prueba cada uno daba 5. Cada `@test` corre una vez, en la suite
/// de su archivo. Pidiendo UN archivo, sus importados siguen entrando (no son otra suite).
#[test]
fn ray_test_runs_each_test_of_a_library_once() {
    let d = tree(
        "testonce",
        &[
            ("ray.toml", "[package]\nname = \"kit\"\nversion = \"0.1.0\"\n"),
            ("a/lex.ray", "pub fn lex() -> int { 1 }\n\n@test\nfn t_lex() { assert_eq(lex(), 1); }\n"),
            ("a/scan.ray", "import kit/a/lex;\n\npub fn scan() -> int { lex.lex() + 1 }\n\n@test\nfn t_scan() { assert_eq(scan(), 2); }\n"),
            ("a/contract.ray", "import kit/a/scan;\n\npub fn contract() -> int { scan.scan() + 1 }\n\n@test\nfn t_contract() { assert_eq(contract(), 3); }\n"),
        ],
    );
    let (out, err, code) = ray(&d, &["test"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("running 3 test(s)") && out.contains("result: 3 test(s), all passed"), "{out}");
    for name in ["t_contract", "t_scan", "t_lex"] {
        assert_eq!(out.matches(name).count(), 1, "{name} corre una vez:\n{out}");
    }
    let (out, err, code) = ray(&d, &["test", "a/contract.ray"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("running 3 test(s)") && out.contains("kit.a.scan.t_scan") && out.contains("kit.a.lex.t_lex"), "{out}");
    // El filtro calificado con el módulo sigue encontrando la prueba, que ahora corre en su suite.
    for (filter, name) in [("scan", "t_scan"), ("scan.", "t_scan"), ("lex.t_lex", "t_lex"), ("a.scan.", "t_scan"), ("kit.a.scan.t_scan", "t_scan")] {
        let (out, err, code) = ray(&d, &["test", filter]);
        assert_eq!(code, 0, "{out}\n{err}");
        assert!(out.contains("running 1 test(s)") && out.contains(name), "filtro {filter}:\n{out}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// Revisión de la 1.27.118: la sustitución de nombres de `ray doc` solo toca TIPOS. Una función
/// re-exportada con otro nombre no cambia el campo ni el parámetro que se llaman como ella, y un
/// tipo traído con `from … import` sale con el nombre del contrato en las tres vistas.
#[test]
fn ray_doc_renames_types_only_and_follows_from_imports() {
    let d = tree(
        "doc_types_only",
        &[
            ("ray.toml", APP_TOML),
            ("src/main.ray", "import booking;\nfn main() { print(1); }\n"),
            (
                "src/booking/model.ray",
                "pub struct Booking { id: int, total: int, next: Option<Booking>, }\n\
/// Builds a booking with the given total.\npub fn make(id: int, total: int) -> Booking { Booking { id: id, total: total, next: Option.None } }\n\
pub fn total(b: Booking) -> int { b.total }\n\
pub struct Cancel { id: int, }\npub enum Event { Booking(Booking), Cancel(Cancel), }\n\
pub const KIND: string = \"Booking\";\n",
            ),
            (
                "src/booking/service.ray",
                "import booking/model;\nfrom booking/model import Booking;\n\n\
pub fn bookings(limit: int) -> Result<[Booking], string> { Result.Ok([model.make(limit, 0)]) }\n",
            ),
            (
                "src/booking/mod.ray",
                "pub from booking/model import Booking as Reservation, total as booking_total, make, Event, KIND;\npub from booking/service import bookings;\n",
            ),
        ],
    );
    let (out, err, code) = ray(&d, &["doc", "src/booking/mod.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("### `struct Booking { id: int, total: int, next: Option<Booking> }`"), "{out}");
    assert!(out.contains("### `fn make(id: int, total: int) -> Reservation`"), "{out}");
    assert!(out.contains("### `fn bookings(limit: int) -> Result<[Reservation], string>`"), "{out}");
    assert!(!out.contains("booking_total: int"), "{out}");
    // Ni el nombre de una variante ni el valor de una constante son tipos.
    assert!(out.contains("enum Event { Booking(Reservation), Cancel(Cancel) }"), "{out}");
    assert!(out.contains("const KIND: string = \"Booking\""), "{out}");
    let (out, err, code) = ray(&d, &["doc", "booking"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("bookings(limit: int) -> Result<[Reservation], string>"), "{out}");
    let (out, err, code) = ray(&d, &["doc", "booking.bookings"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("-> Result<[Reservation], string>") && !out.contains("booking/Reservation"), "{out}");
    let (out, err, code) = ray(&d, &["doc", "booking.make"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("with the given total.") && out.contains("total: int) -> Reservation"), "{out}");
    let _ = std::fs::remove_dir_all(d);
}

/// Revisión de la 1.27.118: el nombre del contrato también para un tipo que llega por la fachada
/// de una cápsula anidada, y para el trait de un bound.
#[test]
fn ray_doc_follows_nested_facades_and_renames_bounds() {
    let d = tree(
        "doc_nested",
        &[
            ("ray.toml", APP_TOML),
            ("src/main.ray", "import booking;\nfn main() { print(1); }\n"),
            ("src/booking/core/inner.ray", "pub struct Deep { id: int, }\npub trait Priced { fn price(self) -> int; }\n"),
            ("src/booking/core/mod.ray", "pub from booking/core/inner import Deep, Priced;\n"),
            ("src/booking/audit.ray", "pub struct Item { n: int, }\n"),
            (
                "src/booking/service.ray",
                "import booking/core;\nfrom booking/core import Deep, Priced;\nfrom booking/audit import Item;\n\n\
pub fn first() -> Deep { Deep { id: 1 } }\npub fn second() -> core.Deep { Deep { id: 2 } }\n\
pub fn log(d: Deep, i: Item) -> int { d.id + i.n }\npub fn pick<Deep>(xs: [Deep]) -> Deep { xs[0] }\n\
pub fn cheapest<T: Priced>(xs: [T]) -> T { xs[0] }\npub fn cheapest_q<T: core.Priced>(xs: [T]) -> T { xs[0] }\n",
            ),
            (
                "src/booking/mod.ray",
                "pub from booking/core import Deep as Item, Priced as Costed;\npub from booking/service import first, second, cheapest, cheapest_q, log, pick;\n",
            ),
        ],
    );
    let (out, err, code) = ray(&d, &["doc", "src/booking/mod.ray"]);
    assert_eq!(code, 0, "{err}");
    for sig in ["fn first() -> Item", "fn second() -> Item", "fn cheapest<T: Costed>(xs: [T]) -> T", "fn cheapest_q<T: Costed>(xs: [T]) -> T"] {
        assert!(out.contains(&format!("### `{sig}`")), "falta {sig}:\n{out}");
    }
    // Un parámetro de tipo que se llama como el tipo re-exportado no se toca.
    assert!(out.contains("### `fn pick<Deep>(xs: [Deep]) -> Deep`"), "{out}");
    let (out, err, code) = ray(&d, &["doc", "booking.first"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("first() -> Item"), "{out}");
    // El nombre del contrato (`Item`) coincide con otro tipo importado y no exportado: cada uno
    // conserva el suyo.
    let (out, err, code) = ray(&d, &["doc", "booking.log"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("log(d: Item, i: booking/audit.Item) -> int"), "{out}");
    let _ = std::fs::remove_dir_all(d);
}
