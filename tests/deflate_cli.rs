//! Pruebas del encoder DEFLATE (`examples/web/deflate.ray`, cierre de M20). Dos niveles de verificación:
//! (1) **round-trip interno** determinista — comprime y descomprime con el propio `inflate.ray`
//! (deflate_raw/gzip/zlib ↔ inflate_raw/gunzip/zlib_inflate), por ambos motores, salida booleana;
//! (2) **compatibilidad estándar** — el gzip que produce raylang lo descomprime **Python** (`gzip`),
//! lo que prueba que el stream DEFLATE es válido, no solo auto-consistente.

use std::process::Command;

const EXPECTED: &[&str] = &[
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
    assert_eq!(lines, EXPECTED);
}

#[test]
fn deflate_round_trip_vm() {
    let (lines, ok) = run(&["--vm"], None);
    assert!(ok, "deflate_demo falló");
    assert_eq!(lines, EXPECTED);
}

/// El gzip que produce raylang debe poder descomprimirlo Python (compatibilidad estándar). Se escribe
/// a un temporal (vía el argumento del demo) y se descomprime con `python3 -c "gzip.decompress(...)"`.
#[test]
fn gzip_from_raylang_is_decompressed_by_python() {
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

/// M253: `deflate_raw` va por el runtime (`miniz_oxide`) en los tres motores → el stream, su CRC y
/// los envoltorios son BYTE-IDÉNTICOS entre VM, intérprete y nativo; y `--without deflate` (sin
/// crate detrás) cae al encoder en raylang, que sigue comprimiendo y descomprimiendo (otro stream).
#[test]
fn runtime_deflate_matches_on_all_three_engines_and_falls_back_without_it() {
    let base = std::env::temp_dir().join(format!("ray_deflate_rt_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // Entrada con estructura (repeticiones + ruido determinista) para que el LZ77 tenga trabajo.
    let mut input = Vec::new();
    let mut x: u32 = 7;
    for i in 0..200_000u32 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        input.push(if i % 7 == 0 { (x >> 16) as u8 } else { b"raylang "[(i % 8) as usize] });
    }
    std::fs::write(base.join("in.bin"), &input).unwrap();
    std::fs::write(
        base.join("prog.ray"),
        "import std/deflate;\nimport std/inflate;\nimport std/fs;\n\nfn main() -> int {\n    let data = fs.read_file_bytes(\"in.bin\").unwrap();\n    let z = deflate.deflate_raw(data);\n    let back = inflate.inflate_raw(z).unwrap();\n    let g = deflate.gzip_compress(data);\n    let zl = deflate.zlib_compress(data);\n    print(to_string(z.len()) + \" \" + to_string(back == data) + \" \" + to_string(inflate.crc32(z)) + \" \" + to_string(g.len()) + \" \" + to_string(inflate.gunzip(g).unwrap() == data) + \" \" + to_string(zl.len()) + \" \" + to_string(inflate.zlib_inflate(zl).unwrap() == data));\n    0\n}\n",
    )
    .unwrap();
    let run_bin = |bin: &str, args: &[&str]| -> String {
        let out = Command::new(bin).args(args).current_dir(&base).output().expect("run");
        assert!(out.status.success(), "{bin} {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let run = |args: &[&str]| -> String { run_bin(env!("CARGO_BIN_EXE_ray"), args) };
    let vm = run(&["--vm", "prog.ray"]);
    let interp = run(&["--interp", "prog.ray"]);
    assert_eq!(vm, interp, "VM ≡ intérprete");
    let fields: Vec<&str> = vm.split(' ').collect();
    assert_eq!(fields.len(), 7, "{vm}");
    assert_eq!((fields[1], fields[4], fields[6]), ("true", "true", "true"), "round-trips: {vm}");
    let zlen: usize = fields[0].parse().unwrap();
    assert!(zlen < input.len() / 2, "comprime de verdad: {zlen} de {}", input.len());
    if !Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("saltando native deflate: rustc no disponible");
        return;
    }
    let exe = std::env::consts::EXE_SUFFIX;
    let fast = format!("fast{exe}");
    let pure = format!("pure{exe}");
    let build = |out: &str, without: &str| {
        let st = Command::new(env!("CARGO_BIN_EXE_ray"))
            .args(["build", "--native", "prog.ray", "-o", out, "--without", without])
            .current_dir(&base)
            .output()
            .expect("build --native");
        assert!(st.status.success(), "build --native ({without}): {}", String::from_utf8_lossy(&st.stderr));
    };
    build(&fast, "mimalloc,ahash,fibers");
    let native = run_bin(base.join(&fast).to_str().unwrap(), &[]);
    assert_eq!(native, vm, "nativo ≡ VM (mismo miniz_oxide)");
    build(&pure, "mimalloc,ahash,fibers,deflate");
    let fallback = run_bin(base.join(&pure).to_str().unwrap(), &[]);
    let f: Vec<&str> = fallback.split(' ').collect();
    assert_eq!((f[1], f[4], f[6]), ("true", "true", "true"), "el encoder raylang de respaldo sigue en pie: {fallback}");
    assert_ne!(f[0], fields[0], "sin el runtime el stream es el del encoder raylang (otro tamaño)");
    let _ = std::fs::remove_dir_all(&base);
}
