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
    // `Iter` (struct del protocolo de iterador del prelude) → saltar sus métodos.
    if key == "Iter" {
        return true;
    }
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
    // wrappers del módulo (llaman a primitivos `__sqrt`/`__read_file` que no transpilamos).
    if name.starts_with("std::math::") || name.starts_with("std::fs::") {
        return true;
    }
    matches!(
        name,
        // --- funciones del prelude (src/prelude.ray) ---
        "all" | "any" | "assert" | "assert_eq" | "char_from_code" | "env" | "filter" | "fold"
            | "from_utf" | "get" | "get_or" | "index_of" | "input" | "iter" | "map" | "max" | "min"
            | "parse_float" | "parse_int" | "pop" | "position" | "range" | "read_int" | "recv"
            | "remove" | "sort" | "sum" | "sum_float" | "try_join"
        // --- builtins públicos manejados en emit_call ---
            | "len" | "push" | "split" | "join" | "chars" | "to_string" | "print" | "eprint"
            | "contains_key" | "keys" | "values" | "insert" | "add_to" | "unwrap" | "unwrap_or"
            | "panic"
    )
}

/// ¿La callee es una llamada a `to_string` (libre o método `x.to_string()`, posiblemente manglada)?
fn is_to_string(callee: &Expr) -> bool {
    match resolve_callee(callee) {
        Ok((n, _)) => n.rsplit('#').next().unwrap_or(n).trim_start_matches("__") == "to_string",
        Err(_) => false,
    }
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
}

/// Transpila un programa (ya chequeado) a Rust autocontenido, o un error si usa algo fuera del subconjunto.
pub fn transpile(prog: &Program) -> Result<String, String> {
    // Índice de firmas de funciones NO genéricas y NO sintéticas (para inferir tipos de llamada).
    let mut funcs = HashMap::new();
    for f in &prog.functions {
        if f.name.starts_with("__") || is_handled_builtin(&f.name) || is_prelude_impl(&f.name) {
            continue;
        }
        funcs.insert(f.name.clone(), FnSig { params: f.params.iter().map(|p| p.ty.clone()).collect(), ret: f.return_type.clone(), tparams: f.type_params.clone() });
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
    out.push_str("impl<T: RayShow> RayShow for Option<T> { fn ray_show(&self) -> String { match self { Some(__v) => format!(\"Option.Some({})\", __v.ray_show()), None => \"Option.None\".to_string() } } }\n");
    out.push_str("impl<T: RayShow, E: RayShow> RayShow for Result<T, E> { fn ray_show(&self) -> String { match self { Ok(__v) => format!(\"Result.Ok({})\", __v.ray_show()), Err(__e) => format!(\"Result.Err({})\", __e.ray_show()) } } }\n\n");

    // Definiciones de tipos de usuario (no genéricos). struct → Rust struct; enum → Rust enum. `Clone`
    // para el clon-al-leer y para los payloads. El orden no importa (Rust permite referencias adelantadas).
    for s in &prog.structs {
        if s.name == "Iter" { continue; } // struct del protocolo de iterador del prelude
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
        writeln!(out, "#[derive(Clone)]\nstruct {}{} {{", s.name, generic_decl(&s.type_params)).unwrap();
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
        writeln!(out, "#[derive(Clone)]\nenum {}{} {{", e.name, generic_decl(&e.type_params)).unwrap();
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
    out.push('\n');

    let mut main_ret_int = false;
    let mut main_seen = false;
    for f in &prog.functions {
        if f.name.starts_with("__") || is_handled_builtin(&f.name) || is_prelude_impl(&f.name) {
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
    // Registro global de handles de archivo (M11.8), solo si el programa los usa. Rust permite items
    // top-level en cualquier orden, así que va al final. Espejo del `FileRegistry` de la VM: un contador +
    // mapa handle→archivo tras un Mutex/OnceLock; los mensajes de error son byte-idénticos a la VM.
    if t.needs_handles {
        out.push_str(concat!(
            "enum __RayHandle { Reader(std::io::BufReader<std::fs::File>), Writer(std::fs::File) }\n",
            "struct __RayReg { next: i64, open: __RayMap<i64, __RayHandle> }\n",
            "fn __ray_reg() -> &'static std::sync::Mutex<__RayReg> {\n",
            "    static R: std::sync::OnceLock<std::sync::Mutex<__RayReg>> = std::sync::OnceLock::new();\n",
            "    R.get_or_init(|| std::sync::Mutex::new(__RayReg { next: 1, open: __RayMap::new() }))\n}\n",
            "fn __ray_open(path: &str, mode: &str) -> Result<i64, Rc<str>> {\n",
            "    let h = match mode {\n",
            "        \"r\" => std::fs::File::open(path).map(|f| __RayHandle::Reader(std::io::BufReader::new(f))),\n",
            "        \"w\" => std::fs::File::create(path).map(__RayHandle::Writer),\n",
            "        \"a\" => std::fs::OpenOptions::new().create(true).append(true).open(path).map(__RayHandle::Writer),\n",
            "        _ => return Err(Rc::<str>::from(format!(\"invalid open mode: '{}' (use \\\"r\\\", \\\"w\\\" or \\\"a\\\")\", mode))),\n",
            "    }.map_err(|e| Rc::<str>::from(e.to_string()))?;\n",
            "    let mut reg = __ray_reg().lock().unwrap(); let id = reg.next; reg.next += 1; reg.open.insert(id, h); Ok(id)\n}\n",
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
            "        None => Err(Rc::<str>::from(format!(\"invalid file handle: {}\", h))) } }\n",
            "fn __ray_close(h: i64) -> i64 { __ray_reg().lock().unwrap().open.remove(&h); 0 }\n",
        ));
    }
    Ok(out)
}

impl Transpiler {
    fn emit_function(&mut self, out: &mut String, rust_name: &str, f: &Function) -> Result<(), String> {
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
        self.emit_block(out, &f.body)?;
        out.push('\n');
        self.scopes.pop();
        self.tparams.clear();
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
                        out.push_str(&mangle(name));
                        out.push_str(" = ");
                        self.emit_typed(out, value, &tty)?; // sized: pina el tipo del literal en el RHS
                        out.push_str(";\n");
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
                    _ => return Err("spike: for sobre iterador (Iterator<T>) no soportado".into()),
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

    fn emit_expr(&mut self, out: &mut String, e: &Expr) -> Result<(), String> {
        match &e.kind {
            ExprKind::Int(n) => write!(out, "{}i64", n).unwrap(),
            ExprKind::Float(x) => write!(out, "{:?}f64", x).unwrap(),
            ExprKind::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            ExprKind::Char(c) => write!(out, "{:?}", c).unwrap(), // `{:?}` de char → literal Rust escapado
            ExprKind::Str(s) => write!(out, "Rc::<str>::from({:?})", s).unwrap(),
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
                if let Some(ty) = self.lookup(name) {
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
                if matches!(self.type_of(array)?, Type::String) {
                    self.emit_expr(out, array)?;
                    out.push_str(".chars().nth(");
                    self.emit_expr(out, index)?;
                    out.push_str(" as usize).unwrap()");
                } else {
                    self.emit_expr(out, array)?;
                    out.push_str(".borrow()[");
                    self.emit_expr(out, index)?;
                    out.push_str(" as usize].clone()");
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
                write!(out, "Rc::new(std::cell::RefCell::new({} {{ ", name).unwrap();
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
                    write!(out, "Rc::new({}::{}", enum_name, variant).unwrap();
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
                self.emit_block(out, &fnexpr.body)?;
                self.scopes.pop();
                out.push(')');
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
            other => return Err(format!("spike: expresión no soportada {:?}", other)),
        }
        Ok(())
    }

    /// Emite `impl Display` para cada struct/enum (= el `Show` de raylang): struct → `Name { f: v, … }`,
    /// enum → `Name.Variant(payload)` / `Name.Variant`. Recursivo (un campo/payload struct se `borrow`ea).
    /// Genera `impl RayShow` para cada struct/enum de usuario (recursivo; genérico-consciente con
    /// `where` `A: RayShow`). struct → `Name { f: v, … }`; enum → `Name.Variant(payload)` / `Name.Variant`.
    fn emit_rayshow_impls(&self, out: &mut String, prog: &Program) -> Result<(), String> {
        for s in &prog.structs {
            if s.name == "Iter" || s.name.starts_with("__dyn_") { continue; }
            let gens = generic_decl(&s.type_params);
            let sfx = type_args(&s.type_params);
            let mut fmt = format!("{} {{{{ ", s.name);
            let mut args = String::new();
            for (i, (fname, _)) in s.fields.iter().enumerate() {
                if i > 0 {
                    fmt.push_str(", ");
                }
                write!(fmt, "{}: {{}}", fname).unwrap();
                write!(args, ", __b.{}.ray_show()", fname).unwrap();
            }
            fmt.push_str(" }}");
            writeln!(out, "impl{} RayShow for Rc<std::cell::RefCell<{}{}>> {{ fn ray_show(&self) -> String {{ let __b = self.borrow(); format!(\"{}\"{}) }} }}", gens, s.name, sfx, fmt, args).unwrap();
        }
        for e in &prog.enums {
            if e.name == "Option" || e.name == "Result" {
                continue;
            }
            let gens = generic_decl(&e.type_params);
            let sfx = type_args(&e.type_params);
            writeln!(out, "impl{} RayShow for Rc<{}{}> {{ fn ray_show(&self) -> String {{ match &**self {{", gens, e.name, sfx).unwrap();
            for v in &e.variants {
                if v.payload.is_empty() {
                    writeln!(out, "{}::{} => \"{}.{}\".to_string(),", e.name, v.name, e.name, v.name).unwrap();
                } else {
                    let binds: Vec<String> = (0..v.payload.len()).map(|i| format!("__p{}", i)).collect();
                    let mut fmt = format!("{}.{}(", e.name, v.name);
                    let mut args = String::new();
                    for (i, _) in v.payload.iter().enumerate() {
                        if i > 0 {
                            fmt.push_str(", ");
                        }
                        fmt.push_str("{}");
                        write!(args, ", {}.ray_show()", binds[i]).unwrap();
                    }
                    fmt.push(')');
                    writeln!(out, "{}::{}({}) => format!(\"{}\"{}),", e.name, v.name, binds.join(", "), fmt, args).unwrap();
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
                    write!(out, "{}::{}", enum_name, variant).unwrap();
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
                // to_string(x) / x.to_string() → inlina x como `{}` (su Display), sin Rc intermedio.
                ExprKind::Call { callee, args: cargs } if is_to_string(callee) => {
                    fmt.push_str("{}");
                    let (_, recv) = resolve_callee(callee)?;
                    args.push(recv.unwrap_or(&cargs[0]));
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
            _ => return Err(format!("spike: std::fs::{} no soportada", ffn)),
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
            "print" => {
                // Uniforme vía RayShow (maneja todo tipo, incl. structs/arreglos/genéricos).
                if matches!(self.type_of(eff[0])?, Type::Fn(_, _)) {
                    out.push_str("println!(\"<fn>\")"); // una función se muestra como <fn>
                } else {
                    out.push_str("println!(\"{}\", ");
                    self.emit_expr(out, eff[0])?;
                    out.push_str(".ray_show())");
                }
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
                    _ => out.push_str(".len() as i64)"),
                }
            }
            // push(a, v) → a.borrow_mut().push(v) (muta en el sitio, devuelve unit).
            "push" => {
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow_mut().push(");
                self.emit_expr(out, eff[1])?;
                out.push(')');
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
            "get_or" => {
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
            // I/O de ENTRADA (no determinista → sin oráculo; probado por subproceso, como tests/io_cli.rs).
            // `input() -> Option<string>`: una línea de stdin, sin '\n'/'\r' finales (como la VM); None en EOF.
            "input" => {
                out.push_str(
                    "{ let mut __s = String::new(); match std::io::stdin().read_line(&mut __s) \
                     { Ok(0) | Err(_) => None, Ok(_) => Some(Rc::<str>::from(__s.trim_end_matches(['\\n', '\\r']))) } }",
                );
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
            "close" => {
                self.needs_handles = true;
                out.push_str("__ray_close(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
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
            // Orden superior (prelude map/filter/fold) → iteradores de Rust. `__f` liga la closure una
            // vez; `__x`/`__acc` son los elementos/acumulador. (Anidados: cada uno en su propio bloque.)
            "map" => {
                out.push_str("{ let __f = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; Rc::new(std::cell::RefCell::new(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().map(|__x| __f(__x.clone())).collect::<Vec<_>>())) }");
            }
            "filter" => {
                out.push_str("{ let __f = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; Rc::new(std::cell::RefCell::new(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().cloned().filter(|__x| __f(__x.clone())).collect::<Vec<_>>())) }");
            }
            "fold" => {
                out.push_str("{ let __f = ");
                self.emit_expr(out, eff[2])?;
                out.push_str("; ");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().fold(");
                self.emit_expr(out, eff[1])?;
                out.push_str(", |__acc, __x| __f(__acc, __x.clone())) }");
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
                        "write_file" | "open" | "write" => Type::Enum("Result".into(), vec![Type::Int, Type::String]),
                        "read_line" => opt_of(Type::String),
                        "exists" => Type::Bool,
                        other => return Err(format!("spike: std::fs::{} no soportada", other)),
                    });
                }
                let _ = &args;
                let method = n.rsplit('#').next().unwrap_or(n).trim_start_matches("__");
                // Receptor efectivo (UFCS o primer argumento), para métodos cuyo tipo depende de él.
                let recv0 = recv.or_else(|| args.first());
                match method {
                    "to_string" | "join" => Type::String,
                    "show" if n.contains('#') => Type::String,
                    "eq" | "less" if n.contains('#') => Type::Bool,
                    "len" => Type::Int,
                    "parse_int" => opt_of(Type::Int),
                    "parse_float" => opt_of(Type::Float),
                    // I/O de entrada del prelude: input → Option<string>; read_int → Option<int>.
                    "input" => opt_of(Type::String),
                    "read_int" => opt_of(Type::Int),
                    "close" => Type::Int, // close(h) de un handle de archivo → 0 (ad-hoc; canal es concurrencia)
                    "print" | "push" | "insert" | "add_to" | "assert" | "assert_eq" | "panic" => Type::Unit,
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
                    // unwrap_or/unwrap desenvuelven un Option<T>/Result<T,E> → T.
                    "unwrap_or" | "unwrap" => {
                        unwrapped(&self.type_of(recv0.ok_or("spike: unwrap sin receptor")?)?)
                    }
                    // Orden superior: map(xs,f) → [ret(f)]; filter(xs,f) → [elem(xs)]; fold(xs,init,f) → ret(f).
                    "map" => match self.type_of(effargs(recv, args, 1)?)? {
                        Type::Fn(_, r) => Type::Array(r),
                        other => return Err(format!("spike: map con f no-función {:?}", other)),
                    },
                    "filter" => self.type_of(effargs(recv, args, 0)?)?,
                    "fold" => match self.type_of(effargs(recv, args, 2)?)? {
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
            ExprKind::Match { arms, .. } => {
                // El tipo del match = el de cualquier brazo. Se prueba hasta que uno resuelva: el cuerpo
                // del primero puede referir un binding del patrón (no en ámbito aquí); otro no.
                arms.iter()
                    .find_map(|a| self.type_of(&a.body).ok())
                    .ok_or("spike: no pude inferir el tipo del match")?
            }
            ExprKind::Cast { ty, .. } => normalize_type(ty),
            ExprKind::MapLit(pairs) => {
                let (k, v) = pairs.first().ok_or("spike: Map literal vacío sin anotación")?;
                Type::Map(Box::new(self.type_of(k)?), Box::new(self.type_of(v)?))
            }
            other => return Err(format!("spike: no sé inferir el tipo de {:?}", other)),
        })
    }
}

/// Normaliza los tipos que el parser deja como `Struct` genérico: `Map<K,V>` llega como
/// `Struct("Map",[K,V])` (el checker lo reclasifica en su tabla, no en la anotación del AST). Recursivo.
fn normalize_type(t: &Type) -> Type {
    match t {
        Type::Struct(n, args) if n == "Map" && args.len() == 2 => {
            Type::Map(Box::new(normalize_type(&args[0])), Box::new(normalize_type(&args[1])))
        }
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
        Type::String => "Rc<str>",
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
            return Ok(if enums.contains(n) {
                format!("Rc<{}{}>", n, sfx)
            } else {
                format!("Rc<std::cell::RefCell<{}{}>>", n, sfx)
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
            return Ok(format!("Rc<{}{}>", n, sfx));
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

/// Declaración de genéricos de Rust `<A: Clone + RayShow, …>` (fn/struct/enum: `Clone` para el clon-al-
/// leer, `RayShow` para mostrar/`to_string`). rustc los monomorfiza → nativo.
fn generic_decl(tparams: &[String]) -> String {
    generic_bound(tparams, "Clone + RayShow")
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
        _ => {}
    }
}

/// Sustituye las variables de tipo de `t` por sus ligaduras en `subst` (las no ligadas se dejan igual).
fn subst_type(t: &Type, subst: &HashMap<String, Type>) -> Type {
    match t {
        Type::Var(n) => subst.get(n).cloned().unwrap_or_else(|| t.clone()),
        Type::Struct(n, a) if a.is_empty() => subst.get(n).cloned().unwrap_or_else(|| t.clone()),
        Type::Array(e) => Type::Array(Box::new(subst_type(e, subst))),
        Type::Map(k, v) => {
            Type::Map(Box::new(subst_type(k, subst)), Box::new(subst_type(v, subst)))
        }
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
fn is_heap(t: &Type) -> bool {
    matches!(
        t,
        Type::String | Type::Bytes | Type::Array(_) | Type::Tuple(_) | Type::Map(_, _)
            | Type::Struct(_, _) | Type::Enum(_, _) | Type::Fn(_, _) | Type::Var(_) | Type::Dyn(_)
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
mod tests {
    use super::transpile;

    fn transpile_src(src: &str) -> String {
        let tokens = crate::lexer::lex(src).expect("lex");
        let mut prog = crate::parser::parse(tokens).expect("parse");
        crate::checker::check(&mut prog).expect("check");
        transpile(&prog).expect("transpile")
    }

    #[test]
    fn transpila_fib_recursivo() {
        let rust = transpile_src(
            "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }\n\
             fn main() { print(fib(10)); }",
        );
        assert!(rust.contains("fn fib(mut n: i64) -> i64"), "{}", rust);
        assert!(rust.contains("fib((n - 1i64))"), "{}", rust);
        assert!(rust.contains("fib(10i64).ray_show()"), "{}", rust);
    }

    #[test]
    fn transpila_bucle_for_rango() {
        let rust = transpile_src(
            "fn main() { var acc: int = 0; for i in 0..100 { acc = acc + i; } print(acc); }",
        );
        assert!(rust.contains("for i in 0i64..100i64"), "{}", rust);
        assert!(rust.contains("let mut acc: i64 = 0i64"), "{}", rust); // anotación emitida (pina inferencia)
    }

    #[test]
    fn transpila_strings_concat_y_clon() {
        let rust = transpile_src(
            "fn greet(name: string) -> string { \"hi \" + name }\n\
             fn main() -> int { let g = greet(\"bob\"); print(g); g.len() }",
        );
        // string → Rc<str>; concat via format!; el `g` heap se clona al leer.
        assert!(rust.contains("fn greet(mut name: Rc<str>) -> Rc<str>"), "{}", rust);
        assert!(rust.contains("Rc::<str>::from(format!"), "{}", rust);
        assert!(rust.contains("g.clone()"), "{}", rust);
    }

    #[test]
    fn transpila_arreglos_split_join() {
        let rust = transpile_src(
            "fn main() -> int { var xs: [string] = []; xs.push(\"a\"); \
             let parts = \"a,b,c\".split(\",\"); parts.join(\"-\").len() }",
        );
        assert!(rust.contains("Rc<std::cell::RefCell<Vec<Rc<str>>>>"), "{}", rust);
        assert!(rust.contains(".borrow_mut().push("), "{}", rust);
        assert!(rust.contains("__ray_split(&"), "{}", rust);
        assert!(rust.contains("__ray_join(&"), "{}", rust);
    }

    #[test]
    fn transpila_map_add_to_get() {
        let rust = transpile_src(
            "fn main() -> int { var m: Map<string, int> = Map.new(); m.add_to(\"a\", 1); \
             m.add_to(\"a\", 2); m.get(\"a\").unwrap_or(0) }",
        );
        assert!(rust.contains("HashMap"), "{}", rust);
        assert!(rust.contains(".entry("), "{}", rust);
        assert!(rust.contains(".or_insert(0i64) += "), "{}", rust);
        assert!(rust.contains(".get(&") && rust.contains(".unwrap_or("), "{}", rust);
    }

    #[test]
    fn transpila_struct_y_enum_match() {
        let rust = transpile_src(
            "struct P { x: int, y: int }\n\
             enum Shape { Circle(float), Dot }\n\
             fn area(s: Shape) -> float { match (s) { Shape.Circle(r) => r * r, Shape.Dot => 0.0 } }\n\
             fn main() -> int { let p = P { x: 1, y: 2 }; print(area(Shape.Circle(2.0))); p.x }",
        );
        assert!(rust.contains("struct P {"), "{}", rust);
        assert!(rust.contains("enum Shape {"), "{}", rust);
        assert!(rust.contains("Rc::new(std::cell::RefCell::new(P {"), "{}", rust);
        assert!(rust.contains("Rc::new(Shape::Circle(2.0f64))"), "{}", rust);
        assert!(rust.contains("match &*") && rust.contains("Shape::Circle(r)"), "{}", rust);
        assert!(rust.contains("RayShow for Rc<std::cell::RefCell<P>>"), "{}", rust);
    }

    #[test]
    fn transpila_option_match_y_try() {
        let rust = transpile_src(
            "fn f(s: string) -> Option<int> { let n = parse_int(s)?; Option.Some(n + 1) }\n\
             fn main() -> int { match (f(\"7\")) { Option.Some(v) => v, Option.None => 0 } }",
        );
        // Option → nativo de Rust: firma Option<i64>, Some(...), `?`, y match con Some/None (sin Rc).
        assert!(rust.contains("-> Option<i64>"), "{}", rust);
        assert!(rust.contains(".parse::<i64>().ok())?"), "{}", rust);
        assert!(rust.contains("Some(") && rust.contains("None"), "{}", rust);
        assert!(rust.contains("match &") && !rust.contains("Option::"), "{}", rust);
    }

    #[test]
    fn transpila_closures_y_map() {
        let rust = transpile_src(
            "fn apply(f: fn(int) -> int, x: int) -> int { f(x) }\n\
             fn main() -> int { \
               let sq: fn(int) -> int = fn(n: int) -> int { n * n }; \
               let xs = [1, 2, 3]; let ys = xs.map(fn(x: int) -> int { x + 1 }); \
               apply(sq, 4) + ys.len() }",
        );
        assert!(rust.contains("Rc<dyn Fn(i64) -> i64>"), "{}", rust); // función-valor
        assert!(rust.contains("Rc::new(move |n: i64| -> i64"), "{}", rust); // anónima → closure move
        assert!(rust.contains(".iter().map(|__x| __f(__x.clone()))"), "{}", rust); // map → iterador
    }

    #[test]
    fn transpila_funciones_genericas() {
        let rust = transpile_src(
            "fn id<T>(x: T) -> T { x }\n\
             fn apply<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }\n\
             fn neg(b: bool) -> bool { !b }\n\
             fn main() -> int { let a: int = id(5); print(apply(neg, false)); a }",
        );
        assert!(rust.contains("fn id<T: Clone + RayShow>(mut x: T) -> T"), "{}", rust);
        assert!(rust.contains("fn apply<T:") && rust.contains("U:"), "{}", rust);
        assert!(rust.contains("apply(Rc::new(neg)"), "{}", rust); // función como valor → Rc::new(fn)
    }

    #[test]
    fn transpila_tipos_genericos() {
        let rust = transpile_src(
            "struct Par<A, B> { a: A, b: B }\n\
             enum Caja<T> { Llena(T), Vacia }\n\
             fn saca(c: Caja<int>) -> int { match (c) { Caja.Llena(v) => v, Caja.Vacia => 0 } }\n\
             fn main() -> int { let p = Par { a: 1, b: true }; saca(Caja.Llena(9)) }",
        );
        assert!(rust.contains("struct Par<A: Clone"), "{}", rust);
        assert!(rust.contains("enum Caja<T: Clone"), "{}", rust);
        assert!(rust.contains("Caja::Llena(v)"), "{}", rust); // match de enum genérico
        assert!(rust.contains("Rc::new(Caja::Llena(9i64))"), "{}", rust);
    }

    #[test]
    fn transpila_traits_despacho_estatico() {
        let rust = transpile_src(
            "trait Valor { fn valor(self) -> int; }\n\
             struct P { x: int }\n\
             impl Valor for P { fn valor(self) -> int { self.x } }\n\
             fn main() -> int { let p = P { x: 7 }; p.valor() }",
        );
        // El método de trait se baja a una función manglada `P#valor` → `P_HH_valor` (erasure, M9).
        assert!(rust.contains("fn P_HH_valor(mut __self: Rc<std::cell::RefCell<P>>) -> i64"), "{}", rust);
        assert!(rust.contains("P_HH_valor("), "{}", rust);
        // Trait propio RayShow para el `Show` de raylang (Display no sirve con Rc<RefCell>).
        assert!(rust.contains("trait RayShow"), "{}", rust);
    }

    #[test]
    fn transpila_tuplas() {
        let rust = transpile_src(
            "fn divmod(a: int, b: int) -> (int, int) { (a / b, a % b) }\n\
             fn main() -> int { let d = divmod(17, 5); let (q, r) = divmod(23, 4); d.0 + q + r }",
        );
        assert!(rust.contains("-> (i64, i64,)"), "{}", rust); // tupla nativa de Rust
        assert!(rust.contains("let (q, r) = "), "{}", rust); // desestructuración
        assert!(rust.contains(".0"), "{}", rust); // acceso por índice de tupla
    }

    #[test]
    fn transpila_trait_objects_dyn() {
        let rust = transpile_src(
            "trait Figura { fn area(self) -> int; }\n\
             struct Cuad { lado: int }\n\
             impl Figura for Cuad { fn area(self) -> int { self.lado * self.lado } }\n\
             fn total(f: dyn Figura) -> int { f.area() }\n\
             fn main() -> int { total(Cuad { lado: 3 }) }",
        );
        // dyn → struct de closures que capturan el concreto (sin Box<dyn Any>, sin data).
        assert!(rust.contains("struct __dyn_Figura"), "{}", rust);
        assert!(rust.contains("area: Rc<dyn Fn() -> i64>"), "{}", rust);
        assert!(rust.contains("let __c = "), "{}", rust); // captura del concreto en la coerción
        assert!(rust.contains(".borrow().area.clone())"), "{}", rust); // despacho dinámico
    }

    #[test]
    fn transpila_std_math() {
        // `import std/math` necesita el loader; cargamos el ejemplo real y comprobamos el mapeo a los
        // métodos de `f64` de Rust (misma impl que la VM → mismo resultado; verificado byte a byte aparte).
        let loaded = match crate::loader::load(std::path::Path::new("examples/basics/matematicas.ray")) {
            Ok(l) => l,
            Err(_) => panic!("no se pudo cargar matematicas.ray"),
        };
        let mut prog = loaded.program;
        crate::checker::check(&mut prog).expect("check");
        let rust = transpile(&prog).expect("transpile");
        assert!(rust.contains(").sqrt()"), "{}", rust);
        assert!(rust.contains(").powf("), "{}", rust);
        assert!(rust.contains(").floor()"), "{}", rust);
        assert!(rust.contains(").abs()"), "{}", rust); // ad-hoc int|float
        assert!(rust.contains(").min("), "{}", rust);
        assert!(rust.contains("std::f64::consts::PI"), "{}", rust);
        assert!(rust.contains("std::f64::consts::E"), "{}", rust);
        // No debe emitir los wrappers del módulo (`fn ...sqrt`) ni el primitivo `__sqrt`.
        assert!(!rust.contains("__sqrt"), "{}", rust);
    }

    #[test]
    fn transpila_args() {
        // args() → arreglo de string (argv tras el binario); a[i] indexa, a.len() cuenta.
        let rust = transpile_src(
            "fn main() -> int { let a = args(); \
             if (a.len() > 0) { a[0].len() } else { 0 } }",
        );
        assert!(rust.contains("std::env::args().skip(1)"), "{}", rust);
        assert!(rust.contains("Rc::<str>::from(__a)"), "{}", rust);
        // el arreglo se indexa/mide como cualquier `[string]` (borrow).
        assert!(rust.contains(".borrow().len() as i64"), "{}", rust);
    }

    #[test]
    fn transpila_operator_overloading_y_show_custom() {
        // `a + b` con `impl Add for Vec2` → llamada al método (`Vec2#add`); un `impl Show` CUSTOM se
        // respeta en `.show()` (llama a `Vec2#show`), mientras `print(x)` usaría el render default (RayShow).
        let rust = transpile_src(
            "struct Vec2 { x: int, y: int }\n\
             impl Add for Vec2 { fn add(self, o: Vec2) -> Vec2 { Vec2 { x: self.x + o.x, y: self.y + o.y } } }\n\
             impl Show for Vec2 { fn show(self) -> string { \"(${self.x}, ${self.y})\" } }\n\
             fn main() { let a = Vec2 { x: 1, y: 2 }; let b = Vec2 { x: 3, y: 4 }; print((a + b).show()); }",
        );
        assert!(rust.contains("Vec2_HH_add"), "operator+ → método: {}", rust); // suma vía impl Add
        assert!(rust.contains("Vec2_HH_show"), "impl Show custom emitido y llamado: {}", rust);
        // `.show()` NO debe mapearse a `.ray_show()` (eso daría el render default `Vec2 { x, y }`).
        assert!(rust.contains("fn Vec2_HH_show"), "el impl Show se emite: {}", rust);
    }

    #[test]
    fn transpila_enteros_con_tamano() {
        // u8/u32/u64 → nativos de Rust; literal tipado por contexto (200u8, elementos de [u8]); aritmética
        // envolvente (wrapping_*) entre valores sized para no chocar con el deny de overflow constante de
        // Rust; cast `as uN`/`as int`. Aritmética con vars sized (no literales) → dispara wrapping.
        let rust = transpile_src(
            "fn fnv(data: [u8]) -> u32 { var h: u32 = 2166136261; let p: u32 = 16777619; \
             for b in data { h = (h ^ b as u32) * p; } h }\n\
             fn main() -> int { let a: u8 = 200; let b: u8 = 100; let d: [u8] = [104, 105]; \
             (a + b) as int + fnv(d) as int }",
        );
        // El checker coacciona los literales con un Cast explícito → `(200i64 as u8)`; mi Cast lo emite.
        assert!(rust.contains("let a: u8 = (200i64 as u8)"), "u8 anotado + literal coaccionado: {}", rust);
        assert!(rust.contains("(104i64 as u8)"), "elemento de [u8] coaccionado: {}", rust);
        assert!(rust.contains(".wrapping_add("), "suma envolvente u8: {}", rust);
        assert!(rust.contains(".wrapping_mul("), "mult envolvente u32: {}", rust);
        assert!(rust.contains(" as u32)"), "cast a u32: {}", rust);
        assert!(rust.contains("as i64"), "cast a int: {}", rust);
    }

    #[test]
    fn transpila_from_conversion() {
        // `?` con From-conversion: el checker baja a un match con temps `$to`/`$te` y una llamada a la
        // conversión `AppError#from#string`. Verificamos que los `$` se manglan y la conversión se emite.
        let rust = transpile_src(
            "enum AppError { Lectura(string), Vacio }\n\
             impl From<string> for AppError { fn convert(o: string) -> AppError { AppError.Lectura(o) } }\n\
             fn leer(ok: bool) -> Result<int, string> { if (ok) { Result.Ok(1) } else { Result.Err(\"x\") } }\n\
             fn cargar(ok: bool) -> Result<int, AppError> { let v = leer(ok)?; Result.Ok(v) }\n\
             fn main() -> int { match (cargar(true)) { Result.Ok(v) => v, Result.Err(_) => 0 } }",
        );
        assert!(rust.contains("AppError_HH_from_HH_string"), "{}", rust); // la conversión se emite
        assert!(!rust.contains('$'), "los temps $ deben manglarse: {}", rust);
    }

    #[test]
    fn transpila_derive_eq() {
        // @derive(Eq) + bound T: Eq → paso de diccionarios (como los traits de usuario): el impl derivado
        // `Tipo#eq` se emite como función ordinaria, y `x.eq(y)` con x: T acotado llama al dict param.
        let rust = transpile_src(
            "@derive(Eq)\nstruct Punto { x: int, y: int }\n\
             fn iguales<T: Eq>(a: T, b: T) -> bool { a.eq(b) }\n\
             fn main() -> int { if (iguales(Punto { x: 1, y: 2 }, Punto { x: 1, y: 2 })) { 1 } else { 0 } }",
        );
        // el impl derivado se emite manglado (Punto#eq → Punto_HH_eq), no se salta.
        assert!(rust.contains("Punto_HH_eq"), "{}", rust);
        // la función acotada conserva el param-diccionario y NO se mapea a `==`.
        assert!(rust.contains("T_HH_Eq_HH_eq"), "{}", rust);
        assert!(!rust.contains("a.clone() == b.clone()"), "no debe mapear .eq() a ==: {}", rust);
    }

    #[test]
    fn transpila_for_sobre_map() {
        // for (k, v) in map → pares ordenados por clave (helper __ray_pairs), determinista como la VM.
        let rust = transpile_src(
            "fn main() { var m: Map<string, int> = Map.new(); m.insert(\"a\", 1); \
             for (k, v) in m { print(k + \": \" + to_string(v)); } }",
        );
        assert!(rust.contains("fn __ray_pairs<"), "{}", rust);
        assert!(rust.contains("for (k, v) in __ray_pairs(&"), "{}", rust);
    }

    #[test]
    fn rechaza_fuera_del_subconjunto() {
        // un `main` con `env()` (I/O de entorno, aún fuera del subconjunto) → sin `main` transpilable.
        let tokens = crate::lexer::lex("fn main() { let e = env(\"PATH\"); print(e); }").unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        assert!(super::transpile(&prog).is_err());
    }

    #[test]
    fn transpila_io_de_entrada() {
        // input()/read_int() (prelude) → stdin; read_file/write_file/exists (std/fs, cualificados) → std::fs.
        let rust = transpile_src(
            "fn main() -> int { \
             match (input()) { Option.Some(l) => print(l), Option.None => print(\"eof\") } \
             match (read_int()) { Option.Some(n) => n, Option.None => 0 } }",
        );
        assert!(rust.contains("std::io::stdin().read_line("), "input/read_int leen stdin: {}", rust);
        assert!(rust.contains("trim_end_matches"), "quita el salto de línea como la VM: {}", rust);
    }
}
