//! Pruebas de las métricas Prometheus (`examples/web/metrics.ray`, M21.2). Salida determinista → se
//! corre `examples/web/metrics_demo.ray` por ambos motores y se compara con el formato de exposición
//! esperado. Además se valida estructuralmente con Python (sin dependencias): formato de las series y
//! **cumulatividad** de los buckets del histograma (le ascendente ⇒ cuentas no decrecientes; +Inf == _count).

use std::process::Command;

const ESPERADO: &[&str] = &[
    "# HELP http_requests_total Total de peticiones HTTP",
    "# TYPE http_requests_total counter",
    r#"http_requests_total{code="200",method="GET"} 2"#,
    r#"http_requests_total{code="500",method="POST"} 1"#,
    "# HELP temperatura_celsius Temperatura actual",
    "# TYPE temperatura_celsius gauge",
    "temperatura_celsius 21.5",
    "# HELP http_duracion_segundos Duracion de las peticiones",
    "# TYPE http_duracion_segundos histogram",
    r#"http_duracion_segundos_bucket{le="0.1"} 1"#,
    r#"http_duracion_segundos_bucket{le="0.5"} 2"#,
    r#"http_duracion_segundos_bucket{le="1"} 3"#,
    r#"http_duracion_segundos_bucket{le="+Inf"} 4"#,
    "http_duracion_segundos_sum 3.15",
    "http_duracion_segundos_count 4",
    // M70 — el texto de # HELP se escapa (\\ y \n, como exige el formato de exposición):
    // antes un salto de línea crudo en el help rompía el scrape entero.
    r#"# HELP raro_total linea 1\nlinea 2 con \\ barra"#,
    "# TYPE raro_total counter",
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/metrics_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta metrics_demo.ray");
    // `render` ya termina cada línea en \n y `print` añade otro → se recorta el \n final sobrante.
    let lines = String::from_utf8_lossy(&out.stdout)
        .trim_end()
        .lines()
        .map(|l| l.to_string())
        .collect();
    (lines, out.status.success())
}

#[test]
fn metrics_exposure_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "metrics_demo falló");
    assert_eq!(lines, ESPERADO);
}

#[test]
fn metrics_exposure_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "metrics_demo falló");
    assert_eq!(lines, ESPERADO);
}

/// Validación estructural con Python plano (sin prometheus_client): cada línea de serie es
/// `nombre[{labels}] valor`, HELP/TYPE preceden a las series, y los buckets del histograma son
/// cumulativos (cuentas no decrecientes por `le` ascendente, y +Inf == _count).
#[test]
fn exposure_format_is_valid() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 no disponible: se omite la validación de formato");
        return;
    }
    let (lines, ok) = run(&[]);
    assert!(ok);
    let text = lines.join("\n");

    let validator = r#"
import sys, re
text = sys.stdin.read()
types = {}
samples = {}
for line in text.splitlines():
    if not line.strip():
        continue
    if line.startswith('# HELP'):
        continue
    if line.startswith('# TYPE'):
        _, _, name, kind = line.split()
        types[name] = kind
        continue
    # name[{labels}] valor
    m = re.match(r'^([a-zA-Z_][a-zA-Z0-9_]*)(\{[^}]*\})?\s+(\S+)$', line)
    assert m, f'línea inválida: {line!r}'
    name, labels, val = m.group(1), m.group(2) or '', m.group(3)
    float(val)  # el valor must ser numérico
    samples.setdefault(name, []).append((labels, float(val)))

assert types.get('http_requests_total') == 'counter'
assert types.get('temperatura_celsius') == 'gauge'
assert types.get('http_duracion_segundos') == 'histogram'

# Cumulatividad del histograma: buckets por le ascendente con cuentas no decrecientes; +Inf == count.
buckets = []
inf = None
count = None
for labels, val in samples.get('http_duracion_segundos_bucket', []):
    le = re.search(r'le="([^"]+)"', labels).group(1)
    if le == '+Inf':
        inf = val
    else:
        buckets.append((float(le), val))
for name, lst in samples.items():
    if name == 'http_duracion_segundos_count':
        count = lst[0][1]
buckets.sort()
prev = 0
for le, v in buckets:
    assert v >= prev, f'bucket no cumulativo en le={le}'
    prev = v
assert inf is not None and count is not None and inf == count, 'el bucket +Inf must igualar a _count'
print('FORMATO OK')
"#;

    let py = Command::new("python3")
        .arg("-c")
        .arg(validator)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(text.as_bytes())?;
            child.wait_with_output()
        })
        .expect("ejecuta python3");
    assert!(
        py.status.success(),
        "validación de formato falló: {}",
        String::from_utf8_lossy(&py.stderr)
    );
    assert!(String::from_utf8_lossy(&py.stdout).contains("FORMATO OK"));
}

/// M70 — chequeo de tipo: `set` sobre un counter y `observe` sobre un counter panican con
/// mensaje claro (antes creaban una serie espuria que corrompía la exposición en silencio).
#[test]
fn wrong_type_panics() {
    let src = r#"
from metrics import registry, register_counter, set, no_labels;
fn main() {
    let reg = registry();
    register_counter(reg, "c_total", "un counter");
    set(reg, "c_total", no_labels(), 5.0);
}
"#;
    let dir = std::env::temp_dir().join("ray_metrics_m70");
    std::fs::create_dir_all(&dir).unwrap();
    // El import `from metrics` resuelve junto al archivo → escribir el demo al lado del módulo.
    let path = format!("{}/examples/web/m70_ty_tmp.ray", env!("CARGO_MANIFEST_DIR"));
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang")).arg("--vm").arg(&path).output().unwrap();
    std::fs::remove_file(&path).ok();
    assert!(!out.status.success(), "set about un counter must panicar");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no es un gauge"), "mensaje claro de type equivocado\n{err}");
}
