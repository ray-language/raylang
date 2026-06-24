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
    Chunk, CompiledEnum, CompiledFn, CompiledProgram, CompiledStruct, CompiledVariant, OpCode,
    UpvalueRef, UpvalueSource,
};
use crate::interpreter::Value;

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
    let main = *indices.get("main").expect("el checker garantiza 'main'");

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

    let mut c = Compiler {
        indices: &indices,
        structs: &struct_defs,
        enums: &enum_defs,
        n_named,
        functions: (0..total).map(|_| None).collect(),
        scopes: Vec::new(),
    };
    for (i, f) in program.functions.iter().enumerate() {
        c.compile_function(i, &f.params, &f.body, f.line, f.col, f.name.clone())?;
    }

    let functions = c.functions.into_iter().map(|o| o.expect("toda función quedó compilada")).collect();
    Ok(CompiledProgram { functions, structs: struct_table, enums: enum_table, main })
}

/// Compila una expresión suelta a un `Chunk` (sin variables ni llamadas). Se usa
/// en los tests de expresiones puras.
pub fn compile_expr(expr: &Expr) -> Result<Chunk, CompileError> {
    let indices = HashMap::new();
    let structs = HashMap::new();
    let enums = HashMap::new();
    let mut c = Compiler { indices: &indices, structs: &structs, enums: &enums, n_named: 0, functions: Vec::new(), scopes: Vec::new() };
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
        self.emit_block(body)?;
        self.emit(OpCode::Return, line, col);

        // M13.3b: optimización de llamadas en cola. Toda llamada cuya continuación es un `Return`
        // (directo o a través de saltos incondicionales) se convierte en `TailCall`, que reutiliza
        // el marco en vez de apilar uno nuevo → recursión de cola en O(1) marcos.
        optimize_tail_calls(&mut self.cur().chunk);

        let mut scope = self.scopes.pop().expect("acabamos de empujar el ámbito");
        scope.captured_slots.resize(scope.max_slots, false);
        self.functions[idx] = Some(CompiledFn {
            name,
            arity: params.len(),
            num_locals: scope.max_slots,
            captured: scope.captured_slots,
            upvalues: scope.upvalues,
            chunk: scope.chunk,
        });
        Ok(())
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
            _ => unreachable!("patch_jump sobre una instrucción que no es salto"),
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
            match &arm.pattern.kind {
                PatternKind::Variant { enum_name, variant, bindings } => {
                    // Tag de esta variante (de la tabla de enums del compilador).
                    let (_, vmap) = self.enums.get(enum_name).expect("el checker registró el enum");
                    let (tag, _arity) = *vmap.get(variant).expect("el checker validó la variante");
                    // ¿Es esta la variante? Si no, al siguiente brazo.
                    self.emit(OpCode::GetLocal(scrut_slot), aline, acol);
                    self.emit(OpCode::EnumTagEq(tag), aline, acol);
                    let to_next = self.emit(OpCode::JumpIfFalse(0), aline, acol);
                    self.emit(OpCode::Pop, aline, acol); // casó → descartar el bool true
                    // Ligar el payload en un ámbito propio del brazo (InitLocal boxea
                    // el slot si una closure del cuerpo lo captura).
                    let arm_saved = self.begin_scope();
                    for (i, b) in bindings.iter().enumerate() {
                        if let Some(name) = b {
                            self.emit(OpCode::GetLocal(scrut_slot), aline, acol);
                            self.emit(OpCode::GetEnumField(i), aline, acol);
                            let slot = self.declare_local(name);
                            self.emit(OpCode::InitLocal(slot), aline, acol);
                        }
                    }
                    self.emit_expr(&arm.body)?;
                    self.end_scope(arm_saved);
                    to_end.push(self.emit(OpCode::Jump(0), aline, acol));
                    // Etiqueta del siguiente brazo: descartar el bool false.
                    self.patch_jump(to_next);
                    self.emit(OpCode::Pop, aline, acol);
                }
                PatternKind::Wildcard => {
                    // Catch-all sin ligar: sin test, siempre se ejecuta.
                    has_catchall = true;
                    self.emit_expr(&arm.body)?;
                    to_end.push(self.emit(OpCode::Jump(0), aline, acol));
                }
                PatternKind::Binding(name) => {
                    // Catch-all que liga el escrutinio completo.
                    has_catchall = true;
                    let arm_saved = self.begin_scope();
                    self.emit(OpCode::GetLocal(scrut_slot), aline, acol);
                    let slot = self.declare_local(name);
                    self.emit(OpCode::InitLocal(slot), aline, acol);
                    self.emit_expr(&arm.body)?;
                    self.end_scope(arm_saved);
                    to_end.push(self.emit(OpCode::Jump(0), aline, acol));
                }
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
                _ => unreachable!("el checker garantiza un lvalue"),
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
                    } else {
                        // No es variable ni upvalue: un nombre de función como valor.
                        let idx = *self.indices.get(name).expect("el checker garantiza el nombre");
                        self.emit(OpCode::Function(idx), line, col);
                    }
                }
            }

            ExprKind::Unary { op, expr: inner } => {
                self.emit_expr(inner)?;
                let opc = match op {
                    UnaryOp::Neg => OpCode::Negate,
                    UnaryOp::Not => OpCode::Not,
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

            ExprKind::ArrayLit(elems) => {
                for e in elems {
                    self.emit_expr(e)?;
                }
                self.emit(OpCode::MakeArray(elems.len()), line, col);
            }

            ExprKind::Index { array, index } => {
                self.emit_expr(array)?;
                self.emit_expr(index)?;
                self.emit(OpCode::Index, line, col);
            }

            ExprKind::StructLit { name, fields } => {
                let (idx, field_names) = self.structs.get(name).expect("el checker registró el struct");
                let idx = *idx;
                let field_names = field_names.clone(); // suelta el préstamo de self
                // Emitimos los valores en ORDEN DE DECLARACIÓN (así MakeStruct los
                // empareja con los nombres de campo de la tabla).
                for fname in &field_names {
                    let value_expr = fields
                        .iter()
                        .find(|(n, _)| n == fname)
                        .map(|(_, e)| e)
                        .expect("el checker garantiza el campo");
                    self.emit_expr(value_expr)?;
                }
                self.emit(OpCode::MakeStruct(idx), line, col);
            }

            ExprKind::EnumLit { enum_name, variant, args } => {
                let (enum_id, variant_map) = self.enums.get(enum_name).expect("el checker registró el enum");
                let enum_id = *enum_id;
                let (tag, _arity) = *variant_map.get(variant).expect("el checker validó la variante");
                // Emitimos el payload en orden; MakeEnum saca esos valores y arma el enum.
                for a in args {
                    self.emit_expr(a)?;
                }
                self.emit(OpCode::MakeEnum(enum_id, tag), line, col);
            }

            ExprKind::Field { object, name } => {
                self.emit_expr(object)?;
                self.emit(OpCode::GetField(name.clone()), line, col);
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
        if let ExprKind::Ident(name) = &callee.kind {
            // Solo es directo si el nombre NO es una variable (local o upvalue).
            if !self.name_is_variable(name) {
                // Builtin: el opcode lo da el registro único (`src/builtins.rs`).
                if let Some(b) = crate::builtins::lookup(name) {
                    for arg in args {
                        self.emit_expr(arg)?;
                    }
                    self.emit(b.opcode.clone(), line, col);
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
fn optimize_tail_calls(chunk: &mut Chunk) {
    for i in 0..chunk.code.len() {
        let nuevo = match &chunk.code[i] {
            OpCode::Call(idx, argc) if returns_immediately(chunk, i + 1) => {
                Some(OpCode::TailCall(*idx, *argc))
            }
            OpCode::CallValue(argc) if returns_immediately(chunk, i + 1) => {
                Some(OpCode::TailCallValue(*argc))
            }
            _ => None,
        };
        if let Some(op) = nuevo {
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
