//! Compilador de raylang: AST → bytecode (M2, ampliado en M4).
//!
//! Recorre el AST (ya verificado por el checker) y *emite* instrucciones. Es un
//! recorrido en **post-orden**: para un nodo binario, primero compila sus hijos
//! (que dejan sus valores en la pila) y luego emite la operación, que los consume.
//!
//! ## Variables locales (M2.3)
//!
//! Cada variable se asigna a un **slot** dentro del marco de su función. A
//! diferencia de clox (que guarda las locales en la pila de operandos), aquí las
//! locales viven en un arreglo aparte por marco; la pila de operandos solo guarda
//! temporales.
//!
//! ## Closures y upvalues (M4.2)
//!
//! Para compilar funciones anidadas y resolver la **captura de entorno**, el
//! compilador mantiene una **cadena de ámbitos de función** (`scopes`): el último
//! es la función que se compila ahora; los anteriores, sus envolventes. Cuando el
//! cuerpo nombra una variable que no es local suya, se busca como *upvalue* en la
//! función envolvente (o, transitivamente, entre los upvalues de aquélla). Esa
//! resolución —al estilo clox— marca qué locales del marco envolvente deben
//! *boxearse* (vivir en una celda compartida).
//!
//! Como el intérprete, el compilador asume entrada verificada: confía en los tipos
//! y en que toda variable/función existe.

use std::collections::HashMap;

use crate::ast::*;
use crate::bytecode::{
    CastTarget, Chunk, CmpOp, CompiledEnum, CompiledFn, CompiledProgram, CompiledStruct, CompiledVariant,
    OpCode, UpvalueRef, UpvalueSource,
};
use crate::runtime::Value;

/// Mapa de structs para el compilador: nombre → (índice en la tabla, nombres de
/// campo en orden de declaración).
type StructDefs = HashMap<String, (usize, Vec<String>)>;

/// Mapa de enums para el compilador: nombre del enum → (`enum_id`, mapa variante →
/// (`tag`, aridad)). Resuelve un `EnumLit` a los índices que necesita `MakeEnum`.
type EnumDefs = HashMap<String, (usize, HashMap<String, (usize, usize)>)>;

#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error de compilación en {}:{}: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for CompileError {}

/// Compila un programa completo a bytecode.
pub fn compile_program(program: &Program) -> Result<CompiledProgram, CompileError> {
    // Pre-pasada: nombre de función → índice (igual que en el checker).
    let mut indices = HashMap::new();
    for (i, f) in program.functions.iter().enumerate() {
        indices.insert(f.name.clone(), i);
    }
    let main = *indices.get("main").expect("the checker guarantees 'main'");

    // Tabla de structs + mapa nombre → (índice, nombres de campo en orden).
    let mut struct_table = Vec::new();
    let mut struct_defs: StructDefs = HashMap::new();
    for (i, s) in program.structs.iter().enumerate() {
        let field_names: Vec<String> = s.fields.iter().map(|(n, _)| n.clone()).collect();
        struct_defs.insert(s.name.clone(), (i, field_names.clone()));
        struct_table.push(CompiledStruct { name: s.name.clone(), fields: field_names });
    }

    // Tabla de enums + mapa nombre → (enum_id, variante → (tag, aridad)). El orden de
    // las variantes fija su tag.
    let mut enum_table = Vec::new();
    let mut enum_defs: EnumDefs = HashMap::new();
    for (ei, e) in program.enums.iter().enumerate() {
        let mut variant_map = HashMap::new();
        let mut variants = Vec::new();
        for (vi, v) in e.variants.iter().enumerate() {
            variant_map.insert(v.name.clone(), (vi, v.payload.len()));
            variants.push(CompiledVariant { name: v.name.clone(), arity: v.payload.len() });
        }
        enum_defs.insert(e.name.clone(), (ei, variant_map));
        enum_table.push(CompiledEnum { name: e.name.clone(), variants });
    }

    // Las funciones nombradas ocupan los índices 0..N; las anónimas, N + id (M4.1).
    // Al compilar el cuerpo de cada nombrada, las fn-exprs anidadas se compilan en
    // línea (recursivamente), así que basta recorrer las nombradas.
    let n_named = program.functions.len();
    let total = n_named + collect_fn_exprs(program).len();

    // M27.5: valores de las constantes de nivel superior (evaluados de sus literales).
    let mut consts = HashMap::new();
    for cst in &program.consts {
        consts.insert(cst.name.clone(), crate::runtime::eval_const_literal(&cst.value));
    }

    // M41: tabla de funciones externas (FFI) + mapa nombre → índice (para bajar `CallExtern`).
    let mut externs = Vec::new();
    let mut extern_indices = HashMap::new();
    for e in &program.externs {
        if let Some(d) = crate::ffi::desc_of(e) {
            extern_indices.insert(e.name.clone(), externs.len());
            externs.push(d);
        }
    }

    let mut c = Compiler {
        indices: &indices,
        structs: &struct_defs,
        enums: &enum_defs,
        n_named,
        functions: (0..total).map(|_| None).collect(),
        scopes: Vec::new(),
        consts,
        extern_indices: &extern_indices,
    };
    for (i, f) in program.functions.iter().enumerate() {
        c.compile_function(i, &f.params, &f.body, f.line, f.col, f.name.clone())?;
    }

    let functions = c.functions.into_iter().map(|o| o.expect("toda función quedó compilada")).collect();
    Ok(CompiledProgram { functions, structs: struct_table, enums: enum_table, main, externs })
}

/// Compila una expresión suelta a un `Chunk` (sin variables ni llamadas). Se usa
/// en los tests de expresiones puras.
pub fn compile_expr(expr: &Expr) -> Result<Chunk, CompileError> {
    let indices = HashMap::new();
    let structs = HashMap::new();
    let enums = HashMap::new();
    let extern_indices = HashMap::new();
    let mut c = Compiler { indices: &indices, structs: &structs, enums: &enums, n_named: 0, functions: Vec::new(), scopes: Vec::new(), consts: HashMap::new(), extern_indices: &extern_indices };
    c.scopes.push(FnScope::new());
    c.emit_expr(expr)?;
    c.emit(OpCode::Return, expr.line, expr.col);
    Ok(c.scopes.pop().unwrap().chunk)
}

/// Una variable local activa: su nombre, su slot y la profundidad de ámbito.
struct Local {
    name: String,
    slot: usize,
    depth: usize,
}

/// Estado de compilación de **una función**: su chunk, sus locales y sus upvalues.
struct FnScope {
    chunk: Chunk,
    locals: Vec<Local>,
    /// Próximo slot libre (crece al declarar, se recupera al cerrar un ámbito).
    next_slot: usize,
    /// Marca de agua: el mayor `next_slot` alcanzado = slots que necesita el marco.
    max_slots: usize,
    scope_depth: usize,
    /// Upvalues de esta función, en el orden en que se descubren.
    upvalues: Vec<UpvalueRef>,
    /// `captured_slots[s] == true` si el slot `s` es capturado por una closure
    /// anidada (debe boxearse). Crece a demanda; se rellena hasta `max_slots`.
    captured_slots: Vec<bool>,
}

impl FnScope {
    fn new() -> Self {
        FnScope {
            chunk: Chunk::new(),
            locals: Vec::new(),
            next_slot: 0,
            max_slots: 0,
            scope_depth: 0,
            upvalues: Vec::new(),
            captured_slots: Vec::new(),
        }
    }
}

/// El compilador: una pila de ámbitos de función (el último es el que se compila
/// ahora) y la tabla de funciones que va llenando.
struct Compiler<'a> {
    indices: &'a HashMap<String, usize>,
    structs: &'a StructDefs,
    enums: &'a EnumDefs,
    /// Número de funciones nombradas: las anónimas viven en `n_named + id` (M4.1).
    n_named: usize,
    functions: Vec<Option<CompiledFn>>,
    scopes: Vec<FnScope>,
    /// Constantes de nivel superior (M27.5): nombre → su valor. Una referencia se compila a `Constant`.
    consts: HashMap<String, Value>,
    /// Funciones externas (M41, FFI): nombre → índice en la tabla `externs` del `CompiledProgram`.
    /// Una llamada a uno de estos nombres se baja a `CallExtern(idx, argc)`.
    extern_indices: &'a HashMap<String, usize>,
}

impl<'a> Compiler<'a> {
    // ----- Compilación de una función -----

    /// Compila una función (nombrada o anónima) y la deposita en `functions[idx]`.
    /// Las fn-exprs anidadas en su cuerpo se compilan en línea (recursivamente).
    fn compile_function(
        &mut self,
        idx: usize,
        params: &[Param],
        body: &Block,
        line: usize,
        col: usize,
        name: String,
    ) -> Result<(), CompileError> {
        self.scopes.push(FnScope::new());
        // Los parámetros son las primeras locales (slots 0..arity).
        for p in params {
            self.declare_local(&p.name);
        }
        // R7: las `run_*` internas de std/regex se despachan al crate `regex` de ray-runtime (el
        // mismo borde que el binario nativo, R5): su cuerpo se compila a `[RegexNative, Return]`.
        // El cuerpo raylang (la Pike VM) solo se compila como fallback — build sin la feature
        // `regex` o con RAYLANG_REGEX_PIKE=1 (A/B y depuración).
        if let Some(op) = self.regex_native_op(&name) {
            self.emit(op, line, col);
            self.emit(OpCode::Return, line, col);
        } else {
            self.emit_block(body)?;
            self.emit(OpCode::Return, line, col);

            // M13.3b: optimización de llamadas en cola. Toda llamada cuya continuación es un `Return`
            // (directo o a través de saltos incondicionales) se convierte en `TailCall`, que reutiliza
            // el marco en vez de apilar uno nuevo → recursión de cola en O(1) marcos.
            optimize_tail_calls(&mut self.cur().chunk);
            // M36.1: superinstrucciones (tras el TCO → no fusiona a través de una llamada).
            fuse_superinstructions(&mut self.cur().chunk);
            fuse_round2(&mut self.cur().chunk); // A4: guardas y aritmética local-const
            fuse_guard_round3(&mut self.cur().chunk); // P0.6: GetLocalConst;CmpJump → guarda en 1 opcode
            fuse_index_round4(&mut self.cur().chunk); // MM2: [GetLocalLocal|GetLocal, Index] → IndexLL/IndexLocal
            fuse_loop_round5(&mut self.cur().chunk); // V9: [AddLocalConst, SetLocal(, Jump)] → IncLocalConst/IncJump
            discard_spawn_results(&mut self.cur().chunk); // M98.1: spawn fire-and-forget sin Task retenida
        }

        let mut scope = self.scopes.pop().expect("acabamos de empujar el ámbito");
        scope.captured_slots.resize(scope.max_slots, false);
        let has_captured = scope.captured_slots.iter().any(|&b| b);
        self.functions[idx] = Some(CompiledFn {
            name,
            arity: params.len(),
            num_locals: scope.max_slots,
            captured: scope.captured_slots,
            has_captured,
            upvalues: scope.upvalues,
            chunk: scope.chunk,
        });
        Ok(())
    }

    /// R7: si `name` es una de las 7 funciones internas `run_*` de std/regex (la misma lista que
    /// intercepta el transpilador nativo, R5), el opcode que la despacha al crate `regex` — o
    /// `None` para compilar el cuerpo raylang (la Pike VM) normal. Resuelve aquí los índices de
    /// `Option` (del prelude, siempre presente) para que el camino caliente no busque por nombre.
    #[cfg(all(feature = "regex", not(target_arch = "wasm32")))]
    fn regex_native_op(&self, name: &str) -> Option<OpCode> {
        use crate::bytecode::RegexNativeFn as R;
        if std::env::var_os("RAYLANG_REGEX_PIKE").is_some() {
            return None; // escape de A/B y depuración: fuerza la Pike VM interpretada
        }
        let f = match name.strip_prefix("std::regex::run_")? {
            "full" => R::Full,
            "search" => R::Search,
            "find" => R::Find,
            "find_all" => R::FindAll,
            "replace_all" => R::ReplaceAll,
            "captures" => R::Captures,
            "captures_str" => R::CapturesStr,
            _ => return None, // p. ej. run_find_str no existe; cualquier otra run_* va normal
        };
        let (enum_id, variants) = self.enums.get("Option")?;
        let some = variants.get("Some")?.0;
        let none = variants.get("None")?.0;
        Some(OpCode::RegexNative { f, opt: (*enum_id, some, none) })
    }

    #[cfg(not(all(feature = "regex", not(target_arch = "wasm32"))))]
    fn regex_native_op(&self, _name: &str) -> Option<OpCode> {
        None
    }

    // ----- Acceso al ámbito actual -----

    fn cur(&mut self) -> &mut FnScope {
        self.scopes.last_mut().expect("siempre hay un ámbito de función activo")
    }

    fn emit(&mut self, op: OpCode, line: usize, col: usize) -> usize {
        self.cur().chunk.emit(op, line, col)
    }

    // ----- Manejo de slots y ámbitos de bloque -----

    fn declare_local(&mut self, name: &str) -> usize {
        let s = self.cur();
        let slot = s.next_slot;
        s.next_slot += 1;
        if s.next_slot > s.max_slots {
            s.max_slots = s.next_slot;
        }
        let depth = s.scope_depth;
        s.locals.push(Local { name: name.to_string(), slot, depth });
        slot
    }

    /// Resuelve un nombre a un slot local de la **función actual**.
    fn resolve_local(&self, name: &str) -> Option<usize> {
        let s = self.scopes.last().expect("ámbito activo");
        s.locals.iter().rev().find(|l| l.name == name).map(|l| l.slot)
    }

    /// Resuelve un nombre a un slot local de la función en el índice de ámbito dado.
    fn resolve_local_at(&self, scope_idx: usize, name: &str) -> Option<usize> {
        self.scopes[scope_idx].locals.iter().rev().find(|l| l.name == name).map(|l| l.slot)
    }

    /// ¿El nombre es una variable (local de alguna función de la cadena)? Pura, sin
    /// efectos: sirve para decidir entre llamada directa (a una global) e indirecta.
    fn name_is_variable(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.locals.iter().any(|l| l.name == name))
    }

    /// Resuelve un nombre como **upvalue** de la función en `depth` (al estilo clox):
    /// lo busca como local de la envolvente (upvalue *local*) o, recursivamente,
    /// entre los upvalues de la envolvente (upvalue *de upvalue*). Devuelve el índice
    /// del upvalue en la función `depth`, y marca como capturado el slot de origen.
    fn resolve_upvalue(&mut self, depth: usize, name: &str) -> Option<usize> {
        if depth == 0 {
            return None; // la función más externa no tiene envolvente
        }
        let enclosing = depth - 1;
        if let Some(slot) = self.resolve_local_at(enclosing, name) {
            self.mark_captured(enclosing, slot);
            return Some(self.add_upvalue(depth, name, UpvalueSource::Local(slot)));
        }
        if let Some(up) = self.resolve_upvalue(enclosing, name) {
            return Some(self.add_upvalue(depth, name, UpvalueSource::Upvalue(up)));
        }
        None
    }

    fn mark_captured(&mut self, scope_idx: usize, slot: usize) {
        let cs = &mut self.scopes[scope_idx].captured_slots;
        if cs.len() <= slot {
            cs.resize(slot + 1, false);
        }
        cs[slot] = true;
    }

    fn add_upvalue(&mut self, depth: usize, name: &str, source: UpvalueSource) -> usize {
        let ups = &mut self.scopes[depth].upvalues;
        if let Some(i) = ups.iter().position(|u| u.source == source) {
            return i; // ya registrado: mismo upvalue
        }
        ups.push(UpvalueRef { name: name.to_string(), source });
        ups.len() - 1
    }

    fn begin_scope(&mut self) -> usize {
        let s = self.cur();
        s.scope_depth += 1;
        s.next_slot
    }

    /// Emite un entero constante (para índices/incrementos del `for`).
    fn emit_int(&mut self, n: i64, line: usize, col: usize) {
        let c = self.cur().chunk.add_constant(Value::Int(n));
        self.emit(OpCode::Constant(c), line, col);
    }

    /// Compila el cuerpo de un `for` (M27.2). El escrutinio ya está en un ámbito (`begin_scope`). Casos:
    /// rango `a..b`, y `in` sobre arreglo/string (por índice) o `Map` (por keys/values ordenados).
    fn emit_for(&mut self, pat: &ForPat, iter: &ForIter, body: &Block, line: usize, col: usize) -> Result<(), CompileError> {
        // Emite: idx (en `idx_slot`) recorre 0..len; cada iteración liga la(s) variable(s) y ejecuta el
        // cuerpo. `bind` genera el código que, dado `idx` en la pila NO, liga la(s) variable(s).
        match iter {
            ForIter::Range { start, end } => {
                let name = match pat { ForPat::Single(n) => n.clone(), _ => unreachable!("checker: un name") };
                self.emit_expr(start)?;
                let i_slot = self.declare_local(&name);
                self.emit(OpCode::InitLocal(i_slot), line, col);
                self.emit_expr(end)?;
                let end_slot = self.declare_local("$end");
                self.emit(OpCode::InitLocal(end_slot), line, col);
                // MM4: si el cuerpo es EXACTAMENTE `s = s + A[k] * B[k];`, emite el kernel
                // DotRange delante del bucle (que sigue completo detrás: es el camino de deopt
                // y el dueño de la semántica de errores). Solo si s/A/B resuelven a locales
                // distintos entre sí y del índice — cualquier otra cosa, bucle normal.
                let dot_at = self.dot_kernel(body, &name, i_slot, end_slot, line, col);
                let loop_start = self.cur().chunk.code.len();
                self.emit(OpCode::GetLocal(i_slot), line, col);
                self.emit(OpCode::GetLocal(end_slot), line, col);
                self.emit(OpCode::Less, line, col);
                let exit = self.emit(OpCode::JumpIfFalse(0), line, col);
                self.emit(OpCode::Pop, line, col);
                self.emit_block(body)?;
                self.emit(OpCode::Pop, line, col);
                // i = i + 1
                self.emit(OpCode::GetLocal(i_slot), line, col);
                self.emit_int(1, line, col);
                self.emit(OpCode::Add, line, col);
                self.emit(OpCode::SetLocal(i_slot), line, col);
                self.emit(OpCode::Jump(loop_start), line, col);
                self.patch_jump(exit);
                self.emit(OpCode::Pop, line, col);
                // MM4: el kernel salta AQUÍ (tras el Pop del bool de la guarda) cuando corre.
                if let Some(at) = dot_at {
                    let after = self.cur().chunk.code.len();
                    if let OpCode::DotRange { exit, .. } = &mut self.cur().chunk.code[at] {
                        *exit = after;
                    }
                }
            }
            ForIter::In(e) => {
                // El valor a iterar. Para arreglo/string se recorre por índice; para Map, keys+values.
                self.emit_expr(e)?;
                let is_map = matches!(pat, ForPat::Tuple(_));
                if is_map {
                    // La pila tiene el Map. Duplica-vía-local: guarda el Map y saca keys/values.
                    let map_slot = self.declare_local("$map");
                    self.emit(OpCode::InitLocal(map_slot), line, col);
                    self.emit(OpCode::GetLocal(map_slot), line, col);
                    self.emit(OpCode::MapKeys, line, col);
                    let keys_slot = self.declare_local("$keys");
                    self.emit(OpCode::InitLocal(keys_slot), line, col);
                    self.emit(OpCode::GetLocal(map_slot), line, col);
                    self.emit(OpCode::MapValues, line, col);
                    let vals_slot = self.declare_local("$vals");
                    self.emit(OpCode::InitLocal(vals_slot), line, col);
                    let names = match pat { ForPat::Tuple(ns) => ns.clone(), _ => unreachable!() };
                    let k_slot = names[0].as_ref().map(|n| self.declare_local(n));
                    let v_slot = names[1].as_ref().map(|n| self.declare_local(n));
                    self.emit_counted_loop(keys_slot, body, line, col, &mut |c, idx| {
                        if let Some(ks) = k_slot {
                            c.emit(OpCode::GetLocal(keys_slot), line, col);
                            c.emit(OpCode::GetLocal(idx), line, col);
                            c.emit(OpCode::Index, line, col);
                            c.emit(OpCode::InitLocal(ks), line, col);
                        }
                        if let Some(vs) = v_slot {
                            c.emit(OpCode::GetLocal(vals_slot), line, col);
                            c.emit(OpCode::GetLocal(idx), line, col);
                            c.emit(OpCode::Index, line, col);
                            c.emit(OpCode::InitLocal(vs), line, col);
                        }
                    })?;
                } else {
                    let name = match pat { ForPat::Single(n) => n.clone(), _ => unreachable!("checker: un name") };
                    let arr_slot = self.declare_local("$arr");
                    self.emit(OpCode::InitLocal(arr_slot), line, col);
                    let x_slot = self.declare_local(&name);
                    self.emit_counted_loop(arr_slot, body, line, col, &mut |c, idx| {
                        c.emit(OpCode::GetLocal(arr_slot), line, col);
                        c.emit(OpCode::GetLocal(idx), line, col);
                        c.emit(OpCode::Index, line, col);
                        c.emit(OpCode::InitLocal(x_slot), line, col);
                    })?;
                }
            }
            // M40.2: iterador de usuario. Evaluamos el iterable una vez (semántica de referencia →
            // `next` muta su estado) y llamamos a `next` hasta que devuelva `None` (tag 1 de Option).
            ForIter::Iter { expr, next_fn } => {
                let &idx = self.indices.get(next_fn).expect("the checker guarantees next");
                self.emit_expr(expr)?;
                let it_slot = self.declare_local("$it");
                self.emit(OpCode::InitLocal(it_slot), line, col);
                let opt_slot = self.declare_local("$opt");
                // Slots de binding según el patrón: un solo nombre, o (M40.2e) una tupla
                // (`enumerate`) que se destructura por posición desde el elemento.
                let (x_slot, tuple_slots): (usize, Vec<Option<usize>>) = match pat {
                    ForPat::Single(n) => (self.declare_local(n), Vec::new()),
                    ForPat::Tuple(names) => {
                        let elem = self.declare_local("$elem");
                        let slots = names.iter().map(|n| n.as_ref().map(|nm| self.declare_local(nm))).collect();
                        (elem, slots)
                    }
                };
                let loop_start = self.cur().chunk.code.len();
                // opt = next(it)
                self.emit(OpCode::GetLocal(it_slot), line, col);
                self.emit(OpCode::Call(idx, 1), line, col);
                self.emit(OpCode::InitLocal(opt_slot), line, col);
                // ¿es Some (tag 0)? Si no (None), salimos.
                self.emit(OpCode::GetLocal(opt_slot), line, col);
                self.emit(OpCode::EnumTagEq(0), line, col);
                let exit = self.emit(OpCode::JumpIfFalse(0), line, col);
                self.emit(OpCode::Pop, line, col); // descartar el bool true
                // x = payload[0] (o la tupla, que luego se destructura)
                self.emit(OpCode::GetLocal(opt_slot), line, col);
                self.emit(OpCode::GetEnumField(0), line, col);
                self.emit(OpCode::InitLocal(x_slot), line, col);
                // M40.2e: destructurar la tupla en sus posiciones (`$elem[i]` → cada nombre).
                for (i, slot) in tuple_slots.iter().enumerate() {
                    if let Some(s) = slot {
                        self.emit(OpCode::GetLocal(x_slot), line, col);
                        self.emit_int(i as i64, line, col);
                        self.emit(OpCode::Index, line, col);
                        self.emit(OpCode::InitLocal(*s), line, col);
                    }
                }
                self.emit_block(body)?;
                self.emit(OpCode::Pop, line, col); // descartar el valor del cuerpo
                self.emit(OpCode::Jump(loop_start), line, col);
                self.patch_jump(exit);
                self.emit(OpCode::Pop, line, col); // descartar el bool false
            }
        }
        Ok(())
    }

    /// MM4: si `body` es exactamente `s = s + A[k] * B[k];` (con `k` = la variable del rango y
    /// s/A/B locales de esta función, distintos entre sí y de `k`), emite el opcode `DotRange`
    /// (con `exit` provisional, parcheado al cerrar el bucle) y devuelve su índice. Si no, `None`.
    fn dot_kernel(
        &mut self,
        body: &Block,
        k: &str,
        k_slot: usize,
        end_slot: usize,
        line: usize,
        col: usize,
    ) -> Option<usize> {
        // La forma sintáctica: una sola sentencia, sin expresión de cola.
        if body.statements.len() != 1 || body.tail.is_some() {
            return None;
        }
        let StmtKind::Assign { target, value } = &body.statements[0].kind else { return None };
        let ExprKind::Ident(s) = &target.kind else { return None };
        let ExprKind::Binary { op: BinaryOp::Add, left, right } = &value.kind else { return None };
        let ExprKind::Ident(s2) = &left.kind else { return None };
        if s2 != s {
            return None;
        }
        let ExprKind::Binary { op: BinaryOp::Mul, left: fa, right: fb } = &right.kind else { return None };
        let (a, ka) = dot_index_shape(fa)?;
        let (b, kb) = dot_index_shape(fb)?;
        if ka != k || kb != k {
            return None;
        }
        // Los cuatro nombres deben ser locales DISTINTOS (un alias con el índice o el acumulador
        // cambiaría la semántica del kernel; el bucle normal lo cubre igual).
        if s == a || s == b || s == k || a == k || b == k {
            return None;
        }
        let acc = self.resolve_local(s)?;
        let a_slot = self.resolve_local(a)?;
        let b_slot = self.resolve_local(b)?;
        let at = self.emit(
            OpCode::DotRange { acc, a: a_slot, b: b_slot, k: k_slot, end: end_slot, exit: 0 },
            line,
            col,
        );
        Some(at)
    }

    /// Emite un bucle `idx: 0..len(arr_slot)` que, en cada iteración, ejecuta `bind` (que liga la(s)
    /// variable(s)), el cuerpo, e incrementa `idx`. Compartido por los casos arreglo/string y Map.
    fn emit_counted_loop(
        &mut self,
        arr_slot: usize,
        body: &Block,
        line: usize,
        col: usize,
        bind: &mut dyn FnMut(&mut Self, usize),
    ) -> Result<(), CompileError> {
        self.emit_int(0, line, col);
        let idx_slot = self.declare_local("$idx");
        self.emit(OpCode::InitLocal(idx_slot), line, col);
        self.emit(OpCode::GetLocal(arr_slot), line, col);
        self.emit(OpCode::Len, line, col);
        let len_slot = self.declare_local("$len");
        self.emit(OpCode::InitLocal(len_slot), line, col);
        let loop_start = self.cur().chunk.code.len();
        self.emit(OpCode::GetLocal(idx_slot), line, col);
        self.emit(OpCode::GetLocal(len_slot), line, col);
        self.emit(OpCode::Less, line, col);
        let exit = self.emit(OpCode::JumpIfFalse(0), line, col);
        self.emit(OpCode::Pop, line, col);
        bind(self, idx_slot); // liga la(s) variable(s) de la iteración
        self.emit_block(body)?;
        self.emit(OpCode::Pop, line, col);
        self.emit(OpCode::GetLocal(idx_slot), line, col);
        self.emit_int(1, line, col);
        self.emit(OpCode::Add, line, col);
        self.emit(OpCode::SetLocal(idx_slot), line, col);
        self.emit(OpCode::Jump(loop_start), line, col);
        self.patch_jump(exit);
        self.emit(OpCode::Pop, line, col);
        Ok(())
    }

    fn end_scope(&mut self, saved_slot: usize) {
        let s = self.cur();
        s.scope_depth -= 1;
        let d = s.scope_depth;
        s.locals.retain(|l| l.depth <= d);
        // Recuperamos los slots del ámbito para reutilizarlos. (Las marcas de
        // 'capturado' NO se borran: el slot, si se reusa, sigue boxeado, lo cual es
        // conservador pero correcto.)
        s.next_slot = saved_slot;
    }

    /// Parchea un salto previamente emitido para que apunte al final actual.
    fn patch_jump(&mut self, at: usize) {
        let s = self.cur();
        let target = s.chunk.code.len();
        s.chunk.code[at] = match s.chunk.code[at] {
            OpCode::Jump(_) => OpCode::Jump(target),
            OpCode::JumpIfFalse(_) => OpCode::JumpIfFalse(target),
            _ => unreachable!("patch_jump about one instrucción what no es salto"),
        };
    }

    // ----- Emisión -----

    fn emit_block(&mut self, block: &Block) -> Result<(), CompileError> {
        let saved = self.begin_scope();
        for stmt in &block.statements {
            self.emit_stmt(stmt)?;
        }
        match &block.tail {
            Some(e) => self.emit_expr(e)?,
            None => {
                self.emit(OpCode::Unit, block.line, block.col);
            }
        }
        self.end_scope(saved);
        Ok(())
    }

    /// Baja un `match` a bytecode (M5.3): guarda el escrutinio en un local temporal y
    /// emite una **cadena de decisión** —probar el tag de cada variante, ligar su
    /// payload, ejecutar el cuerpo y saltar al final—. El valor del brazo que casa
    /// queda en la pila (el `match` es una expresión).
    /// Emite el test + los bindings de un patrón (M40.1c, recursivo) contra el valor guardado en el
    /// local `val_slot`. Cada `EnumTagEq` que puede fallar añade su salto a `to_next` (dejando UN
    /// bool); en runtime el primer fallo salta y deja exactamente un bool → el llamador lo limpia con
    /// un solo `Pop`. Un `Wildcard` no emite nada (siempre casa); un `Binding` liga sin test. Un
    /// sub-patrón anidado se extrae a un local temporal (`$sub`) y se recurre.
    fn emit_pattern_test(&mut self, pat: &Pattern, val_slot: usize, to_next: &mut Vec<usize>, line: usize, col: usize) -> Result<(), CompileError> {
        match &pat.kind {
            PatternKind::Wildcard => {}
            PatternKind::Binding(name) => {
                self.emit(OpCode::GetLocal(val_slot), line, col);
                let slot = self.declare_local(name);
                self.emit(OpCode::InitLocal(slot), line, col);
            }
            PatternKind::Variant { enum_name, variant, subpatterns } => {
                let (_, vmap) = self.enums.get(enum_name).expect("el checker registró el enum");
                let (tag, _arity) = *vmap.get(variant).expect("el checker validó la variant");
                // ¿Es esta la variante? Si no, al siguiente brazo (dejando el bool).
                self.emit(OpCode::GetLocal(val_slot), line, col);
                self.emit(OpCode::EnumTagEq(tag), line, col);
                to_next.push(self.emit(OpCode::JumpIfFalse(0), line, col));
                self.emit(OpCode::Pop, line, col); // casó → descartar el bool true
                // Cada posición del payload: se extrae a un temporal y se casa recursivamente.
                for (i, sub) in subpatterns.iter().enumerate() {
                    if matches!(sub.kind, PatternKind::Wildcard) {
                        continue; // `_` no liga ni testea → no hace falta extraerlo
                    }
                    self.emit(OpCode::GetLocal(val_slot), line, col);
                    self.emit(OpCode::GetEnumField(i), line, col);
                    let sub_slot = self.declare_local("$sub");
                    self.emit(OpCode::InitLocal(sub_slot), line, col);
                    self.emit_pattern_test(sub, sub_slot, to_next, line, col)?;
                }
            }
            PatternKind::Struct { fields, .. } => {
                // M40.1d: sin tag (un struct siempre casa por tipo, garantizado por el checker); se
                // extrae cada campo listado con `GetField` a un temporal y se casa recursivamente.
                for (fname, fpat) in fields {
                    if matches!(fpat.kind, PatternKind::Wildcard) {
                        continue;
                    }
                    self.emit(OpCode::GetLocal(val_slot), line, col);
                    self.emit(OpCode::GetField(fname.clone()), line, col);
                    let sub_slot = self.declare_local("$sub");
                    self.emit(OpCode::InitLocal(sub_slot), line, col);
                    self.emit_pattern_test(fpat, sub_slot, to_next, line, col)?;
                }
            }
        }
        Ok(())
    }

    fn emit_match(&mut self, scrutinee: &Expr, arms: &[MatchArm], line: usize, col: usize) -> Result<(), CompileError> {
        // El escrutinio se evalúa UNA vez y se guarda en un local temporal: así se lee
        // su tag y su payload sin reevaluarlo, y queda rooteado para el GC mientras
        // dura el match. El nombre `$match` no es un identificador válido, no choca.
        self.emit_expr(scrutinee)?;
        let saved = self.begin_scope();
        let scrut_slot = self.declare_local("$match");
        self.emit(OpCode::InitLocal(scrut_slot), line, col);

        let mut to_end: Vec<usize> = Vec::new();
        let mut has_catchall = false;

        for arm in arms {
            let (aline, acol) = (arm.line, arm.col);
            // Saltos que van al SIGUIENTE brazo (test de variante fallido, o guarda falsa). Cada uno
            // deja UN bool en la pila; en runtime se toma como mucho uno → un solo `Pop` los limpia.
            let mut to_next: Vec<usize> = Vec::new();
            // Ámbito del brazo (bindings + guarda + cuerpo). `InitLocal` boxea el slot si una closure
            // del cuerpo lo captura. `end_scope` es solo compile-time → saltar por encima es seguro.
            let arm_saved = self.begin_scope();
            // Emite los tests + bindings del patrón (recursivo, M40.1c) contra el valor del escrutinio.
            self.emit_pattern_test(&arm.pattern, scrut_slot, &mut to_next, aline, acol)?;
            // Guarda (M40.1a): si es falsa, al siguiente brazo (dejando su bool). Con los bindings del
            // patrón ya en ámbito. Un brazo con guarda NO es catch-all (puede no casar).
            if let Some(g) = &arm.guard {
                self.emit_expr(g)?;
                to_next.push(self.emit(OpCode::JumpIfFalse(0), aline, acol));
                self.emit(OpCode::Pop, aline, acol); // guarda true
            } else if matches!(arm.pattern.kind, PatternKind::Wildcard | PatternKind::Binding(_)) {
                has_catchall = true;
            }
            self.emit_expr(&arm.body)?;
            self.end_scope(arm_saved);
            to_end.push(self.emit(OpCode::Jump(0), aline, acol));
            // Etiqueta del siguiente brazo: cada salto de `to_next` cae aquí dejando un bool → 1 Pop.
            for j in &to_next {
                self.patch_jump(*j);
            }
            if !to_next.is_empty() {
                self.emit(OpCode::Pop, aline, acol);
            }
        }

        // Sin catch-all, el checker probó que las variantes cubren todo; si aun así no
        // casara ninguna (un bug), el trap lo señala en vez de corromper la pila.
        if !has_catchall {
            self.emit(OpCode::MatchFail, line, col);
        }
        for j in to_end {
            self.patch_jump(j);
        }
        self.end_scope(saved);
        Ok(())
    }

    /// Baja el operador `expr?` a bytecode (M6.3). Guarda el Result/Option en un local
    /// temporal; si su tag es 0 (`Ok`/`Some`) desempaqueta el payload, si no
    /// (`Err`/`None`) **retorna** ese valor de la función. Reusa los opcodes de enum y
    /// el `Return`; no hace falta uno nuevo.
    fn emit_try(&mut self, inner: &Expr, line: usize, col: usize) -> Result<(), CompileError> {
        self.emit_expr(inner)?;
        let saved = self.begin_scope();
        let slot = self.declare_local("$try");
        self.emit(OpCode::InitLocal(slot), line, col);

        // ¿Es el caso de éxito (tag 0)? El prelude declara Ok/Some primero.
        self.emit(OpCode::GetLocal(slot), line, col);
        self.emit(OpCode::EnumTagEq(0), line, col);
        let to_err = self.emit(OpCode::JumpIfFalse(0), line, col);
        // Éxito: desempaquetar el payload[0].
        self.emit(OpCode::Pop, line, col); // descartar el bool true
        self.emit(OpCode::GetLocal(slot), line, col);
        self.emit(OpCode::GetEnumField(0), line, col);
        let to_end = self.emit(OpCode::Jump(0), line, col);
        // Error: retornar el Err/None tal cual desde la función.
        self.patch_jump(to_err);
        self.emit(OpCode::Pop, line, col); // descartar el bool false
        self.emit(OpCode::GetLocal(slot), line, col);
        self.emit(OpCode::Return, line, col);

        self.patch_jump(to_end);
        self.end_scope(saved);
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        let (line, col) = (stmt.line, stmt.col);
        match &stmt.kind {
            StmtKind::Let { name, value, .. } => {
                // Compilamos el inicializador ANTES de declarar (para que no se vea
                // a sí misma), luego inicializamos su slot (InitLocal estrena celda
                // si el slot está boxeado).
                self.emit_expr(value)?;
                let slot = self.declare_local(name);
                self.emit(OpCode::InitLocal(slot), line, col);
            }
            // M27.1: desestructuración de tupla. La tupla es un arreglo → se guarda en un temp y se
            // liga cada nombre por índice (`$tuple` no es un identificador válido, no choca).
            StmtKind::LetTuple { names, value, .. } => {
                self.emit_expr(value)?;
                let tup_slot = self.declare_local("$tuple");
                self.emit(OpCode::InitLocal(tup_slot), line, col);
                for (i, n) in names.iter().enumerate() {
                    if let Some(name) = n {
                        self.emit(OpCode::GetLocal(tup_slot), line, col);
                        let cidx = self.cur().chunk.add_constant(Value::Int(i as i64));
                        self.emit(OpCode::Constant(cidx), line, col);
                        self.emit(OpCode::Index, line, col);
                        let slot = self.declare_local(name);
                        self.emit(OpCode::InitLocal(slot), line, col);
                    }
                }
            }
            // M27.2: bucle `for`. Se compila a un bucle contado con locales temporales (`$…`, no chocan).
            StmtKind::For { pat, iter, body } => {
                let saved = self.begin_scope();
                self.emit_for(pat, iter, body, line, col)?;
                self.end_scope(saved);
            }
            StmtKind::Assign { target, value } => match &target.kind {
                // x = e  → a un local, a un upvalue, según resuelva el nombre.
                ExprKind::Ident(name) => {
                    self.emit_expr(value)?;
                    if let Some(slot) = self.resolve_local(name) {
                        self.emit(OpCode::SetLocal(slot), line, col);
                    } else {
                        let depth = self.scopes.len() - 1;
                        let up = self.resolve_upvalue(depth, name).expect("el checker garantiza la variable");
                        self.emit(OpCode::SetUpvalue(up), line, col);
                    }
                }
                // a[i] = e  → arreglo, índice, valor, SetIndex (consume los tres).
                ExprKind::Index { array, index } => {
                    self.emit_expr(array)?;
                    self.emit_expr(index)?;
                    self.emit_expr(value)?;
                    self.emit(OpCode::SetIndex, line, col);
                }
                // p.x = e  → struct, valor, SetField.
                ExprKind::Field { object, name } => {
                    self.emit_expr(object)?;
                    self.emit_expr(value)?;
                    self.emit(OpCode::SetField(name.clone()), line, col);
                }
                _ => unreachable!("the checker guarantees an lvalue"),
            },
            StmtKind::Return { value } => {
                match value {
                    Some(e) => self.emit_expr(e)?,
                    None => {
                        self.emit(OpCode::Unit, line, col);
                    }
                }
                self.emit(OpCode::Return, line, col);
            }
            StmtKind::Expr(e) => {
                self.emit_expr(e)?;
                self.emit(OpCode::Pop, line, col); // su valor se descarta
            }
        }
        Ok(())
    }

    fn emit_expr(&mut self, expr: &Expr) -> Result<(), CompileError> {
        let (line, col) = (expr.line, expr.col);
        // Opt.12: plegado de constantes. Una (sub)expresión hecha solo de literales y
        // operaciones que NO pueden fallar en runtime se evalúa en compilación y se
        // emite como una única constante (`1 + 2` → `3`, `24 * 60 * 60` → `86400`).
        // Lo que puede trapear (división/módulo por cero, overflow del int checked) se
        // deja sin plegar → la semántica es idéntica y el oráculo VM↔intérprete no lo ve.
        if matches!(&expr.kind, ExprKind::Binary { .. } | ExprKind::Unary { .. }) {
            if let Some(v) = const_fold(expr) {
                match v {
                    Value::Bool(true) => self.emit(OpCode::True, line, col),
                    Value::Bool(false) => self.emit(OpCode::False, line, col),
                    v => {
                        let idx = self.cur().chunk.add_constant(v);
                        self.emit(OpCode::Constant(idx), line, col)
                    }
                };
                return Ok(());
            }
        }
        match &expr.kind {
            ExprKind::Int(v) => {
                let idx = self.cur().chunk.add_constant(Value::Int(*v));
                self.emit(OpCode::Constant(idx), line, col);
            }
            ExprKind::Float(v) => {
                let idx = self.cur().chunk.add_constant(Value::Float(*v));
                self.emit(OpCode::Constant(idx), line, col);
            }
            ExprKind::Str(s) => {
                let idx = self.cur().chunk.add_constant(Value::Str(s.clone()));
                self.emit(OpCode::Constant(idx), line, col);
            }
            ExprKind::Char(c) => {
                let idx = self.cur().chunk.add_constant(Value::Char(*c));
                self.emit(OpCode::Constant(idx), line, col);
            }
            ExprKind::Bytes(b) => {
                let idx = self.cur().chunk.add_constant(Value::Bytes(std::rc::Rc::new(b.clone())));
                self.emit(OpCode::Constant(idx), line, col);
            }
            ExprKind::Bool(true) => {
                self.emit(OpCode::True, line, col);
            }
            ExprKind::Bool(false) => {
                self.emit(OpCode::False, line, col);
            }

            ExprKind::Ident(name) => {
                if let Some(slot) = self.resolve_local(name) {
                    self.emit(OpCode::GetLocal(slot), line, col);
                } else {
                    let depth = self.scopes.len() - 1;
                    if let Some(up) = self.resolve_upvalue(depth, name) {
                        self.emit(OpCode::GetUpvalue(up), line, col);
                    } else if let Some(v) = self.consts.get(name).cloned() {
                        // M27.5: una constante de nivel superior → su valor como Constant.
                        let cidx = self.cur().chunk.add_constant(v);
                        self.emit(OpCode::Constant(cidx), line, col);
                    } else {
                        // No es variable ni upvalue ni constante: un nombre de función como valor.
                        let idx = *self.indices.get(name).expect("the checker guarantees the name");
                        self.emit(OpCode::Function(idx), line, col);
                    }
                }
            }

            ExprKind::Unary { op, expr: inner } => {
                self.emit_expr(inner)?;
                let opc = match op {
                    UnaryOp::Neg => OpCode::Negate,
                    UnaryOp::Not => OpCode::Not,
                    UnaryOp::BitNot => OpCode::BitNot, // M19.3a
                };
                self.emit(opc, line, col);
            }

            // Lógicos con cortocircuito (JumpIfFalse ojea la condición).
            ExprKind::Binary { op: BinaryOp::And, left, right } => {
                self.emit_expr(left)?;
                let jump = self.emit(OpCode::JumpIfFalse(0), line, col);
                self.emit(OpCode::Pop, line, col);
                self.emit_expr(right)?;
                self.patch_jump(jump);
            }
            ExprKind::Binary { op: BinaryOp::Or, left, right } => {
                self.emit_expr(left)?;
                let to_else = self.emit(OpCode::JumpIfFalse(0), line, col);
                let to_end = self.emit(OpCode::Jump(0), line, col);
                self.patch_jump(to_else);
                self.emit(OpCode::Pop, line, col);
                self.emit_expr(right)?;
                self.patch_jump(to_end);
            }

            ExprKind::Binary { op, left, right } => {
                self.emit_expr(left)?;
                self.emit_expr(right)?;
                let opc = match op {
                    BinaryOp::Add => OpCode::Add,
                    BinaryOp::Sub => OpCode::Sub,
                    BinaryOp::Mul => OpCode::Mul,
                    BinaryOp::Div => OpCode::Div,
                    BinaryOp::Rem => OpCode::Rem,
                    BinaryOp::Eq => OpCode::Equal,
                    BinaryOp::Ne => OpCode::NotEqual,
                    BinaryOp::Lt => OpCode::Less,
                    BinaryOp::Le => OpCode::LessEqual,
                    BinaryOp::Gt => OpCode::Greater,
                    BinaryOp::Ge => OpCode::GreaterEqual,
                    // Bit a bit (M19.3a).
                    BinaryOp::BitAnd => OpCode::BitAnd,
                    BinaryOp::BitOr => OpCode::BitOr,
                    BinaryOp::BitXor => OpCode::BitXor,
                    BinaryOp::Shl => OpCode::Shl,
                    BinaryOp::Shr => OpCode::Shr,
                    BinaryOp::And | BinaryOp::Or => unreachable!("cubiertos arriba"),
                };
                self.emit(opc, line, col);
            }

            ExprKind::If { cond, then_branch, else_branch } => {
                self.emit_expr(cond)?;
                let to_else = self.emit(OpCode::JumpIfFalse(0), line, col);
                self.emit(OpCode::Pop, line, col);
                self.emit_block(then_branch)?;
                let to_end = self.emit(OpCode::Jump(0), line, col);

                self.patch_jump(to_else);
                self.emit(OpCode::Pop, line, col);
                match else_branch {
                    Some(else_e) => self.emit_expr(else_e)?,
                    None => {
                        self.emit(OpCode::Unit, line, col);
                    }
                }
                self.patch_jump(to_end);
            }

            ExprKind::While { cond, body } => {
                let loop_start = self.cur().chunk.code.len();
                self.emit_expr(cond)?;
                let exit = self.emit(OpCode::JumpIfFalse(0), line, col);
                self.emit(OpCode::Pop, line, col); // cond true → descartarla
                self.emit_block(body)?;
                self.emit(OpCode::Pop, line, col); // descartar el valor del cuerpo
                self.emit(OpCode::Jump(loop_start), line, col); // salto hacia atrás
                self.patch_jump(exit);
                self.emit(OpCode::Pop, line, col); // cond false → descartarla
                self.emit(OpCode::Unit, line, col); // el while vale unit
            }

            ExprKind::Block(b) => self.emit_block(b)?,

            ExprKind::Match { scrutinee, arms } => self.emit_match(scrutinee, arms, line, col)?,

            ExprKind::Try(inner) => self.emit_try(inner, line, col)?,

            // M27.1: una tupla `(a, b, …)` se compila como un arreglo (erasure).
            ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
                for e in elems {
                    self.emit_expr(e)?;
                }
                self.emit(OpCode::MakeArray(elems.len()), line, col);
            }

            // M48.2: literal de Map `[k: v, …]` → `Map.new()` + `insert` por par. Como `MapInsert`
            // consume el handle (deja unit) y no hay `Dup`, se guarda el Map en un local temporal y se
            // recupera al final (mismo patrón que el escrutinio del `match`).
            ExprKind::MapLit(pares) => {
                self.emit(OpCode::MapNew, line, col);
                if pares.is_empty() {
                    // `[:]` — el Map vacío; ya está en la pila.
                } else {
                    let slot = self.declare_local("$maplit");
                    self.emit(OpCode::InitLocal(slot), line, col);
                    for (k, v) in pares {
                        self.emit(OpCode::GetLocal(slot), line, col);
                        self.emit_expr(k)?;
                        self.emit_expr(v)?;
                        self.emit(OpCode::MapInsert, line, col);
                        self.emit(OpCode::Pop, line, col); // insert devuelve unit
                    }
                    self.emit(OpCode::GetLocal(slot), line, col);
                }
            }

            // M27.4: conversión numérica `as`. El destino ya lo normalizó el checker (int/float/char).
            ExprKind::Cast { expr: inner, ty } => {
                self.emit_expr(inner)?;
                let target = match ty {
                    Type::Float => CastTarget::Float,
                    Type::Char => CastTarget::Char,
                    Type::UInt(w) => CastTarget::UInt(*w), // M28.3
                    _ => CastTarget::Int, // Type::Int (el checker solo permite int/float/char/uint)
                };
                self.emit(OpCode::Cast(target), line, col);
            }

            ExprKind::Index { array, index } => {
                self.emit_expr(array)?;
                self.emit_expr(index)?;
                self.emit(OpCode::Index, line, col);
            }

            ExprKind::StructLit { name, fields } => {
                let (idx, field_names) = self.structs.get(name).expect("the checker registered the struct");
                let idx = *idx;
                let field_names = field_names.clone(); // suelta el préstamo de self
                // Emitimos los valores en ORDEN DE DECLARACIÓN (así MakeStruct los
                // empareja con los nombres de campo de la tabla).
                for fname in &field_names {
                    let value_expr = fields
                        .iter()
                        .find(|(n, _)| n == fname)
                        .map(|(_, e)| e)
                        .expect("the checker guarantees the field");
                    self.emit_expr(value_expr)?;
                }
                self.emit(OpCode::MakeStruct(idx), line, col);
            }

            ExprKind::EnumLit { enum_name, variant, args } => {
                let (enum_id, variant_map) = self.enums.get(enum_name).expect("el checker registró el enum");
                let enum_id = *enum_id;
                let (tag, _arity) = *variant_map.get(variant).expect("el checker validó la variant");
                // Emitimos el payload en orden; MakeEnum saca esos valores y arma el enum.
                for a in args {
                    self.emit_expr(a)?;
                }
                self.emit(OpCode::MakeEnum(enum_id, tag), line, col);
            }

            ExprKind::Field { object, name } => {
                // M27.1: un nombre de campo numérico es un acceso a tupla `t.0` → índice del arreglo.
                if let Ok(idx) = name.parse::<i64>() {
                    self.emit_expr(object)?;
                    let cidx = self.cur().chunk.add_constant(Value::Int(idx));
                    self.emit(OpCode::Constant(cidx), line, col);
                    self.emit(OpCode::Index, line, col);
                } else {
                    self.emit_expr(object)?;
                    self.emit(OpCode::GetField(name.clone()), line, col);
                }
            }

            ExprKind::Func(fe) => {
                // Compilamos la función anónima en línea (en su propio ámbito, que ve
                // a los envolventes para resolver upvalues), y emitimos un Closure si
                // capturó algo, o un Function simple si no.
                let fn_index = self.n_named + fe.id;
                self.compile_function(fn_index, &fe.params, &fe.body, fe.line, fe.col, format!("<fn#{}>", fe.id))?;
                let has_upvalues = !self.functions[fn_index].as_ref().unwrap().upvalues.is_empty();
                if has_upvalues {
                    self.emit(OpCode::Closure(fn_index), line, col);
                } else {
                    self.emit(OpCode::Function(fn_index), line, col);
                }
            }

            ExprKind::Call { callee, args } => self.emit_call(callee, args, line, col)?,
        }
        Ok(())
    }

    /// Emite una llamada. Distingue el camino **directo** (un builtin o una función
    /// global, identificada por nombre y no tapada por una variable) del **indirecto**
    /// (un valor-función en la pila), que usa `CallValue` (M4.1).
    fn emit_call(&mut self, callee: &Expr, args: &[Expr], line: usize, col: usize) -> Result<(), CompileError> {
        // M48.1: función asociada `Tipo.fn(args)` (`Map.new()`, `Channel.new()`, `Channel.bounded(n)`):
        // se empujan los argumentos y se emite el opcode que declara el registro (`MapNew`/`ChannelNew`/
        // `ChannelNewBounded`). No es UFCS (el checker no la baja) → llega como `Call(Field)` intacta.
        if let ExprKind::Field { object, name } = &callee.kind {
            if let ExprKind::Ident(tn) = &object.kind {
                if let Some(assoc) = crate::builtins::assoc_lookup(tn, name) {
                    for arg in args {
                        self.emit_expr(arg)?;
                    }
                    self.emit(assoc.opcode.clone(), line, col);
                    return Ok(());
                }
            }
        }
        if let ExprKind::Ident(name) = &callee.kind {
            // Solo es directo si el nombre NO es una variable (local o upvalue).
            if !self.name_is_variable(name) {
                // M48.1: `Channel.new()`/`Channel.bounded(n)` (antes `channel()`/`channel(n)`) bajan por la
                // rama de funciones asociadas de arriba (`Call(Field)` → `ChannelNew`/`ChannelNewBounded`).
                // M12.3: `scope(body)` se baja a ScopeBegin; body(); ScopeEnd. Se trata aparte porque el
                // cuerpo se llama ENTRE los dos opcodes de scope (para poseer las tareas que lance) — no es
                // un builtin ordinario que reciba sus args en la pila. La llamada al cuerpo NO está en
                // posición de cola (la sigue ScopeEnd), así que el peephole de TCO no la toca.
                if name == "scope" {
                    self.emit(OpCode::ScopeBegin, line, col);
                    self.emit_expr(&args[0])?;        // el cuerpo: un valor-función fn() -> R
                    self.emit(OpCode::CallValue(0), line, col);
                    self.emit(OpCode::ScopeEnd, line, col);
                    return Ok(());
                }
                // M12.3: `join` es ad-hoc polimórfico (string vs Task). El opcode lo decide la ARIDAD:
                // 1 argumento → unir una Task (TaskJoin); 2 → unir un [string] (Join). El checker ya validó
                // los tipos; aquí basta la forma.
                if name == "join" && args.len() == 1 {
                    self.emit_expr(&args[0])?;
                    self.emit(OpCode::TaskJoin, line, col);
                    return Ok(());
                }
                // V2: `__concat(a, b, …)` (generado por el checker al aplanar cadenas de `+` de
                // strings) → `ConcatN(argc)`. Special-case por la aridad VARIABLE (la fila de la
                // tabla lleva un opcode placeholder), como `channel`/`join`.
                if name == "__concat" {
                    for arg in args {
                        self.emit_expr(arg)?;
                    }
                    self.emit(OpCode::ConcatN(args.len()), line, col);
                    return Ok(());
                }
                // Builtin: el opcode lo da el registro único (`src/builtins.rs`).
                if let Some(b) = crate::builtins::lookup(name) {
                    for arg in args {
                        self.emit_expr(arg)?;
                    }
                    self.emit(b.opcode.clone(), line, col);
                    return Ok(());
                }
                // M41: función externa (FFI) → CallExtern. Sus argumentos van a la pila como en una
                // llamada ordinaria; la VM los marshala y llama a la librería C.
                if let Some(&idx) = self.extern_indices.get(name) {
                    for arg in args {
                        self.emit_expr(arg)?;
                    }
                    self.emit(OpCode::CallExtern(idx, args.len()), line, col);
                    return Ok(());
                }
                if let Some(&idx) = self.indices.get(name) {
                    for arg in args {
                        self.emit_expr(arg)?;
                    }
                    self.emit(OpCode::Call(idx, args.len()), line, col);
                    return Ok(());
                }
            }
        }

        // Indirecto: primero el valor-función, luego los argumentos, luego CallValue.
        self.emit_expr(callee)?;
        for arg in args {
            self.emit_expr(arg)?;
        }
        self.emit(OpCode::CallValue(args.len()), line, col);
        Ok(())
    }
}

/// Optimización de **llamadas en cola** (M13.3b): un *peephole* que convierte cada `Call`/`CallValue`
/// cuya continuación es un `Return` en `TailCall`/`TailCallValue`. Si tras la llamada el control va
/// directo a `Return` —o a saltos incondicionales que acaban en `Return`—, el valor de la llamada se
/// retorna **tal cual**: es una llamada en cola, y reutilizar el marco no cambia el resultado.
///
/// El compilador ya emite ese patrón de forma natural: la rama-else de un `if` cae al `Return` final
/// de la función, una rama-then salta a él, un `return e` lo emite tras `e`. Por eso basta con
/// reconocer el patrón en el bytecode ya generado, sin tocar la emisión.
/// V9 (ronda 5): el CIERRE del bucle contado. Corre tras la ronda 4:
///   - `[AddLocalConst(s, c), SetLocal(s), Jump(t)]` → `IncJump(s, c, t)` — el `i = i + 1;
///     salta a la guarda` de TODO for-range, en una instrucción.
///   - `[AddLocalConst(s, c), SetLocal(s)]` (sin salto detrás) → `IncLocalConst(s, c)` — el
///     incremento a secas (i = i + paso en un `while` manual).
/// Solo si el `SetLocal` escribe el MISMO slot que lee el `AddLocalConst`, y como siempre los
/// opcodes consumidos (i+1, i+2) no pueden ser destino de salto. Mismo esquema de remapeo.
fn fuse_loop_round5(chunk: &mut Chunk) {
    let n = chunk.code.len();
    if n == 0 {
        return;
    }
    let mut is_target = vec![false; n];
    for op in &chunk.code {
        match op {
            OpCode::Jump(t) | OpCode::JumpIfFalse(t) | OpCode::CmpJump(_, t)
            | OpCode::GetLocalConstCmpJump(_, _, _, t)
            | OpCode::LocalLocalCmpJump(_, _, _, t) => is_target[*t] = true,
            OpCode::DotRange { exit, .. } => is_target[*exit] = true,
            _ => {}
        }
    }
    let mut old_a_new = vec![0usize; n + 1];
    let mut code = Vec::with_capacity(n);
    let mut lines = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        old_a_new[i] = code.len();
        if i + 1 < n && !is_target[i + 1] {
            if let (OpCode::AddLocalConst(s, c), OpCode::SetLocal(s2)) =
                (&chunk.code[i], &chunk.code[i + 1])
            {
                if s == s2 {
                    // ¿Y el salto de vuelta justo detrás? → el cierre entero en una.
                    if i + 2 < n && !is_target[i + 2] {
                        if let OpCode::Jump(t) = &chunk.code[i + 2] {
                            code.push(OpCode::IncJump(*s, *c, *t)); // t en coords viejas
                            lines.push(chunk.lines[i]); // posición del Add (su overflow)
                            i += 3;
                            continue;
                        }
                    }
                    code.push(OpCode::IncLocalConst(*s, *c));
                    lines.push(chunk.lines[i]);
                    i += 2;
                    continue;
                }
            }
        }
        code.push(chunk.code[i].clone());
        lines.push(chunk.lines[i]);
        i += 1;
    }
    old_a_new[n] = code.len();
    for op in &mut code {
        match op {
            OpCode::Jump(t)
            | OpCode::JumpIfFalse(t)
            | OpCode::CmpJump(_, t)
            | OpCode::GetLocalConstCmpJump(_, _, _, t)
            | OpCode::LocalLocalCmpJump(_, _, _, t)
            | OpCode::IncJump(_, _, t) => *t = old_a_new[*t],
            OpCode::DotRange { exit, .. } => *exit = old_a_new[*exit],
            _ => {}
        }
    }
    chunk.code = code;
    chunk.lines = lines;
}

/// MM4: ¿`e` es `Ident[Ident]`? Devuelve `(arreglo, índice)` por nombre.
fn dot_index_shape(e: &Expr) -> Option<(&str, &str)> {
    let ExprKind::Index { array, index } = &e.kind else { return None };
    let ExprKind::Ident(a) = &array.kind else { return None };
    let ExprKind::Ident(i) = &index.kind else { return None };
    Some((a, i))
}

fn optimize_tail_calls(chunk: &mut Chunk) {
    for i in 0..chunk.code.len() {
        let new = match &chunk.code[i] {
            OpCode::Call(idx, argc) if returns_immediately(chunk, i + 1) => {
                Some(OpCode::TailCall(*idx, *argc))
            }
            OpCode::CallValue(argc) if returns_immediately(chunk, i + 1) => {
                Some(OpCode::TailCallValue(*argc))
            }
            _ => None,
        };
        if let Some(op) = new {
            chunk.code[i] = op;
        }
    }
}

/// ¿La ejecución desde `j` llega a un `Return` sin tocar la pila (solo saltos incondicionales)?
/// Sigue la cadena de `Jump` con un tope de saltos para no ciclar (un `while` salta hacia atrás).
fn returns_immediately(chunk: &Chunk, mut j: usize) -> bool {
    for _ in 0..=chunk.code.len() {
        match chunk.code.get(j) {
            Some(OpCode::Return) => return true,
            Some(OpCode::Jump(t)) => j = *t,
            _ => return false,
        }
    }
    false
}

/// **Superinstrucciones** (M36.1): fusiona pares de opcodes adyacentes muy frecuentes en uno solo, para
/// que el lazo de despacho de la VM haga **una** iteración en vez de dos. El despacho (el `match` gigante +
/// el avance del `ip`) es un coste dominante en una VM de switch; `GetLocal` es el opcode más frecuente y
/// casi siempre carga un operando seguido de otro `GetLocal` o una `Constant`. Fusionamos:
/// - `GetLocal(s); GetLocal(t)` → `GetLocalLocal(s, t)` (operandos de `a op b`).
/// - `GetLocal(s); Constant(c)` → `GetLocalConst(s, c)` (operandos de `x op <literal>`, `i < N`, …).
///
/// Fusionar acorta el vector de código, lo que **desplaza los índices** → hay que **remapear los destinos
/// de salto** (`Jump`/`JumpIfFalse`, los únicos opcodes con destino de código). Un par NO se fusiona si su
/// segundo opcode es **destino de un salto** (algo aterrizaría entre medias). Corre tras el TCO (no fusiona
/// a través de una llamada). El resultado es equivalente: mismos empujes, mismos saltos → oráculo intacto.
/// Opt.12: evalúa en COMPILACIÓN una expresión hecha solo de literales `int`/`float`/
/// `bool` y operaciones **totales** (que no pueden fallar en runtime). Devuelve `None`
/// ante cualquier cosa que deba quedar para el runtime: una sub-expresión no literal,
/// división/módulo enteros (el 0 y `MIN/-1` deben trapear con su posición), overflow
/// del `int` checked, y los tipos fuera de alcance (string/uint/char). Cada regla es
/// la MISMA expresión de Rust que ejecuta el fast-path de la VM (y `apply_binary`) →
/// plegar no cambia ningún resultado observable y el oráculo VM↔intérprete no lo ve.
fn const_fold(e: &Expr) -> Option<Value> {
    use Value::{Bool, Float, Int};
    match &e.kind {
        ExprKind::Int(n) => Some(Int(*n)),
        ExprKind::Float(f) => Some(Float(*f)),
        ExprKind::Bool(b) => Some(Bool(*b)),
        ExprKind::Unary { op, expr } => match (op, const_fold(expr)?) {
            (UnaryOp::Neg, Int(n)) => n.checked_neg().map(Int),
            (UnaryOp::Neg, Float(f)) => Some(Float(-f)),
            (UnaryOp::Not, Bool(b)) => Some(Bool(!b)),
            (UnaryOp::BitNot, Int(n)) => Some(Int(!n)),
            _ => None,
        },
        ExprKind::Binary { op, left, right } => {
            let (l, r) = (const_fold(left)?, const_fold(right)?);
            match (op, l, r) {
                // Aritmética entera: checked — un overflow NO se pliega (trap de runtime).
                (BinaryOp::Add, Int(a), Int(b)) => a.checked_add(b).map(Int),
                (BinaryOp::Sub, Int(a), Int(b)) => a.checked_sub(b).map(Int),
                (BinaryOp::Mul, Int(a), Int(b)) => a.checked_mul(b).map(Int),
                (BinaryOp::Div, Int(a), Int(b)) if b != 0 => a.checked_div(b).map(Int),
                (BinaryOp::Rem, Int(a), Int(b)) if b != 0 => a.checked_rem(b).map(Int),
                // Bit a bit: totales (mismos `wrapping_*` que ambos motores).
                (BinaryOp::BitAnd, Int(a), Int(b)) => Some(Int(a & b)),
                (BinaryOp::BitOr, Int(a), Int(b)) => Some(Int(a | b)),
                (BinaryOp::BitXor, Int(a), Int(b)) => Some(Int(a ^ b)),
                (BinaryOp::Shl, Int(a), Int(b)) => Some(Int(a.wrapping_shl(b as u32))),
                (BinaryOp::Shr, Int(a), Int(b)) => Some(Int(a.wrapping_shr(b as u32))),
                // Aritmética float: total (IEEE; inf/NaN son valores, no errores).
                (BinaryOp::Add, Float(a), Float(b)) => Some(Float(a + b)),
                (BinaryOp::Sub, Float(a), Float(b)) => Some(Float(a - b)),
                (BinaryOp::Mul, Float(a), Float(b)) => Some(Float(a * b)),
                (BinaryOp::Div, Float(a), Float(b)) => Some(Float(a / b)),
                (BinaryOp::Rem, Float(a), Float(b)) => Some(Float(a % b)),
                // Comparaciones e igualdad.
                (BinaryOp::Lt, Int(a), Int(b)) => Some(Bool(a < b)),
                (BinaryOp::Le, Int(a), Int(b)) => Some(Bool(a <= b)),
                (BinaryOp::Gt, Int(a), Int(b)) => Some(Bool(a > b)),
                (BinaryOp::Ge, Int(a), Int(b)) => Some(Bool(a >= b)),
                (BinaryOp::Eq, Int(a), Int(b)) => Some(Bool(a == b)),
                (BinaryOp::Ne, Int(a), Int(b)) => Some(Bool(a != b)),
                (BinaryOp::Lt, Float(a), Float(b)) => Some(Bool(a < b)),
                (BinaryOp::Le, Float(a), Float(b)) => Some(Bool(a <= b)),
                (BinaryOp::Gt, Float(a), Float(b)) => Some(Bool(a > b)),
                (BinaryOp::Ge, Float(a), Float(b)) => Some(Bool(a >= b)),
                (BinaryOp::Eq, Float(a), Float(b)) => Some(Bool(a == b)),
                (BinaryOp::Ne, Float(a), Float(b)) => Some(Bool(a != b)),
                (BinaryOp::Eq, Bool(a), Bool(b)) => Some(Bool(a == b)),
                (BinaryOp::Ne, Bool(a), Bool(b)) => Some(Bool(a != b)),
                // Lógicos: con ambos lados literales el cortocircuito es irrelevante.
                (BinaryOp::And, Bool(a), Bool(b)) => Some(Bool(a && b)),
                (BinaryOp::Or, Bool(a), Bool(b)) => Some(Bool(a || b)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn fuse_superinstructions(chunk: &mut Chunk) {
    let n = chunk.code.len();
    if n == 0 {
        return;
    }
    // (1) Marca qué índices son destino de algún salto (no se puede fusionar "dentro" de ellos).
    let mut is_target = vec![false; n];
    for op in &chunk.code {
        match op {
            OpCode::Jump(t) | OpCode::JumpIfFalse(t) => is_target[*t] = true,
            OpCode::DotRange { exit, .. } => is_target[*exit] = true,
            _ => {}
        }
    }
    // (2) Reconstruye el código fusionando pares elegibles; `viejo_a_nuevo[i]` = nueva posición de la
    // instrucción que empezaba en `i`. El segundo opcode de un par fusionado nunca es destino de salto
    // (lo garantiza `es_destino`), así que su entrada en el mapa queda sin usar.
    let mut old_a_new = vec![0usize; n];
    let mut code = Vec::with_capacity(n);
    let mut lines = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        old_a_new[i] = code.len();
        if i + 1 < n && !is_target[i + 1] {
            if let Some(fusion) = match (&chunk.code[i], &chunk.code[i + 1]) {
                (OpCode::GetLocal(s), OpCode::GetLocal(t)) => Some(OpCode::GetLocalLocal(*s, *t)),
                (OpCode::GetLocal(s), OpCode::Constant(c)) => Some(OpCode::GetLocalConst(*s, *c)),
                _ => None,
            } {
                code.push(fusion);
                lines.push(chunk.lines[i]); // la posición del primer opcode del par
                i += 2;
                continue;
            }
        }
        code.push(chunk.code[i].clone());
        lines.push(chunk.lines[i]);
        i += 1;
    }
    // (3) Remapea los destinos de salto a las nuevas posiciones.
    for op in &mut code {
        match op {
            OpCode::Jump(t) | OpCode::JumpIfFalse(t) => *t = old_a_new[*t],
            OpCode::DotRange { exit, .. } => *exit = old_a_new[*exit],
            _ => {}
        }
    }
    chunk.code = code;
    chunk.lines = lines;
}

/// A4 (ronda 2, elegida por HISTOGRAMA dinámico de pares): fusiones sobre la salida del pase 1.
///   - `[Unit, Pop]` → se ELIMINA (la asignación-como-sentencia empuja unit y lo tira; un salto
///     que caiga en el `Unit` queda bien remapeado a la siguiente instrucción — unit+pop = no-op).
///   - `[Cmp, JumpIfFalse(t), Pop]` con `code[t] == Pop` → `CmpJump(op, t+1)`: la guarda de todo
///     `if`/`while` en UNA instrucción; el bool nunca toca la pila y el salto brinca el Pop del
///     lado else (que sigue existiendo para otros saltos que lo usen).
///   - `[GetLocalConst(s, c), Add|Sub]` → `AddLocalConst`/`SubLocalConst` (el `i + 1`, `n - 1`).
///     La posición registrada es la del Add/Sub (ahí puede nacer el error de desbordamiento).
/// Mismo esquema de remapeo que el pase 1; los índices consumidos no pueden ser destino de salto.
fn fuse_round2(chunk: &mut Chunk) {
    let n = chunk.code.len();
    if n == 0 {
        return;
    }
    let mut is_target = vec![false; n];
    for op in &chunk.code {
        match op {
            OpCode::Jump(t) | OpCode::JumpIfFalse(t) => is_target[*t] = true,
            OpCode::DotRange { exit, .. } => is_target[*exit] = true,
            _ => {}
        }
    }
    // n+1 entradas: una CmpJump puede apuntar a `t+1 == n` (el Pop era la última instrucción).
    let mut old_a_new = vec![0usize; n + 1];
    let mut code = Vec::with_capacity(n);
    let mut lines = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        old_a_new[i] = code.len();
        // [Cmp, JumpIfFalse(t), Pop] con code[t] == Pop → CmpJump(op, t+1)
        if i + 2 < n && !is_target[i + 1] && !is_target[i + 2] {
            let cmp = match &chunk.code[i] {
                OpCode::Less => Some(CmpOp::Less),
                OpCode::LessEqual => Some(CmpOp::LessEqual),
                OpCode::Greater => Some(CmpOp::Greater),
                OpCode::GreaterEqual => Some(CmpOp::GreaterEqual),
                OpCode::Equal => Some(CmpOp::Equal),
                OpCode::NotEqual => Some(CmpOp::NotEqual),
                _ => None,
            };
            if let (Some(op), OpCode::JumpIfFalse(t), OpCode::Pop) =
                (cmp, &chunk.code[i + 1], &chunk.code[i + 2])
            {
                if matches!(chunk.code.get(*t), Some(OpCode::Pop)) {
                    code.push(OpCode::CmpJump(op, *t + 1)); // en coordenadas VIEJAS; se remapea abajo
                    lines.push(chunk.lines[i]);
                    i += 3;
                    continue;
                }
            }
        }
        if i + 1 < n && !is_target[i + 1] {
            // [GetLocalConst, Add|Sub] → AddLocalConst/SubLocalConst
            if let OpCode::GetLocalConst(s2, c) = &chunk.code[i] {
                let fusion = match &chunk.code[i + 1] {
                    OpCode::Add => Some(OpCode::AddLocalConst(*s2, *c)),
                    OpCode::Sub => Some(OpCode::SubLocalConst(*s2, *c)),
                    _ => None,
                };
                if let Some(f) = fusion {
                    code.push(f);
                    lines.push(chunk.lines[i + 1]); // la posición del Add/Sub (el error nace ahí)
                    i += 2;
                    continue;
                }
            }
            // [Unit, Pop] → nada (unit+pop es un no-op; ver la nota de arriba)
            if matches!(
                (&chunk.code[i], &chunk.code[i + 1]),
                (OpCode::Unit, OpCode::Pop)
            ) {
                i += 2;
                continue;
            }
        }
        code.push(chunk.code[i].clone());
        lines.push(chunk.lines[i]);
        i += 1;
    }
    old_a_new[n] = code.len();
    for op in &mut code {
        match op {
            OpCode::Jump(t) | OpCode::JumpIfFalse(t) | OpCode::CmpJump(_, t) => {
                *t = old_a_new[*t]
            }
            OpCode::DotRange { exit, .. } => *exit = old_a_new[*exit],
            _ => {}
        }
    }
    chunk.code = code;
    chunk.lines = lines;
}

/// **Superinstrucciones — ronda 3** (P0.6, elegida por histograma dinámico de pares ejecutados). Corre
/// DESPUÉS de `fuse_round2` (que es quien crea `CmpJump`) y fusiona la guarda entera de `if`/`while`
/// sobre `local op const`:
///
///   - `[GetLocalConst(s, c), CmpJump(op, t)]` → `GetLocalConstCmpJump(s, c, op, t)`.
///
/// M98.1: peephole `[Spawn, Pop]` → `[SpawnDiscard, Pop]` — un `spawn(f);` como sentencia descarta el
/// `Task<T>`; sin este pase la entrada del almacén de tareas quedaría retenida para siempre (nadie la
/// consume: la fuga de ~1 KB/tarea de la investigación de memoria). Reemplazo IN SITU (no borra
/// instrucciones → cero remapeo de saltos) y con efecto de pila idéntico (`SpawnDiscard` empuja `unit`
/// y el `Pop` que sigue lo tira) → seguro aunque un salto caiga en el `Pop`.
fn discard_spawn_results(chunk: &mut Chunk) {
    let n = chunk.code.len();
    for i in 0..n.saturating_sub(1) {
        if matches!(chunk.code[i], OpCode::Spawn) && matches!(chunk.code[i + 1], OpCode::Pop) {
            chunk.code[i] = OpCode::SpawnDiscard;
        }
    }
}

/// Es el par MÁS ejecutado en fib/bucles (`n < 2`, `i < N`): en fib(34), 18.5M veces. El `GetLocalConst`
/// SÍ puede ser destino de salto (la vuelta de un `while` apunta al inicio de la condición) → un salto a
/// él remapea a la fusión; solo se exige que el `CmpJump` (i+1) NO sea destino (nada salta a mitad de
/// guarda). Mismo esquema de remapeo que los pases 1 y 2.
fn fuse_guard_round3(chunk: &mut Chunk) {
    let n = chunk.code.len();
    if n == 0 {
        return;
    }
    let mut is_target = vec![false; n];
    for op in &chunk.code {
        match op {
            OpCode::Jump(t) | OpCode::JumpIfFalse(t) | OpCode::CmpJump(_, t) => is_target[*t] = true,
            OpCode::DotRange { exit, .. } => is_target[*exit] = true,
            _ => {}
        }
    }
    let mut old_a_new = vec![0usize; n + 1];
    let mut code = Vec::with_capacity(n);
    let mut lines = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        old_a_new[i] = code.len();
        // [GetLocalConst(s, c), CmpJump(op, t)] → GetLocalConstCmpJump(s, c, op, t)
        // V9: y la hermana con tope en VARIABLE:
        // [GetLocalLocal(a, b), CmpJump(op, t)] → LocalLocalCmpJump(a, b, op, t)
        if i + 1 < n && !is_target[i + 1] {
            if let OpCode::CmpJump(op, t) = &chunk.code[i + 1] {
                match &chunk.code[i] {
                    OpCode::GetLocalConst(s, c) => {
                        code.push(OpCode::GetLocalConstCmpJump(*s, *c, *op, *t)); // t en coords viejas
                        lines.push(chunk.lines[i + 1]); // posición del CmpJump
                        i += 2;
                        continue;
                    }
                    OpCode::GetLocalLocal(a, b) => {
                        code.push(OpCode::LocalLocalCmpJump(*a, *b, *op, *t)); // t en coords viejas
                        lines.push(chunk.lines[i + 1]);
                        i += 2;
                        continue;
                    }
                    _ => {}
                }
            }
        }
        code.push(chunk.code[i].clone());
        lines.push(chunk.lines[i]);
        i += 1;
    }
    old_a_new[n] = code.len();
    for op in &mut code {
        match op {
            OpCode::Jump(t)
            | OpCode::JumpIfFalse(t)
            | OpCode::CmpJump(_, t)
            | OpCode::GetLocalConstCmpJump(_, _, _, t)
            | OpCode::LocalLocalCmpJump(_, _, _, t)
            | OpCode::IncJump(_, _, t) => *t = old_a_new[*t],
            OpCode::DotRange { exit, .. } => *exit = old_a_new[*exit],
            _ => {}
        }
    }
    chunk.code = code;
    chunk.lines = lines;
}


/// MM2 (ronda 4, bench matrixmul): fusiona la INDEXACIÓN — el patrón dominante de los bucles
/// numéricos sobre arreglos (`a[i]`, `a[i][k]`):
///   - `[GetLocalLocal(s, t), Index]` → `IndexLL(s, t)` (base e índice locales: la forma `a[i]`).
///   - `[GetLocal(t), Index]` → `IndexLocal(t)` (la base ya está en la pila: el segundo nivel de
///     `a[i][k]`, o un `x[k]` cuya base vino de otra expresión).
/// En `s + a[i][k] * b[k][j]` (matrixmul) el bucle interno pasa de ~15 a ~9 despachos. Mismo
/// esquema de remapeo de saltos que las rondas anteriores; un salto que caiga ENTRE los dos
/// opcodes del par anula esa fusión (`is_target`).
fn fuse_index_round4(chunk: &mut Chunk) {
    let n = chunk.code.len();
    if n == 0 {
        return;
    }
    let mut is_target = vec![false; n];
    for op in &chunk.code {
        match op {
            OpCode::Jump(t) | OpCode::JumpIfFalse(t) | OpCode::CmpJump(_, t)
            | OpCode::GetLocalConstCmpJump(_, _, _, t)
            | OpCode::LocalLocalCmpJump(_, _, _, t) => is_target[*t] = true,
            OpCode::DotRange { exit, .. } => is_target[*exit] = true,
            _ => {}
        }
    }
    let mut old_a_new = vec![0usize; n + 1];
    let mut code = Vec::with_capacity(n);
    let mut lines = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        old_a_new[i] = code.len();
        if i + 1 < n && !is_target[i + 1] && matches!(chunk.code[i + 1], OpCode::Index) {
            match &chunk.code[i] {
                OpCode::GetLocalLocal(s, t) => {
                    code.push(OpCode::IndexLL(*s, *t));
                    lines.push(chunk.lines[i + 1]); // posición del Index (la del error de bounds)
                    i += 2;
                    continue;
                }
                OpCode::GetLocal(t) => {
                    code.push(OpCode::IndexLocal(*t));
                    lines.push(chunk.lines[i + 1]);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        code.push(chunk.code[i].clone());
        lines.push(chunk.lines[i]);
        i += 1;
    }
    old_a_new[n] = code.len();
    for op in &mut code {
        match op {
            OpCode::Jump(t)
            | OpCode::JumpIfFalse(t)
            | OpCode::CmpJump(_, t)
            | OpCode::GetLocalConstCmpJump(_, _, _, t)
            | OpCode::LocalLocalCmpJump(_, _, _, t)
            | OpCode::IncJump(_, _, t) => *t = old_a_new[*t],
            OpCode::DotRange { exit, .. } => *exit = old_a_new[*exit],
            _ => {}
        }
    }
    chunk.code = code;
    chunk.lines = lines;
}
