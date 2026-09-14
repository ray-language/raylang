//! M253 — DEFLATE de PRODUCCIÓN para `std/deflate` y `std/inflate`: compresión/descompresión con
//! `miniz_oxide` (Rust puro, sin `unsafe`) y las sumas CRC-32/Adler-32 a mano. Motivo: `ray release`
//! comprime el bundle en la VM y el LZ77 escrito en raylang tardaba 241 s por cada 500 KB (nativo: 64
//! ms). Los módulos raylang conservan su algoritmo como RESPALDO: si esta primitiva devuelve `None`
//! (build sin la feature, wasm32, `--without deflate`, o un stream que miniz rechaza), el código
//! raylang sigue produciendo el resultado — y en el caso de error, el MISMO mensaje de siempre.
//!
//! Una sola primitiva `op(nombre, data, n)`: `"deflate"` (n = nivel 0..10), `"inflate"` (n = tope de
//! salida en octetos, anti-bomba), `"crc32"` y `"adler32"` (4 octetos big-endian). Determinista →
//! oráculo VM↔nativo por byte-identidad.

#[cfg(feature = "deflate")]
pub fn op(name: &str, data: &[u8], n: i64) -> Option<Vec<u8>> {
    match name {
        "deflate" => {
            let level = n.clamp(0, 10) as u8;
            Some(miniz_oxide::deflate::compress_to_vec(data, level))
        }
        "inflate" => {
            let limit = if n <= 0 { usize::MAX } else { n as usize };
            miniz_oxide::inflate::decompress_to_vec_with_limit(data, limit).ok()
        }
        "crc32" => Some(crc32(data).to_be_bytes().to_vec()),
        "adler32" => Some(adler32(data).to_be_bytes().to_vec()),
        _ => None,
    }
}

#[cfg(not(feature = "deflate"))]
pub fn op(_name: &str, _data: &[u8], _n: i64) -> Option<Vec<u8>> {
    None
}

/// CRC-32 (RFC 1952, polinomio reflejado 0xEDB88320), un octeto por paso sobre una tabla de 256.
#[cfg(feature = "deflate")]
fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// Adler-32 (RFC 1950), con la reducción módulo 65521 diferida por bloques (NMAX = 5552).
#[cfg(feature = "deflate")]
fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

#[cfg(all(test, feature = "deflate"))]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_the_reference_vectors() {
        // "123456789": CRC-32 = 0xCBF43926, Adler-32 = 0x091E01DE.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"123456789"), 0x091E_01DE);
        assert_eq!(crc32(b""), 0);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn deflate_round_trips_and_inflate_honors_the_limit() {
        let text = b"raylang raylang raylang comprime y descomprime".repeat(50);
        let z = op("deflate", &text, 6).expect("deflate");
        assert!(z.len() < text.len());
        assert_eq!(op("inflate", &z, 0).expect("inflate").as_slice(), text.as_slice());
        assert_eq!(op("inflate", &z, text.len() as i64).expect("inflate at the limit").len(), text.len());
        assert!(op("inflate", &z, 10).is_none(), "over the cap → None (the raylang path reports the error)");
        assert!(op("inflate", b"\xff\xff\xff", 0).is_none());
        assert!(op("nope", b"", 0).is_none());
    }
}
