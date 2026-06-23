//! Loader de módulos (M11.3a) — *cliente* host-side, como el REPL/runner/LSP.
//!
//! Un programa raylang puede repartirse en varios archivos; cada archivo es un **módulo**
//! (su nombre es el *stem*: `math.ray` → módulo `math`). Este loader, dado el archivo de
//! entrada, sigue las declaraciones `import M;`, carga y parsea cada módulo (una vez; los
//! ciclos del grafo no son problema) y **fusiona todo en un único `Program`**, de modo que el
//! checker / intérprete / VM no saben de módulos (se borran en el front-end, como UFCS o los
//! diccionarios).
//!
//! Alcance de M11.3a: solo se **namespacan funciones** (`modulo::fn`) y se cruzan funciones
//! `pub` vía acceso calificado `M.f(...)`. Los tipos/enums/traits siguen en un espacio global
//! único (un choque de nombres entre módulos es error); cruzar tipos → diferido. Por eso el
//! resolutor solo reescribe referencias a **funciones** (no a tipos ni patrones).
//!
//! M11.3b añade `from M import a [as b];`: trae **funciones `pub`** de `M` al ámbito del módulo
//! (sin calificar), con renombrado opcional. Es la misma resolución de -a: el nombre local (el
//! alias, o el original) se inyecta en el mapa `own` apuntando al nombre global `M::a`. Importar
//! *tipos* sigue diferido (no se namespacan); el loader lo reporta con claridad.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{Block, Expr, ExprKind, FnExpr, Function, MethodSig, PatternKind, Program, Stmt, StmtKind, Type};
use crate::diagnostic;

/// Un error de carga, ya **renderizado** con su contexto de fuente (la línea + `^`), listo
/// para imprimir. El loader conoce el archivo donde ocurre cada error, así que lo dibuja él.
pub struct LoadError {
    pub message: String,
}

/// El resultado de cargar: el `Program` fusionado (listo para `check`) y los **módulos** con su
/// **banda de líneas** en el espacio de posiciones global, para atribuir y renderizar los errores
/// posteriores del checker/runtime contra el archivo y la línea **local** correctos.
pub struct Loaded {
    pub program: Program,
    /// Ordenados por `start_line` ascendente. La banda del módulo `i` es `[start_line_i, …)` hasta
    /// el `start_line` del siguiente.
    pub modules: Vec<LoadedModule>,
}

/// Un módulo cargado, con su fuente y la línea global donde empieza su banda (L3).
pub struct LoadedModule {
    pub name: String,
    pub source: String,
    pub start_line: usize,
}

impl Loaded {
    /// ¿El programa abarca más de un módulo? (Para decidir si prefijar errores con `[módulo]`.)
    pub fn multi_modulo(&self) -> bool {
        self.modules.len() > 1
    }

    /// Localiza una línea **global** del programa fusionado: devuelve `(módulo, fuente, línea
    /// local)`. La línea local renumera respecto al inicio de la banda del módulo, así un error
    /// se renderiza contra el archivo correcto con su número de línea real.
    pub fn locate(&self, line: usize) -> (&str, &str, usize) {
        let m = self.modules.iter().rev().find(|m| m.start_line <= line)
            .unwrap_or_else(|| &self.modules[0]);
        (&m.name, &m.source, line - m.start_line + 1)
    }
}

/// Un módulo cargado: su nombre, si es el de entrada, su AST y su fuente.
struct Module {
    name: String,
    is_entry: bool,
    program: Program,
    source: String,
}

/// Carga el archivo de entrada y sus imports (transitivos), y devuelve el programa fusionado.
pub fn load(entry: &Path) -> Result<Loaded, LoadError> {
    let root = entry.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let entry_name = module_name(entry);

    // --- Fase 1: cargar y parsear cada módulo una vez (BFS sobre los imports) ---
    let mut modules: Vec<Module> = Vec::new();
    let mut visitados: HashSet<String> = HashSet::new();
    let mut pendientes: Vec<(String, PathBuf, bool)> = vec![(entry_name.clone(), entry.to_path_buf(), true)];
    while let Some((name, path, is_entry)) = pendientes.pop() {
        if !visitados.insert(name.clone()) {
            continue; // ya cargado (los ciclos se cierran aquí)
        }
        let source = std::fs::read_to_string(&path).map_err(|e| LoadError {
            message: format!("no se pudo leer el módulo '{}' ({}): {}", name, path.display(), e),
        })?;
        let program = parse_source(&name, &source)?;
        // Dependencias del módulo: tanto `import M;` como `from M import …;` cargan `M`.
        let deps = program.imports.iter().map(|i| (&i.module, i.line, i.col))
            .chain(program.from_imports.iter().map(|f| (&f.module, f.line, f.col)));
        for (dep, line, col) in deps {
            if !visitados.contains(dep) {
                let mp = root.join(format!("{}.ray", dep));
                if !mp.exists() {
                    return Err(render(&source, line, col, &name,
                        &format!("no se encuentra el módulo '{}' (se esperaba {})", dep, mp.display())));
                }
                pendientes.push((dep.clone(), mp, false));
            }
        }
        modules.push(Module { name, is_entry, program, source });
    }

    // --- Fase 2: tipos únicos **dentro de cada módulo** (M11.3c: ya no globales) ---
    comprobar_tipos_unicos(&modules)?;

    // --- Fase 3: namespacing + resolución + desambiguación de posiciones (L3) ---
    let pub_fns = recolectar_pub_fns(&modules);
    let tipos = recolectar_tipos(&modules); // para diagnosticar `from M import <Tipo>` (diferido)
    let mut fusionado = Program {
        functions: Vec::new(), structs: Vec::new(), enums: Vec::new(),
        traits: Vec::new(), impls: Vec::new(), imports: Vec::new(), from_imports: Vec::new(),
    };

    // El módulo de **entrada** se fusiona primero, en `delta` 0 (sus líneas coinciden con su
    // archivo): un programa de un solo archivo queda **idéntico** a antes. Cada módulo siguiente
    // ocupa una **banda de líneas distinta** del espacio de posiciones global → posiciones
    // globalmente únicas (el lowering por posición de M9 no colisiona entre módulos) y errores
    // que renderizan contra el archivo y la línea local correctos. `sort_by_key` es estable.
    modules.sort_by_key(|m| !m.is_entry);

    let mut loaded_modules: Vec<LoadedModule> = Vec::new();
    let mut next_start = 1usize; // primera línea libre del espacio global
    for mut m in modules {
        let prefix = if m.is_entry { None } else { Some(m.name.clone()) };

        // 1. `@derive` (M11.3c): se expande **aquí**, con los nombres **locales** (re-lexables),
        //    antes de namespacar; los `impl` generados se namespacan luego como los demás. El
        //    checker la reejecuta sobre el programa fusionado, pero es idempotente y los salta.
        crate::checker::generate_derives(&mut m.program)
            .map_err(|e| render(&m.source, e.line, e.col, &m.name, &e.to_string()))?;

        // 2. Resolución de **valores** (funciones propias, `from`-imports, acceso calificado `M.f`).
        let mut resolver = Resolver::new(&m, &pub_fns, &tipos)?;
        resolver.resolve_module(&mut m)?;

        // 3. Namespacing de **tipos** (M11.3c): renombrar las definiciones de este módulo a su
        //    nombre global (`modulo::Tipo`) y **reescribir** todas las referencias de tipo —en
        //    posiciones de tipo y en expresiones que nombran tipos (struct lit, `Color.Rojo`,
        //    patrones)—, sin tocar los parámetros de tipo en ámbito.
        let own_types = build_own_types(&m.program, &prefix);
        rename_type_defs(&mut m.program, &own_types);
        TypeRewriter::new(&own_types).rewrite_program(&mut m.program);

        // 4. Banda de este módulo: empieza en `next_start`; sus posiciones se desplazan por `delta`.
        let start = next_start;
        shift_program(&mut m.program, start - 1);
        next_start = start + m.source.lines().count().max(1) + 1; // +1 de holgura entre bandas

        // 5. Renombrar las definiciones de función a su nombre global y fusionar.
        for mut f in std::mem::take(&mut m.program.functions) {
            f.name = global_fn(&prefix, &f.name);
            fusionado.functions.push(f);
        }
        fusionado.structs.append(&mut m.program.structs);
        fusionado.enums.append(&mut m.program.enums);
        fusionado.traits.append(&mut m.program.traits);
        fusionado.impls.append(&mut m.program.impls);

        loaded_modules.push(LoadedModule { name: m.name, source: m.source, start_line: start });
    }
    loaded_modules.sort_by_key(|m| m.start_line);
    Ok(Loaded { program: fusionado, modules: loaded_modules })
}

/// Desplaza **todas** las posiciones (línea) de un módulo por `delta` (L3). La columna se conserva.
/// Aplicar el mismo `delta` a cada nodo preserva las posiciones **relativas** dentro del módulo
/// (de las que dependen las pre-pasadas del checker, p. ej. que un `Call` comparta posición con su
/// receptor) y, con bandas disjuntas, vuelve las posiciones **únicas entre módulos**. Con `delta`
/// 0 es un no-op (el caso de un solo archivo).
fn shift_program(program: &mut Program, delta: usize) {
    if delta == 0 {
        return;
    }
    for f in &mut program.functions {
        shift_function(f, delta);
    }
    for s in &mut program.structs {
        s.line += delta;
        for a in &mut s.annotations {
            a.line += delta;
        }
    }
    for e in &mut program.enums {
        e.line += delta;
        for a in &mut e.annotations {
            a.line += delta;
        }
        for v in &mut e.variants {
            v.line += delta;
        }
    }
    for t in &mut program.traits {
        t.line += delta;
        for m in &mut t.methods {
            m.line += delta;
            for p in &mut m.params {
                p.line += delta;
            }
            if let Some(b) = &mut m.default_body {
                shift_block(b, delta);
            }
        }
    }
    for imp in &mut program.impls {
        imp.line += delta;
        for m in &mut imp.methods {
            shift_function(m, delta);
        }
    }
}

fn shift_function(f: &mut Function, delta: usize) {
    f.line += delta;
    for a in &mut f.annotations {
        a.line += delta;
    }
    for p in &mut f.params {
        p.line += delta;
    }
    shift_block(&mut f.body, delta);
}

fn shift_block(b: &mut Block, delta: usize) {
    b.line += delta;
    for s in &mut b.statements {
        shift_stmt(s, delta);
    }
    if let Some(t) = &mut b.tail {
        shift_expr(t, delta);
    }
}

fn shift_stmt(s: &mut Stmt, delta: usize) {
    s.line += delta;
    match &mut s.kind {
        StmtKind::Let { value, .. } => shift_expr(value, delta),
        StmtKind::Assign { target, value } => {
            shift_expr(target, delta);
            shift_expr(value, delta);
        }
        StmtKind::Return { value } => {
            if let Some(v) = value {
                shift_expr(v, delta);
            }
        }
        StmtKind::Expr(e) => shift_expr(e, delta),
    }
}

fn shift_expr(e: &mut Expr, delta: usize) {
    e.line += delta;
    match &mut e.kind {
        ExprKind::Unary { expr, .. } => shift_expr(expr, delta),
        ExprKind::Binary { left, right, .. } => {
            shift_expr(left, delta);
            shift_expr(right, delta);
        }
        ExprKind::Call { callee, args } => {
            shift_expr(callee, delta);
            for a in args {
                shift_expr(a, delta);
            }
        }
        ExprKind::ArrayLit(elems) => {
            for x in elems {
                shift_expr(x, delta);
            }
        }
        ExprKind::Index { array, index } => {
            shift_expr(array, delta);
            shift_expr(index, delta);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, x) in fields {
                shift_expr(x, delta);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                shift_expr(a, delta);
            }
        }
        ExprKind::Field { object, .. } => shift_expr(object, delta),
        ExprKind::Func(fe) => shift_fn_expr(fe, delta),
        ExprKind::Match { scrutinee, arms } => {
            shift_expr(scrutinee, delta);
            for arm in arms {
                arm.line += delta;
                arm.pattern.line += delta;
                shift_expr(&mut arm.body, delta);
            }
        }
        ExprKind::Try(inner) => shift_expr(inner, delta),
        ExprKind::If { cond, then_branch, else_branch } => {
            shift_expr(cond, delta);
            shift_block(then_branch, delta);
            if let Some(x) = else_branch {
                shift_expr(x, delta);
            }
        }
        ExprKind::While { cond, body } => {
            shift_expr(cond, delta);
            shift_block(body, delta);
        }
        ExprKind::Block(b) => shift_block(b, delta),
        _ => {}
    }
}

fn shift_fn_expr(fe: &mut FnExpr, delta: usize) {
    fe.line += delta;
    for p in &mut fe.params {
        p.line += delta;
    }
    shift_block(&mut fe.body, delta);
}

/// El nombre de módulo de una ruta: su *stem* (`dir/math.ray` → `math`).
fn module_name(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("main").to_string()
}

/// Nombre global de una función **o tipo**: `modulo::nombre` para un módulo importado; el propio
/// nombre para el de entrada (sus nombres ya son globales; `main` debe seguir siendo `main`).
fn global_fn(prefix: &Option<String>, name: &str) -> String {
    match prefix {
        Some(m) => format!("{}::{}", m, name),
        None => name.to_string(),
    }
}

/// Mapa `nombre local → nombre global` de los **tipos** (struct/enum/trait) de un módulo (M11.3c).
fn build_own_types(program: &Program, prefix: &Option<String>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for s in &program.structs {
        map.insert(s.name.clone(), global_fn(prefix, &s.name));
    }
    for e in &program.enums {
        map.insert(e.name.clone(), global_fn(prefix, &e.name));
    }
    for t in &program.traits {
        map.insert(t.name.clone(), global_fn(prefix, &t.name));
    }
    map
}

/// Renombra las **definiciones** de tipo de un módulo a su nombre global (M11.3c). Las
/// *referencias* las reescribe `TypeRewriter`.
fn rename_type_defs(program: &mut Program, own_types: &HashMap<String, String>) {
    for s in &mut program.structs {
        if let Some(g) = own_types.get(&s.name) {
            s.name = g.clone();
        }
    }
    for e in &mut program.enums {
        if let Some(g) = own_types.get(&e.name) {
            e.name = g.clone();
        }
    }
    for t in &mut program.traits {
        if let Some(g) = own_types.get(&t.name) {
            t.name = g.clone();
        }
    }
}

fn parse_source(name: &str, source: &str) -> Result<Program, LoadError> {
    let tokens = crate::lexer::lex(source).map_err(|e| render(source, e.line, e.col, name, &e.to_string()))?;
    crate::parser::parse(tokens).map_err(|e| render(source, e.line, e.col, name, &e.to_string()))
}

/// Construye un `LoadError` renderizado: antepone `[módulo]` y dibuja el contexto de fuente.
fn render(source: &str, line: usize, col: usize, module: &str, msg: &str) -> LoadError {
    let headline = format!("[{}] {}", module, msg);
    LoadError { message: diagnostic::render(source, line, col, &headline) }
}

/// Los tipos (struct/enum/trait) se namespacan por módulo (M11.3c), así que dos **módulos** pueden
/// reusar un nombre. Pero **dentro** de un módulo siguen debiendo ser únicos (dos `Foo` colisionan).
fn comprobar_tipos_unicos(modules: &[Module]) -> Result<(), LoadError> {
    for m in modules {
        let mut visto: HashSet<String> = HashSet::new();
        let nombres = m.program.structs.iter().map(|s| (s.name.clone(), s.line, s.col))
            .chain(m.program.enums.iter().map(|e| (e.name.clone(), e.line, e.col)))
            .chain(m.program.traits.iter().map(|t| (t.name.clone(), t.line, t.col)));
        for (name, line, col) in nombres {
            if !visto.insert(name.clone()) {
                return Err(render(&m.source, line, col, &m.name, &format!(
                    "el tipo '{}' ya está definido en este módulo", name
                )));
            }
        }
    }
    Ok(())
}

/// Por módulo, el conjunto de nombres de funciones `pub` (para verificar el acceso calificado).
fn recolectar_pub_fns(modules: &[Module]) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();
    for m in modules {
        let set = map.entry(m.name.clone()).or_default();
        for f in &m.program.functions {
            if f.is_pub {
                set.insert(f.name.clone());
            }
        }
    }
    map
}

/// Por módulo, el conjunto de nombres de tipos (struct/enum/trait). Solo se usa para dar un
/// mensaje claro cuando un `from M import X` apunta a un tipo (importar tipos está diferido).
fn recolectar_tipos(modules: &[Module]) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();
    for m in modules {
        let set = map.entry(m.name.clone()).or_default();
        set.extend(m.program.structs.iter().map(|s| s.name.clone()));
        set.extend(m.program.enums.iter().map(|e| e.name.clone()));
        set.extend(m.program.traits.iter().map(|t| t.name.clone()));
    }
    map
}

/// Resuelve un nombre de un `from M import a [as b]` al nombre global `M::a`, verificando que `a`
/// sea una **función `pub`** de `M`. Si `a` es un **tipo** de `M`, da un error de "diferido"; si no
/// existe como función pública, un error de no-exporta.
fn resolver_from_import(
    src: &str,
    module: &str,
    fi: &crate::ast::FromImport,
    name: &crate::ast::ImportName,
    pub_fns: &HashMap<String, HashSet<String>>,
    tipos: &HashMap<String, HashSet<String>>,
) -> Result<String, LoadError> {
    let from = &fi.module;
    if pub_fns.get(from).is_some_and(|s| s.contains(&name.name)) {
        return Ok(format!("{}::{}", from, name.name));
    }
    // No es una función pública: ¿es un tipo? (importar tipos está diferido).
    if tipos.get(from).is_some_and(|s| s.contains(&name.name)) {
        return Err(render(src, name.line, name.col, module, &format!(
            "'{}' es un tipo del módulo '{}'; importar tipos entre módulos está diferido — los tipos son globales, referéncialo por su nombre",
            name.name, from
        )));
    }
    Err(render(src, name.line, name.col, module, &format!(
        "el módulo '{}' no exporta una función '{}' (¿falta 'pub'?)", from, name.name
    )))
}

/// Reescribe, *consciente de ámbitos*, las referencias a funciones de un módulo a sus nombres
/// globales: las propias (`foo` → `modulo::foo`) y las calificadas (`M.f` → `M::f`, con `pub`).
struct Resolver<'a> {
    /// Nombres de nivel superior visibles **sin calificar** en este módulo: las funciones propias
    /// y las traídas por `from M import …` (con su alias). Mapea nombre local → nombre global.
    own: HashMap<String, String>,
    /// Módulos importados con `import M;` (para el acceso calificado `M.item`). Los módulos que
    /// solo aparecen en `from M import …` **no** entran aquí (estilo Python: no traen `M`).
    imports: HashSet<String>,
    /// Funciones `pub` por módulo (de todo el programa), para verificar `M.f` y los from-imports.
    pub_fns: &'a HashMap<String, HashSet<String>>,
    /// Pila de ámbitos locales: nombres que tapan a las funciones de nivel superior.
    scopes: Vec<HashSet<String>>,
}

impl<'a> Resolver<'a> {
    fn new(
        m: &Module,
        pub_fns: &'a HashMap<String, HashSet<String>>,
        tipos: &HashMap<String, HashSet<String>>,
    ) -> Result<Self, LoadError> {
        let prefix = if m.is_entry { None } else { Some(m.name.clone()) };
        let mut own = HashMap::new();
        for f in &m.program.functions {
            own.insert(f.name.clone(), global_fn(&prefix, &f.name));
        }
        // M11.3b: inyectar los `from M import a [as b]` como nombres locales → `M::a`.
        for fi in &m.program.from_imports {
            for n in &fi.names {
                let target = resolver_from_import(&m.source, &m.name, fi, n, pub_fns, tipos)?;
                let local = n.local().to_string();
                if own.insert(local.clone(), target).is_some() {
                    return Err(render(&m.source, n.line, n.col, &m.name, &format!(
                        "el nombre '{}' ya está definido o importado en este módulo; usa 'as' para renombrarlo",
                        local
                    )));
                }
            }
        }
        let imports = m.program.imports.iter().map(|i| i.module.clone()).collect();
        Ok(Resolver { own, imports, pub_fns, scopes: Vec::new() })
    }

    fn resolve_module(&mut self, m: &mut Module) -> Result<(), LoadError> {
        // Nota: `m.program.functions` y `m.program.impls` se recorren por separado, pero
        // ambos comparten el mismo mapa de resolución (`self.own`/`self.imports`).
        let (src, module) = (m.source.clone(), m.name.clone());
        for f in &mut m.program.functions {
            self.resolve_fn(f, &src, &module)?;
        }
        for imp in &mut m.program.impls {
            for method in &mut imp.methods {
                self.resolve_fn(method, &src, &module)?;
            }
        }
        Ok(())
    }

    fn resolve_fn(&mut self, f: &mut Function, src: &str, module: &str) -> Result<(), LoadError> {
        self.scopes.push(f.params.iter().map(|p| p.name.clone()).collect());
        self.resolve_block(&mut f.body, src, module)?;
        self.scopes.pop();
        Ok(())
    }

    fn declarado_local(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(name))
    }

    fn declarar(&mut self, name: &str) {
        if let Some(s) = self.scopes.last_mut() {
            s.insert(name.to_string());
        }
    }

    fn resolve_block(&mut self, block: &mut Block, src: &str, module: &str) -> Result<(), LoadError> {
        self.scopes.push(HashSet::new());
        for stmt in &mut block.statements {
            self.resolve_stmt(stmt, src, module)?;
        }
        if let Some(t) = &mut block.tail {
            self.resolve_expr(t, src, module)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn resolve_stmt(&mut self, stmt: &mut Stmt, src: &str, module: &str) -> Result<(), LoadError> {
        match &mut stmt.kind {
            StmtKind::Let { name, value, .. } => {
                self.resolve_expr(value, src, module)?;
                self.declarar(name); // el binding entra en ámbito tras su inicializador
            }
            StmtKind::Assign { target, value } => {
                self.resolve_expr(target, src, module)?;
                self.resolve_expr(value, src, module)?;
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    self.resolve_expr(v, src, module)?;
                }
            }
            StmtKind::Expr(e) => self.resolve_expr(e, src, module)?,
        }
        Ok(())
    }

    fn resolve_expr(&mut self, expr: &mut Expr, src: &str, module: &str) -> Result<(), LoadError> {
        let (line, col) = (expr.line, expr.col);
        match &mut expr.kind {
            // Acceso calificado `M.f(args)`: si la cabeza es un módulo importado (y no una
            // local que lo tape), se reescribe a `Ident("M::f")` tras verificar que `f` es pub.
            ExprKind::Call { callee, args } => {
                for a in args.iter_mut() {
                    self.resolve_expr(a, src, module)?;
                }
                if let Some(global) = self.qualified_target(callee, src, module)? {
                    **callee = Expr { kind: ExprKind::Ident(global), line: callee.line, col: callee.col };
                } else {
                    self.resolve_expr(callee, src, module)?;
                }
            }
            ExprKind::Ident(name) => {
                // Una referencia a una función propia del módulo (no tapada por una local) se
                // reescribe a su nombre global; lo demás (prelude/builtin/local) lo deja el checker.
                let global = self.own.get(name).filter(|_| !self.declarado_local(name)).cloned();
                if let Some(g) = global {
                    *name = g;
                }
            }
            ExprKind::Field { object, name } => {
                // `M.f` en posición de valor (función como valor): mismo trato calificado.
                if let Some(global) = self.qualified_field(object, name, src, module)? {
                    expr.kind = ExprKind::Ident(global);
                } else {
                    self.resolve_expr(object, src, module)?;
                }
            }
            ExprKind::Unary { expr: inner, .. } => self.resolve_expr(inner, src, module)?,
            ExprKind::Binary { left, right, .. } => {
                self.resolve_expr(left, src, module)?;
                self.resolve_expr(right, src, module)?;
            }
            ExprKind::ArrayLit(elems) => {
                for e in elems {
                    self.resolve_expr(e, src, module)?;
                }
            }
            ExprKind::Index { array, index } => {
                self.resolve_expr(array, src, module)?;
                self.resolve_expr(index, src, module)?;
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, e) in fields {
                    self.resolve_expr(e, src, module)?;
                }
            }
            ExprKind::EnumLit { args, .. } => {
                for a in args {
                    self.resolve_expr(a, src, module)?;
                }
            }
            ExprKind::Func(fe) => self.resolve_fn_expr(fe, src, module)?,
            ExprKind::Match { scrutinee, arms } => {
                self.resolve_expr(scrutinee, src, module)?;
                for arm in arms {
                    self.scopes.push(arm_bindings(arm));
                    self.resolve_expr(&mut arm.body, src, module)?;
                    self.scopes.pop();
                }
            }
            ExprKind::Try(inner) => self.resolve_expr(inner, src, module)?,
            ExprKind::If { cond, then_branch, else_branch } => {
                self.resolve_expr(cond, src, module)?;
                self.resolve_block(then_branch, src, module)?;
                if let Some(e) = else_branch {
                    self.resolve_expr(e, src, module)?;
                }
            }
            ExprKind::While { cond, body } => {
                self.resolve_expr(cond, src, module)?;
                self.resolve_block(body, src, module)?;
            }
            ExprKind::Block(b) => self.resolve_block(b, src, module)?,
            _ => {}
        }
        let _ = (line, col);
        Ok(())
    }

    fn resolve_fn_expr(&mut self, fe: &mut FnExpr, src: &str, module: &str) -> Result<(), LoadError> {
        self.scopes.push(fe.params.iter().map(|p| p.name.clone()).collect());
        self.resolve_block(&mut fe.body, src, module)?;
        self.scopes.pop();
        Ok(())
    }

    /// Si `callee` es `Field { object: Ident(M), name: f }` con `M` un módulo importado (no
    /// tapado por una local), devuelve el nombre global `M::f` (verificando que `f` es pub).
    fn qualified_target(&self, callee: &Expr, src: &str, module: &str) -> Result<Option<String>, LoadError> {
        if let ExprKind::Field { object, name } = &callee.kind {
            return self.qualified_field(object, name, src, module);
        }
        Ok(None)
    }

    /// `object.name` donde `object` es `Ident(M)`, `M` importado y no local → `Some("M::name")`
    /// si `name` es una función `pub` de `M`; error si no lo es; `None` si no es acceso a módulo.
    fn qualified_field(&self, object: &Expr, name: &str, src: &str, module: &str) -> Result<Option<String>, LoadError> {
        let ExprKind::Ident(m) = &object.kind else { return Ok(None) };
        if self.declarado_local(m) || !self.imports.contains(m) {
            return Ok(None); // una local tapa al módulo, o no es un módulo importado
        }
        let exporta = self.pub_fns.get(m).is_some_and(|s| s.contains(name));
        if !exporta {
            return Err(render(src, object.line, object.col, module, &format!(
                "el módulo '{}' no exporta una función '{}' (¿falta 'pub', o es un tipo?)", m, name
            )));
        }
        Ok(Some(format!("{}::{}", m, name)))
    }
}

/// Reescribe las **referencias de tipo** de un módulo a su nombre global (M11.3c), respetando los
/// **parámetros de tipo** en ámbito. Pasada separada de `Resolver` (que resuelve *valores*): aquí se
/// tocan las posiciones de tipo (anotaciones, campos, payloads, objetivo/trait de `impl`, bounds,
/// `dyn`) y las expresiones que **nombran** tipos (literal de struct, construcción de enum
/// `Tipo.Variante`, patrones de `match`). Un `Ident` *bare* en posición de valor lo deja `Resolver`.
struct TypeRewriter<'a> {
    /// Nombre local de tipo → nombre global (propios del módulo; en -2, también importados).
    types: &'a HashMap<String, String>,
    /// Pila de conjuntos de parámetros de tipo en ámbito (los `<…>` de fn/impl/struct/enum).
    tparams: Vec<HashSet<String>>,
}

impl<'a> TypeRewriter<'a> {
    fn new(types: &'a HashMap<String, String>) -> Self {
        TypeRewriter { types, tparams: Vec::new() }
    }

    fn en_ambito(&self, name: &str) -> bool {
        self.tparams.iter().any(|s| s.contains(name))
    }

    /// Reescribe un nombre de tipo/trait si es propio del módulo y **no** es un parámetro de tipo.
    fn rewrite_name(&self, name: &mut String) {
        if self.en_ambito(name) {
            return; // es un parámetro de tipo (`T`), no un tipo nominal
        }
        if let Some(g) = self.types.get(name) {
            *name = g.clone();
        }
    }

    fn rewrite_type(&self, ty: &mut Type) {
        match ty {
            // El parser emite `Struct` para cualquier identificador en posición de tipo; el
            // checker reclasifica a `Enum`/`Var` luego. Aquí se cubren ambos por si acaso.
            Type::Struct(name, args) | Type::Enum(name, args) => {
                self.rewrite_name(name);
                for a in args {
                    self.rewrite_type(a);
                }
            }
            Type::Array(elem) => self.rewrite_type(elem),
            Type::Fn(params, ret) => {
                for p in params {
                    self.rewrite_type(p);
                }
                self.rewrite_type(ret);
            }
            Type::Dyn(trait_name) => self.rewrite_name(trait_name),
            _ => {} // Int/Float/Bool/String/Unit/Var/SelfType
        }
    }

    fn rewrite_program(&mut self, program: &mut Program) {
        for s in &mut program.structs {
            self.tparams.push(s.type_params.iter().cloned().collect());
            for (_, ty) in &mut s.fields {
                self.rewrite_type(ty);
            }
            self.tparams.pop();
        }
        for e in &mut program.enums {
            self.tparams.push(e.type_params.iter().cloned().collect());
            for v in &mut e.variants {
                for ty in &mut v.payload {
                    self.rewrite_type(ty);
                }
            }
            self.tparams.pop();
        }
        for t in &mut program.traits {
            self.tparams.push(HashSet::new()); // los traits no llevan parámetros de tipo
            for m in &mut t.methods {
                self.rewrite_method(m);
            }
            self.tparams.pop();
        }
        for f in &mut program.functions {
            self.tparams.push(f.type_params.iter().cloned().collect());
            self.rewrite_function(f);
            self.tparams.pop();
        }
        for imp in &mut program.impls {
            self.tparams.push(imp.type_params.iter().cloned().collect());
            self.rewrite_type(&mut imp.target);
            self.rewrite_name(&mut imp.trait_name);
            for (_, tr) in &mut imp.bounds {
                self.rewrite_name(tr);
            }
            for method in &mut imp.methods {
                self.tparams.push(method.type_params.iter().cloned().collect());
                self.rewrite_function(method);
                self.tparams.pop();
            }
            self.tparams.pop();
        }
    }

    fn rewrite_function(&self, f: &mut Function) {
        for p in &mut f.params {
            self.rewrite_type(&mut p.ty);
        }
        self.rewrite_type(&mut f.return_type);
        for (_, tr) in &mut f.bounds {
            self.rewrite_name(tr);
        }
        self.rewrite_block(&mut f.body);
    }

    fn rewrite_method(&self, m: &mut MethodSig) {
        for p in &mut m.params {
            self.rewrite_type(&mut p.ty);
        }
        self.rewrite_type(&mut m.return_type);
        if let Some(b) = &mut m.default_body {
            self.rewrite_block(b);
        }
    }

    fn rewrite_block(&self, b: &mut Block) {
        for stmt in &mut b.statements {
            self.rewrite_stmt(stmt);
        }
        if let Some(t) = &mut b.tail {
            self.rewrite_expr(t);
        }
    }

    fn rewrite_stmt(&self, s: &mut Stmt) {
        match &mut s.kind {
            StmtKind::Let { ty, value, .. } => {
                if let Some(t) = ty {
                    self.rewrite_type(t);
                }
                self.rewrite_expr(value);
            }
            StmtKind::Assign { target, value } => {
                self.rewrite_expr(target);
                self.rewrite_expr(value);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    self.rewrite_expr(v);
                }
            }
            StmtKind::Expr(e) => self.rewrite_expr(e),
        }
    }

    fn rewrite_expr(&self, expr: &mut Expr) {
        match &mut expr.kind {
            // Literal de struct: el nombre es un tipo.
            ExprKind::StructLit { name, fields } => {
                self.rewrite_name(name);
                for (_, v) in fields {
                    self.rewrite_expr(v);
                }
            }
            // Construcción de enum `Tipo.Variante`: la cabeza, si es un tipo, se reescribe. Un
            // valor *bare* (`p.x`) no casa con ningún tipo, así que `rewrite_name` lo deja igual.
            ExprKind::Field { object, .. } => {
                if let ExprKind::Ident(name) = &mut object.kind {
                    self.rewrite_name(name);
                } else {
                    self.rewrite_expr(object);
                }
            }
            ExprKind::Call { callee, args } => {
                self.rewrite_expr(callee);
                for a in args {
                    self.rewrite_expr(a);
                }
            }
            // (En la fase del loader la construcción de enum aún es `Field`/`Call`, pero por si
            // acaso se cubre también el nodo resuelto.)
            ExprKind::EnumLit { enum_name, args, .. } => {
                self.rewrite_name(enum_name);
                for a in args {
                    self.rewrite_expr(a);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.rewrite_expr(scrutinee);
                for arm in arms {
                    if let PatternKind::Variant { enum_name, .. } = &mut arm.pattern.kind {
                        self.rewrite_name(enum_name);
                    }
                    self.rewrite_expr(&mut arm.body);
                }
            }
            ExprKind::Unary { expr: inner, .. } => self.rewrite_expr(inner),
            ExprKind::Binary { left, right, .. } => {
                self.rewrite_expr(left);
                self.rewrite_expr(right);
            }
            ExprKind::ArrayLit(elems) => {
                for e in elems {
                    self.rewrite_expr(e);
                }
            }
            ExprKind::Index { array, index } => {
                self.rewrite_expr(array);
                self.rewrite_expr(index);
            }
            ExprKind::Func(fe) => self.rewrite_fn_expr(fe),
            ExprKind::Try(inner) => self.rewrite_expr(inner),
            ExprKind::If { cond, then_branch, else_branch } => {
                self.rewrite_expr(cond);
                self.rewrite_block(then_branch);
                if let Some(e) = else_branch {
                    self.rewrite_expr(e);
                }
            }
            ExprKind::While { cond, body } => {
                self.rewrite_expr(cond);
                self.rewrite_block(body);
            }
            ExprKind::Block(b) => self.rewrite_block(b),
            _ => {} // Int/Float/Bool/Str/Ident(bare valor)
        }
    }

    fn rewrite_fn_expr(&self, fe: &mut FnExpr) {
        for p in &mut fe.params {
            self.rewrite_type(&mut p.ty);
        }
        self.rewrite_type(&mut fe.return_type);
        self.rewrite_block(&mut fe.body);
    }
}

/// Los nombres que liga un brazo de `match` (para meterlos en el ámbito del cuerpo).
fn arm_bindings(arm: &crate::ast::MatchArm) -> HashSet<String> {
    use crate::ast::PatternKind::*;
    let mut set = HashSet::new();
    match &arm.pattern.kind {
        Wildcard => {}
        Binding(n) => {
            set.insert(n.clone());
        }
        Variant { bindings, .. } => {
            for b in bindings.iter().flatten() {
                set.insert(b.clone());
            }
        }
    }
    set
}
