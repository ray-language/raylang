//! Ronda de stdlib sobre 1.27.95 (hallazgos de raycrawl, raycert, raygrpc, raycdc, ray-sublime y
//! ray-daw): cada pieza nueva da lo mismo en la VM, en el intérprete y en el binario nativo.
use std::process::Command;

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// VM, intérprete y (si hay rustc) el binario nativo dan `want` sin nada en stderr.
fn same_everywhere(name: &str, src: &str, want: &str) {
    let d = std::env::temp_dir().join(format!("ray_stdlib_round_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("prog.ray"), src).unwrap();
    for args in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(&d).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stderr), "", "{name} {args:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{name} {args:?}");
    }
    if has_rustc() {
        let bin = d.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "--native", "prog.ray", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        assert!(b.status.success(), "{name} build --native: {}", String::from_utf8_lossy(&b.stderr));
        let o = Command::new(&bin).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{name} nativo");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn result_err_retry_regex_text_and_charset() {
    // #409 `Result.err()`; #365 `regex.escape`; #367 `text.words` corta en el White_Space de
    // Unicode (como `trim`); #363 `charset.decode_lossy`; #364 `retry_if`/`retry_with`.
    same_everywhere(
        "pure",
        r##"import std/regex;
import std/text;
import std/charset;
import std/resilience;
fn main() -> int {
    let r: Result<int, string> = Result.Err("boom");
    print(r.err());
    let k: Result<int, string> = Result.Ok(1);
    print(k.err());
    print(regex.escape("a.b*c(d)[e]{f}^$|\\+?"));
    print(regex.full_match(regex.escape("a.b*c(d)[e]{f}^$|\\+?"), "a.b*c(d)[e]{f}^$|\\+?"));
    print(regex.full_match(regex.escape("a.b"), "axb"));
    let o = regex.onig(regex.escape("1+1=2 (ok)")).unwrap();
    print(o.is_match("x 1+1=2 (ok) y"));
    print(text.words("a\u{A0}b c\u{3000}d\te").len());
    print(charset.decode_lossy(bytes_of([0x82, 0xA0, 0xFF, 0x41]), "shift_jis"));
    print(charset.decode_lossy(b"ok", "nope"));
    var calls = 0;
    let p = resilience.policy(5, 1, 2);
    let a = resilience.retry_if(p, fn(e: string) -> bool { e == "503" }, fn() -> Result<int, string> {
        calls = calls + 1;
        if (calls < 3) { Result.Err("503") } else { Result.Err("404") }
    });
    print(a); print(calls);
    var seen: [string] = [];
    let b = resilience.retry_with(p, fn(n: int, e: string) -> int { seen.push("${n}:${e}"); if (n == 2) { 0 } else { 0 - 1 } },
        fn(n: int) -> Result<int, string> { if (n < 4) { Result.Err("e${n}") } else { Result.Ok(n) } });
    print(b); print(seen);
    calls = 0;
    let c = resilience.retry(resilience.policy(3, 1, 1), fn() -> Result<int, string> { calls = calls + 1; Result.Err("x") });
    print(c); print(calls);
    0
}
"##,
        concat!(
            "Option.Some(boom)\n",
            "Option.None\n",
            "a\\.b\\*c\\(d\\)\\[e\\]\\{f\\}\\^\\$\\|\\\\\\+\\?\n",
            "true\n",
            "false\n",
            "true\n",
            "5\n",
            "Result.Ok((あ�A, 1))\n",
            "Result.Err(charset: unknown charset 'nope' (labels() lists the known ones))\n",
            "Result.Err(404)\n",
            "3\n",
            "Result.Ok(4)\n",
            "[1:e1, 2:e2, 3:e3]\n",
            "Result.Err(x)\n",
            "3\n",
        ),
    );
}

#[test]
fn url_normalize_and_escape_unsafe() {
    // #366: la normalización del RFC 3986 §6 y el escape de un href sin tocar las barras.
    same_everywhere(
        "url",
        r##"import std/url;
fn main() -> int {
    print(url.normalize("HTTP://H:80/%7e/./a/../b?x=%3a#F"));
    print(url.normalize("https://Example.COM:443"));
    print(url.normalize("https://example.com:8443/a%2fb/%e3%81%82"));
    print(url.normalize("../a/./b"));
    print(url.normalize("mailto:A@B.c"));
    print(url.normalize("http://[::1]:80/"));
    print(url.normalize("http://h/a%"));
    print(url.normalize("a b"));
    print(url.escape_unsafe("/a b/é?q=x y&z=\"1\"#f%20"));
    print(url.resolve("http://h/x/", url.escape_unsafe("a b.html")));
    0
}
"##,
        concat!(
            "Result.Ok(http://h/~/b?x=%3A#F)\n",
            "Result.Ok(https://example.com/)\n",
            "Result.Ok(https://example.com:8443/a%2Fb/%E3%81%82)\n",
            "Result.Ok(../a/./b)\n",
            "Result.Ok(mailto:A@B.c)\n",
            "Result.Ok(http://[::1]/)\n",
            "Result.Ok(http://h/a%)\n",
            "Result.Err(invalid URL (control character or space; percent-encode it))\n",
            "/a%20b/%C3%A9?q=x%20y&z=%221%22#f%20\n",
            "Result.Ok(http://h/x/a%20b.html)\n",
        ),
    );
}

#[cfg(unix)]
#[test]
fn files_and_folders_are_born_with_their_mode() {
    // #382: una clave privada no existe nunca con permisos más abiertos que los pedidos.
    same_everywhere(
        "fsmode",
        r##"import std/fs;
fn main() -> int {
    let d = fs.make_temp_dir("b5fs").unwrap();
    print(fs.mkdir_mode(d + "/a/b", 448));
    print(fs.stat(d + "/a").unwrap().mode);
    print(fs.stat(d + "/a/b").unwrap().mode);
    print(fs.mkdir_mode(d + "/a/b", 493));
    print(fs.stat(d + "/a/b").unwrap().mode);
    print(fs.write_file_mode(d + "/a/key", "sécret", 384));
    print(fs.stat(d + "/a/key").unwrap().mode);
    print(fs.read_file(d + "/a/key"));
    fs.write_file(d + "/a/open", "x").unwrap();
    print(fs.stat(d + "/a/open").unwrap().mode != 384);
    print(fs.write_file_bytes_mode(d + "/a/open", b"yz", 384));
    print(fs.stat(d + "/a/open").unwrap().mode);
    print(fs.read_file(d + "/a/open"));
    print(fs.write_file_mode(d + "/nope/x", "x", 384).is_err());
    fs.remove_all(d).unwrap();
    0
}
"##,
        concat!(
            "Result.Ok(0)\n",
            "448\n",
            "448\n",
            "Result.Ok(0)\n",
            "448\n",
            "Result.Ok(6)\n",
            "384\n",
            "Result.Ok(sécret)\n",
            "true\n",
            "Result.Ok(2)\n",
            "384\n",
            "Result.Ok(yz)\n",
            "true\n",
        ),
    );
}

#[test]
fn float32_bits_and_self_pid() {
    // #390/#426: el binary32 de un float (redondeo a par, infinito, subnormal); #397 el pid propio.
    same_everywhere(
        "f32",
        r##"import std/math;
import std/process;
fn main() -> int {
    print(math.float32_bits(1.0));
    print(math.float32_bits(0.0 - 2.5));
    print(math.float32_bits(0.1));
    print(math.float32_bits(1.0e40));
    print(math.float32_bits(1.0e-45));
    print(math.float32_from_bits(1065353216));
    print(math.float32_from_bits(math.float32_bits(0.1)));
    print(math.float32_from_bits(4294967296 + 1065353216));
    print(math.float32_from_bits(0 - 1) == math.float32_from_bits(0 - 1));
    print(process.self_pid() > 1);
    0
}
"##,
        concat!(
            "1065353216\n",
            "3223322624\n",
            "1036831949\n",
            "2139095040\n",
            "1\n",
            "1\n",
            "0.10000000149011612\n",
            "1\n",
            "false\n",
            "true\n",
        ),
    );
}

#[test]
fn binary_reads_and_writes_both_byte_orders() {
    // #426: enteros de 16/24/32/64 bits y floats, little- y big-endian, ida y vuelta.
    same_everywhere(
        "binary",
        r##"import std/binary;
import std/collections/bytesbuilder as bb;
fn main() -> int {
    let o = bb.new();
    binary.push_u16le(o, 0xBEEF);
    binary.push_u16be(o, 0xBEEF);
    binary.push_u24le(o, 0 - 2);
    binary.push_u24be(o, 0x123456);
    binary.push_u32le(o, 0 - 123456);
    binary.push_u32be(o, 0xDEADBEEF);
    binary.push_i64le(o, 0 - 5);
    binary.push_i64be(o, 0x0102030405060708);
    binary.push_f32le(o, 0.5);
    binary.push_f32be(o, 0.0 - 1.25);
    binary.push_f64le(o, 3.141592653589793);
    binary.push_f64be(o, 1.0e300);
    let b = bb.build(o);
    print(b);
    print([binary.u16le(b, 0), binary.u16be(b, 2), binary.i16le(b, 0), binary.i24le(b, 4), binary.u24le(b, 4), binary.u24be(b, 7), binary.i24be(b, 7)]);
    print([binary.i32le(b, 10), binary.u32le(b, 10), binary.u32be(b, 14), binary.i32be(b, 14), binary.i64le(b, 18), binary.i64be(b, 26)]);
    print([binary.f32le(b, 34), binary.f32be(b, 38), binary.f64le(b, 42), binary.f64be(b, 50)]);
    print(b.len());
    0
}
"##,
        concat!(
            "efbebeeffeffff123456c01dfeffdeadbeeffbffffffffffffff01020304050607080000003fbfa00000182d4454fb2109407e37e43c8800759c\n",
            "[48879, 48879, -16657, -2, 16777214, 1193046, 1193046]\n",
            "[-123456, 4294843840, 3735928559, -559038737, -5, 72623859790382856]\n",
            "[0.5, -1.25, 3.141592653589793, 1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000]\n",
            "58\n",
        ),
    );
}

#[test]
fn protobuf_signed_packed_nested_and_floats() {
    // #389: un varint negativo son 10 octetos; zigzag, float/double, repetidos empaquetados,
    // submensajes, varints sueltos y `Show` de `PbField`.
    same_everywhere(
        "protobuf",
        r##"import std/protobuf;
fn main() -> int {
    let w = protobuf.writer();
    protobuf.write_varint(w, 1, 0 - 1);
    protobuf.write_varint(w, 2, 0 - 9223372036854775807 - 1);
    protobuf.write_sint(w, 3, 0 - 3);
    protobuf.write_float(w, 4, 0.5);
    protobuf.write_double(w, 5, 0.0 - 1.0e100);
    protobuf.write_packed_varints(w, 6, [1, 300, 0 - 2]);
    let inner = protobuf.writer();
    protobuf.write_string(inner, 1, "in");
    protobuf.write_message(w, 7, inner);
    protobuf.write_string(w, 8, "a");
    protobuf.write_string(w, 8, "b");
    let b = protobuf.finish(w);
    print(b);
    let fs = protobuf.parse(b).unwrap();
    print(fs);
    print([protobuf.get_int(fs, 1), protobuf.get_int(fs, 2), protobuf.unzigzag(protobuf.get_int(fs, 3))]);
    print([protobuf.get_float(fs, 4), protobuf.get_float(fs, 5), protobuf.get_float(fs, 99)]);
    print(protobuf.read_packed_varints(protobuf.get_bytes(fs, 6)));
    print(protobuf.get_string(protobuf.get_message(fs, 7).unwrap(), 1));
    print(protobuf.get_all(fs, 8).len());
    print([protobuf.zigzag(0), protobuf.zigzag(0 - 1), protobuf.zigzag(1), protobuf.zigzag(0 - 9223372036854775807 - 1), protobuf.unzigzag(protobuf.zigzag(0 - 9223372036854775807 - 1)) + 1, protobuf.unzigzag(protobuf.zigzag(9223372036854775807))]);
    print(protobuf.varint(300));
    print(protobuf.read_varint(b"\xac\x02\x01", 0));
    print(protobuf.read_varint(b"\xac", 0));
    print(protobuf.read_packed_varints(b"\x01\x80"));
    0
}
"##,
        concat!(
            "08ffffffffffffffffff0110808080808080808080011805250000003f297dc39425ad49b2d4320d01ac02feffffffffffffffff013a040a02696e420161420162\n",
            "[#1 varint -1, #2 varint -9223372036854775808, #3 varint 5, #4 fixed32 1056964608, #5 fixed64 -3120350583805656195, #6 bytes(13), #7 bytes(4), #8 bytes(1), #8 bytes(1)]\n",
            "[-1, -9223372036854775808, -3]\n",
            "[0.5, -10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000, 0]\n",
            "Result.Ok([1, 300, -2])\n",
            "in\n",
            "2\n",
            "[0, 1, 2, -1, -9223372036854775807, 9223372036854775807]\n",
            "ac02\n",
            "Result.Ok((300, 2))\n",
            "Result.Err(protobuf: truncated varint of the value at byte 0)\n",
            "Result.Err(protobuf: truncated varint of the value at byte 1)\n",
        ),
    );
}

#[test]
fn p384_rsa_sha384_and_ed25519_pkcs8_against_openssl_vectors() {
    // #380/#381: claves y firmas generadas con OpenSSL 3.5 (solo para esta prueba): ECDSA P-384
    // con SHA-384 y SHA-256 en DER, RSA PKCS#1 con SHA-384/512, PSS con SHA-384 y una clave
    // Ed25519 en PKCS#8 cuya pública y firma (determinista) son las que dio OpenSSL.
    same_everywhere(
        "crypto",
        r##"import std/crypto;
import std/hex;
fn h(s: string) -> bytes { hex.hex_decode(s).unwrap() }
fn main() -> int {
    let msg = "raylang signature fixture".to_bytes();
    let other = "raylang signature fixturf".to_bytes();
    let p384 = h("045de42f57e9792f85926a9dad294690ac1c775a584f05f0e744b81574a33b40d680121094735f82ff7454f201af52b1d398664f1a47ed4804af1c0959fb753d5c1671602fc05a1168612d25e227ad912253c07316dc8103702e28a2dffc654711");
    let s384 = h("3064023027b41d79395bc635a29a104363635ff46c7f7ba2cd49ae43f2e23a9cdafb7f59800284136e070ce326eb1b9f66769d3a02307a4836656406f6dfcabc0b6ef276401e716e924a75c9d9bdcd8825262fee7a44786d86b00d1d19a2beffa979779f30d8");
    let s256 = h("30660231009877c66d5a121cc1d3c4e4a78f4bc9386cc242ebfa492d989bb700372a1e68b3cf7b5371e599ab7ccdde9417960c1ef0023100ad287058e053a537995c2c913d2815333ffd0a4daa7a0d820392b98abfcf7a4e041e7149f2d680766cfecd255c823578");
    print([crypto.ecdsa_verify_asn1("p384", "sha384", p384, msg, s384), crypto.ecdsa_verify_asn1("p384", "sha256", p384, msg, s256)]);
    print([crypto.ecdsa_verify_asn1("p384", "sha384", p384, other, s384), crypto.ecdsa_verify_asn1("p384", "sha256", p384, msg, s384), crypto.ecdsa_verify_asn1("p256", "sha384", p384, msg, s384), crypto.ecdsa_verify_asn1("p521", "sha512", p384, msg, s384)]);
    let rsa = h("3082010a0282010100a325de794fbdc6e4e3d7dc32af5dcd9bb02fae9df7634c3fddc8dcbfba076b62c60a028cc95b14e3953ba0835ced594b18fc73be73c91f667591d846141423e0b5ae426a76e0ee684b85228fe1ef2bab59e80d78807cf898e37ae82e210b5fd1294e305a34d5c181f244b0a3ca177e6db785e8e425229bdbeda5dd0dd705d0a2e0f9aa2f3516adbf6c6e5522295955fb23939ff39f5f96647f3ac06acbf80d003ac225791ad43a042b62a104b3728875dfe67110803220469f585f93a15224deec720f23d6fcbe246db3a16b5250ff498905bf0cd95288696d84fa10cbffb9b62d7aa58eca45479262cc01a7a7cda0f06fb7026100b3e4be8e3a8850ca91a67b0203010001");
    let r384 = h("39f6dcb72bc64e898b0461c3789a040e98355a8de2616db67e5164d8db4890b41e21c61594eca99f3b189770d3fa713f30394155ffb46893ce4132d6c5e3555cd9f9ba42584329e57cb5eaefe867bd3c5716824e259d6a2bf977f07d17aae6d3f4ae586d11cb66b63ad482ce5809a2486b394a85d3ddf312b9cc1ced327a143115980b837a7c217c8eca390e2cf79860a896e69b55c112aa424ec59b30c6a5b3f7c471739675653d633fa4477b3087b1fdbdca084c32bfe97b19f8ebed67918ec53d5711ccb168d129afc61f6d71bdfd6bc1da085eef88ec0a3da3e57de1d1d947fc027f10f7bbb7fab03e76e19b05dda8efebb56b80ab410b405ce47b129b64");
    let r512 = h("35260b6da6ac6630e3e6066a08b5657c8c7705e1756c40396d0732a8a860617e7b159feec86be3f5e9a457f342069b2b8687552574bdf5c2a8cfd667c601255d9dfd2cc3e134e41b910d203ea0a56a85ffb9be9cb4e32a2baa23ec0a5784dcb968abfca2c454c2c897a9c51fc6fc22dbd474f2e70af46501bafbfb01b4d013867af412d4756f525740f0156fa7fe4e768f2af6fe6bd2867d0a2530c2479e86c4cf8d17ef6e0171f251792a02e69c175682abe27b4fc1103104a8bf7149f28ec8dc9996f5cc1469dacacd83dd44b3f99ee726fcd60a81e91b5b80022764c1eaac6bbfcc88d07e63704c6460537f0f11d01f267a3b52ca2583cda4f134619894b8");
    let pss = h("0c29bf72d77447ccc833750dd30f3482dd3b2505a87ad570aafbe17cf9faac9ea27ae30243d92d73522311e379740ef79ea242f4133385ab9feea180ece62382f9477ff4f9ecaa1a6bd71bd94949e228589fdaf6ab1614de9b622fb13be230e96d4bee8ee464cdb347977fbd3813b0b3e593a62150eed751d2e98fa495f07886188c7b82756ac3fcf781b6e74750c91ef6964b13815be8b6704e3cf8ad057a038d7bc39aa64791ff30a7ca8f40d5a184c7738c58e38d2b4de2c4469d793da5386b75cb23966950a89f57976d01cd5b0f84d649908763203de54f135a93ff83d95dd5a095c4b5005c4708d79c653b037740f16b287332f4cf4f9c7a17ca0751ee");
    print([crypto.rsa_pkcs1_verify_with("sha384", rsa, msg, r384), crypto.rsa_pkcs1_verify_with("sha512", rsa, msg, r512), crypto.rsa_pss_verify_with("sha384", rsa, msg, pss)]);
    print([crypto.rsa_pkcs1_verify_with("sha256", rsa, msg, r384), crypto.rsa_pkcs1_verify_with("sha384", rsa, other, r384), crypto.rsa_pkcs1_verify_with("md5", rsa, msg, r384), crypto.rsa_pkcs1_verify(rsa, msg, r384)]);
    // Ed25519 en PKCS#8: la clave de `openssl genpkey` firma y da su pública sin quitar el sobre.
    let p8 = h("302e020100300506032b6570042204203894b60d9958991e3e6582b6f10696c4f39c53fd8e501f5b2e4b667acdc14608");
    let seed = crypto.ed25519_seed_of(p8).unwrap();
    print(hex.hex_encode(crypto.ed25519_public_key(p8).unwrap()));
    print(hex.hex_encode(crypto.ed25519_sign(p8, msg).unwrap()));
    print([seed.len() == 32, crypto.ed25519_pkcs8_of(seed).unwrap() == p8, crypto.ed25519_sign(seed, msg) == crypto.ed25519_sign(p8, msg)]);
    print(crypto.ed25519_seed_of(seed).unwrap() == seed);
    print(crypto.ed25519_seed_of(b"short"));
    print(crypto.ed25519_sign(p8.sub_bytes(0, 40), msg).is_none());
    let fresh = crypto.ed25519_generate().unwrap();
    let sig = crypto.ed25519_sign(fresh, msg).unwrap();
    print([fresh.len() == 48, crypto.ed25519_verify(crypto.ed25519_public_key(fresh).unwrap(), msg, sig), crypto.ed25519_verify(crypto.ed25519_public_key(fresh).unwrap(), other, sig)]);
    // ES256 fijo sigue igual por el nombre nuevo.
    let k = crypto.p256_generate().unwrap();
    let s = crypto.p256_sign(k, msg).unwrap();
    print([crypto.ecdsa_verify("p256", crypto.p256_public_key(k).unwrap(), msg, s), crypto.ecdsa_verify("p384", crypto.p256_public_key(k).unwrap(), msg, s)]);
    0
}
"##,
        concat!(
            "[true, true]\n",
            "[false, false, false, false]\n",
            "[true, true, true]\n",
            "[false, false, false, false]\n",
            "b22f685f7de8647ecddd2e250d1810fd6e149d226f1d4b00c5045d7f195cd50a\n",
            "425055ed344f902616a7d16d416b2d526e1bfaceb3970d5c296dd70c11d3b3775b72b22be716f30c503dfd41698dbe64fe6d9f59de89dc36e2b613ca6a35d00b\n",
            "[true, true, true]\n",
            "true\n",
            "Result.Err(ed25519: not a 32-byte seed or a PKCS#8 Ed25519 key)\n",
            "true\n",
            "[true, true, false]\n",
            "[true, false]\n",
        ),
    );
}
