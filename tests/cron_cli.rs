//! M86 — `packages/cron`: expresiones cron + `next_after` (v1 solo UTC). El núcleo es PURO
//! (parse + siguiente disparo) → goldens deterministas sobre instantes fijos, por ambos
//! motores. Cubre `*`/listas/rangos/pasos, los alias `@…`, el quirk DOM/DOW de vixie
//! (ambos restringidos = OR), el año bisiesto y las expresiones imposibles (Err).
//! El runner (`run`) son 15 líneas sobre next_after + time.sleep (cooperativo, M57.2);
//! su e2e sería un test de minutos de reloj → no se prueba aquí (documentado).

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

fn proyecto(base: &std::path::Path) -> std::path::PathBuf {
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
fn secuencia(expr: string, desde: int, n: int) {
    match (cron.parse(expr)) {
        Result.Ok(s) => {
            var t = desde;
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
* * * * → parse ERR: cron: se esperaban 5 campos (min hora dom mes dow), hay 4\n\
61 * * * * → parse ERR: cron: 'minuto' fuera de rango [0-59] ('61')\n\
* * * * 8-9 → parse ERR: cron: 'día de la semana' fuera de rango [0-7] ('8-9')\n";

fn correr(app: &std::path::Path, flags: &[&str]) -> (String, i32) {
    let out = Command::new(BIN)
        .args(flags)
        .arg(app.join("src/main.ray"))
        .current_dir(app)
        .output()
        .expect("ejecuta raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn cron_next_after_ambos_motores() {
    let base = std::env::temp_dir().join("ray_cron_cli");
    let _ = std::fs::remove_dir_all(&base);
    let app = proyecto(&base);
    let (o_in, c_in) = correr(&app, &[]);
    let (o_vm, c_vm) = correr(&app, &["--vm"]);
    assert_eq!(c_in, 0, "intérprete sale 0\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0\n{o_vm}");
    assert_eq!(o_in, o_vm, "ambos motores coinciden");
    assert_eq!(o_in, ESPERADO, "salida esperada");
}
