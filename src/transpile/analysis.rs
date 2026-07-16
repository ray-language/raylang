//! Análisis del AST para el transpilador (extraído de transpile.rs): los *walkers* que calculan qué
//! variables `var` capturadas-y-mutadas por una closure van en una **celda** `Rc<RefCell>` (B1).
//! Funciones libres, sin estado del `Transpiler`. Refactor puro; el comportamiento no cambia.

use crate::ast::{Block, Expr, ExprKind, ForIter, StmtKind};

/// Recoge TODOS los nombres de identificador que aparecen en `e` (descendiendo también a los cuerpos
/// de closures) → lo que una closure "referencia" (candidatos a captura).
pub(super) fn idents_of_expr(e: &Expr, out: &mut std::collections::HashSet<String>) {
    match &e.kind {
        ExprKind::Ident(n) => { out.insert(n.clone()); }
        ExprKind::Unary { expr, .. } => idents_of_expr(expr, out),
        ExprKind::Binary { left, right, .. } => { idents_of_expr(left, out); idents_of_expr(right, out); }
        ExprKind::Call { callee, args } => { idents_of_expr(callee, out); args.iter().for_each(|a| idents_of_expr(a, out)); }
        ExprKind::ArrayLit(es) | ExprKind::TupleLit(es) => es.iter().for_each(|x| idents_of_expr(x, out)),
        ExprKind::MapLit(ps) => ps.iter().for_each(|(k, v)| { idents_of_expr(k, out); idents_of_expr(v, out); }),
        ExprKind::Index { array, index } => { idents_of_expr(array, out); idents_of_expr(index, out); }
        ExprKind::Cast { expr, .. } => idents_of_expr(expr, out),
        ExprKind::StructLit { fields, .. } => fields.iter().for_each(|(_, v)| idents_of_expr(v, out)),
        ExprKind::Field { object, .. } => idents_of_expr(object, out),
        ExprKind::EnumLit { args, .. } => args.iter().for_each(|a| idents_of_expr(a, out)),
        ExprKind::Func(f) => idents_of_block(&f.body, out),
        ExprKind::Match { scrutinee, arms } => {
            idents_of_expr(scrutinee, out);
            arms.iter().for_each(|a| idents_of_expr(&a.body, out));
        }
        ExprKind::Try(inner) => idents_of_expr(inner, out),
        ExprKind::If { cond, then_branch, else_branch } => {
            idents_of_expr(cond, out);
            idents_of_block(then_branch, out);
            if let Some(eb) = else_branch { idents_of_expr(eb, out); }
        }
        ExprKind::While { cond, body } => { idents_of_expr(cond, out); idents_of_block(body, out); }
        ExprKind::Block(b) => idents_of_block(b, out),
        _ => {}
    }
}

pub(super) fn idents_of_block(b: &Block, out: &mut std::collections::HashSet<String>) {
    for s in &b.statements {
        match &s.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => idents_of_expr(value, out),
            StmtKind::Assign { target, value } => { idents_of_expr(target, out); idents_of_expr(value, out); }
            StmtKind::Return { value } => { if let Some(e) = value { idents_of_expr(e, out); } }
            StmtKind::Expr(e) => idents_of_expr(e, out),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { idents_of_expr(start, out); idents_of_expr(end, out); }
                    ForIter::In(e) => idents_of_expr(e, out),
                    ForIter::Iter { expr, .. } => idents_of_expr(expr, out),
                }
                idents_of_block(body, out);
            }
        }
    }
    if let Some(t) = &b.tail { idents_of_expr(t, out); }
}

/// Recoge en `out` los idents referenciados dentro de ALGUNA closure de `e` (sin contar los usos fuera
/// de closures). Para cada `Func` encontrado, todos los idents de su cuerpo son "capturados".
pub(super) fn captured_idents_expr(e: &Expr, out: &mut std::collections::HashSet<String>) {
    match &e.kind {
        ExprKind::Func(f) => idents_of_block(&f.body, out),
        ExprKind::Unary { expr, .. } => captured_idents_expr(expr, out),
        ExprKind::Binary { left, right, .. } => { captured_idents_expr(left, out); captured_idents_expr(right, out); }
        ExprKind::Call { callee, args } => { captured_idents_expr(callee, out); args.iter().for_each(|a| captured_idents_expr(a, out)); }
        ExprKind::ArrayLit(es) | ExprKind::TupleLit(es) => es.iter().for_each(|x| captured_idents_expr(x, out)),
        ExprKind::MapLit(ps) => ps.iter().for_each(|(k, v)| { captured_idents_expr(k, out); captured_idents_expr(v, out); }),
        ExprKind::Index { array, index } => { captured_idents_expr(array, out); captured_idents_expr(index, out); }
        ExprKind::Cast { expr, .. } => captured_idents_expr(expr, out),
        ExprKind::StructLit { fields, .. } => fields.iter().for_each(|(_, v)| captured_idents_expr(v, out)),
        ExprKind::Field { object, .. } => captured_idents_expr(object, out),
        ExprKind::EnumLit { args, .. } => args.iter().for_each(|a| captured_idents_expr(a, out)),
        ExprKind::Match { scrutinee, arms } => {
            captured_idents_expr(scrutinee, out);
            arms.iter().for_each(|a| captured_idents_expr(&a.body, out));
        }
        ExprKind::Try(inner) => captured_idents_expr(inner, out),
        ExprKind::If { cond, then_branch, else_branch } => {
            captured_idents_expr(cond, out);
            captured_idents_block(then_branch, out);
            if let Some(eb) = else_branch { captured_idents_expr(eb, out); }
        }
        ExprKind::While { cond, body } => { captured_idents_expr(cond, out); captured_idents_block(body, out); }
        ExprKind::Block(b) => captured_idents_block(b, out),
        _ => {}
    }
}

pub(super) fn captured_idents_block(b: &Block, out: &mut std::collections::HashSet<String>) {
    for s in &b.statements {
        match &s.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => captured_idents_expr(value, out),
            StmtKind::Assign { target, value } => { captured_idents_expr(target, out); captured_idents_expr(value, out); }
            StmtKind::Return { value } => { if let Some(e) = value { captured_idents_expr(e, out); } }
            StmtKind::Expr(e) => captured_idents_expr(e, out),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { captured_idents_expr(start, out); captured_idents_expr(end, out); }
                    ForIter::In(e) => captured_idents_expr(e, out),
                    ForIter::Iter { expr, .. } => captured_idents_expr(expr, out),
                }
                captured_idents_block(body, out);
            }
        }
    }
    if let Some(t) = &b.tail { captured_idents_expr(t, out); }
}

/// Nombres declarados como `var` (mutable) en `body`, descendiendo por los bloques de control (if/while/
/// for/match/block) pero NO por los cuerpos de closures (ámbitos propios).
pub(super) fn mut_var_decls_block(b: &Block, out: &mut std::collections::HashSet<String>) {
    for s in &b.statements {
        match &s.kind {
            StmtKind::Let { name, mutable: true, value, .. } => {
                out.insert(name.clone());
                mut_var_decls_expr(value, out);
            }
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => mut_var_decls_expr(value, out),
            StmtKind::Assign { value, .. } => mut_var_decls_expr(value, out),
            StmtKind::Return { value } => { if let Some(e) = value { mut_var_decls_expr(e, out); } }
            StmtKind::Expr(e) => mut_var_decls_expr(e, out),
            StmtKind::For { body, .. } => mut_var_decls_block(body, out),
        }
    }
    if let Some(t) = &b.tail { mut_var_decls_expr(t, out); }
}

/// Desciende por los bloques de control de `e` buscando `var` declaradas (sin entrar en closures).
pub(super) fn mut_var_decls_expr(e: &Expr, out: &mut std::collections::HashSet<String>) {
    match &e.kind {
        ExprKind::If { then_branch, else_branch, .. } => {
            mut_var_decls_block(then_branch, out);
            if let Some(eb) = else_branch { mut_var_decls_expr(eb, out); }
        }
        ExprKind::While { body, .. } => mut_var_decls_block(body, out),
        ExprKind::Block(b) => mut_var_decls_block(b, out),
        ExprKind::Match { arms, .. } => arms.iter().for_each(|a| mut_var_decls_expr(&a.body, out)),
        _ => {}
    }
}

/// Las `var` de `body` que una closure de `body` captura → deben ir en una celda `Rc<RefCell<T>>`.
pub(super) fn cell_vars(body: &Block) -> std::collections::HashSet<String> {
    let mut decls = std::collections::HashSet::new();
    mut_var_decls_block(body, &mut decls);
    if decls.is_empty() {
        return decls; // atajo: sin `var`, no hay celdas
    }
    let mut captured = std::collections::HashSet::new();
    captured_idents_block(body, &mut captured);
    decls.retain(|n| captured.contains(n));
    decls
}
