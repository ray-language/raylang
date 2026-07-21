//! Pruebas de los histogramas CON labels (`examples/web/metrics.ray`, M21.4). Cada conjunto de labels
//! tiene su propia familia de series. Salida determinista → golden por ambos motores + validación
//! estructural con Python: cumulatividad **por conjunto de labels** (agrupando por los labels sin `le`)
//! y `+Inf == _count` por grupo.

use std::process::Command;

const ESPERADO: &[&str] = &[
    "# HELP rpc_duracion_segundos Duracion de RPC por metodo",
    "# TYPE rpc_duracion_segundos histogram",
    r#"rpc_duracion_segundos_bucket{le="0.1",method="get"} 1"#,
    r#"rpc_duracion_segundos_bucket{le="0.5",method="get"} 2"#,
    r#"rpc_duracion_segundos_bucket{le="1",method="get"} 3"#,
    r#"rpc_duracion_segundos_bucket{le="+Inf",method="get"} 3"#,
    r#"rpc_duracion_segundos_sum{method="get"} 1.05"#,
    r#"rpc_duracion_segundos_count{method="get"} 3"#,
    r#"rpc_duracion_segundos_bucket{le="0.1",method="set"} 0"#,
    r#"rpc_duracion_segundos_bucket{le="0.5",method="set"} 0"#,
    r#"rpc_duracion_segundos_bucket{le="1",method="set"} 0"#,
    r#"rpc_duracion_segundos_bucket{le="+Inf",method="set"} 1"#,
    r#"rpc_duracion_segundos_sum{method="set"} 1.5"#,
    r#"rpc_duracion_segundos_count{method="set"} 1"#,
];

fn run(flags: &[&str]) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/metrics_labels_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta metrics_labels_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout)
        .trim_end()
        .lines()
        .map(|l| l.to_string())
        .collect();
    (lines, out.status.success())
}

#[test]
fn histogram_with_labels_interpreter() {
    let (lines, ok) = run(&[]);
    assert!(ok, "metrics_labels_demo falló");
    assert_eq!(lines, ESPERADO);
}

#[test]
fn histogram_with_labels_vm() {
    let (lines, ok) = run(&["--vm"]);
    assert!(ok, "metrics_labels_demo falló");
    assert_eq!(lines, ESPERADO);
}

/// Validación estructural con Python: por cada conjunto de labels (sin `le`), los buckets son
/// cumulativos y el bucket +Inf iguala al _count de ese mismo conjunto.
#[test]
fn cumulativity_by_label_set() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 no disponible: se omite la validación");
        return;
    }
    let (lines, ok) = run(&[]);
    assert!(ok);
    let text = lines.join("\n");

    let validator = r#"
import sys, re, collections
text = sys.stdin.read()
# buckets[grupo] = [(le, valor)]; counts[grupo] = valor
buckets = collections.defaultdict(list)
counts = {}
for line in text.splitlines():
    if line.startswith('#') or not line.strip():
        continue
    m = re.match(r'^(\w+?)(_bucket|_sum|_count)?(\{[^}]*\})?\s+(\S+)$', line)
    assert m, f'línea inválida: {line!r}'
    suf, labels, val = m.group(2) or '', m.group(3) or '', float(m.group(4))
    pairs = dict(re.findall(r'(\w+)="([^"]*)"', labels))
    le = pairs.pop('le', None)
    grupo = tuple(sorted(pairs.items()))  # labels sin le → identifica el conjunto
    if suf == '_bucket':
        buckets[grupo].append((le, val))
    elif suf == '_count':
        counts[grupo] = val

assert buckets, 'no se encontraron buckets'
for grupo, bs in buckets.items():
    finitos = sorted((float(le), v) for le, v in bs if le != '+Inf')
    inf = [v for le, v in bs if le == '+Inf'][0]
    prev = 0
    for le, v in finitos:
        assert v >= prev, f'no cumulativo en {grupo} le={le}'
        prev = v
    assert inf == counts[grupo], f'+Inf != _count en {grupo}'
print('CUMULATIVIDAD OK', len(buckets), 'grupos')
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
        "validación falló: {}",
        String::from_utf8_lossy(&py.stderr)
    );
    let output = String::from_utf8_lossy(&py.stdout);
    assert!(output.contains("CUMULATIVIDAD OK 2 grupos"), "output: {output}");
}
