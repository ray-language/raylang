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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Bool,
    String,
    Unit,
    /// Arreglo dinámico de un tipo de elemento: `[T]`. Tipado estructural (M3).
    Array(Box<Type>),
    /// Un struct nominal, por su nombre: `Punto`. Tipado **nominal** (M3.2): la
    /// igualdad de tipos compara el nombre.
    Struct(String),
    /// Un enum (tipo suma) nominal, por su nombre: `Figura`. Tipado **nominal**
    /// (M5), igual que un struct: la igualdad de tipos compara el nombre. Como un
    /// identificador en posición de tipo puede ser un struct **o** un enum, el
    /// parser produce siempre `Struct(name)` y el checker reclasifica a `Enum` al
    /// resolver el nombre contra la tabla de tipos.
    Enum(String),
    /// Un tipo función: `fn(T1, T2) -> R` (M4.1). Las funciones son valores de
    /// primera clase: se pueden pasar, devolver y guardar. Tipado **estructural**
    /// (dos `fn(int) -> int` son el mismo tipo).
    Fn(Vec<Type>, Box<Type>),
    /// Un **parámetro de tipo** genérico: la `T` dentro de `fn id<T>(x: T) -> T`
    /// (M6). Es opaco: dos `Var` solo son iguales si tienen el mismo nombre. El
    /// parser produce `Struct(name)` para cualquier identificador en posición de
    /// tipo; el checker lo reclasifica a `Var` si el nombre es un parámetro de tipo
    /// en ámbito (igual que reclasifica a `Enum`).
    Var(String),
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => f.write_str("int"),
            Type::Float => f.write_str("float"),
            Type::Bool => f.write_str("bool"),
            Type::String => f.write_str("string"),
            Type::Unit => f.write_str("unit"),
            Type::Array(elem) => write!(f, "[{}]", elem),
            Type::Struct(name) => f.write_str(name),
            Type::Enum(name) => f.write_str(name),
            Type::Fn(params, ret) => {
                let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
                write!(f, "fn({}) -> {}", ps.join(", "), ret)
            }
            Type::Var(name) => f.write_str(name),
        }
    }
}

/// Un programa completo: definiciones de tipos (struct/enum) y funciones de nivel
/// superior.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub functions: Vec<Function>,
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
}

/// Definición de un struct: `struct Nombre { campo: Tipo, ... }` (M3.2). Los campos
/// se guardan **en orden de declaración**.
#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Type)>,
    pub line: usize,
    pub col: usize,
}

/// Definición de un enum (tipo suma): `enum Nombre { Variante(tipos...), ... }`
/// (M5). Las variantes se guardan **en orden de declaración**. Una variante lleva
/// un *payload* posicional (cero o más tipos); sin tipos es una variante *unit*.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<VariantDef>,
    pub line: usize,
    pub col: usize,
}

/// Una variante de enum: su nombre y su payload posicional (`Vec` vacío = unit).
#[derive(Debug, Clone, PartialEq)]
pub struct VariantDef {
    pub name: String,
    pub payload: Vec<Type>,
    pub line: usize,
    pub col: usize,
}

/// Una función: `fn nombre<params de tipo>(params) -> retorno { cuerpo }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    /// Parámetros de tipo: los `T, U` de `fn mapear<T, U>(...)` (M6). Vacío = no
    /// genérica. Dentro del cuerpo, cada nombre está en ámbito como `Type::Var`.
    pub type_params: Vec<String>,
    pub params: Vec<Param>,
    pub return_type: Type, // Unit si se omitió el `-> ...`
    pub body: Block,
    pub line: usize,
    pub col: usize,
}

/// Una función **anónima** usada como expresión: `fn(x: int) -> int { x + 1 }`
/// (M4.1). Comparte la forma de una función nombrada salvo el nombre.
///
/// `id` es un identificador único asignado por el parser (un contador global de
/// fn-exprs). Sirve para que el intérprete y el compilador asocien cada literal de
/// función con su entrada en la tabla de funciones, sin depender de punteros.
#[derive(Debug, Clone, PartialEq)]
pub struct FnExpr {
    pub id: usize,
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
    /// Asignación a un *lvalue*: `x = e;`, `a[i] = e;`, `p.x = e;` (M3.2).
    /// `target` es una expresión asignable (`Ident`, `Index`, o `Field`).
    Assign { target: Expr, value: Expr },
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

    /// Literal de arreglo: `[1, 2, 3]` (o `[]` vacío). (M3)
    ArrayLit(Vec<Expr>),

    /// Indexación: `a[i]`. (M3)
    Index { array: Box<Expr>, index: Box<Expr> },

    /// Literal de struct: `Punto { x: 1, y: 2 }`. (M3.2)
    StructLit { name: String, fields: Vec<(String, Expr)> },

    /// Acceso a campo: `p.x`. (M3.2)
    Field { object: Box<Expr>, name: String },

    /// Construcción de una variante de enum: `Figura.Circulo(2.0)` o, sin payload,
    /// `Figura.Punto` (`args` vacío). (M5)
    ///
    /// El parser **no** produce este nodo: `Enum.Variante` es sintácticamente igual
    /// a un acceso a campo `obj.campo`. Lo genera la **resolución** del checker, que
    /// reescribe los `Field`/`Call` cuya cabeza es un nombre de enum (ver
    /// `checker::resolve_enum_construction`). Así la ambigüedad se decide una sola
    /// vez y los dos motores reciben un AST explícito.
    EnumLit { enum_name: String, variant: String, args: Vec<Expr> },

    /// Función anónima como valor: `fn(x: int) -> int { x + 1 }`. (M4.1)
    Func(Box<FnExpr>),

    /// `match (escrutinio) { patrón => cuerpo, ... }` (M5.2). Es una **expresión**:
    /// produce el valor del brazo que casa. Los brazos se prueban en orden.
    Match { scrutinee: Box<Expr>, arms: Vec<MatchArm> },

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

/// Un brazo de `match`: un patrón y el cuerpo (una expresión) que se evalúa si casa.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub line: usize,
    pub col: usize,
}

/// Un patrón de `match` (M5.2). Patrones **planos** (un nivel): variante, comodín o
/// binding. Lleva ubicación para los errores de exhaustividad/aridad.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    /// `_`: descarta, no liga. Cubre **todo** lo restante (catch-all).
    Wildcard,
    /// Un identificador suelto: liga el escrutinio completo a ese nombre. También
    /// catch-all (cubre todo lo restante).
    Binding(String),
    /// `Enum.Variante(sub-bindings)`: casa con esa variante. Cada sub-binding liga
    /// una posición del payload a un nombre, o lo descarta si es `None` (`_`).
    Variant {
        enum_name: String,
        variant: String,
        bindings: Vec<Option<String>>,
    },
}

/// Recolecta todas las funciones anónimas (`FnExpr`) del programa, **indexadas por
/// su `id`**: el resultado en la posición `i` es la fn-expr con `id == i`. (M4.1)
///
/// Tanto el intérprete como el compilador necesitan una tabla de funciones que
/// incluya las anónimas; este recorrido del AST la construye una sola vez. Como el
/// parser asigna ids densos (`0..n`), el vector queda completo y sin huecos.
pub fn collect_fn_exprs(program: &Program) -> Vec<&FnExpr> {
    let mut acc: Vec<&FnExpr> = Vec::new();
    for f in &program.functions {
        walk_block(&f.body, &mut acc);
    }
    // Colocar cada fn-expr en la posición de su id.
    let mut by_id: Vec<Option<&FnExpr>> = (0..acc.len()).map(|_| None).collect();
    for fe in acc {
        by_id[fe.id] = Some(fe);
    }
    by_id.into_iter().map(|o| o.expect("ids de fn-expr densos")).collect()
}

fn walk_block<'a>(block: &'a Block, acc: &mut Vec<&'a FnExpr>) {
    for s in &block.statements {
        match &s.kind {
            StmtKind::Let { value, .. } => walk_expr(value, acc),
            StmtKind::Assign { target, value } => {
                walk_expr(target, acc);
                walk_expr(value, acc);
            }
            StmtKind::Return { value } => {
                if let Some(e) = value {
                    walk_expr(e, acc);
                }
            }
            StmtKind::Expr(e) => walk_expr(e, acc),
        }
    }
    if let Some(t) = &block.tail {
        walk_expr(t, acc);
    }
}

fn walk_expr<'a>(expr: &'a Expr, acc: &mut Vec<&'a FnExpr>) {
    match &expr.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) | ExprKind::Ident(_) => {}
        ExprKind::Unary { expr, .. } => walk_expr(expr, acc),
        ExprKind::Binary { left, right, .. } => {
            walk_expr(left, acc);
            walk_expr(right, acc);
        }
        ExprKind::Call { callee, args } => {
            walk_expr(callee, acc);
            for a in args {
                walk_expr(a, acc);
            }
        }
        ExprKind::ArrayLit(elems) => {
            for e in elems {
                walk_expr(e, acc);
            }
        }
        ExprKind::Index { array, index } => {
            walk_expr(array, acc);
            walk_expr(index, acc);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                walk_expr(e, acc);
            }
        }
        ExprKind::Field { object, .. } => walk_expr(object, acc),
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                walk_expr(a, acc);
            }
        }
        ExprKind::Func(fe) => {
            acc.push(fe);
            walk_block(&fe.body, acc); // fn-exprs anidadas
        }
        ExprKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, acc);
            for arm in arms {
                walk_expr(&arm.body, acc);
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            walk_expr(cond, acc);
            walk_block(then_branch, acc);
            if let Some(e) = else_branch {
                walk_expr(e, acc);
            }
        }
        ExprKind::While { cond, body } => {
            walk_expr(cond, acc);
            walk_block(body, acc);
        }
        ExprKind::Block(b) => walk_block(b, acc),
    }
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
