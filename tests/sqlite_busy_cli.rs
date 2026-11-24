//! rayhotel #491 — `db/sqlite` espera un candado APARCANDO la fibra. Con el `busy_timeout` de
//! SQLite la espera ocurría dentro de la librería C, reteniendo el hilo (y el registro de handles
//! del runtime): la fibra que tenía el candado no podía terminar y todos los escritores quedaban
//! parados hasta el plazo — 20 reservas a la vez dejaban el servidor sin responder más de 20 s.
//! Ahora el plazo lo lleva la conexión de raylang y cada intento fallido cede con `time.sleep`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

const PROGRAM: &str = r#"import db/sqlite;
import std/time;

fn open(path: string, busy: int) -> sqlite.Conn {
    let c = sqlite.connect(path).unwrap();
    // La forma habitual en SQLite; `sqlite.busy_timeout(c, ms)` es lo mismo.
    let _ = sqlite.exec(c, "PRAGMA busy_timeout = ${busy}", []);
    c
}

fn main() -> int {
    let path = env("DB_PATH").unwrap_or("busy.db");
    let setup = open(path, 0);
    let _ = sqlite.exec(setup, "CREATE TABLE t (n INTEGER)", []).unwrap();

    // 1. Sin plazo: falla en el acto.
    let holder = open(path, 0);
    let _ = sqlite.exec(holder, "BEGIN IMMEDIATE", []).unwrap();
    let nowait = open(path, 0);
    match (sqlite.exec(nowait, "BEGIN IMMEDIATE", [])) {
        Result.Ok(_) => print("nowait: unexpected ok"),
        Result.Err(e) => print("nowait: ${e}"),
    }
    let _ = sqlite.exec(holder, "COMMIT", []).unwrap();

    // 2. Dos fibras en el MISMO hilo: la que espera no retiene a la que tiene el candado.
    let started = time.monotonic();
    let first = spawn_local(fn() -> int {
        let c = open(path, 2000);
        let _ = sqlite.exec(c, "BEGIN IMMEDIATE", []).unwrap();
        time.sleep(100);
        let _ = sqlite.exec(c, "INSERT INTO t VALUES (1)", []).unwrap();
        let _ = sqlite.exec(c, "COMMIT", []).unwrap();
        time.monotonic() - started
    });
    let second = spawn_local(fn() -> string {
        time.sleep(10);
        let c = open(path, 2000);
        match (sqlite.exec(c, "BEGIN IMMEDIATE", [])) {
            Result.Ok(_) => {
                let _ = sqlite.exec(c, "INSERT INTO t VALUES (2)", []).unwrap();
                let _ = sqlite.exec(c, "COMMIT", []).unwrap();
                "ok"
            },
            Result.Err(e) => e,
        }
    });
    let first_ms = join(first);
    print("second: ${join(second)}");
    print("holder finished on time: ${first_ms < 1000}");
    print("both done on time: ${time.monotonic() - started < 1500}");

    // 3. Veinte escritores a la vez, cada uno con su conexión.
    let begun = time.monotonic();
    var tasks: [Task<bool>] = [];
    for i in 0..20 {
        tasks.push(spawn(fn() -> bool {
            let c = open(path, 5000);
            let ok = sqlite.exec(c, "BEGIN IMMEDIATE", []).is_ok()
                && sqlite.exec(c, "INSERT INTO t VALUES (?1)", [to_string(100 + i)]).is_ok()
                && sqlite.exec(c, "COMMIT", []).is_ok();
            sqlite.disconnect(c);
            ok
        }));
    }
    var good = 0;
    for t in tasks {
        if (join(t)) { good = good + 1; }
    }
    print("writers ok: ${good}");
    print("writers on time: ${time.monotonic() - begun < 4000}");
    let rows = sqlite.query(setup, "SELECT count(*) FROM t", []).unwrap();
    print("rows: ${rows[0][0]}");

    // 4. Un script que se queda a medias NO se reintenta entero: su primera sentencia ya se aplicó.
    let scripted = open(path, 300);
    let _ = sqlite.exec(scripted, "CREATE TEMP TABLE audit (n INTEGER)", []).unwrap();
    let lock = open(path, 0);
    let _ = sqlite.exec(lock, "BEGIN IMMEDIATE", []).unwrap();
    match (sqlite.exec(scripted, "INSERT INTO audit VALUES (1); INSERT INTO t VALUES (999);", [])) {
        Result.Ok(_) => print("script: unexpected ok"),
        Result.Err(e) => print("script: ${e}"),
    }
    let audited = sqlite.query(scripted, "SELECT count(*) FROM audit", []).unwrap();
    print("audit rows: ${audited[0][0]}");

    // 5. Un `PRAGMA busy_timeout` DENTRO de un script no devuelve la espera a SQLite.
    let pragmas = open(path, 0);
    let _ = sqlite.exec(pragmas, "PRAGMA busy_timeout = 3000; PRAGMA synchronous = NORMAL;", []).unwrap();
    sqlite.busy_timeout(pragmas, 0);
    let asked = time.monotonic();
    print("after a script pragma: ${sqlite.exec(pragmas, "BEGIN IMMEDIATE", []).is_err()} fast=${time.monotonic() - asked < 1000}");
    let _ = sqlite.exec(lock, "COMMIT", []).unwrap();

    // 6. Lo que esperar no arregla: una transacción que ya lee y quiere escribir mientras otra
    //    tiene el candado de escritura. Falla en el acto aunque el plazo sea de 5 s.
    let reader = open(path, 5000);
    let _ = sqlite.exec(reader, "BEGIN", []).unwrap();
    let _ = sqlite.query(reader, "SELECT count(*) FROM t", []).unwrap();
    let _ = sqlite.exec(lock, "BEGIN IMMEDIATE", []).unwrap();
    let tried = time.monotonic();
    match (sqlite.exec(reader, "INSERT INTO t VALUES (7)", [])) {
        Result.Ok(_) => print("upgrade: unexpected ok"),
        Result.Err(e) => print("upgrade: not waited=${e.contains("not waited for")} fast=${time.monotonic() - tried < 1000}"),
    }
    let _ = sqlite.exec(reader, "ROLLBACK", []);
    let _ = sqlite.exec(lock, "COMMIT", []);
    0
}
"#;

const WANT: &str = "nowait: database is locked\n\
second: ok\n\
holder finished on time: true\n\
both done on time: true\n\
writers ok: 20\n\
writers on time: true\n\
rows: 22\n\
script: database is locked (after 1 statement(s) of the script had run; the rest did not run)\n\
audit rows: 1\n\
after a script pragma: true fast=true\n\
upgrade: not waited=true fast=true\n";

fn project(name: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("ray_sqlite_busy_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    let repo = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"busy\"\nversion = \"0.1.0\"\n\n[dependencies]\ndb = \"path:{repo}/packages/db\"\n"),
    )
    .unwrap();
    std::fs::write(base.join("src/main.ray"), PROGRAM).unwrap();
    base
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn a_locked_database_is_waited_for_without_holding_the_thread_on_the_vm() {
    let d = project("vm");
    let out = Command::new(BIN).arg("run").current_dir(&d).env("DB_PATH", d.join("busy.db")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_locked_database_is_waited_for_without_holding_the_thread_natively() {
    if !has_rustc() {
        return;
    }
    let d = project("native");
    let bin = d.join("busy_bin");
    let b = Command::new(BIN).args(["build", "--native", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
    assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
    let out = Command::new(&bin).env("DB_PATH", d.join("busy.db")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(d);
}
