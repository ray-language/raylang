//! M296 (IDEAS §96 #8, §95 P1): dominios de handles. `spawn` hereda el dominio del padre (el
//! webserver reparte conexiones a fibras hijas); `spawn_isolated` estrena uno: la hija no ve los
//! handles del padre ni el padre los de la hija — un handle ajeno se comporta como cerrado en todos
//! los sitios de acceso. Mismo texto y exit code en la VM y en el binario nativo (con y sin fibras).

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

const PROGRAM: &str = r#"
import std/fs;

fn main() -> int {
    let dir = args()[0];
    let path = dir + "/probe.txt";
    let _ = fs.write_file(path, "line one\nline two\n");
    let parent_h = match (fs.open(path, "r")) { Result.Ok(h) => h, Result.Err(e) => { print("open: " + e); return 1; } };
    // spawn: hereda el dominio → ve (y consume una línea de) el handle del padre.
    let t1 = spawn(fn() -> string {
        match (fs.read_line(parent_h)) { Option.Some(l) => "child sees: " + l, Option.None => "child sees: none" }
    });
    print(join(t1));
    // spawn_isolated: NO ve el handle del padre; abre el suyo y lo devuelve como int.
    let t2 = spawn_isolated(fn() -> int {
        match (fs.read_line(parent_h)) { Option.Some(l) => print("isolated sees parent: " + l), Option.None => print("isolated sees parent: none") };
        match (fs.open(path, "r")) { Result.Ok(h) => h, Result.Err(e) => -1 }
    });
    let child_h = join(t2);
    print("child handle > 0: " + to_string(child_h > 0));
    match (fs.read_line(child_h)) { Option.Some(l) => print("parent sees child's: " + l), Option.None => print("parent sees child's: none") };
    print("parent close child's: " + to_string(close(child_h)));
    // El padre sigue viendo el suyo (la hija normal consumió la primera línea).
    match (fs.read_line(parent_h)) { Option.Some(l) => print("parent own: " + l), Option.None => print("parent own: none") };
    // Un hijo del aislado hereda el dominio AISLADO (no el del abuelo).
    let t3 = spawn_isolated(fn() -> string {
        let mine = match (fs.open(path, "r")) { Result.Ok(h) => h, Result.Err(e) => -1 };
        let g = spawn(fn() -> string {
            let a = match (fs.read_line(mine)) { Option.Some(l) => "grandchild sees isolated parent's: " + l, Option.None => "grandchild sees isolated parent's: none" };
            let b = match (fs.read_line(parent_h)) { Option.Some(l) => " / root's: " + l, Option.None => " / root's: none" };
            a + b
        });
        join(g)
    });
    print(join(t3));
    0
}
"#;

const EXPECTED: &str = "child sees: line one\nisolated sees parent: none\nchild handle > 0: true\nparent sees child's: none\nparent close child's: 0\nparent own: line two\ngrandchild sees isolated parent's: line one / root's: none\n";

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn isolated_domains_behave_the_same_on_vm_and_native() {
    let base = std::env::temp_dir().join(format!("ray_handle_domains_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let src = base.join("domains.ray");
    std::fs::write(&src, PROGRAM).unwrap();
    let dir = base.to_str().unwrap().to_string();

    let out = Command::new(BIN).args(["run", src.to_str().unwrap(), &dir]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "VM: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), EXPECTED, "VM");

    if !has_rustc() {
        eprintln!("saltando la parte nativa: rustc no disponible");
        return;
    }
    for (label, extra) in [("fibras", &[][..]), ("sin fibras", &["--without", "fibers,mimalloc,ahash"][..])] {
        let bin = base.join(format!("domains_{}{}", label.replace(' ', "_"), std::env::consts::EXE_SUFFIX));
        let mut args = vec!["build", "--native"];
        args.extend_from_slice(extra);
        args.extend_from_slice(&[src.to_str().unwrap(), "-o", bin.to_str().unwrap()]);
        let out = Command::new(BIN).args(&args).output().unwrap();
        assert!(out.status.success(), "compila el nativo ({label}): {}", String::from_utf8_lossy(&out.stderr));
        let out = Command::new(&bin).arg(&dir).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "nativo ({label}): {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), EXPECTED, "nativo ({label})");
    }
    let _ = std::fs::remove_dir_all(&base);
}
