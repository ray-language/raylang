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

use crate::token::{InterpPart, Token, TokenKind};

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
        write!(f, "error léxico en {}:{}: {}", self.line, self.col, self.msg)
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
                    // La longitud del lexema (M33a): la emisión está centralizada y ningún
                    // token cruza líneas, así que basta restar columnas al terminar.
                    let len = (self.col - self.start_col).max(1);
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

            '"' => self.string()?,
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
                return Err(self.error(format!("carácter inesperado '{}'", other)));
            }
        };
        Ok(kind)
    }

    /// Lee el resto de un número una vez consumido su primer dígito. Decide entre
    /// entero y flotante según haya un '.' seguido de dígito.
    fn number(&mut self) -> Result<TokenKind, LexError> {
        let start = self.pos - 1; // el primer dígito ya fue consumido por next_token
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.advance();
        }

        // Es flotante solo si hay '.' Y un dígito tras el punto. Así "1." o
        // "1.metodo()" (futuro UFCS) no se confunden con un flotante.
        let is_float =
            self.peek() == Some('.') && matches!(self.peek_next(), Some(c) if c.is_ascii_digit());
        if is_float {
            self.advance(); // consume el '.'
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.advance();
            }
        }

        let text: String = self.chars[start..self.pos].iter().collect();
        if is_float {
            let v = text
                .parse::<f64>()
                .map_err(|_| self.error(format!("flotante inválido '{}'", text)))?;
            Ok(TokenKind::Float(v))
        } else {
            let v = text
                .parse::<i64>()
                .map_err(|_| self.error(format!("entero fuera de rango '{}'", text)))?;
            Ok(TokenKind::Int(v))
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

    /// Lee una cadena `"..."` (ya consumida la comilla de apertura). Toda cadena puede **interpolar**
    /// (M27.3, rediseñado): `${expr}` inserta el valor de `expr` (balanceando llaves anidadas). El `$`
    /// solo es especial seguido de `{`; en cualquier otro sitio es literal (`"$5"`, `"$PATH"` no
    /// necesitan escape). Las llaves `{` `}` son **siempre literales**. Escapes: `\n \t \r \\ \"` y
    /// **`\$`** (para un `${` literal, p. ej. al generar shell/plantillas). Sin ningún `${…}`, la
    /// cadena degrada a un `Str` normal → toda cadena preexistente sin `${` lexea byte-idéntico.
    fn string(&mut self) -> Result<TokenKind, LexError> {
        let mut parts: Vec<InterpPart> = Vec::new();
        let mut cur = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("cadena sin cerrar".into())),
                Some('\n') => {
                    return Err(self.error("salto de línea dentro de una cadena sin cerrar".into()))
                }
                Some('"') => {
                    self.advance(); // comilla de cierre
                    break;
                }
                Some('\\') => {
                    self.advance(); // la barra invertida
                    match self.peek() {
                        Some('n') => cur.push('\n'),
                        Some('t') => cur.push('\t'),
                        Some('r') => cur.push('\r'),
                        Some('\\') => cur.push('\\'),
                        Some('"') => cur.push('"'),
                        Some('$') => cur.push('$'), // `\$` → un `$` literal (para un `${` sin interpolar)
                        Some(other) => {
                            return Err(self.error(format!("secuencia de escape inválida '\\{}'", other)))
                        }
                        None => return Err(self.error("cadena sin cerrar tras '\\'".into())),
                    }
                    self.advance(); // el carácter escapado
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
                            None => return Err(self.error("interpolación '${' sin cerrar en la cadena".into())),
                            Some('\n') => return Err(self.error("salto de línea dentro de una interpolación".into())),
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
                        return Err(self.error("interpolación vacía '${}' en la cadena".into()));
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
                None => return Err(self.error("cadena de bytes sin cerrar".into())),
                Some('\n') => {
                    return Err(self.error("salto de línea dentro de una cadena de bytes sin cerrar".into()))
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
                            return Err(self.error(format!("secuencia de escape inválida '\\{}'", other)))
                        }
                        None => return Err(self.error("cadena de bytes sin cerrar tras '\\'".into())),
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

    /// Lee un dígito hexadecimal y devuelve su valor (0–15). Para el escape `\xNN` (M16.1a).
    fn hex_digit(&mut self) -> Result<u8, LexError> {
        match self.peek() {
            Some(c) if c.is_ascii_hexdigit() => {
                self.advance();
                Ok(c.to_digit(16).unwrap_or_else(|| crate::ice!("'{c}' pasó el guard de hexdigit")) as u8)
            }
            Some(other) => Err(self.error(format!("se esperaba un dígito hexadecimal tras '\\x', no '{}'", other))),
            None => Err(self.error("cadena de bytes sin cerrar (faltan dígitos hex tras '\\x')".into())),
        }
    }

    /// Lee un literal de carácter `'a'` (M11.4c), tras consumir la comilla de apertura. Acepta los
    /// mismos escapes que el string (`\n \t \\`) más `\'`. Exactamente un carácter entre comillas:
    /// `''` (vacío) o varios caracteres son error.
    fn char_literal(&mut self) -> Result<TokenKind, LexError> {
        let c = match self.peek() {
            None => return Err(self.error("carácter sin cerrar".into())),
            Some('\'') => return Err(self.error("un literal de carácter no puede estar vacío ('')".into())),
            Some('\n') => return Err(self.error("salto de línea dentro de un literal de carácter".into())),
            Some('\\') => {
                self.advance(); // la barra invertida
                let escaped = match self.peek() {
                    Some('n') => '\n',
                    Some('t') => '\t',
                    Some('r') => '\r', // M14: retorno de carro
                    Some('\\') => '\\',
                    Some('\'') => '\'',
                    Some(other) => return Err(self.error(format!("secuencia de escape inválida '\\{}'", other))),
                    None => return Err(self.error("carácter sin cerrar tras '\\'".into())),
                };
                self.advance(); // el carácter escapado
                escaped
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
            _ => Err(self.error("se esperaba ''' para cerrar el literal de carácter (¿más de un carácter?)".into())),
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
        "as" => TokenKind::As,
        "int" => TokenKind::IntType,
        "float" => TokenKind::FloatType,
        "bool" => TokenKind::BoolType,
        "string" => TokenKind::StringType,
        "char" => TokenKind::CharType,
        "bytes" => TokenKind::BytesType,
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
    fn interpolacion_de_cadenas() {
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
    fn los_tokens_llevan_su_longitud() {
        // M33a: cada token mide su lexema en caracteres (col..col+len es el span).
        let toks = lex("let foo = 12345 + \"ab\\n\";").expect("tokeniza");
        let lens: Vec<(TokenKind, usize)> =
            toks.into_iter().map(|t| (t.kind, t.len)).collect();
        assert_eq!(lens[0], (TokenKind::Let, 3));
        assert_eq!(lens[1], (TokenKind::Ident("foo".into()), 3));
        assert_eq!(lens[2], (TokenKind::Eq, 1));
        assert_eq!(lens[3], (TokenKind::Int(12345), 5));
        assert_eq!(lens[4], (TokenKind::Plus, 1));
        // El string mide su forma ESCRITA (comillas y escapes incluidos): "ab\n" son 6 chars.
        assert_eq!(lens[5], (TokenKind::Str("ab\n".into()), 6));
        assert_eq!(lens[6], (TokenKind::Semicolon, 1));
        assert_eq!(lens[7], (TokenKind::Eof, 1));
    }

    #[test]
    fn el_error_lexico_lleva_su_extension() {
        // M33a: "cadena sin cerrar" subraya desde la comilla hasta donde se rompió.
        let e = lex("let s = \"hola").unwrap_err();
        assert_eq!((e.line, e.col), (1, 9));
        assert_eq!(e.len, 5, "desde la comilla hasta el final: \"hola son 5 chars");
        // Un carácter inesperado mide 1.
        let e = lex("let # = 1").unwrap_err();
        assert_eq!(e.len, 1);
    }

    #[test]
    fn vacio_produce_solo_eof() {
        assert_eq!(kinds(""), vec![TokenKind::Eof]);
        assert_eq!(kinds("   \n\t  "), vec![TokenKind::Eof]);
    }

    #[test]
    fn literales() {
        assert_eq!(kinds("42"), vec![TokenKind::Int(42), TokenKind::Eof]);
        assert_eq!(kinds("3.14"), vec![TokenKind::Float(3.14), TokenKind::Eof]);
        assert_eq!(
            kinds("\"hola\""),
            vec![TokenKind::Str("hola".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn punto_sin_decimal_no_es_flotante() {
        // El número no se traga el punto si no le sigue un dígito: "1.x" debe ser
        // Int(1), Dot, Ident("x") (acceso a campo), no un flotante.
        assert_eq!(
            kinds("1.x"),
            vec![TokenKind::Int(1), TokenKind::Dot, TokenKind::Ident("x".into()), TokenKind::Eof]
        );
        // Y que "12.5" sí es flotante.
        assert_eq!(kinds("12.5"), vec![TokenKind::Float(12.5), TokenKind::Eof]);
    }

    #[test]
    fn escapes_en_cadena() {
        assert_eq!(
            kinds(r#""a\nb\t\\\"""#),
            vec![TokenKind::Str("a\nb\t\\\"".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn literal_de_caracter_con_escapes() {
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
    fn literal_de_caracter_invalido_es_error() {
        // Vacío, multi-carácter y sin cerrar son errores de lexado.
        assert!(crate::lexer::lex("''").is_err(), "'' vacío");
        assert!(crate::lexer::lex("'ab'").is_err(), "más de un carácter");
        assert!(crate::lexer::lex("'a").is_err(), "sin cerrar");
    }

    #[test]
    fn palabras_clave_vs_identificadores() {
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
    fn palabras_clave_de_m5() {
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
    fn token_de_interrogacion() {
        assert_eq!(kinds("x?"), vec![TokenKind::Ident("x".into()), TokenKind::Question, TokenKind::Eof]);
    }

    #[test]
    fn token_arroba_reservado() {
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
    fn operadores_bit_a_bit() {
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
    fn operadores_de_uno_y_dos_caracteres() {
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
    fn comentarios_se_ignoran() {
        assert_eq!(
            kinds("let // esto se ignora\n x"),
            vec![TokenKind::Let, TokenKind::Ident("x".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn posiciones_son_correctas() {
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
    fn errores_lexicos() {
        // carácter inesperado ('@' ya está reservado para anotaciones; usamos '#')
        let e = lex("#").unwrap_err();
        assert_eq!((e.line, e.col), (1, 1));
        assert!(e.msg.contains("inesperado"));

        // cadena sin cerrar
        let e = lex("\"sin cerrar").unwrap_err();
        assert!(e.msg.contains("sin cerrar"));

        // escape inválido
        assert!(lex("\"\\q\"").is_err());
    }

    // Nota (M19.3a): '&' y '|' sueltos ya NO son error (son AND/OR bit a bit).

    #[test]
    fn programa_fib_completo() {
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
        let toks = lex(src).expect("fib debe tokenizar");
        // Comprobaciones de cordura: empieza con 'fn fib (' y termina en Eof.
        assert_eq!(toks[0].kind, TokenKind::Fn);
        assert_eq!(toks[1].kind, TokenKind::Ident("fib".into()));
        assert_eq!(toks[2].kind, TokenKind::LParen);
        assert_eq!(toks.last().unwrap().kind, TokenKind::Eof);
        // 'print' es un identificador a nivel léxico (es un builtin, no keyword).
        assert!(toks.iter().any(|t| t.kind == TokenKind::Ident("print".into())));
    }
}
