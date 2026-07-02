//! Alias `raylang` del ejecutable (M39a). Toda la lógica vive en `raylang::cli` para que
//! `ray` (el nombre de producto) y `raylang` (compatibilidad; los tests usan
//! `CARGO_BIN_EXE_raylang`) sean el mismo binario. Ver `src/bin/ray.rs`.
fn main() {
    raylang::cli::main();
}
