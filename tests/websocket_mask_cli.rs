//! M292 (IDEAS §96 #5): la clave de enmascarado de las tramas cliente→servidor de `net/websocket`
//! sale del CSPRNG (`crypto.random_bytes`), no de `std/random`. Dos tramas del MISMO mensaje llevan
//! máscaras distintas y ambas desenmascaran al mismo payload; el bit MASK va puesto. En los dos
//! motores, con el paquete `net` real por dependencia de ruta.

use std::process::Command;

const MAIN: &str = r#"
from net/websocket import encode_frame_masked, op_text;

fn unmask(f: bytes) -> string {
    var out: [int] = [];
    var i = 6;
    while (i < f.len()) {
        out.push(f[i] ^ f[2 + (i - 6) % 4]);
        i = i + 1;
    }
    match (from_utf8(bytes_of(out))) {
        Result.Ok(s) => s,
        Result.Err(e) => "<bad utf8>",
    }
}

fn main() -> int {
    let payload = "hola mundo".to_bytes();
    let a = encode_frame_masked(op_text(), payload);
    let b = encode_frame_masked(op_text(), payload);
    let mask_a = a.sub_bytes(2, 6);
    let mask_b = b.sub_bytes(2, 6);
    print("masked bit: " + to_string((a[1] & 128) == 128));
    print("different masks: " + to_string(mask_a != mask_b));
    print("payload ok: " + to_string(unmask(a) == "hola mundo" && unmask(b) == "hola mundo"));
    0
}
"#;

fn run(vm: bool) -> String {
    let dir = std::env::temp_dir().join(format!("ray_ws_mask_{}_{}", std::process::id(), if vm { "vm" } else { "interp" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::write(dir.join("main.ray"), MAIN).unwrap();
    std::fs::write(
        dir.join("ray.toml"),
        format!("[package]\nname = \"wsmask\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{root}/packages/net\"\n"),
    )
    .unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.arg("run");
    if !vm {
        cmd.arg("--interp");
    }
    let out = cmd.arg("main.ray").current_dir(&dir).output().expect("lanza raylang");
    assert!(out.status.success(), "corre sin error (vm={vm}): {}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn masking_key_is_fresh_per_frame_on_both_engines() {
    let expected = "masked bit: true\ndifferent masks: true\npayload ok: true\n";
    assert_eq!(run(true), expected, "VM");
    assert_eq!(run(false), expected, "intérprete");
}
