//! Type checker (análisis semántico) de raylang.
//!
//! Tercera fase del pipeline (DESIGN.md §2, reglas en §8). El parser garantiza que
//! el programa es sintácticamente válido; el checker garantiza que *tiene
//! sentido*: que no sumas un `bool` con un `string`, que no usas variables sin
//! declarar, que `fib` realmente devuelve `int`, etc. Un programa que pasa el
//! checker no puede fallar por un error de tipos en tiempo de ejecución.
//!
//! ## Dos pasadas
//!
//! 1. **Pre-pasada**: registra la firma de cada función (parámetros y retorno).
//!    Así una función puede llamar a otra declarada más abajo, y a sí misma
//!    (recursión), sin que el orden importe.
//! 2. **Verificación**: recorre el cuerpo de cada función comprobando las reglas.
//!
//! ## Ámbitos (scopes)
//!
//! Las variables viven en una **pila de ámbitos**. Cada bloque empuja un ámbito y
//! lo retira al salir. Buscar un nombre recorre la pila de dentro hacia afuera, lo
//! que da *shadowing* (una variable interior tapa una exterior) de forma natural.
//!
//! ## Una nota sobre el flujo
//!
//! Como raylang es orientado a expresiones, el cuerpo de una función `-> int` debe
//! *producir* un `int` (retorno implícito). Pero también vale salir antes con
//! `return`. Para aceptar `fn f() -> int { return 5; }` (sin expresión final)
//! hacemos un pequeño análisis de **divergencia**: si todos los caminos del bloque
//! terminan en `return`, el bloque "diverge" y no necesita valor final.

use std::collections::{HashMap, HashSet};

use crate::ast::*;

/// Error de tipos con ubicación. `len` (M33a) es la extensión del error en
/// caracteres; por ahora siempre `1` (subrayar la **expresión** completa exige
/// que los nodos del AST lleven posición de fin → M33a-2). No entra en el
/// `Display`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error de tipos en {}:{}: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for TypeError {}

/// Firma de una función: parámetros de tipo (genéricos), tipos de parámetros y tipo
/// de retorno. `type_params` vacío = no genérica.
struct FnSig {
    type_params: Vec<String>,
    params: Vec<Type>,
    ret: Type,
    /// Bounds de los parámetros de tipo (M9.2): pares `(parámetro, trait)`. Vacío = sin
    /// bounds. Para verificar la llamada (los diccionarios a pasar) y para reenviarlos.
    bounds: Vec<(String, String)>,
}

/// Datos de un **impl genérico** (M9.2b) —`impl<T: B> Trait for Caja<T>`— que `dict_for`
/// necesita para sintetizar el diccionario anidado: los parámetros de tipo, el tipo objetivo
/// (`Caja<T>`, con los `T` como `Var`) y los bounds del impl.
#[derive(Clone)]
struct GenImpl {
    /// Parámetros de tipo del impl (no usados directamente hoy, pero documentan su forma).
    #[allow(dead_code)]
    type_params: Vec<String>,
    /// Tipo objetivo `Caja<T>` (con los `T` como `Var`): se casa con el concreto para σ_impl.
    target: Type,
    /// Bounds del impl: deciden si el diccionario es plano o un closure anidado.
    bounds: Vec<(String, String)>,
}

/// Información de una variable en un ámbito.
struct VarInfo {
    ty: Type,
    mutable: bool,
    /// Posición de su declaración (M10.2b: ir-a-definición). `(línea, col)` 1-basado del
    /// `let`/`var`, del parámetro o del brazo de `match` que la liga.
    def: (usize, usize),
}

/// Punto de entrada de la fase: verifica un programa completo.
///
/// Recibe el programa por **referencia mutable** porque, antes de verificar,
/// **reescribe** los `Field`/`Call` que en realidad son construcción de variantes de
/// enum (`Enum.Variante(args)`) en nodos `EnumLit` explícitos (M5). Esa resolución
/// es parte del front-end compartido: el intérprete y la VM reciben el AST ya
/// resuelto, sin duplicar la regla.
/// Tope de errores acumulados por `check_all` (M33c).
const MAX_ERRORES: usize = 20;

/// Variante acumuladora de `check` (M33c): devuelve TODOS los errores de tipos (hasta
/// `MAX_ERRORES`), con granularidad por función — las pasadas tempranas siguen fail-fast.
/// **Solo diagnóstico** (LSP/CLI): omite el lowering, porque con errores no se ejecuta
/// nada. El primer error es idéntico al de `check` (mismo recorrido hasta ahí).
pub fn check_all(program: &mut Program) -> Vec<TypeError> {
    if let Err(e) = prepare_program(program) {
        return vec![e];
    }
    let mut checker = Checker::new();
    checker.acumular = true;
    match checker.check_program(program) {
        Err(e) => vec![e], // pasada temprana (fail-fast): un solo error
        Ok(()) => checker.errores,
    }
}

pub fn check(program: &mut Program) -> Result<(), TypeError> {
    // Pasos 0–1: inyectar el prelude, generar derivaciones, bajar los métodos de impl y
    // resolver la construcción de enums (compartido con `semantic_index`).
    prepare_program(program)?;
    // Pasos 2–3: pre-pasada y verificación.
    let mut checker = Checker::new();
    checker.check_program(program)?;
    // Paso 4 (M7.1 + M9): bajar las llamadas por punto (`recv.f(args)`) a llamadas
    // ordinarias (`f(recv, args)`); incluye UFCS, métodos de trait (M9.1) y métodos sobre
    // un tipo acotado, que bajan a una llamada al parámetro-diccionario (M9.2).
    // Paso 3.3 (M28.3b): envolver en un `as u{w}` los literales enteros que el contexto coercionó a
    // un entero sin signo (`let x: u8 = 5`). Antes que el resto: el resultado es un `Cast` corriente.
    lower_uint_literals(program, &checker.uint_literal_sites);
    // Paso 3.4 (M28.2): bajar los `?` que convierten el error — `expr?` (con `impl From<E1> for E2`)
    // a un `match` que aplica la conversión en la rama de error. Front-end puro: el `?` sin conversión
    // sigue siendo nativo; solo los sitios registrados se reescriben. Antes que el resto de bajadas,
    // para que estas recorran el operando (ahora escrutinio del match) y la llamada de conversión.
    lower_try_conversions(program, &checker.try_conversions);
    // Paso 3.5 (M28.1): bajar la sobrecarga de operadores — `a op b` (con un tipo de usuario que
    // implementa el trait del operador) a `a.metodo(b)`, y `-x` a `x.neg()`. Se hace antes que el
    // resto de bajadas: el resultado es una llamada ordinaria a la función manglada del método.
    lower_operators(program, &checker.op_sites);
    lower_ufcs(program, &checker.ufcs_sites);
    // Paso 5 (M9.2): añadir los parámetros-diccionario a las funciones con bounds y los
    // argumentos correspondientes en cada sitio de llamada. Diccionarios = valores función;
    // el runtime no cambia.
    append_dict_params(program);
    lower_dict_calls(program, &checker.dict_calls);
    // Paso 6 (M9.3b): bajar los trait objects — coerciones concreto→objeto a la
    // construcción del struct sintetizado, y los despachos dinámicos a `(r.m)(r.data, ...)`.
    lower_dyn(program, &checker.dyn_coercions, &checker.dyn_dispatch, &checker.dyn_upcasts);
    // Paso 7 (M9.2b): renumerar los `id` de los fn-exprs. El lowering pudo **inyectar**
    // closures sintéticos (diccionarios anidados) con `id` provisional; el intérprete y la VM
    // exigen ids **densos** (`collect_fn_exprs`). Esta pasada final los reasigna en orden.
    renumber_fn_exprs(program);
    Ok(())
}

/// Recolecta el **índice semántico** (M10.2b): corre el front-end hasta `check_program` con el
/// `Checker` en modo `gather` (que apunta tipos y posiciones de declaración por identificador) y
/// devuelve el índice. **Tolera errores**: un programa a medio escribir devuelve la info parcial
/// recolectada hasta el fallo (útil para hover/definición mientras se teclea). No corre los
/// *lowerings* (mutarían posiciones y no hacen falta). Lo usa el LSP (`src/lsp.rs`).
pub fn semantic_index(program: &mut Program) -> SemanticIndex {
    if prepare_program(program).is_err() {
        return SemanticIndex::default();
    }
    let mut checker = Checker::new();
    checker.gather = true;
    let _ = checker.check_program(program); // best-effort: el índice parcial igual sirve
    checker.index
}

/// Pasos 0–1 del front-end, compartidos por `check` y `semantic_index`: inyección del prelude,
/// derivaciones de `@derive`, bajada de los métodos de impl a funciones mangladas y resolución
/// de la construcción de enums. Muta `program`.
fn prepare_program(program: &mut Program) -> Result<(), TypeError> {
    // Paso 0: inyectar el prelude (Option/Result) si no está ya. Sus enums se
    // anteponen, así forman parte del AST que también ven el intérprete y la VM.
    if !program.enums.iter().any(|e| e.name == "Option" || e.name == "Result") {
        let mut all = crate::prelude::enums();
        all.append(&mut program.enums);
        program.enums = all;
    }
    // Paso 0b (M7.3): inyectar las funciones del prelude (map/filter/fold). Se saltan
    // las que el usuario ya definió con ese nombre —permite override y hace la inyección
    // idempotente si se vuelve a verificar—. Como los enums, quedan en el AST que
    // también compilan/ejecutan el intérprete y la VM.
    let definidas: HashSet<String> = program.functions.iter().map(|f| f.name.clone()).collect();
    let mut prelude_fns: Vec<Function> = crate::prelude::functions()
        .into_iter()
        .filter(|f| !definidas.contains(&f.name))
        .collect();
    if !prelude_fns.is_empty() {
        prelude_fns.append(&mut program.functions);
        program.functions = prelude_fns;
    }
    // Paso 0b2 (M10.1, L2): inyectar los traits del prelude (`Eq`, `Show`) que el usuario no
    // haya redefinido (homónimos), uno a uno. Necesario antes de generar las derivaciones.
    let traits_usuario: HashSet<String> = program.traits.iter().map(|t| t.name.clone()).collect();
    let mut prelude_traits: Vec<TraitDef> = crate::prelude::traits()
        .into_iter()
        .filter(|t| !traits_usuario.contains(&t.name))
        .collect();
    if !prelude_traits.is_empty() {
        prelude_traits.append(&mut program.traits);
        program.traits = prelude_traits;
    }
    // Paso 0b4 (M11.7d): inyectar los `impl` del prelude (`Ord` para int/float/string/char).
    // Idempotente: se salta cualquier `(trait, tipo objetivo)` que ya exista (sea del usuario o de
    // una verificación previa). De ahí los procesa el paso 0c como cualquier impl.
    let impls_existentes: HashSet<(String, Option<String>)> = program.impls.iter()
        .map(|i| (i.trait_name.clone(), type_key_of(&i.target)))
        .collect();
    let prelude_impls: Vec<crate::ast::ImplBlock> = crate::prelude::impls()
        .into_iter()
        .filter(|i| !impls_existentes.contains(&(i.trait_name.clone(), type_key_of(&i.target))))
        .collect();
    program.impls.extend(prelude_impls);
    // Paso 0b3 (M10.1, L2): generar los `impl` de `@derive(Eq)` / `@derive(Show)` sobre
    // struct/enum. Genera el AST de un `impl Trait for T { ... }` y lo añade a `program.impls`;
    // de ahí en adelante lo procesa M9 (la bajada de impls del paso 0c). Antes del paso 0c.
    generate_derives(program)?;

    // Paso 0c (M9.1 + M9.3a): bajar los métodos de cada `impl` a funciones ordinarias con
    // nombre manglado (`Tipo#metodo`) y `self` de tipo concreto. Es el truco que hace a
    // M9.1 *front-end puro*: un método es una función con un primer parámetro `self`, así
    // que el resto del pipeline (registro de firmas, chequeo de cuerpos, lowering de UFCS,
    // intérprete y VM) los procesa sin código especial. La **validación** (cobertura y
    // coincidencia de firmas) y la tabla de resolución se construyen luego, ya con los
    // tipos registrados (`check_program`).
    let trait_sigs: HashMap<String, Vec<MethodSig>> = program.traits.iter()
        .map(|t| (t.name.clone(), t.methods.clone()))
        .collect();
    // Contador para renumerar las posiciones de cada cuerpo por defecto clonado (M9.3a):
    // cada clon recibe posiciones únicas para que las bajadas por posición no colisionen.
    let mut fresh_pos = 0usize;
    for imp in &program.impls {
        let key = match type_key_of(&imp.target) {
            Some(k) => k,
            None => continue, // objetivo inválido: el error se da en la validación
        };
        // Métodos provistos por el impl. M9.2b: un impl genérico (`impl<T: B> Trait for
        // Caja<T>`) baja sus métodos a funciones **genéricas acotadas** (heredan los
        // `type_params`/`bounds` del impl); de ahí, `append_dict_params` y
        // `resolve_bound_method` los tratan como cualquier función con bounds. Para un impl
        // concreto (M9.1) ambos son vacíos → función ordinaria, como antes.
        for m in &imp.methods {
            let params = m.params.iter()
                .map(|p| Param { ty: subst_self(&p.ty, &imp.target), ..p.clone() })
                .collect();
            // M28.2: un método de un impl `From<S>` se inyecta con nombre manglado **por origen**
            // (`E#from#string`) para no colisionar con otros `impl From<...> for E`. El resto
            // (impls de M9) usa el manglado ordinario `Tipo#metodo`.
            let name = if is_typed_trait_impl(imp) && m.name == "desde" {
                let src_key = imp.trait_args.first().and_then(type_key_of).unwrap_or_default();
                mangle_from(&key, &src_key)
            } else {
                mangle(&key, &m.name)
            };
            program.functions.push(Function {
                annotations: Vec::new(),
                is_pub: false,
                name,
                type_params: imp.type_params.clone(),
                bounds: imp.bounds.clone(),
                params,
                return_type: subst_self(&m.return_type, &imp.target),
                body: m.body.clone(),
                line: m.line,
                col: m.col,
            });
        }
        // M9.3a: métodos por defecto no redefinidos → se sintetizan desde el cuerpo del
        // trait (con `Self` = el tipo destino). El impl los hereda como funciones propias.
        let provistos: HashSet<&str> = imp.methods.iter().map(|m| m.name.as_str()).collect();
        for tm in trait_sigs.get(&imp.trait_name).into_iter().flatten() {
            let Some(body) = &tm.default_body else { continue };
            if provistos.contains(tm.name.as_str()) {
                continue; // el impl lo redefine: gana el del impl
            }
            let params = tm.params.iter()
                .map(|p| Param { ty: subst_self(&p.ty, &imp.target), ..p.clone() })
                .collect();
            // Clonar el cuerpo del defecto y renumerar sus posiciones (únicas por impl).
            let mut body = body.clone();
            freshen_positions(&mut body, &mut fresh_pos);
            program.functions.push(Function {
                annotations: Vec::new(),
                is_pub: false,
                name: mangle(&key, &tm.name),
                type_params: imp.type_params.clone(),
                bounds: imp.bounds.clone(),
                params,
                return_type: subst_self(&tm.return_type, &imp.target),
                body,
                line: tm.line,
                col: tm.col,
            });
        }
    }

    // Paso 1: resolver la construcción de enums sobre el AST.
    let enum_names: HashSet<String> = program.enums.iter().map(|e| e.name.clone()).collect();
    for f in &mut program.functions {
        resolve_block(&mut f.body, &enum_names);
    }
    Ok(())
}

/// Índice semántico del programa (M10.2b): lo que el LSP necesita para hover e
/// ir-a-definición. Se recolecta durante `check_program` (posiciones de la fuente original,
/// antes de cualquier *lowering*). Granularidad: por identificador resuelto.
#[derive(Default)]
pub struct SemanticIndex {
    /// Hover: por cada uso de un identificador, su rango y el texto a mostrar (su tipo).
    pub hovers: Vec<HoverEntry>,
    /// Ir-a-definición (M10.2b-2): por cada uso, el rango del uso y la posición de su
    /// declaración.
    pub defs: Vec<DefEntry>,
}

/// Una entrada de hover: el identificador en `(line, col)` (1-basado) de largo `len` muestra
/// `text` (p. ej. `x: int`).
pub struct HoverEntry {
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub text: String,
}

/// Una entrada de ir-a-definición (M10.2b-2): el uso en `(line, col)` de largo `len` se declara
/// en `(def_line, def_col)`.
pub struct DefEntry {
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub def_line: usize,
    pub def_col: usize,
}

struct Checker {
    /// Firmas de todas las funciones (llenada en la pre-pasada).
    functions: HashMap<String, FnSig>,
    /// Constantes de nivel superior (M27.5): nombre → tipo. Resueltas como `Ident` globales.
    consts: HashMap<String, Type>,
    /// Definiciones de struct: nombre → campos (en orden). Pre-pasada.
    structs: HashMap<String, Vec<(String, Type)>>,
    /// Definiciones de enum: nombre → variantes (nombre, payload), en orden.
    /// Pre-pasada (M5). Los payloads pueden contener `Type::Var` (M6).
    enums: HashMap<String, Vec<(String, Vec<Type>)>>,
    /// Solo los nombres de enum, para `resolve_type` (reclasificar `Struct`→`Enum`)
    /// y para validar tipos. Se llena antes que cualquier resolución de tipos.
    enum_names: HashSet<String>,
    /// Parámetros de tipo de cada enum/struct genérico (M6): nombre → `[T, U, ...]`.
    /// Dan la aridad (para validar `Caja<int>`) y los nombres (para sustituir).
    enum_tparams: HashMap<String, Vec<String>>,
    struct_tparams: HashMap<String, Vec<String>>,
    /// Bounds de los parámetros de tipo de cada struct/enum (M9.4): nombre → `[(T, Trait), ...]`.
    /// Se verifican en la construcción del valor (no hay runtime).
    struct_bounds: HashMap<String, Vec<(String, String)>>,
    enum_bounds: HashMap<String, Vec<(String, String)>>,
    /// Spans de expresiones del parser (M33a-2): inicio → fin. Los consulta `err()` para
    /// subrayar la expresión completa; una posición ausente degrada a extensión 1.
    expr_spans: HashMap<(usize, usize), (usize, usize)>,
    /// Modo acumulador (M33c, `check_all`): el bucle de cuerpos guarda los errores en
    /// `errores` y sigue con la siguiente función, en vez de cortar en el primero. Las
    /// pasadas tempranas (tipos/firmas) siguen fail-fast: sus tablas a medias
    /// envenenarían todo lo demás. Apagado (fail-fast) en `check`, el camino de ejecución.
    acumular: bool,
    errores: Vec<TypeError>,
    /// Pila de ámbitos de variables. El último es el más interno.
    scopes: Vec<HashMap<String, VarInfo>>,
    /// Tipo de retorno de la función que estamos verificando ahora mismo, para
    /// validar las sentencias `return`.
    current_return: Type,
    /// Parámetros de tipo en ámbito ahora mismo: los `<T, U>` de la función que se
    /// registra o verifica (M6). `resolve_type` los reclasifica de `Struct(name)` a
    /// `Var(name)`, y `ensure_type` los acepta como tipos válidos.
    type_params: HashSet<String>,
    /// Sitios de **llamada por punto** detectados durante la verificación que hay que
    /// bajar a una llamada ordinaria: UFCS de función libre (M7.1) y métodos de trait
    /// (M9.1). Se identifican por `(línea, columna, nombre)` del nodo `Call`: la posición
    /// sola no basta porque el `Call` y su receptor comparten posición (el parser arranca
    /// el `Call` en el callee), así que el nombre del método desambigua en una cadena
    /// `a.f().g()`. El **valor** es el nombre de la función destino: para UFCS de función
    /// libre es el mismo nombre; para un método de trait, el nombre **manglado**
    /// (`Tipo#metodo`). Tras verificar, `lower_ufcs` reescribe `recv.f(args)` a
    /// `destino(recv, args)`, de modo que el intérprete y la VM solo ven llamadas.
    ufcs_sites: HashMap<(usize, usize, String), String>,
    /// M28.1: sitios de sobrecarga de operadores. Clave `(línea, col, "Add"/"Sub"/…)` → función manglada
    /// del método del operador (`Vec2#add`). El lowering reescribe el `Binary`/`Unary` a una llamada.
    op_sites: HashMap<(usize, usize, String), String>,
    /// Traits declarados (M9): nombre → firmas de sus métodos (con `self`/`Self` aún sin
    /// sustituir). Llenado en la pre-pasada, antes de validar los impls.
    traits: HashMap<String, Vec<MethodSig>>,
    /// M28.2: parámetros de tipo de cada trait (`trait From<S>` → `["S"]`). Da la aridad para
    /// validar los argumentos de tipo de un `impl From<string> for E`.
    trait_tparams: HashMap<String, Vec<String>>,
    /// M28.2: conversiones `From`. `(clave_origen, clave_destino)` → función manglada que
    /// convierte (`MiError#from#string`). La consulta `check_try` para el operador `?`: sobre
    /// `Result<_, E1>` en una función que devuelve `Result<_, E2>`, si hay `impl From<E1> for E2`
    /// el `?` convierte el error automáticamente.
    from_impls: HashMap<(String, String), String>,
    /// M28.2: sitios de `?` que requieren conversión de error. `(línea, col)` → función manglada
    /// de `From`. `lower_try_conversions` reescribe ese `Try` a un `match` que convierte.
    try_conversions: HashMap<(usize, usize), String>,
    /// M28.3b: literales enteros que se coercionan a un entero sin signo por el contexto (tipo
    /// esperado, u operando de un operador). `(línea, col)` → ancho. `lower_uint_literals` envuelve
    /// ese literal en un `Cast` al `u8`/`u32`/`u64` correspondiente (el runtime produce el UInt).
    uint_literal_sites: HashMap<(usize, usize), u8>,
    /// Tabla de resolución de métodos de trait (M9): `(clave_de_tipo, método)` → nombre
    /// manglado de la función que lo implementa. La clave de tipo es el nombre del
    /// struct/enum o el primitivo (ver `type_key`). Un mismo `(tipo, método)` solo puede
    /// tener un impl (si no, ambigüedad → error).
    methods: HashMap<(String, String), String>,
    /// Para cada función manglada de impl (M9): su tipo implementador. Permite poner
    /// `current_self` en ámbito al verificar el cuerpo (para anotaciones `Self` internas).
    impl_fn_self: HashMap<String, Type>,
    /// Qué `(clave_de_tipo, trait)` tiene un impl (M9): para verificar bounds —que un tipo
    /// concreto realmente implemente el trait pedido— al elegir su diccionario (M9.2).
    impl_traits: HashSet<(String, String)>,
    /// Impls **genéricos** (M9.2b), por `(clave_de_tipo, trait)`: sus parámetros de tipo, el
    /// tipo objetivo (`Caja<T>`) y sus bounds. Lo usa `dict_for` para distinguir un impl
    /// genérico de uno concreto y, si está acotado, sintetizar su **diccionario anidado**.
    generic_impls: HashMap<(String, String), GenImpl>,
    /// El tipo `Self` en ámbito ahora mismo: el tipo implementador del `impl` cuyo método
    /// se está resolviendo/verificando (M9). `None` fuera de un impl.
    current_self: Option<Type>,
    /// Bounds de la función que se está verificando ahora (M9.2): pares `(parámetro,
    /// trait)`. Sirven para resolver `x.metodo()` con `x: T` acotado y para reenviar
    /// diccionarios al llamar a otro genérico acotado con un `T` rígido.
    current_fn_bounds: Vec<(String, String)>,
    /// Diccionarios a añadir en cada **sitio de llamada** a una función con bounds (M9.2):
    /// `(línea, col, nombre)` → nombres de los valores función (diccionarios) a pasar como
    /// argumentos extra, en orden. `lower_dict_calls` los añade tras verificar.
    dict_calls: DictSites,
    /// Coerciones concreto→`dyn Trait` (M9.3b): `(línea, col)` de la expresión → `(trait,
    /// expresiones-vtable)`. `lower_dyn` envuelve esa expresión en el struct sintetizado del trait
    /// object, usando las expresiones-vtable como sus métodos. Cada método se resuelve con `dict_for`
    /// (M9.4): el método manglado plano para un impl concreto/genérico sin bounds, o un **closure
    /// anidado** para un impl genérico acotado (`Caja<int>`) → habilita `dyn` sobre impls genéricos.
    dyn_coercions: HashMap<(usize, usize), (Vec<String>, Vec<Expr>)>,
    /// Sitios de **despacho dinámico** (M9.3b): `(línea, col, método)` de un `obj.m(args)`
    /// con `obj: dyn Trait`. `lower_dyn` los baja al bloque `{ let r = obj; (r.m)(r.data, ...) }`.
    dyn_dispatch: HashSet<(usize, usize, String)>,
    /// Sitios de **upcasting** (M9.5b): `(línea, col)` de un valor `dyn S1` coercionado a `dyn S2`
    /// con S2 ⊆ S1 → el conjunto destino S2. `lower_dyn` lo baja a reconstruir el struct menor
    /// proyectando los campos del mayor.
    dyn_upcasts: HashMap<(usize, usize), Vec<String>>,
    /// M10.2b: si está activo, el checker **recolecta** el índice semántico (tipos y posiciones
    /// de declaración por identificador) en `index`. Solo lo enciende `semantic_index`; en una
    /// verificación normal queda en `false` (coste cero).
    gather: bool,
    /// El índice semántico recolectado (M10.2b). Vacío si `gather` es `false`.
    index: SemanticIndex,
    /// Posición de declaración de cada función de nivel superior (M10.2b: ir-a-definición).
    fn_defs: HashMap<String, (usize, usize)>,
    /// Posición de declaración de cada tipo (struct/enum/trait) — hover/def de tipos (M10.2f).
    type_defs: HashMap<String, (usize, usize)>,
    /// Alias UFCS de funciones `from`-importadas (nombre local → global), que deja el loader. Permiten
    /// que `recv.f(...)` resuelva una función importada como *fallback* (tras campo/método). Vacío sin
    /// imports.
    ufcs_aliases: HashMap<String, String>,
}

impl Checker {
    fn new() -> Self {
        Checker {
            functions: HashMap::new(),
            consts: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            enum_names: HashSet::new(),
            enum_tparams: HashMap::new(),
            struct_tparams: HashMap::new(),
            struct_bounds: HashMap::new(),
            enum_bounds: HashMap::new(),
            scopes: Vec::new(),
            expr_spans: HashMap::new(),
            acumular: false,
            errores: Vec::new(),
            current_return: Type::Unit,
            type_params: HashSet::new(),
            ufcs_sites: HashMap::new(),
            op_sites: HashMap::new(),
            trait_tparams: HashMap::new(),
            from_impls: HashMap::new(),
            try_conversions: HashMap::new(),
            uint_literal_sites: HashMap::new(),
            traits: HashMap::new(),
            methods: HashMap::new(),
            impl_fn_self: HashMap::new(),
            impl_traits: HashSet::new(),
            generic_impls: HashMap::new(),
            current_self: None,
            current_fn_bounds: Vec::new(),
            dict_calls: HashMap::new(),
            dyn_coercions: HashMap::new(),
            dyn_dispatch: HashSet::new(),
            dyn_upcasts: HashMap::new(),
            type_defs: HashMap::new(),
            gather: false,
            index: SemanticIndex::default(),
            fn_defs: HashMap::new(),
            ufcs_aliases: HashMap::new(),
        }
    }

    fn check_program(&mut self, program: &Program) -> Result<(), TypeError> {
        // El loader deja los alias UFCS de funciones importadas (M11.3b + UFCS cross-module). Se copian
        // aquí, una vez, para usarlos como fallback al resolver `recv.f(...)`.
        if self.ufcs_aliases.is_empty() && !program.ufcs_aliases.is_empty() {
            self.ufcs_aliases = program.ufcs_aliases.clone();
        }
        // M33a-2: los spans de expresiones que dejó el parser, para que `err()` subraye la
        // expresión completa. (Las posiciones del prelude inyectado no están —se parsea
        // aparte— y degradan a extensión 1; un choque hipotético con ellas solo afectaría
        // al ancho del subrayado, nunca al veredicto.)
        if self.expr_spans.is_empty() && !program.expr_spans.is_empty() {
            self.expr_spans = program.expr_spans.clone();
        }
        // M27.5: registrar y validar las constantes de nivel superior. El valor debe ser un literal (o
        // un literal negado) del tipo declarado. Un duplicado o un valor no-literal es error.
        for c in &program.consts {
            self.ensure_type(&c.ty, c.line, c.col)?;
            let declared = self.resolve_type(&c.ty);
            if !is_const_literal(&c.value) {
                return Err(self.err(c.value.line, c.value.col,
                    format!("el valor de la constante '{}' debe ser un literal", c.name)));
            }
            let vt = self.check_expr(&c.value)?;
            if vt != declared {
                return Err(self.err(c.value.line, c.value.col, format!(
                    "la constante '{}' se declara como {} pero su valor es {}", c.name, declared, vt)));
            }
            if self.consts.insert(c.name.clone(), declared).is_some() {
                return Err(self.err(c.line, c.col, format!("constante '{}' declarada dos veces", c.name)));
            }
        }
        // M10.2f: posición de declaración de cada tipo (struct/enum/trait), para hover/ir-a-definición
        // de nombres de tipo. Barato; solo se consulta si `gather`.
        if self.gather {
            for s in &program.structs {
                self.type_defs.entry(s.name.clone()).or_insert((s.line, s.col));
            }
            for e in &program.enums {
                self.type_defs.entry(e.name.clone()).or_insert((e.line, e.col));
            }
            for t in &program.traits {
                self.type_defs.entry(t.name.clone()).or_insert((t.line, t.col));
            }
        }
        // --- Pre-pasada: nombres de los tipos nominales (enum y struct) ---
        // Los nombres de enum se necesitan antes de normalizar cualquier tipo, para
        // reclasificar `Struct(nombre)`→`Enum(nombre)` (`resolve_type`).
        for e in &program.enums {
            if !self.enum_names.insert(e.name.clone()) {
                return Err(self.err(e.line, e.col, format!("enum '{}' declarado dos veces", e.name)));
            }
        }
        for s in &program.structs {
            if self.enum_names.contains(&s.name) {
                return Err(self.err(s.line, s.col, format!("'{}' ya es un enum; no puede ser también un struct", s.name)));
            }
        }

        // --- Pre-pasada: parámetros de tipo de cada enum/struct (aridad conocida
        // antes de resolver/validar cualquier tipo que los referencie) ---
        for e in &program.enums {
            self.check_unique_tparams(&e.type_params, &e.name, e.line, e.col)?;
            self.enum_tparams.insert(e.name.clone(), e.type_params.clone());
            self.enum_bounds.insert(e.name.clone(), e.bounds.clone());
        }
        for s in &program.structs {
            self.check_unique_tparams(&s.type_params, &s.name, s.line, s.col)?;
            self.struct_tparams.insert(s.name.clone(), s.type_params.clone());
            self.struct_bounds.insert(s.name.clone(), s.bounds.clone());
        }

        // --- Pre-pasada: registrar enums (payload normalizado con T en ámbito) ---
        for e in &program.enums {
            self.type_params = e.type_params.iter().cloned().collect();
            let mut seen = HashSet::new();
            let mut variants = Vec::new();
            for v in &e.variants {
                if !seen.insert(v.name.clone()) {
                    return Err(self.err(v.line, v.col, format!("variante '{}' repetida en el enum '{}'", v.name, e.name)));
                }
                let payload: Vec<Type> = v.payload.iter().map(|t| self.resolve_type(t)).collect();
                variants.push((v.name.clone(), payload));
            }
            self.enums.insert(e.name.clone(), variants);
        }

        // --- Pre-pasada: registrar structs (campos con T en ámbito) ---
        for s in &program.structs {
            if self.structs.contains_key(&s.name) {
                return Err(self.err(s.line, s.col, format!("struct '{}' declarado dos veces", s.name)));
            }
            self.type_params = s.type_params.iter().cloned().collect();
            let fields: Vec<(String, Type)> =
                s.fields.iter().map(|(n, t)| (n.clone(), self.resolve_type(t))).collect();
            self.structs.insert(s.name.clone(), fields);
        }

        // --- Validar los tipos referenciados (ahora que todos están registrados con
        // su aridad), con los parámetros de cada definición en ámbito ---
        for e in &program.enums {
            self.type_params = e.type_params.iter().cloned().collect();
            let variants = self.enums.get(&e.name).unwrap_or_else(|| crate::ice!("enum '{}' recién registrado no está en la tabla", e.name)).clone();
            for (_, payload) in &variants {
                for t in payload {
                    self.ensure_type(t, e.line, e.col)?;
                }
            }
        }
        for s in &program.structs {
            self.type_params = s.type_params.iter().cloned().collect();
            let fields = self.structs.get(&s.name).unwrap_or_else(|| crate::ice!("struct '{}' recién registrado no está en la tabla", s.name)).clone();
            for (_, ty) in &fields {
                self.ensure_type(ty, s.line, s.col)?;
            }
        }
        self.type_params.clear();

        // --- Pre-pasada (M9): registrar traits y validar impls ---
        // Debe ir tras registrar los tipos (para validar el objetivo del impl y comparar
        // firmas con `resolve_type`) y ANTES de registrar las firmas de funciones (los
        // métodos manglados ya están en `program.functions`) y de verificar cuerpos (que
        // pueden llamar métodos de trait vía la tabla `methods`).
        self.register_traits_impls(program)?;

        // M9.4: validar los bounds de los parámetros de tipo de struct/enum (ya conocidos los
        // traits). Cada bound debe acotar un parámetro real con un trait existente.
        for s in &program.structs {
            self.check_type_def_bounds(&s.name, &s.type_params, &s.bounds, "struct", s.line, s.col)?;
        }
        for e in &program.enums {
            self.check_type_def_bounds(&e.name, &e.type_params, &e.bounds, "enum", e.line, e.col)?;
        }

        // --- Pre-pasada: registrar firmas (con tipos normalizados) ---
        for f in &program.functions {
            if self.functions.contains_key(&f.name) {
                return Err(self.err(f.line, f.col, format!("función '{}' declarada dos veces", f.name)));
            }
            // Los parámetros de tipo de ESTA función están en ámbito al resolver su
            // firma: así `x: T` se normaliza a `Var("T")` en vez de `Struct("T")`.
            self.type_params = f.type_params.iter().cloned().collect();
            // M9.2: validar los bounds —cada uno acota un parámetro de tipo real con un
            // trait existente—. La firma guarda los bounds para verificar las llamadas.
            self.check_bounds(f)?;
            let sig = FnSig {
                type_params: f.type_params.clone(),
                params: f.params.iter().map(|p| self.resolve_type(&p.ty)).collect(),
                ret: self.resolve_type(&f.return_type),
                bounds: f.bounds.clone(),
            };
            self.functions.insert(f.name.clone(), sig);
            // M10.2b: posición de declaración (para ir-a-definición). Solo al recolectar.
            if self.gather {
                self.fn_defs.insert(f.name.clone(), (f.line, f.col));
            }
        }
        self.type_params.clear();

        // 'main' es obligatoria (DESIGN.md §11): sin parámetros y con retorno int o unit.
        match self.functions.get("main") {
            None => return Err(self.err(1, 1, "falta la función de entrada 'main'".into())),
            Some(sig) => {
                if !sig.params.is_empty() {
                    return Err(self.err(1, 1, "'main' no debe recibir parámetros".into()));
                }
                if sig.ret != Type::Int && sig.ret != Type::Unit {
                    return Err(self.err(1, 1, format!("'main' debe devolver int o unit, no {}", sig.ret)));
                }
            }
        }

        // --- M10.1: validar las anotaciones (conjunto cerrado conocido) ---
        self.check_annotations(program)?;

        // --- Verificación de cada función ---
        for f in &program.functions {
            let profundidad = self.scopes.len();
            if let Err(e) = self.check_function(f) {
                if !self.acumular {
                    return Err(e);
                }
                // M33c: un cuerpo fallido no contamina al siguiente — `check_function` ya
                // restaura type_params/current_self/bounds incluso en error; los ámbitos
                // que el cuerpo dejó a medias se truncan aquí.
                self.scopes.truncate(profundidad);
                self.errores.push(e);
                if self.errores.len() >= MAX_ERRORES {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Valida las anotaciones (M10.1): cada una debe ser **conocida** y estar bien
    /// colocada. `@test` solo en funciones `() -> bool` o `() -> unit` (M13.2b). (`@derive` en M10.1b.)
    fn check_annotations(&self, program: &Program) -> Result<(), TypeError> {
        for f in &program.functions {
            for a in &f.annotations {
                match a.name.as_str() {
                    "test" => {
                        if !a.args.is_empty() {
                            return Err(self.err(a.line, a.col, "'@test' no recibe argumentos".into()));
                        }
                        if !f.params.is_empty() {
                            return Err(self.err(a.line, a.col, format!(
                                "la función de prueba '@test' '{}' no debe recibir parámetros", f.name
                            )));
                        }
                        // M13.2b: una prueba puede devolver `bool` (pasa si es `true`) o `unit`
                        // (pasa si no dispara ningún `assert`/`panic`). El runner distingue ambos.
                        let ret = self.resolve_type(&f.return_type);
                        if ret != Type::Bool && ret != Type::Unit {
                            return Err(self.err(a.line, a.col, format!(
                                "una función '@test' debe devolver bool o unit, no {}", f.return_type
                            )));
                        }
                    }
                    // `@derive` solo tiene sentido sobre tipos (genera su `impl`).
                    "derive" => return Err(self.err(a.line, a.col, "'@derive' solo se permite sobre struct o enum".into())),
                    other => return Err(self.err(a.line, a.col, format!("anotación desconocida: '@{}'", other))),
                }
            }
        }
        let tipos = program.structs.iter().map(|s| &s.annotations)
            .chain(program.enums.iter().map(|e| &e.annotations));
        for anns in tipos {
            for a in anns {
                match a.name.as_str() {
                    // `@derive` ya se validó y generó en `generate_eq_derives` (antes de
                    // `check_program`); aquí solo se acepta como conocida.
                    "derive" => {}
                    "test" => return Err(self.err(a.line, a.col, "'@test' solo se permite sobre funciones".into())),
                    other => return Err(self.err(a.line, a.col, format!("anotación desconocida: '@{}'", other))),
                }
            }
        }
        Ok(())
    }

    fn check_function(&mut self, f: &Function) -> Result<(), TypeError> {
        // Los parámetros de tipo de la función entran en ámbito mientras se verifica
        // su firma y su cuerpo (M6): `Var("T")` es un tipo válido y opaco aquí.
        let mut seen = HashSet::new();
        for tp in &f.type_params {
            if !seen.insert(tp.clone()) {
                return Err(self.err(f.line, f.col, format!("parámetro de tipo '{}' repetido en '{}'", tp, f.name)));
            }
        }
        self.type_params = seen;
        // M9: si es un método de impl (manglado), pon su tipo implementador como `Self`
        // en ámbito mientras se verifica el cuerpo (para anotaciones `Self` internas).
        self.current_self = self.impl_fn_self.get(&f.name).cloned();
        // M9.2: los bounds de esta función en ámbito, para resolver `x.metodo()` con
        // `x: T` acotado y para reenviar diccionarios.
        self.current_fn_bounds = f.bounds.clone();
        for p in &f.params {
            self.ensure_type(&p.ty, p.line, p.col)?;
        }
        self.ensure_type(&f.return_type, f.line, f.col)?;
        let r = self.check_fn_body(&f.params, &f.return_type, &f.body, f.line, f.col, &format!("'{}'", f.name));
        self.current_self = None;
        self.current_fn_bounds = Vec::new();
        self.type_params.clear();
        r
    }

    /// Valida los bounds de una función (M9.2): cada `(parámetro, trait)` debe acotar un
    /// parámetro de tipo declarado por la función, con un trait existente.
    fn check_bounds(&self, f: &Function) -> Result<(), TypeError> {
        for (tp, tr) in &f.bounds {
            if !f.type_params.contains(tp) {
                return Err(self.err(f.line, f.col, format!(
                    "el bound '{}: {}' no acota a ningún parámetro de tipo de '{}'", tp, tr, f.name
                )));
            }
            if !self.traits.contains_key(tr) {
                return Err(self.err(f.line, f.col, format!(
                    "trait '{}' no declarado (en el bound de '{}')", tr, f.name
                )));
            }
        }
        Ok(())
    }

    /// Valida los bounds de un struct/enum (M9.4): cada `(parámetro, trait)` debe acotar un
    /// parámetro de tipo declarado por el tipo, con un trait existente. (Análogo a `check_bounds`.)
    fn check_type_def_bounds(&self, name: &str, type_params: &[String], bounds: &[(String, String)],
        kind: &str, line: usize, col: usize) -> Result<(), TypeError>
    {
        for (tp, tr) in bounds {
            if !type_params.contains(tp) {
                return Err(self.err(line, col, format!(
                    "el bound '{}: {}' no acota a ningún parámetro de tipo del {} '{}'", tp, tr, kind, name
                )));
            }
            if !self.traits.contains_key(tr) {
                return Err(self.err(line, col, format!(
                    "trait '{}' no declarado (en el bound del {} '{}')", tr, kind, name
                )));
            }
        }
        Ok(())
    }

    /// ¿El tipo `concrete` satisface el bound `trait_name`? (M9.2/M9.4). Dos vías: un parámetro de
    /// tipo rígido del llamador que ya declara el mismo bound, o un tipo concreto con impl del trait.
    /// Es la misma lógica de satisfacción que usa `dict_for` para elegir el diccionario.
    fn satisfies_bound(&self, concrete: &Type, trait_name: &str) -> bool {
        if let Type::Var(u) = concrete {
            return self.current_fn_bounds.iter().any(|(bp, tr)| bp == u && tr == trait_name);
        }
        match type_key_of(concrete) {
            Some(key) => self.impl_traits.contains(&(key, trait_name.to_string())),
            None => false,
        }
    }

    /// Registra los traits y valida los impls (M9.1). Construye `self.traits` (firmas),
    /// `self.methods` (`(tipo, método)` → función manglada, para la resolución por punto)
    /// e `self.impl_fn_self` (función manglada → tipo implementador, para `current_self`).
    fn register_traits_impls(&mut self, program: &Program) -> Result<(), TypeError> {
        // 1) Traits: nombres únicos (no chocan con tipos ni funciones) y métodos únicos.
        for t in &program.traits {
            if self.structs.contains_key(&t.name) || self.enum_names.contains(&t.name) {
                return Err(self.err(t.line, t.col, format!("'{}' ya es un tipo; no puede ser también un trait", t.name)));
            }
            if self.traits.contains_key(&t.name) {
                return Err(self.err(t.line, t.col, format!("trait '{}' declarado dos veces", t.name)));
            }
            let mut seen = HashSet::new();
            for m in &t.methods {
                if !seen.insert(m.name.clone()) {
                    return Err(self.err(m.line, m.col, format!("método '{}' repetido en el trait '{}'", m.name, t.name)));
                }
            }
            self.traits.insert(t.name.clone(), t.methods.clone());
            self.trait_tparams.insert(t.name.clone(), t.type_params.clone());
        }

        // 2) Impls: validar contra su trait y poblar las tablas de resolución.
        for imp in &program.impls {
            let trait_methods = match self.traits.get(&imp.trait_name) {
                Some(ms) => ms.clone(),
                None => return Err(self.err(imp.line, imp.col, format!("trait '{}' no declarado", imp.trait_name))),
            };
            // M9.2b: los parámetros de tipo del impl entran en ámbito mientras se resuelve el
            // objetivo y se comparan las firmas, para que `Caja<T>` y un parámetro `x: T`
            // normalicen `T` a `Var` (en vez de `Struct("T")`). Se limpia al terminar el bucle.
            self.type_params = imp.type_params.iter().cloned().collect();
            self.check_impl_bounds(imp)?;
            let target = self.resolve_type(&imp.target);
            self.ensure_impl_target(&target, &imp.type_params, imp.line, imp.col)?;
            let key = type_key_of(&target).unwrap_or_else(|| crate::ice!("el objetivo de impl validado no tiene clave de tipo"));

            // M28.2: impl de un trait con parámetros de tipo (`impl From<S> for E`). Se registra
            // como una **conversión** —no en la tabla de despacho por punto (el método `from` no
            // tiene `self`)—. La consume el operador `?` (`check_try`).
            if is_typed_trait_impl(imp) {
                self.register_typed_trait_impl(imp, &target, &key)?;
                continue;
            }

            // Registrar el impl genérico (para `dict_for`: diccionarios anidados).
            if !imp.type_params.is_empty() {
                self.generic_impls.insert(
                    (key.clone(), imp.trait_name.clone()),
                    GenImpl { type_params: imp.type_params.clone(), target: target.clone(), bounds: imp.bounds.clone() },
                );
            }

            // Nombres del impl sin repetir.
            let mut impl_names = HashSet::new();
            for m in &imp.methods {
                if !impl_names.insert(m.name.clone()) {
                    return Err(self.err(m.line, m.col, format!("método '{}' implementado dos veces", m.name)));
                }
            }
            // Cobertura: no faltan métodos del trait... salvo los que tienen cuerpo por
            // defecto (M9.3a), que se heredan.
            for tm in &trait_methods {
                if !impl_names.contains(&tm.name) && tm.default_body.is_none() {
                    return Err(self.err(imp.line, imp.col, format!(
                        "el impl de '{}' para {} no implementa el método '{}'", imp.trait_name, target, tm.name
                    )));
                }
            }
            // ...ni sobran (cada método del impl pertenece al trait), y las firmas casan.
            for m in &imp.methods {
                let tm = match trait_methods.iter().find(|tm| tm.name == m.name) {
                    Some(tm) => tm,
                    None => return Err(self.err(m.line, m.col, format!(
                        "el trait '{}' no declara un método '{}'", imp.trait_name, m.name
                    ))),
                };
                self.check_method_sig(tm, m, &target)?;
                let mangled = mangle(&key, &m.name);
                if self.methods.contains_key(&(key.clone(), m.name.clone())) {
                    return Err(self.err(m.line, m.col, format!(
                        "método '{}' ambiguo para {}: ya hay un impl que lo provee", m.name, target
                    )));
                }
                self.methods.insert((key.clone(), m.name.clone()), mangled.clone());
                self.impl_fn_self.insert(mangled, target.clone());
            }
            // M9.3a: los métodos por defecto no redefinidos también van a la tabla de
            // resolución (su función sintetizada se inyectó en el paso 0c).
            for tm in &trait_methods {
                if tm.default_body.is_some() && !impl_names.contains(&tm.name) {
                    let mangled = mangle(&key, &tm.name);
                    if self.methods.contains_key(&(key.clone(), tm.name.clone())) {
                        return Err(self.err(imp.line, imp.col, format!(
                            "método '{}' ambiguo para {}: ya hay un impl que lo provee", tm.name, target
                        )));
                    }
                    self.methods.insert((key.clone(), tm.name.clone()), mangled.clone());
                    self.impl_fn_self.insert(mangled, target.clone());
                }
            }
            // Registrar que este tipo implementa este trait (M9.2: verificación de bounds).
            self.impl_traits.insert((key, imp.trait_name.clone()));
        }
        self.type_params.clear();
        Ok(())
    }

    /// M28.2: registra un `impl` de un trait con parámetros de tipo. Hoy solo `From<S>` tiene
    /// semántica (la consume el operador `?`): valida la firma `fn from(origen: S) -> Self` y
    /// guarda la conversión `(origen, destino) → función manglada`. Otros traits con parámetros
    /// de tipo se aceptan sintácticamente pero aún no hacen nada (diferido).
    fn register_typed_trait_impl(&mut self, imp: &ImplBlock, target: &Type, key: &str) -> Result<(), TypeError> {
        let tparams = self.trait_tparams.get(&imp.trait_name).cloned().unwrap_or_default();
        if imp.trait_args.len() != tparams.len() {
            return Err(self.err(imp.line, imp.col, format!(
                "el trait '{}' toma {} parámetro(s) de tipo, pero el impl pasa {}",
                imp.trait_name, tparams.len(), imp.trait_args.len()
            )));
        }
        if imp.trait_name != "From" {
            return Ok(()); // otros traits con parámetros de tipo: sin semántica todavía
        }
        // `From<S> for E` exige `fn from(origen: S) -> E` (sin `self`).
        let src = self.resolve_type(&imp.trait_args[0]);
        let src_key = match type_key_of(&src) {
            Some(k) => k,
            None => return Err(self.err(imp.line, imp.col,
                "el tipo de origen de 'From' no admite conversión".into())),
        };
        let m = match imp.methods.iter().find(|m| m.name == "desde") {
            Some(m) => m,
            None => return Err(self.err(imp.line, imp.col, format!(
                "el impl de 'From' para {} no implementa el método 'desde'", target))),
        };
        if m.params.len() != 1 {
            return Err(self.err(m.line, m.col,
                "'desde' toma exactamente un parámetro (el valor de origen), sin 'self'".into()));
        }
        let got_param = self.resolve_type(&m.params[0].ty);
        if got_param != src {
            return Err(self.err(m.params[0].line, m.params[0].col, format!(
                "el parámetro de 'desde' es {}, pero 'From<{}>' pide {}", got_param, src, src)));
        }
        let got_ret = self.resolve_type(&subst_self(&m.return_type, target));
        if &got_ret != target {
            return Err(self.err(m.line, m.col, format!(
                "'desde' debe devolver {} (el tipo destino), no {}", target, got_ret)));
        }
        self.from_impls.insert((src_key.clone(), key.to_string()), mangle_from(key, &src_key));
        Ok(())
    }

    /// Valida los bounds de un **impl genérico** (M9.2b): cada bound acota un parámetro de
    /// tipo declarado por el impl con un trait existente. (Reusa la idea de `check_bounds`
    /// para funciones, pero sobre los parámetros del impl.)
    fn check_impl_bounds(&self, imp: &ImplBlock) -> Result<(), TypeError> {
        for (tp, trait_name) in &imp.bounds {
            if !imp.type_params.contains(tp) {
                return Err(self.err(imp.line, imp.col, format!(
                    "el bound '{}: {}' menciona un parámetro de tipo que el impl no declara", tp, trait_name
                )));
            }
            if !self.traits.contains_key(trait_name) {
                return Err(self.err(imp.line, imp.col, format!(
                    "el bound '{}: {}' usa un trait no declarado", tp, trait_name
                )));
            }
        }
        Ok(())
    }

    /// Valida el objetivo de un `impl` (`target` ya resuelto) según sus parámetros de tipo:
    /// - **concreto** (`type_params` vacío, M9.1): struct/enum/primitivo conocido **no
    ///   genérico** (las instancias especializadas como `Caja<int>` se difieren);
    /// - **genérico** (M9.2b): `Caja<T>` cuyos argumentos son **exactamente** los parámetros
    ///   de tipo del impl (cada uno un `Var` distinto), y cuya aridad casa con la del tipo.
    fn ensure_impl_target(&self, target: &Type, type_params: &[String], line: usize, col: usize) -> Result<(), TypeError> {
        // Primitivos: solo como objetivo concreto.
        if matches!(target, Type::Int | Type::Float | Type::Bool | Type::String | Type::Char) {
            if type_params.is_empty() {
                return Ok(());
            }
            return Err(self.err(line, col, "un tipo primitivo no es genérico: no admite parámetros de tipo en el impl".into()));
        }
        let (name, args) = match target {
            Type::Struct(n, a) | Type::Enum(n, a) => (n, a),
            _ => return Err(self.err(line, col, "solo se puede implementar un trait para un struct, enum o primitivo".into())),
        };
        // Un struct desconocido sigue siendo `Struct` tras resolver (un enum conocido ya sería
        // `Enum`); rechazarlo aquí.
        if matches!(target, Type::Struct(_, _)) && !self.structs.contains_key(name) {
            return Err(self.err(line, col, format!("no se puede implementar para un tipo desconocido: '{}'", name)));
        }
        let arity = self.struct_tparams.get(name).or_else(|| self.enum_tparams.get(name)).map_or(0, Vec::len);
        if type_params.is_empty() {
            // Impl concreto: el tipo objetivo no puede ser genérico.
            if arity != 0 {
                return Err(self.err(line, col, format!(
                    "'{name}' es genérico: declara sus parámetros en el impl, p. ej. 'impl<T> ... for {name}<T>' (M9.2b)"
                )));
            }
            return Ok(());
        }
        // Impl genérico (M9.2b): aridad y forma del objetivo.
        if arity != type_params.len() {
            return Err(self.err(line, col, format!(
                "'{}' espera {} parámetro(s) de tipo, el impl declara {}", name, arity, type_params.len()
            )));
        }
        let mut vistos = HashSet::new();
        let bien = args.len() == type_params.len()
            && args.iter().all(|a| matches!(a, Type::Var(n) if type_params.contains(n) && vistos.insert(n.clone())));
        if !bien {
            return Err(self.err(line, col, format!(
                "el impl genérico debe aplicarse a '{}<{}>' (sus propios parámetros de tipo, distintos)",
                name, type_params.join(", ")
            )));
        }
        Ok(())
    }

    /// Comprueba que la firma de un método de impl coincide con la del trait, tras
    /// sustituir `Self` por el tipo implementador en ambas (M9.1).
    fn check_method_sig(&self, tm: &MethodSig, m: &Function, target: &Type) -> Result<(), TypeError> {
        if tm.params.len() != m.params.len() {
            return Err(self.err(m.line, m.col, format!(
                "el método '{}' toma {} parámetro(s) (incluido self), pero el trait pide {}",
                m.name, m.params.len(), tm.params.len()
            )));
        }
        for (i, (tp, ip)) in tm.params.iter().zip(&m.params).enumerate() {
            let want = self.resolve_type(&subst_self(&tp.ty, target));
            let got = self.resolve_type(&subst_self(&ip.ty, target));
            if want != got {
                return Err(self.err(ip.line, ip.col, format!(
                    "el parámetro {} del método '{}' es {}, pero el trait pide {}",
                    i + 1, m.name, got, want
                )));
            }
        }
        let want_ret = self.resolve_type(&subst_self(&tm.return_type, target));
        let got_ret = self.resolve_type(&subst_self(&m.return_type, target));
        if want_ret != got_ret {
            return Err(self.err(m.line, m.col, format!(
                "el método '{}' devuelve {}, pero el trait pide {}", m.name, got_ret, want_ret
            )));
        }
        Ok(())
    }

    /// Verifica el cuerpo de una función (nombrada o anónima): declara los
    /// parámetros en un ámbito nuevo, comprueba el bloque y exige que su tipo-valor
    /// (el retorno implícito) coincida con el declarado, salvo que el cuerpo
    /// diverja (retorne por todos los caminos). `label` se usa en los mensajes.
    fn check_fn_body(
        &mut self,
        params: &[Param],
        return_type: &Type,
        body: &Block,
        line: usize,
        col: usize,
        label: &str,
    ) -> Result<(), TypeError> {
        // Normaliza el tipo de retorno (`: Figura` llega como `Struct`, puede ser
        // `Enum`) y úsalo en TODA esta función: tanto para validar los `return` como
        // para comparar con el tipo del cuerpo. Comparar contra el tipo crudo daría
        // un falso negativo `Enum` vs `Struct` con el mismo nombre.
        let return_type = self.resolve_type(return_type);
        self.current_return = return_type.clone();
        self.push_scope();
        // Los parámetros son inmutables (no hay 'var' para ellos).
        for p in params {
            let ty = self.resolve_type(&p.ty);
            self.declare(&p.name, ty, false, (p.line, p.col));
        }

        // El tipo de retorno es el tipo ESPERADO del valor del cuerpo (M6.2): se
        // propaga a la expresión final (y al `if`/`match` que sea) para fijar
        // construcciones como `Lista.Nil` o `None`.
        let body_ty = self.check_block_expected(body, &return_type)?;
        let diverges = block_diverges(body);

        // Posición para el posible error: la expresión final si existe, si no la fn.
        let (eline, ecol) = match &body.tail {
            Some(t) => (t.line, t.col),
            None => (line, col),
        };

        let result = if return_type == Type::Unit {
            // Una función unit no debe terminar produciendo un valor.
            if body_ty != Type::Unit && !diverges {
                Err(self.err(eline, ecol, format!(
                    "{} no declara retorno (unit), pero su cuerpo produce {}",
                    label, body_ty
                )))
            } else {
                Ok(())
            }
        } else if body_ty == return_type || diverges {
            Ok(())
        } else {
            Err(self.err(eline, ecol, format!(
                "{} declara devolver {}, pero su cuerpo produce {}",
                label, return_type, body_ty
            )))
        };

        self.pop_scope();
        result
    }

    // ----- Sentencias -----

    fn check_stmt(&mut self, stmt: &Stmt) -> Result<(), TypeError> {
        match &stmt.kind {
            StmtKind::Let { name, ty, value, mutable } => {
                let var_ty = match ty {
                    // Con anotación: el tipo declarado es el tipo ESPERADO del valor
                    // (chequeo bidireccional, M6.2): fija el `[]` vacío, `Caja.Vacia`,
                    // `None`, etc.
                    Some(ty) => {
                        self.ensure_type(ty, stmt.line, stmt.col)?;
                        // La anotación puede nombrar un enum (llega como `Struct`): normaliza.
                        let ty = self.resolve_type(ty);
                        let vt = self.check_expr_expected(value, &ty)?;
                        if vt != ty {
                            return Err(self.err(value.line, value.col, format!(
                                "'{}' se declara como {} pero se inicializa con {}",
                                name, ty, vt
                            )));
                        }
                        ty
                    }
                    // Sin anotación (M8.1): se infiere del inicializador. Si el valor no
                    // determina su tipo por sí solo (`[]`, `None`, `Caja.Vacia`),
                    // `check_expr` ya falla pidiendo la anotación.
                    None => self.check_expr(value)?,
                };
                self.declare(name, var_ty, *mutable, (stmt.line, stmt.col));
                Ok(())
            }
            StmtKind::LetTuple { names, value, mutable } => {
                // Desestructuración `let (a, b) = e;` (M27.1): el valor debe ser una tupla de la misma
                // aridad; cada nombre se liga con el tipo de su posición (`_` no liga nada).
                let vt = self.check_expr(value)?;
                match vt {
                    Type::Tuple(elems) => {
                        if elems.len() != names.len() {
                            return Err(self.err(value.line, value.col, format!(
                                "la desestructuración tiene {} nombres pero la tupla tiene {} elementos",
                                names.len(), elems.len()
                            )));
                        }
                        for (n, t) in names.iter().zip(elems) {
                            if let Some(name) = n {
                                self.declare(name, t, *mutable, (stmt.line, stmt.col));
                            }
                        }
                        Ok(())
                    }
                    other => Err(self.err(value.line, value.col, format!(
                        "no se puede desestructurar un {} (se esperaba una tupla)", other
                    ))),
                }
            }
            StmtKind::For { pat, iter, body } => {
                // M27.2: determina el/los tipo(s) de la(s) variable(s) según el iterable, los liga en un
                // ámbito nuevo y verifica el cuerpo.
                let bindings: Vec<(String, Type)> = match iter {
                    ForIter::Range { start, end } => {
                        let st = self.check_expr(start)?;
                        let et = self.check_expr(end)?;
                        if st != Type::Int || et != Type::Int {
                            return Err(self.err(stmt.line, stmt.col, format!(
                                "el rango de un for debe ser int..int, no {}..{}", st, et)));
                        }
                        match pat {
                            ForPat::Single(n) => vec![(n.clone(), Type::Int)],
                            ForPat::Tuple(_) => return Err(self.err(stmt.line, stmt.col,
                                "un rango liga una sola variable (no una tupla)".into())),
                        }
                    }
                    ForIter::In(e) => {
                        let it = self.check_expr(e)?;
                        match (&it, pat) {
                            (Type::Array(elem), ForPat::Single(n)) => vec![(n.clone(), (**elem).clone())],
                            (Type::String, ForPat::Single(n)) => vec![(n.clone(), Type::Char)],
                            (Type::Map(k, v), ForPat::Tuple(names)) => {
                                if names.len() != 2 {
                                    return Err(self.err(stmt.line, stmt.col,
                                        "iterar un Map liga exactamente dos variables (clave, valor)".into()));
                                }
                                let mut b = Vec::new();
                                if let Some(kn) = &names[0] { b.push((kn.clone(), (**k).clone())); }
                                if let Some(vn) = &names[1] { b.push((vn.clone(), (**v).clone())); }
                                b
                            }
                            (Type::Map(_, _), ForPat::Single(_)) => return Err(self.err(stmt.line, stmt.col,
                                "iterar un Map requiere una tupla `(clave, valor)`".into())),
                            (other, _) => return Err(self.err(stmt.line, stmt.col, format!(
                                "no se puede iterar sobre {} (se esperaba un arreglo, string o Map)", other))),
                        }
                    }
                };
                self.push_scope();
                for (n, t) in bindings {
                    self.declare(&n, t, false, (stmt.line, stmt.col));
                }
                self.check_block(body)?;
                self.pop_scope();
                Ok(())
            }
            StmtKind::Assign { target, value } => self.check_assign(target, value, stmt.line, stmt.col),
            StmtKind::Return { value } => {
                let vt = match value {
                    // El retorno declarado es el tipo esperado (propaga a `None`, etc.).
                    Some(e) => {
                        let expected = self.current_return.clone();
                        self.check_expr_expected(e, &expected)?
                    }
                    None => Type::Unit,
                };
                if vt != self.current_return {
                    return Err(self.err(stmt.line, stmt.col, format!(
                        "se devuelve {} pero la función declara retorno {}",
                        vt, self.current_return
                    )));
                }
                Ok(())
            }
            StmtKind::Expr(e) => {
                // Una expresión-sentencia solo debe estar bien tipada; su valor se
                // descarta.
                self.check_expr(e)?;
                Ok(())
            }
        }
    }

    /// Verifica una asignación a un lvalue.
    fn check_assign(&mut self, target: &Expr, value: &Expr, line: usize, col: usize) -> Result<(), TypeError> {
        match &target.kind {
            // x = e  — requiere que la variable exista y sea mutable ('var').
            ExprKind::Ident(name) => {
                let (var_ty, mutable) = match self.lookup(name) {
                    Some(v) => (v.ty.clone(), v.mutable),
                    None => return Err(self.err(target.line, target.col, format!("variable '{}' no declarada", name))),
                };
                if !mutable {
                    return Err(self.err(line, col, format!(
                        "no se puede asignar a '{}': es inmutable (declarada con 'let'; usa 'var')",
                        name
                    )));
                }
                // M28.3b: con tipo esperado, un literal entero adopta el ancho uint (`x = 5` con x: u8),
                // igual que en un `let`; para el resto, `check_expr_expected` cae al chequeo normal.
                let vt = self.check_expr_expected(value, &var_ty)?;
                if vt != var_ty {
                    return Err(self.err(value.line, value.col, format!("'{}' es {} pero se le asigna {}", name, var_ty, vt)));
                }
                Ok(())
            }
            // a[i] = e  — mutar el contenido NO requiere 'var' (DESIGN §12.3): la
            // inmutabilidad de `let` ata la variable, no congela el objeto.
            ExprKind::Index { array, index } => {
                // M11.4c-2: los strings son inmutables; `s[i] = c` no se permite (sí se lee `s[i]`).
                if self.check_expr(array)? == Type::String {
                    return Err(self.err(target.line, target.col,
                        "no se puede asignar a un carácter de un string (los strings son inmutables)".into()));
                }
                let elem = self.check_index(array, index)?;
                let vt = self.check_expr_expected(value, &elem)?;
                if vt != elem {
                    return Err(self.err(value.line, value.col, format!("el elemento es {} pero se le asigna {}", elem, vt)));
                }
                Ok(())
            }
            // p.x = e  — mutar un campo (no requiere 'var', como el índice).
            ExprKind::Field { object, name } => {
                // M34 (SPEC §5): una posición de tupla NO es asignable — la tupla es un
                // agregado inmutable (para mutar, desestructura o usa un arreglo). Sin
                // esto, `t.0 = v` pasaba el checker sin bajarse y reventaba los motores.
                if name.chars().all(|c| c.is_ascii_digit()) {
                    let ot = self.check_expr(object)?;
                    if matches!(ot, Type::Tuple(_)) {
                        return Err(self.err(line, col, format!(
                            "una posición de tupla no es asignable ('.{name}'); desestructura la tupla o usa un arreglo"
                        )));
                    }
                }
                let fty = self.check_field(object, name)?;
                let vt = self.check_expr_expected(value, &fty)?;
                if vt != fty {
                    return Err(self.err(value.line, value.col, format!("el campo '{}' es {} pero se le asigna {}", name, fty, vt)));
                }
                Ok(())
            }
            _ => Err(self.err(target.line, target.col, "el lado izquierdo no es asignable".into())),
        }
    }

    /// Verifica `a[i]` y devuelve el tipo de elemento. Reusado por la indexación
    /// como expresión y como destino de asignación.
    fn check_index(&mut self, array: &Expr, index: &Expr) -> Result<Type, TypeError> {
        let at = self.check_expr(array)?;
        let it = self.check_expr(index)?;
        if it != Type::Int {
            return Err(self.err(index.line, index.col, format!("el índice debe ser int, no {}", it)));
        }
        match at {
            Type::Array(elem) => Ok(*elem),
            // M11.4c-2: indexar un string da el carácter en esa posición.
            Type::String => Ok(Type::Char),
            // M16.1a: indexar bytes da el octeto (0–255) como int.
            Type::Bytes => Ok(Type::Int),
            other => Err(self.err(array.line, array.col, format!("no se puede indexar un {} (no es un arreglo, string ni bytes)", other))),
        }
    }

    /// Comprueba que una lista de parámetros de tipo no tenga repetidos.
    fn check_unique_tparams(&self, params: &[String], owner: &str, line: usize, col: usize) -> Result<(), TypeError> {
        let mut seen = HashSet::new();
        for tp in params {
            if !seen.insert(tp) {
                return Err(self.err(line, col, format!("parámetro de tipo '{}' repetido en '{}'", tp, owner)));
            }
        }
        Ok(())
    }

    /// Verifica que un tipo es válido: los nombres referenciados deben existir y, si
    /// son genéricos, llevar la **aridad** correcta de argumentos de tipo.
    fn ensure_type(&self, ty: &Type, line: usize, col: usize) -> Result<(), TypeError> {
        match ty {
            Type::Array(elem) => self.ensure_type(elem, line, col),
            // `Map<K, V>` (M13.1): la clave debe ser hashable (int/string/char/bool, o un
            // parámetro de tipo genérico). Llega ya como `Type::Map` (resuelto) o como
            // `Struct("Map", args)` (sin resolver, p. ej. en un contexto que no pasó por
            // `resolve_type`); se cubren ambas formas.
            Type::Map(k, v) => {
                if !is_hashable_key(k) {
                    return Err(self.err(line, col, format!(
                        "la clave de un Map debe ser int/string/char/bool/bytes, no {}", k
                    )));
                }
                self.ensure_type(k, line, col)?;
                self.ensure_type(v, line, col)
            }
            Type::Struct(name, args) if name == "Map" => {
                if args.len() != 2 {
                    return Err(self.err(line, col, format!(
                        "Map espera 2 argumentos de tipo (clave y valor), no {}", args.len()
                    )));
                }
                self.ensure_type(&Type::Map(Box::new(self.resolve_type(&args[0])), Box::new(self.resolve_type(&args[1]))), line, col)
            }
            // `Channel<T>` (M12.1): el elemento puede ser cualquier tipo (sin restricción, a diferencia de
            // la clave hashable de Map). Llega ya como `Type::Channel` o como `Struct("Channel", [T])`.
            Type::Channel(t) => self.ensure_type(t, line, col),
            Type::Struct(name, args) if name == "Channel" => {
                if args.len() != 1 {
                    return Err(self.err(line, col, format!(
                        "Channel espera 1 argumento de tipo, no {}", args.len()
                    )));
                }
                self.ensure_type(&Type::Channel(Box::new(self.resolve_type(&args[0]))), line, col)
            }
            // `Task<T>` (M12.3): como `Channel`, el elemento puede ser cualquier tipo.
            Type::Task(t) => self.ensure_type(t, line, col),
            Type::Struct(name, args) if name == "Task" => {
                if args.len() != 1 {
                    return Err(self.err(line, col, format!(
                        "Task espera 1 argumento de tipo, no {}", args.len()
                    )));
                }
                self.ensure_type(&Type::Task(Box::new(self.resolve_type(&args[0]))), line, col)
            }
            // `Self` (M9) llega como `Struct("Self")` sin resolver; fuera de un impl no
            // tiene un tipo implementador al que referirse.
            Type::Struct(name, _) if name == "Self" => {
                Err(self.err(line, col, "'Self' solo es válido dentro de un trait o impl".into()))
            }
            // Un identificador en posición de tipo llega como `Struct(name, args)`
            // desde el parser; aquí puede ser un struct, un enum o un parámetro de
            // tipo en ámbito (M6).
            Type::Struct(name, args) => {
                if self.type_params.contains(name) {
                    if !args.is_empty() {
                        return Err(self.err(line, col, format!("el parámetro de tipo '{}' no recibe argumentos", name)));
                    }
                    return Ok(());
                }
                let arity = self.struct_tparams.get(name)
                    .or_else(|| self.enum_tparams.get(name));
                match arity {
                    Some(tparams) => self.ensure_type_args(name, tparams.len(), args, line, col),
                    None => Err(self.err(line, col, format!("tipo desconocido: '{}' no declarado", name))),
                }
            }
            Type::Enum(name, args) => match self.enum_tparams.get(name) {
                Some(tparams) => self.ensure_type_args(name, tparams.len(), args, line, col),
                None => Err(self.err(line, col, format!("tipo desconocido: enum '{}' no declarado", name))),
            },
            // Un parámetro de tipo (M6) es válido si está en ámbito.
            Type::Var(name) if !self.type_params.contains(name) => {
                Err(self.err(line, col, format!("parámetro de tipo '{}' fuera de ámbito", name)))
            }
            // `Self` solo tiene sentido dentro de un trait/impl; aquí ya se habría
            // reclasificado al tipo implementador. Si llega `SelfType`, es un uso fuera
            // de lugar (M9).
            Type::SelfType => Err(self.err(line, col, "'Self' solo es válido dentro de un trait o impl".into())),
            // `dyn Trait` (M9.3b): el trait debe existir.
            Type::Dyn(traits) => {
                // Cada trait del conjunto debe existir, y ningún nombre de método puede repetirse
                // entre los traits (no se sabría a cuál despachar `obj.m()`).
                let mut metodos: HashSet<String> = HashSet::new();
                for tr in traits {
                    let Some(ms) = self.traits.get(tr) else {
                        return Err(self.err(line, col, format!("trait '{}' no declarado (en 'dyn {}')", tr, traits.join(" + "))));
                    };
                    for m in ms {
                        if !metodos.insert(m.name.clone()) {
                            return Err(self.err(line, col, format!(
                                "el método '{}' aparece en más de un trait de 'dyn {}': es ambiguo", m.name, traits.join(" + ")
                            )));
                        }
                    }
                }
                Ok(())
            }
            Type::Fn(params, ret) => {
                for p in params {
                    self.ensure_type(p, line, col)?;
                }
                self.ensure_type(ret, line, col)
            }
            _ => Ok(()),
        }
    }

    /// Comprueba la aridad de los argumentos de tipo y valida cada uno.
    fn ensure_type_args(&self, name: &str, arity: usize, args: &[Type], line: usize, col: usize) -> Result<(), TypeError> {
        if args.len() != arity {
            return Err(self.err(line, col, format!(
                "'{}' espera {} argumento(s) de tipo, se le dieron {}", name, arity, args.len()
            )));
        }
        for a in args {
            self.ensure_type(a, line, col)?;
        }
        Ok(())
    }

    /// Normaliza un tipo proveniente de una anotación. El parser produce
    /// `Struct(name, args)` para cualquier identificador; aquí se reclasifica el
    /// nombre (y se resuelven los argumentos), recursivamente:
    ///   - un **parámetro de tipo** en ámbito → `Var` (M6; tapa a los nombres de tipo);
    ///   - un **enum** → `Enum` (M5); en otro caso, se queda como `Struct`.
    fn resolve_type(&self, ty: &Type) -> Type {
        match ty {
            // `Self` (M9): dentro de un `impl`, denota el tipo implementador
            // (`current_self`). El parser lo trae como `Struct("Self")` (en una
            // anotación) o ya como `SelfType` (el `self` receptor). Fuera de un impl
            // queda como `SelfType` y `ensure_type` lo rechaza.
            Type::SelfType => self.current_self.clone().unwrap_or(Type::SelfType),
            Type::Struct(name, args) if name == "Self" && args.is_empty() => {
                self.current_self.clone().unwrap_or(Type::SelfType)
            }
            // `Map<K, V>` (M13.1) llega como `Struct("Map", [K, V])`; se reclasifica a `Type::Map`
            // (igual que `Enum`/`Var`). La validación de la clave hashable va en `ensure_type`.
            Type::Struct(name, args) if name == "Map" && args.len() == 2 => {
                Type::Map(Box::new(self.resolve_type(&args[0])), Box::new(self.resolve_type(&args[1])))
            }
            Type::Map(k, v) => {
                Type::Map(Box::new(self.resolve_type(k)), Box::new(self.resolve_type(v)))
            }
            // `Channel<T>` (M12.1) llega como `Struct("Channel", [T])`; se reclasifica a `Type::Channel`.
            Type::Struct(name, args) if name == "Channel" && args.len() == 1 => {
                Type::Channel(Box::new(self.resolve_type(&args[0])))
            }
            Type::Channel(t) => Type::Channel(Box::new(self.resolve_type(t))),
            // `Task<T>` (M12.3) llega como `Struct("Task", [T])`; se reclasifica a `Type::Task`.
            Type::Struct(name, args) if name == "Task" && args.len() == 1 => {
                Type::Task(Box::new(self.resolve_type(&args[0])))
            }
            Type::Task(t) => Type::Task(Box::new(self.resolve_type(t))),
            Type::Struct(name, args) => {
                if self.type_params.contains(name) {
                    Type::Var(name.clone())
                } else {
                    let rargs: Vec<Type> = args.iter().map(|a| self.resolve_type(a)).collect();
                    if self.enum_names.contains(name) {
                        Type::Enum(name.clone(), rargs)
                    } else {
                        Type::Struct(name.clone(), rargs)
                    }
                }
            }
            Type::Enum(name, args) => {
                Type::Enum(name.clone(), args.iter().map(|a| self.resolve_type(a)).collect())
            }
            Type::Array(elem) => Type::Array(Box::new(self.resolve_type(elem))),
            Type::Fn(params, ret) => Type::Fn(
                params.iter().map(|p| self.resolve_type(p)).collect(),
                Box::new(self.resolve_type(ret)),
            ),
            other => other.clone(),
        }
    }

    /// Verifica un literal de struct `Nombre { campo: valor, ... }`. Para structs
    /// **genéricos** infiere los argumentos de tipo de los valores de los campos (y
    /// del tipo esperado, si los valores no bastan). Devuelve `Struct(name, args)`.
    fn check_struct_lit(&mut self, name: &str, fields: &[(String, Expr)], expected: Option<&Type>, line: usize, col: usize) -> Result<Type, TypeError> {
        let declared = match self.structs.get(name) {
            Some(d) => d.clone(), // clonamos para soltar el préstamo de self
            None => return Err(self.err(line, col, format!("struct '{}' no declarado", name))),
        };
        // M10.2f: hover/def del nombre de tipo en el literal `Nombre { … }`.
        if self.gather {
            let def = self.type_defs.get(name).copied();
            self.record_named(line, col, name.chars().count(), format!("struct {}", name), def);
        }
        let tparams = self.struct_tparams.get(name).cloned().unwrap_or_default();
        // No debe haber campos desconocidos.
        for (fname, fexpr) in fields {
            if !declared.iter().any(|(dname, _)| dname == fname) {
                return Err(self.err(fexpr.line, fexpr.col, format!("'{}' no tiene un campo '{}'", name, fname)));
            }
        }
        // σ: parámetro de tipo → tipo inferido. Se siembra del tipo esperado.
        let mut sigma = seed_sigma_from_expected(expected, name, &tparams);
        // Cada campo declarado debe estar presente exactamente una vez; su valor
        // determina (unifica) los parámetros de tipo del struct.
        for (dname, dty) in &declared {
            let matches: Vec<&(String, Expr)> = fields.iter().filter(|(fname, _)| fname == dname).collect();
            match matches.as_slice() {
                [] => return Err(self.err(line, col, format!("falta el campo '{}' en el literal de '{}'", dname, name))),
                [(_, value)] => {
                    let vt = self.check_value_against(value, dty, &sigma)?;
                    unify(dty, &vt, &mut sigma).map_err(|reason| self.err(value.line, value.col, format!(
                        "campo '{}' de '{}': {}", dname, name, reason
                    )))?;
                }
                _ => return Err(self.err(line, col, format!("campo '{}' de '{}' repetido", dname, name))),
            }
        }
        let targs = self.finalize_type_args(&tparams, &sigma, &format!("el struct '{}'", name), line, col)?;
        // M9.4: cada parámetro acotado debe resolver a un tipo que satisfaga su bound.
        let bounds = self.struct_bounds.get(name).cloned().unwrap_or_default();
        self.check_construction_bounds(name, &tparams, &targs, &bounds, line, col)?;
        Ok(Type::Struct(name.to_string(), targs))
    }

    /// M9.4: en la construcción de un struct/enum genérico acotado, verifica que cada parámetro de
    /// tipo acotado resolvió a un tipo que **satisface** su bound. `targs` está en el orden de
    /// `tparams`. Es solo una comprobación (no genera diccionarios: un valor de datos no llama métodos).
    fn check_construction_bounds(&self, name: &str, tparams: &[String], targs: &[Type],
        bounds: &[(String, String)], line: usize, col: usize) -> Result<(), TypeError>
    {
        for (tp, trait_name) in bounds {
            let Some(pos) = tparams.iter().position(|p| p == tp) else { continue };
            let concrete = &targs[pos];
            if !self.satisfies_bound(concrete, trait_name) {
                return Err(self.err(line, col, format!(
                    "'{}' requiere que '{}' sea '{}', pero {} no lo implementa", name, tp, trait_name, concrete
                )));
            }
        }
        Ok(())
    }

    /// Verifica la construcción de una variante de enum `Enum.Variante(args)`. Para
    /// enums **genéricos** infiere los argumentos de tipo del payload (y del tipo
    /// esperado, p. ej. para `Caja.Vacia`). Devuelve `Enum(enum_name, args)`.
    fn check_enum_lit(&mut self, enum_name: &str, variant: &str, args: &[Expr], expected: Option<&Type>, line: usize, col: usize) -> Result<Type, TypeError> {
        let payload = match self.enums.get(enum_name) {
            Some(variants) => match variants.iter().find(|(vname, _)| vname == variant) {
                Some((_, payload)) => payload.clone(), // clonar para soltar el préstamo de self
                None => return Err(self.err(line, col, format!("el enum '{}' no tiene la variante '{}'", enum_name, variant))),
            },
            None => return Err(self.err(line, col, format!("enum '{}' no declarado", enum_name))),
        };
        // M10.2f: hover/def del nombre de enum en `Enum.Variante(...)`.
        if self.gather {
            let def = self.type_defs.get(enum_name).copied();
            self.record_named(line, col, enum_name.chars().count(), format!("enum {}", enum_name), def);
        }
        let tparams = self.enum_tparams.get(enum_name).cloned().unwrap_or_default();
        if args.len() != payload.len() {
            return Err(self.err(line, col, format!(
                "la variante '{}.{}' espera {} argumento(s), se dieron {}",
                enum_name, variant, payload.len(), args.len()
            )));
        }
        let mut sigma = seed_sigma_from_expected(expected, enum_name, &tparams);
        for (arg, pty) in args.iter().zip(&payload) {
            let at = self.check_value_against(arg, pty, &sigma)?;
            unify(pty, &at, &mut sigma).map_err(|reason| self.err(arg.line, arg.col, format!(
                "'{}.{}': {}", enum_name, variant, reason
            )))?;
        }
        let targs = self.finalize_type_args(&tparams, &sigma, &format!("la variante '{}.{}'", enum_name, variant), line, col)?;
        // M9.4: cada parámetro acotado debe resolver a un tipo que satisfaga su bound.
        let bounds = self.enum_bounds.get(enum_name).cloned().unwrap_or_default();
        self.check_construction_bounds(enum_name, &tparams, &targs, &bounds, line, col)?;
        Ok(Type::Enum(enum_name.to_string(), targs))
    }

    /// Verifica el valor de un campo/payload propagándole como **tipo esperado** el
    /// tipo declarado ya sustituido con lo inferido hasta ahora (`σ`) —pero solo si
    /// ese tipo es concreto (sin `Var`); si todavía tiene incógnitas, no aporta—.
    fn check_value_against(&mut self, value: &Expr, declared: &Type, sigma: &HashMap<String, Type>) -> Result<Type, TypeError> {
        let exp = subst(declared, sigma);
        if type_has_var(&exp) {
            self.check_expr(value)
        } else {
            self.check_expr_expected(value, &exp)
        }
    }

    /// Para cada parámetro de tipo, recupera lo inferido en `σ` (en orden), o error si
    /// quedó sin determinar (ni de los argumentos ni del tipo esperado).
    fn finalize_type_args(&self, tparams: &[String], sigma: &HashMap<String, Type>, label: &str, line: usize, col: usize) -> Result<Vec<Type>, TypeError> {
        let mut targs = Vec::with_capacity(tparams.len());
        for tp in tparams {
            match sigma.get(tp) {
                Some(t) => targs.push(t.clone()),
                None => return Err(self.err(line, col, format!(
                    "no se pudo inferir el parámetro de tipo '{}' de {}; anota el tipo", tp, label
                ))),
            }
        }
        Ok(targs)
    }

    /// Verifica un `match (escrutinio) { patrón => cuerpo, ... }` (M5.2):
    ///   - el escrutinio debe ser un enum;
    ///   - cada patrón debe pertenecer a ese enum y ligar el payload con la aridad
    ///     correcta; los brazos producen un tipo común (como las ramas de un `if`);
    ///   - debe ser **exhaustivo**: cubrir todas las variantes o tener un catch-all.
    fn check_match(&mut self, scrutinee: &Expr, arms: &[MatchArm], expected: Option<&Type>, line: usize, col: usize) -> Result<Type, TypeError> {
        let scrut_ty = self.check_expr(scrutinee)?;
        let (enum_name, targs) = match &scrut_ty {
            Type::Enum(n, args) => (n.clone(), args.clone()),
            other => return Err(self.err(scrutinee.line, scrutinee.col, format!(
                "match requiere un enum, pero el escrutinio es {}", other
            ))),
        };
        if arms.is_empty() {
            return Err(self.err(line, col, "un match no puede estar vacío".into()));
        }
        // Variantes del enum (clonadas para soltar el préstamo de self).
        let variants = self.enums.get(&enum_name).unwrap_or_else(|| crate::ice!("el enum '{}' no está en la tabla del checker", enum_name)).clone();
        // σ del enum: liga sus parámetros de tipo con los argumentos del escrutinio,
        // para sustituir los payloads (`Some(T)` sobre `Option<int>` liga `T = int`).
        let enum_tparams = self.enum_tparams.get(&enum_name).cloned().unwrap_or_default();
        let enum_sigma: HashMap<String, Type> = enum_tparams.into_iter().zip(targs).collect();

        let mut covered: HashSet<String> = HashSet::new();
        let mut catchall = false;
        let mut result_ty: Option<Type> = None;

        for arm in arms {
            // Un brazo tras un catch-all nunca se alcanza.
            if catchall {
                return Err(self.err(arm.line, arm.col,
                    "brazo inalcanzable: un brazo anterior ya cubre todos los casos".into()));
            }
            // Comprueba el patrón y obtiene las variables a ligar (payload sustituido).
            let binds = self.check_pattern(&arm.pattern, &scrut_ty, &enum_name, &variants, &enum_sigma, &mut covered, &mut catchall)?;
            // Verifica el cuerpo con esas variables en un ámbito propio, propagando el
            // tipo esperado del match a cada brazo (para construcciones como `None`).
            self.push_scope();
            for (name, ty) in binds {
                self.declare(&name, ty, false, (arm.line, arm.col));
            }
            let body_ty = match expected {
                Some(exp) => self.check_expr_expected(&arm.body, exp),
                None => self.check_expr(&arm.body),
            };
            self.pop_scope();
            let body_ty = body_ty?;
            // M13.2b/M14: un brazo que diverge (termina en `panic`/`return`) no fija el tipo del
            // match; lo ceden los demás (igual que una rama de `if`). Así
            // `match (o) { Some(v) => v, None => panic("…") }` cuadra.
            if expr_diverges(&arm.body) {
                continue;
            }
            // Todos los brazos (no divergentes) convergen a un mismo tipo (el tipo del match).
            match &result_ty {
                None => result_ty = Some(body_ty),
                Some(prev) if *prev != body_ty => {
                    return Err(self.err(arm.body.line, arm.body.col, format!(
                        "los brazos del match producen tipos distintos: {} y {}", prev, body_ty
                    )));
                }
                _ => {}
            }
        }

        // Exhaustividad: sin catch-all, deben estar TODAS las variantes.
        if !catchall {
            let missing: Vec<&str> = variants
                .iter()
                .map(|(v, _)| v.as_str())
                .filter(|v| !covered.contains(*v))
                .collect();
            if !missing.is_empty() {
                return Err(self.err(line, col, format!(
                    "match no exhaustivo en '{}': faltan las variantes: {}",
                    enum_name, missing.join(", ")
                )));
            }
        }

        // Si NINGÚN brazo produjo valor (todos divergen: `return`/`panic`), el `match` diverge; le damos
        // tipo unit (antes esto hacía `panic` en el checker sobre código válido — p. ej. un `match` cuyos
        // dos brazos hacen `return`). Como statement es correcto; su valor no se alcanza nunca.
        Ok(result_ty.unwrap_or(Type::Unit))
    }

    /// Verifica el operador de propagación `expr?` (M6.3). El operando debe ser un
    /// `Result<T, E>` o un `Option<T>`; el resultado es el valor desempaquetado `T`.
    /// La función envolvente debe declarar un retorno **compatible** con lo que `?`
    /// propagaría: `Result<_, E>` con la misma `E`, o `Option<_>`.
    fn check_try(&mut self, inner: &Expr, line: usize, col: usize) -> Result<Type, TypeError> {
        let it = self.check_expr(inner)?;
        match &it {
            Type::Enum(name, args) if name == "Result" && args.len() == 2 => {
                let (ok_ty, err_ty) = (args[0].clone(), args[1].clone());
                match &self.current_return {
                    // Mismo tipo de error → `?` propaga tal cual (M6.3).
                    Type::Enum(rn, rargs) if rn == "Result" && rargs.len() == 2 && rargs[1] == err_ty => Ok(ok_ty),
                    // M28.2: distinto tipo de error, pero hay `impl From<E1> for E2` → `?` convierte.
                    // (Solo el camino de ÉXITO diverge de M6.3; el error conserva el mismo texto para no
                    //  romper el oráculo del checker auto-alojado, que no conoce `From`.)
                    Type::Enum(rn, rargs) if rn == "Result" && rargs.len() == 2 => {
                        match (type_key_of(&err_ty), type_key_of(&rargs[1])) {
                            (Some(k1), Some(k2)) if self.from_impls.contains_key(&(k1.clone(), k2.clone())) => {
                                let mangled = self.from_impls[&(k1, k2)].clone();
                                self.try_conversions.insert((line, col), mangled);
                                Ok(ok_ty)
                            }
                            _ => Err(self.err(line, col, format!(
                                "'?' sobre {} requiere que la función devuelva Result<_, {}>, pero devuelve {}",
                                it, err_ty, self.current_return
                            ))),
                        }
                    }
                    other => Err(self.err(line, col, format!(
                        "'?' sobre {} requiere que la función devuelva Result<_, {}>, pero devuelve {}",
                        it, err_ty, other
                    ))),
                }
            }
            Type::Enum(name, args) if name == "Option" && args.len() == 1 => {
                let some_ty = args[0].clone();
                match &self.current_return {
                    Type::Enum(rn, rargs) if rn == "Option" && rargs.len() == 1 => Ok(some_ty),
                    other => Err(self.err(line, col, format!(
                        "'?' sobre {} requiere que la función devuelva Option<_>, pero devuelve {}",
                        it, other
                    ))),
                }
            }
            other => Err(self.err(inner.line, inner.col, format!(
                "'?' requiere un Result o un Option, no {}", other
            ))),
        }
    }

    /// Comprueba un patrón contra el enum del escrutinio. Devuelve las variables que
    /// liga (nombre, tipo) para declararlas en el cuerpo del brazo. Actualiza el
    /// conjunto de variantes cubiertas y marca si el patrón es catch-all.
    fn check_pattern(
        &self,
        pat: &Pattern,
        scrut_ty: &Type,
        enum_name: &str,
        variants: &[(String, Vec<Type>)],
        enum_sigma: &HashMap<String, Type>,
        covered: &mut HashSet<String>,
        catchall: &mut bool,
    ) -> Result<Vec<(String, Type)>, TypeError> {
        match &pat.kind {
            PatternKind::Wildcard => {
                *catchall = true;
                Ok(Vec::new())
            }
            PatternKind::Binding(name) => {
                // Liga el escrutinio completo; cubre todo lo restante.
                *catchall = true;
                Ok(vec![(name.clone(), scrut_ty.clone())])
            }
            PatternKind::Variant { enum_name: pat_enum, variant, bindings } => {
                if pat_enum != enum_name {
                    return Err(self.err(pat.line, pat.col, format!(
                        "el patrón es del enum '{}', pero el escrutinio es '{}'", pat_enum, enum_name
                    )));
                }
                let payload = match variants.iter().find(|(v, _)| v == variant) {
                    Some((_, p)) => p,
                    None => return Err(self.err(pat.line, pat.col, format!(
                        "el enum '{}' no tiene la variante '{}'", enum_name, variant
                    ))),
                };
                if bindings.len() != payload.len() {
                    return Err(self.err(pat.line, pat.col, format!(
                        "el patrón '{}.{}' liga {} valor(es), pero la variante tiene {}",
                        enum_name, variant, bindings.len(), payload.len()
                    )));
                }
                if !covered.insert(variant.clone()) {
                    return Err(self.err(pat.line, pat.col, format!(
                        "la variante '{}' ya está cubierta por un brazo anterior", variant
                    )));
                }
                // Cada sub-binding nombrado liga el payload, ya sustituido con los
                // argumentos de tipo del escrutinio (`x` en `Some(x)` sobre
                // `Option<int>` es un `int`).
                let mut binds = Vec::new();
                for (b, ty) in bindings.iter().zip(payload) {
                    if let Some(name) = b {
                        binds.push((name.clone(), subst(ty, enum_sigma)));
                    }
                }
                Ok(binds)
            }
        }
    }

    /// Verifica `obj.name` y devuelve el tipo del campo. Para un struct genérico, el
    /// tipo del campo se **sustituye** con los argumentos de tipo del objeto: el campo
    /// `primero: A` de `Par<int, bool>` es un `int`.
    fn check_field(&mut self, object: &Expr, name: &str) -> Result<Type, TypeError> {
        let ot = self.check_expr(object)?;
        // Acceso a **tupla** `t.0` (M27.1): un nombre de campo numérico solo es válido sobre una tupla.
        if let Type::Tuple(elems) = &ot {
            let idx: usize = name.parse().map_err(|_| self.err(object.line, object.col,
                format!("no se puede acceder a '.{}' en una tupla (usa un índice como .0)", name)))?;
            if idx >= elems.len() {
                return Err(self.err(object.line, object.col, format!(
                    "la tupla tiene {} elementos; el índice .{} está fuera de rango", elems.len(), idx)));
            }
            return Ok(elems[idx].clone());
        }
        match ot {
            Type::Struct(sname, targs) => {
                let fields = self.structs.get(&sname).unwrap_or_else(|| crate::ice!("el struct '{}' no está en la tabla del checker", sname));
                let fty = match fields.iter().find(|(fname, _)| fname == name) {
                    Some((_, fty)) => fty.clone(),
                    None => return Err(self.err(object.line, object.col, format!("el struct '{}' no tiene un campo '{}'", sname, name))),
                };
                let tparams = self.struct_tparams.get(&sname).cloned().unwrap_or_default();
                let sigma: HashMap<String, Type> = tparams.into_iter().zip(targs).collect();
                Ok(subst(&fty, &sigma))
            }
            other => Err(self.err(object.line, object.col, format!("no se puede acceder a '.{}' en un {} (no es un struct)", name, other))),
        }
    }

    // ----- Expresiones (devuelven su tipo) -----

    /// Verifica una expresión con un **tipo esperado** del contexto (chequeo
    /// bidireccional, M6.2). Solo unos pocos nodos lo aprovechan —la construcción de
    /// enums/structs, el arreglo vacío `[]`, y las formas "transparentes" que lo
    /// propagan (`if`/`match`/bloque)—; el resto delega en `check_expr` (que lo
    /// ignora). El llamador compara igualmente el resultado con lo que necesita.
    fn check_expr_expected(&mut self, expr: &Expr, expected: &Type) -> Result<Type, TypeError> {
        // M9.3b: si se espera un `dyn Trait`, un valor **concreto** que implemente el trait
        // se **coerciona** al trait object. Las formas que propagan el tipo esperado
        // (`if`/`match`/`bloque`) NO se interceptan aquí: dejan que la coerción ocurra en
        // sus hojas (cada rama por separado), igual que el resto del chequeo bidireccional.
        if let Type::Dyn(traits) = expected {
            let propaga = matches!(expr.kind, ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::Block(_));
            if !propaga {
                return self.coerce_to_dyn(expr, &traits.clone(), expr.line, expr.col);
            }
        }
        // M28.3b: un literal entero adopta el ancho sin signo esperado (`let x: u8 = 5`).
        if let Type::UInt(w) = expected {
            if let Some(t) = self.coerce_uint_literal(expr, *w)? {
                return Ok(t);
            }
            // Un binario aritmético/bit a bit (mismo tipo que sus operandos) propaga el ancho a
            // ambos lados: `let z: u8 = 200 + 100` coerciona los dos literales. Solo si ambos
            // acaban siendo el uint esperado; si no, se cae al chequeo normal (que da el error).
            if let ExprKind::Binary { op, left, right } = &expr.kind {
                if is_width_preserving(*op) {
                    let lt = self.check_expr_expected(left, expected)?;
                    let rt = self.check_expr_expected(right, expected)?;
                    if lt == *expected && rt == *expected {
                        return Ok(expected.clone());
                    }
                }
            }
        }
        match &expr.kind {
            // M13.1: `map_new()` es indeterminado (como `[]`/`None`); su tipo lo fija el esperado.
            ExprKind::Call { callee, args }
                if matches!(&callee.kind, ExprKind::Ident(n) if n == "map_new") =>
            {
                if !args.is_empty() {
                    return Err(self.err(expr.line, expr.col, "map_new no recibe argumentos".into()));
                }
                match expected {
                    Type::Map(_, _) => Ok(expected.clone()),
                    _ => Err(self.err(expr.line, expr.col, format!(
                        "map_new produce un Map, pero aquí se espera {}", expected
                    ))),
                }
            }
            // M12.1: `channel()` es indeterminado (como `map_new()`); su tipo lo fija el esperado.
            // M12.2: `channel(n)` admite una capacidad `int` (el tipo de elemento sigue indeterminado).
            ExprKind::Call { callee, args }
                if matches!(&callee.kind, ExprKind::Ident(n) if n == "channel") =>
            {
                if args.len() > 1 {
                    return Err(self.err(expr.line, expr.col,
                        "channel recibe a lo sumo un argumento (la capacidad)".into()));
                }
                if let Some(cap) = args.first() {
                    let ct = self.check_expr(cap)?;
                    if !matches!(ct, Type::Int) {
                        return Err(self.err(cap.line, cap.col,
                            format!("la capacidad de channel debe ser int, no {}", ct)));
                    }
                }
                match expected {
                    Type::Channel(_) => Ok(expected.clone()),
                    _ => Err(self.err(expr.line, expr.col, format!(
                        "channel produce un Channel, pero aquí se espera {}", expected
                    ))),
                }
            }
            ExprKind::StructLit { name, fields } => {
                self.check_struct_lit(name, fields, Some(expected), expr.line, expr.col)
            }
            ExprKind::EnumLit { enum_name, variant, args } => {
                self.check_enum_lit(enum_name, variant, args, Some(expected), expr.line, expr.col)
            }
            ExprKind::Match { scrutinee, arms } => {
                self.check_match(scrutinee, arms, Some(expected), expr.line, expr.col)
            }
            ExprKind::Block(b) => self.check_block_expected(b, expected),
            // Arreglo: con un tipo esperado `[T]`, el vacío adopta `[T]` (arregla la
            // aspereza histórica) y los elementos se chequean contra `T`.
            ExprKind::ArrayLit(elems) => match expected {
                Type::Array(elem_exp) => {
                    for e in elems {
                        let t = self.check_expr_expected(e, elem_exp)?;
                        if t != **elem_exp {
                            return Err(self.err(e.line, e.col, format!(
                                "los elementos del arreglo deben ser {}, no {}", elem_exp, t
                            )));
                        }
                    }
                    Ok(Type::Array(elem_exp.clone()))
                }
                _ => self.check_expr(expr),
            },
            ExprKind::If { cond, then_branch, else_branch } => {
                let ct = self.check_expr(cond)?;
                if ct != Type::Bool {
                    return Err(self.err(cond.line, cond.col, format!("la condición del if debe ser bool, no {}", ct)));
                }
                let then_ty = self.check_block_expected(then_branch, expected)?;
                match else_branch {
                    None => {
                        if then_ty != Type::Unit {
                            return Err(self.err(expr.line, expr.col, format!(
                                "un if sin else tiene tipo unit, pero su rama produce {} (añade un else)", then_ty
                            )));
                        }
                        Ok(Type::Unit)
                    }
                    Some(else_e) => {
                        let else_ty = self.check_expr_expected(else_e, expected)?;
                        // M13.2a: una rama divergente (p.ej. `panic`) cede el tipo a la otra.
                        if block_diverges(then_branch) {
                            return Ok(else_ty);
                        }
                        if expr_diverges(else_e) {
                            return Ok(then_ty);
                        }
                        if then_ty != else_ty {
                            return Err(self.err(expr.line, expr.col, format!(
                                "las ramas del if tienen tipos distintos: {} y {}", then_ty, else_ty
                            )));
                        }
                        Ok(then_ty)
                    }
                }
            }
            // El tipo esperado no aporta a las demás formas: chequeo normal.
            _ => self.check_expr(expr),
        }
    }

    /// Coerciona una expresión a `dyn Trait` (M9.3b): un valor concreto que implemente el
    /// trait se envuelve (en el lowering) en el struct sintetizado del trait object. Si ya
    /// es un `dyn Trait` del mismo trait, no hay coerción. Registra el sitio y devuelve
    /// `dyn Trait` como tipo.
    fn coerce_to_dyn(&mut self, expr: &Expr, traits: &[String], line: usize, col: usize) -> Result<Type, TypeError> {
        let actual = self.check_expr(expr)?;
        // El origen ya es un trait object: misma identidad (nada que hacer) o **upcasting** a un
        // subconjunto (M9.5b: olvidar traits, `dyn S1` → `dyn S2` con S2 ⊆ S1).
        if let Type::Dyn(source) = &actual {
            if source.as_slice() == traits {
                return Ok(actual);
            }
            if traits.iter().all(|t| source.contains(t)) {
                self.dyn_upcasts.insert((line, col), traits.to_vec());
                return Ok(Type::Dyn(traits.to_vec()));
            }
            return Err(self.err(line, col, format!(
                "no se puede convertir 'dyn {}' en 'dyn {}': solo se puede upcastear a un subconjunto de traits",
                source.join(" + "), traits.join(" + ")
            )));
        }
        let key = type_key_of(&actual).ok_or_else(|| self.err(line, col, format!(
            "no se puede convertir {} en 'dyn {}'", actual, traits.join(" + ")
        )))?;
        // El tipo concreto debe implementar **todos** los traits del conjunto.
        for tr in traits {
            if !self.impl_traits.contains(&(key.clone(), tr.clone())) {
                return Err(self.err(line, col, format!(
                    "{} no implementa '{}': no puede usarse como 'dyn {}'", actual, tr, traits.join(" + ")
                )));
            }
        }
        // La vtable: un valor función por método de la **unión** (orden canónico: traits del
        // conjunto, métodos en orden de declaración). `dict_for` elige el método manglado plano
        // (impl concreto/genérico sin bounds) o un closure anidado (impl genérico acotado, M9.4).
        let mut vtable = Vec::new();
        for tr in traits {
            let methods = self.traits.get(tr).cloned().unwrap_or_default();
            for m in &methods {
                vtable.push(self.dict_for(&actual, tr, &m.name, line, col)?);
            }
        }
        self.dyn_coercions.insert((line, col), (traits.to_vec(), vtable));
        Ok(Type::Dyn(traits.to_vec()))
    }

    /// Como `check_block`, pero el valor final (la *tail*) se verifica con un tipo
    /// esperado, que se propaga al `match`/`if` que sea esa expresión final.
    fn check_block_expected(&mut self, block: &Block, expected: &Type) -> Result<Type, TypeError> {
        self.push_scope();
        let mut err = None;
        for stmt in &block.statements {
            if let Err(e) = self.check_stmt(stmt) {
                err = Some(e);
                break;
            }
        }
        let result = match err {
            Some(e) => Err(e),
            None => match &block.tail {
                Some(e) => self.check_expr_expected(e, expected),
                None => Ok(Type::Unit),
            },
        };
        self.pop_scope();
        result
    }

    fn check_expr(&mut self, expr: &Expr) -> Result<Type, TypeError> {
        match &expr.kind {
            ExprKind::Int(_) => Ok(Type::Int),
            ExprKind::Float(_) => Ok(Type::Float),
            ExprKind::Bool(_) => Ok(Type::Bool),
            ExprKind::Str(_) => Ok(Type::String),
            ExprKind::Char(_) => Ok(Type::Char),
            ExprKind::Bytes(_) => Ok(Type::Bytes),

            ExprKind::Ident(name) => {
                // Una variable tapa a una función con el mismo nombre.
                if let Some(v) = self.lookup(name) {
                    let ty = v.ty.clone();
                    let def = v.def;
                    self.record_ident(expr.line, expr.col, name, &ty, Some(def)); // M10.2b
                    return Ok(ty);
                }
                // Un nombre de función de nivel superior es un valor de primera
                // clase: su tipo es el tipo función correspondiente (M4.1). Una
                // función **genérica** no puede tomarse como valor (su tipo no es un
                // `fn(...)` concreto): hay que llamarla directamente (M6.1).
                // M27.5: una constante de nivel superior (global) resuelve a su tipo.
                if let Some(ty) = self.consts.get(name) {
                    return Ok(ty.clone());
                }
                if let Some(sig) = self.functions.get(name) {
                    if !sig.type_params.is_empty() {
                        return Err(self.err(expr.line, expr.col, format!(
                            "no se puede usar la función genérica '{}' como valor; llámala directamente", name
                        )));
                    }
                    let ty = Type::Fn(sig.params.clone(), Box::new(sig.ret.clone()));
                    let def = self.fn_defs.get(name).copied();
                    self.record_ident(expr.line, expr.col, name, &ty, def); // M10.2b
                    return Ok(ty);
                }
                Err(self.err(expr.line, expr.col, format!("nombre '{}' no declarado", name)))
            }

            ExprKind::Unary { op, expr: inner } => {
                let t = self.check_expr(inner)?;
                match op {
                    UnaryOp::Neg if t == Type::Int || t == Type::Float => Ok(t),
                    // M28.1: sobrecarga de `-` unario vía el trait `Neg`. Si el tipo lo implementa,
                    // `-x` baja a `x.neg()`.
                    UnaryOp::Neg => match type_key_of(&t) {
                        Some(key) if self.impl_traits.contains(&(key.clone(), "Neg".to_string())) => {
                            self.op_sites.insert((expr.line, expr.col, "Neg".to_string()), mangle(&key, "neg"));
                            Ok(t)
                        }
                        _ => Err(self.err(expr.line, expr.col, format!("no se puede negar (-) un {}", t))),
                    },
                    UnaryOp::Not if t == Type::Bool => Ok(Type::Bool),
                    UnaryOp::Not => Err(self.err(expr.line, expr.col, format!("el '!' requiere bool, no {}", t))),
                    // M19.3a: NOT bit a bit, int → int. M28.3: también sobre uint (mismo ancho).
                    UnaryOp::BitNot if t == Type::Int => Ok(Type::Int),
                    UnaryOp::BitNot if matches!(t, Type::UInt(_)) => Ok(t),
                    UnaryOp::BitNot => Err(self.err(expr.line, expr.col, format!("el '~' requiere int, no {}", t))),
                }
            }

            ExprKind::Binary { op, left, right } => self.check_binary(*op, left, right, expr.line, expr.col),

            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.line, expr.col),

            ExprKind::ArrayLit(elems) => {
                if elems.is_empty() {
                    return Err(self.err(expr.line, expr.col,
                        "no se puede inferir el tipo de [] aquí; anótalo (p. ej. let xs: [int] = [];)".into()));
                }
                let first = self.check_expr(&elems[0])?;
                for e in &elems[1..] {
                    let t = self.check_expr(e)?;
                    if t != first {
                        return Err(self.err(e.line, e.col, format!(
                            "los elementos del arreglo deben ser del mismo tipo: {} y {}", first, t
                        )));
                    }
                }
                Ok(Type::Array(Box::new(first)))
            }

            ExprKind::TupleLit(elems) => {
                // Una tupla `(a, b, …)` (M27.1): tipos heterogéneos, aridad ≥ 2. En runtime es un arreglo.
                let mut tys = Vec::with_capacity(elems.len());
                for e in elems {
                    tys.push(self.check_expr(e)?);
                }
                Ok(Type::Tuple(tys))
            }

            ExprKind::Index { array, index } => self.check_index(array, index),

            ExprKind::Cast { expr: inner, ty } => {
                // M27.4: conversión numérica. Permitidas: int↔float, char↔int (e identidad).
                // M28.3: int↔uint, uint↔uint (cualquier ancho), float↔uint, char→uint.
                let from = self.check_expr(inner)?;
                self.ensure_type(ty, expr.line, expr.col)?;
                let to = self.resolve_type(ty);
                let ok = matches!(
                    (&from, &to),
                    (Type::Int, Type::Float) | (Type::Float, Type::Int)
                        | (Type::Char, Type::Int) | (Type::Int, Type::Char)
                        | (Type::Int, Type::Int) | (Type::Float, Type::Float) | (Type::Char, Type::Char)
                ) || matches!(
                    (&from, &to),
                    (Type::Int, Type::UInt(_)) | (Type::UInt(_), Type::Int)
                        | (Type::UInt(_), Type::UInt(_)) | (Type::UInt(_), Type::Float)
                        | (Type::Float, Type::UInt(_)) | (Type::Char, Type::UInt(_))
                );
                if !ok {
                    return Err(self.err(expr.line, expr.col, format!(
                        "no se puede convertir {} a {} con 'as' (solo int↔float, char↔int y de/hacia u8/u32/u64)", from, to)));
                }
                Ok(to)
            }

            ExprKind::StructLit { name, fields } => self.check_struct_lit(name, fields, None, expr.line, expr.col),

            ExprKind::Field { object, name } => self.check_field(object, name),

            ExprKind::EnumLit { enum_name, variant, args } => {
                self.check_enum_lit(enum_name, variant, args, None, expr.line, expr.col)
            }

            ExprKind::Match { scrutinee, arms } => self.check_match(scrutinee, arms, None, expr.line, expr.col),

            ExprKind::Try(inner) => self.check_try(inner, expr.line, expr.col),

            ExprKind::Func(fe) => {
                for p in &fe.params {
                    self.ensure_type(&p.ty, p.line, p.col)?;
                }
                self.ensure_type(&fe.return_type, fe.line, fe.col)?;

                // M4.2: con captura. El cuerpo se verifica con los ámbitos
                // envolventes VISIBLES (los parámetros se apilan encima), así que
                // puede referenciar variables externas — una closure. La
                // mutabilidad se respeta (capturar no reata: asignar a un `let`
                // capturado sigue siendo error). Solo guardamos/restauramos el tipo
                // de retorno, que cambia al de esta función.
                let saved_ret = self.current_return.clone();
                let r = self.check_fn_body(&fe.params, &fe.return_type, &fe.body, fe.line, fe.col, "la función anónima");
                self.current_return = saved_ret;
                r?;

                Ok(Type::Fn(
                    fe.params.iter().map(|p| self.resolve_type(&p.ty)).collect(),
                    Box::new(self.resolve_type(&fe.return_type)),
                ))
            }

            ExprKind::If { cond, then_branch, else_branch } => {
                let ct = self.check_expr(cond)?;
                if ct != Type::Bool {
                    return Err(self.err(cond.line, cond.col, format!("la condición del if debe ser bool, no {}", ct)));
                }
                let then_ty = self.check_block(then_branch)?;
                match else_branch {
                    None => {
                        // Un if sin else tiene tipo unit; entonces la rama 'then'
                        // tampoco puede producir un valor útil.
                        if then_ty != Type::Unit {
                            return Err(self.err(expr.line, expr.col, format!(
                                "un if sin else tiene tipo unit, pero su rama produce {} (añade un else)",
                                then_ty
                            )));
                        }
                        Ok(Type::Unit)
                    }
                    Some(else_e) => {
                        let else_ty = self.check_expr(else_e)?;
                        // M13.2a: si una rama diverge (p.ej. termina en `panic`), el if toma el
                        // tipo de la otra; solo la rama que sí produce valor manda.
                        if block_diverges(then_branch) {
                            return Ok(else_ty);
                        }
                        if expr_diverges(else_e) {
                            return Ok(then_ty);
                        }
                        if then_ty != else_ty {
                            return Err(self.err(expr.line, expr.col, format!(
                                "las ramas del if tienen tipos distintos: {} y {}",
                                then_ty, else_ty
                            )));
                        }
                        Ok(then_ty)
                    }
                }
            }

            ExprKind::While { cond, body } => {
                let ct = self.check_expr(cond)?;
                if ct != Type::Bool {
                    return Err(self.err(cond.line, cond.col, format!("la condición del while debe ser bool, no {}", ct)));
                }
                // El valor del cuerpo se descarta en cada iteración; el while es unit.
                self.check_block(body)?;
                Ok(Type::Unit)
            }

            ExprKind::Block(b) => self.check_block(b),
        }
    }

    /// Verifica un bloque en su propio ámbito y devuelve su tipo-valor (el de la
    /// expresión final, o unit si no hay).
    fn check_block(&mut self, block: &Block) -> Result<Type, TypeError> {
        self.push_scope();
        for stmt in &block.statements {
            self.check_stmt(stmt)?;
        }
        let ty = match &block.tail {
            Some(e) => self.check_expr(e)?,
            None => Type::Unit,
        };
        self.pop_scope();
        Ok(ty)
    }

    fn check_binary(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        line: usize,
        col: usize,
    ) -> Result<Type, TypeError> {
        let lt = self.check_expr(left)?;
        let rt = self.check_expr(right)?;
        // M28.3b: si un operando es uint y el otro un literal entero, el literal adopta el ancho
        // (`x + 100` con `x: u8` trata `100` como u8). No es promoción: solo cede el LITERAL.
        let (lt, rt) = self.coerce_uint_binop(left, right, lt, rt)?;
        use BinaryOp::*;
        match op {
            // Aritméticos: ambos int → int, ambos float → float. Sin mezclas.
            // M11.1a: `+` también concatena dos strings → string.
            Add | Sub | Mul | Div | Rem => match (&lt, &rt) {
                (Type::Int, Type::Int) => Ok(Type::Int),
                (Type::Float, Type::Float) => Ok(Type::Float),
                (Type::String, Type::String) if op == Add => Ok(Type::String),
                // M16.1b: `+` concatena dos bytes → bytes.
                (Type::Bytes, Type::Bytes) if op == Add => Ok(Type::Bytes),
                // M11.7b: `+` concatena dos arreglos del mismo tipo de elemento → arreglo.
                (Type::Array(a), Type::Array(b)) if op == Add && a == b => Ok(Type::Array(a.clone())),
                // M28.3: enteros sin signo con tamaño — ambos del MISMO ancho → ese ancho (wrapping
                // en runtime). Sin promoción implícita: mezclar u8+u32 o u8+int es error (usa `as`).
                (Type::UInt(a), Type::UInt(b)) if a == b => Ok(Type::UInt(*a)),
                // M28.1: sobrecarga de operadores. Si ambos operandos son el mismo tipo de usuario
                // que implementa el trait del operador (`Add`/`Sub`/…), `a op b` baja a `a.metodo(b)`.
                _ => match self.try_operator_overload(op, &lt, &rt, line, col) {
                    Some(res) => res,
                    None => Err(self.err(line, col, format!(
                        "el operador '{}' requiere ambos operandos int o ambos float, no {} y {}",
                        bin_op_str(op), lt, rt
                    ))),
                },
            },
            // Orden: números, string o char, del mismo tipo → bool. M11.7d: string (lexicográfico)
            // y char (por code point) se ordenan, lo que habilita `sort` / el trait `Ord`.
            Lt | Le | Gt | Ge => match (&lt, &rt) {
                (Type::Int, Type::Int) | (Type::Float, Type::Float)
                | (Type::String, Type::String) | (Type::Char, Type::Char) => Ok(Type::Bool),
                // M28.3: enteros sin signo del mismo ancho se ordenan (comparación sin signo).
                (Type::UInt(a), Type::UInt(b)) if a == b => Ok(Type::Bool),
                _ => Err(self.err(line, col, format!(
                    "el operador '{}' compara int/float/string/char del mismo tipo, no {} y {}",
                    bin_op_str(op), lt, rt
                ))),
            },
            // Igualdad: mismo tipo y comparable → bool.
            Eq | Ne => {
                if lt == rt && is_comparable(&lt) {
                    Ok(Type::Bool)
                } else {
                    Err(self.err(line, col, format!(
                        "el operador '{}' requiere ambos operandos del mismo tipo comparable, no {} y {}",
                        bin_op_str(op), lt, rt
                    )))
                }
            }
            // Lógicos: ambos bool → bool.
            And | Or => {
                if lt == Type::Bool && rt == Type::Bool {
                    Ok(Type::Bool)
                } else {
                    Err(self.err(line, col, format!(
                        "el operador '{}' requiere operandos bool, no {} y {}",
                        bin_op_str(op), lt, rt
                    )))
                }
            }
            // Bit a bit (M19.3a): ambos operandos int → int. Sin float (los desplazamientos
            // y máscaras no tienen sentido sobre IEEE-754); el runtime opera sobre i64.
            BitAnd | BitOr | BitXor | Shl | Shr => match (&lt, &rt) {
                (Type::Int, Type::Int) => Ok(Type::Int),
                // M28.3: bit a bit sobre enteros sin signo del mismo ancho → ese ancho.
                (Type::UInt(a), Type::UInt(b)) if a == b => Ok(Type::UInt(*a)),
                _ => Err(self.err(line, col, format!(
                    "el operador '{}' requiere operandos int, no {} y {}",
                    bin_op_str(op), lt, rt
                ))),
            },
        }
    }

    /// M28.1: intenta resolver `a op b` como una sobrecarga de operador. Devuelve `Some(...)` si
    /// ambos operandos son el mismo tipo de usuario que implementa el trait del operador
    /// (`Add`/`Sub`/`Mul`/`Div`), registrando el sitio para bajar `a op b` a `a.metodo(b)`; `None`
    /// si el operador no es sobrecargable o el tipo no lo implementa (→ error built-in).
    fn try_operator_overload(
        &mut self,
        op: BinaryOp,
        lt: &Type,
        rt: &Type,
        line: usize,
        col: usize,
    ) -> Option<Result<Type, TypeError>> {
        // El operador ha de tener un trait asociado y ambos operandos el mismo tipo.
        let (trait_name, method) = op_trait_method(op)?;
        if lt != rt {
            return None;
        }
        let key = type_key_of(lt)?;
        // Solo tipos de usuario (struct/enum); los primitivos ya los cubre el camino built-in.
        if !self.impl_traits.contains(&(key.clone(), trait_name.to_string())) {
            return None;
        }
        // El método está garantizado por la validación del impl; registra el sitio y el retorno = Self.
        let mangled = mangle(&key, method);
        self.op_sites.insert((line, col, trait_name.to_string()), mangled);
        Some(Ok(lt.clone()))
    }

    /// M28.3b: si `expr` es un literal entero que cabe en `u{w}`, lo registra para coercionarlo a
    /// ese ancho (en el lowering se envuelve en un `as u{w}`) y devuelve `Some(UInt(w))`. Si no es
    /// un literal, `None` (sin coerción). Si es un literal fuera de rango, error.
    fn coerce_uint_literal(&mut self, expr: &Expr, w: u8) -> Result<Option<Type>, TypeError> {
        if let ExprKind::Int(n) = &expr.kind {
            if !uint_literal_fits(*n, w) {
                return Err(self.err(expr.line, expr.col, format!(
                    "el literal {} no cabe en u{}", n, w)));
            }
            self.uint_literal_sites.insert((expr.line, expr.col), w);
            return Ok(Some(Type::UInt(w)));
        }
        Ok(None)
    }

    /// M28.3b: coerción de un literal entero en un operador binario donde el otro operando es uint.
    /// Devuelve los tipos (posiblemente) ajustados. No hay promoción de valores no-literales.
    fn coerce_uint_binop(&mut self, left: &Expr, right: &Expr, lt: Type, rt: Type) -> Result<(Type, Type), TypeError> {
        if let (Type::UInt(w), Type::Int) = (&lt, &rt) {
            let w = *w;
            if let Some(t) = self.coerce_uint_literal(right, w)? { return Ok((lt, t)); }
        } else if let (Type::Int, Type::UInt(w)) = (&lt, &rt) {
            let w = *w;
            if let Some(t) = self.coerce_uint_literal(left, w)? { return Ok((t, rt)); }
        }
        Ok((lt, rt))
    }

    fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        line: usize,
        col: usize,
    ) -> Result<Type, TypeError> {
        match &callee.kind {
            // Llamada directa por nombre: `f(a, b)`.
            ExprKind::Ident(n) => {
                let n = n.clone();
                // M10.2b: hover/def sobre el nombre llamado, si es una función conocida (no un
                // builtin ni una variable-función, que ya pasan por la rama de `check_expr`).
                let fn_ty = self.gather
                    .then(|| self.functions.get(&n))
                    .flatten()
                    .map(|sig| Type::Fn(sig.params.clone(), Box::new(sig.ret.clone())));
                if let Some(ty) = fn_ty {
                    let def = self.fn_defs.get(&n).copied();
                    self.record_ident(callee.line, callee.col, &n, &ty, def);
                }
                self.check_named_call(&n, args, line, col)
            }

            // UFCS (M7.1): `recv.f(args)`. Si `f` es un **campo** del struct receptor,
            // es una llamada al valor de ese campo (semántica de M3/M4); si **no**, se
            // reescribe a la función libre `f(recv, args)`. La decisión necesita el
            // tipo del receptor —por eso vive aquí y no en una pre-pasada—; el nodo se
            // baja a una llamada ordinaria tras verificar (`lower_ufcs`).
            ExprKind::Field { object, name } => {
                let recv_ty = self.check_expr(object)?;
                // M9.3b/M9.5: receptor `dyn A + B` → despacho dinámico por la vtable del objeto.
                if let Type::Dyn(traits) = &recv_ty {
                    let traits = traits.clone();
                    return self.dispatch_dyn_method(&traits, name, args, line, col);
                }
                if let Type::Struct(sname, targs) = &recv_ty {
                    if let Some(fty) = self.struct_field_type(sname, targs, name) {
                        return self.call_type(fty, args, line, col);
                    }
                }
                // M9.1: ¿es un método de trait del tipo concreto del receptor? Tiene
                // prioridad sobre la función libre (UFCS), pero no sobre un campo del
                // struct. El sitio se registra apuntando a la función **manglada**.
                let trait_method = type_key_of(&recv_ty)
                    .and_then(|key| self.methods.get(&(key, name.clone())).cloned());
                if let Some(mangled) = trait_method {
                    let mut all = Vec::with_capacity(args.len() + 1);
                    all.push((**object).clone());
                    all.extend_from_slice(args);
                    let ty = self.check_named_call(&mangled, &all, line, col)?;
                    self.ufcs_sites.insert((line, col, name.clone()), mangled);
                    return Ok(ty);
                }
                // M9.2: ¿es un método de un trait que **acota** al tipo del receptor?
                // (`x.metodo()` con `x: T` y `T: Trait` en ámbito). Se resuelve al
                // **parámetro-diccionario** y se baja como una llamada a ese valor función.
                if let Type::Var(tp) = &recv_ty {
                    let tp = tp.clone();
                    if let Some((dict_name, ret)) = self.resolve_bound_method(&tp, name, args, line, col)? {
                        self.ufcs_sites.insert((line, col, name.clone()), dict_name);
                        return Ok(ret);
                    }
                }
                self.check_ufcs(name, &recv_ty, object, args, line, col)
            }

            // El callee es una expresión de tipo función (p. ej. `(fn(x: int) -> int
            // { x })(3)` o `dame_fn()(3)`). (M4.1)
            _ => {
                let ty = self.check_expr(callee)?;
                self.call_type(ty, args, line, col)
            }
        }
    }

    /// Tipo del campo `fname` de un struct `sname` con argumentos de tipo `targs`, ya
    /// sustituido (M6). `None` si el struct no tiene ese campo. Compartido por el acceso
    /// a campo en posición de llamada y la resolución UFCS (M7.1).
    fn struct_field_type(&self, sname: &str, targs: &[Type], fname: &str) -> Option<Type> {
        let fields = self.structs.get(sname)?;
        let fty = fields.iter().find(|(n, _)| n == fname).map(|(_, t)| t.clone())?;
        let tparams = self.struct_tparams.get(sname).cloned().unwrap_or_default();
        let sigma: HashMap<String, Type> = tparams.into_iter().zip(targs.iter().cloned()).collect();
        Some(subst(&fty, &sigma))
    }

    /// Verifica una llamada UFCS `recv.f(args)` reescrita conceptualmente a
    /// `f(recv, args)` (M7.1): el receptor pasa a ser el **primer argumento**. Reusa la
    /// resolución de llamada por nombre (builtins, variable local, función libre,
    /// genéricos) y registra el sitio `(línea, columna, nombre)` para que `lower_ufcs`
    /// baje el nodo a una llamada ordinaria tras la verificación.
    fn check_ufcs(&mut self, name: &str, recv_ty: &Type, object: &Expr, args: &[Expr], line: usize, col: usize) -> Result<Type, TypeError> {
        // El destino de la función libre: el propio nombre si es llamable directamente, o —fallback—
        // el global de una función `from`-importada (UFCS cross-module, M11.3b). Si no resuelve a nada,
        // el error habla de UFCS (no es ni campo del receptor ni función), mencionando el tipo.
        let target = if self.name_is_callable(name) {
            name.to_string()
        } else if let Some(global) = self.ufcs_aliases.get(name).cloned() {
            global
        } else {
            return Err(self.err(line, col, format!(
                "no existe campo ni función '{}' aplicable a {}", name, recv_ty
            )));
        };
        let mut all_args = Vec::with_capacity(args.len() + 1);
        all_args.push(object.clone());
        all_args.extend_from_slice(args);
        let ty = self.check_named_call(&target, &all_args, line, col)?;
        // El sitio se baja a `target(recv, args)`; para una función importada, `target` es el global.
        self.ufcs_sites.insert((line, col, name.to_string()), target);
        Ok(ty)
    }

    /// ¿`name` nombra algo llamable? (builtin, variable local de función, o función de
    /// nivel superior). Lo usa UFCS para dar un error específico cuando un `recv.f(...)`
    /// no es ni campo ni función.
    fn name_is_callable(&self, name: &str) -> bool {
        crate::builtins::is_builtin(name)
            || self.lookup(name).is_some()
            || self.functions.contains_key(name)
    }

    /// Resolución de una llamada por **nombre** (`name(args)`): builtins conocidos,
    /// variable local que tape una función global, función de nivel superior (directa o
    /// genérica). Compartida por la llamada directa y por UFCS.
    fn check_named_call(&mut self, name: &str, args: &[Expr], line: usize, col: usize) -> Result<Type, TypeError> {
        // Builtins (DESIGN.md §7): su firma vive en el **registro único** (`src/builtins.rs`), no
        // dispersa aquí. Se comprueban antes que una local/función homónima (un builtin no se tapa).
        // Se tipan los argumentos por el camino normal y la regla del builtin valida y da el tipo.
        if let Some(b) = crate::builtins::lookup(name) {
            let mut arg_types = Vec::with_capacity(args.len());
            for a in args {
                arg_types.push(self.check_expr(a)?);
            }
            return match (b.check)(&arg_types) {
                Ok(t) => Ok(t),
                // El índice señala el argumento culpable (para el cursor); `None` → el sitio de llamada.
                Err((Some(i), msg)) => Err(self.err(args[i].line, args[i].col, msg)),
                Err((None, msg)) => Err(self.err(line, col, msg)),
            };
        }

        // Una variable local que guarda una función: llamada indirecta (M4.1).
        // (Tapa a una función global con el mismo nombre.)
        if let Some(v) = self.lookup(name) {
            let ty = v.ty.clone();
            return self.call_type(ty, args, line, col);
        }

        // Función de nivel superior: llamada directa.
        if let Some(sig) = self.functions.get(name) {
            let (type_params, params, ret, bounds) =
                (sig.type_params.clone(), sig.params.clone(), sig.ret.clone(), sig.bounds.clone());
            let label = format!("'{}'", name);
            if type_params.is_empty() {
                // No genérica: aridad y tipos exactos.
                return self.check_args(&params, ret, args, &label, line, col);
            }
            // Genérica: inferir los argumentos de tipo unificando con los argumentos.
            let (ret_ty, sigma) =
                self.check_generic_call(&type_params, &params, &ret, args, &label, line, col)?;
            // M9.2: si la función tiene bounds, registrar los diccionarios a pasar en este
            // sitio (verificando que cada tipo inferido cumple su bound).
            if !bounds.is_empty() {
                self.record_dict_args(name, &bounds, &sigma, line, col)?;
            }
            return Ok(ret_ty);
        }

        Err(self.err(line, col, format!("función '{}' no declarada", name)))
    }

    /// Verifica una llamada cuyo *callee* es un valor (no un nombre directo): su
    /// tipo debe ser una función, y los argumentos deben encajar con su firma.
    fn call_type(&mut self, ty: Type, args: &[Expr], line: usize, col: usize) -> Result<Type, TypeError> {
        match ty {
            Type::Fn(params, ret) => self.check_args(&params, *ret, args, "la función", line, col),
            other => Err(self.err(line, col, format!(
                "no se puede llamar un valor de tipo {} (no es una función)",
                other
            ))),
        }
    }

    /// Comprueba aridad y tipos de los argumentos contra una firma `(params -> ret)`
    /// y devuelve `ret`. Compartido por las llamadas directas y las indirectas.
    fn check_args(&mut self, params: &[Type], ret: Type, args: &[Expr], label: &str, line: usize, col: usize) -> Result<Type, TypeError> {
        if args.len() != params.len() {
            return Err(self.err(line, col, format!(
                "{} espera {} argumento(s), se le pasaron {}",
                label, params.len(), args.len()
            )));
        }
        for (i, (arg, expected)) in args.iter().zip(params.iter()).enumerate() {
            // El tipo del parámetro es el esperado del argumento (propaga a `None`,
            // `[]`, `Caja.Vacia`...).
            let at = self.check_expr_expected(arg, expected)?;
            if at != *expected {
                return Err(self.err(arg.line, arg.col, format!(
                    "argumento {} de {}: se esperaba {}, se pasó {}",
                    i + 1, label, expected, at
                )));
            }
        }
        Ok(ret)
    }

    /// Verifica una llamada a una función **genérica** (M6.1): infiere sus argumentos
    /// de tipo unificando los tipos de los parámetros con los de los argumentos, y
    /// devuelve el tipo de retorno ya sustituido. Si algún parámetro de tipo no queda
    /// determinado por los argumentos, es error (M6.1 no usa el tipo esperado).
    fn check_generic_call(
        &mut self,
        type_params: &[String],
        params: &[Type],
        ret: &Type,
        args: &[Expr],
        label: &str,
        line: usize,
        col: usize,
    ) -> Result<(Type, HashMap<String, Type>), TypeError> {
        if args.len() != params.len() {
            return Err(self.err(line, col, format!(
                "{} espera {} argumento(s), se le pasaron {}",
                label, params.len(), args.len()
            )));
        }
        // σ: parámetro de tipo → tipo concreto inferido.
        let mut sigma: HashMap<String, Type> = HashMap::new();
        for (i, (arg, param)) in args.iter().zip(params.iter()).enumerate() {
            let at = self.check_expr(arg)?;
            unify(param, &at, &mut sigma).map_err(|reason| self.err(arg.line, arg.col, format!(
                "argumento {} de {}: {}", i + 1, label, reason
            )))?;
        }
        // Todos los parámetros de tipo deben haber quedado determinados.
        for tp in type_params {
            if !sigma.contains_key(tp) {
                return Err(self.err(line, col, format!(
                    "no se pudo inferir el parámetro de tipo '{}' de {} (no aparece en los argumentos)",
                    tp, label
                )));
            }
        }
        // M9.2 devuelve también σ: el sitio de llamada lo necesita para saber a qué tipo
        // resolvió cada parámetro acotado y así elegir el diccionario a pasar.
        Ok((subst(ret, &sigma), sigma))
    }

    /// Resuelve `x.metodo(args)` con `x: T` cuando `T` está **acotado** (M9.2). Devuelve
    /// el nombre del parámetro-diccionario al que baja la llamada y el tipo de retorno (con
    /// `Self → T`). `None` si ningún trait acotado de `T` declara ese método (deja que la
    /// resolución siga su curso). Error si varios lo declaran (ambiguo).
    fn resolve_bound_method(&mut self, tp: &str, method: &str, args: &[Expr], line: usize, col: usize)
        -> Result<Option<(String, Type)>, TypeError>
    {
        let hits: Vec<String> = self.current_fn_bounds.iter()
            .filter(|(bp, _)| bp == tp)
            .filter(|(_, tr)| self.traits.get(tr).is_some_and(|ms| ms.iter().any(|m| m.name == method)))
            .map(|(_, tr)| tr.clone())
            .collect();
        if hits.is_empty() {
            return Ok(None);
        }
        if hits.len() > 1 {
            return Err(self.err(line, col, format!(
                "método '{}' ambiguo para '{}': lo declaran varios traits acotados ({})",
                method, tp, hits.join(", ")
            )));
        }
        let trait_name = &hits[0];
        let sig = self
            .traits
            .get(trait_name)
            .unwrap_or_else(|| crate::ice!("el trait '{}' del bound no está registrado", trait_name))
            .iter()
            .find(|m| m.name == method)
            .unwrap_or_else(|| crate::ice!("el método '{}' no está en el trait '{}'", method, trait_name))
            .clone();
        let self_ty = Type::Var(tp.to_string());
        // El receptor ya casó con `self` (es `T`); comprobar los argumentos restantes.
        let expected: Vec<Type> = sig.params.iter().skip(1)
            .map(|p| self.resolve_type(&subst_self(&p.ty, &self_ty)))
            .collect();
        if args.len() != expected.len() {
            return Err(self.err(line, col, format!(
                "el método '{}' espera {} argumento(s) (sin contar el receptor), se le pasaron {}",
                method, expected.len(), args.len()
            )));
        }
        for (i, (arg, exp)) in args.iter().zip(&expected).enumerate() {
            let at = self.check_expr_expected(arg, exp)?;
            if at != *exp {
                return Err(self.err(arg.line, arg.col, format!(
                    "argumento {} del método '{}': se esperaba {}, se pasó {}", i + 1, method, exp, at
                )));
            }
        }
        let ret = self.resolve_type(&subst_self(&sig.return_type, &self_ty));
        Ok(Some((dict_param_name(tp, trait_name, method), ret)))
    }

    /// Despacha `obj.metodo(args)` con `obj: dyn Trait` (M9.3b): el método se resuelve en
    /// runtime por la vtable del objeto. Verifica que el trait declara el método, que es
    /// *object-safe* (no usa `Self` fuera del receptor) y que los argumentos casan. Registra
    /// el sitio para que `lower_dyn` lo baje al bloque `{ let r = obj; (r.m)(r.data, ...) }`.
    fn dispatch_dyn_method(&mut self, traits: &[String], method: &str, args: &[Expr], line: usize, col: usize)
        -> Result<Type, TypeError>
    {
        // Busca el método entre **todos** los traits del conjunto (la unicidad ya la garantizó
        // `ensure_type`: ningún método se repite entre los traits de un `dyn A + B`).
        let sig = match traits.iter()
            .find_map(|tr| self.traits.get(tr).and_then(|ms| ms.iter().find(|m| m.name == method)))
        {
            Some(m) => m.clone(),
            None => return Err(self.err(line, col, format!(
                "'dyn {}' no declara un método '{}'", traits.join(" + "), method
            ))),
        };
        // *Object safety*: la vtable no puede llevar métodos que dependan del tipo concreto
        // borrado. Si `Self` aparece fuera del receptor (en un parámetro o en el retorno),
        // el método no es invocable sobre un trait object.
        let usa_self = sig.params.iter().skip(1).any(|p| type_uses_self(&p.ty)) || type_uses_self(&sig.return_type);
        if usa_self {
            return Err(self.err(line, col, format!(
                "el método '{}' usa 'Self': no es invocable sobre 'dyn {}'", method, traits.join(" + ")
            )));
        }
        // Argumentos (sin el receptor, que es el propio objeto).
        let expected: Vec<Type> = sig.params.iter().skip(1).map(|p| self.resolve_type(&p.ty)).collect();
        if args.len() != expected.len() {
            return Err(self.err(line, col, format!(
                "el método '{}' espera {} argumento(s) (sin contar el receptor), se le pasaron {}",
                method, expected.len(), args.len()
            )));
        }
        for (i, (arg, exp)) in args.iter().zip(&expected).enumerate() {
            let at = self.check_expr_expected(arg, exp)?;
            if at != *exp {
                return Err(self.err(arg.line, arg.col, format!(
                    "argumento {} del método '{}': se esperaba {}, se pasó {}", i + 1, method, exp, at
                )));
            }
        }
        self.dyn_dispatch.insert((line, col, method.to_string()));
        Ok(self.resolve_type(&sig.return_type))
    }

    /// Registra los diccionarios a pasar en un sitio de llamada a una función con bounds
    /// (M9.2). Por cada `(parámetro, trait)` y cada método del trait, elige el diccionario
    /// según a qué tipo resolvió el parámetro (`σ`): el método manglado de un impl concreto,
    /// o el reenvío del diccionario propio si resolvió a un parámetro acotado del llamador.
    fn record_dict_args(&mut self, callee: &str, bounds: &[(String, String)], sigma: &HashMap<String, Type>, line: usize, col: usize)
        -> Result<(), TypeError>
    {
        let mut dicts: Vec<Expr> = Vec::new();
        for (tp, trait_name) in bounds {
            let concrete = sigma.get(tp).cloned().unwrap_or_else(|| Type::Var(tp.clone()));
            let methods = self.traits.get(trait_name).cloned().unwrap_or_default();
            for m in &methods {
                dicts.push(self.dict_for(&concrete, trait_name, &m.name, line, col)?);
            }
        }
        self.dict_calls.insert((line, col, callee.to_string()), dicts);
        Ok(())
    }

    /// Elige la **expresión-diccionario** a pasar para un método de un trait, según el tipo
    /// `concrete` al que resolvió el parámetro acotado (M9.2 / M9.2b). Tres casos:
    /// - `Var(U)` rígido del llamador con el mismo bound → **reenvía** su diccionario (`Ident`);
    /// - tipo concreto con impl **no genérico** (o genérico **sin** bounds) → el método manglado
    ///   como valor plano (`Ident`);
    /// - tipo concreto con impl **genérico acotado** (`Caja<int>`) → un **closure anidado**
    ///   (`synth_dict_closure`) que captura los diccionarios internos.
    fn dict_for(&self, concrete: &Type, trait_name: &str, method: &str, line: usize, col: usize)
        -> Result<Expr, TypeError>
    {
        if let Type::Var(u) = concrete {
            // Resolvió a un parámetro de tipo rígido del llamador: debe tener el mismo
            // bound, y se **reenvía** su diccionario.
            if self.current_fn_bounds.iter().any(|(bp, tr)| bp == u && tr == trait_name) {
                return Ok(ident_expr(&dict_param_name(u, trait_name, method), line, col));
            }
            return Err(self.err(line, col, format!(
                "el parámetro de tipo '{}' no está acotado por '{}' (requerido por la llamada)", u, trait_name
            )));
        }
        // Tipo concreto: debe implementar el trait → usar el método manglado del impl.
        let key = type_key_of(concrete).ok_or_else(|| self.err(line, col, format!(
            "{} no puede implementar el trait '{}'", concrete, trait_name
        )))?;
        if !self.impl_traits.contains(&(key.clone(), trait_name.to_string())) {
            return Err(self.err(line, col, format!(
                "{} no implementa '{}' (requerido por la llamada)", concrete, trait_name
            )));
        }
        // M9.2b: si el impl es **genérico y acotado**, su función manglada lleva sus propios
        // parámetros-diccionario, así que no se puede pasar plana: hay que envolverla en un
        // **closure** que rellene los diccionarios internos (anidados).
        let gi_acotado = self.generic_impls.get(&(key.clone(), trait_name.to_string()))
            .filter(|gi| !gi.bounds.is_empty())
            .cloned();
        if let Some(gi) = gi_acotado {
            let sig = self.traits.get(trait_name)
                .and_then(|ms| ms.iter().find(|m| m.name == method))
                .cloned()
                .unwrap_or_else(|| crate::ice!("el método no pertenece al trait (el impl se validó)"));
            return self.synth_dict_closure(&gi, &key, &sig, concrete, line, col);
        }
        // Impl no genérico, o genérico **sin** bounds: la función manglada tiene la aridad
        // justa (solo el receptor + params), así que se pasa como valor plano.
        Ok(ident_expr(&mangle(&key, method), line, col))
    }

    /// Sintetiza el **diccionario anidado** (M9.2b) de un método de un impl genérico acotado:
    /// un closure que adapta la aridad. `Caja#mostrar` espera `(self, dicts_internos...)`, pero
    /// el llamador lo invocará con solo el receptor; el closure captura los diccionarios
    /// internos y los rellena:
    ///
    /// ```text
    /// fn(__d0: Caja<int>) -> string { Caja#mostrar(__d0, int#mostrar) }
    /// ```
    ///
    /// El `id` del fn-expr es provisional (0): `renumber_fn_exprs`, al final del lowering, le da
    /// uno denso. Reusa closures (M4): cero cambios de runtime.
    fn synth_dict_closure(&self, gi: &GenImpl, key: &str, sig: &MethodSig, concrete: &Type, line: usize, col: usize)
        -> Result<Expr, TypeError>
    {
        // σ_impl: parámetros de tipo del impl → argumentos concretos (casar `Caja<T>` con
        // `Caja<int>` → T=int).
        let mut sigma_impl: HashMap<String, Type> = HashMap::new();
        let _ = unify(&gi.target, concrete, &mut sigma_impl);
        // Parámetros del closure: `self: concrete`, luego el resto del método (Self→concrete).
        let mut params = Vec::new();
        let mut fwd_args = Vec::new();
        for (i, p) in sig.params.iter().enumerate() {
            let name = format!("__d{}", i);
            let ty = if i == 0 { concrete.clone() } else { subst_self(&p.ty, concrete) };
            params.push(Param { name: name.clone(), ty, line, col });
            fwd_args.push(ident_expr(&name, line, col));
        }
        // Diccionarios internos: en el MISMO orden que `append_dict_params` los añadió a la
        // función manglada (bounds del impl en orden; por bound, los métodos del trait en orden).
        let mut inner = Vec::new();
        for (tp, bound_trait) in &gi.bounds {
            let inner_concrete = sigma_impl.get(tp).cloned().unwrap_or_else(|| Type::Var(tp.clone()));
            let bmethods = self.traits.get(bound_trait).cloned().unwrap_or_default();
            for bm in &bmethods {
                inner.push(self.dict_for(&inner_concrete, bound_trait, &bm.name, line, col)?);
            }
        }
        // Cuerpo: `Caja#metodo(self, params..., dicts_internos...)`.
        let mut call_args = fwd_args;
        call_args.extend(inner);
        let call = Expr {
            kind: ExprKind::Call {
                callee: Box::new(ident_expr(&mangle(key, &sig.name), line, col)),
                args: call_args,
            },
            line, col,
        };
        let body = Block { statements: Vec::new(), tail: Some(Box::new(call)), line, col };
        let fe = FnExpr { id: 0, params, return_type: subst_self(&sig.return_type, concrete), body, line, col };
        Ok(Expr { kind: ExprKind::Func(Box::new(fe)), line, col })
    }

    // ----- Manejo de ámbitos -----

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Declara una variable en el ámbito más interno (permite shadowing del exterior).
    /// `def` es la posición de su declaración (M10.2b: ir-a-definición).
    fn declare(&mut self, name: &str, ty: Type, mutable: bool, def: (usize, usize)) {
        self.scopes
            .last_mut()
            .unwrap_or_else(|| crate::ice!("no hay ámbito activo al declarar una variable"))
            .insert(name.to_string(), VarInfo { ty, mutable, def });
    }

    /// Registra el índice semántico de un uso de identificador (M10.2b): su tipo (hover) y, si
    /// se conoce, la posición de su declaración (ir-a-definición). No hace nada salvo en modo
    /// `gather`, así que la verificación normal no paga nada.
    fn record_ident(&mut self, line: usize, col: usize, name: &str, ty: &Type, def: Option<(usize, usize)>) {
        if !self.gather {
            return;
        }
        let len = name.chars().count();
        self.index.hovers.push(HoverEntry { line, col, len, text: format!("{}: {}", name, ty) });
        if let Some((def_line, def_col)) = def {
            self.index.defs.push(DefEntry { line, col, len, def_line, def_col });
        }
    }

    /// Registra el índice semántico del uso de un **nombre de tipo o método** (M10.2f): el texto a
    /// mostrar en hover y, si se conoce, la posición de su declaración (ir-a-definición). Como
    /// `record_ident`, no hace nada salvo en modo `gather`.
    fn record_named(&mut self, line: usize, col: usize, len: usize, text: String, def: Option<(usize, usize)>) {
        if !self.gather {
            return;
        }
        self.index.hovers.push(HoverEntry { line, col, len, text });
        if let Some((def_line, def_col)) = def {
            self.index.defs.push(DefEntry { line, col, len, def_line, def_col });
        }
    }

    /// Busca una variable de dentro hacia afuera.
    fn lookup(&self, name: &str) -> Option<&VarInfo> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    /// Construye un `TypeError`. La extensión (M33a-2) sale de la tabla de spans del
    /// parser: hit en la misma línea → la expresión completa; hit multilínea → el
    /// sentinela `usize::MAX` ("hasta el fin de línea"; los renderizadores acotan);
    /// miss (expresión sintetizada por el lowering, prelude) → 1.
    fn err(&self, line: usize, col: usize, msg: String) -> TypeError {
        let len = match self.expr_spans.get(&(line, col)) {
            Some(&(el, ec)) if el == line && ec > col => ec - col,
            Some(_) => usize::MAX, // expresión multilínea
            None => 1,
        };
        TypeError { msg, line, col, len }
    }
}

// ----- Auxiliares libres -----

/// ¿Pueden compararse con == / != valores de este tipo? (Compuestos: estructural.)
/// Las funciones **no** son comparables (no tienen identidad estructural); un
/// arreglo lo es solo si su elemento lo es.
/// ¿Es `t` un tipo válido como **clave** de un `Map` (M13.1)? Primitivos hashables
/// (int/string/char/bool/**bytes**; **no** float — no es hashable de forma fiable), o un parámetro de
/// tipo genérico (la restricción real se comprueba al instanciarlo con un tipo concreto).
fn is_hashable_key(t: &Type) -> bool {
    // `bytes` (diferido de M16): secuencia inmutable de octetos → Hash/Eq/Ord fiables, como un string.
    matches!(t, Type::Int | Type::String | Type::Char | Type::Bool | Type::Bytes | Type::Var(_))
}

/// ¿Es `e` un valor válido para una constante (M27.5)? Un literal, o un literal numérico negado (`-5`).
fn is_const_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_)
        | ExprKind::Char(_) | ExprKind::Bytes(_) => true,
        ExprKind::Unary { op: UnaryOp::Neg, expr } => {
            matches!(expr.kind, ExprKind::Int(_) | ExprKind::Float(_))
        }
        _ => false,
    }
}

fn is_comparable(t: &Type) -> bool {
    match t {
        // M16.1a: `bytes` se compara con `==` (igualdad estructural de octetos).
        // M28.3: los enteros sin signo con tamaño se comparan con `==`.
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Bytes | Type::UInt(_) | Type::Struct(_, _) => true,
        Type::Array(elem) => is_comparable(elem),
        // M27.1: una tupla es comparable con == si todos sus elementos lo son (igualdad posición a posición).
        Type::Tuple(ts) => ts.iter().all(is_comparable),
        // Un Map (M13.1) no se compara con == por ahora (como los enums); se consulta.
        Type::Map(_, _) => false,
        // Los enums (M5) no se comparan con ==: pueden ser recursivos y portar
        // funciones; se consumen por `match`. (Un `@derive(Eq)` futuro lo abriría.)
        // Un parámetro de tipo (M6) es opaco: podría ser una función o un enum, así
        // que no se puede comparar dentro de código genérico.
        // `Self` (M9) no debería llegar aquí (se sustituye por el tipo concreto), pero
        // como tipo abstracto no es comparable. Un trait object (M9.3b) tampoco.
        // Un canal (M12.1) no se compara con == (se comunica, no se inspecciona). Una Task (M12.3) tampoco.
        Type::Unit | Type::Fn(_, _) | Type::Enum(_, _) | Type::Var(_) | Type::SelfType | Type::Dyn(_) | Type::Channel(_) | Type::Task(_) => false,
    }
}

/// ¿El tipo contiene algún parámetro de tipo `Var` sin resolver? (M6.2: si lo tiene,
/// no sirve como tipo "esperado" concreto.)
fn type_has_var(t: &Type) -> bool {
    match t {
        Type::Var(_) => true,
        Type::Array(e) => type_has_var(e),
        Type::Map(k, v) => type_has_var(k) || type_has_var(v),
        Type::Channel(t) => type_has_var(t),
        Type::Task(t) => type_has_var(t),
        Type::Fn(ps, r) => ps.iter().any(type_has_var) || type_has_var(r),
        Type::Struct(_, args) | Type::Enum(_, args) => args.iter().any(type_has_var),
        _ => false,
    }
}

/// Siembra `σ` a partir del tipo esperado (M6.2): si se espera `Nombre<a, b, ...>` con
/// la aridad correcta, liga cada parámetro de tipo con su argumento esperado. Así
/// `Caja.Vacia` con tipo esperado `Caja<int>` fija `T = int`.
fn seed_sigma_from_expected(expected: Option<&Type>, name: &str, tparams: &[String]) -> HashMap<String, Type> {
    let mut sigma = HashMap::new();
    if let Some(Type::Struct(en, eargs) | Type::Enum(en, eargs)) = expected {
        if en == name && eargs.len() == tparams.len() {
            for (tp, ea) in tparams.iter().zip(eargs) {
                sigma.insert(tp.clone(), ea.clone());
            }
        }
    }
    sigma
}

/// **Sustitución** (M6): reemplaza cada `Var(n)` por `σ[n]`, recursivamente. Es cómo
/// se instancia un tipo genérico una vez inferidos sus parámetros: `subst([U], {U↦int})
/// = [int]`.
fn subst(ty: &Type, sigma: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Var(n) => sigma.get(n).cloned().unwrap_or_else(|| ty.clone()),
        Type::Array(e) => Type::Array(Box::new(subst(e, sigma))),
        Type::Map(k, v) => Type::Map(Box::new(subst(k, sigma)), Box::new(subst(v, sigma))),
        Type::Channel(t) => Type::Channel(Box::new(subst(t, sigma))),
        Type::Task(t) => Type::Task(Box::new(subst(t, sigma))),
        Type::Fn(ps, r) => Type::Fn(
            ps.iter().map(|p| subst(p, sigma)).collect(),
            Box::new(subst(r, sigma)),
        ),
        // Tipos nominales: sustituir sus argumentos de tipo (M6.2).
        Type::Struct(n, args) => Type::Struct(n.clone(), args.iter().map(|a| subst(a, sigma)).collect()),
        Type::Enum(n, args) => Type::Enum(n.clone(), args.iter().map(|a| subst(a, sigma)).collect()),
        // Primitivos: nada que sustituir.
        other => other.clone(),
    }
}

/// **Unificación** (M6), asimétrica: `param` viene de la firma de la función llamada
/// (sus `Var` son las **incógnitas** a inferir); `arg` viene del contexto del llamador
/// (sus `Var`, si los hay, son **rígidos**/opacos). Liga las incógnitas en `σ` y exige
/// consistencia; cualquier desacuerdo es un error con su razón.
fn unify(param: &Type, arg: &Type, sigma: &mut HashMap<String, Type>) -> Result<(), String> {
    // Incógnita del lado de la firma: ligarla (o exigir que coincida con lo ya ligado).
    if let Type::Var(n) = param {
        if let Some(prev) = sigma.get(n) {
            if prev != arg {
                return Err(format!("'{}' no puede ser {} y {} a la vez", n, prev, arg));
            }
        } else {
            sigma.insert(n.clone(), arg.clone());
        }
        return Ok(());
    }
    match (param, arg) {
        (Type::Array(a), Type::Array(b)) => unify(a, b, sigma),
        (Type::Map(k1, v1), Type::Map(k2, v2)) => {
            unify(k1, k2, sigma)?;
            unify(v1, v2, sigma)
        }
        (Type::Channel(a), Type::Channel(b)) => unify(a, b, sigma),
        (Type::Task(a), Type::Task(b)) => unify(a, b, sigma),
        (Type::Fn(p1, r1), Type::Fn(p2, r2)) => {
            if p1.len() != p2.len() {
                return Err(format!("se esperaba {}, se pasó {}", param, arg));
            }
            for (a, b) in p1.iter().zip(p2) {
                unify(a, b, sigma)?;
            }
            unify(r1, r2, sigma)
        }
        // Tipos nominales: mismo nombre y unificar sus argumentos de tipo (M6.2), p.
        // ej. `Caja<T>` contra `Caja<int>` liga `T = int`.
        (Type::Struct(n1, a1), Type::Struct(n2, a2)) | (Type::Enum(n1, a1), Type::Enum(n2, a2))
            if n1 == n2 && a1.len() == a2.len() =>
        {
            for (a, b) in a1.iter().zip(a2) {
                unify(a, b, sigma)?;
            }
            Ok(())
        }
        // Resto (primitivos, Var rígido del llamador): igualdad exacta.
        _ if param == arg => Ok(()),
        _ => Err(format!("se esperaba {}, se pasó {}", param, arg)),
    }
}

fn bin_op_str(op: BinaryOp) -> &'static str {
    use BinaryOp::*;
    match op {
        Add => "+", Sub => "-", Mul => "*", Div => "/", Rem => "%",
        Eq => "==", Ne => "!=", Lt => "<", Le => "<=", Gt => ">", Ge => ">=",
        And => "&&", Or => "||",
        BitAnd => "&", BitOr => "|", BitXor => "^", Shl => "<<", Shr => ">>",
    }
}

/// Análisis de divergencia: ¿todos los caminos de este bloque terminan en `return`?
/// Es una aproximación *conservadora* (sólida): si dice `true`, es seguro que el
/// bloque siempre retorna; si dice `false`, puede que sí o que no. Eso basta para
/// permitir omitir la expresión final cuando el cuerpo ya retorna por todas partes.
fn block_diverges(block: &Block) -> bool {
    block.statements.iter().any(stmt_diverges)
        || block.tail.as_ref().is_some_and(|t| expr_diverges(t))
}

fn stmt_diverges(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Return { .. } => true,
        StmtKind::Expr(e) => expr_diverges(e),
        _ => false,
    }
}

fn expr_diverges(expr: &Expr) -> bool {
    match &expr.kind {
        // Un if diverge solo si AMBAS ramas divergen (si falta el else, puede caer).
        ExprKind::If { then_branch, else_branch: Some(els), .. } => {
            block_diverges(then_branch) && expr_diverges(els)
        }
        ExprKind::Block(b) => block_diverges(b),
        // Un match diverge si TODOS sus brazos divergen (el checker garantiza que es
        // exhaustivo, así que siempre se toma alguno).
        ExprKind::Match { arms, .. } => !arms.is_empty() && arms.iter().all(|a| expr_diverges(&a.body)),
        // `panic(...)` (M13.2a) nunca retorna: una rama que termina en panic diverge, así que
        // `match (x) { Some(v) => v, None => panic("imposible") }` cuadra de tipo. `panic` gana
        // siempre sobre cualquier homónimo (un builtin no se tapa), así que el chequeo por nombre
        // es seguro.
        ExprKind::Call { callee, .. } => matches!(&callee.kind, ExprKind::Ident(n) if n == "panic"),
        _ => false,
    }
}

// =====================================================================
// Resolución de la construcción de enums (M5)
// =====================================================================
//
// `Enum.Variante(args)` y `obj.campo` comparten forma sintáctica, así que el parser
// no puede distinguirlos. Conocidos los nombres de enum, estas funciones recorren el
// AST y **reescriben** los `Field`/`Call` cuya cabeza es un enum en nodos `EnumLit`.
// Se ejecuta una vez, antes de verificar; los dos motores reciben el AST resuelto.

fn resolve_block(block: &mut Block, enums: &HashSet<String>) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => resolve_expr(value, enums),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { resolve_expr(start, enums); resolve_expr(end, enums); }
                    ForIter::In(e) => resolve_expr(e, enums),
                }
                resolve_block(body, enums);
            }
            StmtKind::Assign { target, value } => {
                resolve_expr(target, enums);
                resolve_expr(value, enums);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    resolve_expr(v, enums);
                }
            }
            StmtKind::Expr(e) => resolve_expr(e, enums),
        }
    }
    if let Some(t) = &mut block.tail {
        resolve_expr(t, enums);
    }
}

fn resolve_expr(expr: &mut Expr, enums: &HashSet<String>) {
    // Detectar la construcción de enum ANTES de recorrer los hijos. Si no, el `Field`
    // de la cabeza (`Enum.Variante`) se reescribiría como variante *nullary* antes de
    // que el `Call` que lo envuelve lo viera, perdiendo el payload.

    // Caso 1: `Enum.Variante(args)` — un Call cuyo callee es un Field con cabeza enum.
    if let ExprKind::Call { callee, args } = &mut expr.kind {
        if let ExprKind::Field { object, name } = &callee.kind {
            if is_enum_head(object, enums) {
                let enum_name = ident_name(object);
                let variant = name.clone();
                let mut args = std::mem::take(args);
                for a in &mut args {
                    resolve_expr(a, enums); // el payload sí se resuelve
                }
                expr.kind = ExprKind::EnumLit { enum_name, variant, args };
                return;
            }
        }
    }
    // Caso 2: `Enum.Variante` sin payload — un Field con cabeza enum.
    if let ExprKind::Field { object, name } = &expr.kind {
        if is_enum_head(object, enums) {
            expr.kind = ExprKind::EnumLit {
                enum_name: ident_name(object),
                variant: name.clone(),
                args: Vec::new(),
            };
            return;
        }
    }

    // Caso general: recorrer los sub-nodos.
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => resolve_expr(inner, enums),
        ExprKind::Binary { left, right, .. } => {
            resolve_expr(left, enums);
            resolve_expr(right, enums);
        }
        ExprKind::Call { callee, args } => {
            resolve_expr(callee, enums);
            for a in args {
                resolve_expr(a, enums);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                resolve_expr(e, enums);
            }
        }
        ExprKind::Index { array, index } => {
            resolve_expr(array, enums);
            resolve_expr(index, enums);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                resolve_expr(e, enums);
            }
        }
        ExprKind::Field { object, .. } => resolve_expr(object, enums),
        ExprKind::Func(fe) => resolve_block(&mut fe.body, enums),
        ExprKind::Match { scrutinee, arms } => {
            resolve_expr(scrutinee, enums);
            for arm in arms {
                resolve_expr(&mut arm.body, enums);
            }
        }
        ExprKind::Try(inner) => resolve_expr(inner, enums),
        ExprKind::If { cond, then_branch, else_branch } => {
            resolve_expr(cond, enums);
            resolve_block(then_branch, enums);
            if let Some(e) = else_branch {
                resolve_expr(e, enums);
            }
        }
        ExprKind::While { cond, body } => {
            resolve_expr(cond, enums);
            resolve_block(body, enums);
        }
        ExprKind::Block(b) => resolve_block(b, enums),
        // Literales, Ident, EnumLit: nada que recorrer.
        _ => {}
    }
}

/// ¿Es `expr` un identificador que nombra un enum?
fn is_enum_head(expr: &Expr, enums: &HashSet<String>) -> bool {
    matches!(&expr.kind, ExprKind::Ident(n) if enums.contains(n))
}

/// Extrae el nombre de un `ExprKind::Ident` (precondición: `is_enum_head` fue cierto).
fn ident_name(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Ident(n) => n.clone(),
        _ => crate::ice!("ident_name exige un Ident"),
    }
}

// =====================================================================
// Auxiliares de traits (M9)
// =====================================================================

/// Nombre manglado de un método de impl: `Tipo#metodo`. El `#` impide colisión con
/// cualquier nombre que el usuario pueda escribir, así el método vive como una función
/// libre más sin chocar con las suyas.
fn mangle(type_key: &str, method: &str) -> String {
    format!("{}#{}", type_key, method)
}

/// M28.2: nombre manglado de un método de conversión `From`. Incluye la **clave del origen**
/// para que `impl From<string> for E` e `impl From<int> for E` no colisionen (mismo destino
/// `E` y método `from`, distinta conversión). Nunca es invocable por el usuario (`#`).
fn mangle_from(target_key: &str, source_key: &str) -> String {
    format!("{}#from#{}", target_key, source_key)
}

/// M28.2: ¿es `imp` un impl de un trait con parámetros de tipo (estilo `From<S>`)? Estos se
/// tratan aparte: su método de conversión se inyecta con un nombre manglado por origen y no
/// entra en la tabla de despacho por punto (no tiene `self`).
fn is_typed_trait_impl(imp: &ImplBlock) -> bool {
    !imp.trait_args.is_empty()
}

/// M28.3b: ¿el operador binario produce un valor del mismo tipo que sus operandos? (Aritméticos y
/// bit a bit sí; comparación/lógicos no.) Decide si propagar el ancho uint esperado a los operandos.
fn is_width_preserving(op: BinaryOp) -> bool {
    use BinaryOp::*;
    matches!(op, Add | Sub | Mul | Div | Rem | BitAnd | BitOr | BitXor | Shl | Shr)
}

/// M28.3b: ¿cabe el literal entero `n` (siempre ≥ 0 aquí; los negativos son `-` unario) en un
/// entero sin signo de `w` bits? Para u64, cualquier i64 no negativo cabe (i64::MAX < u64::MAX).
fn uint_literal_fits(n: i64, w: u8) -> bool {
    if n < 0 { return false; }
    if w >= 64 { return true; }
    (n as u64) <= crate::runtime::uint_mask(w)
}

/// M28.1: mapa operador binario → (trait, método) para la sobrecarga. `None` si el operador
/// no es sobrecargable (`%`, comparación, lógicos, bit a bit).
fn op_trait_method(op: BinaryOp) -> Option<(&'static str, &'static str)> {
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
fn type_key_of(ty: &Type) -> Option<String> {
    Some(match ty {
        Type::Int => "int".into(),
        Type::Float => "float".into(),
        Type::Bool => "bool".into(),
        Type::String => "string".into(),
        Type::Char => "char".into(),
        Type::Struct(n, _) | Type::Enum(n, _) => n.clone(),
        _ => return None,
    })
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
    // Pares (trait, nombre-de-tipo) ya implementados, para no regenerar.
    let mut existentes: HashSet<(String, String)> = program
        .impls
        .iter()
        .filter_map(|i| impl_target_name(&i.target).map(|n| (i.trait_name.clone(), n.to_string())))
        .collect();
    let mut nuevos: Vec<ImplBlock> = Vec::new();
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
                    "Eq" => nuevos.push(parse_derived_impl("Eq", &s.name, "fn igual(self, otro: Self) -> bool", &struct_eq_body(&s.fields))),
                    "Show" => nuevos.push(parse_derived_impl("Show", &s.name, "fn mostrar(self) -> string", &struct_show_body(a, &s.name, &s.fields)?)),
                    _ => crate::ice!("validate_derive garantiza un trait conocido"),
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
                    "Eq" => nuevos.push(parse_derived_impl("Eq", &e.name, "fn igual(self, otro: Self) -> bool", &enum_eq_body(&e.name, &e.variants))),
                    "Show" => nuevos.push(parse_derived_impl("Show", &e.name, "fn mostrar(self) -> string", &enum_show_body(a, &e.name, &e.variants)?)),
                    _ => crate::ice!("validate_derive garantiza un trait conocido"),
                }
            }
        }
    }
    program.impls.extend(nuevos);
    Ok(())
}

/// El nombre del tipo objetivo de un `impl` (`Struct`/`Enum`), si lo tiene.
fn impl_target_name(t: &Type) -> Option<&str> {
    match t {
        Type::Struct(n, _) | Type::Enum(n, _) => Some(n),
        _ => None,
    }
}

/// Valida `@derive(...)` sobre un tipo: argumentos no vacíos, todos derivables (`Eq`/`Show`),
/// y el tipo no genérico (M9.1 no admite impls genéricos).
fn validate_derive(a: &Annotation, name: &str, type_params: &[String]) -> Result<(), TypeError> {
    if a.args.is_empty() {
        return Err(TypeError { msg: "'@derive' requiere al menos un trait (p. ej. @derive(Eq))".into(), line: a.line, col: a.col, len: 1 });
    }
    for arg in &a.args {
        if arg != "Eq" && arg != "Show" {
            return Err(TypeError { msg: format!("no se sabe derivar '{}' (por ahora Eq y Show)", arg), line: a.line, col: a.col, len: 1 });
        }
    }
    if !type_params.is_empty() {
        return Err(TypeError { msg: format!("no se puede derivar para el tipo genérico '{}'", name), line: a.line, col: a.col, len: 1 });
    }
    Ok(())
}

/// Cómo renderizar un valor a string según su tipo (M11.2/L2): primitivos vía `to_string`;
/// struct/enum vía `mostrar()` (Show recursivo). Arrays/funciones/etc. no son derivables aún.
/// `a` aporta la posición del `@derive` para ubicar el error.
fn render_to_string(a: &Annotation, expr: &str, ty: &Type) -> Result<String, TypeError> {
    match ty {
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Char => Ok(format!("to_string({expr})")),
        // En esta fase un tipo de usuario llega como `Struct` (el checker aún no lo resolvió a
        // `Enum`); ambos se imprimen con su propio `mostrar` (deben implementar Show).
        Type::Struct(_, _) | Type::Enum(_, _) => Ok(format!("{expr}.mostrar()")),
        otro => Err(TypeError {
            msg: format!("no se puede derivar Show para un campo de tipo {} (por ahora primitivos, struct y enum)", otro),
            line: a.line,
            col: a.col,
            len: 1,
        }),
    }
}

/// Cuerpo de `mostrar` para un struct: `"Nombre { campo: <v>, … }"` (sin campos → `"Nombre"`).
fn struct_show_body(a: &Annotation, name: &str, fields: &[(String, Type)]) -> Result<String, TypeError> {
    if fields.is_empty() {
        return Ok(format!("        \"{name}\""));
    }
    let mut partes: Vec<String> = Vec::new();
    for (n, ty) in fields {
        partes.push(format!("\"{n}: \" + {}", render_to_string(a, &format!("self.{n}"), ty)?));
    }
    // El string generado usa una cadena normal `"..."` (no `f"..."`), donde `{` es literal (M27.3).
    Ok(format!("        \"{name} {{ \" + {} + \" }}\"", partes.join(" + \", \" + ")))
}

/// Cuerpo de `mostrar` para un enum: `match` sobre `self`; por variante, `"Nombre.Variante"`
/// (unit) o `"Nombre.Variante(<v0>, <v1>)"` (con payload).
fn enum_show_body(a: &Annotation, name: &str, variants: &[VariantDef]) -> Result<String, TypeError> {
    let mut arms = String::new();
    for v in variants {
        let k = v.payload.len();
        if k == 0 {
            arms.push_str(&format!("            {name}.{v} => \"{name}.{v}\",\n", v = v.name));
        } else {
            let binds: Vec<String> = (0..k).map(|i| format!("a{i}")).collect();
            let mut piezas: Vec<String> = Vec::new();
            for (i, ty) in v.payload.iter().enumerate() {
                piezas.push(render_to_string(a, &format!("a{i}"), ty)?);
            }
            arms.push_str(&format!(
                "            {name}.{v}({b}) => \"{name}.{v}(\" + {p} + \")\",\n",
                v = v.name, b = binds.join(", "), p = piezas.join(" + \", \" + ")
            ));
        }
    }
    Ok(format!("        match (self) {{\n{arms}        }}"))
}

/// Construye y parsea `impl Trait for <name> {{ <firma> {{ body }} }}` para un derive.
fn parse_derived_impl(trait_name: &str, name: &str, firma: &str, body: &str) -> ImplBlock {
    let src = format!(
        "impl {trait_name} for {name} {{\n    {firma} {{\n{body}\n    }}\n}}"
    );
    let toks = crate::lexer::lex(&src).unwrap_or_else(|e| crate::ice!("el impl derivado no lexea: {e}"));
    let mut prog = crate::parser::parse(toks).unwrap_or_else(|e| crate::ice!("el impl derivado no parsea: {e}"));
    prog.impls.remove(0)
}

/// Cuerpo de `igual` para un struct: conjunción de la igualdad de cada campo (sin campos →
/// `true`).
fn struct_eq_body(fields: &[(String, Type)]) -> String {
    if fields.is_empty() {
        return "        true".into();
    }
    let cmps: Vec<String> = fields.iter().map(|(n, _)| format!("self.{n} == otro.{n}")).collect();
    format!("        {}", cmps.join(" && "))
}

/// Cuerpo de `igual` para un enum: `match` sobre `self`; por variante, `match` sobre `otro`
/// (misma variante → comparar payload posición a posición; otra → `false`).
fn enum_eq_body(name: &str, variants: &[VariantDef]) -> String {
    let mut arms = String::new();
    for v in variants {
        let k = v.payload.len();
        if k == 0 {
            arms.push_str(&format!(
                "            {name}.{v} => match (otro) {{ {name}.{v} => true, _ => false }},\n",
                v = v.name
            ));
        } else {
            let a: Vec<String> = (0..k).map(|i| format!("a{i}")).collect();
            let b: Vec<String> = (0..k).map(|i| format!("b{i}")).collect();
            let cmp: Vec<String> = (0..k).map(|i| format!("a{i} == b{i}")).collect();
            arms.push_str(&format!(
                "            {name}.{v}({a}) => match (otro) {{ {name}.{v}({b}) => {cmp}, _ => false }},\n",
                v = v.name, a = a.join(", "), b = b.join(", "), cmp = cmp.join(" && ")
            ));
        }
    }
    format!("        match (self) {{\n{arms}        }}")
}

/// Nombre del parámetro-diccionario para un método de un trait acotado (M9.2):
/// `T#Trait#metodo`. Como el `#` es ilegal en identificadores, no choca con locales del
/// usuario; vive como un parámetro función más.
fn dict_param_name(tparam: &str, trait_name: &str, method: &str) -> String {
    format!("{}#{}#{}", tparam, trait_name, method)
}

/// Tipo función de un método visto desde fuera (M9.2): incluye `self` como primer
/// parámetro. Con `Self → self_ty` (un `Var(T)` para un diccionario, un tipo concreto en
/// otros usos). P. ej. `mostrar(self) -> string` con `self_ty = T` da `fn(T) -> string`.
fn method_fn_type(m: &MethodSig, self_ty: &Type) -> Type {
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
fn freshen_positions(block: &mut Block, next: &mut usize) {
    freshen_block(block, next);
}

fn freshen_block(block: &mut Block, next: &mut usize) {
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

fn freshen_expr(expr: &mut Expr, next: &mut usize) {
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
                freshen_expr(&mut arm.body, next);
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
fn renumber_fn_exprs(program: &mut Program) {
    let mut next = 0usize;
    for f in &mut program.functions {
        renumber_block(&mut f.body, &mut next);
    }
}

fn renumber_block(block: &mut Block, next: &mut usize) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => renumber_expr(value, next),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { renumber_expr(start, next); renumber_expr(end, next); }
                    ForIter::In(e) => renumber_expr(e, next),
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

fn renumber_expr(expr: &mut Expr, next: &mut usize) {
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
                renumber_expr(&mut arm.body, next);
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

/// ¿El tipo menciona `Self` (M9.3b)? Cubre `SelfType` y `Struct("Self")`, recursivamente.
/// Lo usa la *object safety*: un método cuya firma (fuera del receptor) usa `Self` no es
/// invocable sobre un trait object.
fn type_uses_self(ty: &Type) -> bool {
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
fn subst_self(ty: &Type, target: &Type) -> Type {
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
        Type::Struct(n, args) => {
            Type::Struct(n.clone(), args.iter().map(|a| subst_self(a, target)).collect())
        }
        Type::Enum(n, args) => {
            Type::Enum(n.clone(), args.iter().map(|a| subst_self(a, target)).collect())
        }
        other => other.clone(),
    }
}

// =====================================================================
// Bajada de llamadas por punto (UFCS M7.1 + métodos de trait M9.1)
// =====================================================================
//
// `recv.f(args)` y `(recv.f)(args)` comparten forma (`Call` con callee `Field`). El
// checker, que conoce el tipo del receptor, decidió cuáles hay que bajar —UFCS de
// función libre o método de trait— y registró cada sitio `(línea, columna, nombre)`
// junto a su **función destino** (el mismo nombre, o el manglado `Tipo#metodo`). Estas
// funciones recorren el AST y **reescriben** esos nodos a `destino(recv, args)`: el
// receptor pasa a ser el primer argumento y el callee se vuelve un `Ident`. Tras esto,
// el intérprete y la VM solo ven llamadas ordinarias.

type SiteMap = HashMap<(usize, usize, String), String>;

fn lower_ufcs(program: &mut Program, sites: &SiteMap) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_ufcs_block(&mut f.body, sites);
    }
}

fn lower_ufcs_block(block: &mut Block, sites: &SiteMap) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_ufcs_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_ufcs_expr(start, sites); lower_ufcs_expr(end, sites); }
                    ForIter::In(e) => lower_ufcs_expr(e, sites),
                }
                lower_ufcs_block(body, sites);
            }
            StmtKind::Assign { target, value } => {
                lower_ufcs_expr(target, sites);
                lower_ufcs_expr(value, sites);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    lower_ufcs_expr(v, sites);
                }
            }
            StmtKind::Expr(e) => lower_ufcs_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_ufcs_expr(t, sites);
    }
}

fn lower_ufcs_expr(expr: &mut Expr, sites: &SiteMap) {
    // ¿Este `Call(Field)` es un sitio registrado? La clave incluye el nombre del método
    // porque el `Call` y su receptor comparten `(línea, columna)`; el valor es la función
    // **destino** (el mismo nombre para UFCS de función libre, el manglado para un método
    // de trait). Reescribir ANTES de recorrer los hijos, para que la recursión baje
    // también el receptor y los argumentos (p. ej. `a.f().g()`).
    let target = match &expr.kind {
        ExprKind::Call { callee, .. } => match &callee.kind {
            ExprKind::Field { name, .. } => sites.get(&(expr.line, expr.col, name.clone())).cloned(),
            _ => None,
        },
        _ => None,
    };
    if let Some(target) = target {
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0));
        if let ExprKind::Call { callee, mut args } = taken {
            let (cl, cc) = (callee.line, callee.col);
            if let ExprKind::Field { object, .. } = callee.kind {
                let mut new_args = Vec::with_capacity(args.len() + 1);
                new_args.push(*object); // el receptor pasa a ser el primer argumento
                new_args.append(&mut args);
                expr.kind = ExprKind::Call {
                    callee: Box::new(Expr { kind: ExprKind::Ident(target), line: cl, col: cc }),
                    args: new_args,
                };
            } else {
                crate::ice!("el guard de sitio garantiza Call con callee Field");
            }
        } else {
            crate::ice!("el guard de sitio garantiza un Call");
        }
    }

    // Recorrer los sub-nodos (incluye los argumentos ya reescritos).
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_ufcs_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => {
            lower_ufcs_expr(left, sites);
            lower_ufcs_expr(right, sites);
        }
        ExprKind::Call { callee, args } => {
            lower_ufcs_expr(callee, sites);
            for a in args {
                lower_ufcs_expr(a, sites);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                lower_ufcs_expr(e, sites);
            }
        }
        ExprKind::Index { array, index } => {
            lower_ufcs_expr(array, sites);
            lower_ufcs_expr(index, sites);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                lower_ufcs_expr(e, sites);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                lower_ufcs_expr(a, sites);
            }
        }
        ExprKind::Field { object, .. } => lower_ufcs_expr(object, sites),
        ExprKind::Func(fe) => lower_ufcs_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_ufcs_expr(scrutinee, sites);
            for arm in arms {
                lower_ufcs_expr(&mut arm.body, sites);
            }
        }
        ExprKind::Try(inner) => lower_ufcs_expr(inner, sites),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_ufcs_expr(cond, sites);
            lower_ufcs_block(then_branch, sites);
            if let Some(e) = else_branch {
                lower_ufcs_expr(e, sites);
            }
        }
        ExprKind::While { cond, body } => {
            lower_ufcs_expr(cond, sites);
            lower_ufcs_block(body, sites);
        }
        ExprKind::Block(b) => lower_ufcs_block(b, sites),
        // Literales, Ident: nada que recorrer.
        _ => {}
    }
}

// =====================================================================
// Bajada de literales enteros coercionados a uint (M28.3b)
// =====================================================================
//
// Un literal entero en posición uint (`let x: u8 = 5`, `x + 100` con `x: u8`) se registró en
// `uint_literal_sites`. Aquí se envuelve en un `Cast` al ancho (`5 as u8`), de modo que el
// runtime —que borra los tipos— produzca el `UInt` correcto. Reusa el `as` de M27.4/M28.3a.

type UIntLitMap = HashMap<(usize, usize), u8>;

fn lower_uint_literals(program: &mut Program, sites: &UIntLitMap) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_uintlit_block(&mut f.body, sites);
    }
}

fn lower_uintlit_block(block: &mut Block, sites: &UIntLitMap) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_uintlit_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_uintlit_expr(start, sites); lower_uintlit_expr(end, sites); }
                    ForIter::In(e) => lower_uintlit_expr(e, sites),
                }
                lower_uintlit_block(body, sites);
            }
            StmtKind::Assign { target, value } => { lower_uintlit_expr(target, sites); lower_uintlit_expr(value, sites); }
            StmtKind::Return { value } => { if let Some(v) = value { lower_uintlit_expr(v, sites); } }
            StmtKind::Expr(e) => lower_uintlit_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_uintlit_expr(t, sites);
    }
}

fn lower_uintlit_expr(expr: &mut Expr, sites: &UIntLitMap) {
    // ¿Es un literal entero registrado? Envolverlo en `Cast` al ancho. (No tiene hijos que recorrer.)
    if let ExprKind::Int(_) = &expr.kind {
        if let Some(&w) = sites.get(&(expr.line, expr.col)) {
            let (l, c) = (expr.line, expr.col);
            let inner = std::mem::replace(&mut expr.kind, ExprKind::Int(0));
            expr.kind = ExprKind::Cast {
                expr: Box::new(Expr { kind: inner, line: l, col: c }),
                ty: Type::UInt(w),
            };
            return;
        }
    }
    // Recorrer los sub-nodos.
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_uintlit_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => { lower_uintlit_expr(left, sites); lower_uintlit_expr(right, sites); }
        ExprKind::Call { callee, args } => {
            lower_uintlit_expr(callee, sites);
            for a in args { lower_uintlit_expr(a, sites); }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => { for e in elems { lower_uintlit_expr(e, sites); } }
        ExprKind::Index { array, index } => { lower_uintlit_expr(array, sites); lower_uintlit_expr(index, sites); }
        ExprKind::StructLit { fields, .. } => { for (_, e) in fields { lower_uintlit_expr(e, sites); } }
        ExprKind::EnumLit { args, .. } => { for a in args { lower_uintlit_expr(a, sites); } }
        ExprKind::Field { object, .. } => lower_uintlit_expr(object, sites),
        ExprKind::Func(fe) => lower_uintlit_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_uintlit_expr(scrutinee, sites);
            for arm in arms { lower_uintlit_expr(&mut arm.body, sites); }
        }
        ExprKind::Try(inner) => lower_uintlit_expr(inner, sites),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_uintlit_expr(cond, sites);
            lower_uintlit_block(then_branch, sites);
            if let Some(e) = else_branch { lower_uintlit_expr(e, sites); }
        }
        ExprKind::While { cond, body } => { lower_uintlit_expr(cond, sites); lower_uintlit_block(body, sites); }
        ExprKind::Block(b) => lower_uintlit_block(b, sites),
        _ => {}
    }
}

// =====================================================================
// Bajada de `?` con conversión de error (M28.2)
// =====================================================================
//
// `expr?` sobre `Result<T, E1>` en una función que devuelve `Result<_, E2>` (con
// `impl From<E1> for E2`) se registró en `try_conversions` con la función manglada de
// conversión. Aquí ese `Try` se reescribe a:
//
//     match (expr) {
//         Result.Ok($to)  => $to,
//         Result.Err($te) => { return Result.Err(<from>($te)); },
//     }
//
// Puro front-end (reusa `match`, construcción de enum y `return`): el runtime no cambia.
// El `?` que NO convierte sigue siendo el nodo nativo `Try` (M6.3).

type TryConvMap = HashMap<(usize, usize), String>;

fn lower_try_conversions(program: &mut Program, sites: &TryConvMap) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_try_block(&mut f.body, sites);
    }
}

fn lower_try_block(block: &mut Block, sites: &TryConvMap) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_try_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_try_expr(start, sites); lower_try_expr(end, sites); }
                    ForIter::In(e) => lower_try_expr(e, sites),
                }
                lower_try_block(body, sites);
            }
            StmtKind::Assign { target, value } => { lower_try_expr(target, sites); lower_try_expr(value, sites); }
            StmtKind::Return { value } => { if let Some(v) = value { lower_try_expr(v, sites); } }
            StmtKind::Expr(e) => lower_try_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_try_expr(t, sites);
    }
}

fn lower_try_expr(expr: &mut Expr, sites: &TryConvMap) {
    // ¿Este `Try` es un sitio de conversión registrado? Reescribir ANTES de recorrer los hijos,
    // para que la recursión baje el operando (ahora escrutinio del match).
    let conv = match &expr.kind {
        ExprKind::Try(_) => sites.get(&(expr.line, expr.col)).cloned(),
        _ => None,
    };
    if let Some(mangled) = conv {
        let (l, c) = (expr.line, expr.col);
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0));
        let inner = match taken {
            ExprKind::Try(inner) => *inner,
            _ => crate::ice!("el guard garantiza un Try"),
        };
        let mk = |kind| Expr { kind, line: l, col: c };
        // Rama Ok: `Result.Ok($to) => $to`.
        let arm_ok = MatchArm {
            pattern: Pattern {
                kind: PatternKind::Variant { enum_name: "Result".into(), variant: "Ok".into(), bindings: vec![Some("$to".into())] },
                line: l, col: c,
            },
            body: mk(ExprKind::Ident("$to".into())),
            line: l, col: c,
        };
        // Rama Err: `Result.Err($te) => { return Result.Err(<from>($te)); }`.
        let convertido = mk(ExprKind::Call {
            callee: Box::new(mk(ExprKind::Ident(mangled))),
            args: vec![mk(ExprKind::Ident("$te".into()))],
        });
        let err_val = mk(ExprKind::EnumLit { enum_name: "Result".into(), variant: "Err".into(), args: vec![convertido] });
        let ret_stmt = Stmt { kind: StmtKind::Return { value: Some(err_val) }, line: l, col: c };
        let arm_err = MatchArm {
            pattern: Pattern {
                kind: PatternKind::Variant { enum_name: "Result".into(), variant: "Err".into(), bindings: vec![Some("$te".into())] },
                line: l, col: c,
            },
            body: mk(ExprKind::Block(Block { statements: vec![ret_stmt], tail: None, line: l, col: c })),
            line: l, col: c,
        };
        expr.kind = ExprKind::Match { scrutinee: Box::new(inner), arms: vec![arm_ok, arm_err] };
    }

    // Recorrer los sub-nodos.
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_try_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => { lower_try_expr(left, sites); lower_try_expr(right, sites); }
        ExprKind::Call { callee, args } => {
            lower_try_expr(callee, sites);
            for a in args { lower_try_expr(a, sites); }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => { for e in elems { lower_try_expr(e, sites); } }
        ExprKind::Index { array, index } => { lower_try_expr(array, sites); lower_try_expr(index, sites); }
        ExprKind::StructLit { fields, .. } => { for (_, e) in fields { lower_try_expr(e, sites); } }
        ExprKind::EnumLit { args, .. } => { for a in args { lower_try_expr(a, sites); } }
        ExprKind::Field { object, .. } => lower_try_expr(object, sites),
        ExprKind::Func(fe) => lower_try_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_try_expr(scrutinee, sites);
            for arm in arms { lower_try_expr(&mut arm.body, sites); }
        }
        ExprKind::Try(inner) => lower_try_expr(inner, sites),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_try_expr(cond, sites);
            lower_try_block(then_branch, sites);
            if let Some(e) = else_branch { lower_try_expr(e, sites); }
        }
        ExprKind::While { cond, body } => { lower_try_expr(cond, sites); lower_try_block(body, sites); }
        ExprKind::Block(b) => lower_try_block(b, sites),
        _ => {}
    }
}

// =====================================================================
// Bajada de sobrecarga de operadores (M28.1)
// =====================================================================
//
// `a op b` (con un tipo de usuario que implementa el trait del operador) y `-x` se
// registraron en `op_sites` con clave `(línea, col, "Add"/"Sub"/…/"Neg")` → función
// manglada del método (`Vec2#add`). Aquí se reescriben esos `Binary`/`Unary` a una
// llamada ordinaria `metodo(a, b)` / `metodo(x)`, que el intérprete y la VM ya saben
// ejecutar (el método es una función libre inyectada por M9). Corre **antes** de
// `lower_ufcs`, así el resultado —un `Call(Ident)`— no necesita más bajadas.

fn lower_operators(program: &mut Program, sites: &SiteMap) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_operators_block(&mut f.body, sites);
    }
}

fn lower_operators_block(block: &mut Block, sites: &SiteMap) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_operators_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_operators_expr(start, sites); lower_operators_expr(end, sites); }
                    ForIter::In(e) => lower_operators_expr(e, sites),
                }
                lower_operators_block(body, sites);
            }
            StmtKind::Assign { target, value } => {
                lower_operators_expr(target, sites);
                lower_operators_expr(value, sites);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    lower_operators_expr(v, sites);
                }
            }
            StmtKind::Expr(e) => lower_operators_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_operators_expr(t, sites);
    }
}

fn lower_operators_expr(expr: &mut Expr, sites: &SiteMap) {
    // ¿Este `Binary`/`Unary` es un sitio registrado? La clave lleva el nombre del trait del
    // operador porque un mismo `(línea, col)` puede corresponder a operadores encadenados
    // (`a + b + c`); un mismo operador en la misma posición baja al mismo método. Reescribir
    // ANTES de recorrer los hijos, para que la recursión baje también los operandos.
    let target = match &expr.kind {
        ExprKind::Binary { op, .. } => op_trait_method(*op)
            .and_then(|(tr, _)| sites.get(&(expr.line, expr.col, tr.to_string())).cloned()),
        ExprKind::Unary { op: UnaryOp::Neg, .. } => {
            sites.get(&(expr.line, expr.col, "Neg".to_string())).cloned()
        }
        _ => None,
    };
    if let Some(target) = target {
        let (l, c) = (expr.line, expr.col);
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0));
        let args = match taken {
            ExprKind::Binary { left, right, .. } => vec![*left, *right],
            ExprKind::Unary { expr: inner, .. } => vec![*inner],
            _ => crate::ice!("el guard de sitio garantiza Binary o Unary Neg"),
        };
        expr.kind = ExprKind::Call {
            callee: Box::new(Expr { kind: ExprKind::Ident(target), line: l, col: c }),
            args,
        };
    }

    // Recorrer los sub-nodos (incluye los operandos ya reescritos).
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_operators_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => {
            lower_operators_expr(left, sites);
            lower_operators_expr(right, sites);
        }
        ExprKind::Call { callee, args } => {
            lower_operators_expr(callee, sites);
            for a in args {
                lower_operators_expr(a, sites);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                lower_operators_expr(e, sites);
            }
        }
        ExprKind::Index { array, index } => {
            lower_operators_expr(array, sites);
            lower_operators_expr(index, sites);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                lower_operators_expr(e, sites);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                lower_operators_expr(a, sites);
            }
        }
        ExprKind::Field { object, .. } => lower_operators_expr(object, sites),
        ExprKind::Func(fe) => lower_operators_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_operators_expr(scrutinee, sites);
            for arm in arms {
                lower_operators_expr(&mut arm.body, sites);
            }
        }
        ExprKind::Try(inner) => lower_operators_expr(inner, sites),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_operators_expr(cond, sites);
            lower_operators_block(then_branch, sites);
            if let Some(e) = else_branch {
                lower_operators_expr(e, sites);
            }
        }
        ExprKind::While { cond, body } => {
            lower_operators_expr(cond, sites);
            lower_operators_block(body, sites);
        }
        ExprKind::Block(b) => lower_operators_block(b, sites),
        _ => {}
    }
}

// =====================================================================
// Bajada de bounds: diccionarios (M9.2)
// =====================================================================
//
// Un bound `T: Trait` se baja a **paso de diccionarios**: la función gana un parámetro
// función por método del trait, y cada sitio de llamada pasa el diccionario adecuado
// (el método de un impl concreto, o el reenvío del diccionario propio). Todo son valores
// función que el runtime ya sabe pasar/llamar (M4): cero cambios en los motores.

/// Añade a cada función con bounds sus **parámetros-diccionario** (M9.2), al final de la
/// lista de parámetros, en el orden canónico (bounds en orden; por bound, los métodos del
/// trait en orden) que casa con el de los argumentos en los sitios de llamada.
fn append_dict_params(program: &mut Program) {
    let trait_sigs: HashMap<String, Vec<MethodSig>> = program.traits.iter()
        .map(|t| (t.name.clone(), t.methods.clone()))
        .collect();
    for f in &mut program.functions {
        if f.bounds.is_empty() {
            continue;
        }
        for (tp, trait_name) in &f.bounds {
            let Some(methods) = trait_sigs.get(trait_name) else { continue };
            let self_ty = Type::Var(tp.clone());
            for m in methods {
                f.params.push(Param {
                    name: dict_param_name(tp, trait_name, &m.name),
                    ty: method_fn_type(m, &self_ty),
                    line: f.line,
                    col: f.col,
                });
            }
        }
    }
}

/// Sitios de llamada a funciones con bounds → **expresiones**-diccionario a añadir como
/// argumentos (M9.2b). En M9.2 eran simples nombres (`Ident`); con impls genéricos acotados un
/// diccionario puede ser un **closure** que captura los diccionarios internos (anidados).
type DictSites = HashMap<(usize, usize, String), Vec<Expr>>;

/// Añade en cada **sitio de llamada** a una función con bounds los argumentos-diccionario
/// registrados (M9.2). Reescribe `f(args)` → `f(args, dicts...)`. Corre **tras** `lower_ufcs`
/// (el callee ya es un `Ident`), reusando la clave `(línea, col, nombre)`.
fn lower_dict_calls(program: &mut Program, sites: &DictSites) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_dict_calls_block(&mut f.body, sites);
    }
}

fn lower_dict_calls_block(block: &mut Block, sites: &DictSites) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_dict_calls_expr(value, sites),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_dict_calls_expr(start, sites); lower_dict_calls_expr(end, sites); }
                    ForIter::In(e) => lower_dict_calls_expr(e, sites),
                }
                lower_dict_calls_block(body, sites);
            }
            StmtKind::Assign { target, value } => {
                lower_dict_calls_expr(target, sites);
                lower_dict_calls_expr(value, sites);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    lower_dict_calls_expr(v, sites);
                }
            }
            StmtKind::Expr(e) => lower_dict_calls_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_dict_calls_expr(t, sites);
    }
}

fn lower_dict_calls_expr(expr: &mut Expr, sites: &DictSites) {
    // Recorrer primero los hijos (el receptor y los argumentos pueden ser otras llamadas).
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_dict_calls_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => {
            lower_dict_calls_expr(left, sites);
            lower_dict_calls_expr(right, sites);
        }
        ExprKind::Call { callee, args } => {
            lower_dict_calls_expr(callee, sites);
            for a in args.iter_mut() {
                lower_dict_calls_expr(a, sites);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                lower_dict_calls_expr(e, sites);
            }
        }
        ExprKind::Index { array, index } => {
            lower_dict_calls_expr(array, sites);
            lower_dict_calls_expr(index, sites);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                lower_dict_calls_expr(e, sites);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                lower_dict_calls_expr(a, sites);
            }
        }
        ExprKind::Field { object, .. } => lower_dict_calls_expr(object, sites),
        ExprKind::Func(fe) => lower_dict_calls_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_dict_calls_expr(scrutinee, sites);
            for arm in arms {
                lower_dict_calls_expr(&mut arm.body, sites);
            }
        }
        ExprKind::Try(inner) => lower_dict_calls_expr(inner, sites),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_dict_calls_expr(cond, sites);
            lower_dict_calls_block(then_branch, sites);
            if let Some(e) = else_branch {
                lower_dict_calls_expr(e, sites);
            }
        }
        ExprKind::While { cond, body } => {
            lower_dict_calls_expr(cond, sites);
            lower_dict_calls_block(body, sites);
        }
        ExprKind::Block(b) => lower_dict_calls_block(b, sites),
        _ => {}
    }
    // Tras recorrer los hijos, si este nodo es una llamada por nombre a una función con
    // bounds registrada en este sitio, añadir los diccionarios como argumentos extra.
    let (line, col) = (expr.line, expr.col);
    let dicts: Option<Vec<Expr>> = match &expr.kind {
        ExprKind::Call { callee, .. } => match &callee.kind {
            ExprKind::Ident(name) => sites.get(&(line, col, name.clone())).cloned(),
            _ => None,
        },
        _ => None,
    };
    // Los diccionarios (M9.2b: posiblemente closures anidados) se añaden ya construidos.
    if let (Some(dicts), ExprKind::Call { args, .. }) = (dicts, &mut expr.kind) {
        args.extend(dicts);
    }
}

// =====================================================================
// Bajada de trait objects (M9.3b)
// =====================================================================
//
// Un `dyn Trait` se realiza como un **struct sintetizado** `__dyn_Trait { data, métodos... }`
// (el fat value / vtable). La **coerción** concreto→objeto construye ese struct; el
// **despacho** `obj.m(args)` baja a `{ let r = obj; (r.m)(r.data, args) }`. Reusa structs +
// funciones de primera clase: el intérprete y la VM no saben de trait objects.

type CoercionMap = HashMap<(usize, usize), (Vec<String>, Vec<Expr>)>;
type DispatchSet = HashSet<(usize, usize, String)>;
type UpcastMap = HashMap<(usize, usize), Vec<String>>;

/// Nombre del struct sintetizado que realiza `dyn A + B` en runtime. El conjunto viene canónico
/// (ordenado), así que el nombre es único por conjunto. El `+` es ilegal en identificadores de
/// usuario, igual que el prefijo `__dyn_`, así que no colisiona con nada escribible.
fn dyn_struct_name(traits: &[String]) -> String {
    format!("__dyn_{}", traits.join("+"))
}

/// Nombres de los métodos de la vtable de un `dyn A + B`, en orden canónico: por cada trait del
/// conjunto (ya ordenado), sus métodos en orden de declaración. Coincide con el orden en que
/// `coerce_to_dyn` armó las expresiones-vtable.
fn dyn_method_names(traits: &[String], tm: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut names = Vec::new();
    for tr in traits {
        if let Some(ms) = tm.get(tr) {
            names.extend(ms.iter().cloned());
        }
    }
    names
}

fn ident_expr(name: &str, line: usize, col: usize) -> Expr {
    Expr { kind: ExprKind::Ident(name.to_string()), line, col }
}

fn lower_dyn(program: &mut Program, coercions: &CoercionMap, dispatch: &DispatchSet, upcasts: &UpcastMap) {
    if coercions.is_empty() && dispatch.is_empty() && upcasts.is_empty() {
        return;
    }
    // Mapa trait → nombres de métodos (en orden), para construir vtables.
    let trait_methods: HashMap<String, Vec<String>> = program.traits.iter()
        .map(|t| (t.name.clone(), t.methods.iter().map(|m| m.name.clone()).collect()))
        .collect();
    // Structs sintetizados: uno por **conjunto** distinto que aparezca en una coerción **o como
    // destino de un upcast**, con `data` + un campo función por método de la unión (la vtable). Los
    // tipos de campo son irrelevantes en runtime (erasure); el motor solo usa los nombres y el orden.
    let mut sets: Vec<Vec<String>> = coercions.values().map(|(set, _)| set.clone())
        .chain(upcasts.values().cloned())
        .collect();
    sets.sort();
    sets.dedup();
    for set in &sets {
        let mut fields = vec![("data".to_string(), Type::Unit)];
        for m in dyn_method_names(set, &trait_methods) {
            fields.push((m, Type::Unit));
        }
        program.structs.push(StructDef {
            annotations: Vec::new(),
            is_pub: false,
            name: dyn_struct_name(set),
            type_params: Vec::new(),
            bounds: Vec::new(),
            fields,
            line: 0,
            col: 0,
        });
    }
    let mut counter = 0usize;
    for f in &mut program.functions {
        lower_dyn_block(&mut f.body, coercions, dispatch, upcasts, &trait_methods, &mut counter);
    }
}

fn lower_dyn_block(block: &mut Block, coercions: &CoercionMap, dispatch: &DispatchSet, upcasts: &UpcastMap, tm: &HashMap<String, Vec<String>>, counter: &mut usize) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_dyn_expr(value, coercions, dispatch, upcasts, tm, counter),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { lower_dyn_expr(start, coercions, dispatch, upcasts, tm, counter); lower_dyn_expr(end, coercions, dispatch, upcasts, tm, counter); }
                    ForIter::In(e) => lower_dyn_expr(e, coercions, dispatch, upcasts, tm, counter),
                }
                lower_dyn_block(body, coercions, dispatch, upcasts, tm, counter);
            }
            StmtKind::Assign { target, value } => {
                lower_dyn_expr(target, coercions, dispatch, upcasts, tm, counter);
                lower_dyn_expr(value, coercions, dispatch, upcasts, tm, counter);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    lower_dyn_expr(v, coercions, dispatch, upcasts, tm, counter);
                }
            }
            StmtKind::Expr(e) => lower_dyn_expr(e, coercions, dispatch, upcasts, tm, counter),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_dyn_expr(t, coercions, dispatch, upcasts, tm, counter);
    }
}

fn lower_dyn_expr(expr: &mut Expr, coercions: &CoercionMap, dispatch: &DispatchSet, upcasts: &UpcastMap, tm: &HashMap<String, Vec<String>>, counter: &mut usize) {
    // Recorrer los sub-nodos primero (post-orden): así los despachos/coerciones anidados
    // (en el receptor y los argumentos) ya están bajados cuando reescribimos este nodo.
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => lower_dyn_expr(inner, coercions, dispatch, upcasts, tm, counter),
        ExprKind::Binary { left, right, .. } => {
            lower_dyn_expr(left, coercions, dispatch, upcasts, tm, counter);
            lower_dyn_expr(right, coercions, dispatch, upcasts, tm, counter);
        }
        ExprKind::Call { callee, args } => {
            lower_dyn_expr(callee, coercions, dispatch, upcasts, tm, counter);
            for a in args.iter_mut() {
                lower_dyn_expr(a, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                lower_dyn_expr(e, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::Index { array, index } => {
            lower_dyn_expr(array, coercions, dispatch, upcasts, tm, counter);
            lower_dyn_expr(index, coercions, dispatch, upcasts, tm, counter);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                lower_dyn_expr(e, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                lower_dyn_expr(a, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::Field { object, .. } => lower_dyn_expr(object, coercions, dispatch, upcasts, tm, counter),
        ExprKind::Func(fe) => lower_dyn_block(&mut fe.body, coercions, dispatch, upcasts, tm, counter),
        ExprKind::Match { scrutinee, arms } => {
            lower_dyn_expr(scrutinee, coercions, dispatch, upcasts, tm, counter);
            for arm in arms {
                lower_dyn_expr(&mut arm.body, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::Try(inner) => lower_dyn_expr(inner, coercions, dispatch, upcasts, tm, counter),
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_dyn_expr(cond, coercions, dispatch, upcasts, tm, counter);
            lower_dyn_block(then_branch, coercions, dispatch, upcasts, tm, counter);
            if let Some(e) = else_branch {
                lower_dyn_expr(e, coercions, dispatch, upcasts, tm, counter);
            }
        }
        ExprKind::While { cond, body } => {
            lower_dyn_expr(cond, coercions, dispatch, upcasts, tm, counter);
            lower_dyn_block(body, coercions, dispatch, upcasts, tm, counter);
        }
        ExprKind::Block(b) => lower_dyn_block(b, coercions, dispatch, upcasts, tm, counter),
        _ => {}
    }

    let (line, col) = (expr.line, expr.col);

    // Despacho dinámico: `obj.m(args)` → `{ let r = obj; (r.m)(r.data, args) }`.
    let dispatch_method = match &expr.kind {
        ExprKind::Call { callee, .. } => match &callee.kind {
            ExprKind::Field { name, .. } if dispatch.contains(&(line, col, name.clone())) => Some(name.clone()),
            _ => None,
        },
        _ => None,
    };
    if dispatch_method.is_some() {
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0));
        let ExprKind::Call { callee, mut args } = taken else { crate::ice!("sitio de despacho es un Call") };
        let ExprKind::Field { object, name } = callee.kind else { crate::ice!("el callee de un despacho es un Field") };
        let tmp = format!("__dynrecv#{}", *counter);
        *counter += 1;
        let let_stmt = Stmt {
            kind: StmtKind::Let { name: tmp.clone(), ty: None, value: *object, mutable: false },
            line, col,
        };
        // (r.name)(r.data, ...args)
        let method_field = Expr {
            kind: ExprKind::Field { object: Box::new(ident_expr(&tmp, line, col)), name },
            line, col,
        };
        let mut new_args = Vec::with_capacity(args.len() + 1);
        new_args.push(Expr {
            kind: ExprKind::Field { object: Box::new(ident_expr(&tmp, line, col)), name: "data".into() },
            line, col,
        });
        new_args.append(&mut args);
        let call = Expr { kind: ExprKind::Call { callee: Box::new(method_field), args: new_args }, line, col };
        expr.kind = ExprKind::Block(Block { statements: vec![let_stmt], tail: Some(Box::new(call)), line, col });
    }

    // Coerción concreto→`dyn Trait`: envolver en el struct sintetizado (la vtable). Los valores
    // función de la vtable los calculó el checker con `dict_for` (M9.4) — método manglado plano o
    // closure anidado para un impl genérico acotado—, así que `dyn` funciona también sobre impls
    // genéricos. Van en el orden de los métodos del trait, igual que `tm`.
    if let Some((set, vtable)) = coercions.get(&(line, col)) {
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0));
        let inner = Expr { kind: taken, line, col };
        let mut fields = vec![("data".to_string(), inner)];
        let names = dyn_method_names(set, tm);
        for (m, vexpr) in names.iter().zip(vtable) {
            fields.push((m.clone(), vexpr.clone()));
        }
        expr.kind = ExprKind::StructLit { name: dyn_struct_name(set), fields };
    }

    // Upcasting `dyn S1` → `dyn S2` (S2 ⊆ S1, M9.5b): reconstruir el struct menor proyectando los
    // campos del mayor. Necesita un temp porque el origen se referencia varias veces:
    // `{ let __dynup = <obj>; __dyn_S2 { data: __dynup.data, m: __dynup.m, … } }`.
    if let Some(target) = upcasts.get(&(line, col)) {
        let taken = std::mem::replace(&mut expr.kind, ExprKind::Int(0));
        let source = Expr { kind: taken, line, col };
        let tmp = format!("__dynup#{}", *counter);
        *counter += 1;
        let let_stmt = Stmt {
            kind: StmtKind::Let { name: tmp.clone(), ty: None, value: source, mutable: false },
            line, col,
        };
        let mut fields = Vec::new();
        for field in std::iter::once("data".to_string()).chain(dyn_method_names(target, tm)) {
            let proj = Expr {
                kind: ExprKind::Field { object: Box::new(ident_expr(&tmp, line, col)), name: field.clone() },
                line, col,
            };
            fields.push((field, proj));
        }
        let lit = Expr { kind: ExprKind::StructLit { name: dyn_struct_name(target), fields }, line, col };
        expr.kind = ExprKind::Block(Block { statements: vec![let_stmt], tail: Some(Box::new(lit)), line, col });
    }
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// Lexea, parsea y verifica un fuente completo.
    fn check_src(src: &str) -> Result<(), TypeError> {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        check(&mut prog)
    }

    /// Atajo: ¿el mensaje de error contiene esta subcadena?
    fn err_contains(src: &str, needle: &str) {
        let e = check_src(src).expect_err("debería fallar la verificación");
        assert!(
            e.msg.contains(needle),
            "mensaje '{}' no contiene '{}'",
            e.msg,
            needle
        );
    }

    #[test]
    fn asignar_a_posicion_de_tupla_es_error() {
        // M34 (SPEC §5): las posiciones de tupla son de solo lectura. Antes esto era
        // un ICE (pasaba el checker sin bajarse y reventaba ambos motores).
        err_contains(
            "fn main() -> int { var t = (1, 2); t.0 = 9; t.0 }",
            "una posición de tupla no es asignable",
        );
        // La lectura y la desestructuración siguen funcionando.
        check_src("fn main() -> int { let t = (1, 2); let (a, b) = t; a + b + t.0 }")
            .expect("leer y desestructurar es válido");
    }

        #[test]
    fn check_all_acumula_por_funcion() {
        // M33c: un error por cuerpo, todos reportados; el primero idéntico al fail-fast.
        let src = "fn f() -> int { 1 + true }\nfn g() -> int { \"x\" * 2 }\nfn main() -> int { f() + g() }";
        let toks = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(toks).expect("parse ok");
        let mut prog2 = prog.clone();
        let errs = check_all(&mut prog2);
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs[0].msg.contains("int y bool"), "{}", errs[0].msg);
        assert!(errs[1].msg.contains("string y int"), "{}", errs[1].msg);
        let solo = check(&mut prog).unwrap_err();
        assert_eq!(errs[0], solo, "el primer error debe ser byte-idéntico (oráculos)");
    }

    #[test]
    fn check_all_con_pasada_temprana_rota_da_un_error() {
        // Un error de la pre-pasada (función duplicada) es fail-fast → exactamente uno,
        // aunque haya además errores de cuerpo más abajo.
        let src = "fn f() -> int { 0 }\nfn f() -> int { 1 }\nfn g() -> int { 1 + true }\nfn main() -> int { 0 }";
        let toks = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(toks).expect("parse ok");
        let errs = check_all(&mut prog);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].msg.contains("declarada dos veces"), "{}", errs[0].msg);
        // Y un tipo desconocido en una FIRMA se valida en la fase de cuerpos → acumula
        // junto a los demás (mejor: más errores de una tacada).
        let src = "fn f(a: NoExiste) -> int { 0 }\nfn g() -> int { 1 + true }\nfn main() -> int { 0 }";
        let toks = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(toks).expect("parse ok");
        let errs = check_all(&mut prog);
        assert_eq!(errs.len(), 2, "{errs:?}");
    }

        #[test]
    fn el_error_de_tipos_subraya_la_expresion_completa() {
        // M33a-2: la extensión del error sale de la tabla de spans del parser.
        let e = check_src("fn main() -> int { let x = 1 + true; x }").unwrap_err();
        assert!(e.msg.contains("requiere ambos operandos"), "{}", e.msg);
        assert_eq!(e.len, "1 + true".chars().count(), "subraya la expresión entera");
        // Un argumento de tipo equivocado subraya el argumento (pasa por expression()).
        let e = check_src("fn f(a: int) -> int { a }\nfn main() -> int { f(\"dos\") }").unwrap_err();
        assert!(e.msg.contains("se esperaba int"), "{}", e.msg);
        assert_eq!(e.len, "\"dos\"".chars().count());
    }

    // M9.4: bounds en parámetros de tipo de struct/enum (verificados en la construcción).
    const BOUND_PRELUDE: &str = r#"
trait Show2 { fn ver(self) -> string; }
struct P { n: int }
impl Show2 for P { fn ver(self) -> string { "P" } }
struct Q { n: int }
"#;

    #[test]
    fn bound_struct_ok_con_impl() {
        let src = format!("{}struct Caja<T: Show2> {{ v: T }}\nfn main() -> int {{ let c = Caja {{ v: P {{ n: 1 }} }}; c.v.ver(); 0 }}\n", BOUND_PRELUDE);
        check_src(&src).expect("P implementa Show2");
    }

    #[test]
    fn bound_struct_falla_sin_impl() {
        let src = format!("{}struct Caja<T: Show2> {{ v: T }}\nfn main() -> int {{ let c = Caja {{ v: Q {{ n: 1 }} }}; 0 }}\n", BOUND_PRELUDE);
        err_contains(&src, "requiere que 'T' sea 'Show2'");
    }

    #[test]
    fn bound_struct_propaga_a_funcion_generica() {
        // Construir Caja<U> exige que U lleve el bound: sin él, error; con él, OK.
        let malo = format!("{}struct Caja<T: Show2> {{ v: T }}\nfn env<U>(x: U) -> Caja<U> {{ Caja {{ v: x }} }}\nfn main() -> int {{ 0 }}\n", BOUND_PRELUDE);
        err_contains(&malo, "requiere que 'T' sea 'Show2'");
        let bueno = format!("{}struct Caja<T: Show2> {{ v: T }}\nfn env<U: Show2>(x: U) -> Caja<U> {{ Caja {{ v: x }} }}\nfn main() -> int {{ 0 }}\n", BOUND_PRELUDE);
        check_src(&bueno).expect("con U: Show2 la propagación se satisface");
    }

    #[test]
    fn bound_enum_falla_sin_impl() {
        let src = format!("{}enum Opt<T: Show2> {{ Nada, Algo(T) }}\nfn main() -> int {{ let x = Opt.Algo(Q {{ n: 1 }}); 0 }}\n", BOUND_PRELUDE);
        err_contains(&src, "requiere que 'T' sea 'Show2'");
    }

    #[test]
    fn bound_struct_trait_inexistente_es_error() {
        err_contains("struct Caja<T: NoExiste> { v: T }\nfn main() -> int { 0 }\n", "trait 'NoExiste' no declarado");
    }

    #[test]
    fn fib_es_valido() {
        let src = r#"
fn fib(n: int) -> int {
    if (n < 2) { n } else { fib(n - 1) + fib(n - 2) }
}
fn main() -> int {
    var i: int = 0;
    while (i < 10) {
        print(fib(i));
        i = i + 1;
    }
    0
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn aritmetica_mezclada_falla() {
        err_contains("fn main() -> int { 1 + true }", "requiere ambos operandos");
        err_contains("fn main() { let x: float = 1 + 2.0; }", "requiere ambos operandos");
    }

    #[test]
    fn condicion_debe_ser_bool() {
        err_contains("fn main() { if (1) { } }", "condición del if debe ser bool");
        err_contains("fn main() { while (1) { } }", "condición del while debe ser bool");
    }

    #[test]
    fn ramas_del_if_mismo_tipo() {
        err_contains(
            "fn main() -> int { if (true) { 1 } else { true } }",
            "ramas del if tienen tipos distintos",
        );
    }

    #[test]
    fn if_sin_else_debe_ser_unit() {
        err_contains("fn main() { if (true) { 5 } }", "sin else tiene tipo unit");
    }

    #[test]
    fn asignar_a_let_falla_pero_a_var_ok() {
        err_contains(
            "fn main() { let x: int = 0; x = 1; }",
            "es inmutable",
        );
        assert!(check_src("fn main() { var x: int = 0; x = 1; }").is_ok());
    }

    #[test]
    fn variable_no_declarada() {
        err_contains("fn main() -> int { x }", "no declarado");
        err_contains("fn main() { y = 1; }", "no declarada");
    }

    #[test]
    fn tipo_de_declaracion_debe_coincidir() {
        err_contains("fn main() { let x: int = true; }", "se inicializa con bool");
    }

    #[test]
    fn retorno_incorrecto() {
        err_contains("fn f() -> int { true } fn main() {}", "produce bool");
        err_contains("fn g() -> int { return true; } fn main() {}", "se devuelve bool");
    }

    #[test]
    fn retorno_temprano_sin_valor_final_es_valido() {
        // Gracias al análisis de divergencia, esto es válido aunque no tenga
        // expresión final: todos los caminos retornan.
        let src = r#"
fn signo(x: int) -> int {
    if (x < 0) { return -1; } else { return 1; }
}
fn main() -> int { signo(3) }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn llamadas_validan_aridad_y_tipos() {
        err_contains(
            "fn add(a: int, b: int) -> int { a + b } fn main() -> int { add(1) }",
            "espera 2 argumento",
        );
        err_contains(
            "fn add(a: int, b: int) -> int { a + b } fn main() -> int { add(1, true) }",
            "se esperaba int, se pasó bool",
        );
        err_contains("fn main() -> int { desconocida() }", "no declarada");
    }

    #[test]
    fn print_builtin() {
        assert!(check_src("fn main() { print(42); print(\"hola\"); print(true); }").is_ok());
        err_contains("fn main() { print(); }", "espera 1 argumento");
        err_contains("fn main() { print(1, 2); }", "espera 1 argumento");
    }

    #[test]
    fn main_obligatoria_y_bien_formada() {
        err_contains("fn otra() -> int { 0 }", "falta la función de entrada 'main'");
        err_contains("fn main(x: int) -> int { x }", "no debe recibir parámetros");
        err_contains("fn main() -> bool { true }", "debe devolver int o unit");
    }

    #[test]
    fn shadowing_en_bloque_interno() {
        // Una variable interior puede tapar a una exterior con otro tipo.
        let src = r#"
fn main() -> int {
    let x: int = 1;
    { let x: bool = true; print(x); }
    x
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn funcion_no_declarada_dos_veces() {
        err_contains("fn f() {} fn f() {} fn main() {}", "declarada dos veces");
    }

    // ----- M3.1: arreglos -----

    #[test]
    fn arreglos_validos() {
        assert!(check_src("fn main() -> int { let a: [int] = [1, 2, 3]; a[0] }").is_ok());
        assert!(check_src("fn main() -> int { let a: [int] = []; push(a, 1); len(a) }").is_ok());
        assert!(check_src("fn main() { var a: [int] = [1]; a[0] = 9; }").is_ok());
        // Arreglos anidados.
        assert!(check_src("fn main() -> int { let m: [[int]] = [[1, 2], [3, 4]]; m[1][0] }").is_ok());
    }

    #[test]
    fn arreglos_errores_de_tipo() {
        err_contains("fn main() -> int { let a: [int] = [1, true]; a[0] }", "deben ser int");
        err_contains("fn main() -> int { let a: [int] = [1]; a[true] }", "índice debe ser int");
        err_contains("fn main() -> int { let x: int = 5; x[0] }", "no es un arreglo");
        err_contains("fn main() { let x: int = []; }", "no se puede inferir");
        err_contains("fn main() -> int { let a: [int] = [1]; a[0] = true; a[0] }", "se le asigna bool");
        err_contains("fn main() -> int { len(5) }", "len espera un arreglo");
        err_contains("fn main() { let a: [int] = [1]; push(a, true); }", "se empuja bool");
    }

    // ----- M3.2: structs -----

    #[test]
    fn structs_validos() {
        assert!(check_src("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 1, y: 2 }; p.x + p.y }").is_ok());
        assert!(check_src("struct P { x: int } fn main() { var p: P = P { x: 1 }; p.x = 9; }").is_ok());
        // Campos en otro orden: válido.
        assert!(check_src("struct P { x: int, y: int } fn main() -> int { let p: P = P { y: 2, x: 1 }; p.x }").is_ok());
        // Structs anidados y como parámetro.
        assert!(check_src(
            "struct P { x: int } struct L { a: P, b: P }
             fn f(l: L) -> int { l.a.x } fn main() -> int { f(L { a: P { x: 1 }, b: P { x: 2 } }) }"
        ).is_ok());
    }

    #[test]
    fn structs_errores() {
        err_contains("fn main() { let p: Foo = Foo { x: 1 }; }", "no declarado");
        err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: true }; p.x }", "se esperaba int");
        err_contains("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 1 }; p.x }", "falta el campo");
        err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: 1, z: 2 }; p.x }", "no tiene un campo");
        err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: 1 }; p.y }", "no tiene un campo");
        err_contains("struct P { x: int } fn main() -> int { let n: int = 5; n.x }", "no es un struct");
        err_contains("struct P {} struct P {} fn main() {}", "declarado dos veces");
    }

    // ----- M4.1: funciones de primera clase -----

    #[test]
    fn funciones_primera_clase_validas() {
        // Anónima en variable, con su tipo función.
        assert!(check_src("fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x + 1 }; f(2) }").is_ok());
        // De orden superior: recibe y aplica una función.
        assert!(check_src(
            "fn aplicar(f: fn(int) -> int, x: int) -> int { f(x) }
             fn main() -> int { aplicar(fn(n: int) -> int { n * n }, 3) }"
        ).is_ok());
        // Un nombre de función es un valor de tipo función.
        assert!(check_src(
            "fn inc(n: int) -> int { n + 1 }
             fn main() -> int { let g: fn(int) -> int = inc; g(4) }"
        ).is_ok());
        // Devolver una función.
        assert!(check_src(
            "fn dame() -> fn(int) -> int { fn(n: int) -> int { n } }
             fn main() -> int { let f: fn(int) -> int = dame(); f(5) }"
        ).is_ok());
        // Sin argumentos y retorno unit.
        assert!(check_src("fn main() { let f: fn() = fn() { print(1); }; f() }").is_ok());
    }

    #[test]
    fn funciones_primera_clase_errores() {
        // Tipo de la anónima no coincide con la anotación.
        err_contains(
            "fn main() { let f: fn(int) -> int = fn(x: bool) -> int { 0 }; }",
            "se inicializa con",
        );
        // Aridad incorrecta en una llamada indirecta.
        err_contains(
            "fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x }; f(1, 2) }",
            "espera 1 argumento",
        );
        // Tipo de argumento incorrecto en una llamada indirecta.
        err_contains(
            "fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x }; f(true) }",
            "se esperaba int, se pasó bool",
        );
        // Llamar a algo que no es función.
        err_contains("fn main() -> int { let x: int = 3; x(1) }", "no es una función");
        // El cuerpo de la anónima no respeta su tipo de retorno.
        err_contains(
            "fn main() { let f: fn() -> int = fn() -> int { true }; }",
            "produce bool",
        );
    }

    // ----- M4.2: closures (captura de entorno) -----

    #[test]
    fn closures_capturan_el_entorno() {
        // Captura de un `let` externo (lectura).
        assert!(check_src(
            "fn main() -> int { let b: int = 10; let f: fn(int) -> int = fn(x: int) -> int { x + b }; f(1) }"
        ).is_ok());
        // Captura de un `var` externo y su mutación.
        assert!(check_src(
            "fn contador() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
             fn main() -> int { let c: fn() -> int = contador(); c() }"
        ).is_ok());
        // Captura transitiva (dos niveles).
        assert!(check_src(
            "fn sumador(x: int) -> fn(int) -> int { fn(y: int) -> int { x + y } }
             fn main() -> int { let add5: fn(int) -> int = sumador(5); add5(10) }"
        ).is_ok());
    }

    #[test]
    fn closure_no_puede_reasignar_un_let_capturado() {
        // Capturar no reata: asignar a un `let` externo sigue siendo error.
        err_contains(
            "fn main() { let b: int = 1; let f: fn() = fn() { b = 2; }; f() }",
            "es inmutable",
        );
    }

    #[test]
    fn funciones_no_son_comparables() {
        err_contains(
            "fn inc(n: int) -> int { n } fn main() -> int { if (inc == inc) { 1 } else { 0 } }",
            "mismo tipo comparable",
        );
    }

    // ----- M5.1: enums (tipos suma) y construcción -----

    #[test]
    fn enum_construccion_valida() {
        let src = r#"
enum Figura { Circulo(float), Rect(float, float), Punto }
fn area(f: Figura) -> Figura { f }
fn main() {
    let a: Figura = Figura.Circulo(2.0);
    let b: Figura = Figura.Rect(3.0, 4.0);
    let c: Figura = Figura.Punto;
    print(a); print(b); print(c); print(area(a));
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn enum_recursivo_es_valido() {
        // Un enum puede portar su propio tipo: el norte de M5 (listas, árboles).
        let src = r#"
enum Lista { Cons(int, Lista), Nil }
fn main() { let xs: Lista = Lista.Cons(1, Lista.Cons(2, Lista.Nil)); print(xs); }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn enum_variante_inexistente() {
        err_contains("enum E { A, B } fn main() { let x: E = E.C; print(x); }", "no tiene la variante 'C'");
    }

    #[test]
    fn enum_aridad_incorrecta() {
        err_contains("enum E { A(int) } fn main() { let x: E = E.A(1, 2); print(x); }", "espera 1 argumento");
    }

    #[test]
    fn enum_tipo_de_payload_incorrecto() {
        err_contains("enum E { A(int) } fn main() { let x: E = E.A(true); print(x); }", "se esperaba int, se pasó bool");
    }

    #[test]
    fn enum_no_es_comparable() {
        err_contains(
            "enum E { A, B } fn main() -> int { let x: E = E.A; if (x == E.B) { 1 } else { 0 } }",
            "mismo tipo comparable",
        );
    }

    #[test]
    fn enum_y_struct_no_comparten_nombre() {
        err_contains("enum E { A } struct E { x: int } fn main() {}", "no puede ser también un struct");
    }

    #[test]
    fn enum_variante_repetida() {
        err_contains("enum E { A, A } fn main() {}", "variante 'A' repetida");
    }

    #[test]
    fn enum_declarado_dos_veces() {
        err_contains("enum E { A } enum E { B } fn main() {}", "declarado dos veces");
    }

    #[test]
    fn enum_como_tipo_desconocido() {
        // Anotar con un nombre que no es ni struct ni enum.
        err_contains("fn main() { let x: NoExiste = 1; print(x); }", "no declarado");
    }

    // ----- M5.2: match y exhaustividad -----

    #[test]
    fn match_exhaustivo_es_valido() {
        let src = r#"
enum Lista { Cons(int, Lista), Nil }
fn suma(xs: Lista) -> int {
    match (xs) {
        Lista.Cons(h, t) => h + suma(t),
        Lista.Nil => 0,
    }
}
fn main() -> int { suma(Lista.Cons(1, Lista.Nil)) }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn match_con_comodin_es_exhaustivo() {
        let src = "enum E { A, B, C } fn f(e: E) -> int { match (e) { E.A => 1, _ => 0 } } fn main() {}";
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn match_no_exhaustivo() {
        err_contains(
            "enum E { A, B, C } fn f(e: E) -> int { match (e) { E.A => 1, E.B => 2 } } fn main() {}",
            "no exhaustivo",
        );
    }

    #[test]
    fn match_brazos_de_tipos_distintos() {
        err_contains(
            "enum E { A, B } fn f(e: E) -> int { match (e) { E.A => 1, E.B => true } } fn main() {}",
            "tipos distintos",
        );
    }

    #[test]
    fn match_variante_repetida() {
        err_contains(
            "enum E { A, B } fn f(e: E) -> int { match (e) { E.A => 1, E.A => 2, E.B => 3 } } fn main() {}",
            "ya está cubierta",
        );
    }

    #[test]
    fn match_brazo_inalcanzable_tras_catchall() {
        err_contains(
            "enum E { A, B } fn f(e: E) -> int { match (e) { otra => 0, E.A => 1 } } fn main() {}",
            "inalcanzable",
        );
    }

    #[test]
    fn match_aridad_de_binding_incorrecta() {
        err_contains(
            "enum E { A(int) } fn f(e: E) -> int { match (e) { E.A => 1 } } fn main() {}",
            "liga 0 valor(es), pero la variante tiene 1",
        );
    }

    #[test]
    fn match_sobre_no_enum() {
        err_contains(
            "fn f(n: int) -> int { match (n) { _ => 0 } } fn main() {}",
            "match requiere un enum",
        );
    }

    #[test]
    fn match_patron_de_otro_enum() {
        err_contains(
            "enum E { A } enum F { B } fn f(e: E) -> int { match (e) { F.B => 1, _ => 0 } } fn main() {}",
            "es del enum 'F'",
        );
    }

    #[test]
    fn match_liga_payload_para_el_cuerpo() {
        // El binding del payload debe estar disponible (y bien tipado) en el cuerpo.
        let src = "enum Caja { Con(int), Vacia } fn val(c: Caja) -> int { match (c) { Caja.Con(n) => n + 1, Caja.Vacia => 0 } } fn main() {}";
        assert!(check_src(src).is_ok());
    }

    // ----- M6.1: funciones genéricas e inferencia -----

    #[test]
    fn generica_identidad_y_uso() {
        let src = r#"
fn identidad<T>(x: T) -> T { x }
fn main() -> int {
    let a: int = identidad(5);
    let b: bool = identidad(true);
    if (b) { a } else { 0 }
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn generica_infiere_de_varios_argumentos() {
        // [T] y fn(T)->U determinan T y U a la vez.
        let src = r#"
fn aplicar<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }
fn doble(n: int) -> int { n * 2 }
fn main() -> int { aplicar(doble, 21) }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn generica_T_inconsistente() {
        err_contains(
            "fn par<T>(a: T, b: T) -> T { a } fn main() -> int { par(1, true) }",
            "no puede ser int y bool",
        );
    }

    #[test]
    fn generica_T_no_inferible() {
        err_contains(
            "fn vacio<T>() -> int { 0 } fn main() -> int { vacio() }",
            "no se pudo inferir el parámetro de tipo 'T'",
        );
    }

    #[test]
    fn generica_como_valor_es_error() {
        err_contains(
            "fn id<T>(x: T) -> T { x } fn main() -> int { let f: fn(int) -> int = id; f(3) }",
            "función genérica 'id' como valor",
        );
    }

    #[test]
    fn generica_no_se_puede_comparar_un_parametro_de_tipo() {
        err_contains(
            "fn ig<T>(a: T, b: T) -> bool { a == b } fn main() {}",
            "mismo tipo comparable",
        );
    }

    #[test]
    fn parametro_de_tipo_repetido() {
        err_contains("fn f<T, T>(x: T) -> T { x } fn main() {}", "parámetro de tipo 'T' repetido");
    }

    #[test]
    fn tipo_desconocido_no_es_parametro() {
        err_contains("fn f(x: Desconocido) -> int { 0 } fn main() {}", "'Desconocido' no declarado");
    }

    // ----- M6.2: tipos genéricos del usuario y chequeo bidireccional -----

    #[test]
    fn enum_generico_construccion_y_match() {
        let src = r#"
enum Caja<T> { Llena(T), Vacia }
fn val(c: Caja<int>, def: int) -> int {
    match (c) { Caja.Llena(v) => v, Caja.Vacia => def }
}
fn main() -> int {
    let a: Caja<int> = Caja.Llena(7);   // T=int del argumento
    let b: Caja<int> = Caja.Vacia;       // T=int del tipo esperado
    val(a, 0) + val(b, 35)
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn struct_generico_campo_sustituido() {
        let src = r#"
struct Par<A, B> { primero: A, segundo: B }
fn main() -> int {
    let p: Par<int, bool> = Par { primero: 10, segundo: true };
    if (p.segundo) { p.primero } else { 0 }
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn generico_mismatch_de_argumento_de_tipo() {
        err_contains(
            "enum Caja<T> { Llena(T), Vacia } fn main() { let b: Caja<bool> = Caja.Llena(7); print(b); }",
            "no puede ser bool y int",
        );
    }

    #[test]
    fn generico_aridad_de_args_de_tipo() {
        err_contains(
            "enum Caja<T> { Llena(T), Vacia } fn main() { let b: Caja<int, bool> = Caja.Vacia; print(b); }",
            "espera 1 argumento(s) de tipo",
        );
    }

    #[test]
    fn generico_vacio_no_inferible_sin_contexto() {
        // Sin tipo esperado ni argumentos, T queda sin determinar.
        err_contains(
            "enum Caja<T> { Llena(T), Vacia } fn main() { print(Caja.Vacia); }",
            "no se pudo inferir",
        );
    }

    #[test]
    fn parametro_de_tipo_de_enum_repetido() {
        err_contains("enum E<T, T> { A(T) } fn main() {}", "parámetro de tipo 'T' repetido");
    }

    #[test]
    fn arreglo_vacio_adopta_el_tipo_esperado() {
        // El chequeo bidireccional arregla la aspereza histórica del [] vacío.
        assert!(check_src("fn main() -> int { let xs: [int] = []; len(xs) }").is_ok());
    }

    // ----- M6.3: Option/Result (prelude) y el operador ? -----

    #[test]
    fn prelude_option_result_disponibles() {
        // Sin declararlos, Option y Result existen (vienen del prelude).
        let src = r#"
fn f() -> Result<int, string> { Result.Ok(1) }
fn g() -> Option<int> { Option.None }
fn main() {}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn try_result_y_option_validos() {
        let src = r#"
fn d(a: int, b: int) -> Result<int, string> {
    if (b == 0) { Result.Err("cero") } else { Result.Ok(a / b) }
}
fn calc(x: int, y: int) -> Result<int, string> {
    let q: int = d(x, y)?;
    Result.Ok(q + 1)
}
fn raw(xs: [int]) -> Option<int> { if (len(xs) == 0) { Option.None } else { Option.Some(xs[0]) } }
fn primero(xs: [int]) -> Option<int> {
    let v: int = raw(xs)?;
    Option.Some(v)
}
fn main() {}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn try_requiere_result_u_option() {
        err_contains(
            "fn f() -> Result<int, string> { let x: int = 5?; Result.Ok(x) } fn main() {}",
            "requiere un Result o un Option",
        );
    }

    #[test]
    fn try_funcion_debe_devolver_compatible() {
        err_contains(
            "fn d() -> Result<int, string> { Result.Ok(1) } fn g() -> int { let x: int = d()?; x } fn main() {}",
            "requiere que la función devuelva Result",
        );
    }

    #[test]
    fn try_result_con_E_distinto() {
        err_contains(
            "fn d() -> Result<int, string> { Result.Ok(1) } fn f() -> Result<int, bool> { let x: int = d()?; Result.Ok(x) } fn main() {}",
            "Result<_, string>",
        );
    }

    // ----- UFCS (M7.1) -----

    #[test]
    fn ufcs_funcion_libre_como_metodo() {
        // recv.f(args) ≡ f(recv, args). Builtin (len) y función del usuario (suma).
        let src = r#"
fn suma(a: int, b: int) -> int { a + b }
fn main() -> int {
    let xs: [int] = [1, 2, 3];
    let n: int = xs.len();      // len(xs)
    let v: int = 10;
    v.suma(n)                    // suma(10, 3)
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn ufcs_no_es_campo_usa_funcion() {
        // 'doble' no es campo de Punto: se resuelve como UFCS doble(p).
        let src = r#"
struct Punto { x: int, y: int }
fn doble(p: Punto) -> int { (p.x + p.y) * 2 }
fn main() -> int {
    let p: Punto = Punto { x: 3, y: 4 };
    p.doble()
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn ufcs_campo_funcion_gana_sobre_libre() {
        // 'op' ES un campo (de tipo función): c.op(x) llama al campo, no es UFCS, aunque
        // exista una función libre 'op' homónima con otra firma.
        let src = r#"
fn op(a: int, b: int) -> int { a - b }
struct Caja { op: fn(int) -> int }
fn main() -> int {
    let c: Caja = Caja { op: fn(x: int) -> int { x + 1 } };
    c.op(41)                     // (c.op)(41) = 42, NO op(c, 41)
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn ufcs_encadenado() {
        let src = r#"
fn doble(x: int) -> int { x * 2 }
fn inc(x: int) -> int { x + 1 }
fn main() -> int {
    let v: int = 5;
    v.doble().inc().doble()      // doble(inc(doble(5)))
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn ufcs_metodo_inexistente() {
        err_contains(
            "fn main() -> int { let v: int = 5; v.frobnicate() }",
            "no existe campo ni función 'frobnicate' aplicable a int",
        );
    }

    #[test]
    fn ufcs_receptor_de_tipo_incorrecto() {
        // El receptor se inserta como primer argumento: si su tipo no encaja, error.
        err_contains(
            "fn doble(x: int) -> int { x * 2 } fn main() -> int { let b: bool = true; b.doble() }",
            "se esperaba int, se pasó bool",
        );
    }

    #[test]
    fn ufcs_generico_infiere_desde_receptor() {
        // El receptor cuenta para la inferencia de genéricos (M6) como cualquier arg.
        let src = r#"
fn primero<T>(xs: [T]) -> T { xs[0] }
fn main() -> int {
    let xs: [int] = [7, 8, 9];
    xs.primero()                 // primero(xs) con T = int
}
"#;
        assert!(check_src(src).is_ok());
    }

    // ----- M7.3: stdlib (prelude de orden superior: map/filter/fold) -----

    #[test]
    fn prelude_map_filter_fold_tipan() {
        // Disponibles sin declararlas; se infieren los genéricos en cada uso.
        let src = r#"
fn doble(x: int) -> int { x * 2 }
fn par(x: int) -> bool { x % 2 == 0 }
fn suma(a: int, b: int) -> int { a + b }
fn main() -> int {
    let xs: [int] = [1, 2, 3, 4];
    let ys: [int] = xs.map(doble).filter(par);
    ys.fold(0, suma)
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn prelude_fold_a_tipo_distinto() {
        // fold<T, A>: el acumulador A puede diferir del elemento T (aquí bool).
        let src = r#"
fn main() -> int {
    let xs: [int] = [2, 4, 6];
    let todos: bool = xs.fold(true, fn(acc: bool, x: int) -> bool { acc && (x % 2 == 0) });
    if (todos) { 1 } else { 0 }
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn prelude_map_exige_funcion_compatible() {
        // map<T,U>(xs:[T], f:fn(T)->U): una f con dominio incompatible hace que el
        // parámetro de tipo T se exija int (por xs) y bool (por f) a la vez: error.
        err_contains(
            "fn f(b: bool) -> int { 1 } fn main() -> int { let xs: [int] = [1]; let ys: [int] = xs.map(f); ys[0] }",
            "no puede ser int y bool",
        );
    }

    #[test]
    fn prelude_usuario_puede_redefinir() {
        // Si el usuario define 'map', el del prelude se omite (override).
        let src = r#"
fn map(x: int) -> int { x + 1 }
fn main() -> int { map(41) }
"#;
        assert!(check_src(src).is_ok());
    }

    // ----- M8.1: inferencia local (let/var sin anotación) -----

    #[test]
    fn infiere_primitivos_y_compuestos() {
        let src = r#"
struct Punto { x: int, y: int }
enum Caja<T> { Llena(T), Vacia }
fn main() -> int {
    let x = 3;                      // int
    let f = 2.5;                    // float
    let b = x > 1;                  // bool
    let s = "hola";                 // string
    let xs = [10, 20, 30];          // [int]
    let p = Punto { x: 7, y: 6 };   // Punto
    let c = Caja.Llena(5);          // Caja<int> (genéricos M6)
    let cv = p.x + p.y;             // int, del campo inferido
    let dentro = match (c) { Caja.Llena(v) => v, Caja.Vacia => 0 };  // int
    x + xs[0] + cv + dentro
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn variable_inferida_conserva_su_tipo() {
        // Una inferida como int no puede luego usarse como bool.
        err_contains(
            "fn main() -> int { let x = 3; if (x) { 0 } else { 1 } }",
            "condición del if debe ser bool",
        );
    }

    #[test]
    fn var_inferida_es_mutable_y_tipada() {
        // 'var t = 0' infiere int y es mutable; asignarle bool falla.
        assert!(check_src("fn main() -> int { var t = 0; t = t + 1; t }").is_ok());
        err_contains(
            "fn main() -> int { var t = 0; t = true; t }",
            "es int pero se le asigna bool",
        );
    }

    #[test]
    fn let_inferida_sigue_siendo_inmutable() {
        // La inferencia no cambia la mutabilidad: un 'let' inferido no se puede reasignar.
        err_contains(
            "fn main() -> int { let x = 3; x = 4; x }",
            "inmutable",
        );
    }

    #[test]
    fn inferencia_no_aplica_a_lo_indeterminado() {
        // Sin anotación, '[]' no se puede inferir: pide la anotación.
        err_contains(
            "fn main() -> int { let xs = []; len(xs) }",
            "no se puede inferir el tipo de []",
        );
    }

    #[test]
    fn anotacion_sigue_validandose() {
        // Con anotación, un inicializador incompatible sigue siendo error.
        err_contains(
            "fn main() -> int { let x: int = true; x }",
            "se inicializa con bool",
        );
    }

    // ----- M9.1: traits -----

    #[test]
    fn trait_e_impl_validos() {
        check_src(r#"
            trait Mostrable { fn mostrar(self) -> string; }
            struct Punto { x: int, y: int }
            impl Mostrable for Punto { fn mostrar(self) -> string { "p" } }
            fn main() -> int { let p = Punto { x: 1, y: 2 }; print(p.mostrar()); 0 }
        "#).expect("trait/impl válidos");
    }

    #[test]
    fn trait_para_enum_y_primitivo() {
        check_src(r#"
            trait Valor { fn valor(self) -> int; }
            enum Moneda { Cara, Cruz }
            impl Valor for Moneda { fn valor(self) -> int { match (self) { Moneda.Cara => 1, Moneda.Cruz => 0 } } }
            impl Valor for int { fn valor(self) -> int { self } }
            fn main() -> int { Moneda.Cara.valor() + 5.valor() }
        "#).expect("impl para enum y primitivo");
    }

    #[test]
    fn self_en_retorno_y_metodo_interno() {
        check_src(r#"
            trait P { fn sumar(self, o: Punto) -> Punto; fn doble(self) -> Self; }
            struct Punto { x: int, y: int }
            impl P for Punto {
                fn sumar(self, o: Punto) -> Punto { Punto { x: self.x + o.x, y: self.y + o.y } }
                fn doble(self) -> Self { self.sumar(self) }
            }
            fn main() -> int { let p = Punto { x: 1, y: 2 }; let q = p.doble(); q.x }
        "#).expect("Self en retorno y self.metodo() interno");
    }

    #[test]
    fn campo_gana_sobre_metodo_de_trait() {
        // Un campo función del struct tiene prioridad sobre un método de trait homónimo.
        check_src(r#"
            trait T { fn f(self) -> int; }
            struct S { f: fn() -> int, x: int }
            impl T for S { fn f(self) -> int { self.x } }
            fn cero() -> int { 0 }
            fn main() -> int { let s = S { f: cero, x: 9 }; s.f() }
        "#).expect("el campo 'f' gana: se invoca el valor del campo, no el método");
    }

    #[test]
    fn impl_no_cubre_todos_los_metodos() {
        err_contains(
            r#"trait T { fn a(self) -> int; fn b(self) -> int; }
               struct S { x: int }
               impl T for S { fn a(self) -> int { self.x } }
               fn main() -> int { 0 }"#,
            "no implementa el método 'b'",
        );
    }

    #[test]
    fn impl_con_firma_distinta() {
        err_contains(
            r#"trait T { fn a(self) -> int; }
               struct S { x: int }
               impl T for S { fn a(self) -> bool { true } }
               fn main() -> int { 0 }"#,
            "devuelve bool, pero el trait pide int",
        );
    }

    #[test]
    fn metodo_ambiguo_entre_dos_traits() {
        err_contains(
            r#"trait A { fn f(self) -> int; }
               trait B { fn f(self) -> int; }
               struct S { x: int }
               impl A for S { fn f(self) -> int { 1 } }
               impl B for S { fn f(self) -> int { 2 } }
               fn main() -> int { 0 }"#,
            "ambiguo",
        );
    }

    #[test]
    fn impl_de_trait_inexistente() {
        err_contains(
            r#"struct S { x: int }
               impl NoExiste for S { fn f(self) -> int { 1 } }
               fn main() -> int { 0 }"#,
            "trait 'NoExiste' no declarado",
        );
    }

    #[test]
    fn impl_concreto_sobre_tipo_generico_es_error() {
        // `impl T for Caja` sin declarar los parámetros de tipo: M9.2b pide `impl<A> T for
        // Caja<A>`. El error guía hacia esa forma.
        err_contains(
            r#"trait T { fn f(self) -> int; }
               struct Caja<A> { v: A }
               impl T for Caja { fn f(self) -> int { 1 } }
               fn main() -> int { 0 }"#,
            "es genérico: declara sus parámetros en el impl",
        );
    }

    #[test]
    fn indice_semantico_hover_de_variable() {
        // M10.2b: el índice registra el tipo de un uso de identificador.
        let src = "fn main() -> int {\n  let x = 5;\n  x\n}";
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        let idx = semantic_index(&mut prog);
        let h = idx.hovers.iter().find(|h| h.line == 3 && h.col == 3).expect("hover de x");
        assert_eq!(h.text, "x: int");
        assert_eq!(h.len, 1);
        // Y registra su definición (el `let` de la línea 2).
        let d = idx.defs.iter().find(|d| d.line == 3 && d.col == 3).expect("def de x");
        assert_eq!((d.def_line, d.def_col), (2, 3));
    }

    #[test]
    fn indice_semantico_hover_de_tipo() {
        // M10.2f: el índice registra el uso de un nombre de tipo en un literal de struct y la
        // construcción de un enum, con su posición de declaración (ir-a-definición).
        let src = "struct Punto { x: int }\nenum Color { Rojo }\nfn main() -> int {\n  let p = Punto { x: 1 };\n  let c = Color.Rojo;\n  p.x\n}";
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        let idx = semantic_index(&mut prog);
        // Hover del nombre `Punto` en el literal (línea 4, col 11).
        let h = idx.hovers.iter().find(|h| h.line == 4 && h.col == 11).expect("hover de Punto");
        assert_eq!(h.text, "struct Punto");
        // Def → la declaración del struct (línea 1).
        let d = idx.defs.iter().find(|d| d.line == 4 && d.col == 11).expect("def de Punto");
        assert_eq!(d.def_line, 1);
        // Hover del enum `Color` en la construcción (línea 5).
        let he = idx.hovers.iter().find(|h| h.line == 5 && h.text == "enum Color").expect("hover de Color");
        assert_eq!(he.line, 5);
    }

    #[test]
    fn impl_generico_valido() {
        // M9.2b-1: `impl<A> T for Caja<A>` con un método que no usa A.
        assert!(check_src(
            r#"trait T { fn f(self) -> int; }
               struct Caja<A> { v: A }
               impl<A> T for Caja<A> { fn f(self) -> int { 1 } }
               fn main() -> int { let c = Caja { v: 9 }; c.f() }"#,
        ).is_ok());
    }

    #[test]
    fn impl_generico_objetivo_mal_formado_es_error() {
        // El objetivo de un impl genérico debe ser `Caja<A>` con sus propios parámetros.
        err_contains(
            r#"trait T { fn f(self) -> int; }
               struct Caja<A> { v: A }
               impl<A> T for Caja<int> { fn f(self) -> int { 1 } }
               fn main() -> int { 0 }"#,
            "debe aplicarse a 'Caja<A>'",
        );
    }

    #[test]
    fn self_fuera_de_impl_es_error() {
        err_contains(
            "fn f(x: Self) -> int { 1 } fn main() -> int { 0 }",
            "'Self' solo es válido dentro de un trait o impl",
        );
    }

    #[test]
    fn metodo_inexistente_no_es_campo_ni_funcion() {
        err_contains(
            r#"struct S { x: int }
               fn main() -> int { let s = S { x: 1 }; s.noexiste() }"#,
            "no existe campo ni función",
        );
    }

    // ----- M9.2: bounds de genéricos -----

    #[test]
    fn bound_concreto_y_reenvio() {
        check_src(r#"
            trait Valor { fn valor(self) -> int; }
            struct Punto { x: int }
            impl Valor for Punto { fn valor(self) -> int { self.x } }
            impl Valor for int { fn valor(self) -> int { self } }
            fn doble<T: Valor>(x: T) -> int { x.valor() + x.valor() }
            fn pasar<T: Valor>(x: T) -> int { doble(x) }
            fn main() -> int {
                let p = Punto { x: 5 };
                doble(p) + doble(9) + pasar(p)
            }
        "#).expect("bound concreto + reenvío");
    }

    #[test]
    fn bounds_multiples() {
        check_src(r#"
            trait A { fn a(self) -> int; }
            trait B { fn b(self) -> int; }
            struct S { x: int }
            impl A for S { fn a(self) -> int { self.x } }
            impl B for S { fn b(self) -> int { self.x } }
            fn usar<T: A + B>(x: T) -> int { x.a() + x.b() }
            fn main() -> int { let s = S { x: 1 }; usar(s) }
        "#).expect("T: A + B");
    }

    #[test]
    fn bound_tipo_no_implementa() {
        err_contains(
            r#"trait Valor { fn valor(self) -> int; }
               struct Punto { x: int }
               fn usar<T: Valor>(x: T) -> int { x.valor() }
               fn main() -> int { let p = Punto { x: 1 }; usar(p) }"#,
            "Punto no implementa 'Valor'",
        );
    }

    #[test]
    fn bound_metodo_fuera_del_trait() {
        err_contains(
            r#"trait Valor { fn valor(self) -> int; }
               fn usar<T: Valor>(x: T) -> int { x.otro() }
               fn main() -> int { 0 }"#,
            "no existe campo ni función 'otro'",
        );
    }

    #[test]
    fn reenvio_sin_bound_es_error() {
        err_contains(
            r#"trait Valor { fn valor(self) -> int; }
               fn usar<T: Valor>(x: T) -> int { x.valor() }
               fn intermediario<U>(y: U) -> int { usar(y) }
               fn main() -> int { 0 }"#,
            "no está acotado por 'Valor'",
        );
    }

    #[test]
    fn bound_a_trait_inexistente() {
        err_contains(
            "fn usar<T: NoExiste>(x: T) -> int { 0 } fn main() -> int { 0 }",
            "trait 'NoExiste' no declarado",
        );
    }

    // ----- M9.3a: métodos por defecto -----

    #[test]
    fn metodo_por_defecto_heredado_y_redefinido() {
        check_src(r#"
            trait Valor {
                fn base(self) -> int;
                fn doble(self) -> int { self.base() + self.base() }
            }
            struct A { n: int }
            impl Valor for A { fn base(self) -> int { self.n } }
            struct B { n: int }
            impl Valor for B { fn base(self) -> int { self.n } fn doble(self) -> int { 0 } }
            fn main() -> int {
                let a = A { n: 1 };
                let b = B { n: 2 };
                a.doble() + b.doble()
            }
        "#).expect("defecto heredado por A, redefinido por B");
    }

    #[test]
    fn metodo_requerido_sin_defecto_sigue_obligatorio() {
        err_contains(
            r#"trait T { fn req(self) -> int; fn opt(self) -> int { 0 } }
               struct S { x: int }
               impl T for S { fn opt(self) -> int { self.x } }
               fn main() -> int { 0 }"#,
            "no implementa el método 'req'",
        );
    }

    #[test]
    fn metodo_por_defecto_via_bound() {
        check_src(r#"
            trait Saludo {
                fn nombre(self) -> int;
                fn doble(self) -> int { self.nombre() + self.nombre() }
            }
            struct P { v: int }
            impl Saludo for P { fn nombre(self) -> int { self.v } }
            fn usar<T: Saludo>(x: T) -> int { x.doble() }
            fn main() -> int { let p = P { v: 1 }; usar(p) }
        "#).expect("defecto invocado vía bound");
    }

    // ----- M9.3b: trait objects -----

    #[test]
    fn trait_object_coercion_y_despacho() {
        check_src(r#"
            trait Figura { fn area(self) -> int; }
            struct Cuadrado { lado: int }
            impl Figura for Cuadrado { fn area(self) -> int { self.lado * self.lado } }
            struct Rect { ancho: int, alto: int }
            impl Figura for Rect { fn area(self) -> int { self.ancho * self.alto } }
            fn total(xs: [dyn Figura]) -> int {
                var s = 0; var i = 0;
                while (i < len(xs)) { s = s + xs[i].area(); i = i + 1; }
                s
            }
            fn main() -> int {
                let fs: [dyn Figura] = [Cuadrado { lado: 2 }, Rect { ancho: 3, alto: 4 }];
                total(fs)
            }
        "#).expect("arreglo heterogéneo de trait objects + despacho");
    }

    #[test]
    fn trait_object_tipo_no_implementa() {
        err_contains(
            r#"trait Figura { fn area(self) -> int; }
               struct P { x: int }
               fn main() -> int { let f: dyn Figura = P { x: 1 }; 0 }"#,
            "no implementa 'Figura'",
        );
    }

    #[test]
    fn trait_object_object_safety() {
        err_contains(
            r#"trait Clon { fn copia(self) -> Self; }
               struct P { x: int }
               impl Clon for P { fn copia(self) -> Self { P { x: self.x } } }
               fn usar(p: dyn Clon) -> int { let q = p.copia(); 0 }
               fn main() -> int { 0 }"#,
            "usa 'Self': no es invocable sobre 'dyn Clon'",
        );
    }

    #[test]
    fn dyn_de_trait_inexistente() {
        err_contains(
            "fn f(x: dyn NoExiste) -> int { 0 } fn main() -> int { 0 }",
            "trait 'NoExiste' no declarado",
        );
    }

    // ----- M10.1: anotaciones -----

    #[test]
    fn test_valido() {
        check_src(r#"
            @test
            fn ok() -> bool { 1 + 1 == 2 }
            fn main() -> int { 0 }
        "#).expect("@test con firma () -> bool");
    }

    #[test]
    fn test_firma_incorrecta() {
        err_contains(
            "@test fn malo() -> int { 1 } fn main() -> int { 0 }",
            "debe devolver bool",
        );
    }

    #[test]
    fn test_con_parametros() {
        err_contains(
            "@test fn malo(x: int) -> bool { true } fn main() -> int { 0 }",
            "no debe recibir parámetros",
        );
    }

    #[test]
    fn anotacion_desconocida() {
        err_contains(
            "@magia fn f() -> bool { true } fn main() -> int { 0 }",
            "anotación desconocida: '@magia'",
        );
    }

    #[test]
    fn test_sobre_struct_es_error() {
        err_contains(
            "@test struct S { x: int } fn main() -> int { 0 }",
            "'@test' solo se permite sobre funciones",
        );
    }

    #[test]
    fn derive_eq_struct_y_enum() {
        check_src(r#"
            @derive(Eq)
            struct Punto { x: int, y: int }
            @derive(Eq)
            enum Color { Rojo, Verde, Azul }
            @derive(Eq)
            enum Forma { Circulo(int), Rect(int, int) }
            fn main() -> int {
                let p = Punto { x: 1, y: 2 };
                let c = Color.Rojo;
                let f = Forma.Rect(1, 2);
                if (p.igual(p)) { 0 } else { 1 }
            }
        "#).expect("@derive(Eq) para struct y enum (unit y con payload)");
    }

    #[test]
    fn derive_eq_compone_con_bound() {
        check_src(r#"
            @derive(Eq)
            enum Color { Rojo, Verde }
            fn iguales<T: Eq>(a: T, b: T) -> bool { a.igual(b) }
            fn main() -> int { if (iguales(Color.Rojo, Color.Rojo)) { 0 } else { 1 } }
        "#).expect("un tipo derivado satisface el bound T: Eq");
    }

    #[test]
    fn derive_trait_no_soportado() {
        err_contains(
            "@derive(Ord) struct P { x: int } fn main() -> int { 0 }",
            "no se sabe derivar 'Ord'",
        );
    }

    #[test]
    fn derive_en_tipo_generico_es_error() {
        err_contains(
            "@derive(Eq) struct Caja<T> { v: T } fn main() -> int { 0 }",
            "tipo genérico",
        );
    }

    #[test]
    fn derive_show_struct_y_enum() {
        check_src(r#"
            @derive(Show)
            struct Punto { x: int, y: int }
            @derive(Show)
            enum Color { Rojo, RGB(int, int, int) }
            @derive(Show)
            struct Etiqueta { nombre: string, donde: Punto, color: Color }
            fn main() -> int {
                let e = Etiqueta { nombre: "o", donde: Punto { x: 1, y: 2 }, color: Color.Rojo };
                print(e.mostrar());
                0
            }
        "#).expect("@derive(Show) para struct, enum y struct anidado");
    }

    #[test]
    fn derive_eq_y_show_juntos() {
        check_src(r#"
            @derive(Eq, Show)
            struct P { x: int }
            fn main() -> int { if (P { x: 1 }.igual(P { x: 1 })) { 0 } else { 1 } }
        "#).expect("@derive(Eq, Show) genera ambos impls");
    }

    #[test]
    fn derive_show_campo_no_soportado_es_error() {
        err_contains(
            "@derive(Show) struct S { xs: [int] } fn main() -> int { 0 }",
            "no se puede derivar Show para un campo de tipo [int]",
        );
    }
}
