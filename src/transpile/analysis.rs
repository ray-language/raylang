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
                        if cn == "spawn" || cn == "spawn_isolated" {
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
                        && cn != "spawn_isolated"
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
            StmtKind::Break | StmtKind::Continue => {}
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
            StmtKind::Break | StmtKind::Continue => {}
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
            StmtKind::Break | StmtKind::Continue => {}
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
            StmtKind::Break | StmtKind::Continue => {}
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

/// M295: qué funciones y qué closures necesitan el prólogo de profundidad (`__ray_enter`). Solo
/// las que pueden RECURRIR: las que están en un ciclo del grafo de llamadas (incluido el bucle a
/// sí mismas). El grafo es conservador: una llamada cuyo callee no es el nombre de una función del
/// programa (un valor `fn`, un método `dyn`) apunta al nodo VALUE, que a su vez apunta a toda
/// función usada como valor (su nombre en posición no-callee, o un método de trait `Tipo#m`,
/// alcanzable por `dyn`) y a TODAS las closures (cada una es un nodo propio, `<closure:id>`, con
/// sus callees; cualquiera podría ser el valor llamado). Una función que crea una closure NO
/// apunta a ella (crearla no la ejecuta); la closure entra en un ciclo solo si sus llamadas
/// alcanzan VALUE — `fn(x) x + 1` no paga nada; `fn(x) xs.map(g)` sí. Devuelve `(funciones en
/// ciclo, ids de closures en ciclo)`. Así el código no recursivo —la mayoría— no paga el contador
/// (~1,3 ns por llamada, medido en fib35).
pub(super) fn depth_checked_fns(prog: &Program) -> (std::collections::HashSet<String>, std::collections::HashSet<usize>) {
    use std::collections::{HashMap, HashSet};
    const VALUE: &str = "<value>";
    let fn_names: HashSet<&str> = prog.functions.iter().map(|f| f.name.as_str()).collect();
    let mut edges: HashMap<String, HashSet<String>> = HashMap::new();
    let mut as_value: HashSet<String> = HashSet::new();
    let mut closure_ids: Vec<usize> = Vec::new();
    // Callees directos de un bloque SIN descender a las closures que contiene (cada closure es
    // su propio nodo); `calls_value` si alguna llamada no es a una función por nombre.
    fn callees_of(b: &Block, fn_names: &HashSet<&str>, out: &mut HashSet<String>) -> bool {
        let mut calls_value = false;
        // `visit_exprs_block` desciende a las closures; se filtran por posición: todo lo que cae
        // dentro del rango de una closure del bloque se atribuye a esa closure, no a este cuerpo.
        let mut inner: Vec<(usize, usize)> = Vec::new(); // (línea, col) de inicio de cada closure
        visit_exprs_block(b, &mut |e: &Expr| {
            if let ExprKind::Func(_) = &e.kind {
                inner.push((e.line, e.col));
            }
        });
        let _ = &inner;
        visit_exprs_block(b, &mut |e: &Expr| {
            if let ExprKind::Call { callee, .. } = &e.kind {
                match &callee.kind {
                    ExprKind::Ident(n) if fn_names.contains(n.as_str()) => {
                        out.insert(n.clone());
                    }
                    _ => calls_value = true,
                }
            }
        });
        calls_value
    }
    // Recorre las closures de un bloque (anidadas incluidas) dándole a cada una su nodo.
    fn closures_of(b: &Block, fn_names: &HashSet<&str>, edges: &mut HashMap<String, HashSet<String>>, ids: &mut Vec<usize>) {
        visit_exprs_block(b, &mut |e: &Expr| {
            if let ExprKind::Func(fx) = &e.kind {
                let mut out = HashSet::new();
                if callees_of(&fx.body, fn_names, &mut out) {
                    out.insert(VALUE.to_string());
                }
                edges.insert(format!("<closure:{}>", fx.id), out);
                ids.push(fx.id);
            }
        });
    }
    for f in &prog.functions {
        let mut out = HashSet::new();
        if callees_of(&f.body, &fn_names, &mut out) {
            out.insert(VALUE.to_string());
        }
        // Usos por valor: un Ident de función que NO es el callee de una llamada.
        let mut callee_positions: HashSet<(usize, usize)> = HashSet::new();
        visit_exprs_block(&f.body, &mut |e: &Expr| {
            if let ExprKind::Call { callee, .. } = &e.kind
                && let ExprKind::Ident(n) = &callee.kind
                && fn_names.contains(n.as_str())
            {
                callee_positions.insert((callee.line, callee.col));
            }
        });
        visit_exprs_block(&f.body, &mut |e: &Expr| {
            if let ExprKind::Ident(n) = &e.kind
                && fn_names.contains(n.as_str())
                && !callee_positions.contains(&(e.line, e.col))
            {
                as_value.insert(n.clone());
            }
        });
        closures_of(&f.body, &fn_names, &mut edges, &mut closure_ids);
        edges.insert(f.name.clone(), out);
    }
    // VALUE: toda función usada por valor, los métodos de trait (`dyn`) y todas las closures.
    let mut val: HashSet<String> = as_value;
    for f in &prog.functions {
        if f.name.contains('#') {
            val.insert(f.name.clone());
        }
    }
    for id in &closure_ids {
        val.insert(format!("<closure:{id}>"));
    }
    edges.insert(VALUE.to_string(), val);
    // Tarjan: nodos en una SCC de tamaño > 1, o con bucle propio.
    let nodes: Vec<String> = edges.keys().cloned().collect();
    let idx: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
    let adj: Vec<Vec<usize>> = nodes.iter().map(|n| edges[n].iter().filter_map(|m| idx.get(m.as_str()).copied()).collect()).collect();
    let comp = scc_components(&adj);
    let mut size = vec![0usize; comp.iter().max().map_or(0, |m| m + 1)];
    for &c in &comp {
        size[c] += 1;
    }
    let mut fns = HashSet::new();
    let mut closures = HashSet::new();
    for (v, name) in nodes.iter().enumerate() {
        if size[comp[v]] > 1 || adj[v].contains(&v) {
            if let Some(id) = name.strip_prefix("<closure:").and_then(|r| r.strip_suffix('>')) {
                closures.insert(id.parse::<usize>().unwrap_or(usize::MAX));
            } else if name != VALUE {
                fns.insert(name.clone());
            }
        }
    }
    (fns, closures)
}

/// Componentes fuertemente conexas (Tarjan): devuelve, por nodo, el índice de su componente.
fn scc_components(adj: &[Vec<usize>]) -> Vec<usize> {
    struct T<'a> {
        adj: &'a [Vec<usize>],
        index: Vec<usize>,
        low: Vec<usize>,
        on: Vec<bool>,
        comp: Vec<usize>,
        stack: Vec<usize>,
        next: usize,
        ncomp: usize,
    }
    impl T<'_> {
        fn strong(&mut self, v: usize) {
            self.index[v] = self.next;
            self.low[v] = self.next;
            self.next += 1;
            self.stack.push(v);
            self.on[v] = true;
            for i in 0..self.adj[v].len() {
                let w = self.adj[v][i];
                if self.index[w] == usize::MAX {
                    self.strong(w);
                    self.low[v] = self.low[v].min(self.low[w]);
                } else if self.on[w] {
                    self.low[v] = self.low[v].min(self.index[w]);
                }
            }
            if self.low[v] == self.index[v] {
                loop {
                    let w = self.stack.pop().unwrap_or(v);
                    self.on[w] = false;
                    self.comp[w] = self.ncomp;
                    if w == v {
                        break;
                    }
                }
                self.ncomp += 1;
            }
        }
    }
    let n = adj.len();
    let mut t = T { adj, index: vec![usize::MAX; n], low: vec![0; n], on: vec![false; n], comp: vec![usize::MAX; n], stack: Vec::new(), next: 0, ncomp: 0 };
    for v in 0..n {
        if t.index[v] == usize::MAX {
            t.strong(v);
        }
    }
    t.comp
}

/// M295: posiciones `(línea, col)` de las llamadas en POSICIÓN DE COLA de un cuerpo — la cola del
/// bloque, las colas de las ramas de `if`/`match`/bloques anidados, y `return <llamada>`. Espeja la
/// regla del `TailCall` de la VM (compiler.rs: una llamada cuya continuación es el `Return`): en
/// esos sitios el nativo suelta el guard de profundidad ANTES de llamar (`drop(_f)`), así una
/// función que itera por recursión en cola no cuenta marcos —como en la VM, que reutiliza el
/// marco— y Rust sigue pudiendo hacer la llamada como salto (sibling call).
pub(super) fn tail_call_sites(body: &Block) -> std::collections::HashSet<(usize, usize)> {
    let mut out = std::collections::HashSet::new();
    fn expr(e: &Expr, out: &mut std::collections::HashSet<(usize, usize)>) {
        match &e.kind {
            ExprKind::Call { .. } => {
                out.insert((e.line, e.col));
            }
            ExprKind::If { then_branch, else_branch, .. } => {
                block(then_branch, out);
                if let Some(eb) = else_branch {
                    expr(eb, out);
                }
            }
            ExprKind::Match { arms, .. } => arms.iter().for_each(|a| expr(&a.body, out)),
            ExprKind::Block(b) => block(b, out),
            _ => {}
        }
    }
    fn block(b: &Block, out: &mut std::collections::HashSet<(usize, usize)>) {
        // `return <llamada>;` en cualquier sentencia del cuerpo (también dentro de bucles: el
        // `return` sale de la función, así que la llamada es de cola igualmente).
        for st in &b.statements {
            returns(st, out);
        }
        if let Some(t) = &b.tail {
            expr(t, out);
        }
    }
    fn returns(st: &Stmt, out: &mut std::collections::HashSet<(usize, usize)>) {
        match &st.kind {
            StmtKind::Return { value: Some(v) } => expr(v, out),
            StmtKind::Expr(e) => returns_in_expr(e, out),
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => returns_in_expr(value, out),
            StmtKind::Assign { value, .. } => returns_in_expr(value, out),
            StmtKind::For { body, .. } => body.statements.iter().for_each(|s| returns(s, out)),
            _ => {}
        }
    }
    // Un `return` puede vivir dentro de un if/match/while/bloque usado como sentencia.
    fn returns_in_expr(e: &Expr, out: &mut std::collections::HashSet<(usize, usize)>) {
        match &e.kind {
            ExprKind::If { then_branch, else_branch, .. } => {
                then_branch.statements.iter().for_each(|s| returns(s, out));
                if let Some(t) = &then_branch.tail { returns_in_expr(t, out); }
                if let Some(eb) = else_branch { returns_in_expr(eb, out); }
            }
            ExprKind::Match { arms, .. } => arms.iter().for_each(|a| returns_in_expr(&a.body, out)),
            ExprKind::While { body, .. } => {
                body.statements.iter().for_each(|s| returns(s, out));
                if let Some(t) = &body.tail { returns_in_expr(t, out); }
            }
            ExprKind::Block(b) => {
                b.statements.iter().for_each(|s| returns(s, out));
                if let Some(t) = &b.tail { returns_in_expr(t, out); }
            }
            _ => {}
        }
    }
    block(body, &mut out);
    out
}

