//! M396 (rayplay #168): parámetros de arreglo de **solo lectura**.
//!
//! Un `const` arreglo de escalares se lee de una copia compartida por fibra (M391: `C[i]`, `C.len()`,
//! `C.contains(x)`), pero PASARLO a una función lo reconstruía entero en cada llamada (180 enteros ×
//! 100 000 llamadas: 120 ms frente a 6 con una copia local), porque la callee podría mutarlo. Este
//! análisis decide, por función y posición, si un parámetro `[T]` puede llegar a mutarse: si no, la
//! llamada puede pasar la copia compartida (`SharedConst`) sin riesgo de alias.
//!
//! Conservador por construcción: un parámetro es de solo lectura si TODAS sus apariciones son lecturas
//! reconocidas — base de un índice, iterable de un `for`, operando de `==`/`!=`/`+`, argumento de un
//! builtin que no muta (`len`, `contains`, `reverse`, `print`…), o argumento de otra función de usuario
//! en una posición que también es de solo lectura (punto fijo sobre el programa fusionado, prelude
//! incluido: `pop` usa `__pop` y queda marcado como mutador; `map`/`filter`/`fold` quedan limpios).
//! Cualquier otra aparición (`let y = p`, devolverlo, meterlo en un literal o un struct, pasarlo a un
//! valor-función, a un `extern`, a `spawn`/`send`, `p.push`, `p[i] = v`) lo marca como mutable, igual
//! que una redeclaración del nombre dentro del cuerpo (sombras: no se razona sobre ámbitos).
use std::collections::{HashMap, HashSet};

use crate::ast::*;

/// Builtins que reciben un arreglo y no lo mutan ni lo retienen.
const READ_ONLY_BUILTINS: &[&str] = &[
    // Primitivos a los que bajan los métodos del prelude (`Len`, `Contains`, `Reverse` → copia nueva).
    "__len", "__contains", "__reverse", "__position", "__sort_prim",
    // Builtins libres que leen o copian.
    "len", "contains", "reverse", "print", "eprint", "to_string", "assert_eq", "join", "bytes_of", "bytes_concat", "sort", "sort_by", "sort_by_key",
];

/// `(nombre de función, índice de parámetro)` de cada parámetro `[T]` que ninguna ruta puede mutar.
pub fn readonly_array_params(program: &Program) -> HashSet<(String, usize)> {
    struct Info {
        direct: bool,
        deps: Vec<(String, usize)>,
    }
    let fn_names: HashSet<&str> = program.functions.iter().map(|f| f.name.as_str()).collect();
    let mut infos: HashMap<(String, usize), Info> = HashMap::new();
    for f in &program.functions {
        for (i, p) in f.params.iter().enumerate() {
            if !matches!(p.ty, Type::Array(_)) {
                continue;
            }
            let mut a = Analyzer { name: &p.name, fn_names: &fn_names, direct: false, deps: Vec::new() };
            if shadows_block(&f.body, &p.name) {
                a.direct = true;
            } else {
                a.block(&f.body);
            }
            infos.insert((f.name.clone(), i), Info { direct: a.direct, deps: a.deps });
        }
    }
    // Punto fijo (mínimo): un parámetro PUEDE mutarse si lo muta directamente o lo pasa a uno que puede.
    let mut may: HashSet<(String, usize)> = HashSet::new();
    loop {
        let mut changed = false;
        for (key, info) in &infos {
            if may.contains(key) {
                continue;
            }
            if info.direct || info.deps.iter().any(|d| may.contains(d) || !infos.contains_key(d)) {
                may.insert(key.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    infos.into_keys().filter(|k| !may.contains(k)).collect()
}

struct Analyzer<'a> {
    name: &'a str,
    fn_names: &'a HashSet<&'a str>,
    direct: bool,
    deps: Vec<(String, usize)>,
}

impl Analyzer<'_> {
    fn is_param(&self, e: &Expr) -> bool {
        matches!(&e.kind, ExprKind::Ident(n) if n == self.name)
    }

    /// Una aparición en posición de VALOR (se copia, se retiene o se muta): mutable.
    fn value(&mut self, e: &Expr) {
        if self.is_param(e) {
            self.direct = true;
        } else {
            self.expr(e);
        }
    }

    fn block(&mut self, b: &Block) {
        for s in &b.statements {
            self.stmt(s);
        }
        if let Some(t) = &b.tail {
            self.value(t);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => self.value(value),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => {
                        self.value(start);
                        self.value(end);
                    }
                    // Iterar el parámetro es una lectura.
                    ForIter::In(e) | ForIter::Iter { expr: e, .. } => {
                        if !self.is_param(e) {
                            self.expr(e);
                        }
                    }
                }
                self.block(body);
            }
            StmtKind::Assign { target, value } => {
                match &target.kind {
                    // `p[i] = v`: mutación.
                    ExprKind::Index { array, index } => {
                        if self.is_param(array) {
                            self.direct = true;
                        } else {
                            self.expr(array);
                        }
                        self.value(index);
                    }
                    _ => self.value(target),
                }
                self.value(value);
            }
            StmtKind::Return { value: Some(v) } => self.value(v),
            StmtKind::Return { value: None } | StmtKind::Break { .. } | StmtKind::Continue { .. } => {}
            StmtKind::Expr(e) => self.expr(e),
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Int(..) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) | ExprKind::Char(_) | ExprKind::Bytes(_) => {}
            // El nombre a secas fuera de todo contexto reconocido: valor (otro nombre no interesa).
            ExprKind::Ident(_) => {
                if self.is_param(e) {
                    self.direct = true;
                }
            }
            ExprKind::Unary { expr, .. } | ExprKind::Try(expr) | ExprKind::Cast { expr, .. } => self.value(expr),
            // `==`/`!=` comparan y `+` construye un arreglo NUEVO: lecturas.
            ExprKind::Binary { op, left, right } => {
                let read = matches!(op, BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Add);
                for side in [left, right] {
                    if self.is_param(side) {
                        if !read {
                            self.direct = true;
                        }
                    } else {
                        self.expr(side);
                    }
                }
            }
            ExprKind::Index { array, index } => {
                if !self.is_param(array) {
                    self.expr(array);
                }
                self.value(index);
            }
            ExprKind::Call { callee, args } => {
                if let ExprKind::Ident(fname) = &callee.kind {
                    let name = fname.strip_suffix("#prelude").unwrap_or(fname);
                    if crate::builtins::lookup(name).is_some() || name == "__concat" {
                        let read = READ_ONLY_BUILTINS.contains(&name);
                        for a in args {
                            if self.is_param(a) {
                                if !read {
                                    self.direct = true;
                                }
                            } else {
                                self.expr(a);
                            }
                        }
                        return;
                    }
                    if self.fn_names.contains(name) {
                        for (i, a) in args.iter().enumerate() {
                            if self.is_param(a) {
                                self.deps.push((name.to_string(), i));
                            } else {
                                self.expr(a);
                            }
                        }
                        return;
                    }
                }
                // Valor-función, `extern`, función asociada, despacho dinámico…: no se sabe qué hace.
                self.value(callee);
                for a in args {
                    self.value(a);
                }
            }
            ExprKind::ArrayLit(es) | ExprKind::TupleLit(es) => es.iter().for_each(|x| self.value(x)),
            ExprKind::MapLit(kvs) => kvs.iter().for_each(|(k, v)| {
                self.value(k);
                self.value(v);
            }),
            ExprKind::StructLit { fields, .. } => fields.iter().for_each(|(_, v)| self.value(v)),
            ExprKind::Field { object, .. } => self.value(object),
            ExprKind::EnumLit { args, .. } => args.iter().for_each(|a| self.value(a)),
            ExprKind::Func(fe) => self.block(&fe.body),
            ExprKind::Match { scrutinee, arms } => {
                self.value(scrutinee);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.value(g);
                    }
                    self.value(&arm.body);
                }
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.value(cond);
                self.block(then_branch);
                if let Some(e) = else_branch {
                    self.value(e);
                }
            }
            ExprKind::While { cond, body, .. } => {
                self.value(cond);
                self.block(body);
            }
            ExprKind::Block(b) => self.block(b),
        }
    }
}

// --- Sombras: ¿se redeclara `name` en algún punto del cuerpo? (conservador, sin ámbitos) ---

fn shadows_block(b: &Block, name: &str) -> bool {
    b.statements.iter().any(|s| shadows_stmt(s, name)) || b.tail.as_ref().is_some_and(|t| shadows_expr(t, name))
}

fn shadows_stmt(s: &Stmt, name: &str) -> bool {
    match &s.kind {
        StmtKind::Let { name: n, value, .. } => n == name || shadows_expr(value, name),
        StmtKind::LetTuple { names, value, .. } => names.iter().flatten().any(|n| n == name) || shadows_expr(value, name),
        StmtKind::For { pat, iter, body, .. } => {
            let in_pat = match pat {
                ForPat::Single(n) => n == name,
                ForPat::Tuple(ns) => ns.iter().flatten().any(|n| n == name),
            };
            let in_iter = match iter {
                ForIter::Range { start, end } => shadows_expr(start, name) || shadows_expr(end, name),
                ForIter::In(e) | ForIter::Iter { expr: e, .. } => shadows_expr(e, name),
            };
            in_pat || in_iter || shadows_block(body, name)
        }
        StmtKind::Assign { target, value } => shadows_expr(target, name) || shadows_expr(value, name),
        StmtKind::Return { value } => value.as_ref().is_some_and(|v| shadows_expr(v, name)),
        StmtKind::Break { .. } | StmtKind::Continue { .. } => false,
        StmtKind::Expr(e) => shadows_expr(e, name),
    }
}

fn shadows_pattern(p: &Pattern, name: &str) -> bool {
    match &p.kind {
        PatternKind::Wildcard | PatternKind::Literal(_) => false,
        PatternKind::Binding(n) => n == name,
        PatternKind::Variant { subpatterns, .. } | PatternKind::Tuple(subpatterns) => subpatterns.iter().any(|q| shadows_pattern(q, name)),
        PatternKind::Struct { fields, .. } => fields.iter().any(|(_, q)| shadows_pattern(q, name)),
    }
}

fn shadows_expr(e: &Expr, name: &str) -> bool {
    match &e.kind {
        ExprKind::Int(..) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) | ExprKind::Char(_) | ExprKind::Bytes(_) | ExprKind::Ident(_) => false,
        ExprKind::Unary { expr, .. } | ExprKind::Try(expr) | ExprKind::Cast { expr, .. } => shadows_expr(expr, name),
        ExprKind::Binary { left, right, .. } => shadows_expr(left, name) || shadows_expr(right, name),
        ExprKind::Index { array, index } => shadows_expr(array, name) || shadows_expr(index, name),
        ExprKind::Call { callee, args } => shadows_expr(callee, name) || args.iter().any(|a| shadows_expr(a, name)),
        ExprKind::ArrayLit(es) | ExprKind::TupleLit(es) => es.iter().any(|x| shadows_expr(x, name)),
        ExprKind::MapLit(kvs) => kvs.iter().any(|(k, v)| shadows_expr(k, name) || shadows_expr(v, name)),
        ExprKind::StructLit { fields, .. } => fields.iter().any(|(_, v)| shadows_expr(v, name)),
        ExprKind::Field { object, .. } => shadows_expr(object, name),
        ExprKind::EnumLit { args, .. } => args.iter().any(|a| shadows_expr(a, name)),
        ExprKind::Func(fe) => fe.params.iter().any(|p| p.name == name) || shadows_block(&fe.body, name),
        ExprKind::Match { scrutinee, arms } => {
            shadows_expr(scrutinee, name)
                || arms.iter().any(|a| shadows_pattern(&a.pattern, name) || a.guard.as_ref().is_some_and(|g| shadows_expr(g, name)) || shadows_expr(&a.body, name))
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            shadows_expr(cond, name) || shadows_block(then_branch, name) || else_branch.as_ref().is_some_and(|e| shadows_expr(e, name))
        }
        ExprKind::While { cond, body, .. } => shadows_expr(cond, name) || shadows_block(body, name),
        ExprKind::Block(b) => shadows_block(b, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze(src: &str) -> HashSet<(String, usize)> {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        readonly_array_params(&prog)
    }

    fn has(set: &HashSet<(String, usize)>, f: &str, i: usize) -> bool {
        set.contains(&(f.to_string(), i))
    }

    #[test]
    fn reads_are_readonly_and_mutations_or_aliases_are_not() {
        let set = analyze(
            "fn first(t: [int], i: int) -> int { t[i] }\n\
             fn total(t: [int]) -> int { var s = 0; for x in t { s = s + x; } s + t.len() }\n\
             fn grow(t: [int]) -> int { t.push(1); t.len() }\n\
             fn set0(t: [int]) { t[0] = 9; }\n\
             fn keep(t: [int]) -> [int] { t }\n\
             fn alias(t: [int]) -> int { let u = t; u.len() }\n\
             fn via(t: [int]) -> int { first(t, 0) }\n\
             fn via_grow(t: [int]) -> int { grow(t) }\n\
             fn shadow(t: [int]) -> int { let t = [1]; t.len() }\n\
             fn popped(t: [int]) -> int { match (t.pop()) { Option.Some(v) => v, Option.None => 0 } }\n\
             fn eq(t: [int], u: [int]) -> bool { t == u }\n\
             fn main() -> int { let xs = [1, 2]; first(xs, 0) + total(xs) + grow(xs) + via(xs) + via_grow(xs) + shadow(xs) + popped(xs) + alias(xs) + keep(xs).len() + (if (eq(xs, xs)) { 1 } else { 0 }) }",
        );
        assert!(has(&set, "first", 0) && has(&set, "total", 0) && has(&set, "via", 0), "{set:?}");
        assert!(has(&set, "eq", 0) && has(&set, "eq", 1), "{set:?}");
        for (f, why) in [("grow", "push"), ("set0", "índice asignado"), ("keep", "devuelto"), ("alias", "let"), ("via_grow", "pasa a un mutador"), ("shadow", "sombra"), ("popped", "pop")] {
            assert!(!has(&set, f, 0), "{f} no debe ser de solo lectura ({why}): {set:?}");
        }
        // Prelude: `map` sobre un arreglo lee; `pop` muta.
        assert!(has(&set, "map", 0), "{set:?}");
        assert!(!has(&set, "pop", 0), "{set:?}");
    }
}
