//! Un JSON mínimo —parser y serializador— en `std` puro, sin dependencias.
//!
//! No es un JSON de producción: es justo lo que el LSP intercambia (leer los campos de
//! los mensajes entrantes y construir los salientes), pero correcto para ese tráfico,
//! incluido el *unescape* de cadenas con `\uXXXX` y parejas sustitutas UTF-16.

/// Un valor JSON. Los objetos preservan el orden de inserción (`Vec` de pares).
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// Busca una clave en un objeto (o `None` si no es objeto o no está).
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    /// El contenido si es una cadena.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    /// Los elementos si es un arreglo.
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// Serializa a texto JSON compacto.
    pub fn serialize(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Num(n) => {
                // Los enteros se escriben sin parte decimal (id, line, severity…).
                if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
                    out.push_str(&(*n as i64).to_string());
                } else {
                    out.push_str(&n.to_string());
                }
            }
            Json::Str(s) => write_string(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    it.write(out);
                }
                out.push(']');
            }
            Json::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// Escribe una cadena JSON, escapando comillas, barras y caracteres de control.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parsea un texto JSON a un `Json`.
pub fn parse(input: &str) -> Result<Json, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut p = Parser { chars: &chars, i: 0 };
    p.skip_ws();
    let v = p.value()?;
    p.skip_ws();
    Ok(v)
}

/// Un descenso recursivo sobre el texto (vector de `char` + cursor).
struct Parser<'a> {
    chars: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => Ok(Json::Str(self.string()?)),
            Some('t' | 'f') => self.boolean(),
            Some('n') => self.null(),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            other => Err(format!("token JSON inesperado: {other:?}")),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.bump(); // '{'
        let mut pairs = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(Json::Obj(pairs));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some('"') {
                return Err("expected an object key".into());
            }
            let key = self.string()?;
            self.skip_ws();
            if self.bump() != Some(':') {
                return Err("expected ':' in an object".into());
            }
            let val = self.value()?;
            pairs.push((key, val));
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some('}') => break,
                other => return Err(format!("expected ',' or '}}', got {other:?}")),
            }
        }
        Ok(Json::Obj(pairs))
    }

    fn array(&mut self) -> Result<Json, String> {
        self.bump(); // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some(']') => break,
                other => return Err(format!("expected ',' or ']', got {other:?}")),
            }
        }
        Ok(Json::Arr(items))
    }

    fn string(&mut self) -> Result<String, String> {
        self.bump(); // '"'
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return Err("unterminated string".into()),
                Some('"') => break,
                Some('\\') => self.escape(&mut s)?,
                Some(c) => s.push(c),
            }
        }
        Ok(s)
    }

    /// Procesa un escape tras `\` y empuja el carácter resultante.
    fn escape(&mut self, s: &mut String) -> Result<(), String> {
        match self.bump() {
            Some('"') => s.push('"'),
            Some('\\') => s.push('\\'),
            Some('/') => s.push('/'),
            Some('n') => s.push('\n'),
            Some('t') => s.push('\t'),
            Some('r') => s.push('\r'),
            Some('b') => s.push('\u{0008}'),
            Some('f') => s.push('\u{000C}'),
            Some('u') => {
                let cp = self.hex4()?;
                // Una pareja sustituta UTF-16 (alta + baja) codifica un carácter > BMP.
                if (0xD800..=0xDBFF).contains(&cp) {
                    if self.bump() != Some('\\') || self.bump() != Some('u') {
                        return Err("pareja sustituta UTF-16 incompleta".into());
                    }
                    let lo = self.hex4()?;
                    let c = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    if let Some(ch) = char::from_u32(c) {
                        s.push(ch);
                    }
                } else if let Some(ch) = char::from_u32(cp) {
                    s.push(ch);
                }
            }
            other => return Err(format!("invalid escape: \\{other:?}")),
        }
        Ok(())
    }

    /// Cuatro dígitos hexadecimales (`\uXXXX`).
    fn hex4(&mut self) -> Result<u32, String> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self.bump().ok_or("escape \\u incompleto")?;
            let d = c.to_digit(16).ok_or("invalid hex digit")?;
            v = v * 16 + d;
        }
        Ok(v)
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.peek() == Some('-') {
            self.bump();
        }
        while matches!(
            self.peek(),
            Some(c) if c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')
        ) {
            self.bump();
        }
        let s: String = self.chars[start..self.i].iter().collect();
        s.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("invalid number: {s}"))
    }

    fn boolean(&mut self) -> Result<Json, String> {
        if self.lit("true") {
            Ok(Json::Bool(true))
        } else if self.lit("false") {
            Ok(Json::Bool(false))
        } else {
            Err("invalid boolean literal".into())
        }
    }

    fn null(&mut self) -> Result<Json, String> {
        if self.lit("null") {
            Ok(Json::Null)
        } else {
            Err("invalid null literal".into())
        }
    }

    /// Consume `word` si aparece literalmente en el cursor.
    fn lit(&mut self, word: &str) -> bool {
        let end = self.i + word.len();
        if end <= self.chars.len() && self.chars[self.i..end].iter().collect::<String>() == word
        {
            self.i = end;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_object() {
        let v = parse(r#"{"a":1,"b":[true,null,"x"],"c":{"d":-2.5}}"#).unwrap();
        assert_eq!(v.get("a"), Some(&Json::Num(1.0)));
        assert_eq!(v.get("b").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(v.get("c").unwrap().get("d"), Some(&Json::Num(-2.5)));
    }

    #[test]
    fn unescapes_strings() {
        let v = parse(r#""línea\n\t\"fin\"""#).unwrap();
        assert_eq!(v.as_str(), Some("línea\n\t\"fin\""));
        // \uXXXX (BMP) y pareja sustituta (emoji).
        assert_eq!(parse(r#""é""#).unwrap().as_str(), Some("é"));
        assert_eq!(parse(r#""😀""#).unwrap().as_str(), Some("😀"));
    }

    #[test]
    fn serialize_and_reparse_equal() {
        let original = obj_from(vec![
            ("jsonrpc", Json::Str("2.0".into())),
            ("id", Json::Num(7.0)),
            ("ok", Json::Bool(true)),
            ("list", Json::Arr(vec![Json::Num(1.0), Json::Null])),
        ]);
        let serialized = original.serialize();
        assert_eq!(parse(&serialized).unwrap(), original);
        // Los enteros no llevan parte decimal.
        assert!(serialized.contains("\"id\":7"));
    }

    fn obj_from(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
}
