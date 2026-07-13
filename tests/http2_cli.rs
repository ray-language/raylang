//! Pruebas de HPACK (RFC 7541) + framing HTTP/2 (RFC 7540) (M26). HPACK se valida contra los **vectores
//! oficiales del RFC 7541 §C.3** (los tres bloques de petición con tabla dinámica compartida — referencias
//! `be`/`bf` a la tabla dinámica incluidas), que son autoritativos. Más round-trip de decodificación y de
//! un HEADERS frame. Golden por ambos motores.

use std::process::Command;

const ESPERADO: &[&str] = &[
    // RFC 7541 §C.3.1 / C.3.2 / C.3.3 (sin Huffman): los tres bloques de petición.
    "828684410f7777772e6578616d706c652e636f6d",
    "828684be58086e6f2d6361636865",
    "828785bf400a637573746f6d2d6b65790c637573746f6d2d76616c7565",
    // Decodificación de C.3.1 → cabeceras originales.
    ":method: GET",
    ":scheme: http",
    ":path: /",
    ":authority: www.example.com",
    // Round-trip de un HEADERS frame (tipo 1, stream 1, flags END_HEADERS|END_STREAM = 5).
    "HEADERS type=1 stream=1 flags=5 blocklen=2",
    // La connection preface del cliente: "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".
    "505249202a20485454502f322e300d0a0d0a534d0d0a0d0a",
];

fn correr(flags: &[&str]) -> Vec<String> {
    let demo = format!("{}/examples/web/http2_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .args(flags)
        .arg(&demo)
        .output()
        .expect("ejecuta http2_demo.ray");
    assert!(
        out.status.success(),
        "http2_demo falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn hpack_y_framing_interprete() {
    assert_eq!(correr(&[]), ESPERADO);
}

#[test]
fn hpack_y_framing_vm() {
    assert_eq!(correr(&["--vm"]), ESPERADO);
}

// ── Huffman de HPACK (M91.2): vectores oficiales C.4 (peticiones) y C.6 (respuestas) ──

const DRIVER_HUFFMAN: &str = r#"
import std/hex;
from hpack import HEntry, Hpack, new_hpack, decode;

fn dump(d: Hpack, hexs: string) {
    match (hex.hex_decode(hexs)) {
        Result.Err(e) => print("hex err: " + e),
        Result.Ok(b) => {
            match (decode(d, b)) {
                Result.Err(e) => print("err: " + e),
                Result.Ok(hs) => {
                    var i = 0;
                    while (i < hs.len()) {
                        print(hs[i].name + ": " + hs[i].value);
                        i = i + 1;
                    }
                },
            }
        },
    }
    print("--");
}

fn main() -> int {
    // C.4: las tres peticiones con Huffman, tabla dinámica COMPARTIDA (be/bf referencian).
    let d = new_hpack();
    dump(d, "828684418cf1e3c2e5f23a6ba0ab90f4ff");
    dump(d, "828684be5886a8eb10649cbf");
    dump(d, "828785bf408825a849e95ba97d7f8925a849e95bb8e8b4bf");
    // C.6: las tres respuestas con Huffman, tabla máxima de 256 octetos (fuerza evicciones).
    let r = new_hpack();
    r.max_size = 256;
    dump(r, "488264025885aec3771a4b6196d07abe941054d444a8200595040b8166e082a62d1bff6e919d29ad171863c78f0b97c8e9ae82ae43d3");
    dump(r, "4883640effc1c0bf");
    dump(r, "88c16196d07abe941054d444a8200595040b8166e084a62d1bffc05a839bd9ab77ad94e7821dd7f2e6c7b335dfdfcd5b3960d5af27087f3672c1ab270fb5291f9587316065c003ed4ee5b1063d5007");
    // Relleno inválido: valor Huffman de 1 octeto 0x00 ('0' + relleno de ceros) → error claro.
    let m = new_hpack();
    dump(m, "418100");
    0
}
"#;

const ESPERADO_HUFFMAN: &[&str] = &[
    ":method: GET", ":scheme: http", ":path: /", ":authority: www.example.com", "--",
    ":method: GET", ":scheme: http", ":path: /", ":authority: www.example.com",
    "cache-control: no-cache", "--",
    ":method: GET", ":scheme: https", ":path: /index.html", ":authority: www.example.com",
    "custom-key: custom-value", "--",
    ":status: 302", "cache-control: private", "date: Mon, 21 Oct 2013 20:13:21 GMT",
    "location: https://www.example.com", "--",
    ":status: 307", "cache-control: private", "date: Mon, 21 Oct 2013 20:13:21 GMT",
    "location: https://www.example.com", "--",
    ":status: 200", "cache-control: private", "date: Mon, 21 Oct 2013 20:13:22 GMT",
    "location: https://www.example.com", "content-encoding: gzip",
    "set-cookie: foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1", "--",
    "err: HPACK: relleno Huffman inválido (debe ser el prefijo del EOS)", "--",
];

#[test]
fn hpack_huffman_vectores_c4_y_c6() {
    let mut dir = std::env::temp_dir();
    dir.push("ray_hpack_huffman");
    std::fs::create_dir_all(&dir).expect("crea dir");
    let lib = format!("{}/examples/web/hpack.ray", env!("CARGO_MANIFEST_DIR"));
    std::fs::copy(&lib, dir.join("hpack.ray")).expect("copia hpack.ray");
    let driver = dir.join("main.ray");
    std::fs::write(&driver, DRIVER_HUFFMAN).expect("escribe driver");
    for vm in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
        if vm {
            cmd.arg("--vm");
        }
        let out = cmd.arg(&driver).output().expect("lanza raylang");
        assert!(out.status.success(), "falló (vm={vm}): {}", String::from_utf8_lossy(&out.stderr));
        let lineas: Vec<String> =
            String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
        assert_eq!(lineas, ESPERADO_HUFFMAN, "vectores C.4/C.6 (vm={vm})");
    }
}
