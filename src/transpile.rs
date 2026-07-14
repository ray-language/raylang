//! **SPIKE / arco P2.b — transpile a Rust** (codegen nativo; jul 2026).
//!
//! Emite código Rust para un subconjunto creciente de raylang. El checker garantiza el tipado; aquí
//! solo se baja a Rust. Fases: **escalares** (`int`/`float`/`bool`, aritmética, `if`/`while`/`for`-rango,
//! recursión, `print`) → **strings** (`Rc<str>`, concat, `to_string`, `len`) → datos → control → …
//! Todo nodo fuera del subconjunto → `Err` claro (el transpilador es honesto sobre su alcance).
//!
//! **Modelo de valores** (como el intérprete): escalares *unboxed* (`i64`/`f64`/`bool`), tipos de heap
//! envueltos en `Rc` (strings inmutables → `Rc<str>`; arreglos/structs → `Rc<RefCell<…>>`, futuro). La
//! semántica de VALOR de raylang sobre la de MOVIMIENTO de Rust se resuelve **clonando al leer** los
//! valores de heap (para `Rc` es un bump de refcount, O(1)); los escalares son `Copy`. Un entorno de
//! tipos propio (params explícitos + inferencia mínima de los `let`) decide qué clonar.
//!
//! Semántica del spike: aritmética `int` con **wrapping** (operadores nativos; release sin
//! overflow-checks), NO checked como la VM. Fiel para programas sin desbordamiento.

use crate::ast::{
    BinaryOp, Block, Expr, ExprKind, ForIter, ForPat, Function, Program, Stmt, StmtKind, Type, UnaryOp,
};
use std::collections::HashMap;
use std::fmt::Write;

/// Firma de una función del usuario: (por ahora) su tipo de retorno.
struct FnSig {
    ret: Type,
}

/// Resuelve el `callee` de una llamada a `(nombre, receptor)`. UFCS `obj.m(args)` llega como callee
/// `Field{object, name}` (el checker no lo baja para builtins) ≡ `m(obj, args)`: el receptor va primero.
fn resolve_callee(callee: &Expr) -> Result<(&str, Option<&Expr>), String> {
    match &callee.kind {
        ExprKind::Ident(n) => Ok((n, None)),
        ExprKind::Field { object, name } => Ok((name, Some(object))),
        _ => Err("spike: llamada a expresión (no nombre ni método) no soportada".into()),
    }
}

/// ¿La callee es una llamada a `to_string` (libre o método `x.to_string()`, posiblemente manglada)?
fn is_to_string(callee: &Expr) -> bool {
    match resolve_callee(callee) {
        Ok((n, _)) => n.rsplit('#').next().unwrap_or(n).trim_start_matches("__") == "to_string",
        Err(_) => false,
    }
}

struct Transpiler {
    funcs: HashMap<String, FnSig>,
    /// Pila de ámbitos: nombre de variable → su tipo (para decidir clonado y para la inferencia de `let`).
    scopes: Vec<HashMap<String, Type>>,
}

/// Transpila un programa (ya chequeado) a Rust autocontenido, o un error si usa algo fuera del subconjunto.
pub fn transpile(prog: &Program) -> Result<String, String> {
    // Índice de firmas de funciones NO genéricas y NO sintéticas (para inferir tipos de llamada).
    let mut funcs = HashMap::new();
    for f in &prog.functions {
        if f.name.contains('#') || f.name.contains("::") || f.name.starts_with("__") || !f.type_params.is_empty() {
            continue;
        }
        funcs.insert(f.name.clone(), FnSig { ret: f.return_type.clone() });
    }
    let mut t = Transpiler { funcs, scopes: Vec::new() };

    let mut out = String::new();
    out.push_str("// Generado por el transpilador raylang→Rust (P2.b).\n");
    out.push_str("#![allow(unused_parens, unused_mut, dead_code, unused_variables)]\n");
    out.push_str("use std::rc::Rc;\n");
    // Preámbulo: helpers de runtime para operaciones de arreglo/string que no son 1:1 con Rust.
    out.push_str("fn __ray_split(s: &str, sep: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n");
    out.push_str("    Rc::new(std::cell::RefCell::new(s.split(sep).map(Rc::<str>::from).collect()))\n}\n");
    out.push_str("fn __ray_join(a: &Rc<std::cell::RefCell<Vec<Rc<str>>>>, sep: &str) -> Rc<str> {\n");
    out.push_str("    let v = a.borrow();\n");
    out.push_str("    let parts: Vec<&str> = v.iter().map(|s| &**s).collect();\n");
    out.push_str("    Rc::<str>::from(parts.join(sep))\n}\n\n");

    let mut main_ret_int = false;
    let mut main_seen = false;
    for f in &prog.functions {
        if f.name.contains('#') || f.name.contains("::") || f.name.starts_with("__") || !f.type_params.is_empty() {
            continue;
        }
        let rust_name = if f.name == "main" { "ray_main".to_string() } else { f.name.clone() };
        let mut fbuf = String::new();
        match t.emit_function(&mut fbuf, &rust_name, f) {
            Ok(()) => {
                out.push_str(&fbuf);
                out.push('\n');
                if f.name == "main" {
                    main_seen = true;
                    main_ret_int = matches!(f.return_type, Type::Int);
                }
            }
            Err(e) => {
                if f.name == "main" {
                    return Err(format!("spike: main fuera del subconjunto: {}", e));
                }
            }
        }
    }
    if !main_seen {
        return Err("spike: `main` no está en el subconjunto soportado".into());
    }

    out.push_str("fn main() {\n");
    if main_ret_int {
        out.push_str("    std::process::exit(ray_main() as i32);\n");
    } else {
        out.push_str("    ray_main();\n");
    }
    out.push_str("}\n");
    Ok(out)
}

impl Transpiler {
    fn emit_function(&mut self, out: &mut String, rust_name: &str, f: &Function) -> Result<(), String> {
        self.scopes.push(HashMap::new());
        let mut params = Vec::new();
        for p in &f.params {
            params.push(format!("mut {}: {}", p.name, rust_ty(&p.ty)?));
            self.declare(&p.name, p.ty.clone());
        }
        write!(out, "fn {}({}) -> {} ", rust_name, params.join(", "), rust_ty(&f.return_type)?).unwrap();
        self.emit_block(out, &f.body)?;
        out.push('\n');
        self.scopes.pop();
        Ok(())
    }

    fn declare(&mut self, name: &str, ty: Type) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), ty);
    }

    fn lookup(&self, name: &str) -> Option<&Type> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn emit_block(&mut self, out: &mut String, b: &Block) -> Result<(), String> {
        out.push_str("{\n");
        self.scopes.push(HashMap::new());
        for s in &b.statements {
            self.emit_stmt(out, s)?;
        }
        if let Some(tail) = &b.tail {
            out.push_str("    ");
            self.emit_expr(out, tail)?;
            out.push('\n');
        }
        self.scopes.pop();
        out.push('}');
        Ok(())
    }

    fn emit_stmt(&mut self, out: &mut String, s: &Stmt) -> Result<(), String> {
        out.push_str("    ");
        match &s.kind {
            StmtKind::Let { name, ty, value, mutable } => {
                // Tipo de la variable: la anotación si está, si no se infiere del inicializador.
                let vty = match ty {
                    Some(t) => t.clone(),
                    None => self.type_of(value)?,
                };
                out.push_str(if *mutable { "let mut " } else { "let " });
                out.push_str(name);
                out.push_str(" = ");
                self.emit_expr(out, value)?;
                out.push_str(";\n");
                self.declare(name, vty);
            }
            StmtKind::Assign { target, value } => {
                // El TARGET es un lvalue: NO se clona (a diferencia de una lectura). `x` → `x`;
                // `a[i]` → `a.borrow_mut()[i as usize]`. (Campos de struct: fase futura.)
                match &target.kind {
                    ExprKind::Ident(name) => out.push_str(name),
                    ExprKind::Index { array, index } => {
                        self.emit_expr(out, array)?;
                        out.push_str(".borrow_mut()[");
                        self.emit_expr(out, index)?;
                        out.push_str(" as usize]");
                    }
                    _ => return Err("spike: lvalue no soportado".into()),
                }
                out.push_str(" = ");
                self.emit_expr(out, value)?;
                out.push_str(";\n");
            }
            StmtKind::Return { value } => {
                out.push_str("return");
                if let Some(v) = value {
                    out.push(' ');
                    self.emit_expr(out, v)?;
                }
                out.push_str(";\n");
            }
            StmtKind::Expr(e) => {
                self.emit_expr(out, e)?;
                out.push_str(";\n");
            }
            StmtKind::For { pat, iter, body } => {
                let var = match pat {
                    ForPat::Single(n) => n.clone(),
                    ForPat::Tuple(_) => return Err("spike: for sobre tupla (Map) no soportado".into()),
                };
                match iter {
                    ForIter::Range { start, end } => {
                        write!(out, "for {} in ", var).unwrap();
                        self.emit_expr(out, start)?;
                        out.push_str("..");
                        self.emit_expr(out, end)?;
                        out.push(' ');
                        self.scopes.push(HashMap::new());
                        self.declare(&var, Type::Int);
                        self.emit_block(out, body)?;
                        self.scopes.pop();
                        out.push('\n');
                    }
                    // `for x in <arreglo>` → itera una copia del Vec (los elementos son Rc → bump barato)
                    // para NO retener el borrow durante el cuerpo (que podría mutar el arreglo).
                    // `for c in <string>` → `.chars()` (char por char).
                    ForIter::In(expr) => {
                        let ety = match self.type_of(expr)? {
                            Type::Array(t) => (*t).clone(),
                            Type::String => Type::Char,
                            other => return Err(format!("spike: for sobre {:?} no soportado", other)),
                        };
                        write!(out, "for {} in ", var).unwrap();
                        self.emit_expr(out, expr)?;
                        out.push_str(if matches!(ety, Type::Char) { ".chars()" } else { ".borrow().clone()" });
                        out.push(' ');
                        self.scopes.push(HashMap::new());
                        self.declare(&var, ety);
                        self.emit_block(out, body)?;
                        self.scopes.pop();
                        out.push('\n');
                    }
                    _ => return Err("spike: for sobre iterador (Iterator<T>) no soportado".into()),
                }
            }
            other => return Err(format!("spike: sentencia no soportada {:?}", other)),
        }
        Ok(())
    }

    fn emit_expr(&mut self, out: &mut String, e: &Expr) -> Result<(), String> {
        match &e.kind {
            ExprKind::Int(n) => write!(out, "{}i64", n).unwrap(),
            ExprKind::Float(x) => write!(out, "{:?}f64", x).unwrap(),
            ExprKind::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            ExprKind::Str(s) => write!(out, "Rc::<str>::from({:?})", s).unwrap(),
            ExprKind::Ident(name) => {
                // Clonar al leer los valores de heap (Rc → bump barato); los escalares son Copy.
                out.push_str(name);
                if let Some(ty) = self.lookup(name) {
                    if is_heap(ty) {
                        out.push_str(".clone()");
                    }
                }
            }
            ExprKind::Unary { op, expr } => {
                out.push('(');
                out.push_str(match op { UnaryOp::Neg => "-", UnaryOp::Not => "!", UnaryOp::BitNot => "!" });
                self.emit_expr(out, expr)?;
                out.push(')');
            }
            ExprKind::Binary { op, left, right } => {
                // `string + string` → concatenación. Se APLANA toda la cadena `a + b + c + …` en un solo
                // `format!` (una alocación, no una anidada por `+`), e inlinea `to_string(x)` como `{}`
                // sobre `x` (evita el Rc intermedio). Clave para que el nativo bata a la VM en strings.
                if matches!(op, BinaryOp::Add) && matches!(self.type_of(left)?, Type::String) {
                    let mut operands = Vec::new();
                    self.flatten_concat(e, &mut operands)?;
                    self.emit_concat(out, &operands)?;
                } else {
                    out.push('(');
                    self.emit_expr(out, left)?;
                    write!(out, " {} ", binop(*op)).unwrap();
                    self.emit_expr(out, right)?;
                    out.push(')');
                }
            }
            ExprKind::Call { callee, args } => self.emit_call(out, callee, args)?,
            ExprKind::If { cond, then_branch, else_branch } => {
                out.push_str("if ");
                self.emit_expr(out, cond)?;
                out.push(' ');
                self.emit_block(out, then_branch)?;
                if let Some(eb) = else_branch {
                    out.push_str(" else ");
                    self.emit_expr(out, eb)?;
                }
            }
            ExprKind::While { cond, body } => {
                out.push_str("while ");
                self.emit_expr(out, cond)?;
                out.push(' ');
                self.emit_block(out, body)?;
            }
            ExprKind::Block(b) => self.emit_block(out, b)?,
            // Literal de arreglo → Rc<RefCell<Vec>>. Vacío: Vec::new() (Rust infiere el elemento del uso).
            ExprKind::ArrayLit(elems) => {
                out.push_str("Rc::new(std::cell::RefCell::new(");
                if elems.is_empty() {
                    out.push_str("Vec::new()");
                } else {
                    out.push_str("vec![");
                    for (i, el) in elems.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_expr(out, el)?;
                    }
                    out.push(']');
                }
                out.push_str("))");
            }
            // Indexación de LECTURA: a[i] → a.borrow()[i as usize].clone() (clona el elemento).
            ExprKind::Index { array, index } => {
                self.emit_expr(out, array)?;
                out.push_str(".borrow()[");
                self.emit_expr(out, index)?;
                out.push_str(" as usize].clone()");
            }
            other => return Err(format!("spike: expresión no soportada {:?}", other)),
        }
        Ok(())
    }

    /// Aplana una cadena de concatenación de strings `a + b + c + …` en sus operandos (izq→der),
    /// descendiendo por los `+` cuyo operando izquierdo es string.
    fn flatten_concat<'a>(&self, e: &'a Expr, out: &mut Vec<&'a Expr>) -> Result<(), String> {
        if let ExprKind::Binary { op: BinaryOp::Add, left, right } = &e.kind {
            if matches!(self.type_of(left)?, Type::String) {
                self.flatten_concat(left, out)?;
                out.push(right);
                return Ok(());
            }
        }
        out.push(e);
        Ok(())
    }

    /// Emite una concatenación aplanada como UN solo `format!` → un `Rc<str>` (2 allocs en vez de ~2N).
    /// Un literal de string se incrusta en la plantilla; `to_string(x)` se inlinea como `{}` sobre `x`
    /// (sin Rc intermedio); el resto va como `{}` con el operando emitido.
    fn emit_concat(&mut self, out: &mut String, operands: &[&Expr]) -> Result<(), String> {
        let mut fmt = String::new();
        let mut args: Vec<&Expr> = Vec::new();
        for op in operands {
            match &op.kind {
                // texto literal: escapado completo para un literal de plantilla `format!` de Rust
                // (`{`/`}` se duplican; `"`, `\`, saltos de línea… se escapan como en un string de Rust).
                ExprKind::Str(s) => push_fmt_literal(&mut fmt, s),
                // to_string(x) / x.to_string() → inlina x como `{}` (su Display), sin Rc intermedio.
                ExprKind::Call { callee, args: cargs } if is_to_string(callee) => {
                    fmt.push_str("{}");
                    let (_, recv) = resolve_callee(callee)?;
                    args.push(recv.unwrap_or(&cargs[0]));
                }
                _ => {
                    fmt.push_str("{}");
                    args.push(op);
                }
            }
        }
        out.push_str("Rc::<str>::from(format!(\"");
        out.push_str(&fmt);
        out.push('"');
        for a in args {
            out.push_str(", ");
            self.emit_expr(out, a)?;
        }
        out.push_str("))");
        Ok(())
    }

    fn emit_call(&mut self, out: &mut String, callee: &Expr, args: &[Expr]) -> Result<(), String> {
        let (name, recv) = resolve_callee(callee)?;
        // Argumentos efectivos: el receptor de UFCS (si lo hay) va primero.
        let eff: Vec<&Expr> = recv.into_iter().chain(args.iter()).collect();
        // Métodos de la stdlib manglados por el checker (`string#len`, `Len` trait…): el método real es
        // lo que va tras el último `#`. Los nombres de usuario no llevan `#` (ilegal) → quedan intactos.
        let method = name.rsplit('#').next().unwrap_or(name).trim_start_matches("__");
        match method {
            "print" => {
                out.push_str("println!(\"{}\", ");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // to_string(x) → Rc<str> (int/float/bool/char/string; usa el Display de Rust).
            "to_string" => {
                out.push_str("Rc::<str>::from(format!(\"{}\", ");
                self.emit_expr(out, eff[0])?;
                out.push_str("))");
            }
            // len(x) → i64. String: nº de octetos; arreglo: nº de elementos (vía borrow()).
            "len" => {
                out.push('(');
                self.emit_expr(out, eff[0])?;
                match self.type_of(eff[0])? {
                    Type::Array(_) => out.push_str(".borrow().len() as i64)"),
                    _ => out.push_str(".len() as i64)"),
                }
            }
            // push(a, v) → a.borrow_mut().push(v) (muta en el sitio, devuelve unit).
            "push" => {
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow_mut().push(");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // split(s, sep) → [string]; join(a, sep) → string (helpers del preámbulo generado).
            "split" => {
                out.push_str("__ray_split(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "join" => {
                out.push_str("__ray_join(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            _ => {
                if !self.funcs.contains_key(name) {
                    return Err(format!("spike: builtin/función '{}' no soportada", name));
                }
                out.push_str(name);
                out.push('(');
                for (i, a) in eff.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    self.emit_expr(out, a)?;
                }
                out.push(')');
            }
        }
        Ok(())
    }

    /// Inferencia MÍNIMA del tipo de una expresión del subconjunto — solo lo justo para clasificar
    /// heap-vs-escalar y decidir la concatenación de strings. No sustituye al checker (que ya validó).
    fn type_of(&self, e: &Expr) -> Result<Type, String> {
        Ok(match &e.kind {
            ExprKind::Int(_) => Type::Int,
            ExprKind::Float(_) => Type::Float,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Str(_) => Type::String,
            ExprKind::Char(_) => Type::Char,
            ExprKind::Ident(n) => self
                .lookup(n)
                .cloned()
                .ok_or_else(|| format!("spike: variable '{}' sin tipo conocido", n))?,
            ExprKind::Unary { op, expr } => match op {
                UnaryOp::Not => Type::Bool,
                UnaryOp::BitNot => Type::Int,
                UnaryOp::Neg => self.type_of(expr)?,
            },
            ExprKind::Binary { op, left, .. } => match op {
                BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
                | BinaryOp::And | BinaryOp::Or => Type::Bool,
                _ => self.type_of(left)?, // aritmética/bitwise/concat: el tipo del operando izquierdo
            },
            ExprKind::Call { callee, .. } => {
                let (n, _) = resolve_callee(callee)?;
                let method = n.rsplit('#').next().unwrap_or(n).trim_start_matches("__");
                match method {
                    "to_string" => Type::String,
                    "len" => Type::Int,
                    "print" | "push" => Type::Unit,
                    "split" => Type::Array(Box::new(Type::String)),
                    "join" => Type::String,
                    _ => self
                        .funcs
                        .get(n)
                        .map(|s| s.ret.clone())
                        .ok_or_else(|| format!("spike: no sé el tipo de retorno de '{}'", n))?,
                }
            }
            ExprKind::If { then_branch, .. } => match &then_branch.tail {
                Some(t) => self.type_of(t)?,
                None => Type::Unit,
            },
            ExprKind::Block(b) => match &b.tail {
                Some(t) => self.type_of(t)?,
                None => Type::Unit,
            },
            ExprKind::While { .. } => Type::Unit,
            ExprKind::ArrayLit(elems) => {
                let elem = match elems.first() {
                    Some(e) => self.type_of(e)?,
                    None => return Err("spike: literal de arreglo vacío sin anotación".into()),
                };
                Type::Array(Box::new(elem))
            }
            ExprKind::Index { array, .. } => match self.type_of(array)? {
                Type::Array(t) => *t,
                Type::String => Type::Char, // s[i] → char
                other => return Err(format!("spike: indexar {:?} no soportado", other)),
            },
            other => return Err(format!("spike: no sé inferir el tipo de {:?}", other)),
        })
    }
}

/// Un tipo de raylang → su equivalente Rust (subconjunto actual: escalares + string).
fn rust_ty(t: &Type) -> Result<String, String> {
    Ok(match t {
        Type::Int => "i64",
        Type::Float => "f64",
        Type::Bool => "bool",
        Type::Char => "char",
        Type::Unit => "()",
        Type::String => "Rc<str>",
        // Arreglo: semántica de referencia + mutación → Rc<RefCell<Vec<…>>> (como el intérprete).
        Type::Array(t) => return Ok(format!("Rc<std::cell::RefCell<Vec<{}>>>", rust_ty(t)?)),
        other => return Err(format!("spike: tipo no soportado {:?}", other)),
    }
    .to_string())
}

/// ¿Es un tipo de heap (semántica de referencia / no `Copy`) → hay que clonar al leer?
fn is_heap(t: &Type) -> bool {
    matches!(
        t,
        Type::String | Type::Bytes | Type::Array(_) | Type::Tuple(_) | Type::Map(_, _) | Type::Struct(_, _) | Type::Enum(_, _)
    )
}

/// Empuja `s` a un literal de plantilla `format!` de Rust, escapando lo necesario: `{`/`}` se duplican
/// (son metacaracteres de format!), y `"`/`\`/saltos se escapan como en cualquier string de Rust.
fn push_fmt_literal(fmt: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '{' => fmt.push_str("{{"),
            '}' => fmt.push_str("}}"),
            '"' => fmt.push_str("\\\""),
            '\\' => fmt.push_str("\\\\"),
            '\n' => fmt.push_str("\\n"),
            '\t' => fmt.push_str("\\t"),
            '\r' => fmt.push_str("\\r"),
            _ => fmt.push(c),
        }
    }
}

fn binop(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
    }
}

#[cfg(test)]
mod tests {
    use super::transpile;

    fn transpile_src(src: &str) -> String {
        let tokens = crate::lexer::lex(src).expect("lex");
        let mut prog = crate::parser::parse(tokens).expect("parse");
        crate::checker::check(&mut prog).expect("check");
        transpile(&prog).expect("transpile")
    }

    #[test]
    fn transpila_fib_recursivo() {
        let rust = transpile_src(
            "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }\n\
             fn main() { print(fib(10)); }",
        );
        assert!(rust.contains("fn fib(mut n: i64) -> i64"), "{}", rust);
        assert!(rust.contains("fib((n - 1i64))"), "{}", rust);
        assert!(rust.contains("println!(\"{}\", fib(10i64))"), "{}", rust);
    }

    #[test]
    fn transpila_bucle_for_rango() {
        let rust = transpile_src(
            "fn main() { var acc: int = 0; for i in 0..100 { acc = acc + i; } print(acc); }",
        );
        assert!(rust.contains("for i in 0i64..100i64"), "{}", rust);
        assert!(rust.contains("let mut acc = 0i64"), "{}", rust);
    }

    #[test]
    fn transpila_strings_concat_y_clon() {
        let rust = transpile_src(
            "fn greet(name: string) -> string { \"hi \" + name }\n\
             fn main() -> int { let g = greet(\"bob\"); print(g); g.len() }",
        );
        // string → Rc<str>; concat via format!; el `g` heap se clona al leer.
        assert!(rust.contains("fn greet(mut name: Rc<str>) -> Rc<str>"), "{}", rust);
        assert!(rust.contains("Rc::<str>::from(format!"), "{}", rust);
        assert!(rust.contains("g.clone()"), "{}", rust);
    }

    #[test]
    fn transpila_arreglos_split_join() {
        let rust = transpile_src(
            "fn main() -> int { var xs: [string] = []; xs.push(\"a\"); \
             let parts = \"a,b,c\".split(\",\"); parts.join(\"-\").len() }",
        );
        assert!(rust.contains("Rc<std::cell::RefCell<Vec<Rc<str>>>>"), "{}", rust);
        assert!(rust.contains(".borrow_mut().push("), "{}", rust);
        assert!(rust.contains("__ray_split(&"), "{}", rust);
        assert!(rust.contains("__ray_join(&"), "{}", rust);
    }

    #[test]
    fn rechaza_fuera_del_subconjunto() {
        // un `main` con struct (aún fuera del subconjunto) → sin `main` transpilable.
        let tokens = crate::lexer::lex("struct P { x: int }\nfn main() { let p = P { x: 1 }; print(p.x); }")
            .unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        assert!(super::transpile(&prog).is_err());
    }
}
