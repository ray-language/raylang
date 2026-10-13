//! base64 (RFC 4648) en Rust, M380 (rayplay #151): la stdlib `std/base64` escrita en raylang
//! decodificaba a ~15 MB/s en nativo (~1,7 MB/s en la VM) y retenía ~14× la entrada (un `[int]`
//! por octeto antes de `bytes_of`): 75 MB de base64 eran 1,1 GB de pico en un iPhone con 2 GB por
//! proceso. Aquí: una pasada, `len * 3 / 4` reservados y nada más. Sin dependencias ni features: el
//! mismo archivo se incluye TEXTUALMENTE en el programa nativo (`include_str!`, módulo `__ray_b64`),
//! así que solo puede usar `std`. Los mensajes de error son los de la versión raylang (los tests y
//! el handshake SCRAM/JWT los aseveran); la decodificación sigue siendo ESTRICTA (M59.3): un solo
//! encoding válido por payload.

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Codifica `data`: alfabeto estándar con relleno `=` (`url == false`) o base64url sin relleno.
pub fn encode(data: &[u8], url: bool) -> String {
    let table = if url { URL } else { STD };
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut chunks = data.chunks_exact(3);
    for c in &mut chunks {
        let triple = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
        out.push(table[(triple >> 18 & 63) as usize] as char);
        out.push(table[(triple >> 12 & 63) as usize] as char);
        out.push(table[(triple >> 6 & 63) as usize] as char);
        out.push(table[(triple & 63) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let triple = (rem[0] as u32) << 16;
            out.push(table[(triple >> 18 & 63) as usize] as char);
            out.push(table[(triple >> 12 & 63) as usize] as char);
            if !url {
                out.push_str("==");
            }
        }
        2 => {
            let triple = (rem[0] as u32) << 16 | (rem[1] as u32) << 8;
            out.push(table[(triple >> 18 & 63) as usize] as char);
            out.push(table[(triple >> 12 & 63) as usize] as char);
            out.push(table[(triple >> 6 & 63) as usize] as char);
            if !url {
                out.push('=');
            }
        }
        _ => {}
    }
    out
}

/// El valor de 6 bits de un carácter del alfabeto (estándar o url), o `None`.
fn value(c: u8, url: bool) -> Option<u32> {
    let v = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' if !url => 62,
        b'/' if !url => 63,
        b'-' if url => 62,
        b'_' if url => 63,
        _ => return None,
    };
    Some(v as u32)
}

/// Decodifica, ESTRICTO. Estándar (`url == false`): relleno `=` obligatorio hasta múltiplo de 4,
/// nada tras el relleno, como mucho dos `=`, bits sobrantes a cero. base64url (`url == true`): sin
/// relleno (`=` es un carácter inválido), una cola de 1 carácter es imposible, bits sobrantes a cero.
pub fn decode(s: &str, url: bool) -> Result<Vec<u8>, String> {
    decode_bytes(s.as_bytes(), url)
}

/// M395 (rayplay #161): la misma decodificación sobre un tramo de `bytes` ya en memoria (el cuerpo
/// de una petición), sin pasar por `string`: ni validación UTF-8 ni copia del tramo.
pub fn decode_bytes(bytes: &[u8], url: bool) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() / 4 * 3 + 2);
    let mut acc: u32 = 0;
    let mut nbits: u32 = 0;
    let mut chars: usize = 0;
    let mut pad: usize = 0;
    for &c in bytes {
        if c == b'=' && !url {
            pad += 1;
            continue;
        }
        if pad > 0 {
            return Err("base64: data after '=' padding".to_string());
        }
        let Some(v) = value(c, url) else {
            return Err(if url { "invalid base64url character" } else { "invalid base64 character" }.to_string());
        };
        acc = (acc << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((acc >> nbits & 255) as u8);
            acc &= (1 << nbits) - 1;
        }
        chars += 1;
    }
    if url {
        if chars % 4 == 1 {
            return Err("base64url: invalid length".to_string());
        }
        if nbits > 0 && acc & ((1 << nbits) - 1) != 0 {
            return Err("base64url: non-zero trailing bits (non-canonical encoding)".to_string());
        }
    } else {
        if !(chars + pad).is_multiple_of(4) {
            return Err("base64: invalid length (not a multiple of 4)".to_string());
        }
        if pad > 2 {
            return Err("base64: invalid '=' padding".to_string());
        }
        if nbits > 0 && acc & ((1 << nbits) - 1) != 0 {
            return Err("base64: non-zero trailing bits (non-canonical encoding)".to_string());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn round_trips_and_rfc_vectors() {
        for (raw, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(encode(raw.as_bytes(), false), enc);
            assert_eq!(decode(enc, false).unwrap(), raw.as_bytes());
            let u = enc.trim_end_matches('=').replace('+', "-").replace('/', "_");
            assert_eq!(encode(raw.as_bytes(), true), u);
            assert_eq!(decode(&u, true).unwrap(), raw.as_bytes());
        }
        assert_eq!(encode(&[0xfb, 0xff], false), "+/8=");
        assert_eq!(encode(&[0xfb, 0xff], true), "-_8");
    }

    #[test]
    fn strict_decoding_keeps_the_raylang_messages() {
        assert_eq!(decode("Zm9v*", false).unwrap_err(), "invalid base64 character");
        assert_eq!(decode("Zg==Zg", false).unwrap_err(), "base64: data after '=' padding");
        assert_eq!(decode("Zm9", false).unwrap_err(), "base64: invalid length (not a multiple of 4)");
        assert_eq!(decode("Z===", false).unwrap_err(), "base64: invalid '=' padding");
        assert_eq!(decode("Zh==", false).unwrap_err(), "base64: non-zero trailing bits (non-canonical encoding)");
        assert_eq!(decode("Zg==", true).unwrap_err(), "invalid base64url character");
        assert_eq!(decode("Z", true).unwrap_err(), "base64url: invalid length");
        assert_eq!(decode("Zh", true).unwrap_err(), "base64url: non-zero trailing bits (non-canonical encoding)");
    }
}
