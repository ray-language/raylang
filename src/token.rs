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
    True,
    False,
    Struct,
    Enum,  // M5
    Match, // M5

    // --- Palabras clave de tipo ---
    IntType,    // int
    FloatType,  // float
    BoolType,   // bool
    StringType, // string

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
