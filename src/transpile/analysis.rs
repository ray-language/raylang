//! Análisis de captura mutable y de spawn (B1/H21-N5c; movimiento puro, usar `git log --follow`).
//!
//! `cell_vars` decide qué `var` locales van en una celda `Rc<RefCell<T>>` (capturadas y
//! mutadas por una closure); `spawn_fn_param_marks` marca los params de tipo función que
//! cruzan un `spawn` (punto fijo sobre el grafo de llamadas, H21-N5c).

use super::*;

// =====================================================================
// Análisis de captura mutable (B1): qué `var` locales van en una celda
// =====================================================================
//
// En raylang una closure captura POR REFERENCIA y puede MUTAR la variable capturada (patrón contador:
// `var n` que la closure incrementa entre llamadas). En Rust, un `move ||` es `Fn` inmutable → mutar
// una captura no compila. Solución (espejo de la semántica M4 de raylang): una `var` que sea
// CAPTURADA por una closure vive en una celda `Rc<RefCell<T>>` compartida — se lee con `.borrow()`,
// se escribe con `.borrow_mut()`, y la closure captura un clon del `Rc` (mutación compartida).
//
// `cell_vars(body)` = { `var` declaradas en `body` } ∩ { idents referenciados dentro de alguna closure
// de `body` }. No desciende a los cuerpos de closures anidadas (esos son ámbitos propios, con su
// propio análisis al emitirlos).

/// H21-N5c: marca los PARAMS de tipo función que "cruzan un spawn" — directamente (el closure de un
/// `spawn` en el cuerpo captura el param) o transitivamente (el param se pasa a un param ya marcado
/// de otra función). Punto fijo sobre el grafo de llamadas. Un param marcado se emite como GENÉRICO
/// de Rust con bound `Fn(..) + Send + Sync + Clone + 'static`: una función NOMBRADA lo satisface
/// (monomorfización → el spawn compila); un closure con capturas no-Send que llegue ahí lo rechaza
/// rustc (honesto — ese programa sí cruzaría un valor no enviable).
pub(super) fn spawn_fn_param_marks(prog: &Program) -> HashMap<String, std::collections::HashSet<usize>> {
    use std::collections::HashSet;
    // params de tipo fn por función: (índice, nombre)
    let mut fn_params: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    for f in &prog.functions {
        if skip_fn_def(f) {
            continue;
        }
        let fps: Vec<(usize, String)> = f
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(normalize_type(&p.ty), Type::Fn(..)))
            .map(|(i, p)| (i, p.name.clone()))
            .collect();
        if !fps.is_empty() {
            fn_params.insert(f.name.clone(), fps);
        }
    }
    let mut marks: HashMap<String, HashSet<usize>> = HashMap::new();
    loop {
        let mut changed = false;
        for f in &prog.functions {
            if skip_fn_def(f) {
                continue;
            }
            let Some(fps) = fn_params.get(&f.name) else { continue };
            let mut hits: HashSet<usize> = HashSet::new();
            visit_exprs_block(&f.body, &mut |e: &Expr| {
                if let ExprKind::Call { callee, args } = &e.kind {
                    if let ExprKind::Ident(cn) = &callee.kind {
                        if cn == "spawn" {
                            if let Some(arg0) = args.first() {
                                if let ExprKind::Func(fx) = &arg0.kind {
                                    let mut ids = std::collections::HashSet::new();
                                    idents_of_block(&fx.body, &mut ids);
                                    for (i, pname) in fps {
                                        if ids.contains(pname) {
                                            hits.insert(*i);
                                        }
                                    }
                                }
                            }
                        } else if let Some(cm) = marks.get(cn) {
                            for (j, a) in args.iter().enumerate() {
                                if cm.contains(&j) {
                                    match &a.kind {
                                        ExprKind::Ident(an) => {
                                            for (i, pname) in fps {
                                                if an == pname {
                                                    hits.insert(*i);
                                                }
                                            }
                                        }
                                        // Un CLOSURE pasado a una posición marcada cruzará el hilo;
                                        // los fn-params del llamador que capture cruzan con él (el
                                        // patrón builder: serve(h, p, fn(req){ handle(build(), req) })).
                                        ExprKind::Func(fx) => {
                                            let mut ids = std::collections::HashSet::new();
                                            idents_of_block(&fx.body, &mut ids);
                                            for (i, pname) in fps {
                                                if ids.contains(pname) {
                                                    hits.insert(*i);
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            });
            // M97.2: propagación HACIA DELANTE. Las reglas de arriba marcan al LLAMADOR (su param
            // cruza porque lo captura un spawn, propio o de un callee ya marcado). Falta el sentido
            // contrario: si un param YA marcado se pasa tal cual a otra función, la posición
            // receptora también tiene que viajar como genérico — un `__F` no se convierte solo a
            // `Rc<dyn Fn>` y rustc lo rechaza con "expected Rc<dyn Fn…>, found type parameter __F".
            //
            // Se destapó al cambiar `handle_http` de `spawn`+`try_join` a `try_call`: perdió su
            // `spawn` (y con él su marca) mientras `loop_iter_server`, que sí spawnea por conexión y
            // le pasa el handler, seguía marcado. Marcar la posición receptora es además lo
            // SEMÁNTICAMENTE correcto: ese handler sigue cruzando a la fibra de la conexión, así que
            // necesita los mismos bounds (`Send + Sync + Clone`) que ya tiene en el llamador.
            let mine: Vec<usize> = marks.get(&f.name).map(|m| m.iter().copied().collect()).unwrap_or_default();
            let mut forward: Vec<(String, usize)> = Vec::new();
            if !mine.is_empty() {
                let marked_names: Vec<&String> =
                    fps.iter().filter(|(i, _)| mine.contains(i)).map(|(_, n)| n).collect();
                visit_exprs_block(&f.body, &mut |e: &Expr| {
                    if let ExprKind::Call { callee, args } = &e.kind
                        && let ExprKind::Ident(cn) = &callee.kind
                        && cn != "spawn"
                    {
                        for (j, a) in args.iter().enumerate() {
                            if let ExprKind::Ident(an) = &a.kind
                                && marked_names.iter().any(|n| *n == an)
                            {
                                forward.push((cn.clone(), j));
                            }
                        }
                    }
                });
            }
            let entry = marks.entry(f.name.clone()).or_default();
            for h in hits {
                if entry.insert(h) {
                    changed = true;
                }
            }
            for (cn, j) in forward {
                // Solo si esa posición del callee es de verdad un param de tipo fn (un homónimo o
                // un builtin no tienen entrada en `fn_params` y se ignoran solos).
                if fn_params.get(&cn).is_some_and(|ps| ps.iter().any(|(i, _)| *i == j))
                    && marks.entry(cn).or_default().insert(j)
                {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    marks
}

/// Visita cada Expr de un bloque (sentencias + cola), descendiendo a sub-exprs y cuerpos de closures.
pub(super) fn visit_exprs_block(b: &Block, f: &mut impl FnMut(&Expr)) {
    for st in &b.statements {
        match &st.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } | StmtKind::Expr(value) => {
                visit_exprs_expr(value, f)
            }
            StmtKind::Assign { target, value } => {
                visit_exprs_expr(target, f);
                visit_exprs_expr(value, f);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    visit_exprs_expr(v, f);
                }
            }
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => {
                        visit_exprs_expr(start, f);
                        visit_exprs_expr(end, f);
                    }
                    ForIter::In(e) => visit_exprs_expr(e, f),
                    ForIter::Iter { expr, .. } => visit_exprs_expr(expr, f),
                }
                visit_exprs_block(body, f);
            }
        }
    }
    if let Some(t) = &b.tail {
        visit_exprs_expr(t, f);
    }
}

pub(super) fn visit_exprs_expr(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Unary { expr, .. } | ExprKind::Cast { expr, .. } | ExprKind::Try(expr) => {
            visit_exprs_expr(expr, f)
        }
        ExprKind::Binary { left, right, .. } => {
            visit_exprs_expr(left, f);
            visit_exprs_expr(right, f);
        }
        ExprKind::Call { callee, args } => {
            visit_exprs_expr(callee, f);
            args.iter().for_each(|a| visit_exprs_expr(a, f));
        }
        ExprKind::ArrayLit(es) | ExprKind::TupleLit(es) => es.iter().for_each(|x| visit_exprs_expr(x, f)),
        ExprKind::MapLit(ps) => ps.iter().for_each(|(k, v)| {
            visit_exprs_expr(k, f);
            visit_exprs_expr(v, f);
        }),
        ExprKind::Index { array, index } => {
            visit_exprs_expr(array, f);
            visit_exprs_expr(index, f);
        }
        ExprKind::StructLit { fields, .. } => fields.iter().for_each(|(_, v)| visit_exprs_expr(v, f)),
        ExprKind::Field { object, .. } => visit_exprs_expr(object, f),
        ExprKind::EnumLit { args, .. } => args.iter().for_each(|a| visit_exprs_expr(a, f)),
        ExprKind::Func(fx) => visit_exprs_block(&fx.body, f),
        ExprKind::Match { scrutinee, arms } => {
            visit_exprs_expr(scrutinee, f);
            arms.iter().for_each(|a| visit_exprs_expr(&a.body, f));
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            visit_exprs_expr(cond, f);
            visit_exprs_block(then_branch, f);
            if let Some(eb) = else_branch {
                visit_exprs_expr(eb, f);
            }
        }
        ExprKind::While { cond, body } => {
            visit_exprs_expr(cond, f);
            visit_exprs_block(body, f);
        }
        ExprKind::Block(b) => visit_exprs_block(b, f),
        _ => {}
    }
}

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
