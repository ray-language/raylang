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

use crate::token::{Token, TokenKind};

/// Error léxico con ubicación. Se produce ante un carácter inesperado, una cadena
/// sin cerrar, un escape inválido o un número mal formado.
#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
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
                    tokens.push(Token::new(TokenKind::Eof, self.start_line, self.start_col));
                    return Ok(tokens);
                }
                Some(_) => {
                    let kind = self.next_token()?;
                    tokens.push(Token::new(kind, self.start_line, self.start_col));
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
            '.' => TokenKind::Dot,

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
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.match_char('=') {
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                }
            }
            // '&' y '|' solo existen duplicados en M1 (&& y ||). Sueltos son error.
            '&' => {
                if self.match_char('&') {
                    TokenKind::AmpAmp
                } else {
                    return Err(self.error("se esperaba '&&' (¿olvidaste un '&'?)".into()));
                }
            }
            '|' => {
                if self.match_char('|') {
                    TokenKind::PipePipe
                } else {
                    return Err(self.error("se esperaba '||' (el pipeline '|>' llega en M7)".into()));
                }
            }

            '"' => self.string()?,
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

    /// Lee una cadena `"..."` (ya consumida la comilla de apertura), resolviendo
    /// los escapes `\n \t \\ \"`.
    fn string(&mut self) -> Result<TokenKind, LexError> {
        let mut value = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("cadena sin cerrar".into())),
                Some('\n') => {
                    return Err(self.error("salto de línea dentro de una cadena sin cerrar".into()))
                }
                Some('"') => {
                    self.advance(); // comilla de cierre
                    return Ok(TokenKind::Str(value));
                }
                Some('\\') => {
                    self.advance(); // la barra invertida
                    match self.peek() {
                        Some('n') => value.push('\n'),
                        Some('t') => value.push('\t'),
                        Some('\\') => value.push('\\'),
                        Some('"') => value.push('"'),
                        Some(other) => {
                            return Err(self.error(format!("secuencia de escape inválida '\\{}'", other)))
                        }
                        None => return Err(self.error("cadena sin cerrar tras '\\'".into())),
                    }
                    self.advance(); // el carácter escapado
                }
                Some(c) => {
                    value.push(c);
                    self.advance();
                }
            }
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

    /// Construye un `LexError` apuntando al inicio del token actual.
    fn error(&self, msg: String) -> LexError {
        LexError {
            msg,
            line: self.start_line,
            col: self.start_col,
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
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "struct" => TokenKind::Struct,
        "enum" => TokenKind::Enum,
        "match" => TokenKind::Match,
        "int" => TokenKind::IntType,
        "float" => TokenKind::FloatType,
        "bool" => TokenKind::BoolType,
        "string" => TokenKind::StringType,
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
        // carácter inesperado
        let e = lex("@").unwrap_err();
        assert_eq!((e.line, e.col), (1, 1));
        assert!(e.msg.contains("inesperado"));

        // cadena sin cerrar
        let e = lex("\"sin cerrar").unwrap_err();
        assert!(e.msg.contains("sin cerrar"));

        // '&' suelto
        assert!(lex("a & b").is_err());

        // escape inválido
        assert!(lex("\"\\q\"").is_err());
    }

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
