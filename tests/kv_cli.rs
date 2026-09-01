//! G-rev (IDEAS §48) — `std/kv`: estado clave/valor persistido en raylang puro. El caso de uso
//! es conservar sesiones/config entre reloads de `ray dev` sin una base de datos. Como toca
//! disco (no determinista para el oráculo de `vm.rs`), se prueba por subproceso corriendo cada
//! fuente por AMBOS motores y exigiendo que coincidan y den la salida esperada (patrón de
//! `collections_cli.rs`), sobre archivos temporales propios de cada test.

use std::process::Command;

/// Corre `src` (fuente inline en un temporal) por el motor elegido; devuelve (stdout, código).
fn run_src(name: &str, src: &str, vm: bool) -> (String, i32) {
    let path = std::env::temp_dir().join(format!("kv_cli_{name}.ray"));
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(if vm { "--vm" } else { "--interp" })
        .arg(&path)
        .output()
        .expect("ejecuta raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

/// Oráculo por subproceso: ambos motores coinciden (stdout + código 0) y la salida es la esperada.
/// `store` es la ruta del archivo de estado del test, que se limpia antes y después.
fn ambos_engines(name: &str, store: &std::path::Path, src: &str, expected: &str) {
    let _ = std::fs::remove_file(store);
    let (o_in, c_in) = run_src(name, src, false);
    assert_eq!(c_in, 0, "intérprete sale 0 ({name})\n{o_in}");
    assert_eq!(o_in, expected, "salida esperada en el intérprete ({name})");
    let _ = std::fs::remove_file(store);
    let (o_vm, c_vm) = run_src(name, src, true);
    assert_eq!(c_vm, 0, "vm sale 0 ({name})\n{o_vm}");
    assert_eq!(o_vm, o_in, "ambos motores coinciden ({name})");
    let _ = std::fs::remove_file(store);
}

/// Ruta del archivo de estado del test (única por nombre, en el temp del sistema).
fn store_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("kv_cli_{name}.rkv"))
}

/// El ciclo completo de dev-state: abrir (vacío), poblar, guardar, REABRIR (el "reload") y
/// encontrar el estado — strings, bytes crudos, borrado, keys ordenadas y clave ausente.
#[test]
fn roundtrip_persistencia() {
    let store = store_path("roundtrip");
    let src = format!(
        r#"import std/kv;
fn main() -> int {{
    let path = "{p}";
    var s = match (kv.open(path)) {{ Result.Ok(st) => st, Result.Err(e) => {{ print("open: " + e); return 1; }}, }};
    s.set_string("user", "roberto");
    s.set("raw", bytes_of([0, 255, 128]));
    s.set_string("gone", "x");
    assert(s.delete("gone"));
    assert(!s.delete("gone"));
    match (s.save()) {{ Result.Ok(n) => print("saved " + to_string(n)), Result.Err(e) => {{ print("save: " + e); return 1; }}, }}
    let s2 = match (kv.open(path)) {{ Result.Ok(st) => st, Result.Err(e) => {{ print("reopen: " + e); return 1; }}, }};
    print("count " + to_string(s2.count()));
    print("keys " + s2.keys().join(","));
    match (s2.get_string("user")) {{ Option.Some(v) => print("user " + v), Option.None => print("user MISSING"), }}
    match (s2.get("raw")) {{ Option.Some(b) => print("raw " + to_string(b.len()) + " " + to_string(b[1])), Option.None => print("raw MISSING"), }}
    match (s2.get("nope")) {{ Option.Some(b) => print("nope PRESENT"), Option.None => print("nope absent"), }}
    0
}}
"#,
        p = store.display()
    );
    let expected = "saved 41\ncount 2\nkeys raw,user\nuser roberto\nraw 3 255\nnope absent\n";
    ambos_engines("roundtrip", &store, &src, expected);
}

/// Sobrescribir una clave y volver a guardar conserva el ÚLTIMO valor (y solo ese).
#[test]
fn overwrite_key() {
    let store = store_path("sobrescribir");
    let src = format!(
        r#"import std/kv;
fn main() -> int {{
    let path = "{p}";
    var s = match (kv.open(path)) {{ Result.Ok(st) => st, Result.Err(e) => {{ return 1; }}, }};
    s.set_string("k", "primero");
    match (s.save()) {{ Result.Ok(n) => 0, Result.Err(e) => {{ return 1; }}, }};
    var s2 = match (kv.open(path)) {{ Result.Ok(st) => st, Result.Err(e) => {{ return 1; }}, }};
    s2.set_string("k", "segundo");
    match (s2.save()) {{ Result.Ok(n) => 0, Result.Err(e) => {{ return 1; }}, }};
    let s3 = match (kv.open(path)) {{ Result.Ok(st) => st, Result.Err(e) => {{ return 1; }}, }};
    match (s3.get_string("k")) {{ Option.Some(v) => print(v), Option.None => print("MISSING"), }}
    print(to_string(s3.count()));
    0
}}
"#,
        p = store.display()
    );
    ambos_engines("sobrescribir", &store, &src, "segundo\n1\n");
}

/// Un store vacío también persiste y reabre vacío (solo la cabecera: magic + count 0).
#[test]
fn empty_store() {
    let store = store_path("vacio");
    let src = format!(
        r#"import std/kv;
fn main() -> int {{
    let path = "{p}";
    var s = match (kv.open(path)) {{ Result.Ok(st) => st, Result.Err(e) => {{ return 1; }}, }};
    match (s.save()) {{ Result.Ok(n) => print("saved " + to_string(n)), Result.Err(e) => {{ return 1; }}, }}
    let s2 = match (kv.open(path)) {{ Result.Ok(st) => st, Result.Err(e) => {{ return 1; }}, }};
    print("count " + to_string(s2.count()));
    0
}}
"#,
        p = store.display()
    );
    ambos_engines("vacio", &store, &src, "saved 8\ncount 0\n");
}

/// Un archivo existente que NO es un store (magic malo / truncado) es un error honesto de
/// `open` — nunca se descarta estado en silencio.
#[test]
fn corrupt_file_is_error() {
    let store = store_path("corrupto");
    let src = format!(
        r#"import std/kv;
fn main() -> int {{
    match (kv.open("{p}")) {{
        Result.Ok(st) => print("ok?!"),
        Result.Err(e) => print("err: " + e),
    }}
    0
}}
"#,
        p = store.display()
    );
    for (caso, contenido, msg) in [
        ("basura", b"esto no es un store".to_vec(), "bad magic"),
        ("truncado", b"RKV".to_vec(), "truncated header"),
        // cabecera válida que anuncia 1 entrada que no está → truncated key length
        ("cortado", b"RKV1\x01\x00\x00\x00".to_vec(), "truncated key length"),
    ] {
        std::fs::write(&store, &contenido).unwrap();
        let (o_in, c_in) = run_src("corrupto", &src, false);
        std::fs::write(&store, &contenido).unwrap();
        let (o_vm, c_vm) = run_src("corrupto", &src, true);
        assert_eq!(c_in, 0, "intérprete sale 0 ({caso})\n{o_in}");
        assert_eq!(c_vm, 0, "vm sale 0 ({caso})\n{o_vm}");
        assert_eq!(o_in, o_vm, "ambos motores coinciden ({caso})");
        assert!(o_in.starts_with("err: ") && o_in.contains(msg), "detecta {caso}: {o_in}");
    }
    let _ = std::fs::remove_file(&store);
}

/// G.2 — el store COMPARTIDO (actor CSP): el handle cruza fibras (spawn/scope/join), la
/// coherencia es FIFO, `save` persiste vía el actor y el archivo reabre local. Solo VM
/// (spawn/canales) y `--deterministic` (salida exacta, como `concurrency_cli`).
#[test]
fn shared_store_actor() {
    let store = store_path("shared");
    let _ = std::fs::remove_file(&store);
    let src = format!(
        r#"import std/kv;
fn main() -> int {{
    let sh = match (kv.open_shared("{p}")) {{ Result.Ok(s) => s, Result.Err(e) => {{ print("open: " + e); return 1; }}, }};
    sh.set_string("main", "hola");
    let r = scope(fn() -> int {{
        let t1 = spawn(fn() -> int {{
            sh.set_string("w1", "uno");
            sh.count()
        }});
        let t2 = spawn(fn() -> int {{
            sh.set_string("w2", "dos");
            sh.count()
        }});
        join(t1) + join(t2)
    }});
    print("joined " + to_string(r >= 2));
    print("count " + to_string(sh.count()));
    print("keys " + sh.keys().join(","));
    match (sh.get_string("w1")) {{ Option.Some(v) => print("w1 " + v), Option.None => print("w1 MISSING"), }}
    assert(sh.delete("w2"));
    match (sh.save()) {{ Result.Ok(n) => print("saved " + to_string(n)), Result.Err(e) => {{ print("save: " + e); return 1; }}, }}
    kv.stop(sh);
    let s2 = match (kv.open("{p}")) {{ Result.Ok(st) => st, Result.Err(e) => {{ print("reopen: " + e); return 1; }}, }};
    print("persisted " + s2.keys().join(","));
    0
}}
"#,
        p = store.display()
    );
    let path = std::env::temp_dir().join("kv_cli_shared.ray");
    std::fs::write(&path, &src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["--vm", "--deterministic"])
        .arg(&path)
        .output()
        .expect("ejecuta raylang");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "sale 0\n{stdout}");
    assert_eq!(
        stdout,
        "joined true\ncount 3\nkeys main,w1,w2\nw1 uno\nsaved 37\npersisted main,w1\n",
        "salida del actor compartido"
    );
    let _ = std::fs::remove_file(&store);
}

/// El guardado es atómico (temp + rename): tras `save` no queda `<path>.tmp` y el archivo
/// final existe.
#[test]
fn atomic_save_leaves_no_residue() {
    let store = store_path("atomico");
    let src = format!(
        r#"import std/kv;
import std/fs;
fn main() -> int {{
    let path = "{p}";
    var s = match (kv.open(path)) {{ Result.Ok(st) => st, Result.Err(e) => {{ return 1; }}, }};
    s.set_string("k", "v");
    match (s.save()) {{ Result.Ok(n) => 0, Result.Err(e) => {{ return 1; }}, }};
    print("final " + to_string(fs.exists(path)));
    print("tmp " + to_string(fs.exists(path + ".tmp")));
    0
}}
"#,
        p = store.display()
    );
    ambos_engines("atomico", &store, &src, "final true\ntmp false\n");
}

/// M154: read-modify-write atómico — incr (ausente=0, acumulado, no-entero=Err) y set_if
/// (CAS; None = solo-si-ausente), sobre el Store LOCAL (ambos motores).
#[test]
fn incr_and_set_if_on_the_local_store() {
    let store = std::env::temp_dir().join("kv_cli_rmw.rkv");
    let src = format!(
        r#"import std/kv;

fn main() {{
    let s = kv.empty("{p}");
    let _ = s.incr("hits", 1);
    match (s.incr("hits", 4)) {{
        Result.Ok(n) => print("hits: " + to_string(n)),
        Result.Err(e) => print("err: " + e),
    }}
    s.set_string("name", "ray");
    match (s.incr("name", 1)) {{
        Result.Ok(_) => print("bad"),
        Result.Err(e) => print("rejected: " + to_string(e.contains("not an integer"))),
    }}
    print(to_string(s.set_if("flag", Option.None, "on".to_bytes())));
    print(to_string(s.set_if("flag", Option.Some("off".to_bytes()), "x".to_bytes())));
    print(to_string(s.set_if("flag", Option.Some("on".to_bytes()), "off".to_bytes())));
    match (s.get_string("hits")) {{
        Option.Some(v) => print("as string: " + v),
        Option.None => print("missing"),
    }}
}}
"#,
        p = store.display()
    );
    ambos_engines(
        "rmw",
        &store,
        &src,
        "hits: 5\nrejected: true\ntrue\nfalse\ntrue\nas string: 5\n",
    );
    let _ = std::fs::remove_file(&store);
}

/// M154: los mismos RMW por el ACTOR (SharedStore) — el ciclo entero corre en la fibra
/// dueña. Solo VM (spawn), --deterministic para salida estable (patrón shared_store_actor).
#[test]
fn incr_and_set_if_on_the_shared_store() {
    let store = std::env::temp_dir().join("kv_cli_rmw_shared.rkv");
    let _ = std::fs::remove_file(&store);
    let src = format!(
        r#"import std/kv;

fn main() {{
    let sh = kv.share(kv.empty("{p}"));
    let _ = sh.incr("n", 2);
    match (sh.incr("n", 3)) {{
        Result.Ok(n) => print("n: " + to_string(n)),
        Result.Err(e) => print("err: " + e),
    }}
    print(to_string(sh.set_if("k", Option.None, "v".to_bytes())));
    print(to_string(sh.set_if("k", Option.None, "w".to_bytes())));
    kv.stop(sh);
}}
"#,
        p = store.display()
    );
    let path = std::env::temp_dir().join("kv_cli_rmw_shared.ray");
    std::fs::write(&path, &src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(["--deterministic", "--vm"])
        .arg(&path)
        .output()
        .expect("runs the vm");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "n: 5\ntrue\nfalse\n");
    let _ = std::fs::remove_file(&store);
}
