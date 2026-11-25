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

    // 4. Un script cuya PRIMERA sentencia ya corrió y una posterior encuentra el candado: espera
    //    dentro de la llamada y, vencido el plazo, lo dice. Nunca se repite desde el principio:
    //    la primera sentencia (sobre una tabla temporal, que no necesita el candado) queda una vez.
    let scripted = open(path, 200);
    let _ = sqlite.exec(scripted, "CREATE TEMP TABLE audit (n INTEGER)", []).unwrap();
    let lock = open(path, 0);
    let _ = sqlite.exec(lock, "BEGIN IMMEDIATE", []).unwrap();
    match (sqlite.exec(scripted, "INSERT INTO audit VALUES (1); INSERT INTO t VALUES (999);", [])) {
        Result.Ok(_) => print("script: unexpected ok"),
        Result.Err(e) => print("script: ${e}"),
    }
    let audited = sqlite.query(scripted, "SELECT count(*) FROM audit", []).unwrap();
    print("audit rows: ${audited[0][0]}");
    // La MISMA llamada otra vez es una ejecución nueva y completa (la conexión no recuerda nada).
    let _ = sqlite.exec(lock, "COMMIT", []).unwrap();
    print("script again: ${sqlite.exec(scripted, "INSERT INTO audit VALUES (1); INSERT INTO t VALUES (999);", []).unwrap()}");
    let audited2 = sqlite.query(scripted, "SELECT count(*) FROM audit", []).unwrap();
    print("audit rows: ${audited2[0][0]}");

    // 4b. El mismo script cuando el candado SÍ se suelta —lo tiene otra fibra de este hilo, que
    //     solo puede soltarlo si la espera no retiene el hilo—: sigue desde la sentencia que lo
    //     encontró, sin repetir la primera, y devuelve las filas de todo el script.
    sqlite.busy_timeout(scripted, 3000);
    let _ = sqlite.exec(lock, "BEGIN IMMEDIATE", []).unwrap();
    let holder = spawn_local(fn() -> int {
        time.sleep(80);
        sqlite.exec(lock, "COMMIT", []).unwrap()
    });
    print("script resumed: ${sqlite.exec(scripted, "INSERT INTO audit VALUES (2);\n-- después\nINSERT INTO t VALUES (1000); INSERT INTO t VALUES (1001);", []).unwrap()}");
    let _ = join(holder);
    let audited3 = sqlite.query(scripted, "SELECT count(*) FROM audit", []).unwrap();
    print("audit rows: ${audited3[0][0]}");

    // 5. `PRAGMA busy_timeout`, llegue por donde llegue, fija el plazo de la conexión y la espera
    //    sigue siendo de la fibra: mientras una espera 300 ms, otra del mismo hilo avanza.
    let pragmas = open(path, 0);
    let _ = sqlite.exec(pragmas, "PRAGMA synchronous = NORMAL;\n-- menos espera\nPRAGMA main.busy_timeout = 600;", []).unwrap();
    print("script pragma: ${sqlite.query(pragmas, "PRAGMA busy_timeout", []).unwrap()[0][0]}");
    let _ = sqlite.exec(lock, "BEGIN IMMEDIATE", []).unwrap();
    let asked = time.monotonic();
    // Termina a los ~100 ms SOLO si la espera de abajo no retiene el hilo.
    let ticker = spawn_local(fn() -> int {
        var n = 0;
        while (n < 5) {
            time.sleep(20);
            n = n + 1;
        }
        time.monotonic()
    });
    let refused = sqlite.exec(pragmas, "BEGIN IMMEDIATE", []).is_err();
    let waited = time.monotonic() - asked;
    let ticked = join(ticker) - asked;
    print("waited the script's timeout: ${refused} ${waited >= 550 && waited < 3000} ticker ran meanwhile=${ticked < 500}");
    let _ = sqlite.exec(lock, "COMMIT", []).unwrap();
    // Por `query` también; y una COLUMNA con ese nombre no es el PRAGMA.
    let _ = sqlite.query(pragmas, "PRAGMA busy_timeout = 700", []).unwrap();
    print("query pragma: ${sqlite.query(pragmas, "PRAGMA busy_timeout", []).unwrap()[0][0]}");
    let _ = sqlite.exec(pragmas, "CREATE TABLE settings (id INTEGER, busy_timeout INTEGER)", []).unwrap();
    let _ = sqlite.exec(pragmas, "INSERT INTO settings VALUES (1, 9); UPDATE settings SET busy_timeout = 0 WHERE id = 1;", []).unwrap();
    print("a column is not the pragma: ${sqlite.query(pragmas, "PRAGMA busy_timeout", []).unwrap()[0][0]}");
    sqlite.busy_timeout(pragmas, 9999999999);
    print("busy_timeout(huge): ${sqlite.query(pragmas, "PRAGMA busy_timeout", []).unwrap()[0][0]}");
    sqlite.busy_timeout(pragmas, 0);
    print("busy_timeout(): ${sqlite.query(pragmas, "PRAGMA busy_timeout", []).unwrap()[0][0]}");

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
script: database is locked (part of the script had run; the rest did not run — a transaction it opened is still open)\n\
audit rows: 1\n\
script again: 2\n\
audit rows: 2\n\
script resumed: 3\n\
audit rows: 3\n\
script pragma: 600\n\
waited the script's timeout: true true ticker ran meanwhile=true\n\
query pragma: 700\n\
a column is not the pragma: 700\n\
busy_timeout(huge): 2147483647\n\
busy_timeout(): 0\n\
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
