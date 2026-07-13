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
        write!(f, "type error at {}:{}: {}", self.line, self.col, self.msg)
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
const MAX_ERRORS: usize = 20;

/// Variante acumuladora de `check` (M33c): devuelve TODOS los errores de tipos (hasta
/// `MAX_ERRORES`), con granularidad por función — las pasadas tempranas siguen fail-fast.
/// **Solo diagnóstico** (LSP/CLI): omite el lowering, porque con errores no se ejecuta
/// nada. El primer error es idéntico al de `check` (mismo recorrido hasta ahí).
pub fn check_all(program: &mut Program) -> Vec<TypeError> {
    check_all_impl(program, true)
}

/// Como [`check_all`], pero **sin exigir `main`** (M10.2): para que el LSP analice un archivo de
/// **módulo** (submódulo `pub` sin función de entrada) y aun así reporte los diagnósticos reales de
/// sus cuerpos, en vez de cortar con "falta la función de entrada 'main'". La entrada de un proyecto
/// (con `main`) se sigue analizando con `check_all`.
pub fn check_all_modulo(program: &mut Program) -> Vec<TypeError> {
    check_all_impl(program, false)
}

fn check_all_impl(program: &mut Program, require_main: bool) -> Vec<TypeError> {
    if let Err(e) = check_builtin_redefinition(program) {
        return vec![e];
    }
    if let Err(e) = prepare_program(program) {
        return vec![e];
    }
    let mut checker = Checker::new();
    checker.accumulate = true;
    checker.require_main = require_main;
    match checker.check_program(program) {
        Err(e) => vec![e], // pasada temprana (fail-fast): un solo error
        Ok(()) => checker.errors,
    }
}

/// M48.3: un builtin del lenguaje (`len`, `push`, `insert`, `print`…) NO puede redefinirse como
/// función libre — se resuelve **antes** que cualquier función del usuario, así que un `fn len` sería
/// inalcanzable (shadowing silencioso al revés). Se comprueba ANTES de inyectar el prelude (aquí
/// `program.functions` son solo las del usuario ya fusionadas): las de un módulo van namespacadas
/// (`M::len`) → no colisionan; solo las del archivo de entrada (nombre pelado) o traídas sin calificar
/// llegan como `len`. Las funciones del prelude (map/filter/fold/sort/assert…) NO son builtins → el
/// usuario SÍ puede redefinirlas (override). Los internos `__x` no son de cara al usuario. Lo llaman
/// tanto `check` (fail-fast) como `check_all` (recuperación de errores) para que el mensaje se emita.
fn check_builtin_redefinition(program: &Program) -> Result<(), TypeError> {
    for f in &program.functions {
        if !f.name.contains("::") && !f.name.contains('#') && !f.name.starts_with("__")
            && crate::builtins::is_builtin(&f.name)
        {
            return Err(TypeError {
                msg: format!("'{}' is a language builtin and cannot be redefined", f.name),
                line: f.line,
                col: f.col,
                len: f.name.chars().count(),
            });
        }
    }
    Ok(())
}

pub fn check(program: &mut Program) -> Result<(), TypeError> {
    check_builtin_redefinition(program)?;
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
    // M40.2: reescribir `for x in it` (sobre un iterador) a `ForIter::Iter`, con el `next` manglado.
    lower_for_iters(program, &checker.for_iter_sites);
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
    // Paso 8 (M52): inlining de forwarders triviales — una llamada a un método manglado cuyo
    // cuerpo es exactamente `__builtin(params en orden)` (los impls-para-builtins de M48.4,
    // p. ej. `[T]#push`) se reescribe a la llamada al builtin directamente, recuperando el
    // opcode directo en la VM (y ahorrando el marco en el intérprete). Semántica idéntica.
    inline_forwarders(program);
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
    // Sin exigir `main`: el chequeo de `main` es fail-fast ANTES de recorrer los cuerpos (donde se
    // recogen los hovers/defs). Un archivo de módulo (submódulo sin entrada) cortaría ahí y no
    // reuniría nada del cuerpo → hover/def no funcionarían. Es introspección, no ejecución.
    checker.require_main = false;
    checker.field_name_pos = program.field_name_pos.clone(); // posiciones de nombres de campo/método
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
    // Paso 0a (M40.2b): inyectar los structs del prelude (`ArrayIter`/`RangeIter` para `.iter()`/
    // `range`) que el usuario no haya redefinido. Como los enums, quedan en el AST que también ven
    // el intérprete y la VM. Idempotente (si se re-verifica, no duplica).
    let structs_user: HashSet<String> = program.structs.iter().map(|s| s.name.clone()).collect();
    let mut prelude_structs: Vec<StructDef> = crate::prelude::structs()
        .into_iter()
        .filter(|s| !structs_user.contains(&s.name))
        .collect();
    if !prelude_structs.is_empty() {
        prelude_structs.append(&mut program.structs);
        program.structs = prelude_structs;
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
    let traits_user: HashSet<String> = program.traits.iter().map(|t| t.name.clone()).collect();
    let mut prelude_traits: Vec<TraitDef> = crate::prelude::traits()
        .into_iter()
        .filter(|t| !traits_user.contains(&t.name))
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
    // M40.2c: parámetros de tipo de cada trait (`trait Iterator<T>` → ["T"]), para sustituirlos por
    // los argumentos del impl al bajar sus métodos (un `impl Iterator<int>` fija `T = int`).
    let trait_tparams: HashMap<String, Vec<String>> = program.traits.iter()
        .map(|t| (t.name.clone(), t.type_params.clone()))
        .collect();
    // Contador para renumerar las posiciones de cada cuerpo por defecto clonado (M9.3a):
    // cada clon recibe posiciones únicas para que las bajadas por posición no colisionen.
    let mut fresh_pos = 0usize;
    for imp in &program.impls {
        let key = match type_key_of(&imp.target) {
            Some(k) => k,
            None => continue, // objetivo inválido: el error se da en la validación
        };
        // M40.2c: σ que lleva cada parámetro de tipo del trait a su argumento en este impl
        // (`impl Iterator<int>` → {T: int}). Vacío para un trait sin parámetros (M9). Se aplica,
        // tras `subst_self`, a los tipos de los métodos bajados (firma), así heredan `T` concreto.
        let trait_sigma: HashMap<String, Type> = trait_tparams.get(&imp.trait_name)
            .map(|tps| tps.iter().cloned().zip(imp.trait_args.iter().cloned()).collect())
            .unwrap_or_default();
        // Métodos provistos por el impl. M9.2b: un impl genérico (`impl<T: B> Trait for
        // Caja<T>`) baja sus métodos a funciones **genéricas acotadas** (heredan los
        // `type_params`/`bounds` del impl); de ahí, `append_dict_params` y
        // `resolve_bound_method` los tratan como cualquier función con bounds. Para un impl
        // concreto (M9.1) ambos son vacíos → función ordinaria, como antes.
        for m in &imp.methods {
            let params = m.params.iter()
                .map(|p| Param { ty: subst_named(&subst_self(&p.ty, &imp.target), &trait_sigma), ..p.clone() })
                .collect();
            // M28.2: un método de un impl `From<S>` se inyecta con nombre manglado **por origen**
            // (`E#from#string`) para no colisionar con otros `impl From<...> for E`. El resto
            // (impls de M9) usa el manglado ordinario `Tipo#metodo`.
            let name = if is_typed_trait_impl(imp) && m.name == "convert" {
                let src_key = imp.trait_args.first().and_then(type_key_of).unwrap_or_default();
                mangle_from(&key, &src_key)
            } else {
                mangle(&key, &m.name)
            };
            // M40.2c: el manglado hereda los type_params/bounds del impl (M9.2b) MÁS los propios
            // del método (`fn map<U>`). Así `Iter#map<T, U>` es una función genérica: la inferencia
            // fija T por el receptor y U por el argumento `f`.
            let mut type_params = imp.type_params.clone();
            type_params.extend(m.type_params.iter().cloned());
            let mut bounds = imp.bounds.clone();
            bounds.extend(m.bounds.iter().cloned());
            let mut body = m.body.clone();
            if !trait_sigma.is_empty() { subst_named_block(&mut body, &trait_sigma); }
            program.functions.push(Function {
                annotations: Vec::new(),
                is_pub: false,
                name,
                type_params,
                bounds,
                params,
                return_type: subst_named(&subst_self(&m.return_type, &imp.target), &trait_sigma),
                body,
                line: m.line,
                col: m.col,
            });
        }
        // M9.3a: métodos por defecto no redefinidos → se sintetizan desde el cuerpo del
        // trait (con `Self` = el tipo destino). El impl los hereda como funciones propias.
        let provided: HashSet<&str> = imp.methods.iter().map(|m| m.name.as_str()).collect();
        for tm in trait_sigs.get(&imp.trait_name).into_iter().flatten() {
            let Some(body) = &tm.default_body else { continue };
            if provided.contains(tm.name.as_str()) {
                continue; // el impl lo redefine: gana el del impl
            }
            let params = tm.params.iter()
                .map(|p| Param { ty: subst_named(&subst_self(&p.ty, &imp.target), &trait_sigma), ..p.clone() })
                .collect();
            // Clonar el cuerpo del defecto y renumerar sus posiciones (únicas por impl).
            let mut body = body.clone();
            freshen_positions(&mut body, &mut fresh_pos);
            // M40.2c: sustituir los parámetros del trait en el cuerpo (p. ej. `Option<T>` de `filter`
            // → `Option<int>` para `impl Iterator<int>`), como en la firma.
            if !trait_sigma.is_empty() { subst_named_block(&mut body, &trait_sigma); }
            // M40.2c: un método por defecto genérico (`fn map<U>` en el trait) → el manglado hereda
            // los type_params/bounds del impl más los del propio método.
            let mut type_params = imp.type_params.clone();
            type_params.extend(tm.type_params.iter().cloned());
            let mut bounds = imp.bounds.clone();
            bounds.extend(tm.bounds.iter().cloned());
            program.functions.push(Function {
                annotations: Vec::new(),
                is_pub: false,
                name: mangle(&key, &tm.name),
                type_params,
                bounds,
                params,
                return_type: subst_named(&subst_self(&tm.return_type, &imp.target), &trait_sigma),
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
    accumulate: bool,
    errors: Vec<TypeError>,
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
    /// M40.2: `for x in it` sobre un iterador → posición del `for` (línea, col) → nombre manglado de
    /// su método `next`. Un pase lo baja reescribiendo `ForIter::In` a `ForIter::Iter`.
    for_iter_sites: HashMap<(usize, usize), String>,
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
    /// Posición del nombre de campo/método en `recv.name` (M10.2g), copiada del `Program` en modo
    /// `gather`: `(línea, col, nombre)` del acceso → `(línea, col)` del `name`. Para registrar el
    /// hover del campo/método en su posición (el AST `Field` no la lleva). Vacío sin `gather`.
    field_name_pos: std::collections::HashMap<(usize, usize, String), Vec<(usize, usize)>>,
    /// El índice semántico recolectado (M10.2b). Vacío si `gather` es `false`.
    index: SemanticIndex,
    /// Posición de declaración de cada función de nivel superior (M10.2b: ir-a-definición).
    fn_defs: HashMap<String, (usize, usize)>,
    /// Posición de declaración de cada tipo (struct/enum/trait) — hover/def de tipos (M10.2f).
    type_defs: HashMap<String, (usize, usize)>,
    /// Posición de declaración de cada constante de nivel superior — hover/def de consts.
    const_defs: HashMap<String, (usize, usize)>,
    /// Alias UFCS de funciones `from`-importadas (nombre local → global), que deja el loader. Permiten
    /// que `recv.f(...)` resuelva una función importada como *fallback* (tras campo/método). Vacío sin
    /// imports.
    ufcs_aliases: HashMap<String, String>,
    /// Exigir la función de entrada `main` (DESIGN §11). `true` por defecto (`check`/`check_all`: un
    /// programa ejecutable la necesita). El LSP la pone en `false` al analizar un **archivo de módulo**
    /// (submódulo `pub`, sin `main`): un módulo suelto es legítimo sin entrada, y esa regla es de
    /// **proyecto**, no de archivo. Sin ella, el chequeo prosigue a los cuerpos y da diagnósticos reales.
    require_main: bool,
    /// Modo **completion de miembros** (M45): al tipar un acceso `recv.<centinela>`, en vez de dar
    /// error por miembro inexistente, enumera los miembros del tipo del receptor en `member_hits`.
    /// `false` en el chequeo normal (coste cero). Lo activa la entrada `member_completion`.
    completing: bool,
    /// Miembros enumerados en modo `completing` (M45): campos, métodos, builtins-como-método y
    /// funciones UFCS aplicables al tipo del receptor bajo el cursor.
    member_hits: Vec<MemberItem>,
}

/// El nombre-centinela que el LSP inserta tras el `.` (`recv.__raycomplete__`) para marcar el
/// punto de completion (M45). Empieza por `__` → ya se filtra como sintético en el resto del LSP,
/// y el usuario no puede escribirlo.
pub const COMPLETION_SENTINEL: &str = "__raycomplete__";

/// Un miembro ofrecible en `recv.` (M45): su etiqueta, el `CompletionItemKind` de LSP
/// (2=Method, 3=Function, 5=Field), un detalle opcional (p. ej. el tipo del campo), si toma
/// argumentos más allá del receptor (para el snippet `m(…)`), y la posición de declaración de la
/// función destino (método de impl / función UFCS), para resolver sus `///` docs.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberItem {
    pub label: String,
    pub kind: u8,
    pub detail: Option<String>,
    pub has_args: bool,
    pub def: Option<(usize, usize)>,
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
            accumulate: false,
            errors: Vec::new(),
            current_return: Type::Unit,
            type_params: HashSet::new(),
            ufcs_sites: HashMap::new(),
            for_iter_sites: HashMap::new(),
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
            const_defs: HashMap::new(),
            gather: false,
            field_name_pos: std::collections::HashMap::new(),
            index: SemanticIndex::default(),
            fn_defs: HashMap::new(),
            ufcs_aliases: HashMap::new(),
            require_main: true,
            completing: false,
            member_hits: Vec::new(),
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
                    format!("the value of constant '{}' must be a literal", c.name)));
            }
            let vt = self.check_expr(&c.value)?;
            if vt != declared {
                return Err(self.err(c.value.line, c.value.col, format!(
                    "constant '{}' is declared as {} but its value is {}", c.name, declared, vt)));
            }
            if self.consts.insert(c.name.clone(), declared).is_some() {
                return Err(self.err(c.line, c.col, format!("constant '{}' declared twice", c.name)));
            }
            if self.gather {
                self.const_defs.entry(c.name.clone()).or_insert((c.line, c.col));
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
                return Err(self.err(e.line, e.col, format!("enum '{}' declared twice", e.name)));
            }
        }
        for s in &program.structs {
            if self.enum_names.contains(&s.name) {
                return Err(self.err(s.line, s.col, format!("'{}' is already an enum; it cannot also be a struct", s.name)));
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
                    return Err(self.err(v.line, v.col, format!("variant '{}' repeated in enum '{}'", v.name, e.name)));
                }
                let payload: Vec<Type> = v.payload.iter().map(|t| self.resolve_type(t)).collect();
                variants.push((v.name.clone(), payload));
            }
            self.enums.insert(e.name.clone(), variants);
        }

        // --- Pre-pasada: registrar structs (campos con T en ámbito) ---
        for s in &program.structs {
            if self.structs.contains_key(&s.name) {
                return Err(self.err(s.line, s.col, format!("struct '{}' declared twice", s.name)));
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
            let variants = self.enums.get(&e.name).unwrap_or_else(|| crate::ice!("enum '{}' just registered is not in the table", e.name)).clone();
            for (_, payload) in &variants {
                for t in payload {
                    self.ensure_type(t, e.line, e.col)?;
                }
            }
        }
        for s in &program.structs {
            self.type_params = s.type_params.iter().cloned().collect();
            let fields = self.structs.get(&s.name).unwrap_or_else(|| crate::ice!("struct '{}' just registered is not in the table", s.name)).clone();
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
                return Err(self.err(f.line, f.col, format!("function '{}' declared twice", f.name)));
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

        // M41: registrar las funciones externas (FFI) como llamables. Cada una es una firma sin cuerpo;
        // se valida que sus tipos sean marshalables (primitivos en M41.1) y se registra en la tabla de
        // firmas para que `nombre(args)` typee como una llamada ordinaria. Los motores despachan a `ffi`.
        for e in &program.externs {
            if self.functions.contains_key(&e.name) {
                return Err(self.err(e.line, e.col, format!(
                    "'{}' is already declared (collision between a function and an 'extern fn')", e.name)));
            }
            for p in &e.params {
                self.ensure_type(&p.ty, p.line, p.col)?;
                let pt = self.resolve_type(&p.ty);
                if crate::ffi::ckind(&pt).is_none() || matches!(pt, Type::Unit) {
                    return Err(self.err(p.line, p.col, format!(
                        "the type {} of parameter '{}' of 'extern fn {}' is not FFI-marshalable (int, float, bool, string, bytes)",
                        pt, p.name, e.name)));
                }
            }
            let ret = self.resolve_type(&e.return_type);
            if crate::ffi::ret_ckind(&ret).is_none() {
                return Err(self.err(e.line, e.col, format!(
                    "the return type {} of 'extern fn {}' is not FFI-marshalable (int, float, bool, unit; a char* return is declared Option<bytes> or Option<string>)",
                    ret, e.name)));
            }
            let sig = FnSig {
                type_params: Vec::new(),
                params: e.params.iter().map(|p| self.resolve_type(&p.ty)).collect(),
                ret,
                bounds: Vec::new(),
            };
            self.functions.insert(e.name.clone(), sig);
        }

        // 'main' es obligatoria (DESIGN.md §11): sin parámetros y con retorno int o unit. El LSP
        // desactiva `require_main` al analizar un archivo de módulo (submódulo sin `main`).
        match self.functions.get("main") {
            None if self.require_main => {
                return Err(self.err(1, 1, "missing entry function 'main'".into()));
            }
            None => {}
            Some(sig) => {
                if !sig.params.is_empty() {
                    return Err(self.err(1, 1, "'main' must not take parameters".into()));
                }
                if sig.ret != Type::Int && sig.ret != Type::Unit {
                    return Err(self.err(1, 1, format!("'main' must return int or unit, not {}", sig.ret)));
                }
            }
        }

        // --- M10.1: validar las anotaciones (conjunto cerrado conocido) ---
        self.check_annotations(program)?;

        // --- Verificación de cada función ---
        for f in &program.functions {
            let depth = self.scopes.len();
            if let Err(e) = self.check_function(f) {
                if !self.accumulate {
                    return Err(e);
                }
                // M33c: un cuerpo fallido no contamina al siguiente — `check_function` ya
                // restaura type_params/current_self/bounds incluso en error; los ámbitos
                // que el cuerpo dejó a medias se truncan aquí.
                self.scopes.truncate(depth);
                self.errors.push(e);
                if self.errors.len() >= MAX_ERRORS {
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
                            return Err(self.err(a.line, a.col, "'@test' takes no arguments".into()));
                        }
                        if !f.params.is_empty() {
                            return Err(self.err(a.line, a.col, format!(
                                "the '@test' test function '{}' must not take parameters", f.name
                            )));
                        }
                        // M13.2b: una prueba puede devolver `bool` (pasa si es `true`) o `unit`
                        // (pasa si no dispara ningún `assert`/`panic`). El runner distingue ambos.
                        let ret = self.resolve_type(&f.return_type);
                        if ret != Type::Bool && ret != Type::Unit {
                            return Err(self.err(a.line, a.col, format!(
                                "an '@test' function must return bool or unit, not {}", f.return_type
                            )));
                        }
                    }
                    // `@derive` solo tiene sentido sobre tipos (genera su `impl`).
                    "derive" => return Err(self.err(a.line, a.col, "'@derive' is only allowed on a struct or enum".into())),
                    other => return Err(self.err(a.line, a.col, format!("unknown annotation: '@{}'", other))),
                }
            }
        }
        let type_items = program.structs.iter().map(|s| &s.annotations)
            .chain(program.enums.iter().map(|e| &e.annotations));
        for anns in type_items {
            for a in anns {
                match a.name.as_str() {
                    // `@derive` ya se validó y generó en `generate_eq_derives` (antes de
                    // `check_program`); aquí solo se acepta como conocida.
                    "derive" => {}
                    "test" => return Err(self.err(a.line, a.col, "'@test' is only allowed on functions".into())),
                    other => return Err(self.err(a.line, a.col, format!("unknown annotation: '@{}'", other))),
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
                return Err(self.err(f.line, f.col, format!("type parameter '{}' repeated in '{}'", tp, f.name)));
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
                    "the bound '{}: {}' does not constrain any type parameter of '{}'", tp, tr, f.name
                )));
            }
            if !self.traits.contains_key(tr) {
                return Err(self.err(f.line, f.col, format!(
                    "trait '{}' not declared (in the bound of '{}')", tr, f.name
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
                    "the bound '{}: {}' does not constrain any type parameter of the {} '{}'", tp, tr, kind, name
                )));
            }
            if !self.traits.contains_key(tr) {
                return Err(self.err(line, col, format!(
                    "trait '{}' not declared (in the bound of the {} '{}')", tr, kind, name
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
                return Err(self.err(t.line, t.col, format!("'{}' is already a type; it cannot also be a trait", t.name)));
            }
            if self.traits.contains_key(&t.name) {
                return Err(self.err(t.line, t.col, format!("trait '{}' declared twice", t.name)));
            }
            let mut seen = HashSet::new();
            for m in &t.methods {
                if !seen.insert(m.name.clone()) {
                    return Err(self.err(m.line, m.col, format!("method '{}' repeated in trait '{}'", m.name, t.name)));
                }
            }
            self.traits.insert(t.name.clone(), t.methods.clone());
            self.trait_tparams.insert(t.name.clone(), t.type_params.clone());
        }

        // 2) Impls: validar contra su trait y poblar las tablas de resolución.
        for imp in &program.impls {
            let trait_methods = match self.traits.get(&imp.trait_name) {
                Some(ms) => ms.clone(),
                None => return Err(self.err(imp.line, imp.col, format!("trait '{}' not declared", imp.trait_name))),
            };
            // M9.2b: los parámetros de tipo del impl entran en ámbito mientras se resuelve el
            // objetivo y se comparan las firmas, para que `Caja<T>` y un parámetro `x: T`
            // normalicen `T` a `Var` (en vez de `Struct("T")`). Se limpia al terminar el bucle.
            self.type_params = imp.type_params.iter().cloned().collect();
            // M40.2: los parámetros de tipo del TRAIT también entran en ámbito, para que
            // `resolve_type` normalice `T` (en `Option<T>` del método del trait) a `Var` y la σ del
            // trait (`T`→`int`) lo sustituya al comparar las firmas de un `impl Iterator<int>`.
            let ttp = self.trait_tparams.get(&imp.trait_name).cloned().unwrap_or_default();
            self.type_params.extend(ttp);
            self.check_impl_bounds(imp)?;
            let target = self.resolve_type(&imp.target);
            self.ensure_impl_target(&target, &imp.type_params, imp.line, imp.col)?;
            let key = type_key_of(&target).unwrap_or_else(|| crate::ice!("the validated impl target has no type key"));

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
                    return Err(self.err(m.line, m.col, format!("method '{}' implemented twice", m.name)));
                }
            }
            // Cobertura: no faltan métodos del trait... salvo los que tienen cuerpo por
            // defecto (M9.3a), que se heredan.
            for tm in &trait_methods {
                if !impl_names.contains(&tm.name) && tm.default_body.is_none() {
                    return Err(self.err(imp.line, imp.col, format!(
                        "the impl of '{}' for {} does not implement method '{}'", imp.trait_name, target, tm.name
                    )));
                }
            }
            // σ del trait (M40.2): sus parámetros de tipo → los argumentos del impl (`T`→`int` para
            // `impl Iterator<int>`). Vacío para un trait ordinario (M9) → sin efecto.
            let trait_tp = self.trait_tparams.get(&imp.trait_name).cloned().unwrap_or_default();
            let trait_sigma: HashMap<String, Type> = trait_tp
                .into_iter()
                .zip(imp.trait_args.iter().map(|t| self.resolve_type(t)))
                .collect();
            // ...ni sobran (cada método del impl pertenece al trait), y las firmas casan.
            for m in &imp.methods {
                let tm = match trait_methods.iter().find(|tm| tm.name == m.name) {
                    Some(tm) => tm,
                    None => return Err(self.err(m.line, m.col, format!(
                        "the trait '{}' does not declare a method '{}'", imp.trait_name, m.name
                    ))),
                };
                self.check_method_sig(tm, m, &target, &trait_sigma)?;
                let mangled = mangle(&key, &m.name);
                if self.methods.contains_key(&(key.clone(), m.name.clone())) {
                    return Err(self.err(m.line, m.col, format!(
                        "method '{}' ambiguous for {}: an impl already provides it", m.name, target
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
                            "method '{}' ambiguous for {}: an impl already provides it", tm.name, target
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
    /// semántica (la consume el operador `?`): valida la firma `fn convert(origen: S) -> Self` y
    /// guarda la conversión `(origen, destino) → función manglada`. Otros traits con parámetros
    /// de tipo se aceptan sintácticamente pero aún no hacen nada (diferido).
    fn register_typed_trait_impl(&mut self, imp: &ImplBlock, target: &Type, key: &str) -> Result<(), TypeError> {
        let tparams = self.trait_tparams.get(&imp.trait_name).cloned().unwrap_or_default();
        if imp.trait_args.len() != tparams.len() {
            return Err(self.err(imp.line, imp.col, format!(
                "the trait '{}' takes {} type parameter(s), but the impl passes {}",
                imp.trait_name, tparams.len(), imp.trait_args.len()
            )));
        }
        if imp.trait_name != "From" {
            return Ok(()); // otros traits con parámetros de tipo: sin semántica todavía
        }
        // `From<S> for E` exige `fn convert(origen: S) -> E` (sin `self`).
        let src = self.resolve_type(&imp.trait_args[0]);
        let src_key = match type_key_of(&src) {
            Some(k) => k,
            None => return Err(self.err(imp.line, imp.col,
                "the source type of 'From' does not support conversion".into())),
        };
        let m = match imp.methods.iter().find(|m| m.name == "convert") {
            Some(m) => m,
            None => return Err(self.err(imp.line, imp.col, format!(
                "the impl of 'From' for {} does not implement method 'convert'", target))),
        };
        if m.params.len() != 1 {
            return Err(self.err(m.line, m.col,
                "'convert' takes exactly one parameter (the source value), without 'self'".into()));
        }
        let got_param = self.resolve_type(&m.params[0].ty);
        if got_param != src {
            return Err(self.err(m.params[0].line, m.params[0].col, format!(
                "the parameter of 'convert' is {}, but 'From<{}>' requires {}", got_param, src, src)));
        }
        let got_ret = self.resolve_type(&subst_self(&m.return_type, target));
        if &got_ret != target {
            return Err(self.err(m.line, m.col, format!(
                "'convert' must return {} (the target type), not {}", target, got_ret)));
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
                    "the bound '{}: {}' mentions a type parameter the impl does not declare", tp, trait_name
                )));
            }
            if !self.traits.contains_key(trait_name) {
                return Err(self.err(imp.line, imp.col, format!(
                    "the bound '{}: {}' uses an undeclared trait", tp, trait_name
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
        // Primitivos + bytes: solo como objetivo concreto (sin parámetros de tipo). M48.4 añade `bytes`.
        if matches!(target, Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Bytes) {
            if type_params.is_empty() {
                return Ok(());
            }
            return Err(self.err(line, col, "a primitive type is not generic: it does not take type parameters in the impl".into()));
        }
        // M48.4: constructores incorporados `[T]` (aridad 1) y `Map<K,V>` (aridad 2). Siempre genéricos,
        // como `Caja<T>` (M9.2b): solo impls PLENAMENTE genéricos (`impl<T> ... for [T]`, no `[int]`),
        // con cada argumento un `Var` distinto de los propios parámetros del impl.
        let builtin_ctor: Option<(&str, Vec<&Type>)> = match target {
            Type::Array(e) => Some(("[]", vec![e.as_ref()])),
            Type::Map(k, v) => Some(("Map", vec![k.as_ref(), v.as_ref()])),
            _ => None,
        };
        if let Some((name, args)) = builtin_ctor {
            if type_params.len() != args.len() {
                return Err(self.err(line, col, format!(
                    "'{}' expects {} type parameter(s), the impl declares {}", name, args.len(), type_params.len())));
            }
            let mut seen = HashSet::new();
            let valid = args.iter().all(|a|
                matches!(a, Type::Var(n) if type_params.contains(n) && seen.insert(n.clone())));
            if !valid {
                return Err(self.err(line, col, format!(
                    "the impl of a built-in type must apply to its own distinct type parameters, e.g. 'impl<T> ... for [T]' or 'impl<K, V> ... for Map<K, V>'")));
            }
            return Ok(());
        }
        let (name, args) = match target {
            Type::Struct(n, a) | Type::Enum(n, a) => (n, a),
            _ => return Err(self.err(line, col, "a trait can only be implemented for a struct, enum or primitive".into())),
        };
        // Un struct desconocido sigue siendo `Struct` tras resolver (un enum conocido ya sería
        // `Enum`); rechazarlo aquí.
        if matches!(target, Type::Struct(_, _)) && !self.structs.contains_key(name) {
            return Err(self.err(line, col, format!("cannot implement for an unknown type: '{}'", name)));
        }
        let arity = self.struct_tparams.get(name).or_else(|| self.enum_tparams.get(name)).map_or(0, Vec::len);
        if type_params.is_empty() {
            // Impl concreto: el tipo objetivo no puede ser genérico.
            if arity != 0 {
                return Err(self.err(line, col, format!(
                    "'{name}' is generic: declare its parameters in the impl, e.g. 'impl<T> ... for {name}<T>' (M9.2b)"
                )));
            }
            return Ok(());
        }
        // Impl genérico (M9.2b): aridad y forma del objetivo.
        if arity != type_params.len() {
            return Err(self.err(line, col, format!(
                "'{}' expects {} type parameter(s), the impl declares {}", name, arity, type_params.len()
            )));
        }
        let mut seen = HashSet::new();
        let valid = args.len() == type_params.len()
            && args.iter().all(|a| matches!(a, Type::Var(n) if type_params.contains(n) && seen.insert(n.clone())));
        if !valid {
            return Err(self.err(line, col, format!(
                "the generic impl must apply to '{}<{}>' (its own distinct type parameters)",
                name, type_params.join(", ")
            )));
        }
        Ok(())
    }

    /// Comprueba que la firma de un método de impl coincide con la del trait, tras
    /// sustituir `Self` por el tipo implementador en ambas (M9.1).
    fn check_method_sig(&self, tm: &MethodSig, m: &Function, target: &Type, trait_sigma: &HashMap<String, Type>) -> Result<(), TypeError> {
        if tm.params.len() != m.params.len() {
            return Err(self.err(m.line, m.col, format!(
                "the method '{}' takes {} parameter(s) (including self), but the trait requires {}",
                m.name, m.params.len(), tm.params.len()
            )));
        }
        // Los tipos del trait se sustituyen: `Self`→target y los parámetros de tipo del trait por los
        // argumentos del impl (`T`→`int` para `impl Iterator<int>`, M40.2). Se resuelve ANTES de la σ
        // del trait: `resolve_type` normaliza `T` (`Struct`) a `Var`, que es lo que `subst` sustituye.
        let expected = |ty: &Type| subst(&self.resolve_type(&subst_self(ty, target)), trait_sigma);
        for (i, (tp, ip)) in tm.params.iter().zip(&m.params).enumerate() {
            let want = expected(&tp.ty);
            let got = self.resolve_type(&subst_self(&ip.ty, target));
            if want != got {
                return Err(self.err(ip.line, ip.col, format!(
                    "the parameter {} of method '{}' is {}, but the trait requires {}",
                    i + 1, m.name, got, want
                )));
            }
        }
        let want_ret = expected(&tm.return_type);
        let got_ret = self.resolve_type(&subst_self(&m.return_type, target));
        if want_ret != got_ret {
            return Err(self.err(m.line, m.col, format!(
                "the method '{}' returns {}, but the trait requires {}", m.name, got_ret, want_ret
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
                    "{} declares no return (unit), but its body produces {}",
                    label, body_ty
                )))
            } else {
                Ok(())
            }
        } else if body_ty == return_type || diverges {
            Ok(())
        } else {
            Err(self.err(eline, ecol, format!(
                "{} declares return type {}, but its body produces {}",
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
                                "'{}' is declared as {} but initialized with {}",
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
                                "the destructuring has {} names but the tuple has {} elements",
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
                        "cannot destructure a {} (expected a tuple)", other
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
                                "a for range must be int..int, not {}..{}", st, et)));
                        }
                        match pat {
                            ForPat::Single(n) => vec![(n.clone(), Type::Int)],
                            ForPat::Tuple(_) => return Err(self.err(stmt.line, stmt.col,
                                "a range binds a single variable (not a tuple)".into())),
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
                                        "iterating a Map binds exactly two variables (key, value)".into()));
                                }
                                let mut b = Vec::new();
                                if let Some(kn) = &names[0] { b.push((kn.clone(), (**k).clone())); }
                                if let Some(vn) = &names[1] { b.push((vn.clone(), (**v).clone())); }
                                b
                            }
                            (Type::Map(_, _), ForPat::Single(_)) => return Err(self.err(stmt.line, stmt.col,
                                "iterating a Map requires a `(key, value)` tuple".into())),
                            // M40.2: un tipo que implementa `Iterator<T>` → se itera llamando a `next`.
                            (other, ForPat::Single(n)) => match self.iterator_of(other) {
                                Some((elem, next_fn)) => {
                                    self.for_iter_sites.insert((stmt.line, stmt.col), next_fn);
                                    vec![(n.clone(), elem)]
                                }
                                None => return Err(self.err(stmt.line, stmt.col, format!(
                                    "cannot iterate over {} (expected an array, string, Map or an Iterator)", other))),
                            },
                            // M40.2e: `for (a, b) in it` sobre un iterador cuyo elemento es una tupla
                            // (p. ej. `enumerate()` → `(int, T)`). Cada nombre liga una posición.
                            (other, ForPat::Tuple(names)) => match self.iterator_of(other) {
                                Some((elem, next_fn)) => {
                                    let comps = match &elem {
                                        Type::Tuple(ts) if ts.len() == names.len() => ts.clone(),
                                        _ => return Err(self.err(stmt.line, stmt.col, format!(
                                            "the {}-variable pattern does not match the iterator element {}",
                                            names.len(), elem))),
                                    };
                                    self.for_iter_sites.insert((stmt.line, stmt.col), next_fn);
                                    names.iter().zip(comps)
                                        .filter_map(|(name, ty)| name.as_ref().map(|n| (n.clone(), ty)))
                                        .collect()
                                }
                                None => return Err(self.err(stmt.line, stmt.col, format!(
                                    "cannot iterate over {} (expected an array, string, Map or an Iterator)", other))),
                            },
                        }
                    }
                    // M40.2: `Iter` lo produce el lowering DESPUÉS del chequeo; aquí nunca aparece.
                    ForIter::Iter { .. } => crate::ice!("ForIter::Iter does not exist during checking"),
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
                        "returning {} but the function declares return type {}",
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
                    None => return Err(self.err(target.line, target.col, format!("variable '{}' not declared", name))),
                };
                if !mutable {
                    return Err(self.err(line, col, format!(
                        "cannot assign to '{}': it is immutable (declared with 'let'; use 'var')",
                        name
                    )));
                }
                // M28.3b: con tipo esperado, un literal entero adopta el ancho uint (`x = 5` con x: u8),
                // igual que en un `let`; para el resto, `check_expr_expected` cae al chequeo normal.
                let vt = self.check_expr_expected(value, &var_ty)?;
                if vt != var_ty {
                    return Err(self.err(value.line, value.col, format!("'{}' is {} but is assigned {}", name, var_ty, vt)));
                }
                Ok(())
            }
            // a[i] = e  — mutar el contenido NO requiere 'var' (DESIGN §12.3): la
            // inmutabilidad de `let` ata la variable, no congela el objeto.
            ExprKind::Index { array, index } => {
                // M11.4c-2: los strings son inmutables; `s[i] = c` no se permite (sí se lee `s[i]`).
                if self.check_expr(array)? == Type::String {
                    return Err(self.err(target.line, target.col,
                        "cannot assign to a character of a string (strings are immutable)".into()));
                }
                let elem = self.check_index(array, index)?;
                let vt = self.check_expr_expected(value, &elem)?;
                if vt != elem {
                    return Err(self.err(value.line, value.col, format!("the element is {} but is assigned {}", elem, vt)));
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
                            "a tuple position is not assignable ('.{name}'); destructure the tuple or use an array"
                        )));
                    }
                }
                let fty = self.check_field(object, name)?;
                let vt = self.check_expr_expected(value, &fty)?;
                if vt != fty {
                    return Err(self.err(value.line, value.col, format!("the field '{}' is {} but is assigned {}", name, fty, vt)));
                }
                Ok(())
            }
            _ => Err(self.err(target.line, target.col, "the left-hand side is not assignable".into())),
        }
    }

    /// Verifica `a[i]` y devuelve el tipo de elemento. Reusado por la indexación
    /// como expresión y como destino de asignación.
    fn check_index(&mut self, array: &Expr, index: &Expr) -> Result<Type, TypeError> {
        let at = self.check_expr(array)?;
        let it = self.check_expr(index)?;
        if it != Type::Int {
            return Err(self.err(index.line, index.col, format!("the index must be int, not {}", it)));
        }
        match at {
            Type::Array(elem) => Ok(*elem),
            // M11.4c-2: indexar un string da el carácter en esa posición.
            Type::String => Ok(Type::Char),
            // M16.1a: indexar bytes da el octeto (0–255) como int.
            Type::Bytes => Ok(Type::Int),
            other => {
                let mut msg = format!("cannot index a {} (not an array, string or bytes)", other);
                if Self::block_like(array) {
                    msg.push_str(
                        "; note: a tail starting with '[' after an if/while/match/block parses as an indexing of its value — separate it with 'return' or 'let'",
                    );
                }
                Err(self.err(array.line, array.col, msg))
            }
        }
    }

    /// Comprueba que una lista de parámetros de tipo no tenga repetidos.
    fn check_unique_tparams(&self, params: &[String], owner: &str, line: usize, col: usize) -> Result<(), TypeError> {
        let mut seen = HashSet::new();
        for tp in params {
            if !seen.insert(tp) {
                return Err(self.err(line, col, format!("type parameter '{}' repeated in '{}'", tp, owner)));
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
                        "a Map key must be int/string/char/bool/bytes, not {}", k
                    )));
                }
                self.ensure_type(k, line, col)?;
                self.ensure_type(v, line, col)
            }
            Type::Struct(name, args) if name == "Map" => {
                if args.len() != 2 {
                    return Err(self.err(line, col, format!(
                        "Map expects 2 type arguments (key and value), not {}", args.len()
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
                        "Channel expects 1 type argument, not {}", args.len()
                    )));
                }
                self.ensure_type(&Type::Channel(Box::new(self.resolve_type(&args[0]))), line, col)
            }
            // `Task<T>` (M12.3): como `Channel`, el elemento puede ser cualquier tipo.
            Type::Task(t) => self.ensure_type(t, line, col),
            Type::Struct(name, args) if name == "Task" => {
                if args.len() != 1 {
                    return Err(self.err(line, col, format!(
                        "Task expects 1 type argument, not {}", args.len()
                    )));
                }
                self.ensure_type(&Type::Task(Box::new(self.resolve_type(&args[0]))), line, col)
            }
            // `Self` (M9) llega como `Struct("Self")` sin resolver; fuera de un impl no
            // tiene un tipo implementador al que referirse.
            Type::Struct(name, _) if name == "Self" => {
                Err(self.err(line, col, "'Self' is only valid inside a trait or impl".into()))
            }
            // Un identificador en posición de tipo llega como `Struct(name, args)`
            // desde el parser; aquí puede ser un struct, un enum o un parámetro de
            // tipo en ámbito (M6).
            Type::Struct(name, args) => {
                if self.type_params.contains(name) {
                    if !args.is_empty() {
                        return Err(self.err(line, col, format!("the type parameter '{}' takes no arguments", name)));
                    }
                    return Ok(());
                }
                let arity = self.struct_tparams.get(name)
                    .or_else(|| self.enum_tparams.get(name));
                match arity {
                    Some(tparams) => self.ensure_type_args(name, tparams.len(), args, line, col),
                    None => Err(self.err(line, col, format!("unknown type: '{}' not declared", name))),
                }
            }
            Type::Enum(name, args) => match self.enum_tparams.get(name) {
                Some(tparams) => self.ensure_type_args(name, tparams.len(), args, line, col),
                None => Err(self.err(line, col, format!("unknown type: enum '{}' not declared", name))),
            },
            // Un parámetro de tipo (M6) es válido si está en ámbito.
            Type::Var(name) if !self.type_params.contains(name) => {
                Err(self.err(line, col, format!("type parameter '{}' out of scope", name)))
            }
            // `Self` solo tiene sentido dentro de un trait/impl; aquí ya se habría
            // reclasificado al tipo implementador. Si llega `SelfType`, es un uso fuera
            // de lugar (M9).
            Type::SelfType => Err(self.err(line, col, "'Self' is only valid inside a trait or impl".into())),
            // `dyn Trait` (M9.3b): el trait debe existir.
            Type::Dyn(traits) => {
                // Cada trait del conjunto debe existir, y ningún nombre de método puede repetirse
                // entre los traits (no se sabría a cuál despachar `obj.m()`).
                let mut methods: HashSet<String> = HashSet::new();
                for tr in traits {
                    let Some(ms) = self.traits.get(tr) else {
                        return Err(self.err(line, col, format!("trait '{}' not declared (in 'dyn {}')", tr, traits.join(" + "))));
                    };
                    for m in ms {
                        if !methods.insert(m.name.clone()) {
                            return Err(self.err(line, col, format!(
                                "the method '{}' appears in more than one trait of 'dyn {}': it is ambiguous", m.name, traits.join(" + ")
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
                "'{}' expects {} type argument(s), got {}", name, arity, args.len()
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
            // Tuplas (M27.1): resolver cada componente (antes se dejaban sin resolver → un `Struct("T")`
            // dentro de una tupla no se reclasificaba a `Var`/`Enum`, rompiendo la inferencia genérica).
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| self.resolve_type(t)).collect()),
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
            None => return Err(self.err(line, col, format!("struct '{}' not declared", name))),
        };
        // M10.2f: hover/def del nombre de tipo en el literal `Nombre { … }`.
        if self.gather {
            let def = self.type_defs.get(name).copied();
            self.record_named(line, col, name.chars().count(), format!("struct {}", name), def);
        }
        let orig_tparams = self.struct_tparams.get(name).cloned().unwrap_or_default();
        // No debe haber campos desconocidos.
        for (fname, fexpr) in fields {
            if !declared.iter().any(|(dname, _)| dname == fname) {
                return Err(self.err(fexpr.line, fexpr.col, format!("'{}' has no field '{}'", name, fname)));
            }
        }
        // M40.2e: renombrar los params del struct a nombres frescos para la inferencia (higiene: que un
        // `T` del struct no colisione con un `T` del ámbito). Los bounds usan los nombres originales.
        let field_types: Vec<Type> = declared.iter().map(|(_, t)| t.clone()).collect();
        let (tparams, fresh_types) = freshen_ctor_params(&orig_tparams, &field_types, &self.type_params);
        let declared: Vec<(String, Type)> = declared.iter().map(|(n, _)| n.clone()).zip(fresh_types).collect();
        // σ: parámetro de tipo → tipo inferido. Se siembra del tipo esperado (resuelto, ver check_enum_lit).
        let expected_owned = expected.map(|e| self.resolve_type(e));
        let mut sigma = seed_sigma_from_expected(expected_owned.as_ref(), name, &tparams);
        // Cada campo declarado debe estar presente exactamente una vez; su valor
        // determina (unifica) los parámetros de tipo del struct.
        for (dname, dty) in &declared {
            let matches: Vec<&(String, Expr)> = fields.iter().filter(|(fname, _)| fname == dname).collect();
            match matches.as_slice() {
                [] => return Err(self.err(line, col, format!("missing field '{}' in the literal of '{}'", dname, name))),
                [(_, value)] => {
                    let vt = self.check_value_against(value, dty, &sigma)?;
                    unify(dty, &vt, &mut sigma).map_err(|reason| self.err(value.line, value.col, format!(
                        "field '{}' of '{}': {}", dname, name, reason
                    )))?;
                }
                _ => return Err(self.err(line, col, format!("field '{}' of '{}' repeated", dname, name))),
            }
        }
        let targs = self.finalize_type_args(&tparams, &sigma, &format!("the struct '{}'", name), line, col)?;
        // M9.4: cada parámetro acotado debe resolver a un tipo que satisfaga su bound.
        let bounds = self.struct_bounds.get(name).cloned().unwrap_or_default();
        self.check_construction_bounds(name, &orig_tparams, &targs, &bounds, line, col)?;
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
                    "'{}' requires that '{}' be '{}', but {} does not implement it", name, tp, trait_name, concrete
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
                None => return Err(self.err(line, col, format!("the enum '{}' has no variant '{}'", enum_name, variant))),
            },
            None => return Err(self.err(line, col, format!("enum '{}' not declared", enum_name))),
        };
        // M10.2f: hover/def del nombre de enum en `Enum.Variante(...)`.
        if self.gather {
            let def = self.type_defs.get(enum_name).copied();
            self.record_named(line, col, enum_name.chars().count(), format!("enum {}", enum_name), def);
            // Y el hover de la **variante** (el identificador tras el `.`): su firma con el payload.
            // La posición asume `Enum.Variante` sin espacios (la grafía canónica); el `+1` es el punto.
            let vcol = col + enum_name.chars().count() + 1;
            let signature = if payload.is_empty() {
                format!("{}.{}", enum_name, variant)
            } else {
                let type_strs: Vec<String> = payload.iter().map(|t| format!("{}", t)).collect();
                format!("{}.{}({})", enum_name, variant, type_strs.join(", "))
            };
            self.record_named(line, vcol, variant.chars().count(), signature, def);
        }
        let orig_tparams = self.enum_tparams.get(enum_name).cloned().unwrap_or_default();
        if args.len() != payload.len() {
            return Err(self.err(line, col, format!(
                "the variant '{}.{}' expects {} argument(s), got {}",
                enum_name, variant, payload.len(), args.len()
            )));
        }
        // M40.2e: renombrar los params del enum a nombres frescos para la inferencia (higiene: que el
        // `T` de Option no colisione con un `T` del ámbito). Los bounds usan los nombres originales.
        let (tparams, payload) = freshen_ctor_params(&orig_tparams, &payload, &self.type_params);
        // El esperado se **resuelve** (Struct("T")→Var("T") según el ámbito) para que sus parámetros de
        // tipo casen con los del cuerpo (que ya vienen resueltos); si no, `T` de una anotación y `T`
        // inferido diferirían como Struct vs Var.
        let expected_owned = expected.map(|e| self.resolve_type(e));
        let mut sigma = seed_sigma_from_expected(expected_owned.as_ref(), enum_name, &tparams);
        for (arg, pty) in args.iter().zip(&payload) {
            let at = self.check_value_against(arg, pty, &sigma)?;
            unify(pty, &at, &mut sigma).map_err(|reason| self.err(arg.line, arg.col, format!(
                "'{}.{}': {}", enum_name, variant, reason
            )))?;
        }
        let targs = self.finalize_type_args(&tparams, &sigma, &format!("the variant '{}.{}'", enum_name, variant), line, col)?;
        // M9.4: cada parámetro acotado debe resolver a un tipo que satisfaga su bound.
        let bounds = self.enum_bounds.get(enum_name).cloned().unwrap_or_default();
        self.check_construction_bounds(enum_name, &orig_tparams, &targs, &bounds, line, col)?;
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
                    "could not infer the type parameter '{}' of {}; annotate the type", tp, label
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
        let enum_name = match &scrut_ty {
            Type::Enum(n, _) => n.clone(),
            other => return Err(self.err(scrutinee.line, scrutinee.col, format!(
                "match requires an enum, but the scrutinee is {}", other
            ))),
        };
        if arms.is_empty() {
            return Err(self.err(line, col, "a match cannot be empty".into()));
        }
        // Variantes del enum (para la exhaustividad). La σ de tipos ya no hace falta aquí:
        // `check_subpattern` la resuelve del tipo de cada sub-valor (M40.1c).
        let variants = self.enums.get(&enum_name).unwrap_or_else(|| crate::ice!("the enum '{}' is not in the checker table", enum_name)).clone();

        let mut covered: HashSet<String> = HashSet::new();
        let mut catchall = false;
        let mut result_ty: Option<Type> = None;

        for arm in arms {
            // Un brazo tras un catch-all nunca se alcanza.
            if catchall {
                return Err(self.err(arm.line, arm.col,
                    "unreachable arm: a previous arm already covers all cases".into()));
            }
            // Comprueba el patrón (recursivo, M40.1c) y obtiene las variables a ligar. La cobertura
            // (exhaustividad) se registra aparte y SOLO para brazos sin guarda: un brazo con **guarda**
            // (M40.1a) puede no casar aunque el patrón ligue → no cuenta para exhaustividad/alcance.
            let binds = self.check_subpattern(&arm.pattern, &scrut_ty)?;
            if arm.guard.is_none() {
                self.register_coverage(&arm.pattern, &mut covered, &mut catchall)?;
            }
            // Verifica la guarda (bool) y el cuerpo con esas variables en un ámbito propio,
            // propagando el tipo esperado del match a cada brazo (para construcciones como `None`).
            self.push_scope();
            for (name, ty) in binds {
                self.declare(&name, ty, false, (arm.line, arm.col));
            }
            let guard_ty = arm.guard.as_ref().map(|g| self.check_expr(g));
            let body_ty = match expected {
                Some(exp) => self.check_expr_expected(&arm.body, exp),
                None => self.check_expr(&arm.body),
            };
            self.pop_scope();
            if let (Some(g), Some(gt)) = (arm.guard.as_ref(), guard_ty)
                && gt? != Type::Bool
            {
                return Err(self.err(g.line, g.col,
                    "a match arm guard must be of type bool".into()));
            }
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
                        "the match arms produce different types: {} and {}", prev, body_ty
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
                    "non-exhaustive match on '{}': missing variants: {}",
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
                                "'?' on {} requires the function to return Result<_, {}>, but it returns {}",
                                it, err_ty, self.current_return
                            ))),
                        }
                    }
                    other => Err(self.err(line, col, format!(
                        "'?' on {} requires the function to return Result<_, {}>, but it returns {}",
                        it, err_ty, other
                    ))),
                }
            }
            Type::Enum(name, args) if name == "Option" && args.len() == 1 => {
                let some_ty = args[0].clone();
                match &self.current_return {
                    Type::Enum(rn, rargs) if rn == "Option" && rargs.len() == 1 => Ok(some_ty),
                    other => Err(self.err(line, col, format!(
                        "'?' on {} requires the function to return Option<_>, but it returns {}",
                        it, other
                    ))),
                }
            }
            other => Err(self.err(inner.line, inner.col, format!(
                "'?' requires a Result or an Option, not {}", other
            ))),
        }
    }

    /// Comprueba un patrón contra el enum del escrutinio. Devuelve las variables que
    /// liga (nombre, tipo) para declararlas en el cuerpo del brazo. Actualiza el
    /// conjunto de variantes cubiertas y marca si el patrón es catch-all.
    /// Si `ty` implementa `Iterator<T>` (M40.2), devuelve `(T, next_manglado)`: el tipo de elemento
    /// (el argumento de `Option` en el retorno de `next`) y el nombre manglado de su método `next`,
    /// para que `for x in it` lo consuma. `None` si el tipo no es un iterador.
    fn iterator_of(&self, ty: &Type) -> Option<(Type, String)> {
        let key = type_key_of(ty)?;
        if !self.impl_traits.contains(&(key.clone(), "Iterator".to_string())) {
            return None;
        }
        let next_fn = self.methods.get(&(key, "next".to_string()))?.clone();
        let sig = self.functions.get(&next_fn)?;
        // Impl genérico (`impl<T> Iterator<T> for ArrayIter<T>`): el `next` manglado es una función
        // genérica; su retorno es `Option<T>` con `T` sin fijar. Unificamos el tipo del receptor
        // (`self`, params[0]) con el tipo REAL del iterable para obtener σ y sustituir el retorno →
        // el elemento sale concreto (`int` para `[int].iter()`). Para un impl concreto (Contador),
        // σ queda vacío y `Option<int>` pasa tal cual.
        let mut sigma = HashMap::new();
        if let Some(self_ty) = sig.params.first() {
            let _ = unify(self_ty, ty, &mut sigma);
        }
        // El elemento es el argumento de `Option` en el retorno de `next` (`Option<T>` → `T`).
        match subst(&sig.ret, &sigma) {
            Type::Enum(n, args) if n == "Option" && args.len() == 1 => {
                let elem = args.into_iter().next().unwrap_or_else(|| crate::ice!("Option with 1 arg but no element"));
                Some((elem, next_fn))
            }
            _ => None,
        }
    }

    /// Verifica que `pat` casa con un valor de tipo `ty` y recolecta sus bindings (nombre → tipo).
    /// Recursivo (M40.1c): un sub-patrón de variante puede ser otra variante anidada; su enum se
    /// resuelve del **tipo** del sub-valor (el payload sustituido), no de un parámetro externo.
    fn check_subpattern(&mut self, pat: &Pattern, ty: &Type) -> Result<Vec<(String, Type)>, TypeError> {
        match &pat.kind {
            PatternKind::Wildcard => Ok(Vec::new()),
            PatternKind::Binding(name) => {
                // M10.2f: hover del binding del patrón (su declaración) → `nombre: Tipo`.
                if self.gather {
                    self.record_ident(pat.line, pat.col, name, ty, Some((pat.line, pat.col)));
                }
                Ok(vec![(name.clone(), ty.clone())])
            }
            PatternKind::Variant { enum_name: pat_enum, variant, subpatterns } => {
                let (ty_enum, targs) = match ty {
                    Type::Enum(n, args) => (n.clone(), args.clone()),
                    other => return Err(self.err(pat.line, pat.col, format!(
                        "the pattern is of enum '{}', but the value here is {}", pat_enum, other
                    ))),
                };
                if *pat_enum != ty_enum {
                    // Redacción idéntica a la original (el checker auto-alojado la espeja byte a byte).
                    return Err(self.err(pat.line, pat.col, format!(
                        "the pattern is of enum '{}', but the scrutinee is '{}'", pat_enum, ty_enum
                    )));
                }
                let variants = self.enums.get(&ty_enum)
                    .unwrap_or_else(|| crate::ice!("the enum '{}' is not in the checker table", ty_enum)).clone();
                let payload = match variants.iter().find(|(v, _)| v == variant) {
                    Some((_, p)) => p.clone(),
                    None => return Err(self.err(pat.line, pat.col, format!(
                        "the enum '{}' has no variant '{}'", ty_enum, variant
                    ))),
                };
                if subpatterns.len() != payload.len() {
                    return Err(self.err(pat.line, pat.col, format!(
                        "the pattern '{}.{}' binds {} value(s), but the variant has {}",
                        ty_enum, variant, subpatterns.len(), payload.len()
                    )));
                }
                // M10.2f: hover del enum y la variante en el patrón (como en la construcción). La
                // variante va tras `enum.` (grafía canónica sin espacios); el `+1` es el punto.
                if self.gather {
                    let def = self.type_defs.get(&ty_enum).copied();
                    self.record_named(pat.line, pat.col, ty_enum.chars().count(), format!("enum {}", ty_enum), def);
                    let vcol = pat.col + ty_enum.chars().count() + 1;
                    let signature = if payload.is_empty() {
                        format!("{}.{}", ty_enum, variant)
                    } else {
                        let type_strs: Vec<String> = payload.iter().map(|t| format!("{}", t)).collect();
                        format!("{}.{}({})", ty_enum, variant, type_strs.join(", "))
                    };
                    self.record_named(pat.line, vcol, variant.chars().count(), signature, def);
                }
                // σ del enum del sub-valor: liga sus parámetros de tipo con los argumentos del tipo.
                let tparams = self.enum_tparams.get(&ty_enum).cloned().unwrap_or_default();
                let sigma: HashMap<String, Type> = tparams.into_iter().zip(targs).collect();
                let mut binds = Vec::new();
                for (sub, pty) in subpatterns.iter().zip(&payload) {
                    binds.extend(self.check_subpattern(sub, &subst(pty, &sigma))?); // recursivo
                }
                Ok(binds)
            }
            PatternKind::Struct { name, fields } => {
                // El valor debe ser un struct con este nombre (M40.1d).
                let (sname, targs) = match ty {
                    Type::Struct(n, args) => (n.clone(), args.clone()),
                    other => return Err(self.err(pat.line, pat.col, format!(
                        "the struct pattern '{}' does not match: the value here is {}", name, other
                    ))),
                };
                if *name != sname {
                    return Err(self.err(pat.line, pat.col, format!(
                        "the pattern is of struct '{}', but the value here is of struct '{}'", name, sname
                    )));
                }
                let struct_fields = self.structs.get(&sname)
                    .unwrap_or_else(|| crate::ice!("the struct '{}' is not in the checker table", sname)).clone();
                // σ del struct: liga sus parámetros de tipo con los argumentos (`Par<int,bool>`).
                let tparams = self.struct_tparams.get(&sname).cloned().unwrap_or_default();
                let sigma: HashMap<String, Type> = tparams.into_iter().zip(targs).collect();
                let mut binds = Vec::new();
                for (fname, fpat) in fields {
                    let fty = match struct_fields.iter().find(|(f, _)| f == fname) {
                        Some((_, t)) => subst(t, &sigma),
                        None => return Err(self.err(fpat.line, fpat.col, format!(
                            "the struct '{}' has no field '{}'", sname, fname
                        ))),
                    };
                    binds.extend(self.check_subpattern(fpat, &fty)?); // recursivo
                }
                Ok(binds)
            }
        }
    }

    /// Registra la cobertura del patrón de **primer nivel** para la exhaustividad (conservadora,
    /// M40.1c): una variante cuenta como cubierta solo si TODOS sus sub-patrones son catch-all
    /// (`_`/binding); si alguno es una variante anidada, NO se marca (hace falta un fallback). Un
    /// `_`/binding de primer nivel es catch-all total. Repetir una variante ya cubierta = inalcanzable.
    fn register_coverage(
        &self,
        pat: &Pattern,
        covered: &mut HashSet<String>,
        catchall: &mut bool,
    ) -> Result<(), TypeError> {
        match &pat.kind {
            PatternKind::Wildcard | PatternKind::Binding(_) => *catchall = true,
            PatternKind::Variant { variant, subpatterns, .. } => {
                // Cubre la variante si todos sus sub-patrones son **irrefutables** (siempre casan):
                // `_`/binding o un struct de campos irrefutables (`Punto { x, y }`). Una variante
                // anidada es refutable → no cubre (conservador; hace falta un fallback).
                let cubre_todo = subpatterns.iter().all(is_irrefutable);
                if cubre_todo {
                    if !covered.insert(variant.clone()) {
                        return Err(self.err(pat.line, pat.col, format!(
                            "the variant '{}' is already covered by a previous arm", variant
                        )));
                    }
                }
                // Si no cubre todo (un sub-patrón anidado), no se marca: sigue haciendo falta un fallback.
            }
            // Un patrón de struct nunca es de primer nivel (el escrutinio de un match es un enum); no
            // marca cobertura. Este brazo existe solo para la exhaustividad del `match` de Rust.
            PatternKind::Struct { .. } => {}
        }
        Ok(())
    }

    /// Verifica `obj.name` y devuelve el tipo del campo. Para un struct genérico, el
    /// tipo del campo se **sustituye** con los argumentos de tipo del objeto: el campo
    /// `primero: A` de `Par<int, bool>` es un `int`.
    fn check_field(&mut self, object: &Expr, name: &str) -> Result<Type, TypeError> {
        let ot = self.check_expr(object)?;
        // M45: completion de miembros. El LSP repara `recv.` como `recv.<centinela>`; aquí, con el
        // tipo del receptor ya calculado, enumeramos sus miembros en vez de dar error por miembro
        // inexistente. Devolvemos Unit para que el chequeo (best-effort) no aborte antes de recogerlo.
        if self.completing && name == COMPLETION_SENTINEL {
            self.member_hits = self.enumerate_members(&ot);
            return Ok(Type::Unit);
        }
        // Acceso a **tupla** `t.0` (M27.1): un nombre de campo numérico solo es válido sobre una tupla.
        if let Type::Tuple(elems) = &ot {
            let idx: usize = name.parse().map_err(|_| self.err(object.line, object.col,
                format!("cannot access '.{}' on a tuple (use an index like .0)", name)))?;
            if idx >= elems.len() {
                return Err(self.err(object.line, object.col, format!(
                    "the tuple has {} elements; index .{} is out of range", elems.len(), idx)));
            }
            return Ok(elems[idx].clone());
        }
        match ot {
            Type::Struct(sname, targs) => {
                let fields = self.structs.get(&sname).unwrap_or_else(|| crate::ice!("the struct '{}' is not in the checker table", sname));
                let fty = match fields.iter().find(|(fname, _)| fname == name) {
                    Some((_, fty)) => fty.clone(),
                    None => return Err(self.err(object.line, object.col, format!("the struct '{}' has no field '{}'", sname, name))),
                };
                let tparams = self.struct_tparams.get(&sname).cloned().unwrap_or_default();
                let sigma: HashMap<String, Type> = tparams.into_iter().zip(targs).collect();
                let result = subst(&fty, &sigma);
                // M10.2g: hover del **campo** en su posición (nombre tras el `.`); un campo de datos
                // no tiene declaración de función → sin `def`.
                self.record_field_hover(object.line, object.col, name, &result, None);
                Ok(result)
            }
            other => Err(self.err(object.line, object.col, format!("cannot access '.{}' on a {} (not a struct)", name, other))),
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
            let propagates = matches!(expr.kind, ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::Block(_));
            if !propagates {
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
        // M48.1: función asociada `Tipo.fn(args)` (`Map.new()`, `Channel.new()`, `Channel.bounded(n)`)
        // con tipo esperado → su resultado (Map/Channel) se fija desde el esperado (indeterminado, como
        // `[]`/`None`). Antes eran `map_new()`/`channel()` (builtins de función libre).
        if let ExprKind::Call { callee, args } = &expr.kind {
            if let Some(r) = self.try_assoc_call(callee, args, Some(expected), expr.line, expr.col) {
                return r;
            }
        }
        match &expr.kind {
            // M40.3b: llamada a una función **genérica** de usuario en contexto tipado. Se pasa el
            // esperado para rellenar los parámetros de tipo que los argumentos no determinen (p. ej.
            // un constructor vacío `set_new() -> Set<T>`). No afecta a builtins/variables-función ni a
            // funciones no genéricas (caen al chequeo normal).
            ExprKind::Call { callee, args }
                if matches!(&callee.kind, ExprKind::Ident(n)
                    if crate::builtins::lookup(n).is_none()
                       && self.lookup(n).is_none()
                       && self.functions.get(n).is_some_and(|s| !s.type_params.is_empty())) =>
            {
                let name = match &callee.kind { ExprKind::Ident(n) => n.clone(), _ => crate::ice!("callee guaranteed Ident by the match guard") };
                if let Some(sig) = self.functions.get(&name).filter(|_| self.gather) {
                    let ty = Type::Fn(sig.params.clone(), Box::new(sig.ret.clone()));
                    let def = self.fn_defs.get(&name).copied();
                    self.record_ident(callee.line, callee.col, &name, &ty, def);
                }
                self.check_named_call(&name, args, expr.line, expr.col, Some(expected))
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
                                "the array elements must be {}, not {}", elem_exp, t
                            )));
                        }
                    }
                    Ok(Type::Array(elem_exp.clone()))
                }
                _ => self.check_expr(expr),
            },
            // M48.2: literal de Map `[k: v, …]` con tipo esperado `Map<K,V>` → cada clave contra K, cada
            // valor contra V. `[:]` vacío se fija aquí (indeterminado, como `[]`). Sin esperado-Map, cae
            // a `check_expr` (que infiere de los pares o exige anotar el vacío).
            ExprKind::MapLit(pares) => match expected {
                Type::Map(kexp, vexp) => {
                    self.ensure_type(expected, expr.line, expr.col)?; // clave hashable
                    for (k, v) in pares {
                        let kt = self.check_expr_expected(k, kexp)?;
                        if kt != **kexp {
                            return Err(self.err(k.line, k.col, format!(
                                "the Map keys must be {}, not {}", kexp, kt)));
                        }
                        let vt = self.check_expr_expected(v, vexp)?;
                        if vt != **vexp {
                            return Err(self.err(v.line, v.col, format!(
                                "the Map values must be {}, not {}", vexp, vt)));
                        }
                    }
                    Ok(expected.clone())
                }
                _ => self.check_expr(expr),
            },
            ExprKind::If { cond, then_branch, else_branch } => {
                let ct = self.check_expr(cond)?;
                if ct != Type::Bool {
                    return Err(self.err(cond.line, cond.col, format!("the if condition must be bool, not {}", ct)));
                }
                let then_ty = self.check_block_expected(then_branch, expected)?;
                match else_branch {
                    None => {
                        if then_ty != Type::Unit {
                            return Err(self.err(expr.line, expr.col, format!(
                                "an if without else has type unit, but its branch produces {} (add an else)", then_ty
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
                                "the if branches have different types: {} and {}", then_ty, else_ty
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
                "cannot convert 'dyn {}' into 'dyn {}': can only upcast to a subset of traits",
                source.join(" + "), traits.join(" + ")
            )));
        }
        let key = type_key_of(&actual).ok_or_else(|| self.err(line, col, format!(
            "cannot convert {} into 'dyn {}'", actual, traits.join(" + ")
        )))?;
        // El tipo concreto debe implementar **todos** los traits del conjunto.
        for tr in traits {
            if !self.impl_traits.contains(&(key.clone(), tr.clone())) {
                return Err(self.err(line, col, format!(
                    "{} does not implement '{}': it cannot be used as 'dyn {}'", actual, tr, traits.join(" + ")
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
                    let ty = ty.clone();
                    let def = self.const_defs.get(name).copied();
                    self.record_ident(expr.line, expr.col, name, &ty, def); // hover/def de la const
                    return Ok(ty);
                }
                if let Some(sig) = self.functions.get(name) {
                    if !sig.type_params.is_empty() {
                        return Err(self.err(expr.line, expr.col, format!(
                            "cannot use the generic function '{}' as a value; call it directly", name
                        )));
                    }
                    let ty = Type::Fn(sig.params.clone(), Box::new(sig.ret.clone()));
                    let def = self.fn_defs.get(name).copied();
                    self.record_ident(expr.line, expr.col, name, &ty, def); // M10.2b
                    return Ok(ty);
                }
                Err(self.err(expr.line, expr.col, format!("name '{}' not declared", name)))
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
                        _ => Err(self.err(expr.line, expr.col, format!("cannot negate (-) a {}", t))),
                    },
                    UnaryOp::Not if t == Type::Bool => Ok(Type::Bool),
                    UnaryOp::Not => Err(self.err(expr.line, expr.col, format!("'!' requires bool, not {}", t))),
                    // M19.3a: NOT bit a bit, int → int. M28.3: también sobre uint (mismo ancho).
                    UnaryOp::BitNot if t == Type::Int => Ok(Type::Int),
                    UnaryOp::BitNot if matches!(t, Type::UInt(_)) => Ok(t),
                    UnaryOp::BitNot => Err(self.err(expr.line, expr.col, format!("'~' requires int, not {}", t))),
                }
            }

            ExprKind::Binary { op, left, right } => self.check_binary(*op, left, right, expr.line, expr.col),

            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.line, expr.col),

            ExprKind::ArrayLit(elems) => {
                if elems.is_empty() {
                    return Err(self.err(expr.line, expr.col,
                        "cannot infer the type of [] here; annotate it (e.g. let xs: [int] = [];)".into()));
                }
                let first = self.check_expr(&elems[0])?;
                for e in &elems[1..] {
                    let t = self.check_expr(e)?;
                    if t != first {
                        return Err(self.err(e.line, e.col, format!(
                            "the array elements must be of the same type: {} and {}", first, t
                        )));
                    }
                }
                Ok(Type::Array(Box::new(first)))
            }

            // M48.2: literal de Map sin tipo esperado. `[:]` vacío es indeterminado (como `[]`) → error de
            // "anota el tipo". `[k: v, …]` infiere `Map<K,V>` del primer par y exige que el resto coincida
            // (claves homogéneas, valores homogéneos); la clave debe ser hashable.
            ExprKind::MapLit(pares) => {
                if pares.is_empty() {
                    return Err(self.err(expr.line, expr.col,
                        "cannot infer the type of [:] here; annotate it (e.g. let m: Map<string, int> = [:];)".into()));
                }
                let kty = self.check_expr(&pares[0].0)?;
                let vty = self.check_expr(&pares[0].1)?;
                for (k, v) in &pares[1..] {
                    let kt = self.check_expr(k)?;
                    if kt != kty {
                        return Err(self.err(k.line, k.col, format!(
                            "the Map keys must be of the same type: {} and {}", kty, kt)));
                    }
                    let vt = self.check_expr(v)?;
                    if vt != vty {
                        return Err(self.err(v.line, v.col, format!(
                            "the Map values must be of the same type: {} and {}", vty, vt)));
                    }
                }
                let mty = Type::Map(Box::new(kty), Box::new(vty));
                self.ensure_type(&mty, expr.line, expr.col)?; // clave hashable
                Ok(mty)
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
                        "cannot convert {} to {} with 'as' (only int↔float, char↔int and to/from u8/u32/u64)", from, to)));
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
                let r = self.check_fn_body(&fe.params, &fe.return_type, &fe.body, fe.line, fe.col, "the anonymous function");
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
                    return Err(self.err(cond.line, cond.col, format!("the if condition must be bool, not {}", ct)));
                }
                let then_ty = self.check_block(then_branch)?;
                match else_branch {
                    None => {
                        // Un if sin else tiene tipo unit; entonces la rama 'then'
                        // tampoco puede producir un valor útil.
                        if then_ty != Type::Unit {
                            return Err(self.err(expr.line, expr.col, format!(
                                "an if without else has type unit, but its branch produces {} (add an else)",
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
                                "the if branches have different types: {} and {}",
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
                    return Err(self.err(cond.line, cond.col, format!("the while condition must be bool, not {}", ct)));
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
                        "the operator '{}' requires both operands int or both float, not {} and {}",
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
                    "the operator '{}' compares int/float/string/char of the same type, not {} and {}",
                    bin_op_str(op), lt, rt
                ))),
            },
            // Igualdad: mismo tipo y comparable → bool.
            Eq | Ne => {
                if lt == rt && is_comparable(&lt) {
                    Ok(Type::Bool)
                } else {
                    Err(self.err(line, col, format!(
                        "the operator '{}' requires both operands of the same comparable type, not {} and {}",
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
                        "the operator '{}' requires bool operands, not {} and {}",
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
                    "the operator '{}' requires int operands, not {} and {}",
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
                    "the literal {} does not fit in u{}", n, w)));
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

    /// M48.1: reconoce y tipa una llamada a una **función asociada** `Tipo.fn(args)` (`Map.new()`,
    /// `Channel.new()`, `Channel.bounded(n)`). Llega como `Call(Field(Ident(tipo), fn), args)`. Devuelve
    /// `Some(resultado)` si `(tipo, fn)` es una asociada registrada; `None` si no lo es (el llamador
    /// sigue su camino normal: campo/método/UFCS). El resultado es un tipo genérico **indeterminado**
    /// (Map/Channel) → se toma del `expected`; sin él, error pidiendo anotar (como `[]`/`None`).
    fn try_assoc_call(&mut self, callee: &Expr, args: &[Expr], expected: Option<&Type>, line: usize, col: usize)
        -> Option<Result<Type, TypeError>>
    {
        let ExprKind::Field { object, name } = &callee.kind else { return None };
        let ExprKind::Ident(tn) = &object.kind else { return None };
        let assoc = crate::builtins::assoc_lookup(tn, name)?;
        // M48.1/LSP: hover del nombre asociado (`new`/`bounded`), tras `Tipo.` (grafía canónica sin
        // espacios; el `+1` es el punto). Muestra la firma legible del registro.
        if self.gather {
            let ncol = object.col + tn.chars().count() + 1;
            self.record_named(object.line, ncol, name.chars().count(), assoc.sig.to_string(), None);
        }
        Some((|| {
            if args.len() != assoc.arity {
                return Err(self.err(line, col, format!(
                    "'{}.{}' expects {} argument(s), got {}", tn, name, assoc.arity, args.len())));
            }
            // Todos los argumentos actuales de asociadas son una capacidad `int` (`Channel.bounded`).
            for a in args {
                let at = self.check_expr(a)?;
                if !matches!(at, Type::Int) {
                    return Err(self.err(a.line, a.col, format!(
                        "the argument of '{}.{}' must be int, not {}", tn, name, at)));
                }
            }
            // El tipo del resultado lo fija el contexto esperado (indeterminado, como `map_new()`).
            match expected {
                Some(e) if matches!((tn.as_str(), e),
                    ("Map", Type::Map(_, _)) | ("Channel", Type::Channel(_))) => Ok(e.clone()),
                Some(e) => Err(self.err(line, col, format!(
                    "'{}.{}' produces a {}, but the context expects {}", tn, name, tn, e))),
                None => {
                    let ejemplo = if tn == "Map" {
                        "let m: Map<string, int> = Map.new()"
                    } else {
                        "let c: Channel<int> = Channel.new()"
                    };
                    Err(self.err(line, col, format!(
                        "cannot infer the type of '{}.{}'; annotate it, e.g. '{}'", tn, name, ejemplo)))
                }
            }
        })())
    }

    fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        line: usize,
        col: usize,
    ) -> Result<Type, TypeError> {
        // M48.1: función asociada `Tipo.fn(args)` sin tipo esperado (contexto no tipado) → error de
        // "anota el tipo". Con tipo esperado, la intercepta `check_expr_expected` (devuelve el tipo).
        if let Some(r) = self.try_assoc_call(callee, args, None, line, col) {
            return r;
        }
        match &callee.kind {
            // Llamada directa por nombre: `f(a, b)`.
            ExprKind::Ident(n) => {
                let n = n.clone();
                // Completion tras `|>`: el LSP repara `x |> parc` como `x |> __raycomplete__`, que el
                // parser desazucara a `__raycomplete__(x)`. Como `x |> f` ≡ `f(x)`, enumeramos los
                // miembros del tipo del receptor (el primer argumento) —las mismas funciones que ofrece
                // `x.`— en vez de dar error por función desconocida (M7.2 + M45).
                if self.completing && n == COMPLETION_SENTINEL {
                    if let Some(recv) = args.first() {
                        let ty = self.check_expr(recv)?;
                        self.member_hits = self.enumerate_pipeable(&ty);
                    }
                    return Ok(Type::Unit);
                }
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
                // `hover_directo = true`: `(line, col)` es la posición del nombre → registra el hover
                // del builtin ahí (`print`/`pow`/`abs`… muestran su firma con los tipos de la llamada).
                self.check_named_call_impl(&n, args, line, col, None, true)
            }

            // UFCS (M7.1): `recv.f(args)`. Si `f` es un **campo** del struct receptor,
            // es una llamada al valor de ese campo (semántica de M3/M4); si **no**, se
            // reescribe a la función libre `f(recv, args)`. La decisión necesita el
            // tipo del receptor —por eso vive aquí y no en una pre-pasada—; el nodo se
            // baja a una llamada ordinaria tras verificar (`lower_ufcs`).
            ExprKind::Field { object, name } => {
                let recv_ty = self.check_expr(object)?;
                // M45: completion de miembros cuando el reparado dejó una llamada `recv.<centinela>(…)`.
                if self.completing && name == COMPLETION_SENTINEL {
                    self.member_hits = self.enumerate_members(&recv_ty);
                    return Ok(Type::Unit);
                }
                // M9.3b/M9.5: receptor `dyn A + B` → despacho dinámico por la vtable del objeto.
                if let Type::Dyn(traits) = &recv_ty {
                    let traits = traits.clone();
                    return self.dispatch_dyn_method(&traits, name, args, line, col);
                }
                if let Type::Struct(sname, targs) = &recv_ty {
                    if let Some(fty) = self.struct_field_type(sname, targs, name) {
                        // M10.2g: hover del campo-función invocado (un campo no tiene declaración de fn).
                        self.record_field_hover(object.line, object.col, name, &fty, None);
                        return self.call_type(fty, args, false, line, col);
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
                    let ty = self.check_named_call(&mangled, &all, line, col, None)?;
                    // M10.2g: hover del **método de trait** en su nombre (su firma incluye el receptor).
                    let mty = self.functions.get(&mangled)
                        .map(|s| Type::Fn(s.params.clone(), Box::new(s.ret.clone())));
                    if let Some(mty) = mty {
                        let def = self.fn_defs.get(&mangled).copied();
                        self.record_field_hover(object.line, object.col, name, &mty, def);
                    }
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
                self.call_type(ty, args, Self::block_like(callee), line, col)
            }
        }
    }

    /// Enumera los miembros ofrecibles en `recv.` para un receptor de tipo `rt` (M45): campos del
    /// struct, métodos de trait/impl (incl. `@derive`), builtins-como-método de la categoría del
    /// tipo, y funciones libres UFCS cuyo primer parámetro acepta el receptor (esto cubre
    /// `map`/`filter`/`fold`/`sort` del prelude y las UFCS del usuario). Dedup por etiqueta.
    fn enumerate_members(&self, rt: &Type) -> Vec<MemberItem> {
        self.enumerate_members_impl(rt, false)
    }

    /// Los miembros ofrecibles tras un `|>` (pipeline, M7.2), que difieren de los de `recv.`: como
    /// `x |> f(a)` ≡ `f(x, a)` (desugaring puro del parser), lo pipeable son **funciones libres**
    /// (para CUALQUIER tipo, incluidos primitivos: `n |> duplicar`, `path |> read_file`) + los
    /// builtins invocables por nombre; NO los campos ni los **métodos de trait** (`n |> show` sería
    /// `show(n)`, y no existe una función libre `show` —solo `int#show`—).
    fn enumerate_pipeable(&self, rt: &Type) -> Vec<MemberItem> {
        self.enumerate_members_impl(rt, true)
    }

    /// Núcleo compartido por `enumerate_members` (para `recv.`) y `enumerate_pipeable` (para `x |>`).
    /// El flag `pipeable` cambia dos cosas: (a) omite campos y métodos de trait (no son pipeable), y
    /// (b) enumera funciones libres para todo tipo, no solo receptores compuestos.
    fn enumerate_members_impl(&self, rt: &Type, pipeable: bool) -> Vec<MemberItem> {
        let mut out: Vec<MemberItem> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let add = |out: &mut Vec<MemberItem>, seen: &mut std::collections::HashSet<String>,
                   label: String, kind: u8, detail: Option<String>, has_args: bool,
                   def: Option<(usize, usize)>| {
            if seen.insert(label.clone()) {
                out.push(MemberItem { label, kind, detail, has_args, def });
            }
        };

        // 1. Campos del struct (kind 5 = Field), con su tipo sustituido como detalle. No pipeable
        //    (`p |> x` sería `x(p)`; un campo no es una función libre).
        if !pipeable {
            if let Type::Struct(sname, targs) = rt {
                if let Some(fields) = self.structs.get(sname) {
                    let tparams = self.struct_tparams.get(sname).cloned().unwrap_or_default();
                    let sigma: HashMap<String, Type> = tparams.into_iter().zip(targs.iter().cloned()).collect();
                    for (fname, fty) in fields {
                        let ty = subst(fty, &sigma);
                        add(&mut out, &mut seen, fname.clone(), 5, Some(format!("{}", ty)), false, None);
                    }
                }
            }
        }

        // 2. Métodos de trait/impl del tipo concreto (kind 2 = Method). La tabla `methods` va por
        //    constructor (`type_key_of`): `Caja<int>` y `Caja<bool>` comparten métodos. Del mangled
        //    sacamos la aridad (para el snippet) y la posición de declaración (para sus `///` docs).
        //    No pipeable: un método de trait no es una función libre invocable por su nombre pelado.
        if !pipeable {
            if let Some(key) = type_key_of(rt) {
                for ((k, m), mangled) in &self.methods {
                    if k == &key {
                        let sig = self.functions.get(mangled);
                        let has_args = sig.map(|s| s.params.len() > 1).unwrap_or(false); // > self
                        let def = self.fn_defs.get(mangled).copied();
                        add(&mut out, &mut seen, m.clone(), 2, None, has_args, def);
                    }
                }
            }
        }

        // 3. Builtins invocables como método sobre la categoría del tipo (kind 2 = Method). Sí son
        //    pipeable (son globales invocables por nombre: `xs |> len`, `n |> to_string`).
        if let Some(cat) = member_category(rt) {
            for b in crate::builtins::methods_for(cat) {
                let has_args = crate::builtins::method_takes_args(b);
                add(&mut out, &mut seen, (*b).to_string(), 2, None, has_args, None);
            }
        }

        // 4. Funciones libres UFCS: primer parámetro que acepta el receptor (kind 3 = Function).
        //    Para `recv.` solo se enumeran con receptores **compuestos** (array/map/struct/enum/tupla):
        //    ahí la función opera SOBRE la estructura y `recv.f()` es idiomático (captura
        //    `map`/`filter`/`fold`/`sort` del prelude y las UFCS del usuario); para primitivos NO,
        //    porque una función que toma un `string` suele tratarlo como DATO (`read_file(path)`), no
        //    como método. Para `x |>` (pipeable) SÍ se enumeran para todo tipo: piping un primitivo a
        //    una función libre es justo el caso de uso del `|>`.
        //    Excluye sintéticos (`#`/`::`/`__`) y primer parámetro genérico pelado (`Var`, que
        //    unificaría con todo, p. ej. `assert_eq`).
        let composite_receiver = matches!(
            rt,
            Type::Array(_) | Type::Map(_, _) | Type::Struct(_, _) | Type::Enum(_, _) | Type::Tuple(_)
        );
        if pipeable || composite_receiver {
            for (fname, sig) in &self.functions {
                if fname.contains('#') || fname.contains("::") || fname.starts_with("__") {
                    continue;
                }
                if let Some(p0) = sig.params.first() {
                    if matches!(p0, Type::Var(_)) {
                        continue;
                    }
                    let mut sigma: HashMap<String, Type> = HashMap::new();
                    if unify(p0, rt, &mut sigma).is_ok() {
                        let has_args = sig.params.len() > 1; // > el receptor
                        let def = self.fn_defs.get(fname).copied();
                        add(&mut out, &mut seen, fname.clone(), 3, None, has_args, def);
                    }
                }
            }
        }

        out.sort_by(|a, b| a.label.cmp(&b.label));
        out
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
                "no field or function '{}' applicable to {}", name, recv_ty
            )));
        };
        let mut all_args = Vec::with_capacity(args.len() + 1);
        all_args.push(object.clone());
        all_args.extend_from_slice(args);
        let ty = self.check_named_call(&target, &all_args, line, col, None)?;
        // M10.2g: hover del **método UFCS** en su nombre — la firma de la función libre resuelta
        // (map/filter/fold, funciones propias). Los builtins (len/trim/…) no tienen FnSig → se omiten.
        let mty = self.functions.get(&target)
            .map(|s| Type::Fn(s.params.clone(), Box::new(s.ret.clone())));
        if let Some(mty) = mty {
            let def = self.fn_defs.get(&target).copied();
            self.record_field_hover(object.line, object.col, name, &mty, def);
        }
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
    fn check_named_call(&mut self, name: &str, args: &[Expr], line: usize, col: usize, expected: Option<&Type>) -> Result<Type, TypeError> {
        self.check_named_call_impl(name, args, line, col, expected, false)
    }

    /// Como [`check_named_call`], pero `hover_directo` indica que `(line, col)` es la posición del
    /// **nombre** llamado (llamada directa `f(...)`), no una reescritura (UFCS/método). Solo entonces
    /// se registra el hover del builtin ahí (M10.2i): así `print`/`pow`/`abs`… muestran su firma.
    fn check_named_call_impl(&mut self, name: &str, args: &[Expr], line: usize, col: usize, expected: Option<&Type>, hover_direct: bool) -> Result<Type, TypeError> {
        // Builtins (DESIGN.md §7): su firma vive en el **registro único** (`src/builtins.rs`), no
        // dispersa aquí. Se comprueban antes que una local/función homónima (un builtin no se tapa).
        // Se tipan los argumentos por el camino normal y la regla del builtin valida y da el tipo.
        if let Some(b) = crate::builtins::lookup(name) {
            let mut arg_types = Vec::with_capacity(args.len());
            for a in args {
                arg_types.push(self.check_expr(a)?);
            }
            return match (b.check)(&arg_types) {
                Ok(t) => {
                    // M10.2i: hover del builtin en su nombre — su firma con los tipos de ESTA llamada
                    // (`pow: fn(float, float) -> float`). Solo en llamada directa (posición correcta) y
                    // modo gather. Los builtins internos (`__…`) no se muestran (el usuario no los escribe).
                    // Se omite además un **wrapper sintético**: el `to_string(e)` que el parser inserta al
                    // desazucarar una interpolación `${e}` comparte la posición de su argumento (ambos en
                    // `(el, ec)`); su hover solaparía —y taparía por menor `len`— al del propio `e`. Una
                    // llamada escrita a mano nunca tiene el argumento en la misma columna que el callee.
                    let wrapper_sintetico = args.len() == 1 && args[0].line == line && args[0].col == col;
                    if hover_direct && self.gather && !name.starts_with("__") && !wrapper_sintetico {
                        let fn_ty = Type::Fn(arg_types.clone(), Box::new(t.clone()));
                        self.record_ident(line, col, name, &fn_ty, None);
                    }
                    Ok(t)
                }
                // El índice señala el argumento culpable (para el cursor); `None` → el sitio de llamada.
                Err((Some(i), msg)) => Err(self.err(args[i].line, args[i].col, msg)),
                Err((None, msg)) => Err(self.err(line, col, msg)),
            };
        }

        // Una variable local que guarda una función: llamada indirecta (M4.1).
        // (Tapa a una función global con el mismo nombre.)
        if let Some(v) = self.lookup(name) {
            let ty = v.ty.clone();
            return self.call_type(ty, args, false, line, col);
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
                self.check_generic_call(&type_params, &params, &ret, args, &label, line, col, expected)?;
            // M9.2: si la función tiene bounds, registrar los diccionarios a pasar en este
            // sitio (verificando que cada tipo inferido cumple su bound).
            if !bounds.is_empty() {
                self.record_dict_args(name, &bounds, &sigma, line, col)?;
            }
            return Ok(ret_ty);
        }

        Err(self.err(line, col, format!("function '{}' not declared", name)))
    }

    /// Verifica una llamada cuyo *callee* es un valor (no un nombre directo): su
    /// tipo debe ser una función, y los argumentos deben encajar con su firma.
    /// M87: `pista` = el callee es una expresión de BLOQUE (if/match/while/bloque) —
    /// casi siempre el gotcha §55 (la cola con '(' tras una sentencia) → el error lo dice.
    fn call_type(&mut self, ty: Type, args: &[Expr], pista: bool, line: usize, col: usize) -> Result<Type, TypeError> {
        match ty {
            Type::Fn(params, ret) => self.check_args(&params, *ret, args, "the function", line, col),
            other => {
                let mut msg = format!(
                    "cannot call a value of type {} (not a function)",
                    other
                );
                if pista {
                    msg.push_str(
                        "; note: a tail starting with '(' after an if/while/match/block parses as a call of its value — separate it with 'return' or 'let'",
                    );
                }
                Err(self.err(line, col, msg))
            }
        }
    }

    /// M87 (gotcha §55): ¿es una expresión "de bloque"? Una llamada/indexación cuyo
    /// callee es una de estas es casi siempre la cola mal parseada tras una sentencia.
    fn block_like(e: &Expr) -> bool {
        matches!(
            e.kind,
            ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::While { .. } | ExprKind::Block(_)
        )
    }

    /// Comprueba aridad y tipos de los argumentos contra una firma `(params -> ret)`
    /// y devuelve `ret`. Compartido por las llamadas directas y las indirectas.
    fn check_args(&mut self, params: &[Type], ret: Type, args: &[Expr], label: &str, line: usize, col: usize) -> Result<Type, TypeError> {
        if args.len() != params.len() {
            return Err(self.err(line, col, format!(
                "{} expects {} argument(s), received {}",
                label, params.len(), args.len()
            )));
        }
        for (i, (arg, expected)) in args.iter().zip(params.iter()).enumerate() {
            // El tipo del parámetro es el esperado del argumento (propaga a `None`,
            // `[]`, `Caja.Vacia`...).
            let at = self.check_expr_expected(arg, expected)?;
            if at != *expected {
                return Err(self.err(arg.line, arg.col, format!(
                    "argument {} of {}: expected {}, got {}",
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
    #[allow(clippy::too_many_arguments)]
    fn check_generic_call(
        &mut self,
        type_params: &[String],
        params: &[Type],
        ret: &Type,
        args: &[Expr],
        label: &str,
        line: usize,
        col: usize,
        expected: Option<&Type>,
    ) -> Result<(Type, HashMap<String, Type>), TypeError> {
        if args.len() != params.len() {
            return Err(self.err(line, col, format!(
                "{} expects {} argument(s), received {}",
                label, params.len(), args.len()
            )));
        }
        // σ: parámetro de tipo → tipo concreto inferido.
        let mut sigma: HashMap<String, Type> = HashMap::new();
        for (i, (arg, param)) in args.iter().zip(params.iter()).enumerate() {
            let at = self.check_expr(arg)?;
            unify(param, &at, &mut sigma).map_err(|reason| self.err(arg.line, arg.col, format!(
                "argument {} of {}: {}", i + 1, label, reason
            )))?;
        }
        // M40.3b: los argumentos mandan; si algún parámetro de tipo NO aparece en ellos (p. ej. un
        // constructor vacío `set_new() -> Set<T>`), se rellena desde el tipo ESPERADO unificando el
        // retorno con él. Best-effort en un σ aparte para no alterar lo ya inferido de los argumentos.
        if let Some(exp) = expected.filter(|_| type_params.iter().any(|tp| !sigma.contains_key(tp))) {
            let mut seed = HashMap::new();
            if unify(ret, exp, &mut seed).is_ok() {
                for tp in type_params {
                    if !sigma.contains_key(tp) {
                        if let Some(t) = seed.get(tp) { sigma.insert(tp.clone(), t.clone()); }
                    }
                }
            }
        }
        // Todos los parámetros de tipo deben haber quedado determinados.
        for tp in type_params {
            if !sigma.contains_key(tp) {
                return Err(self.err(line, col, format!(
                    "could not infer the type parameter '{}' of {} (it does not appear in the arguments)",
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
                "method '{}' ambiguous for '{}': several bounded traits declare it ({})",
                method, tp, hits.join(", ")
            )));
        }
        let trait_name = &hits[0];
        let sig = self
            .traits
            .get(trait_name)
            .unwrap_or_else(|| crate::ice!("the trait '{}' of the bound is not registered", trait_name))
            .iter()
            .find(|m| m.name == method)
            .unwrap_or_else(|| crate::ice!("the method '{}' is not in trait '{}'", method, trait_name))
            .clone();
        let self_ty = Type::Var(tp.to_string());
        // El receptor ya casó con `self` (es `T`); comprobar los argumentos restantes.
        let expected: Vec<Type> = sig.params.iter().skip(1)
            .map(|p| self.resolve_type(&subst_self(&p.ty, &self_ty)))
            .collect();
        if args.len() != expected.len() {
            return Err(self.err(line, col, format!(
                "the method '{}' expects {} argument(s) (not counting the receiver), received {}",
                method, expected.len(), args.len()
            )));
        }
        for (i, (arg, exp)) in args.iter().zip(&expected).enumerate() {
            let at = self.check_expr_expected(arg, exp)?;
            if at != *exp {
                return Err(self.err(arg.line, arg.col, format!(
                    "argument {} of method '{}': expected {}, got {}", i + 1, method, exp, at
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
                "'dyn {}' does not declare a method '{}'", traits.join(" + "), method
            ))),
        };
        // *Object safety*: la vtable no puede llevar métodos que dependan del tipo concreto
        // borrado. Si `Self` aparece fuera del receptor (en un parámetro o en el retorno),
        // el método no es invocable sobre un trait object.
        let uses_self = sig.params.iter().skip(1).any(|p| type_uses_self(&p.ty)) || type_uses_self(&sig.return_type);
        if uses_self {
            return Err(self.err(line, col, format!(
                "the method '{}' uses 'Self': it is not callable on 'dyn {}'", method, traits.join(" + ")
            )));
        }
        // Argumentos (sin el receptor, que es el propio objeto).
        let expected: Vec<Type> = sig.params.iter().skip(1).map(|p| self.resolve_type(&p.ty)).collect();
        if args.len() != expected.len() {
            return Err(self.err(line, col, format!(
                "the method '{}' expects {} argument(s) (not counting the receiver), received {}",
                method, expected.len(), args.len()
            )));
        }
        for (i, (arg, exp)) in args.iter().zip(&expected).enumerate() {
            let at = self.check_expr_expected(arg, exp)?;
            if at != *exp {
                return Err(self.err(arg.line, arg.col, format!(
                    "argument {} of method '{}': expected {}, got {}", i + 1, method, exp, at
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
                "the type parameter '{}' is not bounded by '{}' (required by the call)", u, trait_name
            )));
        }
        // Tipo concreto: debe implementar el trait → usar el método manglado del impl.
        let key = type_key_of(concrete).ok_or_else(|| self.err(line, col, format!(
            "{} cannot implement the trait '{}'", concrete, trait_name
        )))?;
        if !self.impl_traits.contains(&(key.clone(), trait_name.to_string())) {
            return Err(self.err(line, col, format!(
                "{} does not implement '{}' (required by the call)", concrete, trait_name
            )));
        }
        // M9.2b: si el impl es **genérico y acotado**, su función manglada lleva sus propios
        // parámetros-diccionario, así que no se puede pasar plana: hay que envolverla en un
        // **closure** que rellene los diccionarios internos (anidados).
        let bounded_gi = self.generic_impls.get(&(key.clone(), trait_name.to_string()))
            .filter(|gi| !gi.bounds.is_empty())
            .cloned();
        if let Some(gi) = bounded_gi {
            let sig = self.traits.get(trait_name)
                .and_then(|ms| ms.iter().find(|m| m.name == method))
                .cloned()
                .unwrap_or_else(|| crate::ice!("the method does not belong to the trait (the impl was validated)"));
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
        let body = Block { statements: Vec::new(), tail: Some(Box::new(call)), line, col, end_line: line };
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
            .unwrap_or_else(|| crate::ice!("no active scope when declaring a variable"))
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

    /// Registra el hover de un **campo o método** `name` accedido en `recv.name` (M10.2g). La
    /// posición del acceso es la del receptor `(recv_line, recv_col)`; el hover se coloca en la
    /// posición real del `name` tras el `.` (de `field_name_pos`, poblada por el parser). No hace
    /// nada sin `gather` ni si no se conoce la posición del nombre.
    fn record_field_hover(&mut self, recv_line: usize, recv_col: usize, name: &str, ty: &Type, def: Option<(usize, usize)>) {
        if !self.gather {
            return;
        }
        // Todas las posiciones de este `(receptor, nombre)`: en una cadena (`v.doble().inc().doble()`)
        // dos `.doble()` comparten clave (misma posición de receptor) → se registra el hover en ambas.
        // Todas resuelven a la misma función (mismo `name` sobre el mismo receptor) → misma firma.
        if let Some(positions) = self.field_name_pos.get(&(recv_line, recv_col, name.to_string())).cloned() {
            let len = name.chars().count();
            for (nl, nc) in positions {
                self.index.hovers.push(HoverEntry { line: nl, col: nc, len, text: format!("{}: {}", name, ty) });
                // M10.2h: si el método resuelve a una función conocida (manglada de un impl, o libre en
                // UFCS), registramos su declaración → habilita ir-a-definición y documentación (`///`) del
                // método en el hover. Un campo-función del struct no tiene `fn_defs` → sin `def` (None).
                if let Some((def_line, def_col)) = def {
                    self.index.defs.push(DefEntry { line: nl, col: nc, len, def_line, def_col });
                }
            }
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
        // M41.4b: un `ptr` se compara con == por identidad (misma dirección foránea).
        Type::Ptr => true,
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
/// Higiene de la inferencia de construcción (M40.2e): renombra los parámetros de tipo del tipo
/// construido a nombres frescos (`$ctor$i`, ilegales para el usuario) para que **no colisionen con
/// parámetros de tipo rígidos del ámbito**. Sin esto, `fn f<T>() -> Option<(int, T)> { Option.Some(
/// (0, x)) }` confunde el `T` de `Option` con el `T` de `f` y liga `T := (int, T)` (occurs-check
/// falso). Devuelve `(tparams frescos, tipos con los params renombrados)` en el mismo orden (los
/// argumentos de tipo resultantes siguen la posición, así que los bounds usan los nombres originales).
fn freshen_ctor_params(tparams: &[String], types: &[Type], in_scope: &HashSet<String>) -> (Vec<String>, Vec<Type>) {
    // Solo se renombran los parámetros que **colisionan** con un parámetro de tipo del ámbito; sin
    // colisión no se toca nada, así los mensajes de error conservan los nombres originales (`'A'`).
    if !tparams.iter().any(|t| in_scope.contains(t)) {
        return (tparams.to_vec(), types.to_vec());
    }
    let mut ren: HashMap<String, Type> = HashMap::new();
    let fresh: Vec<String> = tparams.iter().enumerate().map(|(i, t)| {
        if in_scope.contains(t) {
            let f = format!("$ctor${}", i);
            ren.insert(t.clone(), Type::Var(f.clone()));
            f
        } else {
            t.clone()
        }
    }).collect();
    let types = types.iter().map(|t| subst(t, &ren)).collect();
    (fresh, types)
}

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
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| subst(t, sigma)).collect()),
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
                return Err(format!("'{}' cannot be {} and {} at the same time", n, prev, arg));
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
                return Err(format!("expected {}, got {}", param, arg));
            }
            for (a, b) in p1.iter().zip(p2) {
                unify(a, b, sigma)?;
            }
            unify(r1, r2, sigma)
        }
        // Tuplas (M27.1): misma aridad y unificar posición a posición (habilita genéricos sobre
        // tuplas, p. ej. `Iter<(int, T)>` de `enumerate`).
        (Type::Tuple(t1), Type::Tuple(t2)) if t1.len() == t2.len() => {
            for (a, b) in t1.iter().zip(t2) {
                unify(a, b, sigma)?;
            }
            Ok(())
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
        _ => Err(format!("expected {}, got {}", param, arg)),
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

/// ¿El patrón es **irrefutable** (casa siempre, sin importar el valor)? Un `_`/binding, o un patrón
/// de struct cuyos campos son todos irrefutables (`Punto { x, y }`). Una variante es **refutable**
/// (solo casa una de las variantes). Se usa para la exhaustividad conservadora (M40.1c/1d): una
/// variante de primer nivel cubre solo si sus sub-patrones son irrefutables.
fn is_irrefutable(p: &Pattern) -> bool {
    match &p.kind {
        PatternKind::Wildcard | PatternKind::Binding(_) => true,
        PatternKind::Struct { fields, .. } => fields.iter().all(|(_, f)| is_irrefutable(f)),
        PatternKind::Variant { .. } => false,
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
                    ForIter::Iter { expr, .. } => resolve_expr(expr, enums),
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
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { resolve_expr(k, enums); resolve_expr(v, enums); }
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
                resolve_expr(&mut arm.body, enums); if let Some(g) = &mut arm.guard { resolve_expr(g, enums); }
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
        _ => crate::ice!("ident_name requires an Ident"),
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
    // Solo `From<S>` usa el mecanismo de **conversión** (su método `desde` es asociado —sin `self`—,
    // consumido por `?`). Otros traits parametrizados (p. ej. `Iterator<T>`, M40.2) van por el
    // despacho normal por punto: sus métodos con `self` se registran en la tabla de métodos.
    !imp.trait_args.is_empty() && imp.trait_name == "From"
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
        // M48.4: constructores incorporados como objetivo de impl (`impl Len for [T]`/`Map<K,V>`/`bytes`).
        // La clave va por CONSTRUCTOR (como `Caja<int>`→"Caja"): `[int]`/`[bool]` comparten "[]".
        Type::Array(_) => "[]".into(),
        Type::Map(_, _) => "Map".into(),
        Type::Bytes => "bytes".into(),
        _ => return None,
    })
}

/// La **categoría** de un tipo para los builtins-como-método del completion (M45): la clave que
/// entiende `builtins::methods_for`. Cubre también arreglos y `Map`, que no tienen `type_key_of`.
fn member_category(ty: &Type) -> Option<&'static str> {
    Some(match ty {
        Type::String => "string",
        Type::Bytes => "bytes",
        Type::Char => "char",
        Type::Int => "int",
        Type::Float => "float",
        Type::Bool => "bool",
        Type::Array(_) => "array",
        Type::Map(_, _) => "map",
        _ => return None,
    })
}

/// Completion de miembros (M45): los símbolos ofrecibles tras `recv.`. El LSP repara la fuente
/// insertando el centinela `__raycomplete__` tras el `.`; aquí corremos el front-end best-effort
/// (con recuperación de errores) y, al tipar ese acceso, enumeramos los miembros del tipo del
/// receptor. Devuelve `[]` si el receptor no tipa o no tiene miembros. No exige `main` (puede ser
/// un fragmento a medio escribir).
pub fn member_completion(program: &mut Program) -> Vec<MemberItem> {
    if prepare_program(program).is_err() {
        return Vec::new();
    }
    let mut checker = Checker::new();
    checker.completing = true;
    checker.require_main = false;
    checker.gather = true; // puebla `fn_defs` → posición de los métodos/UFCS para sus `///` docs
    let _ = checker.check_program(program); // best-effort: el error de tipos del fragmento es esperado
    checker.member_hits
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
    let mut new_impls: Vec<ImplBlock> = Vec::new();
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
                    "Eq" => new_impls.push(parse_derived_impl("Eq", &s.name, "fn eq(self, other: Self) -> bool", &struct_eq_body(&s.fields))),
                    "Show" => new_impls.push(parse_derived_impl("Show", &s.name, "fn show(self) -> string", &struct_show_body(a, &s.name, &s.fields)?)),
                    "Hash" => new_impls.push(parse_derived_impl("Hash", &s.name, "fn hash(self) -> int", &struct_hash_body(&s.fields))),
                    _ => crate::ice!("validate_derive guarantees a known trait"),
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
                    "Eq" => new_impls.push(parse_derived_impl("Eq", &e.name, "fn eq(self, other: Self) -> bool", &enum_eq_body(&e.name, &e.variants))),
                    "Show" => new_impls.push(parse_derived_impl("Show", &e.name, "fn show(self) -> string", &enum_show_body(a, &e.name, &e.variants)?)),
                    "Hash" => new_impls.push(parse_derived_impl("Hash", &e.name, "fn hash(self) -> int", &enum_hash_body(&e.name, &e.variants))),
                    _ => crate::ice!("validate_derive guarantees a known trait"),
                }
            }
        }
    }
    // M40.3a: dar a cada cuerpo derivado posiciones **sintéticas únicas y globales**. Cada impl se
    // parsea desde la línea 1, así que dos derivados (o el mismo re-generado por módulo) colisionarían
    // en las bajadas por posición (UFCS/despacho): p. ej. `self.x.hash()` (int) y `self.n.hash()`
    // (string) en la misma `(línea, col)` se bajarían al MISMO destino → despacho equivocado. Un
    // contador atómico global reserva una banda de 1M por método (base 50M, disjunta de la 1M de los
    // métodos por defecto y muy por encima de cualquier fuente real). Antes solo funcionaba por suerte
    // cuando los campos colisionantes iban al mismo destino (p. ej. `@derive(Show)` con campos del
    // mismo tipo). Ver `freshen_positions`.
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DERIVE_FRESH: AtomicUsize = AtomicUsize::new(49_000_000);
    for imp in &mut new_impls {
        for m in &mut imp.methods {
            let mut next = DERIVE_FRESH.fetch_add(1_000_000, Ordering::Relaxed);
            freshen_positions(&mut m.body, &mut next);
        }
    }
    program.impls.extend(new_impls);
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
        return Err(TypeError { msg: "'@derive' requires at least one trait (e.g. @derive(Eq))".into(), line: a.line, col: a.col, len: 1 });
    }
    for arg in &a.args {
        if arg != "Eq" && arg != "Show" && arg != "Hash" {
            return Err(TypeError { msg: format!("cannot derive '{}' (for now Eq, Show and Hash)", arg), line: a.line, col: a.col, len: 1 });
        }
    }
    if !type_params.is_empty() {
        return Err(TypeError { msg: format!("cannot derive for the generic type '{}'", name), line: a.line, col: a.col, len: 1 });
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
        Type::Struct(_, _) | Type::Enum(_, _) => Ok(format!("{expr}.show()")),
        other => Err(TypeError {
            msg: format!("cannot derive Show for a field of type {} (for now primitives, struct and enum)", other),
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
    let mut parts: Vec<String> = Vec::new();
    for (n, ty) in fields {
        parts.push(format!("\"{n}: \" + {}", render_to_string(a, &format!("self.{n}"), ty)?));
    }
    // El string generado usa llaves literales `{`/`}` (siempre lo son; solo `${` interpola, M27.3).
    Ok(format!("        \"{name} {{ \" + {} + \" }}\"", parts.join(" + \", \" + ")))
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
fn parse_derived_impl(trait_name: &str, name: &str, signature: &str, body: &str) -> ImplBlock {
    let src = format!(
        "impl {trait_name} for {name} {{\n    {signature} {{\n{body}\n    }}\n}}"
    );
    let toks = crate::lexer::lex(&src).unwrap_or_else(|e| crate::ice!("the derived impl does not lex: {e}"));
    let mut prog = crate::parser::parse(toks).unwrap_or_else(|e| crate::ice!("the derived impl does not parse: {e}"));
    prog.impls.remove(0)
}

/// Cuerpo de `igual` para un struct: conjunción de la igualdad de cada campo (sin campos →
/// `true`).
/// Cuerpo de `hash` para un struct (M40.3a): combina el `.hash()` de cada campo con un polinomio
/// `h = h*31 + campo.hash()` (arranca en 17). Sin campos → `17`. Cada campo debe implementar `Hash`
/// (el checker lo exige al verificar el cuerpo generado); un campo no hashable (float/array) → error.
/// M61.1: el int es checked (trap) → tanto el acumulador como el hash ENTRANTE de cada campo
/// (que puede ser cualquier i64, p. ej. un int grande que hashea a sí mismo) se acotan a 32 bits.
fn struct_hash_body(fields: &[(String, Type)]) -> String {
    let mut acc = "17".to_string();
    for (n, _) in fields {
        acc = format!("(({acc} * 31 + (self.{n}.hash() & 4294967295)) & 4294967295)");
    }
    format!("        {acc}")
}

/// Cuerpo de `hash` para un enum (M40.3a): `match` sobre `self`; el hash arranca en el índice de la
/// variante y combina el `.hash()` de cada elemento del payload (variante unit → su índice).
fn enum_hash_body(name: &str, variants: &[VariantDef]) -> String {
    let mut arms = String::new();
    for (idx, v) in variants.iter().enumerate() {
        let k = v.payload.len();
        if k == 0 {
            arms.push_str(&format!("            {name}.{v} => {idx},\n", v = v.name));
        } else {
            let binds: Vec<String> = (0..k).map(|i| format!("a{i}")).collect();
            let mut acc = format!("{idx}");
            for i in 0..k {
                // M61.1: acotado a 32 bits, como struct_hash_body (el int es checked).
                acc = format!("(({acc} * 31 + (a{i}.hash() & 4294967295)) & 4294967295)");
            }
            arms.push_str(&format!(
                "            {name}.{v}({b}) => {acc},\n",
                v = v.name, b = binds.join(", ")
            ));
        }
    }
    format!("        match (self) {{\n{arms}        }}")
}

fn struct_eq_body(fields: &[(String, Type)]) -> String {
    if fields.is_empty() {
        return "        true".into();
    }
    let cmps: Vec<String> = fields.iter().map(|(n, _)| format!("self.{n} == other.{n}")).collect();
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
                "            {name}.{v} => match (other) {{ {name}.{v} => true, _ => false }},\n",
                v = v.name
            ));
        } else {
            let a: Vec<String> = (0..k).map(|i| format!("a{i}")).collect();
            let b: Vec<String> = (0..k).map(|i| format!("b{i}")).collect();
            let cmp: Vec<String> = (0..k).map(|i| format!("a{i} == b{i}")).collect();
            arms.push_str(&format!(
                "            {name}.{v}({a}) => match (other) {{ {name}.{v}({b}) => {cmp}, _ => false }},\n",
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
                    ForIter::Iter { expr, .. } => freshen_expr(expr, next),
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
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { freshen_expr(k, next); freshen_expr(v, next); }
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
                freshen_expr(&mut arm.body, next); if let Some(g) = &mut arm.guard { freshen_expr(g, next); }
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
                    ForIter::Iter { expr, .. } => renumber_expr(expr, next),
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
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { renumber_expr(k, next); renumber_expr(v, next); }
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
                renumber_expr(&mut arm.body, next); if let Some(g) = &mut arm.guard { renumber_expr(g, next); }
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

/// M52: **inlining de forwarders triviales**. Los impls-para-builtins de M48.4 (`impl<T> Push<T>
/// for [T] { fn push(self, x) { __push(self, x) } }`) hacen que cada `a.push(i)` pague una llamada
/// VM completa (marco + call + return) para ejecutar UN opcode — medido: arrays/gcnested +38-39 %
/// respecto al opcode directo pre-M48.4 (IDEAS §11). Este pase detecta las funciones **manglada**
/// (`Tipo#metodo`; un local no puede llamarse así → reescribir el callee es seguro) cuyo cuerpo es
/// **exactamente una llamada a builtin pasando sus params en orden**, y reescribe cada sitio de
/// llamada `Tipo#metodo(args)` a `__builtin(args)`. Los args se evalúan igual y en el mismo orden →
/// semántica idéntica en ambos motores (el oráculo no se toca). El forwarder NO se elimina: puede
/// seguir referenciado como valor (vtables de `dyn`, diccionarios de bounds).
fn inline_forwarders(program: &mut Program) {
    // 1. Mapa método-manglado → builtin. Solo funciones sin bounds (con bounds,
    //    `append_dict_params` ya les añadió params-diccionario y el patrón no casa).
    let user_fns: HashSet<&str> = program.functions.iter().map(|f| f.name.as_str()).collect();
    let mut fwd: HashMap<String, String> = HashMap::new();
    for f in &program.functions {
        if !f.name.contains('#') || !f.bounds.is_empty() || !f.body.statements.is_empty() {
            continue;
        }
        let Some(tail) = &f.body.tail else { continue };
        let ExprKind::Call { callee, args } = &tail.kind else { continue };
        let ExprKind::Ident(b) = &callee.kind else { continue };
        // Debe ser un builtin de verdad (y no taparlo una función del programa homónima).
        if crate::builtins::lookup(b).is_none() || user_fns.contains(b.as_str()) {
            continue;
        }
        let en_order = args.len() == f.params.len()
            && args
                .iter()
                .zip(&f.params)
                .all(|(a, p)| matches!(&a.kind, ExprKind::Ident(n) if *n == p.name));
        if en_order {
            fwd.insert(f.name.clone(), b.clone());
        }
    }
    if fwd.is_empty() {
        return;
    }
    // 1b. Sonoridad: el compilador resuelve variable-local ANTES que builtin, así que si en algún
    //     sitio del programa hay una variable ligada con el nombre de un builtin objetivo (un
    //     `let __push = …`, legal aunque exótico), reescribir hacia ese nombre podría capturarla.
    //     Aproximación conservadora: se excluye ese builtin del inlining en TODO el programa
    //     (coste cero en la práctica: nadie liga nombres `__*`).
    let mut bound: HashSet<String> = HashSet::new();
    for f in &program.functions {
        for p in &f.params {
            bound.insert(p.name.clone());
        }
        collect_bound_names_block(&f.body, &mut bound);
    }
    fwd.retain(|_, b| !bound.contains(b));
    if fwd.is_empty() {
        return;
    }
    // 2. Reescribir los sitios de llamada en todo el AST (incluidos cuerpos de fn-exprs).
    for f in &mut program.functions {
        inline_forwarders_block(&mut f.body, &fwd);
    }
}

/// M52: recolecta todos los nombres que el programa liga como **variables** (let/var, tuplas,
/// `for`, bindings de `match`, params de fn anónimas) — soporte de la guarda de sonoridad de
/// `inline_forwarders` (ver arriba). No distingue ámbitos: es una aproximación conservadora.
fn collect_bound_names_block(block: &Block, bound: &mut HashSet<String>) {
    for stmt in &block.statements {
        match &stmt.kind {
            StmtKind::Let { name, value, .. } => {
                bound.insert(name.clone());
                collect_bound_names_expr(value, bound);
            }
            StmtKind::LetTuple { names, value, .. } => {
                for n in names.iter().flatten() {
                    bound.insert(n.clone());
                }
                collect_bound_names_expr(value, bound);
            }
            StmtKind::For { pat, iter, body } => {
                match pat {
                    ForPat::Single(n) => {
                        bound.insert(n.clone());
                    }
                    ForPat::Tuple(ns) => {
                        for n in ns.iter().flatten() {
                            bound.insert(n.clone());
                        }
                    }
                }
                match iter {
                    ForIter::Range { start, end } => {
                        collect_bound_names_expr(start, bound);
                        collect_bound_names_expr(end, bound);
                    }
                    ForIter::In(e) | ForIter::Iter { expr: e, .. } => collect_bound_names_expr(e, bound),
                }
                collect_bound_names_block(body, bound);
            }
            StmtKind::Assign { target, value } => {
                collect_bound_names_expr(target, bound);
                collect_bound_names_expr(value, bound);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    collect_bound_names_expr(v, bound);
                }
            }
            StmtKind::Expr(e) => collect_bound_names_expr(e, bound),
        }
    }
    if let Some(t) = &block.tail {
        collect_bound_names_expr(t, bound);
    }
}

fn collect_bound_names_pattern(pat: &Pattern, bound: &mut HashSet<String>) {
    match &pat.kind {
        PatternKind::Binding(n) => {
            bound.insert(n.clone());
        }
        PatternKind::Variant { subpatterns, .. } => {
            for sp in subpatterns {
                collect_bound_names_pattern(sp, bound);
            }
        }
        _ => {}
    }
}

fn collect_bound_names_expr(expr: &Expr, bound: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } | ExprKind::Try(inner) => {
            collect_bound_names_expr(inner, bound)
        }
        ExprKind::Binary { left, right, .. } => {
            collect_bound_names_expr(left, bound);
            collect_bound_names_expr(right, bound);
        }
        ExprKind::Call { callee, args } => {
            collect_bound_names_expr(callee, bound);
            for a in args {
                collect_bound_names_expr(a, bound);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                collect_bound_names_expr(e, bound);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares {
                collect_bound_names_expr(k, bound);
                collect_bound_names_expr(v, bound);
            }
        }
        ExprKind::Index { array, index } => {
            collect_bound_names_expr(array, bound);
            collect_bound_names_expr(index, bound);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                collect_bound_names_expr(e, bound);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                collect_bound_names_expr(a, bound);
            }
        }
        ExprKind::Field { object, .. } => collect_bound_names_expr(object, bound),
        ExprKind::Func(fe) => {
            for p in &fe.params {
                bound.insert(p.name.clone());
            }
            collect_bound_names_block(&fe.body, bound);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_bound_names_expr(scrutinee, bound);
            for arm in arms {
                collect_bound_names_pattern(&arm.pattern, bound);
                collect_bound_names_expr(&arm.body, bound);
                if let Some(g) = &arm.guard {
                    collect_bound_names_expr(g, bound);
                }
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            collect_bound_names_expr(cond, bound);
            collect_bound_names_block(then_branch, bound);
            if let Some(e) = else_branch {
                collect_bound_names_expr(e, bound);
            }
        }
        ExprKind::While { cond, body } => {
            collect_bound_names_expr(cond, bound);
            collect_bound_names_block(body, bound);
        }
        ExprKind::Block(b) => collect_bound_names_block(b, bound),
        _ => {}
    }
}

fn inline_forwarders_block(block: &mut Block, fwd: &HashMap<String, String>) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => inline_forwarders_expr(value, fwd),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => {
                        inline_forwarders_expr(start, fwd);
                        inline_forwarders_expr(end, fwd);
                    }
                    ForIter::In(e) => inline_forwarders_expr(e, fwd),
                    ForIter::Iter { expr, .. } => inline_forwarders_expr(expr, fwd),
                }
                inline_forwarders_block(body, fwd);
            }
            StmtKind::Assign { target, value } => {
                inline_forwarders_expr(target, fwd);
                inline_forwarders_expr(value, fwd);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    inline_forwarders_expr(v, fwd);
                }
            }
            StmtKind::Expr(e) => inline_forwarders_expr(e, fwd),
        }
    }
    if let Some(t) = &mut block.tail {
        inline_forwarders_expr(t, fwd);
    }
}

fn inline_forwarders_expr(expr: &mut Expr, fwd: &HashMap<String, String>) {
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => inline_forwarders_expr(inner, fwd),
        ExprKind::Binary { left, right, .. } => {
            inline_forwarders_expr(left, fwd);
            inline_forwarders_expr(right, fwd);
        }
        ExprKind::Call { callee, args } => {
            // El corazón del pase: renombrar el callee si es un forwarder conocido. Solo en
            // posición de llamada (una referencia como VALOR debe seguir apuntando a la función).
            if let ExprKind::Ident(n) = &mut callee.kind
                && let Some(b) = fwd.get(n)
            {
                *n = b.clone();
            }
            inline_forwarders_expr(callee, fwd);
            for a in args {
                inline_forwarders_expr(a, fwd);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                inline_forwarders_expr(e, fwd);
            }
        }
        ExprKind::MapLit(pares) => {
            for (k, v) in pares {
                inline_forwarders_expr(k, fwd);
                inline_forwarders_expr(v, fwd);
            }
        }
        ExprKind::Index { array, index } => {
            inline_forwarders_expr(array, fwd);
            inline_forwarders_expr(index, fwd);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                inline_forwarders_expr(e, fwd);
            }
        }
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                inline_forwarders_expr(a, fwd);
            }
        }
        ExprKind::Field { object, .. } => inline_forwarders_expr(object, fwd),
        ExprKind::Func(fe) => inline_forwarders_block(&mut fe.body, fwd),
        ExprKind::Match { scrutinee, arms } => {
            inline_forwarders_expr(scrutinee, fwd);
            for arm in arms {
                inline_forwarders_expr(&mut arm.body, fwd);
                if let Some(g) = &mut arm.guard {
                    inline_forwarders_expr(g, fwd);
                }
            }
        }
        ExprKind::Try(inner) => inline_forwarders_expr(inner, fwd),
        ExprKind::If { cond, then_branch, else_branch } => {
            inline_forwarders_expr(cond, fwd);
            inline_forwarders_block(then_branch, fwd);
            if let Some(e) = else_branch {
                inline_forwarders_expr(e, fwd);
            }
        }
        ExprKind::While { cond, body } => {
            inline_forwarders_expr(cond, fwd);
            inline_forwarders_block(body, fwd);
        }
        ExprKind::Block(b) => inline_forwarders_block(b, fwd),
        _ => {}
    }
}

/// M40.2: baja `for x in it` (sobre un iterador) reescribiendo `ForIter::In` a `ForIter::Iter` con
/// el nombre manglado de `next`, en cada `for` cuya `(línea, col)` esté en `sites`. Recorre todo el
/// AST (bloques anidados en if/while/match/fn) para alcanzar cualquier `for`.
fn lower_for_iters(program: &mut Program, sites: &HashMap<(usize, usize), String>) {
    if sites.is_empty() {
        return;
    }
    for f in &mut program.functions {
        lower_for_iters_block(&mut f.body, sites);
    }
}

fn lower_for_iters_block(block: &mut Block, sites: &HashMap<(usize, usize), String>) {
    for stmt in &mut block.statements {
        let pos = (stmt.line, stmt.col);
        match &mut stmt.kind {
            StmtKind::For { iter, body, .. } => {
                if let (Some(next_fn), ForIter::In(_)) = (sites.get(&pos), &*iter) {
                    let old = std::mem::replace(iter, ForIter::In(Expr { kind: ExprKind::Int(0), line: 0, col: 0 }));
                    if let ForIter::In(e) = old {
                        *iter = ForIter::Iter { expr: e, next_fn: next_fn.clone() };
                    }
                }
                match iter {
                    ForIter::Range { start, end } => { lower_for_iters_expr(start, sites); lower_for_iters_expr(end, sites); }
                    ForIter::In(e) => lower_for_iters_expr(e, sites),
                    ForIter::Iter { expr, .. } => lower_for_iters_expr(expr, sites),
                }
                lower_for_iters_block(body, sites);
            }
            StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => lower_for_iters_expr(value, sites),
            StmtKind::Assign { target, value } => { lower_for_iters_expr(target, sites); lower_for_iters_expr(value, sites); }
            StmtKind::Return { value } => { if let Some(v) = value { lower_for_iters_expr(v, sites); } }
            StmtKind::Expr(e) => lower_for_iters_expr(e, sites),
        }
    }
    if let Some(t) = &mut block.tail {
        lower_for_iters_expr(t, sites);
    }
}

fn lower_for_iters_expr(expr: &mut Expr, sites: &HashMap<(usize, usize), String>) {
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } | ExprKind::Try(inner) => lower_for_iters_expr(inner, sites),
        ExprKind::Binary { left, right, .. } => { lower_for_iters_expr(left, sites); lower_for_iters_expr(right, sites); }
        ExprKind::Call { callee, args } => { lower_for_iters_expr(callee, sites); for a in args { lower_for_iters_expr(a, sites); } }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => { for e in elems { lower_for_iters_expr(e, sites); } }
        ExprKind::MapLit(pares) => { for (k, v) in pares { lower_for_iters_expr(k, sites); lower_for_iters_expr(v, sites); } }
        ExprKind::Index { array, index } => { lower_for_iters_expr(array, sites); lower_for_iters_expr(index, sites); }
        ExprKind::StructLit { fields, .. } => { for (_, e) in fields { lower_for_iters_expr(e, sites); } }
        ExprKind::EnumLit { args, .. } => { for a in args { lower_for_iters_expr(a, sites); } }
        ExprKind::Field { object, .. } => lower_for_iters_expr(object, sites),
        ExprKind::Func(fe) => lower_for_iters_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_for_iters_expr(scrutinee, sites);
            for arm in arms {
                lower_for_iters_expr(&mut arm.body, sites);
                if let Some(g) = &mut arm.guard { lower_for_iters_expr(g, sites); }
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            lower_for_iters_expr(cond, sites);
            lower_for_iters_block(then_branch, sites);
            if let Some(e) = else_branch { lower_for_iters_expr(e, sites); }
        }
        ExprKind::While { cond, body } => { lower_for_iters_expr(cond, sites); lower_for_iters_block(body, sites); }
        ExprKind::Block(b) => lower_for_iters_block(b, sites),
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
/// M40.2c: sustituye los parámetros de tipo de un trait por los argumentos del impl. En el AST
/// crudo (pre-resolución) un parámetro de tipo aparece como `Struct(nombre, [])`; aquí se reemplaza
/// por su argumento. Se usa al bajar un método de un impl de trait parametrizado
/// (`impl Iterator<int> for RangeIter`) para que `fn map<U>(self, f: fn(T) -> U)` herede `T = int`
/// en su firma. Como `subst_self` pero por nombre y para varios parámetros a la vez.
fn subst_named(ty: &Type, sigma: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Struct(n, args) if args.is_empty() && sigma.contains_key(n) => sigma[n].clone(),
        Type::Array(e) => Type::Array(Box::new(subst_named(e, sigma))),
        Type::Map(k, v) => Type::Map(Box::new(subst_named(k, sigma)), Box::new(subst_named(v, sigma))),
        Type::Channel(t) => Type::Channel(Box::new(subst_named(t, sigma))),
        Type::Task(t) => Type::Task(Box::new(subst_named(t, sigma))),
        Type::Fn(ps, r) => Type::Fn(
            ps.iter().map(|p| subst_named(p, sigma)).collect(),
            Box::new(subst_named(r, sigma)),
        ),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| subst_named(t, sigma)).collect()),
        Type::Struct(n, args) => Type::Struct(n.clone(), args.iter().map(|a| subst_named(a, sigma)).collect()),
        Type::Enum(n, args) => Type::Enum(n.clone(), args.iter().map(|a| subst_named(a, sigma)).collect()),
        other => other.clone(),
    }
}

/// M40.2c: aplica `subst_named` a TODAS las anotaciones de tipo del cuerpo de un método (tipos de
/// `let`, firmas de closures, casts), recursivamente. Necesario porque el cuerpo de un método por
/// defecto genérico puede anotar un parámetro del trait —`filter` escribe `Option<T>`—, y sobre un
/// impl concreto (`impl Iterator<int>`) ese `T` debe volverse `int` (no queda en `type_params`).
fn subst_named_block(block: &mut Block, sigma: &HashMap<String, Type>) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { ty, value, .. } => {
                if let Some(t) = ty { *t = subst_named(t, sigma); }
                subst_named_expr(value, sigma);
            }
            StmtKind::LetTuple { value, .. } => subst_named_expr(value, sigma),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { subst_named_expr(start, sigma); subst_named_expr(end, sigma); }
                    ForIter::In(e) => subst_named_expr(e, sigma),
                    ForIter::Iter { expr, .. } => subst_named_expr(expr, sigma),
                }
                subst_named_block(body, sigma);
            }
            StmtKind::Assign { target, value } => { subst_named_expr(target, sigma); subst_named_expr(value, sigma); }
            StmtKind::Return { value } => { if let Some(v) = value { subst_named_expr(v, sigma); } }
            StmtKind::Expr(e) => subst_named_expr(e, sigma),
        }
    }
    if let Some(t) = &mut block.tail { subst_named_expr(t, sigma); }
}

fn subst_named_expr(expr: &mut Expr, sigma: &HashMap<String, Type>) {
    match &mut expr.kind {
        ExprKind::Cast { expr: inner, ty } => { subst_named_expr(inner, sigma); *ty = subst_named(ty, sigma); }
        ExprKind::Unary { expr: inner, .. } | ExprKind::Try(inner) => subst_named_expr(inner, sigma),
        ExprKind::Binary { left, right, .. } => { subst_named_expr(left, sigma); subst_named_expr(right, sigma); }
        ExprKind::Call { callee, args } => { subst_named_expr(callee, sigma); for a in args { subst_named_expr(a, sigma); } }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => { for e in elems { subst_named_expr(e, sigma); } }
        ExprKind::MapLit(pares) => { for (k, v) in pares { subst_named_expr(k, sigma); subst_named_expr(v, sigma); } }
        ExprKind::Index { array, index } => { subst_named_expr(array, sigma); subst_named_expr(index, sigma); }
        ExprKind::StructLit { fields, .. } => { for (_, e) in fields { subst_named_expr(e, sigma); } }
        ExprKind::EnumLit { args, .. } => { for a in args { subst_named_expr(a, sigma); } }
        ExprKind::Field { object, .. } => subst_named_expr(object, sigma),
        ExprKind::Func(fe) => {
            for p in &mut fe.params { p.ty = subst_named(&p.ty, sigma); }
            fe.return_type = subst_named(&fe.return_type, sigma);
            subst_named_block(&mut fe.body, sigma);
        }
        ExprKind::Match { scrutinee, arms } => {
            subst_named_expr(scrutinee, sigma);
            for arm in arms {
                subst_named_expr(&mut arm.body, sigma);
                if let Some(g) = &mut arm.guard { subst_named_expr(g, sigma); }
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            subst_named_expr(cond, sigma);
            subst_named_block(then_branch, sigma);
            if let Some(e) = else_branch { subst_named_expr(e, sigma); }
        }
        ExprKind::While { cond, body } => { subst_named_expr(cond, sigma); subst_named_block(body, sigma); }
        ExprKind::Block(b) => subst_named_block(b, sigma),
        _ => {}
    }
}

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
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| subst_self(t, target)).collect()),
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
                    ForIter::Iter { expr, .. } => lower_ufcs_expr(expr, sites),
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
                crate::ice!("the site guard guarantees a Call with a Field callee");
            }
        } else {
            crate::ice!("the site guard guarantees a Call");
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
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { lower_ufcs_expr(k, sites); lower_ufcs_expr(v, sites); }
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
                lower_ufcs_expr(&mut arm.body, sites); if let Some(g) = &mut arm.guard { lower_ufcs_expr(g, sites); }
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
                    ForIter::Iter { expr, .. } => lower_uintlit_expr(expr, sites),
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
        ExprKind::MapLit(pares) => { for (k, v) in pares { lower_uintlit_expr(k, sites); lower_uintlit_expr(v, sites); } }
        ExprKind::Index { array, index } => { lower_uintlit_expr(array, sites); lower_uintlit_expr(index, sites); }
        ExprKind::StructLit { fields, .. } => { for (_, e) in fields { lower_uintlit_expr(e, sites); } }
        ExprKind::EnumLit { args, .. } => { for a in args { lower_uintlit_expr(a, sites); } }
        ExprKind::Field { object, .. } => lower_uintlit_expr(object, sites),
        ExprKind::Func(fe) => lower_uintlit_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_uintlit_expr(scrutinee, sites);
            for arm in arms { lower_uintlit_expr(&mut arm.body, sites); if let Some(g) = &mut arm.guard { lower_uintlit_expr(g, sites); } }
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
                    ForIter::Iter { expr, .. } => lower_try_expr(expr, sites),
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
            _ => crate::ice!("the guard guarantees a Try"),
        };
        let mk = |kind| Expr { kind, line: l, col: c };
        // Rama Ok: `Result.Ok($to) => $to`.
        let arm_ok = MatchArm {
            pattern: Pattern {
                kind: PatternKind::Variant { enum_name: "Result".into(), variant: "Ok".into(), subpatterns: vec![Pattern { kind: PatternKind::Binding("$to".into()), line: l, col: c }] },
                line: l, col: c,
            },
            guard: None,
            body: mk(ExprKind::Ident("$to".into())),
            line: l, col: c,
        };
        // Rama Err: `Result.Err($te) => { return Result.Err(<from>($te)); }`.
        let converted = mk(ExprKind::Call {
            callee: Box::new(mk(ExprKind::Ident(mangled))),
            args: vec![mk(ExprKind::Ident("$te".into()))],
        });
        let err_val = mk(ExprKind::EnumLit { enum_name: "Result".into(), variant: "Err".into(), args: vec![converted] });
        let ret_stmt = Stmt { kind: StmtKind::Return { value: Some(err_val) }, line: l, col: c };
        let arm_err = MatchArm {
            pattern: Pattern {
                kind: PatternKind::Variant { enum_name: "Result".into(), variant: "Err".into(), subpatterns: vec![Pattern { kind: PatternKind::Binding("$te".into()), line: l, col: c }] },
                line: l, col: c,
            },
            guard: None,
            body: mk(ExprKind::Block(Block { statements: vec![ret_stmt], tail: None, line: l, col: c, end_line: l })),
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
        ExprKind::MapLit(pares) => { for (k, v) in pares { lower_try_expr(k, sites); lower_try_expr(v, sites); } }
        ExprKind::Index { array, index } => { lower_try_expr(array, sites); lower_try_expr(index, sites); }
        ExprKind::StructLit { fields, .. } => { for (_, e) in fields { lower_try_expr(e, sites); } }
        ExprKind::EnumLit { args, .. } => { for a in args { lower_try_expr(a, sites); } }
        ExprKind::Field { object, .. } => lower_try_expr(object, sites),
        ExprKind::Func(fe) => lower_try_block(&mut fe.body, sites),
        ExprKind::Match { scrutinee, arms } => {
            lower_try_expr(scrutinee, sites);
            for arm in arms { lower_try_expr(&mut arm.body, sites); if let Some(g) = &mut arm.guard { lower_try_expr(g, sites); } }
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
                    ForIter::Iter { expr, .. } => lower_operators_expr(expr, sites),
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
            _ => crate::ice!("the site guard guarantees Binary or Unary Neg"),
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
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { lower_operators_expr(k, sites); lower_operators_expr(v, sites); }
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
                lower_operators_expr(&mut arm.body, sites); if let Some(g) = &mut arm.guard { lower_operators_expr(g, sites); }
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
                    ForIter::Iter { expr, .. } => lower_dict_calls_expr(expr, sites),
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
        ExprKind::MapLit(pares) => {
            for (k, v) in pares { lower_dict_calls_expr(k, sites); lower_dict_calls_expr(v, sites); }
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
                lower_dict_calls_expr(&mut arm.body, sites); if let Some(g) = &mut arm.guard { lower_dict_calls_expr(g, sites); }
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
                    ForIter::Iter { expr, .. } => lower_dyn_expr(expr, coercions, dispatch, upcasts, tm, counter),
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
        ExprKind::MapLit(pares) => {
            for (k, v) in pares {
                lower_dyn_expr(k, coercions, dispatch, upcasts, tm, counter);
                lower_dyn_expr(v, coercions, dispatch, upcasts, tm, counter);
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
                lower_dyn_expr(&mut arm.body, coercions, dispatch, upcasts, tm, counter); if let Some(g) = &mut arm.guard { lower_dyn_expr(g, coercions, dispatch, upcasts, tm, counter); }
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
        let ExprKind::Call { callee, mut args } = taken else { crate::ice!("a dispatch site is a Call") };
        let ExprKind::Field { object, name } = callee.kind else { crate::ice!("the callee of a dispatch is a Field") };
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
        expr.kind = ExprKind::Block(Block { statements: vec![let_stmt], tail: Some(Box::new(call)), line, col, end_line: line });
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
        expr.kind = ExprKind::Block(Block { statements: vec![let_stmt], tail: Some(Box::new(lit)), line, col, end_line: line });
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

    /// M45: etiquetas de `member_completion` sobre una fuente que YA lleva el centinela.
    fn members(src: &str) -> Vec<String> {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let (mut prog, _) = crate::parser::parse_all(tokens);
        member_completion(&mut prog).into_iter().map(|m| m.label).collect()
    }

    /// M52: recolecta los nombres de callee (Ident) de todas las llamadas del programa verificado.
    fn call_targets(src: &str) -> Vec<String> {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        check(&mut prog).expect("check ok");
        fn walk_block(b: &Block, acc: &mut Vec<String>) {
            for s in &b.statements {
                match &s.kind {
                    StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } | StmtKind::Expr(value) => {
                        walk_expr(value, acc)
                    }
                    StmtKind::Assign { target, value } => {
                        walk_expr(target, acc);
                        walk_expr(value, acc);
                    }
                    StmtKind::Return { value: Some(v) } => walk_expr(v, acc),
                    StmtKind::For { body, .. } => walk_block(body, acc),
                    _ => {}
                }
            }
            if let Some(t) = &b.tail {
                walk_expr(t, acc);
            }
        }
        fn walk_expr(e: &Expr, acc: &mut Vec<String>) {
            match &e.kind {
                ExprKind::Call { callee, args } => {
                    if let ExprKind::Ident(n) = &callee.kind {
                        acc.push(n.clone());
                    }
                    walk_expr(callee, acc);
                    for a in args {
                        walk_expr(a, acc);
                    }
                }
                ExprKind::Binary { left, right, .. } => {
                    walk_expr(left, acc);
                    walk_expr(right, acc);
                }
                ExprKind::While { cond, body } => {
                    walk_expr(cond, acc);
                    walk_block(body, acc);
                }
                ExprKind::Block(b) => walk_block(b, acc),
                ExprKind::If { cond, then_branch, else_branch } => {
                    walk_expr(cond, acc);
                    walk_block(then_branch, acc);
                    if let Some(e2) = else_branch {
                        walk_expr(e2, acc);
                    }
                }
                _ => {}
            }
        }
        // Solo `main`: el prelude contiene los cuerpos forwarder (que llaman `__x` legítimamente).
        let mut acc = Vec::new();
        for f in prog.functions.iter().filter(|f| f.name == "main") {
            walk_block(&f.body, &mut acc);
        }
        acc
    }

    #[test]
    fn inline_forwarders_baja_push_y_len_al_builtin() {
        // M52: `a.push(i)` / `a.len()` (métodos de trait forwarder de M48.4) deben quedar
        // reescritos a la llamada al builtin (`__push`/`__len`), no al método manglado.
        let targets =
            call_targets("fn main() -> int {\n  var a: [int] = [];\n  a.push(1);\n  a.len()\n}");
        assert!(targets.iter().any(|t| t == "__push"), "push inlineado: {targets:?}");
        assert!(targets.iter().any(|t| t == "__len"), "len inlineado: {targets:?}");
        assert!(!targets.iter().any(|t| t.ends_with("#push") || t.ends_with("#len")),
            "sin calls al forwarder: {targets:?}");
    }

    #[test]
    fn inline_forwarders_respects_un_local_homonimo() {
        // M52 (guarda de sonoridad): si el programa liga una variable `__push`, el inlining hacia
        // ese nombre se desactiva (el compilador resuelve local antes que builtin) y la llamada
        // sigue yendo al método manglado. `__len` no está ligado → sí se inlinea.
        let targets = call_targets(
            "fn main() -> int {\n  let __push = 5;\n  var a: [int] = [];\n  a.push(__push);\n  a.len()\n}",
        );
        assert!(!targets.iter().any(|t| t == "__push"), "push NO inlineado: {targets:?}");
        assert!(targets.iter().any(|t| t.ends_with("#push")), "va al forwarder: {targets:?}");
        assert!(targets.iter().any(|t| t == "__len"), "len sí inlineado: {targets:?}");
    }

    #[test]
    fn hover_de_function_associated() {
        // M48.1/LSP: hover sobre el nombre asociado (`new`/`bounded`) → su firma del registro.
        let src = "fn main() -> int {\n  let m: Map<string, int> = Map.new();\n  m.len()\n}";
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        let idx = semantic_index(&mut prog);
        assert!(idx.hovers.iter().any(|h| h.line == 2 && h.text == "Map.new() -> Map<K, V>"),
            "hover de Map.new: {:?}", idx.hovers.iter().filter(|h| h.line == 2).map(|h| &h.text).collect::<Vec<_>>());
    }

    #[test]
    fn trait_len_para_types_incorporados() {
        // M48.4a: el trait `Len` (prelude) se implementa para string/[T]/Map/bytes → `.len()` despacha
        // por trait; funciona con un bound `T: Len` y con un tipo de usuario que lo implemente.
        assert!(check_src("fn main() -> int { \"hello\".len() + [1,2,3].len() + \"a\".to_bytes().len() }").is_ok());
        assert!(check_src("fn f<T: Len>(x: T) -> int { x.len() }\nfn main() -> int { f([1,2]) + f(\"ab\") }").is_ok());
        assert!(check_src("fn main() -> int { let m: Map<int, int> = [1: 2]; m.len() }").is_ok());
        // Un tipo del usuario puede implementar Len y usarse con el bound.
        assert!(check_src(
            "struct P { d: [int] }\nimpl Len for P { fn len(self) -> int { self.d.len() } }\n\
             fn f<T: Len>(x: T) -> int { x.len() }\nfn main() -> int { f(P { d: [1,2,3] }) }").is_ok());
        // Un tipo SIN Len no satisface el bound.
        let e = check_src("struct Q { x: int }\nfn f<T: Len>(x: T) -> int { x.len() }\nfn main() -> int { f(Q { x: 1 }) }").unwrap_err();
        assert!(format!("{e}").contains("Len"), "Q no implementa Len: {e}");
    }

    #[test]
    fn traits_strops_bytesops() {
        // M48.4d: los métodos de string/bytes despachan por trait (StrOps/BytesOps).
        assert!(check_src(
            "fn main() -> int { let s = \"hello\"; \
             s.trim().split(\",\").len() + s.to_upper().len() + s.substring(0, 2).len() \
             + s.to_bytes().sub_bytes(0, 1).len() + s.chars().len() }").is_ok());
        // to_upper sobre un no-string → error.
        let e = check_src("fn main() -> int { (42).to_upper().len() }").unwrap_err();
        assert!(!format!("{e}").is_empty(), "int no has to_upper: {e}");
    }

    #[test]
    fn trait_mapops() {
        // M48.4c: insert/contains_key/keys/values como métodos del trait MapOps.
        assert!(check_src(
            "fn main() -> int { let m: Map<int, int> = [1: 10]; m.insert(2, 20); \
             m.keys().len() + m.values()[0] + (if (m.contains_key(1)) { 1 } else { 0 }) }").is_ok());
        // clave del tipo equivocado → error.
        let e = check_src("fn main() { let m: Map<int, int> = [:]; m.insert(\"x\", 1); }").unwrap_err();
        assert!(format!("{e}").contains("clave") || format!("{e}").contains("int"), "{e}");
    }

    #[test]
    fn traits_push_reverse_contains() {
        // M48.4b: los tres traits despachan como método; extensibles a tipos de usuario.
        assert!(check_src("fn main() -> int { var a = [1,2]; a.push(3); a.reverse().len() }").is_ok());
        assert!(check_src("fn ok(b: bool) -> int { if (b) { 1 } else { 0 } }\nfn main() -> int { ok([1,2,3].contains(2)) + ok(\"hello\".contains(\"la\")) }").is_ok());
        // Un tipo del usuario implementa Push<int>/Contains<int>.
        assert!(check_src(
            "struct C { d: [int] }\nimpl Push<int> for C { fn push(self, x: int) { self.d.push(x) } }\n\
             fn main() { let c = C { d: [] }; c.push(5); }").is_ok());
        // contains con el tipo de elemento equivocado → error.
        let e = check_src("fn main() -> int { if ([1,2,3].contains(\"x\")) { 1 } else { 0 } }").unwrap_err();
        assert!(format!("{e}").contains("string") || format!("{e}").contains("contains"), "{e}");
    }

    #[test]
    fn impl_para_ty_incorporado_must_ser_generic() {
        // M48.4a: `impl X for [int]` (no plenamente genérico) se rechaza, como `impl X for Caja<int>`.
        let e = check_src("trait X { fn m(self) -> int; }\nimpl X for [int] { fn m(self) -> int { 0 } }\nfn main() -> int { 0 }").unwrap_err();
        assert!(format!("{e}").contains("distinct type parameters") || format!("{e}").contains("expects 1 type parameter"), "{e}");
    }

    #[test]
    fn redefine_builtin_es_error() {
        // M48.3: un builtin del núcleo (print/to_string/panic…) no puede redefinirse como función libre.
        for name in ["print", "to_string", "panic"] {
            let src = format!("fn {name}(x: int) -> int {{ x }}\nfn main() -> int {{ 0 }}");
            let e = check_src(&src).unwrap_err();
            assert!(format!("{e}").contains(&format!("'{name}' is a language builtin")),
                "redefine {name}: {e}");
        }
        // M48.4e: los builtins de contenedor RETIRADOS (len/push/… → ahora métodos de trait) dejaron el
        // namespace libre → una función libre con ese nombre YA es legal (el footgun no dispara).
        for name in ["len", "push", "insert", "keys", "reverse", "contains", "split", "chars"] {
            let src = format!("fn {name}(x: int) -> int {{ x }}\nfn main() -> int {{ {name}(1) }}");
            assert!(check_src(&src).is_ok(), "'{name}' as a free function must now compile");
        }
        // Una función del PRELUDE (map/filter/fold/sort) SÍ puede redefinirse (override).
        assert!(check_src("fn map(x: int) -> int { x + 1 }\nfn main() -> int { map(5) }").is_ok());
        assert!(check_src("fn sort(x: int) -> int { x }\nfn main() -> int { sort(3) }").is_ok());
        // Y un nombre normal, obviamente, es válido.
        assert!(check_src("fn fold(x: int) -> int { x * 2 }\nfn main() -> int { fold(2) }").is_ok());
    }

    #[test]
    fn literal_de_map() {
        // M48.2: `[k: v]` infiere `Map<K,V>`; `[:]` lo fija el esperado.
        assert!(check_src("fn main() -> int { let m = [1: \"a\", 2: \"b\"]; m.len() }").is_ok());
        assert!(check_src("fn main() { let m: Map<string, int> = [:]; }").is_ok());
        assert!(check_src("fn main() { let m: Map<int, string> = [1: \"a\"]; }").is_ok());
        // `[:]` sin anotar → error de "anota el tipo".
        let e = check_src("fn main() -> int { let m = [:]; 0 }").unwrap_err();
        assert!(format!("{e}").contains("cannot infer the type of [:]"), "{e}");
        // Claves/valores heterogéneos → error.
        let k = check_src("fn main() -> int { let m = [1: \"a\", \"b\": \"c\"]; 0 }").unwrap_err();
        assert!(format!("{k}").contains("the Map keys must be of the same type"), "{k}");
        let v = check_src("fn main() -> int { let m = [1: \"a\", 2: 3]; 0 }").unwrap_err();
        assert!(format!("{v}").contains("the Map values must be of the same type"), "{v}");
        // Clave no hashable (float) → error.
        let f = check_src("fn main() -> int { let m = [1.5: \"a\"]; 0 }").unwrap_err();
        assert!(format!("{f}").contains("Map key"), "{f}");
        // Contra un esperado que no es Map → error de tipos del `let`.
        assert!(check_src("fn main() { let xs: [int] = [1: 2]; }").is_err());
    }

    #[test]
    fn functions_asociadas_de_ty() {
        // M48.1: `Map.new()`/`Channel.new()`/`Channel.bounded(n)` — el tipo lo fija el esperado.
        assert!(check_src("fn main() -> int { let m: Map<string, int> = Map.new(); m.len() }").is_ok());
        assert!(check_src("fn main() { let c: Channel<int> = Channel.new(); }").is_ok());
        assert!(check_src("fn main() { let c: Channel<int> = Channel.bounded(2); }").is_ok());
        // Sin tipo esperado → error de "anota el tipo".
        let e = check_src("fn main() -> int { let m = Map.new(); 0 }").unwrap_err();
        assert!(format!("{e}").contains("cannot infer the type of 'Map.new'"), "{e}");
        // Aridad: `Map.new` no recibe argumentos; `Channel.bounded` exige uno.
        let a = check_src("fn main() { let m: Map<int, int> = Map.new(1); }").unwrap_err();
        assert!(format!("{a}").contains("expects 0 argument(s)"), "{a}");
        // El argumento de `bounded` debe ser int.
        let b = check_src("fn main() { let c: Channel<int> = Channel.bounded(\"x\"); }").unwrap_err();
        assert!(format!("{b}").contains("must be int"), "{b}");
        // El tipo esperado debe casar la familia (Map.new no produce un Channel).
        let f = check_src("fn main() { let c: Channel<int> = Map.new(); }").unwrap_err();
        assert!(format!("{f}").contains("produces a Map"), "{f}");
    }

    #[test]
    fn member_completion_fields_methods_y_builtins() {
        // Struct: campos + método de trait + UFCS del usuario; kinds correctos.
        let src = "struct P { x: int, y: int }\ntrait Ver { fn see(self) -> int; }\nimpl Ver for P { fn see(self) -> int { self.x } }\nfn fold(p: P) -> int { p.x }\nfn main() -> int { let p = P { x: 1, y: 2 }; p.__raycomplete__; 0 }\n";
        let m = members(src);
        for expected in ["x", "y", "see", "fold"] {
            assert!(m.contains(&expected.to_string()), "falta '{expected}': {m:?}");
        }
        // string: builtins de string, sin funciones de E/S sobre una ruta string.
        let s = members("fn main() -> int { let s = \"h\"; s.__raycomplete__; 0 }");
        assert!(s.contains(&"trim".to_string()) && s.contains(&"split".to_string()), "{s:?}");
        assert!(!s.contains(&"read_file".to_string()), "sin E/S about string: {s:?}");
        // array: builtins + orden superior del prelude.
        let a = members("fn main() -> int { let xs = [1,2,3]; xs.__raycomplete__; 0 }");
        for expected in ["len", "push", "map", "filter", "fold", "sort"] {
            assert!(a.contains(&expected.to_string()), "array falta '{expected}': {a:?}");
        }
        // receptor sin tipo conocido → sin miembros (sin pánico).
        assert!(members("fn main() -> int { unknown.__raycomplete__; 0 }").is_empty());
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
            "a tuple position is not assignable",
        );
        // La lectura y la desestructuración siguen funcionando.
        check_src("fn main() -> int { let t = (1, 2); let (a, b) = t; a + b + t.0 }")
            .expect("leer y desestructurar es válido");
    }

        #[test]
    fn check_all_acumula_por_function() {
        // M33c: un error por cuerpo, todos reportados; el primero idéntico al fail-fast.
        let src = "fn f() -> int { 1 + true }\nfn g() -> int { \"x\" * 2 }\nfn main() -> int { f() + g() }";
        let toks = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(toks).expect("parse ok");
        let mut prog2 = prog.clone();
        let errs = check_all(&mut prog2);
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs[0].msg.contains("int and bool"), "{}", errs[0].msg);
        assert!(errs[1].msg.contains("string and int"), "{}", errs[1].msg);
        let solo = check(&mut prog).unwrap_err();
        assert_eq!(errs[0], solo, "el primer error must ser byte-idéntico (oráculos)");
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
        assert!(errs[0].msg.contains("declared twice"), "{}", errs[0].msg);
        // Y un tipo desconocido en una FIRMA se valida en la fase de cuerpos → acumula
        // junto a los demás (mejor: más errores de una tacada).
        let src = "fn f(a: NoExiste) -> int { 0 }\nfn g() -> int { 1 + true }\nfn main() -> int { 0 }";
        let toks = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(toks).expect("parse ok");
        let errs = check_all(&mut prog);
        assert_eq!(errs.len(), 2, "{errs:?}");
    }

        #[test]
    fn el_error_de_types_underscores_la_expression_complete() {
        // M33a-2: la extensión del error sale de la tabla de spans del parser.
        let e = check_src("fn main() -> int { let x = 1 + true; x }").unwrap_err();
        assert!(e.msg.contains("requires both operands"), "{}", e.msg);
        assert_eq!(e.len, "1 + true".chars().count(), "underscores la expresión entera");
        // Un argumento de tipo equivocado subraya el argumento (pasa por expression()).
        let e = check_src("fn f(a: int) -> int { a }\nfn main() -> int { f(\"dos\") }").unwrap_err();
        assert!(e.msg.contains("expected int"), "{}", e.msg);
        assert_eq!(e.len, "\"dos\"".chars().count());
    }

    // M9.4: bounds en parámetros de tipo de struct/enum (verificados en la construcción).
    const BOUND_PRELUDE: &str = r#"
trait Show2 { fn see(self) -> string; }
struct P { n: int }
impl Show2 for P { fn see(self) -> string { "P" } }
struct Q { n: int }
"#;

    #[test]
    fn bound_struct_ok_con_impl() {
        let src = format!("{}struct Caja<T: Show2> {{ v: T }}\nfn main() -> int {{ let c = Caja {{ v: P {{ n: 1 }} }}; c.v.see(); 0 }}\n", BOUND_PRELUDE);
        check_src(&src).expect("P implementa Show2");
    }

    #[test]
    fn bound_struct_fails_sin_impl() {
        let src = format!("{}struct Caja<T: Show2> {{ v: T }}\nfn main() -> int {{ let c = Caja {{ v: Q {{ n: 1 }} }}; 0 }}\n", BOUND_PRELUDE);
        err_contains(&src, "requires that 'T' be 'Show2'");
    }

    #[test]
    fn bound_struct_propagates_a_function_generic() {
        // Construir Caja<U> exige que U lleve el bound: sin él, error; con él, OK.
        let bad = format!("{}struct Caja<T: Show2> {{ v: T }}\nfn env<U>(x: U) -> Caja<U> {{ Caja {{ v: x }} }}\nfn main() -> int {{ 0 }}\n", BOUND_PRELUDE);
        err_contains(&bad, "requires that 'T' be 'Show2'");
        let good = format!("{}struct Caja<T: Show2> {{ v: T }}\nfn env<U: Show2>(x: U) -> Caja<U> {{ Caja {{ v: x }} }}\nfn main() -> int {{ 0 }}\n", BOUND_PRELUDE);
        check_src(&good).expect("con U: Show2 la propagación se satisface");
    }

    #[test]
    fn bound_enum_fails_sin_impl() {
        let src = format!("{}enum Opt<T: Show2> {{ Nada, Algo(T) }}\nfn main() -> int {{ let x = Opt.Algo(Q {{ n: 1 }}); 0 }}\n", BOUND_PRELUDE);
        err_contains(&src, "requires that 'T' be 'Show2'");
    }

    #[test]
    fn bound_struct_trait_nonexistent_es_error() {
        err_contains("struct Caja<T: NoExiste> { v: T }\nfn main() -> int { 0 }\n", "trait 'NoExiste' not declared");
    }

    #[test]
    fn fib_es_valid() {
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
    fn arithmetic_mezclada_fails() {
        err_contains("fn main() -> int { 1 + true }", "requires both operands");
        err_contains("fn main() { let x: float = 1 + 2.0; }", "requires both operands");
    }

    #[test]
    fn condicion_must_ser_bool() {
        err_contains("fn main() { if (1) { } }", "if condition must be bool");
        err_contains("fn main() { while (1) { } }", "while condition must be bool");
    }

    #[test]
    fn ramas_del_if_same_ty() {
        err_contains(
            "fn main() -> int { if (true) { 1 } else { true } }",
            "if branches have different types",
        );
    }

    #[test]
    fn if_sin_else_must_ser_unit() {
        err_contains("fn main() { if (true) { 5 } }", "without else has type unit");
    }

    #[test]
    fn asignar_a_let_fails_pero_a_var_ok() {
        err_contains(
            "fn main() { let x: int = 0; x = 1; }",
            "it is immutable",
        );
        assert!(check_src("fn main() { var x: int = 0; x = 1; }").is_ok());
    }

    #[test]
    fn variable_no_declarada() {
        err_contains("fn main() -> int { x }", "not declared");
        err_contains("fn main() { y = 1; }", "not declared");
    }

    #[test]
    fn ty_de_declaracion_must_coincidir() {
        err_contains("fn main() { let x: int = true; }", "initialized with bool");
    }

    #[test]
    fn return_val_incorrect() {
        err_contains("fn f() -> int { true } fn main() {}", "produces bool");
        err_contains("fn g() -> int { return true; } fn main() {}", "returning bool");
    }

    #[test]
    fn return_val_early_sin_valor_final_es_valid() {
        // Gracias al análisis de divergencia, esto es válido aunque no tenga
        // expresión final: todos los caminos retornan.
        let src = r#"
fn sign(x: int) -> int {
    if (x < 0) { return -1; } else { return 1; }
}
fn main() -> int { sign(3) }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn calls_validan_arity_y_types() {
        err_contains(
            "fn add(a: int, b: int) -> int { a + b } fn main() -> int { add(1) }",
            "expects 2 argument",
        );
        err_contains(
            "fn add(a: int, b: int) -> int { a + b } fn main() -> int { add(1, true) }",
            "expected int, got bool",
        );
        err_contains("fn main() -> int { desconocida() }", "not declared");
    }

    #[test]
    fn print_builtin() {
        assert!(check_src("fn main() { print(42); print(\"hello\"); print(true); }").is_ok());
        err_contains("fn main() { print(); }", "expects 1 argument");
        err_contains("fn main() { print(1, 2); }", "expects 1 argument");
    }

    #[test]
    fn main_obligatoria_y_bien_formada() {
        err_contains("fn other() -> int { 0 }", "missing entry function 'main'");
        err_contains("fn main(x: int) -> int { x }", "must not take parameters");
        err_contains("fn main() -> bool { true }", "must return int or unit");
    }

    #[test]
    fn shadowing_en_block_internal() {
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
    fn function_no_declarada_dos_veces() {
        err_contains("fn f() {} fn f() {} fn main() {}", "declared twice");
    }

    // ----- M3.1: arreglos -----

    #[test]
    fn arrays_valid_vals() {
        assert!(check_src("fn main() -> int { let a: [int] = [1, 2, 3]; a[0] }").is_ok());
        assert!(check_src("fn main() -> int { let a: [int] = []; a.push(1); a.len() }").is_ok());
        assert!(check_src("fn main() { var a: [int] = [1]; a[0] = 9; }").is_ok());
        // Arreglos anidados.
        assert!(check_src("fn main() -> int { let m: [[int]] = [[1, 2], [3, 4]]; m[1][0] }").is_ok());
    }

    #[test]
    fn arrays_errors_de_ty() {
        err_contains("fn main() -> int { let a: [int] = [1, true]; a[0] }", "must be int");
        err_contains("fn main() -> int { let a: [int] = [1]; a[true] }", "index must be int");
        err_contains("fn main() -> int { let x: int = 5; x[0] }", "not an array");
        err_contains("fn main() { let x: int = []; }", "cannot infer");
        err_contains("fn main() -> int { let a: [int] = [1]; a[0] = true; a[0] }", "is assigned bool");
        err_contains("fn main() -> int { 5.len() }", "no field or function 'len' applicable to int");
        err_contains("fn main() { let a: [int] = [1]; a.push(true); }", "'T' cannot be int and bool at the same time");
    }

    // ----- M3.2: structs -----

    #[test]
    fn structs_valid_vals() {
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
    fn structs_errors() {
        err_contains("fn main() { let p: Foo = Foo { x: 1 }; }", "not declared");
        err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: true }; p.x }", "expected int");
        err_contains("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 1 }; p.x }", "missing field");
        err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: 1, z: 2 }; p.x }", "has no field");
        err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: 1 }; p.y }", "has no field");
        err_contains("struct P { x: int } fn main() -> int { let n: int = 5; n.x }", "not a struct");
        err_contains("struct P {} struct P {} fn main() {}", "declared twice");
    }

    // ----- M4.1: funciones de primera clase -----

    #[test]
    fn functions_first_class_validas() {
        // Anónima en variable, con su tipo función.
        assert!(check_src("fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x + 1 }; f(2) }").is_ok());
        // De orden superior: recibe y aplica una función.
        assert!(check_src(
            "fn apply(f: fn(int) -> int, x: int) -> int { f(x) }
             fn main() -> int { apply(fn(n: int) -> int { n * n }, 3) }"
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
    fn functions_first_class_errors() {
        // Tipo de la anónima no coincide con la anotación.
        err_contains(
            "fn main() { let f: fn(int) -> int = fn(x: bool) -> int { 0 }; }",
            "initialized with",
        );
        // Aridad incorrecta en una llamada indirecta.
        err_contains(
            "fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x }; f(1, 2) }",
            "expects 1 argument",
        );
        // Tipo de argumento incorrecto en una llamada indirecta.
        err_contains(
            "fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x }; f(true) }",
            "expected int, got bool",
        );
        // Llamar a algo que no es función.
        err_contains("fn main() -> int { let x: int = 3; x(1) }", "not a function");
        // El cuerpo de la anónima no respeta su tipo de retorno.
        err_contains(
            "fn main() { let f: fn() -> int = fn() -> int { true }; }",
            "produces bool",
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
            "fn counter() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
             fn main() -> int { let c: fn() -> int = counter(); c() }"
        ).is_ok());
        // Captura transitiva (dos niveles).
        assert!(check_src(
            "fn adder(x: int) -> fn(int) -> int { fn(y: int) -> int { x + y } }
             fn main() -> int { let add5: fn(int) -> int = adder(5); add5(10) }"
        ).is_ok());
    }

    #[test]
    fn closure_no_can_reasignar_un_let_capturado() {
        // Capturar no reata: asignar a un `let` externo sigue siendo error.
        err_contains(
            "fn main() { let b: int = 1; let f: fn() = fn() { b = 2; }; f() }",
            "it is immutable",
        );
    }

    #[test]
    fn functions_no_son_comparables() {
        err_contains(
            "fn inc(n: int) -> int { n } fn main() -> int { if (inc == inc) { 1 } else { 0 } }",
            "same comparable type",
        );
    }

    // ----- M5.1: enums (tipos suma) y construcción -----

    #[test]
    fn enum_construccion_validates() {
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
    fn enum_recursive_es_valid() {
        // Un enum puede portar su propio tipo: el norte de M5 (listas, árboles).
        let src = r#"
enum Lista { Cons(int, Lista), Nil }
fn main() { let xs: Lista = Lista.Cons(1, Lista.Cons(2, Lista.Nil)); print(xs); }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn enum_variant_nonexistent() {
        err_contains("enum E { A, B } fn main() { let x: E = E.C; print(x); }", "has no variant 'C'");
    }

    #[test]
    fn enum_arity_incorrect() {
        err_contains("enum E { A(int) } fn main() { let x: E = E.A(1, 2); print(x); }", "expects 1 argument");
    }

    #[test]
    fn enum_ty_de_payload_incorrect() {
        err_contains("enum E { A(int) } fn main() { let x: E = E.A(true); print(x); }", "expected int, got bool");
    }

    #[test]
    fn enum_no_es_comparable() {
        err_contains(
            "enum E { A, B } fn main() -> int { let x: E = E.A; if (x == E.B) { 1 } else { 0 } }",
            "same comparable type",
        );
    }

    #[test]
    fn enum_y_struct_no_comparten_name() {
        err_contains("enum E { A } struct E { x: int } fn main() {}", "cannot also be a struct");
    }

    #[test]
    fn enum_variant_repetida() {
        err_contains("enum E { A, A } fn main() {}", "variant 'A' repeated");
    }

    #[test]
    fn enum_declarado_dos_veces() {
        err_contains("enum E { A } enum E { B } fn main() {}", "declared twice");
    }

    #[test]
    fn enum_como_ty_unknown() {
        // Anotar con un nombre que no es ni struct ni enum.
        err_contains("fn main() { let x: NoExiste = 1; print(x); }", "not declared");
    }

    // ----- M5.2: match y exhaustividad -----

    #[test]
    fn match_exhaustive_es_valid() {
        let src = r#"
enum Lista { Cons(int, Lista), Nil }
fn sum(xs: Lista) -> int {
    match (xs) {
        Lista.Cons(h, t) => h + sum(t),
        Lista.Nil => 0,
    }
}
fn main() -> int { sum(Lista.Cons(1, Lista.Nil)) }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn match_con_comodin_es_exhaustive() {
        let src = "enum E { A, B, C } fn f(e: E) -> int { match (e) { E.A => 1, _ => 0 } } fn main() {}";
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn match_no_exhaustive() {
        err_contains(
            "enum E { A, B, C } fn f(e: E) -> int { match (e) { E.A => 1, E.B => 2 } } fn main() {}",
            "non-exhaustive",
        );
    }

    #[test]
    fn match_brazos_de_types_distintos() {
        err_contains(
            "enum E { A, B } fn f(e: E) -> int { match (e) { E.A => 1, E.B => true } } fn main() {}",
            "different types",
        );
    }

    #[test]
    fn match_variant_repetida() {
        err_contains(
            "enum E { A, B } fn f(e: E) -> int { match (e) { E.A => 1, E.A => 2, E.B => 3 } } fn main() {}",
            "is already covered",
        );
    }

    #[test]
    fn match_branch_inalcanzable_after_catchall() {
        err_contains(
            "enum E { A, B } fn f(e: E) -> int { match (e) { other => 0, E.A => 1 } } fn main() {}",
            "unreachable",
        );
    }

    #[test]
    fn match_arity_de_binding_incorrect() {
        err_contains(
            "enum E { A(int) } fn f(e: E) -> int { match (e) { E.A => 1 } } fn main() {}",
            "binds 0 value(s), but the variant has 1",
        );
    }

    #[test]
    fn match_about_no_enum() {
        err_contains(
            "fn f(n: int) -> int { match (n) { _ => 0 } } fn main() {}",
            "match requires an enum",
        );
    }

    #[test]
    fn match_patron_de_other_enum() {
        err_contains(
            "enum E { A } enum F { B } fn f(e: E) -> int { match (e) { F.B => 1, _ => 0 } } fn main() {}",
            "is of enum 'F'",
        );
    }

    #[test]
    fn match_liga_payload_para_el_body() {
        // El binding del payload debe estar disponible (y bien tipado) en el cuerpo.
        let src = "enum Caja { Con(int), Vacia } fn val(c: Caja) -> int { match (c) { Caja.Con(n) => n + 1, Caja.Vacia => 0 } } fn main() {}";
        assert!(check_src(src).is_ok());
    }

    // ----- M6.1: funciones genéricas e inferencia -----

    #[test]
    fn generic_identity_y_usage() {
        let src = r#"
fn identity<T>(x: T) -> T { x }
fn main() -> int {
    let a: int = identity(5);
    let b: bool = identity(true);
    if (b) { a } else { 0 }
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn generic_infers_de_various_argumentos() {
        // [T] y fn(T)->U determinan T y U a la vez.
        let src = r#"
fn apply<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }
fn double(n: int) -> int { n * 2 }
fn main() -> int { apply(double, 21) }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn generic_t_inconsistente() {
        err_contains(
            "fn par<T>(a: T, b: T) -> T { a } fn main() -> int { par(1, true) }",
            "cannot be int and bool",
        );
    }

    #[test]
    fn generic_t_no_inferible() {
        err_contains(
            "fn empty<T>() -> int { 0 } fn main() -> int { empty() }",
            "could not infer the type parameter 'T'",
        );
    }

    #[test]
    fn generic_como_valor_es_error() {
        err_contains(
            "fn id<T>(x: T) -> T { x } fn main() -> int { let f: fn(int) -> int = id; f(3) }",
            "generic function 'id' as a value",
        );
    }

    #[test]
    fn generic_no_se_can_compare_un_parameter_de_ty() {
        err_contains(
            "fn ig<T>(a: T, b: T) -> bool { a == b } fn main() {}",
            "same comparable type",
        );
    }

    #[test]
    fn parameter_de_ty_repetido() {
        err_contains("fn f<T, T>(x: T) -> T { x } fn main() {}", "type parameter 'T' repeated");
    }

    #[test]
    fn ty_unknown_no_es_parameter() {
        err_contains("fn f(x: Desconocido) -> int { 0 } fn main() {}", "'Desconocido' not declared");
    }

    // ----- M6.2: tipos genéricos del usuario y chequeo bidireccional -----

    #[test]
    fn enum_generic_construccion_y_match() {
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
    fn struct_generic_campo_sustituido() {
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
    fn generic_mismatch_de_argumento_de_ty() {
        err_contains(
            "enum Caja<T> { Llena(T), Vacia } fn main() { let b: Caja<bool> = Caja.Llena(7); print(b); }",
            "cannot be bool and int",
        );
    }

    #[test]
    fn generic_arity_de_args_de_ty() {
        err_contains(
            "enum Caja<T> { Llena(T), Vacia } fn main() { let b: Caja<int, bool> = Caja.Vacia; print(b); }",
            "expects 1 type argument(s)",
        );
    }

    #[test]
    fn generic_empty_no_inferible_sin_context() {
        // Sin tipo esperado ni argumentos, T queda sin determinar.
        err_contains(
            "enum Caja<T> { Llena(T), Vacia } fn main() { print(Caja.Vacia); }",
            "could not infer",
        );
    }

    #[test]
    fn parameter_de_ty_de_enum_repetido() {
        err_contains("enum E<T, T> { A(T) } fn main() {}", "type parameter 'T' repeated");
    }

    #[test]
    fn array_empty_adopta_el_ty_expected() {
        // El chequeo bidireccional arregla la aspereza histórica del [] vacío.
        assert!(check_src("fn main() -> int { let xs: [int] = []; xs.len() }").is_ok());
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
    fn try_result_y_option_valid_vals() {
        let src = r#"
fn d(a: int, b: int) -> Result<int, string> {
    if (b == 0) { Result.Err("cero") } else { Result.Ok(a / b) }
}
fn calc(x: int, y: int) -> Result<int, string> {
    let q: int = d(x, y)?;
    Result.Ok(q + 1)
}
fn raw(xs: [int]) -> Option<int> { if (xs.len() == 0) { Option.None } else { Option.Some(xs[0]) } }
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
            "requires a Result or an Option",
        );
    }

    #[test]
    fn try_function_must_devolver_compatible() {
        err_contains(
            "fn d() -> Result<int, string> { Result.Ok(1) } fn g() -> int { let x: int = d()?; x } fn main() {}",
            "requires the function to return Result",
        );
    }

    #[test]
    fn try_result_con_e_distinto() {
        err_contains(
            "fn d() -> Result<int, string> { Result.Ok(1) } fn f() -> Result<int, bool> { let x: int = d()?; Result.Ok(x) } fn main() {}",
            "Result<_, string>",
        );
    }

    // ----- UFCS (M7.1) -----

    #[test]
    fn ufcs_function_libre_como_method() {
        // recv.f(args) ≡ f(recv, args). Builtin (len) y función del usuario (suma).
        let src = r#"
fn sum(a: int, b: int) -> int { a + b }
fn main() -> int {
    let xs: [int] = [1, 2, 3];
    let n: int = xs.len();      // len(xs)
    let v: int = 10;
    v.sum(n)                    // suma(10, 3)
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn ufcs_no_es_campo_uses_function() {
        // 'doble' no es campo de Punto: se resuelve como UFCS doble(p).
        let src = r#"
struct Punto { x: int, y: int }
fn double(p: Punto) -> int { (p.x + p.y) * 2 }
fn main() -> int {
    let p: Punto = Punto { x: 3, y: 4 };
    p.double()
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn ufcs_campo_function_gana_about_libre() {
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
    fn ufcs_chained() {
        let src = r#"
fn double(x: int) -> int { x * 2 }
fn inc(x: int) -> int { x + 1 }
fn main() -> int {
    let v: int = 5;
    v.double().inc().double()      // doble(inc(doble(5)))
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn ufcs_method_nonexistent() {
        err_contains(
            "fn main() -> int { let v: int = 5; v.frobnicate() }",
            "no field or function 'frobnicate' applicable to int",
        );
    }

    #[test]
    fn ufcs_receptor_de_ty_incorrect() {
        // El receptor se inserta como primer argumento: si su tipo no encaja, error.
        err_contains(
            "fn double(x: int) -> int { x * 2 } fn main() -> int { let b: bool = true; b.double() }",
            "expected int, got bool",
        );
    }

    #[test]
    fn ufcs_generic_infers_from_receptor() {
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
fn double(x: int) -> int { x * 2 }
fn par(x: int) -> bool { x % 2 == 0 }
fn sum(a: int, b: int) -> int { a + b }
fn main() -> int {
    let xs: [int] = [1, 2, 3, 4];
    let ys: [int] = xs.map(double).filter(par);
    ys.fold(0, sum)
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn prelude_fold_a_ty_distinto() {
        // fold<T, A>: el acumulador A puede diferir del elemento T (aquí bool).
        let src = r#"
fn main() -> int {
    let xs: [int] = [2, 4, 6];
    let all: bool = xs.fold(true, fn(acc: bool, x: int) -> bool { acc && (x % 2 == 0) });
    if (all) { 1 } else { 0 }
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn prelude_map_exige_function_compatible() {
        // map<T,U>(xs:[T], f:fn(T)->U): una f con dominio incompatible hace que el
        // parámetro de tipo T se exija int (por xs) y bool (por f) a la vez: error.
        err_contains(
            "fn f(b: bool) -> int { 1 } fn main() -> int { let xs: [int] = [1]; let ys: [int] = xs.map(f); ys[0] }",
            "cannot be int and bool",
        );
    }

    #[test]
    fn prelude_user_can_redefine() {
        // Si el usuario define 'map', el del prelude se omite (override).
        let src = r#"
fn map(x: int) -> int { x + 1 }
fn main() -> int { map(41) }
"#;
        assert!(check_src(src).is_ok());
    }

    // ----- M8.1: inferencia local (let/var sin anotación) -----

    #[test]
    fn infers_primitivos_y_compuestos() {
        let src = r#"
struct Punto { x: int, y: int }
enum Caja<T> { Llena(T), Vacia }
fn main() -> int {
    let x = 3;                      // int
    let f = 2.5;                    // float
    let b = x > 1;                  // bool
    let s = "hello";                 // string
    let xs = [10, 20, 30];          // [int]
    let p = Punto { x: 7, y: 6 };   // Punto
    let c = Caja.Llena(5);          // Caja<int> (genéricos M6)
    let cv = p.x + p.y;             // int, del campo inferido
    let inside = match (c) { Caja.Llena(v) => v, Caja.Vacia => 0 };  // int
    x + xs[0] + cv + inside
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn variable_inferred_conserva_su_ty() {
        // Una inferida como int no puede luego usarse como bool.
        err_contains(
            "fn main() -> int { let x = 3; if (x) { 0 } else { 1 } }",
            "if condition must be bool",
        );
    }

    #[test]
    fn var_inferred_es_mutable_y_tipada() {
        // 'var t = 0' infiere int y es mutable; asignarle bool falla.
        assert!(check_src("fn main() -> int { var t = 0; t = t + 1; t }").is_ok());
        err_contains(
            "fn main() -> int { var t = 0; t = true; t }",
            "is int but is assigned bool",
        );
    }

    #[test]
    fn let_inferred_follows_siendo_inmutable() {
        // La inferencia no cambia la mutabilidad: un 'let' inferido no se puede reasignar.
        err_contains(
            "fn main() -> int { let x = 3; x = 4; x }",
            "immutable",
        );
    }

    #[test]
    fn inferencia_no_applies_a_lo_indeterminado() {
        // Sin anotación, '[]' no se puede inferir: pide la anotación.
        err_contains(
            "fn main() -> int { let xs = []; xs.len() }",
            "cannot infer the type of []",
        );
    }

    #[test]
    fn annotation_follows_validandose() {
        // Con anotación, un inicializador incompatible sigue siendo error.
        err_contains(
            "fn main() -> int { let x: int = true; x }",
            "initialized with bool",
        );
    }

    // ----- M9.1: traits -----

    #[test]
    fn trait_e_impl_valid_vals() {
        check_src(r#"
            trait Mostrable { fn show(self) -> string; }
            struct Punto { x: int, y: int }
            impl Mostrable for Punto { fn show(self) -> string { "p" } }
            fn main() -> int { let p = Punto { x: 1, y: 2 }; print(p.show()); 0 }
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
    fn self_en_return_val_y_method_internal() {
        check_src(r#"
            trait P { fn add(self, o: Punto) -> Punto; fn double(self) -> Self; }
            struct Punto { x: int, y: int }
            impl P for Punto {
                fn add(self, o: Punto) -> Punto { Punto { x: self.x + o.x, y: self.y + o.y } }
                fn double(self) -> Self { self.add(self) }
            }
            fn main() -> int { let p = Punto { x: 1, y: 2 }; let q = p.double(); q.x }
        "#).expect("Self en return_val y self.method() internal");
    }

    #[test]
    fn campo_gana_about_method_de_trait() {
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
    fn impl_no_cubre_all_los_methods() {
        err_contains(
            r#"trait T { fn a(self) -> int; fn b(self) -> int; }
               struct S { x: int }
               impl T for S { fn a(self) -> int { self.x } }
               fn main() -> int { 0 }"#,
            "does not implement method 'b'",
        );
    }

    #[test]
    fn impl_con_signature_distinta() {
        err_contains(
            r#"trait T { fn a(self) -> int; }
               struct S { x: int }
               impl T for S { fn a(self) -> bool { true } }
               fn main() -> int { 0 }"#,
            "returns bool, but the trait requires int",
        );
    }

    #[test]
    fn method_ambiguo_between_dos_traits() {
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
    fn impl_de_trait_nonexistent() {
        err_contains(
            r#"struct S { x: int }
               impl NoExiste for S { fn f(self) -> int { 1 } }
               fn main() -> int { 0 }"#,
            "trait 'NoExiste' not declared",
        );
    }

    #[test]
    fn impl_concreto_about_ty_generic_es_error() {
        // `impl T for Caja` sin declarar los parámetros de tipo: M9.2b pide `impl<A> T for
        // Caja<A>`. El error guía hacia esa forma.
        err_contains(
            r#"trait T { fn f(self) -> int; }
               struct Caja<A> { v: A }
               impl T for Caja { fn f(self) -> int { 1 } }
               fn main() -> int { 0 }"#,
            "is generic: declare its parameters in the impl",
        );
    }

    #[test]
    fn index_semantico_hover_de_variable() {
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
    fn index_semantico_hover_de_ty() {
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
        // Hover de la **variante** `Rojo` (tras el `.`): su firma. `Color.Rojo` no tiene payload.
        let hv = idx.hovers.iter().find(|h| h.line == 5 && h.text == "Color.Rojo").expect("hover de Rojo");
        assert!(hv.col > he.col, "la variant va after el enum: {} vs {}", hv.col, he.col);
    }

    #[test]
    fn index_semantico_hover_en_interpolation() {
        // El `to_string(e)` sintético de `${e}` comparte posición con `e`; su hover NO debe taparlo.
        // Hover sobre `area` dentro de `${area(3.0)}` → la función, nunca `to_string`.
        let src = "fn area(r: float) -> float { r * r }\nfn main() {\n  print(\"x: ${area(3.0)}\");\n}";
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        let idx = semantic_index(&mut prog);
        // En la línea 3, NINGÚN hover debe ser de `to_string` (el wrapper sintético se omite).
        assert!(!idx.hovers.iter().any(|h| h.line == 3 && h.text.starts_with("to_string:")),
            "el to_string sintético no registra hover");
        // Y `area` sí tiene su hover de función.
        assert!(idx.hovers.iter().any(|h| h.line == 3 && h.text == "area: fn(float) -> float"),
            "hover de area en la interpolación");
    }

    #[test]
    fn index_semantico_hover_en_string_ufcs() {
        // En una cadena `v.doble().inc().doble()` todos los eslabones comparten la posición del
        // receptor: las dos `.doble()` colisionaban en `field_name_pos` y la primera perdía su hover.
        // Ahora se registran ambas posiciones.
        let src = "fn double(x: int) -> int { x * 2 }\nfn inc(x: int) -> int { x + 1 }\nfn main() -> int {\n  let v: int = 5;\n  v.double().inc().double()\n}";
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        let idx = semantic_index(&mut prog);
        // Los dos `doble` de la línea 5 tienen hover en posiciones distintas.
        let cols: Vec<usize> = idx.hovers.iter()
            .filter(|h| h.line == 5 && h.text == "double: fn(int) -> int").map(|h| h.col).collect();
        assert!(cols.len() >= 2, "ambas `.double()` con hover: {cols:?}");
        assert!(cols[0] != cols[1], "en columnas distintas: {cols:?}");
    }

    #[test]
    fn index_semantico_hover_en_match() {
        // M10.2f: dentro de un `match` el índice registra el escrutinio, el enum y la variante del
        // patrón, y los bindings que liga (tanto en el patrón como en el cuerpo).
        let src = "enum Figura { Circulo(float), Punto }\nfn area(f: Figura) -> float {\n  match (f) {\n    Figura.Circulo(r) => r,\n    Figura.Punto => 0.0,\n  }\n}\nfn main() -> int { 0 }";
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        let idx = semantic_index(&mut prog);
        // Enum y variante en el patrón (línea 4).
        assert!(idx.hovers.iter().any(|h| h.line == 4 && h.text == "enum Figura"), "hover enum en patrón");
        assert!(idx.hovers.iter().any(|h| h.line == 4 && h.text == "Figura.Circulo(float)"), "hover variant en patrón");
        // Binding `r` del patrón → su tipo.
        assert!(idx.hovers.iter().any(|h| h.line == 4 && h.text == "r: float"), "hover binding en patrón");
    }

    #[test]
    fn impl_generic_valid() {
        // M9.2b-1: `impl<A> T for Caja<A>` con un método que no usa A.
        assert!(check_src(
            r#"trait T { fn f(self) -> int; }
               struct Caja<A> { v: A }
               impl<A> T for Caja<A> { fn f(self) -> int { 1 } }
               fn main() -> int { let c = Caja { v: 9 }; c.f() }"#,
        ).is_ok());
    }

    #[test]
    fn impl_generic_objetivo_mal_formado_es_error() {
        // El objetivo de un impl genérico debe ser `Caja<A>` con sus propios parámetros.
        err_contains(
            r#"trait T { fn f(self) -> int; }
               struct Caja<A> { v: A }
               impl<A> T for Caja<int> { fn f(self) -> int { 1 } }
               fn main() -> int { 0 }"#,
            "must apply to 'Caja<A>'",
        );
    }

    #[test]
    fn self_outside_de_impl_es_error() {
        err_contains(
            "fn f(x: Self) -> int { 1 } fn main() -> int { 0 }",
            "'Self' is only valid inside a trait or impl",
        );
    }

    #[test]
    fn method_nonexistent_no_es_campo_ni_function() {
        err_contains(
            r#"struct S { x: int }
               fn main() -> int { let s = S { x: 1 }; s.noexiste() }"#,
            "no field or function",
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
            fn double<T: Valor>(x: T) -> int { x.valor() + x.valor() }
            fn pasar<T: Valor>(x: T) -> int { double(x) }
            fn main() -> int {
                let p = Punto { x: 5 };
                double(p) + double(9) + pasar(p)
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
    fn bound_ty_no_implementa() {
        err_contains(
            r#"trait Valor { fn valor(self) -> int; }
               struct Punto { x: int }
               fn usar<T: Valor>(x: T) -> int { x.valor() }
               fn main() -> int { let p = Punto { x: 1 }; usar(p) }"#,
            "Punto does not implement 'Valor'",
        );
    }

    #[test]
    fn bound_method_outside_del_trait() {
        err_contains(
            r#"trait Valor { fn valor(self) -> int; }
               fn usar<T: Valor>(x: T) -> int { x.other() }
               fn main() -> int { 0 }"#,
            "no field or function 'other'",
        );
    }

    #[test]
    fn reenvio_sin_bound_es_error() {
        err_contains(
            r#"trait Valor { fn valor(self) -> int; }
               fn usar<T: Valor>(x: T) -> int { x.valor() }
               fn intermediario<U>(y: U) -> int { usar(y) }
               fn main() -> int { 0 }"#,
            "is not bounded by 'Valor'",
        );
    }

    #[test]
    fn bound_a_trait_nonexistent() {
        err_contains(
            "fn usar<T: NoExiste>(x: T) -> int { 0 } fn main() -> int { 0 }",
            "trait 'NoExiste' not declared",
        );
    }

    // ----- M9.3a: métodos por defecto -----

    #[test]
    fn method_por_default_heredado_y_redefinido() {
        check_src(r#"
            trait Valor {
                fn base(self) -> int;
                fn double(self) -> int { self.base() + self.base() }
            }
            struct A { n: int }
            impl Valor for A { fn base(self) -> int { self.n } }
            struct B { n: int }
            impl Valor for B { fn base(self) -> int { self.n } fn double(self) -> int { 0 } }
            fn main() -> int {
                let a = A { n: 1 };
                let b = B { n: 2 };
                a.double() + b.double()
            }
        "#).expect("default heredado por A, redefinido por B");
    }

    #[test]
    fn method_requerido_sin_default_follows_obligatorio() {
        err_contains(
            r#"trait T { fn req(self) -> int; fn opt(self) -> int { 0 } }
               struct S { x: int }
               impl T for S { fn opt(self) -> int { self.x } }
               fn main() -> int { 0 }"#,
            "does not implement method 'req'",
        );
    }

    #[test]
    fn method_por_default_via_bound() {
        check_src(r#"
            trait Saludo {
                fn name(self) -> int;
                fn double(self) -> int { self.name() + self.name() }
            }
            struct P { v: int }
            impl Saludo for P { fn name(self) -> int { self.v } }
            fn usar<T: Saludo>(x: T) -> int { x.double() }
            fn main() -> int { let p = P { v: 1 }; usar(p) }
        "#).expect("default invocado vía bound");
    }

    // ----- M9.3b: trait objects -----

    #[test]
    fn trait_object_coercion_y_dispatch() {
        check_src(r#"
            trait Figura { fn area(self) -> int; }
            struct Cuadrado { lado: int }
            impl Figura for Cuadrado { fn area(self) -> int { self.lado * self.lado } }
            struct Rect { ancho: int, alto: int }
            impl Figura for Rect { fn area(self) -> int { self.ancho * self.alto } }
            fn total(xs: [dyn Figura]) -> int {
                var s = 0; var i = 0;
                while (i < xs.len()) { s = s + xs[i].area(); i = i + 1; }
                s
            }
            fn main() -> int {
                let fs: [dyn Figura] = [Cuadrado { lado: 2 }, Rect { ancho: 3, alto: 4 }];
                total(fs)
            }
        "#).expect("array heterogéneo de trait objects + dispatch");
    }

    #[test]
    fn trait_object_ty_no_implementa() {
        err_contains(
            r#"trait Figura { fn area(self) -> int; }
               struct P { x: int }
               fn main() -> int { let f: dyn Figura = P { x: 1 }; 0 }"#,
            "does not implement 'Figura'",
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
            "uses 'Self': it is not callable on 'dyn Clon'",
        );
    }

    #[test]
    fn dyn_de_trait_nonexistent() {
        err_contains(
            "fn f(x: dyn NoExiste) -> int { 0 } fn main() -> int { 0 }",
            "trait 'NoExiste' not declared",
        );
    }

    // ----- M10.1: anotaciones -----

    #[test]
    fn test_valid() {
        check_src(r#"
            @test
            fn ok() -> bool { 1 + 1 == 2 }
            fn main() -> int { 0 }
        "#).expect("@test con signature () -> bool");
    }

    #[test]
    fn test_signature_incorrect() {
        err_contains(
            "@test fn malo() -> int { 1 } fn main() -> int { 0 }",
            "must return bool",
        );
    }

    #[test]
    fn test_con_parametros() {
        err_contains(
            "@test fn malo(x: int) -> bool { true } fn main() -> int { 0 }",
            "must not take parameters",
        );
    }

    #[test]
    fn annotation_desconocida() {
        err_contains(
            "@magia fn f() -> bool { true } fn main() -> int { 0 }",
            "unknown annotation: '@magia'",
        );
    }

    #[test]
    fn test_about_struct_es_error() {
        err_contains(
            "@test struct S { x: int } fn main() -> int { 0 }",
            "'@test' is only allowed on functions",
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
                if (p.eq(p)) { 0 } else { 1 }
            }
        "#).expect("@derive(Eq) para struct y enum (unit y con payload)");
    }

    #[test]
    fn derive_eq_compone_con_bound() {
        check_src(r#"
            @derive(Eq)
            enum Color { Rojo, Verde }
            fn iguales<T: Eq>(a: T, b: T) -> bool { a.eq(b) }
            fn main() -> int { if (iguales(Color.Rojo, Color.Rojo)) { 0 } else { 1 } }
        "#).expect("un type derivado satisface el bound T: Eq");
    }

    #[test]
    fn derive_trait_no_soportado() {
        err_contains(
            "@derive(Ord) struct P { x: int } fn main() -> int { 0 }",
            "cannot derive 'Ord'",
        );
    }

    #[test]
    fn derive_en_ty_generic_es_error() {
        err_contains(
            "@derive(Eq) struct Caja<T> { v: T } fn main() -> int { 0 }",
            "generic type",
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
            struct Etiqueta { name: string, donde: Punto, color: Color }
            fn main() -> int {
                let e = Etiqueta { name: "o", donde: Punto { x: 1, y: 2 }, color: Color.Rojo };
                print(e.show());
                0
            }
        "#).expect("@derive(Show) para struct, enum y struct nested");
    }

    #[test]
    fn derive_eq_y_show_juntos() {
        check_src(r#"
            @derive(Eq, Show)
            struct P { x: int }
            fn main() -> int { if (P { x: 1 }.eq(P { x: 1 })) { 0 } else { 1 } }
        "#).expect("@derive(Eq, Show) genera ambos impls");
    }

    #[test]
    fn derive_show_campo_no_soportado_es_error() {
        err_contains(
            "@derive(Show) struct S { xs: [int] } fn main() -> int { 0 }",
            "cannot derive Show for a field of type [int]",
        );
    }
}
