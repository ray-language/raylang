//! Emisión de funciones/bloques/sentencias/expresiones/match (movimiento puro; usar
//! `git log --follow`).
//!
//! El grueso del backend: bajar cada forma del AST a texto Rust. `emit_call`/`type_of`
//! (llamadas + inferencia de tipos para decidir codegen) y el envío entre hilos (`emit_send_*`,
//! H21-N5) viven en `calls.rs`; el runtime embebido en `runtime.rs`.

use super::*;

impl Transpiler {
    /// Registra las celdas de `body` (var capturadas por closures) en `self.cells`, devolviendo las que
    /// AÑADIÓ (para quitarlas al salir del ámbito). Un nombre ya presente por un ámbito externo no se
    /// duplica ni se quita aquí.
    pub(super) fn enter_cells(&mut self, body: &Block) -> Vec<String> {
        let mut added = Vec::new();
        for n in cell_vars(body) {
            if self.cells.insert(n.clone()) {
                added.push(n);
            }
        }
        added
    }

    pub(super) fn exit_cells(&mut self, added: Vec<String>) {
        for n in added {
            self.cells.remove(&n);
        }
    }

    pub(super) fn emit_function(&mut self, out: &mut String, rust_name: &str, f: &Function) -> Result<(), String> {
        // Un cuerpo que NO transpila (p. ej. usa `try_join`) propaga `Err` con `?` sin haber popeado los
        // scopes ni deshecho las cells que ya declaró → sus locales (p. ej. un `Task t`) se FILTRABAN al
        // siguiente `emit_function`, cuyo `spawn` capturaba ese `t` fantasma (`in_scope_channels`) y emitía
        // `let t = t.clone()` con `t` inexistente en Rust. Se restaura el estado en TODOS los caminos.
        let base_scopes = self.scopes.len();
        let prev_tparams = std::mem::take(&mut self.tparams);
        let prev_cells = std::mem::take(&mut self.cells);
        let prev_marked = std::mem::take(&mut self.send_fn_params);
        let r = self.emit_function_inner(out, rust_name, f);
        self.scopes.truncate(base_scopes);
        self.tparams = prev_tparams;
        self.cells = prev_cells;
        self.send_fn_params = prev_marked;
        r
    }

    /// H21-N5c: genéricos extra + tipos de param para los params fn MARCADOS de `f` (cruzan un
    /// spawn). Devuelve (declaración de genéricos combinada, tipo por param con los marcados como
    /// `__F{i}`) y deja los nombres marcados en `self.send_fn_params`.
    pub(super) fn fn_generics(&mut self, f: &Function) -> Result<(String, Vec<String>), String> {
        let marked = self.fn_marks.get(&f.name).cloned().unwrap_or_default();
        let mut gens: Vec<String> = f
            .type_params
            .iter()
            .map(|t| format!("{}: Clone + RayShow + 'static", t))
            .collect();
        let mut ptys = Vec::new();
        self.send_fn_params.clear();
        for (i, p) in f.params.iter().enumerate() {
            if marked.contains(&i) {
                let Type::Fn(ats, rt) = normalize_type(&p.ty) else {
                    return Err(format!("marked param {} of '{}' is not a fn type", i, f.name));
                };
                let mut sig = Vec::new();
                for at in &ats {
                    sig.push(rust_ty(at, &self.enums, &self.tparams)?);
                }
                gens.push(format!(
                    "__F{i}: Fn({}) -> {} + Send + Sync + Clone + 'static",
                    sig.join(", "),
                    rust_ty(&rt, &self.enums, &self.tparams)?
                ));
                ptys.push(format!("__F{i}"));
                self.send_fn_params.insert(p.name.clone());
            } else {
                ptys.push(rust_ty(&p.ty, &self.enums, &self.tparams)?);
            }
        }
        let gdecl = if gens.is_empty() { String::new() } else { format!("<{}>", gens.join(", ")) };
        Ok((gdecl, ptys))
    }

    pub(super) fn emit_function_inner(&mut self, out: &mut String, rust_name: &str, f: &Function) -> Result<(), String> {
        // Params de tipo en ámbito (para `rust_ty` y la clasificación de `Struct(T)`→genérico).
        self.tparams = f.type_params.iter().cloned().collect();
        // Genéricos de Rust con bound `Clone` (todo valor genérico se clona al leer) + `RayShow` (por si
        // se imprime/`to_string`). rustc los monomorfiza → nativo. Los bounds de raylang (Eq/Ord/traits de
        // usuario) los realiza el **paso de diccionarios** del checker: sus params ocultos (`T#Trait#m`,
        // valores función) y el impl manglado (`Tipo#m`) se emiten tal cual (como funciones ordinarias).
        let (generics, ptys) = self.fn_generics(f)?;
        self.scopes.push(HashMap::new());
        let mut params = Vec::new();
        for (p, pty) in f.params.iter().zip(&ptys) {
            params.push(format!("mut {}: {}", mangle(&p.name), pty));
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
    pub(super) fn emit_stub(&mut self, out: &mut String, rust_name: &str, f: &Function) -> Result<(), String> {
        self.tparams = f.type_params.iter().cloned().collect();
        let (generics, ptys) = self.fn_generics(f)?;
        self.send_fn_params.clear(); // el stub no emite cuerpo: no hay capturas que mirar
        let mut params = Vec::new();
        for (p, pty) in f.params.iter().zip(&ptys) {
            params.push(format!("mut {}: {}", mangle(&p.name), pty));
        }
        let ret = rust_ty(&f.return_type, &self.enums, &self.tparams)?;
        write!(
            out,
            "fn {}{}({}) -> {} {{ __ray_rt_err(\"'{}' is not supported in the native binary (Rust transpilation)\") }}\n",
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
    pub(super) fn emit_externs(&self, out: &mut String, prog: &Program) -> Result<(), String> {
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
            // `{:?}` produce un literal de string Rust ESCAPADO: un nombre de librería con comillas no puede
            // inyectar ítems en el fuente generado (no es frontera de seguridad —el usuario compila su
            // propio código— pero evita que un `extern "..."` raro genere Rust arbitrario).
            if *lib != "c" {
                writeln!(out, "#[link(name = {:?})]", lib).unwrap();
            }
            out.push_str("unsafe extern \"C\" {\n");
            for e in fns {
                let cargs: Vec<String> = e
                    .params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| ffi_c_arg_ty(&p.ty).map(|c| format!("__a{}: {}", i, c)))
                    .collect::<Result<_, _>>()?;
                writeln!(out, "    #[link_name = {:?}]", e.name).unwrap(); // escapado (ver #[link] arriba)
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
                        // mismo texto que la VM (src/ffi.rs; allí es error de ejecución, aquí panic).
                        writeln!(out, "    let __rt_c{} = std::ffi::CString::new(&*__p{} as &str).expect(\"the string argument of '{}' contains an interior NUL\");", i, i, e.name).unwrap();
                        passes.push(format!("__rt_c{}.as_ptr()", i));
                    }
                    Type::Bytes => passes.push(format!("__p{}.as_ptr()", i)),
                    Type::Ptr => passes.push(format!("(__p{} as *mut std::ffi::c_void)", i)),
                    other => return Err(format!("FFI argument is not marshalable: {:?}", other)),
                }
            }
            writeln!(out, "    let __rt_r = unsafe {{ __ffi_{}({}) }};", mangle(&e.name), passes.join(", ")).unwrap();
            // Marshalling del retorno C → valor raylang.
            let ret_expr = match normalize_type(&e.return_type) {
                // `__rt_r` es `c_int` (i32) para Int → extiende el signo a i64 (como la VM).
                Type::Int => "__rt_r as i64".to_string(),
                Type::Float => "__rt_r".to_string(),
                Type::Bool => "__rt_r != 0".to_string(),
                Type::Unit => "()".to_string(),
                Type::Ptr => "__rt_r as i64".to_string(),
                Type::Enum(n, args) if n == "Option" && args.len() == 1 => match normalize_type(&args[0]) {
                    // char* → Option<bytes>: NULL→None; si no, copia los bytes hasta el NUL (nunca libera).
                    Type::Bytes => "if __rt_r.is_null() { None } else { Some(Rc::<[u8]>::from(unsafe { std::ffi::CStr::from_ptr(__rt_r) }.to_bytes())) }".to_string(),
                    // char* → Option<string>: como bytes, validando UTF-8. Mismo texto que la VM
                    // (src/interpreter.rs, ffi_to_value; allí es error de ejecución, aquí panic).
                    Type::String => "if __rt_r.is_null() { None } else { Some(Rc::<str>::from(std::str::from_utf8(unsafe { std::ffi::CStr::from_ptr(__rt_r) }.to_bytes()).expect(\"the C function returned bytes that are not valid UTF-8 (declare Option<bytes> to receive them raw)\"))) }".to_string(),
                    // ptr fallible → Option<ptr>: NULL→None; si no, la dirección opaca.
                    Type::Ptr => "if __rt_r.is_null() { None } else { Some(__rt_r as i64) }".to_string(),
                    other => return Err(format!("FFI return type Option<{:?}> is not supported", other)),
                },
                other => return Err(format!("FFI return type is not marshalable: {:?}", other)),
            };
            writeln!(out, "    {}\n}}", ret_expr).unwrap();
        }
        Ok(())
    }

    /// Normaliza + reclasifica RECURSIVAMENTE los `Struct(n)` que son enums del usuario (el parser
    /// no distingue; `declare` solo reclasifica el nivel superior). Lo usan los conversores Send y
    /// `type_of` de join/try_join, donde el tipo anida (Task<Forma>, campos, payloads).
    pub(super) fn classify(&self, t: &Type) -> Type {
        match normalize_type(t) {
            Type::Struct(n, a) => {
                let a: Vec<Type> = a.iter().map(|x| self.classify(x)).collect();
                if self.enums.contains(&n) { Type::Enum(n, a) } else { Type::Struct(n, a) }
            }
            Type::Enum(n, a) => Type::Enum(n, a.iter().map(|x| self.classify(x)).collect()),
            Type::Array(e) => Type::Array(Box::new(self.classify(&e))),
            Type::Map(k, v) => Type::Map(Box::new(self.classify(&k)), Box::new(self.classify(&v))),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|x| self.classify(x)).collect()),
            Type::Task(e) => Type::Task(Box::new(self.classify(&e))),
            Type::Channel(e) => Type::Channel(Box::new(self.classify(&e))),
            other => other,
        }
    }

    pub(super) fn declare(&mut self, name: &str, ty: Type) {
        let t = normalize_type(&ty);
        // Un `Struct(n)` cuyo `n` es un enum del usuario → `Enum(n)` (el parser no distingue; el checker
        // lo hace en su tabla). Así el entorno lleva el tipo correcto para el dispatch de `match`/campos.
        let t = match &t {
            Type::Struct(n, a) if self.enums.contains(n) => Type::Enum(n.clone(), a.clone()),
            _ => t,
        };
        self.scopes.last_mut().unwrap().insert(name.to_string(), t);
    }

    pub(super) fn lookup(&self, name: &str) -> Option<&Type> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    pub(super) fn in_scope_channels(&self) -> Vec<String> {
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

    pub(super) fn emit_block(&mut self, out: &mut String, b: &Block) -> Result<(), String> {
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

    pub(super) fn emit_stmt(&mut self, out: &mut String, s: &Stmt) -> Result<(), String> {
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
                    other => return Err(format!("tuple let over {:?} is not supported", other)),
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
                            out.push_str("{ let __rt_v = ");
                            self.emit_typed(out, value, &tty)?;
                            write!(out, "; *{}.borrow_mut() = __rt_v; }}\n", mangle(name)).unwrap();
                        } else {
                            out.push_str(&mangle(name));
                            out.push_str(" = ");
                            self.emit_typed(out, value, &tty)?; // sized: pina el tipo del literal en el RHS
                            out.push_str(";\n");
                        }
                    }
                    ExprKind::Index { array, index } => {
                        // Orden de la VM (compiler.rs: SetIndex consume arreglo, índice, valor en ese
                        // orden): izquierda→derecha. Los TRES van a temporales ANTES del borrow_mut: el
                        // índice o el valor pueden leer el MISMO arreglo (`a[a.len()-1] = a[0]`) →
                        // izarlos evita el doble borrow del RefCell (leer + mutar a la vez = panic).
                        out.push_str("{ let __rt_arr = ");
                        self.emit_expr(out, array)?;
                        out.push_str("; let __rt_idx = ");
                        self.emit_expr(out, index)?;
                        out.push_str("; let __rt_rhs = ");
                        self.emit_expr(out, value)?;
                        out.push_str("; __rt_arr.borrow_mut()[__rt_idx as usize] = __rt_rhs; }\n");
                    }
                    ExprKind::Field { object, name } => {
                        // Orden de la VM (SetField consume objeto, valor): objeto ANTES que el valor.
                        // Ambos a temporales antes del borrow_mut (el RHS puede leer el mismo campo,
                        // `p.x = p.x + 1`) → evita el doble borrow.
                        out.push_str("{ let __rt_obj = ");
                        self.emit_expr(out, object)?;
                        out.push_str("; let __rt_rhs = ");
                        self.emit_expr(out, value)?;
                        write!(out, "; __rt_obj.borrow_mut().{} = __rt_rhs; }}\n", mangle(name)).unwrap();
                    }
                    _ => return Err("unsupported lvalue".into()),
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
                        .ok_or_else(|| format!("iterator without a next method '{}'", next_fn))?;
                    let it_ty = self.type_of(expr)?;
                    let mut subst = HashMap::new();
                    if let Some(p0) = sig.params.first() {
                        unify(p0, &it_ty, &sig.tparams, &mut subst);
                    }
                    let elems = match subst_type(&normalize_type(&sig.ret), &subst) {
                        Type::Enum(n, args) if n == "Option" && args.len() == 1 => match &args[0] {
                            Type::Tuple(ts) if ts.len() == names.len() => ts.clone(),
                            other => return Err(format!("tuple for over a next that yields {:?}", other)),
                        },
                        other => return Err(format!("next of '{}' does not yield Option<tuple> ({:?})", next_fn, other)),
                    };
                    let binder = |n: &Option<String>| n.clone().map(|x| mangle(&x)).unwrap_or_else(|| "_".into());
                    let binders: Vec<String> = names.iter().map(binder).collect();
                    out.push_str("{ let __rt_it = ");
                    self.emit_expr(out, expr)?;
                    write!(out, "; loop {{ match {}(__rt_it.clone()) {{ Some((", mangle(next_fn)).unwrap();
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
                        _ => return Err("tuple for is only supported over a Map".into()),
                    };
                    let (kt, vt) = match self.type_of(expr)? {
                        Type::Map(k, v) => ((*k).clone(), (*v).clone()),
                        other => return Err(format!("for (k, v) over {:?} is not supported", other)),
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
                            other => return Err(format!("for over {:?} is not supported", other)),
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
                    // ligando cada `Some(x)`. El iterador se liga a `__rt_it` UNA vez; `next` recibe un clon del
                    // Rc → su estado (campos mutados por referencia) persiste entre iteraciones.
                    ForIter::Iter { expr, next_fn } => {
                        // T del elemento = el `T` de `Option<T>` de la firma de `next`, tras unificar el tipo
                        // real del iterador con su `self` (para adaptadores genéricos como `ArrayIter<int>`).
                        let sig = self
                            .funcs
                            .get(next_fn)
                            .ok_or_else(|| format!("iterator without a next method '{}'", next_fn))?;
                        let it_ty = self.type_of(expr)?;
                        let mut subst = HashMap::new();
                        if let Some(p0) = sig.params.first() {
                            unify(p0, &it_ty, &sig.tparams, &mut subst);
                        }
                        let elem = match subst_type(&normalize_type(&sig.ret), &subst) {
                            Type::Enum(n, args) if n == "Option" && args.len() == 1 => args[0].clone(),
                            other => return Err(format!("next of '{}' does not return Option<T> ({:?})", next_fn, other)),
                        };
                        out.push_str("{ let __rt_it = ");
                        self.emit_expr(out, expr)?;
                        write!(out, "; loop {{ match {}(__rt_it.clone()) {{ Some(", mangle(next_fn)).unwrap();
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
    pub(super) fn emit_typed(&mut self, out: &mut String, e: &Expr, expected: &Type) -> Result<(), String> {
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

    /// Emite un literal de función. `boxed` = envuelto en `Rc::new(...)` (la repr ordinaria de un
    /// valor-función); `false` (H21-N5c) = el closure PELADO, para un argumento a un param MARCADO
    /// (genérico `__F: Fn + Send + ...`) — si sus capturas no son Send, rustc lo rechaza (honesto).
    pub(super) fn emit_func_literal(&mut self, out: &mut String, fnexpr: &crate::ast::FnExpr, boxed: bool) -> Result<(), String> {
        // Celdas que ESTA closure captura (var-celda en ámbito referenciadas en su cuerpo): se
        // PRE-CLONAN antes del `move` (`{ let c = c.clone(); Rc::new(move || …) }`), para que el
        // ámbito exterior pueda seguir usándolas y la mutación se comparta (M4).
        let mut refd = std::collections::HashSet::new();
        idents_of_block(&fnexpr.body, &mut refd);
        let mut captured: Vec<String> = self.cells.iter().filter(|c| refd.contains(*c)).cloned().collect();
        // Toda captura de HEAP (no solo las celdas) se PRE-CLONA antes del `move`: dos closures
        // hermanos que capturan la misma local no-Copy (p. ej. los dos de `cors(origin)`) movían
        // la primera y el segundo veía E0382. Clonar un Rc comparte el valor inmutable — misma
        // semántica que el move (las capturas mutables ya son celdas).
        if boxed {
            let pnames: std::collections::HashSet<&str> =
                fnexpr.params.iter().map(|p| p.name.as_str()).collect();
            let mut extra: Vec<String> = refd
                .iter()
                .filter(|n| !pnames.contains(n.as_str()) && !captured.contains(n))
                .filter(|n| {
                    self.lookup(n).is_some_and(|ty| {
                        !matches!(
                            normalize_type(ty),
                            Type::Int | Type::Float | Type::Bool | Type::Char | Type::UInt(_) | Type::Unit
                        )
                    })
                })
                .cloned()
                .collect();
            extra.sort();
            captured.extend(extra);
        }
        // H21-N5c: una closure NO-boxed va a un param marcado (genérico Send) → cruzará hilos. Sus
        // capturas de heap se convierten a __RaySend FUERA y se reconstruyen DENTRO en cada llamada
        // (`.clone()` del árbol: la closure es Fn, no FnOnce) — deep copy, semántica M38. Los
        // canales/Tasks y los params fn marcados se pre-clonan (son Send de por sí).
        let mut send_caps: Vec<(String, Type, bool)> = Vec::new();
        if !boxed {
            let (caps, clones) = self.spawn_captures(&fnexpr.body)?;
            send_caps = caps;
            out.push_str("{ ");
            for n in self.in_scope_channels() {
                if refd.contains(&n) {
                    write!(out, "let {n} = {n}.clone(); ", n = mangle(&n)).unwrap();
                }
            }
            for n in &clones {
                write!(out, "let {n} = {n}.clone(); ", n = mangle(n)).unwrap();
            }
            for (i, (name, ty, is_cell)) in send_caps.iter().enumerate() {
                let src = if *is_cell {
                    format!("{}.borrow().clone()", mangle(name))
                } else {
                    format!("{}.clone()", mangle(name))
                };
                let conv = self.to_send_expr(ty, &src)?;
                write!(out, "let __snd_{i} = {conv}; ").unwrap();
            }
        }
        let wrap = !captured.is_empty();
        if wrap {
            out.push_str("{ ");
            for c in &captured {
                write!(out, "let {} = {}.clone(); ", mangle(c), mangle(c)).unwrap();
            }
        }
        if boxed {
            out.push_str("Rc::new(");
        }
        out.push_str("move |");
        for (i, p) in fnexpr.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            // el param de la closure se mangla (igual que su uso en el cuerpo vía `mangle`): puede
            // ser palabra reservada de Rust.
            write!(out, "{}: {}", mangle(&p.name), rust_ty(&p.ty, &self.enums, &self.tparams)?).unwrap();
        }
        write!(out, "| -> {} ", rust_ty(&fnexpr.return_type, &self.enums, &self.tparams)?).unwrap();
        self.scopes.push(HashMap::new());
        for p in &fnexpr.params {
            self.declare(&p.name, p.ty.clone());
        }
        if !send_caps.is_empty() {
            out.push_str("{ ");
            for (i, (name, ty, is_cell)) in send_caps.iter().enumerate() {
                let conv = self.from_send_expr(ty, &format!("__snd_{i}.clone()"))?;
                if *is_cell {
                    write!(out, "let {} = Rc::new(std::cell::RefCell::new({conv})); ", mangle(name)).unwrap();
                } else {
                    write!(out, "let {} = {conv}; ", mangle(name)).unwrap();
                }
            }
        }
        // Las celdas propias de esta closure (una var suya capturada por una closure aún más interna).
        let added = self.enter_cells(&fnexpr.body);
        self.emit_block(out, &fnexpr.body)?;
        self.exit_cells(added);
        self.scopes.pop();
        if !send_caps.is_empty() {
            out.push_str(" }");
        }
        if boxed {
            out.push(')');
        }
        if wrap {
            out.push_str(" }");
        }
        if !boxed {
            out.push_str(" }");
        }
        Ok(())
    }

    pub(super) fn emit_expr(&mut self, out: &mut String, e: &Expr) -> Result<(), String> {
        match &e.kind {
            ExprKind::Int(n) => write!(out, "{}i64", n).unwrap(),
            ExprKind::Float(x) => {
                // `{:?}` de un f64 no finito da `inf`/`-inf`/`NaN` → `inff64` es Rust INVÁLIDO. Un literal
                // como `1e999` parsea a infinito: se emite la constante de Rust correspondiente.
                if x.is_nan() {
                    out.push_str("f64::NAN");
                } else if x.is_infinite() {
                    out.push_str(if *x < 0.0 { "f64::NEG_INFINITY" } else { "f64::INFINITY" });
                } else {
                    write!(out, "{:?}f64", x).unwrap();
                }
            }
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
                    _ => return Err(format!("cast {:?} -> {:?} is not supported", src, target)),
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
                // H6: `-x` sobre int es checked (`-i64::MIN` desborda), como la VM.
                if matches!(op, UnaryOp::Neg) && matches!(self.type_of(expr)?, Type::Int) {
                    out.push_str("__ray_neg(");
                    self.emit_expr(out, expr)?;
                    out.push(')');
                } else {
                    out.push('(');
                    out.push_str(match op { UnaryOp::Neg => "-", UnaryOp::Not => "!", UnaryOp::BitNot => "!" });
                    self.emit_expr(out, expr)?;
                    out.push(')');
                }
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
                    out.push_str("{ let mut __rt_v = ");
                    self.emit_expr(out, left)?;
                    out.push_str(".borrow().clone(); __rt_v.extend(");
                    self.emit_expr(out, right)?;
                    out.push_str(".borrow().iter().cloned()); Rc::new(std::cell::RefCell::new(__rt_v)) }");
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
                } else if matches!(op, BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem)
                    && matches!(self.type_of(left)?, Type::Int)
                {
                    // H6: aritmética de `int` CHECKED como la VM (overflow → runtime error + exit 70,
                    // no wrapping silencioso; div/mod por cero con el texto de la VM). Los helpers
                    // `__ray_*` van inline; el coste medido en release es ~0.
                    let f = match op {
                        BinaryOp::Add => "__ray_add",
                        BinaryOp::Sub => "__ray_sub",
                        BinaryOp::Mul => "__ray_mul",
                        BinaryOp::Div => "__ray_div",
                        _ => "__ray_mod",
                    };
                    write!(out, "{}(", f).unwrap();
                    self.emit_expr(out, left)?;
                    out.push_str(", ");
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
                    _ => return Err("non-literal tuple index".into()),
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
            // `m: { let __rt_c = <concreto>; move |args| m_concreto(__rt_c.clone(), args) }` (sin `data`).
            ExprKind::StructLit { name, fields } if name.starts_with("__dyn_") => {
                // fields[0] = ("data", <concreto>); el resto = (método, <valor-vtable>).
                let concrete = &fields[0].1;
                out.push_str("{ let __rt_c = ");
                self.emit_expr(out, concrete)?;
                write!(out, "; Rc::new(std::cell::RefCell::new({} {{ ", mangle(name)).unwrap();
                for (i, (mname, mval)) in fields.iter().enumerate().skip(1) {
                    if i > 1 {
                        out.push_str(", ");
                    }
                    let (args, _) = self
                        .trait_method_sigs
                        .get(mname)
                        .ok_or_else(|| format!("unknown dyn method '{}'", mname))?
                        .clone();
                    // params de la closure: __a0: T0, __a1: T1, …
                    let mut params = String::new();
                    for (j, aty) in args.iter().enumerate() {
                        if j > 0 {
                            params.push_str(", ");
                        }
                        write!(params, "__a{}: {}", j, rust_ty(aty, &self.enums, &self.tparams)?).unwrap();
                    }
                    // el NOMBRE del campo-vtable se mangla (mismo que la def 592 y el acceso 2476); la clave
                    // del mapa `trait_method_sigs` sigue siendo el nombre original de raylang (arriba).
                    write!(out, "{}: {{ let __rt_c = __rt_c.clone(); Rc::new(move |{}| ", mangle(mname), params).unwrap();
                    // llamada al método concreto: m_concreto(__rt_c.clone(), __a0, …).
                    match &mval.kind {
                        ExprKind::Ident(fname) => write!(out, "{}(", mangle(fname)).unwrap(),
                        _ => {
                            out.push('(');
                            self.emit_expr(out, mval)?;
                            out.push_str(")(");
                        }
                    }
                    out.push_str("__rt_c.clone()");
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
                    write!(out, "{}: ", mangle(fname)).unwrap(); // el campo puede ser keyword de Rust
                    self.emit_expr(out, val)?;
                }
                out.push_str(" }))");
            }
            // Acceso a campo (lectura). Tupla: `t.0` → `t.0` (campo nativo, sin borrow). Struct: `p.x` →
            // `p.borrow().x.clone()`. (El `Field` de un método/UFCS lo consume `emit_call`.)
            ExprKind::Field { object, name } => {
                if matches!(self.type_of(object)?, Type::Tuple(_)) {
                    self.emit_expr(out, object)?;
                    write!(out, ".{}", name).unwrap(); // índice numérico de tupla: NO manglar
                } else {
                    self.emit_expr(out, object)?;
                    write!(out, ".borrow().{}.clone()", mangle(name)).unwrap();
                }
            }
            // Construcción de variante de enum. Option/Result → Some/None/Ok/Err NATIVOS de Rust (sin Rc);
            // un enum de usuario → Rc::new(EnumName::Variant(args)).
            ExprKind::EnumLit { enum_name, variant, args } => {
                let native = enum_name == "Option" || enum_name == "Result";
                if native {
                    out.push_str(variant); // Some / None / Ok / Err
                } else {
                    write!(out, "Rc::new({}::{}", mangle(enum_name), mangle(variant)).unwrap();
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
            ExprKind::Func(fnexpr) => self.emit_func_literal(out, fnexpr, true)?,
            ExprKind::Match { scrutinee, arms } => self.emit_match(out, scrutinee, arms)?,
            // Operador `?`: sobre Option/Result nativos de Rust → el `?` de Rust (la fn envolvente
            // devuelve un Option/Result compatible, garantizado por el checker).
            ExprKind::Try(inner) => {
                self.emit_expr(out, inner)?;
                out.push('?');
            }
            // Literal de Map: [k1: v1, k2: v2] → __RayMap::from_iter([(k1,v1), …]); [:] vacío → default().
            // (`default()`/`from_iter` y no `new()`/`from`: valen para CUALQUIER hasher `S: Default` —
            // con aHash (N2) el `new()`/`from` de HashMap no existen, son solo del RandomState de std.)
            ExprKind::MapLit(pairs) => {
                out.push_str("Rc::new(std::cell::RefCell::new(");
                if pairs.is_empty() {
                    out.push_str("__RayMap::default()");
                } else {
                    out.push_str("__RayMap::from_iter([");
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
    pub(super) fn emit_rayshow_impls(&self, out: &mut String, prog: &Program) -> Result<(), String> {
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
                write!(fmt, "{}: {{}}", fname).unwrap(); // el NOMBRE mostrado es el original (`type`, no `r#type`)
                // Un campo de tipo función se muestra como `<fn>` (como el Display del runtime): los tipos
                // función tienen firmas variadas → no hay un `impl RayShow` único; se renderiza el literal.
                if matches!(normalize_type(fty), Type::Fn(_, _)) {
                    write!(args, ", \"<fn>\"").unwrap();
                } else {
                    write!(args, ", __rt_b.{}.ray_show()", mangle(fname)).unwrap(); // el ACCESO sí mangla
                }
            }
            fmt.push_str(" }}");
            writeln!(out, "impl{} RayShow for Rc<std::cell::RefCell<{}{}>> {{ fn ray_show(&self) -> String {{ let __rt_b = self.borrow(); format!(\"{}\"{}) }} }}", gens, sm, sfx, fmt, args).unwrap();
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
                    // patrón Rust: variante manglada; display: nombre ORIGINAL (`E.loop`, no `E.r#loop`).
                    writeln!(out, "{}::{} => \"{}.{}\".to_string(),", em, mangle(&v.name), e.name, v.name).unwrap();
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
                    writeln!(out, "{}::{}({}) => format!(\"{}\"{}),", em, mangle(&v.name), binds.join(", "), fmt, args).unwrap();
                }
            }
            out.push_str("} } }\n");
        }
        Ok(())
    }

    /// Baja un `match` sobre un enum. El escrutinio (`Rc<E>`) se liga a un temporal y se matchea sobre
    /// `&*temp` (matchea `&E`). Los bindings del patrón quedan como `&campo`; al inicio de cada brazo se
    /// **clonan a valores propios** (`let b = b.clone();`) → el cuerpo los usa como cualquier variable.
    pub(super) fn emit_match(&mut self, out: &mut String, scrutinee: &Expr, arms: &[MatchArm]) -> Result<(), String> {
        // `classify` (no `normalize_type` pelado): el tipo puede llegar como Struct("E") sin resolver
        // cuando viene del RETORNO de una anotación de tipo función (`fn(..) -> E` de un param/campo).
        let scrut_ty = {
            let t = self.type_of(scrutinee)?;
            self.classify(&t)
        };
        // Option/Result son NATIVOS de Rust (no `Rc<E>`): se matchea sobre `&opt`, no `&*Rc`.
        let native = match &scrut_ty {
            Type::Enum(n, _) => n == "Option" || n == "Result",
            other => return Err(format!("match over {:?} (an enum was expected)", other)),
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
                return Err("match guards (`if`) are not supported".into());
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
    pub(super) fn emit_pattern(
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
                    write!(out, "{}::{}", mangle(enum_name), mangle(variant)).unwrap();
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
                            _ => return Err("Option/Result pattern without an expected type".into()),
                        }
                    } else {
                        let raw = self
                            .enum_variants
                            .get(enum_name)
                            .and_then(|m| m.get(variant))
                            .ok_or_else(|| format!("unknown variant {}.{}", enum_name, variant))?
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
                return Err("struct destructuring pattern is not supported".into())
            }
        }
        Ok(())
    }

    /// Aplana una cadena de concatenación de strings `a + b + c + …` en sus operandos (izq→der),
    /// descendiendo por los `+` cuyo operando izquierdo es string.
    pub(super) fn flatten_concat<'a>(&self, e: &'a Expr, out: &mut Vec<&'a Expr>) -> Result<(), String> {
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
    pub(super) fn emit_concat(&mut self, out: &mut String, operands: &[&Expr]) -> Result<(), String> {
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

    /// El tipo que aporta un brazo de `match` al tipo del `match`, o `None` si diverge o no se resuelve.
    /// Resuelve un cuerpo que es un binding del patrón (`Ok(conn) => conn`) desde el tipo del escrutinio.
    pub(super) fn arm_type(&self, scrut_ty: Option<&Type>, arm: &MatchArm) -> Option<Type> {
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
    pub(super) fn pattern_binding_types(&self, scrut_ty: Option<&Type>, pat: &Pattern) -> HashMap<String, Type> {
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

/// El i-ésimo argumento EFECTIVO de una llamada (el receptor de UFCS va primero, luego los args).
pub(super) fn effargs<'a>(recv: Option<&'a Expr>, args: &'a [Expr], i: usize) -> Result<&'a Expr, String> {
    recv.into_iter().chain(args.iter()).nth(i).ok_or_else(|| "missing an argument".to_string())
}

/// `Option<t>` (usando el Option nativo de Rust).
pub(super) fn opt_of(t: Type) -> Type {
    Type::Enum("Option".to_string(), vec![t])
}

/// Desenvuelve `Option<T>`/`Result<T,E>` → `T` (para `unwrap_or`/`unwrap`/`?`); otro tipo se deja igual.
pub(super) fn unwrapped(t: &Type) -> Type {
    match normalize_type(t) {
        Type::Enum(n, args) if (n == "Option" || n == "Result") && !args.is_empty() => args[0].clone(),
        other => other,
    }
}

/// ¿Es un tipo de heap (semántica de referencia / no `Copy`) → hay que clonar al leer?
/// ¿La expresión DIVERGE (no produce valor: termina en `return` o `panic`)? Un brazo de `match` que
/// diverge no contribuye al tipo del `match` (su "tipo" sería `!`). Se usa para inferir el tipo de un
/// `match` cuyo brazo real lleva un binding del patrón y el otro solo aborta (`Err(e) => { …; return }`).
pub(super) fn expr_diverges(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Block(b) => match &b.tail {
            Some(t) => expr_diverges(t),
            None => matches!(b.statements.last().map(|s| &s.kind), Some(StmtKind::Return { .. })),
        },
        ExprKind::Call { callee, .. } => matches!(&callee.kind, ExprKind::Ident(n) if n == "panic"),
        _ => false,
    }
}

pub(super) fn is_heap(t: &Type) -> bool {
    matches!(
        t,
        Type::String | Type::Bytes | Type::Array(_) | Type::Tuple(_) | Type::Map(_, _)
            | Type::Struct(_, _) | Type::Enum(_, _) | Type::Fn(_, _) | Type::Var(_) | Type::Dyn(_)
            | Type::Channel(_) | Type::Task(_) // semántica de referencia: clon = Arc bump
    )
}

/// Empuja `s` a un literal de plantilla `format!` de Rust, escapando lo necesario: `{`/`}` se duplican
/// (son metacaracteres de format!), y `"`/`\`/saltos se escapan como en cualquier string de Rust.
pub(super) fn push_fmt_literal(fmt: &mut String, s: &str) {
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

pub(super) fn binop(op: BinaryOp) -> &'static str {
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

