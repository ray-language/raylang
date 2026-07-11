//! Pruebas de JWT (HS256) + UUID v4 (`examples/web/{jwt,uuid}.ray`, M20.3).
//! El JWT con secreto/claims fijos es **reproducible** (verificado byte a byte contra una
//! implementación de referencia) → se fija. El UUID es aleatorio → se valida por su FORMA (el propio
//! `is_uuid_v4` en raylang) y por que dos generados difieran, lo que da una salida booleana
//! determinista. Se corre `examples/web/jwt_demo.ray` por ambos motores.

use std::process::Command;

/// Las líneas fijas que `jwt_demo.ray` imprime (en orden). Las dos últimas (UUID) deben ser "true".
const ESPERADO: &[&str] = &[
    "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkFkYSIsImFkbWluIjp0cnVlfQ.lbFHAPAyxfbCKv0qbJb1ukylm0ZOW_skQJhpnZkZLcM",
    "{\"sub\":\"1234567890\",\"name\":\"Ada\",\"admin\":true}",
    "firma inválida",
    "tamper detectado",
    "mal formado detectado",
    "alg:none rechazado", // M74: la validación de `alg` rechaza el ataque de confusión de algoritmo
    "true", // is_uuid_v4(u1) && is_uuid_v4(u2)
    "true", // u1 != u2
    // M57.3: UUID v7 (ordenable por tiempo)
    "true", // is_uuid_v7
    "true", // prefijo del vector de la RFC 9562 (timestamp big-endian en hex)
    "true", // orden lexicográfico = orden temporal (ms consecutivos)
];

fn correr(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/jwt_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta jwt_demo.ray");
    assert!(
        out.status.success(),
        "jwt_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn jwt_y_uuid_interprete() {
    assert_eq!(correr(&[]), ESPERADO);
}

#[test]
fn jwt_y_uuid_vm() {
    assert_eq!(correr(&["--vm"]), ESPERADO);
}
