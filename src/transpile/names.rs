//! Identidad y clasificación de nombres (movimiento puro; usar `git log --follow`).
//!
//! `mangle`/`is_rust_keyword` cruzan de identificador raylang a identificador Rust válido;
//! `is_prelude_impl`/`is_handled_builtin`/`skip_fn_def`/`resolve_callee` deciden si una
//! función/llamada la maneja el backend nativo o es un builtin/impl del prelude ya cubierto.

use super::*;

/// Convierte un nombre de raylang en un identificador Rust válido: los métodos de trait bajados por el
/// checker llevan `#` (`Punto#show`) y los módulos `::` (`m::f`), ilegales en Rust. Identidad para los
/// nombres normales (salvo keywords de Rust → raw identifiers). Los traits son ERASURE (M9): los métodos
/// son funciones ordinarias tras el bajado.
///
/// NOTA sobre temporales: el transpilador emite temporales sintéticos con el prefijo **reservado `__rt_`**
/// (`__rt_arr`, `__rt_rhs`, `__rt_v`, …). Un identificador de usuario que empiece por `__rt_` PODRÍA
/// colisionar (raylang permite `_` inicial); es una convención reservada, como el `#`/`::`/`$` que el
/// checker ya reserva para nombres sintetizados. La des-colisión total exigiría prefijar TODOS los
/// nombres de usuario (namespace disjunto) — diferido por no justificar el churn.
pub(super) fn mangle(name: &str) -> String {
    if name == "self" {
        return "__self".to_string(); // `self` es palabra reservada de Rust fuera de un método
    }
    // `$` lo usan los temporales sintéticos del checker (p. ej. el bind del `?` con From-conversion,
    // `$to`/`$te`) → no es identificador Rust válido.
    let base = name.replace('#', "_HH_").replace("::", "_CC_").replace('+', "_P_").replace('$', "_D_");
    // Un identificador LEGAL de raylang puede ser palabra RESERVADA de Rust (`type`, `loop`, `mod`,
    // `move`, `ref`, `where`, `use`, `unsafe`, `async`, …) → generaría Rust inválido. Se emite como raw
    // identifier `r#type`, válido en posición de variable/param/función/campo. Las cuatro que NO admiten
    // `r#` (`crate`/`self`/`Self`/`super`) se escapan con sufijo. (Las palabras clave COMPARTIDAS con
    // raylang —`fn`/`let`/`if`/`struct`/…— no llegan aquí: no son identificadores en raylang.) Solo se
    // aplica a nombres "limpios" (sin `::`/`#`, que marcan nombres ya sintetizados por el checker).
    if base == name {
        if matches!(base.as_str(), "crate" | "Self" | "super") {
            return format!("{base}_");
        }
        if is_rust_keyword(&base) {
            return format!("r#{base}");
        }
    }
    base
}

/// ¿Es `s` una palabra reservada de Rust (estricta o reservada-para-el-futuro) que un identificador
/// de raylang podría llevar? Excluye las compartidas con raylang (no llegan como identificadores).
pub(super) fn is_rust_keyword(s: &str) -> bool {
    matches!(
        s,
        // Estrictas de Rust que SON identificadores legales en raylang (raylang no las reserva):
        "as" | "const" | "extern" | "loop" | "mod" | "move" | "mut" | "ref" | "static" | "type"
            | "unsafe" | "use" | "where" | "async" | "await" | "dyn"
            // Reservadas de Rust para el futuro (y `gen`, keyword desde la edition 2024 — la que
            // usan AMBOS caminos de build, rustc pelado y el proyecto Cargo generado):
            | "abstract" | "become" | "box" | "do" | "final" | "macro" | "override" | "priv"
            | "typeof" | "unsized" | "virtual" | "yield" | "try" | "gen"
    )
}

/// ¿Es un método de un impl del PRELUDE sobre un tipo builtin (`[]#len`, `string#trim`, `int#show`)?
/// = clave de tipo builtin Y método del prelude. Su método lo maneja el transpilador directamente → se
/// salta. Un impl de USUARIO sobre un builtin (`int#valor`) NO se salta (método no-prelude → se emite).
pub(super) fn is_prelude_impl(name: &str) -> bool {
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

/// ¿La clave de un método manglado (`clave#metodo`) es un tipo CORE de la stdlib (primitivos,
/// contenedores, Option/Result/Iter)? Sus métodos pueden interceptarse por el nombre pelado (los
/// brazos nativos de emit_call/type_of se escribieron para ellos: `Option#unwrap_or` → `.unwrap_or`).
/// Para una clave de USUARIO o de módulo (`Store`, `std::kv::Store`) el checker ya resolvió el impl
/// concreto y su def se emite → la llamada va SIEMPRE a esa def, aunque el nombre pelado coincida
/// con un builtin (`Store#get` nunca es el `get` de Map).
pub(super) fn is_core_impl_key(key: &str) -> bool {
    matches!(
        key,
        "int" | "float" | "bool" | "char" | "string" | "bytes" | "uint" | "u8" | "u32" | "u64"
            | "unit" | "[]" | "Map" | "Channel" | "Task" | "Option" | "Result" | "Iter"
    )
}

/// Resuelve el `callee` de una llamada a `(nombre, receptor)`. UFCS `obj.m(args)` llega como callee
/// `Field{object, name}` (el checker no lo baja para builtins) ≡ `m(obj, args)`: el receptor va primero.
pub(super) fn resolve_callee(callee: &Expr) -> Result<(&str, Option<&Expr>), String> {
    match &callee.kind {
        ExprKind::Ident(n) => Ok((n, None)),
        ExprKind::Field { object, name } => Ok((name, Some(object))),
        _ => Err("call to an expression (neither a name nor a method) is not supported".into()),
    }
}

/// ¿Es una función del PRELUDE o un builtin que el transpilador maneja directamente? Sus definiciones
/// inyectadas por el checker se SALTAN (el transpilador las mapea a Rust nativo, o no las soporta y su
/// cuerpo referiría builtins ausentes). Lista extraída de `src/prelude.ray` + los builtins públicos.
pub(super) fn is_handled_builtin(name: &str) -> bool {
    // `std::math::*`/`std::fs::*` se interceptan en emit_call/type_of (→ Rust nativo); no emitimos sus
    // wrappers del módulo (llaman a primitivos `__sqrt`/`__read_file`… ausentes).
    if name.starts_with("std::math::") || name.starts_with("std::fs::") {
        return true;
    }
    // De `std/time` y `std/random` SOLO se saltan las funciones que envuelven un primitivo (interceptadas);
    // el resto (p. ej. `std::time::to_epoch_millis`, helpers de `DateTime`) son raylang puro → se emiten.
    if matches!(
        name,
        "std::time::now" | "std::time::monotonic" | "std::time::monotonic_nanos" | "std::time::sleep"
    )
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
    // Un builtin público de la tabla `BUILTINS` (print/join/spawn/…) siempre está manejado — H16: la
    // tabla es la fuente de verdad, no una lista repetida aquí. (Un nombre de la tabla nunca es una
    // función de usuario: el checker prohíbe redefinir un builtin.)
    if crate::builtins::lookup(name).is_some() {
        return true;
    }
    matches!(
        name,
        // --- funciones del prelude (src/prelude.ray) ---
        "all" | "any" | "assert" | "assert_eq" | "char_from_code" | "env" | "filter" | "fold"
            | "from_utf" | "from_utf8" | "get" | "get_or" | "index_of" | "input" | "map" | "max" | "min"
            | "parse_float" | "parse_int" | "pop" | "position" | "read_int" | "recv"
            | "remove" | "sort" | "try_join"
        // --- builtins públicos manejados en emit_call que NO son filas de la tabla (bajan a
        // primitivos `__len`/`__push`/… o son azúcar del prelude) ---
            | "len" | "push" | "split" | "chars" | "contains_key" | "keys" | "values" | "insert"
            | "unwrap" | "unwrap_or"
    )
}

/// ¿Se salta la DEFINICIÓN de esta función al registrarla/emitirla? Sí para las sintéticas (`__`),
/// los impls del prelude (`int#eq`…) y los builtins manejados. Matiz del override: un builtin con `::`
/// (`std::fs::*`) envuelve un primitivo → siempre se salta; un builtin del prelude de nombre pelado
/// (`map`/`get_or`/`sort`…) se salta SOLO si viene del prelude (`line >= LINE_BASE`). Si el usuario lo
/// **redefine** (línea de usuario, por debajo de la banda del prelude), es una función de usuario y debe
/// emitirse (override), o su llamada quedaría sin destino (p. ej. un `get_or(m, k)` de 2 args propio).
pub(super) fn skip_fn_def(f: &Function) -> bool {
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
pub(super) fn is_display_primitive(t: &Type) -> bool {
    matches!(
        normalize_type(t),
        Type::Int | Type::Float | Type::Bool | Type::Char | Type::String
    )
}

/// ¿La callee es una llamada a `to_string` (libre o método `x.to_string()`, posiblemente manglada)?
pub(super) fn is_to_string(callee: &Expr) -> bool {
    match resolve_callee(callee) {
        Ok((n, _)) => n.rsplit('#').next().unwrap_or(n).trim_start_matches("__") == "to_string",
        Err(_) => false,
    }
}
