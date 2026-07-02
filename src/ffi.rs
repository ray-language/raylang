//! FFI: llamar a funciones C nativas desde raylang (M41).
//!
//! Es **la** frontera insegura del lenguaje: todo lo demás es seguro por construcción (sin `null`, GC,
//! límites comprobados), pero al cruzar a C raylang no puede garantizar nada. Declarar una `extern fn`
//! es el acto consciente de abrir esa puerta.
//!
//! **Sin libffi (invariante cero-deps de Cargo).** Llamar a una función C arbitraria en tiempo de
//! ejecución normalmente pide libffi, que armaría el marco según la ABI para cualquier firma. Como no
//! podemos depender de ella, raylang soporta un **catálogo acotado de firmas**: obtenemos el puntero
//! del símbolo con `dlsym` y lo **transmutamos** a un tipo `extern "C" fn(...)` concreto —uno por
//! combinación de aridad y clases de argumento que soportamos— y lo llamamos. Es una limitación honesta
//! (documentada): la mayoría de las APIs C útiles caen en unas pocas formas. M41.1 cubre **primitivos**
//! (int/float/bool) con aridad 0..=3; `bytes`/punteros llegan en M41.2+.
//!
//! `dlopen`/`dlsym` se declaran a mano como `unsafe extern "C"` (patrón de `src/poll.rs`, sin traer el
//! crate `libc`). El handle de cada librería se cachea. Un nombre de librería (`"m"`) se resuelve al
//! archivo de plataforma (`libm.dylib`/`libm.so`) y, si falla, al **handle global** del proceso
//! (`dlopen(NULL)`), donde ya viven libc/libm enlazadas por el propio binario.

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int};
use std::sync::Mutex;

// --- Declaraciones C crudas (como poll.rs; cero deps de Cargo) ---
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
const RTLD_NOW: c_int = 2; // resolución inmediata (igual en Linux y macOS)

/// La clase de un valor en la frontera FFI. `Bool` se marshala como entero C (`int`), pero se
/// conserva aparte para reconstruir un `bool` de raylang al volver. `Unit` solo como retorno (void).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CKind {
    Int,
    Float,
    Bool,
    Unit,
}

/// Descriptor de una función externa listo para llamar: qué librería, qué símbolo, y las clases de
/// sus argumentos y su retorno (para el molde de transmutación y la conversión de valores). Lo produce
/// el checker desde el `ExternFn` del AST y lo consultan ambos motores.
#[derive(Debug, Clone)]
pub struct ExternDesc {
    pub name: String,
    pub lib: String,
    pub arg_kinds: Vec<CKind>,
    pub ret_kind: CKind,
}

/// Clasifica un tipo de raylang para la frontera FFI (M41.1: solo primitivos). `None` si el tipo no
/// es marshalable (arreglos, structs, `bytes`, `string`, … llegan en fases posteriores). Compartido
/// por el checker (validación) y por los motores (construcción del descriptor de llamada).
pub fn ckind(ty: &crate::ast::Type) -> Option<CKind> {
    use crate::ast::Type;
    match ty {
        Type::Int => Some(CKind::Int),
        Type::Float => Some(CKind::Float),
        Type::Bool => Some(CKind::Bool),
        Type::Unit => Some(CKind::Unit),
        _ => None,
    }
}

/// Construye el descriptor de llamada de una función externa desde su AST. `None` si algún tipo no es
/// marshalable (el checker ya lo rechaza; los motores lo usan sobre externs ya validadas).
pub fn desc_of(ext: &crate::ast::ExternFn) -> Option<ExternDesc> {
    let arg_kinds: Option<Vec<CKind>> = ext.params.iter().map(|p| ckind(&p.ty)).collect();
    Some(ExternDesc {
        name: ext.name.clone(),
        lib: ext.lib.clone(),
        arg_kinds: arg_kinds?,
        ret_kind: ckind(&ext.return_type)?,
    })
}

/// Un valor en la frontera, ya colapsado a las dos clases que van en registro: entero (i64, cubre
/// int y bool) o doble (f64). Cada motor convierte su propio `Value`/`HeapValue` a esto y de vuelta.
#[derive(Debug, Clone, Copy)]
pub enum FfiVal {
    Int(i64),
    Float(f64),
}

/// El resultado de una llamada FFI: un `FfiVal`, o `Unit` (void). El motor lo convierte al `Value`
/// que corresponda según `ret_kind` (un `i64` se vuelve `bool` si el retorno declarado era `bool`).
#[derive(Debug, Clone, Copy)]
pub enum FfiRet {
    Int(i64),
    Float(f64),
    Unit,
}

// El molde de una clase de argumento a efectos de la ABI: solo importa si va por el banco de
// registros enteros (I) o el de flotantes (F) —bool va como entero—.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mold {
    I,
    F,
}

fn mold_of(k: CKind) -> Mold {
    match k {
        CKind::Float => Mold::F,
        _ => Mold::I, // Int y Bool van como entero
    }
}

// Caché de handles de librería abiertos (por nombre corto). El puntero es opaco y válido durante toda
// la vida del proceso; se comparte entre hilos tras el Mutex (nunca se cierra: las libs viven siempre).
struct Handle(*mut c_void);
unsafe impl Send for Handle {}

fn handles() -> &'static Mutex<HashMap<String, Handle>> {
    static HANDLES: std::sync::OnceLock<Mutex<HashMap<String, Handle>>> = std::sync::OnceLock::new();
    HANDLES.get_or_init(|| Mutex::new(HashMap::new()))
}

// Nombres de archivo candidatos para una librería corta, según la plataforma.
fn lib_filenames(short: &str) -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec![format!("lib{short}.dylib"), format!("{short}.dylib"), short.to_string()]
    } else {
        vec![format!("lib{short}.so"), format!("lib{short}.so.6"), short.to_string()]
    }
}

// Abre (o recupera de caché) el handle de la librería `lib`. Prueba los nombres de plataforma y, si
// ninguno resuelve, cae al **handle global** del proceso (`dlopen(NULL)`), donde están libc/libm.
fn open_lib(lib: &str) -> Result<*mut c_void, String> {
    let mut map = handles().lock().unwrap();
    if let Some(h) = map.get(lib) {
        return Ok(h.0);
    }
    let mut handle = std::ptr::null_mut();
    for name in lib_filenames(lib) {
        if let Ok(c) = std::ffi::CString::new(name) {
            let h = unsafe { dlopen(c.as_ptr(), RTLD_NOW) };
            if !h.is_null() {
                handle = h;
                break;
            }
        }
    }
    if handle.is_null() {
        // Handle global del proceso: símbolos ya cargados (libc/libm que enlaza el propio binario).
        handle = unsafe { dlopen(std::ptr::null(), RTLD_NOW) };
    }
    if handle.is_null() {
        return Err(format!("no se pudo cargar la librería '{lib}'"));
    }
    map.insert(lib.to_string(), Handle(handle));
    Ok(handle)
}

// Resuelve el puntero de un símbolo en una librería.
fn resolve_symbol(lib: &str, symbol: &str) -> Result<*mut c_void, String> {
    let handle = open_lib(lib)?;
    let c = std::ffi::CString::new(symbol).map_err(|_| format!("símbolo inválido '{symbol}'"))?;
    let sym = unsafe { dlsym(handle, c.as_ptr()) };
    if sym.is_null() {
        return Err(format!("no se encontró el símbolo '{symbol}' en la librería '{lib}'"));
    }
    Ok(sym)
}

/// Llama a la función externa descrita por `desc` con `args` (ya convertidos por el motor). Resuelve
/// el símbolo, transmuta el puntero al molde de la firma y ejecuta la llamada C. `Err` si la librería/
/// símbolo no resuelve o la firma no está en el catálogo soportado.
pub fn call(desc: &ExternDesc, args: &[FfiVal]) -> Result<FfiRet, String> {
    let sym = resolve_symbol(&desc.lib, &desc.name)?;
    let molds: Vec<Mold> = desc.arg_kinds.iter().map(|&k| mold_of(k)).collect();
    let ret = mold_of(desc.ret_kind);
    // Los enteros/flotantes de cada argumento, ya listos para pasar por registro.
    let i = |n: usize| match args[n] { FfiVal::Int(v) => v, FfiVal::Float(v) => v as i64 };
    let f = |n: usize| match args[n] { FfiVal::Float(v) => v, FfiVal::Int(v) => v as f64 };

    // El catálogo acotado de firmas. Cada brazo transmuta el puntero al tipo `extern "C" fn(...)`
    // concreto y llama. `unsafe`: confiamos en que la firma declarada casa con la función real.
    unsafe {
        macro_rules! ret_int {
            ($e:expr) => {
                Ok(if desc.ret_kind == CKind::Unit { FfiRet::Unit } else { FfiRet::Int($e) })
            };
        }
        Ok(match (molds.as_slice(), ret) {
            // --- aridad 0 ---
            ([], Mold::I) => { let g: extern "C" fn() -> i64 = std::mem::transmute(sym); return ret_int!(g()); }
            ([], Mold::F) => { let g: extern "C" fn() -> f64 = std::mem::transmute(sym); FfiRet::Float(g()) }
            // --- aridad 1 ---
            ([Mold::I], Mold::I) => { let g: extern "C" fn(i64) -> i64 = std::mem::transmute(sym); return ret_int!(g(i(0))); }
            ([Mold::I], Mold::F) => { let g: extern "C" fn(i64) -> f64 = std::mem::transmute(sym); FfiRet::Float(g(i(0))) }
            ([Mold::F], Mold::I) => { let g: extern "C" fn(f64) -> i64 = std::mem::transmute(sym); return ret_int!(g(f(0))); }
            ([Mold::F], Mold::F) => { let g: extern "C" fn(f64) -> f64 = std::mem::transmute(sym); FfiRet::Float(g(f(0))) }
            // --- aridad 2 ---
            ([Mold::I, Mold::I], Mold::I) => { let g: extern "C" fn(i64, i64) -> i64 = std::mem::transmute(sym); return ret_int!(g(i(0), i(1))); }
            ([Mold::I, Mold::I], Mold::F) => { let g: extern "C" fn(i64, i64) -> f64 = std::mem::transmute(sym); FfiRet::Float(g(i(0), i(1))) }
            ([Mold::F, Mold::F], Mold::I) => { let g: extern "C" fn(f64, f64) -> i64 = std::mem::transmute(sym); return ret_int!(g(f(0), f(1))); }
            ([Mold::F, Mold::F], Mold::F) => { let g: extern "C" fn(f64, f64) -> f64 = std::mem::transmute(sym); FfiRet::Float(g(f(0), f(1))) }
            ([Mold::I, Mold::F], Mold::F) => { let g: extern "C" fn(i64, f64) -> f64 = std::mem::transmute(sym); FfiRet::Float(g(i(0), f(1))) }
            ([Mold::F, Mold::I], Mold::F) => { let g: extern "C" fn(f64, i64) -> f64 = std::mem::transmute(sym); FfiRet::Float(g(f(0), i(1))) }
            // --- aridad 3 ---
            ([Mold::I, Mold::I, Mold::I], Mold::I) => { let g: extern "C" fn(i64, i64, i64) -> i64 = std::mem::transmute(sym); return ret_int!(g(i(0), i(1), i(2))); }
            ([Mold::F, Mold::F, Mold::F], Mold::F) => { let g: extern "C" fn(f64, f64, f64) -> f64 = std::mem::transmute(sym); FfiRet::Float(g(f(0), f(1), f(2))) }
            _ => return Err(format!(
                "la firma de '{}' no está en el catálogo FFI soportado (M41.1: primitivos int/float/bool, aridad 0..=3)",
                desc.name
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(name: &str, args: Vec<CKind>, ret: CKind) -> ExternDesc {
        ExternDesc { name: name.into(), lib: "m".into(), arg_kinds: args, ret_kind: ret }
    }

    #[test]
    fn llama_a_sqrt_de_libm() {
        let d = desc("sqrt", vec![CKind::Float], CKind::Float);
        match call(&d, &[FfiVal::Float(2.0)]).unwrap() {
            FfiRet::Float(v) => assert!((v - std::f64::consts::SQRT_2).abs() < 1e-12),
            other => panic!("se esperaba float, {other:?}"),
        }
    }

    #[test]
    fn llama_a_pow_aridad_2() {
        let d = desc("pow", vec![CKind::Float, CKind::Float], CKind::Float);
        match call(&d, &[FfiVal::Float(2.0), FfiVal::Float(10.0)]).unwrap() {
            FfiRet::Float(v) => assert!((v - 1024.0).abs() < 1e-9),
            other => panic!("se esperaba float, {other:?}"),
        }
    }

    #[test]
    fn simbolo_inexistente_es_error() {
        let d = desc("no_existe_este_simbolo_xyz", vec![], CKind::Int);
        assert!(call(&d, &[]).is_err());
    }
}
