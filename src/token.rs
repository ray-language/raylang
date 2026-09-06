//! Tokens de raylang.
//!
//! Un *token* es la unidad léxica mínima con significado: un número, una palabra
//! clave, un operador, un paréntesis. El lexer (ver `lexer.rs`) transforma el
//! texto fuente en una secuencia de `Token`. Cada token lleva su posición
//! `(línea, columna)` para poder dar errores con ubicación (principio 3 del
//! diseño).

/// La base en que se ESCRIBIÓ un literal entero (M118). El valor ya está
/// interpretado a `i64`; la base solo sirve para que el formateador reimprima el
/// literal como lo escribió el usuario (`0xFF`, `0o755`, `0b1010`) en vez de
/// canonizarlo a decimal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    Dec,
    Hex,
    Oct,
    Bin,
}

/// Cómo se escribió un literal entero (M118 base; M192 sufijo). `suffix` = `Some(8|32|64)` si el
/// literal lleva `u8`/`u32`/`u64` — o si es **amplio** (no cabe en `int`, solo en `u64`: se guarda
/// como sus 64 bits en `i64` y `suffix == Some(64)`). El formateador lo reemite tal cual; el checker
/// lo tipa directamente como `uN`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Radix {
    pub base: Base,
    /// Sufijo ESCRITO (`u8`/`u32`/`u64`), si lo hay.
    pub suffix: Option<u8>,
    /// Literal AMPLIO: no cabe en `int`, solo en `u64` (sus 64 bits van en el `i64` del token). Se
    /// tipa como `u64` sin sufijo escrito, y el formateador lo reemite sin sufijo.
    pub wide: bool,
}

impl Radix {
    pub const DEC: Radix = Radix { base: Base::Dec, suffix: None, wide: false };
    pub const HEX: Radix = Radix { base: Base::Hex, suffix: None, wide: false };
    pub const OCT: Radix = Radix { base: Base::Oct, suffix: None, wide: false };
    pub const BIN: Radix = Radix { base: Base::Bin, suffix: None, wide: false };
    pub fn with_suffix(self, suffix: Option<u8>) -> Radix {
        Radix { suffix, ..self }
    }
    pub fn with_wide(self) -> Radix {
        Radix { wide: true, ..self }
    }
    /// El ancho FIJO del literal, si lo tiene: el sufijo escrito, o 64 si es amplio.
    pub fn fixed_width(&self) -> Option<u8> {
        self.suffix.or(if self.wide { Some(64) } else { None })
    }
}

/// El "qué es" de un token. Las variantes siguen la sección 3 de DESIGN.md.
///
/// Cuidado con la nomenclatura: `Int(i64, _)` es el **literal** entero `42`,
/// mientras que `IntType` es la **palabra clave de tipo** `int`. Lo mismo para
/// `Float`/`FloatType` y `Str`/`StringType`.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // --- Literales (cargan su valor ya interpretado) ---
    Int(i64, Radix), // 42, 0xFF, 0o755, 0b1010
    Float(f64),  // 3.14
    Str(String), // "hola\n"  (escapes ya resueltos)
    /// Cadena con **interpolación** `"a${x}b"` (M27.3): partes literales y expresiones (código crudo).
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
    /// M191: `break` / `continue` — sentencias de salida de bucle (SPEC §5).
    Break,
    Continue,
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
    Extern, // M41 (extern "lib" { fn … } — declara funciones C para FFI)
    As,     // M11.3b (renombrado en un from-import)

    // --- Palabras clave de tipo ---
    IntType,    // int
    FloatType,  // float
    BoolType,   // bool
    StringType, // string
    CharType,   // char (M11.4c)
    BytesType,  // bytes (M16.1a)
    PtrType,    // ptr (M41.4b: puntero opaco foráneo, FFI)
    UIntType(u8), // u8/u32/u64 (M28.3); el u8 es el ancho en bits

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

/// Una parte de una cadena interpolada (M27.3): texto literal, o el código crudo de una expresión `${…}`.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpPart {
    Lit(String),
    /// El código crudo de una expresión `${…}` más la `(línea, col)` donde empieza en la fuente
    /// (tras `${`), para que el parser la re-lexe con posiciones reales (hover del LSP en `${x}`).
    Expr(String, usize, usize),
}

/// Un token concreto en el texto: su clase, dónde empieza y cuánto mide.
///
/// `line` y `col` son 1-basados (la primera posición es 1:1), que es lo que un
/// humano espera ver en un mensaje de error. `len` (M33a) es la longitud del
/// lexema en **caracteres**: junto con `col` forma el *span* del token
/// (`[col, col+len)`), que los diagnósticos subrayan completo. Es exacta porque
/// ningún token de raylang cruza líneas (el lexer rechaza el salto de línea
/// dentro de una cadena).
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

impl Token {
    pub fn new(kind: TokenKind, line: usize, col: usize, len: usize) -> Self {
        Token { kind, line, col, len }
    }
}
