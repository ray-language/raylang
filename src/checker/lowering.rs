//! Las 6 bajadas post-check: UFCS/dyn/uint/`?`-conversión/operadores/diccionarios (movimiento
//! puro; usar `git log --follow`).
//!
//! Cada pasada reescribe el AST **después** de verificar tipos, usando los sitios que el
//! checker registró por posición: UFCS/métodos de trait (M7.1/M9.1), diccionarios de bounds
//! (M9.2), trait objects `dyn` (M9.3b), literales uint coercionados (M28.3b), `?` con
//! conversión de error (M28.2), sobrecarga de operadores (M28.1).

use super::*;

// =====================================================================
// Bajada de llamadas por punto (UFCS M7.1 + métodos de trait M9.1)
// =====================================================================
//
// `recv.f(args)` y `(recv.f)(args)` comparten forma (`Call` con callee `Field`). El
// checker, que conoce el tipo del receptor, decidió cuáles hay que bajar —UFCS de
// función libre o método de trait— y registró cada sitio `(línea, columna, nombre)`
// junto a su **función destino** (el mismo nombre, o el manglado `Tipo#metodo`). Estas
// funciones recorren el AST y **reescriben** esos nodos a `destino(recv, args)`: el
// receptor pasa a ser el primer argumento y el callee se vuelve un `Ident`. Tras esto,
// el intérprete y la VM solo ven llamadas ordinarias.

type SiteMap = HashMap<(usize, usize, String), String>;

pub(super) fn lower_ufcs(program: &mut Program, sites: &SiteMap) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_ufcs_block(&mut f.body, sites);
    }
}

pub(super) fn lower_ufcs_block(block: &mut Block, sites: &SiteMap) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_ufcs_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_ufcs_expr(start, sites); lower_ufcs_expr(end, sites); }
                    ForIter::In(e) => lower_ufcs_expr(e, sites),
                    ForIter::Iter { expr, .. } => lower_ufcs_expr(expr, sites),
                }
                lower_ufcs_block(body, sites);
            }
            StmtKind::Assign { target, value } => {
                lower_ufcs_expr(target, sites);
                lower_ufcs_expr(value, sites);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    lower_ufcs_expr(v, sites);
                }
            }
            StmtKind::Expr(e) => lower_ufcs_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_ufcs_expr(t, sites);
    }
}

pub(super) fn lower_ufcs_expr(expr: &mut Expr, sites: &SiteMap) {
    // ¿Este `Call(Field)` es un sitio registrado? La clave incluye el nombre del método
    // porque el `Call` y su receptor comparten `(línea, columna)`; el valor es la función
    // **destino** (el mismo nombre para UFCS de función libre, el manglado para un método
    // de trait). Reescribir ANTES de recorrer los hijos, para que la recursión baje
    // también el receptor y los argumentos (p. ej. `a.f().g()`).
    let target = match &expr.kind {
        ExprKind::Call { callee, .. } => match &callee.kind {
            ExprKind::Field { name, .. } => sites.get(&(expr.line, expr.col, name.clone())).cloned(),
            _ => None,
        },
        _ => None,
    };
    if let Some(target) = target {
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0, crate::token::Radix::Dec));
        if let ExprKind::Call { callee, mut args } = taken {
            let (cl, cc) = (callee.line, callee.col);
            if let ExprKind::Field { object, .. } = callee.kind {
                let mut new_args = Vec::with_capacity(args.len() + 1);
                new_args.push(*object); // el receptor pasa a ser el primer argumento
                new_args.append(&mut args);
                expr.kind = ExprKind::Call {
                    callee: Box::new(Expr { kind: ExprKind::Ident(target), line: cl, col: cc }),
                    args: new_args,
                };
            } else {
                crate::ice!("the site guard guarantees a Call with a Field callee");
            }
        } else {
            crate::ice!("the site guard guarantees a Call");
        }
    }

    // Recorrer los sub-nodos (incluye los argumentos ya reescritos).
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_ufcs_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => {
            lower_ufcs_expr(left, sites);
            lower_ufcs_expr(right, sites);
        }
        ExprKind::Call { callee, args } => {
            lower_ufcs_expr(callee, sites);
            for a in args {
                lower_ufcs_expr(a, sites);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                lower_ufcs_expr(e, sites);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { lower_ufcs_expr(k, sites); lower_ufcs_expr(v, sites); }
        }
        ExprKind::Index { array, index } => {
            lower_ufcs_expr(array, sites);
            lower_ufcs_expr(index, sites);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                lower_ufcs_expr(e, sites);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                lower_ufcs_expr(a, sites);
            }
        }
        ExprKind::Field { object, .. } => lower_ufcs_expr(object, sites),
        ExprKind::Func(fe) => lower_ufcs_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_ufcs_expr(scrutinee, sites);
            for arm in arms {
                lower_ufcs_expr(&mut arm.body, sites); if let Some(g) = &mut arm.guard { lower_ufcs_expr(g, sites); }
            }
        }
        ExprKind::Try(inner) => lower_ufcs_expr(inner, sites),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_ufcs_expr(cond, sites);
            lower_ufcs_block(then_branch, sites);
            if let Some(e) = else_branch {
                lower_ufcs_expr(e, sites);
            }
        }
        ExprKind::While { cond, body } => {
            lower_ufcs_expr(cond, sites);
            lower_ufcs_block(body, sites);
        }
        ExprKind::Block(b) => lower_ufcs_block(b, sites),
        // Literales, Ident: nada que recorrer.
        _ => {}
    }
}

// =====================================================================
// Bajada de literales enteros coercionados a uint (M28.3b)
// =====================================================================
//
// Un literal entero en posición uint (`let x: u8 = 5`, `x + 100` con `x: u8`) se registró en
// `uint_literal_sites`. Aquí se envuelve en un `Cast` al ancho (`5 as u8`), de modo que el
// runtime —que borra los tipos— produzca el `UInt` correcto. Reusa el `as` de M27.4/M28.3a.

type UIntLitMap = HashMap<(usize, usize), u8>;

pub(super) fn lower_uint_literals(program: &mut Program, sites: &UIntLitMap) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_uintlit_block(&mut f.body, sites);
    }
}

pub(super) fn lower_uintlit_block(block: &mut Block, sites: &UIntLitMap) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_uintlit_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_uintlit_expr(start, sites); lower_uintlit_expr(end, sites); }
                    ForIter::In(e) => lower_uintlit_expr(e, sites),
                    ForIter::Iter { expr, .. } => lower_uintlit_expr(expr, sites),
                }
                lower_uintlit_block(body, sites);
            }
            StmtKind::Assign { target, value } => { lower_uintlit_expr(target, sites); lower_uintlit_expr(value, sites); }
            StmtKind::Return { value } => { if let Some(v) = value { lower_uintlit_expr(v, sites); } }
            StmtKind::Expr(e) => lower_uintlit_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_uintlit_expr(t, sites);
    }
}

pub(super) fn lower_uintlit_expr(expr: &mut Expr, sites: &UIntLitMap) {
    // ¿Es un literal entero registrado? Envolverlo en `Cast` al ancho. (No tiene hijos que recorrer.)
    if let ExprKind::Int(..) = &expr.kind {
        if let Some(&w) = sites.get(&(expr.line, expr.col)) {
            let (l, c) = (expr.line, expr.col);
            let inner = std::mem::replace(&mut expr.kind, ExprKind::Int(0, crate::token::Radix::Dec));
            expr.kind = ExprKind::Cast {
                expr: Box::new(Expr { kind: inner, line: l, col: c }),
                ty: Type::UInt(w),
            };
            return;
        }
    }
    // Recorrer los sub-nodos.
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_uintlit_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => { lower_uintlit_expr(left, sites); lower_uintlit_expr(right, sites); }
        ExprKind::Call { callee, args } => {
            lower_uintlit_expr(callee, sites);
            for a in args { lower_uintlit_expr(a, sites); }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => { for e in elems { lower_uintlit_expr(e, sites); } }
        ExprKind::MapLit(pares) => { for (k, v) in pares { lower_uintlit_expr(k, sites); lower_uintlit_expr(v, sites); } }
        ExprKind::Index { array, index } => { lower_uintlit_expr(array, sites); lower_uintlit_expr(index, sites); }
        ExprKind::StructLit { fields, .. } => { for (_, e) in fields { lower_uintlit_expr(e, sites); } }
        ExprKind::EnumLit { args, .. } => { for a in args { lower_uintlit_expr(a, sites); } }
        ExprKind::Field { object, .. } => lower_uintlit_expr(object, sites),
        ExprKind::Func(fe) => lower_uintlit_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_uintlit_expr(scrutinee, sites);
            for arm in arms { lower_uintlit_expr(&mut arm.body, sites); if let Some(g) = &mut arm.guard { lower_uintlit_expr(g, sites); } }
        }
        ExprKind::Try(inner) => lower_uintlit_expr(inner, sites),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_uintlit_expr(cond, sites);
            lower_uintlit_block(then_branch, sites);
            if let Some(e) = else_branch { lower_uintlit_expr(e, sites); }
        }
        ExprKind::While { cond, body } => { lower_uintlit_expr(cond, sites); lower_uintlit_block(body, sites); }
        ExprKind::Block(b) => lower_uintlit_block(b, sites),
        _ => {}
    }
}

// =====================================================================
// Bajada de `?` con conversión de error (M28.2)
// =====================================================================
//
// `expr?` sobre `Result<T, E1>` en una función que devuelve `Result<_, E2>` (con
// `impl From<E1> for E2`) se registró en `try_conversions` con la función manglada de
// conversión. Aquí ese `Try` se reescribe a:
//
//     match (expr) {
//         Result.Ok($to)  => $to,
//         Result.Err($te) => { return Result.Err(<from>($te)); },
//     }
//
// Puro front-end (reusa `match`, construcción de enum y `return`): el runtime no cambia.
// El `?` que NO convierte sigue siendo el nodo nativo `Try` (M6.3).

type TryConvMap = HashMap<(usize, usize), String>;

pub(super) fn lower_try_conversions(program: &mut Program, sites: &TryConvMap) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_try_block(&mut f.body, sites);
    }
}

pub(super) fn lower_try_block(block: &mut Block, sites: &TryConvMap) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_try_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_try_expr(start, sites); lower_try_expr(end, sites); }
                    ForIter::In(e) => lower_try_expr(e, sites),
                    ForIter::Iter { expr, .. } => lower_try_expr(expr, sites),
                }
                lower_try_block(body, sites);
            }
            StmtKind::Assign { target, value } => { lower_try_expr(target, sites); lower_try_expr(value, sites); }
            StmtKind::Return { value } => { if let Some(v) = value { lower_try_expr(v, sites); } }
            StmtKind::Expr(e) => lower_try_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_try_expr(t, sites);
    }
}

pub(super) fn lower_try_expr(expr: &mut Expr, sites: &TryConvMap) {
    // ¿Este `Try` es un sitio de conversión registrado? Reescribir ANTES de recorrer los hijos,
    // para que la recursión baje el operando (ahora escrutinio del match).
    let conv = match &expr.kind {
        ExprKind::Try(_) => sites.get(&(expr.line, expr.col)).cloned(),
        _ => None,
    };
    if let Some(mangled) = conv {
        let (l, c) = (expr.line, expr.col);
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0, crate::token::Radix::Dec));
        let inner = match taken {
            ExprKind::Try(inner) => *inner,
            _ => crate::ice!("the guard guarantees a Try"),
        };
        let mk = |kind| Expr { kind, line: l, col: c };
        // Rama Ok: `Result.Ok($to) => $to`.
        let arm_ok = MatchArm {
            pattern: Pattern {
                kind: PatternKind::Variant { enum_name: "Result".into(), variant: "Ok".into(), subpatterns: vec![Pattern { kind: PatternKind::Binding("$to".into()), line: l, col: c }] },
                line: l, col: c,
            },
            guard: None,
            body: mk(ExprKind::Ident("$to".into())),
            line: l, col: c,
        };
        // Rama Err: `Result.Err($te) => { return Result.Err(<from>($te)); }`.
        let converted = mk(ExprKind::Call {
            callee: Box::new(mk(ExprKind::Ident(mangled))),
            args: vec![mk(ExprKind::Ident("$te".into()))],
        });
        let err_val = mk(ExprKind::EnumLit { enum_name: "Result".into(), variant: "Err".into(), args: vec![converted] });
        let ret_stmt = Stmt { kind: StmtKind::Return { value: Some(err_val) }, line: l, col: c };
        let arm_err = MatchArm {
            pattern: Pattern {
                kind: PatternKind::Variant { enum_name: "Result".into(), variant: "Err".into(), subpatterns: vec![Pattern { kind: PatternKind::Binding("$te".into()), line: l, col: c }] },
                line: l, col: c,
            },
            guard: None,
            body: mk(ExprKind::Block(Block { statements: vec![ret_stmt], tail: None, line: l, col: c, end_line: l })),
            line: l, col: c,
        };
        expr.kind = ExprKind::Match { scrutinee: Box::new(inner), arms: vec![arm_ok, arm_err] };
    }

    // Recorrer los sub-nodos.
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_try_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => { lower_try_expr(left, sites); lower_try_expr(right, sites); }
        ExprKind::Call { callee, args } => {
            lower_try_expr(callee, sites);
            for a in args { lower_try_expr(a, sites); }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => { for e in elems { lower_try_expr(e, sites); } }
        ExprKind::MapLit(pares) => { for (k, v) in pares { lower_try_expr(k, sites); lower_try_expr(v, sites); } }
        ExprKind::Index { array, index } => { lower_try_expr(array, sites); lower_try_expr(index, sites); }
        ExprKind::StructLit { fields, .. } => { for (_, e) in fields { lower_try_expr(e, sites); } }
        ExprKind::EnumLit { args, .. } => { for a in args { lower_try_expr(a, sites); } }
        ExprKind::Field { object, .. } => lower_try_expr(object, sites),
        ExprKind::Func(fe) => lower_try_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_try_expr(scrutinee, sites);
            for arm in arms { lower_try_expr(&mut arm.body, sites); if let Some(g) = &mut arm.guard { lower_try_expr(g, sites); } }
        }
        ExprKind::Try(inner) => lower_try_expr(inner, sites),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_try_expr(cond, sites);
            lower_try_block(then_branch, sites);
            if let Some(e) = else_branch { lower_try_expr(e, sites); }
        }
        ExprKind::While { cond, body } => { lower_try_expr(cond, sites); lower_try_block(body, sites); }
        ExprKind::Block(b) => lower_try_block(b, sites),
        _ => {}
    }
}

// =====================================================================
// Bajada de sobrecarga de operadores (M28.1)
// =====================================================================
//
// `a op b` (con un tipo de usuario que implementa el trait del operador) y `-x` se
// registraron en `op_sites` con clave `(línea, col, "Add"/"Sub"/…/"Neg")` → función
// manglada del método (`Vec2#add`). Aquí se reescriben esos `Binary`/`Unary` a una
// llamada ordinaria `metodo(a, b)` / `metodo(x)`, que el intérprete y la VM ya saben
// ejecutar (el método es una función libre inyectada por M9). Corre **antes** de
// `lower_ufcs`, así el resultado —un `Call(Ident)`— no necesita más bajadas.

pub(super) fn lower_operators(program: &mut Program, sites: &SiteMap) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_operators_block(&mut f.body, sites);
    }
}

pub(super) fn lower_operators_block(block: &mut Block, sites: &SiteMap) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_operators_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_operators_expr(start, sites); lower_operators_expr(end, sites); }
                    ForIter::In(e) => lower_operators_expr(e, sites),
                    ForIter::Iter { expr, .. } => lower_operators_expr(expr, sites),
                }
                lower_operators_block(body, sites);
            }
            StmtKind::Assign { target, value } => {
                lower_operators_expr(target, sites);
                lower_operators_expr(value, sites);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    lower_operators_expr(v, sites);
                }
            }
            StmtKind::Expr(e) => lower_operators_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_operators_expr(t, sites);
    }
}

pub(super) fn lower_operators_expr(expr: &mut Expr, sites: &SiteMap) {
    // ¿Este `Binary`/`Unary` es un sitio registrado? La clave lleva el nombre del trait del
    // operador porque un mismo `(línea, col)` puede corresponder a operadores encadenados
    // (`a + b + c`); un mismo operador en la misma posición baja al mismo método. Reescribir
    // ANTES de recorrer los hijos, para que la recursión baje también los operandos.
    let target = match &expr.kind {
        ExprKind::Binary { op, .. } => op_trait_method(*op)
            .and_then(|(tr, _)| sites.get(&(expr.line, expr.col, tr.to_string())).cloned()),
        ExprKind::Unary { op: UnaryOp::Neg, .. } => {
            sites.get(&(expr.line, expr.col, "Neg".to_string())).cloned()
        }
        _ => None,
    };
    if let Some(target) = target {
        let (l, c) = (expr.line, expr.col);
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0, crate::token::Radix::Dec));
        let args = match taken {
            ExprKind::Binary { left, right, .. } => vec![*left, *right],
            ExprKind::Unary { expr: inner, .. } => vec![*inner],
            _ => crate::ice!("the site guard guarantees Binary or Unary Neg"),
        };
        expr.kind = ExprKind::Call {
            callee: Box::new(Expr { kind: ExprKind::Ident(target), line: l, col: c }),
            args,
        };
    }

    // Recorrer los sub-nodos (incluye los operandos ya reescritos).
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_operators_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => {
            lower_operators_expr(left, sites);
            lower_operators_expr(right, sites);
        }
        ExprKind::Call { callee, args } => {
            lower_operators_expr(callee, sites);
            for a in args {
                lower_operators_expr(a, sites);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { lower_operators_expr(k, sites); lower_operators_expr(v, sites); }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                lower_operators_expr(e, sites);
            }
        }
        ExprKind::Index { array, index } => {
            lower_operators_expr(array, sites);
            lower_operators_expr(index, sites);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                lower_operators_expr(e, sites);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                lower_operators_expr(a, sites);
            }
        }
        ExprKind::Field { object, .. } => lower_operators_expr(object, sites),
        ExprKind::Func(fe) => lower_operators_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_operators_expr(scrutinee, sites);
            for arm in arms {
                lower_operators_expr(&mut arm.body, sites); if let Some(g) = &mut arm.guard { lower_operators_expr(g, sites); }
            }
        }
        ExprKind::Try(inner) => lower_operators_expr(inner, sites),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_operators_expr(cond, sites);
            lower_operators_block(then_branch, sites);
            if let Some(e) = else_branch {
                lower_operators_expr(e, sites);
            }
        }
        ExprKind::While { cond, body } => {
            lower_operators_expr(cond, sites);
            lower_operators_block(body, sites);
        }
        ExprKind::Block(b) => lower_operators_block(b, sites),
        _ => {}
    }
}

// =====================================================================
// Bajada de bounds: diccionarios (M9.2)
// =====================================================================
//
// Un bound `T: Trait` se baja a **paso de diccionarios**: la función gana un parámetro
// función por método del trait, y cada sitio de llamada pasa el diccionario adecuado
// (el método de un impl concreto, o el reenvío del diccionario propio). Todo son valores
// función que el runtime ya sabe pasar/llamar (M4): cero cambios en los motores.

/// Añade a cada función con bounds sus **parámetros-diccionario** (M9.2), al final de la
/// lista de parámetros, en el orden canónico (bounds en orden; por bound, los métodos del
/// trait en orden) que casa con el de los argumentos en los sitios de llamada.
pub(super) fn append_dict_params(program: &mut Program) {
    let trait_sigs: HashMap<String, Vec<MethodSig>> = program.traits.iter()
        .map(|t| (t.name.clone(), t.methods.clone()))
        .collect();
    for f in &mut program.functions {
        if f.bounds.is_empty() {
            continue;
        }
        for (tp, trait_name) in &f.bounds {
            let Some(methods) = trait_sigs.get(trait_name) else { continue };
            let self_ty = Type::Var(tp.clone());
            for m in methods {
                f.params.push(Param {
                    name: dict_param_name(tp, trait_name, &m.name),
                    ty: method_fn_type(m, &self_ty),
                    line: f.line,
                    col: f.col,
                });
            }
        }
    }
}

/// Sitios de llamada a funciones con bounds → **expresiones**-diccionario a añadir como
/// argumentos (M9.2b). En M9.2 eran simples nombres (`Ident`); con impls genéricos acotados un
/// diccionario puede ser un **closure** que captura los diccionarios internos (anidados).
// COLA por sitio (M93.5): una cadena del MISMO método acotado (`obj().field(a).field(b)`)
// comparte (línea, col, nombre) — el parser arranca cada Call en el callee, o sea en el inicio
// de la cadena. Con un valor único, el último registro pisaba a los anteriores y todas las
// llamadas recibían el diccionario de la última (T equivocado → despacho roto). El checker
// registra en orden de evaluación (receptor primero) y la bajada es post-orden (hijos primero):
// mismos órdenes → encolar/desencolar empareja cada llamada con SUS diccionarios.
pub(super) type DictSites = HashMap<(usize, usize, String), std::collections::VecDeque<Vec<Expr>>>;

/// Añade en cada **sitio de llamada** a una función con bounds los argumentos-diccionario
/// registrados (M9.2). Reescribe `f(args)` → `f(args, dicts...)`. Corre **tras** `lower_ufcs`
/// (el callee ya es un `Ident`), reusando la clave `(línea, col, nombre)`.
pub(super) fn lower_dict_calls(program: &mut Program, sites: &mut DictSites) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_dict_calls_block(&mut f.body, sites);
    }
}

pub(super) fn lower_dict_calls_block(block: &mut Block, sites: &mut DictSites) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_dict_calls_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_dict_calls_expr(start, sites); lower_dict_calls_expr(end, sites); }
                    ForIter::In(e) => lower_dict_calls_expr(e, sites),
                    ForIter::Iter { expr, .. } => lower_dict_calls_expr(expr, sites),
                }
                lower_dict_calls_block(body, sites);
            }
            StmtKind::Assign { target, value } => {
                lower_dict_calls_expr(target, sites);
                lower_dict_calls_expr(value, sites);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    lower_dict_calls_expr(v, sites);
                }
            }
            StmtKind::Expr(e) => lower_dict_calls_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_dict_calls_expr(t, sites);
    }
}

pub(super) fn lower_dict_calls_expr(expr: &mut Expr, sites: &mut DictSites) {
    // Recorrer primero los hijos (el receptor y los argumentos pueden ser otras llamadas).
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_dict_calls_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => {
            lower_dict_calls_expr(left, sites);
            lower_dict_calls_expr(right, sites);
        }
        ExprKind::Call { callee, args } => {
            lower_dict_calls_expr(callee, sites);
            for a in args.iter_mut() {
                lower_dict_calls_expr(a, sites);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { lower_dict_calls_expr(k, sites); lower_dict_calls_expr(v, sites); }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                lower_dict_calls_expr(e, sites);
            }
        }
        ExprKind::Index { array, index } => {
            lower_dict_calls_expr(array, sites);
            lower_dict_calls_expr(index, sites);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                lower_dict_calls_expr(e, sites);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                lower_dict_calls_expr(a, sites);
            }
        }
        ExprKind::Field { object, .. } => lower_dict_calls_expr(object, sites),
        ExprKind::Func(fe) => lower_dict_calls_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_dict_calls_expr(scrutinee, sites);
            for arm in arms {
                lower_dict_calls_expr(&mut arm.body, sites); if let Some(g) = &mut arm.guard { lower_dict_calls_expr(g, sites); }
            }
        }
        ExprKind::Try(inner) => lower_dict_calls_expr(inner, sites),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_dict_calls_expr(cond, sites);
            lower_dict_calls_block(then_branch, sites);
            if let Some(e) = else_branch {
                lower_dict_calls_expr(e, sites);
            }
        }
        ExprKind::While { cond, body } => {
            lower_dict_calls_expr(cond, sites);
            lower_dict_calls_block(body, sites);
        }
        ExprKind::Block(b) => lower_dict_calls_block(b, sites),
        _ => {}
    }
    // Tras recorrer los hijos, si este nodo es una llamada por nombre a una función con
    // bounds registrada en este sitio, añadir los diccionarios como argumentos extra.
    let (line, col) = (expr.line, expr.col);
    let dicts: Option<Vec<Expr>> = match &expr.kind {
        ExprKind::Call { callee, .. } => match &callee.kind {
            // Desencola: la PRIMERA entrada pendiente de este sitio (ver el comentario de
            // DictSites — las cadenas del mismo método comparten la clave).
            ExprKind::Ident(name) => sites.get_mut(&(line, col, name.clone())).and_then(|q| q.pop_front()),
            _ => None,
        },
        _ => None,
    };
    // Los diccionarios (M9.2b: posiblemente closures anidados) se añaden ya construidos.
    if let (Some(dicts), ExprKind::Call { args, .. }) = (dicts, &mut expr.kind) {
        args.extend(dicts);
    }
}

// =====================================================================
// Bajada de trait objects (M9.3b)
// =====================================================================
//
// Un `dyn Trait` se realiza como un **struct sintetizado** `__dyn_Trait { data, métodos... }`
// (el fat value / vtable). La **coerción** concreto→objeto construye ese struct; el
// **despacho** `obj.m(args)` baja a `{ let r = obj; (r.m)(r.data, args) }`. Reusa structs +
// funciones de primera clase: el intérprete y la VM no saben de trait objects.

type CoercionMap = HashMap<(usize, usize), (Vec<String>, Vec<Expr>)>;
type DispatchSet = HashSet<(usize, usize, String)>;
type UpcastMap = HashMap<(usize, usize), Vec<String>>;

/// Nombre del struct sintetizado que realiza `dyn A + B` en runtime. El conjunto viene canónico
/// (ordenado), así que el nombre es único por conjunto. El `+` es ilegal en identificadores de
/// usuario, igual que el prefijo `__dyn_`, así que no colisiona con nada escribible.
pub(super) fn dyn_struct_name(traits: &[String]) -> String {
    format!("__dyn_{}", traits.join("+"))
}

/// Nombres de los métodos de la vtable de un `dyn A + B`, en orden canónico: por cada trait del
/// conjunto (ya ordenado), sus métodos en orden de declaración. Coincide con el orden en que
/// `coerce_to_dyn` armó las expresiones-vtable.
pub(super) fn dyn_method_names(traits: &[String], tm: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut names = Vec::new();
    for tr in traits {
        if let Some(ms) = tm.get(tr) {
            names.extend(ms.iter().cloned());
        }
    }
    names
}

pub(super) fn ident_expr(name: &str, line: usize, col: usize) -> Expr {
    Expr { kind: ExprKind::Ident(name.to_string()), line, col }
}

pub(super) fn lower_dyn(program: &mut Program, coercions: &CoercionMap, dispatch: &DispatchSet, upcasts: &UpcastMap) {
    if coercions.is_empty() && dispatch.is_empty() && upcasts.is_empty() {
        return;
    }
    // Mapa trait → nombres de métodos (en orden), para construir vtables.
    let trait_methods: HashMap<String, Vec<String>> = program.traits.iter()
        .map(|t| (t.name.clone(), t.methods.iter().map(|m| m.name.clone()).collect()))
        .collect();
    // Structs sintetizados: uno por **conjunto** distinto que aparezca en una coerción **o como
    // destino de un upcast**, con `data` + un campo función por método de la unión (la vtable). Los
    // tipos de campo son irrelevantes en runtime (erasure); el motor solo usa los nombres y el orden.
    let mut sets: Vec<Vec<String>> = coercions.values().map(|(set, _)| set.clone())
        .chain(upcasts.values().cloned())
        .collect();
    sets.sort();
    sets.dedup();
    for set in &sets {
        let mut fields = vec![("data".to_string(), Type::Unit)];
        for m in dyn_method_names(set, &trait_methods) {
            fields.push((m, Type::Unit));
        }
        program.structs.push(StructDef {
            annotations: Vec::new(),
            is_pub: false,
            name: dyn_struct_name(set),
            type_params: Vec::new(),
            bounds: Vec::new(),
            fields,
            field_lines: Vec::new(),
            line: 0,
            col: 0,
        });
    }
    let mut counter = 0usize;
    for f in &mut program.functions {
        lower_dyn_block(&mut f.body, coercions, dispatch, upcasts, &trait_methods, &mut counter);
    }
}

pub(super) fn lower_dyn_block(block: &mut Block, coercions: &CoercionMap, dispatch: &DispatchSet, upcasts: &UpcastMap, tm: &HashMap<String, Vec<String>>, counter: &mut usize) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_dyn_expr(value, coercions, dispatch, upcasts, tm, counter),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_dyn_expr(start, coercions, dispatch, upcasts, tm, counter); lower_dyn_expr(end, coercions, dispatch, upcasts, tm, counter); }
                    ForIter::In(e) => lower_dyn_expr(e, coercions, dispatch, upcasts, tm, counter),
                    ForIter::Iter { expr, .. } => lower_dyn_expr(expr, coercions, dispatch, upcasts, tm, counter),
                }
                lower_dyn_block(body, coercions, dispatch, upcasts, tm, counter);
            }
            StmtKind::Assign { target, value } => {
                lower_dyn_expr(target, coercions, dispatch, upcasts, tm, counter);
                lower_dyn_expr(value, coercions, dispatch, upcasts, tm, counter);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    lower_dyn_expr(v, coercions, dispatch, upcasts, tm, counter);
                }
            }
            StmtKind::Expr(e) => lower_dyn_expr(e, coercions, dispatch, upcasts, tm, counter),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_dyn_expr(t, coercions, dispatch, upcasts, tm, counter);
    }
}

pub(super) fn lower_dyn_expr(expr: &mut Expr, coercions: &CoercionMap, dispatch: &DispatchSet, upcasts: &UpcastMap, tm: &HashMap<String, Vec<String>>, counter: &mut usize) {
    // Recorrer los sub-nodos primero (post-orden): así los despachos/coerciones anidados
    // (en el receptor y los argumentos) ya están bajados cuando reescribimos este nodo.
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_dyn_expr(inner, coercions, dispatch, upcasts, tm, counter),
        ExprKind::Binary { left, right, .. } => {
            lower_dyn_expr(left, coercions, dispatch, upcasts, tm, counter);
            lower_dyn_expr(right, coercions, dispatch, upcasts, tm, counter);
        }
        ExprKind::Call { callee, args } => {
            lower_dyn_expr(callee, coercions, dispatch, upcasts, tm, counter);
            for a in args.iter_mut() {
                lower_dyn_expr(a, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares {
                lower_dyn_expr(k, coercions, dispatch, upcasts, tm, counter);
                lower_dyn_expr(v, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                lower_dyn_expr(e, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::Index { array, index } => {
            lower_dyn_expr(array, coercions, dispatch, upcasts, tm, counter);
            lower_dyn_expr(index, coercions, dispatch, upcasts, tm, counter);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                lower_dyn_expr(e, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                lower_dyn_expr(a, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::Field { object, .. } => lower_dyn_expr(object, coercions, dispatch, upcasts, tm, counter),
        ExprKind::Func(fe) => lower_dyn_block(&mut fe.body, coercions, dispatch, upcasts, tm, counter),
        ExprKind::Match { scrutinee, arms } => {
            lower_dyn_expr(scrutinee, coercions, dispatch, upcasts, tm, counter);
            for arm in arms {
                lower_dyn_expr(&mut arm.body, coercions, dispatch, upcasts, tm, counter); if let Some(g) = &mut arm.guard { lower_dyn_expr(g, coercions, dispatch, upcasts, tm, counter); }
            }
        }
        ExprKind::Try(inner) => lower_dyn_expr(inner, coercions, dispatch, upcasts, tm, counter),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_dyn_expr(cond, coercions, dispatch, upcasts, tm, counter);
            lower_dyn_block(then_branch, coercions, dispatch, upcasts, tm, counter);
            if let Some(e) = else_branch {
                lower_dyn_expr(e, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::While { cond, body } => {
            lower_dyn_expr(cond, coercions, dispatch, upcasts, tm, counter);
            lower_dyn_block(body, coercions, dispatch, upcasts, tm, counter);
        }
        ExprKind::Block(b) => lower_dyn_block(b, coercions, dispatch, upcasts, tm, counter),
        _ => {}
    }

    let (line, col) = (expr.line, expr.col);

    // Despacho dinámico: `obj.m(args)` → `{ let r = obj; (r.m)(r.data, args) }`.
    let dispatch_method = match &expr.kind {
        ExprKind::Call { callee, .. } => match &callee.kind {
            ExprKind::Field { name, .. } if dispatch.contains(&(line, col, name.clone())) => Some(name.clone()),
            _ => None,
        },
        _ => None,
    };
    if dispatch_method.is_some() {
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0, crate::token::Radix::Dec));
        let ExprKind::Call { callee, mut args } = taken else { crate::ice!("a dispatch site is a Call") };
        let ExprKind::Field { object, name } = callee.kind else { crate::ice!("the callee of a dispatch is a Field") };
        let tmp = format!("__dynrecv#{}", *counter);
        *counter += 1;
        let let_stmt = Stmt {
            kind: StmtKind::Let { name: tmp.clone(), ty: None, value: *object, mutable: false },
            line, col,
        };
        // (r.name)(r.data, ...args)
        let method_field = Expr {
            kind: ExprKind::Field { object: Box::new(ident_expr(&tmp, line, col)), name },
            line, col,
        };
        let mut new_args = Vec::with_capacity(args.len() + 1);
        new_args.push(Expr {
            kind: ExprKind::Field { object: Box::new(ident_expr(&tmp, line, col)), name: "data".into() },
            line, col,
        });
        new_args.append(&mut args);
        let call = Expr { kind: ExprKind::Call { callee: Box::new(method_field), args: new_args }, line, col };
        expr.kind = ExprKind::Block(Block { statements: vec![let_stmt], tail: Some(Box::new(call)), line, col, end_line: line });
    }

    // Coerción concreto→`dyn Trait`: envolver en el struct sintetizado (la vtable). Los valores
    // función de la vtable los calculó el checker con `dict_for` (M9.4) — método manglado plano o
    // closure anidado para un impl genérico acotado—, así que `dyn` funciona también sobre impls
    // genéricos. Van en el orden de los métodos del trait, igual que `tm`.
    if let Some((set, vtable)) = coercions.get(&(line, col)) {
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0, crate::token::Radix::Dec));
        let inner = Expr { kind: taken, line, col };
        let mut fields = vec![("data".to_string(), inner)];
        let names = dyn_method_names(set, tm);
        for (m, vexpr) in names.iter().zip(vtable) {
            fields.push((m.clone(), vexpr.clone()));
        }
        expr.kind = ExprKind::StructLit { name: dyn_struct_name(set), fields };
    }

    // Upcasting `dyn S1` → `dyn S2` (S2 ⊆ S1, M9.5b): reconstruir el struct menor proyectando los
    // campos del mayor. Necesita un temp porque el origen se referencia varias veces:
    // `{ let __dynup = <obj>; __dyn_S2 { data: __dynup.data, m: __dynup.m, … } }`.
    if let Some(target) = upcasts.get(&(line, col)) {
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0, crate::token::Radix::Dec));
        let source = Expr { kind: taken, line, col };
        let tmp = format!("__dynup#{}", *counter);
        *counter += 1;
        let let_stmt = Stmt {
            kind: StmtKind::Let { name: tmp.clone(), ty: None, value: source, mutable: false },
            line, col,
        };
        let mut fields = Vec::new();
        for field in std::iter::once("data".to_string()).chain(dyn_method_names(target, tm)) {
            let proj = Expr {
                kind: ExprKind::Field { object: Box::new(ident_expr(&tmp, line, col)), name: field.clone() },
                line, col,
            };
            fields.push((field, proj));
        }
        let lit = Expr { kind: ExprKind::StructLit { name: dyn_struct_name(target), fields }, line, col };
        expr.kind = ExprKind::Block(Block { statements: vec![let_stmt], tail: Some(Box::new(lit)), line, col, end_line: line });
    }
}

// =====================================================================
// V2 (bench políglota) — aplanar cadenas de concatenación de strings
// =====================================================================
//
// El checker registró la posición de cada `Add` tipado `string + string` (`concat_sites`).
// Esta pasada reescribe cada cadena `a + b + c + …` (incl. la interpolación, que el parser
// desazucara a `+`) en UNA llamada al primitivo interno `__concat(a, b, c, …)`: el compilador
// la baja al opcode `ConcatN(n)` (un String con capacidad exacta, sin los n−1 intermedios) y
// el intérprete la implementa en `eval_builtin` con la misma semántica → oráculo intacto.
// Corre la ÚLTIMA de las bajadas: las demás indexan por (línea, col) y el `Call` sintético
// comparte posición con el `Add` raíz (que a su vez la comparte con su operando izquierdo) —
// después de ellas ya no hay tablas que puedan confundirlo.

pub(super) fn lower_concat(program: &mut Program, sites: &std::collections::HashSet<(usize, usize)>) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_concat_block(&mut f.body, sites);
    }
}

fn lower_concat_block(block: &mut Block, sites: &std::collections::HashSet<(usize, usize)>) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_concat_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_concat_expr(start, sites); lower_concat_expr(end, sites); }
                    ForIter::In(e) => lower_concat_expr(e, sites),
                    ForIter::Iter { expr, .. } => lower_concat_expr(expr, sites),
                }
                lower_concat_block(body, sites);
            }
            StmtKind::Assign { target, value } => {
                lower_concat_expr(target, sites);
                lower_concat_expr(value, sites);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    lower_concat_expr(v, sites);
                }
            }
            StmtKind::Expr(e) => lower_concat_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_concat_expr(t, sites);
    }
}

fn lower_concat_expr(expr: &mut Expr, sites: &std::collections::HashSet<(usize, usize)>) {
    // ¿Es la RAÍZ de una cadena registrada? Se desmonta el spine izquierdo completo (los `Add` de
    // string anidados, también registrados), se recorre cada operando (puede contener otras cadenas,
    // p. ej. dentro de una llamada) y se reescribe a `__concat(operandos…)`.
    if let ExprKind::Binary { op: BinaryOp::Add, .. } = &expr.kind {
        if sites.contains(&(expr.line, expr.col)) {
            let kind = std::mem::replace(&mut expr.kind, ExprKind::Bool(false));
            let mut parts = Vec::new();
            collect_concat(Expr { kind, line: expr.line, col: expr.col }, sites, &mut parts);
            for p in &mut parts {
                lower_concat_expr(p, sites);
            }
            let callee = Box::new(Expr { kind: ExprKind::Ident("__concat".to_string()), line: expr.line, col: expr.col });
            expr.kind = ExprKind::Call { callee, args: parts };
            return;
        }
    }
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } | ExprKind::Try(inner) => {
            lower_concat_expr(inner, sites)
        }
        ExprKind::Binary { left, right, .. } => {
            lower_concat_expr(left, sites);
            lower_concat_expr(right, sites);
        }
        ExprKind::Call { callee, args } => {
            lower_concat_expr(callee, sites);
            for a in args {
                lower_concat_expr(a, sites);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                lower_concat_expr(e, sites);
            }
        }
        ExprKind::MapLit(pairs) => {
            for (k, v) in pairs {
                lower_concat_expr(k, sites);
                lower_concat_expr(v, sites);
            }
        }
        ExprKind::Index { array, index } => {
            lower_concat_expr(array, sites);
            lower_concat_expr(index, sites);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                lower_concat_expr(e, sites);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                lower_concat_expr(a, sites);
            }
        }
        ExprKind::Field { object, .. } => lower_concat_expr(object, sites),
        ExprKind::Func(f) => lower_concat_block(&mut f.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_concat_expr(scrutinee, sites);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    lower_concat_expr(g, sites);
                }
                lower_concat_expr(&mut arm.body, sites);
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_concat_expr(cond, sites);
            lower_concat_block(then_branch, sites);
            if let Some(e) = else_branch {
                lower_concat_expr(e, sites);
            }
        }
        ExprKind::While { cond, body } => {
            lower_concat_expr(cond, sites);
            lower_concat_block(body, sites);
        }
        ExprKind::Block(b) => lower_concat_block(b, sites),
        ExprKind::Int(..) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_)
        | ExprKind::Char(_) | ExprKind::Bytes(_) | ExprKind::Ident(_) => {}
    }
}

/// Desmonta el spine izquierdo de una cadena de `Add` de strings registrada, acumulando los
/// operandos en orden izquierda→derecha (el mismo orden de evaluación que los `Add` anidados).
fn collect_concat(e: Expr, sites: &std::collections::HashSet<(usize, usize)>, out: &mut Vec<Expr>) {
    match e.kind {
        ExprKind::Binary { op: BinaryOp::Add, left, right } if sites.contains(&(e.line, e.col)) => {
            collect_concat(*left, sites, out);
            out.push(*right);
        }
        kind => out.push(Expr { kind, line: e.line, col: e.col }),
    }
}

// =====================================================================
// V5 + D3 (bench políglota) — fusiones de llamadas al prelude
// =====================================================================
//
// Reescrituras guardadas por `PreludeOrigin` (solo si las piezas implicadas son las del prelude,
// sin overrides del usuario), en UNA pasada:
//
// - **V5 sort**: tras `lower_dict_calls`, `sort(a, <prim>#less)` (el sort genérico con el
//   diccionario de un `impl Ord` primitivo) → `__sort_prim(a)` (sort nativo de Rust; el del
//   prelude paga 2 clones de String por comparación vía `Index`). `float` fuera (NaN).
// - **D3 unwrap_or**: `index_of(s, sub).unwrap_or(d)` y `parse_int(s).unwrap_or(d)` — que tras
//   las bajadas son `Option#unwrap_or(index_of(s, sub), d)` — → `__index_of_or(s, sub, d)` /
//   `__parse_int_or(s, d)`: muere el arreglo etiquetado del primitivo, el Option del wrapper y
//   los marcos de ambas llamadas (~3 allocs de heap + 2 marcos por uso; patrón P0.2 `get_or`).

pub(super) fn lower_prelude_fusions(program: &mut Program, origin: &PreludeOrigin) {
    let sort_on = origin.sort_fn && !origin.ord_prims.is_empty();
    let or_on = origin.unwrap_or_impl && (origin.index_of_fn || origin.parse_int_fn);
    if !sort_on && !or_on {
        return;
    }
    for f in &mut program.functions {
        lower_fusion_block(&mut f.body, origin);
    }
}

fn sortable_dict(d: &str, origin: &PreludeOrigin) -> bool {
    matches!(d, "int#less" | "string#less" | "char#less")
        && origin.ord_prims.contains(d.split('#').next().unwrap_or(""))
}

fn lower_fusion_block(block: &mut Block, origin: &PreludeOrigin) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_fusion_expr(value, origin),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_fusion_expr(start, origin); lower_fusion_expr(end, origin); }
                    ForIter::In(e) => lower_fusion_expr(e, origin),
                    ForIter::Iter { expr, .. } => lower_fusion_expr(expr, origin),
                }
                lower_fusion_block(body, origin);
            }
            StmtKind::Assign { target, value } => {
                lower_fusion_expr(target, origin);
                lower_fusion_expr(value, origin);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    lower_fusion_expr(v, origin);
                }
            }
            StmtKind::Expr(e) => lower_fusion_expr(e, origin),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_fusion_expr(t, origin);
    }
}

fn lower_fusion_expr(expr: &mut Expr, origin: &PreludeOrigin) {
    if let ExprKind::Call { callee, args } = &mut expr.kind {
        if let ExprKind::Ident(n) = &callee.kind {
            let n = n.clone(); // termina el préstamo: abajo se REESCRIBE callee.kind
            if n == "sort" && args.len() == 2 {
                if let ExprKind::Ident(d) = &args[1].kind {
                    if sortable_dict(d, origin) {
                        callee.kind = ExprKind::Ident("__sort_prim".to_string());
                        args.truncate(1);
                    }
                }
            }
            // D3: `Option#unwrap_or(<wrapper>(…), d)` → forma fusionada. El receptor debe ser la
            // llamada al wrapper del prelude EXACTA (aridad incluida); cualquier otro receptor
            // (otro Option) queda intacto.
            if n == "Option#unwrap_or" && args.len() == 2 && origin.unwrap_or_impl {
                let target = match &args[0].kind {
                    ExprKind::Call { callee: inner, args: iargs } => match &inner.kind {
                        ExprKind::Ident(w) if w == "index_of" && iargs.len() == 2 && origin.index_of_fn => {
                            Some("__index_of_or")
                        }
                        ExprKind::Ident(w) if w == "parse_int" && iargs.len() == 1 && origin.parse_int_fn => {
                            Some("__parse_int_or")
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(t) = target {
                    // Desmonta `[wrapper(args…), d]` → `t(args…, d)` (mismo orden de evaluación).
                    let Some(d) = args.pop() else { crate::ice!("unwrap_or carries the default") };
                    let Some(recv) = args.pop() else { crate::ice!("unwrap_or carries the receiver") };
                    let ExprKind::Call { args: mut iargs, .. } = recv.kind else {
                        crate::ice!("the fusion pattern guarantees a call receiver");
                    };
                    iargs.push(d);
                    callee.kind = ExprKind::Ident(t.to_string());
                    *args = iargs;
                }
            }
        }
    }
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } | ExprKind::Try(inner) => {
            lower_fusion_expr(inner, origin)
        }
        ExprKind::Binary { left, right, .. } => {
            lower_fusion_expr(left, origin);
            lower_fusion_expr(right, origin);
        }
        ExprKind::Call { callee, args } => {
            lower_fusion_expr(callee, origin);
            for a in args {
                lower_fusion_expr(a, origin);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                lower_fusion_expr(e, origin);
            }
        }
        ExprKind::MapLit(pairs) => {
            for (k, v) in pairs {
                lower_fusion_expr(k, origin);
                lower_fusion_expr(v, origin);
            }
        }
        ExprKind::Index { array, index } => {
            lower_fusion_expr(array, origin);
            lower_fusion_expr(index, origin);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                lower_fusion_expr(e, origin);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                lower_fusion_expr(a, origin);
            }
        }
        ExprKind::Field { object, .. } => lower_fusion_expr(object, origin),
        ExprKind::Func(f) => lower_fusion_block(&mut f.body, origin),
        ExprKind::Match { scrutinee, arms } => {
            lower_fusion_expr(scrutinee, origin);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    lower_fusion_expr(g, origin);
                }
                lower_fusion_expr(&mut arm.body, origin);
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_fusion_expr(cond, origin);
            lower_fusion_block(then_branch, origin);
            if let Some(e) = else_branch {
                lower_fusion_expr(e, origin);
            }
        }
        ExprKind::While { cond, body } => {
            lower_fusion_expr(cond, origin);
            lower_fusion_block(body, origin);
        }
        ExprKind::Block(b) => lower_fusion_block(b, origin),
        ExprKind::Int(..) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_)
        | ExprKind::Char(_) | ExprKind::Bytes(_) | ExprKind::Ident(_) => {}
    }
}
