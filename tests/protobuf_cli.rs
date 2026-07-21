//! Pruebas del códec protobuf + framing gRPC (`examples/web/protobuf.ray`, M25). Cómputo puro
//! determinista → golden por ambos motores (incl. los vectores canónicos de la doc de protobuf:
//! campo 1 varint 150 → `08 96 01`) + validación de los octetos con un decodificador protobuf en
//! **Python sin dependencias** (prueba de compatibilidad con el wire format real).

use std::process::Command;

const EXPECTED: &[&str] = &[
    "089601120774657374696e6718ac02",             // protobuf: f1=150, f2="testing", f3=300
    "field1=150",
    "field2=testing",
    "field3=300",
    "000000000f089601120774657374696e6718ac02",   // gRPC frame: 00 + len(15) + protobuf
    "roundtrip=true",
];

fn run(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/protobuf_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta protobuf_demo.ray");
    assert!(
        out.status.success(),
        "protobuf_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn protobuf_grpc_interpreter() {
    assert_eq!(run(&[]), EXPECTED);
}

#[test]
fn protobuf_grpc_vm() {
    assert_eq!(run(&["--vm"]), EXPECTED);
}

/// El protobuf que produce raylang debe decodificarlo un parser del wire format en Python (sin deps).
#[test]
fn wire_format_compatible_con_python() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 no disponible: se omite la validación");
        return;
    }
    let hex = &run(&[])[0];
    let validator = r#"
import sys
data = bytes.fromhex(sys.argv[1])
def rv(d,p):
    r=0;s=0
    while True:
        b=d[p];p+=1;r|=(b&0x7f)<<s
        if not (b&0x80): return r,p
        s+=7
p=0;f={}
while p<len(data):
    tag,p=rv(data,p);num=tag>>3;wire=tag&7
    if wire==0: v,p=rv(data,p);f[num]=v
    elif wire==2:
        l,p=rv(data,p);f[num]=data[p:p+l];p+=l
    else: raise SystemExit('wire '+str(wire))
assert f[1]==150, f[1]
assert f[2]==b'testing', f[2]
assert f[3]==300, f[3]
print('WIRE OK')
"#;
    let py = Command::new("python3")
        .arg("-c")
        .arg(validator)
        .arg(hex)
        .output()
        .expect("ejecuta python3");
    assert!(
        py.status.success(),
        "validación del wire format falló: {}",
        String::from_utf8_lossy(&py.stderr)
    );
    assert!(String::from_utf8_lossy(&py.stdout).contains("WIRE OK"));
}

/// M59.4 — un varint negativo debe PANICAR (antes emitía octetos corruptos en silencio: el
/// bucle LEB128 no entraba con negativos y salía un solo octeto mal).
#[test]
fn negative_varint_panics_instead_of_corrupting() {
    let mut dir = std::env::temp_dir();
    dir.push("ray_pb_negativo");
    std::fs::create_dir_all(&dir).expect("crea dir");
    let driver = dir.join("main.ray");
    std::fs::write(
        &driver,
        r#"
from std/protobuf import writer, write_varint;
fn main() {
    let w = writer();
    w.write_varint(1, 0 - 1);
}
"#,
    )
    .expect("escribe driver");
    for flags in [&[][..], &["--vm"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
            .args(flags)
            .arg(&driver)
            .output()
            .expect("ejecuta driver");
        assert!(!out.status.success(), "debería fallar (flags {flags:?})");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("protobuf: varint negativo no soportado"),
            "stderr inesperado (flags {flags:?}): {err}"
        );
    }
}
