//! M53.4 — cliente SQLite (`packages/db/sqlite.ray`) sobre los primitivos `__sqlite_*` (rusqlite,
//! M53.3). A diferencia de mysql/postgres NO hay servidor (ni de juguete): SQLite es embebido y
//! `":memory:"` da una base determinista → el test es un oráculo conductual puro en ambos motores
//! (mismo stdout exacto): DDL + INSERT con parámetros + SELECT (con NULL → "") + transacción con
//! ROLLBACK + error SQL como valor + uso tras disconnect.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Arma un mini-proyecto con la dependencia por ruta al paquete `db` y el programa de la prueba.
fn project(base: &std::path::Path) -> std::path::PathBuf {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/db");
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ndb = \"path:{}\"\n",
            db.display()
        ),
    )
    .unwrap();
    let main = r#"import db/sqlite;

fn main() -> int {
    var c = match (sqlite.connect(":memory:")) {
        Result.Ok(conn) => conn,
        Result.Err(e) => { print(e); return 1; },
    };
    let sin: [string] = [];
    match (sqlite.exec(c, "CREATE TABLE alumnos (id INTEGER, name TEXT, nota REAL)", sin)) {
        Result.Ok(_) => {},
        Result.Err(e) => { print(e); return 1; },
    }
    // INSERT con parámetros posicionales; el segundo deja `nota` en NULL.
    let _ = sqlite.exec(c, "INSERT INTO alumnos VALUES (?1, ?2, ?3)", ["1", "ada", "36"]);
    match (sqlite.exec(c, "INSERT INTO alumnos (id, name) VALUES (?1, ?2)", ["2", "grace"])) {
        Result.Ok(n) => { print("afectadas: " + to_string(n)); },
        Result.Err(e) => { print(e); return 1; },
    }
    // El rowid del último INSERT (raylang puro sobre la misma conexión).
    match (sqlite.last_insert_rowid(c)) {
        Result.Ok(id) => { print("rowid: " + to_string(id)); },
        Result.Err(e) => { print(e); return 1; },
    }
    match (sqlite.query(c, "SELECT name, nota FROM alumnos ORDER BY id", sin)) {
        Result.Ok(rows) => {
            var i = 0;
            while (i < rows.len()) {
                print(rows[i].join("|"));
                i = i + 1;
            }
        },
        Result.Err(e) => { print(e); return 1; },
    }
    // Consulta con parámetro.
    match (sqlite.query(c, "SELECT nota FROM alumnos WHERE name = ?1", ["ada"])) {
        Result.Ok(rows) => { print("nota de ada: " + rows[0][0]); },
        Result.Err(e) => { print(e); return 1; },
    }
    // Transacción revertida: el INSERT dentro no debe sobrevivir al ROLLBACK.
    let _ = sqlite.exec(c, "BEGIN", sin);
    let _ = sqlite.exec(c, "INSERT INTO alumnos (id, name) VALUES (?1, ?2)", ["3", "fantasma"]);
    let _ = sqlite.exec(c, "ROLLBACK", sin);
    match (sqlite.query(c, "SELECT count(*) FROM alumnos", sin)) {
        Result.Ok(rows) => { print("after rollback: " + rows[0][0]); },
        Result.Err(e) => { print(e); return 1; },
    }
    // Un error SQL vuelve como valor, no aborta.
    match (sqlite.query(c, "SELECT * FROM no_existe", sin)) {
        Result.Ok(_) => { print("no debería"); },
        Result.Err(e) => { print("sqlite: " + e); },
    }
    sqlite.disconnect(c);
    // Usar la conexión cerrada falla limpio (error como valor).
    match (sqlite.exec(c, "SELECT 1", sin)) {
        Result.Ok(_) => { print("no debería"); },
        Result.Err(e) => { print("cerrada: " + e); },
    }
    0
}
"#;
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

fn run(app: &std::path::Path, flags: &[&str]) -> (String, i32) {
    let mut args = vec!["run"];
    args.extend_from_slice(flags);
    let out = Command::new(BIN).args(&args).current_dir(app).output().expect("lanza el binary");
    assert!(
        out.status.success(),
        "runs sin error\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

const ESPERADO: &str = "afectadas: 1\nrowid: 2\nada|36\ngrace|\nnota de ada: 36\nafter rollback: 2\nsqlite: no such table: no_existe\ncerrada: invalid or already closed handle\n";

#[test]
fn sqlite_crud_transaccion_y_errors() {
    let base = std::env::temp_dir().join("ray_sqlite_cli");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let app = project(&base);

    // VM (motor de producto) e intérprete (oráculo): mismo stdout exacto.
    let (out_vm, _) = run(&app, &[]);
    assert_eq!(out_vm, ESPERADO, "VM");
    let (out_interp, _) = run(&app, &["--interp"]);
    assert_eq!(out_interp, ESPERADO, "intérprete");
}

#[test]
fn sqlite_path_invalida_da_error_claro() {
    let base = std::env::temp_dir().join("ray_sqlite_cli_badpath");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // Abrir un DIRECTORIO como base de datos debe fallar como valor (Result.Err), no abortar.
    let app = base.join("app");
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/db");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ndb = \"path:{}\"\n",
            db.display()
        ),
    )
    .unwrap();
    let main = format!(
        r#"import db/sqlite;

fn main() -> int {{
    match (sqlite.connect("{}")) {{
        Result.Ok(_) => {{ print("no debería"); return 1; }},
        Result.Err(e) => {{ print("error: " + e); return 0; }},
    }}
}}
"#,
        base.display()
    );
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    let (out, _) = run(&app, &[]);
    assert!(out.starts_with("error: "), "stdout: {out}");
}
