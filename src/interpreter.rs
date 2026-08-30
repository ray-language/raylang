//! Intérprete (tree-walking) de raylang.
//!
//! Cuarta y última fase de M1 (DESIGN.md §2, §8): por fin **ejecuta**. Recorre el
//! AST ya verificado por el checker y lo evalúa nodo a nodo. Como el checker ya
//! garantizó que el programa está bien tipado, el intérprete **confía**: no
//! re-verifica tipos, y las combinaciones imposibles se marcan con `unreachable!`
//! (si alguna saltara, sería un bug del checker, no del programa del usuario).
//!
//! ## Tres ideas nuevas
//!
//! 1. **Valores en runtime** (`Value`): lo que las expresiones producen al
//!    ejecutarse (un `Int`, un `Bool`, …). Distinto de `Type`, que es lo que el
//!    checker razonaba en estático.
//!
//! 2. **Entorno con marcos**: las variables viven en una pila de ámbitos. Cada
//!    *llamada a función* arranca con una pila **nueva** (solo sus parámetros y
//!    locales), de modo que una función no ve las variables de quien la llamó:
//!    eso es *scoping léxico*. Guardamos y restauramos la pila al entrar/salir.
//!
//! 3. **`return` como señal**: en un intérprete de árbol, `return` es un salto que
//!    debe abandonar varios bloques de golpe. Lo modelamos como un `Err(Flow::
//!    Return(v))` que se propaga hacia arriba (con `?`) hasta que la llamada a la
//!    función lo "atrapa" y lo convierte en el valor de retorno.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem;
use std::rc::Rc;

use crate::ast::*;
use crate::bytecode::MathFn;
use crate::runtime::{
    eval_const_literal, make_uint, program_args, Cell, Closure, EnumInstance, MapKey,
    RuntimeError, StructInstance, Value, MAX_CALL_DEPTH,
};


/// Lo que interrumpe la evaluación normal de una expresión/sentencia. Usamos el
/// canal de error de `Result` para DOS cosas:
///   - `Return(v)`: un `return` que se está propagando hacia el borde de la función.
///   - `Error(e)`: un error de ejecución real, que se propaga hasta el tope.
/// Es un truco clásico: ambos "desenrollan" la pila de llamadas de Rust con `?`,
/// pero se tratan distinto en el borde de la función.
enum Flow {
    Return(Value),
    Error(RuntimeError),
    /// Una **llamada en cola** (M13.3b): en vez de recurrir, se propaga hasta `call_body`, que la
    /// ejecuta en un **bucle** (trampolín) reutilizando el marco lógico → recursión de cola en O(1)
    /// de pila de Rust. Es el análogo del opcode `TailCall` de la VM (mismo criterio de posición de
    /// cola, así los dos motores coinciden). Lleva el índice de la función, los argumentos ya
    /// evaluados y el entorno capturado (vacío salvo para una closure).
    TailCall { index: usize, args: Vec<Value>, captured: Vec<(String, Cell)> },
}

/// Resultado de evaluar una expresión: un valor, o una interrupción de flujo.
type EvalResult = Result<Value, Flow>;

/// Punto de entrada: ejecuta el programa llamando a `main` y devuelve su valor.
pub fn run(program: &Program) -> Result<Value, RuntimeError> {
    Interpreter::new(program).run_main()
}


/// M27.4: convierte `v` al tipo `ty` (`as`). El checker garantiza una combinación válida; solo el
/// `int as char` con un code point inválido puede fallar en runtime.
fn cast_value(v: Value, ty: &Type, line: usize, col: usize) -> Result<Value, Flow> {
    match (&v, ty) {
        (Value::Int(n), Type::Float) => Ok(Value::Float(*n as f64)),
        (Value::Float(f), Type::Int) => Ok(Value::Int(*f as i64)), // trunca hacia cero
        (Value::Char(c), Type::Int) => Ok(Value::Int(*c as i64)),  // code point
        (Value::Int(n), Type::Char) => match u32::try_from(*n).ok().and_then(char::from_u32) {
            Some(c) => Ok(Value::Char(c)),
            None => Err(Flow::Error(RuntimeError {
                msg: format!("{} is not a valid Unicode character for 'as char'", n),
                line,
                col,
                trace: Vec::new(),
            })),
        },
        // M28.3: conversiones de/hacia enteros sin signo con tamaño. Enmascaran al ancho destino.
        (Value::Int(n), Type::UInt(w)) => Ok(make_uint(*n as u64, *w)),
        (Value::UInt(n, _), Type::Int) => Ok(Value::Int(*n as i64)),
        (Value::UInt(n, _), Type::UInt(w)) => Ok(make_uint(*n, *w)),
        (Value::UInt(n, _), Type::Float) => Ok(Value::Float(*n as f64)),
        (Value::Float(f), Type::UInt(w)) => Ok(make_uint(*f as i64 as u64, *w)),
        (Value::Char(c), Type::UInt(w)) => Ok(make_uint(*c as u64, *w)),
        // Identidad (int as int, etc.): sin cambio.
        _ => Ok(v),
    }
}


struct Interpreter<'a> {
    /// Todas las funciones del programa, por nombre (las referencias viven mientras
    /// viva el `program`, de ahí el lifetime `'a`).
    functions: HashMap<String, &'a Function>,
    /// Las funciones nombradas, en orden de declaración (para resolver un
    /// `Value::Function(idx)` con `idx < N`).
    named: &'a [Function],
    /// Nombre de función → su índice en `named` (para usar el nombre como valor).
    named_index: HashMap<String, usize>,
    /// Las funciones anónimas, indexadas por su `id` (M4.1).
    anon: Vec<&'a FnExpr>,
    /// Definiciones de struct, por nombre (para construir literales en orden).
    structs: HashMap<String, &'a StructDef>,
    /// Constantes de nivel superior (M27.5): nombre → su valor (ya evaluado del literal).
    consts: HashMap<String, Value>,
    /// Pila de ámbitos de la función en ejecución. El último es el más interno.
    /// Cada variable es una **celda** compartible (M4.2): así una closure puede
    /// capturarla por referencia.
    scopes: Vec<HashMap<String, Cell>>,
    /// Profundidad de llamadas anidadas actualmente activas (M13.3a). Se incrementa
    /// al entrar en `call_body` y se decrementa al salir; al alcanzar `MAX_CALL_DEPTH`
    /// se corta con un error en vez de desbordar la pila de Rust.
    depth: usize,
    /// M79: pila de llamadas activas para la traza de errores. Cada entrada es el
    /// nombre de la función LLAMADA y la posición del SITIO de llamada en el llamador
    /// (así la posición de un llamador en la traza es la de su llamada en vuelo, como
    /// deriva la VM de `lines[ip-1]`). La mantiene `call_body` (push/pop; el trampolín
    /// TCO renombra la cima sin apilar, espejo del `TailCall` de la VM).
    call_stack: Vec<crate::runtime::TraceFrame>,
    /// Funciones externas (M41, FFI): nombre → descriptor de llamada. Una llamada a uno de estos
    /// nombres se despacha a `ffi::call` (a la librería C) en vez de ejecutar un cuerpo raylang.
    externs: HashMap<String, crate::ffi::ExternDesc>,
}

impl<'a> Interpreter<'a> {
    fn new(program: &'a Program) -> Self {
        let mut functions = HashMap::new();
        let mut named_index = HashMap::new();
        for (i, f) in program.functions.iter().enumerate() {
            functions.insert(f.name.clone(), f);
            named_index.insert(f.name.clone(), i);
        }
        let mut structs = HashMap::new();
        for s in &program.structs {
            structs.insert(s.name.clone(), s);
        }
        let mut consts = HashMap::new();
        for c in &program.consts {
            consts.insert(c.name.clone(), eval_const_literal(&c.value));
        }
        let mut externs = HashMap::new();
        for e in &program.externs {
            if let Some(d) = crate::ffi::desc_of(e) {
                externs.insert(e.name.clone(), d);
            }
        }
        Interpreter {
            functions,
            named: &program.functions,
            named_index,
            anon: collect_fn_exprs(program),
            structs,
            consts,
            scopes: Vec::new(),
            depth: 0,
            call_stack: Vec::new(),
            externs,
        }
    }

    fn run_main(&mut self) -> Result<Value, RuntimeError> {
        // El checker ya garantizó que 'main' existe.
        let main = *self.functions.get("main").expect("the checker guarantees 'main'");
        match self.call_function(main, Vec::new(), 0, 0) {
            Ok(v) => Ok(v),
            Err(Flow::Error(e)) => Err(e),
            // Un 'return' nunca debería escapar de call_function, pero por si acaso.
            Err(Flow::Return(v)) => Ok(v),
            // Una llamada en cola siempre la consume el trampolín de call_body; no escapa.
            Err(Flow::TailCall { .. }) => unreachable!("a tail call does not escape call_body"),
        }
    }

    /// Ejecuta una función nombrada con sus argumentos ya evaluados (sin entorno
    /// capturado). `(call_line, call_col)` es la posición del sitio de llamada (M79,
    /// para la traza; `(0, 0)` para `main`, que nadie llama).
    fn call_function(&mut self, func: &'a Function, args: Vec<Value>, call_line: usize, call_col: usize) -> EvalResult {
        self.call_body(&func.params, &func.body, args, &[], &func.name, call_line, call_col)
    }

    /// Despacha una llamada a través de un índice de la tabla de funciones: `idx`
    /// menor que el número de funciones nombradas es una nombrada; el resto, una
    /// anónima (`idx - N`). `captured` es el entorno de la closure (vacío si es una
    /// función sin captura). (M4.1/M4.2)
    fn call_index(&mut self, idx: usize, args: Vec<Value>, captured: &[(String, Cell)], call_line: usize, call_col: usize) -> EvalResult {
        let n = self.named.len();
        if idx < n {
            let name = self.named[idx].name.clone();
            self.call_body(&self.named[idx].params, &self.named[idx].body, args, captured, &name, call_line, call_col)
        } else {
            let fe = self.anon[idx - n];
            // El mismo nombre que da el compilador a las anónimas (la traza debe
            // coincidir entre motores).
            let name = format!("<fn#{}>", idx - n);
            self.call_body(&fe.params, &fe.body, args, captured, &name, call_line, call_col)
        }
    }

    /// Ejecuta el cuerpo de una función (nombrada o anónima) con sus argumentos y su
    /// entorno capturado.
    fn call_body(&mut self, params: &'a [Param], body: &'a Block, args: Vec<Value>, captured: &[(String, Cell)], name: &str, call_line: usize, call_col: usize) -> EvalResult {
        // Guardia de recursión (M13.3a): si ya hay `MAX_CALL_DEPTH` llamadas activas,
        // cortamos con un error limpio en vez de seguir recurriendo sobre la pila de
        // Rust (que acabaría en segfault). La comprobación es ANTES de incrementar,
        // igual que la VM mira `frames.len()` antes de empujar el marco → ambos motores
        // coinciden en la frontera. La posición es la del cuerpo de la función.
        if self.depth >= MAX_CALL_DEPTH {
            return Err(runtime_error(
                body.line,
                body.col,
                "stack overflow (recursion too deep)",
            ));
        }
        self.depth += 1;
        // M79: marco de la traza — el nombre del llamado + la posición del sitio de
        // llamada. Se apila DESPUÉS de la guardia (el desbordamiento se atribuye al
        // llamador, como la VM, cuyo `Call` no llegó a empujar el marco).
        self.call_stack.push(crate::runtime::TraceFrame {
            name: name.to_string(),
            line: call_line,
            col: call_col,
        });

        // Scoping léxico: la función arranca con una pila de ámbitos NUEVA, no la de
        // quien llama. Guardamos la actual y la restauramos al volver.
        let saved = mem::take(&mut self.scopes);

        // Trampolín de llamadas en cola (M13.3b): el cuerpo se evalúa en posición de cola
        // (`eval_tail_block`). Si su resultado es una `Flow::TailCall`, en vez de recurrir se
        // **reemplaza** la función actual y se vuelve a iterar → la recursión de cola no crece la
        // pila de Rust ni `depth`. Es el análogo del opcode `TailCall` de la VM (que reutiliza el
        // marco). Una función sin llamadas en cola itera una sola vez.
        let mut cur_params = params;
        let mut cur_body = body;
        let mut cur_captured: Vec<(String, Cell)> = captured.to_vec();
        let mut cur_args = args;

        let result = loop {
            self.scopes.clear();
            // Ámbito base: las celdas capturadas (compartidas con su origen).
            let mut base: HashMap<String, Cell> = HashMap::new();
            for (name, cell) in &cur_captured {
                base.insert(name.clone(), cell.clone());
            }
            self.scopes.push(base);
            // Ámbito de los parámetros, encima (tapan capturas con el mismo nombre).
            self.scopes.push(HashMap::new());
            for (param, arg) in cur_params.iter().zip(mem::take(&mut cur_args)) {
                self.define(&param.name, arg);
            }

            match self.eval_tail_block(cur_body) {
                Ok(v) => break Ok(v),                          // el cuerpo cayó a su valor final
                Err(Flow::Return(v)) => break Ok(v),           // un 'return' temprano: ese es el valor
                // Un error real se propaga; el `call_body` más interno compone la traza
                // (M79) ANTES de despilar (la pila aún incluye este marco). El chequeo
                // `is_empty` evita que los envolventes la re-rellenen al desenrollar.
                Err(Flow::Error(mut e)) => {
                    if e.trace.is_empty() {
                        e.trace = self.compose_trace(e.line, e.col);
                    }
                    break Err(Flow::Error(e));
                }
                // Llamada en cola: reemplaza la función actual y reitera (no recurre).
                Err(Flow::TailCall { index, args, captured }) => {
                    let (p, b) = self.params_body_of(index);
                    // M79: el marco se REUTILIZA (como el `TailCall` de la VM, que
                    // reemplaza `frames[top].function`): se renombra la cima sin apilar;
                    // la posición del sitio de llamada original se conserva.
                    let new_name = self.name_of_index_owned(index);
                    if let Some(top) = self.call_stack.last_mut() {
                        top.name = new_name;
                    }
                    cur_params = p;
                    cur_body = b;
                    cur_captured = captured;
                    cur_args = args;
                }
            }
        };

        self.scopes = saved; // restaurar el entorno de quien llama
        self.depth -= 1; // salimos de esta llamada
        self.call_stack.pop(); // M79: el marco de la traza sale con la llamada
        result
    }

    /// M79: el nombre de la función con índice `index` (nombrada, o `<fn#id>` para una
    /// anónima — la misma grafía que les da el compilador, para que la traza coincida
    /// entre motores).
    fn name_of_index_owned(&self, index: usize) -> String {
        let n = self.named.len();
        if index < n { self.named[index].name.clone() } else { format!("<fn#{}>", index - n) }
    }

    /// M79: compone la traza de llamadas en el momento del error. La entrada 0 es el
    /// marco más interno (su nombre + la posición del error); cada entrada siguiente
    /// es un llamador con la posición de su llamada en vuelo (que es, precisamente,
    /// el sitio de llamada guardado en el marco de ENCIMA — la misma derivación que
    /// hace la VM con `lines[frame.ip - 1]`).
    fn compose_trace(&self, err_line: usize, err_col: usize) -> Vec<crate::runtime::TraceFrame> {
        let n = self.call_stack.len();
        let mut trace = Vec::with_capacity(n);
        if n == 0 {
            return trace;
        }
        trace.push(crate::runtime::TraceFrame {
            name: self.call_stack[n - 1].name.clone(),
            line: err_line,
            col: err_col,
        });
        for k in 1..n {
            trace.push(crate::runtime::TraceFrame {
                name: self.call_stack[n - 1 - k].name.clone(),
                line: self.call_stack[n - k].line,
                col: self.call_stack[n - k].col,
            });
        }
        trace
    }

    /// Los parámetros y el cuerpo de la función con índice `index` (M13.3b, para el trampolín).
    /// `index < N` es una función nombrada; el resto, una anónima (`index - N`).
    fn params_body_of(&self, index: usize) -> (&'a [Param], &'a Block) {
        let n = self.named.len();
        if index < n {
            let named: &'a [Function] = self.named;
            (&named[index].params, &named[index].body)
        } else {
            let fe: &'a FnExpr = self.anon[index - n];
            (&fe.params, &fe.body)
        }
    }

    /// Evalúa un bloque en **posición de cola** (M13.3b): igual que `exec_block`, pero su
    /// expresión final se evalúa con `eval_tail` (una llamada ahí produce `Flow::TailCall`).
    fn eval_tail_block(&mut self, block: &'a Block) -> EvalResult {
        self.scopes.push(HashMap::new());
        let mut interrupted: Option<Flow> = None;
        for stmt in &block.statements {
            if let Err(flow) = self.exec_stmt(stmt) {
                interrupted = Some(flow);
                break;
            }
        }
        let result = match interrupted {
            Some(flow) => Err(flow),
            None => match &block.tail {
                Some(tail) => self.eval_tail(tail),
                None => Ok(Value::Unit),
            },
        };
        self.scopes.pop();
        result
    }

    /// Evalúa una expresión en **posición de cola** (M13.3b). Una llamada a función/closure aquí
    /// produce `Flow::TailCall` (la ejecuta el trampolín de `call_body` sin recurrir); `if`/`match`/
    /// bloque propagan la posición de cola a sus ramas; lo demás delega en `eval_expr`. Las reglas
    /// son las MISMAS que el *peephole* de la VM (un `Call` seguido de `Return`), así los dos
    /// motores coinciden en qué es una llamada en cola.
    fn eval_tail(&mut self, expr: &'a Expr) -> EvalResult {
        match &expr.kind {
            ExprKind::Call { callee, args } => {
                // Camino directo por nombre (como `eval_call`), pero en cola.
                if let ExprKind::Ident(name) = &callee.kind {
                    let is_local = self.lookup_opt(name).is_some();
                    if !is_local {
                        // Los builtins son hoja (no recurren): se ejecutan normal (incl. `panic`).
                        if crate::builtins::is_builtin(name) {
                            return self.eval_call(callee, args);
                        }
                        if let Some(&idx) = self.named_index.get(name.as_str()) {
                            let mut values = Vec::with_capacity(args.len());
                            for arg in args {
                                values.push(self.eval_expr(arg)?);
                            }
                            return Err(Flow::TailCall { index: idx, args: values, captured: Vec::new() });
                        }
                    }
                }
                // Camino indirecto: el callee produce un valor-función.
                let callee_val = self.eval_expr(callee)?;
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval_expr(arg)?);
                }
                match callee_val {
                    Value::Function(idx) => Err(Flow::TailCall { index: idx, args: values, captured: Vec::new() }),
                    Value::Closure(c) => Err(Flow::TailCall { index: c.index, args: values, captured: c.upvalues.clone() }),
                    _ => unreachable!("the checker guarantees a function"),
                }
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                if self.eval_bool(cond)? {
                    self.eval_tail_block(then_branch)
                } else if let Some(else_e) = else_branch {
                    self.eval_tail(else_e)
                } else {
                    Ok(Value::Unit)
                }
            }
            ExprKind::Block(b) => self.eval_tail_block(b),
            ExprKind::Match { scrutinee, arms } => {
                let value = self.eval_expr(scrutinee)?;
                for arm in arms {
                    if let Some(binds) = match_pattern(&arm.pattern, &value) {
                        self.scopes.push(HashMap::new());
                        for (name, v) in binds {
                            self.define(&name, v);
                        }
                        // Guarda (M40.1a): si no evalúa a `true`, el brazo no casa → siguiente
                        // (soltando su ámbito). Un error de la guarda se propaga tras soltarlo.
                        if let Some(g) = &arm.guard {
                            match self.eval_expr(g) {
                                Ok(Value::Bool(true)) => {}
                                Ok(_) => { self.scopes.pop(); continue; }
                                Err(e) => { self.scopes.pop(); return Err(e); }
                            }
                        }
                        let result = self.eval_tail(&arm.body);
                        self.scopes.pop();
                        return result;
                    }
                }
                Err(Flow::Error(RuntimeError {
                    msg: "no match branch matched (should not happen)".into(),
                    line: scrutinee.line,
                    col: scrutinee.col,
                    trace: Vec::new(),
                }))
            }
            // Cualquier otra forma no es una llamada en cola: evaluación normal.
            _ => self.eval_expr(expr),
        }
    }

    /// Ejecuta un bloque en su propio ámbito y devuelve su valor (el de la
    /// expresión final, o `Unit`). Propaga `return`/errores que ocurran dentro.
    fn exec_block(&mut self, block: &'a Block) -> EvalResult {
        self.scopes.push(HashMap::new());

        let mut interrupted: Option<Flow> = None;
        for stmt in &block.statements {
            if let Err(flow) = self.exec_stmt(stmt) {
                interrupted = Some(flow);
                break;
            }
        }

        let result = match interrupted {
            Some(flow) => Err(flow),
            None => match &block.tail {
                Some(tail) => self.eval_expr(tail),
                None => Ok(Value::Unit),
            },
        };

        self.scopes.pop();
        result
    }

    /// Ejecuta una sentencia. No produce valor; puede interrumpir el flujo.
    fn exec_stmt(&mut self, stmt: &'a Stmt) -> Result<(), Flow> {
        match &stmt.kind {
            StmtKind::Let { name, value, .. } => {
                let v = self.eval_expr(value)?;
                self.define(name, v);
                Ok(())
            }
            // M27.1: desestructuración de tupla. La tupla es un arreglo en runtime → se liga por índice.
            StmtKind::LetTuple { names, value, .. } => {
                let v = self.eval_expr(value)?;
                let rc = match v {
                    Value::Array(rc) => rc,
                    _ => unreachable!("the checker guarantees a tuple (array)"),
                };
                let elems = rc.borrow();
                for (i, n) in names.iter().enumerate() {
                    if let Some(name) = n {
                        self.define(name, elems[i].clone());
                    }
                }
                Ok(())
            }
            // M27.2: bucle `for`. Cada iteración liga la(s) variable(s) en un ámbito fresco y ejecuta el
            // cuerpo (su valor se descarta; un return/error se propaga).
            StmtKind::For { pat, iter, body } => {
                match iter {
                    ForIter::Range { start, end } => {
                        let s = self.eval_int(start)?;
                        let e = self.eval_int(end)?;
                        let name = match pat { ForPat::Single(n) => n, _ => unreachable!("checker: un name") };
                        let mut i = s;
                        while i < e {
                            self.scopes.push(HashMap::new());
                            self.define(name, Value::Int(i));
                            let r = self.exec_block(body);
                            self.scopes.pop();
                            r?;
                            i += 1;
                        }
                    }
                    ForIter::In(e) => {
                        let v = self.eval_expr(e)?;
                        match v {
                            Value::Array(rc) => {
                                let name = match pat { ForPat::Single(n) => n, _ => unreachable!("checker: un name") };
                                let len = rc.borrow().len();
                                for idx in 0..len {
                                    let item = rc.borrow()[idx].clone();
                                    self.scopes.push(HashMap::new());
                                    self.define(name, item);
                                    let r = self.exec_block(body);
                                    self.scopes.pop();
                                    r?;
                                }
                            }
                            Value::Str(s) => {
                                let name = match pat { ForPat::Single(n) => n, _ => unreachable!("checker: un name") };
                                for c in s.chars() {
                                    self.scopes.push(HashMap::new());
                                    self.define(name, Value::Char(c));
                                    let r = self.exec_block(body);
                                    self.scopes.pop();
                                    r?;
                                }
                            }
                            Value::Map(rc) => {
                                let (kn, vn) = match pat {
                                    ForPat::Tuple(names) => (names[0].clone(), names[1].clone()),
                                    _ => unreachable!("checker: one tupla (k, v)"),
                                };
                                // Clave ordenada (determinista, como el builtin `keys`) → casa con la VM.
                                let mut ks: Vec<MapKey> = rc.borrow().keys().cloned().collect();
                                ks.sort();
                                for k in ks {
                                    let val = rc.borrow().get(&k).cloned().expect("key present");
                                    self.scopes.push(HashMap::new());
                                    if let Some(n) = &kn { self.define(n, k.to_value()); }
                                    if let Some(n) = &vn { self.define(n, val); }
                                    let r = self.exec_block(body);
                                    self.scopes.pop();
                                    r?;
                                }
                            }
                            _ => unreachable!("the checker guarantees array/string/Map"),
                        }
                    }
                    // M40.2: iterador de usuario. Evaluamos el iterable una vez (semántica de referencia
                    // → `next` muta su estado interno) y llamamos a `next` hasta `None`.
                    ForIter::Iter { expr, next_fn } => {
                        let it = self.eval_expr(expr)?;
                        let func = *self.functions.get(next_fn.as_str()).expect("the checker guarantees next");
                        loop {
                            // La posición del `for` (la misma que emite el compilador
                            // para el `Call` a `next` → la traza casa entre motores).
                            let r = self.call_function(func, vec![it.clone()], stmt.line, stmt.col)?;
                            let inst = match r {
                                Value::Enum(rc) => rc,
                                _ => unreachable!("next returns Option"),
                            };
                            if inst.variant == "None" {
                                break;
                            }
                            let item = inst.payload[0].clone();
                            self.scopes.push(HashMap::new());
                            match pat {
                                ForPat::Single(n) => self.define(n, item),
                                // M40.2e: patrón de tupla (p. ej. `enumerate`) — el elemento es una tupla
                                // (arreglo en runtime); se liga por posición.
                                ForPat::Tuple(names) => {
                                    let comps = match &item {
                                        Value::Array(rc) => rc.borrow().clone(),
                                        _ => unreachable!("the checker guarantees a tuple element"),
                                    };
                                    for (name, v) in names.iter().zip(comps) {
                                        if let Some(n) = name { self.define(n, v); }
                                    }
                                }
                            }
                            let r = self.exec_block(body);
                            self.scopes.pop();
                            r?;
                        }
                    }
                }
                Ok(())
            }
            StmtKind::Assign { target, value } => {
                let v = self.eval_expr(value)?;
                match &target.kind {
                    ExprKind::Ident(name) => self.assign(name, v),
                    ExprKind::Index { array, index } => {
                        let rc = self.eval_array(array)?;
                        let i = self.eval_int(index)?;
                        let len = rc.borrow().len();
                        let idx = check_bounds(i, len, index.line, index.col)?;
                        rc.borrow_mut()[idx] = v;
                    }
                    ExprKind::Field { object, name } => {
                        let rc = self.eval_struct(object)?;
                        let mut s = rc.borrow_mut();
                        let slot = s.fields.iter_mut().find(|(n, _)| n == name)
                            .expect("the checker guarantees the field");
                        slot.1 = v;
                    }
                    _ => unreachable!("the checker guarantees an lvalue"),
                }
                Ok(())
            }
            StmtKind::Return { value } => {
                // M13.3b: el valor de un `return` está en posición de cola → `eval_tail`. Si es una
                // llamada en cola, propaga `Flow::TailCall` (el trampolín la ejecuta); si es un valor
                // normal, lo envuelve en `Flow::Return` (desenrolla hasta `call_body`).
                match value {
                    Some(e) => match self.eval_tail(e) {
                        Ok(v) => Err(Flow::Return(v)),
                        Err(flow) => Err(flow), // TailCall o Error se propagan tal cual
                    },
                    None => Err(Flow::Return(Value::Unit)),
                }
            }
            StmtKind::Expr(e) => {
                self.eval_expr(e)?; // se evalúa por su efecto; el valor se descarta
                Ok(())
            }
        }
    }

    /// Evalúa una expresión a un valor.
    fn eval_expr(&mut self, expr: &'a Expr) -> EvalResult {
        match &expr.kind {
            ExprKind::Int(v, _) => Ok(Value::Int(*v)),
            ExprKind::Float(v) => Ok(Value::Float(*v)),
            ExprKind::Bool(v) => Ok(Value::Bool(*v)),
            ExprKind::Str(v) => Ok(Value::Str(v.clone())),
            ExprKind::Char(v) => Ok(Value::Char(*v)),
            ExprKind::Bytes(v) => Ok(Value::Bytes(Rc::new(v.clone()))),

            ExprKind::Ident(name) => match self.lookup_opt(name) {
                Some(v) => Ok(v),
                None => {
                    // M27.5: una constante de nivel superior.
                    if let Some(v) = self.consts.get(name) {
                        return Ok(v.clone());
                    }
                    // No es una variable ni constante: un nombre de función usado como valor.
                    let idx = *self.named_index.get(name).expect("the checker guarantees the name");
                    Ok(Value::Function(idx))
                }
            },

            ExprKind::Unary { op, expr: inner } => {
                let v = self.eval_expr(inner)?;
                Ok(match (op, v) {
                    // -i64::MIN desborda (M34, SPEC §8): error, como la aritmética binaria.
                    (UnaryOp::Neg, Value::Int(n)) => Value::Int(
                        n.checked_neg().ok_or_else(|| runtime_error(expr.line, expr.col, "arithmetic overflow on int"))?,
                    ),
                    (UnaryOp::Neg, Value::Float(x)) => Value::Float(-x),
                    (UnaryOp::Not, Value::Bool(b)) => Value::Bool(!b),
                    (UnaryOp::BitNot, Value::Int(n)) => Value::Int(!n), // M19.3a: NOT bit a bit
                    (UnaryOp::BitNot, Value::UInt(n, w)) => make_uint(!n, w), // M28.3: NOT sobre uint (enmascarado)
                    _ => unreachable!("the checker guarantees valid operands for the unary"),
                })
            }

            ExprKind::Binary { op, left, right } => {
                self.eval_binary(*op, left, right, expr.line, expr.col)
            }

            ExprKind::Call { callee, args } => self.eval_call(callee, args),

            // M27.1: una tupla `(a, b, …)` se construye como un arreglo (erasure).
            ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
                let mut vec = Vec::with_capacity(elems.len());
                for e in elems {
                    vec.push(self.eval_expr(e)?);
                }
                Ok(Value::Array(Rc::new(RefCell::new(vec))))
            }

            // M48.2: literal de Map `[k: v, …]` (`[:]` vacío) → un Map con los pares insertados. Las
            // claves y valores se evalúan en orden; una clave repetida gana la última (como `insert`).
            ExprKind::MapLit(pairs) => {
                let mut m = crate::runtime::MapStore::default();
                for (k, v) in pairs {
                    let kv = self.eval_expr(k)?;
                    let vv = self.eval_expr(v)?;
                    m.insert(MapKey::from_value(&kv), vv);
                }
                Ok(Value::Map(Rc::new(RefCell::new(m))))
            }

            // M27.4: conversión numérica `as`. El checker garantiza una combinación válida.
            ExprKind::Cast { expr: inner, ty } => {
                let v = self.eval_expr(inner)?;
                Ok(cast_value(v, ty, inner.line, inner.col)?)
            }

            ExprKind::Index { array, index } => {
                let target = self.eval_expr(array)?;
                let i = self.eval_int(index)?;
                match target {
                    Value::Array(rc) => {
                        let len = rc.borrow().len();
                        let idx = check_bounds(i, len, index.line, index.col)?;
                        Ok(rc.borrow()[idx].clone())
                    }
                    // M11.4c-2: indexar un string → el carácter en esa posición.
                    // M90.6 (superset de Opt.16): sin materializar los chars (como en la VM):
                    // ASCII indexa el byte en O(1); no-ASCII escanea hasta `i` sin asignar.
                    Value::Str(s) => {
                        if s.is_ascii() {
                            let idx = check_bounds(i, s.len(), index.line, index.col)?;
                            Ok(Value::Char(s.as_bytes()[idx] as char))
                        } else {
                            match usize::try_from(i).ok().and_then(|idx| s.chars().nth(idx)) {
                                Some(c) => Ok(Value::Char(c)),
                                None => {
                                    check_bounds(i, s.chars().count(), index.line, index.col)?;
                                    unreachable!("nth failed ⇒ index out of range")
                                }
                            }
                        }
                    }
                    // M16.1a: indexar bytes → el octeto como int.
                    Value::Bytes(b) => {
                        let idx = check_bounds(i, b.len(), index.line, index.col)?;
                        Ok(Value::Int(b[idx] as i64))
                    }
                    _ => unreachable!("the checker guarantees an array, a string or bytes"),
                }
            }

            ExprKind::StructLit { name, fields } => {
                // Construimos (y evaluamos) los campos en ORDEN DE DECLARACIÓN, para
                // que la igualdad y la impresión coincidan con la VM.
                let field_names: Vec<String> = self
                    .structs
                    .get(name.as_str())
                    .expect("the checker registered the struct")
                    .fields
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect();
                let mut built = Vec::with_capacity(field_names.len());
                for fname in &field_names {
                    let value_expr = fields
                        .iter()
                        .find(|(n, _)| n == fname)
                        .map(|(_, e)| e)
                        .expect("the checker guarantees that the field is present");
                    let v = self.eval_expr(value_expr)?;
                    built.push((fname.clone(), v));
                }
                let inst = StructInstance { name: name.clone(), fields: built };
                Ok(Value::Struct(Rc::new(RefCell::new(inst))))
            }

            ExprKind::EnumLit { enum_name, variant, args } => {
                // Evaluamos el payload en orden y armamos el valor de enum.
                let mut payload = Vec::with_capacity(args.len());
                for a in args {
                    payload.push(self.eval_expr(a)?);
                }
                let inst = EnumInstance {
                    enum_name: enum_name.clone(),
                    variant: variant.clone(),
                    payload,
                };
                Ok(Value::Enum(Rc::new(inst)))
            }

            ExprKind::Field { object, name } => {
                // M27.1: un nombre de campo numérico es un acceso a tupla `t.0` (la tupla es un arreglo).
                if let Ok(idx) = name.parse::<usize>() {
                    let v = self.eval_expr(object)?;
                    if let Value::Array(rc) = v {
                        return Ok(rc.borrow()[idx].clone());
                    }
                    unreachable!("the checker guarantees a tuple for the .N access");
                }
                let rc = self.eval_struct(object)?;
                let v = rc
                    .borrow()
                    .fields
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
                    .expect("the checker guarantees the field");
                Ok(v)
            }

            ExprKind::Func(fe) => {
                let index = self.named.len() + fe.id;
                // Capturamos por referencia las celdas visibles en este punto
                // (M4.2). Snapshot de todos los ámbitos, de fuera hacia dentro para
                // que una variable interior tape a una exterior del mismo nombre.
                let mut map: HashMap<String, Cell> = HashMap::new();
                for scope in &self.scopes {
                    for (name, cell) in scope {
                        map.insert(name.clone(), cell.clone());
                    }
                }
                if map.is_empty() {
                    // Sin nada que capturar: una función simple (más barata).
                    Ok(Value::Function(index))
                } else {
                    let upvalues: Vec<(String, Cell)> = map.into_iter().collect();
                    Ok(Value::Closure(Rc::new(Closure { index, upvalues })))
                }
            }

            ExprKind::If { cond, then_branch, else_branch } => {
                if self.eval_bool(cond)? {
                    self.exec_block(then_branch)
                } else if let Some(else_e) = else_branch {
                    self.eval_expr(else_e)
                } else {
                    Ok(Value::Unit)
                }
            }

            ExprKind::While { cond, body } => {
                while self.eval_bool(cond)? {
                    // Ejecutar el cuerpo; su valor se descarta, pero un 'return' o un
                    // error dentro del bucle se propaga (el '?' sale de la función).
                    self.exec_block(body)?;
                }
                Ok(Value::Unit)
            }

            ExprKind::Match { scrutinee, arms } => {
                let value = self.eval_expr(scrutinee)?;
                // Los brazos se prueban en orden; el checker garantiza que alguno
                // casa (exhaustividad), así que el error final es inalcanzable.
                for arm in arms {
                    if let Some(binds) = match_pattern(&arm.pattern, &value) {
                        self.scopes.push(HashMap::new());
                        for (name, v) in binds {
                            self.define(&name, v);
                        }
                        // Guarda (M40.1a): si no da `true`, el brazo no casa → siguiente.
                        if let Some(g) = &arm.guard {
                            match self.eval_expr(g) {
                                Ok(Value::Bool(true)) => {}
                                Ok(_) => { self.scopes.pop(); continue; }
                                Err(e) => { self.scopes.pop(); return Err(e); }
                            }
                        }
                        let result = self.eval_expr(&arm.body);
                        self.scopes.pop();
                        return result;
                    }
                }
                Err(Flow::Error(RuntimeError {
                    msg: "no match branch matched (should not happen)".into(),
                    line: scrutinee.line,
                    col: scrutinee.col,
                    trace: Vec::new(),
                }))
            }

            ExprKind::Try(inner) => {
                // `?`: desempaqueta Ok/Some, o propaga Err/None como un `return` de la
                // función. El checker garantiza que el valor es un Result o un Option.
                let value = self.eval_expr(inner)?;
                match &value {
                    Value::Enum(e) if e.variant == "Ok" || e.variant == "Some" => {
                        Ok(e.payload[0].clone())
                    }
                    // Err(e) / None: retornar ese mismo valor desde la función actual.
                    Value::Enum(_) => Err(Flow::Return(value)),
                    _ => unreachable!("the checker guarantees a Result or an Option"),
                }
            }

            ExprKind::Block(b) => self.exec_block(b),
        }
    }

    fn eval_call(&mut self, callee: &'a Expr, args: &'a [Expr]) -> EvalResult {
        // M48.1: función asociada `Tipo.fn(args)`. `Map.new()` construye un Map vacío; los canales
        // (`Channel.*`) solo corren en la VM (como el antiguo `channel()`), aquí error limpio.
        if let ExprKind::Field { object, name } = &callee.kind {
            if let ExprKind::Ident(tn) = &object.kind {
                if crate::builtins::assoc_lookup(tn, name).is_some() {
                    if tn == "Map" && name == "new" {
                        return Ok(Value::Map(Rc::new(RefCell::new(crate::runtime::MapStore::default()))));
                    }
                    return Err(runtime_error(callee.line, callee.col,
                        "concurrency (spawn/channel/send/recv/join/scope/select) requires the VM; the interpreter is only the sequential oracle (do not use --interp)"));
                }
            }
        }
        // Camino directo: el callee es un nombre que NO está tapado por una variable
        // local — un builtin o una función global. Es la vía eficiente (no se
        // construye un valor-función intermedio).
        if let ExprKind::Ident(name) = &callee.kind {
            let is_local = self.lookup_opt(name).is_some();
            if !is_local {
                // Builtins: evalúan sus argumentos y operan directamente. La membresía la da el
                // registro único (`src/builtins.rs`); la implementación vive en `eval_builtin`.
                if crate::builtins::is_builtin(name) {
                    let mut values = Vec::with_capacity(args.len());
                    for arg in args {
                        values.push(self.eval_expr(arg)?);
                    }
                    // `panic` (M13.2a) aborta con el mensaje en la posición de la llamada. Se
                    // intercepta aquí —no en `eval_builtin`— porque debe producir un `Flow::Error`
                    // (un valor de error, no un `Value`), y aquí tenemos la posición del callee.
                    if name == "panic" {
                        let msg = match &values[0] {
                            Value::Str(s) => s.clone(),
                            _ => unreachable!("the checker guarantees a string"),
                        };
                        return Err(runtime_error(callee.line, callee.col, &msg));
                    }
                    // M130: `exit(code)` termina el PROCESO aquí mismo (flusheando salida antes).
                    if name == "exit" {
                        let code = match &values[0] {
                            Value::Int(n) => *n,
                            _ => unreachable!("the checker guarantees an int"),
                        };
                        crate::builtins::process_exit(code);
                    }
                    // M131: normalización Unicode — se intercepta aquí (no en eval_builtin)
                    // porque el Err (stub slim / forma desconocida) aborta con posición.
                    if name == "__unicode_normalize" {
                        let (Value::Str(s), Value::Str(form)) = (&values[0], &values[1]) else {
                            unreachable!("the checker guarantees string, string");
                        };
                        return match crate::builtins::unicode_normalize(s, form) {
                            Ok(out) => Ok(Value::Str(out)),
                            Err(e) => Err(runtime_error(callee.line, callee.col, &e)),
                        };
                    }
                    // M97.2: `__try_call(f)` llama a `f` y convierte un fallo en valor: `[]` si fue
                    // bien, `[msg]` si falló. Se intercepta aquí —no en `eval_builtin`— por dos
                    // razones: hace falta `&mut self` para llamar (`eval_builtin` toma `&self`), y
                    // el fallo que se captura ES un `Flow::Error`, que solo se ve desde aquí.
                    // El desenrollado lo hace el `?` de Rust: al capturar el `Err`, los marcos del
                    // tramo abortado ya se descartaron con la pila de Rust.
                    if name == "__try_call" {
                        let outcome = match &values[0] {
                            Value::Function(idx) => {
                                self.call_index(*idx, vec![], &[], callee.line, callee.col)
                            }
                            Value::Closure(c) => {
                                let captured = c.upvalues.clone();
                                self.call_index(c.index, vec![], &captured, callee.line, callee.col)
                            }
                            _ => unreachable!("the checker guarantees a function of no arguments"),
                        };
                        let cell = match outcome {
                            Ok(_) => vec![],
                            // Solo el MENSAJE: quien lo observe le pone su propia posición, igual
                            // que `try_join` (`fail_current_fiber` hace lo mismo con la fibra).
                            Err(Flow::Error(e)) => vec![Value::Str(e.msg)],
                            // `call_index` consume el `Return` del cuerpo y ejecuta las `TailCall`
                            // en su trampolín, así que ninguno de los dos escapa hasta aquí.
                            Err(_) => unreachable!("call_index consumes Return and TailCall"),
                        };
                        return Ok(Value::Array(std::rc::Rc::new(
                            std::cell::RefCell::new(cell),
                        )));
                    }
                    // P0.3: `add_to(m, k, delta)` — upsert acumulativo en 1 lookup (entry-API). Se
                    // intercepta aquí (no en `eval_builtin`) porque el overflow int es un `Flow::Error`
                    // (como `+`), y aquí tenemos la posición del callee. Espejo del opcode `MapAdd`.
                    if name == "add_to" {
                        use std::collections::hash_map::Entry;
                        let k = MapKey::from_value(&values[1]);
                        let delta = values[2].clone();
                        match &values[0] {
                            Value::Map(rc) => match rc.borrow_mut().entry(k) {
                                Entry::Occupied(mut e) => {
                                    let nv = match (e.get(), &delta) {
                                        (Value::Int(a), Value::Int(b)) => Value::Int(
                                            a.checked_add(*b).ok_or_else(|| runtime_error(
                                                callee.line, callee.col, "arithmetic overflow on int"))?),
                                        (Value::Float(a), Value::Float(b)) => Value::Float(a + b),
                                        _ => unreachable!("the checker guarantees int/float map value + matching delta"),
                                    };
                                    e.insert(nv);
                                }
                                Entry::Vacant(e) => { e.insert(delta); }
                            },
                            _ => unreachable!("the checker guarantees a Map"),
                        }
                        return Ok(Value::Unit);
                    }
                    // M12.1: la concurrencia (CSP) vive SOLO en la VM (necesita el scheduler de fibras y
                    // continuaciones que el intérprete tree-walking no tiene). Error limpio en vez de panic.
                    // `close` NO va aquí: es ad-hoc polimórfico y su forma de handle de archivo (M11.8) sí
                    // corre en el intérprete; un canal nunca llega al intérprete (channel() ya da error).
                    // `join` NO va aquí: es ad-hoc polimórfico y su forma de strings (M11.7a) corre en el
                    // intérprete; la forma de Task nunca llega (spawn ya da error → no existen Tasks aquí).
                    if name == "spawn" || name == "send" || name == "__recv"
                        || name == "scope" || name == "select" || name == "try_recv"
                        || name == "__select_timeout" || name == "__task_failed" || name == "signals" {
                        return Err(runtime_error(callee.line, callee.col,
                            "concurrency (spawn/channel/send/recv/join/scope/select) requires the VM; the interpreter is only the sequential oracle (do not use --interp)"));
                    }
                    // M89.2: la cripto de ring en un binario sin la feature 'net-tls' ABORTA
                    // con un error claro (nunca un hash vacío ni una firma que falla en
                    // silencio); el TLS degrada como Err-valor desde su stub. Espejo de la VM.
                    if !crate::builtins::net_tls_available()
                        && matches!(name.as_str(),
                            "__crypto_random_bytes" | "__sha256" | "__sha512" | "__sha1"
                            | "__hasher_new" | "__hasher_update" | "__hasher_final"
                            | "__hmac_sha256" | "__ed25519_public_key" | "__ed25519_sign"
                            | "__ed25519_verify" | "__chacha20poly1305_seal"
                            | "__chacha20poly1305_open" | "__x25519_public_key"
                            | "__x25519_shared_secret" | "__hkdf_sha256"
                            | "__constant_time_eq")
                    {
                        return Err(runtime_error(callee.line, callee.col,
                            crate::builtins::NET_TLS_UNAVAILABLE));
                    }
                    return Ok(self.eval_builtin(name, values));
                }
                // M41: función externa (FFI). Se despacha a la librería C vía `ffi::call`. La frontera
                // insegura: un fallo de carga/símbolo o firma no soportada es un error de ejecución en
                // la posición de la llamada.
                if let Some(desc) = self.externs.get(name.as_str()).cloned() {
                    // Los `Value` se evalúan y **retienen** en `vals`: los `FfiVal` de string/bytes los
                    // toman prestados y deben vivir durante la llamada C.
                    let mut vals = Vec::with_capacity(args.len());
                    for arg in args {
                        vals.push(self.eval_expr(arg)?);
                    }
                    let fargs: Vec<_> = vals.iter().map(value_to_ffi).collect();
                    return match crate::ffi::call(&desc, &fargs) {
                        Ok(r) => ffi_to_value(r, desc.ret_kind, callee.line, callee.col),
                        Err(msg) => Err(runtime_error(callee.line, callee.col, &msg)),
                    };
                }
                // Función de nivel superior: llamada directa.
                if let Some(&idx) = self.named_index.get(name.as_str()) {
                    let mut values = Vec::with_capacity(args.len());
                    for arg in args {
                        values.push(self.eval_expr(arg)?);
                    }
                    return self.call_index(idx, values, &[], callee.line, callee.col);
                }
            }
        }

        // Camino indirecto: el callee es una expresión que produce un valor-función
        // (una variable, un literal `fn`, el resultado de otra llamada...). Puede ser
        // una función simple o una closure con su entorno. (M4.1/M4.2)
        let callee_val = self.eval_expr(callee)?;
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval_expr(arg)?);
        }
        match callee_val {
            Value::Function(idx) => self.call_index(idx, values, &[], callee.line, callee.col),
            Value::Closure(c) => {
                // Clonamos el entorno capturado (clona los `Rc` de las celdas, las
                // comparte) para soltar el préstamo del valor antes de la llamada.
                let captured = c.upvalues.clone();
                self.call_index(c.index, values, &captured, callee.line, callee.col)
            }
            _ => unreachable!("the checker guarantees a function"),
        }
    }

    /// Ejecuta un builtin (`print`/`len`/`push`) con sus argumentos ya evaluados.
    fn eval_builtin(&self, name: &str, values: Vec<Value>) -> Value {
        match name {
            "print" => {
                crate::host_print(&values[0].to_string());
                Value::Unit
            }
            // V5 (bench políglota): sort nativo de [int]/[string]/[char] (el checker reescribe el
            // `sort` del prelude cuando resuelve con el Ord primitivo del prelude). Arreglo NUEVO,
            // misma semántica que el merge sort del prelude → oráculo intacto.
            "__sort_prim" => match &values[0] {
                Value::Array(rc) => {
                    let mut s = rc.borrow().clone();
                    s.sort_unstable_by(|a, b| match (a, b) {
                        (Value::Str(x), Value::Str(y)) => x.cmp(y),
                        (Value::Int(x), Value::Int(y)) => x.cmp(y),
                        (Value::Char(x), Value::Char(y)) => x.cmp(y),
                        _ => unreachable!("the checker guarantees primitive elements"),
                    });
                    Value::Array(std::rc::Rc::new(std::cell::RefCell::new(s)))
                }
                _ => unreachable!("the checker guarantees an array"),
            },
            // V2 (bench políglota): concatenación n-aria de strings (el checker aplana las cadenas
            // de `+`/interpolación a `__concat`). Un solo String con la capacidad exacta; misma
            // semántica que la cadena de `Add` par a par → oráculo intacto.
            "__concat" => {
                let total: usize = values.iter().map(|v| match v {
                    Value::Str(s) => s.len(),
                    _ => unreachable!("the checker guarantees strings"),
                }).sum();
                let mut out = String::with_capacity(total);
                for v in &values {
                    if let Value::Str(s) = v { out.push_str(s); }
                }
                Value::Str(out)
            }
            // M48.4: `__len` es el primitivo interno al que baja el trait `Len`; idéntico a `len`.
            "__len" => match &values[0] {
                Value::Array(rc) => Value::Int(rc.borrow().len() as i64),
                // M11.1a: len de string = nº de caracteres (Unicode scalar values).
                // M90.6: ASCII → nº de chars == nº de bytes (como en la VM).
                Value::Str(s) => Value::Int(if s.is_ascii() {
                    s.len() as i64
                } else {
                    s.chars().count() as i64
                }),
                // M13.1: len de un Map = nº de entradas.
                Value::Map(rc) => Value::Int(rc.borrow().len() as i64),
                // M16.1a: len de bytes = nº de octetos.
                Value::Bytes(b) => Value::Int(b.len() as i64),
                _ => unreachable!("the checker guarantees an array, string, Map or bytes"),
            },
            // --- Mapas (M13.1) --- (`Map.new()` es una función asociada, M48.1: se evalúa en `eval_call`)
            // M48.4c: los `__*` son los primitivos internos a los que baja el trait `MapOps`.
            "__insert" => {
                match &values[0] {
                    Value::Map(rc) => {
                        rc.borrow_mut().insert(MapKey::from_value(&values[1]), values[2].clone());
                    }
                    _ => unreachable!("the checker guarantees a Map"),
                }
                Value::Unit
            }
            "__contains_key" => match &values[0] {
                Value::Map(rc) => Value::Bool(rc.borrow().contains_key(&MapKey::from_value(&values[1]))),
                _ => unreachable!("the checker guarantees a Map"),
            },
            // Primitivo: [] o [v]; el prelude lo envuelve en Option<V>.
            "__map_get" => match &values[0] {
                Value::Map(rc) => {
                    let elems = match rc.borrow().get(&MapKey::from_value(&values[1])) {
                        Some(v) => vec![v.clone()],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees a Map"),
            },
            // P0.2: get-or-default SIN alocar (a diferencia de __map_get, que aloca [V]).
            "__get_or" => match &values[0] {
                Value::Map(rc) => match rc.borrow().get(&MapKey::from_value(&values[1])) {
                    Some(v) => v.clone(),
                    None => values[2].clone(),
                },
                _ => unreachable!("the checker guarantees a Map"),
            },
            // M13.1b: quita y devuelve [] o [v]; el prelude → Option<V>.
            "__map_remove" => match &values[0] {
                Value::Map(rc) => {
                    let elems = match rc.borrow_mut().remove(&MapKey::from_value(&values[1])) {
                        Some(v) => vec![v],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees a Map"),
            },
            // M13.1b: claves ordenadas (determinista).
            "__keys" => match &values[0] {
                Value::Map(rc) => {
                    let mut ks: Vec<MapKey> = rc.borrow().keys().cloned().collect();
                    ks.sort();
                    let elems: Vec<Value> = ks.iter().map(|k| k.to_value()).collect();
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees a Map"),
            },
            // M13.1b: valores en orden de clave ordenada (casa posición a posición con keys).
            "__values" => match &values[0] {
                Value::Map(rc) => {
                    let m = rc.borrow();
                    let mut pairs: Vec<(&MapKey, &Value)> = m.iter().collect();
                    pairs.sort_by(|a, b| a.0.cmp(b.0));
                    let elems: Vec<Value> = pairs.iter().map(|(_, v)| (*v).clone()).collect();
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees a Map"),
            },
            // M48.4b: `__push`/`__reverse`/`__contains` son los primitivos internos a los que bajan
            // los traits `Push`/`Reverse`/`Contains`; idénticos a sus públicos.
            "__push" => {
                match &values[0] {
                    Value::Array(rc) => rc.borrow_mut().push(values[1].clone()),
                    _ => unreachable!("the checker guarantees an array"),
                }
                Value::Unit
            }
            // M11.1a: representación textual de un primitivo (la misma que `print`/Display).
            "to_string" => Value::Str(format!("{}", values[0])),
            // M11.1b: recorta los extremos.
            "__trim" => match &values[0] {
                Value::Str(s) => Value::Str(s.trim().to_string()),
                _ => unreachable!("the checker guarantees a string"),
            },
            // M11.1b: parte por el separador → arreglo de strings.
            "__split" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Str(sep)) => {
                    let parts: Vec<Value> = s.split(sep.as_str()).map(|p| Value::Str(p.to_string())).collect();
                    Value::Array(Rc::new(RefCell::new(parts)))
                }
                _ => unreachable!("the checker guarantees two strings"),
            },
            // M11.4c-2: los caracteres del string → arreglo de char.
            "__chars" => match &values[0] {
                Value::Str(s) => {
                    let cs: Vec<Value> = s.chars().map(Value::Char).collect();
                    Value::Array(Rc::new(RefCell::new(cs)))
                }
                _ => unreachable!("the checker guarantees a string"),
            },
            // M40.3a: el code point Unicode de un char → int.
            "char_code" => match &values[0] {
                Value::Char(c) => Value::Int(*c as i64),
                _ => unreachable!("the checker guarantees a char"),
            },
            // M16.1b: los octetos UTF-8 del string → bytes.
            "__to_bytes" => match &values[0] {
                Value::Str(s) => Value::Bytes(Rc::new(s.clone().into_bytes())),
                _ => unreachable!("the checker guarantees a string"),
            },
            // M43: hashes de producción vía `ring`. Delegan en los helpers de `builtins` (compartidos
            // con la VM → salida idéntica, el oráculo se mantiene).
            // M68.2: aleatoriedad criptográfica (CSPRNG del SO).
            "__crypto_random_bytes" => match &values[0] {
                Value::Int(n) => Value::Bytes(Rc::new(crate::builtins::crypto_random_bytes(*n))),
                _ => unreachable!("the checker guarantees an int"),
            },
            // M126: hasher incremental (estado en ray_runtime::crypto, compartido con VM/nativo).
            "__hasher_new" => match &values[0] {
                Value::Str(alg) => {
                    let arr = match crate::builtins::hasher_new(alg) {
                        Ok(id) => vec![Value::Str("ok".to_string()), Value::Str(id.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    };
                    Value::Array(Rc::new(RefCell::new(arr)))
                }
                _ => unreachable!("the checker guarantees a string"),
            },
            "__hasher_update" => match (&values[0], &values[1]) {
                (Value::Int(h), Value::Bytes(chunk)) => {
                    let arr = match crate::builtins::hasher_update(*h, chunk) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    };
                    Value::Array(Rc::new(RefCell::new(arr)))
                }
                _ => unreachable!("the checker guarantees int, bytes"),
            },
            "__hasher_final" => match &values[0] {
                Value::Int(h) => {
                    let arr = match crate::builtins::hasher_final(*h) {
                        Ok(d) => vec![Value::Bytes(Rc::new(b"ok".to_vec())), Value::Bytes(Rc::new(d))],
                        Err(e) => vec![Value::Bytes(Rc::new(b"err".to_vec())), Value::Bytes(Rc::new(e.into_bytes()))],
                    };
                    Value::Array(Rc::new(RefCell::new(arr)))
                }
                _ => unreachable!("the checker guarantees an int"),
            },
            "__sha256" => match &values[0] {
                Value::Bytes(b) => Value::Bytes(Rc::new(crate::builtins::sha256(b))),
                _ => unreachable!("the checker guarantees bytes"),
            },
            "__sha512" => match &values[0] {
                Value::Bytes(b) => Value::Bytes(Rc::new(crate::builtins::sha512(b))),
                _ => unreachable!("the checker guarantees bytes"),
            },
            "__sha1" => match &values[0] {
                Value::Bytes(b) => Value::Bytes(Rc::new(crate::builtins::sha1(b))),
                _ => unreachable!("the checker guarantees bytes"),
            },
            "__hmac_sha256" => match (&values[0], &values[1]) {
                (Value::Bytes(k), Value::Bytes(m)) => Value::Bytes(Rc::new(crate::builtins::hmac_sha256(k, m))),
                _ => unreachable!("the checker guarantees bytes, bytes"),
            },
            // M43.3: Ed25519. Los fallibles devuelven `[bytes]` etiquetado (vacío/único); el prelude → Option.
            "__ed25519_public_key" => match &values[0] {
                Value::Bytes(seed) => {
                    let elems = match crate::builtins::ed25519_public_key(seed) {
                        Some(pk) => vec![Value::Bytes(Rc::new(pk))],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees bytes"),
            },
            "__ed25519_sign" => match (&values[0], &values[1]) {
                (Value::Bytes(seed), Value::Bytes(msg)) => {
                    let elems = match crate::builtins::ed25519_sign(seed, msg) {
                        Some(sig) => vec![Value::Bytes(Rc::new(sig))],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees bytes, bytes"),
            },
            "__ed25519_verify" => match (&values[0], &values[1], &values[2]) {
                (Value::Bytes(pk), Value::Bytes(msg), Value::Bytes(sig)) => {
                    Value::Bool(crate::builtins::ed25519_verify(pk, msg, sig))
                }
                _ => unreachable!("the checker guarantees bytes, bytes, bytes"),
            },
            // M43.4: ChaCha20-Poly1305 AEAD. seal/open devuelven `[bytes]` etiquetado; el prelude → Option.
            "__chacha20poly1305_seal" | "__chacha20poly1305_open" => {
                match (&values[0], &values[1], &values[2], &values[3]) {
                    (Value::Bytes(k), Value::Bytes(n), Value::Bytes(aad), Value::Bytes(data)) => {
                        let res = if name == "__chacha20poly1305_seal" {
                            crate::builtins::chacha20poly1305_seal(k, n, aad, data)
                        } else {
                            crate::builtins::chacha20poly1305_open(k, n, aad, data)
                        };
                        let elems = match res {
                            Some(out) => vec![Value::Bytes(Rc::new(out))],
                            None => vec![],
                        };
                        Value::Array(Rc::new(RefCell::new(elems)))
                    }
                    _ => unreachable!("the checker guarantees four bytes"),
                }
            }
            // M114: X25519 + HKDF. Los tres fallibles devuelven `[bytes]` etiquetado; el prelude → Option.
            "__x25519_public_key" => match &values[0] {
                Value::Bytes(sk) => {
                    let elems = match crate::builtins::x25519_public_key(sk) {
                        Some(pk) => vec![Value::Bytes(Rc::new(pk))],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees bytes"),
            },
            "__x25519_shared_secret" => match (&values[0], &values[1]) {
                (Value::Bytes(sk), Value::Bytes(peer)) => {
                    let elems = match crate::builtins::x25519_shared_secret(sk, peer) {
                        Some(secret) => vec![Value::Bytes(Rc::new(secret))],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees bytes, bytes"),
            },
            "__hkdf_sha256" => match (&values[0], &values[1], &values[2], &values[3]) {
                (Value::Bytes(salt), Value::Bytes(ikm), Value::Bytes(info), Value::Int(len)) => {
                    let elems = match crate::builtins::hkdf_sha256(salt, ikm, info, *len) {
                        Some(okm) => vec![Value::Bytes(Rc::new(okm))],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees bytes, bytes, bytes, int"),
            },
            "__constant_time_eq" => match (&values[0], &values[1]) {
                (Value::Bytes(a), Value::Bytes(b)) => Value::Bool(crate::builtins::constant_time_eq(a, b)),
                _ => unreachable!("the checker guarantees bytes, bytes"),
            },
            // M16.1b: decodifica bytes como UTF-8 → ["ok", s] o ["err", msg]. El prelude → Result.
            "__from_utf8" => {
                let arr = match &values[0] {
                    Value::Bytes(b) => match std::str::from_utf8(b) {
                        Ok(s) => vec![Value::Str("ok".to_string()), Value::Str(s.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("the checker guarantees bytes"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M16.1c: lee un archivo como bytes → [b"ok", datos] o [b"err", msg]. Tag en bytes para
            // que el arreglo sea homogéneo ([bytes]); el prelude → Result<bytes,string>.
            "__read_file_bytes" => {
                let arr = match &values[0] {
                    Value::Str(path) => match crate::builtins::read_file_bytes(path) {
                        Ok(data) => vec![bytes_tag("ok"), Value::Bytes(Rc::new(data))],
                        Err(e) => vec![bytes_tag("err"), bytes_of_str(&e.to_string())],
                    },
                    _ => unreachable!("the checker guarantees a string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M147: un asset del espacio embed → [b"ok", datos] o [b"err", msg].
            "__embed_read" => {
                let arr = match &values[0] {
                    Value::Str(path) => match crate::builtins::embed_read(path) {
                        Ok(data) => vec![bytes_tag("ok"), Value::Bytes(Rc::new(data))],
                        Err(e) => vec![bytes_tag("err"), bytes_of_str(&e)],
                    },
                    _ => unreachable!("the checker guarantees a string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M147: las claves del espacio embed → ["ok", clave…] o ["err", msg].
            "__embed_list" => {
                let arr = match crate::builtins::embed_list() {
                    Ok(keys) => {
                        let mut v = vec![Value::Str("ok".to_string())];
                        v.extend(keys.into_iter().map(Value::Str));
                        v
                    }
                    Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M16.1c: escribe bytes a un archivo → ["ok"] o ["err", msg].
            "__write_file_bytes" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(path), Value::Bytes(data)) => match crate::builtins::write_file_bytes(path, data) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("the checker guarantees string, bytes"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M16.1c: lee del socket como bytes (bloqueante en el intérprete) → [b"ok", datos]/[b"err", msg].
            // M100 v2: `__proc_read` es un alias con opcode compartido — el mismo camino lee un
            // socket o el pipe de un proceso hijo (la variante bloqueante ya distingue el Pipe).
            "__socket_read_bytes" | "__proc_read" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::socket_read_bytes_blocking(*h) {
                        Ok(data) => vec![bytes_tag("ok"), Value::Bytes(Rc::new(data))],
                        Err(e) => vec![bytes_tag("err"), bytes_of_str(&e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M100 v3: `__proc_write` es un alias con opcode compartido — el mismo camino escribe
            // en el stdin de un hijo vivo (el despacho por tipo de handle vive en el host).
            // M16.1c: escribe bytes en el socket → ["ok", ""] o ["err", msg].
            "__socket_write_bytes" | "__proc_write" | "__audio_write" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(h), Value::Bytes(data)) => match crate::builtins::socket_write_raw(*h, data) {
                        Ok(_) => vec![Value::Str("ok".to_string()), Value::Str(String::new())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees int, bytes"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M20.8: enlaza un socket UDP → ["ok", handle] o ["err", msg].
            "__udp_bind" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(host), Value::Int(port)) => match crate::builtins::udp_bind(host, *port) {
                        Ok(h) => vec![Value::Str("ok".to_string()), Value::Str(h.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees string, int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M20.8: envía un datagrama → ["ok", n] o ["err", msg].
            "__udp_send_to" => {
                let arr = match (&values[0], &values[1], &values[2], &values[3]) {
                    (Value::Int(h), Value::Str(host), Value::Int(port), Value::Bytes(data)) => {
                        match crate::builtins::udp_send_to(*h, host, *port, data) {
                            Ok(n) => vec![Value::Str("ok".to_string()), Value::Str(n.to_string())],
                            Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                        }
                    }
                    _ => unreachable!("the checker guarantees int, string, int, bytes"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M20.8: recibe un datagrama (bloqueante) → [b"ok", host, puerto, datos] o [b"err", msg].
            "__udp_recv_from" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::udp_recv_from(*h) {
                        Ok((host, port, data)) => vec![
                            bytes_tag("ok"),
                            bytes_of_str(&host),
                            bytes_of_str(&port.to_string()),
                            Value::Bytes(Rc::new(data)),
                        ],
                        Err(e) => vec![bytes_tag("err"), bytes_of_str(&e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M100 (IDEAS §53.8): ejecuta un proceso del SO (bloqueante: el intérprete es el oráculo
            // secuencial) → el arreglo etiquetado de `run_encoded`.
            "__run" => {
                let (Value::Str(program), Value::Array(args), Value::Str(dir), Value::Array(env),
                    Value::Bool(env_clear), Value::Bytes(stdin), Value::Bool(has_stdin),
                    Value::Int(timeout_ms), Value::Int(max_output), Value::Bool(merge_output)) =
                    (&values[0], &values[1], &values[2], &values[3], &values[4], &values[5],
                     &values[6], &values[7], &values[8], &values[9])
                else { unreachable!("the checker guarantees the __run signature") };
                let as_strings = |rc: &Rc<RefCell<Vec<Value>>>| -> Vec<String> {
                    rc.borrow().iter().map(|v| match v {
                        Value::Str(s) => s.clone(),
                        _ => unreachable!("the checker guarantees [string]"),
                    }).collect()
                };
                let opts = crate::builtins::run_opts_from_flat(
                    dir, as_strings(env), *env_clear, stdin, *has_stdin, false,
                    *timeout_ms, *max_output, *merge_output,
                );
                let arr = crate::builtins::run_encoded(program, &as_strings(args), &opts)
                    .into_iter().map(|b| Value::Bytes(Rc::new(b))).collect();
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M100 v2 (IDEAS §53.9): los primitivos del streaming son NO-bloqueantes y funcionan
            // también aquí — pero las bombas de std/process usan spawn/canales, que el intérprete
            // rechaza con su mensaje propio: stream() es de la VM y el nativo.
            "__proc_spawn" => {
                let (Value::Str(program), Value::Array(args), Value::Str(dir), Value::Array(env),
                    Value::Bool(env_clear), Value::Bytes(stdin), Value::Bool(has_stdin),
                    Value::Bool(stdin_open), Value::Bool(merge_output)) =
                    (&values[0], &values[1], &values[2], &values[3], &values[4], &values[5],
                     &values[6], &values[7], &values[8])
                else { unreachable!("the checker guarantees the __proc_spawn signature") };
                let as_strings = |rc: &Rc<RefCell<Vec<Value>>>| -> Vec<String> {
                    rc.borrow().iter().map(|v| match v {
                        Value::Str(s) => s.clone(),
                        _ => unreachable!("the checker guarantees [string]"),
                    }).collect()
                };
                let opts = crate::builtins::run_opts_from_flat(
                    dir, as_strings(env), *env_clear, stdin, *has_stdin, *stdin_open, 0, 0, *merge_output,
                );
                let arr = crate::builtins::proc_spawn_encoded(program, &as_strings(args), &opts)
                    .into_iter().map(|b| Value::Bytes(Rc::new(b))).collect();
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__proc_try_wait" => {
                let Value::Int(h) = &values[0] else { unreachable!("the checker guarantees an int") };
                let arr = crate::builtins::proc_try_wait_encoded(*h)
                    .into_iter().map(|b| Value::Bytes(Rc::new(b))).collect();
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__proc_kill" => {
                let (Value::Int(h), Value::Bool(force)) = (&values[0], &values[1])
                else { unreachable!("the checker guarantees int, bool") };
                crate::builtins::proc_kill(*h, *force);
                Value::Unit
            }
            // M11.4a/M11.7b: ¿el string contiene la subcadena? / ¿el arreglo contiene el elemento?
            "__contains" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Str(sub)) => Value::Bool(s.contains(sub.as_str())),
                (Value::Array(rc), x) => Value::Bool(rc.borrow().iter().any(|e| e == x)),
                _ => unreachable!("the checker guarantees string+string or array+element"),
            },
            // M11.4a: reemplaza todas las ocurrencias de `de` por `a`.
            "__replace" => match (&values[0], &values[1], &values[2]) {
                (Value::Str(s), Value::Str(de), Value::Str(a)) => {
                    Value::Str(s.replace(de.as_str(), a.as_str()))
                }
                _ => unreachable!("the checker guarantees three strings"),
            },
            // M11.7a: ¿empieza/termina con la subcadena?
            "__starts_with" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Str(p)) => Value::Bool(s.starts_with(p.as_str())),
                _ => unreachable!("the checker guarantees two strings"),
            },
            "__ends_with" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Str(p)) => Value::Bool(s.ends_with(p.as_str())),
                _ => unreachable!("the checker guarantees two strings"),
            },
            // M11.7a: mayúsculas/minúsculas.
            "__to_upper" => match &values[0] {
                Value::Str(s) => Value::Str(s.to_uppercase()),
                _ => unreachable!("the checker guarantees a string"),
            },
            "__to_lower" => match &values[0] {
                Value::Str(s) => Value::Str(s.to_lowercase()),
                _ => unreachable!("the checker guarantees a string"),
            },
            // M11.7a: subcadena por índice de carácter (con clamp); repetir.
            "__substring" => match (&values[0], &values[1], &values[2]) {
                (Value::Str(s), Value::Int(i), Value::Int(j)) => {
                    Value::Str(crate::builtins::substring_chars(s, *i, *j))
                }
                _ => unreachable!("the checker guarantees string, int, int"),
            },
            // M19.2: sub-secuencia de bytes por octeto (con clamp).
            "__sub_bytes" => match (&values[0], &values[1], &values[2]) {
                (Value::Bytes(b), Value::Int(i), Value::Int(j)) => {
                    Value::Bytes(Rc::new(crate::builtins::sub_bytes_octets(b, *i, *j)))
                }
                _ => unreachable!("the checker guarantees bytes, int, int"),
            },
            // bytes_of (M19.3c): [int] → bytes, cada elemento truncado a octeto (`& 255`).
            "bytes_of" => match &values[0] {
                Value::Array(xs) => {
                    let octets: Vec<u8> = xs.borrow().iter().map(|v| match v {
                        Value::Int(n) => (*n & 0xff) as u8,
                        _ => unreachable!("the checker guarantees [int]"),
                    }).collect();
                    Value::Bytes(Rc::new(octets))
                }
                _ => unreachable!("the checker guarantees an array"),
            },
            "__repeat" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Int(n)) => Value::Str(crate::builtins::repeat_str(s, *n)),
                _ => unreachable!("the checker guarantees string, int"),
            },
            // M11.7a: primitivo de búsqueda → [] o [i] (índice de carácter). El prelude → Option<int>.
            "__index_of" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Str(sub)) => {
                    let elems = match crate::builtins::char_index_of(s, sub) {
                        Some(i) => vec![Value::Int(i as i64)],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees two strings"),
            },
            // M11.7a: une un [string] con el separador.
            "join" => match (&values[0], &values[1]) {
                (Value::Array(rc), Value::Str(sep)) => {
                    let parts: Vec<String> = rc.borrow().iter().map(|v| match v {
                        Value::Str(s) => s.clone(),
                        _ => unreachable!("the checker guarantees [string]"),
                    }).collect();
                    Value::Str(parts.join(sep.as_str()))
                }
                _ => unreachable!("the checker guarantees [string], string"),
            },
            // M11.7b: arreglo nuevo en orden inverso.
            "__reverse" => match &values[0] {
                Value::Array(rc) => {
                    let mut v = rc.borrow().clone();
                    v.reverse();
                    Value::Array(Rc::new(RefCell::new(v)))
                }
                _ => unreachable!("the checker guarantees an array"),
            },
            // M11.7b: primitivo que muta el arreglo quitando el último → [] o [x]. Prelude → Option<T>.
            "__pop" => match &values[0] {
                Value::Array(rc) => {
                    let popped = rc.borrow_mut().pop();
                    let elems = popped.map(|v| vec![v]).unwrap_or_default();
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees an array"),
            },
            // M11.7b: primitivo de búsqueda en arreglo → [] o [i]. Prelude → Option<int>.
            "__position" => match (&values[0], &values[1]) {
                (Value::Array(rc), x) => {
                    let idx = rc.borrow().iter().position(|e| e == x);
                    let elems = idx.map(|i| vec![Value::Int(i as i64)]).unwrap_or_default();
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees array+element"),
            },
            // M11.2a: como print, pero a stderr.
            "eprint" => {
                crate::host_eprint(&values[0].to_string());
                Value::Unit
            }
            // M11.2a: primitivo de parseo → [] o [n]. El prelude lo envuelve en Option.
            // D3: formas fusionadas de `<wrapper>(…).unwrap_or(d)` (misma semántica que la cadena
            // wrapper+unwrap_or del prelude → oráculo intacto).
            "__index_of_or" => match (&values[0], &values[1], &values[2]) {
                (Value::Str(s), Value::Str(sub), Value::Int(d)) => {
                    Value::Int(crate::builtins::char_index_of(s, sub).map(|i| i as i64).unwrap_or(*d))
                }
                _ => unreachable!("the checker guarantees (string, string, int)"),
            },
            "__parse_int_or" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Int(d)) => Value::Int(s.trim().parse::<i64>().unwrap_or(*d)),
                _ => unreachable!("the checker guarantees (string, int)"),
            },
            "__parse_int" => match &values[0] {
                Value::Str(s) => match s.trim().parse::<i64>() {
                    Ok(n) => Value::Array(Rc::new(RefCell::new(vec![Value::Int(n)]))),
                    Err(_) => Value::Array(Rc::new(RefCell::new(vec![]))),
                },
                _ => unreachable!("the checker guarantees a string"),
            },
            // M14: parseo de flotante (lo pide el lexer auto-alojado). [] o [f].
            "__parse_float" => match &values[0] {
                Value::Str(s) => match s.trim().parse::<f64>() {
                    Ok(f) => Value::Array(Rc::new(RefCell::new(vec![Value::Float(f)]))),
                    Err(_) => Value::Array(Rc::new(RefCell::new(vec![]))),
                },
                _ => unreachable!("the checker guarantees a string"),
            },
            // M11.2a: primitivo de lectura de línea → [] en EOF, [linea] si no (sin el '\n').
            "__read_line" => {
                let mut line = String::new();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) => Value::Array(Rc::new(RefCell::new(vec![]))), // EOF
                    Ok(_) => {
                        let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
                        Value::Array(Rc::new(RefCell::new(vec![Value::Str(trimmed)])))
                    }
                    Err(_) => Value::Array(Rc::new(RefCell::new(vec![]))),
                }
            }
            // M11.2b: primitivo de entorno → [] si no existe, [valor] si sí.
            "__env" => match &values[0] {
                Value::Str(name) => match std::env::var(name) {
                    Ok(v) => Value::Array(Rc::new(RefCell::new(vec![Value::Str(v)]))),
                    Err(_) => Value::Array(Rc::new(RefCell::new(vec![]))),
                },
                _ => unreachable!("the checker guarantees a string"),
            },
            // M11.2b: argumentos del programa (del almacén de proceso; [] si no se fijaron).
            "args" => {
                let items = program_args().iter().map(|a| Value::Str(a.clone())).collect();
                Value::Array(Rc::new(RefCell::new(items)))
            }
            // M11.2c: lee un archivo → arreglo etiquetado ["ok", contenido] o ["err", msg].
            "__read_file" => {
                let arr = match &values[0] {
                    Value::Str(path) => match std::fs::read_to_string(path) {
                        Ok(c) => vec![Value::Str("ok".to_string()), Value::Str(c)],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("the checker guarantees a string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.2c: escribe un archivo → ["ok"] o ["err", msg].
            "__write_file" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(path), Value::Str(contents)) => match std::fs::write(path, contents) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("the checker guarantees two strings"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.4b (M50.1: __x): ¿existe la ruta? (total, no falla).
            "__exists" => match &values[0] {
                Value::Str(path) => Value::Bool(std::path::Path::new(path).exists()),
                _ => unreachable!("the checker guarantees a string"),
            },
            // M11.4b: añade al final del archivo (lo crea si no existe) → ["ok"] o ["err", msg].
            "__append_file" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(path), Value::Str(contents)) => match crate::builtins::append_to_file(path, contents) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("the checker guarantees two strings"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M67: operaciones de fs etiquetadas (mkdir/remove_dir/file_size/rename/copy_file) —
            // el helper compartido monta el ["ok"(, dato)]/["err", msg]; aquí solo se convierte.
            "__mkdir" | "__remove_dir" | "__file_size" | "__mtime" | "__stat" | "__rename" | "__copy_file" => {
                use crate::bytecode::FsOp;
                let op = match name {
                    "__mkdir" => FsOp::Mkdir,
                    "__remove_dir" => FsOp::RemoveDir,
                    "__file_size" => FsOp::FileSize,
                    "__mtime" => FsOp::Mtime,
                    "__stat" => FsOp::Stat,
                    "__rename" => FsOp::Rename,
                    "__copy_file" => FsOp::CopyFile,
                    _ => unreachable!(),
                };
                let args: Vec<String> = values
                    .iter()
                    .map(|v| match v {
                        Value::Str(s) => s.clone(),
                        _ => unreachable!("the checker guarantees strings"),
                    })
                    .collect();
                let arr = crate::builtins::fs_tagged(op, &args).into_iter().map(Value::Str).collect();
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M67: tests totales de fs → bool.
            "__is_dir" | "__is_file" => {
                use crate::bytecode::FsTest;
                let t = if name == "__is_dir" { FsTest::IsDir } else { FsTest::IsFile };
                match &values[0] {
                    Value::Str(path) => Value::Bool(crate::builtins::fs_test(t, path)),
                    _ => unreachable!("the checker guarantees a string"),
                }
            }
            // M67: append binario → ["ok"] o ["err", msg].
            "__append_file_bytes" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(path), Value::Bytes(data)) => {
                        match crate::builtins::append_bytes_to_file(path, data) {
                            Ok(()) => vec![Value::Str("ok".to_string())],
                            Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                        }
                    }
                    _ => unreachable!("the checker guarantees string, bytes"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.7c: borra un archivo → ["ok"] o ["err", msg].
            "__remove_file" => {
                let arr = match &values[0] {
                    Value::Str(path) => match std::fs::remove_file(path) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("the checker guarantees a string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.7c: lista un directorio → ["ok", n0, …] o ["err", msg].
            "__list_dir" => {
                let arr = match &values[0] {
                    Value::Str(path) => match crate::builtins::list_dir(path) {
                        Ok(names) => {
                            let mut v = vec![Value::Str("ok".to_string())];
                            v.extend(names.into_iter().map(Value::Str));
                            v
                        }
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("the checker guarantees a string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.8: abre un archivo → ["ok", handle] o ["err", msg].
            "__open" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(path), Value::Str(mode)) => match crate::builtins::open_file(path, mode) {
                        Ok(h) => vec![Value::Str("ok".to_string()), Value::Str(h.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees two strings"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.8: lee una línea del handle → [] (EOF) o [linea].
            "__read_line_handle" => match &values[0] {
                Value::Int(h) => {
                    let elems = crate::builtins::read_line_handle(*h).map(|l| vec![Value::Str(l)]).unwrap_or_default();
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("the checker guarantees an int"),
            },
            // M113: lee hasta `max` octetos del handle → [b"ok", datos] | [b"eof"] | [b"err", msg].
            "__read_bytes_handle" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(h), Value::Int(max)) => match crate::builtins::read_bytes_handle(*h, *max) {
                        Ok(Some(data)) => vec![bytes_tag("ok"), Value::Bytes(Rc::new(data))],
                        Ok(None) => vec![bytes_tag("eof")],
                        Err(e) => vec![bytes_tag("err"), bytes_of_str(&e)],
                    },
                    _ => unreachable!("the checker guarantees two ints"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M113: mueve la posición del handle → ["ok", nueva_pos] o ["err", msg].
            "__seek_handle" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(h), Value::Int(pos)) => match crate::builtins::seek_handle(*h, *pos) {
                        Ok(p) => vec![Value::Str("ok".to_string()), Value::Str(p.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees two ints"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.8: escribe en el handle → ["ok"] o ["err", msg].
            "__write_handle" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(h), Value::Str(s)) => match crate::builtins::write_handle(*h, s) {
                        Ok(_) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees int, string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M115.1: escritura binaria en el handle → ["ok"] o ["err", msg].
            "__write_bytes_handle" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(h), Value::Bytes(data)) => match crate::builtins::write_bytes_handle(*h, data) {
                        Ok(_) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees int, bytes"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M115.1: fsync del handle → ["ok"] o ["err", msg].
            "__sync_handle" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::sync_handle(*h) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M115.2: candado consultivo del archivo → ["ok","1"/"0"] / ["ok"] / ["err", msg].
            "__try_lock_handle" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::try_lock_handle(*h) {
                        Ok(got) => vec![
                            Value::Str("ok".to_string()),
                            Value::Str(if got { "1" } else { "0" }.to_string()),
                        ],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M115.3: chmod → ["ok"] o ["err", msg].
            "__chmod" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(path), Value::Int(mode)) => match crate::builtins::chmod_path(path, *mode) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees string, int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M115.4: watch de fs. El intérprete (oráculo) BLOQUEA el hilo esperando el evento.
            "__watch" => {
                let arr = match &values[0] {
                    Value::Str(path) => match crate::builtins::watch_open(path) {
                        Ok(id) => vec![Value::Str("ok".to_string()), Value::Str(id.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees a string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__watch_next" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(h), Value::Int(ms)) => match crate::builtins::watch_next_blocking(*h, *ms) {
                        Ok(Some((kind, path))) => {
                            vec![Value::Str("ok".to_string()), Value::Str(kind), Value::Str(path)]
                        }
                        Ok(None) => vec![Value::Str("timeout".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees two ints"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__unlock_handle" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::unlock_handle(*h) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // std/io (M107.1): stdout/stderr sin salto + flush → ["ok"] o ["err", msg].
            "__stdout_write" | "__stderr_write" => {
                let arr = match &values[0] {
                    Value::Str(s) => {
                        let r = if name == "__stdout_write" {
                            crate::builtins::stdout_write(s)
                        } else {
                            crate::builtins::stderr_write(s)
                        };
                        match r {
                            Ok(()) => vec![Value::Str("ok".to_string())],
                            Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                        }
                    }
                    _ => unreachable!("the checker guarantees a string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__stdout_write_bytes" => {
                let arr = match &values[0] {
                    Value::Bytes(b) => match crate::builtins::stdout_write_bytes(b.as_slice()) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees bytes"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__stdout_flush" => {
                let arr = match crate::builtins::stdout_flush() {
                    Ok(()) => vec![Value::Str("ok".to_string())],
                    Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // std/io (M107.2): lectura de stdin por bytes. El intérprete es el oráculo M:1 →
            // bloquear el hilo es correcto; el aparcado de fibras es cosa de la VM.
            "__stdin_read" => match &values[0] {
                Value::Int(max) => {
                    // Un error de lectura (stdin cerrado/EBADF) se reporta como EOF: el fin de
                    // la entrada, sin panico — misma politica en la VM.
                    let arr = match crate::builtins::stdin_read(*max) {
                        Ok(b) if !b.is_empty() => vec![Value::Bytes(Rc::new(b))],
                        _ => vec![], // EOF (o error de lectura)
                    };
                    Value::Array(Rc::new(RefCell::new(arr)))
                }
                _ => unreachable!("the checker guarantees an int"),
            },
            "__stdin_read_timeout" => match (&values[0], &values[1]) {
                (Value::Int(max), Value::Int(ms)) => {
                    let arr = if crate::builtins::stdin_ready((*ms).clamp(0, i32::MAX as i64) as i32) {
                        match crate::builtins::stdin_read(*max) {
                            Ok(b) if !b.is_empty() => vec![Value::Bytes(Rc::new(b"data".to_vec())), Value::Bytes(Rc::new(b))],
                            _ => vec![Value::Bytes(Rc::new(b"eof".to_vec()))], // EOF (o error)
                        }
                    } else {
                        vec![Value::Bytes(Rc::new(b"timeout".to_vec()))]
                    };
                    Value::Array(Rc::new(RefCell::new(arr)))
                }
                _ => unreachable!("the checker guarantees ints"),
            },
            // std/term (M107.3): isatty / tamaño / modo crudo.
            "__term_is_tty" => match &values[0] {
                Value::Int(fd) => Value::Bool(crate::builtins::term_is_tty(*fd)),
                _ => unreachable!("the checker guarantees an int"),
            },
            "__term_size" => {
                let arr = match crate::builtins::term_size() {
                    Some((c, r)) => vec![Value::Int(c), Value::Int(r)],
                    None => vec![],
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__audio_open" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(rate), Value::Int(ch)) => match crate::builtins::audio_open(*rate, *ch) {
                        Ok(id) => vec![Value::Str("ok".to_string()), Value::Str(id.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees two ints"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__audio_drain" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::audio_drain(*h) {
                        Ok(()) => vec![Value::Str("ok".to_string()), Value::Str(String::new())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__term_size_px" => {
                let arr = match crate::builtins::term_size_px() {
                    Some((w, h)) => vec![Value::Int(w), Value::Int(h)],
                    None => vec![],
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M146 (std/ui): ventana + webview. En el intérprete la espera de eventos BLOQUEA
            // el hilo (oráculo secuencial), como __watch_next.
            "__ui_open" => {
                let arr = match (&values[0], &values[1], &values[2], &values[3]) {
                    (Value::Str(title), Value::Str(url), Value::Int(w), Value::Int(h)) => {
                        match crate::builtins::ui_open(title, url, *w, *h) {
                            Ok(id) => vec![Value::Str("ok".to_string()), Value::Str(id.to_string())],
                            Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                        }
                    }
                    _ => unreachable!("the checker guarantees (string, string, int, int)"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__ui_eval_js" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(h), Value::Str(js)) => match crate::builtins::ui_eval_js(*h, js) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees (int, string)"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__ui_menu" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(title), Value::Array(items)) => {
                        let items: Vec<String> = items
                            .borrow()
                            .iter()
                            .map(|v| match v {
                                Value::Str(s) => s.clone(),
                                _ => unreachable!("the checker guarantees [string]"),
                            })
                            .collect();
                        match crate::builtins::ui_menu(title, &items) {
                            Ok(()) => vec![Value::Str("ok".to_string())],
                            Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                        }
                    }
                    _ => unreachable!("the checker guarantees (string, [string])"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__ui_dialog" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(kind), Value::Str(arg)) => match crate::builtins::ui_dialog(kind, arg) {
                        Ok(Some(path)) => vec![Value::Str("ok".to_string()), Value::Str(path)],
                        Ok(None) => vec![Value::Str("none".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees (string, string)"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
                        "__ui_next_event" => {
                let arr = match &values[0] {
                    Value::Int(ms) => match crate::builtins::ui_next_blocking(*ms) {
                        Ok(Some((kind, window, tag))) => vec![
                            Value::Str("ok".to_string()),
                            Value::Str(kind),
                            Value::Str(window.to_string()),
                            Value::Str(tag),
                        ],
                        Ok(None) => vec![Value::Str("timeout".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__term_raw_on" | "__term_raw_off" => {
                let r = if name == "__term_raw_on" {
                    crate::builtins::term_raw_on()
                } else {
                    crate::builtins::term_raw_off()
                };
                let arr = match r {
                    Ok(()) => vec![Value::Str("ok".to_string())],
                    Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M15.2: conecta por TCP → ["ok", handle] o ["err", msg].
            // Diferido JSON-1: code point → char ([] si inválido). El inverso de char_code.
            "__char_from_code" => match &values[0] {
                Value::Int(n) => {
                    // El guard de rango evita que un int enorme haga wrap al castear a u32.
                    let arr = if (0..=0x10FFFF).contains(n) {
                        char::from_u32(*n as u32).map(|c| vec![Value::Char(c)]).unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    Value::Array(Rc::new(RefCell::new(arr)))
                }
                _ => unreachable!("the checker guarantees an int"),
            },
            // M54.1: bits IEEE 754 de un float, y el inverso. Totales.
            "__float_bits" => match &values[0] {
                Value::Float(f) => Value::Int(f.to_bits() as i64),
                _ => unreachable!("the checker guarantees a float"),
            },
            "__float_from_bits" => match &values[0] {
                Value::Int(n) => Value::Float(f64::from_bits(*n as u64)),
                _ => unreachable!("the checker guarantees an int"),
            },
            // M53.3: SQLite embebido → arreglo etiquetado; el paquete `db/sqlite` lo traduce a Result.
            "__sqlite_open" => {
                let arr = match &values[0] {
                    Value::Str(path) => match crate::builtins::sqlite_open(path) {
                        Ok(h) => vec![Value::Str("ok".to_string()), Value::Str(h.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees a string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__sqlite_exec" | "__sqlite_query" => {
                let (Value::Int(h), Value::Str(sql), Value::Array(ps)) = (&values[0], &values[1], &values[2]) else {
                    unreachable!("the checker guarantees int, string, [string]");
                };
                let params: Vec<String> = ps.borrow().iter().map(|v| match v {
                    Value::Str(s) => s.clone(),
                    _ => unreachable!("the checker guarantees [string]"),
                }).collect();
                let arr = if name == "__sqlite_exec" {
                    match crate::builtins::sqlite_exec(*h, sql, &params) {
                        Ok(n) => vec![Value::Str("ok".to_string()), Value::Str(n.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    }
                } else {
                    match crate::builtins::sqlite_query(*h, sql, &params) {
                        Ok((ncols, cells)) => {
                            let mut v = vec![Value::Str("ok".to_string()), Value::Str(ncols.to_string())];
                            v.extend(cells.into_iter().map(Value::Str));
                            v
                        }
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    }
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__tcp_connect" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(host), Value::Int(port)) => match crate::builtins::tcp_connect(host, *port) {
                        Ok(h) => vec![Value::Str("ok".to_string()), Value::Str(h.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees string, int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M124: el resumen del certificado del peer TLS → ["ok", subject, issuer, nb, na, san...].
            "__tls_peer_cert" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::tls_peer_cert(*h) {
                        Ok(s) => {
                            let mut v = vec![
                                Value::Str("ok".to_string()),
                                Value::Str(s.subject),
                                Value::Str(s.issuer),
                                Value::Str(s.not_before_ms.to_string()),
                                Value::Str(s.not_after_ms.to_string()),
                            ];
                            v.extend(s.san.into_iter().map(Value::Str));
                            v
                        }
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M123: la dirección del peer de una conexión TCP/TLS → ["ok", "ip:puerto"] / ["err", msg].
            // M130: half-close — shutdown(SHUT_WR); el peer ve EOF, este lado sigue leyendo.
            "__socket_shutdown_write" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::shutdown_write(*h) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            "__peer_addr" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::peer_addr(*h) {
                        Ok(a) => vec![Value::Str("ok".to_string()), Value::Str(a)],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M122: connect con PLAZO — el intento vencido devuelve el error estable "connect timeout".
            "__tcp_connect_timeout" => {
                let arr = match (&values[0], &values[1], &values[2]) {
                    (Value::Str(host), Value::Int(port), Value::Int(ms)) => {
                        match crate::builtins::tcp_connect_timeout(host, *port, *ms) {
                            Ok(h) => vec![Value::Str("ok".to_string()), Value::Str(h.to_string())],
                            Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                        }
                    }
                    _ => unreachable!("the checker guarantees string, int, int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M19.4a: abre una conexión TLS → ["ok", handle] o ["err", msg].
            "__tls_connect" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(host), Value::Int(port)) => match crate::builtins::tls_connect(host, *port) {
                        Ok(h) => vec![Value::Str("ok".to_string()), Value::Str(h.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees string, int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M31.2a: conexión TLS con ALPN h2 → ["ok", handle]/["err", msg].
            "__tls_connect_h2" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(host), Value::Int(port)) => match crate::builtins::tls_connect_h2(host, *port) {
                        Ok(h) => vec![Value::Str("ok".to_string()), Value::Str(h.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees string, int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M19.4b: envuelve un socket aceptado en una sesión TLS de servidor → ["ok", handle]/["err",…].
            "__tls_accept" => {
                let arr = match (&values[0], &values[1], &values[2]) {
                    (Value::Int(h), Value::Str(cert), Value::Str(key)) => match crate::builtins::tls_accept(*h, cert, key) {
                        Ok(nh) => vec![Value::Str("ok".to_string()), Value::Str(nh.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees int, string, string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // Diferido TLS: STARTTLS de cliente sobre un TCP plano → ["ok", handle] o ["err", msg].
            "__tls_upgrade" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(h), Value::Str(host)) => match crate::builtins::tls_upgrade(*h, host) {
                        Ok(nh) => vec![Value::Str("ok".to_string()), Value::Str(nh.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees int, string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M15.2: lee del socket → ["ok", datos] o ["err", msg].
            "__socket_read" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::socket_read(*h) {
                        Ok(s) => vec![Value::Str("ok".to_string()), Value::Str(s)],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M15.2: escribe en el socket → ["ok", ""] o ["err", msg].
            "__socket_write" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(h), Value::Str(s)) => match crate::builtins::socket_write(*h, s) {
                        Ok(_) => vec![Value::Str("ok".to_string()), Value::Str(String::new())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees int, string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M15.3: bind+listen → ["ok", handle] o ["err", msg].
            "__tcp_listen" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(host), Value::Int(port)) => match crate::builtins::tcp_listen(host, *port) {
                        Ok(h) => vec![Value::Str("ok".to_string()), Value::Str(h.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees string, int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M15.3: acepta una conexión → ["ok", handle] o ["err", msg].
            "__tcp_accept" => {
                let arr = match &values[0] {
                    Value::Int(h) => match crate::builtins::tcp_accept(*h) {
                        Ok(c) => vec![Value::Str("ok".to_string()), Value::Str(c.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("the checker guarantees an int"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M15.3 (M50.3: __x): puerto local del socket (total).
            "__local_port" => match &values[0] {
                Value::Int(h) => Value::Int(crate::builtins::local_port(*h)),
                _ => unreachable!("the checker guarantees an int"),
            },
            // M56.4: timeout de lectura del socket (total; en este motor aplica el SO_RCVTIMEO real).
            "__socket_set_read_timeout" => match (&values[0], &values[1]) {
                (Value::Int(h), Value::Int(ms)) => {
                    crate::builtins::socket_set_read_timeout(*h, *ms);
                    Value::Unit
                }
                _ => unreachable!("the checker guarantees int, int"),
            },
            // M11.8: cierra el handle (total).
            "close" => match &values[0] {
                Value::Int(h) => {
                    crate::builtins::close_handle(*h);
                    Value::Int(0)
                }
                _ => unreachable!("the checker guarantees an int"),
            },
            // --- Matemáticas (M15.1a) ---
            // Funciones unarias float -> float: el nombre fija qué MathFn aplicar; el cálculo lo hace
            // `builtins::apply_mathf` (compartido con la VM → el oráculo cuadra, incl. NaN/inf).
            "__sqrt" | "__sin" | "__cos" | "__tan" | "__ln" | "__log10" | "__exp" | "__floor" | "__ceil" | "__round"
            | "__asin" | "__acos" | "__atan" | "__log2" | "__trunc" => {
                let f = match name {
                    "__sqrt" => MathFn::Sqrt,
                    "__sin" => MathFn::Sin,
                    "__cos" => MathFn::Cos,
                    "__tan" => MathFn::Tan,
                    "__ln" => MathFn::Ln,
                    "__log10" => MathFn::Log10,
                    "__exp" => MathFn::Exp,
                    "__floor" => MathFn::Floor,
                    "__ceil" => MathFn::Ceil,
                    "__round" => MathFn::Round,
                    // M65.2
                    "__asin" => MathFn::Asin,
                    "__acos" => MathFn::Acos,
                    "__atan" => MathFn::Atan,
                    "__log2" => MathFn::Log2,
                    "__trunc" => MathFn::Trunc,
                    _ => unreachable!(),
                };
                match &values[0] {
                    Value::Float(x) => Value::Float(crate::builtins::apply_mathf(f, *x)),
                    _ => unreachable!("the checker guarantees a float"),
                }
            }
            "__pow" => match (&values[0], &values[1]) {
                (Value::Float(b), Value::Float(e)) => Value::Float(b.powf(*e)),
                _ => unreachable!("the checker guarantees two floats"),
            },
            // M65.2: atan2(y, x) — el ángulo de (x, y) en (-π, π].
            "__atan2" => match (&values[0], &values[1]) {
                (Value::Float(y), Value::Float(x)) => Value::Float(y.atan2(*x)),
                _ => unreachable!("the checker guarantees two floats"),
            },
            // M49.1b: abs/min/max/pi/e ya no son builtins (funciones puras en `std/math`).
            // --- Reloj y aleatoriedad (M15.1b): no deterministas, delegan en los helpers compartidos. ---
            "__now" => Value::Int(crate::builtins::now_millis()),
            "__ffi_errno" => Value::Int(crate::ffi::errno()),
            "__monotonic" => Value::Int(crate::builtins::monotonic_millis()),
            "__monotonic_nanos" => Value::Int(crate::builtins::monotonic_nanos()),
            "__sleep" => match &values[0] {
                Value::Int(ms) => {
                    crate::builtins::sleep_millis(*ms);
                    Value::Unit
                }
                _ => unreachable!("the checker guarantees an int"),
            },
            "__random" => Value::Float(crate::builtins::random_f64()),
            "__random_int" => match &values[0] {
                Value::Int(n) => Value::Int(crate::builtins::random_int(*n)),
                _ => unreachable!("the checker guarantees an int"),
            },
            // M68.1: fija la semilla del PRNG (reproducibilidad).
            "__random_seed" => match &values[0] {
                Value::Int(n) => {
                    crate::builtins::random_seed(*n);
                    Value::Unit
                }
                _ => unreachable!("the checker guarantees an int"),
            },
            _ => unreachable!("builtin unknown"),
        }
    }

    fn eval_binary(
        &mut self,
        op: BinaryOp,
        left: &'a Expr,
        right: &'a Expr,
        line: usize,
        col: usize,
    ) -> EvalResult {
        use BinaryOp::*;

        // Los operadores lógicos hacen *cortocircuito*: no evalúan la derecha si la
        // izquierda ya decide el resultado. Por eso se tratan aparte.
        match op {
            And => {
                if !self.eval_bool(left)? {
                    return Ok(Value::Bool(false));
                }
                return Ok(Value::Bool(self.eval_bool(right)?));
            }
            Or => {
                if self.eval_bool(left)? {
                    return Ok(Value::Bool(true));
                }
                return Ok(Value::Bool(self.eval_bool(right)?));
            }
            _ => {}
        }

        let l = self.eval_expr(left)?;
        let r = self.eval_expr(right)?;
        use Value::*;
        Ok(match (op, l, r) {
            // Concatenación de strings (M11.1a): `+` sobre dos strings.
            (Add, Str(a), Str(b)) => Str(a + &b),
            // M16.1b: concatenación de bytes → bytes nuevo.
            (Add, Bytes(a), Bytes(b)) => {
                let mut v = (*a).clone();
                v.extend_from_slice(&b);
                Bytes(Rc::new(v))
            }
            // Concatenación de arreglos (M11.7b): `+` sobre dos arreglos → arreglo nuevo.
            (Add, Array(a), Array(b)) => {
                let mut v = a.borrow().clone();
                v.extend(b.borrow().iter().cloned());
                Array(Rc::new(RefCell::new(v)))
            }
            // Aritmética entera. El desbordamiento es ERROR de ejecución (M34, SPEC §8):
            // coherente con la división por cero y con el eje "seguro" — antes el
            // comportamiento dependía del build del compilador (panic en debug, wrap en
            // release). Los u8/u32/u64 siguen con wrapping POR DISEÑO (M28.3).
            (Add, Int(a), Int(b)) => Int(a.checked_add(b).ok_or_else(|| runtime_error(line, col, "arithmetic overflow on int"))?),
            (Sub, Int(a), Int(b)) => Int(a.checked_sub(b).ok_or_else(|| runtime_error(line, col, "arithmetic overflow on int"))?),
            (Mul, Int(a), Int(b)) => Int(a.checked_mul(b).ok_or_else(|| runtime_error(line, col, "arithmetic overflow on int"))?),
            (Div, Int(a), Int(b)) => {
                if b == 0 {
                    return Err(runtime_error(line, col, "integer division by zero"));
                }
                // El único desbordamiento de división: i64::MIN / -1.
                Int(a.checked_div(b).ok_or_else(|| runtime_error(line, col, "arithmetic overflow on int"))?)
            }
            (Rem, Int(a), Int(b)) => {
                if b == 0 {
                    return Err(runtime_error(line, col, "modulo by zero"));
                }
                Int(a.checked_rem(b).ok_or_else(|| runtime_error(line, col, "arithmetic overflow on int"))?)
            }
            // Aritmética flotante (división por cero da inf/NaN, como IEEE-754).
            (Add, Float(a), Float(b)) => Float(a + b),
            (Sub, Float(a), Float(b)) => Float(a - b),
            (Mul, Float(a), Float(b)) => Float(a * b),
            (Div, Float(a), Float(b)) => Float(a / b),
            (Rem, Float(a), Float(b)) => Float(a % b),
            // Orden (números del mismo tipo).
            (Lt, Int(a), Int(b)) => Bool(a < b),
            (Le, Int(a), Int(b)) => Bool(a <= b),
            (Gt, Int(a), Int(b)) => Bool(a > b),
            (Ge, Int(a), Int(b)) => Bool(a >= b),
            (Lt, Float(a), Float(b)) => Bool(a < b),
            (Le, Float(a), Float(b)) => Bool(a <= b),
            (Gt, Float(a), Float(b)) => Bool(a > b),
            (Ge, Float(a), Float(b)) => Bool(a >= b),
            // M11.7d: orden de strings (lexicográfico) y char (por code point).
            (Lt, Str(a), Str(b)) => Bool(a < b),
            (Le, Str(a), Str(b)) => Bool(a <= b),
            (Gt, Str(a), Str(b)) => Bool(a > b),
            (Ge, Str(a), Str(b)) => Bool(a >= b),
            (Lt, Char(a), Char(b)) => Bool(a < b),
            (Le, Char(a), Char(b)) => Bool(a <= b),
            (Gt, Char(a), Char(b)) => Bool(a > b),
            (Ge, Char(a), Char(b)) => Bool(a >= b),
            // Igualdad (mismo tipo, garantizado por el checker).
            (Eq, a, b) => Bool(a == b),
            (Ne, a, b) => Bool(a != b),
            // Bit a bit (M19.3a): sobre i64. Los desplazamientos usan `wrapping_*` con el
            // contador como u32 → deterministas y SIN panic (en debug, `<<` con cuenta ≥64
            // o negativa abortaría); ambos motores comparten exactamente esta semántica.
            (BitAnd, Int(a), Int(b)) => Int(a & b),
            (BitOr, Int(a), Int(b)) => Int(a | b),
            (BitXor, Int(a), Int(b)) => Int(a ^ b),
            (Shl, Int(a), Int(b)) => Int(a.wrapping_shl(b as u32)),
            (Shr, Int(a), Int(b)) => Int(a.wrapping_shr(b as u32)),
            // M28.3: enteros sin signo con tamaño. Ambos operandos comparten ancho (garantía del
            // checker); la aritmética envuelve dentro del ancho (`make_uint` enmascara). División y
            // módulo sin signo; desplazamientos lógicos.
            (Add, UInt(a, w), UInt(b, _)) => make_uint(a.wrapping_add(b), w),
            (Sub, UInt(a, w), UInt(b, _)) => make_uint(a.wrapping_sub(b), w),
            (Mul, UInt(a, w), UInt(b, _)) => make_uint(a.wrapping_mul(b), w),
            (Div, UInt(a, w), UInt(b, _)) => {
                if b == 0 { return Err(runtime_error(line, col, "integer division by zero")); }
                make_uint(a / b, w)
            }
            (Rem, UInt(a, w), UInt(b, _)) => {
                if b == 0 { return Err(runtime_error(line, col, "modulo by zero")); }
                make_uint(a % b, w)
            }
            (Lt, UInt(a, _), UInt(b, _)) => Bool(a < b),
            (Le, UInt(a, _), UInt(b, _)) => Bool(a <= b),
            (Gt, UInt(a, _), UInt(b, _)) => Bool(a > b),
            (Ge, UInt(a, _), UInt(b, _)) => Bool(a >= b),
            (BitAnd, UInt(a, w), UInt(b, _)) => make_uint(a & b, w),
            (BitOr, UInt(a, w), UInt(b, _)) => make_uint(a | b, w),
            (BitXor, UInt(a, w), UInt(b, _)) => make_uint(a ^ b, w),
            (Shl, UInt(a, w), UInt(b, _)) => make_uint(a.wrapping_shl(b as u32), w),
            (Shr, UInt(a, w), UInt(b, _)) => make_uint(a.wrapping_shr(b as u32), w),
            _ => unreachable!("operator/operand combination that the checker should have rejected"),
        })
    }

    // ----- Auxiliares -----

    /// Evalúa una expresión que el checker garantizó booleana, y extrae el `bool`.
    fn eval_bool(&mut self, expr: &'a Expr) -> Result<bool, Flow> {
        match self.eval_expr(expr)? {
            Value::Bool(b) => Ok(b),
            _ => unreachable!("the checker guarantees a boolean condition"),
        }
    }

    /// Evalúa una expresión que el checker garantizó `int`.
    fn eval_int(&mut self, expr: &'a Expr) -> Result<i64, Flow> {
        match self.eval_expr(expr)? {
            Value::Int(n) => Ok(n),
            _ => unreachable!("the checker guarantees an int"),
        }
    }

    /// Evalúa una expresión que el checker garantizó arreglo y devuelve su `Rc`
    /// (compartido: mutar a través de él afecta a todos los alias).
    fn eval_array(&mut self, expr: &'a Expr) -> Result<Rc<RefCell<Vec<Value>>>, Flow> {
        match self.eval_expr(expr)? {
            Value::Array(rc) => Ok(rc),
            _ => unreachable!("the checker guarantees an array"),
        }
    }

    /// Evalúa una expresión que el checker garantizó struct y devuelve su `Rc`.
    fn eval_struct(&mut self, expr: &'a Expr) -> Result<Rc<RefCell<StructInstance>>, Flow> {
        match self.eval_expr(expr)? {
            Value::Struct(rc) => Ok(rc),
            _ => unreachable!("the checker guarantees a struct"),
        }
    }

    /// Declara una variable: una **celda nueva**. Como cada declaración crea su
    /// propia celda, el shadowing es seguro aunque una closure haya capturado la
    /// celda anterior (se queda con la vieja).
    fn define(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("there is always an active scope")
            .insert(name.to_string(), Rc::new(RefCell::new(value)));
    }

    /// Busca una variable de dentro hacia afuera; `None` si no es una variable (en
    /// ese caso, en `eval` el nombre se interpreta como una función).
    fn lookup_opt(&self, name: &str) -> Option<Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(cell) = scope.get(name) {
                return Some(cell.borrow().clone());
            }
        }
        None
    }

    /// Asigna a una variable **mutando su celda** (no reemplazándola): así el cambio
    /// se ve a través de cualquier closure que la haya capturado.
    fn assign(&mut self, name: &str, value: Value) {
        for scope in self.scopes.iter().rev() {
            if let Some(cell) = scope.get(name) {
                *cell.borrow_mut() = value;
                return;
            }
        }
        unreachable!("the checker guarantees that '{}' is declared", name)
    }
}

fn runtime_error(line: usize, col: usize, msg: &str) -> Flow {
    Flow::Error(RuntimeError { msg: msg.to_string(), line, col, trace: Vec::new() })
}

/// Convierte un `Value` de raylang a un valor de la frontera FFI (M41). El checker ya garantizó que
/// el argumento es un primitivo marshalable; `bool` va como entero (0/1).
fn value_to_ffi(v: &Value) -> crate::ffi::FfiVal<'_> {
    match v {
        Value::Int(n) => crate::ffi::FfiVal::Int(*n),
        Value::UInt(v, _) => crate::ffi::FfiVal::Int(*v as i64), // M41.4: u64 → 64 bits por registro entero
        Value::Float(f) => crate::ffi::FfiVal::Float(*f),
        Value::Bool(b) => crate::ffi::FfiVal::Int(*b as i64),
        Value::Str(s) => crate::ffi::FfiVal::Str(s.as_str()),      // M41.2: → char* (NUL-terminado en ffi::call)
        Value::Bytes(b) => crate::ffi::FfiVal::Bytes(b.as_slice()), // M41.2: → puntero al buffer crudo
        Value::Ptr(p) => crate::ffi::FfiVal::Int(*p),              // M41.4b: la dirección opaca por registro
        _ => unreachable!("the checker guarantees a type marshalable at the FFI boundary"),
    }
}

/// Construye un valor `Option` del prelude (`Some(v)`/`None`). El intérprete identifica los enums por
/// nombre, así que basta con el nombre `Option` y la variante. (M41.3)
fn opt_value(variant: &str, payload: Vec<Value>) -> Value {
    Value::Enum(Rc::new(EnumInstance {
        enum_name: "Option".to_string(),
        variant: variant.to_string(),
        payload,
    }))
}

/// Convierte el resultado de una llamada FFI al `Value` que corresponda según el tipo de retorno
/// declarado (`ret_kind`): un entero C se vuelve `bool` si el retorno era `bool`; `unit` es `Unit`; un
/// `char*` se envuelve en `Option<bytes>` (`None` si NULL) o, para `OptStr`, en `Option<string>`
/// validando UTF-8 (bytes inválidos → error de ejecución). (M41.3)
fn ffi_to_value(r: crate::ffi::FfiRet, ret: crate::ffi::CKind, line: usize, col: usize) -> EvalResult {
    use crate::ffi::{CKind, FfiRet};
    Ok(match r {
        FfiRet::Int(n) if ret == CKind::Bool => Value::Bool(n != 0),
        FfiRet::Int(n) if ret == CKind::U64 => Value::UInt(n as u64, 64), // M41.4: C long/size_t → u64
        FfiRet::Int(n) => Value::Int(n),
        FfiRet::Float(f) => Value::Float(f),
        FfiRet::Unit => Value::Unit,
        FfiRet::OptBytes(None) => opt_value("None", vec![]),
        FfiRet::OptBytes(Some(bytes)) => {
            let inner = if ret == CKind::OptStr {
                match String::from_utf8(bytes) {
                    Ok(s) => Value::Str(s),
                    Err(_) => return Err(runtime_error(line, col,
                        "the C function returned bytes that are not valid UTF-8 (declare Option<bytes> to receive them raw)")),
                }
            } else {
                Value::Bytes(Rc::new(bytes))
            };
            opt_value("Some", vec![inner])
        }
        FfiRet::Ptr(p) => Value::Ptr(p),                                  // M41.4b
        FfiRet::OptPtr(None) => opt_value("None", vec![]),
        FfiRet::OptPtr(Some(p)) => opt_value("Some", vec![Value::Ptr(p)]),
    })
}

/// Comprueba que `i` es un índice válido en `0..len`; si no, error de ejecución.
fn check_bounds(i: i64, len: usize, line: usize, col: usize) -> Result<usize, Flow> {
    if i < 0 || (i as usize) >= len {
        return Err(runtime_error(line, col, &format!("index {} out of range (length {})", i, len)));
    }
    Ok(i as usize)
}

/// M16.1c: una etiqueta (`"ok"`/`"err"`) como valor `bytes`, para los arreglos `[bytes]` homogéneos
/// que devuelven las lecturas binarias.
fn bytes_tag(tag: &str) -> Value {
    Value::Bytes(Rc::new(tag.as_bytes().to_vec()))
}

/// M16.1c: un string (p. ej. un mensaje de error) como valor `bytes`.
fn bytes_of_str(s: &str) -> Value {
    Value::Bytes(Rc::new(s.as_bytes().to_vec()))
}

/// Intenta casar un patrón (M5.2) contra un valor. Si casa, devuelve las variables a
/// ligar `(nombre, valor)`; si no, `None`. El checker garantiza que la variante y la
/// aridad son consistentes, así que aquí solo se compara la etiqueta y se reparte el
/// payload.
fn match_pattern(pat: &Pattern, value: &Value) -> Option<Vec<(String, Value)>> {
    match &pat.kind {
        PatternKind::Wildcard => Some(Vec::new()),
        PatternKind::Binding(name) => Some(vec![(name.clone(), value.clone())]),
        PatternKind::Variant { variant, subpatterns, .. } => {
            let e = match value {
                Value::Enum(e) => e,
                _ => return None, // el checker lo impide; por robustez
            };
            if e.variant != *variant {
                return None;
            }
            // M40.1c: cada sub-patrón se casa recursivamente contra su posición del payload; si
            // alguno no casa (una variante anidada que no coincide), el brazo entero falla.
            let mut binds = Vec::new();
            for (sub, v) in subpatterns.iter().zip(&e.payload) {
                binds.extend(match_pattern(sub, v)?);
            }
            Some(binds)
        }
        PatternKind::Struct { fields, .. } => {
            // M40.1d: destructura un struct. El checker garantiza el tipo; se casan los campos
            // listados (recursivo). Un campo cuyo sub-patrón no case hace fallar el brazo.
            let s = match value {
                Value::Struct(s) => s,
                _ => return None, // el checker lo impide; por robustez
            };
            let mut binds = Vec::new();
            for (fname, fpat) in fields {
                let fv = s.borrow().fields.iter().find(|(f, _)| f == fname).map(|(_, v)| v.clone());
                match fv {
                    Some(v) => binds.extend(match_pattern(fpat, &v)?),
                    None => return None, // el checker garantiza el campo; por robustez
                }
            }
            Some(binds)
        }
    }
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn run_ok(src: &str) -> Value {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        run(&prog).expect("execution without error")
    }

    fn run_err(src: &str) -> RuntimeError {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        run(&prog).expect_err("debería fallar en ejecución")
    }

    /// Ejecuta una función concreta por nombre (no solo `main`). Sirve para probar
    /// expresiones cuyo tipo no sería válido como retorno de `main` (bool, float).
    /// El fuente debe incluir esa función y un `main` (que el checker exige).
    fn run_named(src: &str, name: &str) -> Value {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        // Construimos el intérprete a mano para llamar a la función elegida.
        // Como los tests viven en el mismo módulo, accedemos a sus internos.
        let prog_ref: &'static Program = Box::leak(Box::new(prog));
        let mut interp = Interpreter::new(prog_ref);
        let func = *interp.functions.get(name).expect("la función del test existe");
        match interp.call_function(func, Vec::new(), 0, 0) {
            Ok(v) => v,
            Err(Flow::Return(v)) => v,
            Err(Flow::Error(e)) => panic!("error de ejecución inesperado: {}", e),
            Err(Flow::TailCall { .. }) => unreachable!("a tail call does not escape call_body"),
        }
    }

    #[test]
    fn arithmetic_y_precedence() {
        assert_eq!(run_ok("fn main() -> int { 1 + 2 * 3 }"), Value::Int(7));
        assert_eq!(run_ok("fn main() -> int { (1 + 2) * 3 }"), Value::Int(9));
        assert_eq!(run_ok("fn main() -> int { 10 - 2 - 3 }"), Value::Int(5));
        assert_eq!(run_ok("fn main() -> int { 17 % 5 }"), Value::Int(2));
    }

    /// Envuelve un cuerpo de expresión en `fn v() -> RET { BODY } fn main() {}` y
    /// evalúa `v`. Cómodo para probar expresiones bool/float.
    fn eval_as(ret: &str, body: &str) -> Value {
        let src = format!("fn v() -> {} {{ {} }} fn main() {{}}", ret, body);
        run_named(&src, "v")
    }

    #[test]
    fn flotantes() {
        assert_eq!(eval_as("float", "1.0 / 2.0"), Value::Float(0.5));
    }

    #[test]
    fn booleans_and_comparisons() {
        assert_eq!(eval_as("bool", "3 < 5"), Value::Bool(true));
        assert_eq!(eval_as("bool", "3 == 5"), Value::Bool(false));
        assert_eq!(eval_as("bool", "!(2 > 1)"), Value::Bool(false));
        assert_eq!(eval_as("bool", "true && false"), Value::Bool(false));
        assert_eq!(eval_as("bool", "false || true"), Value::Bool(true));
    }

    #[test]
    fn short_circuit_does_not_evaluate_right() {
        // Si '&&' NO cortocircuitara, evaluaría '1 / 0' y reventaría. Como la
        // izquierda es false, ni lo toca → resultado false, sin error.
        assert_eq!(eval_as("bool", "false && (1 / 0 == 0)"), Value::Bool(false));
        // Lo mismo con '||' y la izquierda true.
        assert_eq!(eval_as("bool", "true || (1 / 0 == 0)"), Value::Bool(true));
    }

    #[test]
    fn if_as_expression() {
        assert_eq!(run_ok("fn main() -> int { if (true) { 1 } else { 2 } }"), Value::Int(1));
        assert_eq!(
            run_ok("fn main() -> int { let x: int = -4; if (x < 0) { -x } else { x } }"),
            Value::Int(4)
        );
    }

    #[test]
    fn variables_mutation_and_while() {
        // Suma 0+1+2+3+4 = 10.
        let src = r#"
fn main() -> int {
    var i: int = 0;
    var s: int = 0;
    while (i < 5) {
        s = s + i;
        i = i + 1;
    }
    s
}
"#;
        assert_eq!(run_ok(src), Value::Int(10));
    }

    #[test]
    fn iterative_factorial() {
        let src = r#"
fn main() -> int {
    var n: int = 5;
    var f: int = 1;
    while (n > 1) {
        f = f * n;
        n = n - 1;
    }
    f
}
"#;
        assert_eq!(run_ok(src), Value::Int(120));
    }

    #[test]
    fn recursion_fibonacci() {
        let src = r#"
fn fib(n: int) -> int {
    if (n < 2) { n } else { fib(n - 1) + fib(n - 2) }
}
fn main() -> int { fib(10) }
"#;
        assert_eq!(run_ok(src), Value::Int(55));
    }

    #[test]
    fn return_val_early() {
        let src = r#"
fn sign(x: int) -> int {
    if (x < 0) { return -1; }
    if (x > 0) { return 1; }
    0
}
fn main() -> int { sign(-7) + sign(0) + sign(42) }
"#;
        // -1 + 0 + 1 = 0
        assert_eq!(run_ok(src), Value::Int(0));
    }

    #[test]
    fn lexical_scoping_function_does_not_see_caller() {
        // 'g' usa su propio parámetro 'x'; no ve el 'x' de 'main'. Si hubiera
        // scoping dinámico, esto daría otro resultado.
        let src = r#"
fn g(x: int) -> int { x + 1 }
fn main() -> int {
    let x: int = 100;
    g(5)
}
"#;
        assert_eq!(run_ok(src), Value::Int(6));
    }

    #[test]
    fn shadowing_restores_outer_value() {
        let src = r#"
fn main() -> int {
    let x: int = 1;
    { let x: int = 99; }
    x
}
"#;
        assert_eq!(run_ok(src), Value::Int(1));
    }

    #[test]
    fn division_by_zero_is_execution_error() {
        let e = run_err("fn main() -> int { 10 / 0 }");
        assert!(e.msg.contains("division"));
        let e = run_err("fn main() -> int { 10 % 0 }");
        assert!(e.msg.contains("modulo"));
    }

    // ----- M5.2: match (ejecución en el intérprete) -----

    #[test]
    fn match_traverses_recursive_list() {
        let src = r#"
enum List { Cons(int, List), Nil }
fn length(xs: List) -> int {
    match (xs) { List.Cons(_, t) => 1 + length(t), List.Nil => 0 }
}
fn sum(xs: List) -> int {
    match (xs) { List.Cons(h, t) => h + sum(t), List.Nil => 0 }
}
fn main() -> int {
    let xs: List = List.Cons(10, List.Cons(20, List.Cons(30, List.Nil)));
    length(xs) * 100 + sum(xs)
}
"#;
        assert_eq!(run_ok(src), Value::Int(360)); // 3*100 + 60
    }

    #[test]
    fn match_selects_correct_branch() {
        let src = r#"
enum Shape { Circulo(int), Rect(int, int), Punto }
fn area(f: Shape) -> int {
    match (f) {
        Shape.Circulo(r) => 3 * r * r,
        Shape.Rect(w, h) => w * h,
        Shape.Punto => 0,
    }
}
fn main() -> int { area(Shape.Rect(4, 5)) + area(Shape.Circulo(2)) + area(Shape.Punto) }
"#;
        assert_eq!(run_ok(src), Value::Int(32)); // 20 + 12 + 0
    }

    #[test]
    fn match_con_binding_catchall() {
        // El binding suelto liga el valor completo del escrutinio.
        let src = r#"
enum E { Uno, Dos, Otro }
fn n(e: E) -> int { match (e) { E.Uno => 1, E.Dos => 2, other => 99 } }
fn main() -> int { n(E.Dos) * 100 + n(E.Otro) }
"#;
        assert_eq!(run_ok(src), Value::Int(299)); // 2*100 + 99
    }

    #[test]
    fn try_propagates_and_unwraps() {
        let src = r#"
fn d(a: int, b: int) -> Result<int, string> {
    if (b == 0) { Result.Err("cero") } else { Result.Ok(a / b) }
}
fn calc(x: int, y: int, z: int) -> Result<int, string> {
    let q1: int = d(x, y)?;
    let q2: int = d(q1, z)?;
    Result.Ok(q1 + q2)
}
fn desemp(r: Result<int, string>) -> int { match (r) { Result.Ok(v) => v, Result.Err(_) => -1 } }
fn main() -> int {
    desemp(calc(100, 5, 2)) * 100 + desemp(calc(100, 0, 2))   // 30*100 + (-1)
}
"#;
        assert_eq!(run_ok(src), Value::Int(2999));
    }

    #[test]
    fn try_option_none_propagates() {
        let src = r#"
fn primero(xs: [int]) -> Option<int> { if (xs.len() == 0) { Option.None } else { Option.Some(xs[0]) } }
fn mas_one(xs: [int]) -> Option<int> { let v: int = primero(xs)?; Option.Some(v + 1) }
fn desemp(o: Option<int>) -> int { match (o) { Option.Some(v) => v, Option.None => -99 } }
fn main() -> int { desemp(mas_one([41])) * 100 + desemp(mas_one([])) }
"#;
        assert_eq!(run_ok(src), Value::Int(4101)); // 42*100 + (-99)
    }

    #[test]
    fn match_body_constructs_enum() {
        // El cuerpo de un brazo puede construir otra variante (resolución dentro del
        // brazo): comprobamos la cadena completa devolviendo un int derivado.
        let src = r#"
enum Sem { Rojo, Verde }
fn opuesto(s: Sem) -> Sem { match (s) { Sem.Rojo => Sem.Verde, Sem.Verde => Sem.Rojo } }
fn a_int(s: Sem) -> int { match (s) { Sem.Rojo => 0, Sem.Verde => 1 } }
fn main() -> int { a_int(opuesto(Sem.Rojo)) }
"#;
        assert_eq!(run_ok(src), Value::Int(1));
    }
}
