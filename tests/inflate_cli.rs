//! Pruebas de INFLATE / gunzip (`examples/web/inflate.ray`, M20.10). Cómputo puro determinista → se
//! corre `examples/web/inflate_demo.ray` por ambos motores y se compara con lo esperado. Los blobs gzip
//! de entrada los generó Python (`gzip.compress`) y cubren los TRES tipos de bloque DEFLATE: almacenado,
//! Huffman fijo y Huffman dinámico. CRC-32 contra el vector estándar.

use std::process::Command;

const ESPERADO: &[&str] = &[
    "raylang es un lenguaje de aprendizaje. raylang es divertido. raylang raylang raylang!", // fijo
    "raylang es un lenguaje de aprendizaje. raylang es divertido. raylang raylang raylang!", // stored
    "516",                                       // longitud del texto del bloque dinámico
    "El veloz murcielago hindu comia feliz car", // primeros 41 chars del dinámico
    "3421780262",                                // crc32("123456789") = 0xcbf43926
    // M64.1 — robustez: input corrupto = Err con mensaje (antes: crash por índice fuera de rango).
    "vacio: err: stream DEFLATE truncado",
    "un-octeto: err: stream DEFLATE truncado",
    "stored-truncado: err: bloque almacenado truncado (declara más octetos de los que hay)",
    "hlit-excesivo: err: demasiados códigos en el bloque dinámico",
    "fname-sin-nul: err: cabecera gzip truncada (FNAME sin terminar)",
    "gzip-truncado: err: gzip demasiado corto",
    "fextra-gigante: err: cabecera gzip truncada (FEXTRA)",
    "zlib-fdict: err: zlib con diccionario preestablecido (FDICT) no soportado",
    // M64.2 — tope de salida (anti-bomba): gunzip_limit con tope menor que la salida = Err;
    // con tope justo, el round-trip pasa.
    "limite-salida: err: salida descomprimida excede el límite (posible bomba de descompresión)",
    "roundtrip: ok (32 octetos)",
];

fn run(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/inflate_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta inflate_demo.ray");
    assert!(
        out.status.success(),
        "inflate_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn inflate_gzip_interpreter() {
    assert_eq!(run(&[]), ESPERADO);
}

#[test]
fn inflate_gzip_vm() {
    assert_eq!(run(&["--vm"]), ESPERADO);
}
