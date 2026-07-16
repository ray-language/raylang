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
    BinaryOp, Block, Expr, ExprKind, ForIter, ForPat, Function, MatchArm, Pattern, PatternKind, Program,
    Stmt, StmtKind, Type, UnaryOp,
};
use std::collections::HashMap;
use std::fmt::Write;

/// Firma de una función del usuario: params, retorno y sus parámetros de tipo (para inferir las
/// llamadas genéricas por unificación).
struct FnSig {
    params: Vec<Type>,
    ret: Type,
    tparams: Vec<String>,
}

/// Convierte un nombre de raylang en un identificador Rust válido: los métodos de trait bajados por el
/// checker llevan `#` (`Punto#show`) y los módulos `::` (`m::f`), ilegales en Rust. Identidad para los
/// nombres normales. Los traits son ERASURE (M9): los métodos son funciones ordinarias tras el bajado.
fn mangle(name: &str) -> String {
    if name == "self" {
        return "__self".to_string(); // `self` es palabra reservada de Rust fuera de un método
    }
    // `$` lo usan los temporales sintéticos del checker (p. ej. el bind del `?` con From-conversion,
    // `$to`/`$te`) → no es identificador Rust válido.
    name.replace('#', "_HH_").replace("::", "_CC_").replace('+', "_P_").replace('$', "_D_")
}

/// ¿Es un método de un impl del PRELUDE sobre un tipo builtin (`[]#len`, `string#trim`, `int#show`)?
/// = clave de tipo builtin Y método del prelude. Su método lo maneja el transpilador directamente → se
/// salta. Un impl de USUARIO sobre un builtin (`int#valor`) NO se salta (método no-prelude → se emite).
fn is_prelude_impl(name: &str) -> bool {
    if !name.contains('#') {
        return false;
    }
    let key = name.split('#').next().unwrap_or("");
    let method = name.rsplit('#').next().unwrap_or("");
    // Ord (less) → manejado directo (`<`): saltar en CUALQUIER tipo. `eq`/`show` NO se saltan aquí: se
    // emiten (impl derivado/custom `Tipo#eq`/`Tipo#show` + prelude `int#eq`/`int#show`), realizando el
    // bound `T: Eq`/`T: Show` por diccionarios. `print`/`to_string` siguen usando RayShow (render default);
    // solo `.eq()`/`.show()` EXPLÍCITOS llaman al impl (que puede ser custom, p. ej. `impl Show for Vec2`).
    if method == "less" {
        return true;
    }
    // (Los métodos `Iter#*` del protocolo de iterador SÍ se emiten desde B2.)
    // `eq`/`show` se EMITEN (realizan el bound `T: Eq`/`T: Show` por diccionarios) para tipos de usuario y
    // primitivos ESCALARES (`int#eq` = `self == other`, `int#show` = `to_string(self)`); para contenedores
    // ([], Map, Channel, Task, bytes, unit…) se salta: clave no-identificador o impl no transpilable.
    if matches!(method, "eq" | "show") {
        return matches!(
            key,
            "bytes" | "uint" | "u8" | "u32" | "u64" | "unit" | "[]" | "Map" | "Channel" | "Task"
        );
    }
    let builtin_key = matches!(
        key,
        "int" | "float" | "bool" | "char" | "string" | "bytes" | "uint" | "u8" | "u32" | "u64"
            | "unit" | "[]" | "Map" | "Channel" | "Task"
    );
    let prelude_method = matches!(
        method,
        "len" | "push" | "reverse" | "contains" | "trim" | "split" | "replace" | "chars" | "starts_with"
            | "ends_with" | "to_upper" | "to_lower" | "substring" | "repeat" | "join" | "to_bytes"
            | "sub_bytes" | "char_code" | "to_string" | "insert" | "contains_key" | "keys" | "values"
            | "get" | "get_or" | "remove" | "add_to" | "less" | "index_of" | "position" | "pop"
    );
    builtin_key && prelude_method
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

/// ¿Es una función del PRELUDE o un builtin que el transpilador maneja directamente? Sus definiciones
/// inyectadas por el checker se SALTAN (el transpilador las mapea a Rust nativo, o no las soporta y su
/// cuerpo referiría builtins ausentes). Lista extraída de `src/prelude.ray` + los builtins públicos.
fn is_handled_builtin(name: &str) -> bool {
    // `std::math::*`/`std::fs::*` se interceptan en emit_call/type_of (→ Rust nativo); no emitimos sus
    // wrappers del módulo (llaman a primitivos `__sqrt`/`__read_file`… ausentes).
    if name.starts_with("std::math::") || name.starts_with("std::fs::") {
        return true;
    }
    // De `std/time` y `std/random` SOLO se saltan las funciones que envuelven un primitivo (interceptadas);
    // el resto (p. ej. `std::time::to_epoch_millis`, helpers de `DateTime`) son raylang puro → se emiten.
    if matches!(name, "std::time::now" | "std::time::monotonic" | "std::time::sleep")
        || matches!(name, "std::random::next" | "std::random::below" | "std::random::seed")
    {
        return true;
    }
    // Sockets TCP de `std/net` (interceptados en emit_call → std::net de Rust). TLS y demás quedan fuera.
    if matches!(
        name,
        "std::net::tcp_connect" | "std::net::tcp_listen" | "std::net::tcp_accept" | "std::net::socket_read"
            | "std::net::socket_read_bytes" | "std::net::socket_write" | "std::net::socket_write_bytes"
            | "std::net::local_port" | "std::net::set_read_timeout"
    ) {
        return true;
    }
    matches!(
        name,
        // --- funciones del prelude (src/prelude.ray) ---
        "all" | "any" | "assert" | "assert_eq" | "char_from_code" | "env" | "filter" | "fold"
            | "from_utf" | "from_utf8" | "get" | "get_or" | "index_of" | "input" | "map" | "max" | "min"
            | "parse_float" | "parse_int" | "pop" | "position" | "read_int" | "recv"
            | "remove" | "sort" | "try_join"
        // --- builtins públicos manejados en emit_call ---
            | "len" | "push" | "split" | "join" | "chars" | "to_string" | "print" | "eprint"
            | "contains_key" | "keys" | "values" | "insert" | "add_to" | "unwrap" | "unwrap_or"
            | "panic"
    )
}

/// ¿Se salta la DEFINICIÓN de esta función al registrarla/emitirla? Sí para las sintéticas (`__`),
/// los impls del prelude (`int#eq`…) y los builtins manejados. Matiz del override: un builtin con `::`
/// (`std::fs::*`) envuelve un primitivo → siempre se salta; un builtin del prelude de nombre pelado
/// (`map`/`get_or`/`sort`…) se salta SOLO si viene del prelude (`line >= LINE_BASE`). Si el usuario lo
/// **redefine** (línea de usuario, por debajo de la banda del prelude), es una función de usuario y debe
/// emitirse (override), o su llamada quedaría sin destino (p. ej. un `get_or(m, k)` de 2 args propio).
fn skip_fn_def(f: &Function) -> bool {
    if f.name.starts_with("__") || is_prelude_impl(&f.name) {
        return true;
    }
    if is_handled_builtin(&f.name) {
        return f.name.contains("::") || f.line >= crate::prelude::LINE_BASE;
    }
    false
}

/// ¿Es un tipo primitivo que implementa `Display` en Rust (i64/f64/bool/char/Rc<str>)? Los demás
/// (bytes, struct, enum, array, Map, tupla, función) no → su `to_string` debe pasar por `ray_show`.
fn is_display_primitive(t: &Type) -> bool {
    matches!(
        normalize_type(t),
        Type::Int | Type::Float | Type::Bool | Type::Char | Type::String
    )
}

/// ¿La callee es una llamada a `to_string` (libre o método `x.to_string()`, posiblemente manglada)?
fn is_to_string(callee: &Expr) -> bool {
    match resolve_callee(callee) {
        Ok((n, _)) => n.rsplit('#').next().unwrap_or(n).trim_start_matches("__") == "to_string",
        Err(_) => false,
    }
}

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

/// Recoge TODOS los nombres de identificador que aparecen en `e` (descendiendo también a los cuerpos
/// de closures) → lo que una closure "referencia" (candidatos a captura).
fn idents_of_expr(e: &Expr, out: &mut std::collections::HashSet<String>) {
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

fn idents_of_block(b: &Block, out: &mut std::collections::HashSet<String>) {
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
fn captured_idents_expr(e: &Expr, out: &mut std::collections::HashSet<String>) {
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

fn captured_idents_block(b: &Block, out: &mut std::collections::HashSet<String>) {
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
fn mut_var_decls_block(b: &Block, out: &mut std::collections::HashSet<String>) {
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
fn mut_var_decls_expr(e: &Expr, out: &mut std::collections::HashSet<String>) {
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
fn cell_vars(body: &Block) -> std::collections::HashSet<String> {
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

struct Transpiler {
    funcs: HashMap<String, FnSig>,
    /// Pila de ámbitos: nombre de variable → su tipo (para decidir clonado y para la inferencia de `let`).
    scopes: Vec<HashMap<String, Type>>,
    /// Nombres de enum del usuario (para clasificar un `Type::Struct(n)` como struct vs enum).
    enums: std::collections::HashSet<String>,
    /// Campos de cada struct (nombre → tipo), en orden, para inferir el tipo de `p.campo`.
    struct_fields: HashMap<String, Vec<(String, Type)>>,
    /// Parámetros de tipo de cada struct/enum (para sustituir en `Caja<int>`).
    struct_tparams: HashMap<String, Vec<String>>,
    enum_tparams: HashMap<String, Vec<String>>,
    /// Payload de cada variante de enum (`Enum` → `Variante` → tipos), para los bindings de `match`.
    enum_variants: HashMap<String, HashMap<String, Vec<Type>>>,
    /// Contador para nombres de temporales de escrutinio de `match` (evita colisiones al anidar).
    match_temp: usize,
    /// Constantes de nivel superior (nombre → tipo). Se bajan a funciones `NAME()` (uniforme para
    /// escalares y strings, que no pueden ser `const` en Rust por el `Rc`); una referencia `NAME` → `NAME()`.
    consts: HashMap<String, Type>,
    /// Firma de cada método de trait (nombre → (tipos de args SIN `self`, tipo de retorno)). Para bajar
    /// `dyn Trait`: el struct sintetizado `__dyn_T` lleva un campo-closure por método (`Rc<dyn Fn(..)->R>`).
    trait_method_sigs: HashMap<String, (Vec<Type>, Type)>,
    /// Parámetros de tipo de la función genérica en curso (p. ej. `{T, U}`): un `Struct(n)` con `n` aquí
    /// es un tipo VARIABLE → se emite como el genérico `n` de Rust (no como un struct de usuario).
    tparams: std::collections::HashSet<String>,
    /// ¿El programa usa handles de archivo (`open`/`read_line`/`write`/`close`)? Se activa al emitirlos;
    /// si es cierto, se anexa al final el registro global de handles (espejo del `FileRegistry` de la VM).
    needs_handles: bool,
    /// ¿Usa concurrencia (`spawn`/canales)? Si es cierto, se anexa el runtime de canales MPMC.
    needs_concurrency: bool,
    /// ¿Usa `signals()`? Si es cierto, se anexa el runtime de señales del SO (self-pipe + FFI a libc).
    needs_signals: bool,
    /// ¿Usa `std::time::monotonic`/`std::random::*`? Si es cierto, se anexa el PRNG (SplitMix64) + el
    /// reloj monotónico (necesitan estado global; `now`/`sleep` son inline y no lo activan).
    needs_time_rng: bool,
    /// ¿Usa sockets TCP (`std::net::*`)? Comparte el registro de handles con los archivos y añade los ops
    /// de socket (`std::net::TcpStream`/`TcpListener`).
    needs_net: bool,
    /// ¿Usa cripto de producción (`__sha256`/`__hmac_sha256`/`__ed25519_*`/`__chacha20poly1305_*`/…)? Se
    /// interceptan a `ray_runtime::crypto::*` → el binario nativo llama al MISMO código que la VM (ring).
    /// Activa la feature `crypto` de `ray-runtime` → `build_native` genera un proyecto Cargo (no rustc pelado).
    needs_rt_crypto: bool,
    /// ¿Usa TLS (`__tls_connect`/`__tls_connect_h2`/`__tls_accept`/`__tls_upgrade`)? El binario transpilado
    /// hace I/O TLS **bloqueante** (hilos reales; `ray_runtime::tls::TlsStream` sobre `StreamOwned`). Añade
    /// la variante `Tls` al registro de handles inline y el despacho en `socket_read/write`. Implica
    /// `needs_net` (registro + TcpStream). Activa la feature `tls` de `ray-runtime`.
    needs_rt_tls: bool,
    /// ¿Usa SQLite (`__sqlite_open`/`__sqlite_exec`/`__sqlite_query`)? El binario transpilado guarda cada
    /// conexión (`ray_runtime::sqlite::Conn`) en el registro de handles inline (variante `Sqlite`). I/O
    /// local → se retiene el lock global (como la VM). Activa la feature `sqlite` de `ray-runtime`.
    needs_rt_sqlite: bool,
    /// Subsistemas con-crate EXCLUIDOS por `--without` (crypto/tls/sqlite): sus builtins no se interceptan
    /// (caen en stub que panica) → el binario puede usar la vía rápida `rustc`. Ver `transpile_with`.
    exclude: std::collections::HashSet<String>,
    /// Nombres de `var` locales que van en una **celda** `Rc<RefCell<T>>` (B1): capturadas y mutadas por
    /// una closure. Se leen con `.borrow().clone()` y se escriben con `.borrow_mut()`; la closure captura
    /// un clon del `Rc`. Se pueblan al entrar en cada función/closure (con su `cell_vars`) y se quitan al
    /// salir (set plano; el shadowing de una var-celda es una limitación conocida, raro en la práctica).
    cells: std::collections::HashSet<String>,
}

/// Transpila un programa (ya chequeado) a Rust autocontenido, o un error si usa algo fuera del subconjunto.
/// El resultado de transpilar: el fuente Rust + las **features de `ray-runtime`** que el programa necesita
/// (activadas bajo demanda al interceptar un builtin que envuelve un crate). Vacío → cero deps externas →
/// `build_native` compila con `rustc` pelado (camino rápido); no vacío → genera un proyecto Cargo con
/// `ray-runtime` (esas features) y compila con `cargo`. Ver docs/transpilador-nativo.md §4.5.
pub struct Transpiled {
    pub source: String,
    pub rt_features: Vec<&'static str>,
}

/// Transpila sin excluir ningún subsistema (el caso común; lo usan `ray emit-rust` y los tests).
pub fn transpile(prog: &Program) -> Result<Transpiled, String> {
    transpile_with(prog, &[])
}

/// Transpila EXCLUYENDO los subsistemas con-crate dados (`--without crypto,tls,sqlite`): un uso de un
/// subsistema excluido NO se intercepta a `ray_runtime::*` → su función cae en un stub que panica, y el
/// binario compila por la vía rápida (`rustc` pelada) si no queda otro subsistema con-crate. Escape hatch
/// para builds herméticos/cross-compile/policy (docs/transpilador-nativo.md §3.3).
pub fn transpile_with(prog: &Program, exclude: &[String]) -> Result<Transpiled, String> {
    // Índice de firmas de funciones NO genéricas y NO sintéticas (para inferir tipos de llamada).
    let mut funcs = HashMap::new();
    for f in &prog.functions {
        if skip_fn_def(f) {
            continue;
        }
        // Se NORMALIZAN los tipos de la firma (Struct("Map"/"Channel"/"Task"/"Option"/"Result") → su
        // variante propia): el parser deja `Map<K,V>` como `Struct("Map", …)`, y `type_of` de una llamada
        // devuelve el retorno guardado → sin normalizar, `get(mkmap(), k)` veía `Struct("Map")` y fallaba.
        funcs.insert(f.name.clone(), FnSig {
            params: f.params.iter().map(|p| normalize_type(&p.ty)).collect(),
            ret: normalize_type(&f.return_type),
            tparams: f.type_params.clone(),
        });
    }
    // Funciones externas (FFI, M41): se registran como funciones ordinarias → una llamada `sqrt(2.0)`
    // resuelve al WRAPPER emitido (que marshala y llama al símbolo C por `extern "C"`).
    for e in &prog.externs {
        funcs.insert(e.name.clone(), FnSig {
            params: e.params.iter().map(|p| normalize_type(&p.ty)).collect(),
            ret: normalize_type(&e.return_type),
            tparams: Vec::new(),
        });
    }
    // Enums de USUARIO (incl. genéricos). Option/Result se excluyen: son los nativos de Rust, no se emiten.
    let enums: std::collections::HashSet<String> =
        prog.enums.iter().filter(|e| e.name != "Option" && e.name != "Result").map(|e| e.name.clone()).collect();
    let struct_fields = prog.structs.iter().map(|s| (s.name.clone(), s.fields.clone())).collect();
    let struct_tparams = prog.structs.iter().map(|s| (s.name.clone(), s.type_params.clone())).collect();
    let enum_variants = prog
        .enums
        .iter()
        .map(|e| {
            (e.name.clone(), e.variants.iter().map(|v| (v.name.clone(), v.payload.clone())).collect())
        })
        .collect();
    let enum_tparams = prog.enums.iter().map(|e| (e.name.clone(), e.type_params.clone())).collect();
    let consts = prog.consts.iter().map(|c| (c.name.clone(), c.ty.clone())).collect();
    // Firmas de los métodos de trait (self excluido) para bajar `dyn Trait`.
    let mut trait_method_sigs = HashMap::new();
    for tr in &prog.traits {
        for m in &tr.methods {
            let args: Vec<Type> = m.params.iter().skip(1).map(|p| p.ty.clone()).collect(); // skip self
            trait_method_sigs.insert(m.name.clone(), (args, m.return_type.clone()));
        }
    }
    let mut t = Transpiler {
        funcs,
        scopes: Vec::new(),
        enums,
        struct_fields,
        struct_tparams,
        enum_variants,
        enum_tparams,
        match_temp: 0,
        consts,
        trait_method_sigs,
        tparams: std::collections::HashSet::new(),
        needs_handles: false,
        needs_concurrency: false,
        needs_signals: false,
        needs_time_rng: false,
        needs_net: false,
        needs_rt_crypto: false,
        needs_rt_tls: false,
        needs_rt_sqlite: false,
        exclude: exclude.iter().cloned().collect(),
        cells: std::collections::HashSet::new(),
    };

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
    out.push_str("    Rc::<str>::from(parts.join(sep))\n}\n");
    // index_of(s, sub) -> Option<int>: índice por CARÁCTER de la primera aparición de sub (como la VM;
    // sub vacío → Some(0)). Rust `str::find` da índice de BYTE, así que se compara por char.
    out.push_str("fn __ray_index_of(s: &str, sub: &str) -> Option<i64> {\n");
    out.push_str("    let chars: Vec<char> = s.chars().collect(); let sub: Vec<char> = sub.chars().collect();\n");
    out.push_str("    if sub.is_empty() { return Some(0); }\n");
    out.push_str("    if sub.len() > chars.len() { return None; }\n");
    out.push_str("    (0..=chars.len() - sub.len()).find(|&i| chars[i..i + sub.len()] == sub[..]).map(|i| i as i64)\n}\n");
    out.push_str("use std::collections::HashMap as __RayMap;\n");
    out.push_str("fn __ray_sort<T: Ord + Clone>(a: &Rc<std::cell::RefCell<Vec<T>>>) -> Rc<std::cell::RefCell<Vec<T>>> {\n");
    out.push_str("    let mut v = a.borrow().clone(); v.sort(); Rc::new(std::cell::RefCell::new(v))\n}\n");
    // keys()/values() ORDENADAS por clave (determinista, como la VM). values() en el orden de keys().
    out.push_str("fn __ray_keys<K: Ord + Clone, V>(m: &Rc<std::cell::RefCell<__RayMap<K, V>>>) -> Rc<std::cell::RefCell<Vec<K>>> {\n");
    out.push_str("    let b = m.borrow(); let mut ks: Vec<K> = b.keys().cloned().collect(); ks.sort();\n");
    out.push_str("    Rc::new(std::cell::RefCell::new(ks))\n}\n");
    out.push_str("fn __ray_values<K: Ord + Clone + std::hash::Hash + Eq, V: Clone>(m: &Rc<std::cell::RefCell<__RayMap<K, V>>>) -> Rc<std::cell::RefCell<Vec<V>>> {\n");
    out.push_str("    let b = m.borrow(); let mut ks: Vec<K> = b.keys().cloned().collect(); ks.sort();\n");
    out.push_str("    let vs: Vec<V> = ks.iter().map(|k| b[k].clone()).collect(); Rc::new(std::cell::RefCell::new(vs))\n}\n");
    // for (k, v) in Map: pares ORDENADOS por clave (como la VM). Materializa un Vec (suelta el borrow)
    // antes del cuerpo, que podría mutar el Map.
    out.push_str("fn __ray_pairs<K: Ord + Clone + std::hash::Hash + Eq, V: Clone>(m: &Rc<std::cell::RefCell<__RayMap<K, V>>>) -> Vec<(K, V)> {\n");
    out.push_str("    let b = m.borrow(); let mut ks: Vec<K> = b.keys().cloned().collect(); ks.sort();\n");
    out.push_str("    ks.into_iter().map(|k| { let v = b[&k].clone(); (k, v) }).collect()\n}\n");
    // RayShow: el `Show` de raylang como trait propio (Display no sirve: los structs son Rc<RefCell<..>>,
    // y RefCell no es Display; además un bound genérico `T: Display` fallaría). Impl para todo tipo; los
    // structs/enums de usuario reciben su impl generado (recursivo).
    out.push_str("trait RayShow { fn ray_show(&self) -> String; }\n");
    for (ty, body) in [
        ("i64", "self.to_string()"),
        ("f64", "self.to_string()"),
        ("bool", "self.to_string()"),
        ("char", "self.to_string()"),
        ("()", "\"()\".to_string()"),
        ("Rc<str>", "self.to_string()"),
    ] {
        writeln!(out, "impl RayShow for {} {{ fn ray_show(&self) -> String {{ {} }} }}", ty, body).unwrap();
    }
    out.push_str("impl<T: RayShow> RayShow for Rc<std::cell::RefCell<Vec<T>>> { fn ray_show(&self) -> String { format!(\"[{}]\", self.borrow().iter().map(|__e| __e.ray_show()).collect::<Vec<_>>().join(\", \")) } }\n");
    // Map: `Map{k: v, …}` con los pares (renderizados) ordenados como cadena, como el Display del
    // runtime (`Value::Map`): determinista pese al HashMap. `print(map)` directo lo veta el checker,
    // pero un struct/enum que CONTENGA un Map (p. ej. `Json.JObject`) sí se renderiza recursivamente.
    out.push_str("impl<K: RayShow + std::hash::Hash + Eq, V: RayShow> RayShow for Rc<std::cell::RefCell<std::collections::HashMap<K, V>>> { fn ray_show(&self) -> String { let __m = self.borrow(); let mut __parts: Vec<String> = __m.iter().map(|(__k, __v)| format!(\"{}: {}\", __k.ray_show(), __v.ray_show())).collect(); __parts.sort(); format!(\"Map{{{}}}\", __parts.join(\", \")) } }\n");
    out.push_str("impl<T: RayShow> RayShow for Option<T> { fn ray_show(&self) -> String { match self { Some(__v) => format!(\"Option.Some({})\", __v.ray_show()), None => \"Option.None\".to_string() } } }\n");
    out.push_str("impl<T: RayShow, E: RayShow> RayShow for Result<T, E> { fn ray_show(&self) -> String { match self { Ok(__v) => format!(\"Result.Ok({})\", __v.ray_show()), Err(__e) => format!(\"Result.Err({})\", __e.ray_show()) } } }\n");
    // Tuplas (2 y 3 elementos): `(a, b)`. El checker no deja `print`ar una tupla, así que esto rara vez
    // se llama; hace falta para satisfacer el bound `T: RayShow` de un `Iter<(k, v)>` (los adaptadores
    // `enumerate`/`zip` generados por el trait Iterator, aun cuando queden como stubs).
    out.push_str("impl<A: RayShow, B: RayShow> RayShow for (A, B) { fn ray_show(&self) -> String { format!(\"({}, {})\", self.0.ray_show(), self.1.ray_show()) } }\n");
    out.push_str("impl<A: RayShow, B: RayShow, C: RayShow> RayShow for (A, B, C) { fn ray_show(&self) -> String { format!(\"({}, {}, {})\", self.0.ray_show(), self.1.ray_show(), self.2.ray_show()) } }\n");
    // bytes → hex minúsculas sin separador ({:02x} por octeto), como la VM (bytes_to_hex).
    out.push_str("impl RayShow for Rc<[u8]> { fn ray_show(&self) -> String { let mut __s = String::with_capacity(self.len() * 2); for __b in self.iter() { __s.push_str(&format!(\"{:02x}\", __b)); } __s } }\n\n");

    // Definiciones de tipos de usuario (no genéricos). struct → Rust struct; enum → Rust enum. `Clone`
    // para el clon-al-leer y para los payloads. El orden no importa (Rust permite referencias adelantadas).
    for s in &prog.structs {
        // (El struct `Iter` del protocolo de iterador del prelude SÍ se emite desde B2: es
        // `{ step: Rc<dyn Fn() -> Option<T>> }`, y `iter`/`range`/`map`/`filter` lo construyen con
        // closures que mutan su cursor capturado — transpilable desde B1.)
        t.tparams = s.type_params.iter().cloned().collect();
        // `dyn Trait` (M9.3b): struct sintetizado `__dyn_T { data, métodos… }`. Aquí, un juego de closures
        // que CAPTURAN el valor concreto (sin `data`, sin Box<dyn Any>): cada campo `Rc<dyn Fn(args)->ret>`.
        if s.name.starts_with("__dyn_") {
            writeln!(out, "#[derive(Clone)]\nstruct {} {{", mangle(&s.name)).unwrap();
            for (fname, _) in &s.fields {
                if fname == "data" {
                    continue; // el valor concreto lo capturan las closures, no se guarda aparte
                }
                let (args, ret) = t
                    .trait_method_sigs
                    .get(fname)
                    .ok_or_else(|| format!("spike: método de dyn desconocido '{}'", fname))?;
                let atys: Vec<String> =
                    args.iter().map(|a| rust_ty(a, &t.enums, &t.tparams)).collect::<Result<_, _>>()?;
                writeln!(out, "    {}: Rc<dyn Fn({}) -> {}>,", fname, atys.join(", "), rust_ty(ret, &t.enums, &t.tparams)?).unwrap();
            }
            out.push_str("}\n");
            continue;
        }
        writeln!(out, "#[derive(Clone)]\nstruct {}{} {{", mangle(&s.name), generic_decl(&s.type_params)).unwrap();
        for (fname, fty) in &s.fields {
            writeln!(out, "    {}: {},", fname, rust_ty(fty, &t.enums, &t.tparams)?).unwrap();
        }
        out.push_str("}\n");
    }
    for e in &prog.enums {
        if e.name == "Option" || e.name == "Result" {
            continue; // nativos de Rust
        }
        t.tparams = e.type_params.iter().cloned().collect();
        writeln!(out, "#[derive(Clone)]\nenum {}{} {{", mangle(&e.name), generic_decl(&e.type_params)).unwrap();
        for v in &e.variants {
            if v.payload.is_empty() {
                writeln!(out, "    {},", v.name).unwrap();
            } else {
                let tys: Vec<String> =
                    v.payload.iter().map(|t2| rust_ty(t2, &t.enums, &t.tparams)).collect::<Result<_, _>>()?;
                writeln!(out, "    {}({}),", v.name, tys.join(", ")).unwrap();
            }
        }
        out.push_str("}\n");
    }
    t.tparams.clear();
    // impls de Display (= el Show de raylang): struct `Name { f: v, … }`, enum `Name.Variant(payload)`.
    t.emit_rayshow_impls(&mut out, prog)?;
    // Constantes de nivel superior → funciones `fn NAME() -> T { <literal> }`.
    for c in &prog.consts {
        // `std::math::PI`/`E` se emiten como constantes de `std::f64::consts` en el sitio de uso.
        if c.name.starts_with("std::math::") {
            continue;
        }
        write!(out, "fn {}() -> {} {{ ", c.name, rust_ty(&c.ty, &t.enums, &t.tparams)?).unwrap();
        t.emit_expr(&mut out, &c.value)?;
        out.push_str(" }\n");
    }
    // Funciones externas (FFI, M41): declaraciones `extern "C"` + wrappers que marshalan.
    t.emit_externs(&mut out, prog)?;
    out.push('\n');

    let mut main_ret_int = false;
    let mut main_seen = false;
    for f in &prog.functions {
        if skip_fn_def(f) {
            continue;
        }
        let rust_name = if f.name == "main" { "ray_main".to_string() } else { mangle(&f.name) };
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
                // Una función no-main cuyo CUERPO no transpila se emite como STUB que panica (con su firma):
                // el programa COMPILA y, si el flujo real no la llama, corre igual que la VM. Si ni la firma
                // es representable, se OMITE (última salida; una llamada colgante haría fallar rustc).
                // `RAYLANG_TRANSPILE_DEBUG` reporta qué se convirtió en stub (u omitió) y por qué.
                let mut sbuf = String::new();
                match t.emit_stub(&mut sbuf, &rust_name, f) {
                    Ok(()) => {
                        out.push_str(&sbuf);
                        out.push('\n');
                        if std::env::var_os("RAYLANG_TRANSPILE_DEBUG").is_some() {
                            eprintln!("[transpile stub] {} — {}", f.name, e);
                        }
                    }
                    Err(se) => {
                        if std::env::var_os("RAYLANG_TRANSPILE_DEBUG").is_some() {
                            eprintln!("[transpile skip] {} — cuerpo: {} — firma: {}", f.name, e, se);
                        }
                    }
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
    // TLS reusa el registro de handles + `TcpStream` (accept/upgrade parten de un handle TCP) → implica net.
    if t.needs_rt_tls {
        t.needs_net = true;
    }
    // Registro global de handles de archivo (M11.8), solo si el programa los usa. Rust permite items
    // top-level en cualquier orden, así que va al final. Espejo del `FileRegistry` de la VM: un contador +
    // mapa handle→archivo tras un Mutex/OnceLock; los mensajes de error son byte-idénticos a la VM.
    // Registro de handles (M11.8): compartido por archivos y sockets. Se emite si el programa usa cualquiera.
    if t.needs_handles || t.needs_net || t.needs_rt_sqlite {
        // Variantes con-crate del registro, añadidas solo si el programa usa el subsistema: `Tls` (conexión
        // TLS bloqueante tras `Arc<Mutex>` propio → el I/O no retiene el lock global) y `Sqlite` (conexión
        // rusqlite; I/O local → se opera reteniendo el lock global, como la VM).
        let tls_variant = if t.needs_rt_tls {
            ", Tls(std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>)"
        } else {
            ""
        };
        let sqlite_variant = if t.needs_rt_sqlite { ", Sqlite(ray_runtime::sqlite::Conn)" } else { "" };
        writeln!(
            out,
            "enum __RayHandle {{ Reader(std::io::BufReader<std::fs::File>), Writer(std::fs::File), Tcp(std::net::TcpStream), Listener(std::net::TcpListener), Udp(std::net::UdpSocket){tls_variant}{sqlite_variant} }}"
        )
        .unwrap();
        out.push_str(concat!(
            "struct __RayReg { next: i64, open: __RayMap<i64, __RayHandle> }\n",
            "fn __ray_reg() -> &'static std::sync::Mutex<__RayReg> {\n",
            "    static R: std::sync::OnceLock<std::sync::Mutex<__RayReg>> = std::sync::OnceLock::new();\n",
            "    R.get_or_init(|| std::sync::Mutex::new(__RayReg { next: 1, open: __RayMap::new() }))\n}\n",
            "fn __ray_reg_insert(h: __RayHandle) -> i64 { let mut reg = __ray_reg().lock().unwrap(); let id = reg.next; reg.next += 1; reg.open.insert(id, h); id }\n",
            "fn __ray_close(h: i64) -> i64 { __ray_reg().lock().unwrap().open.remove(&h); 0 }\n",
        ));
    }
    // Ops de archivo (open/read_line/write) — solo si se usan handles de archivo.
    if t.needs_handles {
        out.push_str(concat!(
            "fn __ray_open(path: &str, mode: &str) -> Result<i64, Rc<str>> {\n",
            "    let h = match mode {\n",
            "        \"r\" => std::fs::File::open(path).map(|f| __RayHandle::Reader(std::io::BufReader::new(f))),\n",
            "        \"w\" => std::fs::File::create(path).map(__RayHandle::Writer),\n",
            "        \"a\" => std::fs::OpenOptions::new().create(true).append(true).open(path).map(__RayHandle::Writer),\n",
            "        _ => return Err(Rc::<str>::from(format!(\"invalid open mode: '{}' (use \\\"r\\\", \\\"w\\\" or \\\"a\\\")\", mode))),\n",
            "    }.map_err(|e| Rc::<str>::from(e.to_string()))?;\n",
            "    Ok(__ray_reg_insert(h))\n}\n",
            "fn __ray_read_line(h: i64) -> Option<Rc<str>> {\n",
            "    use std::io::BufRead; let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Reader(r)) => { let mut line = String::new(); match r.read_line(&mut line) {\n",
            "            Ok(0) | Err(_) => None, Ok(_) => Some(Rc::<str>::from(line.trim_end_matches(['\\n', '\\r']))) } }\n",
            "        _ => None } }\n",
            "fn __ray_write(h: i64, s: &str) -> Result<i64, Rc<str>> {\n",
            "    use std::io::Write; let mut reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get_mut(&h) {\n",
            "        Some(__RayHandle::Writer(f)) => f.write_all(s.as_bytes()).map(|_| s.chars().count() as i64).map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(__RayHandle::Reader(_)) => Err(Rc::<str>::from(\"the handle is open for reading, not writing\")),\n",
            "        _ => Err(Rc::<str>::from(format!(\"invalid file handle: {}\", h))) } }\n",
        ));
    }
    // Ops de socket TCP — solo si se usa la red. Clonan el stream para no retener el lock en la I/O
    // bloqueante (como la VM). read lee ≤64KiB (lossy UTF-8; EOF → ""); write escribe todo (Ok(nº bytes)).
    if t.needs_net {
        out.push_str(concat!(
            "fn __ray_sock_clone(h: i64) -> Result<std::net::TcpStream, Rc<str>> {\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) { Some(__RayHandle::Tcp(s)) => s.try_clone().map_err(|e| Rc::<str>::from(e.to_string())),\n",
            "        Some(_) => Err(Rc::<str>::from(format!(\"handle {} is not a socket\", h))), None => Err(Rc::<str>::from(format!(\"invalid handle: {}\", h))) } }\n",
            "fn __ray_tcp_connect(host: &str, port: i64) -> Result<i64, Rc<str>> {\n",
            "    match std::net::TcpStream::connect((host, port as u16)) { Ok(s) => Ok(__ray_reg_insert(__RayHandle::Tcp(s))), Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
            "fn __ray_tcp_listen(host: &str, port: i64) -> Result<i64, Rc<str>> {\n",
            "    match std::net::TcpListener::bind((host, port as u16)) { Ok(l) => Ok(__ray_reg_insert(__RayHandle::Listener(l))), Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
            "fn __ray_tcp_accept(h: i64) -> Result<i64, Rc<str>> {\n",
            "    let l = { let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Listener(l)) => l.try_clone().map_err(|e| Rc::<str>::from(e.to_string())), _ => return Err(Rc::<str>::from(format!(\"handle {} is not a listener\", h))) } }?;\n",
            "    match l.accept() { Ok((s, _)) => Ok(__ray_reg_insert(__RayHandle::Tcp(s))), Err(e) => Err(Rc::<str>::from(e.to_string())) } }\n",
        ));
        // socket_read/read_bytes/write DESPACHAN a TLS si el handle es una conexión TLS (solo si el
        // programa usa TLS): se clona el `Arc<Mutex<TlsStream>>` del registro y se hace I/O tras SU lock
        // (no el global) → conexiones concurrentes no se serializan. Si no, la vía TCP de siempre (clona el
        // stream para no retener el lock durante la I/O bloqueante).
        // Como la VM: SOLO las variantes `_bytes` despachan a TLS (socket_read/write string dan el error de
        // no-socket sobre un handle TLS). read tiene helper propio (matchea la VM: sin TLS); el `write`
        // compartido cubre write_bytes (el uso real de TLS) → lleva el despacho.
        let (tls_rdb, tls_wr) = if t.needs_rt_tls {
            (
                "if let Some(__t) = __ray_tls_get(h) { let mut __g = __t.lock().unwrap(); let mut buf = [0u8; 65536]; return match __g.read(&mut buf) { Ok(n) => Ok(Rc::<[u8]>::from(&buf[..n])), Err(e) => Err(Rc::<str>::from(e.to_string())) }; } ",
                "if let Some(__t) = __ray_tls_get(h) { let mut __g = __t.lock().unwrap(); return match __g.write_all(bytes) { Ok(()) => Ok(bytes.len() as i64), Err(e) => Err(Rc::<str>::from(e.to_string())) }; } ",
            )
        } else {
            ("", "")
        };
        write!(out, "fn __ray_socket_read(h: i64) -> Result<Rc<str>, Rc<str>> {{ use std::io::Read; let mut s = __ray_sock_clone(h)?; let mut buf = [0u8; 65536]; match s.read(&mut buf) {{ Ok(n) => Ok(Rc::<str>::from(String::from_utf8_lossy(&buf[..n]).into_owned())), Err(e) => Err(Rc::<str>::from(e.to_string())) }} }}\n").unwrap();
        write!(out, "fn __ray_socket_read_bytes(h: i64) -> Result<Rc<[u8]>, Rc<str>> {{ {tls_rdb}use std::io::Read; let mut s = __ray_sock_clone(h)?; let mut buf = [0u8; 65536]; match s.read(&mut buf) {{ Ok(n) => Ok(Rc::<[u8]>::from(&buf[..n])), Err(e) => Err(Rc::<str>::from(e.to_string())) }} }}\n").unwrap();
        write!(out, "fn __ray_socket_write(h: i64, bytes: &[u8]) -> Result<i64, Rc<str>> {{ {tls_wr}use std::io::Write; let mut s = __ray_sock_clone(h)?; let mut off = 0; while off < bytes.len() {{ match s.write(&bytes[off..]) {{ Ok(0) => return Err(Rc::<str>::from(\"the connection closed during the write\")), Ok(n) => off += n, Err(e) => return Err(Rc::<str>::from(e.to_string())) }} }} Ok(bytes.len() as i64) }}\n").unwrap();
        out.push_str(concat!(
            "fn __ray_local_port(h: i64) -> i64 {\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    match reg.open.get(&h) { Some(__RayHandle::Tcp(s)) => s.local_addr().map(|a| a.port() as i64).unwrap_or(0), Some(__RayHandle::Listener(l)) => l.local_addr().map(|a| a.port() as i64).unwrap_or(0), Some(__RayHandle::Udp(s)) => s.local_addr().map(|a| a.port() as i64).unwrap_or(0), _ => 0 } }\n",
            "fn __ray_set_read_timeout(h: i64, ms: i64) {\n",
            "    let d = if ms <= 0 { None } else { Some(std::time::Duration::from_millis(ms as u64)) };\n",
            "    let reg = __ray_reg().lock().unwrap();\n",
            "    if let Some(__RayHandle::Tcp(s)) = reg.open.get(&h) { let _ = s.set_read_timeout(d); } }\n",
            // UDP: los primitivos devuelven ARREGLOS ETIQUETADOS (bind/send → [\"ok\"/\"err\", ...]; recv →
            // [b\"ok\"/b\"err\", host, port, data]) que los wrappers de raylang (udp.ray) parsean. recv es
            // BLOQUEANTE (con hilos de SO reales; la VM usa no-bloqueante + scheduler → mismo efecto).
            "fn __ray_udp_bind(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match std::net::UdpSocket::bind((host, port as u16)) {\n",
            "        Ok(s) => { let id = __ray_reg_insert(__RayHandle::Udp(s)); Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())])) }\n",
            "        Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e.to_string())])) } }\n",
            "fn __ray_udp_clone(h: i64) -> Option<std::net::UdpSocket> { let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Udp(s)) => s.try_clone().ok(), _ => None } }\n",
            "fn __ray_udp_send_to(h: i64, host: &str, port: i64, data: &[u8]) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let r = match __ray_udp_clone(h) { Some(s) => s.send_to(data, (host, port as u16)).map_err(|e| e.to_string()), None => Err(format!(\"handle {} is not a UDP socket\", h)) };\n",
            "    match r { Ok(n) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(n.to_string())])), Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(e)])) } }\n",
            "fn __ray_udp_recv_from(h: i64) -> Rc<std::cell::RefCell<Vec<Rc<[u8]>>>> {\n",
            "    match __ray_udp_clone(h) {\n",
            "        Some(s) => { let mut buf = vec![0u8; 65536]; match s.recv_from(&mut buf) {\n",
            "            Ok((n, addr)) => { buf.truncate(n); Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"ok\"[..]), Rc::<[u8]>::from(addr.ip().to_string().as_bytes()), Rc::<[u8]>::from(addr.port().to_string().as_bytes()), Rc::<[u8]>::from(&buf[..])])) }\n",
            "            Err(e) => Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(e.to_string().as_bytes())])) } }\n",
            "        None => Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(format!(\"handle {} is not a UDP socket\", h).as_bytes())])) } }\n",
        ));
    }
    // Helpers de TLS (P2.b Paso 1), solo si el programa usa TLS. El binario transpilado hace I/O TLS
    // BLOQUEANTE (hilos reales) vía `ray_runtime::tls` — a diferencia de la VM (no-bloqueante + fibras).
    // Los primitivos devuelven arreglos ETIQUETADOS (`["ok", handle]`/`["err", msg]`, como UDP); los
    // wrappers de `std/net.ray` los parsean a `Result`. accept/upgrade parten de un handle TCP: sacan su
    // `TcpStream` del registro y reinsertan la conexión TLS con el MISMO handle (como la VM).
    if t.needs_rt_tls {
        out.push_str(concat!(
            // Clona el Arc<Mutex<TlsStream>> del handle (si es TLS) → la I/O va tras su lock, no el global.
            "fn __ray_tls_get(h: i64) -> Option<std::sync::Arc<std::sync::Mutex<ray_runtime::tls::TlsStream>>> {\n",
            "    let reg = __ray_reg().lock().unwrap(); match reg.open.get(&h) { Some(__RayHandle::Tls(a)) => Some(a.clone()), _ => None } }\n",
            "fn __ray_tls_tag_ok(id: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())])) }\n",
            "fn __ray_tls_tag_err(msg: String) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(msg)])) }\n",
            "fn __ray_tls_wrap(s: ray_runtime::tls::TlsStream) -> i64 { __ray_reg_insert(__RayHandle::Tls(std::sync::Arc::new(std::sync::Mutex::new(s)))) }\n",
            "fn __ray_tls_connect(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match ray_runtime::tls::connect(host, port) { Ok(s) => __ray_tls_tag_ok(__ray_tls_wrap(s)), Err(e) => __ray_tls_tag_err(e) } }\n",
            "fn __ray_tls_connect_h2(host: &str, port: i64) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match ray_runtime::tls::connect_h2(host, port) { Ok(s) => __ray_tls_tag_ok(__ray_tls_wrap(s)), Err(e) => __ray_tls_tag_err(e) } }\n",
            // Saca el TcpStream del handle `h` (debe ser TCP), lo deja fuera del registro y lo devuelve.
            "fn __ray_tls_take_tcp(h: i64) -> Result<std::net::TcpStream, String> {\n",
            "    let mut reg = __ray_reg().lock().unwrap(); match reg.open.remove(&h) {\n",
            "        Some(__RayHandle::Tcp(s)) => Ok(s),\n",
            "        Some(other) => { reg.open.insert(h, other); Err(format!(\"handle {} is not an accepted TCP socket\", h)) }\n",
            "        None => Err(format!(\"invalid handle: {}\", h)) } }\n",
            "fn __ray_tls_accept(h: i64, cert: &str, key: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let sock = match __ray_tls_take_tcp(h) { Ok(s) => s, Err(e) => return __ray_tls_tag_err(e) };\n",
            "    match ray_runtime::tls::accept(sock, cert, key) { Ok(s) => { __ray_reg().lock().unwrap().open.insert(h, __RayHandle::Tls(std::sync::Arc::new(std::sync::Mutex::new(s)))); __ray_tls_tag_ok(h) } Err(e) => __ray_tls_tag_err(e) } }\n",
            "fn __ray_tls_upgrade(h: i64, host: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let sock = match __ray_tls_take_tcp(h) { Ok(s) => s, Err(e) => return __ray_tls_tag_err(e) };\n",
            "    match ray_runtime::tls::upgrade(sock, host) { Ok(s) => { __ray_reg().lock().unwrap().open.insert(h, __RayHandle::Tls(std::sync::Arc::new(std::sync::Mutex::new(s)))); __ray_tls_tag_ok(h) } Err(e) => __ray_tls_tag_err(e) } }\n",
        ));
    }
    // Helpers de SQLite (P2.b Paso 2), solo si el programa usa SQLite. Los primitivos devuelven arreglos
    // ETIQUETADOS que los wrappers de `db/sqlite.ray` parsean: open → ["ok", handle]/["err", msg]; exec →
    // ["ok", n_afectadas]/["err", msg]; query → ["ok", ncols, celda0, celda1, …]/["err", msg]. La conexión
    // vive en el registro (variante Sqlite); exec/query la operan reteniendo el lock global (I/O local).
    if t.needs_rt_sqlite {
        out.push_str(concat!(
            "fn __ray_sqlite_tag(v: Vec<Rc<str>>) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { Rc::new(std::cell::RefCell::new(v)) }\n",
            "fn __ray_sqlite_err(msg: String) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> { __ray_sqlite_tag(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(msg)]) }\n",
            "fn __ray_sqlite_open(path: &str) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    match ray_runtime::sqlite::open(path) { Ok(c) => { let id = __ray_reg_insert(__RayHandle::Sqlite(c)); __ray_sqlite_tag(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(id.to_string())]) } Err(e) => __ray_sqlite_err(e) } }\n",
            // Colecta los parámetros [string] a Vec<String> para la firma de ray_runtime::sqlite.
            "fn __ray_sqlite_params(params: &Rc<std::cell::RefCell<Vec<Rc<str>>>>) -> Vec<String> { params.borrow().iter().map(|s| s.to_string()).collect() }\n",
            "fn __ray_sqlite_exec(h: i64, sql: &str, params: &Rc<std::cell::RefCell<Vec<Rc<str>>>>) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let p = __ray_sqlite_params(params); let reg = __ray_reg().lock().unwrap();\n",
            "    let r = match reg.open.get(&h) { Some(__RayHandle::Sqlite(c)) => c.exec(sql, &p), Some(_) => Err(\"the handle is not a SQLite connection\".to_string()), None => Err(\"invalid or already closed handle\".to_string()) };\n",
            "    match r { Ok(n) => __ray_sqlite_tag(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(n.to_string())]), Err(e) => __ray_sqlite_err(e) } }\n",
            "fn __ray_sqlite_query(h: i64, sql: &str, params: &Rc<std::cell::RefCell<Vec<Rc<str>>>>) -> Rc<std::cell::RefCell<Vec<Rc<str>>>> {\n",
            "    let p = __ray_sqlite_params(params); let reg = __ray_reg().lock().unwrap();\n",
            "    let r = match reg.open.get(&h) { Some(__RayHandle::Sqlite(c)) => c.query(sql, &p), Some(_) => Err(\"the handle is not a SQLite connection\".to_string()), None => Err(\"invalid or already closed handle\".to_string()) };\n",
            "    match r { Ok((ncols, cells)) => { let mut v = vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(ncols.to_string())]; for cell in cells { v.push(Rc::<str>::from(cell)); } __ray_sqlite_tag(v) } Err(e) => __ray_sqlite_err(e) } }\n",
        ));
    }
    // Runtime de canales MPMC (concurrencia, M12.1/M12.2), solo si el programa usa spawn/canales. Es un
    // canal thread-safe propio (Arc<Mutex+Condvar>) — sin deps, ya que el `.rs` es standalone — con
    // backpressure (bounded) y cierre. FIFO como la VM. `T: Send` (primitivos en v1).
    if t.needs_concurrency {
        out.push_str(concat!(
            "struct __ChanState<T> { q: std::collections::VecDeque<T>, closed: bool, cap: Option<usize> }\n",
            "struct __RayChan<T> { inner: std::sync::Arc<(std::sync::Mutex<__ChanState<T>>, std::sync::Condvar)> }\n",
            "impl<T> Clone for __RayChan<T> { fn clone(&self) -> Self { __RayChan { inner: self.inner.clone() } } }\n",
            "impl<T: Send> __RayChan<T> {\n",
            "    fn make(cap: Option<usize>) -> Self { __RayChan { inner: std::sync::Arc::new((std::sync::Mutex::new(__ChanState { q: std::collections::VecDeque::new(), closed: false, cap }), std::sync::Condvar::new())) } }\n",
            "    fn send(&self, v: T) {\n",
            "        let (m, cv) = &*self.inner; let mut st = m.lock().unwrap();\n",
            "        while !st.closed && st.cap.map_or(false, |c| st.q.len() >= c) { st = cv.wait(st).unwrap(); }\n",
            "        if st.closed { return; }\n",
            "        st.q.push_back(v); cv.notify_all();\n",
            "    }\n",
            "    fn recv(&self) -> Option<T> {\n",
            "        let (m, cv) = &*self.inner; let mut st = m.lock().unwrap();\n",
            "        while st.q.is_empty() && !st.closed { st = cv.wait(st).unwrap(); }\n",
            "        let v = st.q.pop_front(); if v.is_some() { cv.notify_all(); } v\n",
            "    }\n",
            "    fn close(&self) { let (m, cv) = &*self.inner; m.lock().unwrap().closed = true; cv.notify_all(); }\n",
            "}\n",
            // Structured concurrency (M12.3): Task<T> = un JoinHandle envuelto (Arc<Mutex>) que cachea el
            // resultado (join una vez ejecuta el hilo; joins posteriores devuelven el clon cacheado → una
            // tarea puede unirse explícitamente O por el scope, no dos veces).
            "struct __TaskState<T> { handle: Option<std::thread::JoinHandle<T>>, result: Option<T> }\n",
            "struct __RayTask<T> { inner: std::sync::Arc<std::sync::Mutex<__TaskState<T>>> }\n",
            "impl<T> Clone for __RayTask<T> { fn clone(&self) -> Self { __RayTask { inner: self.inner.clone() } } }\n",
            "impl<T: Send + Clone + 'static> __RayTask<T> {\n",
            "    fn join(&self) -> T {\n",
            "        let mut st = self.inner.lock().unwrap();\n",
            "        if let Some(h) = st.handle.take() { let r = h.join().unwrap(); st.result = Some(r); }\n",
            "        st.result.clone().unwrap()\n",
            "    }\n",
            "}\n",
            // Cada scope activo (por hilo) acumula clausuras que unen las tareas lanzadas dentro; al salir
            // el scope las ejecuta (une todas). `spawn` registra su tarea en el scope más interno, si hay.
            "thread_local! { static __SCOPES: std::cell::RefCell<Vec<Vec<Box<dyn FnOnce()>>>> = std::cell::RefCell::new(Vec::new()); }\n",
            "fn __ray_spawn<T: Send + Clone + 'static, F: FnOnce() -> T + Send + 'static>(f: F) -> __RayTask<T> {\n",
            "    let task = __RayTask { inner: std::sync::Arc::new(std::sync::Mutex::new(__TaskState { handle: Some(std::thread::spawn(f)), result: None })) };\n",
            "    let t = task.clone();\n",
            "    __SCOPES.with(|s| { if let Some(frame) = s.borrow_mut().last_mut() { frame.push(Box::new(move || { let _ = t.join(); })); } });\n",
            "    task\n}\n",
            "fn __ray_scope<R, F: FnOnce() -> R>(body: F) -> R {\n",
            "    __SCOPES.with(|s| s.borrow_mut().push(Vec::new()));\n",
            "    let r = body();\n",
            "    let frame = __SCOPES.with(|s| s.borrow_mut().pop().unwrap());\n",
            "    for j in frame { j(); }\n",
            "    r\n}\n",
            // select (M12.4): espera a que algún canal de la lista esté LISTO para recibir (cola no vacía
            // ∨ cerrado) y devuelve el índice del PRIMERO listo (menor índice → determinista en el índice;
            // el ORDEN entre canales listos a la vez depende del scheduling, como la VM multicore por
            // default). Poll con backoff (std no tiene un select multi-condvar; el resultado es correcto).
            "fn __ray_select<T>(chs: &[__RayChan<T>]) -> i64 {\n",
            "    loop {\n",
            "        for (i, ch) in chs.iter().enumerate() {\n",
            "            let (m, _) = &*ch.inner; let st = m.lock().unwrap();\n",
            "            if !st.q.is_empty() || st.closed { return i as i64; }\n",
            "        }\n",
            "        std::thread::sleep(std::time::Duration::from_micros(50));\n",
            "    }\n}\n",
        ));
    }
    // signals() (M88.1): el canal de señales del SO (SIGTERM=15/SIGINT=2). El truco del self-pipe (como
    // la VM, `src/builtins.rs`): el handler (async-signal-safe: solo `write`) escribe el nº de señal a un
    // pipe; un hilo lector lo lee (bloqueante) y lo envía al canal. FFI a libc sin crates (siempre
    // enlazada). Unix; en otras plataformas signals() no se soporta (el checker lo permite, pero aquí
    // no compilaría → se documenta como diferido no-unix).
    if t.needs_signals {
        out.push_str(concat!(
            "static __RAY_SIG_PIPE_W: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);\n",
            "unsafe extern \"C\" { fn pipe(fds: *mut i32) -> i32; fn read(fd: i32, buf: *mut u8, n: usize) -> isize; fn write(fd: i32, buf: *const u8, n: usize) -> isize; fn signal(sig: i32, handler: usize) -> usize; }\n",
            "extern \"C\" fn __ray_on_signal(sig: i32) {\n",
            "    let b = sig as u8; let w = __RAY_SIG_PIPE_W.load(std::sync::atomic::Ordering::Relaxed);\n",
            "    if w >= 0 { unsafe { let _ = write(w, &b as *const u8, 1); } }\n}\n",
            "fn __ray_signals() -> __RayChan<i64> {\n",
            "    static CHAN: std::sync::OnceLock<__RayChan<i64>> = std::sync::OnceLock::new();\n",
            "    CHAN.get_or_init(|| {\n",
            "        let ch: __RayChan<i64> = __RayChan::make(None);\n",
            "        let mut fds = [0i32; 2];\n",
            "        unsafe { if pipe(fds.as_mut_ptr()) == 0 {\n",
            "            __RAY_SIG_PIPE_W.store(fds[1], std::sync::atomic::Ordering::Release);\n",
            "            signal(15, __ray_on_signal as *const () as usize);\n",
            "            signal(2, __ray_on_signal as *const () as usize);\n",
            "        } }\n",
            "        let rfd = fds[0]; let ch2 = ch.clone();\n",
            "        std::thread::spawn(move || loop {\n",
            "            let mut b = 0u8; let n = unsafe { read(rfd, &mut b as *mut u8, 1) };\n",
            "            if n == 1 { ch2.send(b as i64); } else if n == 0 { break; }\n",
            "        });\n",
            "        ch\n",
            "    }).clone()\n}\n",
        ));
    }
    // PRNG (SplitMix64, mismo que la VM) + reloj monotónico, solo si el programa usa monotonic/random.
    // Estado global tras un Mutex/OnceLock; sembrado del reloj. No determinista → casa por propiedades.
    if t.needs_time_rng {
        out.push_str(concat!(
            "fn __ray_monotonic() -> i64 {\n",
            "    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();\n",
            "    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as i64\n}\n",
            "fn __ray_rng() -> &'static std::sync::Mutex<u64> {\n",
            "    static R: std::sync::OnceLock<std::sync::Mutex<u64>> = std::sync::OnceLock::new();\n",
            "    R.get_or_init(|| std::sync::Mutex::new(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9_7F4A_7C15)))\n}\n",
            "fn __ray_next_u64() -> u64 {\n",
            "    let mut s = __ray_rng().lock().unwrap();\n",
            "    *s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);\n",
            "    let mut z = *s;\n",
            "    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);\n",
            "    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);\n",
            "    z ^ (z >> 31)\n}\n",
            "fn __ray_random_f64() -> f64 { (__ray_next_u64() >> 11) as f64 / (1u64 << 53) as f64 }\n",
            "fn __ray_random_int(n: i64) -> i64 { if n <= 0 { 0 } else { (__ray_next_u64() % (n as u64)) as i64 } }\n",
            "fn __ray_random_seed(n: i64) { *__ray_rng().lock().unwrap() = n as u64; }\n",
        ));
    }
    // Features de `ray-runtime` a activar (bajo demanda). Vacío → `build_native` usa `rustc` pelado.
    let mut rt_features = Vec::new();
    if t.needs_rt_crypto {
        rt_features.push("crypto");
    }
    if t.needs_rt_tls {
        rt_features.push("tls");
    }
    if t.needs_rt_sqlite {
        rt_features.push("sqlite");
    }
    Ok(Transpiled { source: out, rt_features })
}

impl Transpiler {
    /// Registra las celdas de `body` (var capturadas por closures) en `self.cells`, devolviendo las que
    /// AÑADIÓ (para quitarlas al salir del ámbito). Un nombre ya presente por un ámbito externo no se
    /// duplica ni se quita aquí.
    fn enter_cells(&mut self, body: &Block) -> Vec<String> {
        let mut added = Vec::new();
        for n in cell_vars(body) {
            if self.cells.insert(n.clone()) {
                added.push(n);
            }
        }
        added
    }

    fn exit_cells(&mut self, added: Vec<String>) {
        for n in added {
            self.cells.remove(&n);
        }
    }

    fn emit_function(&mut self, out: &mut String, rust_name: &str, f: &Function) -> Result<(), String> {
        // Un cuerpo que NO transpila (p. ej. usa `try_join`) propaga `Err` con `?` sin haber popeado los
        // scopes ni deshecho las cells que ya declaró → sus locales (p. ej. un `Task t`) se FILTRABAN al
        // siguiente `emit_function`, cuyo `spawn` capturaba ese `t` fantasma (`in_scope_channels`) y emitía
        // `let t = t.clone()` con `t` inexistente en Rust. Se restaura el estado en TODOS los caminos.
        let base_scopes = self.scopes.len();
        let prev_tparams = std::mem::take(&mut self.tparams);
        let prev_cells = std::mem::take(&mut self.cells);
        let r = self.emit_function_inner(out, rust_name, f);
        self.scopes.truncate(base_scopes);
        self.tparams = prev_tparams;
        self.cells = prev_cells;
        r
    }

    fn emit_function_inner(&mut self, out: &mut String, rust_name: &str, f: &Function) -> Result<(), String> {
        // Params de tipo en ámbito (para `rust_ty` y la clasificación de `Struct(T)`→genérico).
        self.tparams = f.type_params.iter().cloned().collect();
        // Genéricos de Rust con bound `Clone` (todo valor genérico se clona al leer) + `RayShow` (por si
        // se imprime/`to_string`). rustc los monomorfiza → nativo. Los bounds de raylang (Eq/Ord/traits de
        // usuario) los realiza el **paso de diccionarios** del checker: sus params ocultos (`T#Trait#m`,
        // valores función) y el impl manglado (`Tipo#m`) se emiten tal cual (como funciones ordinarias).
        let generics = generic_decl(&f.type_params);
        self.scopes.push(HashMap::new());
        let mut params = Vec::new();
        for p in &f.params {
            params.push(format!("mut {}: {}", mangle(&p.name), rust_ty(&p.ty, &self.enums, &self.tparams)?));
            self.declare(&p.name, p.ty.clone());
        }
        write!(
            out,
            "fn {}{}({}) -> {} ",
            rust_name,
            generics,
            params.join(", "),
            rust_ty(&f.return_type, &self.enums, &self.tparams)?
        )
        .unwrap();
        let added = self.enter_cells(&f.body);
        self.emit_block(out, &f.body)?;
        self.exit_cells(added);
        out.push('\n');
        self.scopes.pop();
        self.tparams.clear();
        Ok(())
    }

    /// Emite un STUB que panica, con la FIRMA declarada de `f`, cuando su CUERPO no transpila (usa algo
    /// fuera del subconjunto: un primitivo TLS, `for` sobre iterador, etc.). Motivo: antes esas funciones
    /// se OMITÍAN y sus llamadas quedaban colgantes → rustc fallaba **aunque el flujo real nunca las
    /// llamara** (p. ej. `http_demo` habla HTTP plano pero arrastra `tls_connect`). Con el stub el programa
    /// COMPILA; si nunca se alcanza, corre idéntico a la VM; si se alcanza, panica con un mensaje claro
    /// (mejor que un error críptico de rustc). El tipo de retorno `!` de `panic!` encaja con cualquier
    /// firma. Devuelve Err solo si ni la FIRMA es representable (raro) → el llamador vuelve a omitirla.
    fn emit_stub(&mut self, out: &mut String, rust_name: &str, f: &Function) -> Result<(), String> {
        self.tparams = f.type_params.iter().cloned().collect();
        let generics = generic_decl(&f.type_params);
        let mut params = Vec::new();
        for p in &f.params {
            params.push(format!("mut {}: {}", mangle(&p.name), rust_ty(&p.ty, &self.enums, &self.tparams)?));
        }
        let ret = rust_ty(&f.return_type, &self.enums, &self.tparams)?;
        write!(
            out,
            "fn {}{}({}) -> {} {{ panic!(\"'{}' no está soportada en el binario nativo (transpilación a Rust)\") }}\n",
            rust_name, generics, params.join(", "), ret, f.name
        )
        .unwrap();
        self.tparams.clear();
        Ok(())
    }

    /// Emite el FFI (M41): por cada `extern "lib" { fn name(...) -> ret; }`, (1) una declaración
    /// `extern "C"` del símbolo C (`__ffi_name` con `#[link_name = "name"]`, tipos de ABI) agrupada por
    /// librería con su `#[link(name = "lib")]` (libc va implícita), y (2) un WRAPPER `fn name(...)` con la
    /// firma raylang que **marshala** los argumentos (string→`CString`, bool→`c_int`, ptr→`*mut c_void`),
    /// llama al símbolo dentro de `unsafe`, y marshala el retorno (`c_int`→bool, `char*`→`Option<...>`
    /// copiando hasta el NUL, etc.). Espejo conductual de `src/ffi.rs`. Es la frontera insegura.
    fn emit_externs(&self, out: &mut String, prog: &Program) -> Result<(), String> {
        if prog.externs.is_empty() {
            return Ok(());
        }
        // (1) Declaraciones `extern "C"`, agrupadas por librería (orden estable).
        let mut by_lib: std::collections::BTreeMap<&str, Vec<&crate::ast::ExternFn>> =
            std::collections::BTreeMap::new();
        for e in &prog.externs {
            by_lib.entry(e.lib.as_str()).or_default().push(e);
        }
        for (lib, fns) in &by_lib {
            // libc ya está enlazada (símbolos disponibles); otras librerías (`m`, …) llevan `#[link]`.
            if *lib != "c" {
                writeln!(out, "#[link(name = \"{}\")]", lib).unwrap();
            }
            out.push_str("unsafe extern \"C\" {\n");
            for e in fns {
                let cargs: Vec<String> = e
                    .params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| ffi_c_arg_ty(&p.ty).map(|c| format!("__a{}: {}", i, c)))
                    .collect::<Result<_, _>>()?;
                writeln!(out, "    #[link_name = \"{}\"]", e.name).unwrap();
                writeln!(
                    out,
                    "    fn __ffi_{}({}) -> {};",
                    mangle(&e.name),
                    cargs.join(", "),
                    ffi_c_ret_ty(&e.return_type)?
                )
                .unwrap();
            }
            out.push_str("}\n");
        }
        // (2) Wrappers con la firma raylang + marshalling.
        for e in &prog.externs {
            let params: Vec<String> = e
                .params
                .iter()
                .enumerate()
                .map(|(i, p)| rust_ty(&p.ty, &self.enums, &self.tparams).map(|r| format!("mut __p{}: {}", i, r)))
                .collect::<Result<_, _>>()?;
            let ret = rust_ty(&e.return_type, &self.enums, &self.tparams)?;
            write!(out, "fn {}({}) -> {} {{\n", mangle(&e.name), params.join(", "), ret).unwrap();
            // Pre-marshalling de argumentos: los `string` necesitan un `CString` VIVO durante la llamada.
            let mut passes = Vec::new();
            for (i, p) in e.params.iter().enumerate() {
                match normalize_type(&p.ty) {
                    Type::Int | Type::Float => passes.push(format!("__p{}", i)),
                    Type::Bool => passes.push(format!("(__p{} as std::os::raw::c_int)", i)),
                    Type::String => {
                        writeln!(out, "    let __c{} = std::ffi::CString::new(&*__p{} as &str).expect(\"FFI: el string tiene un NUL interno\");", i, i).unwrap();
                        passes.push(format!("__c{}.as_ptr()", i));
                    }
                    Type::Bytes => passes.push(format!("__p{}.as_ptr()", i)),
                    Type::Ptr => passes.push(format!("(__p{} as *mut std::ffi::c_void)", i)),
                    other => return Err(format!("spike: FFI arg no marshalable: {:?}", other)),
                }
            }
            writeln!(out, "    let __r = unsafe {{ __ffi_{}({}) }};", mangle(&e.name), passes.join(", ")).unwrap();
            // Marshalling del retorno C → valor raylang.
            let ret_expr = match normalize_type(&e.return_type) {
                // `__r` es `c_int` (i32) para Int → extiende el signo a i64 (como la VM).
                Type::Int => "__r as i64".to_string(),
                Type::Float => "__r".to_string(),
                Type::Bool => "__r != 0".to_string(),
                Type::Unit => "()".to_string(),
                Type::Ptr => "__r as i64".to_string(),
                Type::Enum(n, args) if n == "Option" && args.len() == 1 => match normalize_type(&args[0]) {
                    // char* → Option<bytes>: NULL→None; si no, copia los bytes hasta el NUL (nunca libera).
                    Type::Bytes => "if __r.is_null() { None } else { Some(Rc::<[u8]>::from(unsafe { std::ffi::CStr::from_ptr(__r) }.to_bytes())) }".to_string(),
                    // char* → Option<string>: como bytes, validando UTF-8 (inválido → error de ejecución).
                    Type::String => "if __r.is_null() { None } else { Some(Rc::<str>::from(std::str::from_utf8(unsafe { std::ffi::CStr::from_ptr(__r) }.to_bytes()).expect(\"FFI: el char* devuelto no es UTF-8 válido\"))) }".to_string(),
                    // ptr fallible → Option<ptr>: NULL→None; si no, la dirección opaca.
                    Type::Ptr => "if __r.is_null() { None } else { Some(__r as i64) }".to_string(),
                    other => return Err(format!("spike: FFI retorno Option<{:?}> no soportado", other)),
                },
                other => return Err(format!("spike: FFI retorno no marshalable: {:?}", other)),
            };
            writeln!(out, "    {}\n}}", ret_expr).unwrap();
        }
        Ok(())
    }

    fn declare(&mut self, name: &str, ty: Type) {
        let t = normalize_type(&ty);
        // Un `Struct(n)` cuyo `n` es un enum del usuario → `Enum(n)` (el parser no distingue; el checker
        // lo hace en su tabla). Así el entorno lleva el tipo correcto para el dispatch de `match`/campos.
        let t = match &t {
            Type::Struct(n, a) if self.enums.contains(n) => Type::Enum(n.clone(), a.clone()),
            _ => t,
        };
        self.scopes.last_mut().unwrap().insert(name.to_string(), t);
    }

    fn lookup(&self, name: &str) -> Option<&Type> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    /// Nombres de las variables en ámbito cuyo tipo es `Channel`/`Task` (los valores compartibles entre
    /// hilos). Se clonan antes de un `spawn` para que el closure `move` no consuma el original. Dedup:
    /// el nombre más interno (shadowing) gana, y no se repite.
    fn in_scope_channels(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        for scope in self.scopes.iter().rev() {
            for (name, ty) in scope {
                if matches!(ty, Type::Channel(_) | Type::Task(_)) && seen.insert(name.clone()) {
                    names.push(name.clone());
                }
            }
        }
        names
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
                // Tipo de la variable: la anotación si está, si no se infiere del inicializador. Si hay
                // anotación, se EMITE (`let x: T = …`) para pinar la inferencia de Rust — necesario para
                // colecciones vacías (`[:]`/`[]`/`Map.new()`), cuyos K/V no se deducen del literal.
                let vty = match ty {
                    Some(t) => normalize_type(t),
                    None => self.type_of(value)?,
                };
                let ann = match ty {
                    Some(t) => Some(rust_ty(t, &self.enums, &self.tparams)?),
                    None => None,
                };
                // Var-celda (B1): capturada+mutada por una closure → `let n = Rc::new(RefCell::new(init))`
                // (el Rc es inmutable; la mutación va por el RefCell). Las lecturas/escrituras la desenvuelven.
                if self.cells.contains(name) {
                    write!(out, "let {} = Rc::new(std::cell::RefCell::new(", mangle(name)).unwrap();
                    self.emit_typed(out, value, &vty)?;
                    out.push_str("));\n");
                    self.declare(name, vty);
                    return Ok(());
                }
                out.push_str(if *mutable { "let mut " } else { "let " });
                out.push_str(&mangle(name));
                if let Some(a) = ann {
                    write!(out, ": {}", a).unwrap();
                }
                out.push_str(" = ");
                self.emit_typed(out, value, &vty)?; // pina el tipo sized de un literal (`u8`/`u32`/`u64`)
                out.push_str(";\n");
                self.declare(name, vty);
            }
            // Desestructuración de tupla: `let (a, b) = e;` → `let (a, b) = e;` (`_` descarta; `var`→mut).
            StmtKind::LetTuple { names, value, mutable } => {
                let elems = match self.type_of(value)? {
                    Type::Tuple(ts) => ts,
                    other => return Err(format!("spike: let-tupla sobre {:?}", other)),
                };
                out.push_str("let (");
                for (i, nm) in names.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    match nm {
                        Some(n) => {
                            if *mutable {
                                out.push_str("mut ");
                            }
                            out.push_str(&mangle(n));
                        }
                        None => out.push('_'),
                    }
                }
                out.push_str(") = ");
                self.emit_expr(out, value)?;
                out.push_str(";\n");
                for (nm, et) in names.iter().zip(&elems) {
                    if let Some(n) = nm {
                        self.declare(n, et.clone());
                    }
                }
            }
            StmtKind::Assign { target, value } => {
                // El TARGET es un lvalue: NO se clona (a diferencia de una lectura). Para `a[i]`/`p.x` el
                // RHS se evalúa a un temporal ANTES del `borrow_mut()` del target: si el RHS lee el MISMO
                // objeto (`p.x = p.x + 1`), evita el doble borrow del RefCell (leer + mutar a la vez).
                match &target.kind {
                    ExprKind::Ident(name) => {
                        let tty = self.type_of(target)?;
                        if self.cells.contains(name) {
                            // Var-celda (B1): `n = e` → `*n.borrow_mut() = e`. El RHS va a un temp ANTES del
                            // borrow_mut: si lee la MISMA celda (`n = n + 1`), evita el doble borrow.
                            out.push_str("{ let __v = ");
                            self.emit_typed(out, value, &tty)?;
                            write!(out, "; *{}.borrow_mut() = __v; }}\n", mangle(name)).unwrap();
                        } else {
                            out.push_str(&mangle(name));
                            out.push_str(" = ");
                            self.emit_typed(out, value, &tty)?; // sized: pina el tipo del literal en el RHS
                            out.push_str(";\n");
                        }
                    }
                    ExprKind::Index { array, index } => {
                        out.push_str("{ let __rhs = ");
                        self.emit_expr(out, value)?;
                        out.push_str("; ");
                        self.emit_expr(out, array)?;
                        out.push_str(".borrow_mut()[");
                        self.emit_expr(out, index)?;
                        out.push_str(" as usize] = __rhs; }\n");
                    }
                    ExprKind::Field { object, name } => {
                        out.push_str("{ let __rhs = ");
                        self.emit_expr(out, value)?;
                        out.push_str("; ");
                        self.emit_expr(out, object)?;
                        write!(out, ".borrow_mut().{} = __rhs; }}\n", name).unwrap();
                    }
                    _ => return Err("spike: lvalue no soportado".into()),
                }
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
                // `for (a, b) in <iterador que entrega tuplas>` (M40.2: `enumerate`/`zip`): el `next(it)`
                // devuelve `Option<(A, B)>` → se destructura en el `match Some((a, b))`. Mismo `loop` que el
                // caso simple pero ligando dos nombres.
                if let (ForPat::Tuple(names), ForIter::Iter { expr, next_fn }) = (pat, iter) {
                    let sig = self
                        .funcs
                        .get(next_fn)
                        .ok_or_else(|| format!("spike: iterador sin método next '{}'", next_fn))?
                        .clone();
                    let it_ty = self.type_of(expr)?;
                    let mut subst = HashMap::new();
                    if let Some(p0) = sig.params.first() {
                        unify(p0, &it_ty, &sig.tparams, &mut subst);
                    }
                    let elems = match subst_type(&normalize_type(&sig.ret), &subst) {
                        Type::Enum(n, args) if n == "Option" && args.len() == 1 => match &args[0] {
                            Type::Tuple(ts) if ts.len() == names.len() => ts.clone(),
                            other => return Err(format!("spike: for de tupla sobre next que da {:?}", other)),
                        },
                        other => return Err(format!("spike: next de '{}' no da Option<tupla> ({:?})", next_fn, other)),
                    };
                    let binder = |n: &Option<String>| n.clone().map(|x| mangle(&x)).unwrap_or_else(|| "_".into());
                    let binders: Vec<String> = names.iter().map(binder).collect();
                    out.push_str("{ let __it = ");
                    self.emit_expr(out, expr)?;
                    write!(out, "; loop {{ match {}(__it.clone()) {{ Some((", mangle(next_fn)).unwrap();
                    out.push_str(&binders.join(", "));
                    out.push_str(")) => ");
                    self.scopes.push(HashMap::new());
                    for (n, t) in names.iter().zip(&elems) {
                        if let Some(nm) = n { self.declare(nm, t.clone()); }
                    }
                    self.emit_block(out, body)?;
                    self.scopes.pop();
                    out.push_str(", None => break, } } }\n");
                    return Ok(());
                }
                // `for (k, v) in <Map>`: itera pares ordenados por clave (helper `__ray_pairs`).
                if let ForPat::Tuple(names) = pat {
                    let expr = match iter {
                        ForIter::In(e) => e,
                        _ => return Err("spike: for de tupla solo sobre Map".into()),
                    };
                    let (kt, vt) = match self.type_of(expr)? {
                        Type::Map(k, v) => ((*k).clone(), (*v).clone()),
                        other => return Err(format!("spike: for (k,v) sobre {:?}", other)),
                    };
                    let binder = |n: &Option<String>| n.clone().unwrap_or_else(|| "_".into());
                    let (kn, vn) = (binder(&names[0]), binder(&names[1]));
                    write!(out, "for ({}, {}) in __ray_pairs(&", kn, vn).unwrap();
                    self.emit_expr(out, expr)?;
                    out.push_str(") ");
                    self.scopes.push(HashMap::new());
                    if let Some(n) = &names[0] {
                        self.declare(n, kt);
                    }
                    if let Some(n) = &names[1] {
                        self.declare(n, vt);
                    }
                    self.emit_block(out, body)?;
                    self.scopes.pop();
                    out.push('\n');
                    return Ok(());
                }
                let var = match pat {
                    ForPat::Single(n) => n.clone(),
                    ForPat::Tuple(_) => unreachable!("tupla ya manejada arriba"),
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
                    // `for x in <it>` sobre un Iterator<T> de usuario (M40.2): el checker guarda el método
                    // `next(self) -> Option<T>` manglado. Se baja a un `loop`: llamar `next(it)` hasta `None`,
                    // ligando cada `Some(x)`. El iterador se liga a `__it` UNA vez; `next` recibe un clon del
                    // Rc → su estado (campos mutados por referencia) persiste entre iteraciones.
                    ForIter::Iter { expr, next_fn } => {
                        // T del elemento = el `T` de `Option<T>` de la firma de `next`, tras unificar el tipo
                        // real del iterador con su `self` (para adaptadores genéricos como `ArrayIter<int>`).
                        let sig = self
                            .funcs
                            .get(next_fn)
                            .ok_or_else(|| format!("spike: iterador sin método next '{}'", next_fn))?
                            .clone();
                        let it_ty = self.type_of(expr)?;
                        let mut subst = HashMap::new();
                        if let Some(p0) = sig.params.first() {
                            unify(p0, &it_ty, &sig.tparams, &mut subst);
                        }
                        let elem = match subst_type(&normalize_type(&sig.ret), &subst) {
                            Type::Enum(n, args) if n == "Option" && args.len() == 1 => args[0].clone(),
                            other => return Err(format!("spike: next de '{}' no devuelve Option<T> ({:?})", next_fn, other)),
                        };
                        out.push_str("{ let __it = ");
                        self.emit_expr(out, expr)?;
                        write!(out, "; loop {{ match {}(__it.clone()) {{ Some(", mangle(next_fn)).unwrap();
                        out.push_str(&mangle(&var));
                        out.push_str(") => ");
                        self.scopes.push(HashMap::new());
                        self.declare(&var, elem);
                        self.emit_block(out, body)?;
                        self.scopes.pop();
                        out.push_str(", None => break, } } }\n");
                    }
                }
            }
        }
        Ok(())
    }

    /// Emite `e` sabiendo el tipo ESPERADO. Solo cambia algo para enteros con tamaño (`u8`/`u32`/`u64`):
    /// Rust no coacciona `i64`→`u8`, así que un literal entero se emite con su sufijo (`200u8`), un arreglo
    /// propaga el tipo de elemento, y cualquier otra expr cuyo tipo real no case con el sized esperado se
    /// castea `(e) as uW`. La aritmética entre valores sized ya es del tipo correcto (`type_of` lo propaga),
    /// así que NO se castea de más. Para tipos no-sized delega en `emit_expr`.
    fn emit_typed(&mut self, out: &mut String, e: &Expr, expected: &Type) -> Result<(), String> {
        let exp = normalize_type(expected);
        match (&e.kind, &exp) {
            (ExprKind::Int(n), Type::UInt(w)) => write!(out, "{}u{}", n, w).unwrap(),
            (ExprKind::ArrayLit(elems), Type::Array(et)) => {
                out.push_str("Rc::new(std::cell::RefCell::new(");
                if elems.is_empty() {
                    out.push_str("Vec::new()");
                } else {
                    out.push_str("vec![");
                    for (i, el) in elems.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_typed(out, el, et)?;
                    }
                    out.push(']');
                }
                out.push_str("))");
            }
            (_, Type::UInt(w)) => {
                if self.type_of(e)? == exp {
                    self.emit_expr(out, e)?; // ya es del tipo sized (var/cast/aritmética entre sized)
                } else {
                    out.push('(');
                    self.emit_expr(out, e)?;
                    write!(out, ") as u{}", w).unwrap(); // p. ej. un producto de literales i64 → uW
                }
            }
            _ => self.emit_expr(out, e)?,
        }
        Ok(())
    }

    /// Emite `e` convertido a la repr SEND de un `Channel<T>`/`Task<T>` (para cruzar el hilo): string→
    /// Arc<str>, bytes→Arc<[u8]> (copia al borde, seguro por ser inmutables); primitivos sin cambio.
    fn emit_to_send(&mut self, out: &mut String, e: &Expr, t: &Type) -> Result<(), String> {
        match normalize_type(t) {
            Type::String => {
                out.push_str("std::sync::Arc::<str>::from(&*");
                self.emit_expr(out, e)?;
                out.push(')');
            }
            Type::Bytes => {
                out.push_str("std::sync::Arc::<[u8]>::from(&*");
                self.emit_expr(out, e)?;
                out.push(')');
            }
            _ => self.emit_expr(out, e)?,
        }
        Ok(())
    }

    fn emit_expr(&mut self, out: &mut String, e: &Expr) -> Result<(), String> {
        match &e.kind {
            ExprKind::Int(n) => write!(out, "{}i64", n).unwrap(),
            ExprKind::Float(x) => write!(out, "{:?}f64", x).unwrap(),
            ExprKind::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            ExprKind::Char(c) => write!(out, "{:?}", c).unwrap(), // `{:?}` de char → literal Rust escapado
            ExprKind::Str(s) => write!(out, "Rc::<str>::from({:?})", s).unwrap(),
            // Literal de bytes `b"..."` → Rc<[u8]> desde un Vec de octetos.
            ExprKind::Bytes(b) => {
                out.push_str("Rc::<[u8]>::from(vec![");
                for (i, byte) in b.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write!(out, "{}u8", byte).unwrap();
                }
                out.push_str("])");
            }
            // Conversión `expr as T` (M27.4): int↔float, char↔int. Rust `as` cubre los numéricos;
            // char→int pasa por u32; int→char usa char::from_u32.
            ExprKind::Cast { expr, ty } => {
                let target = normalize_type(ty);
                let src = self.type_of(expr)?;
                match (&src, &target) {
                    (Type::Char, Type::Int) => {
                        out.push('(');
                        self.emit_expr(out, expr)?;
                        out.push_str(" as u32 as i64)");
                    }
                    (Type::Int, Type::Char) => {
                        out.push_str("char::from_u32((");
                        self.emit_expr(out, expr)?;
                        out.push_str(") as u32).unwrap()");
                    }
                    (_, Type::Int) => {
                        out.push('(');
                        self.emit_expr(out, expr)?;
                        out.push_str(" as i64)");
                    }
                    (_, Type::Float) => {
                        out.push('(');
                        self.emit_expr(out, expr)?;
                        out.push_str(" as f64)");
                    }
                    // A entero con tamaño (`x as u32`): el `as` de Rust trunca/envuelve a N bits (mismo
                    // resultado que la VM). Cubre int→uN, uN→uM, char→uN.
                    (_, Type::UInt(w)) => {
                        out.push('(');
                        self.emit_expr(out, expr)?;
                        write!(out, " as u{})", w).unwrap();
                    }
                    _ => return Err(format!("spike: cast {:?}→{:?} no soportado", src, target)),
                }
            }
            ExprKind::Ident(name) if name == "std::math::PI" => out.push_str("std::f64::consts::PI"),
            ExprKind::Ident(name) if name == "std::math::E" => out.push_str("std::f64::consts::E"),
            ExprKind::Ident(name) => {
                if self.cells.contains(name) {
                    // Var-celda (B1): leer = desenvolver la celda con un clon del valor (`n.borrow().clone()`).
                    // clone() vale para todo tipo (para int es copia); mantiene la semántica de "leer clona".
                    write!(out, "{}.borrow().clone()", mangle(name)).unwrap();
                } else if let Some(ty) = self.lookup(name) {
                    // Variable local: clonar al leer los valores de heap (Rc → bump barato); escalares Copy.
                    let heap = is_heap(ty);
                    out.push_str(&mangle(name));
                    if heap {
                        out.push_str(".clone()");
                    }
                } else if self.consts.contains_key(name) {
                    write!(out, "{}()", mangle(name)).unwrap(); // constante → llamada NAME()
                } else if self.funcs.contains_key(name) {
                    // Función usada como VALOR → Rc::new(fn) (coerciona a Rc<dyn Fn> por el contexto).
                    write!(out, "Rc::new({})", mangle(name)).unwrap();
                } else {
                    out.push_str(&mangle(name));
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
                } else if matches!(op, BinaryOp::Add) && matches!(self.type_of(left)?, Type::Bytes) {
                    // bytes: `a + b` concatena los dos slices en un Rc<[u8]> nuevo (como `Value::Bytes` +).
                    out.push_str("Rc::<[u8]>::from([&*");
                    self.emit_expr(out, left)?;
                    out.push_str(", &*");
                    self.emit_expr(out, right)?;
                    out.push_str("].concat())");
                } else if matches!(op, BinaryOp::Add) && matches!(self.type_of(left)?, Type::Array(_)) {
                    // arreglos (M11.7b): `a + b` es un arreglo NUEVO con los elementos (clonados) de ambos.
                    // Dos `.borrow()` compartidos coexisten (incl. `a + a`); el `clone()` libera el primero.
                    out.push_str("{ let mut __v = ");
                    self.emit_expr(out, left)?;
                    out.push_str(".borrow().clone(); __v.extend(");
                    self.emit_expr(out, right)?;
                    out.push_str(".borrow().iter().cloned()); Rc::new(std::cell::RefCell::new(__v)) }");
                } else if matches!(op, BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul)
                    && matches!(self.type_of(left)?, Type::UInt(_))
                {
                    // Enteros con tamaño: aritmética ENVOLVENTE explícita — Rust deniega en compilación el
                    // overflow constante (`200u8 + 100u8`), y `wrapping_*` garantiza mod 2^N como la VM.
                    let m = match op {
                        BinaryOp::Add => "wrapping_add",
                        BinaryOp::Sub => "wrapping_sub",
                        _ => "wrapping_mul",
                    };
                    out.push('(');
                    self.emit_expr(out, left)?;
                    write!(out, ").{}(", m).unwrap();
                    self.emit_expr(out, right)?;
                    out.push(')');
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
            // Literal de tupla `(a, b, …)` → tupla nativa de Rust `(a, b,)`.
            ExprKind::TupleLit(elems) => {
                out.push('(');
                for e in elems {
                    self.emit_expr(out, e)?;
                    out.push_str(", ");
                }
                out.push(')');
            }
            // Indexación de LECTURA. Tupla: `t.0` → `t.0` (campo nativo). Arreglo: `a[i]` →
            // `a.borrow()[i].clone()`. String: `s[i]` → char por índice (chars().nth; OOB → panic).
            ExprKind::Index { array, index } if matches!(self.type_of(array)?, Type::Tuple(_)) => {
                self.emit_expr(out, array)?;
                match &index.kind {
                    ExprKind::Int(n) => write!(out, ".{}", n).unwrap(),
                    _ => return Err("spike: índice de tupla no literal".into()),
                }
            }
            ExprKind::Index { array, index } => {
                match self.type_of(array)? {
                    // string: `s[i]` → el carácter en la posición i (por carácter, como la VM).
                    Type::String => {
                        self.emit_expr(out, array)?;
                        out.push_str(".chars().nth(");
                        self.emit_expr(out, index)?;
                        out.push_str(" as usize).unwrap()");
                    }
                    // bytes: `b[i]` → el octeto como int (Rc<[u8]>, sin borrow); OOB = pánico (~error de la VM).
                    // Paréntesis: el `as i64` no puede ir seguido de un método (p. ej. `.ray_show()`).
                    Type::Bytes => {
                        out.push('(');
                        self.emit_expr(out, array)?;
                        out.push('[');
                        self.emit_expr(out, index)?;
                        out.push_str(" as usize] as i64)");
                    }
                    // arreglo/Map: `a[i]` → el elemento (clon al leer; a través del RefCell).
                    _ => {
                        self.emit_expr(out, array)?;
                        out.push_str(".borrow()[");
                        self.emit_expr(out, index)?;
                        out.push_str(" as usize].clone()");
                    }
                }
            }
            // Coerción concreto→`dyn Trait` (M9.3b): el checker la baja a `__dyn_T { data: <concreto>,
            // m: <método>, … }`. Aquí → un struct de closures que CAPTURAN el concreto: cada método
            // `m: { let __c = <concreto>; move |args| m_concreto(__c.clone(), args) }` (sin `data`).
            ExprKind::StructLit { name, fields } if name.starts_with("__dyn_") => {
                // fields[0] = ("data", <concreto>); el resto = (método, <valor-vtable>).
                let concrete = &fields[0].1;
                out.push_str("{ let __c = ");
                self.emit_expr(out, concrete)?;
                write!(out, "; Rc::new(std::cell::RefCell::new({} {{ ", mangle(name)).unwrap();
                for (i, (mname, mval)) in fields.iter().enumerate().skip(1) {
                    if i > 1 {
                        out.push_str(", ");
                    }
                    let (args, _) = self
                        .trait_method_sigs
                        .get(mname)
                        .ok_or_else(|| format!("spike: método de dyn desconocido '{}'", mname))?
                        .clone();
                    // params de la closure: __a0: T0, __a1: T1, …
                    let mut params = String::new();
                    for (j, aty) in args.iter().enumerate() {
                        if j > 0 {
                            params.push_str(", ");
                        }
                        write!(params, "__a{}: {}", j, rust_ty(aty, &self.enums, &self.tparams)?).unwrap();
                    }
                    write!(out, "{}: {{ let __c = __c.clone(); Rc::new(move |{}| ", mname, params).unwrap();
                    // llamada al método concreto: m_concreto(__c.clone(), __a0, …).
                    match &mval.kind {
                        ExprKind::Ident(fname) => write!(out, "{}(", mangle(fname)).unwrap(),
                        _ => {
                            out.push('(');
                            self.emit_expr(out, mval)?;
                            out.push_str(")(");
                        }
                    }
                    out.push_str("__c.clone()");
                    for j in 0..args.len() {
                        write!(out, ", __a{}", j).unwrap();
                    }
                    out.push_str(")) }");
                }
                out.push_str(" })) }");
            }
            // Literal de struct: Punto { x: 1, y: 2 } → Rc::new(RefCell::new(Punto { x: 1, y: 2 })).
            ExprKind::StructLit { name, fields } => {
                write!(out, "Rc::new(std::cell::RefCell::new({} {{ ", mangle(name)).unwrap();
                for (i, (fname, val)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write!(out, "{}: ", fname).unwrap();
                    self.emit_expr(out, val)?;
                }
                out.push_str(" }))");
            }
            // Acceso a campo (lectura). Tupla: `t.0` → `t.0` (campo nativo, sin borrow). Struct: `p.x` →
            // `p.borrow().x.clone()`. (El `Field` de un método/UFCS lo consume `emit_call`.)
            ExprKind::Field { object, name } => {
                if matches!(self.type_of(object)?, Type::Tuple(_)) {
                    self.emit_expr(out, object)?;
                    write!(out, ".{}", name).unwrap();
                } else {
                    self.emit_expr(out, object)?;
                    write!(out, ".borrow().{}.clone()", name).unwrap();
                }
            }
            // Construcción de variante de enum. Option/Result → Some/None/Ok/Err NATIVOS de Rust (sin Rc);
            // un enum de usuario → Rc::new(EnumName::Variant(args)).
            ExprKind::EnumLit { enum_name, variant, args } => {
                let native = enum_name == "Option" || enum_name == "Result";
                if native {
                    out.push_str(variant); // Some / None / Ok / Err
                } else {
                    write!(out, "Rc::new({}::{}", mangle(enum_name), variant).unwrap();
                }
                if !args.is_empty() {
                    out.push('(');
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_expr(out, a)?;
                    }
                    out.push(')');
                }
                if !native {
                    out.push(')');
                }
            }
            // Función anónima → closure `move` de Rust envuelto en Rc (captura por valor: para los
            // `Rc<RefCell>` comparte el estado, como las celdas del intérprete; los escalares se copian —
            // la captura MUTABLE de un escalar diverge, diferida).
            ExprKind::Func(fnexpr) => {
                // Celdas que ESTA closure captura (var-celda en ámbito referenciadas en su cuerpo): se
                // PRE-CLONAN antes del `move` (`{ let c = c.clone(); Rc::new(move || …) }`), para que el
                // ámbito exterior pueda seguir usándolas y la mutación se comparta (M4).
                let mut refd = std::collections::HashSet::new();
                idents_of_block(&fnexpr.body, &mut refd);
                let captured: Vec<String> = self.cells.iter().filter(|c| refd.contains(*c)).cloned().collect();
                let wrap = !captured.is_empty();
                if wrap {
                    out.push_str("{ ");
                    for c in &captured {
                        write!(out, "let {} = {}.clone(); ", mangle(c), mangle(c)).unwrap();
                    }
                }
                out.push_str("Rc::new(move |");
                for (i, p) in fnexpr.params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write!(out, "{}: {}", p.name, rust_ty(&p.ty, &self.enums, &self.tparams)?).unwrap();
                }
                write!(out, "| -> {} ", rust_ty(&fnexpr.return_type, &self.enums, &self.tparams)?).unwrap();
                self.scopes.push(HashMap::new());
                for p in &fnexpr.params {
                    self.declare(&p.name, p.ty.clone());
                }
                // Las celdas propias de esta closure (una var suya capturada por una closure aún más interna).
                let added = self.enter_cells(&fnexpr.body);
                self.emit_block(out, &fnexpr.body)?;
                self.exit_cells(added);
                self.scopes.pop();
                out.push(')');
                if wrap {
                    out.push_str(" }");
                }
            }
            ExprKind::Match { scrutinee, arms } => self.emit_match(out, scrutinee, arms)?,
            // Operador `?`: sobre Option/Result nativos de Rust → el `?` de Rust (la fn envolvente
            // devuelve un Option/Result compatible, garantizado por el checker).
            ExprKind::Try(inner) => {
                self.emit_expr(out, inner)?;
                out.push('?');
            }
            // Literal de Map: [k1: v1, k2: v2] → HashMap::from([(k1,v1), …]); [:] vacío → HashMap::new().
            ExprKind::MapLit(pairs) => {
                out.push_str("Rc::new(std::cell::RefCell::new(");
                if pairs.is_empty() {
                    out.push_str("__RayMap::new()");
                } else {
                    out.push_str("__RayMap::from([");
                    for (i, (k, v)) in pairs.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        out.push('(');
                        self.emit_expr(out, k)?;
                        out.push_str(", ");
                        self.emit_expr(out, v)?;
                        out.push(')');
                    }
                    out.push_str("])");
                }
                out.push_str("))");
            }
            // (Match exhaustivo sobre ExprKind: toda variante tiene su arm. Una variante nueva del AST
            // hará fallar la compilación aquí → obliga a decidir su bajada, mejor que un error en runtime.)
        }
        Ok(())
    }

    /// Emite `impl Display` para cada struct/enum (= el `Show` de raylang): struct → `Name { f: v, … }`,
    /// enum → `Name.Variant(payload)` / `Name.Variant`. Recursivo (un campo/payload struct se `borrow`ea).
    /// Genera `impl RayShow` para cada struct/enum de usuario (recursivo; genérico-consciente con
    /// `where` `A: RayShow`). struct → `Name { f: v, … }`; enum → `Name.Variant(payload)` / `Name.Variant`.
    fn emit_rayshow_impls(&self, out: &mut String, prog: &Program) -> Result<(), String> {
        // Un tipo-función usado como ELEMENTO de un contenedor (`[fn]`, `Map<_, fn>`, `(fn, …)`) de un campo
        // o payload necesita su `impl RayShow` (el impl genérico del contenedor exige `T: RayShow`). Un `fn`
        // se muestra `<fn>` (como el Display del runtime). Uno por firma concreta distinta (dedup).
        let mut fn_types: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for s in &prog.structs {
            let tps: std::collections::HashSet<String> = s.type_params.iter().cloned().collect();
            for (_, fty) in &s.fields {
                collect_fn_rayshow(fty, &self.enums, &tps, &mut fn_types);
            }
        }
        for e in &prog.enums {
            let tps: std::collections::HashSet<String> = e.type_params.iter().cloned().collect();
            for v in &e.variants {
                for pty in &v.payload {
                    collect_fn_rayshow(pty, &self.enums, &tps, &mut fn_types);
                }
            }
        }
        for ft in &fn_types {
            writeln!(out, "impl RayShow for {} {{ fn ray_show(&self) -> String {{ \"<fn>\".to_string() }} }}", ft).unwrap();
        }
        for s in &prog.structs {
            if s.name == "Iter" || s.name.starts_with("__dyn_") { continue; }
            let gens = generic_decl(&s.type_params);
            let sfx = type_args(&s.type_params);
            // El nombre del TIPO en Rust va manglado (multi-módulo); en la cadena de Display, el nombre
            // COMPLETO namespacado (`geo::Punto`), como el render default de `print` en la VM.
            let sm = mangle(&s.name);
            let mut fmt = format!("{} {{{{ ", s.name);
            let mut args = String::new();
            for (i, (fname, fty)) in s.fields.iter().enumerate() {
                if i > 0 {
                    fmt.push_str(", ");
                }
                write!(fmt, "{}: {{}}", fname).unwrap();
                // Un campo de tipo función se muestra como `<fn>` (como el Display del runtime): los tipos
                // función tienen firmas variadas → no hay un `impl RayShow` único; se renderiza el literal.
                if matches!(normalize_type(fty), Type::Fn(_, _)) {
                    write!(args, ", \"<fn>\"").unwrap();
                } else {
                    write!(args, ", __b.{}.ray_show()", fname).unwrap();
                }
            }
            fmt.push_str(" }}");
            writeln!(out, "impl{} RayShow for Rc<std::cell::RefCell<{}{}>> {{ fn ray_show(&self) -> String {{ let __b = self.borrow(); format!(\"{}\"{}) }} }}", gens, sm, sfx, fmt, args).unwrap();
        }
        for e in &prog.enums {
            if e.name == "Option" || e.name == "Result" {
                continue;
            }
            let gens = generic_decl(&e.type_params);
            let sfx = type_args(&e.type_params);
            // Rust manglado; Display con el nombre COMPLETO (como el render default de `print` en la VM).
            let em = mangle(&e.name);
            writeln!(out, "impl{} RayShow for Rc<{}{}> {{ fn ray_show(&self) -> String {{ match &**self {{", gens, em, sfx).unwrap();
            for v in &e.variants {
                if v.payload.is_empty() {
                    writeln!(out, "{}::{} => \"{}.{}\".to_string(),", em, v.name, e.name, v.name).unwrap();
                } else {
                    let binds: Vec<String> = (0..v.payload.len()).map(|i| format!("__p{}", i)).collect();
                    let mut fmt = format!("{}.{}(", e.name, v.name);
                    let mut args = String::new();
                    for (i, pty) in v.payload.iter().enumerate() {
                        if i > 0 {
                            fmt.push_str(", ");
                        }
                        fmt.push_str("{}");
                        // Payload de tipo función → `<fn>` literal (como en los campos de struct).
                        if matches!(normalize_type(pty), Type::Fn(_, _)) {
                            write!(args, ", \"<fn>\"").unwrap();
                        } else {
                            write!(args, ", {}.ray_show()", binds[i]).unwrap();
                        }
                    }
                    fmt.push(')');
                    writeln!(out, "{}::{}({}) => format!(\"{}\"{}),", em, v.name, binds.join(", "), fmt, args).unwrap();
                }
            }
            out.push_str("} } }\n");
        }
        Ok(())
    }

    /// Baja un `match` sobre un enum. El escrutinio (`Rc<E>`) se liga a un temporal y se matchea sobre
    /// `&*temp` (matchea `&E`). Los bindings del patrón quedan como `&campo`; al inicio de cada brazo se
    /// **clonan a valores propios** (`let b = b.clone();`) → el cuerpo los usa como cualquier variable.
    fn emit_match(&mut self, out: &mut String, scrutinee: &Expr, arms: &[MatchArm]) -> Result<(), String> {
        let scrut_ty = normalize_type(&self.type_of(scrutinee)?);
        // Option/Result son NATIVOS de Rust (no `Rc<E>`): se matchea sobre `&opt`, no `&*Rc`.
        let native = match &scrut_ty {
            Type::Enum(n, _) => n == "Option" || n == "Result",
            other => return Err(format!("spike: match sobre {:?} (se esperaba un enum)", other)),
        };
        let temp = format!("__scrut{}", self.match_temp);
        self.match_temp += 1;
        out.push_str("{ let ");
        out.push_str(&temp);
        out.push_str(" = ");
        self.emit_expr(out, scrutinee)?;
        out.push_str(if native { "; match &" } else { "; match &*" });
        out.push_str(&temp);
        out.push_str(" {\n");
        for arm in arms {
            if arm.guard.is_some() {
                return Err("spike: guardas de match (`if`) no soportadas".into());
            }
            self.scopes.push(HashMap::new());
            let mut binds: Vec<(String, Type)> = Vec::new();
            // El binding de TODO el escrutinio (`x => …`) es un caso especial: liga el `Rc<E>` (temp),
            // no un `&campo`. Se emite `_` y se clona desde el temporal.
            let whole_binding = match &arm.pattern.kind {
                PatternKind::Binding(x) => Some(x.clone()),
                _ => None,
            };
            if let Some(x) = &whole_binding {
                out.push('_');
                self.declare(x, scrut_ty.clone());
            } else {
                self.emit_pattern(out, &arm.pattern, &scrut_ty, &mut binds)?;
            }
            out.push_str(" => {\n");
            // Los bindings se emiten manglados (pueden ser temps `$…` del checker); `declare` usa el nombre
            // crudo (el que llevan los `Ident` del AST) → los usos, que también manglan, casan.
            if let Some(x) = &whole_binding {
                writeln!(out, "let {} = {}.clone();", mangle(x), temp).unwrap();
            }
            for (b, bt) in &binds {
                writeln!(out, "let {} = {}.clone();", mangle(b), mangle(b)).unwrap();
                self.declare(b, bt.clone());
            }
            self.emit_expr(out, &arm.body)?;
            out.push_str("\n}\n");
            self.scopes.pop();
        }
        out.push_str("} }");
        Ok(())
    }

    /// Emite un patrón como patrón de Rust y recolecta sus bindings (nombre, tipo). `expected` es el tipo
    /// del valor que el patrón matchea (para el tipo de un `Binding`). Un nombre de binding se emite tal
    /// cual (Rust lo liga a `&campo`); `_` a comodín; una variante anidada recursivamente.
    fn emit_pattern(
        &self,
        out: &mut String,
        pat: &Pattern,
        expected: &Type,
        binds: &mut Vec<(String, Type)>,
    ) -> Result<(), String> {
        match &pat.kind {
            PatternKind::Wildcard => out.push('_'),
            PatternKind::Binding(x) => {
                out.push_str(&mangle(x)); // el bind puede ser un temp `$…` del checker (`?` From-conv)
                binds.push((x.clone(), expected.clone()));
            }
            PatternKind::Variant { enum_name, variant, subpatterns } => {
                let native = enum_name == "Option" || enum_name == "Result";
                if native {
                    out.push_str(variant); // Some / None / Ok / Err (nativos, sin `EnumName::`)
                } else {
                    write!(out, "{}::{}", mangle(enum_name), variant).unwrap();
                }
                if !subpatterns.is_empty() {
                    // Payload: user enum → tabla de variantes; Option/Result → los args del tipo esperado
                    // (`Some(T)`/`Ok(T)` = args[0], `Err(E)` = args[1]).
                    let payload: Vec<Type> = if native {
                        match normalize_type(expected) {
                            Type::Enum(_, args) => match variant.as_str() {
                                "Some" | "Ok" => vec![args[0].clone()],
                                "Err" => vec![args[1].clone()],
                                _ => vec![],
                            },
                            _ => return Err("spike: patrón Option/Result sin tipo esperado".into()),
                        }
                    } else {
                        let raw = self
                            .enum_variants
                            .get(enum_name)
                            .and_then(|m| m.get(variant))
                            .ok_or_else(|| format!("spike: variante desconocida {}.{}", enum_name, variant))?
                            .clone();
                        // Sustituir los params de tipo del enum por los args del tipo esperado
                        // (`Caja<int>` → T=int), para el tipo de cada binding del payload.
                        let subst = enum_subst(&self.enum_tparams, enum_name, expected);
                        raw.iter().map(|p| subst_type(p, &subst)).collect()
                    };
                    out.push('(');
                    for (i, sp) in subpatterns.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_pattern(out, sp, &payload[i], binds)?;
                    }
                    out.push(')');
                }
            }
            PatternKind::Struct { .. } => {
                return Err("spike: patrón de destructuración de struct no soportado".into())
            }
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
                // to_string(x) / x.to_string(): si `x` es un PRIMITIVO Display (int/float/bool/char/string)
                // se inlina como `{}` sobre `x` (sin Rc intermedio). Si no (bytes/struct/enum/array — no
                // son Display en Rust), se emite el `to_string(x)` entero → `Rc<str>` (que sí es Display).
                ExprKind::Call { callee, args: cargs } if is_to_string(callee) => {
                    fmt.push_str("{}");
                    let (_, recv) = resolve_callee(callee)?;
                    let arg = recv.unwrap_or(&cargs[0]);
                    if is_display_primitive(&self.type_of(arg)?) {
                        args.push(arg);
                    } else {
                        args.push(op);
                    }
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

    /// `std::math::<fn>(args)` → el método de `f64` de Rust equivalente. Unarias float→float directas;
    /// pow→powf; abs/min/max preservan el tipo (int|float, ambos con esos métodos en Rust).
    /// `std::fs::<fn>(args)` → I/O de archivos con `std::fs`/`std::io` de Rust. Ok/Err como la VM (mensajes
    /// vía `e.to_string()`). No determinista → probado por subproceso (no oráculo). Se cubre la ENTRADA
    /// (read_file) + la salida básica (write_file) + la consulta (exists); el resto → error claro.
    fn emit_fs(&mut self, out: &mut String, ffn: &str, eff: &[&Expr]) -> Result<(), String> {
        match ffn {
            // read_file(path) -> Result<string, string>: lee el archivo entero a un string.
            "read_file" => {
                out.push_str("(match std::fs::read_to_string(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(
                    ") { Ok(__c) => Ok::<Rc<str>, Rc<str>>(Rc::<str>::from(__c)), \
                     Err(__e) => Err(Rc::<str>::from(__e.to_string())) })",
                );
            }
            // write_file(path, content) -> Result<int, string>: escribe (trunca); Ok(0) como la VM.
            "write_file" => {
                out.push_str("(match std::fs::write(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push_str(
                    ".as_bytes()) { Ok(()) => Ok::<i64, Rc<str>>(0i64), \
                     Err(__e) => Err(Rc::<str>::from(__e.to_string())) })",
                );
            }
            // exists(path) -> bool.
            "exists" => {
                out.push_str("std::path::Path::new(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(").exists()");
            }
            // remove_file(path) -> Result<int, string>: borra un archivo; Ok(0) u Err(mensaje).
            "remove_file" => {
                out.push_str("(match std::fs::remove_file(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(
                    ") { Ok(()) => Ok::<i64, Rc<str>>(0i64), Err(__e) => Err(Rc::<str>::from(__e.to_string())) })",
                );
            }
            // list_dir(path) -> Result<[string], string>: nombres de las entradas, ORDENADOS (como la VM
            // → determinista). El arreglo usa la repr del transpilador (Rc<RefCell<Vec<Rc<str>>>>).
            "list_dir" => {
                out.push_str("(match std::fs::read_dir(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(
                    ") { Ok(__rd) => { \
                     let mut __ns: Vec<Rc<str>> = __rd.filter_map(|__e| __e.ok()) \
                     .map(|__e| Rc::<str>::from(__e.file_name().to_string_lossy().into_owned())).collect(); \
                     __ns.sort(); \
                     Ok::<Rc<std::cell::RefCell<Vec<Rc<str>>>>, Rc<str>>(Rc::new(std::cell::RefCell::new(__ns))) }, \
                     Err(__e) => Err(Rc::<str>::from(__e.to_string())) })",
                );
            }
            // Handles de archivo (M11.8): un registro global (espejo del FileRegistry de la VM). open →
            // Result<int,string>, read_line → Option<string> (bufferizada, sin '\n'), write → Result<int,string>.
            "open" => {
                self.needs_handles = true;
                out.push_str("__ray_open(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "read_line" => {
                self.needs_handles = true;
                out.push_str("__ray_read_line(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "write" => {
                self.needs_handles = true;
                out.push_str("__ray_write(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // Operaciones de directorio con resultado unitario → Result<int,string> (Ok(0)/Err(msg)):
            // mkdir (create_dir_all), remove_dir (solo vacío), rename, copy_file (std::fs::copy).
            "mkdir" | "remove_dir" | "rename" | "copy_file" => {
                let (rust_fn, two_args, map_unit) = match ffn {
                    "mkdir" => ("std::fs::create_dir_all", false, false),
                    "remove_dir" => ("std::fs::remove_dir", false, false),
                    "rename" => ("std::fs::rename", true, false),
                    _ => ("std::fs::copy", true, true), // copy devuelve u64 → .map(|_| ())
                };
                write!(out, "(match {}(&*", rust_fn).unwrap();
                self.emit_expr(out, eff[0])?;
                if two_args {
                    out.push_str(", &*");
                    self.emit_expr(out, eff[1])?;
                }
                out.push(')');
                if map_unit {
                    out.push_str(".map(|_| ())");
                }
                out.push_str(
                    " { Ok(()) => Ok::<i64, Rc<str>>(0i64), Err(__e) => Err(Rc::<str>::from(__e.to_string())) })",
                );
            }
            // is_dir/is_file(path) -> bool (totales, nunca fallan).
            "is_dir" | "is_file" => {
                out.push_str("std::path::Path::new(&*");
                self.emit_expr(out, eff[0])?;
                write!(out, ").{}()", ffn).unwrap();
            }
            // file_size(path) -> Result<int,string>: tamaño en bytes; un directorio es error (mensaje
            // byte-idéntico a la VM: "no es un file").
            "file_size" => {
                out.push_str("(match std::fs::metadata(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(
                    ") { Ok(__md) if __md.is_file() => Ok::<i64, Rc<str>>(__md.len() as i64), \
                     Ok(_) => Err(Rc::<str>::from(\"no es un file\")), \
                     Err(__e) => Err(Rc::<str>::from(__e.to_string())) })",
                );
            }
            // I/O binaria: read_file_bytes -> Result<bytes,string>; write/append_file_bytes -> Result<int,string>.
            "read_file_bytes" => {
                out.push_str("(match std::fs::read(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(
                    ") { Ok(__b) => Ok::<Rc<[u8]>, Rc<str>>(Rc::<[u8]>::from(__b)), \
                     Err(__e) => Err(Rc::<str>::from(__e.to_string())) })",
                );
            }
            "write_file_bytes" => {
                out.push_str("(match std::fs::write(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push_str(") { Ok(()) => Ok::<i64, Rc<str>>(");
                self.emit_expr(out, eff[1])?;
                out.push_str(".len() as i64), Err(__e) => Err(Rc::<str>::from(__e.to_string())) })");
            }
            "append_file_bytes" => {
                out.push_str(
                    "(match std::fs::OpenOptions::new().create(true).append(true).open(&*",
                );
                self.emit_expr(out, eff[0])?;
                out.push_str(").and_then(|mut __f| { use std::io::Write; __f.write_all(&*");
                self.emit_expr(out, eff[1])?;
                out.push_str(") }) { Ok(()) => Ok::<i64, Rc<str>>(");
                self.emit_expr(out, eff[1])?;
                out.push_str(".len() as i64), Err(__e) => Err(Rc::<str>::from(__e.to_string())) })");
            }
            _ => return Err(format!("spike: std::fs::{} no soportada", ffn)),
        }
        Ok(())
    }

    /// `std::time::<fn>`: now/monotonic → int (millis), sleep(ms) → duerme. now/sleep inline; monotonic
    /// usa un `Instant` global (helper `__ray_monotonic`, activa `needs_time_rng`).
    fn emit_time(&mut self, out: &mut String, tfn: &str, eff: &[&Expr]) -> Result<(), String> {
        match tfn {
            "now" => out.push_str(
                "(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|__d| __d.as_millis() as i64).unwrap_or(0))",
            ),
            "monotonic" => {
                self.needs_time_rng = true;
                out.push_str("__ray_monotonic()");
            }
            "sleep" => {
                out.push_str("std::thread::sleep(std::time::Duration::from_millis((");
                self.emit_expr(out, eff[0])?;
                out.push_str(").max(0) as u64))");
            }
            _ => return Err(format!("spike: std::time::{} no soportada", tfn)),
        }
        Ok(())
    }

    /// `std::random::<fn>`: next() → float [0,1); below(n) → int [0,n); seed(n) fija la semilla. PRNG
    /// SplitMix64 propio (mismo que la VM) con estado global; no determinista → casa por propiedades.
    fn emit_random(&mut self, out: &mut String, rfn: &str, eff: &[&Expr]) -> Result<(), String> {
        self.needs_time_rng = true;
        match rfn {
            "next" => out.push_str("__ray_random_f64()"),
            "below" => {
                out.push_str("__ray_random_int(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "seed" => {
                out.push_str("__ray_random_seed(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            _ => return Err(format!("spike: std::random::{} no soportada", rfn)),
        }
        Ok(())
    }

    /// `std::net::<fn>` (sockets TCP) → los helpers `__ray_tcp_*`/`__ray_socket_*` del registro de handles
    /// (activa `needs_net`). connect/listen/accept → Result<int,string>; read → Result<string,string>;
    /// read_bytes → Result<bytes,string>; write/write_bytes → Result<int,string>; local_port → int.
    fn emit_net(&mut self, out: &mut String, nfn: &str, eff: &[&Expr]) -> Result<(), String> {
        self.needs_net = true;
        match nfn {
            "tcp_connect" | "tcp_listen" => {
                let f = if nfn == "tcp_connect" { "__ray_tcp_connect" } else { "__ray_tcp_listen" };
                write!(out, "{}(&*", f).unwrap();
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "tcp_accept" => {
                out.push_str("__ray_tcp_accept(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "socket_read" | "socket_read_bytes" | "local_port" => {
                let f = match nfn {
                    "socket_read" => "__ray_socket_read",
                    "socket_read_bytes" => "__ray_socket_read_bytes",
                    _ => "__ray_local_port",
                };
                write!(out, "{}(", f).unwrap();
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // set_read_timeout(h, ms) -> unit: fija el timeout de lectura del socket (ms<=0 → sin límite).
            "set_read_timeout" => {
                out.push_str("__ray_set_read_timeout(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // socket_write(h, s) → escribe los bytes UTF-8 del string; socket_write_bytes(h, data) → los bytes.
            "socket_write" | "socket_write_bytes" => {
                out.push_str("__ray_socket_write(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                if nfn == "socket_write" {
                    self.emit_expr(out, eff[1])?;
                    out.push_str(".as_bytes()");
                } else {
                    out.push_str("&*");
                    self.emit_expr(out, eff[1])?;
                }
                out.push(')');
            }
            _ => return Err(format!("spike: std::net::{} no soportada", nfn)),
        }
        Ok(())
    }

    fn emit_math(&mut self, out: &mut String, mfn: &str, eff: &[&Expr]) -> Result<(), String> {
        const UNARY: &[&str] = &[
            "sqrt", "sin", "cos", "tan", "ln", "log10", "log2", "exp", "floor", "ceil", "round", "trunc",
            "asin", "acos", "atan",
        ];
        if UNARY.contains(&mfn) {
            out.push('(');
            self.emit_expr(out, eff[0])?;
            write!(out, ").{}()", mfn).unwrap();
            return Ok(());
        }
        let bin = |m: &str| -> &'static str {
            match m {
                "pow" => "powf",
                _ => "",
            }
        };
        match mfn {
            "pow" | "min" | "max" => {
                out.push('(');
                self.emit_expr(out, eff[0])?;
                let m = if mfn == "pow" { bin("pow") } else { mfn };
                write!(out, ").{}(", m).unwrap();
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "abs" => {
                out.push('(');
                self.emit_expr(out, eff[0])?;
                out.push_str(").abs()");
            }
            _ => return Err(format!("spike: std::math::{} no soportada", mfn)),
        }
        Ok(())
    }

    fn emit_call(&mut self, out: &mut String, callee: &Expr, args: &[Expr]) -> Result<(), String> {
        let (name, recv) = resolve_callee(callee)?;
        // Despacho dinámico (M9.3b): el checker baja `obj.m(a)` a `(r.m)(r.data, a)` con `r: dyn`. Aquí el
        // campo `m` es una closure que capturó el concreto → `(r.borrow().m.clone())(a)` (se descarta el
        // arg `r.data` que añadió el checker: es `args[0]`).
        if let Some(r) = recv {
            if matches!(self.type_of(r).ok(), Some(Type::Dyn(_))) {
                out.push('(');
                self.emit_expr(out, r)?;
                write!(out, ".borrow().{}.clone())(", name).unwrap();
                for (i, a) in args.iter().skip(1).enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    self.emit_expr(out, a)?;
                }
                out.push(')');
                return Ok(());
            }
        }
        // Llamada a un CAMPO-closure: `r.f(args)` donde `f` es un campo de tipo función del struct receptor
        // (p. ej. `self.step()` en `Iter#next`, con `step: fn() -> Option<T>`). Prioridad campo→método (como
        // el checker). Baja a `(r.borrow().f.clone())(args)`; el receptor NO se pasa (el closure no lleva self).
        if let Some(r) = recv {
            let rty = self.type_of(r).ok().map(|t| normalize_type(&t));
            if let Some(Type::Struct(sname, _)) = &rty {
                let is_fn_field = self
                    .struct_fields
                    .get(sname)
                    .and_then(|fs| fs.iter().find(|(fnm, _)| fnm.as_str() == name))
                    .map(|(_, fty)| matches!(normalize_type(fty), Type::Fn(_, _)))
                    .unwrap_or(false);
                if is_fn_field {
                    out.push('(');
                    self.emit_expr(out, r)?;
                    write!(out, ".borrow().{}.clone())(", name).unwrap();
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_expr(out, a)?;
                    }
                    out.push(')');
                    return Ok(());
                }
            }
        }
        // Argumentos efectivos: el receptor de UFCS (si lo hay) va primero.
        let eff: Vec<&Expr> = recv.into_iter().chain(args.iter()).collect();
        // `std::math::*` (módulo std/math) → los métodos de `f64` de Rust (misma impl que la VM → mismo
        // resultado). abs/min/max son ad-hoc int|float (ambos tienen esos métodos en Rust).
        if let Some(mfn) = name.strip_prefix("std::math::") {
            return self.emit_math(out, mfn, &eff);
        }
        // `std::fs::*` (módulo std/fs) → I/O de archivos con `std::fs`/`std::io` de Rust (Ok/Err como la VM).
        if let Some(ffn) = name.strip_prefix("std::fs::") {
            return self.emit_fs(out, ffn, &eff);
        }
        // `std::time::{now,monotonic,sleep}`/`std::random::{next,below,seed}` → reloj + PRNG de Rust (no
        // deterministas → casan por propiedades). El resto de std/time|random es raylang puro → pasa de largo.
        if let Some(tfn) = name.strip_prefix("std::time::") {
            if matches!(tfn, "now" | "monotonic" | "sleep") {
                return self.emit_time(out, tfn, &eff);
            }
        }
        if let Some(rfn) = name.strip_prefix("std::random::") {
            if matches!(rfn, "next" | "below" | "seed") {
                return self.emit_random(out, rfn, &eff);
            }
        }
        // `std::net::*` (sockets TCP) → std::net de Rust (registro de handles compartido con archivos).
        if let Some(nfn) = name.strip_prefix("std::net::") {
            if matches!(
                nfn,
                "tcp_connect" | "tcp_listen" | "tcp_accept" | "socket_read" | "socket_read_bytes"
                    | "socket_write" | "socket_write_bytes" | "local_port" | "set_read_timeout"
            ) {
                return self.emit_net(out, nfn, &eff);
            }
        }
        // Métodos de la stdlib manglados por el checker (`string#len`, `Len` trait…): el método real es
        // lo que va tras el último `#`. Los nombres de usuario no llevan `#` (ilegal) → quedan intactos.
        let method = name.rsplit('#').next().unwrap_or(name).trim_start_matches("__");
        // `Map.new()` (función asociada): un HashMap vacío. El elemento lo infiere Rust del uso/anotación.
        if method == "new" {
            if let Some(o) = recv {
                if matches!(&o.kind, ExprKind::Ident(n) if n == "Map") {
                    out.push_str("Rc::new(std::cell::RefCell::new(__RayMap::new()))");
                    return Ok(());
                }
                // `Channel.new()` (función asociada): canal MPMC no acotado.
                if matches!(&o.kind, ExprKind::Ident(n) if n == "Channel") {
                    self.needs_concurrency = true;
                    out.push_str("__RayChan::make(None)");
                    return Ok(());
                }
            }
        }
        // `Channel.bounded(n)` (función asociada): canal MPMC acotado a `n` (backpressure).
        if method == "bounded" {
            if let Some(o) = recv {
                if matches!(&o.kind, ExprKind::Ident(n) if n == "Channel") {
                    self.needs_concurrency = true;
                    out.push_str("__RayChan::make(Some((");
                    self.emit_expr(out, &args[0])?;
                    out.push_str(") as usize))");
                    return Ok(());
                }
            }
        }
        match method {
            // args() → [string]: los argumentos de línea de comandos tras el binario. La VM devuelve
            // argv tras el `.ray`; el nativo, tras el binario (`skip(1)`) → equivalen. Repr = arreglo.
            "args" => {
                out.push_str(
                    "Rc::new(std::cell::RefCell::new(std::env::args().skip(1)\
                     .map(|__a| Rc::<str>::from(__a)).collect::<Vec<Rc<str>>>()))",
                );
            }
            "print" | "eprint" => {
                // Uniforme vía RayShow (maneja todo tipo, incl. structs/arreglos/genéricos). eprint → stderr.
                let macro_name = if method == "eprint" { "eprintln" } else { "println" };
                if matches!(self.type_of(eff[0])?, Type::Fn(_, _)) {
                    write!(out, "{}!(\"<fn>\")", macro_name).unwrap(); // una función se muestra como <fn>
                } else {
                    write!(out, "{}!(\"{{}}\", ", macro_name).unwrap();
                    self.emit_expr(out, eff[0])?;
                    out.push_str(".ray_show())");
                }
            }
            // UDP: primitivos `__udp_*` (los llaman los wrappers de raylang de udp.ray). Devuelven arreglos
            // etiquetados; recv_from es un arreglo de bytes. Activan `needs_net` (registro de handles).
            "udp_bind" => {
                self.needs_net = true;
                out.push_str("__ray_udp_bind(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "udp_send_to" => {
                self.needs_net = true;
                out.push_str("__ray_udp_send_to(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push_str(", ");
                self.emit_expr(out, eff[2])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[3])?;
                out.push(')');
            }
            "udp_recv_from" => {
                self.needs_net = true;
                out.push_str("__ray_udp_recv_from(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // char_code(c) -> int: el code point Unicode del char (paréntesis por el `as`, como bytes[i]).
            "char_code" => {
                out.push('(');
                self.emit_expr(out, eff[0])?;
                out.push_str(" as u32 as i64)");
            }
            // char_from_code(n) -> Option<char>: el char del code point (None si inválido, como la VM).
            "char_from_code" => {
                out.push_str("char::from_u32(");
                self.emit_expr(out, eff[0])?;
                out.push_str(" as u32)");
            }
            // bytes_of([int]) -> bytes: construye bytes de un arreglo de octetos (cada 0–255, `as u8`).
            "bytes_of" => {
                out.push_str("Rc::<[u8]>::from(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().map(|__x| *__x as u8).collect::<Vec<u8>>())");
            }
            // Más builtins de string (→ métodos de `str`/`String` de Rust, misma semántica que la VM).
            "trim" => {
                out.push_str("Rc::<str>::from(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".trim())");
            }
            // index_of(s, sub) -> Option<int>: índice por carácter de la subcadena (helper del preámbulo).
            "index_of" => {
                out.push_str("__ray_index_of(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "to_upper" | "to_lower" => {
                let m = if method == "to_upper" { "to_uppercase" } else { "to_lowercase" };
                out.push_str("Rc::<str>::from(");
                self.emit_expr(out, eff[0])?;
                write!(out, ".{}())", m).unwrap();
            }
            "starts_with" | "ends_with" => {
                self.emit_expr(out, eff[0])?;
                write!(out, ".{}(&*", method).unwrap();
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // repeat(s, n): n<=0 → "" (como la VM).
            "repeat" => {
                out.push_str("{ let __n = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; if __n <= 0 { Rc::<str>::from(\"\") } else { Rc::<str>::from(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".repeat(__n as usize)) } }");
            }
            "replace" => {
                out.push_str("Rc::<str>::from(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".replace(&*");
                self.emit_expr(out, eff[1])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[2])?;
                out.push_str("))");
            }
            // substring(s, i, j): corte por CARÁCTER con clamp (nunca falla), como la VM.
            "substring" => {
                out.push_str("{ let __c: Vec<char> = ");
                self.emit_expr(out, eff[0])?;
                out.push_str(".chars().collect(); let __n = __c.len() as i64; let __lo = (");
                self.emit_expr(out, eff[1])?;
                out.push_str(").clamp(0, __n); let __hi = (");
                self.emit_expr(out, eff[2])?;
                out.push_str(").clamp(__lo, __n); Rc::<str>::from(__c[__lo as usize..__hi as usize].iter().collect::<String>()) }");
            }
            // to_string(x) → Rc<str>. Vía show_expr (maneja struct→borrow, arreglo→[…], escalar/enum).
            "to_string" => {
                out.push_str("Rc::<str>::from(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".ray_show())");
            }
            // len(x) → i64. String: nº de octetos; arreglo: nº de elementos (vía borrow()).
            "len" => {
                out.push('(');
                self.emit_expr(out, eff[0])?;
                match self.type_of(eff[0])? {
                    Type::Array(_) | Type::Map(_, _) => out.push_str(".borrow().len() as i64)"),
                    // string: `len` cuenta CARACTERES (como la VM), no bytes — clave con UTF-8 multibyte
                    // (`más`, `ñ`): usar `.len()` (bytes) haría que `while i < len` sobre-itere `s[i]`.
                    Type::String => out.push_str(".chars().count() as i64)"),
                    // bytes: `len` es el nº de octetos → `.len()` es correcto.
                    _ => out.push_str(".len() as i64)"),
                }
            }
            // push(a, v) → a.borrow_mut().push(v) (muta en el sitio, devuelve unit).
            "push" => {
                // El valor se evalúa a un TEMP ANTES del borrow_mut: si lee del MISMO arreglo (p. ej.
                // `w.push(w[i] + w[j])`, típico en cripto), evita el doble borrow del RefCell (panic).
                out.push_str("{ let __v = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; ");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow_mut().push(__v); }");
            }
            // chars(s) → [char]: los caracteres del string como arreglo.
            "chars" => {
                out.push_str("Rc::new(std::cell::RefCell::new(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".chars().collect::<Vec<char>>()))");
            }
            // split(s, sep) → [string]; join(a, sep) → string (helpers del preámbulo generado).
            "split" => {
                out.push_str("__ray_split(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // join(t) → t.join() (Task, structured concurrency); join(arr, sep) → __ray_join (string). El
            // `join` es ad-hoc: se distingue por el tipo del primer arg (Task vs arreglo).
            "join" if matches!(self.type_of(eff[0])?, Type::Task(_)) => {
                // t.join() da la repr SEND; se convierte de vuelta a la del programa (string/bytes → Rc).
                let elem = match self.type_of(eff[0])? {
                    Type::Task(t) => (*t).clone(),
                    _ => unreachable!("guard garantiza Task"),
                };
                let (pre, post): (&str, &str) = match normalize_type(&elem) {
                    Type::String => ("Rc::<str>::from(&*", ")"),
                    Type::Bytes => ("Rc::<[u8]>::from(&*", ")"),
                    _ => ("", ""),
                };
                out.push_str(pre);
                self.emit_expr(out, eff[0])?;
                out.push_str(".join()");
                out.push_str(post);
            }
            "join" => {
                out.push_str("__ray_join(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // --- Map ---
            "insert" => {
                // devuelve unit → bloque con `;` (HashMap::insert de Rust devuelve Option).
                out.push('{');
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow_mut().insert(");
                self.emit_expr(out, eff[1])?;
                out.push_str(", ");
                self.emit_expr(out, eff[2])?;
                out.push_str(");}");
            }
            // add_to(m, k, delta): `*m.entry(k).or_insert(0) += delta` (upsert acumulativo, como la VM).
            "add_to" => {
                let zero = match self.type_of(eff[0])? {
                    Type::Map(_, v) if matches!(*v, Type::Float) => "0.0",
                    _ => "0i64",
                };
                out.push_str("(*");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow_mut().entry(");
                self.emit_expr(out, eff[1])?;
                write!(out, ").or_insert({}) += ", zero).unwrap();
                self.emit_expr(out, eff[2])?;
                out.push(')');
            }
            // get_or(m, k, default) — el del prelude lleva 3 args; un `get_or` con otra aridad no es este
            // builtin → cae al fallback (evita el pánico por `eff[2]` inexistente).
            "get_or" if eff.len() == 3 => {
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().get(&");
                self.emit_expr(out, eff[1])?;
                out.push_str(").cloned().unwrap_or(");
                self.emit_expr(out, eff[2])?;
                out.push(')');
            }
            // get(m, k) → Option<V> (Rust). Se usa fusionado con `.unwrap_or(d)` (nativo de Option).
            "get" => {
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().get(&");
                self.emit_expr(out, eff[1])?;
                out.push_str(").cloned()");
            }
            // remove(m, k) → Option<V> (quita y devuelve). Fusionado con unwrap_or.
            "remove" => {
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow_mut().remove(&");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "contains_key" => {
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().contains_key(&");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "keys" => {
                out.push_str("__ray_keys(&");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "values" => {
                out.push_str("__ray_values(&");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "sort" => {
                out.push_str("__ray_sort(&");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // parse_int(s) → Rust Option<i64>; se usa fusionado con `.unwrap_or(d)` (nativo de Option).
            "parse_int" => {
                out.push('(');
                self.emit_expr(out, eff[0])?;
                out.push_str(".parse::<i64>().ok())");
            }
            "parse_float" => {
                out.push('(');
                self.emit_expr(out, eff[0])?;
                out.push_str(".parse::<f64>().ok())");
            }
            // contains ad-hoc: string → subcadena; bytes → subsecuencia; arreglo → pertenencia (==).
            "contains" => match self.type_of(eff[0])? {
                Type::String => {
                    self.emit_expr(out, eff[0])?;
                    out.push_str(".contains(&*");
                    self.emit_expr(out, eff[1])?;
                    out.push(')');
                }
                Type::Bytes => {
                    out.push_str("{ let __s = ");
                    self.emit_expr(out, eff[1])?;
                    out.push_str("; __s.is_empty() || ");
                    self.emit_expr(out, eff[0])?;
                    out.push_str(".windows(__s.len().max(1)).any(|__w| __w == &*__s) }");
                }
                Type::Array(_) => {
                    out.push_str("{ let __x = ");
                    self.emit_expr(out, eff[1])?;
                    out.push_str("; ");
                    self.emit_expr(out, eff[0])?;
                    out.push_str(".borrow().iter().any(|__e| *__e == __x) }");
                }
                other => return Err(format!("spike: contains sobre {:?}", other)),
            },
            // Bytes: to_bytes(s) codifica un string a UTF-8; from_utf8(b) decodifica (Result); sub_bytes
            // corta [i,j) por octeto con clamp (nunca falla). Repr = Rc<[u8]>.
            "to_bytes" => {
                out.push_str("Rc::<[u8]>::from(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".as_bytes())");
            }
            "from_utf8" => {
                out.push_str("(match std::str::from_utf8(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(
                    ") { Ok(__s) => Ok::<Rc<str>, Rc<str>>(Rc::<str>::from(__s)), \
                     Err(__e) => Err(Rc::<str>::from(__e.to_string())) })",
                );
            }
            "sub_bytes" => {
                out.push_str("{ let __b = ");
                self.emit_expr(out, eff[0])?;
                out.push_str("; let __n = __b.len() as i64; let __lo = (");
                self.emit_expr(out, eff[1])?;
                out.push_str(").clamp(0, __n); let __hi = (");
                self.emit_expr(out, eff[2])?;
                out.push_str(").clamp(__lo, __n); Rc::<[u8]>::from(&__b[__lo as usize..__hi as usize]) }");
            }
            // I/O de ENTRADA (no determinista → sin oráculo; probado por subproceso, como tests/io_cli.rs).
            // `input() -> Option<string>`: una línea de stdin, sin '\n'/'\r' finales (como la VM); None en EOF.
            "input" => {
                out.push_str(
                    "{ let mut __s = String::new(); match std::io::stdin().read_line(&mut __s) \
                     { Ok(0) | Err(_) => None, Ok(_) => Some(Rc::<str>::from(__s.trim_end_matches(['\\n', '\\r']))) } }",
                );
            }
            // `env(name) -> Option<string>`: variable de entorno; None si no está (como la VM).
            "env" => {
                out.push_str("std::env::var(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(").ok().map(Rc::<str>::from)");
            }
            // `read_int() -> Option<int>` = input() + parse_int (composición del prelude).
            "read_int" => {
                out.push_str(
                    "{ let mut __s = String::new(); match std::io::stdin().read_line(&mut __s) \
                     { Ok(0) | Err(_) => None, Ok(_) => __s.trim_end_matches(['\\n', '\\r']).parse::<i64>().ok() } }",
                );
            }
            // `close(h) -> int` (builtin pelado, ad-hoc): un handle de archivo (int) → lo quita del registro
            // y devuelve 0 (el caso de canal es concurrencia, fuera del subconjunto).
            // `close` ad-hoc: un CANAL (concurrencia) → `.close()` (unit); un handle de archivo (int) → 0.
            "close" if matches!(self.type_of(eff[0])?, Type::Channel(_)) => {
                self.emit_expr(out, eff[0])?;
                out.push_str(".close()");
            }
            "close" => {
                self.needs_handles = true;
                out.push_str("__ray_close(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // Canales (concurrencia): send(ch, v) → ch.send(v); recv(ch) → ch.recv() (Option<T>).
            "send" => {
                // El valor se convierte a la repr SEND del canal (string/bytes → Arc; primitivos igual).
                let elem = match self.type_of(eff[0])? {
                    Type::Channel(t) => (*t).clone(),
                    other => return Err(format!("spike: send sobre {:?}", other)),
                };
                self.emit_expr(out, eff[0])?;
                out.push_str(".send(");
                self.emit_to_send(out, eff[1], &elem)?;
                out.push(')');
            }
            "recv" => {
                // recv devuelve Option<repr-send>; se convierte de vuelta a la repr del programa (Rc).
                let elem = match self.type_of(eff[0])? {
                    Type::Channel(t) => (*t).clone(),
                    other => return Err(format!("spike: recv sobre {:?}", other)),
                };
                self.emit_expr(out, eff[0])?;
                out.push_str(".recv()");
                out.push_str(from_send_map(&elem));
            }
            // signals() -> Channel<int>: canal de señales del SO (SIGTERM/SIGINT), singleton (self-pipe).
            "signals" => {
                self.needs_concurrency = true;
                self.needs_signals = true;
                out.push_str("__ray_signals()");
            }
            // select([chs]) -> int: índice del primer canal listo para recibir (poll del índice menor).
            "select" => {
                self.needs_concurrency = true;
                out.push_str("__ray_select(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow()[..])");
            }
            // spawn(f) → __ray_spawn(move || {...}) → Task<T> (JoinHandle envuelto, registrado en el scope
            // activo). scope(f) → __ray_scope(move || {...}): corre el cuerpo y une las tareas de dentro. `f`
            // es una función anónima literal `fn(){}` (captura valores Send, p. ej. canales) O el NOMBRE de
            // una función de nivel superior de aridad 0 (`spawn(worker)` → `move || worker()`; sin captura).
            "spawn" | "scope" => {
                self.needs_concurrency = true;
                let named: Option<String> = match &eff[0].kind {
                    ExprKind::Func(_) => None,
                    ExprKind::Ident(n) if self.funcs.contains_key(n) => {
                        if !self.funcs[n].params.is_empty() {
                            return Err(format!("spike: {} de una función con parámetros ('{}')", method, n));
                        }
                        Some(n.clone())
                    }
                    _ => {
                        return Err(format!(
                            "spike: {} solo acepta una función anónima literal o el nombre de una función",
                            method
                        ))
                    }
                };
                let ret = match &eff[0].kind {
                    ExprKind::Func(fnexpr) => normalize_type(&fnexpr.return_type),
                    _ => normalize_type(&self.funcs[named.as_ref().unwrap()].ret),
                };
                out.push_str("{ ");
                // El literal captura por `move` los canales del ámbito (compartidos → se CLONAN antes; el
                // closure mueve un clon, el original sigue). Una fn nombrada es top-level y no captura nada.
                if named.is_none() {
                    for name in self.in_scope_channels() {
                        write!(out, "let {n} = {n}.clone(); ", n = mangle(&name)).unwrap();
                    }
                }
                let runtime = if method == "spawn" { "__ray_spawn" } else { "__ray_scope" };
                write!(out, "{}(move || ", runtime).unwrap();
                // spawn: el closure corre en OTRO hilo → devuelve la repr SEND (string/bytes → Arc); el
                // cuerpo produce la repr del programa, se envuelve. scope corre en el hilo actual → sin conv.
                let wrap = if method == "spawn" { ret } else { Type::Unit };
                let (pre, suf) = match wrap {
                    Type::String => ("std::sync::Arc::<str>::from(&*", ")"),
                    Type::Bytes => ("std::sync::Arc::<[u8]>::from(&*", ")"),
                    _ => ("", ""),
                };
                out.push_str(pre);
                match &eff[0].kind {
                    ExprKind::Func(fnexpr) => self.emit_block(out, &fnexpr.body)?,
                    _ => write!(out, "{}()", mangle(named.as_ref().unwrap())).unwrap(),
                }
                out.push_str(suf);
                out.push_str(") }");
            }
            "unwrap_or" => {
                self.emit_expr(out, eff[0])?;
                out.push_str(".unwrap_or(");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "unwrap" => {
                self.emit_expr(out, eff[0])?;
                out.push_str(".unwrap()");
            }
            // `.less()` (Ord) → `<` nativo. `.eq()`/`.show()` NO se interceptan: fluyen como llamada normal
            // al impl (`Tipo#eq`/`Tipo#show`, que se emiten) o al diccionario del bound — así un `impl Show`
            // CUSTOM (p. ej. `Vec2`) se respeta, mientras `print`/`to_string` siguen con el render default.
            "less" if name.contains('#') => {
                out.push('(');
                self.emit_expr(out, eff[0])?;
                out.push_str(" < ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "panic" => {
                out.push_str("panic!(\"{}\", ");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // Aserciones (prelude): assert(c) → assert!(c); assert_eq(a, b) → assert_eq!(a, b).
            "assert" => {
                out.push_str("assert!(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "assert_eq" => {
                out.push_str("assert_eq!(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // Orden superior (prelude map/filter/fold SOBRE ARREGLOS) → iteradores de Rust. `__f` liga la
            // closure una vez; `__x`/`__acc` son los elementos/acumulador. La guarda `!name.contains('#')`
            // distingue la función libre `map`/`filter`/`fold` (sobre `[T]`) del MÉTODO `Iter#map`/… (sobre
            // un iterador de primera clase), que cae al despacho de método ordinario (`_ =>`).
            "map" if !name.contains('#') => {
                out.push_str("{ let __f = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; Rc::new(std::cell::RefCell::new(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().map(|__x| __f(__x.clone())).collect::<Vec<_>>())) }");
            }
            "filter" if !name.contains('#') => {
                out.push_str("{ let __f = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; Rc::new(std::cell::RefCell::new(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().cloned().filter(|__x| __f(__x.clone())).collect::<Vec<_>>())) }");
            }
            "fold" if !name.contains('#') => {
                out.push_str("{ let __f = ");
                self.emit_expr(out, eff[2])?;
                out.push_str("; ");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().fold(");
                self.emit_expr(out, eff[1])?;
                out.push_str(", |__acc, __x| __f(__acc, __x.clone())) }");
            }
            // Cripto de producción (M43): los primitivos `__*` se interceptan a `ray_runtime::crypto::*`
            // (el MISMO código que la VM → oráculo byte-idéntico) y activan la feature `crypto` de
            // ray-runtime (→ `build_native` genera un proyecto Cargo). NOTA: `method` ya viene SIN el prefijo
            // `__` (línea ~2399 lo recorta), así que se matchea el nombre pelado con guarda `name` empieza
            // por `__` (no interceptar un método de usuario homónimo). El arg `bytes` es `Rc<[u8]>`; `&expr`
            // deref-coerce a `&[u8]`. Retorno `Vec<u8>` → `Rc<[u8]>`; `Option<Vec<u8>>` → `[bytes]` etiquetado
            // (`Rc<RefCell<Vec<Rc<[u8]>>>>`: vacío/único), que el prelude envuelve en `Option`.
            "sha256" | "sha512" | "sha1" if name.starts_with("__") && !self.exclude.contains("crypto") => {
                self.needs_rt_crypto = true;
                write!(out, "Rc::<[u8]>::from(ray_runtime::crypto::{}(&", method).unwrap();
                self.emit_expr(out, eff[0])?;
                out.push_str("))");
            }
            "hmac_sha256" if name.starts_with("__") && !self.exclude.contains("crypto") => {
                self.needs_rt_crypto = true;
                out.push_str("Rc::<[u8]>::from(ray_runtime::crypto::hmac_sha256(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push_str("))");
            }
            "crypto_random_bytes" if name.starts_with("__") && !self.exclude.contains("crypto") => {
                self.needs_rt_crypto = true;
                out.push_str("Rc::<[u8]>::from(ray_runtime::crypto::crypto_random_bytes(");
                self.emit_expr(out, eff[0])?;
                out.push_str("))");
            }
            "ed25519_verify" if name.starts_with("__") && !self.exclude.contains("crypto") => {
                self.needs_rt_crypto = true;
                out.push_str("ray_runtime::crypto::ed25519_verify(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push_str(", &");
                self.emit_expr(out, eff[2])?;
                out.push(')');
            }
            "ed25519_public_key" | "ed25519_sign" | "chacha20poly1305_seal" | "chacha20poly1305_open"
                if name.starts_with("__") && !self.exclude.contains("crypto") =>
            {
                self.needs_rt_crypto = true;
                let argc = match method {
                    "ed25519_public_key" => 1,
                    "ed25519_sign" => 2,
                    _ => 4, // chacha seal/open: clave, nonce, aad, dato
                };
                write!(out, "{{ let __r = ray_runtime::crypto::{}(", method).unwrap();
                for i in 0..argc {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push('&');
                    self.emit_expr(out, eff[i])?;
                }
                out.push_str("); Rc::new(std::cell::RefCell::new(match __r { Some(__v) => vec![Rc::<[u8]>::from(__v)], None => Vec::new() })) }");
            }
            // TLS de producción (Paso 1): los primitivos `__tls_*` → helpers `__ray_tls_*` (I/O bloqueante
            // vía ray_runtime::tls) y activan `needs_rt_tls`. Devuelven arreglos ETIQUETADOS (`["ok",h]`/
            // `["err",msg]`) que los wrappers de std/net.ray parsean; se emiten tal cual (sin envolver). El
            // arg string es `Rc<str>`; `&expr` deref-coerce a `&str`. `method` ya viene sin el prefijo `__`.
            "tls_connect" | "tls_connect_h2" if name.starts_with("__") && !self.exclude.contains("tls") => {
                self.needs_rt_tls = true;
                write!(out, "__ray_{}(&", method).unwrap();
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "tls_accept" if name.starts_with("__") && !self.exclude.contains("tls") => {
                self.needs_rt_tls = true;
                out.push_str("__ray_tls_accept(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push_str(", &");
                self.emit_expr(out, eff[2])?;
                out.push(')');
            }
            "tls_upgrade" if name.starts_with("__") && !self.exclude.contains("tls") => {
                self.needs_rt_tls = true;
                out.push_str("__ray_tls_upgrade(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // SQLite (Paso 2): `__sqlite_*` → helpers `__ray_sqlite_*` (rusqlite en ray-runtime) + activa
            // `needs_rt_sqlite`. Devuelven arreglos etiquetados que los wrappers de db/sqlite.ray parsean.
            // open(path): string→&str. exec/query(h, sql, params): h int, sql `&str`, params `[string]` →
            // se pasa por referencia (`&Rc<RefCell<Vec<Rc<str>>>>`) y el helper lo colecta a Vec<String>.
            "sqlite_open" if name.starts_with("__") && !self.exclude.contains("sqlite") => {
                self.needs_rt_sqlite = true;
                out.push_str("__ray_sqlite_open(&");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "sqlite_exec" | "sqlite_query" if name.starts_with("__") && !self.exclude.contains("sqlite") => {
                self.needs_rt_sqlite = true;
                write!(out, "__ray_{}(", method).unwrap();
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push_str(", &");
                self.emit_expr(out, eff[2])?;
                out.push(')');
            }
            _ => {
                // Función de usuario, o llamada a un valor-función (closure) en ámbito: `name(args)`.
                let is_closure = matches!(self.lookup(name), Some(Type::Fn(_, _)));
                if !self.funcs.contains_key(name) && !is_closure {
                    return Err(format!("spike: builtin/función '{}' no soportada", name));
                }
                out.push_str(&mangle(name));
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
            ExprKind::Bytes(_) => Type::Bytes,
            ExprKind::Char(_) => Type::Char,
            ExprKind::Ident(n) if n == "std::math::PI" || n == "std::math::E" => Type::Float,
            ExprKind::Ident(n) => {
                if let Some(t) = self.lookup(n).or_else(|| self.consts.get(n)) {
                    t.clone()
                } else if let Some(s) = self.funcs.get(n) {
                    // Función como valor → su tipo Fn.
                    Type::Fn(s.params.clone(), Box::new(s.ret.clone()))
                } else {
                    return Err(format!("spike: variable '{}' sin tipo conocido", n));
                }
            }
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
            ExprKind::Call { callee, args } => {
                let (n, recv) = resolve_callee(callee)?;
                // Despacho dinámico: el tipo es el retorno del método del trait.
                if let Some(r) = recv {
                    if matches!(self.type_of(r).ok(), Some(Type::Dyn(_))) {
                        return Ok(self.trait_method_sigs.get(n).map(|(_, ret)| ret.clone())
                            .ok_or_else(|| format!("spike: método de dyn desconocido '{}'", n))?);
                    }
                }
                // `std::math::*`: abs/min/max preservan el tipo del primer arg (int|float); el resto
                // (sqrt/pow/sin/…) → float. Antes de la ruta genérica (su FnSig lleva params-diccionario
                // que no sabríamos tipar: el arg es `int#less`, un impl del prelude que no emitimos).
                if let Some(mfn) = n.strip_prefix("std::math::") {
                    return Ok(match mfn {
                        "abs" | "min" | "max" => self.type_of(args.first().or(recv).ok_or("spike: math sin arg")?)?,
                        _ => Type::Float,
                    });
                }
                // `std::fs::*`: read_file → Result<string,string>; write_file → Result<int,string>; exists → bool.
                if let Some(ffn) = n.strip_prefix("std::fs::") {
                    return Ok(match ffn {
                        "read_file" => Type::Enum("Result".into(), vec![Type::String, Type::String]),
                        "read_file_bytes" => Type::Enum("Result".into(), vec![Type::Bytes, Type::String]),
                        "write_file" | "open" | "write" | "remove_file" | "mkdir" | "remove_dir"
                        | "rename" | "copy_file" | "file_size" | "write_file_bytes"
                        | "append_file_bytes" => {
                            Type::Enum("Result".into(), vec![Type::Int, Type::String])
                        }
                        "read_line" => opt_of(Type::String),
                        "list_dir" => Type::Enum(
                            "Result".into(),
                            vec![Type::Array(Box::new(Type::String)), Type::String],
                        ),
                        "exists" | "is_dir" | "is_file" => Type::Bool,
                        other => return Err(format!("spike: std::fs::{} no soportada", other)),
                    });
                }
                // std::time: now/monotonic → int; sleep → unit. std::random: next → float; below → int;
                // seed → unit.
                // Solo las funciones-primitivo; el resto de std/time|random (raylang puro) → ruta genérica.
                match n {
                    "std::time::now" | "std::time::monotonic" | "std::random::below" => return Ok(Type::Int),
                    "std::time::sleep" | "std::random::seed" => return Ok(Type::Unit),
                    "std::random::next" => return Ok(Type::Float),
                    "std::net::local_port" => return Ok(Type::Int),
                    "std::net::set_read_timeout" => return Ok(Type::Unit),
                    "std::net::tcp_connect" | "std::net::tcp_listen" | "std::net::tcp_accept"
                    | "std::net::socket_write" | "std::net::socket_write_bytes" => {
                        return Ok(Type::Enum("Result".into(), vec![Type::Int, Type::String]))
                    }
                    "std::net::socket_read" => {
                        return Ok(Type::Enum("Result".into(), vec![Type::String, Type::String]))
                    }
                    "std::net::socket_read_bytes" => {
                        return Ok(Type::Enum("Result".into(), vec![Type::Bytes, Type::String]))
                    }
                    _ => {}
                }
                let _ = &args;
                let method = n.rsplit('#').next().unwrap_or(n).trim_start_matches("__");
                // Receptor efectivo (UFCS o primer argumento), para métodos cuyo tipo depende de él.
                let recv0 = recv.or_else(|| args.first());
                match method {
                    "to_string" => Type::String,
                    // join(t) → T de la Task; join(arr, sep) → String (ad-hoc por el tipo del primer arg).
                    "join" => match recv0.map(|e| self.type_of(e)).transpose()? {
                        Some(Type::Task(t)) => *t,
                        _ => Type::String,
                    },
                    "show" if n.contains('#') => Type::String,
                    "eq" | "less" if n.contains('#') => Type::Bool,
                    "len" => Type::Int,
                    "parse_int" => opt_of(Type::Int),
                    "parse_float" => opt_of(Type::Float),
                    // Bytes: to_bytes → bytes; sub_bytes → bytes; from_utf8 → Result<string,string>.
                    "to_bytes" | "sub_bytes" => Type::Bytes,
                    "from_utf8" => Type::Enum("Result".into(), vec![Type::String, Type::String]),
                    // I/O de entrada del prelude: input → Option<string>; read_int → Option<int>;
                    // env → Option<string> (variable de entorno).
                    "input" | "env" => opt_of(Type::String),
                    "read_int" => opt_of(Type::Int),
                    // Concurrencia: recv(ch) → Option<T> (T = elemento del canal); send/spawn → unit.
                    "recv" => match self.type_of(recv0.ok_or("spike: recv sin canal")?)? {
                        Type::Channel(t) => opt_of(*t),
                        other => return Err(format!("spike: recv sobre {:?}", other)),
                    },
                    "send" => Type::Unit,
                    "select" => Type::Int, // índice del canal listo
                    "signals" => Type::Channel(Box::new(Type::Int)), // canal de señales del SO
                    // spawn(f)→Task<T>; scope(f)→R. `f` es un literal `fn()->T` o el nombre de una función.
                    "spawn" | "scope" => {
                        let ret = match recv0.map(|e| &e.kind) {
                            Some(ExprKind::Func(f)) => normalize_type(&f.return_type),
                            Some(ExprKind::Ident(n)) if self.funcs.contains_key(n) => {
                                normalize_type(&self.funcs[n].ret)
                            }
                            _ => return Err(format!("spike: {} sin función anónima ni nombre de función", method)),
                        };
                        if method == "spawn" {
                            Type::Task(Box::new(ret))
                        } else {
                            ret
                        }
                    }
                    // close ad-hoc: canal → unit; handle de archivo (int) → 0.
                    "close" => match recv0.map(|e| self.type_of(e)).transpose()? {
                        Some(Type::Channel(_)) => Type::Unit,
                        _ => Type::Int,
                    },
                    "print" | "eprint" | "push" | "insert" | "add_to" | "assert" | "assert_eq" | "panic" => Type::Unit,
                    "bytes_of" => Type::Bytes,
                    "char_code" => Type::Int,
                    "char_from_code" => opt_of(Type::Char),
                    // UDP primitivos: bind/send → [string] etiquetado; recv → [bytes] etiquetado.
                    "udp_bind" | "udp_send_to" => Type::Array(Box::new(Type::String)),
                    "udp_recv_from" => Type::Array(Box::new(Type::Bytes)),
                    // Más string builtins: trim/to_upper/to_lower/repeat/replace/substring → string;
                    // starts_with/ends_with → bool.
                    "trim" | "to_upper" | "to_lower" | "repeat" | "replace" | "substring" => Type::String,
                    "starts_with" | "ends_with" | "contains" => Type::Bool,
                    "index_of" => opt_of(Type::Int), // índice de subcadena → Option<int>

                    "split" => Type::Array(Box::new(Type::String)),
                    "args" => Type::Array(Box::new(Type::String)),
                    "chars" => Type::Array(Box::new(Type::Char)),
                    "contains_key" => Type::Bool,
                    // get_or → V (desenvuelto); get/remove → Option<V> (para match/`?`); keys→[K]; values→[V].
                    "get_or" => match self.type_of(recv0.ok_or("spike: get_or sin receptor")?)? {
                        Type::Map(_, v) => *v,
                        other => return Err(format!("spike: get_or sobre {:?}", other)),
                    },
                    "get" | "remove" => match self.type_of(recv0.ok_or("spike: get sin receptor")?)? {
                        Type::Map(_, v) => opt_of(*v),
                        other => return Err(format!("spike: get sobre {:?}", other)),
                    },
                    "keys" => match self.type_of(recv0.ok_or("spike: keys sin receptor")?)? {
                        Type::Map(k, _) => Type::Array(k),
                        other => return Err(format!("spike: keys sobre {:?}", other)),
                    },
                    "values" => match self.type_of(recv0.ok_or("spike: values sin receptor")?)? {
                        Type::Map(_, v) => Type::Array(v),
                        other => return Err(format!("spike: values sobre {:?}", other)),
                    },
                    "sort" => self.type_of(recv0.ok_or("spike: sort sin receptor")?)?,
                    // Cripto (M43): hash/hmac/csprng → bytes; verify → bool; los fallibles (ed25519
                    // pk/sign, chacha seal/open) → `[bytes]` etiquetado (el prelude → Option<bytes>). `method`
                    // ya viene sin `__` (ver emit_call); guarda `n` empieza por `__`.
                    "sha256" | "sha512" | "sha1" | "hmac_sha256" | "crypto_random_bytes"
                        if n.starts_with("__") =>
                    {
                        Type::Bytes
                    }
                    "ed25519_verify" if n.starts_with("__") => Type::Bool,
                    "ed25519_public_key" | "ed25519_sign" | "chacha20poly1305_seal"
                    | "chacha20poly1305_open"
                        if n.starts_with("__") =>
                    {
                        Type::Array(Box::new(Type::Bytes))
                    }
                    // TLS (Paso 1) y SQLite (Paso 2): los primitivos devuelven `[string]` etiquetado
                    // (["ok",…]/["err",msg]; sqlite_query aplana ncols + celdas).
                    "tls_connect" | "tls_connect_h2" | "tls_accept" | "tls_upgrade" | "sqlite_open"
                    | "sqlite_exec" | "sqlite_query"
                        if n.starts_with("__") =>
                    {
                        Type::Array(Box::new(Type::String))
                    }
                    // unwrap_or/unwrap desenvuelven un Option<T>/Result<T,E> → T.
                    "unwrap_or" | "unwrap" => {
                        unwrapped(&self.type_of(recv0.ok_or("spike: unwrap sin receptor")?)?)
                    }
                    // Orden superior: map(xs,f) → [ret(f)]; filter(xs,f) → [elem(xs)]; fold(xs,init,f) → ret(f).
                    // Guarda `!n.contains('#')`: la función libre sobre `[T]`; `Iter#map`/… (método) cae al `_`.
                    "map" if !n.contains('#') => match self.type_of(effargs(recv, args, 1)?)? {
                        Type::Fn(_, r) => Type::Array(r),
                        other => return Err(format!("spike: map con f no-función {:?}", other)),
                    },
                    "filter" if !n.contains('#') => self.type_of(effargs(recv, args, 0)?)?,
                    "fold" if !n.contains('#') => match self.type_of(effargs(recv, args, 2)?)? {
                        Type::Fn(_, r) => *r,
                        other => return Err(format!("spike: fold con f no-función {:?}", other)),
                    },
                    _ => {
                        // Función de usuario (quizá genérica), o llamada a un closure en ámbito.
                        if let Some(s) = self.funcs.get(n) {
                            if s.tparams.is_empty() {
                                s.ret.clone()
                            } else {
                                // Genérica: unifica los params con los tipos de los args → sustituye el retorno.
                                let (params, ret, tps) = (s.params.clone(), s.ret.clone(), s.tparams.clone());
                                let eff: Vec<&Expr> = recv.into_iter().chain(args.iter()).collect();
                                let mut subst = HashMap::new();
                                for (i, pt) in params.iter().enumerate() {
                                    if let Some(a) = eff.get(i) {
                                        let at = self.type_of(a)?;
                                        unify(pt, &at, &tps, &mut subst);
                                    }
                                }
                                subst_type(&ret, &subst)
                            }
                        } else if let Some(Type::Fn(_, r)) = self.lookup(n) {
                            (**r).clone()
                        } else {
                            return Err(format!("spike: no sé el tipo de retorno de '{}'", n));
                        }
                    }
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
            ExprKind::Index { array, index } => match self.type_of(array)? {
                Type::Array(t) => *t,
                Type::String => Type::Char, // s[i] → char
                Type::Bytes => Type::Int, // b[i] → el octeto como int
                Type::Tuple(ts) => {
                    let i = match &index.kind {
                        ExprKind::Int(n) => *n as usize,
                        _ => return Err("spike: índice de tupla no literal".into()),
                    };
                    ts.get(i).cloned().ok_or("spike: índice de tupla fuera de rango")?
                }
                other => return Err(format!("spike: indexar {:?} no soportado", other)),
            },
            ExprKind::TupleLit(elems) => {
                Type::Tuple(elems.iter().map(|e| self.type_of(e)).collect::<Result<_, _>>()?)
            }
            ExprKind::StructLit { name, .. } => Type::Struct(name.clone(), vec![]),
            ExprKind::EnumLit { enum_name, variant, args } => {
                if enum_name == "Option" {
                    // Some(x) → Option<tipo(x)>; None → Option<Unit> (placeholder; lo fija el contexto).
                    let t = args.first().map(|a| self.type_of(a)).transpose()?.unwrap_or(Type::Unit);
                    opt_of(t)
                } else if enum_name == "Result" {
                    match variant.as_str() {
                        "Ok" => Type::Enum("Result".into(), vec![self.type_of(&args[0])?, Type::Unit]),
                        "Err" => Type::Enum("Result".into(), vec![Type::Unit, self.type_of(&args[0])?]),
                        _ => Type::Enum("Result".into(), vec![Type::Unit, Type::Unit]),
                    }
                } else {
                    Type::Enum(enum_name.clone(), vec![])
                }
            }
            ExprKind::Try(inner) => unwrapped(&self.type_of(inner)?),
            ExprKind::Func(fnexpr) => Type::Fn(
                fnexpr.params.iter().map(|p| p.ty.clone()).collect(),
                Box::new(fnexpr.return_type.clone()),
            ),
            ExprKind::Field { object, name } => {
                let obj_ty = self.type_of(object)?;
                // Tupla: `t.0` → el tipo del i-ésimo elemento.
                if let Type::Tuple(ts) = &obj_ty {
                    let i: usize = name.parse().map_err(|_| "spike: campo de tupla no numérico")?;
                    return ts.get(i).cloned().ok_or_else(|| "spike: campo de tupla fuera de rango".into());
                }
                let sn = match &obj_ty {
                    Type::Struct(n, _) => n.clone(),
                    other => return Err(format!("spike: acceso a campo sobre {:?}", other)),
                };
                let fty = self
                    .struct_fields
                    .get(&sn)
                    .and_then(|fs| fs.iter().find(|(f, _)| f == name))
                    .ok_or_else(|| format!("spike: campo '{}' desconocido en {}", name, sn))?
                    .1
                    .clone();
                // Sustituir los params de tipo del struct por los args (`Par<int,bool>` → A=int, B=bool).
                let subst = enum_subst(&self.struct_tparams, &sn, &obj_ty);
                subst_type(&normalize_type(&fty), &subst)
            }
            ExprKind::Match { scrutinee, arms } => {
                // El tipo del match = el de un brazo NO divergente. Su cuerpo puede ser un binding del
                // patrón (`Ok(conn) => conn`), que no está en ámbito para `type_of`; se resuelve desde el
                // tipo del escrutinio + el patrón. Se saltan los brazos que divergen (`Err(e) => { return }`)
                // → su "tipo" (`!`) no debe ganar sobre el real (bug: un `var c = match {...}` con struct no
                // se clonaba al leer → move error).
                let scrut_ty = self.type_of(scrutinee).ok();
                arms.iter()
                    .find_map(|a| self.arm_type(scrut_ty.as_ref(), a))
                    .ok_or("spike: no pude inferir el tipo del match")?
            }
            ExprKind::Cast { ty, .. } => normalize_type(ty),
            ExprKind::MapLit(pairs) => {
                let (k, v) = pairs.first().ok_or("spike: Map literal vacío sin anotación")?;
                Type::Map(Box::new(self.type_of(k)?), Box::new(self.type_of(v)?))
            }
            // (Exhaustivo sobre ExprKind, como emit_expr: una variante nueva rompe la compilación aquí.)
        })
    }

    /// El tipo que aporta un brazo de `match` al tipo del `match`, o `None` si diverge o no se resuelve.
    /// Resuelve un cuerpo que es un binding del patrón (`Ok(conn) => conn`) desde el tipo del escrutinio.
    fn arm_type(&self, scrut_ty: Option<&Type>, arm: &MatchArm) -> Option<Type> {
        if expr_diverges(&arm.body) {
            return None;
        }
        let binds = self.pattern_binding_types(scrut_ty, &arm.pattern);
        if let ExprKind::Ident(n) = &arm.body.kind {
            if let Some(t) = binds.get(n) {
                return Some(t.clone());
            }
        }
        self.type_of(&arm.body).ok()
    }

    /// Los tipos de los bindings de un patrón, dado el tipo del escrutinio. Cubre el binding suelto (todo
    /// el escrutinio) y una variante con subpatrones-binding (payload sustituido con los args del tipo,
    /// como en `emit_pattern`). Los patrones más complejos (anidados) se omiten (no aportan al caso común).
    fn pattern_binding_types(&self, scrut_ty: Option<&Type>, pat: &Pattern) -> HashMap<String, Type> {
        let mut out = HashMap::new();
        match &pat.kind {
            PatternKind::Binding(x) => {
                if let Some(t) = scrut_ty {
                    out.insert(x.clone(), t.clone());
                }
            }
            PatternKind::Variant { enum_name, variant, subpatterns } if !subpatterns.is_empty() => {
                let payload: Option<Vec<Type>> = match scrut_ty.map(normalize_type) {
                    Some(Type::Enum(_, args)) if enum_name == "Option" || enum_name == "Result" => {
                        match variant.as_str() {
                            "Some" | "Ok" => args.first().map(|t| vec![t.clone()]),
                            "Err" => args.get(1).map(|t| vec![t.clone()]),
                            _ => None,
                        }
                    }
                    Some(expected @ Type::Enum(_, _)) => self
                        .enum_variants
                        .get(enum_name)
                        .and_then(|m| m.get(variant))
                        .map(|raw| {
                            let subst = enum_subst(&self.enum_tparams, enum_name, &expected);
                            raw.iter().map(|p| subst_type(p, &subst)).collect()
                        }),
                    _ => None,
                };
                if let Some(payload) = payload {
                    for (sp, ty) in subpatterns.iter().zip(payload) {
                        if let PatternKind::Binding(x) = &sp.kind {
                            out.insert(x.clone(), ty);
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }
}

/// Normaliza los tipos que el parser deja como `Struct` genérico: `Map<K,V>` llega como
/// `Struct("Map",[K,V])` (el checker lo reclasifica en su tabla, no en la anotación del AST). Recursivo.
fn normalize_type(t: &Type) -> Type {
    match t {
        Type::Struct(n, args) if n == "Map" && args.len() == 2 => {
            Type::Map(Box::new(normalize_type(&args[0])), Box::new(normalize_type(&args[1])))
        }
        // Channel<T>/Task<T> (concurrencia): el parser los deja como `Struct`; el checker los reclasifica,
        // pero la anotación del AST puede llegar como `Struct` → los normalizamos aquí (como Map).
        Type::Struct(n, args) if n == "Channel" && args.len() == 1 => {
            Type::Channel(Box::new(normalize_type(&args[0])))
        }
        Type::Struct(n, args) if n == "Task" && args.len() == 1 => {
            Type::Task(Box::new(normalize_type(&args[0])))
        }
        Type::Channel(t) => Type::Channel(Box::new(normalize_type(t))),
        Type::Task(t) => Type::Task(Box::new(normalize_type(t))),
        // Option/Result (enums genéricos del prelude) → se mapean a los NATIVOS de Rust. El parser los
        // deja como `Struct`; los tratamos como `Enum` con sus args para bajarlos a `Option`/`Result`.
        Type::Struct(n, args) if (n == "Option" && args.len() == 1) || (n == "Result" && args.len() == 2) => {
            Type::Enum(n.clone(), args.iter().map(normalize_type).collect())
        }
        Type::Array(e) => Type::Array(Box::new(normalize_type(e))),
        Type::Map(k, v) => Type::Map(Box::new(normalize_type(k)), Box::new(normalize_type(v))),
        Type::Enum(n, args) => Type::Enum(n.clone(), args.iter().map(normalize_type).collect()),
        other => other.clone(),
    }
}

/// Un tipo de raylang → su equivalente Rust (subconjunto actual: escalares + string + arreglo + Map).
fn rust_ty(raw: &Type, enums: &std::collections::HashSet<String>, tparams: &std::collections::HashSet<String>) -> Result<String, String> {
    let t = normalize_type(raw);
    Ok(match &t {
        Type::Int => "i64",
        // Enteros con tamaño (M-tipos): a los nativos de Rust. Su aritmética ENVUELVE (mod 2^N); rustc con
        // `-O` desactiva los overflow-checks → `+`/`*` envuelven, casando con la VM.
        Type::UInt(w) => return Ok(format!("u{}", w)),
        Type::Float => "f64",
        Type::Bool => "bool",
        Type::Char => "char",
        Type::Unit => "()",
        // ptr (FFI, M41.4b): una dirección opaca de C → un `i64` inline (como `Value::Ptr`). Se pasa a
        // otras funciones C pero no se desreferencia en raylang.
        Type::Ptr => "i64",
        Type::String => "Rc<str>",
        // bytes: secuencia INMUTABLE de octetos (como string) → Rc<[u8]> (clon barato, compartible).
        Type::Bytes => return Ok("Rc<[u8]>".to_string()),
        // Channel<T> (concurrencia): canal MPMC thread-safe propio. El elemento usa la repr SEND
        // (`send_type`): primitivos tal cual, string→Arc<str>, bytes→Arc<[u8]> (se convierte al borde);
        // structs/arreglos (mutables) → diferido, requerirían el modelo de valores thread-safe.
        Type::Channel(t) => return Ok(format!("__RayChan<{}>", send_type(t, enums, tparams)?)),
        // Task<T> (structured concurrency): handle al resultado futuro de un `spawn` (JoinHandle envuelto).
        Type::Task(t) => return Ok(format!("__RayTask<{}>", send_type(t, enums, tparams)?)),
        // Arreglo: semántica de referencia + mutación → Rc<RefCell<Vec<…>>> (como el intérprete).
        Type::Array(t) => return Ok(format!("Rc<std::cell::RefCell<Vec<{}>>>", rust_ty(t, enums, tparams)?)),
        // Map: igual, sobre un HashMap.
        Type::Map(k, v) => {
            return Ok(format!(
                "Rc<std::cell::RefCell<std::collections::HashMap<{}, {}>>>",
                rust_ty(k, enums, tparams)?,
                rust_ty(v, enums, tparams)?
            ))
        }
        // Parámetro de tipo en ámbito → el genérico de Rust (rustc lo monomorfiza; NO Rc-envuelto).
        Type::Var(n) => return Ok(n.clone()),
        Type::Struct(n, args) if args.is_empty() && tparams.contains(n) => return Ok(n.clone()),
        // Tipo de usuario, con o sin args de tipo (`Caja<int>`): enum → Rc<E<..>>; struct → Rc<RefCell<S<..>>>.
        Type::Struct(n, args) => {
            let sfx = if args.is_empty() {
                String::new()
            } else {
                let ra: Vec<String> = args.iter().map(|a| rust_ty(a, enums, tparams)).collect::<Result<_, _>>()?;
                format!("<{}>", ra.join(", "))
            };
            // Multi-módulo: un tipo namespacado (`figuras::Rect`) se mangla a un identificador Rust válido.
            return Ok(if enums.contains(n) {
                format!("Rc<{}{}>", mangle(n), sfx)
            } else {
                format!("Rc<std::cell::RefCell<{}{}>>", mangle(n), sfx)
            });
        }
        // Option/Result → los nativos de Rust (genéricos gestionados por rustc, sin monomorfizar).
        Type::Enum(n, args) if n == "Option" && args.len() == 1 => {
            return Ok(format!("Option<{}>", rust_ty(&args[0], enums, tparams)?))
        }
        Type::Enum(n, args) if n == "Result" && args.len() == 2 => {
            return Ok(format!("Result<{}, {}>", rust_ty(&args[0], enums, tparams)?, rust_ty(&args[1], enums, tparams)?))
        }
        // Enum de usuario (con o sin args de tipo, `Caja<int>`): Rc<E<..>> (inmutable, permite recursión).
        Type::Enum(n, args) => {
            let sfx = if args.is_empty() {
                String::new()
            } else {
                let ra: Vec<String> = args.iter().map(|a| rust_ty(a, enums, tparams)).collect::<Result<_, _>>()?;
                format!("<{}>", ra.join(", "))
            };
            return Ok(format!("Rc<{}{}>", mangle(n), sfx));
        }
        // Tupla → tupla NATIVA de Rust (heterogénea, inmutable; sin Rc — valor, como en raylang).
        Type::Tuple(ts) => {
            let rs: Vec<String> = ts.iter().map(|t| rust_ty(t, enums, tparams)).collect::<Result<_, _>>()?;
            return Ok(format!("({},)", rs.join(", ")));
        }
        // Trait object `dyn A + B` → el struct sintetizado por el checker (__dyn_A+B), un juego de closures.
        Type::Dyn(traits) => {
            return Ok(format!("Rc<std::cell::RefCell<{}>>", mangle(&format!("__dyn_{}", traits.join("+")))));
        }
        // Función como valor → Rc<dyn Fn(...)->R> (clon barato + compartible; captura por `move`).
        Type::Fn(params, ret) => {
            let ps: Vec<String> = params.iter().map(|p| rust_ty(p, enums, tparams)).collect::<Result<_, _>>()?;
            return Ok(format!("Rc<dyn Fn({}) -> {}>", ps.join(", "), rust_ty(ret, enums, tparams)?));
        }
        other => return Err(format!("spike: tipo no soportado {:?}", other)),
    }
    .to_string())
}

/// Recolecta los tipos-**función** que aparecen (incluso ANIDADOS en arreglos/Map/tuplas) en un tipo, a su
/// forma `rust_ty` (`Rc<dyn Fn(..)->..>`). Un `fn` dentro de un contenedor de un campo/payload necesita su
/// propio `impl RayShow` (los impls genéricos de `Vec`/`Map`/tupla exigen `T: RayShow`, y no hay un RayShow
/// único para todas las firmas de función). Solo firmas CONCRETAS (representables): las que mencionan un
/// param de tipo se saltan (`rust_ty` falla → se omite). Dedup en el `BTreeSet` del llamador.
fn collect_fn_rayshow(
    ty: &Type,
    enums: &std::collections::HashSet<String>,
    item_tparams: &std::collections::HashSet<String>,
    acc: &mut std::collections::BTreeSet<String>,
) {
    match normalize_type(ty) {
        Type::Fn(ref params, ref ret) => {
            // Solo firmas SIN parámetros de tipo del item envolvente: `rust_ty` no falla ante un `T`
            // (lo renderiza literal → `cannot find type T`), así que la concreción se chequea aparte.
            if !ty_mentions_tparam(ty, item_tparams) {
                if let Ok(rt) = rust_ty(ty, enums, &std::collections::HashSet::new()) {
                    acc.insert(rt);
                }
            }
            for p in params {
                collect_fn_rayshow(p, enums, item_tparams, acc);
            }
            collect_fn_rayshow(ret, enums, item_tparams, acc);
        }
        Type::Array(inner) => collect_fn_rayshow(&inner, enums, item_tparams, acc),
        Type::Map(k, v) => {
            collect_fn_rayshow(&k, enums, item_tparams, acc);
            collect_fn_rayshow(&v, enums, item_tparams, acc);
        }
        Type::Tuple(ts) => ts.iter().for_each(|t| collect_fn_rayshow(t, enums, item_tparams, acc)),
        Type::Struct(_, args) | Type::Enum(_, args) => {
            args.iter().for_each(|a| collect_fn_rayshow(a, enums, item_tparams, acc))
        }
        _ => {}
    }
}

/// ¿El tipo menciona algún parámetro de tipo del conjunto dado (o una `Type::Var` cualquiera)? Un tipo así
/// NO es representable como `impl RayShow` concreto (mencionaría un `T` inexistente en ese ámbito).
fn ty_mentions_tparam(ty: &Type, tps: &std::collections::HashSet<String>) -> bool {
    match ty {
        Type::Var(_) => true,
        Type::Struct(n, args) | Type::Enum(n, args) => {
            tps.contains(n) || args.iter().any(|a| ty_mentions_tparam(a, tps))
        }
        Type::Array(inner) => ty_mentions_tparam(inner, tps),
        Type::Map(k, v) => ty_mentions_tparam(k, tps) || ty_mentions_tparam(v, tps),
        Type::Tuple(ts) => ts.iter().any(|t| ty_mentions_tparam(t, tps)),
        Type::Fn(params, ret) => {
            params.iter().any(|p| ty_mentions_tparam(p, tps)) || ty_mentions_tparam(ret, tps)
        }
        _ => false,
    }
}

/// La repr **Send** de un tipo que viaja por un `Channel<T>`/`Task<T>`: se convierte al BORDE (send/recv/
/// spawn/join) para no cambiar el modelo de valores del resto (que sigue `Rc`, mono-hilo). Los primitivos
/// son Send tal cual; string/bytes (heap INMUTABLE) → `Arc<str>`/`Arc<[u8]>` (copia barata al cruzar el
/// hilo, semánticamente idéntico por ser inmutables). Structs/arreglos/Map (mutables) → error (diferido:
/// necesitarían el modelo de valores thread-safe con sus hazards de bloqueo).
fn send_type(t: &Type, enums: &std::collections::HashSet<String>, tparams: &std::collections::HashSet<String>) -> Result<String, String> {
    match normalize_type(t) {
        Type::String => Ok("std::sync::Arc<str>".to_string()),
        Type::Bytes => Ok("std::sync::Arc<[u8]>".to_string()),
        Type::Int | Type::Float | Type::Bool | Type::Char | Type::UInt(_) => rust_ty(t, enums, tparams),
        other => Err(format!(
            "spike: canal/tarea de tipo no-Send {:?} — soportados: int/float/bool/char/string/bytes",
            other
        )),
    }
}

/// El sufijo `.map(|__x| Rc::…::from(&*__x))` para convertir la repr SEND recibida (Arc) de vuelta a la
/// del programa (Rc). Vacío para primitivos (sin conversión).
fn from_send_map(t: &Type) -> &'static str {
    match normalize_type(t) {
        Type::String => ".map(|__x| Rc::<str>::from(&*__x))",
        Type::Bytes => ".map(|__x| Rc::<[u8]>::from(&*__x))",
        _ => "",
    }
}

/// Sustitución `param_de_tipo → arg` para un tipo nominal aplicado: los `tparams` del enum/struct `name`
/// ligados a los args del tipo esperado (`Caja<int>` con tparams `[T]` → `{T: int}`).
fn enum_subst(
    tparams_map: &HashMap<String, Vec<String>>,
    name: &str,
    expected: &Type,
) -> HashMap<String, Type> {
    let args = match normalize_type(expected) {
        Type::Enum(_, a) | Type::Struct(_, a) => a,
        _ => return HashMap::new(),
    };
    match tparams_map.get(name) {
        Some(tps) => tps.iter().cloned().zip(args).collect(),
        None => HashMap::new(),
    }
}

/// Declaración de genéricos de Rust `<A: Clone + RayShow + 'static, …>` (fn/struct/enum: `Clone` para el
/// clon-al-leer, `RayShow` para mostrar/`to_string`, `'static` porque un valor genérico puede acabar en
/// un `Rc<dyn Fn…>` —p. ej. el `Iter<T>` de los iteradores— que exige `T: 'static`; es SIEMPRE cierto en
/// raylang (todos los valores son `Rc`/`Copy`, sin préstamos). rustc los monomorfiza → nativo.
// =====================================================================
// FFI (M41): tipos y marshalling de la frontera C
// =====================================================================

/// El tipo C de un ARGUMENTO de `extern fn` en la declaración `extern "C"` (ABI). Espejo de `ffi::arg_kind`.
fn ffi_c_arg_ty(t: &Type) -> Result<&'static str, String> {
    Ok(match normalize_type(t) {
        Type::Int => "i64",                              // long
        Type::Float => "f64",                            // double
        Type::Bool => "std::os::raw::c_int",             // int (0/1)
        Type::String => "*const std::os::raw::c_char",   // char* (NUL-terminado)
        Type::Bytes => "*const u8",                      // buffer crudo
        Type::Ptr => "*mut std::ffi::c_void",            // void* opaco
        other => return Err(format!("spike: FFI arg no marshalable: {:?}", other)),
    })
}

/// El tipo C de RETORNO de una `extern fn`. Un `char*` de retorno se modela con `Option` (NULL→None);
/// un `ptr` fallible con `Option<ptr>`. Espejo de `ffi::ret_kind`.
fn ffi_c_ret_ty(t: &Type) -> Result<&'static str, String> {
    Ok(match normalize_type(t) {
        // `int` de retorno → C **`int`** de 32 bits (como la VM, `RetMold::I32`): el `int` de raylang es
        // `long`, pero un `-> int` FFI se lee como el `int` de C y se extiende el signo. Declararlo `i64`
        // rompería el ABI (los 32 bits altos de `rax` son basura → p. ej. el EOF -1 de `fgetc` se vería +).
        Type::Int => "std::os::raw::c_int",
        Type::Float => "f64",
        Type::Bool => "std::os::raw::c_int",
        Type::Unit => "()",
        Type::Ptr => "*mut std::ffi::c_void",
        Type::Enum(n, args) if n == "Option" && args.len() == 1 => match normalize_type(&args[0]) {
            Type::Bytes | Type::String => "*const std::os::raw::c_char",
            Type::Ptr => "*mut std::ffi::c_void",
            other => return Err(format!("spike: FFI retorno Option<{:?}> no soportado", other)),
        },
        other => return Err(format!("spike: FFI retorno no marshalable: {:?}", other)),
    })
}

fn generic_decl(tparams: &[String]) -> String {
    generic_bound(tparams, "Clone + RayShow + 'static")
}

/// `<A: bound, B: bound>` para una lista de params de tipo (o "" si vacía).
fn generic_bound(tparams: &[String], bound: &str) -> String {
    if tparams.is_empty() {
        String::new()
    } else {
        let ps: Vec<String> = tparams.iter().map(|t| format!("{}: {}", t, bound)).collect();
        format!("<{}>", ps.join(", "))
    }
}

/// Los args de tipo `<A, B>` (sin bounds) para instanciar un tipo genérico (o "" si vacía).
fn type_args(tparams: &[String]) -> String {
    if tparams.is_empty() {
        String::new()
    } else {
        format!("<{}>", tparams.join(", "))
    }
}

/// Unifica un tipo de parámetro (que puede llevar variables de tipo) con el tipo real de un argumento,
/// ligando cada variable en `subst`. Asimétrico: las variables son las de la firma llamada (`tparams`).
fn unify(param: &Type, arg: &Type, tparams: &[String], subst: &mut HashMap<String, Type>) {
    let is_var = |n: &str| tparams.iter().any(|t| t == n);
    match param {
        Type::Var(n) if is_var(n) => {
            subst.entry(n.clone()).or_insert_with(|| arg.clone());
        }
        Type::Struct(n, a) if a.is_empty() && is_var(n) => {
            subst.entry(n.clone()).or_insert_with(|| arg.clone());
        }
        Type::Array(p) => {
            if let Type::Array(a2) = normalize_type(arg) {
                unify(p, &a2, tparams, subst);
            }
        }
        Type::Map(pk, pv) => {
            if let Type::Map(ak, av) = normalize_type(arg) {
                unify(pk, &ak, tparams, subst);
                unify(pv, &av, tparams, subst);
            }
        }
        Type::Fn(ps, r) => {
            if let Type::Fn(as2, ar) = normalize_type(arg) {
                for (pp, aa) in ps.iter().zip(&as2) {
                    unify(pp, aa, tparams, subst);
                }
                unify(r, &ar, tparams, subst);
            }
        }
        // Structs/enums genéricos (`Iter<T>`, `Caja<T>`, `Option<T>`) y tuplas: unificar arg-a-arg, para
        // resolver el `T` a través de cadenas de adaptadores (`iter().enumerate()` → `Iter<(int, T)>`).
        Type::Struct(n, pargs) if !pargs.is_empty() => {
            if let Type::Struct(an, aargs) = normalize_type(arg) {
                if *n == an && pargs.len() == aargs.len() {
                    for (p, a) in pargs.iter().zip(&aargs) { unify(p, a, tparams, subst); }
                }
            }
        }
        Type::Enum(n, pargs) => {
            if let Type::Enum(an, aargs) = normalize_type(arg) {
                if *n == an && pargs.len() == aargs.len() {
                    for (p, a) in pargs.iter().zip(&aargs) { unify(p, a, tparams, subst); }
                }
            }
        }
        Type::Tuple(ps) => {
            if let Type::Tuple(as2) = normalize_type(arg) {
                for (p, a) in ps.iter().zip(&as2) { unify(p, a, tparams, subst); }
            }
        }
        _ => {}
    }
}

/// Sustituye las variables de tipo de `t` por sus ligaduras en `subst` (las no ligadas se dejan igual).
fn subst_type(t: &Type, subst: &HashMap<String, Type>) -> Type {
    match t {
        Type::Var(n) => subst.get(n).cloned().unwrap_or_else(|| t.clone()),
        Type::Struct(n, a) if a.is_empty() => subst.get(n).cloned().unwrap_or_else(|| t.clone()),
        // Structs/enums genéricos y tuplas: sustituir en cada argumento (p. ej. `Iter<(int, T)>`).
        Type::Struct(n, a) => Type::Struct(n.clone(), a.iter().map(|x| subst_type(x, subst)).collect()),
        Type::Enum(n, a) => Type::Enum(n.clone(), a.iter().map(|x| subst_type(x, subst)).collect()),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|x| subst_type(x, subst)).collect()),
        Type::Array(e) => Type::Array(Box::new(subst_type(e, subst))),
        Type::Map(k, v) => {
            Type::Map(Box::new(subst_type(k, subst)), Box::new(subst_type(v, subst)))
        }
        Type::Channel(e) => Type::Channel(Box::new(subst_type(e, subst))),
        Type::Task(e) => Type::Task(Box::new(subst_type(e, subst))),
        Type::Fn(ps, r) => Type::Fn(
            ps.iter().map(|p| subst_type(p, subst)).collect(),
            Box::new(subst_type(r, subst)),
        ),
        other => other.clone(),
    }
}

/// El i-ésimo argumento EFECTIVO de una llamada (el receptor de UFCS va primero, luego los args).
fn effargs<'a>(recv: Option<&'a Expr>, args: &'a [Expr], i: usize) -> Result<&'a Expr, String> {
    recv.into_iter().chain(args.iter()).nth(i).ok_or_else(|| "spike: falta un argumento".to_string())
}

/// `Option<t>` (usando el Option nativo de Rust).
fn opt_of(t: Type) -> Type {
    Type::Enum("Option".to_string(), vec![t])
}

/// Desenvuelve `Option<T>`/`Result<T,E>` → `T` (para `unwrap_or`/`unwrap`/`?`); otro tipo se deja igual.
fn unwrapped(t: &Type) -> Type {
    match normalize_type(t) {
        Type::Enum(n, args) if (n == "Option" || n == "Result") && !args.is_empty() => args[0].clone(),
        other => other,
    }
}

/// ¿Es un tipo de heap (semántica de referencia / no `Copy`) → hay que clonar al leer?
/// ¿La expresión DIVERGE (no produce valor: termina en `return` o `panic`)? Un brazo de `match` que
/// diverge no contribuye al tipo del `match` (su "tipo" sería `!`). Se usa para inferir el tipo de un
/// `match` cuyo brazo real lleva un binding del patrón y el otro solo aborta (`Err(e) => { …; return }`).
fn expr_diverges(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Block(b) => match &b.tail {
            Some(t) => expr_diverges(t),
            None => matches!(b.statements.last().map(|s| &s.kind), Some(StmtKind::Return { .. })),
        },
        ExprKind::Call { callee, .. } => matches!(&callee.kind, ExprKind::Ident(n) if n == "panic"),
        _ => false,
    }
}

fn is_heap(t: &Type) -> bool {
    matches!(
        t,
        Type::String | Type::Bytes | Type::Array(_) | Type::Tuple(_) | Type::Map(_, _)
            | Type::Struct(_, _) | Type::Enum(_, _) | Type::Fn(_, _) | Type::Var(_) | Type::Dyn(_)
            | Type::Channel(_) | Type::Task(_) // semántica de referencia: clon = Arc bump
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
mod tests;
