//! M131 — normalización Unicode (NFC/NFD/NFKC/NFKD) para `std/text`.
//!
//! El hallazgo de raysite (IDEAS §71.5): sin normalización, un slugify translitera a mano y dos
//! strings visualmente idénticos ("é" precompuesto vs "e"+combinante) no comparan iguales. Las
//! tablas de descomposición/composición de Unicode son exactamente la clase de datos que no se
//! transcribe a mano: el crate `unicode-normalization` (del proyecto unicode-rs, el que usan
//! rustc y servo) las trae generadas de UnicodeData.txt.
//!
//! Compartido por los DOS motores (VM vía la feature `unicode` de la toolchain; el binario
//! transpilado la activa por USO) → salida byte-idéntica por construcción.

#[cfg(feature = "unicode")]
mod real {
    use unicode_normalization::UnicodeNormalization;

    /// Normaliza `s` a la forma pedida: `"nfc"`, `"nfd"`, `"nfkc"` o `"nfkd"`.
    pub fn normalize(s: &str, form: &str) -> Result<String, String> {
        Ok(match form {
            "nfc" => s.nfc().collect(),
            "nfd" => s.nfd().collect(),
            "nfkc" => s.nfkc().collect(),
            "nfkd" => s.nfkd().collect(),
            other => return Err(format!("unknown normalization form: '{}'", other)),
        })
    }
}

#[cfg(feature = "unicode")]
pub use real::*;

/// Stub sin la feature (build slim): error claro, nunca una normalización que "pasa" en silencio.
#[cfg(not(feature = "unicode"))]
pub fn normalize(_s: &str, _form: &str) -> Result<String, String> {
    Err("this binary was built without Unicode normalization support (rebuild with the 'unicode' feature)".to_string())
}
