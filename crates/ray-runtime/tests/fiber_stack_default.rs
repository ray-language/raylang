//! El default programático de pila de fibra (`set_default_fiber_stack_kib`) — en un binario de
//! test PROPIO a propósito: el tamaño se decide UNA vez por proceso (OnceLock) y dentro del crate
//! competiría con los demás tests de fibras por quién lo inicializa primero.
#![cfg(feature = "fibers")]

use ray_runtime::fibers;

/// ~64 KiB LÓGICOS de marco por nivel (en debug, black_box añade copias: cuenta el doble o más):
/// 16 niveles ≈ 1 MiB — desborda con margen los 128 KiB del default pelado (la página de guarda
/// mataría el proceso) y cabe holgado en los 4 MiB que fija el test.
fn recurse(depth: usize) -> u8 {
    let buf = std::hint::black_box([0u8; 64 * 1024]);
    if depth == 0 { buf[0] } else { recurse(depth - 1) ^ buf[depth] }
}

#[test]
fn programmatic_default_gives_c_sized_fiber_stacks() {
    // Como el binario emitido con externs: fijar ANTES de la primera fibra. (4 MiB y no el 1 MiB
    // del emitido: aquí se verifica el SETTER, con margen para el overhead de marcos de debug.)
    fibers::set_default_fiber_stack_kib(4096);
    let h = fibers::spawn(|| {
        assert_eq!(recurse(15), 0);
    });
    h.join().expect("la fibra con la pila programática completa la recursión profunda");
}
