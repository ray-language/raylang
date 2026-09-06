//! M194 (plan ray-remote C2): `std/crypto/{md5,aes,des}` — cripto LEGADA en raylang puro contra los
//! vectores oficiales: RFC 1321 (MD5), FIPS-197 C.1/C.2/C.3 (AES-128/192/256), NIST SP 800-38A F.2.1
//! (AES-128-CBC), FIPS 46-3 (DES) y NIST SP 800-67 (3DES), más los round-trips de descifrado y un
//! error de tamaño de clave. Misma salida en VM, intérprete y binario nativo.

use std::io::Write;
use std::process::Command;

const PROGRAM: &str = r#"
import std/crypto/md5;
import std/crypto/aes;
import std/crypto/des;
import std/hex;
fn hexed(b: bytes) -> string { hex.hex_encode(b) }
fn h(s: string) -> bytes { match (hex.hex_decode(s)) { Result.Ok(b) => b, Result.Err(e) => b"" } }
fn show(r: Result<bytes, string>) -> string { match (r) { Result.Ok(b) => hexed(b), Result.Err(e) => "err: " + e } }
fn main() -> int {
    print(hexed(md5.md5(b"")));
    print(hexed(md5.md5("message digest".to_bytes())));
    let k128 = h("000102030405060708090a0b0c0d0e0f");
    let pt = h("00112233445566778899aabbccddeeff");
    print(show(aes.encrypt_block(k128, pt)));                                   // 69c4e0d86a7b0430d8cdb78070b4c55a
    print(show(aes.decrypt_block(k128, h("69c4e0d86a7b0430d8cdb78070b4c55a"))));
    let k256 = h("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
    print(show(aes.encrypt_block(k256, pt)));                                   // 8ea2b7ca516745bfeafc49904b496089
    print(show(aes.decrypt_block(k256, h("8ea2b7ca516745bfeafc49904b496089"))));
    let k192 = h("000102030405060708090a0b0c0d0e0f1011121314151617");
    print(show(aes.encrypt_block(k192, pt)));                                   // dda97ca4864cdfe06eaf70a0ec0d7191
    let kc = h("2b7e151628aed2a6abf7158809cf4f3c"); let iv = h("000102030405060708090a0b0c0d0e0f");
    let p2 = h("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
    print(show(aes.encrypt_cbc(kc, iv, p2)));                                    // 7649abac8119b246cee98e9b12e9197d 5086cb9b507219ee95db113a917678b2
    print(show(aes.decrypt_cbc(kc, iv, h("7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2"))) == hexed(p2));
    let dk = h("133457799bbcdff1"); let dp = h("0123456789abcdef");
    print(show(des.encrypt_block(dk, dp)));                                     // 85e813540f0ab405
    print(show(des.decrypt_block(dk, h("85e813540f0ab405"))));
    let tk = h("0123456789abcdef23456789abcdef01456789abcdef0123");
    print(show(des.tdes_encrypt_ecb(tk, "The qufck brown fox jump".to_bytes())));  // a826fd8ce53b855fcce21c8112256fe668d5c05dd9b6b900
    print(show(des.tdes_decrypt_ecb(tk, h("a826fd8ce53b855fcce21c8112256fe668d5c05dd9b6b900"))) == hexed("The qufck brown fox jump".to_bytes()));
    print(show(des.encrypt_cbc(dk, h("1234567890abcdef"), "Now is the time for all ".to_bytes())));
    print(show(des.decrypt_cbc(dk, h("1234567890abcdef"), des.encrypt_cbc(dk, h("1234567890abcdef"), "Now is the time for all ".to_bytes()).unwrap_or(b""))) == hexed("Now is the time for all ".to_bytes()));
    print(show(aes.encrypt_ecb(h("00"), pt)));
    0
}"#;

const WANT: &str = "d41d8cd98f00b204e9800998ecf8427e\nf96b697d7cb7938d525a2f31aaf161d0\n69c4e0d86a7b0430d8cdb78070b4c55a\n00112233445566778899aabbccddeeff\n8ea2b7ca516745bfeafc49904b496089\n00112233445566778899aabbccddeeff\ndda97ca4864cdfe06eaf70a0ec0d7191\n7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2\ntrue\n85e813540f0ab405\n0123456789abcdef\na826fd8ce53b855fcce21c8112256fe668d5c05dd9b6b900\ntrue\nc363c8b3565c0e19a531404ecf092c12ca9543be7f2033c4\ntrue\nerr: aes: the key must be 16, 24 or 32 bytes, got 1\n";

fn write_program(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_crypto_legacy_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crea el directorio temporal");
    let path = dir.join("prog.ray");
    std::fs::File::create(&path).expect("crea").write_all(PROGRAM.as_bytes()).expect("escribe");
    path
}

fn run(path: &std::path::Path, flag: &str) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang")).arg(flag).arg(path).output().expect("lanza raylang");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn vm_and_interpreter_match_the_official_vectors() {
    let path = write_program("engines");
    let (vm, err, code) = run(&path, "--vm");
    assert_eq!(vm, WANT, "VM\n{err}");
    assert_eq!(code, 0);
    let (interp, err, code) = run(&path, "--interp");
    assert_eq!(interp, WANT, "intérprete\n{err}");
    assert_eq!(code, 0);
}

#[test]
fn native_matches_the_official_vectors() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando native crypto legacy: rustc no disponible");
        return;
    }
    let path = write_program("native");
    let bin = path.with_file_name(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "--native", path.to_str().unwrap(), "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza ray build");
    assert!(build.status.success(), "build --native\n{}", String::from_utf8_lossy(&build.stderr));
    let out = Command::new(&bin).output().expect("corre el binario nativo");
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
}
