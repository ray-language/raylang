//! M85 — `packages/tz`: hora local IANA (parser TZif v2 en raylang puro, RFC 8536).
//! Corre sobre los **fixtures commiteados** (`packages/tz/fixtures/*.tzif`, tzdata es dominio
//! público) → determinista y sin depender del zoneinfo del sistema. Verifica offsets/abreviaturas
//! /DST en ambos lados de las transiciones de 2026, el `LocalResult` de tres casos (Single /
//! Ambiguous en el solape de otoño / Gap en el hueco de primavera), el round-trip civil↔UTC y
//! los errores como valores. Ambos motores deben coincidir.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

fn project(base: &std::path::Path) -> std::path::PathBuf {
    let tz = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/tz");
    let fixtures = tz.join("fixtures");
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ntz = \"path:{}\"\n",
            tz.display()
        ),
    )
    .unwrap();
    let main = format!(
        r#"import tz/tz;
import std/time;

fn civil(y: int, mo: int, d: int, h: int, mi: int) -> time.DateTime {{
    time.DateTime {{ year: y, month: mo, day: d, hour: h, minute: mi, second: 0, weekday: 0 }}
}}

// Resuelve una hora civil SIN ambigüedad a su instante (falla claro si no es Single).
fn instante(z: tz.Zone, c: time.DateTime) -> int {{
    match (tz.to_utc(z, c)) {{
        tz.LocalResult.Single(ms) => ms,
        tz.LocalResult.Ambiguous(a, b) => {{ panic("ambiguous inesperado"); 0 }},
        tz.LocalResult.Gap => {{ panic("gap inesperado"); 0 }},
    }}
}}

fn describe(z: tz.Zone, ms: int) {{
    print(time.to_iso8601(tz.to_local(z, ms)) + " " + tz.abbrev_at(z, ms)
        + " dst=" + to_string(tz.is_dst_at(z, ms))
        + " off_h=" + to_string(tz.offset_at(z, ms) / 3600000));
}}

fn main() -> int {{
    let mad = match (tz.load_file("{fixtures}/Europe_Madrid.tzif")) {{
        Result.Ok(z) => z,
        Result.Err(e) => {{ print("ERR " + e); return 1; }},
    }};
    // Invierno y verano 2026 (CET +1 / CEST +2).
    let invierno = instante(mad, civil(2026, 1, 15, 12, 0));
    describe(mad, invierno);
    let verano = instante(mad, civil(2026, 7, 15, 12, 0));
    describe(mad, verano);
    // Round-trip civil → UTC → civil.
    print(time.to_iso8601(tz.to_local(mad, verano)));
    // El HUECO de primavera: 02:30 del 29-03-2026 no existe.
    match (tz.to_utc(mad, civil(2026, 3, 29, 2, 30))) {{
        tz.LocalResult.Single(ms) => print("FALLO: single"),
        tz.LocalResult.Ambiguous(a, b) => print("FALLO: ambiguous"),
        tz.LocalResult.Gap => print("gap"),
    }}
    // El SOLAPE de otoño: 02:30 del 25-10-2026 ocurre dos veces (CEST y luego CET).
    match (tz.to_utc(mad, civil(2026, 10, 25, 2, 30))) {{
        tz.LocalResult.Single(ms) => print("FALLO: single"),
        tz.LocalResult.Ambiguous(a, b) => {{
            print("ambiguous delta_min=" + to_string((b - a) / 60000)
                + " antes=" + tz.abbrev_at(mad, a) + " despues=" + tz.abbrev_at(mad, b));
        }},
        tz.LocalResult.Gap => print("FALLO: gap"),
    }}
    // Los dos lados exactos de la transición de primavera (01:00 UTC).
    let salto = instante(mad, civil(2026, 3, 29, 3, 0));  // 03:00 CEST = 01:00 UTC
    describe(mad, salto - 1000);
    describe(mad, salto);
    // Zona fija.
    let utc = match (tz.load_file("{fixtures}/UTC.tzif")) {{
        Result.Ok(z) => z,
        Result.Err(e) => {{ print("ERR " + e); return 1; }},
    }};
    print("utc off=" + to_string(tz.offset_at(utc, verano)) + " " + tz.abbrev_at(utc, verano));
    // Otro hemisferio de reglas: Nueva York (EST -5 / EDT -4).
    let ny = match (tz.load_file("{fixtures}/America_New_York.tzif")) {{
        Result.Ok(z) => z,
        Result.Err(e) => {{ print("ERR " + e); return 1; }},
    }};
    describe(ny, invierno);
    describe(ny, verano);
    // M85b: más allá de la última transición explícita rigen las reglas del FOOTER
    // (TZ-string). Invierno/verano 2100 y los dos lados del cambio de primavera
    // (último domingo de marzo de 2100 = día 28; 01:00 UTC).
    describe(mad, 4103697600000);
    describe(mad, 4119336000000);
    describe(ny, 4103697600000);
    describe(ny, 4119336000000);
    describe(mad, 4109878800000 - 1000);
    describe(mad, 4109878800000);
    // Y LocalResult sigue funcionando en territorio del footer (gap y solape de 2100).
    match (tz.to_utc(mad, civil(2100, 3, 28, 2, 30))) {{
        tz.LocalResult.Single(ms) => print("FALLO: single"),
        tz.LocalResult.Ambiguous(a, b) => print("FALLO: ambiguous"),
        tz.LocalResult.Gap => print("gap 2100"),
    }}
    match (tz.to_utc(mad, civil(2100, 10, 31, 2, 30))) {{
        tz.LocalResult.Single(ms) => print("FALLO: single"),
        tz.LocalResult.Ambiguous(a, b) => print("ambiguous 2100 delta_min=" + to_string((b - a) / 60000)),
        tz.LocalResult.Gap => print("FALLO: gap"),
    }}
    // Errores como valores: archivo inexistente y nombre inválido.
    match (tz.load_file("{fixtures}/NoExiste.tzif")) {{
        Result.Ok(z) => print("FALLO: cargó"),
        Result.Err(e) => print("err file ok"),
    }}
    match (tz.load("../etc/passwd")) {{
        Result.Ok(z) => print("FALLO: cargó"),
        Result.Err(e) => print("err name ok"),
    }}
    0
}}
"#,
        fixtures = fixtures.display()
    );
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

const ESPERADO: &str = "2026-01-15T12:00:00Z CET dst=false off_h=1\n\
2026-07-15T12:00:00Z CEST dst=true off_h=2\n\
2026-07-15T12:00:00Z\n\
gap\n\
ambiguous delta_min=60 antes=CEST despues=CET\n\
2026-03-29T01:59:59Z CET dst=false off_h=1\n\
2026-03-29T03:00:00Z CEST dst=true off_h=2\n\
utc off=0 UTC\n\
2026-01-15T06:00:00Z EST dst=false off_h=-5\n\
2026-07-15T06:00:00Z EDT dst=true off_h=-4\n\
2100-01-15T13:00:00Z CET dst=false off_h=1\n\
2100-07-15T14:00:00Z CEST dst=true off_h=2\n\
2100-01-15T07:00:00Z EST dst=false off_h=-5\n\
2100-07-15T08:00:00Z EDT dst=true off_h=-4\n\
2100-03-28T01:59:59Z CET dst=false off_h=1\n\
2100-03-28T03:00:00Z CEST dst=true off_h=2\n\
gap 2100\n\
ambiguous 2100 delta_min=60\n\
err file ok\n\
err name ok\n";

fn run(app: &std::path::Path, flags: &[&str]) -> (String, i32) {
    let out = Command::new(BIN)
        .args(flags)
        .arg(app.join("src/main.ray"))
        .current_dir(app)
        .output()
        .expect("ejecuta raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn tz_hora_local_ambos_engines() {
    let base = std::env::temp_dir().join("ray_tz_cli");
    let _ = std::fs::remove_dir_all(&base);
    let app = project(&base);
    let (o_in, c_in) = run(&app, &[]);
    let (o_vm, c_vm) = run(&app, &["--vm"]);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, o_vm, "ambos engines match");
    assert_eq!(o_in, ESPERADO, "output expected_val");
}
