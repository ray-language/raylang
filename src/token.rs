//! Tokens de raylang.
//!
//! Un *token* es la unidad léxica mínima con significado: un número, una palabra
//! clave, un operador, un paréntesis. El lexer (ver `lexer.rs`) transforma el
//! texto fuente en una secuencia de `Token`. Cada token lleva su posición
//! `(línea, columna)` para poder dar errores con ubicación (principio 3 del
//! diseño).

/// El "qué es" de un token. Las variantes siguen la sección 3 de DESIGN.md.
///
/// Cuidado con la nomenclatura: `Int(i64)` es el **literal** entero `42`,
/// mientras que `IntType` es la **palabra clave de tipo** `int`. Lo mismo para
/// `Float`/`FloatType` y `Str`/`StringType`.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // --- Literales (cargan su valor ya interpretado) ---
    Int(i64),    // 42
    Float(f64),  // 3.14
    Str(String), // "hola\n"  (escapes ya resueltos)
    /// Cadena con **interpolación** `"a{x}b"` (M27.3): partes literales y expresiones (código crudo).
    /// El parser la baja a concatenación con `to_string` de cada expresión.
    InterpStr(Vec<InterpPart>),
    Char(char),  // 'a'  (M11.4c; escapes ya resueltos)
    Bytes(Vec<u8>), // b"..."  (M16.1a; escapes resueltos, incl. \xNN)

    // --- Identificador ---
    Ident(String), // nombre de variable o función

    // --- Palabras clave ---
    Let,
    Var,
    Fn,
    Return,
    If,
    Else,
    While,
    For, // M27.2
    In,  // M27.2
    True,
    False,
    Struct,
    Const, // M27.5
    Enum,  // M5
    Match, // M5
    Trait, // M9
    Impl,  // M9
    Dyn,   // M9.3b (dyn Trait: trait object)
    Pub,    // M11.3 (visibilidad: exporta un ítem del módulo)
    Import, // M11.3 (import M; — importa un módulo como espacio de nombres)
    From,   // M11.3b (from M import a [as b]; — trae nombres al ámbito)
    As,     // M11.3b (renombrado en un from-import)

    // --- Palabras clave de tipo ---
    IntType,    // int
    FloatType,  // float
    BoolType,   // bool
    StringType, // string
    CharType,   // char (M11.4c)
    BytesType,  // bytes (M16.1a)

    // --- Operadores ---
    Plus,    // +
    Minus,   // -
    Star,    // *
    Slash,   // /
    Percent, // %
    EqEq,    // ==
    BangEq,  // !=
    Lt,      // <
    LtEq,    // <=
    Gt,      // >
    GtEq,    // >=
    AmpAmp,  // &&
    PipePipe,// ||
    Bang,    // !
    Eq,      // =
    // Operadores bit a bit (M19.3a): habilitan SHA-1 / base64 / framing de WebSocket.
    Amp,     // &  (AND bit a bit)
    Pipe,    // |  (OR bit a bit)
    Caret,   // ^  (XOR bit a bit)
    Tilde,   // ~  (NOT bit a bit, unario)
    Shl,     // << (desplazamiento a la izquierda)
    Shr,     // >> (desplazamiento a la derecha)

    // --- Puntuación / agrupación ---
    LParen,    // (
    RParen,    // )
    LBrace,    // {
    RBrace,    // }
    LBracket,  // [
    RBracket,  // ]
    Comma,     // ,
    Semicolon, // ;
    Colon,     // :
    Dot,       // .
    DotDot,    // ..  (rango, M27.2)
    Arrow,     // ->
    FatArrow,  // =>  (brazos de match, M5)
    Question,  // ?   (propagación de errores, M6)
    PipeArrow, // |>  (pipeline, M7.2)
    At,        // @   (anotaciones, reservado para M10)

    // --- Marca de fin de entrada ---
    // El parser se apoya en este token centinela para saber dónde termina todo
    // sin tener que comprobar continuamente "¿quedan tokens?".
    Eof,
}

/// Una parte de una cadena interpolada (M27.3): texto literal, o el código crudo de una expresión `{…}`.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpPart {
    Lit(String),
    Expr(String),
}

/// Un token concreto en el texto: su clase y dónde empieza.
///
/// `line` y `col` son 1-basados (la primera posición es 1:1), que es lo que un
/// humano espera ver en un mensaje de error.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub col: usize,
}

impl Token {
    pub fn new(kind: TokenKind, line: usize, col: usize) -> Self {
        Token { kind, line, col }
    }
}
