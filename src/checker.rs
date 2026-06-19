//! Type checker (análisis semántico) de raylang.
//!
//! Tercera fase del pipeline (DESIGN.md §2, reglas en §8). El parser garantiza que
//! el programa es sintácticamente válido; el checker garantiza que *tiene
//! sentido*: que no sumas un `bool` con un `string`, que no usas variables sin
//! declarar, que `fib` realmente devuelve `int`, etc. Un programa que pasa el
//! checker no puede fallar por un error de tipos en tiempo de ejecución.
//!
//! ## Dos pasadas
//!
//! 1. **Pre-pasada**: registra la firma de cada función (parámetros y retorno).
//!    Así una función puede llamar a otra declarada más abajo, y a sí misma
//!    (recursión), sin que el orden importe.
//! 2. **Verificación**: recorre el cuerpo de cada función comprobando las reglas.
//!
//! ## Ámbitos (scopes)
//!
//! Las variables viven en una **pila de ámbitos**. Cada bloque empuja un ámbito y
//! lo retira al salir. Buscar un nombre recorre la pila de dentro hacia afuera, lo
//! que da *shadowing* (una variable interior tapa una exterior) de forma natural.
//!
//! ## Una nota sobre el flujo
//!
//! Como raylang es orientado a expresiones, el cuerpo de una función `-> int` debe
//! *producir* un `int` (retorno implícito). Pero también vale salir antes con
//! `return`. Para aceptar `fn f() -> int { return 5; }` (sin expresión final)
//! hacemos un pequeño análisis de **divergencia**: si todos los caminos del bloque
//! terminan en `return`, el bloque "diverge" y no necesita valor final.

use std::collections::HashMap;

use crate::ast::*;

/// Error de tipos con ubicación.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error de tipos en {}:{}: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for TypeError {}

/// Firma de una función: tipos de parámetros y tipo de retorno.
struct FnSig {
    params: Vec<Type>,
    ret: Type,
}

/// Información de una variable en un ámbito.
struct VarInfo {
    ty: Type,
    mutable: bool,
}

/// Punto de entrada de la fase: verifica un programa completo.
pub fn check(program: &Program) -> Result<(), TypeError> {
    Checker::new().check_program(program)
}

struct Checker {
    /// Firmas de todas las funciones (llenada en la pre-pasada).
    functions: HashMap<String, FnSig>,
    /// Pila de ámbitos de variables. El último es el más interno.
    scopes: Vec<HashMap<String, VarInfo>>,
    /// Tipo de retorno de la función que estamos verificando ahora mismo, para
    /// validar las sentencias `return`.
    current_return: Type,
}

impl Checker {
    fn new() -> Self {
        Checker {
            functions: HashMap::new(),
            scopes: Vec::new(),
            current_return: Type::Unit,
        }
    }

    fn check_program(&mut self, program: &Program) -> Result<(), TypeError> {
        // --- Pre-pasada: registrar firmas ---
        for f in &program.functions {
            if self.functions.contains_key(&f.name) {
                return Err(self.err(f.line, f.col, format!("función '{}' declarada dos veces", f.name)));
            }
            let sig = FnSig {
                params: f.params.iter().map(|p| p.ty.clone()).collect(),
                ret: f.return_type.clone(),
            };
            self.functions.insert(f.name.clone(), sig);
        }

        // 'main' es obligatoria (DESIGN.md §11): sin parámetros y con retorno int o unit.
        match self.functions.get("main") {
            None => return Err(self.err(1, 1, "falta la función de entrada 'main'".into())),
            Some(sig) => {
                if !sig.params.is_empty() {
                    return Err(self.err(1, 1, "'main' no debe recibir parámetros".into()));
                }
                if sig.ret != Type::Int && sig.ret != Type::Unit {
                    return Err(self.err(1, 1, format!("'main' debe devolver int o unit, no {}", sig.ret)));
                }
            }
        }

        // --- Verificación de cada función ---
        for f in &program.functions {
            self.check_function(f)?;
        }
        Ok(())
    }

    fn check_function(&mut self, f: &Function) -> Result<(), TypeError> {
        self.current_return = f.return_type.clone();
        self.push_scope();

        // Los parámetros son inmutables (no hay 'var' para ellos).
        for p in &f.params {
            self.declare(&p.name, p.ty.clone(), false);
        }

        // El cuerpo se verifica como un bloque; su valor es el retorno implícito.
        let body_ty = self.check_block(&f.body)?;
        let diverges = block_diverges(&f.body);

        // Posición para el posible error: la expresión final si existe, si no la fn.
        let (eline, ecol) = match &f.body.tail {
            Some(t) => (t.line, t.col),
            None => (f.line, f.col),
        };

        if f.return_type == Type::Unit {
            // Una función unit no debe terminar produciendo un valor.
            if body_ty != Type::Unit && !diverges {
                return Err(self.err(eline, ecol, format!(
                    "'{}' no declara retorno (unit), pero su cuerpo produce {}",
                    f.name, body_ty
                )));
            }
        } else if !(body_ty == f.return_type || diverges) {
            return Err(self.err(eline, ecol, format!(
                "'{}' declara devolver {}, pero su cuerpo produce {}",
                f.name, f.return_type, body_ty
            )));
        }

        self.pop_scope();
        Ok(())
    }

    // ----- Sentencias -----

    fn check_stmt(&mut self, stmt: &Stmt) -> Result<(), TypeError> {
        match &stmt.kind {
            StmtKind::Let { name, ty, value, mutable } => {
                // Caso especial: `[]` adopta el tipo de arreglo declarado (no hay
                // de dónde inferir el tipo de elemento de un arreglo vacío).
                let vt = if matches!(&value.kind, ExprKind::ArrayLit(e) if e.is_empty()) {
                    if matches!(ty, Type::Array(_)) {
                        ty.clone()
                    } else {
                        return Err(self.err(value.line, value.col, format!(
                            "'{}' se declara como {} pero se inicializa con un arreglo vacío",
                            name, ty
                        )));
                    }
                } else {
                    self.check_expr(value)?
                };
                if vt != *ty {
                    return Err(self.err(value.line, value.col, format!(
                        "'{}' se declara como {} pero se inicializa con {}",
                        name, ty, vt
                    )));
                }
                self.declare(name, ty.clone(), *mutable);
                Ok(())
            }
            StmtKind::Assign { target, value } => self.check_assign(target, value, stmt.line, stmt.col),
            StmtKind::Return { value } => {
                let vt = match value {
                    Some(e) => self.check_expr(e)?,
                    None => Type::Unit,
                };
                if vt != self.current_return {
                    return Err(self.err(stmt.line, stmt.col, format!(
                        "se devuelve {} pero la función declara retorno {}",
                        vt, self.current_return
                    )));
                }
                Ok(())
            }
            StmtKind::Expr(e) => {
                // Una expresión-sentencia solo debe estar bien tipada; su valor se
                // descarta.
                self.check_expr(e)?;
                Ok(())
            }
        }
    }

    /// Verifica una asignación a un lvalue.
    fn check_assign(&mut self, target: &Expr, value: &Expr, line: usize, col: usize) -> Result<(), TypeError> {
        match &target.kind {
            // x = e  — requiere que la variable exista y sea mutable ('var').
            ExprKind::Ident(name) => {
                let (var_ty, mutable) = match self.lookup(name) {
                    Some(v) => (v.ty.clone(), v.mutable),
                    None => return Err(self.err(target.line, target.col, format!("variable '{}' no declarada", name))),
                };
                if !mutable {
                    return Err(self.err(line, col, format!(
                        "no se puede asignar a '{}': es inmutable (declarada con 'let'; usa 'var')",
                        name
                    )));
                }
                let vt = self.check_expr(value)?;
                if vt != var_ty {
                    return Err(self.err(value.line, value.col, format!("'{}' es {} pero se le asigna {}", name, var_ty, vt)));
                }
                Ok(())
            }
            // a[i] = e  — mutar el contenido NO requiere 'var' (DESIGN §12.3): la
            // inmutabilidad de `let` ata la variable, no congela el objeto.
            ExprKind::Index { array, index } => {
                let elem = self.check_index(array, index)?;
                let vt = self.check_expr(value)?;
                if vt != elem {
                    return Err(self.err(value.line, value.col, format!("el elemento es {} pero se le asigna {}", elem, vt)));
                }
                Ok(())
            }
            _ => Err(self.err(target.line, target.col, "el lado izquierdo no es asignable".into())),
        }
    }

    /// Verifica `a[i]` y devuelve el tipo de elemento. Reusado por la indexación
    /// como expresión y como destino de asignación.
    fn check_index(&mut self, array: &Expr, index: &Expr) -> Result<Type, TypeError> {
        let at = self.check_expr(array)?;
        let it = self.check_expr(index)?;
        if it != Type::Int {
            return Err(self.err(index.line, index.col, format!("el índice debe ser int, no {}", it)));
        }
        match at {
            Type::Array(elem) => Ok(*elem),
            other => Err(self.err(array.line, array.col, format!("no se puede indexar un {} (no es un arreglo)", other))),
        }
    }

    // ----- Expresiones (devuelven su tipo) -----

    fn check_expr(&mut self, expr: &Expr) -> Result<Type, TypeError> {
        match &expr.kind {
            ExprKind::Int(_) => Ok(Type::Int),
            ExprKind::Float(_) => Ok(Type::Float),
            ExprKind::Bool(_) => Ok(Type::Bool),
            ExprKind::Str(_) => Ok(Type::String),

            ExprKind::Ident(name) => match self.lookup(name) {
                Some(v) => Ok(v.ty.clone()),
                None => Err(self.err(expr.line, expr.col, format!("nombre '{}' no declarado", name))),
            },

            ExprKind::Unary { op, expr: inner } => {
                let t = self.check_expr(inner)?;
                match op {
                    UnaryOp::Neg if t == Type::Int || t == Type::Float => Ok(t),
                    UnaryOp::Neg => Err(self.err(expr.line, expr.col, format!("no se puede negar (-) un {}", t))),
                    UnaryOp::Not if t == Type::Bool => Ok(Type::Bool),
                    UnaryOp::Not => Err(self.err(expr.line, expr.col, format!("el '!' requiere bool, no {}", t))),
                }
            }

            ExprKind::Binary { op, left, right } => self.check_binary(*op, left, right, expr.line, expr.col),

            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.line, expr.col),

            ExprKind::ArrayLit(elems) => {
                if elems.is_empty() {
                    return Err(self.err(expr.line, expr.col,
                        "no se puede inferir el tipo de [] aquí; anótalo (p. ej. let xs: [int] = [];)".into()));
                }
                let first = self.check_expr(&elems[0])?;
                for e in &elems[1..] {
                    let t = self.check_expr(e)?;
                    if t != first {
                        return Err(self.err(e.line, e.col, format!(
                            "los elementos del arreglo deben ser del mismo tipo: {} y {}", first, t
                        )));
                    }
                }
                Ok(Type::Array(Box::new(first)))
            }

            ExprKind::Index { array, index } => self.check_index(array, index),

            ExprKind::If { cond, then_branch, else_branch } => {
                let ct = self.check_expr(cond)?;
                if ct != Type::Bool {
                    return Err(self.err(cond.line, cond.col, format!("la condición del if debe ser bool, no {}", ct)));
                }
                let then_ty = self.check_block(then_branch)?;
                match else_branch {
                    None => {
                        // Un if sin else tiene tipo unit; entonces la rama 'then'
                        // tampoco puede producir un valor útil.
                        if then_ty != Type::Unit {
                            return Err(self.err(expr.line, expr.col, format!(
                                "un if sin else tiene tipo unit, pero su rama produce {} (añade un else)",
                                then_ty
                            )));
                        }
                        Ok(Type::Unit)
                    }
                    Some(else_e) => {
                        let else_ty = self.check_expr(else_e)?;
                        if then_ty != else_ty {
                            return Err(self.err(expr.line, expr.col, format!(
                                "las ramas del if tienen tipos distintos: {} y {}",
                                then_ty, else_ty
                            )));
                        }
                        Ok(then_ty)
                    }
                }
            }

            ExprKind::While { cond, body } => {
                let ct = self.check_expr(cond)?;
                if ct != Type::Bool {
                    return Err(self.err(cond.line, cond.col, format!("la condición del while debe ser bool, no {}", ct)));
                }
                // El valor del cuerpo se descarta en cada iteración; el while es unit.
                self.check_block(body)?;
                Ok(Type::Unit)
            }

            ExprKind::Block(b) => self.check_block(b),
        }
    }

    /// Verifica un bloque en su propio ámbito y devuelve su tipo-valor (el de la
    /// expresión final, o unit si no hay).
    fn check_block(&mut self, block: &Block) -> Result<Type, TypeError> {
        self.push_scope();
        for stmt in &block.statements {
            self.check_stmt(stmt)?;
        }
        let ty = match &block.tail {
            Some(e) => self.check_expr(e)?,
            None => Type::Unit,
        };
        self.pop_scope();
        Ok(ty)
    }

    fn check_binary(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        line: usize,
        col: usize,
    ) -> Result<Type, TypeError> {
        let lt = self.check_expr(left)?;
        let rt = self.check_expr(right)?;
        use BinaryOp::*;
        match op {
            // Aritméticos: ambos int → int, ambos float → float. Sin mezclas.
            Add | Sub | Mul | Div | Rem => match (&lt, &rt) {
                (Type::Int, Type::Int) => Ok(Type::Int),
                (Type::Float, Type::Float) => Ok(Type::Float),
                _ => Err(self.err(line, col, format!(
                    "el operador '{}' requiere ambos operandos int o ambos float, no {} y {}",
                    bin_op_str(op), lt, rt
                ))),
            },
            // Orden: solo números, del mismo tipo → bool.
            Lt | Le | Gt | Ge => match (&lt, &rt) {
                (Type::Int, Type::Int) | (Type::Float, Type::Float) => Ok(Type::Bool),
                _ => Err(self.err(line, col, format!(
                    "el operador '{}' compara números del mismo tipo, no {} y {}",
                    bin_op_str(op), lt, rt
                ))),
            },
            // Igualdad: mismo tipo y comparable → bool.
            Eq | Ne => {
                if lt == rt && is_comparable(&lt) {
                    Ok(Type::Bool)
                } else {
                    Err(self.err(line, col, format!(
                        "el operador '{}' requiere ambos operandos del mismo tipo comparable, no {} y {}",
                        bin_op_str(op), lt, rt
                    )))
                }
            }
            // Lógicos: ambos bool → bool.
            And | Or => {
                if lt == Type::Bool && rt == Type::Bool {
                    Ok(Type::Bool)
                } else {
                    Err(self.err(line, col, format!(
                        "el operador '{}' requiere operandos bool, no {} y {}",
                        bin_op_str(op), lt, rt
                    )))
                }
            }
        }
    }

    fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        line: usize,
        col: usize,
    ) -> Result<Type, TypeError> {
        // En M1 solo se puede llamar a una función por su nombre.
        let name = match &callee.kind {
            ExprKind::Ident(n) => n.clone(),
            _ => return Err(self.err(line, col, "solo se pueden llamar funciones por su nombre".into())),
        };

        // 'print' es un builtin que el checker conoce (DESIGN.md §7): acepta un
        // único argumento de un tipo imprimible y devuelve unit.
        if name == "print" {
            if args.len() != 1 {
                return Err(self.err(line, col, format!("print espera 1 argumento, se le pasaron {}", args.len())));
            }
            let at = self.check_expr(&args[0])?;
            if !is_printable(&at) {
                return Err(self.err(args[0].line, args[0].col, format!("print no puede imprimir un {}", at)));
            }
            return Ok(Type::Unit);
        }

        // 'len(a) -> int': longitud de un arreglo.
        if name == "len" {
            if args.len() != 1 {
                return Err(self.err(line, col, format!("len espera 1 argumento, se le pasaron {}", args.len())));
            }
            let at = self.check_expr(&args[0])?;
            if !matches!(at, Type::Array(_)) {
                return Err(self.err(args[0].line, args[0].col, format!("len espera un arreglo, no {}", at)));
            }
            return Ok(Type::Int);
        }

        // 'push(a, x) -> unit': agrega x al final del arreglo a (lo muta).
        if name == "push" {
            if args.len() != 2 {
                return Err(self.err(line, col, format!("push espera 2 argumentos (arreglo, valor), se le pasaron {}", args.len())));
            }
            let elem = match self.check_expr(&args[0])? {
                Type::Array(e) => *e,
                other => return Err(self.err(args[0].line, args[0].col, format!("push espera un arreglo como primer argumento, no {}", other))),
            };
            let vt = self.check_expr(&args[1])?;
            if vt != elem {
                return Err(self.err(args[1].line, args[1].col, format!("push: el arreglo es de {} pero se empuja {}", elem, vt)));
            }
            return Ok(Type::Unit);
        }

        // Función definida por el usuario: comprobamos aridad y tipos.
        // Clonamos la firma para soltar el préstamo de `self` antes de verificar
        // los argumentos (que vuelven a tomar `self` prestado mutable).
        let (param_types, ret) = match self.functions.get(&name) {
            Some(sig) => (sig.params.clone(), sig.ret.clone()),
            None => return Err(self.err(line, col, format!("función '{}' no declarada", name))),
        };

        if args.len() != param_types.len() {
            return Err(self.err(line, col, format!(
                "'{}' espera {} argumento(s), se le pasaron {}",
                name, param_types.len(), args.len()
            )));
        }
        for (i, (arg, expected)) in args.iter().zip(param_types.iter()).enumerate() {
            let at = self.check_expr(arg)?;
            if at != *expected {
                return Err(self.err(arg.line, arg.col, format!(
                    "argumento {} de '{}': se esperaba {}, se pasó {}",
                    i + 1, name, expected, at
                )));
            }
        }
        Ok(ret)
    }

    // ----- Manejo de ámbitos -----

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Declara una variable en el ámbito más interno (permite shadowing del exterior).
    fn declare(&mut self, name: &str, ty: Type, mutable: bool) {
        self.scopes
            .last_mut()
            .expect("siempre hay un ámbito activo al declarar")
            .insert(name.to_string(), VarInfo { ty, mutable });
    }

    /// Busca una variable de dentro hacia afuera.
    fn lookup(&self, name: &str) -> Option<&VarInfo> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn err(&self, line: usize, col: usize, msg: String) -> TypeError {
        TypeError { msg, line, col }
    }
}

// ----- Auxiliares libres -----

/// ¿Pueden compararse con == / != valores de este tipo? (Arreglos: estructural.)
fn is_comparable(t: &Type) -> bool {
    matches!(t, Type::Int | Type::Float | Type::Bool | Type::String | Type::Array(_))
}

/// ¿Puede `print` imprimir este tipo?
fn is_printable(t: &Type) -> bool {
    matches!(t, Type::Int | Type::Float | Type::Bool | Type::String | Type::Array(_))
}

fn bin_op_str(op: BinaryOp) -> &'static str {
    use BinaryOp::*;
    match op {
        Add => "+", Sub => "-", Mul => "*", Div => "/", Rem => "%",
        Eq => "==", Ne => "!=", Lt => "<", Le => "<=", Gt => ">", Ge => ">=",
        And => "&&", Or => "||",
    }
}

/// Análisis de divergencia: ¿todos los caminos de este bloque terminan en `return`?
/// Es una aproximación *conservadora* (sólida): si dice `true`, es seguro que el
/// bloque siempre retorna; si dice `false`, puede que sí o que no. Eso basta para
/// permitir omitir la expresión final cuando el cuerpo ya retorna por todas partes.
fn block_diverges(block: &Block) -> bool {
    block.statements.iter().any(stmt_diverges)
        || block.tail.as_ref().is_some_and(|t| expr_diverges(t))
}

fn stmt_diverges(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Return { .. } => true,
        StmtKind::Expr(e) => expr_diverges(e),
        _ => false,
    }
}

fn expr_diverges(expr: &Expr) -> bool {
    match &expr.kind {
        // Un if diverge solo si AMBAS ramas divergen (si falta el else, puede caer).
        ExprKind::If { then_branch, else_branch: Some(els), .. } => {
            block_diverges(then_branch) && expr_diverges(els)
        }
        ExprKind::Block(b) => block_diverges(b),
        _ => false,
    }
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// Lexea, parsea y verifica un fuente completo.
    fn check_src(src: &str) -> Result<(), TypeError> {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let prog = crate::parser::parse(tokens).expect("parse ok");
        check(&prog)
    }

    /// Atajo: ¿el mensaje de error contiene esta subcadena?
    fn err_contains(src: &str, needle: &str) {
        let e = check_src(src).expect_err("debería fallar la verificación");
        assert!(
            e.msg.contains(needle),
            "mensaje '{}' no contiene '{}'",
            e.msg,
            needle
        );
    }

    #[test]
    fn fib_es_valido() {
        let src = r#"
fn fib(n: int) -> int {
    if (n < 2) { n } else { fib(n - 1) + fib(n - 2) }
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
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn aritmetica_mezclada_falla() {
        err_contains("fn main() -> int { 1 + true }", "requiere ambos operandos");
        err_contains("fn main() { let x: float = 1 + 2.0; }", "requiere ambos operandos");
    }

    #[test]
    fn condicion_debe_ser_bool() {
        err_contains("fn main() { if (1) { } }", "condición del if debe ser bool");
        err_contains("fn main() { while (1) { } }", "condición del while debe ser bool");
    }

    #[test]
    fn ramas_del_if_mismo_tipo() {
        err_contains(
            "fn main() -> int { if (true) { 1 } else { true } }",
            "ramas del if tienen tipos distintos",
        );
    }

    #[test]
    fn if_sin_else_debe_ser_unit() {
        err_contains("fn main() { if (true) { 5 } }", "sin else tiene tipo unit");
    }

    #[test]
    fn asignar_a_let_falla_pero_a_var_ok() {
        err_contains(
            "fn main() { let x: int = 0; x = 1; }",
            "es inmutable",
        );
        assert!(check_src("fn main() { var x: int = 0; x = 1; }").is_ok());
    }

    #[test]
    fn variable_no_declarada() {
        err_contains("fn main() -> int { x }", "no declarado");
        err_contains("fn main() { y = 1; }", "no declarada");
    }

    #[test]
    fn tipo_de_declaracion_debe_coincidir() {
        err_contains("fn main() { let x: int = true; }", "se inicializa con bool");
    }

    #[test]
    fn retorno_incorrecto() {
        err_contains("fn f() -> int { true } fn main() {}", "produce bool");
        err_contains("fn g() -> int { return true; } fn main() {}", "se devuelve bool");
    }

    #[test]
    fn retorno_temprano_sin_valor_final_es_valido() {
        // Gracias al análisis de divergencia, esto es válido aunque no tenga
        // expresión final: todos los caminos retornan.
        let src = r#"
fn signo(x: int) -> int {
    if (x < 0) { return -1; } else { return 1; }
}
fn main() -> int { signo(3) }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn llamadas_validan_aridad_y_tipos() {
        err_contains(
            "fn add(a: int, b: int) -> int { a + b } fn main() -> int { add(1) }",
            "espera 2 argumento",
        );
        err_contains(
            "fn add(a: int, b: int) -> int { a + b } fn main() -> int { add(1, true) }",
            "se esperaba int, se pasó bool",
        );
        err_contains("fn main() -> int { desconocida() }", "no declarada");
    }

    #[test]
    fn print_builtin() {
        assert!(check_src("fn main() { print(42); print(\"hola\"); print(true); }").is_ok());
        err_contains("fn main() { print(); }", "espera 1 argumento");
        err_contains("fn main() { print(1, 2); }", "espera 1 argumento");
    }

    #[test]
    fn main_obligatoria_y_bien_formada() {
        err_contains("fn otra() -> int { 0 }", "falta la función de entrada 'main'");
        err_contains("fn main(x: int) -> int { x }", "no debe recibir parámetros");
        err_contains("fn main() -> bool { true }", "debe devolver int o unit");
    }

    #[test]
    fn shadowing_en_bloque_interno() {
        // Una variable interior puede tapar a una exterior con otro tipo.
        let src = r#"
fn main() -> int {
    let x: int = 1;
    { let x: bool = true; print(x); }
    x
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn funcion_no_declarada_dos_veces() {
        err_contains("fn f() {} fn f() {} fn main() {}", "declarada dos veces");
    }

    // ----- M3.1: arreglos -----

    #[test]
    fn arreglos_validos() {
        assert!(check_src("fn main() -> int { let a: [int] = [1, 2, 3]; a[0] }").is_ok());
        assert!(check_src("fn main() -> int { let a: [int] = []; push(a, 1); len(a) }").is_ok());
        assert!(check_src("fn main() { var a: [int] = [1]; a[0] = 9; }").is_ok());
        // Arreglos anidados.
        assert!(check_src("fn main() -> int { let m: [[int]] = [[1, 2], [3, 4]]; m[1][0] }").is_ok());
    }

    #[test]
    fn arreglos_errores_de_tipo() {
        err_contains("fn main() -> int { let a: [int] = [1, true]; a[0] }", "mismo tipo");
        err_contains("fn main() -> int { let a: [int] = [1]; a[true] }", "índice debe ser int");
        err_contains("fn main() -> int { let x: int = 5; x[0] }", "no es un arreglo");
        err_contains("fn main() { let x: int = []; }", "arreglo vacío");
        err_contains("fn main() -> int { let a: [int] = [1]; a[0] = true; a[0] }", "se le asigna bool");
        err_contains("fn main() -> int { len(5) }", "len espera un arreglo");
        err_contains("fn main() { let a: [int] = [1]; push(a, true); }", "se empuja bool");
    }
}
