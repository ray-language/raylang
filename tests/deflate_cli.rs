//! Pruebas del encoder DEFLATE (`examples/web/deflate.ray`, cierre de M20). Dos niveles de verificación:
//! (1) **round-trip interno** determinista — comprime y descomprime con el propio `inflate.ray`
//! (deflate_raw/gzip/zlib ↔ inflate_raw/gunzip/zlib_inflate), por ambos motores, salida booleana;
//! (2) **compatibilidad estándar** — el gzip que produce raylang lo descomprime **Python** (`gzip`),
//! lo que prueba que el stream DEFLATE es válido, no solo auto-consistente.

use std::process::Command;

const ESPERADO: &[&str] = &[
    "true", // deflate_raw → inflate_raw
    "true", // comprime (len < original)
    "true", // gzip_compress → gunzip (con verificación CRC-32)
    "true", // zlib_compress → zlib_inflate
];

fn run(flags: &[&str], extra_arg: Option<&str>) -> (Vec<String>, bool) {
    let demo = format!("{}/examples/web/deflate_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.args(flags).arg(&demo);
    if let Some(a) = extra_arg {
        cmd.arg(a);
    }
    let out = cmd.output().expect("ejecuta deflate_demo.ray");
    let lines = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    (lines, out.status.success())
}

#[test]
fn deflate_round_trip_interpreter() {
    let (lines, ok) = run(&[], None);
    assert!(ok, "deflate_demo falló");
    assert_eq!(lines, ESPERADO);
}

#[test]
fn deflate_round_trip_vm() {
    let (lines, ok) = run(&["--vm"], None);
    assert!(ok, "deflate_demo falló");
    assert_eq!(lines, ESPERADO);
}

/// El gzip que produce raylang debe poder descomprimirlo Python (compatibilidad estándar). Se escribe
/// a un temporal (vía el argumento del demo) y se descomprime con `python3 -c "gzip.decompress(...)"`.
#[test]
fn gzip_de_raylang_lo_descomprime_python() {
    // Si no hay python3, se omite (no es un fallo del proyecto).
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 no disponible: se omite la verificación de compatibilidad");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("ray_deflate_{}.gz", std::process::id()));
    let tmp_str = tmp.to_str().unwrap();
    let (_, ok) = run(&["--vm"], Some(tmp_str));
    assert!(ok, "deflate_demo (con path) falló");

    let py = Command::new("python3")
        .arg("-c")
        .arg(format!(
            "import gzip,sys; sys.stdout.write(gzip.decompress(open(r'{tmp_str}','rb').read()).decode())"
        ))
        .output()
        .expect("ejecuta python3");
    let _ = std::fs::remove_file(&tmp);
    assert!(py.status.success(), "python gzip.decompress falló: {}", String::from_utf8_lossy(&py.stderr));
    let text = String::from_utf8_lossy(&py.stdout);
    assert!(text.starts_with("raylang raylang raylang"), "descompresión Python inesperada: {text}");
    assert!(text.contains("comprimir y descomprimir"), "contenido incompleto: {text}");
}
