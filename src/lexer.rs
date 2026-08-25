//! Lexer (analizador léxico) de raylang.
//!
//! Convierte el texto fuente en un `Vec<Token>`. Es la primera fase del pipeline
//! (DESIGN.md §2). No entiende de gramática ni de tipos: solo reconoce las
//! unidades léxicas de la §3 del diseño y les asigna posición.
//!
//! Estrategia: un cursor sobre los caracteres del fuente. En cada vuelta del
//! bucle principal saltamos espacios/comentarios, anotamos la posición de inicio,
//! y emitimos exactamente un token consumiendo los caracteres que le
//! correspondan. Para operadores de dos caracteres (`==`, `->`, …) usamos un
//! lookahead de un carácter.

use crate::token::{InterpPart, Radix, Token, TokenKind};

/// Error léxico con ubicación. Se produce ante un carácter inesperado, una cadena
/// sin cerrar, un escape inválido o un número mal formado.
///
/// `len` (M33a) es la extensión del error en caracteres: lo consumido del token en
/// curso cuando se detectó (una "cadena sin cerrar" subraya desde la comilla). No
/// entra en el `Display` (la cabecera no cambia); la usa el renderizador de
/// diagnósticos para dibujar el rango.
#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lex error at {}:{}: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for LexError {}

/// Función de conveniencia: tokeniza un fuente completo de una sola llamada.
pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(src).tokenize()
}

/// Como [`lex`], pero arranca el cursor en `(line, col)` en vez de `(1, 1)`: los tokens (y sus
/// posiciones) salen como si el fragmento viviera ahí. Lo usa el parser para re-lexar el cuerpo de
/// una interpolación `${…}` con posiciones reales (M27.3 + hover del LSP).
pub fn lex_at(src: &str, line: usize, col: usize) -> Result<Vec<Token>, LexError> {
    let mut lx = Lexer::new(src);
    lx.line = line;
    lx.col = col;
    lx.start_line = line;
    lx.start_col = col;
    lx.tokenize()
}

pub struct Lexer {
    /// El fuente como vector de caracteres Unicode. Trabajar con `char` (y no con
    /// bytes) evita partir un carácter multibyte por la mitad.
    chars: Vec<char>,
    /// Índice del próximo carácter a consumir.
    pos: usize,
    /// Posición "viva" del cursor (1-basada).
    line: usize,
    col: usize,
    /// Posición donde empezó el token que estamos construyendo. La congelamos al
    /// inicio de cada token para que tanto el token como sus posibles errores
    /// apunten al comienzo, no a media palabra.
    start_line: usize,
    start_col: usize,
}

impl Lexer {
    pub fn new(src: &str) -> Self {
        Lexer {
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            start_line: 1,
            start_col: 1,
        }
    }

    /// Bucle principal: produce tokens hasta agotar la entrada, cerrando siempre
    /// con un `Eof`.
    pub fn tokenize(&mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            // Congelamos la posición de inicio del próximo token.
            self.start_line = self.line;
            self.start_col = self.col;

            match self.peek() {
                None => {
                    tokens.push(Token::new(TokenKind::Eof, self.start_line, self.start_col, 1));
                    return Ok(tokens);
                }
                Some(_) => {
                    let kind = self.next_token()?;
                    // La longitud del lexema (M33a): restar columnas al terminar. Un backtick
                    // multilínea (M95) cruza líneas → degrada a 1 (solo afecta al subrayado LSP).
                    let len = if self.line > self.start_line { 1 } else { (self.col - self.start_col).max(1) };
                    tokens.push(Token::new(kind, self.start_line, self.start_col, len));
                }
            }
        }
    }

    // ----- Reconocimiento de un token -----

    /// Reconoce y consume el siguiente token. Se asume que `peek()` no es `None`.
    fn next_token(&mut self) -> Result<TokenKind, LexError> {
        let c = self.advance();
        let kind = match c {
            '+' => TokenKind::Plus,
            '-' => {
                // '-' puede ser resta o el inicio de la flecha '->'.
                if self.match_char('>') {
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash, // los comentarios '//' ya se filtraron antes
            '%' => TokenKind::Percent,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ';' => TokenKind::Semicolon,
            ':' => TokenKind::Colon,
            '.' => {
                // `..` es el operador de rango (M27.2); `.` solo es acceso a campo.
                if self.peek() == Some('.') {
                    self.advance();
                    TokenKind::DotDot
                } else {
                    TokenKind::Dot
                }
            }
            '?' => TokenKind::Question,
            '@' => TokenKind::At, // reservado para anotaciones (M10); el parser aún no lo usa

            // Operadores que pueden tener una segunda parte.
            '=' => {
                if self.match_char('=') {
                    TokenKind::EqEq
                } else if self.match_char('>') {
                    TokenKind::FatArrow // => (brazos de match, M5)
                } else {
                    TokenKind::Eq
                }
            }
            '!' => {
                if self.match_char('=') {
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }
            '<' => {
                if self.match_char('=') {
                    TokenKind::LtEq
                } else if self.match_char('<') {
                    TokenKind::Shl // << (desplazamiento, M19.3a)
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                // Ojo: '>>' es ambiguo con genéricos anidados (`Caja<Caja<int>>`). El lexer
                // siempre emite `Shr`; al cerrar argumentos de tipo, el parser **parte** un
                // `Shr` en dos `>` (igual que Rust/Java). Ver `Parser::close_type_angle`.
                if self.match_char('=') {
                    TokenKind::GtEq
                } else if self.match_char('>') {
                    TokenKind::Shr // >> (desplazamiento, M19.3a)
                } else {
                    TokenKind::Gt
                }
            }
            // '&' y '|' duplicados son lógicos (&& ||); sueltos son bit a bit (M19.3a).
            '&' => {
                if self.match_char('&') {
                    TokenKind::AmpAmp
                } else {
                    TokenKind::Amp // & (AND bit a bit)
                }
            }
            '|' => {
                if self.match_char('|') {
                    TokenKind::PipePipe
                } else if self.match_char('>') {
                    TokenKind::PipeArrow // |> (pipeline, M7.2)
                } else {
                    TokenKind::Pipe // | (OR bit a bit)
                }
            }
            '^' => TokenKind::Caret, // ^ (XOR bit a bit, M19.3a)
            '~' => TokenKind::Tilde, // ~ (NOT bit a bit, M19.3a)

            '"' => self.string('"')?,
            // M95: backticks — un string donde `"` es literal y los saltos de línea están
            // permitidos (multilínea); la interpolación `${}` funciona igual (M27.3).
            '`' => self.string('`')?,
            '\'' => self.char_literal()?,
            // M16.1a: literal de bytes `b"..."`. Se distingue de un identificador que empiece por 'b'
            // mirando si la comilla sigue inmediatamente. Va ANTES del caso de identificador.
            'b' if self.peek() == Some('"') => {
                self.advance(); // consume la comilla de apertura
                self.byte_string()?
            }
            c if c.is_ascii_digit() => self.number()?,
            c if is_ident_start(c) => self.identifier(),

            other => {
                return Err(self.error(format!("unexpected character '{}'", other)));
            }
        };
        Ok(kind)
    }

    /// Lee el resto de un número una vez consumido su primer dígito. Decide entre
    /// entero y flotante según haya un '.' seguido de dígito.
    fn number(&mut self) -> Result<TokenKind, LexError> {
        let start = self.pos - 1; // el primer dígito ya fue consumido por next_token
        // M118: literal con prefijo de base — `0x`/`0X` hex, `0o`/`0O` octal, `0b`/`0B` binario. El
        // '0' ya se consumió; si le sigue el prefijo, se lee en esa base (solo enteros, sin `.`).
        if self.chars[start] == '0' {
            if let Some((radix, base)) = self.peek().and_then(|p| match p {
                'x' | 'X' => Some((16, Radix::Hex)),
                'o' | 'O' => Some((8, Radix::Oct)),
                'b' | 'B' => Some((2, Radix::Bin)),
                _ => None,
            }) {
                let prefix = self.advance(); // consume x/o/b
                let dstart = self.pos;
                while matches!(self.peek(), Some(c) if c.is_digit(radix)) {
                    self.advance();
                }
                if self.pos == dstart {
                    return Err(self.error(format!("expected at least one digit after '0{}'", prefix)));
                }
                let digits: String = self.chars[dstart..self.pos].iter().collect();
                let v = i64::from_str_radix(&digits, radix).map_err(|_| {
                    let text: String = self.chars[start..self.pos].iter().collect();
                    self.error(format!("integer out of range '{}'", text))
                })?;
                return Ok(TokenKind::Int(v, base));
            }
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.advance();
        }

        // Es flotante solo si hay '.' Y un dígito tras el punto. Así "1." o
        // "1.metodo()" (futuro UFCS) no se confunden con un flotante.
        let has_dot =
            self.peek() == Some('.') && matches!(self.peek_next(), Some(c) if c.is_ascii_digit());
        if has_dot {
            self.advance(); // consume el '.'
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.advance();
            }
        }

        // M80: exponente opcional `e|E [+|-] dígitos` (`1e21`, `1.5e-3`, `2E+10`); hace el
        // literal flotante aunque no lleve punto. Guarda conservadora (espeja la del '.'):
        // el e/E solo se consume si le sigue un dígito, o un signo Y un dígito → `1eabc`
        // sigue siendo `1` + identificador, y `1e+x` sigue siendo `1` + `e` + `+` + `x`.
        let has_exp = matches!(self.peek(), Some('e') | Some('E'))
            && match self.peek_next() {
                Some(c) if c.is_ascii_digit() => true,
                Some('+') | Some('-') => {
                    matches!(self.chars.get(self.pos + 2), Some(c) if c.is_ascii_digit())
                }
                _ => false,
            };
        if has_exp {
            self.advance(); // el e/E
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.advance();
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.advance();
            }
        }
        let is_float = has_dot || has_exp;

        let text: String = self.chars[start..self.pos].iter().collect();
        if is_float {
            let v = text
                .parse::<f64>()
                .map_err(|_| self.error(format!("invalid float '{}'", text)))?;
            Ok(TokenKind::Float(v))
        } else {
            let v = text
                .parse::<i64>()
                .map_err(|_| self.error(format!("integer out of range '{}'", text)))?;
            Ok(TokenKind::Int(v, Radix::Dec))
        }
    }

    /// Lee un identificador (ya consumido su primer carácter) y lo clasifica: si
    /// coincide con una palabra clave, devuelve esa; si no, es un `Ident`.
    fn identifier(&mut self) -> TokenKind {
        let start = self.pos - 1;
        while matches!(self.peek(), Some(c) if is_ident_continue(c)) {
            self.advance();
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        keyword(&text).unwrap_or(TokenKind::Ident(text))
    }

    /// Lee una cadena (ya consumido el delimitador de apertura: `"` ordinaria, `` ` `` template
    /// M95). Toda cadena puede **interpolar** (M27.3, rediseñado): `${expr}` inserta el valor de
    /// `expr` (balanceando llaves anidadas). El `$` solo es especial seguido de `{`; en cualquier
    /// otro sitio es literal (`"$5"`, `"$PATH"` no necesitan escape). Las llaves `{` `}` son
    /// **siempre literales**. Escapes: `\n \t \r \\ \" \\`` y **`\$`** (para un `${` literal).
    /// Diferencias del backtick: la `"` es LITERAL (adiós `\"`) y los saltos de línea están
    /// permitidos (multilínea). Sin ningún `${…}`, la cadena degrada a un `Str` normal → toda
    /// cadena preexistente lexea byte-idéntico; ambos delimitadores producen el MISMO token.
    fn string(&mut self, delim: char) -> Result<TokenKind, LexError> {
        let mut parts: Vec<InterpPart> = Vec::new();
        let mut cur = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated string".into())),
                Some('\n') if delim == '"' => {
                    return Err(self.error("newline inside an unterminated string".into()))
                }
                Some(c) if c == delim => {
                    self.advance(); // el delimitador de cierre
                    break;
                }
                Some('\\') => {
                    self.advance(); // la barra invertida
                    // Cada brazo consume EXACTAMENTE sus caracteres (los simples, 1; `\x`, 3; `\u{…}`,
                    // variable) — no hay un `advance` común al final (M118 añadió escapes de longitud
                    // variable, incompatibles con él).
                    match self.peek() {
                        Some('n') => { cur.push('\n'); self.advance(); }
                        Some('t') => { cur.push('\t'); self.advance(); }
                        Some('r') => { cur.push('\r'); self.advance(); }
                        Some('0') => { cur.push('\0'); self.advance(); } // M118: NUL
                        Some('\\') => { cur.push('\\'); self.advance(); }
                        Some('"') => { cur.push('"'); self.advance(); }
                        Some('`') => { cur.push('`'); self.advance(); }
                        Some('$') => { cur.push('$'); self.advance(); } // `\$` → un `$` literal (para un `${` sin interpolar)
                        Some('x') => { self.advance(); cur.push(self.hex_escape_char()?); } // M118: \xNN → U+00NN
                        Some('u') => { self.advance(); cur.push(self.unicode_escape_char()?); } // M118: \u{H…H}
                        Some(other) => {
                            return Err(self.error(format!("invalid escape sequence '\\{}'", other)))
                        }
                        None => return Err(self.error("unterminated string after '\\'".into())),
                    }
                }
                // `${expr}`: inicio de una interpolación. Un `$` sin `{` detrás es un carácter normal.
                Some('$') if self.peek_next() == Some('{') => {
                    self.advance(); // el `$`
                    self.advance(); // el `{`
                    if !cur.is_empty() {
                        parts.push(InterpPart::Lit(std::mem::take(&mut cur)));
                    }
                    // Posición del primer carácter de la expresión (para re-lexarla con posiciones reales).
                    let (el, ec) = (self.line, self.col);
                    let mut expr_src = String::new();
                    let mut depth = 1;
                    loop {
                        match self.peek() {
                            None => return Err(self.error("unterminated '${' interpolation in string".into())),
                            Some('\n') => return Err(self.error("newline inside an interpolation".into())),
                            Some('{') => { depth += 1; expr_src.push('{'); self.advance(); }
                            Some('}') => {
                                depth -= 1;
                                self.advance();
                                if depth == 0 { break; }
                                expr_src.push('}');
                            }
                            Some(c) => { expr_src.push(c); self.advance(); }
                        }
                    }
                    if expr_src.trim().is_empty() {
                        return Err(self.error("empty interpolation '${}' in string".into()));
                    }
                    parts.push(InterpPart::Expr(expr_src, el, ec));
                }
                Some(c) => {
                    cur.push(c);
                    self.advance();
                }
            }
        }
        // Cierra el literal final (o el string vacío si no hubo partes).
        if !cur.is_empty() || parts.is_empty() {
            parts.push(InterpPart::Lit(cur));
        }
        // Sin interpolaciones → un `Str` normal (compat hacia atrás).
        if parts.len() == 1 {
            if let InterpPart::Lit(s) = &parts[0] {
                return Ok(TokenKind::Str(s.clone()));
            }
        }
        Ok(TokenKind::InterpStr(parts))
    }

    /// Lee un literal de bytes `b"..."` (M16.1a), tras consumir la comilla de apertura. Acepta los
    /// escapes del string (`\n \t \r \\ \"`) más **`\xNN`** (dos dígitos hex → un octeto arbitrario,
    /// la clave para escribir binario literal). Los caracteres normales se codifican como UTF-8.
    fn byte_string(&mut self) -> Result<TokenKind, LexError> {
        let mut bytes: Vec<u8> = Vec::new();
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated byte string".into())),
                Some('\n') => {
                    return Err(self.error("newline inside an unterminated byte string".into()))
                }
                Some('"') => {
                    self.advance(); // comilla de cierre
                    return Ok(TokenKind::Bytes(bytes));
                }
                Some('\\') => {
                    self.advance(); // la barra invertida
                    match self.peek() {
                        Some('n') => { bytes.push(b'\n'); self.advance(); }
                        Some('t') => { bytes.push(b'\t'); self.advance(); }
                        Some('r') => { bytes.push(b'\r'); self.advance(); }
                        Some('\\') => { bytes.push(b'\\'); self.advance(); }
                        Some('"') => { bytes.push(b'"'); self.advance(); }
                        Some('x') => {
                            self.advance(); // la x
                            let hi = self.hex_digit()?;
                            let lo = self.hex_digit()?;
                            bytes.push((hi << 4) | lo);
                        }
                        Some(other) => {
                            return Err(self.error(format!("invalid escape sequence '\\{}'", other)))
                        }
                        None => return Err(self.error("unterminated byte string after '\\'".into())),
                    }
                }
                Some(c) => {
                    let mut buf = [0u8; 4];
                    bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    self.advance();
                }
            }
        }
    }

    /// M118: `\xNN` en un string/char — dos dígitos hex (la `x` ya consumida) → el carácter del code
    /// point `U+00NN` (0–255). En un `b"…"` el `\x` produce un OCTETO (ver `byte_string`); aquí, sobre
    /// texto Unicode, produce el carácter (Latin-1 / control).
    fn hex_escape_char(&mut self) -> Result<char, LexError> {
        let hi = self.hex_digit()? as u32;
        let lo = self.hex_digit()? as u32;
        Ok(char::from_u32(hi * 16 + lo).unwrap_or_else(|| crate::ice!("0..=255 is always a valid code point")))
    }

    /// M118: `\u{H…H}` — de 1 a 6 dígitos hex entre llaves (la `u` ya consumida) → un code point
    /// Unicode. Error si excede `U+10FFFF` o cae en el rango surrogate (no es un carácter válido).
    fn unicode_escape_char(&mut self) -> Result<char, LexError> {
        if self.peek() != Some('{') {
            return Err(self.error("expected '{' after '\\u' (write it as \\u{1F600})".into()));
        }
        self.advance(); // '{'
        let dstart = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
            self.advance();
        }
        let ndigits = self.pos - dstart;
        if ndigits == 0 || ndigits > 6 {
            return Err(self.error("'\\u{...}' takes 1 to 6 hex digits".into()));
        }
        if self.peek() != Some('}') {
            return Err(self.error("expected '}' to close '\\u{...}'".into()));
        }
        let digits: String = self.chars[dstart..self.pos].iter().collect();
        self.advance(); // '}'
        // 6 dígitos hex caben en u32 (max 0xFFFFFF); char::from_u32 rechaza > 0x10FFFF y surrogates.
        let cp = u32::from_str_radix(&digits, 16)
            .unwrap_or_else(|_| crate::ice!("1..=6 hex digits parse as u32"));
        char::from_u32(cp)
            .ok_or_else(|| self.error(format!("'\\u{{{}}}' is not a valid Unicode code point", digits)))
    }

    /// Lee un dígito hexadecimal y devuelve su valor (0–15). Para el escape `\xNN` (M16.1a).
    fn hex_digit(&mut self) -> Result<u8, LexError> {
        match self.peek() {
            Some(c) if c.is_ascii_hexdigit() => {
                self.advance();
                Ok(c.to_digit(16).unwrap_or_else(|| crate::ice!("'{c}' passed the hexdigit guard")) as u8)
            }
            Some(other) => Err(self.error(format!("expected a hex digit after '\\x', not '{}'", other))),
            None => Err(self.error("unterminated byte string (missing hex digits after '\\x')".into())),
        }
    }

    /// Lee un literal de carácter `'a'` (M11.4c), tras consumir la comilla de apertura. Acepta los
    /// mismos escapes que el string (`\n \t \\`) más `\'`. Exactamente un carácter entre comillas:
    /// `''` (vacío) o varios caracteres son error.
    fn char_literal(&mut self) -> Result<TokenKind, LexError> {
        let c = match self.peek() {
            None => return Err(self.error("unterminated char literal".into())),
            Some('\'') => return Err(self.error("a char literal cannot be empty ('')".into())),
            Some('\n') => return Err(self.error("newline inside a char literal".into())),
            Some('\\') => {
                self.advance(); // la barra invertida
                // `\x`/`\u{…}` leen su propia longitud (y ya dejan el cursor tras el escape); los
                // simples consumen su único carácter con el `advance` de después.
                match self.peek() {
                    Some('x') => { self.advance(); self.hex_escape_char()? }
                    Some('u') => { self.advance(); self.unicode_escape_char()? }
                    other => {
                        let escaped = match other {
                            Some('n') => '\n',
                            Some('t') => '\t',
                            Some('r') => '\r', // M14: retorno de carro
                            Some('0') => '\0', // M118: NUL
                            Some('\\') => '\\',
                            Some('\'') => '\'',
                            Some(o) => return Err(self.error(format!("invalid escape sequence '\\{}'", o))),
                            None => return Err(self.error("unterminated char literal after '\\'".into())),
                        };
                        self.advance(); // el carácter escapado
                        escaped
                    }
                }
            }
            Some(c) => {
                self.advance();
                c
            }
        };
        match self.peek() {
            Some('\'') => {
                self.advance(); // comilla de cierre
                Ok(TokenKind::Char(c))
            }
            _ => Err(self.error("expected ''' to close the char literal (more than one character?)".into())),
        }
    }

    // ----- Saltar lo que no produce tokens -----

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.peek() {
                Some(' ') | Some('\t') | Some('\r') | Some('\n') => {
                    self.advance();
                }
                // Comentario de línea: '//' hasta el fin de línea (sin consumir el '\n').
                Some('/') if self.peek_next() == Some('/') => {
                    while !matches!(self.peek(), Some('\n') | None) {
                        self.advance();
                    }
                }
                _ => return,
            }
        }
    }

    // ----- Primitivas del cursor -----

    /// Devuelve el carácter actual sin consumirlo.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Devuelve el carácter siguiente al actual sin consumirlo (lookahead de 1).
    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    /// Consume y devuelve el carácter actual, actualizando línea/columna.
    fn advance(&mut self) -> char {
        let c = self.chars[self.pos];
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        c
    }

    /// Si el carácter actual es `expected`, lo consume y devuelve `true`.
    fn match_char(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Construye un `LexError` apuntando al inicio del token actual. La extensión
    /// (M33a) es lo consumido del token hasta detectar el error — solo si el error
    /// cae en la misma línea donde empezó el token (siempre, salvo la cadena rota
    /// por un salto de línea, que subraya solo lo recorrido en su línea).
    fn error(&self, msg: String) -> LexError {
        let len = if self.line == self.start_line { (self.col - self.start_col).max(1) } else { 1 };
        LexError {
            msg,
            line: self.start_line,
            col: self.start_col,
            len,
        }
    }
}

// ----- Funciones auxiliares libres -----

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Tabla de palabras clave. Devuelve `None` si la cadena es un identificador
/// normal. Es la única fuente de verdad sobre qué nombres están reservados.
fn keyword(s: &str) -> Option<TokenKind> {
    Some(match s {
        "let" => TokenKind::Let,
        "var" => TokenKind::Var,
        "fn" => TokenKind::Fn,
        "return" => TokenKind::Return,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "for" => TokenKind::For,
        "in" => TokenKind::In,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "struct" => TokenKind::Struct,
        "const" => TokenKind::Const,
        "enum" => TokenKind::Enum,
        "match" => TokenKind::Match,
        "trait" => TokenKind::Trait,
        "impl" => TokenKind::Impl,
        "dyn" => TokenKind::Dyn,
        "pub" => TokenKind::Pub,
        "import" => TokenKind::Import,
        "from" => TokenKind::From,
        "extern" => TokenKind::Extern,
        "as" => TokenKind::As,
        "int" => TokenKind::IntType,
        "float" => TokenKind::FloatType,
        "bool" => TokenKind::BoolType,
        "string" => TokenKind::StringType,
        "char" => TokenKind::CharType,
        "bytes" => TokenKind::BytesType,
        "ptr" => TokenKind::PtrType, // M41.4b: puntero opaco foráneo (FFI)
        "u8" => TokenKind::UIntType(8),   // M28.3: enteros sin signo con tamaño
        "u32" => TokenKind::UIntType(32),
        "u64" => TokenKind::UIntType(64),
        _ => return None,
    })
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// Tokeniza y devuelve solo las clases (sin posiciones), terminadas en Eof.
    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).expect("debería tokenizar sin error").into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn flotantes_con_exponente() {
        // M80: `e|E [+|-] dígitos` hace el literal flotante, con o sin punto.
        assert_eq!(kinds("1e21"), vec![TokenKind::Float(1e21), TokenKind::Eof]);
        assert_eq!(kinds("1.5e-3"), vec![TokenKind::Float(1.5e-3), TokenKind::Eof]);
        assert_eq!(kinds("2E+10"), vec![TokenKind::Float(2e10), TokenKind::Eof]);
        assert_eq!(kinds("7e0"), vec![TokenKind::Float(7.0), TokenKind::Eof]);
        // Guarda conservadora: sin dígito tras el e (o tras el signo), NO es exponente.
        assert_eq!(
            kinds("1eabc"),
            vec![TokenKind::Int(1, Radix::Dec), TokenKind::Ident("eabc".into()), TokenKind::Eof]
        );
        assert_eq!(
            kinds("1e+"),
            vec![TokenKind::Int(1, Radix::Dec), TokenKind::Ident("e".into()), TokenKind::Plus, TokenKind::Eof]
        );
        // Un exponente que desborda f64 no es error: satura a infinito (semántica de f64).
        assert_eq!(kinds("1e999"), vec![TokenKind::Float(f64::INFINITY), TokenKind::Eof]);
    }

    #[test]
    fn backticks_template_strings() {
        use crate::token::InterpPart::{Expr, Lit};
        // M95: el backtick delimita un string donde `"` es LITERAL — mismo token que `"…"`.
        assert_eq!(kinds("`di \"hola\"`"), vec![TokenKind::Str("di \"hola\"".into()), TokenKind::Eof]);
        // Multilínea: el salto de línea es literal (y las posiciones siguen avanzando).
        assert_eq!(kinds("`a\nb`"), vec![TokenKind::Str("a\nb".into()), TokenKind::Eof]);
        // Interpolación idéntica a la de comillas (M27.3).
        assert_eq!(
            kinds("`x=${n}`"),
            vec![TokenKind::InterpStr(vec![Lit("x=".into()), Expr("n".into(), 1, 6)]), TokenKind::Eof]
        );
        // Escapes: `\`` → backtick literal; `\$` → `${` sin interpolar.
        assert_eq!(kinds("`un \\` tick`"), vec![TokenKind::Str("un ` tick".into()), TokenKind::Eof]);
        assert_eq!(kinds("`\\${x}`"), vec![TokenKind::Str("${x}".into()), TokenKind::Eof]);
        // Sin cerrar → error (mismo mensaje que un string normal).
        assert!(matches!(Lexer::new("`abc").tokenize(), Err(e) if e.msg == "unterminated string"));
    }

    #[test]
    fn string_interpolation() {
        use crate::token::InterpPart::{Expr, Lit};
        // `${expr}` en cualquier cadena → InterpStr con partes literales y de expresión (código crudo).
        assert_eq!(
            kinds("\"a${x + 1}b\""),
            vec![
                // `"a${x + 1}b"`: la expresión `x + 1` empieza en la columna 5 (tras `"a${`).
                TokenKind::InterpStr(vec![Lit("a".into()), Expr("x + 1".into(), 1, 5), Lit("b".into())]),
                TokenKind::Eof
            ]
        );
        // Sin `${`, degrada a un `Str` normal: las llaves son literales y `$` suelto también.
        assert_eq!(kinds("\"{n} cuesta $5\""), vec![TokenKind::Str("{n} cuesta $5".into()), TokenKind::Eof]);
        // `\$` escapa un `${` literal (no interpola).
        assert_eq!(kinds("\"\\${x}\""), vec![TokenKind::Str("${x}".into()), TokenKind::Eof]);
        // Interpolación al inicio y llaves anidadas dentro de la expresión.
        assert_eq!(
            kinds("\"${ f({a}) }\""),
            vec![TokenKind::InterpStr(vec![Expr(" f({a}) ".into(), 1, 4)]), TokenKind::Eof]
        );
        // Errores: interpolación vacía y sin cerrar.
        assert!(lex("\"${}\"").is_err());
        assert!(lex("\"${x\"").is_err());
    }

    #[test]
    fn tokens_carry_their_length() {
        // M33a: cada token mide su lexema en caracteres (col..col+len es el span).
        let toks = lex("let foo = 12345 + \"ab\\n\";").expect("tokeniza");
        let lens: Vec<(TokenKind, usize)> =
            toks.into_iter().map(|t| (t.kind, t.len)).collect();
        assert_eq!(lens[0], (TokenKind::Let, 3));
        assert_eq!(lens[1], (TokenKind::Ident("foo".into()), 3));
        assert_eq!(lens[2], (TokenKind::Eq, 1));
        assert_eq!(lens[3], (TokenKind::Int(12345, Radix::Dec), 5));
        assert_eq!(lens[4], (TokenKind::Plus, 1));
        // El string mide su forma ESCRITA (comillas y escapes incluidos): "ab\n" son 6 chars.
        assert_eq!(lens[5], (TokenKind::Str("ab\n".into()), 6));
        assert_eq!(lens[6], (TokenKind::Semicolon, 1));
        assert_eq!(lens[7], (TokenKind::Eof, 1));
    }

    #[test]
    fn lex_error_carries_its_extent() {
        // M33a: "cadena sin cerrar" subraya desde la comilla hasta donde se rompió.
        let e = lex("let s = \"hello").unwrap_err();
        assert_eq!((e.line, e.col), (1, 9));
        assert_eq!(e.len, 6, "desde la comilla hasta el final: \"hello son 6 chars");
        // Un carácter inesperado mide 1.
        let e = lex("let # = 1").unwrap_err();
        assert_eq!(e.len, 1);
    }

    #[test]
    fn empty_produces_only_eof() {
        assert_eq!(kinds(""), vec![TokenKind::Eof]);
        assert_eq!(kinds("   \n\t  "), vec![TokenKind::Eof]);
    }

    #[test]
    // `3.14` prueba el lexeo de floats, no es una aproximación de PI (falso positivo de `approx_constant`).
    #[allow(clippy::approx_constant)]
    fn literals() {
        assert_eq!(kinds("42"), vec![TokenKind::Int(42, Radix::Dec), TokenKind::Eof]);
        assert_eq!(kinds("3.14"), vec![TokenKind::Float(3.14), TokenKind::Eof]);
        assert_eq!(
            kinds("\"hello\""),
            vec![TokenKind::Str("hello".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn base_prefixed_integer_literals() {
        // M118: prefijos 0x/0o/0b (mayúsculas también) para hex, octal y binario.
        assert_eq!(kinds("0xFF"), vec![TokenKind::Int(255, Radix::Hex), TokenKind::Eof]);
        assert_eq!(kinds("0x1F300"), vec![TokenKind::Int(127744, Radix::Hex), TokenKind::Eof]);
        assert_eq!(kinds("0o755"), vec![TokenKind::Int(493, Radix::Oct), TokenKind::Eof]);
        assert_eq!(kinds("0o600"), vec![TokenKind::Int(384, Radix::Oct), TokenKind::Eof]);
        assert_eq!(kinds("0b1010"), vec![TokenKind::Int(10, Radix::Bin), TokenKind::Eof]);
        assert_eq!(kinds("0X10"), vec![TokenKind::Int(16, Radix::Hex), TokenKind::Eof]);
        assert_eq!(kinds("0O17"), vec![TokenKind::Int(15, Radix::Oct), TokenKind::Eof]);
        assert_eq!(kinds("0B1"), vec![TokenKind::Int(1, Radix::Bin), TokenKind::Eof]);
        // `0` a secas y `0.5` siguen siendo lo de siempre (no confundir el prefijo).
        assert_eq!(kinds("0"), vec![TokenKind::Int(0, Radix::Dec), TokenKind::Eof]);
        assert_eq!(kinds("0.5"), vec![TokenKind::Float(0.5), TokenKind::Eof]);
        // El lexema mide su forma escrita (prefijo incluido).
        let toks = lex("0xFF").expect("tokeniza");
        assert_eq!(toks[0].len, 4);
        // Sin dígitos tras el prefijo → error.
        assert!(matches!(lex("0x"), Err(e) if e.msg.contains("expected at least one digit")));
        assert!(matches!(lex("0b"), Err(e) if e.msg.contains("expected at least one digit")));
        // Un dígito fuera del rango de la base no forma parte del literal.
        assert_eq!(
            kinds("0b12"),
            vec![TokenKind::Int(1, Radix::Bin), TokenKind::Int(2, Radix::Dec), TokenKind::Eof]
        );
    }

    #[test]
    fn hex_and_unicode_escapes_in_string() {
        // M118: \0, \xNN y \u{H…H} en cadenas.
        assert_eq!(kinds(r#""\x41""#), vec![TokenKind::Str("A".into()), TokenKind::Eof]);
        assert_eq!(kinds(r#""\u{00e9}""#), vec![TokenKind::Str("é".into()), TokenKind::Eof]);
        assert_eq!(kinds(r#""\u{1F680}""#), vec![TokenKind::Str("🚀".into()), TokenKind::Eof]);
        assert_eq!(kinds(r#""a\0b""#), vec![TokenKind::Str("a\0b".into()), TokenKind::Eof]);
        // Errores: hex no válido, code point fuera de rango, surrogate, sin llave/cierre.
        assert!(lex(r#""\xZZ""#).is_err());
        assert!(lex(r#""\u{110000}""#).is_err());
        assert!(lex(r#""\u{D800}""#).is_err());
        assert!(lex(r#""\u{}""#).is_err());
        // Falta el cierre de la secuencia \u{…}.
        assert!(lex(r#""\u{1F680""#).is_err());
    }

    #[test]
    fn hex_and_unicode_escapes_in_char() {
        // M118: los mismos escapes en literales de carácter.
        assert_eq!(kinds(r"'\x41'"), vec![TokenKind::Char('A'), TokenKind::Eof]);
        assert_eq!(kinds(r"'\0'"), vec![TokenKind::Char('\0'), TokenKind::Eof]);
        assert_eq!(kinds(r"'\u{1F600}'"), vec![TokenKind::Char('\u{1F600}'), TokenKind::Eof]);
        assert!(lex(r"'\u{110000}'").is_err());
    }

    #[test]
    fn dot_without_decimal_is_not_a_float() {
        // El número no se traga el punto si no le sigue un dígito: "1.x" debe ser
        // Int(1), Dot, Ident("x") (acceso a campo), no un flotante.
        assert_eq!(
            kinds("1.x"),
            vec![TokenKind::Int(1, Radix::Dec), TokenKind::Dot, TokenKind::Ident("x".into()), TokenKind::Eof]
        );
        // Y que "12.5" sí es flotante.
        assert_eq!(kinds("12.5"), vec![TokenKind::Float(12.5), TokenKind::Eof]);
    }

    #[test]
    fn escapes_in_string() {
        assert_eq!(
            kinds(r#""a\nb\t\\\"""#),
            vec![TokenKind::Str("a\nb\t\\\"".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn char_literal_with_escapes() {
        // M11.4c: 'a', escapes, y la keyword de tipo `char`.
        assert_eq!(kinds("'a'"), vec![TokenKind::Char('a'), TokenKind::Eof]);
        assert_eq!(
            kinds(r"'\n' '\t' '\\' '\''"),
            vec![
                TokenKind::Char('\n'),
                TokenKind::Char('\t'),
                TokenKind::Char('\\'),
                TokenKind::Char('\''),
                TokenKind::Eof,
            ]
        );
        assert_eq!(kinds("char"), vec![TokenKind::CharType, TokenKind::Eof]);
    }

    #[test]
    fn invalid_char_literal_is_error() {
        // Vacío, multi-carácter y sin cerrar son errores de lexado.
        assert!(crate::lexer::lex("''").is_err(), "'' vacío");
        assert!(crate::lexer::lex("'ab'").is_err(), "más de un carácter");
        assert!(crate::lexer::lex("'a").is_err(), "sin close");
    }

    #[test]
    fn keywords_vs_identifiers() {
        assert_eq!(
            kinds("let x int while123 ifx"),
            vec![
                TokenKind::Let,
                TokenKind::Ident("x".into()),
                TokenKind::IntType,
                TokenKind::Ident("while123".into()), // no es la keyword 'while'
                TokenKind::Ident("ifx".into()),      // ni 'if'
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keywords_of_m5() {
        // `enum` y `match` son palabras clave (M5); `=>` es un token propio.
        assert_eq!(
            kinds("enum match => enumx"),
            vec![
                TokenKind::Enum,
                TokenKind::Match,
                TokenKind::FatArrow,
                TokenKind::Ident("enumx".into()), // no es la keyword 'enum'
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn question_token() {
        assert_eq!(kinds("x?"), vec![TokenKind::Ident("x".into()), TokenKind::Question, TokenKind::Eof]);
    }

    #[test]
    fn reserved_at_token() {
        // '@' se lexea como token (reservado para anotaciones, M10); ya no es error.
        assert_eq!(kinds("@test"), vec![TokenKind::At, TokenKind::Ident("test".into()), TokenKind::Eof]);
    }

    #[test]
    fn token_pipeline_y_or() {
        // '|>' (pipeline, M7.2), '||' (or) y '|' (OR bit a bit, M19.3a) se distinguen.
        assert_eq!(
            kinds("a |> b || c | d"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::PipeArrow,
                TokenKind::Ident("b".into()),
                TokenKind::PipePipe,
                TokenKind::Ident("c".into()),
                TokenKind::Pipe,
                TokenKind::Ident("d".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn bitwise_operators() {
        // M19.3a: & | ^ ~ << >> sueltos. Ojo: '<<'/'>>' priman sobre '<'/'>'.
        assert_eq!(
            kinds("& | ^ ~ << >>"),
            vec![
                TokenKind::Amp,
                TokenKind::Pipe,
                TokenKind::Caret,
                TokenKind::Tilde,
                TokenKind::Shl,
                TokenKind::Shr,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn one_and_two_char_operators() {
        assert_eq!(
            kinds("== = != ! <= < >= > && || -> - + * / %"),
            vec![
                TokenKind::EqEq,
                TokenKind::Eq,
                TokenKind::BangEq,
                TokenKind::Bang,
                TokenKind::LtEq,
                TokenKind::Lt,
                TokenKind::GtEq,
                TokenKind::Gt,
                TokenKind::AmpAmp,
                TokenKind::PipePipe,
                TokenKind::Arrow,
                TokenKind::Minus,
                TokenKind::Plus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn comments_are_ignored() {
        assert_eq!(
            kinds("let // esto se ignora\n x"),
            vec![TokenKind::Let, TokenKind::Ident("x".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn positions_are_correct() {
        // fuente:
        //   let x      (línea 1)
        //     42       (línea 2, con 2 espacios de sangría)
        let toks = lex("let x\n  42").unwrap();
        // let @ 1:1
        assert_eq!((toks[0].line, toks[0].col), (1, 1));
        // x @ 1:5
        assert_eq!((toks[1].line, toks[1].col), (1, 5));
        // 42 @ 2:3
        assert_eq!((toks[2].line, toks[2].col), (2, 3));
        // Eof @ 2:5
        assert_eq!(toks[3].kind, TokenKind::Eof);
        assert_eq!((toks[3].line, toks[3].col), (2, 5));
    }

    #[test]
    fn lex_errors() {
        // carácter inesperado ('@' ya está reservado para anotaciones; usamos '#')
        let e = lex("#").unwrap_err();
        assert_eq!((e.line, e.col), (1, 1));
        assert!(e.msg.contains("unexpected"));

        // cadena sin cerrar
        let e = lex("\"sin close").unwrap_err();
        assert!(e.msg.contains("unterminated"));

        // escape inválido
        assert!(lex("\"\\q\"").is_err());
    }

    // Nota (M19.3a): '&' y '|' sueltos ya NO son error (son AND/OR bit a bit).

    #[test]
    fn program_fib_complete() {
        // El programa-objetivo de M1 (DESIGN.md §9) debe tokenizar sin error.
        let src = r#"
fn fib(n: int) -> int {
    if (n < 2) {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn main() -> int {
    var i: int = 0;
    while (i < 10) {
        print(fib(i));
        i = i + 1;
    }
    0
}
"#;
        let toks = lex(src).expect("fib must tokenizar");
        // Comprobaciones de cordura: empieza con 'fn fib (' y termina en Eof.
        assert_eq!(toks[0].kind, TokenKind::Fn);
        assert_eq!(toks[1].kind, TokenKind::Ident("fib".into()));
        assert_eq!(toks[2].kind, TokenKind::LParen);
        assert_eq!(toks.last().unwrap().kind, TokenKind::Eof);
        // 'print' es un identificador a nivel léxico (es un builtin, no keyword).
        assert!(toks.iter().any(|t| t.kind == TokenKind::Ident("print".into())));
    }
}
