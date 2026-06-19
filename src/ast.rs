//! AST (árbol de sintaxis abstracta) de raylang.
//!
//! El parser (`parser.rs`) transforma la secuencia plana de tokens en este árbol,
//! que *sí* tiene estructura: una llamada contiene sus argumentos, un `if`
//! contiene su condición y sus ramas, etc. El AST es la moneda de cambio entre el
//! front-end y las fases siguientes (checker, intérprete).
//!
//! Es "abstracto" porque descarta detalles sintácticos irrelevantes para la
//! semántica: paréntesis de agrupación, espacios, el `;` final. Solo guarda la
//! forma esencial.
//!
//! Cada nodo de expresión y sentencia lleva su posición `(line, col)` para poder
//! reportar errores con ubicación en las fases posteriores.

/// Tipos que el programador puede escribir en M1 (DESIGN.md §4).
///
/// `Unit` no se escribe nunca de forma explícita: es el tipo de retorno implícito
/// de una función sin `-> ...`. Lo modelamos aquí para uniformar.
///
/// Nota de diseño: este enum está pensado para **crecer** (futuros structs, enums,
/// `Option<T>`, `Result<T,E>` añadirán una variante tipo `Named(String, Vec<Type>)`),
/// como dice la nota de arquitectura de DESIGN.md §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Bool,
    String,
    Unit,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Type::Int => "int",
            Type::Float => "float",
            Type::Bool => "bool",
            Type::String => "string",
            Type::Unit => "unit",
        };
        f.write_str(s)
    }
}

/// Un programa completo: una lista de funciones de nivel superior.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub functions: Vec<Function>,
}

/// Una función: `fn nombre(params) -> retorno { cuerpo }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Type, // Unit si se omitió el `-> ...`
    pub body: Block,
    pub line: usize,
    pub col: usize,
}

/// Un parámetro formal: `nombre: tipo`.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub line: usize,
    pub col: usize,
}

/// Un bloque `{ sentencias... [expresión-final] }`.
///
/// `tail` es la expresión final **sin** `;`: el valor del bloque (DESIGN.md §6).
/// Si es `None`, el bloque vale `unit`.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub line: usize,
    pub col: usize,
}

/// Una sentencia: se ejecuta por su efecto, no produce un valor de bloque.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// `let x: T = e;` (mutable=false) o `var x: T = e;` (mutable=true).
    Let {
        name: String,
        ty: Type,
        value: Expr,
        mutable: bool,
    },
    /// `x = e;` — reasignación de una variable existente.
    Assign { name: String, value: Expr },
    /// `return e;` o `return;`.
    Return { value: Option<Expr> },
    /// Una expresión usada como sentencia: su valor se descarta. P. ej.
    /// `print(x);` o `if (c) { ... }`.
    Expr(Expr),
}

/// Una expresión: produce un valor.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    // --- Literales ---
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),

    /// Referencia a una variable o función por nombre.
    Ident(String),

    /// Operador unario: `-x`, `!b`.
    Unary { op: UnaryOp, expr: Box<Expr> },

    /// Operador binario: `a + b`, `x < y`, `p && q`.
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },

    /// Llamada: `f(a, b)`. `callee` es lo que se llama (en M1, casi siempre un
    /// `Ident`, pero lo dejamos general para el futuro UFCS/closures).
    Call { callee: Box<Expr>, args: Vec<Expr> },

    // --- Expresiones con bloque (producen valor, DESIGN.md §6) ---
    /// `if (cond) { then } else { ... }`. `else_branch`, si existe, es otro `Expr`
    /// que será un `Block` o (en cadenas `else if`) otro `If`.
    If {
        cond: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Box<Expr>>,
    },
    /// `while (cond) { body }`. Su valor siempre es `unit`.
    While { cond: Box<Expr>, body: Block },
    /// Un bloque usado como expresión: `{ ...; valor }`.
    Block(Block),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg, // -
    Not, // !
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add, // +
    Sub, // -
    Mul, // *
    Div, // /
    Rem, // %
    Eq,  // ==
    Ne,  // !=
    Lt,  // <
    Le,  // <=
    Gt,  // >
    Ge,  // >=
    And, // &&
    Or,  // ||
}
