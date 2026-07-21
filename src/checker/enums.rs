//! Resolución de la construcción de enums, M5 (movimiento puro; usar `git log --follow`).
//!
//! `Enum.Variante(args)` llega del parser como `Field`/`Call` (sintácticamente igual a un
//! acceso a campo); esta pasada pre-tipos los reescribe a `EnumLit` antes de verificar.

use super::*;

// =====================================================================
// Resolución de la construcción de enums (M5)
// =====================================================================
//
// `Enum.Variante(args)` y `obj.campo` comparten forma sintáctica, así que el parser
// no puede distinguirlos. Conocidos los nombres de enum, estas funciones recorren el
// AST y **reescriben** los `Field`/`Call` cuya cabeza es un enum en nodos `EnumLit`.
// Se ejecuta una vez, antes de verificar; los dos motores reciben el AST resuelto.

pub(super) fn resolve_block(block: &mut Block, enums: &HashSet<String>) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => resolve_expr(value, enums),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { resolve_expr(start, enums); resolve_expr(end, enums); }
                    ForIter::In(e) => resolve_expr(e, enums),
                    ForIter::Iter { expr, .. } => resolve_expr(expr, enums),
                }
                resolve_block(body, enums);
            }
            StmtKind::Assign { target, value } => {
                resolve_expr(target, enums);
                resolve_expr(value, enums);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    resolve_expr(v, enums);
                }
            }
            StmtKind::Expr(e) => resolve_expr(e, enums),
        }
    }
    if let Some(t) = &mut block.tail {
        resolve_expr(t, enums);
    }
}

pub(super) fn resolve_expr(expr: &mut Expr, enums: &HashSet<String>) {
    // Detectar la construcción de enum ANTES de recorrer los hijos. Si no, el `Field`
    // de la cabeza (`Enum.Variante`) se reescribiría como variante *nullary* antes de
    // que el `Call` que lo envuelve lo viera, perdiendo el payload.

    // Caso 1: `Enum.Variante(args)` — un Call cuyo callee es un Field con cabeza enum.
    if let ExprKind::Call { callee, args } = &mut expr.kind {
        if let ExprKind::Field { object, name } = &callee.kind {
            if is_enum_head(object, enums) {
                let enum_name = ident_name(object);
                let variant = name.clone();
                let mut args = std::mem::take(args);
                for a in &mut args {
                    resolve_expr(a, enums); // el payload sí se resuelve
                }
                expr.kind = ExprKind::EnumLit { enum_name, variant, args };
                return;
            }
        }
    }
    // Caso 2: `Enum.Variante` sin payload — un Field con cabeza enum.
    if let ExprKind::Field { object, name } = &expr.kind {
        if is_enum_head(object, enums) {
            expr.kind = ExprKind::EnumLit {
                enum_name: ident_name(object),
                variant: name.clone(),
                args: Vec::new(),
            };
            return;
        }
    }

    // Caso general: recorrer los sub-nodos.
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => resolve_expr(inner, enums),
        ExprKind::Binary { left, right, .. } => {
            resolve_expr(left, enums);
            resolve_expr(right, enums);
        }
        ExprKind::Call { callee, args } => {
            resolve_expr(callee, enums);
            for a in args {
                resolve_expr(a, enums);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                resolve_expr(e, enums);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { resolve_expr(k, enums); resolve_expr(v, enums); }
        }
        ExprKind::Index { array, index } => {
            resolve_expr(array, enums);
            resolve_expr(index, enums);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                resolve_expr(e, enums);
            }
        }
        ExprKind::Field { object, .. } => resolve_expr(object, enums),
        ExprKind::Func(fe) => resolve_block(&mut fe.body, enums),
        ExprKind::Match { scrutinee, arms } => {
            resolve_expr(scrutinee, enums);
            for arm in arms {
                resolve_expr(&mut arm.body, enums); if let Some(g) = &mut arm.guard { resolve_expr(g, enums); }
            }
        }
        ExprKind::Try(inner) => resolve_expr(inner, enums),
        ExprKind::If { cond, then_branch, else_branch } => {
            resolve_expr(cond, enums);
            resolve_block(then_branch, enums);
            if let Some(e) = else_branch {
                resolve_expr(e, enums);
            }
        }
        ExprKind::While { cond, body } => {
            resolve_expr(cond, enums);
            resolve_block(body, enums);
        }
        ExprKind::Block(b) => resolve_block(b, enums),
        // Literales, Ident, EnumLit: nada que recorrer.
        _ => {}
    }
}

/// ¿Es `expr` un identificador que nombra un enum?
pub(super) fn is_enum_head(expr: &Expr, enums: &HashSet<String>) -> bool {
    matches!(&expr.kind, ExprKind::Ident(n) if enums.contains(n))
}

/// Extrae el nombre de un `ExprKind::Ident` (precondición: `is_enum_head` fue cierto).
pub(super) fn ident_name(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Ident(n) => n.clone(),
        _ => crate::ice!("ident_name requires an Ident"),
    }
}
