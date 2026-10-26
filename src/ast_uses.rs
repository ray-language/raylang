//! Análisis de USOS de una variable sobre el AST, compartido por los dos backends (el emisor
//! nativo `transpile/emit.rs` y el compilador de la VM `compiler.rs`): las fusiones N-D4b/N-D4c
//! (`let f = s.split(sep)` consumido solo por `f[const]` o solo por `for x in f`) deciden lo mismo
//! en los dos motores porque preguntan aquí. Funciones libres, sin estado. Conservadoras: un
//! `match` o una `fn` anónima que podría ligar el nombre cuenta como uso.

use crate::ast::*;

/// N-D4c (M433): ¿el ÚNICO uso de `name` en `rest` + `tail` es exactamente un `for x in name { … }`
/// (con `x != name` y un cuerpo que no menciona `name`)? Entonces el `let name = s.split(sep)` no
/// necesita materializar el arreglo: el `for` itera el split directo.
pub fn split_consumed_only_by_for(name: &str, rest: &[Stmt], tail: Option<&Expr>) -> bool {
    let mut for_uses = 0usize;
    for st in rest {
        match &st.kind {
            StmtKind::For { pat: ForPat::Single(v), iter: ForIter::In(it), body, .. }
                if matches!(&it.kind, ExprKind::Ident(n) if n == name) =>
            {
                if v == name || mentions_name_block(name, body) {
                    return false;
                }
                for_uses += 1;
            }
            _ => {
                if mentions_name_stmt(name, st) {
                    return false;
                }
            }
        }
    }
    for_uses == 1 && !tail.is_some_and(|t| mentions_name_expr(name, t))
}

// --- N-D4c (M433): ¿menciona `name` en absoluto? Conservador sobre `split_uses_*`: cualquier uso
// (índice incluido) o cualquier construcción que pueda ligarlo (`match`, `fn`) cuenta como mención.
pub fn mentions_name_expr(name: &str, e: &Expr) -> bool {
    let mut ks = Vec::new();
    !split_uses_expr(name, e, &mut ks) || !ks.is_empty()
}
pub fn mentions_name_stmt(name: &str, s: &Stmt) -> bool {
    let mut ks = Vec::new();
    !split_uses_stmt(name, s, &mut ks) || !ks.is_empty()
}
pub fn mentions_name_block(name: &str, b: &Block) -> bool {
    let mut ks = Vec::new();
    !split_uses_block(name, b, &mut ks) || !ks.is_empty()
}

// --- N-D4b: análisis de usos de un `let f = split(…)` (funciones libres, sin estado) ---

/// ¿Todos los usos de `name` dentro de `e` son lecturas `name[entero literal ≥ 0]`? Acumula los
/// índices en `ks`. Conservador: `match` (sus patrones pueden ligar `name`) y las funciones
/// anónimas (sus parámetros pueden sombrearlo) descartan la fusión entera.
pub fn split_uses_expr(name: &str, e: &Expr, ks: &mut Vec<i64>) -> bool {
    match &e.kind {
        ExprKind::Index { array, index } => {
            if matches!(&array.kind, ExprKind::Ident(n) if n == name) {
                return match &index.kind {
                    ExprKind::Int(k, _) if *k >= 0 => {
                        ks.push(*k);
                        true
                    }
                    _ => false, // índice no constante (o negativo): sin fusión
                };
            }
            split_uses_expr(name, array, ks) && split_uses_expr(name, index, ks)
        }
        ExprKind::Ident(n) => n != name, // cualquier otro uso (alias, arg, len, for-in…) descarta
        ExprKind::Match { .. } => false, // un patrón podría ligar `name` y confundir el mapeo
        ExprKind::Func(_) => false,      // parámetros/capturas podrían sombrearlo
        ExprKind::Int(..) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_)
        | ExprKind::Char(_) | ExprKind::Bytes(_) => true,
        ExprKind::Unary { expr, .. } | ExprKind::Cast { expr, .. } | ExprKind::Try(expr) => {
            split_uses_expr(name, expr, ks)
        }
        ExprKind::Binary { left, right, .. } => {
            split_uses_expr(name, left, ks) && split_uses_expr(name, right, ks)
        }
        ExprKind::Call { callee, args } => {
            // el callee puede ser Ident (nombre de función ≠ variable) o Field (UFCS: el receptor
            // SÍ es un uso). Un Ident-callee homónimo sería una función, no la variable → se salta.
            let callee_ok = match &callee.kind {
                ExprKind::Ident(_) => true,
                ExprKind::Field { object, .. } => split_uses_expr(name, object, ks),
                other_callee => {
                    let fake = Expr { kind: other_callee.clone(), line: e.line, col: e.col };
                    split_uses_expr(name, &fake, ks)
                }
            };
            callee_ok && args.iter().all(|a| split_uses_expr(name, a, ks))
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            elems.iter().all(|x| split_uses_expr(name, x, ks))
        }
        ExprKind::MapLit(pairs) => pairs
            .iter()
            .all(|(k, v)| split_uses_expr(name, k, ks) && split_uses_expr(name, v, ks)),
        ExprKind::StructLit { fields, .. } => {
            fields.iter().all(|(_, x)| split_uses_expr(name, x, ks))
        }
        ExprKind::EnumLit { args, .. } => args.iter().all(|a| split_uses_expr(name, a, ks)),
        ExprKind::Field { object, .. } => split_uses_expr(name, object, ks),
        ExprKind::If { cond, then_branch, else_branch } => {
            split_uses_expr(name, cond, ks)
                && split_uses_block(name, then_branch, ks)
                && else_branch.as_ref().is_none_or(|x| split_uses_expr(name, x, ks))
        }
        ExprKind::While { cond, body, .. } => {
            split_uses_expr(name, cond, ks) && split_uses_block(name, body, ks)
        }
        ExprKind::Block(b) => split_uses_block(name, b, ks),
    }
}

pub fn split_uses_block(name: &str, b: &Block, ks: &mut Vec<i64>) -> bool {
    b.statements.iter().all(|s| split_uses_stmt(name, s, ks))
        && b.tail.as_ref().is_none_or(|t| split_uses_expr(name, t, ks))
}

/// La base (variable raíz) de un lvalue: `a` en `a`, `a[i]`, `a.x.y[j]`.
pub fn lvalue_base(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n),
        ExprKind::Index { array, .. } => lvalue_base(array),
        ExprKind::Field { object, .. } => lvalue_base(object),
        _ => None,
    }
}

pub fn split_uses_stmt(name: &str, s: &Stmt, ks: &mut Vec<i64>) -> bool {
    match &s.kind {
        StmtKind::Let { name: n, value, .. } => n != name && split_uses_expr(name, value, ks),
        StmtKind::LetTuple { names, value, .. } => {
            !names.iter().flatten().any(|n| n == name) && split_uses_expr(name, value, ks)
        }
        StmtKind::For { pat, iter, body, .. } => {
            let binds = match pat {
                ForPat::Single(n) => n == name,
                ForPat::Tuple(ns) => ns.iter().flatten().any(|n| n == name),
            };
            if binds {
                return false; // el for re-liga `name`: fuera
            }
            let iter_ok = match iter {
                ForIter::Range { start, end } => {
                    split_uses_expr(name, start, ks) && split_uses_expr(name, end, ks)
                }
                ForIter::In(e) | ForIter::Iter { expr: e, .. } => {
                    split_uses_expr(name, e, ks)
                }
            };
            iter_ok && split_uses_block(name, body, ks)
        }
        StmtKind::Assign { target, value } => {
            // ESCRIBIR sobre `name` (o dentro de él) descarta; los índices del lvalue y el RHS
            // son lecturas normales.
            if lvalue_base(target) == Some(name) {
                return false;
            }
            fn target_reads(name: &str, t: &Expr, ks: &mut Vec<i64>) -> bool {
                match &t.kind {
                    ExprKind::Ident(_) => true,
                    ExprKind::Index { array, index } => {
                        target_reads(name, array, ks) && split_uses_expr(name, index, ks)
                    }
                    ExprKind::Field { object, .. } => target_reads(name, object, ks),
                    _ => split_uses_expr(name, t, ks),
                }
            }
            target_reads(name, target, ks) && split_uses_expr(name, value, ks)
        }
        StmtKind::Break { .. } | StmtKind::Continue { .. } => true, // sin expresiones: como `return;`
        StmtKind::Return { value } => value.as_ref().is_none_or(|v| split_uses_expr(name, v, ks)),
        StmtKind::Expr(e) => split_uses_expr(name, e, ks),
    }
}
