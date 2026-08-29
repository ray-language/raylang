//! Emisión de llamadas + inferencia de tipos para codegen (movimiento puro; usar
//! `git log --follow`).
//!
//! `emit_call` (el más grande: resuelve builtins/UFCS/traits/genéricos a la llamada Rust
//! correcta) + `type_of` (qué tipo tiene una expresión, para decidir clonado/conversión) +
//! los builtins de E/S por dominio (`emit_fs`/`emit_time`/`emit_random`/`emit_net`/
//! `emit_math`) + la repr SEND (H21-N5: `spawn_captures`, `emit_to_send`/`from_send_expr`,
//! `emit_send_convs`) que convierte valores al cruzar un hilo (spawn/canal/Task).

use super::*;

impl Transpiler {
    /// Nombres de las variables en ámbito cuyo tipo es `Channel`/`Task` (los valores compartibles entre
    /// hilos). Se clonan antes de un `spawn` para que el closure `move` no consuma el original. Dedup:
    /// el nombre más interno (shadowing) gana, y no se repite.
    /// Capturas de HEAP de un closure de `spawn` (H21-N5b): variables libres del cuerpo que son
    /// locales del ámbito envolvente y cuyo tipo debe cruzar por deep copy. Excluye canales/Tasks
    /// (se clonan: son el conducto compartido) y los escalares Copy (el move los copia). Una captura
    /// de tipo FUNCIÓN no puede cruzar (aún, N5c) → error claro en vez del E0277 de rustc.
    pub(super) fn spawn_captures(&mut self, body: &Block) -> Result<(Vec<(String, Type, bool)>, Vec<String>), String> {
        let mut ids = std::collections::HashSet::new();
        idents_of_block(body, &mut ids);
        let mut names: Vec<String> = ids.into_iter().collect();
        names.sort(); // orden determinista de emisión
        let mut out = Vec::new();
        let mut clones: Vec<String> = Vec::new();
        for name in names {
            let Some(ty) = self.lookup(&name) else { continue }; // global/función/builtin → no es local
            let ty = ty.clone();
            match normalize_type(&ty) {
                Type::Channel(_) | Type::Task(_) => {}
                Type::Int | Type::Float | Type::Bool | Type::Char | Type::UInt(_) | Type::Unit => {}
                // H21-N5c: un param fn MARCADO es un genérico `__F: ... + Send + Sync + Clone` → se
                // pre-clona como los canales (el bound ya garantiza que cruza). Uno NO marcado no
                // debería llegar aquí (el marcado por punto fijo lo habría marcado) — defensivo.
                Type::Fn(..) if self.send_fn_params.contains(&name) => clones.push(name),
                Type::Fn(..) => {
                    return Err(format!(
                        "'{}': a function value captured by 'spawn' cannot cross threads in the native backend yet",
                        name
                    ))
                }
                _ => {
                    let is_cell = self.cells.contains(&name);
                    out.push((name, ty, is_cell));
                }
            }
        }
        Ok((out, clones))
    }

    /// Emite `e` convertido a la repr SEND de un `Channel<T>`/`Task<T>` (para cruzar el hilo): string→
    /// Arc<str>, bytes→Arc<[u8]> (copia al borde, seguro por ser inmutables); primitivos sin cambio.
    /// H21-N5c: emite UN argumento de llamada; si la posición está MARCADA en el callee (param fn
    /// genérico Send), lo emite en su forma enviable (fn item pelado / closure sin Rc::new).
    /// Emite la llamada a una función de usuario (o closure en ámbito) con los argumentos **izados**
    /// a temporales. La clase RefCell-en-args (ago 2026, la destapó `stream_take(s, s.remaining)` de
    /// net/http): un argumento que lee un campo/índice emite `x.borrow().f.clone()`, y en Rust ese
    /// guard temporal vive hasta el final de la SENTENCIA — es decir, DURANTE la llamada; si el
    /// callee hace `borrow_mut` del mismo objeto, panica con "already borrowed" (la VM lo permite:
    /// evalúa los args y ya). Cada `let` cierra sus temporales → guards muertos al llamar, mismo
    /// orden de evaluación. Sirve a las DOS rutas de llamada de usuario (`shadows_builtin` — mismo
    /// módulo — y el brazo genérico del final, nombres calificados `mod::fn`).
    pub(super) fn emit_user_call_hoisted(&mut self, out: &mut String, name: &str, eff: &[&Expr]) -> Result<(), String> {
        let marked = self.fn_marks.get(name).cloned().unwrap_or_default();
        if eff.is_empty() {
            out.push_str(&mangle(name));
            out.push_str("()");
            return Ok(());
        }
        out.push_str("{ ");
        for (i, a) in eff.iter().enumerate() {
            write!(out, "let __rt_a{} = ", i).unwrap();
            self.emit_call_arg(out, a, marked.contains(&i))?;
            out.push_str("; ");
        }
        write!(out, "{}(", mangle(name)).unwrap();
        for i in 0..eff.len() {
            if i > 0 {
                out.push_str(", ");
            }
            write!(out, "__rt_a{}", i).unwrap();
        }
        out.push_str(") }");
        Ok(())
    }

    pub(super) fn emit_call_arg(&mut self, out: &mut String, a: &Expr, marked: bool) -> Result<(), String> {
        if marked {
            match &a.kind {
                ExprKind::Ident(n) if self.funcs.contains_key(n) && self.lookup(n).is_none() => {
                    out.push_str(&mangle(n)); // fn item: Send+Sync+Clone gratis
                    return Ok(());
                }
                ExprKind::Func(fx) => return self.emit_func_literal(out, fx, false),
                _ => {}
            }
        }
        self.emit_expr(out, a)
    }

    pub(super) fn emit_to_send(&mut self, out: &mut String, e: &Expr, t: &Type) -> Result<(), String> {
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
            // H21-N5a: los tipos COMPUESTOS cruzan como `__RaySend` (deep copy, semántica M38).
            ty if send_is_tree(&ty) => {
                let mut tmp = String::new();
                self.emit_expr(&mut tmp, e)?;
                let conv = self.to_send_expr(&ty, &tmp)?;
                out.push_str(&conv);
            }
            _ => self.emit_expr(out, e)?,
        }
        Ok(())
    }

    /// Id del conversor con nombre para un tipo NOMINAL concreto (struct/enum de usuario); lo registra
    /// si es nuevo (los cuerpos se generan al final, en `emit_send_convs`).
    pub(super) fn send_conv_id(&mut self, t: &Type) -> Result<usize, String> {
        let key = rust_ty(t, &self.enums, &self.tparams)?;
        if let Some(i) = self.send_convs.iter().position(|(_, k)| *k == key) {
            return Ok(i);
        }
        self.send_convs.push((normalize_type(t), key));
        Ok(self.send_convs.len() - 1)
    }

    /// Expresión Rust que convierte `expr` (repr del PROGRAMA, owned) a `__RaySend` (deep copy).
    pub(super) fn to_send_expr(&mut self, t: &Type, expr: &str) -> Result<String, String> {
        Ok(match self.classify(t) {
            Type::Int => format!("__RaySend::I({expr})"),
            Type::Float => format!("__RaySend::F({expr})"),
            Type::Bool => format!("__RaySend::B({expr})"),
            Type::Char => format!("__RaySend::C({expr})"),
            Type::Unit => format!("{{ let _ = {expr}; __RaySend::U }}"),
            Type::UInt(_) => format!("__RaySend::UI(({expr}) as u64)"),
            Type::String => format!("__RaySend::S(std::sync::Arc::<str>::from(&*({expr})))"),
            Type::Bytes => format!("__RaySend::By(std::sync::Arc::<[u8]>::from(&*({expr})))"),
            Type::Array(el) => {
                let inner = self.to_send_expr(&el, "__sx")?;
                format!(
                    "__RaySend::A(({expr}).borrow().iter().map(|__sr| {{ let __sx = __sr.clone(); {inner} }}).collect())"
                )
            }
            Type::Map(k, v) => {
                let ik = self.to_send_expr(&k, "__sk")?;
                let iv = self.to_send_expr(&v, "__sv")?;
                format!(
                    "__RaySend::M(({expr}).borrow().iter().map(|(__kr, __vr)| {{ let __sk = __kr.clone(); let __sv = __vr.clone(); ({ik}, {iv}) }}).collect())"
                )
            }
            Type::Tuple(ts) => {
                let mut parts = Vec::new();
                for (i, et) in ts.iter().enumerate() {
                    parts.push(self.to_send_expr(et, &format!("__st.{}.clone()", i))?);
                }
                format!("{{ let __st = {expr}; __RaySend::T(vec![{}]) }}", parts.join(", "))
            }
            // Option/Result: repr nativa de Rust → E(0/1, payload) (índices propios; solo han de
            // cuadrar entre to y from, no con la VM).
            Type::Enum(n, args) if n == "Option" && args.len() == 1 => {
                let inner = self.to_send_expr(&args[0], "__sx")?;
                format!("match {expr} {{ Some(__sx) => __RaySend::E(0, vec![{inner}]), None => __RaySend::E(1, vec![]) }}")
            }
            Type::Enum(n, args) if n == "Result" && args.len() == 2 => {
                let iok = self.to_send_expr(&args[0], "__sx")?;
                let ierr = self.to_send_expr(&args[1], "__sx")?;
                format!("match {expr} {{ Ok(__sx) => __RaySend::E(0, vec![{iok}]), Err(__sx) => __RaySend::E(1, vec![{ierr}]) }}")
            }
            t @ (Type::Struct(..) | Type::Enum(..)) => {
                let id = self.send_conv_id(&t)?;
                format!("__to_send_{id}({expr})")
            }
            // Canal/tarea: cruzan COMPARTIÉNDOSE (clone del Arc), type-erased en el árbol (el enum
            // `__RaySend` es monomórfico y `__RayChan<T>`/`__RayTask<T>` genéricos). Semántica de la
            // VM (M12): el canal es el conducto entre fibras, no un dato que copiar.
            Type::Channel(_) | Type::Task(_) => {
                format!("__RaySend::Ch(std::sync::Arc::new(({expr}).clone()) as std::sync::Arc<dyn std::any::Any + Send + Sync>)")
            }
            Type::Fn(..) => {
                return Err(
                    "a function value cannot cross a thread boundary (spawn capture / channel / Task) in the native backend"
                        .into(),
                )
            }
            other => return Err(format!("type {:?} cannot cross a thread boundary in the native backend", other)),
        })
    }

    /// Expresión Rust que reconstruye la repr del PROGRAMA desde `expr` (un `__RaySend` owned).
    pub(super) fn from_send_expr(&mut self, t: &Type, expr: &str) -> Result<String, String> {
        let un = "_ => unreachable!()";
        Ok(match self.classify(t) {
            Type::Int => format!("match {expr} {{ __RaySend::I(__sx) => __sx, {un} }}"),
            Type::Float => format!("match {expr} {{ __RaySend::F(__sx) => __sx, {un} }}"),
            Type::Bool => format!("match {expr} {{ __RaySend::B(__sx) => __sx, {un} }}"),
            Type::Char => format!("match {expr} {{ __RaySend::C(__sx) => __sx, {un} }}"),
            Type::Unit => format!("{{ let _ = {expr}; }}"),
            Type::UInt(_) => {
                let rty = rust_ty(t, &self.enums, &self.tparams)?;
                format!("match {expr} {{ __RaySend::UI(__sx) => __sx as {rty}, {un} }}")
            }
            Type::String => format!("match {expr} {{ __RaySend::S(__sx) => Rc::<str>::from(&*__sx), {un} }}"),
            Type::Bytes => format!("match {expr} {{ __RaySend::By(__sx) => Rc::<[u8]>::from(&*__sx), {un} }}"),
            Type::Array(el) => {
                let inner = self.from_send_expr(&el, "__sx")?;
                format!(
                    "match {expr} {{ __RaySend::A(__sa) => Rc::new(std::cell::RefCell::new(__sa.into_iter().map(|__sx| {inner}).collect::<Vec<_>>())), {un} }}"
                )
            }
            Type::Map(k, v) => {
                let ik = self.from_send_expr(&k, "__sk")?;
                let iv = self.from_send_expr(&v, "__sv2")?;
                format!(
                    "match {expr} {{ __RaySend::M(__sm) => Rc::new(std::cell::RefCell::new(__sm.into_iter().map(|(__sk, __sv2)| ({ik}, {iv})).collect::<__RayMap<_, _>>())), {un} }}"
                )
            }
            Type::Tuple(ts) => {
                let mut parts = Vec::new();
                for et in ts.iter() {
                    parts.push(self.from_send_expr(et, "__si.next().unwrap()")?);
                }
                format!(
                    "match {expr} {{ __RaySend::T(__st) => {{ let mut __si = __st.into_iter(); ({},) }}, {un} }}",
                    parts.join(", ")
                )
            }
            Type::Enum(n, args) if n == "Option" && args.len() == 1 => {
                let inner = self.from_send_expr(&args[0], "__sp.remove(0)")?;
                format!(
                    "match {expr} {{ __RaySend::E(0, mut __sp) => Some({inner}), __RaySend::E(_, _) => None, {un} }}"
                )
            }
            Type::Enum(n, args) if n == "Result" && args.len() == 2 => {
                let iok = self.from_send_expr(&args[0], "__sp.remove(0)")?;
                let ierr = self.from_send_expr(&args[1], "__sp.remove(0)")?;
                format!(
                    "match {expr} {{ __RaySend::E(0, mut __sp) => Ok({iok}), __RaySend::E(_, mut __sp) => Err({ierr}), {un} }}"
                )
            }
            t @ (Type::Struct(..) | Type::Enum(..)) => {
                let id = self.send_conv_id(&t)?;
                format!("__from_send_{id}({expr})")
            }
            // Canal/tarea: el árbol trae el Arc type-erased → downcast al genérico concreto y clone
            // (comparte el mismo canal; el `unreachable` está garantizado por el checker: el tipo del
            // elemento del canal es estático).
            ty @ (Type::Channel(_) | Type::Task(_)) => {
                let rty = rust_ty(&ty, &self.enums, &self.tparams)?;
                format!("match {expr} {{ __RaySend::Ch(__sc) => __sc.downcast_ref::<{rty}>().expect(\"channel/task type mismatch across threads\").clone(), {un} }}")
            }
            other => return Err(format!("type {:?} cannot cross a thread boundary in the native backend", other)),
        })
    }

    /// Genera (al final de la transpilación) las fns `__to_send_N`/`__from_send_N` de los tipos
    /// NOMINALES registrados. Worklist: generar un cuerpo puede registrar tipos anidados nuevos.
    /// Un conversor NO generable (p. ej. un struct con campos de tipo función, como la `App` del
    /// framework web) degrada a un STUB que panica — misma filosofía que las funciones: el programa
    /// COMPILA y solo falla en runtime si el flujo real cruza ese valor por un hilo.
    pub(super) fn emit_send_convs(&mut self, out: &mut String) -> Result<(), String> {
        let mut done = 0;
        while done < self.send_convs.len() {
            let (t, _) = self.send_convs[done].clone();
            let id = done;
            done += 1;
            let rty = rust_ty(&t, &self.enums, &self.tparams)?;
            let mut cbuf = String::new();
            if let Err(e) = self.emit_send_conv_one(&mut cbuf, id, &t, &rty) {
                if std::env::var_os("RAYLANG_TRANSPILE_DEBUG").is_some() {
                    eprintln!("[transpile send-stub] {rty} — {e}");
                }
                let msg = format!("value of a type holding functions cannot cross a thread boundary in the native backend ({e}); rebuild it inside the fiber (e.g. web/framework's listen_app)");
                writeln!(
                    out,
                    "#[allow(unused_variables)] fn __to_send_{id}(__sv: {rty}) -> __RaySend {{ panic!(\"{{}}\", {msg:?}) }}"
                )
                .unwrap();
                writeln!(
                    out,
                    "#[allow(unused_variables)] fn __from_send_{id}(__ss: __RaySend) -> {rty} {{ panic!(\"{{}}\", {msg:?}) }}"
                )
                .unwrap();
                continue;
            }
            out.push_str(&cbuf);
        }
        Ok(())
    }

    /// El cuerpo de UN par de conversores (`__to_send_id`/`__from_send_id`) para el tipo nominal `t`.
    pub(super) fn emit_send_conv_one(&mut self, out: &mut String, id: usize, t: &Type, rty: &str) -> Result<(), String> {
        {
            match normalize_type(t) {
                Type::Struct(n, args) => {
                    let fields = self
                        .struct_fields
                        .get(&n)
                        .cloned()
                        .ok_or_else(|| format!("unknown struct '{}' in send conversion", n))?;
                    // Sustituye los params de tipo del struct por los args concretos (Caja<int>).
                    let tps = self.struct_tparams.get(&n).cloned().unwrap_or_default();
                    let subst: HashMap<String, Type> =
                        tps.iter().cloned().zip(args.iter().cloned()).collect();
                    let mut tos = Vec::new();
                    let mut froms = Vec::new();
                    for (fname, fty) in &fields {
                        let cty = subst_type(fty, &subst);
                        tos.push(self.to_send_expr(&cty, &format!("__sb.{}.clone()", mangle(fname)))?);
                        froms.push(format!(
                            "{}: {}",
                            mangle(fname),
                            self.from_send_expr(&cty, "__si.next().unwrap()")?
                        ));
                    }
                    writeln!(
                        out,
                        "fn __to_send_{id}(__sv: {rty}) -> __RaySend {{ let __sb = __sv.borrow(); __RaySend::T(vec![{}]) }}",
                        tos.join(", ")
                    )
                    .unwrap();
                    writeln!(
                        out,
                        "fn __from_send_{id}(__ss: __RaySend) -> {rty} {{ match __ss {{ __RaySend::T(__st) => {{ let mut __si = __st.into_iter(); Rc::new(std::cell::RefCell::new({} {{ {} }})) }}, _ => unreachable!() }} }}",
                        mangle(&n),
                        froms.join(", ")
                    )
                    .unwrap();
                }
                Type::Enum(n, args) => {
                    let variants = self
                        .enum_variants
                        .get(&n)
                        .cloned()
                        .ok_or_else(|| format!("unknown enum '{}' in send conversion", n))?;
                    // Orden DETERMINISTA de variantes (HashMap no lo garantiza): por nombre. Los índices
                    // solo han de cuadrar entre to y from (mismo orden en ambos).
                    let mut vnames: Vec<String> = variants.keys().cloned().collect();
                    vnames.sort();
                    let tps = self.enum_tparams.get(&n).cloned().unwrap_or_default();
                    let subst: HashMap<String, Type> =
                        tps.iter().cloned().zip(args.iter().cloned()).collect();
                    let ename = mangle(&n);
                    let mut to_arms = Vec::new();
                    let mut from_arms = Vec::new();
                    for (vi, vname) in vnames.iter().enumerate() {
                        let payload = &variants[vname];
                        if payload.is_empty() {
                            to_arms.push(format!("{}::{} => __RaySend::E({vi}, vec![])", ename, mangle(vname)));
                            from_arms.push(format!("({vi}, _) => Rc::new({}::{})", ename, mangle(vname)));
                        } else {
                            let binds: Vec<String> = (0..payload.len()).map(|i| format!("__sp{i}")).collect();
                            let mut tos = Vec::new();
                            let mut froms = Vec::new();
                            for (i, pty) in payload.iter().enumerate() {
                                let cty = subst_type(pty, &subst);
                                tos.push(self.to_send_expr(&cty, &format!("__sp{i}.clone()"))?);
                                froms.push(self.from_send_expr(&cty, "__si.next().unwrap()")?);
                            }
                            to_arms.push(format!(
                                "{}::{}({}) => __RaySend::E({vi}, vec![{}])",
                                ename,
                                mangle(vname),
                                binds.join(", "),
                                tos.join(", ")
                            ));
                            from_arms.push(format!(
                                "({vi}, __sp) => {{ let mut __si = __sp.into_iter(); Rc::new({}::{}({})) }}",
                                ename,
                                mangle(vname),
                                froms.join(", ")
                            ));
                        }
                    }
                    writeln!(
                        out,
                        "fn __to_send_{id}(__sv: {rty}) -> __RaySend {{ match &*__sv {{ {} }} }}",
                        to_arms.join(", ")
                    )
                    .unwrap();
                    writeln!(
                        out,
                        "#[allow(unused_variables)] fn __from_send_{id}(__ss: __RaySend) -> {rty} {{ match __ss {{ __RaySend::E(__svi, __sp) => match (__svi, __sp) {{ {}, _ => unreachable!() }}, _ => unreachable!() }} }}",
                        from_arms.join(", ")
                    )
                    .unwrap();
                }
                other => unreachable!("solo tipos nominales llevan conversor con nombre: {:?}", other),
            }
        }
        Ok(())
    }

    /// `std::math::<fn>(args)` → el método de `f64` de Rust equivalente. Unarias float→float directas;
    /// pow→powf; abs/min/max preservan el tipo (int|float, ambos con esos métodos en Rust).
    /// `std::fs::<fn>(args)` → I/O de archivos con `std::fs`/`std::io` de Rust. Ok/Err como la VM (mensajes
    /// vía `e.to_string()`). No determinista → probado por subproceso (no oráculo). Se cubre la ENTRADA
    /// (read_file) + la salida básica (write_file) + la consulta (exists); el resto → error claro.
    pub(super) fn emit_fs(&mut self, out: &mut String, ffn: &str, eff: &[&Expr]) -> Result<(), String> {
        match ffn {
            // read_file(path) -> Result<string, string>: lee el archivo entero a un string.
            "read_file" => {
                out.push_str("(match std::fs::read_to_string(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(
                    ") { Ok(__rt_c) => Ok::<Rc<str>, Rc<str>>(Rc::<str>::from(__rt_c)), \
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
            // M113: lectura por trozos (Result<Option<bytes>,string>) + seek (Result<int,string>).
            "read_bytes" => {
                self.needs_handles = true;
                out.push_str("__ray_read_bytes(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "seek" => {
                self.needs_handles = true;
                out.push_str("__ray_seek(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // M115.1: escritura binaria (Result<int,string>) + fsync (Result<int,string>).
            "write_bytes" => {
                self.needs_handles = true;
                out.push_str("__ray_write_bytes(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "sync" => {
                self.needs_handles = true;
                out.push_str("__ray_sync(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // M115.2: candado consultivo (Result<bool,string>) + unlock (Result<int,string>).
            "try_lock" => {
                self.needs_handles = true;
                out.push_str("__ray_try_lock(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "unlock" => {
                self.needs_handles = true;
                out.push_str("__ray_unlock(");
                self.emit_expr(out, eff[0])?;
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
            // mtime(path) -> Result<int,string>: última modificación en epoch-ms UTC (mensajes
            // byte-idénticos a la VM, incl. "mtime before epoch").
            "mtime" => {
                out.push_str("(match std::fs::metadata(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(
                    ").and_then(|__md| __md.modified()) { \
                     Ok(__t) => match __t.duration_since(std::time::UNIX_EPOCH) { \
                       Ok(__d) => Ok::<i64, Rc<str>>(__d.as_millis() as i64), \
                       Err(_) => Err(Rc::<str>::from(\"mtime before epoch\")) }, \
                     Err(__e) => Err(Rc::<str>::from(__e.to_string())) })",
                );
            }
            // I/O binaria: read_file_bytes -> Result<bytes,string>; write/append_file_bytes -> Result<int,string>.
            "read_file_bytes" => {
                out.push_str("(match std::fs::read(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(
                    ") { Ok(__rt_b) => Ok::<Rc<[u8]>, Rc<str>>(Rc::<[u8]>::from(__rt_b)), \
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
            // append_file(path, content): añade al final (crea si no existe). Devuelve Ok(nº de
            // CARACTERES escritos) como la VM (`std/fs.ray`: `Result.Ok(content.len())`, y `len` de
            // string cuenta caracteres) — de ahí el mismo fast-path ASCII que el builtin `len`.
            "append_file" => {
                out.push_str("{ let __rt_p = ");
                self.emit_expr(out, eff[0])?;
                out.push_str("; let __rt_c = ");
                self.emit_expr(out, eff[1])?;
                out.push_str(
                    "; (match std::fs::OpenOptions::new().create(true).append(true).open(&*__rt_p)\
                     .and_then(|mut __rt_f| { use std::io::Write; __rt_f.write_all(__rt_c.as_bytes()) }) {\
                     Ok(()) => Ok::<i64, Rc<str>>(if __rt_c.is_ascii() { __rt_c.len() as i64 } \
                     else { __rt_c.chars().count() as i64 }), \
                     Err(__e) => Err(Rc::<str>::from(__e.to_string())) }) }",
                );
            }
            "append_file_bytes" => {
                out.push_str(
                    "(match std::fs::OpenOptions::new().create(true).append(true).open(&*",
                );
                self.emit_expr(out, eff[0])?;
                out.push_str(").and_then(|mut __rt_f| { use std::io::Write; __rt_f.write_all(&*");
                self.emit_expr(out, eff[1])?;
                out.push_str(") }) { Ok(()) => Ok::<i64, Rc<str>>(");
                self.emit_expr(out, eff[1])?;
                out.push_str(".len() as i64), Err(__e) => Err(Rc::<str>::from(__e.to_string())) })");
            }
            _ => return Err(format!("std::fs::{} is not supported", ffn)),
        }
        Ok(())
    }

    /// `std::time::<fn>`: now/monotonic → int (millis), monotonic_nanos → int (ns), sleep(ms) → duerme.
    /// now/sleep inline; monotonic/monotonic_nanos usan un `Instant` global compartido (helpers
    /// `__ray_monotonic`/`__ray_monotonic_nanos`, activan `needs_time_rng`).
    pub(super) fn emit_time(&mut self, out: &mut String, tfn: &str, eff: &[&Expr]) -> Result<(), String> {
        match tfn {
            "now" => out.push_str(
                "(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|__d| __d.as_millis() as i64).unwrap_or(0))",
            ),
            "monotonic" => {
                self.needs_time_rng = true;
                out.push_str("__ray_monotonic()");
            }
            "monotonic_nanos" => {
                self.needs_time_rng = true;
                out.push_str("__ray_monotonic_nanos()");
            }
            "sleep" => {
                // F2 (--fibers): dentro de una fibra, dormir el HILO retendría un worker; se
                // duerme la fibra (timer del reactor). Fuera de fibra (main), duerme el hilo.
                if self.fibers {
                    out.push_str("ray_runtime::fibers::sleep_ms((");
                    self.emit_expr(out, eff[0])?;
                    out.push_str(").max(0))");
                } else {
                    out.push_str("std::thread::sleep(std::time::Duration::from_millis((");
                    self.emit_expr(out, eff[0])?;
                    out.push_str(").max(0) as u64))");
                }
            }
            _ => return Err(format!("std::time::{} is not supported", tfn)),
        }
        Ok(())
    }

    /// `std::random::<fn>`: next() → float [0,1); below(n) → int [0,n); seed(n) fija la semilla. PRNG
    /// SplitMix64 propio (mismo que la VM) con estado global; no determinista → casa por propiedades.
    pub(super) fn emit_random(&mut self, out: &mut String, rfn: &str, eff: &[&Expr]) -> Result<(), String> {
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
            _ => return Err(format!("std::random::{} is not supported", rfn)),
        }
        Ok(())
    }

    /// `std::net::<fn>` (sockets TCP) → los helpers `__ray_tcp_*`/`__ray_socket_*` del registro de handles
    /// (activa `needs_net`). connect/listen/accept → Result<int,string>; read → Result<string,string>;
    /// read_bytes → Result<bytes,string>; write/write_bytes → Result<int,string>; local_port → int.
    pub(super) fn emit_net(&mut self, out: &mut String, nfn: &str, eff: &[&Expr]) -> Result<(), String> {
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
            // M123: la dirección del peer — intercept a nivel de WRAPPER (como tcp_connect):
            // __ray_peer_addr devuelve el Result nativo directamente.
            "peer_addr" => {
                out.push_str("__ray_peer_addr(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // M122: connect con plazo (host, port, ms) — vencido = "connect timeout".
            "tcp_connect_timeout" => {
                out.push_str("__ray_tcp_connect_timeout(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push_str(", ");
                self.emit_expr(out, eff[2])?;
                out.push(')');
            }
            // M130: half-close — shutdown(SHUT_WR).
            "shutdown_write" => {
                out.push_str("__ray_shutdown_write(");
                self.emit_expr(out, eff[0])?;
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
            _ => return Err(format!("std::net::{} is not supported", nfn)),
        }
        Ok(())
    }

    pub(super) fn emit_math(&mut self, out: &mut String, mfn: &str, eff: &[&Expr]) -> Result<(), String> {
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
            // atan2(y, x): el receptor es la ORDENADA (como en la VM, que saca x y luego y).
            "atan2" => {
                out.push('(');
                self.emit_expr(out, eff[0])?;
                out.push_str(").atan2(");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // Reinterpretación bit a bit IEEE-754 (misma impl que la VM: `to_bits`/`from_bits`).
            "float_bits" => {
                // Los paréntesis EXTERNOS son obligatorios: el `as` es la última operación y el sitio
                // de llamada puede encadenar un método (`.ray_show()`), que Rust no admite tras un cast.
                out.push_str("((");
                self.emit_expr(out, eff[0])?;
                out.push_str(").to_bits() as i64)");
            }
            "float_from_bits" => {
                out.push_str("f64::from_bits(");
                self.emit_expr(out, eff[0])?;
                out.push_str(" as u64)");
            }
            _ => return Err(format!("std::math::{} is not supported", mfn)),
        }
        Ok(())
    }

    /// N-D (fusiones): si `e` es una llamada al BUILTIN `want` (no a una función de usuario que lo
    /// sombree), devuelve sus argumentos efectivos (receptor UFCS primero). Reconoce las dos formas
    /// (llamada libre y método) con la MISMA resolución que `emit_call`.
    pub(super) fn as_builtin_call<'a>(&self, e: &'a Expr, want: &str) -> Option<Vec<&'a Expr>> {
        let ExprKind::Call { callee, args } = &e.kind else { return None };
        let (name, recv) = resolve_callee(callee).ok()?;
        if self.funcs.contains_key(name) {
            return None; // función de usuario homónima: sin fusión
        }
        let method = name.rsplit('#').next().unwrap_or(name).trim_start_matches("__");
        if method != want {
            return None;
        }
        Some(recv.into_iter().chain(args.iter()).collect())
    }

    /// N-D1/N-D2: emite el USO fusionado de un `substring(s, a, b)` que solo se consume (parsear,
    /// buscar) — el camino ASCII trabaja sobre el **slice** `&s[lo..hi]` sin materializar el
    /// `Rc<str>`; el no-ASCII cae en `__ray_substring` (corte por carácter, como hoy). El closure
    /// `use_` recibe el TEXTO del receptor `&str` y emite la operación completa sobre él (se emite
    /// en ambas ramas; en runtime solo corre una). El clamp espeja `__ray_substring` exactamente.
    fn emit_substring_fused<F>(&mut self, out: &mut String, sub: &[&Expr], mut use_: F) -> Result<(), String>
    where
        F: FnMut(&mut Self, &mut String, &str) -> Result<(), String>,
    {
        out.push_str("{ let __rt_fs = ");
        self.emit_expr(out, sub[0])?;
        out.push_str("; let __rt_fa: i64 = ");
        self.emit_expr(out, sub[1])?;
        out.push_str("; let __rt_fb: i64 = ");
        self.emit_expr(out, sub[2])?;
        out.push_str("; if __rt_fs.is_ascii() { let __rt_fl = __rt_fs.len() as i64; let __rt_lo = __rt_fa.clamp(0, __rt_fl) as usize; let __rt_hi = __rt_fb.clamp(__rt_lo as i64, __rt_fl) as usize; ");
        use_(self, out, "&__rt_fs[__rt_lo..__rt_hi]")?;
        out.push_str(" } else { ");
        use_(self, out, "&*__ray_substring(&__rt_fs, __rt_fa, __rt_fb)")?;
        out.push_str(" } }");
        Ok(())
    }

    pub(super) fn emit_call(&mut self, out: &mut String, callee: &Expr, args: &[Expr]) -> Result<(), String> {
        let (name, recv) = resolve_callee(callee)?;
        // Despacho dinámico (M9.3b): el checker baja `obj.m(a)` a `(r.m)(r.data, a)` con `r: dyn`. Aquí el
        // campo `m` es una closure que capturó el concreto → `(r.borrow().m.clone())(a)` (se descarta el
        // arg `r.data` que añadió el checker: es `args[0]`).
        if let Some(r) = recv {
            if matches!(self.type_of(r).ok(), Some(Type::Dyn(_))) {
                // El clon del campo-vtable se IZA a un `let` propio: cierra el guard del `borrow()`
                // del receptor ANTES de llamar (si quedara vivo, un `borrow_mut` del mismo objeto
                // dentro del closure panicaría con "already borrowed" — la clase RefCell-en-args).
                out.push_str("{ let __rt_cl = ");
                self.emit_expr(out, r)?;
                write!(out, ".borrow().{}.clone(); __rt_cl(", mangle(name)).unwrap(); // campo-vtable: mismo mangle que 592
                for (i, a) in args.iter().skip(1).enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    self.emit_expr(out, a)?;
                }
                out.push_str(") }");
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
                    // Mismo izado que el despacho dyn: el guard del receptor se cierra antes de llamar.
                    out.push_str("{ let __rt_cl = ");
                    self.emit_expr(out, r)?;
                    write!(out, ".borrow().{}.clone(); __rt_cl(", mangle(name)).unwrap(); // campo-función: mismo mangle que 599
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_expr(out, a)?;
                    }
                    out.push_str(") }");
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
        // std/ffi (revisión FFI jul 2026): `errno()` → helper emitido que lee el errno del hilo.
        if name == "std::ffi::errno" {
            self.needs_ffi_errno = true;
            out.push_str("__ray_ffi_errno()");
            return Ok(());
        }
        // R5 (bench regex): las funciones INTERNAS `run_*` de std/regex (reciben el `Prog`, que
        // retiene el patrón FUENTE ya validado por el parser raylang) se ejecutan con el crate
        // `regex` de ray-runtime — la validación y sus errores siguen siendo los de std/regex
        // (paridad byte-idéntica con la VM). `--without regex` desactiva la interceptación y la
        // Pike VM raylang se transpila tal cual: el fallback es la implementación real, sin stubs.
        if let Some(rfn) = name.strip_prefix("std::regex::run_") {
            if !self.exclude.contains("regex")
                && matches!(rfn, "full" | "search" | "find" | "find_all" | "replace_all" | "captures" | "captures_str")
            {
                self.needs_rt_regex = true;
                out.push_str("{ let __rx_p = ");
                self.emit_expr(out, eff[0])?;
                out.push_str("; let __rx_b = __rx_p.borrow(); ");
                match rfn {
                    "full" => {
                        out.push_str("ray_runtime::regex::full_match(&*__rx_b.pat, &");
                        self.emit_expr(out, eff[1])?;
                        out.push(')');
                    }
                    "search" => {
                        out.push_str("ray_runtime::regex::search(&*__rx_b.pat, &");
                        self.emit_expr(out, eff[1])?;
                        out.push(')');
                    }
                    "find" => {
                        out.push_str("ray_runtime::regex::find(&*__rx_b.pat, &");
                        self.emit_expr(out, eff[1])?;
                        out.push(')');
                    }
                    "find_all" => {
                        out.push_str("Rc::new(std::cell::RefCell::new(ray_runtime::regex::find_all(&*__rx_b.pat, &");
                        self.emit_expr(out, eff[1])?;
                        out.push_str(").into_iter().map(Rc::<str>::from).collect::<Vec<Rc<str>>>()))");
                    }
                    "replace_all" => {
                        out.push_str("Rc::<str>::from(ray_runtime::regex::replace_all(&*__rx_b.pat, &");
                        self.emit_expr(out, eff[1])?;
                        out.push_str(", &");
                        self.emit_expr(out, eff[2])?;
                        out.push_str("))");
                    }
                    "captures" => {
                        out.push_str("ray_runtime::regex::captures(&*__rx_b.pat, &");
                        self.emit_expr(out, eff[1])?;
                        out.push_str(").map(|__rx_v| Rc::new(std::cell::RefCell::new(__rx_v)))");
                    }
                    "captures_str" => {
                        // R6: rangos de bytes → los Rc<str> se cortan DIRECTO del texto (un alloc
                        // por grupo, sin el Vec<Option<String>> intermedio ni su recopia).
                        out.push_str("{ let __rx_t = ");
                        self.emit_expr(out, eff[1])?;
                        out.push_str("; ray_runtime::regex::captures_byte_ranges(&*__rx_b.pat, &__rx_t).map(|__rx_v| Rc::new(std::cell::RefCell::new(__rx_v.into_iter().map(|__rx_o| __rx_o.map(|(__rx_s, __rx_e)| Rc::<str>::from(&__rx_t[__rx_s..__rx_e]))).collect::<Vec<Option<Rc<str>>>>()))) }");
                    }
                    _ => unreachable!("guarded by the matches! above"),
                }
                out.push_str(" }");
                return Ok(());
            }
        }
        // `std::fs::*` (módulo std/fs) → I/O de archivos con `std::fs`/`std::io` de Rust (Ok/Err como la VM).
        // Excepción M115.3: stat/chmod NO se interceptan aquí — sus wrappers se emiten (el intercept
        // es a nivel de primitivo `__stat`/`__chmod`, abajo), así el struct `Stat` vive en raylang.
        if let Some(ffn) = name.strip_prefix("std::fs::") {
            if !matches!(ffn, "stat" | "chmod" | "watch" | "next_event" | "next_event_timeout") {
                return self.emit_fs(out, ffn, &eff);
            }
        }
        // `std::time::{now,monotonic,sleep}`/`std::random::{next,below,seed}` → reloj + PRNG de Rust (no
        // deterministas → casan por propiedades). El resto de std/time|random es raylang puro → pasa de largo.
        if let Some(tfn) = name.strip_prefix("std::time::") {
            if matches!(tfn, "now" | "monotonic" | "monotonic_nanos" | "sleep") {
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
                "tcp_connect" | "tcp_connect_timeout" | "tcp_listen" | "tcp_accept" | "peer_addr" | "shutdown_write" | "socket_read" | "socket_read_bytes"
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
                    out.push_str("Rc::new(std::cell::RefCell::new(__RayMap::default()))");
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
        // H3: un OVERRIDE del usuario gana sobre el builtin homónimo interceptado, como en la VM (M7.3).
        // `self.funcs` solo contiene funciones NO saltadas por `skip_fn_def` = las que el usuario definió y
        // cuya def SÍ se emite (las de prelude se saltan) → su llamada debe ir a esa def, nunca a un builtin.
        // También gana un `let f = fn(){…}` local que sombree el nombre (closure en ámbito). Un nombre con
        // `::`/`#` lo sintetizó el checker (módulo/método) → nunca colisiona con un builtin de nombre pelado.
        // OJO: la regla del override solo vale para funciones del PRELUDE (sort/map/get_or…, escritas en
        // raylang). Un builtin REAL de la tabla `BUILTINS` (print/to_string/`__*`…) NO se tapa: el checker
        // lo RECHAZA en la definición ("is a language builtin and cannot be redefined"), así que
        // `self.funcs` nunca debería traer uno — la guarda `lookup(name).is_none()` es defensiva, por si
        // la tabla crece o esa regla cambia: mantiene al nativo alineado con el compilador de la VM (que
        // emite el opcode del builtin antes de mirar `indices`). Una VARIABLE local sí puede taparlo y
        // gana en ambos motores (el compilador de la VM mira `name_is_variable` primero).
        let shadows_builtin = matches!(self.lookup(name), Some(Type::Fn(_, _)))
            || (!name.contains("::")
                && !name.contains('#')
                && self.funcs.contains_key(name)
                && crate::builtins::lookup(name).is_none())
            // Un método de TRAIT sobre un tipo de usuario/módulo cuyo nombre pelado coincide con un
            // builtin (`Store#get`, `Store#keys`…): el checker ya lo resolvió al manglado y su def SE
            // EMITE (vive en `funcs`) → llamada ordinaria, nunca la interceptación por `method` (que lo
            // confundiría con el builtin homónimo de Map/string). Las claves CORE (`Option#unwrap_or`,
            // `string#len`…) SÍ se interceptan: sus brazos nativos son la bajada intencional.
            || (name.contains('#')
                && self.funcs.contains_key(name)
                && !is_core_impl_key(name.split('#').next().unwrap_or("")));
        if shadows_builtin {
            self.emit_user_call_hoisted(out, name, &eff)?;
            return Ok(());
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
            "eprint" => {
                // Uniforme vía RayShow (maneja todo tipo, incl. structs/arreglos/genéricos). Sin
                // buffering (a diferencia de `print`, ver más abajo): stderr es tpicamente para
                // errores, baja frecuencia, y se espera visible de inmediato. Va por `__ray_eprint`
                // (no `eprintln!`): un stderr cerrado sigue la convención Unix (exit 141), no un
                // pánico de Rust — el gemelo del writer de stdout.
                if matches!(self.type_of(eff[0])?, Type::Fn(_, _)) {
                    out.push_str("__ray_eprint(\"<fn>\".to_string())"); // una función se muestra como <fn>
                } else {
                    out.push_str("__ray_eprint(");
                    self.emit_expr(out, eff[0])?;
                    out.push_str(".ray_show())");
                }
            }
            "print" => {
                // M96f: bufferizado (ver `__ray_buffered_print`/`__ray_flush_prints`, emitidos siempre
                // en el preámbulo) — bajo impresión concurrente intensiva (p. ej. `log_requests()`),
                // el lock global de `Stdout` en CADA `println!` era el mayor cuello de contención
                // medido (docs/investigacion-p99-framework-web.md §12).
                if matches!(self.type_of(eff[0])?, Type::Fn(_, _)) {
                    out.push_str("__ray_buffered_print(\"<fn>\".to_string())");
                } else {
                    out.push_str("__ray_buffered_print(");
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
                out.push_str(".borrow().iter().map(|__rt_x| *__rt_x as u8).collect::<Vec<u8>>())");
            }
            // Más builtins de string (→ métodos de `str`/`String` de Rust, misma semántica que la VM).
            "trim" => {
                out.push_str("Rc::<str>::from(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".trim())");
            }
            // index_of(s, sub) -> Option<int>: índice por carácter de la subcadena (helper del preámbulo).
            "index_of" => {
                // N-D2: `index_of(s.substring(a, b), aguja)` busca sobre el SLICE, sin materializar
                // la subcadena (`after_name` en el patrón de parsing manual copiaba toda la cola).
                if let Some(sub) = self.as_builtin_call(eff[0], "substring") {
                    let needle = eff[1];
                    return self.emit_substring_fused(out, &sub, |t, out, recv| {
                        write!(out, "__ray_index_of({}, &*", recv).unwrap();
                        t.emit_expr(out, needle)?;
                        out.push(')');
                        Ok(())
                    });
                }
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
                out.push_str("{ let __rt_n = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; if __rt_n <= 0 { Rc::<str>::from(\"\") } else { Rc::<str>::from(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".repeat(__rt_n as usize)) } }");
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
                out.push_str("__ray_substring(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push_str(", ");
                self.emit_expr(out, eff[2])?;
                out.push(')');
            }
            // to_string(x) → Rc<str>. Vía show_expr (maneja struct→borrow, arreglo→[…], escalar/enum).
            "to_string" => {
                out.push_str("Rc::<str>::from(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".ray_show())");
            }
            // len(x) → i64. String: nº de CARACTERES; arreglo/map: nº de elementos (vía borrow()).
            "len" => {
                match self.type_of(eff[0])? {
                    Type::Array(_) | Type::Map(_, _) => {
                        out.push('(');
                        self.emit_expr(out, eff[0])?;
                        out.push_str(".borrow().len() as i64)");
                    }
                    // string: `len` cuenta CARACTERES, no octetos — clave con UTF-8 multibyte (`más`, `ñ`).
                    // Fast-path ASCII (H19, como la VM en vm.rs:960): para un string ASCII, nº de octetos ==
                    // nº de chars → `.len()` (escaneo `is_ascii` con SIMD) es mucho más rápido que
                    // `.chars().count()` (decodifica char a char). Cierra la brecha O(n²) de `while i < s.len()`.
                    Type::String => {
                        out.push_str("{ let __rt_s = ");
                        self.emit_expr(out, eff[0])?;
                        out.push_str("; if __rt_s.is_ascii() { __rt_s.len() as i64 } else { __rt_s.chars().count() as i64 } }");
                    }
                    // bytes: `len` es el nº de octetos → `.len()` es correcto.
                    _ => {
                        out.push('(');
                        self.emit_expr(out, eff[0])?;
                        out.push_str(".len() as i64)");
                    }
                }
            }
            // push(a, v) → a.borrow_mut().push(v) (muta en el sitio, devuelve unit).
            "push" => {
                // Orden de la VM: arreglo, luego valor. Ambos a temporales ANTES del borrow_mut: si el
                // valor lee del MISMO arreglo (p. ej. `w.push(w[i] + w[j])`, típico en cripto), evita el
                // doble borrow del RefCell (panic). El receptor también se iza por si borrowea.
                out.push_str("{ let __rt_arr = ");
                self.emit_expr(out, eff[0])?;
                out.push_str("; let __rt_v = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; __rt_arr.borrow_mut().push(__rt_v); }");
            }
            // chars(s) → [char]: los caracteres del string como arreglo.
            "chars" => {
                out.push_str("Rc::new(std::cell::RefCell::new(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".chars().collect::<Vec<char>>()))");
            }
            // V2: `__concat(a, b, …)` (el checker aplana ahí las cadenas de `+` de strings y la
            // interpolación) → el MISMO `format!` único de `emit_concat` (los operandos llegan ya
            // aplanados; `to_string(x)` se inlinea como `{}`). Antes de V2 este aplanado lo hacía
            // el propio transpilador sobre la cadena de `Add` (`flatten_concat`, que sigue para
            // los `+` no registrados).
            // (el `match` es sobre `method`, que recorta el prefijo `__` → "concat")
            "concat" => {
                self.emit_concat(out, &eff)?;
            }
            // D3: formas fusionadas de `<wrapper>(…).unwrap_or(d)` (las genera el checker). En
            // nativo el Option ya era barato, pero la forma fusionada llega como builtin propio.
            "index_of_or" => {
                // N-D2 sobre la forma D3 (`index_of(sub(…), aguja).unwrap_or(d)` fusionada por el checker).
                if let Some(sub) = self.as_builtin_call(eff[0], "substring") {
                    let (needle, default) = (eff[1], eff[2]);
                    return self.emit_substring_fused(out, &sub, |t, out, recv| {
                        write!(out, "__ray_index_of({}, &*", recv).unwrap();
                        t.emit_expr(out, needle)?;
                        out.push_str(").unwrap_or(");
                        t.emit_expr(out, default)?;
                        out.push(')');
                        Ok(())
                    });
                }
                out.push_str("__ray_index_of(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push_str(").unwrap_or(");
                self.emit_expr(out, eff[2])?;
                out.push(')');
            }
            "parse_int_or" => {
                // N-D1 sobre la forma D3.
                if let Some(sub) = self.as_builtin_call(eff[0], "substring") {
                    let default = eff[1];
                    return self.emit_substring_fused(out, &sub, |t, out, recv| {
                        write!(out, "({}).trim().parse::<i64>().unwrap_or(", recv).unwrap();
                        t.emit_expr(out, default)?;
                        out.push(')');
                        Ok(())
                    });
                }
                out.push('(');
                self.emit_expr(out, eff[0])?;
                out.push_str(".trim().parse::<i64>().unwrap_or(");
                self.emit_expr(out, eff[1])?;
                out.push_str("))");
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
                // t.join() da la repr SEND; se convierte de vuelta a la del programa (string/bytes → Rc;
                // compuestos → desde el árbol __RaySend, N5a).
                let elem = match self.type_of(eff[0])? {
                    Type::Task(t) => (*t).clone(),
                    _ => unreachable!("guard garantiza Task"),
                };
                if send_is_tree(&elem) {
                    let mut tmp = String::new();
                    self.emit_expr(&mut tmp, eff[0])?;
                    let conv = self.from_send_expr(&elem, &format!("({tmp}).join()"))?;
                    out.push_str(&conv);
                    return Ok(());
                }
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
            // try_join(t) → Result<T, string> (H21-N2): une SIN re-lanzar — el fallo de la tarea como
            // VALOR (M56.5). Sobre `wait()` (N1); el Ok se convierte de la repr Send a la del programa,
            // como `join`.
            "try_join" if matches!(self.type_of(eff[0])?, Type::Task(_)) => {
                let elem = match self.type_of(eff[0])? {
                    Type::Task(t) => (*t).clone(),
                    _ => unreachable!("guard garantiza Task"),
                };
                let okconv = if send_is_tree(&elem) {
                    self.from_send_expr(&elem, "__rt_v")?
                } else {
                    match normalize_type(&elem) {
                        Type::String => "Rc::<str>::from(&*__rt_v)".to_string(),
                        Type::Bytes => "Rc::<[u8]>::from(&*__rt_v)".to_string(),
                        _ => "__rt_v".to_string(),
                    }
                };
                out.push_str("match (");
                self.emit_expr(out, eff[0])?;
                write!(out, ").wait_consume() {{ Ok(__rt_v) => Ok({}), Err(__rt_m) => Err(Rc::<str>::from(__rt_m)) }}", okconv).unwrap();
            }
            // M97.2: __try_call(f) → [string] (el primitivo del prelude): [] si `f` volvió bien,
            // [msg] si falló. En nativo es `catch_unwind` en el MISMO hilo — sin `spawn`, que es
            // justo el punto (docs/investigacion-p999-webserver-nativo.md). `__ray_rt_err` ya
            // panica con el payload `__RayErr`, así que no hay nada que cambiar en el lado del
            // error: solo hay que interceptar el unwind antes de que llegue al `main`.
            // `AssertUnwindSafe` es la misma decisión que ya toma el `catch_unwind` de `main`, y el
            // sharp edge (recuperar con estado a medio mutar) está documentado en el prelude.
            // Los paréntesis alrededor del argumento NO son decorativos: una closure emite como un
            // BLOQUE que produce un `Rc<dyn Fn() -> ()>` (`{ let x = x.clone(); Rc::new(move || …) }`),
            // y `bloque()` no es una llamada válida en Rust — hace falta `(bloque)()`.
            // La guarda sobre el nombre COMPLETO es necesaria: `method` ya viene con el prefijo `__`
            // recortado, así que sin ella este brazo taparía también al envoltorio `try_call` del
            // prelude — y ese sí queremos que transpile como raylang normal (es quien arma el
            // `Result`), no que lo reimplemente el backend.
            "try_call" if name == "__try_call" => {
                out.push_str("__ray_try_call(|| (");
                self.emit_expr(out, eff[0])?;
                out.push_str(")())");
            }
            // std/io (M107.1): stdout sin salto va por el CANAL del escritor de M96f
            // (`__ray_stdout_write`, sin '\n'): escribir directo a `std::io::stdout` rompería el
            // orden respecto a `print` (asíncrono). stderr sí escribe directo (como `eprintln!`).
            // El primitivo devuelve el arreglo etiquetado ["ok"]/["err", msg] del contrato de la VM;
            // por el canal el envío no falla (los errores del writer se tragan, como en `print`) →
            // siempre ["ok"]. Guardas por el nombre crudo: solo interceptan el PRIMITIVO `__…`.
            "stdout_write" if name.starts_with("__") => {
                out.push_str("{ __ray_stdout_write((&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(").as_bytes().to_vec()); Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\")])) }");
            }
            "stdout_write_bytes" if name.starts_with("__") => {
                out.push_str("{ __ray_stdout_write((&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(").to_vec()); Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\")])) }");
            }
            "stdout_flush" if name.starts_with("__") => {
                out.push_str(
                    "{ __ray_flush_prints(); Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\")])) }",
                );
            }
            "stderr_write" if name.starts_with("__") => {
                out.push_str("{ use std::io::Write; match std::io::stderr().lock().write_all((&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(
                    ").as_bytes()) { Ok(()) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\")])), \
                     Err(__rt_e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(__rt_e.to_string())])) } }",
                );
            }
            // std/io (M107.2): lectura de stdin por bytes (runtime `__ray_stdin_*`, ver runtime.rs).
            // El primitivo produce el arreglo etiquetado del contrato de la VM ([bytes]).
            "stdin_read" if name.starts_with("__") && eff.len() == 1 => {
                self.needs_stdin = true;
                out.push_str("{ let __rt_b = __ray_stdin_read(");
                self.emit_expr(out, eff[0])?;
                out.push_str("); Rc::new(std::cell::RefCell::new(if __rt_b.is_empty() { Vec::<Rc<[u8]>>::new() } else { vec![Rc::<[u8]>::from(__rt_b)] })) }");
            }
            "stdin_read_timeout" if name.starts_with("__") => {
                self.needs_stdin = true;
                out.push_str("{ let __rt_r = __ray_stdin_read_timeout(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push_str("); Rc::new(std::cell::RefCell::new(match __rt_r { \
                     None => vec![Rc::<[u8]>::from(&b\"timeout\"[..])], \
                     Some(__rt_b) if __rt_b.is_empty() => vec![Rc::<[u8]>::from(&b\"eof\"[..])], \
                     Some(__rt_b) => vec![Rc::<[u8]>::from(&b\"data\"[..]), Rc::<[u8]>::from(__rt_b)] })) }");
            }
            // std/term (M107.3): terminal (runtime `__ray_term_*`, ver runtime.rs).
            "term_is_tty" if name.starts_with("__") => {
                self.needs_term = true;
                out.push_str("__ray_term_is_tty(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "term_size_px" if name.starts_with("__") => {
                self.needs_term = true;
                out.push_str("Rc::new(std::cell::RefCell::new(match __ray_term_size_px() { Some((w, h)) => vec![w, h], None => Vec::new() }))");
            }
            "term_size" if name.starts_with("__") => {
                self.needs_term = true;
                out.push_str("Rc::new(std::cell::RefCell::new(match __ray_term_size() { Some((c, r)) => vec![c, r], None => Vec::new() }))");
            }
            "term_raw_on" | "term_raw_off" if name.starts_with("__") => {
                self.needs_term = true;
                let on = if method == "term_raw_on" { "true" } else { "false" };
                write!(out, "Rc::new(std::cell::RefCell::new(match __ray_term_raw({on}) {{ \
                     Ok(()) => vec![Rc::<str>::from(\"ok\")], \
                     Err(__rt_e) => vec![Rc::<str>::from(\"err\"), Rc::<str>::from(__rt_e)] }}))").unwrap();
            }
            // __task_failed(t) → [string] (el primitivo del prelude): [] si acabó bien, [msg] si falló.
            // El wrapper try_join se intercepta arriba; esto cubre un uso directo del primitivo.
            "__task_failed" => {
                out.push_str("Rc::new(std::cell::RefCell::new(match (");
                self.emit_expr(out, eff[0])?;
                out.push_str(").wait_failed() { None => Vec::<Rc<str>>::new(), Some(__rt_m) => vec![Rc::<str>::from(__rt_m)] }))");
            }
            "join" => {
                out.push_str("__ray_join(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // --- Map ---
            // H4 (la clase completa): igual que en las asignaciones indexadas, la clave/valor/delta
            // pueden LEER el mismo map (`m.insert(k, m.get_or(k,0)+1)`) → van a temporales `__rt_*`
            // ANTES del borrow_mut, en el orden de la VM (map, clave, valor), o el RefCell panica
            // donde la VM funciona.
            "insert" => {
                // devuelve unit → bloque con `;` (HashMap::insert de Rust devuelve Option).
                out.push_str("{ let __rt_m = ");
                self.emit_expr(out, eff[0])?;
                out.push_str("; let __rt_k = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; let __rt_v = ");
                self.emit_expr(out, eff[2])?;
                out.push_str("; __rt_m.borrow_mut().insert(__rt_k, __rt_v); }");
            }
            // add_to(m, k, delta): `*m.entry(k).or_insert(0) += delta` (upsert acumulativo, como la VM).
            "add_to" => {
                let zero = match self.type_of(eff[0])? {
                    Type::Map(_, v) if matches!(*v, Type::Float) => "0.0",
                    _ => "0i64",
                };
                out.push_str("{ let __rt_m = ");
                self.emit_expr(out, eff[0])?;
                out.push_str("; let __rt_k = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; let __rt_d = ");
                self.emit_expr(out, eff[2])?;
                write!(out, "; *__rt_m.borrow_mut().entry(__rt_k).or_insert({}) += __rt_d; }}", zero)
                    .unwrap();
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
            // remove(m, k) → Option<V> (quita y devuelve). Fusionado con unwrap_or. La clave se iza
            // antes del borrow_mut (puede leer el mismo map: `m.remove(m.keys()[0])`).
            "remove" => {
                out.push_str("{ let __rt_m = ");
                self.emit_expr(out, eff[0])?;
                out.push_str("; let __rt_k = ");
                self.emit_expr(out, eff[1])?;
                // el resultado se liga a un temporal: el RefMut de una expr de cola sobreviviría
                // a los locales del bloque (E0597).
                out.push_str("; let __rt_r = __rt_m.borrow_mut().remove(&__rt_k); __rt_r }");
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
            // "sort_prim" = `__sort_prim` (V5): solo primitivos (checker) → sort INESTABLE (SN1:
            // idéntico observable, sin el buffer n/2 del estable). El `sort` genérico (tipos de
            // usuario, estabilidad observable) sigue en `__ray_sort` estable.
            "sort" => {
                // IDEAS §63: [float] no puede ir por __ray_sort (f64 no es Ord en Rust → E0277 en
                // el build del usuario). Va por __ray_sort_float: el merge del prelude con `<`,
                // byte-idéntico a la VM incluso con NaN.
                let is_float =
                    matches!(self.type_of(eff[0])?, Type::Array(ref e) if matches!(**e, Type::Float));
                out.push_str(if is_float { "__ray_sort_float(&" } else { "__ray_sort(&" });
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // reverse(a) -> [T]: copia invertida (no muta), como el opcode `Reverse` de la VM.
            "reverse" => {
                out.push_str("Rc::new(std::cell::RefCell::new(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().rev().cloned().collect::<Vec<_>>()))");
            }
            // pop(a) -> Option<T>: quita el último EN EL SITIO. El envoltorio del prelude (que traduce
            // el `[]`/`[x]` del primitivo `__pop` a Option) no se emite, así que aquí se produce ya el
            // Option de Rust — igual que `index_of`.
            "pop" => {
                out.push_str("(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow_mut().pop())");
            }
            // position(a, x) -> Option<int>: índice de la 1ª ocurrencia. Igualdad estructural con `==`,
            // como `contains`. El valor buscado se iza ANTES del borrow (puede leer del mismo arreglo).
            "position" => {
                out.push_str("{ let __rt_x = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; ");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().position(|__e| *__e == __rt_x).map(|__i| __i as i64) }");
            }
            "sort_prim" => {
                out.push_str("__ray_sort_unstable(&");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // parse_int(s) → Rust Option<i64>. Con `.trim()`: la VM trimea (`__parse_int`,
            // vm/mod.rs) y el nativo no lo hacía — bug de paridad cazado con las fusiones N-D
            // (`parse_int(" 42 ")`: VM Some(42), nativo None). `parse_int_or` ya trimeaba.
            "parse_int" => {
                // N-D1: `parse_int(s.substring(a, b))` parsea el SLICE en sitio, sin el Rc<str>.
                if let Some(sub) = self.as_builtin_call(eff[0], "substring") {
                    return self.emit_substring_fused(out, &sub, |_, out, recv| {
                        write!(out, "({}).trim().parse::<i64>().ok()", recv).unwrap();
                        Ok(())
                    });
                }
                out.push('(');
                self.emit_expr(out, eff[0])?;
                out.push_str(".trim().parse::<i64>().ok())");
            }
            "parse_float" => {
                out.push('(');
                self.emit_expr(out, eff[0])?;
                out.push_str(".trim().parse::<f64>().ok())");
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
                    out.push_str("{ let __rt_s = ");
                    self.emit_expr(out, eff[1])?;
                    out.push_str("; __rt_s.is_empty() || ");
                    self.emit_expr(out, eff[0])?;
                    out.push_str(".windows(__rt_s.len().max(1)).any(|__w| __w == &*__rt_s) }");
                }
                Type::Array(_) => {
                    out.push_str("{ let __rt_x = ");
                    self.emit_expr(out, eff[1])?;
                    out.push_str("; ");
                    self.emit_expr(out, eff[0])?;
                    out.push_str(".borrow().iter().any(|__e| *__e == __rt_x) }");
                }
                other => return Err(format!("contains on {:?} is not supported", other)),
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
                    ") { Ok(__rt_s) => Ok::<Rc<str>, Rc<str>>(Rc::<str>::from(__rt_s)), \
                     Err(__e) => Err(Rc::<str>::from(__e.to_string())) })",
                );
            }
            "sub_bytes" => {
                out.push_str("{ let __rt_b = ");
                self.emit_expr(out, eff[0])?;
                out.push_str("; let __rt_n = __rt_b.len() as i64; let __rt_lo = (");
                self.emit_expr(out, eff[1])?;
                out.push_str(").clamp(0, __rt_n); let __rt_hi = (");
                self.emit_expr(out, eff[2])?;
                out.push_str(").clamp(__rt_lo, __rt_n); Rc::<[u8]>::from(&__rt_b[__rt_lo as usize..__rt_hi as usize]) }");
            }
            // I/O de ENTRADA (no determinista → sin oráculo; probado por subproceso, como tests/io_cli.rs).
            // `input() -> Option<string>`: una línea de stdin, sin '\n'/'\r' finales (como la VM); None en EOF.
            "input" => {
                out.push_str(
                    "{ let mut __rt_s = String::new(); match std::io::stdin().read_line(&mut __rt_s) \
                     { Ok(0) | Err(_) => None, Ok(_) => Some(Rc::<str>::from(__rt_s.trim_end_matches(['\\n', '\\r']))) } }",
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
                    "{ let mut __rt_s = String::new(); match std::io::stdin().read_line(&mut __rt_s) \
                     { Ok(0) | Err(_) => None, Ok(_) => __rt_s.trim_end_matches(['\\n', '\\r']).parse::<i64>().ok() } }",
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
                    other => return Err(format!("send on {:?} is not supported", other)),
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
                    other => return Err(format!("recv on {:?} is not supported", other)),
                };
                self.emit_expr(out, eff[0])?;
                out.push_str(".recv()");
                if send_is_tree(&elem) {
                    let inner = self.from_send_expr(&elem, "__rt_x")?;
                    write!(out, ".map(|__rt_x| {})", inner).unwrap();
                } else {
                    out.push_str(from_send_map(&elem));
                }
            }
            // M116: try_recv(ch) -> Received<T> — recepción no bloqueante. Mapea el __TryRecv interno
            // (payload en repr SEND) al enum del prelude Received (repr programa): la rama Got convierte
            // el payload igual que recv; Empty/Closed infieren T de la rama Got.
            "try_recv" => {
                self.needs_concurrency = true;
                let elem = match self.type_of(eff[0])? {
                    Type::Channel(t) => (*t).clone(),
                    other => return Err(format!("try_recv on {:?} is not supported", other)),
                };
                out.push_str("{ match ");
                self.emit_expr(out, eff[0])?;
                out.push_str(".try_recv() { __TryRecv::Got(__rt_x) => Rc::new(Received::Got(");
                if send_is_tree(&elem) {
                    out.push_str(&self.from_send_expr(&elem, "__rt_x")?);
                } else {
                    match normalize_type(&elem) {
                        Type::String => out.push_str("Rc::<str>::from(&*__rt_x)"),
                        Type::Bytes => out.push_str("Rc::<[u8]>::from(&*__rt_x)"),
                        _ => out.push_str("__rt_x"),
                    }
                }
                out.push_str(")), __TryRecv::Empty => Rc::new(Received::Empty), __TryRecv::Closed => Rc::new(Received::Closed) } }");
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
            // M116.1: select_timeout([chs], ms) -> Option<int>. -1 del helper → None; índice → Some(i).
            "select_timeout" => {
                self.needs_concurrency = true;
                out.push_str("{ let __st_i = __ray_select_timeout(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow()[..], ");
                self.emit_expr(out, eff[1])?;
                out.push_str("); if __st_i < 0 { None } else { Some(__st_i) } }");
            }
            // spawn(f) → __ray_spawn(move || {...}) → Task<T> (estado compartido, registrado en el scope
            // activo). scope(f) → __ray_scope(move || {...}): corre el cuerpo y une las tareas de dentro. `f`
            // es una función anónima literal `fn(){}` (captura valores Send, p. ej. canales) O el NOMBRE de
            // una función de nivel superior de aridad 0 (`spawn(worker)` → `move || worker()`; sin captura).
            "spawn" | "scope" => {
                self.needs_concurrency = true;
                let named: Option<String> = match &eff[0].kind {
                    ExprKind::Func(_) => None,
                    ExprKind::Ident(n) if self.funcs.contains_key(n) => {
                        if !self.funcs[n].params.is_empty() {
                            return Err(format!("{} of a function with parameters ('{}')", method, n));
                        }
                        Some(n.clone())
                    }
                    _ => {
                        return Err(format!(
                            "{} only accepts a literal anonymous function or a function name",
                            method
                        ))
                    }
                };
                let ret = match &eff[0].kind {
                    ExprKind::Func(fnexpr) => normalize_type(&fnexpr.return_type),
                    _ => normalize_type(&self.funcs[named.as_ref().unwrap()].ret),
                };
                out.push_str("{ ");
                // El literal captura por `move` los canales/Tasks del ámbito (el CONDUCTO compartido →
                // se CLONAN antes; el closure mueve un clon, el original sigue). Una fn nombrada es
                // top-level y no captura nada.
                let mut captures: Vec<(String, Type, bool)> = Vec::new();
                if named.is_none() {
                    for name in self.in_scope_channels() {
                        write!(out, "let {n} = {n}.clone(); ", n = mangle(&name)).unwrap();
                    }
                    // H21-N5b (solo spawn: cruza de hilo): las demás capturas de HEAP se convierten a
                    // la repr Send FUERA (deep copy) y se reconstruyen DENTRO — la semántica de heap
                    // aislado de la VM (M38): la mutación no se comparte; los canales son el conducto.
                    if method == "spawn" {
                        let mut fn_clones: Vec<String> = Vec::new();
                        if let ExprKind::Func(fnexpr) = &eff[0].kind {
                            let (caps, cls) = self.spawn_captures(&fnexpr.body)?;
                            captures = caps;
                            fn_clones = cls;
                        }
                        for n in &fn_clones {
                            write!(out, "let {n} = {n}.clone(); ", n = mangle(n)).unwrap();
                        }
                        for (i, (name, ty, is_cell)) in captures.iter().enumerate() {
                            let src = if *is_cell {
                                format!("{}.borrow().clone()", mangle(name))
                            } else {
                                format!("{}.clone()", mangle(name))
                            };
                            let conv = self.to_send_expr(ty, &src)?;
                            write!(out, "let __snd_{i} = {conv}; ").unwrap();
                        }
                    }
                }
                let runtime = if method == "spawn" { "__ray_spawn" } else { "__ray_scope" };
                write!(out, "{}(move || ", runtime).unwrap();
                if !captures.is_empty() {
                    out.push_str("{ ");
                    for (i, (name, ty, is_cell)) in captures.iter().enumerate() {
                        let conv = self.from_send_expr(ty, &format!("__snd_{i}"))?;
                        if *is_cell {
                            // la var era una CELDA (mutable capturada): se reconstruye como celda local
                            // — la mutación queda AISLADA en la tarea, como en la VM.
                            write!(out, "let {} = Rc::new(std::cell::RefCell::new({conv})); ", mangle(name)).unwrap();
                        } else {
                            write!(out, "let {} = {conv}; ", mangle(name)).unwrap();
                        }
                    }
                }
                // spawn: el closure corre en OTRO hilo → devuelve la repr SEND (string/bytes → Arc;
                // compuestos → __RaySend); el cuerpo produce la repr del programa, se envuelve. scope
                // corre en el hilo actual → sin conversión.
                let wrap = if method == "spawn" { ret } else { Type::Unit };
                // El cuerpo del literal se emite AQUÍ (no por `emit_fn_expr`), así que hay que
                // registrar sus celdas a mano: una `var` declarada DENTRO del cuerpo y capturada por
                // una closure aún más interna necesita `Rc<RefCell<_>>`, o el `Rc<closure>` que la
                // envuelve intenta mutar a través de un `Rc` (E0596). Va después de
                // `spawn_captures`, que clasifica las capturas con las celdas del ámbito de FUERA.
                let body_cells = match &eff[0].kind {
                    ExprKind::Func(fnexpr) => self.enter_cells(&fnexpr.body),
                    _ => Vec::new(),
                };
                // IDEAS §68: el cuerpo LITERAL se emite como closure inmediatamente invocado
                // `(|| { … })()` — un `return` del usuario retorna de ESA frontera con el tipo
                // raylang, y la conversión Send se aplica al resultado. Sin la frontera, `return;`
                // fija `()` como retorno del closure de hilo y choca con la cola `__RaySend::U`
                // (E0308); lo mismo pasaba con `return s;` (Rc<str>) frente a la cola Arc<str>.
                if send_is_tree(&wrap) && method == "spawn" {
                    let mut tmp = String::new();
                    match &eff[0].kind {
                        ExprKind::Func(fnexpr) => {
                            tmp.push_str("(|| ");
                            self.emit_block(&mut tmp, &fnexpr.body)?;
                            tmp.push_str(")()");
                        }
                        _ => write!(tmp, "{}()", mangle(named.as_ref().unwrap())).unwrap(),
                    }
                    let conv = self.to_send_expr(&wrap, "__rt_r")?;
                    write!(out, "{{ let __rt_r = {tmp}; {conv} }}").unwrap();
                } else {
                    let (pre, suf) = match wrap {
                        Type::String => ("std::sync::Arc::<str>::from(&*", ")"),
                        Type::Bytes => ("std::sync::Arc::<[u8]>::from(&*", ")"),
                        _ => ("", ""),
                    };
                    out.push_str(pre);
                    let iife = !pre.is_empty() && matches!(&eff[0].kind, ExprKind::Func(_));
                    if iife {
                        out.push_str("(|| ");
                    }
                    match &eff[0].kind {
                        ExprKind::Func(fnexpr) => self.emit_block(out, &fnexpr.body)?,
                        _ => write!(out, "{}()", mangle(named.as_ref().unwrap())).unwrap(),
                    }
                    if iife {
                        out.push_str(")()");
                    }
                    out.push_str(suf);
                }
                self.exit_cells(body_cells);
                if !captures.is_empty() {
                    out.push_str(" }");
                }
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
            // H6: panic/assert abortan como la VM — `runtime error: <msg>` + exit 70 (antes: panic de
            // Rust, texto distinto y exit 101).
            "panic" => {
                out.push_str("__ray_rt_err(&*(");
                self.emit_expr(out, eff[0])?;
                out.push_str("))");
            }
            // M130: exit(code) — termina el proceso (flushea salida); diverge como panic.
            "exit" => {
                out.push_str("__ray_exit(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // Aserciones (prelude): mismos MENSAJES que los cuerpos de src/prelude.ray (que la VM ejecuta).
            "assert" => {
                out.push_str("if !(");
                self.emit_expr(out, eff[0])?;
                out.push_str(") { __ray_rt_err(\"assertion failed\") }");
            }
            "assert_eq" => {
                out.push_str("{ let __rt_a = ");
                self.emit_expr(out, eff[0])?;
                out.push_str("; let __rt_b = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; if !(__rt_a == __rt_b) { __ray_rt_err(&format!(\"assert_eq failed: {} != {}\", __rt_a.ray_show(), __rt_b.ray_show())) } }");
            }
            // Orden superior (prelude map/filter/fold SOBRE ARREGLOS) → iteradores de Rust. `__rt_f` liga la
            // closure una vez; `__rt_x`/`__acc` son los elementos/acumulador. La guarda `!name.contains('#')`
            // distingue la función libre `map`/`filter`/`fold` (sobre `[T]`) del MÉTODO `Iter#map`/… (sobre
            // un iterador de primera clase), que cae al despacho de método ordinario (`_ =>`).
            "map" if !name.contains('#') => {
                out.push_str("{ let __rt_f = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; Rc::new(std::cell::RefCell::new(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().map(|__rt_x| __rt_f(__rt_x.clone())).collect::<Vec<_>>())) }");
            }
            "filter" if !name.contains('#') => {
                out.push_str("{ let __rt_f = ");
                self.emit_expr(out, eff[1])?;
                out.push_str("; Rc::new(std::cell::RefCell::new(");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().cloned().filter(|__rt_x| __rt_f(__rt_x.clone())).collect::<Vec<_>>())) }");
            }
            "fold" if !name.contains('#') => {
                out.push_str("{ let __rt_f = ");
                self.emit_expr(out, eff[2])?;
                out.push_str("; ");
                self.emit_expr(out, eff[0])?;
                out.push_str(".borrow().iter().fold(");
                self.emit_expr(out, eff[1])?;
                out.push_str(", |__acc, __rt_x| __rt_f(__acc, __rt_x.clone())) }");
            }
            // Cripto de producción (M43): los primitivos `__*` se interceptan a `ray_runtime::crypto::*`
            // (el MISMO código que la VM → oráculo byte-idéntico) y activan la feature `crypto` de
            // ray-runtime (→ `build_native` genera un proyecto Cargo). NOTA: `method` ya viene SIN el prefijo
            // `__` (línea ~2399 lo recorta), así que se matchea el nombre pelado con guarda `name` empieza
            // por `__` (no interceptar un método de usuario homónimo). El arg `bytes` es `Rc<[u8]>`; `&expr`
            // deref-coerce a `&[u8]`. Retorno `Vec<u8>` → `Rc<[u8]>`; `Option<Vec<u8>>` → `[bytes]` etiquetado
            // (`Rc<RefCell<Vec<Rc<[u8]>>>>`: vacío/único), que el prelude envuelve en `Option`.
            // M126: hasher incremental — el estado vive en ray_runtime::crypto (el MISMO registro
            // por-proceso que usa la VM: mismos digests por construcción). Arreglos etiquetados,
            // como los wrappers de std/crypto esperan.
            "hasher_new" if name.starts_with("__") && !self.exclude.contains("crypto") => {
                self.needs_rt_crypto = true;
                out.push_str("{ match ray_runtime::crypto::hasher_new(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(") { Ok(__rt_id) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\"), Rc::<str>::from(__rt_id.to_string())])), Err(__rt_e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(__rt_e)])) } }");
            }
            "hasher_update" if name.starts_with("__") && !self.exclude.contains("crypto") => {
                self.needs_rt_crypto = true;
                out.push_str("{ match ray_runtime::crypto::hasher_update(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push_str(") { Ok(()) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"ok\")])), Err(__rt_e) => Rc::new(std::cell::RefCell::new(vec![Rc::<str>::from(\"err\"), Rc::<str>::from(__rt_e)])) } }");
            }
            "hasher_final" if name.starts_with("__") && !self.exclude.contains("crypto") => {
                self.needs_rt_crypto = true;
                out.push_str("{ match ray_runtime::crypto::hasher_final(");
                self.emit_expr(out, eff[0])?;
                out.push_str(") { Ok(__rt_d) => Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"ok\"[..]), Rc::<[u8]>::from(__rt_d)])), Err(__rt_e) => Rc::new(std::cell::RefCell::new(vec![Rc::<[u8]>::from(&b\"err\"[..]), Rc::<[u8]>::from(__rt_e.into_bytes())])) } }");
            }
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
            // M114: comparación en tiempo constante — total, como ed25519_verify → bool directo.
            "constant_time_eq" if name.starts_with("__") && !self.exclude.contains("crypto") => {
                self.needs_rt_crypto = true;
                out.push_str("ray_runtime::crypto::constant_time_eq(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // M114: HKDF-SHA256. Va aparte del brazo de abajo porque su último argumento es un `int`
            // (la longitud), no `bytes`: los tres primeros llevan `&` y el cuarto va por valor.
            "hkdf_sha256" if name.starts_with("__") && !self.exclude.contains("crypto") => {
                self.needs_rt_crypto = true;
                out.push_str("{ let __rt_r = ray_runtime::crypto::hkdf_sha256(&");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &");
                self.emit_expr(out, eff[1])?;
                out.push_str(", &");
                self.emit_expr(out, eff[2])?;
                out.push_str(", ");
                self.emit_expr(out, eff[3])?;
                out.push_str("); Rc::new(std::cell::RefCell::new(match __rt_r { Some(__rt_v) => vec![Rc::<[u8]>::from(__rt_v)], None => Vec::new() })) }");
            }
            "ed25519_public_key" | "ed25519_sign" | "chacha20poly1305_seal" | "chacha20poly1305_open"
            | "x25519_public_key" | "x25519_shared_secret"
                if name.starts_with("__") && !self.exclude.contains("crypto") =>
            {
                self.needs_rt_crypto = true;
                let argc = match method {
                    "ed25519_public_key" | "x25519_public_key" => 1,
                    "ed25519_sign" | "x25519_shared_secret" => 2,
                    _ => 4, // chacha seal/open: clave, nonce, aad, dato
                };
                write!(out, "{{ let __rt_r = ray_runtime::crypto::{}(", method).unwrap();
                for i in 0..argc {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push('&');
                    self.emit_expr(out, eff[i])?;
                }
                out.push_str("); Rc::new(std::cell::RefCell::new(match __rt_r { Some(__rt_v) => vec![Rc::<[u8]>::from(__rt_v)], None => Vec::new() })) }");
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
            // M124: el resumen del certificado del peer → arreglo etiquetado plano (el wrapper de
            // std/net construye el struct PeerCert, patrón stat).
            "tls_peer_cert" if name.starts_with("__") && !self.exclude.contains("tls") => {
                self.needs_rt_tls = true;
                out.push_str("__ray_tls_peer_cert(");
                self.emit_expr(out, eff[0])?;
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
            // M115.3: primitivos de metadatos (los llaman los wrappers EMITIDOS fs.stat/fs.chmod).
            // Devuelven el arreglo etiquetado, byte-idéntico a builtins::fs_tagged/chmod_path.
            "stat" if name.starts_with("__") => {
                self.needs_fs_meta = true;
                out.push_str("__ray_stat_prim(&*");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "chmod" if name.starts_with("__") => {
                self.needs_fs_meta = true;
                out.push_str("__ray_chmod_prim(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // M131: normalización Unicode (ray_runtime::unicode tras la feature `unicode`; los
            // wrappers EMITIDOS text.nfc/nfd/nfkc/nfkd llaman aquí con la forma como literal).
            "unicode_normalize" if name.starts_with("__") && !self.exclude.contains("unicode") => {
                self.needs_rt_unicode = true;
                out.push_str("__ray_unicode_normalize(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // M115.4: watch de fs por eventos de kernel (ray_runtime::watch tras la feature
            // `watch`; los wrappers EMITIDOS fs.watch/next_event/next_event_timeout llaman aquí).
            // M145: salida de audio PCM. El write comparte la plomería PipeW de proc_write
            // (mismo despacho, misma espera de escribible) — por eso audio enciende también
            // needs_rt_process: la variante PipeW y __ray_proc_write viven tras ese flag.
            "audio_open" if name.starts_with("__") && !self.exclude.contains("audio") => {
                self.needs_rt_audio = true;
                self.needs_rt_process = true;
                out.push_str("__ray_audio_open(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "audio_write" if name.starts_with("__") && !self.exclude.contains("audio") => {
                self.needs_rt_audio = true;
                self.needs_rt_process = true;
                out.push_str("__ray_proc_write(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "audio_drain" if name.starts_with("__") && !self.exclude.contains("audio") => {
                self.needs_rt_audio = true;
                self.needs_rt_process = true;
                out.push_str("__ray_audio_drain(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // M146: std/ui — ventana + webview (ray_runtime::ui tras la feature `ui`). El flag
            // además cambia la FORMA del main emitido: el programa corre en un hilo del SO y el
            // hilo 1 queda para el loop de AppKit (ver la emisión de `fn main`).
            "ui_open" if name.starts_with("__") && !self.exclude.contains("ui") => {
                self.needs_rt_ui = true;
                out.push_str("__ray_ui_open(&*");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push_str(", ");
                self.emit_expr(out, eff[2])?;
                out.push_str(", ");
                self.emit_expr(out, eff[3])?;
                out.push(')');
            }
            "ui_eval_js" if name.starts_with("__") && !self.exclude.contains("ui") => {
                self.needs_rt_ui = true;
                out.push_str("__ray_ui_eval_js(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "ui_next_event" if name.starts_with("__") && !self.exclude.contains("ui") => {
                self.needs_rt_ui = true;
                out.push_str("__ray_ui_next_event(");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            // M147: std/embed — la tabla horneada (sin feature: helpers inline, vía rustc intacta).
            "embed_read" if name.starts_with("__") => {
                self.needs_embed = true;
                out.push_str("__ray_embed_read(&*");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "embed_list" if name.starts_with("__") => {
                self.needs_embed = true;
                out.push_str("__ray_embed_list()");
            }
            "watch" if name.starts_with("__") && !self.exclude.contains("watch") => {
                self.needs_rt_watch = true;
                out.push_str("__ray_watch(&*");
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "watch_next" if name.starts_with("__") && !self.exclude.contains("watch") => {
                self.needs_rt_watch = true;
                out.push_str("__ray_watch_next(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            // M100: `__run` (procesos del SO) → `__ray_run` (ray_runtime::process, el MISMO código que
            // la VM) + activa `needs_rt_process`. `--without process` es gating de POLÍTICA: sin
            // interceptar, cae al Err de "no soportado" de abajo. Strings/arreglos por referencia
            // (coerción a &str / &Rc<…>), bytes como `&*` (Rc<[u8]> → &[u8]); los escalares, tal cual.
            "run" if name.starts_with("__") && !self.exclude.contains("process") => {
                self.needs_rt_process = true;
                out.push_str("__ray_run(&");
                self.emit_expr(out, eff[0])?;
                for (i, e) in eff.iter().enumerate().skip(1) {
                    out.push_str(match i {
                        1..=3 => ", &",
                        5 => ", &*",
                        _ => ", ",
                    });
                    self.emit_expr(out, e)?;
                }
                out.push(')');
            }
            // M100 v2 (IDEAS §53.9): los primitivos del streaming → __ray_proc_* (mismo gating de
            // política que __run). La firma de __proc_spawn es la de __run sin timeout/max_output.
            "proc_spawn" if name.starts_with("__") && !self.exclude.contains("process") => {
                self.needs_rt_process = true;
                out.push_str("__ray_proc_spawn(&");
                self.emit_expr(out, eff[0])?;
                for (i, e) in eff.iter().enumerate().skip(1) {
                    out.push_str(match i {
                        1..=3 => ", &",
                        5 => ", &*",
                        _ => ", ",
                    });
                    self.emit_expr(out, e)?;
                }
                out.push(')');
            }
            // M100 v3: escritura en el stdin de un hijo vivo (`__ray_proc_write` espera a que el
            // pipe sea escribible, como la VM aparcando por interés de escritura).
            "proc_write" if name.starts_with("__") && !self.exclude.contains("process") => {
                self.needs_rt_process = true;
                out.push_str("__ray_proc_write(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", &*");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            "proc_read" | "proc_try_wait" if name.starts_with("__") && !self.exclude.contains("process") => {
                self.needs_rt_process = true;
                write!(out, "__ray_{}(", method).unwrap();
                self.emit_expr(out, eff[0])?;
                out.push(')');
            }
            "proc_kill" if name.starts_with("__") && !self.exclude.contains("process") => {
                self.needs_rt_process = true;
                out.push_str("__ray_proc_kill(");
                self.emit_expr(out, eff[0])?;
                out.push_str(", ");
                self.emit_expr(out, eff[1])?;
                out.push(')');
            }
            _ => {
                // Función de usuario, o llamada a un valor-función (closure) en ámbito: `name(args)`.
                let is_closure = matches!(self.lookup(name), Some(Type::Fn(_, _)));
                if !self.funcs.contains_key(name) && !is_closure {
                    return Err(format!("builtin/function '{}' is not supported in the native backend", name));
                }
                // H21-N5c: los args a params MARCADOS del callee (genéricos Send) se emiten en su
                // forma "enviable": fn nombrada → el fn item pelado; closure literal → sin Rc::new;
                // variable → tal cual (si es un param marcado del llamador, su genérico ya es Send).
                self.emit_user_call_hoisted(out, name, &eff)?;
            }
        }
        Ok(())
    }

    /// Inferencia MÍNIMA del tipo de una expresión del subconjunto — solo lo justo para clasificar
    /// heap-vs-escalar y decidir la concatenación de strings. No sustituye al checker (que ya validó).
    pub(super) fn type_of(&self, e: &Expr) -> Result<Type, String> {
        Ok(match &e.kind {
            ExprKind::Int(_, _) => Type::Int,
            ExprKind::Float(_) => Type::Float,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Str(_) => Type::String,
            ExprKind::Bytes(_) => Type::Bytes,
            ExprKind::Char(_) => Type::Char,
            ExprKind::Ident(n) if n == "std::math::PI" || n == "std::math::E" => Type::Float,
            ExprKind::Ident(n) => {
                // Bindings de patrón en pie (overlay de `arm_type`): se consulta primero, con clon
                // inmediato (el guard del RefCell no debe sobrevivir a la recursión).
                let probed = self.probe_binds.borrow().iter().rev().find_map(|m| m.get(n).cloned());
                if let Some(t) = probed {
                    t
                } else if let Some(t) = self.lookup(n).or_else(|| self.consts.get(n)) {
                    t.clone()
                } else if let Some(s) = self.funcs.get(n) {
                    // Función como valor → su tipo Fn.
                    Type::Fn(s.params.clone(), Box::new(s.ret.clone()))
                } else {
                    return Err(format!("variable '{}' has no known type", n));
                }
            }
            // Función anónima → su firma declarada (los params van anotados). Habilita que la regla
            // de tabla de `spawn`/`scope` (H16) reciba el `Type::Fn` del literal.
            ExprKind::Func(f) => Type::Fn(
                f.params.iter().map(|p| normalize_type(&p.ty)).collect(),
                Box::new(normalize_type(&f.return_type)),
            ),
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
                            .ok_or_else(|| format!("unknown dyn method '{}'", n))?);
                    }
                }
                // Llamada a un CAMPO-closure (`b.f(x)`, espejo del branch de emit_call): el tipo es
                // el retorno del campo función del struct receptor.
                if let Some(r) = recv {
                    if let Ok(rt) = self.type_of(r)
                        && let Type::Struct(sname, _) = self.classify(&rt)
                        && let Some(Type::Fn(_, ret)) = self
                            .struct_fields
                            .get(&sname)
                            .and_then(|fs| fs.iter().find(|(fnm, _)| fnm.as_str() == n))
                            .map(|(_, fty)| normalize_type(fty))
                    {
                        return Ok(self.classify(&ret));
                    }
                }
                // `std::math::*`: abs/min/max preservan el tipo del primer arg (int|float); el resto
                // (sqrt/pow/sin/…) → float. Antes de la ruta genérica (su FnSig lleva params-diccionario
                // que no sabríamos tipar: el arg es `int#less`, un impl del prelude que no emitimos).
                if let Some(mfn) = n.strip_prefix("std::math::") {
                    return Ok(match mfn {
                        "abs" | "min" | "max" => self.type_of(args.first().or(recv).ok_or("math without an argument")?)?,
                        "float_bits" => Type::Int, // la reinterpretación bit a bit devuelve el patrón como int
                        _ => Type::Float,
                    });
                }
                // `std::fs::*`: read_file → Result<string,string>; write_file → Result<int,string>; exists → bool.
                // stat/chmod caen a la ruta genérica (sus wrappers emitidos viven en `funcs`).
                if let Some(ffn) = n.strip_prefix("std::fs::")
                    && !matches!(ffn, "stat" | "chmod" | "watch" | "next_event" | "next_event_timeout")
                {
                    return Ok(match ffn {
                        "read_file" => Type::Enum("Result".into(), vec![Type::String, Type::String]),
                        "read_file_bytes" => Type::Enum("Result".into(), vec![Type::Bytes, Type::String]),
                        "write_file" | "open" | "write" | "remove_file" | "mkdir" | "remove_dir"
                        | "rename" | "copy_file" | "file_size" | "mtime" | "write_file_bytes"
                        | "append_file_bytes" | "append_file" | "write_bytes" | "sync" | "unlock" => {
                            Type::Enum("Result".into(), vec![Type::Int, Type::String])
                        }
                        "try_lock" => Type::Enum("Result".into(), vec![Type::Bool, Type::String]),
                        "read_line" => opt_of(Type::String),
                        "read_bytes" => Type::Enum("Result".into(), vec![opt_of(Type::Bytes), Type::String]),
                        "seek" => Type::Enum("Result".into(), vec![Type::Int, Type::String]),
                        "list_dir" => Type::Enum(
                            "Result".into(),
                            vec![Type::Array(Box::new(Type::String)), Type::String],
                        ),
                        "exists" | "is_dir" | "is_file" => Type::Bool,
                        other => return Err(format!("std::fs::{} is not supported", other)),
                    });
                }
                // std::time: now/monotonic → int; sleep → unit. std::random: next → float; below → int;
                // seed → unit.
                // Solo las funciones-primitivo; el resto de std/time|random (raylang puro) → ruta genérica.
                match n {
                    "std::time::now" | "std::time::monotonic" | "std::time::monotonic_nanos"
                    | "std::random::below" => return Ok(Type::Int),
                    "std::time::sleep" | "std::random::seed" => return Ok(Type::Unit),
                    "std::random::next" => return Ok(Type::Float),
                    "std::net::local_port" => return Ok(Type::Int),
                    "std::net::set_read_timeout" => return Ok(Type::Unit),
                    "std::net::tcp_connect" | "std::net::tcp_connect_timeout" | "std::net::tcp_listen" | "std::net::tcp_accept"
                    | "std::net::socket_write" | "std::net::socket_write_bytes" | "std::net::shutdown_write" => {
                        return Ok(Type::Enum("Result".into(), vec![Type::Int, Type::String]))
                    }
                    "std::net::socket_read" | "std::net::peer_addr" => {
                        return Ok(Type::Enum("Result".into(), vec![Type::String, Type::String]))
                    }
                    "std::net::socket_read_bytes" => {
                        return Ok(Type::Enum("Result".into(), vec![Type::Bytes, Type::String]))
                    }
                    _ => {}
                }
                let _ = &args;
                // Como en emit_call: un método de trait sobre un tipo de usuario/módulo cuyo nombre
                // pelado coincide con un builtin (`Store#get`, `Store#keys`…) vive en `funcs` (su def
                // se emite) y gana sobre los brazos manuales por nombre pelado — `method = ""` lo manda
                // directo al brazo `_` (función de usuario). Igual un closure local que sombree el
                // nombre. Las claves CORE (`Option#unwrap_or`…) siguen por sus brazos nativos.
                let user_call = matches!(self.lookup(n), Some(Type::Fn(_, _)))
                    || (self.funcs.contains_key(n)
                        && !(n.contains('#') && is_core_impl_key(n.split('#').next().unwrap_or(""))));
                let method = if user_call { "" } else { n.rsplit('#').next().unwrap_or(n).trim_start_matches("__") };
                // Receptor efectivo (UFCS o primer argumento), para métodos cuyo tipo depende de él.
                let recv0 = recv.or_else(|| args.first());
                // H16: el tipo de retorno de un builtin lo aporta su regla `check` de la tabla
                // `BUILTINS` (L1) — un solo lugar de verdad, como checker/VM/intérprete. Se consulta
                // por nombre EXACTO (público `join`/`spawn`/… o primitivo `__sha256`/… tal como llega
                // el sitio) y, para un método manglado `Tipo#m`, por el nombre pelado (`int#to_string`
                // → `to_string`). Guardas: un nombre definido por el USUARIO (override, método de
                // trait: vive en `funcs`) o un closure local en ámbito ganan sobre la tabla; y si la
                // regla no casa (colisión de nombre de método), se sigue por el camino manual. Los
                // WRAPPERS del prelude (get/recv/parse_int/try_join…, que reenvasan `[T]` →
                // Option/Result) no están en la tabla → brazos manuales de abajo.
                if !self.funcs.contains_key(n) && self.lookup(n).is_none() {
                    let table_name = if n.contains('#') { method } else { n };
                    if let Some(b) = crate::builtins::lookup(table_name) {
                        let eff: Vec<&Expr> = recv.into_iter().chain(args.iter()).collect();
                        if let Ok(ats) = eff.iter().map(|a| self.type_of(a)).collect::<Result<Vec<_>, _>>()
                            && let Ok(ret) = (b.check)(&ats)
                        {
                            return Ok(self.classify(&ret));
                        }
                    }
                }
                match method {
                    // try_join(t) → Result<T, string> (H21-N2; wrapper sobre `__task_failed`).
                    "try_join" => match recv0.map(|e| self.type_of(e)).transpose()? {
                        Some(Type::Task(t)) => Type::Enum("Result".into(), vec![self.classify(&t), Type::String]),
                        other => return Err(format!("try_join expects a Task, got {:?}", other)),
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
                    // recv(ch) → Option<T>: wrapper del prelude sobre `__recv` (que devuelve `[T]`).
                    "recv" => match self.type_of(recv0.ok_or("recv without a channel")?)? {
                        Type::Channel(t) => opt_of(*t),
                        other => return Err(format!("recv on {:?} is not supported", other)),
                    },
                    // M116.1: select_timeout([chs], ms) -> Option<int>.
                    "select_timeout" => opt_of(Type::Int),
                    "push" | "insert" | "assert" | "assert_eq" => Type::Unit,
                    "char_from_code" => opt_of(Type::Char),
                    // Más string builtins como métodos manglados (`string#trim` → "trim"; sus filas de
                    // tabla son los primitivos `__trim`/…, que no reenvasan pero cambian de nombre).
                    "trim" | "to_upper" | "to_lower" | "repeat" | "replace" | "substring" => Type::String,
                    "starts_with" | "ends_with" | "contains" => Type::Bool,
                    "index_of" => opt_of(Type::Int), // índice de subcadena → Option<int>

                    "split" => Type::Array(Box::new(Type::String)),
                    "chars" => Type::Array(Box::new(Type::Char)),
                    "contains_key" => Type::Bool,
                    // get_or → V (desenvuelto); get/remove → Option<V> (para match/`?`); keys→[K]; values→[V].
                    "get_or" => match self.type_of(recv0.ok_or("get_or without a receiver")?)? {
                        Type::Map(_, v) => *v,
                        other => return Err(format!("get_or on {:?} is not supported", other)),
                    },
                    "get" | "remove" => match self.type_of(recv0.ok_or("get without a receiver")?)? {
                        Type::Map(_, v) => opt_of(*v),
                        other => return Err(format!("get on {:?} is not supported", other)),
                    },
                    "keys" => match self.type_of(recv0.ok_or("keys without a receiver")?)? {
                        Type::Map(k, _) => Type::Array(k),
                        other => return Err(format!("keys on {:?} is not supported", other)),
                    },
                    "values" => match self.type_of(recv0.ok_or("values without a receiver")?)? {
                        Type::Map(_, v) => Type::Array(v),
                        other => return Err(format!("values on {:?} is not supported", other)),
                    },
                    "sort" => self.type_of(recv0.ok_or("sort without a receiver")?)?,
                    // reverse conserva el tipo del arreglo; pop da Option<T>; position, Option<int>.
                    "reverse" => self.type_of(recv0.ok_or("reverse without a receiver")?)?,
                    "pop" => match self.type_of(recv0.ok_or("pop without a receiver")?)? {
                        Type::Array(elem) => opt_of((*elem).clone()),
                        other => return Err(format!("pop on {:?} is not supported", other)),
                    },
                    "position" => opt_of(Type::Int),
                    // (Cripto/TLS/SQLite/UDP: sus sitios llegan con el nombre primitivo `__sha256`/… y
                    // los tipa la tabla en el fast-path de arriba.)
                    // unwrap_or/unwrap desenvuelven un Option<T>/Result<T,E> → T.
                    "unwrap_or" | "unwrap" => {
                        unwrapped(&self.type_of(recv0.ok_or("unwrap without a receiver")?)?)
                    }
                    // Orden superior: map(xs,f) → [ret(f)]; filter(xs,f) → [elem(xs)]; fold(xs,init,f) → ret(f).
                    // Guarda `!n.contains('#')`: la función libre sobre `[T]`; `Iter#map`/… (método) cae al `_`.
                    "map" if !n.contains('#') => match self.type_of(effargs(recv, args, 1)?)? {
                        Type::Fn(_, r) => Type::Array(r),
                        other => return Err(format!("map with a non-function f {:?}", other)),
                    },
                    "filter" if !n.contains('#') => self.type_of(effargs(recv, args, 0)?)?,
                    "fold" if !n.contains('#') => match self.type_of(effargs(recv, args, 2)?)? {
                        Type::Fn(_, r) => *r,
                        other => return Err(format!("fold with a non-function f {:?}", other)),
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
                        } else if matches!(n, "min" | "max") {
                            // Terminales `min`/`max` de iterador: funciones del prelude con bound
                            // `T: Ord` cuya DEFINICIÓN se salta (is_handled_builtin) y no tiene brazo
                            // en emit_call. (M120 emitió los diccionarios `int#less` — los genéricos
                            // de usuario acotados por Ord ya compilan —, así que soportarlos hoy sería
                            // dejar de saltar sus defs; pendiente.) Se informa como lo que es.
                            return Err(format!(
                                "builtin/function '{}' is not supported in the native backend \
                                 (iterator terminal with an Ord bound)", n));
                        } else {
                            return Err(format!("unknown return type of '{}'", n));
                        }
                    }
                }
            }
            ExprKind::If { then_branch, .. } => match &then_branch.tail {
                Some(t) => self.type_of(t)?,
                None => Type::Unit,
            },
            ExprKind::Block(b) => {
                // M120: los `let` del bloque se registran en el overlay ANTES de tipar el tail — un
                // bloque en posición de ARGUMENTO se tipa sin haberse emitido, así que sus locales no
                // están en `scopes`. El caso que lo cazó (harness diferencial): el lowering de un
                // despacho dyn envuelve la llamada en `{ let __dynrecv#N = r; (t.m)(t.data) }`, y
                // `print(x.m())` moría con "unknown return type" al no conocer `__dynrecv#N`.
                self.probe_binds.borrow_mut().push(HashMap::new());
                let result = (|| -> Result<Type, String> {
                    for st in &b.statements {
                        match &st.kind {
                            StmtKind::Let { name, ty, value, .. } => {
                                let t = match ty {
                                    Some(t) => normalize_type(t),
                                    None => self.type_of(value)?,
                                };
                                self.probe_binds.borrow_mut().last_mut().expect("just pushed").insert(name.clone(), t);
                            }
                            StmtKind::LetTuple { names, value, .. } => {
                                if let Type::Tuple(ts) = self.type_of(value)? {
                                    let mut overlay = self.probe_binds.borrow_mut();
                                    let top = overlay.last_mut().expect("just pushed");
                                    for (nm, t) in names.iter().zip(ts) {
                                        if let Some(nm) = nm {
                                            top.insert(nm.clone(), t);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    match &b.tail {
                        Some(t) => self.type_of(t),
                        None => Ok(Type::Unit),
                    }
                })();
                self.probe_binds.borrow_mut().pop();
                result?
            }
            ExprKind::While { .. } => Type::Unit,
            ExprKind::ArrayLit(elems) => {
                let elem = match elems.first() {
                    Some(e) => self.type_of(e)?,
                    None => return Err("empty array literal without an annotation".into()),
                };
                Type::Array(Box::new(elem))
            }
            ExprKind::Index { array, index } => match self.type_of(array)? {
                Type::Array(t) => *t,
                Type::String => Type::Char, // s[i] → char
                Type::Bytes => Type::Int, // b[i] → el octeto como int
                Type::Tuple(ts) => {
                    let i = match &index.kind {
                        ExprKind::Int(n, _) => *n as usize,
                        _ => return Err("non-literal tuple index".into()),
                    };
                    ts.get(i).cloned().ok_or("tuple index out of range")?
                }
                other => return Err(format!("indexing {:?} is not supported", other)),
            },
            ExprKind::TupleLit(elems) => {
                Type::Tuple(elems.iter().map(|e| self.type_of(e)).collect::<Result<_, _>>()?)
            }
            ExprKind::StructLit { name, fields } => {
                // Inferir los ARGS DE TIPO desde los campos (como el checker): unificar cada tipo de campo
                // declarado (con params) contra el tipo del valor → fija los params. Sin esto, un literal
                // genérico daba args vacíos y un acceso anidado (`b.v.v` con `Box<Box<[int]>>`) no podía
                // sustituir el param → "campo desconocido en T" (H9).
                let tparams = self.struct_tparams.get(name).cloned().unwrap_or_default();
                if tparams.is_empty() {
                    Type::Struct(name.clone(), vec![])
                } else {
                    let decl = self.struct_fields.get(name).cloned().unwrap_or_default();
                    let mut subst = HashMap::new();
                    for (fname, fval) in fields {
                        if let Some((_, fty)) = decl.iter().find(|(n, _)| n == fname) {
                            if let Ok(vty) = self.type_of(fval) {
                                unify(&normalize_type(fty), &vty, &tparams, &mut subst);
                            }
                        }
                    }
                    let args = tparams
                        .iter()
                        .map(|p| subst.get(p).cloned().unwrap_or_else(|| Type::Var(p.clone())))
                        .collect();
                    Type::Struct(name.clone(), args)
                }
            }
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
                    // Enum de usuario: inferir los args de tipo desde el payload (análogo a StructLit, H9).
                    let tparams = self.enum_tparams.get(enum_name).cloned().unwrap_or_default();
                    if tparams.is_empty() {
                        Type::Enum(enum_name.clone(), vec![])
                    } else {
                        let payload = self
                            .enum_variants
                            .get(enum_name)
                            .and_then(|vs| vs.get(variant))
                            .cloned()
                            .unwrap_or_default();
                        let mut subst = HashMap::new();
                        for (pty, aval) in payload.iter().zip(args) {
                            if let Ok(vty) = self.type_of(aval) {
                                unify(&normalize_type(pty), &vty, &tparams, &mut subst);
                            }
                        }
                        let targs = tparams
                            .iter()
                            .map(|p| subst.get(p).cloned().unwrap_or_else(|| Type::Var(p.clone())))
                            .collect();
                        Type::Enum(enum_name.clone(), targs)
                    }
                }
            }
            ExprKind::Try(inner) => unwrapped(&self.type_of(inner)?),
            ExprKind::Field { object, name } => {
                let obj_ty = self.type_of(object)?;
                // Tupla: `t.0` → el tipo del i-ésimo elemento.
                if let Type::Tuple(ts) = &obj_ty {
                    let i: usize = name.parse().map_err(|_| "non-numeric tuple field")?;
                    return ts.get(i).cloned().ok_or_else(|| "tuple field out of range".into());
                }
                let sn = match &obj_ty {
                    Type::Struct(n, _) => n.clone(),
                    other => return Err(format!("field access on {:?} is not supported", other)),
                };
                let fty = self
                    .struct_fields
                    .get(&sn)
                    .and_then(|fs| fs.iter().find(|(f, _)| f == name))
                    .ok_or_else(|| format!("unknown field '{}' on {}", name, sn))?
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
                let scrut_ty = match self.type_of(scrutinee) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        if std::env::var_os("RAYLANG_TRANSPILE_DEBUG").is_some() {
                            eprintln!("[matchtype] scrutinee err: {e}");
                        }
                        None
                    }
                };
                arms.iter()
                    .find_map(|a| self.arm_type(scrut_ty.as_ref(), a))
                    .ok_or("could not infer the type of the match")?
            }
            ExprKind::Cast { ty, .. } => normalize_type(ty),
            ExprKind::MapLit(pairs) => {
                let (k, v) = pairs.first().ok_or("empty Map literal without an annotation")?;
                Type::Map(Box::new(self.type_of(k)?), Box::new(self.type_of(v)?))
            }
            // (Exhaustivo sobre ExprKind, como emit_expr: una variante nueva rompe la compilación aquí.)
        })
    }

}
