//! M86 — `packages/cron`: expresiones cron + `next_after` (v1 solo UTC). El núcleo es PURO
//! (parse + siguiente disparo) → goldens deterministas sobre instantes fijos, por ambos
//! motores. Cubre `*`/listas/rangos/pasos, los alias `@…`, el quirk DOM/DOW de vixie
//! (ambos restringidos = OR), el año bisiesto y las expresiones imposibles (Err).
//! El runner (`run`) son 15 líneas sobre next_after + time.sleep (cooperativo, M57.2);
//! su e2e sería un test de minutos de reloj → no se prueba aquí (documentado).

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

fn project(base: &std::path::Path) -> std::path::PathBuf {
    let cron = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/cron");
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ncron = \"path:{}\"\n",
            cron.display()
        ),
    )
    .unwrap();
    let main = r#"import cron/cron;
import std/time;

// Imprime los `n` siguientes disparos de `expr` tras `desde` (epoch-ms), en ISO.
fn secuencia(expr: string, from: int, n: int) {
    match (cron.parse(expr)) {
        Result.Ok(s) => {
            var t = from;
            var i = 0;
            var line = expr + " →";
            var ok = true;
            while (i < n && ok) {
                match (cron.next_after(s, t)) {
                    Result.Ok(next) => {
                        line = line + " " + time.to_iso8601(time.from_epoch_millis(next));
                        t = next;
                    },
                    Result.Err(e) => {
                        line = line + " ERR:" + e;
                        ok = false;
                    },
                }
                i = i + 1;
            }
            print(line);
        },
        Result.Err(e) => print(expr + " → parse ERR: " + e),
    }
}

fn main() -> int {
    // 2026-07-11T10:35:30Z (sábado).
    let base = 1783766130000;
    secuencia("*/15 * * * *", base, 3);
    secuencia("0 9 * * 1-5", base, 3);        // 9:00 laborables (lun-vie)
    secuencia("30 2 1 * *", base, 2);         // día 1 de cada mes
    secuencia("@daily", base, 2);
    secuencia("@weekly", base, 2);
    secuencia("0 0 29 2 *", base, 1);         // 29-feb: el siguiente bisiesto (2028)
    secuencia("0 12 13 * 5", base, 3);        // quirk vixie: día 13 O viernes (OR)
    secuencia("5,20 8-10/2 * * *", base, 4);  // lista + rango con paso
    secuencia("0 0 30 2 *", base, 1);         // imposible → Err
    // Errores de parse.
    secuencia("* * * *", base, 1);            // 4 campos
    secuencia("61 * * * *", base, 1);         // fuera de rango
    secuencia("* * * * 8-9", base, 1);        // dow fuera de rango
    0
}
"#;
    std::fs::write(app.join("src/main.ray"), main).unwrap();
    app
}

const ESPERADO: &str = "*/15 * * * * → 2026-07-11T10:45:00Z 2026-07-11T11:00:00Z 2026-07-11T11:15:00Z\n\
0 9 * * 1-5 → 2026-07-13T09:00:00Z 2026-07-14T09:00:00Z 2026-07-15T09:00:00Z\n\
30 2 1 * * → 2026-08-01T02:30:00Z 2026-09-01T02:30:00Z\n\
@daily → 2026-07-12T00:00:00Z 2026-07-13T00:00:00Z\n\
@weekly → 2026-07-12T00:00:00Z 2026-07-19T00:00:00Z\n\
0 0 29 2 * → 2028-02-29T00:00:00Z\n\
0 12 13 * 5 → 2026-07-13T12:00:00Z 2026-07-17T12:00:00Z 2026-07-24T12:00:00Z\n\
5,20 8-10/2 * * * → 2026-07-12T08:05:00Z 2026-07-12T08:20:00Z 2026-07-12T10:05:00Z 2026-07-12T10:20:00Z\n\
0 0 30 2 * → ERR:cron: la expresión no casa con ninguna fecha (¿día imposible?)\n\
* * * * → parse ERR: cron: se esperaban 5 fields (min hora dom mes dow), hay 4\n\
61 * * * * → parse ERR: cron: 'minuto' outside de range [0-59] ('61')\n\
* * * * 8-9 → parse ERR: cron: 'día de la semana' outside de range [0-7] ('8-9')\n";

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
fn cron_next_after_ambos_engines() {
    let base = std::env::temp_dir().join("ray_cron_cli");
    let _ = std::fs::remove_dir_all(&base);
    let app = project(&base);
    let (o_in, c_in) = run(&app, &[]);
    let (o_vm, c_vm) = run(&app, &["--vm"]);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, o_vm, "ambos engines match");
    assert_eq!(o_in, ESPERADO, "output expected_val");
}

// ─────────────────────────────────────────────────────────────────────────────
// M86b — cron en HORA LOCAL (cron/local sobre packages/tz, fixture de Madrid).
// Política DST: hueco de primavera → dispara al acabar el hueco; solape de otoño →
// solo la primera ocurrencia.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cron_local_dst_ambos_engines() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let (cron, tz) = (repo.join("packages/cron"), repo.join("packages/tz"));
    let fixture = tz.join("fixtures/Europe_Madrid.tzif");
    let base = std::env::temp_dir().join("ray_cron_local_cli");
    let _ = std::fs::remove_dir_all(&base);
    let app = base.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ncron = \"path:{}\"\ntz = \"path:{}\"\n",
            cron.display(),
            tz.display()
        ),
    )
    .unwrap();
    let main = format!(
        r#"import cron/cron;
import cron/local;
import tz/tz;
import std/time;

fn secuencia(z: tz.Zone, expr: string, from: int, n: int) {{
    match (cron.parse(expr)) {{
        Result.Ok(s) => {{
            var t = from;
            var i = 0;
            var line = expr + " →";
            while (i < n) {{
                match (local.next_after_in(s, z, t)) {{
                    Result.Ok(next) => {{
                        line = line + " " + time.to_iso8601(time.from_epoch_millis(next)) + "(" + tz.abbrev_at(z, next) + ")";
                        t = next;
                    }},
                    Result.Err(e) => {{
                        line = line + " ERR:" + e;
                        i = n;
                    }},
                }}
                i = i + 1;
            }}
            print(line);
        }},
        Result.Err(e) => print("parse ERR: " + e),
    }}
}}

fn main() -> int {{
    let mad = match (tz.load_file("{fixture}")) {{
        Result.Ok(z) => z,
        Result.Err(e) => {{ print("ERR " + e); return 1; }},
    }};
    // Día normal: 02:30 local CEST = 00:30 UTC.
    secuencia(mad, "30 2 * * *", 1783684800000, 1);
    // El HUECO de primavera (29-03-2026: 02:30 no existe) → dispara al acabar el hueco
    // (03:00 CEST = 01:00 UTC); el día siguiente, normal (00:30 UTC).
    secuencia(mad, "30 2 * * *", 1774699200000, 2);
    // El SOLAPE de otoño (25-10-2026: 02:30 ocurre dos veces) → solo la PRIMERA
    // (02:30 CEST = 00:30 UTC); el día siguiente ya en CET (01:30 UTC).
    secuencia(mad, "30 2 * * *", 1792843200000, 2);
    0
}}
"#,
        fixture = fixture.display()
    );
    std::fs::write(app.join("src/main.ray"), main).unwrap();

    let expected = "30 2 * * * → 2026-07-11T00:30:00Z(CEST)\n\
30 2 * * * → 2026-03-29T01:00:00Z(CEST) 2026-03-30T00:30:00Z(CEST)\n\
30 2 * * * → 2026-10-25T00:30:00Z(CEST) 2026-10-26T01:30:00Z(CET)\n";

    let (o_in, c_in) = run(&app, &[]);
    let (o_vm, c_vm) = run(&app, &["--vm"]);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, o_vm, "ambos engines match");
    assert_eq!(o_in, expected, "output expected_val");
}
