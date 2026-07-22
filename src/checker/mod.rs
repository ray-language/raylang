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

mod core;
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
    let prelude_origin = prepare_program(program)?;
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
    // Paso 8b (V5+D3, bench políglota): fusiones de llamadas al prelude, guardadas por
    // `prelude_origin` (sin overrides del usuario): `sort(a, <prim>#less)` → `__sort_prim(a)`
    // (sort nativo; float fuera por NaN), y `index_of(…)/parse_int(…) .unwrap_or(d)` →
    // `__index_of_or`/`__parse_int_or` (muere el arreglo etiquetado + el Option + los marcos).
    lower_prelude_fusions(program, &prelude_origin);
    // Paso 9 (V2, bench políglota): aplanar las cadenas de `+` de strings (incl. interpolación) a
    // `__concat(a, b, …)` → opcode `ConcatN` (un String con capacidad exacta, sin intermedios).
    // La ÚLTIMA bajada: el `Call` sintético comparte (línea, col) con el `Add` raíz y no debe
    // entrar en las tablas por posición de las pasadas anteriores.
    lower_concat(program, &checker.concat_sites);
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
/// V5+D3 (bench políglota): qué piezas vienen del PRELUDE (no redefinidas por el usuario).
/// `lower_prelude_fusions` solo reescribe si TODAS las piezas del patrón son las del prelude —
/// un override del usuario (p. ej. un sort inverso o un `index_of` propio) debe seguir por el
/// camino genérico.
#[derive(Default)]
pub(super) struct PreludeOrigin {
    /// ¿El `fn sort` en el programa es el del prelude?
    pub(super) sort_fn: bool,
    /// Primitivos cuyo `impl Ord` es el del prelude (claves `int`/`string`/`char`).
    pub(super) ord_prims: HashSet<String>,
    /// D3: ¿los wrappers `fn index_of`/`fn parse_int` son los del prelude?
    pub(super) index_of_fn: bool,
    pub(super) parse_int_fn: bool,
    /// D3: ¿el `unwrap_or` que resolverá como `Option#unwrap_or` es el del prelude? (el usuario no
    /// redefinió el trait `OptionOps` ni aporta su propio `impl OptionOps for Option`).
    pub(super) unwrap_or_impl: bool,
}

fn prepare_program(program: &mut Program) -> Result<PreludeOrigin, TypeError> {
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
    // V5/D3: si el usuario definió su propia función homónima, la del prelude no se inyecta → la
    // fusión correspondiente no aplica (la suya puede tener OTRA semántica).
    let mut origin = PreludeOrigin {
        sort_fn: !defined.contains("sort"),
        index_of_fn: !defined.contains("index_of"),
        parse_int_fn: !defined.contains("parse_int"),
        ..Default::default()
    };
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
    // D3: el `unwrap_or` de Option es del prelude si el usuario NI redefinió el trait `OptionOps`
    // NI aporta un impl propio `(OptionOps, Option)` (que desplazaría al del prelude).
    origin.unwrap_or_impl = !traits_user.contains("OptionOps")
        && !impls_existentes.contains(&("OptionOps".to_string(), Some("Option".to_string())));
    // V5: los `impl Ord` de primitivos que quedan siendo los del prelude (un impl del usuario para
    // el mismo (Ord, tipo) lo desplaza — p. ej. un orden inverso — y el sort nativo no aplica).
    for i in &prelude_impls {
        if i.trait_name == "Ord" {
            if let Some(k) = type_key_of(&i.target) {
                if matches!(k.as_str(), "int" | "string" | "char") {
                    origin.ord_prims.insert(k);
                }
            }
        }
    }
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
    Ok(origin)
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
    /// V2 (bench políglota): posiciones de los `Add` **de strings** (`string + string`). La pasada
    /// `lower_concat` (la ÚLTIMA, para no interferir con las tablas por posición de las demás) aplana
    /// cada cadena en una llamada al primitivo `__concat(a, b, …)` → opcode `ConcatN` en la VM (un
    /// String con capacidad exacta en vez de n−1 intermedios). La interpolación desazucara a `+` en
    /// el parser, así que entra gratis.
    concat_sites: std::collections::HashSet<(usize, usize)>,
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
    /// Bandas de módulo del loader (fix de IDEAS §52): `(línea_de_inicio, prefijo)` ordenado por
    /// inicio. Un sitio UFCS resuelve primero contra las funciones **propias** de su módulo
    /// (`prefijo::nombre`, por la banda a la que pertenece su línea) — el ámbito léxico del módulo,
    /// sin depender de lo que importe la entrada. Vacío en un programa de un solo archivo.
    module_bands: Vec<(usize, String)>,
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

#[cfg(test)]
mod tests;
