//! Auxiliares libres del checker (movimiento puro; usar `git log --follow`).
//!
//! Helpers puros sin estado: comparabilidad/hashabilidad de tipos, `subst`/`unify` (la
//! inferencia por unificación de M6), y clasificación de divergencia (`block_diverges`/
//! `expr_diverges`, M13.2a/M13.3b: un brazo que termina en `return`/`panic` cede el tipo).

use super::*;

// ----- Auxiliares libres -----

/// ¿Pueden compararse con == / != valores de este tipo? (Compuestos: estructural.)
/// Las funciones **no** son comparables (no tienen identidad estructural); un
/// arreglo lo es solo si su elemento lo es.
/// ¿Es `t` un tipo válido como **clave** de un `Map` (M13.1)? Primitivos hashables
/// (int/string/char/bool/**bytes**; **no** float — no es hashable de forma fiable), o un parámetro de
/// tipo genérico (la restricción real se comprueba al instanciarlo con un tipo concreto).
pub(super) fn is_hashable_key(t: &Type) -> bool {
    // `bytes` (diferido de M16): secuencia inmutable de octetos → Hash/Eq/Ord fiables, como un string.
    matches!(t, Type::Int | Type::String | Type::Char | Type::Bool | Type::Bytes | Type::Var(_))
}

/// ¿Es `e` un valor válido para una constante (M27.5)? Un literal, o un literal numérico negado (`-5`).
pub(super) fn is_const_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(..) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_)
        | ExprKind::Char(_) | ExprKind::Bytes(_) => true,
        ExprKind::Unary { op: UnaryOp::Neg, expr } => {
            matches!(expr.kind, ExprKind::Int(..) | ExprKind::Float(_))
        }
        _ => false,
    }
}

pub(super) fn is_comparable(t: &Type) -> bool {
    match t {
        // M16.1a: `bytes` se compara con `==` (igualdad estructural de octetos).
        // M28.3: los enteros sin signo con tamaño se comparan con `==`.
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Bytes | Type::UInt(_) | Type::Struct(_, _) => true,
        // M41.4b: un `ptr` se compara con == por identidad (misma dirección foránea).
        Type::Ptr => true,
        Type::Array(elem) => is_comparable(elem),
        // M27.1: una tupla es comparable con == si todos sus elementos lo son (igualdad posición a posición).
        Type::Tuple(ts) => ts.iter().all(is_comparable),
        // Un Map (M13.1) no se compara con == por ahora (como los enums); se consulta.
        Type::Map(_, _) => false,
        // Los enums (M5) no se comparan con ==: pueden ser recursivos y portar
        // funciones; se consumen por `match`. (Un `@derive(Eq)` futuro lo abriría.)
        // Un parámetro de tipo (M6) es opaco: podría ser una función o un enum, así
        // que no se puede comparar dentro de código genérico.
        // `Self` (M9) no debería llegar aquí (se sustituye por el tipo concreto), pero
        // como tipo abstracto no es comparable. Un trait object (M9.3b) tampoco.
        // Un canal (M12.1) no se compara con == (se comunica, no se inspecciona). Una Task (M12.3) tampoco.
        Type::Unit | Type::Fn(_, _) | Type::Enum(_, _) | Type::Var(_) | Type::SelfType | Type::Dyn(_) | Type::Channel(_) | Type::Task(_) => false,
    }
}

/// ¿El tipo contiene algún parámetro de tipo `Var` sin resolver? (M6.2: si lo tiene,
/// no sirve como tipo "esperado" concreto.)
pub(super) fn type_has_var(t: &Type) -> bool {
    match t {
        Type::Var(_) => true,
        Type::Array(e) => type_has_var(e),
        Type::Map(k, v) => type_has_var(k) || type_has_var(v),
        Type::Channel(t) => type_has_var(t),
        Type::Task(t) => type_has_var(t),
        Type::Fn(ps, r) => ps.iter().any(type_has_var) || type_has_var(r),
        Type::Struct(_, args) | Type::Enum(_, args) => args.iter().any(type_has_var),
        _ => false,
    }
}

/// Siembra `σ` a partir del tipo esperado (M6.2): si se espera `Nombre<a, b, ...>` con
/// la aridad correcta, liga cada parámetro de tipo con su argumento esperado. Así
/// `Caja.Vacia` con tipo esperado `Caja<int>` fija `T = int`.
/// Higiene de la inferencia de construcción (M40.2e): renombra los parámetros de tipo del tipo
/// construido a nombres frescos (`$ctor$i`, ilegales para el usuario) para que **no colisionen con
/// parámetros de tipo rígidos del ámbito**. Sin esto, `fn f<T>() -> Option<(int, T)> { Option.Some(
/// (0, x)) }` confunde el `T` de `Option` con el `T` de `f` y liga `T := (int, T)` (occurs-check
/// falso). Devuelve `(tparams frescos, tipos con los params renombrados)` en el mismo orden (los
/// argumentos de tipo resultantes siguen la posición, así que los bounds usan los nombres originales).
pub(super) fn freshen_ctor_params(tparams: &[String], types: &[Type], in_scope: &HashSet<String>) -> (Vec<String>, Vec<Type>) {
    // Solo se renombran los parámetros que **colisionan** con un parámetro de tipo del ámbito; sin
    // colisión no se toca nada, así los mensajes de error conservan los nombres originales (`'A'`).
    if !tparams.iter().any(|t| in_scope.contains(t)) {
        return (tparams.to_vec(), types.to_vec());
    }
    let mut ren: HashMap<String, Type> = HashMap::new();
    let fresh: Vec<String> = tparams.iter().enumerate().map(|(i, t)| {
        if in_scope.contains(t) {
            let f = format!("$ctor${}", i);
            ren.insert(t.clone(), Type::Var(f.clone()));
            f
        } else {
            t.clone()
        }
    }).collect();
    let types = types.iter().map(|t| subst(t, &ren)).collect();
    (fresh, types)
}

pub(super) fn seed_sigma_from_expected(expected: Option<&Type>, name: &str, tparams: &[String]) -> HashMap<String, Type> {
    let mut sigma = HashMap::new();
    if let Some(Type::Struct(en, eargs) | Type::Enum(en, eargs)) = expected {
        if en == name && eargs.len() == tparams.len() {
            for (tp, ea) in tparams.iter().zip(eargs) {
                sigma.insert(tp.clone(), ea.clone());
            }
        }
    }
    sigma
}

/// **Sustitución** (M6): reemplaza cada `Var(n)` por `σ[n]`, recursivamente. Es cómo
/// se instancia un tipo genérico una vez inferidos sus parámetros: `subst([U], {U↦int})
/// = [int]`.
pub(super) fn subst(ty: &Type, sigma: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Var(n) => sigma.get(n).cloned().unwrap_or_else(|| ty.clone()),
        Type::Array(e) => Type::Array(Box::new(subst(e, sigma))),
        Type::Map(k, v) => Type::Map(Box::new(subst(k, sigma)), Box::new(subst(v, sigma))),
        Type::Channel(t) => Type::Channel(Box::new(subst(t, sigma))),
        Type::Task(t) => Type::Task(Box::new(subst(t, sigma))),
        Type::Fn(ps, r) => Type::Fn(
            ps.iter().map(|p| subst(p, sigma)).collect(),
            Box::new(subst(r, sigma)),
        ),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| subst(t, sigma)).collect()),
        // Tipos nominales: sustituir sus argumentos de tipo (M6.2).
        Type::Struct(n, args) => Type::Struct(n.clone(), args.iter().map(|a| subst(a, sigma)).collect()),
        Type::Enum(n, args) => Type::Enum(n.clone(), args.iter().map(|a| subst(a, sigma)).collect()),
        // Primitivos: nada que sustituir.
        other => other.clone(),
    }
}

/// **Unificación** (M6), asimétrica: `param` viene de la firma de la función llamada
/// (sus `Var` son las **incógnitas** a inferir); `arg` viene del contexto del llamador
/// (sus `Var`, si los hay, son **rígidos**/opacos). Liga las incógnitas en `σ` y exige
/// consistencia; cualquier desacuerdo es un error con su razón.
pub(super) fn unify(param: &Type, arg: &Type, sigma: &mut HashMap<String, Type>) -> Result<(), String> {
    // Incógnita del lado de la firma: ligarla (o exigir que coincida con lo ya ligado).
    if let Type::Var(n) = param {
        if let Some(prev) = sigma.get(n) {
            if prev != arg {
                return Err(format!("'{}' cannot be {} and {} at the same time", n, prev, arg));
            }
        } else {
            sigma.insert(n.clone(), arg.clone());
        }
        return Ok(());
    }
    match (param, arg) {
        (Type::Array(a), Type::Array(b)) => unify(a, b, sigma),
        (Type::Map(k1, v1), Type::Map(k2, v2)) => {
            unify(k1, k2, sigma)?;
            unify(v1, v2, sigma)
        }
        (Type::Channel(a), Type::Channel(b)) => unify(a, b, sigma),
        (Type::Task(a), Type::Task(b)) => unify(a, b, sigma),
        (Type::Fn(p1, r1), Type::Fn(p2, r2)) => {
            if p1.len() != p2.len() {
                return Err(format!("expected {}, got {}", param, arg));
            }
            for (a, b) in p1.iter().zip(p2) {
                unify(a, b, sigma)?;
            }
            unify(r1, r2, sigma)
        }
        // Tuplas (M27.1): misma aridad y unificar posición a posición (habilita genéricos sobre
        // tuplas, p. ej. `Iter<(int, T)>` de `enumerate`).
        (Type::Tuple(t1), Type::Tuple(t2)) if t1.len() == t2.len() => {
            for (a, b) in t1.iter().zip(t2) {
                unify(a, b, sigma)?;
            }
            Ok(())
        }
        // Tipos nominales: mismo nombre y unificar sus argumentos de tipo (M6.2), p.
        // ej. `Caja<T>` contra `Caja<int>` liga `T = int`.
        (Type::Struct(n1, a1), Type::Struct(n2, a2)) | (Type::Enum(n1, a1), Type::Enum(n2, a2))
            if n1 == n2 && a1.len() == a2.len() =>
        {
            for (a, b) in a1.iter().zip(a2) {
                unify(a, b, sigma)?;
            }
            Ok(())
        }
        // Resto (primitivos, Var rígido del llamador): igualdad exacta.
        _ if param == arg => Ok(()),
        _ => Err(format!("expected {}, got {}", param, arg)),
    }
}

pub(super) fn bin_op_str(op: BinaryOp) -> &'static str {
    use BinaryOp::*;
    match op {
        Add => "+", Sub => "-", Mul => "*", Div => "/", Rem => "%",
        Eq => "==", Ne => "!=", Lt => "<", Le => "<=", Gt => ">", Ge => ">=",
        And => "&&", Or => "||",
        BitAnd => "&", BitOr => "|", BitXor => "^", Shl => "<<", Shr => ">>",
    }
}

/// Análisis de divergencia: ¿todos los caminos de este bloque terminan en `return`?
/// Es una aproximación *conservadora* (sólida): si dice `true`, es seguro que el
/// bloque siempre retorna; si dice `false`, puede que sí o que no. Eso basta para
/// permitir omitir la expresión final cuando el cuerpo ya retorna por todas partes.
pub(super) fn block_diverges(block: &Block) -> bool {
    block.statements.iter().any(stmt_diverges)
        || block.tail.as_ref().is_some_and(|t| expr_diverges(t))
}

pub(super) fn stmt_diverges(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Return { .. } => true,
        StmtKind::Expr(e) => expr_diverges(e),
        _ => false,
    }
}

/// ¿El patrón es **irrefutable** (casa siempre, sin importar el valor)? Un `_`/binding, o un patrón
/// de struct cuyos campos son todos irrefutables (`Punto { x, y }`). Una variante es **refutable**
/// (solo casa una de las variantes). Se usa para la exhaustividad conservadora (M40.1c/1d): una
/// variante de primer nivel cubre solo si sus sub-patrones son irrefutables.
pub(super) fn is_irrefutable(p: &Pattern) -> bool {
    match &p.kind {
        PatternKind::Wildcard | PatternKind::Binding(_) => true,
        PatternKind::Struct { fields, .. } => fields.iter().all(|(_, f)| is_irrefutable(f)),
        PatternKind::Variant { .. } => false,
    }
}

pub(super) fn expr_diverges(expr: &Expr) -> bool {
    match &expr.kind {
        // Un if diverge solo si AMBAS ramas divergen (si falta el else, puede caer).
        ExprKind::If { then_branch, else_branch: Some(els), .. } => {
            block_diverges(then_branch) && expr_diverges(els)
        }
        ExprKind::Block(b) => block_diverges(b),
        // Un match diverge si TODOS sus brazos divergen (el checker garantiza que es
        // exhaustivo, así que siempre se toma alguno).
        ExprKind::Match { arms, .. } => !arms.is_empty() && arms.iter().all(|a| expr_diverges(&a.body)),
        // `panic(...)` (M13.2a) nunca retorna: una rama que termina en panic diverge, así que
        // `match (x) { Some(v) => v, None => panic("imposible") }` cuadra de tipo. `panic` gana
        // siempre sobre cualquier homónimo (un builtin no se tapa), así que el chequeo por nombre
        // es seguro.
        ExprKind::Call { callee, .. } => matches!(&callee.kind, ExprKind::Ident(n) if n == "panic"),
        _ => false,
    }
}
