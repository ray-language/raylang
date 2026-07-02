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
//! alias, o el original) se inyecta en el mapa `own` apuntando al nombre global `M::a`.
//!
//! M11.3c-2 extiende `from M import …` a **tipos `pub`** (`from M import Punto [as P]`): el nombre
//! local se inyecta, no en el `own` de valores, sino en el mapa del `TypeRewriter`, apuntando al
//! global `M::Punto`. Así una referencia de tipo a `Punto`/`P` (anotación, literal, patrón…) se
//! reescribe igual que un tipo propio. La clasificación valor-vs-tipo es por `pub_fns`/`pub_types`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{Block, Expr, ExprKind, FnExpr, ForIter, ForPat, Function, MethodSig, PatternKind, Program, Stmt, StmtKind, Type};
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
    load_con_deps(entry, &[])
}

/// Como [`load`], pero además busca los módulos en las **raíces de dependencias** `dep_roots`
/// (la caché `.ray-deps/`, M39c): tras la raíz del proyecto, cada raíz extra se prueba en orden
/// (lo local tapa a una dependencia con el mismo nombre). Una dependencia descargada es una
/// **cápsula** (`<dep>/mod.ray`): `import geo;` la trae y sus submódulos internos quedan
/// protegidos por el enforcement de cápsula (M11.6b) sin código nuevo. Con `dep_roots` vacío el
/// comportamiento es idéntico a antes (un solo `root`).
pub fn load_con_deps(entry: &Path, dep_roots: &[PathBuf]) -> Result<Loaded, LoadError> {
    load_impl(entry, dep_roots, None)
}

/// Como [`load_con_deps`], pero usa `fuente` como contenido del **archivo de entrada** en vez de
/// leerlo del disco (los imports sí se leen de disco). Es lo que necesita el **LSP**: analizar el
/// buffer en memoria (con cambios sin guardar) mientras resuelve sus imports desde los archivos.
pub fn load_fuente(entry: &Path, fuente: &str, dep_roots: &[PathBuf]) -> Result<Loaded, LoadError> {
    load_impl(entry, dep_roots, Some(fuente))
}

/// Núcleo de la carga. `entry_source`, si está, es el contenido del archivo de entrada (buffer en
/// memoria); si es `None`, la entrada se lee del disco como los demás módulos.
fn load_impl(entry: &Path, dep_roots: &[PathBuf], entry_source: Option<&str>) -> Result<Loaded, LoadError> {
    let project_root = entry.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    // Raíces de búsqueda de módulos: el proyecto primero, luego la caché de dependencias.
    let mut roots = vec![project_root];
    roots.extend(dep_roots.iter().cloned());
    let entry_name = module_name(entry);

    // --- Fase 1: cargar y parsear cada módulo una vez (BFS sobre los imports) ---
    let mut modules: Vec<Module> = Vec::new();
    let mut visitados: HashSet<String> = HashSet::new();
    let mut pendientes: Vec<(String, PathBuf, bool)> = vec![(entry_name.clone(), entry.to_path_buf(), true)];
    while let Some((name, path, is_entry)) = pendientes.pop() {
        if !visitados.insert(name.clone()) {
            continue; // ya cargado (los ciclos se cierran aquí)
        }
        // El archivo de entrada puede venir de un buffer en memoria (LSP); el resto, del disco.
        let source = match entry_source {
            Some(s) if is_entry => s.to_string(),
            _ => std::fs::read_to_string(&path).map_err(|e| LoadError {
                message: format!("no se pudo leer el módulo '{}' ({}): {}", name, path.display(), e),
            })?,
        };
        let program = parse_source(&name, &source)?;
        // Dependencias del módulo: tanto `import M;` como `from M import …;` cargan `M`.
        let deps = program.imports.iter().map(|i| (&i.module, i.line, i.col))
            .chain(program.from_imports.iter().map(|f| (&f.module, f.line, f.col)));
        for (dep, line, col) in deps {
            // M11.6b: la arista de import debe respetar el borde de cápsula (aunque `dep` ya
            // esté visitado: cada sitio que importa un submódulo interno desde fuera es ilegal).
            if let Some(c) = capsula_violada(&roots, &name, dep) {
                return Err(render(&source, line, col, 1, &name, &format!(
                    "el módulo '{}' es interno a la cápsula '{}'; impórtalo con 'import {};'",
                    dep, c, c
                )));
            }
            if !visitados.contains(dep) {
                match resolve_module_path(&roots, dep) {
                    Err(msg) => return Err(render(&source, line, col, 1, &name, &msg)),
                    Ok(Some(mp)) => pendientes.push((dep.clone(), mp, false)),
                    Ok(None) => return Err(render(&source, line, col, 1, &name, &format!(
                        "no se encuentra el módulo '{}' (se esperaba '{}.ray' o '{}/mod.ray' en: {})",
                        dep, dep, dep,
                        roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")
                    ))),
                }
            }
        }
        modules.push(Module { name, is_entry, program, source });
    }

    // --- Fase 2: tipos únicos **dentro de cada módulo** (M11.3c: ya no globales) ---
    comprobar_tipos_unicos(&modules)?;

    // --- Fase 3: namespacing + resolución + desambiguación de posiciones (L3) ---
    // La **superficie pública** por módulo: nombre exportado → nombre global de destino (M11.6a).
    // Unifica ítems `pub` definidos y reexports (`pub from …`); la consultan el Resolver, el
    // TypeRewriter y la clasificación de `from`-imports.
    let surfaces = build_surfaces(&modules);
    let tipos = recolectar_tipos(&modules); // todos los tipos: para distinguir "privado" de "no existe"
    let mut fusionado = Program {
        functions: Vec::new(), structs: Vec::new(), enums: Vec::new(), consts: Vec::new(),
        traits: Vec::new(), impls: Vec::new(), imports: Vec::new(), from_imports: Vec::new(),
        ufcs_aliases: HashMap::new(), expr_spans: HashMap::new(), field_name_pos: HashMap::new(),
    };
    // Alias UFCS: nombre local de función `from`-importada → global, agregado de todos los módulos.
    // Un nombre que mapee a DOS globales distintos en módulos distintos es ambiguo sin contexto de
    // módulo → se **excluye** (degradación segura: ese nombre cae al comportamiento previo).
    let mut ufcs_aliases: HashMap<String, String> = HashMap::new();
    let mut ufcs_ambiguos: HashSet<String> = HashSet::new();

    // El módulo de **entrada** se fusiona primero, en `delta` 0 (sus líneas coinciden con su
    // archivo): un programa de un solo archivo queda **idéntico** a antes. Cada módulo siguiente
    // ocupa una **banda de líneas distinta** del espacio de posiciones global → posiciones
    // globalmente únicas (el lowering por posición de M9 no colisiona entre módulos) y errores
    // que renderizan contra el archivo y la línea local correctos. `sort_by_key` es estable.
    modules.sort_by_key(|m| !m.is_entry);

    let mut loaded_modules: Vec<LoadedModule> = Vec::new();
    let mut next_start = 1usize; // primera línea libre del espacio global
    for mut m in modules {
        let prefix = module_prefix(&m);

        // 1. `@derive` (M11.3c): se expande **aquí**, con los nombres **locales** (re-lexables),
        //    antes de namespacar; los `impl` generados se namespacan luego como los demás. El
        //    checker la reejecuta sobre el programa fusionado, pero es idempotente y los salta.
        crate::checker::generate_derives(&mut m.program)
            .map_err(|e| render(&m.source, e.line, e.col, e.len, &m.name, &e.to_string()))?;

        // 2. Clasificar los `from M import …` (M11.3b/-2): funciones `pub` → mapa de **valores**
        //    (al `own` del Resolver); tipos `pub` → mapa de **tipos** (al TypeRewriter).
        let (from_values, from_types) =
            clasificar_from_imports(&m, &surfaces, &tipos)?;

        // Acumular los alias UFCS de este módulo (función from-importada: local → global). Un alias
        // que ya existía con OTRO global es ambiguo entre módulos → se marca para excluir.
        for (local, global) in &from_values {
            match ufcs_aliases.get(local) {
                Some(g) if g != global => { ufcs_ambiguos.insert(local.clone()); }
                _ => { ufcs_aliases.insert(local.clone(), global.clone()); }
            }
        }

        // 3. Resolución de **valores** (funciones propias, `from`-imports de función, `c.f`,
        //    construcción de enum calificada `c.Color.Rojo`). El `import_map` (leaf → ruta, M11.5)
        //    lo comparten el Resolver y el TypeRewriter.
        let import_map = build_import_map(&m)?;
        let mut resolver = Resolver::new(&m, &surfaces, &from_values, &import_map);
        resolver.resolve_module(&mut m)?;

        // 4. Namespacing de **tipos** (M11.3c): renombrar las definiciones de este módulo a su
        //    nombre global (`modulo::Tipo`) y **reescribir** todas las referencias de tipo —en
        //    posiciones de tipo y en expresiones que nombran tipos (struct lit, `Color.Rojo`,
        //    patrones)—, sin tocar los parámetros de tipo en ámbito. El mapa de reescritura suma
        //    los tipos propios y los `from`-importados (-2).
        let own_types = build_own_types(&m.program, &prefix);
        rename_type_defs(&mut m.program, &own_types);
        let mut type_refs = own_types;
        type_refs.extend(from_types);
        TypeRewriter::new(&type_refs, &import_map, &surfaces).rewrite_program(&mut m.program);

        // 5. Banda de este módulo: empieza en `next_start`; sus posiciones se desplazan por `delta`.
        let start = next_start;
        shift_program(&mut m.program, start - 1);
        next_start = start + m.source.lines().count().max(1) + 1; // +1 de holgura entre bandas

        // 6. Renombrar las definiciones de función a su nombre global y fusionar.
        for mut f in std::mem::take(&mut m.program.functions) {
            f.name = global_fn(&prefix, &f.name);
            fusionado.functions.push(f);
        }
        fusionado.structs.append(&mut m.program.structs);
        fusionado.enums.append(&mut m.program.enums);
        fusionado.consts.append(&mut m.program.consts); // M27.5
        fusionado.traits.append(&mut m.program.traits);
        fusionado.impls.append(&mut m.program.impls);
        fusionado.expr_spans.extend(std::mem::take(&mut m.program.expr_spans));
        fusionado.field_name_pos.extend(std::mem::take(&mut m.program.field_name_pos));

        loaded_modules.push(LoadedModule { name: m.name, source: m.source, start_line: start });
    }
    loaded_modules.sort_by_key(|m| m.start_line);
    for amb in &ufcs_ambiguos {
        ufcs_aliases.remove(amb);
    }
    fusionado.ufcs_aliases = ufcs_aliases;
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
    // M33a-2: los spans de expresiones viajan con su módulo (claves y valores).
    program.expr_spans = std::mem::take(&mut program.expr_spans)
        .into_iter()
        .map(|((l, c), (el, ec))| ((l + delta, c), (el + delta, ec)))
        .collect();
    // M10.2g: la posición del nombre de campo/método también (clave y valor llevan línea).
    program.field_name_pos = std::mem::take(&mut program.field_name_pos)
        .into_iter()
        .map(|((l, c, n), (nl, nc))| ((l + delta, c, n), (nl + delta, nc)))
        .collect();
    for f in &mut program.functions {
        shift_function(f, delta);
    }
    for c in &mut program.consts {
        c.line += delta;
        shift_expr(&mut c.value, delta);
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
        StmtKind::LetTuple { value, .. } => shift_expr(value, delta),
        StmtKind::For { iter, body, .. } => {
            match iter {
                ForIter::Range { start, end } => { shift_expr(start, delta); shift_expr(end, delta); }
                ForIter::In(e) => shift_expr(e, delta),
            }
            shift_block(body, delta);
        }
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
        ExprKind::Unary { expr, .. } | ExprKind::Cast { expr, .. } => shift_expr(expr, delta),
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
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
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

/// Enforcement de la cápsula (M11.6b). Una importación del módulo `target` por `importer` **cruza el
/// borde de una cápsula** si `target` es interno a una cápsula `C` que no contiene a `importer`. `C`
/// es el **ancestro-directorio estricto más cercano** de `target` con un `mod.ray` (de ahí que se
/// itere de la ruta más profunda a la más superficial). `importer` está **dentro** de `C` si es la
/// propia cápsula (`importer == C`, su `mod.ray`) o vive bajo `C/`. Devuelve `Some(C)` si la viola.
/// Las cápsulas anidadas componen: estar dentro de la interior implica estar dentro de la exterior,
/// así que basta comprobar la más cercana.
fn capsula_violada(roots: &[PathBuf], importer: &str, target: &str) -> Option<String> {
    let segs: Vec<&str> = target.split('/').collect();
    for k in (1..segs.len()).rev() {
        let c = segs[..k].join("/");
        // Una cápsula `C` existe si CUALQUIER raíz tiene `<raíz>/C/mod.ray` (una dependencia
        // descargada es una cápsula en la caché; sus internos quedan protegidos igual que los del
        // proyecto). La más profunda gana (cápsulas anidadas componen).
        if roots.iter().any(|r| r.join(&c).join("mod.ray").exists()) {
            let dentro = importer == c || importer.starts_with(&format!("{}/", c));
            return if dentro { None } else { Some(c) };
        }
    }
    None
}

/// Resuelve la **ruta de módulo** `dep` (p. ej. `geo/formas/circulo` o `geo`) a un archivo bajo
/// `root` (M11.6a). Prueba primero `dep.ray` (módulo-archivo) y, si no, `dep/mod.ray` (módulo-
/// directorio: el `mod.ray` *es* el módulo de identidad `dep`). Una sola forma canónica: si **ambos**
/// existen, es ambiguo (error) —evita el lío histórico de `foo.rs`-vs-`foo/mod.rs`—. `None` si no
/// existe ninguno.
fn resolve_module_path(roots: &[PathBuf], dep: &str) -> Result<Option<PathBuf>, String> {
    // Se prueban las raíces en orden (proyecto → caché de dependencias); la primera que resuelva
    // gana. La ambigüedad archivo-vs-directorio se comprueba **dentro** de cada raíz.
    for root in roots {
        let as_file = root.join(format!("{}.ray", dep));
        let as_dir = root.join(dep).join("mod.ray");
        match (as_file.exists(), as_dir.exists()) {
            (true, true) => return Err(format!(
                "el módulo '{}' es ambiguo: existen '{}' y '{}'; deja solo uno",
                dep, as_file.display(), as_dir.display()
            )),
            (true, false) => return Ok(Some(as_file)),
            (false, true) => return Ok(Some(as_dir)),
            (false, false) => {} // no está en esta raíz; probar la siguiente
        }
    }
    Ok(None)
}

/// Prefijo de namespacing de una **ruta** de módulo (M11.5): los separadores de directorio `/` se
/// traducen al separador interno `::` (`geo/formas/circulo` → `geo::formas::circulo`). Para una ruta
/// de un solo segmento (carpeta plana, M11.3) es la identidad. El `::` es ilegal en identificadores
/// del usuario, así que los nombres globales nunca colisionan con los suyos.
fn ns_prefix(path: &str) -> String {
    path.replace('/', "::")
}

/// Nombre global de una función **o tipo**: `prefijo::nombre` para un módulo importado; el propio
/// nombre para el de entrada (sus nombres ya son globales; `main` debe seguir siendo `main`).
fn global_fn(prefix: &Option<String>, name: &str) -> String {
    match prefix {
        Some(m) => format!("{}::{}", m, name),
        None => name.to_string(),
    }
}

/// El prefijo de namespacing de un módulo: `None` para el de entrada (sin namespacing), o
/// `Some(ns_prefix(ruta))` para uno importado (M11.5: traduce `/`→`::`).
fn module_prefix(m: &Module) -> Option<String> {
    if m.is_entry { None } else { Some(ns_prefix(&m.name)) }
}

/// Mapa `nombre local → nombre global` de los **tipos** (struct/enum/trait) de un módulo (M11.3c).
fn build_own_types(program: &Program, prefix: &Option<String>) -> NameMap {
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
fn rename_type_defs(program: &mut Program, own_types: &NameMap) {
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
    let tokens = crate::lexer::lex(source).map_err(|e| render(source, e.line, e.col, e.len, name, &e.to_string()))?;
    crate::parser::parse(tokens).map_err(|e| render(source, e.line, e.col, e.len, name, &e.to_string()))
}

/// Construye un `LoadError` renderizado: antepone `[módulo]` y dibuja el contexto de fuente.
fn render(source: &str, line: usize, col: usize, len: usize, module: &str, msg: &str) -> LoadError {
    let headline = format!("[{}] {}", module, msg);
    LoadError { message: diagnostic::render(source, line, col, len, &headline) }
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
                return Err(render(&m.source, line, col, 1, &m.name, &format!(
                    "el tipo '{}' ya está definido en este módulo", name
                )));
            }
        }
    }
    Ok(())
}

/// La **superficie pública** de un módulo (M11.6a): por cada nombre exportado, el **nombre global de
/// destino**. Se separa en `values` (funciones) y `types` (struct/enum/trait) porque el acceso los
/// trata distinto (valor vs posición de tipo). A diferencia de M11.3, guarda el global en vez de solo
/// el nombre: así un **reexport** (`pub from P import a`) apunta al global del ítem en `P` —que se
/// define en *otro* módulo, no aquí—, no a un `ns_prefix(este_módulo)::a` inexistente.
#[derive(Default)]
struct Surface {
    values: HashMap<String, String>,
    types: HashMap<String, String>,
}

/// La superficie pública de **cada** módulo, por su nombre (ruta).
type Surfaces = HashMap<String, Surface>;

/// Calcula la superficie pública de todos los módulos (M11.6a). Dos pasadas:
/// 1. **Ítems `pub` definidos**: su global es `ns_prefix(módulo)::nombre` (lo de M11.3c).
/// 2. **Reexports** (`pub from P import a as b`): el global es el ya **resuelto** de `a` en la
///    superficie de `P`. Punto fijo: se repite hasta que una pasada no añade nada, para cubrir las
///    cadenas reexport-de-reexport (el caso común —reexportar un `pub` definido— converge en una).
fn build_surfaces(modules: &[Module]) -> Surfaces {
    let mut surfaces: Surfaces = HashMap::new();
    // Pasada 1: ítems pub definidos.
    for m in modules {
        let prefix = module_prefix(m);
        let s = surfaces.entry(m.name.clone()).or_default();
        for f in &m.program.functions {
            if f.is_pub {
                s.values.insert(f.name.clone(), global_fn(&prefix, &f.name));
            }
        }
        for st in &m.program.structs {
            if st.is_pub {
                s.types.insert(st.name.clone(), global_fn(&prefix, &st.name));
            }
        }
        for e in &m.program.enums {
            if e.is_pub {
                s.types.insert(e.name.clone(), global_fn(&prefix, &e.name));
            }
        }
        for t in &m.program.traits {
            if t.is_pub {
                s.types.insert(t.name.clone(), global_fn(&prefix, &t.name));
            }
        }
    }
    // Pasada 2: reexports, a punto fijo.
    loop {
        // (módulo destino, nombre local, global, es_tipo)
        let mut additions: Vec<(String, String, String, bool)> = Vec::new();
        for m in modules {
            for fi in m.program.from_imports.iter().filter(|f| f.is_pub) {
                let from = surfaces.get(&fi.module);
                let actual = surfaces.get(&m.name);
                for n in &fi.names {
                    let local = n.local().to_string();
                    let Some(fs) = from else { continue };
                    if let Some(g) = fs.values.get(&n.name)
                        && actual.is_none_or(|s| !s.values.contains_key(&local))
                    {
                        additions.push((m.name.clone(), local, g.clone(), false));
                    } else if let Some(g) = fs.types.get(&n.name)
                        && actual.is_none_or(|s| !s.types.contains_key(&local))
                    {
                        additions.push((m.name.clone(), local, g.clone(), true));
                    }
                }
            }
        }
        if additions.is_empty() {
            break;
        }
        for (module, local, global, es_tipo) in additions {
            let s = surfaces.entry(module).or_default();
            if es_tipo {
                s.types.insert(local, global);
            } else {
                s.values.insert(local, global);
            }
        }
    }
    surfaces
}

/// Por módulo, el conjunto de **todos** los nombres de tipos (struct/enum/trait, `pub` o no). Sirve
/// para distinguir, en un `from M import X` que no resuelve, un tipo **privado** (falta `pub`) de un
/// nombre **inexistente**.
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

/// Mapa `nombre local → nombre global` (`M::nombre`). Lo usan tanto el `own` de valores del
/// Resolver como el mapa de tipos del TypeRewriter.
type NameMap = HashMap<String, String>;

/// Mapa `leaf → ruta` de los `import a/b/c [as x];` de un módulo (M11.5): el nombre local con el que
/// se accede (el último segmento, o el alias) → la ruta completa del módulo (`geo/formas/circulo`),
/// que es su identidad (clave de `pub_fns`/`pub_types`). Lo consultan `Resolver` y `TypeRewriter`
/// para resolver el acceso calificado (`circulo.f`, `circulo.Tipo`). En M11.3 (carpeta plana) leaf y
/// ruta coinciden, así que es compatible hacia atrás.
type ImportMap = HashMap<String, String>;

/// Construye el `ImportMap` de un módulo a partir de sus `import` (M11.5). Detecta colisiones de
/// **leaf** (dos rutas con el mismo último segmento sin `as`) y pide renombrar con `as`.
fn build_import_map(m: &Module) -> Result<ImportMap, LoadError> {
    let mut map = ImportMap::new();
    for i in &m.program.imports {
        let leaf = i.leaf().to_string();
        if let Some(otra) = map.insert(leaf.clone(), i.module.clone())
            && otra != i.module
        {
            return Err(render(&m.source, i.line, i.col, 1, &m.name, &format!(
                "el nombre de módulo '{}' ya nombra a '{}'; usa 'as' para renombrar esta importación",
                leaf, otra
            )));
        }
    }
    Ok(map)
}

/// El destino global de un nombre `from`-importado: una **función** (va al `own` del Resolver) o un
/// **tipo** (va al mapa del TypeRewriter). Ambos guardan el nombre global `M::nombre`.
enum FromTarget {
    Funcion(String),
    Tipo(String),
}

/// Clasifica **todos** los `from M import a [as b]{, …}` de un módulo en dos mapas `local → global`:
/// uno de **valores** (funciones `pub`) y otro de **tipos** (tipos `pub`, M11.3c-2). Valida la
/// exportación (`pub`) y la unicidad del nombre local (contra las definiciones propias del módulo y
/// entre los propios imports): una colisión pide renombrar con `as`.
fn clasificar_from_imports(
    m: &Module,
    surfaces: &Surfaces,
    tipos: &HashMap<String, HashSet<String>>,
) -> Result<(NameMap, NameMap), LoadError> {
    // Nombres ya ocupados en el módulo: funciones y tipos propios (para detectar colisiones).
    let mut locales: HashSet<String> = m.program.functions.iter().map(|f| f.name.clone()).collect();
    locales.extend(m.program.structs.iter().map(|s| s.name.clone()));
    locales.extend(m.program.enums.iter().map(|e| e.name.clone()));
    locales.extend(m.program.traits.iter().map(|t| t.name.clone()));

    let (mut values, mut types) = (HashMap::new(), HashMap::new());
    for fi in &m.program.from_imports {
        let from = &fi.module;
        for n in &fi.names {
            let target = clasificar_from_name(&m.source, &m.name, from, n, surfaces, tipos)?;
            let local = n.local().to_string();
            if !locales.insert(local.clone()) {
                return Err(render(&m.source, n.line, n.col, 1, &m.name, &format!(
                    "el nombre '{}' ya está definido o importado en este módulo; usa 'as' para renombrarlo",
                    local
                )));
            }
            match target {
                FromTarget::Funcion(g) => { values.insert(local, g); }
                FromTarget::Tipo(g) => { types.insert(local, g); }
            }
        }
    }
    Ok((values, types))
}

/// Clasifica un único nombre `from`-importado: función `pub`, tipo `pub`, o error (tipo privado /
/// inexistente). El orden prioriza funciones; un módulo no debería tener función y tipo homónimos.
fn clasificar_from_name(
    src: &str,
    module: &str,
    from: &str,
    name: &crate::ast::ImportName,
    surfaces: &Surfaces,
    tipos: &HashMap<String, HashSet<String>>,
) -> Result<FromTarget, LoadError> {
    if let Some(g) = surfaces.get(from).and_then(|s| s.values.get(&name.name)) {
        return Ok(FromTarget::Funcion(g.clone()));
    }
    if let Some(g) = surfaces.get(from).and_then(|s| s.types.get(&name.name)) {
        return Ok(FromTarget::Tipo(g.clone()));
    }
    // No exporta el nombre: ¿existe como tipo privado? (mensaje más preciso que "no existe").
    if tipos.get(from).is_some_and(|s| s.contains(&name.name)) {
        return Err(render(src, name.line, name.col, 1, module, &format!(
            "'{}' es un tipo privado del módulo '{}' (¿falta 'pub'?)", name.name, from
        )));
    }
    Err(render(src, name.line, name.col, 1, module, &format!(
        "el módulo '{}' no exporta '{}' (¿falta 'pub'?)", from, name.name
    )))
}

/// Reescribe, *consciente de ámbitos*, las referencias a funciones de un módulo a sus nombres
/// globales: las propias (`foo` → `modulo::foo`) y las calificadas (`M.f` → `M::f`, con `pub`).
struct Resolver<'a> {
    /// Nombres de nivel superior visibles **sin calificar** en este módulo: las funciones propias
    /// y las traídas por `from M import …` (con su alias). Mapea nombre local → nombre global.
    own: NameMap,
    /// Módulos importados con `import a/b/c [as x];`: mapa **leaf → ruta** (M11.5), para el acceso
    /// calificado `circulo.item`. Los módulos que solo aparecen en `from M import …` **no** entran
    /// aquí (estilo Python: no traen el leaf).
    imports: ImportMap,
    /// Superficie pública por módulo (M11.6a): para resolver `M.f` / `M.Color.Rojo` al global de
    /// destino (que con reexports puede no ser `ns_prefix(M)::f`).
    surfaces: &'a Surfaces,
    /// Pila de ámbitos locales: nombres que tapan a las funciones de nivel superior.
    scopes: Vec<HashSet<String>>,
}

impl<'a> Resolver<'a> {
    /// `from_values` son los `from M import <fn>` ya clasificados (local → `M::fn`); las colisiones
    /// y la validación `pub` las hizo `clasificar_from_imports`, así que aquí solo se vuelcan.
    fn new(
        m: &Module,
        surfaces: &'a Surfaces,
        from_values: &NameMap,
        import_map: &ImportMap,
    ) -> Self {
        let prefix = module_prefix(m);
        let mut own = HashMap::new();
        for f in &m.program.functions {
            own.insert(f.name.clone(), global_fn(&prefix, &f.name));
        }
        // M11.3b: los `from M import a [as b]` de función entran como nombres locales → `M::a`.
        for (local, global) in from_values {
            own.insert(local.clone(), global.clone());
        }
        Resolver { own, imports: import_map.clone(), surfaces, scopes: Vec::new() }
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
            StmtKind::LetTuple { names, value, .. } => {
                self.resolve_expr(value, src, module)?;
                for n in names.iter().flatten() {
                    self.declarar(n);
                }
            }
            StmtKind::For { pat, iter, body } => {
                match iter {
                    ForIter::Range { start, end } => {
                        self.resolve_expr(start, src, module)?;
                        self.resolve_expr(end, src, module)?;
                    }
                    ForIter::In(e) => self.resolve_expr(e, src, module)?,
                }
                self.scopes.push(std::collections::HashSet::new()); // ámbito de la(s) variable(s) del for
                match pat {
                    ForPat::Single(n) => self.declarar(n),
                    ForPat::Tuple(names) => {
                        for n in names.iter().flatten() {
                            self.declarar(n);
                        }
                    }
                }
                self.resolve_block(body, src, module)?;
                self.scopes.pop();
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
            ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => self.resolve_expr(inner, src, module)?,
            ExprKind::Binary { left, right, .. } => {
                self.resolve_expr(left, src, module)?;
                self.resolve_expr(right, src, module)?;
            }
            ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
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
    /// si `name` es una función **o tipo** `pub` de `M`; error si no lo es; `None` si no es acceso a
    /// módulo. El caso de tipo cubre la construcción de enum calificada `M.Color.Rojo` (M11.3c-3):
    /// la cabeza `M.Color` colapsa a `Ident("M::Color")` y el checker la trata como enum normal.
    fn qualified_field(&self, object: &Expr, name: &str, src: &str, module: &str) -> Result<Option<String>, LoadError> {
        let ExprKind::Ident(leaf) = &object.kind else { return Ok(None) };
        if self.declarado_local(leaf) {
            return Ok(None); // una local tapa al módulo
        }
        let Some(ruta) = self.imports.get(leaf) else {
            return Ok(None); // no es un módulo importado (su leaf)
        };
        let surf = self.surfaces.get(ruta);
        let global = surf.and_then(|s| s.values.get(name).or_else(|| s.types.get(name)));
        match global {
            Some(g) => Ok(Some(g.clone())),
            None => Err(render(src, object.line, object.col, 1, module, &format!(
                "el módulo '{}' no exporta '{}' (¿falta 'pub'?)", ruta, name
            ))),
        }
    }
}

/// Reescribe las **referencias de tipo** de un módulo a su nombre global (M11.3c), respetando los
/// **parámetros de tipo** en ámbito. Pasada separada de `Resolver` (que resuelve *valores*): aquí se
/// tocan las posiciones de tipo (anotaciones, campos, payloads, objetivo/trait de `impl`, bounds,
/// `dyn`) y las expresiones que **nombran** tipos (literal de struct, construcción de enum
/// `Tipo.Variante`, patrones de `match`). Un `Ident` *bare* en posición de valor lo deja `Resolver`.
struct TypeRewriter<'a> {
    /// Nombre local de tipo → nombre global (propios del módulo; en -2, también importados).
    types: &'a NameMap,
    /// Módulos importados (leaf → ruta, M11.5) en este módulo, para resolver `c.Tipo` calificado.
    imports: &'a ImportMap,
    /// Superficie pública por módulo (M11.6a), para validar `c.Tipo` calificado y dar su global.
    surfaces: &'a Surfaces,
    /// Pila de conjuntos de parámetros de tipo en ámbito (los `<…>` de fn/impl/struct/enum).
    tparams: Vec<HashSet<String>>,
}

impl<'a> TypeRewriter<'a> {
    fn new(
        types: &'a NameMap,
        imports: &'a ImportMap,
        surfaces: &'a Surfaces,
    ) -> Self {
        TypeRewriter { types, imports, surfaces, tparams: Vec::new() }
    }

    fn en_ambito(&self, name: &str) -> bool {
        self.tparams.iter().any(|s| s.contains(name))
    }

    /// Reescribe un nombre de tipo/trait a su nombre global. Tres casos:
    /// - **Calificado** `c.Tipo` (lleva un `.`, M11.3c-3): si `c` es el leaf de un módulo importado
    ///   (M11.5) y `Tipo` es `pub`, → `ruta::Tipo`; si no resuelve, se deja con el `.` (el checker lo
    ///   rechaza → encapsulación).
    /// - **Parámetro de tipo** (`T`): se deja intacto.
    /// - **Tipo propio o importado** del módulo: → su nombre global (mapa `types`).
    fn rewrite_name(&self, name: &mut String) {
        if let Some((leaf, ty)) = name.split_once('.') {
            if let Some(ruta) = self.imports.get(leaf)
                && let Some(g) = self.surfaces.get(ruta).and_then(|s| s.types.get(ty))
            {
                *name = g.clone();
            }
            return; // calificado: resuelto o dejado para que el checker lo rechace
        }
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
            Type::Tuple(ts) => {
                for t in ts {
                    self.rewrite_type(t);
                }
            }
            Type::Map(k, v) => {
                self.rewrite_type(k);
                self.rewrite_type(v);
            }
            Type::Fn(params, ret) => {
                for p in params {
                    self.rewrite_type(p);
                }
                self.rewrite_type(ret);
            }
            Type::Dyn(traits) => {
                for tr in traits.iter_mut() {
                    self.rewrite_name(tr);
                }
            }
            _ => {} // Int/Float/Bool/String/Unit/Var/SelfType
        }
    }

    fn rewrite_program(&mut self, program: &mut Program) {
        for s in &mut program.structs {
            self.tparams.push(s.type_params.iter().cloned().collect());
            for (_, ty) in &mut s.fields {
                self.rewrite_type(ty);
            }
            for (_, tr) in &mut s.bounds {
                self.rewrite_name(tr); // M9.4: el trait del bound se namespaca como cualquier tipo
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
            for (_, tr) in &mut e.bounds {
                self.rewrite_name(tr); // M9.4
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
            // M28.2: los argumentos de tipo del trait (`impl From<Otro> for E`) son posiciones de
            // tipo → namespaciarlos como cualquier otra referencia de tipo.
            for arg in &mut imp.trait_args {
                self.rewrite_type(arg);
            }
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
            StmtKind::LetTuple { value, .. } => {
                self.rewrite_expr(value);
            }
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { self.rewrite_expr(start); self.rewrite_expr(end); }
                    ForIter::In(e) => self.rewrite_expr(e),
                }
                self.rewrite_block(body);
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
            ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => self.rewrite_expr(inner),
            ExprKind::Binary { left, right, .. } => {
                self.rewrite_expr(left);
                self.rewrite_expr(right);
            }
            ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
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
