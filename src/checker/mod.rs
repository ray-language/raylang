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

mod aux;
mod enums;
mod traits;
mod lowering;
use aux::*;
use enums::*;
use traits::*;
use lowering::*;
pub use traits::{generate_derives, member_completion};

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
    lower_dict_calls(program, &mut checker.dict_calls);
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
    let defined: HashSet<String> = program.functions.iter().map(|f| f.name.clone()).collect();
    let mut prelude_fns: Vec<Function> = crate::prelude::functions()
        .into_iter()
        .filter(|f| !defined.contains(&f.name))
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
                let covers_all = subpatterns.iter().all(is_irrefutable);
                if covers_all {
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
        let current = self.check_expr(expr)?;
        // El origen ya es un trait object: misma identidad (nada que hacer) o **upcasting** a un
        // subconjunto (M9.5b: olvidar traits, `dyn S1` → `dyn S2` con S2 ⊆ S1).
        if let Type::Dyn(source) = &current {
            if source.as_slice() == traits {
                return Ok(current);
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
        let key = type_key_of(&current).ok_or_else(|| self.err(line, col, format!(
            "cannot convert {} into 'dyn {}'", current, traits.join(" + ")
        )))?;
        // El tipo concreto debe implementar **todos** los traits del conjunto.
        for tr in traits {
            if !self.impl_traits.contains(&(key.clone(), tr.clone())) {
                return Err(self.err(line, col, format!(
                    "{} does not implement '{}': it cannot be used as 'dyn {}'", current, tr, traits.join(" + ")
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
                vtable.push(self.dict_for(&current, tr, &m.name, line, col)?);
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
                    let example = if tn == "Map" {
                        "let m: Map<string, int> = Map.new()"
                    } else {
                        "let c: Channel<int> = Channel.new()"
                    };
                    Err(self.err(line, col, format!(
                        "cannot infer the type of '{}.{}'; annotate it, e.g. '{}'", tn, name, example)))
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
                self.check_named_call_impl(&n, args, line, col, None, true, None)
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
                    let ty = self.check_named_call_recv(&mangled, &all, line, col, None, &recv_ty)?;
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
        let ty = self.check_named_call_recv(&target, &all_args, line, col, None, recv_ty)?;
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
        self.check_named_call_impl(name, args, line, col, expected, false, None)
    }

    /// Como [`check_named_call`], con el TIPO del primer argumento (el receptor de una reescritura
    /// UFCS/método) ya calculado: NO se re-verifica su expresión. Sin esto, el receptor se
    /// chequeaba dos veces (una al resolver, otra como argumento) y sus registros de lowering por
    /// posición (diccionarios M9.2) se duplicaban — una cadena del mismo método acotado
    /// (`obj().field(a).field(b)`) desencolaba diccionarios corridos → despacho roto (M93.5).
    fn check_named_call_recv(&mut self, name: &str, args: &[Expr], line: usize, col: usize, expected: Option<&Type>, recv: &Type) -> Result<Type, TypeError> {
        self.check_named_call_impl(name, args, line, col, expected, false, Some(recv))
    }

    /// Como [`check_named_call`], pero `hover_directo` indica que `(line, col)` es la posición del
    /// **nombre** llamado (llamada directa `f(...)`), no una reescritura (UFCS/método). Solo entonces
    /// se registra el hover del builtin ahí (M10.2i): así `print`/`pow`/`abs`… muestran su firma.
    #[allow(clippy::too_many_arguments)]
    fn check_named_call_impl(&mut self, name: &str, args: &[Expr], line: usize, col: usize, expected: Option<&Type>, hover_direct: bool, recv: Option<&Type>) -> Result<Type, TypeError> {
        // Builtins (DESIGN.md §7): su firma vive en el **registro único** (`src/builtins.rs`), no
        // dispersa aquí. Se comprueban antes que una local/función homónima (un builtin no se tapa).
        // Se tipan los argumentos por el camino normal y la regla del builtin valida y da el tipo.
        if let Some(b) = crate::builtins::lookup(name) {
            let mut arg_types = Vec::with_capacity(args.len());
            for (i, a) in args.iter().enumerate() {
                if i == 0 && let Some(r) = recv {
                    arg_types.push(r.clone()); // receptor ya tipado: no re-verificar
                    continue;
                }
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
                    let synthetic_wrapper = args.len() == 1 && args[0].line == line && args[0].col == col;
                    if hover_direct && self.gather && !name.starts_with("__") && !synthetic_wrapper {
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
            return self.call_type_recv(ty, args, false, line, col, recv);
        }

        // Función de nivel superior: llamada directa.
        if let Some(sig) = self.functions.get(name) {
            let (type_params, params, ret, bounds) =
                (sig.type_params.clone(), sig.params.clone(), sig.ret.clone(), sig.bounds.clone());
            let label = format!("'{}'", name);
            if type_params.is_empty() {
                // No genérica: aridad y tipos exactos.
                return self.check_args_recv(&params, ret, args, &label, line, col, recv);
            }
            // Genérica: inferir los argumentos de tipo unificando con los argumentos.
            let (ret_ty, sigma) =
                self.check_generic_call(&type_params, &params, &ret, args, &label, line, col, expected, recv)?;
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
    /// M87: `hint` = el callee es una expresión de BLOQUE (if/match/while/bloque) —
    /// casi siempre el gotcha §55 (la cola con '(' tras una sentencia) → el error lo dice.
    fn call_type(&mut self, ty: Type, args: &[Expr], hint: bool, line: usize, col: usize) -> Result<Type, TypeError> {
        self.call_type_recv(ty, args, hint, line, col, None)
    }

    /// Como [`call_type`], con el tipo del primer argumento ya calculado (ver `check_named_call_recv`).
    fn call_type_recv(&mut self, ty: Type, args: &[Expr], hint: bool, line: usize, col: usize, recv: Option<&Type>) -> Result<Type, TypeError> {
        match ty {
            Type::Fn(params, ret) => self.check_args_recv(&params, *ret, args, "the function", line, col, recv),
            other => {
                let mut msg = format!(
                    "cannot call a value of type {} (not a function)",
                    other
                );
                if hint {
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
    /// Con `recv`, el tipo del primer argumento llega YA calculado y su expresión no se
    /// re-verifica (ver `check_named_call_recv`).
    #[allow(clippy::too_many_arguments)]
    fn check_args_recv(&mut self, params: &[Type], ret: Type, args: &[Expr], label: &str, line: usize, col: usize, recv: Option<&Type>) -> Result<Type, TypeError> {
        if args.len() != params.len() {
            return Err(self.err(line, col, format!(
                "{} expects {} argument(s), received {}",
                label, params.len(), args.len()
            )));
        }
        for (i, (arg, expected)) in args.iter().zip(params.iter()).enumerate() {
            // El tipo del parámetro es el esperado del argumento (propaga a `None`,
            // `[]`, `Caja.Vacia`...).
            let at = if i == 0 && let Some(r) = recv {
                r.clone() // receptor ya tipado: no re-verificar (ver check_named_call_recv)
            } else {
                self.check_expr_expected(arg, expected)?
            };
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
        recv: Option<&Type>,
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
            let at = if i == 0 && let Some(r) = recv {
                r.clone() // receptor ya tipado: no re-verificar (ver check_named_call_recv)
            } else {
                self.check_expr(arg)?
            };
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
        self.dict_calls.entry((line, col, callee.to_string())).or_default().push_back(dicts);
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


#[cfg(test)]
mod tests;
