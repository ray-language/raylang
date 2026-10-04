//! M347 (rayauth R23) — **códigos QR** (`std/qr`) sobre el crate `qrcode` (Rust puro, determinista):
//! la toolchain ya lo usa para `ray dev --device`; aquí lo ve el programa. Solo la MATRIZ de módulos:
//! el render (texto, SVG, PNG) está escrito en raylang en `std/qr`. Sin la feature `qr` → `None`.

/// La matriz del QR de `text` con el nivel de corrección `ecl` (`"L"`, `"M"`, `"Q"`, `"H"`; otro →
/// `None`): filas de `'1'`/`'0'` (oscuro/claro), tantas como columnas (sin margen). `None` si el texto
/// no cabe en la versión 40 del nivel pedido.
#[cfg(feature = "qr")]
pub fn matrix(text: &str, ecl: &str) -> Option<Vec<String>> {
    let level = match ecl {
        "L" => qrcode::EcLevel::L,
        "M" => qrcode::EcLevel::M,
        "Q" => qrcode::EcLevel::Q,
        "H" => qrcode::EcLevel::H,
        _ => return None,
    };
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), level).ok()?;
    let w = code.width();
    let colors = code.to_colors();
    Some(
        (0..w)
            .map(|y| (0..w).map(|x| if colors[y * w + x] == qrcode::Color::Dark { '1' } else { '0' }).collect())
            .collect(),
    )
}
#[cfg(not(feature = "qr"))]
pub fn matrix(_text: &str, _ecl: &str) -> Option<Vec<String>> { None }
