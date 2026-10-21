//! M253 — DEFLATE de PRODUCCIÓN para `std/deflate` y `std/inflate`: compresión/descompresión con
//! `miniz_oxide` (Rust puro, sin `unsafe`) y las sumas CRC-32/Adler-32 a mano. Motivo: `ray release`
//! comprime el bundle en la VM y el LZ77 escrito en raylang tardaba 241 s por cada 500 KB (nativo: 64
//! ms). Los módulos raylang conservan su algoritmo como RESPALDO: si esta primitiva devuelve `None`
//! (build sin la feature, wasm32, `--without deflate`, o un stream que miniz rechaza), el código
//! raylang sigue produciendo el resultado — y en el caso de error, el MISMO mensaje de siempre.
//!
//! Una sola primitiva `op(nombre, data, n)`: `"deflate"` (n = nivel 0..10), `"inflate"` (n = tope de
//! salida en octetos, anti-bomba), `"crc32"` y `"adler32"` (4 octetos big-endian). Determinista →
//! oráculo VM↔nativo por byte-identidad. rayzip (#188–#192): `"deflate_sync"` (un trozo que acaba en
//! un flush de sincronización, sin BFINAL: los trozos se concatenan en un solo stream),
//! `"inflate_consumed"` (los octetos de entrada usados, 8 octetos BE, delante de la salida) y
//! `"crc32_update"` (n = la CRC acumulada; sigue sumando `data`).

#[cfg(feature = "deflate")]
pub fn op(name: &str, data: &[u8], n: i64) -> Option<Vec<u8>> {
    match name {
        "deflate" => {
            let level = n.clamp(0, 10) as u8;
            Some(miniz_oxide::deflate::compress_to_vec(data, level))
        }
        "inflate" => {
            // M293 (IDEAS §96 #6): `n <= 0` acota a CERO octetos, no a "sin tope". Antes era
            // `usize::MAX`, y `std/zip` pasaba como tope el `size` declarado en la cabecera: un ZIP
            // con `size = 0` y datos deflate reales descomprimía sin límite (bomba) antes de que la
            // comprobación de tamaño lo rechazara. Así el camino rápido coincide con el inflater en
            // raylang (que con `limit` 0 rechaza el primer octeto emitido).
            let limit = n.max(0) as usize;
            miniz_oxide::inflate::decompress_to_vec_with_limit(data, limit).ok()
        }
        "deflate_sync" => Some(deflate_sync(data, n.clamp(0, 10) as i32)),
        "inflate_consumed" => {
            let (out, used) = inflate_consumed(data, n.max(0) as usize)?;
            let mut r = (used as u64).to_be_bytes().to_vec();
            r.extend_from_slice(&out);
            Some(r)
        }
        "crc32_update" => Some(crc32_with((n & 0xFFFF_FFFF) as u32, data).to_be_bytes().to_vec()),
        "crc32" => Some(crc32(data).to_be_bytes().to_vec()),
        "adler32" => Some(adler32(data).to_be_bytes().to_vec()),
        _ => None,
    }
}

#[cfg(not(feature = "deflate"))]
pub fn op(_name: &str, _data: &[u8], _n: i64) -> Option<Vec<u8>> {
    None
}

/// rayzip #190: comprime `data` como un trozo de un stream DEFLATE crudo terminado en un flush de
/// sincronización (un bloque almacenado vacío, alineado a octeto) y SIN el bit BFINAL: la
/// concatenación de trozos, cerrada por uno normal (`"deflate"`), es un único stream válido. Cada
/// trozo empieza con el diccionario vacío (como `pigz` sin `--independent` en sentido contrario):
/// algo menos de compresión a cambio de memoria acotada y trozos comprimibles en paralelo.
#[cfg(feature = "deflate")]
fn deflate_sync(data: &[u8], level: i32) -> Vec<u8> {
    use miniz_oxide::deflate::core::{compress_to_output, create_comp_flags_from_zip_params, CompressorOxide, TDEFLFlush};
    let mut c = CompressorOxide::new(create_comp_flags_from_zip_params(level, -15, 0));
    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    let _ = compress_to_output(&mut c, data, TDEFLFlush::Sync, |chunk| {
        out.extend_from_slice(chunk);
        true
    });
    out
}

/// rayzip #191: como `"inflate"`, y además cuántos octetos de entrada ocupó el stream (lo que va
/// detrás —el tráiler de un miembro gzip y el siguiente miembro— queda sin tocar).
#[cfg(feature = "deflate")]
fn inflate_consumed(mut input: &[u8], limit: usize) -> Option<(Vec<u8>, usize)> {
    use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
    use miniz_oxide::inflate::TINFLStatus;
    let flags = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let mut out: Vec<u8> = vec![0; input.len().saturating_mul(2).clamp(1, limit.max(1))];
    let mut d = Box::<DecompressorOxide>::default();
    let (mut out_pos, mut used) = (0usize, 0usize);
    loop {
        let (status, in_consumed, out_consumed) = decompress(&mut d, input, &mut out, out_pos, flags);
        out_pos += out_consumed;
        used += in_consumed;
        match status {
            TINFLStatus::Done => {
                if out_pos > limit {
                    return None;
                }
                out.truncate(out_pos);
                return Some((out, used));
            }
            TINFLStatus::HasMoreOutput if in_consumed <= input.len() && out.len() < limit => {
                input = &input[in_consumed..];
                let new_len = out.len().saturating_mul(2).min(limit);
                out.resize(new_len, 0);
            }
            _ => return None,
        }
    }
}

/// CRC-32 (RFC 1952, polinomio reflejado 0xEDB88320), un octeto por paso sobre una tabla de 256.
#[cfg(feature = "deflate")]
fn crc32(data: &[u8]) -> u32 {
    crc32_with(0, data)
}

/// rayzip #192: continúa la CRC-32 `prev` (la de lo ya sumado; 0 al empezar) con `data` — la forma
/// encadenable de zlib (`crc32(crc, buf, len)`): `crc32_with(crc32(a), b) == crc32(a ++ b)`.
#[cfg(feature = "deflate")]
fn crc32_with(prev: u32, data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = !prev;
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
        assert_eq!(op("inflate", &z, text.len() as i64).expect("inflate at the limit").len(), text.len());
        // M293: 0 (o negativo) NO es "sin tope": es tope cero → cualquier salida lo rebasa.
        assert!(op("inflate", &z, 0).is_none(), "limit 0 rejects any output");
        assert!(op("inflate", &z, -1).is_none(), "negative limit rejects any output");
        let empty = op("deflate", b"", 6).expect("deflate empty");
        assert_eq!(op("inflate", &empty, 0).expect("empty stream fits in a zero cap").len(), 0);
        assert!(op("inflate", &z, 10).is_none(), "over the cap → None (the raylang path reports the error)");
        assert!(op("inflate", b"\xff\xff\xff", 0).is_none());
        assert!(op("nope", b"", 0).is_none());
    }

    #[test]
    fn sync_chunks_concatenate_into_one_stream_and_consumed_stops_at_its_end() {
        let a = b"first chunk, first chunk, first chunk ".repeat(20);
        let b = b"second and last".to_vec();
        let mut stream = op("deflate_sync", &a, 6).expect("sync chunk");
        stream.extend(op("deflate", &b, 6).expect("final chunk"));
        let whole = [a.clone(), b.clone()].concat();
        assert_eq!(op("inflate", &stream, 1 << 20).expect("one stream"), whole);
        let tail = b"TRAILING";
        let r = op("inflate_consumed", &[stream.clone(), tail.to_vec()].concat(), 1 << 20).expect("consumed");
        let used = u64::from_be_bytes(r[..8].try_into().unwrap()) as usize;
        assert_eq!(used, stream.len(), "the bytes after the final block are not consumed");
        assert_eq!(&r[8..], &whole[..]);
        assert_eq!(crc32_with(crc32(&a), &b), crc32(&whole));
    }
}
