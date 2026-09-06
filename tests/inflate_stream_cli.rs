//! M193 (plan ray-remote C1): `std/inflate` INCREMENTAL — un stream zlib/DEFLATE que sobrevive a
//! muchos `stream_push`, con la ventana LZ77 entre llamadas. Python `zlib` genera un stream con
//! `Z_SYNC_FLUSH` por mensaje (lo que hacen RFB y WebSocket); raylang lo descomprime (a) en trozos de
//! tamaños arbitrarios (cortes dentro de un bloque) y (b) mensaje a mensaje, en la VM, el intérprete
//! y el binario nativo. Sin python3 se omite.

use std::process::Command;

const PROGRAM: &str = r#"
import std/fs;
import std/inflate;
fn read(p: string) -> bytes { match (fs.read_file_bytes(p)) { Result.Ok(d) => d, Result.Err(e) => { panic(e); b"" } } }
fn ints(p: string) -> [int] {
    let t = match (fs.read_file(p)) { Result.Ok(d) => d, Result.Err(e) => { panic(e); "" } };
    var out: [int] = [];
    for w in t.trim().split(" ") { match (parse_int(w)) { Option.Some(n) => out.push(n), Option.None => { } } }
    out
}
fn main() -> int {
    let data = read("stream.bin");
    let want = read("expected.bin");
    // (a) trozos arbitrarios, incluidos cortes dentro de un bloque
    let sizes = [1, 7, 100, 3, 5000, 33, 2, 9999, 1, 1];
    let z = inflate.zlib_stream();
    var out = b"";
    var pos = 0;
    var i = 0;
    while (pos < data.len()) {
        var n = sizes[i % sizes.len()];
        if (pos + n > data.len()) { n = data.len() - pos; }
        match (inflate.stream_push(z, data.sub_bytes(pos, pos + n))) {
            Result.Ok(b) => { out = out + b; },
            Result.Err(e) => { print("err: " + e); return 1; },
        }
        pos = pos + n;
        i = i + 1;
    }
    print(out == want);
    print(inflate.stream_finished(z));
    // (b) un push por mensaje: cada push produce exactamente su mensaje
    let lens = ints("seg_lens.txt");
    let mlens = ints("msg_lens.txt");
    let z2 = inflate.zlib_stream();
    var p = 0;
    var k = 0;
    var all_ok = true;
    while (k < lens.len()) {
        match (inflate.stream_push(z2, data.sub_bytes(p, p + lens[k]))) {
            Result.Ok(b) => { if (b.len() != mlens[k]) { all_ok = false; } },
            Result.Err(e) => { print("err: " + e); return 1; },
        }
        p = p + lens[k];
        k = k + 1;
    }
    print(all_ok);
    // corrupto: error pegajoso
    let bad = inflate.inflate_stream();
    match (inflate.stream_push(bad, b"\x07\x00")) { Result.Ok(b) => print("no error"), Result.Err(e) => print(e), }
    match (inflate.stream_push(bad, b"\x00")) { Result.Ok(b) => print("no error"), Result.Err(e) => print("sticky"), }
    0
}
"#;

const WANT: &str = "true\ntrue\ntrue\nreserved DEFLATE block type (3)\nsticky\n";

const ORACLE: &str = r#"
import zlib, sys
d = sys.argv[1]
msgs = [b"frame uno " * 30, b"frame dos con frame uno frame uno", bytes(range(256)) * 3, b"x" * 40000, b"ultimo frame con mas frame uno"]
c = zlib.compressobj(6)
segs = [c.compress(m) + c.flush(zlib.Z_SYNC_FLUSH) for m in msgs]
tail = c.flush(zlib.Z_FINISH)
open(d + "/stream.bin", "wb").write(b"".join(segs) + tail)
open(d + "/expected.bin", "wb").write(b"".join(msgs))
# el último segmento incluye el cierre del stream (para el push por mensaje)
lens = [len(s) for s in segs]; lens[-1] += len(tail)
open(d + "/seg_lens.txt", "w").write(" ".join(map(str, lens)))
open(d + "/msg_lens.txt", "w").write(" ".join(str(len(m)) for m in msgs))
"#;

fn prepare(name: &str) -> Option<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("ray_inflate_stream_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crea el directorio temporal");
    let py = Command::new("python3").arg("-c").arg(ORACLE).arg(&dir).output();
    match py {
        Ok(o) if o.status.success() => {}
        _ => {
            eprintln!("saltando inflate incremental: python3 no disponible");
            return None;
        }
    }
    std::fs::write(dir.join("prog.ray"), PROGRAM).expect("escribe el programa");
    Some(dir)
}

fn run(dir: &std::path::Path, flag: &str) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg(flag)
        .arg(dir.join("prog.ray"))
        .current_dir(dir)
        .output()
        .expect("lanza raylang");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn vm_and_interpreter_decode_sync_flushed_streams_incrementally() {
    let Some(dir) = prepare("engines") else { return };
    let (vm, err, code) = run(&dir, "--vm");
    assert_eq!(vm, WANT, "VM\n{err}");
    assert_eq!(code, 0);
    let (interp, err, code) = run(&dir, "--interp");
    assert_eq!(interp, WANT, "intérprete\n{err}");
    assert_eq!(code, 0);
}

#[test]
fn native_decodes_sync_flushed_streams_incrementally() {
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando native inflate incremental: rustc no disponible");
        return;
    }
    let Some(dir) = prepare("native") else { return };
    let bin = dir.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new(env!("CARGO_BIN_EXE_ray"))
        .args(["build", "--native", dir.join("prog.ray").to_str().unwrap(), "-o", bin.to_str().unwrap()])
        .output()
        .expect("lanza ray build");
    assert!(build.status.success(), "build --native\n{}", String::from_utf8_lossy(&build.stderr));
    let out = Command::new(&bin).current_dir(&dir).output().expect("corre el binario nativo");
    assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
}
