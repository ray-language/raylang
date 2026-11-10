//! M446 (ray-sublime #317) — codificaciones heredadas (`std/charset`): Windows-1252, ISO-8859-x,
//! KOI8-R, Shift_JIS, EUC-JP, EUC-KR, GBK, GB18030, Big5… por las tablas del **WHATWG Encoding
//! Standard** que trae `encoding_rs` (el motor de Firefox y de `TextDecoder`/`TextEncoder`).
//!
//! Las tablas multibyte tienen miles de entradas: no se transcriben a mano. Sin feature (build
//! slim, `--without charset`, wasm) el primitivo devuelve `Err` claro y `std/charset` lo
//! propaga. Compartido por los DOS motores (VM vía la feature `charset` de la toolchain; el
//! binario transpilado la activa por USO) → salida byte-idéntica por construcción.
//!
//! Contrato del primitivo: `op(op, label, data)` con `op` = `"decode"` (`data` son los octetos,
//! devuelve UTF-8), `"encode"` (`data` es el texto en UTF-8, devuelve los octetos) o `"labels"`
//! (devuelve los nombres canónicos, uno por línea). Un octeto malformado al decodificar o un
//! carácter sin representación al codificar es `Err` con la posición — como hace Sublime al
//! guardar — en vez de un U+FFFD o un `?` silencioso.

#[cfg(feature = "charset")]
mod real {
    use encoding_rs::{DecoderResult, Encoding, EncoderResult};

    /// Las etiquetas que el WHATWG Encoding Standard reconoce, una por codificación canónica (el
    /// resto de alias — `latin1`, `cp1252`, `sjis`, `ascii`… — también resuelven por `for_label`).
    const CANONICAL: &[&str] = &[
        "UTF-8", "IBM866", "ISO-8859-2", "ISO-8859-3", "ISO-8859-4", "ISO-8859-5", "ISO-8859-6",
        "ISO-8859-7", "ISO-8859-8", "ISO-8859-8-I", "ISO-8859-10", "ISO-8859-13", "ISO-8859-14",
        "ISO-8859-15", "ISO-8859-16", "KOI8-R", "KOI8-U", "macintosh", "windows-874", "windows-1250",
        "windows-1251", "windows-1252", "windows-1253", "windows-1254", "windows-1255", "windows-1256",
        "windows-1257", "windows-1258", "x-mac-cyrillic", "GBK", "gb18030", "Big5", "EUC-JP",
        "ISO-2022-JP", "Shift_JIS", "EUC-KR", "replacement", "UTF-16BE", "UTF-16LE", "x-user-defined",
    ];

    fn encoding(label: &str) -> Result<&'static Encoding, String> {
        Encoding::for_label(label.trim().as_bytes()).ok_or_else(|| format!("unknown charset '{label}' (labels() lists the known ones)"))
    }

    /// Decodifica `data` según `label` a UTF-8, sin tratar el BOM (la etiqueta manda). Un
    /// octeto malformado es `Err` con su posición.
    pub fn decode(data: &[u8], label: &str) -> Result<Vec<u8>, String> {
        let enc = encoding(label)?;
        let mut decoder = enc.new_decoder_without_bom_handling();
        let cap = decoder.max_utf8_buffer_length_without_replacement(data.len()).ok_or("input too large")?;
        let mut out = String::with_capacity(cap);
        let (result, read) = decoder.decode_to_string_without_replacement(data, &mut out, true);
        match result {
            DecoderResult::InputEmpty => Ok(out.into_bytes()),
            DecoderResult::Malformed(len, _) => Err(format!("malformed {} sequence at byte {}", enc.name(), read.saturating_sub(len as usize))),
            DecoderResult::OutputFull => Err("decoder output buffer too small (internal)".to_string()),
        }
    }

    /// Codifica `text` (UTF-8) según `label`. Un carácter sin representación es `Err` con el
    /// carácter y su índice. `replacement` y UTF-16 no tienen codificador en el estándar (WHATWG
    /// los codifica como UTF-8): se rechazan para no mentir.
    pub fn encode(text: &str, label: &str) -> Result<Vec<u8>, String> {
        let enc = encoding(label)?;
        if enc.output_encoding() != enc {
            return Err(format!("'{}' has no encoder (the WHATWG standard encodes it as UTF-8): use \"utf-8\"", enc.name()));
        }
        let mut encoder = enc.new_encoder();
        let cap = encoder.max_buffer_length_from_utf8_without_replacement(text.len()).ok_or("input too large")?;
        let mut out = Vec::with_capacity(cap);
        let (result, read) = encoder.encode_from_utf8_to_vec_without_replacement(text, &mut out, true);
        match result {
            EncoderResult::InputEmpty => Ok(out),
            // `read` incluye el carácter que no cupo: su índice es el anterior.
            EncoderResult::Unmappable(c) => Err(format!("'{}' (U+{:04X}) cannot be encoded in {} at char index {}", c, c as u32, enc.name(), text[..read].chars().count().saturating_sub(1))),
            EncoderResult::OutputFull => Err("encoder output buffer too small (internal)".to_string()),
        }
    }

    /// Los nombres canónicos conocidos, uno por línea, en el orden del estándar.
    pub fn labels() -> Vec<u8> {
        CANONICAL.join("\n").into_bytes()
    }

    pub fn op(op: &str, label: &str, data: &[u8]) -> Result<Vec<u8>, String> {
        match op {
            "decode" => decode(data, label),
            "encode" => encode(std::str::from_utf8(data).map_err(|_| "encode: the text is not UTF-8".to_string())?, label),
            "labels" => Ok(labels()),
            other => Err(format!("unknown charset op '{other}'")),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn single_byte_and_multibyte_round_trip() {
            assert_eq!(decode(&[0x63, 0x61, 0x66, 0xE9], "windows-1252").unwrap(), "café".as_bytes());
            assert_eq!(encode("café €", "cp1252").unwrap(), vec![0x63, 0x61, 0x66, 0xE9, 0x20, 0x80]);
            let sjis = encode("日本語", "shift_jis").unwrap();
            assert_eq!(sjis, vec![0x93, 0xFA, 0x96, 0x7B, 0x8C, 0xEA]);
            assert_eq!(decode(&sjis, "sjis").unwrap(), "日本語".as_bytes());
            assert_eq!(decode(b"\xC4\xE3\xBA\xC3", "gbk").unwrap(), "你好".as_bytes());
            assert_eq!(decode(b"\xD0\xBF\xD1\x80", "utf-8").unwrap(), "пр".as_bytes());
        }

        #[test]
        fn errors_carry_the_position_and_unknown_labels_fail() {
            // WHATWG: `iso-8859-1`/`latin1` SON windows-1252 (el € entra como 0x80); un kanji no.
            assert_eq!(encode("a€b", "iso-8859-1").unwrap(), vec![0x61, 0x80, 0x62]);
            assert_eq!(encode("a日b", "latin1").unwrap_err(), "'日' (U+65E5) cannot be encoded in windows-1252 at char index 1");
            assert_eq!(decode(&[0x41, 0x82], "shift_jis").unwrap_err(), "malformed Shift_JIS sequence at byte 1");
            assert!(decode(b"\xFF", "utf-8").unwrap_err().starts_with("malformed UTF-8 sequence at byte 0"));
            assert!(encode("x", "klingon").unwrap_err().starts_with("unknown charset 'klingon'"));
            assert!(encode("x", "utf-16le").unwrap_err().contains("has no encoder"));
            assert_eq!(decode(b"a\x00b\x00", "utf-16le").unwrap(), b"ab");
            let labels = String::from_utf8(labels()).unwrap();
            assert!(labels.lines().any(|l| l == "Shift_JIS") && labels.lines().all(|l| Encoding::for_label(l.as_bytes()).is_some()));
        }
    }
}

#[cfg(feature = "charset")]
pub use real::*;

#[cfg(not(feature = "charset"))]
pub fn op(_op: &str, _label: &str, _data: &[u8]) -> Result<Vec<u8>, String> {
    Err("charset conversion is not available in this build (feature 'charset')".to_string())
}
