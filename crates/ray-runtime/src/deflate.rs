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
//!
//! M415 (rayzip #189): el INFLATE INCREMENTAL también vive aquí. `std/inflate` lo hacía en raylang
//! puro (15× más lento que `inflate_raw` en nativo, 1350× en la VM). Como un stream a medias es
//! estado (las tablas Huffman del bloque en curso, la ventana), hay un `InflateState` de miniz por
//! HANDLE en una tabla del proceso: `"stream_new"` (→ handle, 8 octetos BE), `"stream_push"` (n =
//! handle; `data` = tope de salida de este push en 8 octetos BE + el trozo; → 1 octeto de estado +
//! 8 BE de entrada consumida + lo producido; estado 0 = sigue, 1 = bloque final visto, 2 = stream
//! corrupto, 3 = tope rebasado) y `"stream_close"` (n = handle; libera). El runtime libera el handle
//! solo al terminar o fallar (ya no puede producir nada); un stream de sesión que se abandona a
//! medias sin `stream_close` es memoria perdida — por eso la API raylang expone el cierre explícito.

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
        "stream_new" => Some(stream_new().to_be_bytes().to_vec()),
        "stream_push" => stream_push(n as u64, data),
        "stream_close" => {
            stream_close(n as u64);
            Some(Vec::new())
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

/// M415: los streams incrementales abiertos, por handle. Un `Mutex` del proceso: cada `push`
/// toma el candado lo que dura la llamada (un stream no se usa desde dos fibras a la vez; el
/// candado solo protege la tabla).
#[cfg(feature = "deflate")]
type Streams = std::sync::Mutex<std::collections::HashMap<u64, Box<miniz_oxide::inflate::stream::InflateState>>>;
#[cfg(feature = "deflate")]
fn streams() -> &'static Streams {
    static STREAMS: std::sync::OnceLock<Streams> = std::sync::OnceLock::new();
    STREAMS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// M415: abre un stream DEFLATE crudo (la cabecera zlib la consume `std/inflate`, como hasta ahora)
/// y devuelve su handle (desde 1; 0 significa «sin runtime» en raylang).
#[cfg(feature = "deflate")]
fn stream_new() -> u64 {
    use miniz_oxide::inflate::stream::InflateState;
    use miniz_oxide::DataFormat;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    streams().lock().unwrap_or_else(|e| e.into_inner()).insert(id, InflateState::new_boxed(DataFormat::Raw));
    id
}

/// M415: libera el stream `id` (no-op si no existe o ya se liberó).
#[cfg(feature = "deflate")]
fn stream_close(id: u64) {
    streams().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
}

/// M415: alimenta el stream `id` con `data` = tope (8 BE) + trozo. Devuelve `[estado][consumidos
/// 8 BE][producido…]`; `None` si el handle no existe (raylang cae a su decodificador). Con miniz el
/// bloque a medias NO se descarta: lo decodificado sale en este push y el resto en el siguiente. Al
/// ver el bloque final (estado 1) o un error (2, 3) el stream se libera aquí mismo.
#[cfg(feature = "deflate")]
fn stream_push(id: u64, data: &[u8]) -> Option<Vec<u8>> {
    use miniz_oxide::inflate::stream::inflate;
    use miniz_oxide::{MZError, MZFlush, MZStatus};
    if data.len() < 8 {
        return None;
    }
    let limit = u64::from_be_bytes(data[..8].try_into().ok()?) as usize;
    let input = &data[8..];
    let mut table = streams().lock().unwrap_or_else(|e| e.into_inner());
    let state = table.get_mut(&id)?;
    let mut out = vec![0u8; 9];
    let mut used = 0usize;
    let mut buf = [0u8; 32 * 1024];
    let status: u8 = loop {
        let r = inflate(state, &input[used..], &mut buf, MZFlush::None);
        used += r.bytes_consumed;
        out.extend_from_slice(&buf[..r.bytes_written]);
        if out.len() - 9 > limit {
            break 3;
        }
        // rayzip #223: `inflate` de miniz VUELVE en cuanto vacía la salida pendiente de su
        // diccionario (32 KiB), sin tocar la entrada: un `bytes_written` menor que el búfer NO
        // significa «pide más entrada». Se sigue mientras haya progreso; la entrada agotada sin
        // salida pendiente da `Err(Buf)` (la vuelta siguiente) o una vuelta sin progreso.
        match r.status {
            Ok(MZStatus::StreamEnd) => break 1,
            Ok(_) if r.bytes_consumed == 0 && r.bytes_written == 0 => break 0,
            Ok(_) => continue,
            // Sin progreso posible con lo que hay: hace falta más entrada.
            Err(MZError::Buf) => break 0,
            Err(_) => break 2,
        }
    };
    if status != 0 {
        table.remove(&id);
    }
    out[0] = status;
    out[1..9].copy_from_slice(&(used as u64).to_be_bytes());
    Some(out)
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

    /// rayzip #223: un stream MUY comprimible (MB de salida por KB de entrada) troceado en 1, 7,
    /// 100, 2000 y 64 KiB octetos da lo mismo que `inflate` de una vez y consume toda la entrada.
    #[test]
    fn a_highly_compressible_stream_survives_any_chunking() {
        let handle = || u64::from_be_bytes(op("stream_new", &[], 0).unwrap().try_into().unwrap());
        let mut plain = vec![b'a'; 3_000_000];
        plain.extend((0..200_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8));
        let z = op("deflate", &plain, 6).expect("deflate");
        for size in [1usize, 7, 100, 2000, 65536] {
            let h = handle();
            let (mut out, mut pos, mut finished) = (Vec::new(), 0usize, false);
            while pos < z.len() {
                let end = (pos + size).min(z.len());
                let mut req = (1u64 << 40).to_be_bytes().to_vec();
                req.extend_from_slice(&z[pos..end]);
                let r = op("stream_push", &req, h as i64).expect("push");
                let used = u64::from_be_bytes(r[1..9].try_into().unwrap()) as usize;
                out.extend_from_slice(&r[9..]);
                match r[0] {
                    0 => assert_eq!(used, end - pos, "trozo {size}: entrada sin consumir"),
                    1 => finished = true,
                    s => panic!("trozo {size}: estado {s}"),
                }
                pos = end;
            }
            assert!(finished, "trozo {size}: sin bloque final");
            assert_eq!(out.len(), plain.len(), "trozo {size}");
            assert!(out == plain, "trozo {size}: contenido distinto");
        }
    }

    /// M415: el stream incremental produce lo mismo que `inflate` aunque la entrada llegue partida
    /// en cualquier octeto, señala el bloque final con lo que sobraba detrás y respeta el tope.
    #[test]
    fn an_incremental_stream_decodes_in_arbitrary_pieces_and_reports_the_end() {
        let handle = || u64::from_be_bytes(op("stream_new", &[], 0).unwrap().try_into().unwrap());
        let plain: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        let z = op("deflate", &plain, 6).expect("deflate");
        let mut data = z.clone();
        data.extend_from_slice(b"TRAILER");
        let h = handle();
        let mut out = Vec::new();
        let (mut pos, mut finished, mut remaining) = (0usize, false, Vec::new());
        for n in [1usize, 7, 100, 3, 5000, 33, 2, 9999, 1, 1].iter().cycle() {
            if pos >= data.len() || finished {
                break;
            }
            let end = (pos + n).min(data.len());
            let mut req = (1u64 << 30).to_be_bytes().to_vec();
            req.extend_from_slice(&data[pos..end]);
            let r = op("stream_push", &req, h as i64).expect("push");
            let used = u64::from_be_bytes(r[1..9].try_into().unwrap()) as usize;
            out.extend_from_slice(&r[9..]);
            match r[0] {
                0 => assert_eq!(used, end - pos),
                1 => {
                    finished = true;
                    remaining = data[pos + used..].to_vec();
                }
                s => panic!("estado {s}"),
            }
            pos = end;
        }
        assert!(finished);
        assert_eq!(out, plain);
        assert_eq!(remaining, b"TRAILER");
        // Liberado al terminar: otro push no encuentra el handle.
        assert!(op("stream_push", &[0u8; 9], h as i64).is_none());
        // Tope por push: rebasarlo es estado 3 y libera.
        let h2 = handle();
        let mut req = 100u64.to_be_bytes().to_vec();
        req.extend_from_slice(&z);
        assert_eq!(op("stream_push", &req, h2 as i64).unwrap()[0], 3);
        assert!(op("stream_push", &[0u8; 9], h2 as i64).is_none());
        // Corrupto: tipo de bloque reservado → 2.
        let h3 = handle();
        let mut req = (1u64 << 20).to_be_bytes().to_vec();
        req.extend_from_slice(&[0x07, 0x00]);
        assert_eq!(op("stream_push", &req, h3 as i64).unwrap()[0], 2);
        // `stream_close` sobre uno vivo lo quita.
        let h4 = handle();
        op("stream_close", &[], h4 as i64);
        assert!(op("stream_push", &[0u8; 9], h4 as i64).is_none());
    }

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
