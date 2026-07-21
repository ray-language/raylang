//! El sistema de tipos del backend nativo (movimiento puro; usar `git log --follow`).
//!
//! `normalize_type`/`rust_ty`/`send_type` bajan un `Type` de raylang al tipo Rust que lo
//! representa (o al que cruza un `spawn`/canal); `unify`/`subst_type` resuelven genéricos
//! por unificación (como el checker, M6/M9); `ffi_c_arg_ty`/`ffi_c_ret_ty` son el mapeo FFI.

use super::*;

/// Normaliza los tipos que el parser deja como `Struct` genérico: `Map<K,V>` llega como
/// `Struct("Map",[K,V])` (el checker lo reclasifica en su tabla, no en la anotación del AST). Recursivo.
pub(super) fn normalize_type(t: &Type) -> Type {
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
pub(super) fn rust_ty(raw: &Type, enums: &std::collections::HashSet<String>, tparams: &std::collections::HashSet<String>) -> Result<String, String> {
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
        // Task<T> (structured concurrency): handle al resultado futuro de un `spawn` (estado compartido).
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
        other => return Err(format!("unsupported type {:?}", other)),
    }
    .to_string())
}

/// Recolecta los tipos-**función** que aparecen (incluso ANIDADOS en arreglos/Map/tuplas) en un tipo, a su
/// forma `rust_ty` (`Rc<dyn Fn(..)->..>`). Un `fn` dentro de un contenedor de un campo/payload necesita su
/// propio `impl RayShow` (los impls genéricos de `Vec`/`Map`/tupla exigen `T: RayShow`, y no hay un RayShow
/// único para todas las firmas de función). Solo firmas CONCRETAS (representables): las que mencionan un
/// param de tipo se saltan (`rust_ty` falla → se omite). Dedup en el `BTreeSet` del llamador.
pub(super) fn collect_fn_rayshow(
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
pub(super) fn ty_mentions_tparam(ty: &Type, tps: &std::collections::HashSet<String>) -> bool {
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
pub(super) fn send_type(t: &Type, enums: &std::collections::HashSet<String>, tparams: &std::collections::HashSet<String>) -> Result<String, String> {
    match normalize_type(t) {
        Type::String => Ok("std::sync::Arc<str>".to_string()),
        Type::Bytes => Ok("std::sync::Arc<[u8]>".to_string()),
        Type::Int | Type::Float | Type::Bool | Type::Char | Type::UInt(_) => rust_ty(t, enums, tparams),
        // H21-N5a: los compuestos cruzan como el árbol Send universal (deep copy, semántica M38).
        ty if send_is_tree(&ty) => Ok("__RaySend".to_string()),
        other => Err(format!(
            "channel/task of type {:?} cannot cross a thread boundary in the native backend",
            other
        )),
    }
}

/// ¿Este tipo cruza los hilos como `__RaySend` (árbol Send universal, deep copy)? Los primitivos y
/// string/bytes tienen repr Send directa; las funciones NO cruzan (error claro en los conversores).
pub(super) fn send_is_tree(t: &Type) -> bool {
    matches!(
        normalize_type(t),
        Type::Array(_) | Type::Map(..) | Type::Struct(..) | Type::Enum(..) | Type::Tuple(_) | Type::Unit
    )
}

/// El sufijo `.map(|__rt_x| Rc::…::from(&*__rt_x))` para convertir la repr SEND recibida (Arc) de vuelta a la
/// del programa (Rc). Vacío para primitivos (sin conversión).
pub(super) fn from_send_map(t: &Type) -> &'static str {
    match normalize_type(t) {
        Type::String => ".map(|__rt_x| Rc::<str>::from(&*__rt_x))",
        Type::Bytes => ".map(|__rt_x| Rc::<[u8]>::from(&*__rt_x))",
        _ => "",
    }
}

/// Sustitución `param_de_tipo → arg` para un tipo nominal aplicado: los `tparams` del enum/struct `name`
/// ligados a los args del tipo esperado (`Caja<int>` con tparams `[T]` → `{T: int}`).
pub(super) fn enum_subst(
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
pub(super) fn ffi_c_arg_ty(t: &Type) -> Result<&'static str, String> {
    Ok(match normalize_type(t) {
        Type::Int => "i64",                              // long
        Type::Float => "f64",                            // double
        Type::Bool => "std::os::raw::c_int",             // int (0/1)
        Type::String => "*const std::os::raw::c_char",   // char* (NUL-terminado)
        Type::Bytes => "*const u8",                      // buffer crudo
        Type::Ptr => "*mut std::ffi::c_void",            // void* opaco
        other => return Err(format!("FFI argument is not marshalable: {:?}", other)),
    })
}

/// El tipo C de RETORNO de una `extern fn`. Un `char*` de retorno se modela con `Option` (NULL→None);
/// un `ptr` fallible con `Option<ptr>`. Espejo de `ffi::ret_kind`.
pub(super) fn ffi_c_ret_ty(t: &Type) -> Result<&'static str, String> {
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
            other => return Err(format!("FFI return type Option<{:?}> is not supported", other)),
        },
        other => return Err(format!("FFI return type is not marshalable: {:?}", other)),
    })
}

pub(super) fn generic_decl(tparams: &[String]) -> String {
    generic_bound(tparams, "Clone + RayShow + 'static")
}

/// `<A: bound, B: bound>` para una lista de params de tipo (o "" si vacía).
pub(super) fn generic_bound(tparams: &[String], bound: &str) -> String {
    if tparams.is_empty() {
        String::new()
    } else {
        let ps: Vec<String> = tparams.iter().map(|t| format!("{}: {}", t, bound)).collect();
        format!("<{}>", ps.join(", "))
    }
}

/// Los args de tipo `<A, B>` (sin bounds) para instanciar un tipo genérico (o "" si vacía).
pub(super) fn type_args(tparams: &[String]) -> String {
    if tparams.is_empty() {
        String::new()
    } else {
        format!("<{}>", tparams.join(", "))
    }
}

/// Unifica un tipo de parámetro (que puede llevar variables de tipo) con el tipo real de un argumento,
/// ligando cada variable en `subst`. Asimétrico: las variables son las de la firma llamada (`tparams`).
pub(super) fn unify(param: &Type, arg: &Type, tparams: &[String], subst: &mut HashMap<String, Type>) {
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
pub(super) fn subst_type(t: &Type, subst: &HashMap<String, Type>) -> Type {
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
