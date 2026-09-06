//! M195 (plan ray-remote C3): `std/bigint` — enteros grandes sobre `bytes` con el runtime detrás
//! (`num-bigint`). Vectores pequeños (modpow/modinv y los tres errores), aritmética de 600×300 bits
//! contra valores calculados por Python, y un Diffie-Hellman de 4096 bits con generador 5 (el grupo
//! que negocia macOS) cuyo resultado calculó Python `pow`; la `modpow` debe tardar milisegundos.
//! Misma salida en VM, intérprete y binario nativo.

use std::io::Write;
use std::process::Command;

const PROGRAM: &str = r#"
import std/bigint;
import std/time;
fn hx(s: string) -> bytes { match (bigint.from_hex(s)) { Result.Ok(b) => b, Result.Err(e) => { panic(e); b"" } } }
fn show(r: Result<bytes, string>) -> string { match (r) { Result.Ok(b) => bigint.to_hex(b), Result.Err(e) => "err: " + e } }
fn main() -> int {
    let four = bigint.from_int(4); let thirteen = bigint.from_int(13); let m497 = bigint.from_int(497);
    print(show(bigint.modpow(four, thirteen, m497)));
    print(show(bigint.modinv(bigint.from_int(3), bigint.from_int(11))));
    print(show(bigint.modinv(bigint.from_int(6), bigint.from_int(9))));
    print(show(bigint.div(four, bigint.from_int(0))));
    print(show(bigint.sub(four, thirteen)));
    print(show(bigint.shl(bigint.from_int(1), 100)));
    print(show(bigint.shr(hx("10000000000000000000000000"), 99)));
    print(bigint.bit_len(hx("ff")) == 8 && bigint.bit_len(hx("100")) == 9 && bigint.bit_len(b"") == 0);
    print(bigint.to_int(hx("7fffffffffffffff"))); print(bigint.to_int(hx("8000000000000000")));
    print(bigint.cmp(four, thirteen)); print(bigint.cmp(thirteen, thirteen)); print(bigint.cmp(thirteen, four));
    print(bigint.to_hex(bigint.from_bytes(b"\x00\x00\x01\x02"))); print(bigint.to_hex(bigint.from_int(0)));
    let a = hx("3437f5ea3a0683ead81dcd365fdcd647bc754812fad8029d42f6709da9b14dda36e0d6a74c46118f32a1f27ab366023a782ebb205bc308119b4fe5fa285a0db869135cede26c2e2ce933e1"); let b = hx("2d10beddb070f7a04433fc2a9087219c1da6953404844e9e4a511b41900043e3ef5bfbd7d14");
    print(show(bigint.add(a, b)) == "3437f5ea3a0683ead81dcd365fdcd647bc754812fad8029d42f6709da9b14dda36e0d6a74c48e29b207cf98a2d6a457a3ad7c3927584e27aee902e3f123eb2ca1d2c5cf220ab23eca6b0f5"); print(show(bigint.sub(a, b)) == "3437f5ea3a0683ead81dcd365fdcd647bc754812fad8029d42f6709da9b14dda36e0d6a74c43408344c6eb6b3961befab585b2ae42012da8480f9db53e7568a6b4fa5ce9a42d386d2bb6cd"); print(show(bigint.mul(a, b)) == "93140a84ba76becc1b8f2e5c4c4a48dbcbb865aca30059ceb834531abb56ec178221ad319f97cc49e97693dcb4aa72e02b655b04d394413a800514f14456ac787662a66fabb0b9ff2d2d9800c4ce60bceccf0a9d4899b6a5834d4cd1c7fc0653e99fde08f4cda7a6005949be8ca9ea94");
    print(show(bigint.div(a, b)) == "128a2753013106fe044dcc9c2805d2e8df6142a3a00ca267d7c3d938cdbd59f4fb169cabc733"); print(show(bigint.rem(a, b)) == "c32e06d7ca2a4b23ddf80db681c7a0ecbd7e24db87f23527d00919f8257490f1a9f192bce5");
    // Diffie-Hellman de 4096 bits con generador 5 (el grupo que negocia macOS): y = 5^x mod p
    let p = hx("ec3fbf4dc20ef16468f918d8f6cdb2f803e0d681552454f14fab6f3e164f1513563e9bed45100358acc6d8f2c74c7ccf32d03fdda123f50190f5380e12b2a4146b77730f65bd9acbb57a6a1dfaf8cda9601e5b45785116080d650372e90794dfed52a24135b00a5436a80bdf0023b682af5570eed8e94b150452ef05f542441d111b8aaa62f28d1a4a789cb3d8b9b45c1b98fbe466809a111ba1192ec42b7170902a174f11fa2ac0079dd25a49fe85b0834c687a3acb6266c20ba2c250b601fc4105cca7b53302fc154cd2aad7185ddaee82ec3ffee5a5b28d1fe1daff6665896822a6b24735af1ca7a114907513923715c1d2dfa9964aef012d0ea67ff122294b4d8474a3ea284d3bd0334684e55160320094ead7a94ded97491e2370c6a5b85387f61376c468aec7321cc007b37e14998092253deffa38e12b2b8f30b17d0b09208a650f3ebdd3102b938b8743feb6d4ea65d003d716849f8558a628518867a66b0d389d95847ebd299753a767779673f778aaf6fa5db8656abd72fb710734986e86cb0ab8ab67a26b7f62b1852f27e3eff9c0cf44dd3f89e7d15f17362f25244caf9c4dabb4817253edc6181879932fa91425cb0088539d2c67eda13ffe7979cb9e86830c71c2cdcc69292f45e678309d6b79965eda32dae445508201e2bd73ab48767734d7c1c7fde805ec99108ddb5b5fab8f4d3e27dda1494c73cf256d");
    let x = hx("852395744b1e943e7db224cb98b20411e7a28cbdd2df2c206bba8d2141c9886e64409ddbb45f51c3bd65693b3d0840fb41536363f6724ba08329c05b09e803191bea85931a953cca0c2282666be49ee714186ebf9a8137e97b862eace1d7300f6361b9f8f33c1a7fafdd87333253b5628dce6f52f0be600da104a795bd4aeab02891dd3c3096c6c8b9b338eb3fdf23489c461cb5d15b77f23a775505e88e752f4f91540c27756991a0931ed42ecdcc0a62d74145ddd4a05422bfb8e0931719fdd5157e9d7bd55ee6965768e0f589d99a20918fa7740572419f452c075f27ff085e617f8e99edbce703f8670d3e361858a2f7647a952e1b8b356f8bd11711eb571304145212ca3f7062dc08d64bdbf090d48dd9f354366c219c3ecb54c5cefdd8027385c9421e7a607108e02236971e1b2577c1ecfd42e0440ac793f519af685d93b3a3d9a44f576a9a1de24edab871d5feef16e964ef2ebe2ff3600735f11af2050684bfe286852cff769e374ddc74c897bdd982cdac6046f9903b72f88ece64dd44fd3645114889001edc8e367e5d6dfd7410696bb6a3de65151c401dd377bf623d8eb7a4ca83b26b52b08d21870f0bc4ff64debb5d6b48fc3b66fa30d0b19482450164728a6fcf303a07b28f2df760ae9ca08b2d7c50487ca07386cc099a1e77064c2c0f552c9402cdf2af19de2bc1b4ff00ae3f1347de2274ea181e34b3f1");
    let t0 = time.monotonic();
    let r = show(bigint.modpow(bigint.from_int(5), x, p));
    let dt = time.monotonic() - t0;
    print(r == "579567774bfd92c91297eb73951b93fcbf0e291081052f92b3969afb0d05c920d941ff1910a3f7b478e305c24d2501fdb4525f1765b1a4c101b7f5ad98884fe997e9dc15cdde9b085ae7892588f3c8f809a54ea9ffeb1e445ab23921a71edf2799464341e39d135a845cc19cc9a5303f351db6e24be6aebd097a09d97a1d7b031e1851444e0d7d289882a0ad8955ed817a29a9ec98517da329e01f8aaebacf27fbaa5403dd6b9bc02b5ad2eb906ae5f44c233e6792222a11bc88a29c197bdefa0f840946e4a475e7172bba2f5cfcbc33735fcb945a693bc74a01ba89f163185d5e6dd06328fd9312b6c779755dda79a86ff172bac938b15f656d65294e31979498bcc73cbb5b9e99ef94a25becbddcd964471f6f77ebc738a98b943cb9e4634d1baa2c35c53f7b9f16a0aec33ef41fdd7f87aa23fff7d62bca6d492def56cc68ff49eb441626aaaef1b79ce62d1f593ea8c902eae58b0d3b286000e666543fe06e5e76d03c2b27834604ab5c3f37527922d9fc754b5597600b613d022a01d0847372e52d2f0210f47757417325ad77a289f05b3919974af81deba45f2468d4394a39d9931849c67eb26813ed5f06b2377811994965d211f6e18f7b7ccd1d1a7c3b9d8b81c3629e7fced6f809e34ea7fd7fe2640420e17fd8566ae5ef82fd170c9cfa22daa1a2bfa77f57c85f60792dde8352a88b35305e592176f42e3647305e");
    print(if (dt < 2000) { "modpow 4096 rapido" } else { "modpow 4096 LENTO: " + to_string(dt) + " ms" });
    0
}"#;

const WANT: &str = "1bd\n4\nerr: bigint: no modular inverse (not coprime)\nerr: bigint: division by zero\nerr: bigint: subtraction would be negative\n10000000000000000000000000\n2\ntrue\nOption.Some(9223372036854775807)\nOption.None\n-1\n0\n1\n102\n0\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\nmodpow 4096 rapido\n";

fn write_program(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ray_bigint_{name}"));
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
fn vm_and_interpreter_agree_with_python() {
    let path = write_program("engines");
    let (vm, err, code) = run(&path, "--vm");
    assert_eq!(vm, WANT, "VM\n{err}");
    assert_eq!(code, 0);
    let (interp, err, code) = run(&path, "--interp");
    assert_eq!(interp, WANT, "intérprete\n{err}");
    assert_eq!(code, 0);
}

#[test]
fn native_agrees_with_python() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando native bigint: rustc no disponible");
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
