//! M144 — std/image: decodificación PNG. La batería (filtros None/Sub/Up/Paeth, gris 1-bit,
//! paleta 2-bit + tRNS, gris+alfa, RGB de 16 bits reducido, entrelazado/CRC/truncado → Err)
//! corre byte-idéntica en los TRES motores; los PNG van embebidos como literales b"…"
//! (generados con zlib de Python — el oráculo externo del formato).

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("ray_image_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("lanza ray");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn png_battery_matches_on_all_three_engines() {
    let src = include_str!("fixtures/image_decode.ray");
    let want = include_str!("fixtures/image_decode.out");
    let base = tmp("battery");
    std::fs::write(base.join("prog.ray"), src).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, err, code) = ray(&base, &[engine, "prog.ray"]);
        assert_eq!(code, 0, "{engine}: exit 0\n{err}");
        assert_eq!(out, want, "{engine}: salida exacta");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(bcode, 0, "build --native ok\n{berr}");
        let native = Command::new(&bin).stdin(std::process::Stdio::null()).output().expect("nativo");
        assert_eq!(String::from_utf8_lossy(&native.stdout), want, "nativo ≡ VM");
        assert_eq!(native.status.code(), Some(0));
    }
}

/// M164 — `encode_png`: la batería (3x2 RGBA con alfa, 1x1, errores de tamaño/dimensiones) corre
/// byte-idéntica en los TRES motores, y el PNG emitido lo valida un oráculo EXTERNO (zlib de
/// Python: firma, CRC de cada chunk, IHDR RGBA8 y los octetos crudos tras inflar) si hay python3.
#[test]
fn png_encode_round_trips_and_is_valid_for_an_external_decoder() {
    let src = include_str!("fixtures/image_encode.ray");
    let want = include_str!("fixtures/image_encode.out");
    let base = tmp("encode");
    std::fs::write(base.join("prog.ray"), src).unwrap();
    for engine in ["--vm", "--interp"] {
        let (out, err, code) = ray(&base, &[engine, "prog.ray"]);
        assert_eq!(code, 0, "{engine}: exit 0\n{err}");
        assert_eq!(out, want, "{engine}: salida exacta");
    }
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = base.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let (_o, berr, bcode) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
        assert_eq!(bcode, 0, "build --native ok\n{berr}");
        let native = Command::new(&bin).stdin(std::process::Stdio::null()).output().expect("nativo");
        assert_eq!(String::from_utf8_lossy(&native.stdout), want, "nativo ≡ VM");
    }
    // El oráculo externo: la segunda línea del fixture es el PNG como lista de octetos.
    let png: Vec<u8> = want
        .lines()
        .nth(1)
        .unwrap()
        .trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    std::fs::write(base.join("out.png"), &png).unwrap();
    let oracle = "import zlib,struct,sys\n\
        d=open(sys.argv[1],'rb').read()\n\
        assert d[:8]==b'\\x89PNG\\r\\n\\x1a\\n'\n\
        i=8; idat=b''; ihdr=None\n\
        while i<len(d):\n\
        \x20   n=struct.unpack('>I',d[i:i+4])[0]; t=d[i+4:i+8]; body=d[i+8:i+8+n]\n\
        \x20   crc=struct.unpack('>I',d[i+8+n:i+12+n])[0]\n\
        \x20   assert zlib.crc32(t+body)&0xffffffff==crc, t\n\
        \x20   if t==b'IHDR': ihdr=struct.unpack('>IIBBBBB',body)\n\
        \x20   if t==b'IDAT': idat+=body\n\
        \x20   i+=12+n\n\
        print(ihdr, list(zlib.decompress(idat)))\n";
    let py = Command::new("python3").arg("-c").arg(oracle).arg(base.join("out.png")).output();
    match py {
        Ok(o) if o.status.success() => {
            let got = String::from_utf8_lossy(&o.stdout);
            assert_eq!(
                got.trim(),
                "(3, 2, 8, 6, 0, 0, 0) [0, 255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 0, 10, 20, 30, 40, 255, 255, 255, 255, 0, 0, 0, 255]",
                "el decodificador externo ve IHDR RGBA8 y las scanlines exactas"
            );
        }
        Ok(o) => panic!("el oráculo Python rechazó el PNG: {}", String::from_utf8_lossy(&o.stderr)),
        Err(_) => eprintln!("saltando el oráculo externo: python3 no disponible"),
    }
}
