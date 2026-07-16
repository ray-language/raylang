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
pub mod tls;
