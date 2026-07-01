//! Diagnósticos con contexto de fuente (M8.3; rangos en M33a).
//!
//! Las cuatro fases producen errores con `(mensaje, línea, columna, extensión)` y su
//! propio `Display` ("error de tipos en 3:9: …"). Este módulo añade lo que faltaba para
//! que un error sea fácil de localizar: **la línea de fuente y un subrayado bajo el
//! rango** (`^` repetido `len` veces; con `len = 1`, el cursor puntual de siempre).
//!
//! ```text
//! error de sintaxis en 2:13: se esperaba una expresión, se encontró While
//!   2 | let x = 1 + while;
//!     |             ^^^^^
//! ```
//!
//! Es **solo presentación**: no toca el lexer, el parser, el checker ni el intérprete.
//! Cada fase sigue reportando `(línea, columna, extensión)`; aquí se dibuja el contexto.
//! La columna es 1-basada y se asume que la sangría es con espacios (un tab desalinearía
//! el subrayado).

/// El único pánico permitido en el front-end (M33b): una **invariante interna rota** (un
/// ICE, *internal compiler error*). Panica con el prefijo `ICE:` — el hook estándar de Rust
/// añade archivo:línea del sitio, útil para el reporte — y la red central de
/// `with_big_stack` lo convierte en el banner de [`ice_banner`] + salida con código 101.
/// Un error del usuario NUNCA pasa por aquí: eso es un `LexError`/`ParseError`/`TypeError`.
#[macro_export]
macro_rules! ice {
    ($($arg:tt)*) => {
        panic!("ICE: {}", format_args!($($arg)*)) // ice-ok: la única definición permitida
    };
}

/// El banner que ve el usuario cuando el compilador panica (M33b): distingue el ICE de un
/// error de su programa y pide el reporte. `detalle` es el payload del pánico (el mensaje de
/// `ice!` o el de un pánico no auditado, p. ej. un índice fuera de rango).
pub fn ice_banner(detalle: &str) -> String {
    format!(
        "error interno del compilador (ICE): {detalle}\n\
         Esto es un bug de raylang, no de tu programa. Por favor repórtalo\n\
         junto con el fuente que lo causó."
    )
}

/// Renderiza un diagnóstico: la cabecera del error (su `Display`, que ya incluye la
/// ubicación y el mensaje) seguida de la línea de fuente y `^` repetido `len` veces
/// bajo la columna (M33a). El subrayado se **acota** al final de la línea: una
/// extensión corrupta o exagerada nunca desborda el render.
///
/// `headline` es el `to_string()` del error. Si `(line, col)` no cae en la fuente
/// (p. ej. un error sin ubicación útil), se devuelve solo la cabecera.
pub fn render(source: &str, line: usize, col: usize, len: usize, headline: &str) -> String {
    let mut out = String::from(headline);
    if line == 0 {
        return out;
    }
    let Some(src_line) = source.lines().nth(line - 1) else {
        return out;
    };
    let num = line.to_string();
    let gutter = " ".repeat(num.len());
    // La línea de fuente, con un canalón "  N | ".
    out.push('\n');
    out.push_str(&format!("  {} | {}", num, src_line));
    // La línea del subrayado, alineada bajo la columna del error. La extensión se
    // acota a lo que queda de línea (y como mínimo un `^`, aunque apunte al final).
    let caret_pad = " ".repeat(col.saturating_sub(1));
    let resto = src_line.chars().count().saturating_sub(col.saturating_sub(1));
    let width = len.max(1).min(resto.max(1));
    out.push('\n');
    out.push_str(&format!("  {} | {}{}", gutter, caret_pad, "^".repeat(width)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dibuja_la_linea_y_el_cursor() {
        let src = "fn main() -> int {\n    let x = 1 + true;\n    x\n}\n";
        let out = render(src, 2, 13, 1, "error de tipos en 2:13: no se pueden sumar int y bool");
        let esperado = "\
error de tipos en 2:13: no se pueden sumar int y bool
  2 |     let x = 1 + true;
    |             ^";
        assert_eq!(out, esperado);
    }

    #[test]
    fn subraya_el_rango_completo() {
        // M33a: un error con extensión subraya el lexema entero.
        let src = "let x = 1 + while;\n";
        let out = render(src, 1, 13, 5, "error de sintaxis en 1:13: se esperaba una expresión, se encontró While");
        let esperado = "\
error de sintaxis en 1:13: se esperaba una expresión, se encontró While
  1 | let x = 1 + while;
    |             ^^^^^";
        assert_eq!(out, esperado);
    }

    #[test]
    fn el_subrayado_se_acota_a_la_linea() {
        // Una extensión que se pasa del final de la línea se recorta (nunca desborda).
        let src = "let x\n";
        let out = render(src, 1, 5, 99, "err");
        assert_eq!(out, "err\n  1 | let x\n    |     ^");
        // Y un col más allá del final aún dibuja un `^` (mínimo uno).
        let out = render(src, 1, 6, 3, "err");
        assert_eq!(out, "err\n  1 | let x\n    |      ^");
    }

    #[test]
    fn el_cursor_apunta_a_la_columna_1() {
        let src = "let x = 5\n";
        let out = render(src, 1, 1, 1, "err");
        assert_eq!(out, "err\n  1 | let x = 5\n    | ^");
    }

    #[test]
    fn ice_panica_con_el_prefijo() {
        // M33b: el payload de `ice!` lleva el prefijo distinguible.
        let r = std::panic::catch_unwind(|| crate::ice!("mapa sin la clave '{}'", "x"));
        let payload = r.unwrap_err();
        let msg = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        assert_eq!(msg, "ICE: mapa sin la clave 'x'");
    }

    #[test]
    fn el_banner_pide_reporte() {
        let b = ice_banner("ICE: algo se rompió");
        assert!(b.starts_with("error interno del compilador (ICE): ICE: algo se rompió"));
        assert!(b.contains("bug de raylang"), "{b}");
        assert!(b.contains("repórtalo"), "{b}");
    }

    #[test]
    fn sin_ubicacion_util_solo_la_cabecera() {
        // Línea fuera de rango: se devuelve solo la cabecera, sin reventar.
        assert_eq!(render("una línea\n", 99, 1, 1, "err"), "err");
        assert_eq!(render("x", 0, 0, 1, "err"), "err");
    }
}
