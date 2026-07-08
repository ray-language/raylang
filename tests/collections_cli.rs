//! M50.2 — `std/collections/{set,deque,stringbuilder}` importables. Las colecciones (puras en raylang,
//! sin primitivos `__x`) salieron del prelude a submódulos opt-in con leaf-binding (`import
//! std/collections/set;` → `set.new()`). Como el uso pasa por el **loader** (resuelve el import), el
//! oráculo VM↔intérprete de `vm.rs` (que es pre-loader) ya no aplica: aquí se corre cada ejemplo por
//! **ambos motores** en subproceso y se exige que coincidan y den la salida esperada.

use std::process::Command;

/// Corre un archivo `.ray` por el motor elegido; devuelve (stdout, código de salida).
fn run_file(path: &str, vm: bool) -> (String, i32) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    cmd.arg(if vm { "--vm" } else { "--interp" });
    let out = cmd.arg(path).output().expect("ejecuta raylang");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

/// Corre `src` (fuente inline en un temporal) por la VM; devuelve el código de salida.
fn run_src(name: &str, src: &str) -> i32 {
    let path = std::env::temp_dir().join(format!("{name}.ray"));
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&path)
        .output()
        .expect("ejecuta raylang");
    out.status.code().unwrap_or(-1)
}

/// Oráculo por subproceso: los dos motores coinciden (stdout + código) sobre el ejemplo, y la salida
/// es la esperada.
fn ambos_motores_coinciden(path: &str, esperado: &str) {
    let (o_in, c_in) = run_file(path, false);
    let (o_vm, c_vm) = run_file(path, true);
    assert_eq!(c_in, 0, "intérprete sale 0 ({path})\n{o_in}");
    assert_eq!(c_vm, 0, "vm sale 0 ({path})\n{o_vm}");
    assert_eq!(o_in, o_vm, "ambos motores coinciden ({path})");
    assert_eq!(o_in, esperado, "salida esperada ({path})");
}

#[test]
fn set_conjunto_ambos_motores() {
    ambos_motores_coinciden(
        "examples/stdlib/conjunto.ray",
        "7\ntrue\nfalse\nfalse\n6\n2\ntrue\n26\n",
    );
}

#[test]
fn stringbuilder_y_deque_ambos_motores() {
    ambos_motores_coinciden(
        "examples/stdlib/builder_deque.ray",
        "fila1\nfila2\nfila3\nfila4\nfila5\n\n15\n10\n20\nb\n3\n0\n",
    );
}

#[test]
fn nombres_globales_ya_no_existen() {
    // M50.2: la forma global del prelude se retiró; usar los nombres sin importar el submódulo es error.
    assert_ne!(
        run_src("m50_set_bad", "fn main() -> int { let s: Set<int> = set_new(); set_size(s) }"),
        0,
        "set_new/Set global debe fallar (ya no está en el prelude)"
    );
    assert_ne!(
        run_src("m50_deque_bad", "fn main() -> int { let d: Deque<int> = deque_new(); deque_len(d) }"),
        0,
        "deque_new/Deque global debe fallar"
    );
    assert_ne!(
        run_src("m50_sb_bad", "fn main() -> int { let b = sb_new(); sb_count(b) }"),
        0,
        "sb_new global debe fallar"
    );
}

#[test]
fn import_calificado_de_submodulo() {
    // El leaf-binding liga el último segmento: `import std/collections/set;` → `set.new()`.
    let src = "import std/collections/set;\n\
               fn main() -> int {\n\
               \x20 let s: set.Set<int> = set.new();\n\
               \x20 set.add(s, 1); set.add(s, 1); set.add(s, 2);\n\
               \x20 set.size(s)\n\
               }";
    assert_eq!(run_src("m50_set_ok", src), 2, "set con leaf-binding dedup → size 2");
}
