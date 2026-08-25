//! Auxiliares de traits (M9) + derivación `@derive` (M10.1) (movimiento puro; usar
//! `git log --follow`).
//!
//! El manglado `Tipo#metodo` de M9 + `generate_derives`/`validate_derive` (M10.1: genera el
//! fuente `impl Eq/Show` y lo parsea) + su plomería de soporte (freshen/renumber de posiciones
//! para los cuerpos de defecto clonados por impl, M9.3a; inline de forwarders; bajada de
//! iteradores `for`).

use super::*;

// =====================================================================
// Auxiliares de traits (M9)
// =====================================================================

/// Nombre manglado de un método de impl: `Tipo#metodo`. El `#` impide colisión con
/// cualquier nombre que el usuario pueda escribir, así el método vive como una función
/// libre más sin chocar con las suyas.
pub(super) fn mangle(type_key: &str, method: &str) -> String {
    format!("{}#{}", type_key, method)
}

/// M28.2: nombre manglado de un método de conversión `From`. Incluye la **clave del origen**
/// para que `impl From<string> for E` e `impl From<int> for E` no colisionen (mismo destino
/// `E` y método `from`, distinta conversión). Nunca es invocable por el usuario (`#`).
pub(super) fn mangle_from(target_key: &str, source_key: &str) -> String {
    format!("{}#from#{}", target_key, source_key)
}

/// M28.2: ¿es `imp` un impl de un trait con parámetros de tipo (estilo `From<S>`)? Estos se
/// tratan aparte: su método de conversión se inyecta con un nombre manglado por origen y no
/// entra en la tabla de despacho por punto (no tiene `self`).
pub(super) fn is_typed_trait_impl(imp: &ImplBlock) -> bool {
    // Solo `From<S>` usa el mecanismo de **conversión** (su método `desde` es asociado —sin `self`—,
    // consumido por `?`). Otros traits parametrizados (p. ej. `Iterator<T>`, M40.2) van por el
    // despacho normal por punto: sus métodos con `self` se registran en la tabla de métodos.
    !imp.trait_args.is_empty() && imp.trait_name == "From"
}

/// M28.3b: ¿el operador binario produce un valor del mismo tipo que sus operandos? (Aritméticos y
/// bit a bit sí; comparación/lógicos no.) Decide si propagar el ancho uint esperado a los operandos.
pub(super) fn is_width_preserving(op: BinaryOp) -> bool {
    use BinaryOp::*;
    matches!(op, Add | Sub | Mul | Div | Rem | BitAnd | BitOr | BitXor | Shl | Shr)
}

/// M28.3b: ¿cabe el literal entero `n` (siempre ≥ 0 aquí; los negativos son `-` unario) en un
/// entero sin signo de `w` bits? Para u64, cualquier i64 no negativo cabe (i64::MAX < u64::MAX).
pub(super) fn uint_literal_fits(n: i64, w: u8) -> bool {
    if n < 0 { return false; }
    if w >= 64 { return true; }
    (n as u64) <= crate::runtime::uint_mask(w)
}

/// M28.1: mapa operador binario → (trait, método) para la sobrecarga. `None` si el operador
/// no es sobrecargable (`%`, comparación, lógicos, bit a bit).
pub(super) fn op_trait_method(op: BinaryOp) -> Option<(&'static str, &'static str)> {
    match op {
        BinaryOp::Add => Some(("Add", "add")),
        BinaryOp::Sub => Some(("Sub", "sub")),
        BinaryOp::Mul => Some(("Mul", "mul")),
        BinaryOp::Div => Some(("Div", "div")),
        _ => None,
    }
}

/// Clave de tipo para la tabla de métodos: el nombre del struct/enum o el primitivo.
/// `None` para los tipos que no pueden recibir un impl en M9.1 (arreglos, funciones,
/// unit, parámetros de tipo, `Self`).
pub(super) fn type_key_of(ty: &Type) -> Option<String> {
    Some(match ty {
        Type::Int => "int".into(),
        Type::Float => "float".into(),
        Type::Bool => "bool".into(),
        Type::String => "string".into(),
        Type::Char => "char".into(),
        Type::Struct(n, _) | Type::Enum(n, _) => n.clone(),
        // M48.4: constructores incorporados como objetivo de impl (`impl Len for [T]`/`Map<K,V>`/`bytes`).
        // La clave va por CONSTRUCTOR (como `Caja<int>`→"Caja"): `[int]`/`[bool]` comparten "[]".
        Type::Array(_) => "[]".into(),
        Type::Map(_, _) => "Map".into(),
        Type::Bytes => "bytes".into(),
        _ => return None,
    })
}

/// La **categoría** de un tipo para los builtins-como-método del completion (M45): la clave que
/// entiende `builtins::methods_for`. Cubre también arreglos y `Map`, que no tienen `type_key_of`.
pub(super) fn member_category(ty: &Type) -> Option<&'static str> {
    Some(match ty {
        Type::String => "string",
        Type::Bytes => "bytes",
        Type::Char => "char",
        Type::Int => "int",
        Type::Float => "float",
        Type::Bool => "bool",
        Type::Array(_) => "array",
        Type::Map(_, _) => "map",
        _ => return None,
    })
}

/// Completion de miembros (M45): los símbolos ofrecibles tras `recv.`. El LSP repara la fuente
/// insertando el centinela `__raycomplete__` tras el `.`; aquí corremos el front-end best-effort
/// (con recuperación de errores) y, al tipar ese acceso, enumeramos los miembros del tipo del
/// receptor. Devuelve `[]` si el receptor no tipa o no tiene miembros. No exige `main` (puede ser
/// un fragmento a medio escribir).
pub fn member_completion(program: &mut Program) -> Vec<MemberItem> {
    if prepare_program(program).is_err() {
        return Vec::new();
    }
    let mut checker = Checker::new();
    checker.completing = true;
    checker.require_main = false;
    checker.gather = true; // puebla `fn_defs` → posición de los métodos/UFCS para sus `///` docs
    let _ = checker.check_program(program); // best-effort: el error de tipos del fragmento es esperado
    checker.member_hits
}

// =====================================================================
// Derivación de `@derive(Eq)` (M10.1)
// =====================================================================

/// Genera los `impl` de `@derive(...)` (`Eq`, `Show`) de las declaraciones anotadas. Para cada
/// trait pedido construye el **fuente** del `impl Trait for T { ... }`, lo parsea y lo añade a
/// `program.impls`; el resto (bajada a `T#metodo`, registro) lo hace M9. Generar fuente y
/// parsearlo evita armar el AST a mano.
///
/// **Idempotente** (M11.3c): si ya existe un `impl Trait for T` no lo regenera. El *loader* la
/// llama por módulo con nombres **locales** (re-lexables) antes de namespacar los tipos; luego el
/// checker la vuelve a llamar sobre el programa fusionado (nombres ya namespacados, con `::`, que
/// no se podrían re-lexar) y, gracias a la idempotencia, **salta** los ya generados —sin intentar
/// generar fuente con `::`—. Los caminos sin loader (REPL, runner de `@test`) la usan normal.
pub fn generate_derives(program: &mut Program) -> Result<(), TypeError> {
    // Pares (trait, nombre-de-tipo) ya implementados, para no regenerar. El trait se compara por
    // su LEAF (`std::json::ToJson` → `ToJson`): un trait derivable puede vivir en un módulo
    // namespacado (ToJson, M93.5) y el impl que el loader ya expandió llega con el nombre global —
    // sin normalizar, la re-ejecución idempotente del checker lo regeneraría con el nombre local
    // sin resolver ("trait 'ToJson' not declared").
    fn trait_leaf(name: &str) -> &str {
        name.rsplit("::").next().unwrap_or(name)
    }
    let mut existentes: HashSet<(String, String)> = program
        .impls
        .iter()
        .filter_map(|i| impl_target_name(&i.target).map(|n| (trait_leaf(&i.trait_name).to_string(), n.to_string())))
        .collect();
    let mut new_impls: Vec<ImplBlock> = Vec::new();
    for s in &program.structs {
        for a in &s.annotations {
            if a.name != "derive" {
                continue;
            }
            validate_derive(a, &s.name, &s.type_params)?;
            for trait_arg in &a.args {
                if !existentes.insert((trait_arg.clone(), s.name.clone())) {
                    continue; // ya existe ese impl → idempotente
                }
                match trait_arg.as_str() {
                    "Eq" => new_impls.push(parse_derived_impl("Eq", &s.name, "fn eq(self, other: Self) -> bool", &struct_eq_body(&s.fields))),
                    "Show" => new_impls.push(parse_derived_impl("Show", &s.name, "fn show(self) -> string", &struct_show_body(a, &s.name, &s.fields)?)),
                    "Hash" => new_impls.push(parse_derived_impl("Hash", &s.name, "fn hash(self) -> int", &struct_hash_body(&s.fields))),
                    // M93.5: el trait ToJson vive en std/json → el módulo debe tenerlo en ámbito
                    // (`from std/json import ToJson;`); si no, el impl generado da "unknown trait".
                    "ToJson" => new_impls.push(parse_derived_impl("ToJson", &s.name, "fn to_json(self) -> string", &struct_tojson_body(a, &s.fields)?)),
                    _ => crate::ice!("validate_derive guarantees a known trait"),
                }
            }
        }
    }
    for e in &program.enums {
        for a in &e.annotations {
            if a.name != "derive" {
                continue;
            }
            validate_derive(a, &e.name, &e.type_params)?;
            for trait_arg in &a.args {
                if !existentes.insert((trait_arg.clone(), e.name.clone())) {
                    continue;
                }
                match trait_arg.as_str() {
                    "Eq" => new_impls.push(parse_derived_impl("Eq", &e.name, "fn eq(self, other: Self) -> bool", &enum_eq_body(&e.name, &e.variants))),
                    "Show" => new_impls.push(parse_derived_impl("Show", &e.name, "fn show(self) -> string", &enum_show_body(a, &e.name, &e.variants)?)),
                    "Hash" => new_impls.push(parse_derived_impl("Hash", &e.name, "fn hash(self) -> int", &enum_hash_body(&e.name, &e.variants))),
                    // M93.5: la representación JSON de un enum es una decisión abierta → diferido.
                    "ToJson" => {
                        return Err(TypeError { msg: "cannot derive ToJson for an enum (only structs for now)".into(), line: a.line, col: a.col, len: 1 });
                    }
                    _ => crate::ice!("validate_derive guarantees a known trait"),
                }
            }
        }
    }
    // M40.3a: dar a cada cuerpo derivado posiciones **sintéticas únicas y globales**. Cada impl se
    // parsea desde la línea 1, así que dos derivados (o el mismo re-generado por módulo) colisionarían
    // en las bajadas por posición (UFCS/despacho): p. ej. `self.x.hash()` (int) y `self.n.hash()`
    // (string) en la misma `(línea, col)` se bajarían al MISMO destino → despacho equivocado. Un
    // contador atómico global reserva una banda de 1M por método (base 50M, disjunta de la 1M de los
    // métodos por defecto y muy por encima de cualquier fuente real). Antes solo funcionaba por suerte
    // cuando los campos colisionantes iban al mismo destino (p. ej. `@derive(Show)` con campos del
    // mismo tipo). Ver `freshen_positions`.
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DERIVE_FRESH: AtomicUsize = AtomicUsize::new(49_000_000);
    for imp in &mut new_impls {
        for m in &mut imp.methods {
            let mut next = DERIVE_FRESH.fetch_add(1_000_000, Ordering::Relaxed);
            freshen_positions(&mut m.body, &mut next);
        }
    }
    program.impls.extend(new_impls);
    Ok(())
}

/// El nombre del tipo objetivo de un `impl` (`Struct`/`Enum`), si lo tiene.
pub(super) fn impl_target_name(t: &Type) -> Option<&str> {
    match t {
        Type::Struct(n, _) | Type::Enum(n, _) => Some(n),
        _ => None,
    }
}

/// Valida `@derive(...)` sobre un tipo: argumentos no vacíos, todos derivables (`Eq`/`Show`),
/// y el tipo no genérico (M9.1 no admite impls genéricos).
pub(super) fn validate_derive(a: &Annotation, name: &str, type_params: &[String]) -> Result<(), TypeError> {
    if a.args.is_empty() {
        return Err(TypeError { msg: "'@derive' requires at least one trait (e.g. @derive(Eq))".into(), line: a.line, col: a.col, len: 1 });
    }
    for arg in &a.args {
        if arg != "Eq" && arg != "Show" && arg != "Hash" && arg != "ToJson" {
            return Err(TypeError { msg: format!("cannot derive '{}' (for now Eq, Show, Hash and ToJson)", arg), line: a.line, col: a.col, len: 1 });
        }
    }
    if !type_params.is_empty() {
        return Err(TypeError { msg: format!("cannot derive for the generic type '{}'", name), line: a.line, col: a.col, len: 1 });
    }
    Ok(())
}

/// Cómo renderizar un valor a string según su tipo (M11.2/L2): primitivos vía `to_string`;
/// struct/enum vía `mostrar()` (Show recursivo). Arrays/funciones/etc. no son derivables aún.
/// `a` aporta la posición del `@derive` para ubicar el error.
pub(super) fn render_to_string(a: &Annotation, expr: &str, ty: &Type) -> Result<String, TypeError> {
    match ty {
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Char => Ok(format!("to_string({expr})")),
        // En esta fase un tipo de usuario llega como `Struct` (el checker aún no lo resolvió a
        // `Enum`); ambos se imprimen con su propio `mostrar` (deben implementar Show).
        Type::Struct(_, _) | Type::Enum(_, _) => Ok(format!("{expr}.show()")),
        other => Err(TypeError {
            msg: format!("cannot derive Show for a field of type {} (for now primitives, struct and enum)", other),
            line: a.line,
            col: a.col,
            len: 1,
        }),
    }
}

/// Cuerpo de `mostrar` para un struct: `"Nombre { campo: <v>, … }"` (sin campos → `"Nombre"`).
pub(super) fn struct_show_body(a: &Annotation, name: &str, fields: &[(String, Type)]) -> Result<String, TypeError> {
    if fields.is_empty() {
        return Ok(format!("        \"{name}\""));
    }
    let mut parts: Vec<String> = Vec::new();
    for (n, ty) in fields {
        parts.push(format!("\"{n}: \" + {}", render_to_string(a, &format!("self.{n}"), ty)?));
    }
    // El string generado usa llaves literales `{`/`}` (siempre lo son; solo `${` interpola, M27.3).
    Ok(format!("        \"{name} {{ \" + {} + \" }}\"", parts.join(" + \", \" + ")))
}

/// Cuerpo de `mostrar` para un enum: `match` sobre `self`; por variante, `"Nombre.Variante"`
/// (unit) o `"Nombre.Variante(<v0>, <v1>)"` (con payload).
pub(super) fn enum_show_body(a: &Annotation, name: &str, variants: &[VariantDef]) -> Result<String, TypeError> {
    let mut arms = String::new();
    for v in variants {
        let k = v.payload.len();
        if k == 0 {
            arms.push_str(&format!("            {name}.{v} => \"{name}.{v}\",\n", v = v.name));
        } else {
            let binds: Vec<String> = (0..k).map(|i| format!("a{i}")).collect();
            let mut pieces: Vec<String> = Vec::new();
            for (i, ty) in v.payload.iter().enumerate() {
                pieces.push(render_to_string(a, &format!("a{i}"), ty)?);
            }
            arms.push_str(&format!(
                "            {name}.{v}({b}) => \"{name}.{v}(\" + {p} + \")\",\n",
                v = v.name, b = binds.join(", "), p = pieces.join(" + \", \" + ")
            ));
        }
    }
    Ok(format!("        match (self) {{\n{arms}        }}"))
}

/// Cómo serializar un campo a JSON (M93.5): primitivos vía su `.to_json()` de std/json (char
/// pasa por `to_string` — JSON no tiene char); struct/enum vía `.to_json()` recursivo (deben
/// implementar ToJson, p. ej. con su propio derive). Arrays/Map/funciones no derivables aún.
pub(super) fn render_to_json(a: &Annotation, expr: &str, ty: &Type) -> Result<String, TypeError> {
    match ty {
        Type::Int | Type::Float | Type::Bool | Type::String => Ok(format!("{expr}.to_json()")),
        Type::Char => Ok(format!("to_string({expr}).to_json()")),
        Type::Struct(_, _) | Type::Enum(_, _) => Ok(format!("{expr}.to_json()")),
        other => Err(TypeError {
            msg: format!("cannot derive ToJson for a field of type {} (for now primitives, struct and enum)", other),
            line: a.line,
            col: a.col,
            len: 1,
        }),
    }
}

/// Cuerpo de `to_json` para un struct: `{"campo": <v>, …}` (claves = nombres de campo; los
/// valores se serializan con su `to_json`, escapado por construcción). Sin campos → `{}`.
pub(super) fn struct_tojson_body(a: &Annotation, fields: &[(String, Type)]) -> Result<String, TypeError> {
    if fields.is_empty() {
        return Ok("        \"{}\"".to_string());
    }
    let mut parts: Vec<String> = Vec::new();
    for (n, ty) in fields {
        parts.push(format!("\"\\\"{n}\\\": \" + {}", render_to_json(a, &format!("self.{n}"), ty)?));
    }
    Ok(format!("        \"{{\" + {} + \"}}\"", parts.join(" + \", \" + ")))
}

/// Construye y parsea `impl Trait for <name> {{ <firma> {{ body }} }}` para un derive.
pub(super) fn parse_derived_impl(trait_name: &str, name: &str, signature: &str, body: &str) -> ImplBlock {
    let src = format!(
        "impl {trait_name} for {name} {{\n    {signature} {{\n{body}\n    }}\n}}"
    );
    let toks = crate::lexer::lex(&src).unwrap_or_else(|e| crate::ice!("the derived impl does not lex: {e}"));
    let mut prog = crate::parser::parse(toks).unwrap_or_else(|e| crate::ice!("the derived impl does not parse: {e}"));
    prog.impls.remove(0)
}

/// Cuerpo de `igual` para un struct: conjunción de la igualdad de cada campo (sin campos →
/// `true`).
/// Cuerpo de `hash` para un struct (M40.3a): combina el `.hash()` de cada campo con un polinomio
/// `h = h*31 + campo.hash()` (arranca en 17). Sin campos → `17`. Cada campo debe implementar `Hash`
/// (el checker lo exige al verificar el cuerpo generado); un campo no hashable (float/array) → error.
/// M61.1: el int es checked (trap) → tanto el acumulador como el hash ENTRANTE de cada campo
/// (que puede ser cualquier i64, p. ej. un int grande que hashea a sí mismo) se acotan a 32 bits.
pub(super) fn struct_hash_body(fields: &[(String, Type)]) -> String {
    let mut acc = "17".to_string();
    for (n, _) in fields {
        acc = format!("(({acc} * 31 + (self.{n}.hash() & 4294967295)) & 4294967295)");
    }
    format!("        {acc}")
}

/// Cuerpo de `hash` para un enum (M40.3a): `match` sobre `self`; el hash arranca en el índice de la
/// variante y combina el `.hash()` de cada elemento del payload (variante unit → su índice).
pub(super) fn enum_hash_body(name: &str, variants: &[VariantDef]) -> String {
    let mut arms = String::new();
    for (idx, v) in variants.iter().enumerate() {
        let k = v.payload.len();
        if k == 0 {
            arms.push_str(&format!("            {name}.{v} => {idx},\n", v = v.name));
        } else {
            let binds: Vec<String> = (0..k).map(|i| format!("a{i}")).collect();
            let mut acc = format!("{idx}");
            for i in 0..k {
                // M61.1: acotado a 32 bits, como struct_hash_body (el int es checked).
                acc = format!("(({acc} * 31 + (a{i}.hash() & 4294967295)) & 4294967295)");
            }
            arms.push_str(&format!(
                "            {name}.{v}({b}) => {acc},\n",
                v = v.name, b = binds.join(", ")
            ));
        }
    }
    format!("        match (self) {{\n{arms}        }}")
}

pub(super) fn struct_eq_body(fields: &[(String, Type)]) -> String {
    if fields.is_empty() {
        return "        true".into();
    }
    let cmps: Vec<String> = fields.iter().map(|(n, _)| format!("self.{n} == other.{n}")).collect();
    format!("        {}", cmps.join(" && "))
}

/// Cuerpo de `igual` para un enum: `match` sobre `self`; por variante, `match` sobre `otro`
/// (misma variante → comparar payload posición a posición; otra → `false`).
pub(super) fn enum_eq_body(name: &str, variants: &[VariantDef]) -> String {
    let mut arms = String::new();
    for v in variants {
        let k = v.payload.len();
        if k == 0 {
            arms.push_str(&format!(
                "            {name}.{v} => match (other) {{ {name}.{v} => true, _ => false }},\n",
                v = v.name
            ));
        } else {
            let a: Vec<String> = (0..k).map(|i| format!("a{i}")).collect();
            let b: Vec<String> = (0..k).map(|i| format!("b{i}")).collect();
            let cmp: Vec<String> = (0..k).map(|i| format!("a{i} == b{i}")).collect();
            arms.push_str(&format!(
                "            {name}.{v}({a}) => match (other) {{ {name}.{v}({b}) => {cmp}, _ => false }},\n",
                v = v.name, a = a.join(", "), b = b.join(", "), cmp = cmp.join(" && ")
            ));
        }
    }
    format!("        match (self) {{\n{arms}        }}")
}

/// Nombre del parámetro-diccionario para un método de un trait acotado (M9.2):
/// `T#Trait#metodo`. Como el `#` es ilegal en identificadores, no choca con locales del
/// usuario; vive como un parámetro función más.
pub(super) fn dict_param_name(tparam: &str, trait_name: &str, method: &str) -> String {
    format!("{}#{}#{}", tparam, trait_name, method)
}

/// Tipo función de un método visto desde fuera (M9.2): incluye `self` como primer
/// parámetro. Con `Self → self_ty` (un `Var(T)` para un diccionario, un tipo concreto en
/// otros usos). P. ej. `mostrar(self) -> string` con `self_ty = T` da `fn(T) -> string`.
pub(super) fn method_fn_type(m: &MethodSig, self_ty: &Type) -> Type {
    let params: Vec<Type> = m.params.iter().map(|p| subst_self(&p.ty, self_ty)).collect();
    Type::Fn(params, Box::new(subst_self(&m.return_type, self_ty)))
}

/// Renumera las posiciones `(línea, col)` de todos los nodos de un bloque a un rango
/// **sintético único** (M9.3a). Un cuerpo de método **por defecto** se clona una vez por
/// impl que lo hereda; como las bajadas (UFCS, despacho, diccionarios, coerciones) se
/// indexan por posición, dos clones con las posiciones originales del trait colisionarían
/// y se resolverían al mismo destino. Darle a cada clon posiciones únicas (y mayores que
/// cualquier línea real, base 1_000_000) las separa. Las posiciones sintéticas degradan el
/// contexto de fuente de un eventual error dentro del defecto (raro), no la corrección.
pub(super) fn freshen_positions(block: &mut Block, next: &mut usize) {
    freshen_block(block, next);
}

pub(super) fn freshen_block(block: &mut Block, next: &mut usize) {
    for stmt in &mut block.statements {
        *next += 1;
        stmt.line = 1_000_000 + *next;
        stmt.col = 1;
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => freshen_expr(value, next),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { freshen_expr(start, next); freshen_expr(end, next); }
                    ForIter::In(e) => freshen_expr(e, next),
                    ForIter::Iter { expr, .. } => freshen_expr(expr, next),
                }
                freshen_block(body, next);
            }
            StmtKind::Assign { target, value } => {
                freshen_expr(target, next);
                freshen_expr(value, next);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    freshen_expr(v, next);
                }
            }
            StmtKind::Expr(e) => freshen_expr(e, next),
        }
    }
    if let Some(t) = &mut block.tail {
        freshen_expr(t, next);
    }
    *next += 1;
    block.line = 1_000_000 + *next;
    block.col = 1;
}

pub(super) fn freshen_expr(expr: &mut Expr, next: &mut usize) {
    *next += 1;
    expr.line = 1_000_000 + *next;
    expr.col = 1;
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => freshen_expr(inner, next),
        ExprKind::Binary { left, right, .. } => {
            freshen_expr(left, next);
            freshen_expr(right, next);
        }
        ExprKind::Call { callee, args } => {
            freshen_expr(callee, next);
            for a in args {
                freshen_expr(a, next);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                freshen_expr(e, next);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { freshen_expr(k, next); freshen_expr(v, next); }
        }
        ExprKind::Index { array, index } => {
            freshen_expr(array, next);
            freshen_expr(index, next);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                freshen_expr(e, next);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                freshen_expr(a, next);
            }
        }
        ExprKind::Field { object, .. } => freshen_expr(object, next),
        ExprKind::Func(fe) => freshen_block(&mut fe.body, next),
        ExprKind::Match { scrutinee, arms } => {
            freshen_expr(scrutinee, next);
            for arm in arms {
                freshen_expr(&mut arm.body, next); if let Some(g) = &mut arm.guard { freshen_expr(g, next); }
            }
        }
        ExprKind::Try(inner) => freshen_expr(inner, next),
        ExprKind::If { cond, then_branch, else_branch } => {
            freshen_expr(cond, next);
            freshen_block(then_branch, next);
            if let Some(e) = else_branch {
                freshen_expr(e, next);
            }
        }
        ExprKind::While { cond, body } => {
            freshen_expr(cond, next);
            freshen_block(body, next);
        }
        ExprKind::Block(b) => freshen_block(b, next),
        _ => {}
    }
}

/// Reasigna los `id` de todos los fn-exprs del programa a un rango denso `0..N` (M9.2b).
/// El lowering pudo inyectar closures sintéticos (diccionarios anidados) con `id` provisional;
/// el intérprete y la VM indexan la tabla de funciones por `id` y `collect_fn_exprs` exige que
/// sean densos. Recorre el AST en el mismo orden que `collect_fn_exprs` y numera al vuelo. El
/// orden concreto da igual (ambos motores reconstruyen por `id`), basta con que sea una
/// biyección sobre todos los fn-exprs alcanzables.
pub(super) fn renumber_fn_exprs(program: &mut Program) {
    let mut next = 0usize;
    for f in &mut program.functions {
        renumber_block(&mut f.body, &mut next);
    }
}

pub(super) fn renumber_block(block: &mut Block, next: &mut usize) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => renumber_expr(value, next),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { renumber_expr(start, next); renumber_expr(end, next); }
                    ForIter::In(e) => renumber_expr(e, next),
                    ForIter::Iter { expr, .. } => renumber_expr(expr, next),
                }
                renumber_block(body, next);
            }
            StmtKind::Assign { target, value } => {
                renumber_expr(target, next);
                renumber_expr(value, next);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    renumber_expr(v, next);
                }
            }
            StmtKind::Expr(e) => renumber_expr(e, next),
        }
    }
    if let Some(t) = &mut block.tail {
        renumber_expr(t, next);
    }
}

pub(super) fn renumber_expr(expr: &mut Expr, next: &mut usize) {
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => renumber_expr(inner, next),
        ExprKind::Binary { left, right, .. } => {
            renumber_expr(left, next);
            renumber_expr(right, next);
        }
        ExprKind::Call { callee, args } => {
            renumber_expr(callee, next);
            for a in args {
                renumber_expr(a, next);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                renumber_expr(e, next);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { renumber_expr(k, next); renumber_expr(v, next); }
        }
        ExprKind::Index { array, index } => {
            renumber_expr(array, next);
            renumber_expr(index, next);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                renumber_expr(e, next);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                renumber_expr(a, next);
            }
        }
        ExprKind::Field { object, .. } => renumber_expr(object, next),
        // Pre-orden (igual que `collect_fn_exprs`): el fn-expr toma su id antes de recursar.
        ExprKind::Func(fe) => {
            fe.id = *next;
            *next += 1;
            renumber_block(&mut fe.body, next);
        }
        ExprKind::Match { scrutinee, arms } => {
            renumber_expr(scrutinee, next);
            for arm in arms {
                renumber_expr(&mut arm.body, next); if let Some(g) = &mut arm.guard { renumber_expr(g, next); }
            }
        }
        ExprKind::Try(inner) => renumber_expr(inner, next),
        ExprKind::If { cond, then_branch, else_branch } => {
            renumber_expr(cond, next);
            renumber_block(then_branch, next);
            if let Some(e) = else_branch {
                renumber_expr(e, next);
            }
        }
        ExprKind::While { cond, body } => {
            renumber_expr(cond, next);
            renumber_block(body, next);
        }
        ExprKind::Block(b) => renumber_block(b, next),
        _ => {}
    }
}

/// M52: **inlining de forwarders triviales**. Los impls-para-builtins de M48.4 (`impl<T> Push<T>
/// for [T] { fn push(self, x) { __push(self, x) } }`) hacen que cada `a.push(i)` pague una llamada
/// VM completa (marco + call + return) para ejecutar UN opcode — medido: arrays/gcnested +38-39 %
/// respecto al opcode directo pre-M48.4 (IDEAS §11). Este pase detecta las funciones **manglada**
/// (`Tipo#metodo`; un local no puede llamarse así → reescribir el callee es seguro) cuyo cuerpo es
/// **exactamente una llamada a builtin pasando sus params en orden**, y reescribe cada sitio de
/// llamada `Tipo#metodo(args)` a `__builtin(args)`. Los args se evalúan igual y en el mismo orden →
/// semántica idéntica en ambos motores (el oráculo no se toca). El forwarder NO se elimina: puede
/// seguir referenciado como valor (vtables de `dyn`, diccionarios de bounds).
pub(super) fn inline_forwarders(program: &mut Program) {
    // 1. Mapa método-manglado → builtin. Solo funciones sin bounds (con bounds,
    //    `append_dict_params` ya les añadió params-diccionario y el patrón no casa).
    let user_fns: HashSet<&str> = program.functions.iter().map(|f| f.name.as_str()).collect();
    let mut fwd: HashMap<String, String> = HashMap::new();
    for f in &program.functions {
        if !f.name.contains('#') || !f.bounds.is_empty() || !f.body.statements.is_empty() {
            continue;
        }
        let Some(tail) = &f.body.tail else { continue };
        let ExprKind::Call { callee, args } = &tail.kind else { continue };
        let ExprKind::Ident(b) = &callee.kind else { continue };
        // Debe ser un builtin de verdad (y no taparlo una función del programa homónima).
        if crate::builtins::lookup(b).is_none() || user_fns.contains(b.as_str()) {
            continue;
        }
        let en_order = args.len() == f.params.len()
            && args
                .iter()
                .zip(&f.params)
                .all(|(a, p)| matches!(&a.kind, ExprKind::Ident(n) if *n == p.name));
        if en_order {
            fwd.insert(f.name.clone(), b.clone());
        }
    }
    if fwd.is_empty() {
        return;
    }
    // 1b. Sonoridad: el compilador resuelve variable-local ANTES que builtin, así que si en algún
    //     sitio del programa hay una variable ligada con el nombre de un builtin objetivo (un
    //     `let __push = …`, legal aunque exótico), reescribir hacia ese nombre podría capturarla.
    //     Aproximación conservadora: se excluye ese builtin del inlining en TODO el programa
    //     (coste cero en la práctica: nadie liga nombres `__*`).
    let mut bound: HashSet<String> = HashSet::new();
    for f in &program.functions {
        for p in &f.params {
            bound.insert(p.name.clone());
        }
        collect_bound_names_block(&f.body, &mut bound);
    }
    fwd.retain(|_, b| !bound.contains(b));
    if fwd.is_empty() {
        return;
    }
    // 2. Reescribir los sitios de llamada en todo el AST (incluidos cuerpos de fn-exprs).
    for f in &mut program.functions {
        inline_forwarders_block(&mut f.body, &fwd);
    }
}

/// M52: recolecta todos los nombres que el programa liga como **variables** (let/var, tuplas,
/// `for`, bindings de `match`, params de fn anónimas) — soporte de la guarda de sonoridad de
/// `inline_forwarders` (ver arriba). No distingue ámbitos: es una aproximación conservadora.
pub(super) fn collect_bound_names_block(block: &Block, bound: &mut HashSet<String>) {
    for stmt in &block.statements {
        match &stmt.kind {
            StmtKind::Let { name, value, .. } => {
                bound.insert(name.clone());
                collect_bound_names_expr(value, bound);
            }
            StmtKind::LetTuple { names, value, .. } => {
                for n in names.iter().flatten() {
                    bound.insert(n.clone());
                }
                collect_bound_names_expr(value, bound);
            }
            StmtKind::For { pat, iter, body } => {
                match pat {
                    ForPat::Single(n) => {
                        bound.insert(n.clone());
                    }
                    ForPat::Tuple(ns) => {
                        for n in ns.iter().flatten() {
                            bound.insert(n.clone());
                        }
                    }
                }
                match iter {
                    ForIter::Range { start, end } => {
                        collect_bound_names_expr(start, bound);
                        collect_bound_names_expr(end, bound);
                    }
                    ForIter::In(e) | ForIter::Iter { expr: e, .. } => collect_bound_names_expr(e, bound),
                }
                collect_bound_names_block(body, bound);
            }
            StmtKind::Assign { target, value } => {
                collect_bound_names_expr(target, bound);
                collect_bound_names_expr(value, bound);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    collect_bound_names_expr(v, bound);
                }
            }
            StmtKind::Expr(e) => collect_bound_names_expr(e, bound),
        }
    }
    if let Some(t) = &block.tail {
        collect_bound_names_expr(t, bound);
    }
}

pub(super) fn collect_bound_names_pattern(pat: &Pattern, bound: &mut HashSet<String>) {
    match &pat.kind {
        PatternKind::Binding(n) => {
            bound.insert(n.clone());
        }
        PatternKind::Variant { subpatterns, .. } => {
            for sp in subpatterns {
                collect_bound_names_pattern(sp, bound);
            }
        }
        _ => {}
    }
}

pub(super) fn collect_bound_names_expr(expr: &Expr, bound: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } | ExprKind::Try(inner) => {
            collect_bound_names_expr(inner, bound)
        }
        ExprKind::Binary { left, right, .. } => {
            collect_bound_names_expr(left, bound);
            collect_bound_names_expr(right, bound);
        }
        ExprKind::Call { callee, args } => {
            collect_bound_names_expr(callee, bound);
            for a in args {
                collect_bound_names_expr(a, bound);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                collect_bound_names_expr(e, bound);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares {
                collect_bound_names_expr(k, bound);
                collect_bound_names_expr(v, bound);
            }
        }
        ExprKind::Index { array, index } => {
            collect_bound_names_expr(array, bound);
            collect_bound_names_expr(index, bound);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                collect_bound_names_expr(e, bound);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                collect_bound_names_expr(a, bound);
            }
        }
        ExprKind::Field { object, .. } => collect_bound_names_expr(object, bound),
        ExprKind::Func(fe) => {
            for p in &fe.params {
                bound.insert(p.name.clone());
            }
            collect_bound_names_block(&fe.body, bound);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_bound_names_expr(scrutinee, bound);
            for arm in arms {
                collect_bound_names_pattern(&arm.pattern, bound);
                collect_bound_names_expr(&arm.body, bound);
                if let Some(g) = &arm.guard {
                    collect_bound_names_expr(g, bound);
                }
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            collect_bound_names_expr(cond, bound);
            collect_bound_names_block(then_branch, bound);
            if let Some(e) = else_branch {
                collect_bound_names_expr(e, bound);
            }
        }
        ExprKind::While { cond, body } => {
            collect_bound_names_expr(cond, bound);
            collect_bound_names_block(body, bound);
        }
        ExprKind::Block(b) => collect_bound_names_block(b, bound),
        _ => {}
    }
}

pub(super) fn inline_forwarders_block(block: &mut Block, fwd: &HashMap<String, String>) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => inline_forwarders_expr(value, fwd),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => {
                        inline_forwarders_expr(start, fwd);
                        inline_forwarders_expr(end, fwd);
                    }
                    ForIter::In(e) => inline_forwarders_expr(e, fwd),
                    ForIter::Iter { expr, .. } => inline_forwarders_expr(expr, fwd),
                }
                inline_forwarders_block(body, fwd);
            }
            StmtKind::Assign { target, value } => {
                inline_forwarders_expr(target, fwd);
                inline_forwarders_expr(value, fwd);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    inline_forwarders_expr(v, fwd);
                }
            }
            StmtKind::Expr(e) => inline_forwarders_expr(e, fwd),
        }
    }
    if let Some(t) = &mut block.tail {
        inline_forwarders_expr(t, fwd);
    }
}

pub(super) fn inline_forwarders_expr(expr: &mut Expr, fwd: &HashMap<String, String>) {
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => inline_forwarders_expr(inner, fwd),
        ExprKind::Binary { left, right, .. } => {
            inline_forwarders_expr(left, fwd);
            inline_forwarders_expr(right, fwd);
        }
        ExprKind::Call { callee, args } => {
            // El corazón del pase: renombrar el callee si es un forwarder conocido. Solo en
            // posición de llamada (una referencia como VALOR debe seguir apuntando a la función).
            if let ExprKind::Ident(n) = &mut callee.kind
                && let Some(b) = fwd.get(n)
            {
                *n = b.clone();
            }
            inline_forwarders_expr(callee, fwd);
            for a in args {
                inline_forwarders_expr(a, fwd);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                inline_forwarders_expr(e, fwd);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares {
                inline_forwarders_expr(k, fwd);
                inline_forwarders_expr(v, fwd);
            }
        }
        ExprKind::Index { array, index } => {
            inline_forwarders_expr(array, fwd);
            inline_forwarders_expr(index, fwd);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                inline_forwarders_expr(e, fwd);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                inline_forwarders_expr(a, fwd);
            }
        }
        ExprKind::Field { object, .. } => inline_forwarders_expr(object, fwd),
        ExprKind::Func(fe) => inline_forwarders_block(&mut fe.body, fwd),
        ExprKind::Match { scrutinee, arms } => {
            inline_forwarders_expr(scrutinee, fwd);
            for arm in arms {
                inline_forwarders_expr(&mut arm.body, fwd);
                if let Some(g) = &mut arm.guard {
                    inline_forwarders_expr(g, fwd);
                }
            }
        }
        ExprKind::Try(inner) => inline_forwarders_expr(inner, fwd),
        ExprKind::If { cond, then_branch, else_branch } => {
            inline_forwarders_expr(cond, fwd);
            inline_forwarders_block(then_branch, fwd);
            if let Some(e) = else_branch {
                inline_forwarders_expr(e, fwd);
            }
        }
        ExprKind::While { cond, body } => {
            inline_forwarders_expr(cond, fwd);
            inline_forwarders_block(body, fwd);
        }
        ExprKind::Block(b) => inline_forwarders_block(b, fwd),
        _ => {}
    }
}

/// M40.2: baja `for x in it` (sobre un iterador) reescribiendo `ForIter::In` a `ForIter::Iter` con
/// el nombre manglado de `next`, en cada `for` cuya `(línea, col)` esté en `sites`. Recorre todo el
/// AST (bloques anidados en if/while/match/fn) para alcanzar cualquier `for`.
pub(super) fn lower_for_iters(program: &mut Program, sites: &HashMap<(usize, usize), String>) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_for_iters_block(&mut f.body, sites);
    }
}

pub(super) fn lower_for_iters_block(block: &mut Block, sites: &HashMap<(usize, usize), String>) {
    for stmt in &mut block.statements {
        let pos = (stmt.line, stmt.col);
        match &mut stmt.kind {
            StmtKind::For { iter, body, .. } => {
                if let (Some(next_fn), ForIter::In(_)) = (sites.get(&pos), &*iter) {
                    let old = std::mem::replace(iter, ForIter::In(Expr { kind: ExprKind::Int(0, crate::token::Radix::Dec), line: 0, col: 0 }));
                    if let ForIter::In(e) = old {
                        *iter = ForIter::Iter { expr: e, next_fn: next_fn.clone() };
                    }
                }
                match iter {
                    ForIter::Range { start, end } => { lower_for_iters_expr(start, sites); lower_for_iters_expr(end, sites); }
                    ForIter::In(e) => lower_for_iters_expr(e, sites),
                    ForIter::Iter { expr, .. } => lower_for_iters_expr(expr, sites),
                }
                lower_for_iters_block(body, sites);
            }
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_for_iters_expr(value, sites),
            StmtKind::Assign { target, value } => { lower_for_iters_expr(target, sites); lower_for_iters_expr(value, sites); }
            StmtKind::Return { value } => { if let Some(v) = value { lower_for_iters_expr(v, sites); } }
            StmtKind::Expr(e) => lower_for_iters_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_for_iters_expr(t, sites);
    }
}

pub(super) fn lower_for_iters_expr(expr: &mut Expr, sites: &HashMap<(usize, usize), String>) {
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } | ExprKind::Try(inner) => lower_for_iters_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => { lower_for_iters_expr(left, sites); lower_for_iters_expr(right, sites); }
        ExprKind::Call { callee, args } => { lower_for_iters_expr(callee, sites); for a in args { lower_for_iters_expr(a, sites); } }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => { for e in elems { lower_for_iters_expr(e, sites); } }
        ExprKind::MapLit(pares) => { for (k, v) in pares { lower_for_iters_expr(k, sites); lower_for_iters_expr(v, sites); } }
        ExprKind::Index { array, index } => { lower_for_iters_expr(array, sites); lower_for_iters_expr(index, sites); }
        ExprKind::StructLit { fields, .. } => { for (_, e) in fields { lower_for_iters_expr(e, sites); } }
        ExprKind::EnumLit { args, .. } => { for a in args { lower_for_iters_expr(a, sites); } }
        ExprKind::Field { object, .. } => lower_for_iters_expr(object, sites),
        ExprKind::Func(fe) => lower_for_iters_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_for_iters_expr(scrutinee, sites);
            for arm in arms {
                lower_for_iters_expr(&mut arm.body, sites);
                if let Some(g) = &mut arm.guard { lower_for_iters_expr(g, sites); }
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_for_iters_expr(cond, sites);
            lower_for_iters_block(then_branch, sites);
            if let Some(e) = else_branch { lower_for_iters_expr(e, sites); }
        }
        ExprKind::While { cond, body } => { lower_for_iters_expr(cond, sites); lower_for_iters_block(body, sites); }
        ExprKind::Block(b) => lower_for_iters_block(b, sites),
        _ => {}
    }
}

/// ¿El tipo menciona `Self` (M9.3b)? Cubre `SelfType` y `Struct("Self")`, recursivamente.
/// Lo usa la *object safety*: un método cuya firma (fuera del receptor) usa `Self` no es
/// invocable sobre un trait object.
pub(super) fn type_uses_self(ty: &Type) -> bool {
    match ty {
        Type::SelfType => true,
        Type::Struct(n, _) if n == "Self" => true,
        Type::Array(e) => type_uses_self(e),
        Type::Map(k, v) => type_uses_self(k) || type_uses_self(v),
        Type::Channel(t) => type_uses_self(t),
        Type::Task(t) => type_uses_self(t),
        Type::Fn(ps, r) => ps.iter().any(type_uses_self) || type_uses_self(r),
        Type::Struct(_, args) | Type::Enum(_, args) => args.iter().any(type_uses_self),
        _ => false,
    }
}

/// Sustituye `Self` por el tipo implementador (M9). Cubre las dos formas con que `Self`
/// llega del parser: `Type::SelfType` (el receptor `self`) y `Struct("Self")` (en una
/// anotación como `-> Self`).
/// M40.2c: sustituye los parámetros de tipo de un trait por los argumentos del impl. En el AST
/// crudo (pre-resolución) un parámetro de tipo aparece como `Struct(nombre, [])`; aquí se reemplaza
/// por su argumento. Se usa al bajar un método de un impl de trait parametrizado
/// (`impl Iterator<int> for RangeIter`) para que `fn map<U>(self, f: fn(T) -> U)` herede `T = int`
/// en su firma. Como `subst_self` pero por nombre y para varios parámetros a la vez.
pub(super) fn subst_named(ty: &Type, sigma: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Struct(n, args) if args.is_empty() && sigma.contains_key(n) => sigma[n].clone(),
        Type::Array(e) => Type::Array(Box::new(subst_named(e, sigma))),
        Type::Map(k, v) => Type::Map(Box::new(subst_named(k, sigma)), Box::new(subst_named(v, sigma))),
        Type::Channel(t) => Type::Channel(Box::new(subst_named(t, sigma))),
        Type::Task(t) => Type::Task(Box::new(subst_named(t, sigma))),
        Type::Fn(ps, r) => Type::Fn(
            ps.iter().map(|p| subst_named(p, sigma)).collect(),
            Box::new(subst_named(r, sigma)),
        ),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| subst_named(t, sigma)).collect()),
        Type::Struct(n, args) => Type::Struct(n.clone(), args.iter().map(|a| subst_named(a, sigma)).collect()),
        Type::Enum(n, args) => Type::Enum(n.clone(), args.iter().map(|a| subst_named(a, sigma)).collect()),
        other => other.clone(),
    }
}

/// M40.2c: aplica `subst_named` a TODAS las anotaciones de tipo del cuerpo de un método (tipos de
/// `let`, firmas de closures, casts), recursivamente. Necesario porque el cuerpo de un método por
/// defecto genérico puede anotar un parámetro del trait —`filter` escribe `Option<T>`—, y sobre un
/// impl concreto (`impl Iterator<int>`) ese `T` debe volverse `int` (no queda en `type_params`).
pub(super) fn subst_named_block(block: &mut Block, sigma: &HashMap<String, Type>) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { ty, value, .. } => {
                if let Some(t) = ty { *t = subst_named(t, sigma); }
                subst_named_expr(value, sigma);
            }
            StmtKind::LetTuple { value, .. } => subst_named_expr(value, sigma),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { subst_named_expr(start, sigma); subst_named_expr(end, sigma); }
                    ForIter::In(e) => subst_named_expr(e, sigma),
                    ForIter::Iter { expr, .. } => subst_named_expr(expr, sigma),
                }
                subst_named_block(body, sigma);
            }
            StmtKind::Assign { target, value } => { subst_named_expr(target, sigma); subst_named_expr(value, sigma); }
            StmtKind::Return { value } => { if let Some(v) = value { subst_named_expr(v, sigma); } }
            StmtKind::Expr(e) => subst_named_expr(e, sigma),
        }
    }
    if let Some(t) = &mut block.tail { subst_named_expr(t, sigma); }
}

pub(super) fn subst_named_expr(expr: &mut Expr, sigma: &HashMap<String, Type>) {
    match &mut expr.kind {
        ExprKind::Cast { expr: inner, ty } => { subst_named_expr(inner, sigma); *ty = subst_named(ty, sigma); }
        ExprKind::Unary { expr: inner, .. } | ExprKind::Try(inner) => subst_named_expr(inner, sigma),
        ExprKind::Binary { left, right, .. } => { subst_named_expr(left, sigma); subst_named_expr(right, sigma); }
        ExprKind::Call { callee, args } => { subst_named_expr(callee, sigma); for a in args { subst_named_expr(a, sigma); } }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => { for e in elems { subst_named_expr(e, sigma); } }
        ExprKind::MapLit(pares) => { for (k, v) in pares { subst_named_expr(k, sigma); subst_named_expr(v, sigma); } }
        ExprKind::Index { array, index } => { subst_named_expr(array, sigma); subst_named_expr(index, sigma); }
        ExprKind::StructLit { fields, .. } => { for (_, e) in fields { subst_named_expr(e, sigma); } }
        ExprKind::EnumLit { args, .. } => { for a in args { subst_named_expr(a, sigma); } }
        ExprKind::Field { object, .. } => subst_named_expr(object, sigma),
        ExprKind::Func(fe) => {
            for p in &mut fe.params { p.ty = subst_named(&p.ty, sigma); }
            fe.return_type = subst_named(&fe.return_type, sigma);
            subst_named_block(&mut fe.body, sigma);
        }
        ExprKind::Match { scrutinee, arms } => {
            subst_named_expr(scrutinee, sigma);
            for arm in arms {
                subst_named_expr(&mut arm.body, sigma);
                if let Some(g) = &mut arm.guard { subst_named_expr(g, sigma); }
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            subst_named_expr(cond, sigma);
            subst_named_block(then_branch, sigma);
            if let Some(e) = else_branch { subst_named_expr(e, sigma); }
        }
        ExprKind::While { cond, body } => { subst_named_expr(cond, sigma); subst_named_block(body, sigma); }
        ExprKind::Block(b) => subst_named_block(b, sigma),
        _ => {}
    }
}

pub(super) fn subst_self(ty: &Type, target: &Type) -> Type {
    match ty {
        Type::SelfType => target.clone(),
        Type::Struct(n, args) if n == "Self" && args.is_empty() => target.clone(),
        Type::Array(e) => Type::Array(Box::new(subst_self(e, target))),
        Type::Map(k, v) => Type::Map(Box::new(subst_self(k, target)), Box::new(subst_self(v, target))),
        Type::Channel(t) => Type::Channel(Box::new(subst_self(t, target))),
        Type::Task(t) => Type::Task(Box::new(subst_self(t, target))),
        Type::Fn(ps, r) => Type::Fn(
            ps.iter().map(|p| subst_self(p, target)).collect(),
            Box::new(subst_self(r, target)),
        ),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| subst_self(t, target)).collect()),
        Type::Struct(n, args) => {
            Type::Struct(n.clone(), args.iter().map(|a| subst_self(a, target)).collect())
        }
        Type::Enum(n, args) => {
            Type::Enum(n.clone(), args.iter().map(|a| subst_self(a, target)).collect())
        }
        other => other.clone(),
    }
}
