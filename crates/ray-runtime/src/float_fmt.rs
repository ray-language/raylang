//! Formateo de un `float` con modo y precisión (raysql #299): `"f"` (posicional con `precision`
//! decimales), `"e"` (mantisa con `precision` decimales y exponente con signo y al menos dos dígitos:
//! `1.5e+20`), `"g"` (el más corto de los dos según las reglas de C: exponente si `x < -4` o
//! `x >= precision`, sin ceros finales) y `"shortest"` (los dígitos más cortos que se releen igual,
//! como `to_string`, pero en exponente cuando `x < -4` o `x >= 15`, lo que imprime PostgreSQL para un
//! float8). `precision` se ignora en `shortest`; negativa cuenta como 0. Un modo desconocido es `Err`.
//!
//! UNA SOLA FUENTE para los tres motores: la VM y el intérprete la enlazan desde `ray-runtime`
//! (wasm32 la incluye por `#[path]` desde `src/builtins.rs`), y el transpilador la pega tal cual en
//! el runtime del programa nativo (`include_str!`), así que no usa nada de `crate::`.

/// Formatea `f` según `mode` (`e` | `f` | `g` | `shortest`) y `precision`.
pub fn format_float(f: f64, mode: &str, precision: i64) -> Result<String, String> {
    fn exp_of(e: &str) -> i64 {
        e.rsplit_once('e').map(|(_, x)| x.parse().unwrap_or(0)).unwrap_or(0)
    }
    fn exp_style(e: &str) -> String {
        let (m, x) = e.rsplit_once('e').unwrap_or((e, "0"));
        let x: i64 = x.parse().unwrap_or(0);
        format!("{m}e{}{:02}", if x < 0 { '-' } else { '+' }, x.abs())
    }
    fn strip_zeros(s: &str) -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
    }
    fn positional(e: &str) -> String {
        let (m, x) = e.rsplit_once('e').unwrap_or((e, "0"));
        let x: i64 = x.parse().unwrap_or(0);
        let neg = m.starts_with('-');
        let digits: String = m.trim_start_matches('-').chars().filter(|c| *c != '.').collect();
        let point = 1 + x;
        let s = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point as usize >= digits.len() {
            format!("{}{}", digits, "0".repeat(point as usize - digits.len()))
        } else {
            format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
        };
        if neg { format!("-{s}") } else { s }
    }
    if !f.is_finite() {
        return Ok(if f.is_nan() { "NaN" } else if f > 0.0 { "inf" } else { "-inf" }.to_string());
    }
    let p = precision.max(0) as usize;
    match mode {
        "f" => Ok(format!("{:.*}", p, f)),
        "e" => Ok(exp_style(&format!("{:.*e}", p, f))),
        "g" => {
            let p = p.max(1);
            let e = format!("{:.*e}", p - 1, f);
            let x = exp_of(&e);
            if x < -4 || x >= p as i64 {
                let (m, _) = e.rsplit_once('e').unwrap_or((&e, "0"));
                Ok(exp_style(&format!("{}e{}", strip_zeros(m), x)))
            } else {
                Ok(strip_zeros(&format!("{:.*}", (p as i64 - 1 - x).max(0) as usize, f)))
            }
        }
        "shortest" => {
            let e = format!("{:e}", f);
            let x = exp_of(&e);
            if x < -4 || x >= 15 { Ok(exp_style(&e)) } else { Ok(positional(&e)) }
        }
        other => Err(format!("format_float: unknown mode '{other}' (e | f | g | shortest)")),
    }
}

#[cfg(test)]
mod tests {
    use super::format_float;

    #[test]
    fn modes_match_printf_and_postgres() {
        let f = |v: f64, m: &str, p: i64| format_float(v, m, p).unwrap();
        assert_eq!(f(1e100, "shortest", 0), "1e+100");
        assert_eq!(f(1e-5, "shortest", 0), "1e-05");
        assert_eq!(f(1e15, "shortest", 0), "1e+15");
        assert_eq!(f(123.456, "shortest", 0), "123.456");
        assert_eq!(f(100.0, "shortest", 0), "100");
        assert_eq!(f(0.001, "shortest", 0), "0.001");
        assert_eq!(f(-0.0, "shortest", 0), "-0");
        assert_eq!(f(3.0, "shortest", 0), "3");
        assert_eq!(f(123.456, "e", 3), "1.235e+02");
        assert_eq!(f(-2.5, "f", 2), "-2.50");
        assert_eq!(f(100.0, "g", 6), "100");
        assert_eq!(f(1.5e20, "g", 6), "1.5e+20");
        assert_eq!(f(f64::INFINITY, "f", 1), "inf");
        assert_eq!(f(f64::NAN, "e", 1), "NaN");
        assert!(format_float(1.0, "x", 1).unwrap_err().contains("unknown mode 'x'"));
    }
}
