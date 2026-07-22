//! **`ray-runtime`** — el runtime compartido de raylang con dependencias de crates externos.
//!
//! Ver `docs/transpilador-nativo.md` §4. Cada módulo envuelve un crate de producción tras una **feature**
//! (bajo demanda): lo consumen tanto la VM (el binario `ray`) como el binario **transpilado** a Rust, con
//! el MISMO código → paridad byte-idéntica por construcción. Las firmas son de tipos simples (`&[u8]`,
//! `Vec<u8>`, `i64`, `String`), sin `Value` ni GC: el borde (opcode / marshalling) vive en cada consumidor.
//!
//! Sin ninguna feature activa, el crate compila sin dependencias externas (cada función tiene un *stub*):
//! así un build "slim" de raylang (sin `net-tls`) no arrastra nada.

pub mod crypto;
pub mod sqlite;
pub mod tls;

// N1 — reexport de `MiMalloc` para el binario transpilado. El `#[global_allocator]` lo declara el
// `main.rs` GENERADO (no este crate): una dep que el binario no referencia NO se enlaza, y un
// `#[global_allocator]` aquí podría quedarse silenciosamente fuera. El reexport fuerza la referencia
// y deja la declaración a la vista en el Rust generado.
#[cfg(feature = "mimalloc")]
pub use mimalloc::MiMalloc;

// N2 — reexport del `RandomState` de aHash para los `Map` del binario transpilado (mismo hasher que el
// `MapStore` de la VM, P0.1). El alias `__RayMap` del Rust generado lo usa como tercer parámetro del
// `HashMap` std; el reexport evita que el programa generado dependa del crate `ahash` por su nombre.
#[cfg(feature = "ahash")]
pub use ahash::RandomState;
