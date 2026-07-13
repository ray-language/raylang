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
//! La **carga** de librerías va por `libloading` (endurecimiento jul 2026; antes `dlopen`/`dlsym`
//! declarados a mano, que no existían en Windows/MSVC → el binario no linkeaba allí): puro Rust
//! sobre las APIs de plataforma (dlopen en Unix, LoadLibrary en Windows), con los mensajes de error
//! reales del loader. El handle de cada librería se cachea. Un nombre de librería (`"m"`) se
//! resuelve al archivo de plataforma (`libm.dylib`/`libm.so`/`m.dll`) y, si falla, al **handle
//! global** del proceso, donde ya viven libc/libm enlazadas por el propio binario.

// M44a: estos imports solo los usa la maquinaria de carga (cfg(not wasm)); en wasm quedarían sin usar.
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
use std::collections::HashMap;
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
use std::ffi::c_void;
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
use std::os::raw::c_char;
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
use std::sync::Mutex;

/// La clase de un valor en la frontera FFI. `Bool` se marshala como entero C (`int`), pero se
/// conserva aparte para reconstruir un `bool` de raylang al volver. `Unit` solo como retorno (void).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CKind {
    /// `int` → C **`int`** (32 bits, con signo). El caso más común en C (fgetc/abs/atoi/…). Al volver se
    /// **extiende el signo** de 32 a 64 bits (M41.4).
    Int,
    /// `u64` → C **`long`/`size_t`** (64 bits). Para valores anchos: `strlen`, `off_t`, o un puntero
    /// tratado como entero (M41.4).
    U64,
    Float,
    Bool,
    Unit,
    /// `string` → `char*` (NUL-terminado). Como **argumento** (M41.2). A efectos de ABI es un puntero
    /// (banco de registros enteros).
    Str,
    /// `bytes` → puntero al buffer crudo (`void*`/`char*` sin NUL). Como argumento (M41.2).
    Bytes,
    /// **Retorno** `char*` → `Option<bytes>` (M41.3): `NULL → None`, no-NULL → `Some(bytes hasta el NUL)`.
    /// La frontera **copia** los bytes y **nunca libera** el puntero (seguro para prestados/estáticos como
    /// `getenv`/`strstr`; los retornos con propiedad `malloc`ados fugan — honesto, sin corromper el heap).
    OptBytes,
    /// **Retorno** `char*` → `Option<string>` (M41.3): azúcar sobre `OptBytes` que **valida UTF-8**
    /// (bytes inválidos → error de ejecución; para no asumir codificación, usa `Option<bytes>`).
    OptStr,
    /// `ptr` → puntero opaco foráneo (M41.4b), 64 bits. Como argumento (su dirección) o retorno (`void*`
    /// no-NULL).
    Ptr,
    /// **Retorno** `ptr` fallible → `Option<ptr>` (M41.4b): `NULL → None` (p. ej. `fopen`).
    OptPtr,
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
        Type::UInt(64) => Some(CKind::U64),
        Type::Float => Some(CKind::Float),
        Type::Bool => Some(CKind::Bool),
        Type::Unit => Some(CKind::Unit),
        Type::String => Some(CKind::Str),
        Type::Bytes => Some(CKind::Bytes),
        Type::Ptr => Some(CKind::Ptr),
        _ => None,
    }
}

/// Clasifica el tipo de **retorno** de una extern fn. Admite los primitivos (`int/float/bool/unit`) y,
/// para un `char*` de retorno, `Option<bytes>` (→ `OptBytes`) y `Option<string>` (→ `OptStr`, azúcar con
/// validación UTF-8). Un `string`/`bytes` **pelado** de retorno NO se admite: un `char*` puede ser NULL y
/// raylang no tiene `null`, así que la ausencia se modela con `Option`. Acepta el tipo tanto en forma
/// cruda del parser (`Struct("Option", …)`) como resuelta por el checker (`Enum("Option", …)`).
pub fn ret_ckind(ty: &crate::ast::Type) -> Option<CKind> {
    use crate::ast::Type;
    match ty {
        Type::Int => Some(CKind::Int),
        Type::UInt(64) => Some(CKind::U64),
        Type::Float => Some(CKind::Float),
        Type::Bool => Some(CKind::Bool),
        Type::Unit => Some(CKind::Unit),
        Type::Ptr => Some(CKind::Ptr),
        Type::Struct(n, args) | Type::Enum(n, args) if n == "Option" && args.len() == 1 => {
            match &args[0] {
                Type::Bytes => Some(CKind::OptBytes),
                Type::String => Some(CKind::OptStr),
                Type::Ptr => Some(CKind::OptPtr),
                _ => None,
            }
        }
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
        ret_kind: ret_ckind(&ext.return_type)?,
    })
}

/// Un argumento en la frontera FFI. Los primitivos van por valor; `Str`/`Bytes` se toman **prestados**
/// del `Value`/`HeapValue` del motor (viven durante la llamada) y `ffi::call` los materializa a un
/// puntero C (una `CString` NUL-terminada para `Str`; el buffer crudo para `Bytes`).
#[derive(Debug, Clone, Copy)]
pub enum FfiVal<'a> {
    Int(i64),
    Float(f64),
    Str(&'a str),
    Bytes(&'a [u8]),
}

/// El resultado de una llamada FFI: un `FfiVal`, o `Unit` (void). El motor lo convierte al `Value`
/// que corresponda según `ret_kind` (un `i64` se vuelve `bool` si el retorno declarado era `bool`).
#[derive(Debug, Clone)]
pub enum FfiRet {
    Int(i64),
    Float(f64),
    Unit,
    /// Retorno `char*` (M41.3): los bytes copiados hasta el NUL, o `None` si el puntero era NULL. El
    /// motor lo envuelve en `Option<bytes>` o (validando UTF-8) `Option<string>`.
    OptBytes(Option<Vec<u8>>),
    /// Retorno `ptr` (M41.4b): la dirección opaca (no-NULL, por contrato).
    Ptr(i64),
    /// Retorno `Option<ptr>` (M41.4b): la dirección, o `None` si era NULL.
    OptPtr(Option<i64>),
}

// El molde de un **argumento** a efectos de la ABI: banco de registros enteros (`I`) o de flotantes
// (`F`). Aquí NO importa la anchura: la ABI pasa cada argumento entero por un registro y el callee lee
// los bits que necesite de los bajos, así que un `int` (32), un `u64`/puntero (64) y un `bool` van todos
// por el mismo molde `i64`. Por eso el catálogo de firmas no explota con la anchura (solo el retorno la
// distingue).
// M44a: los moldes de ABI y su uso viven solo en `call` (cfg(not wasm)) → gateados para no dar código
// muerto en el build del playground.
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArgMold {
    I,
    F,
}

// El molde del **retorno**: aquí SÍ importa la anchura, porque leemos el registro de retorno con un
// tipo concreto. `I32` (C `int`, se extiende el signo a 64), `I64` (C `long`/`size_t`/puntero), `F`.
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum RetMold {
    I32,
    I64,
    F,
}

#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
fn arg_mold(k: CKind) -> ArgMold {
    match k {
        CKind::Float => ArgMold::F,
        _ => ArgMold::I,
    }
}

#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
fn ret_mold(k: CKind) -> RetMold {
    match k {
        CKind::Float => RetMold::F,
        // C `int` (32 bits): Bool también (un `bool` de C es int-sized).
        CKind::Int | CKind::Bool => RetMold::I32,
        // 64 bits: `u64`/`long`/`size_t`, los punteros de retorno (`char*`, `ptr`, y sus `Option`).
        CKind::U64 | CKind::OptBytes | CKind::OptStr | CKind::Ptr | CKind::OptPtr => RetMold::I64,
        // `void`: el valor no se lee; la anchura da igual.
        CKind::Unit | CKind::Str | CKind::Bytes => RetMold::I64,
    }
}

/// Interpreta el valor de retorno (ya un `i64`: extendido de 32 bits si el molde era `I32`, o directo si
/// `I64`) según el `ret_kind`: `Unit` es `void`; `OptBytes`/`OptStr` tratan el `i64` como `char*` (0 =
/// NULL → `None`; si no, **copian** los bytes hasta el NUL, sin liberar); el resto es un entero (el motor
/// decide `int` vs `u64`). La validación UTF-8 de `OptStr` la hace el motor.
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
fn int_return(desc: &ExternDesc, raw: i64) -> FfiRet {
    match desc.ret_kind {
        CKind::Unit => FfiRet::Unit,
        CKind::OptBytes | CKind::OptStr => {
            if raw == 0 {
                FfiRet::OptBytes(None)
            } else {
                // SAFETY: el contrato de la extern fn dice que devuelve un `char*` válido NUL-terminado.
                let bytes = unsafe { std::ffi::CStr::from_ptr(raw as *const c_char) }.to_bytes().to_vec();
                FfiRet::OptBytes(Some(bytes))
            }
        }
        // Puntero opaco: NO se desreferencia; solo se transporta la dirección (M41.4b).
        CKind::Ptr => FfiRet::Ptr(raw),
        CKind::OptPtr => FfiRet::OptPtr(if raw == 0 { None } else { Some(raw) }),
        _ => FfiRet::Int(raw),
    }
}

// Caché de librerías abiertas (por nombre corto). `libloading::Library` es Send+Sync; NUNCA se
// dropea (fuga deliberada: las librerías —y los punteros a sus símbolos, que la VM puede retener—
// viven toda la ejecución).
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
fn handles() -> &'static Mutex<HashMap<String, &'static libloading::Library>> {
    static HANDLES: std::sync::OnceLock<Mutex<HashMap<String, &'static libloading::Library>>> =
        std::sync::OnceLock::new();
    HANDLES.get_or_init(|| Mutex::new(HashMap::new()))
}

// Nombres de archivo candidatos para una librería corta, según la plataforma.
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
fn lib_filenames(short: &str) -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec![format!("lib{short}.dylib"), format!("{short}.dylib"), short.to_string()]
    } else if cfg!(target_os = "windows") {
        vec![format!("{short}.dll"), format!("lib{short}.dll"), short.to_string()]
    } else {
        vec![format!("lib{short}.so"), format!("lib{short}.so.6"), short.to_string()]
    }
}

// La librería que representa el PROPIO proceso (símbolos ya enlazados: libc/libm). Es el fallback
// cuando el nombre corto no resuelve a un archivo.
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
fn this_process() -> Result<libloading::Library, String> {
    #[cfg(unix)]
    {
        Ok(libloading::os::unix::Library::this().into())
    }
    #[cfg(windows)]
    {
        libloading::os::windows::Library::this().map(Into::into).map_err(|e| e.to_string())
    }
}

// Abre (o recupera de caché) el handle de la librería `lib`. Prueba los nombres de plataforma y, si
// ninguno resuelve, cae al **handle global** del proceso (`dlopen(NULL)`), donde están libc/libm.
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
fn open_lib(lib: &str) -> Result<&'static libloading::Library, String> {
    let mut map = handles().lock().unwrap();
    if let Some(l) = map.get(lib) {
        return Ok(l);
    }
    let mut opened: Option<libloading::Library> = None;
    for name in lib_filenames(lib) {
        // SAFETY: cargar una librería puede ejecutar sus inicializadores; es el contrato del FFI
        // (declarar la extern fn es el acto consciente de abrir la puerta).
        if let Ok(l) = unsafe { libloading::Library::new(&name) } {
            opened = Some(l);
            break;
        }
    }
    let l = match opened {
        Some(l) => l,
        // Handle global del proceso: símbolos ya cargados (libc/libm que enlaza el propio binario).
        None => this_process().map_err(|e| format!("no se pudo cargar la librería '{lib}': {e}"))?,
    };
    // La librería vive para siempre (los punteros a símbolos que retiene la VM lo exigen).
    let l: &'static libloading::Library = Box::leak(Box::new(l));
    map.insert(lib.to_string(), l);
    Ok(l)
}

// Resuelve el puntero de un símbolo en una librería.
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
fn resolve_symbol(lib: &str, symbol: &str) -> Result<*mut c_void, String> {
    let library = open_lib(lib)?;
    // SAFETY: pedimos el símbolo como puntero crudo y lo interpretará el catálogo de moldes según la
    // firma DECLARADA — el mismo contrato de confianza de siempre. La librería es 'static (leak).
    let sym: libloading::Symbol<'static, *mut c_void> = unsafe {
        library
            .get(symbol.as_bytes())
            .map_err(|e| format!("no se encontró el símbolo '{symbol}' en la librería '{lib}': {e}"))?
    };
    Ok(*sym)
}

/// Llama a la función externa descrita por `desc` con `args` (ya convertidos por el motor). Resuelve
/// el símbolo, transmuta el puntero al molde de la firma y ejecuta la llamada C. `Err` si la librería/
/// símbolo no resuelve o la firma no está en el catálogo soportado.
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
pub fn call(desc: &ExternDesc, args: &[FfiVal]) -> Result<FfiRet, String> {
    let sym = resolve_symbol(&desc.lib, &desc.name)?;
    let molds: Vec<ArgMold> = desc.arg_kinds.iter().map(|&k| arg_mold(k)).collect();

    // Materializar cada argumento a su valor de registro (i64 para int/bool/puntero, f64 para float).
    // Las `CString` de los argumentos `string` se **retienen vivas** en `keep` hasta el final de la
    // llamada (la función C recibe su puntero). Los `bytes` se pasan por el puntero del slice prestado.
    enum Reg {
        I(i64),
        F(f64),
    }
    let mut keep: Vec<std::ffi::CString> = Vec::new();
    let mut regs: Vec<Reg> = Vec::with_capacity(args.len());
    for a in args {
        regs.push(match a {
            FfiVal::Int(v) => Reg::I(*v),
            FfiVal::Float(v) => Reg::F(*v),
            FfiVal::Str(s) => {
                let cs = std::ffi::CString::new(*s)
                    .map_err(|_| format!("el argumento string de '{}' contiene un NUL interior", desc.name))?;
                let ptr = cs.as_ptr() as i64;
                keep.push(cs);
                Reg::I(ptr)
            }
            FfiVal::Bytes(b) => Reg::I(b.as_ptr() as i64),
        });
    }
    // Los enteros/flotantes de cada argumento, ya listos para pasar por registro.
    let i = |n: usize| match &regs[n] { Reg::I(v) => *v, Reg::F(v) => *v as i64 };
    let f = |n: usize| match &regs[n] { Reg::F(v) => *v, Reg::I(v) => *v as f64 };

    // El catálogo acotado de firmas. Cada brazo transmuta el puntero al tipo `extern "C" fn(...)`
    // concreto y llama. `unsafe`: confiamos en que la firma declarada casa con la función real.
    // Despacho: la firma de **argumentos** (molde `I`/`F` por posición) elige el tipo `extern "C"`; la
    // macro transmuta y llama con la anchura de **retorno** correcta (i32→signo-extendido, i64, o f64).
    // `int_return` interpreta el i64 resultante según `ret_kind`. `unsafe`: confiamos en la firma declarada.
    macro_rules! dispatch {
        ( ($($aty:ty),*), ($($aval:expr),*) ) => {{
            match ret_mold(desc.ret_kind) {
                RetMold::I32 => { let g: extern "C" fn($($aty),*) -> i32 = unsafe { std::mem::transmute(sym) }; int_return(desc, g($($aval),*) as i64) }
                RetMold::I64 => { let g: extern "C" fn($($aty),*) -> i64 = unsafe { std::mem::transmute(sym) }; int_return(desc, g($($aval),*)) }
                RetMold::F   => { let g: extern "C" fn($($aty),*) -> f64 = unsafe { std::mem::transmute(sym) }; FfiRet::Float(g($($aval),*)) }
            }
        }};
    }
    use ArgMold::{F, I};
    Ok(match molds.as_slice() {
        [] => dispatch!((), ()),
        [I] => dispatch!((i64), (i(0))),
        [F] => dispatch!((f64), (f(0))),
        [I, I] => dispatch!((i64, i64), (i(0), i(1))),
        [I, F] => dispatch!((i64, f64), (i(0), f(1))),
        [F, I] => dispatch!((f64, i64), (f(0), i(1))),
        [F, F] => dispatch!((f64, f64), (f(0), f(1))),
        // Aridad 3: las 8 combinaciones (endurecimiento jul 2026 — faltaban 5 y un
        // `extern fn f(float, int, int)` legítimo caía en "no soportada").
        [I, I, I] => dispatch!((i64, i64, i64), (i(0), i(1), i(2))),
        [I, I, F] => dispatch!((i64, i64, f64), (i(0), i(1), f(2))),
        [I, F, I] => dispatch!((i64, f64, i64), (i(0), f(1), i(2))),
        [I, F, F] => dispatch!((i64, f64, f64), (i(0), f(1), f(2))),
        [F, I, I] => dispatch!((f64, i64, i64), (f(0), i(1), i(2))),
        [F, I, F] => dispatch!((f64, i64, f64), (f(0), i(1), f(2))),
        [F, F, I] => dispatch!((f64, f64, i64), (f(0), f(1), i(2))),
        [F, F, F] => dispatch!((f64, f64, f64), (f(0), f(1), f(2))),
        _ => return Err(format!(
            "la signature de '{}' no está en el catálogo FFI soportado (int/u64/float/bool/puntero, arity 0..=3)",
            desc.name
        )),
    })
}

/// M44a/M89.3: sin carga dinámica de librerías — en el playground web (wasm) o en un binario slim
/// (sin la feature `ffi`). Un `extern fn` chequea igual (su descriptor es puro), pero llamarlo da
/// un error de ejecución con el motivo. La propiedad del slim: este binario NO puede cargar código
/// nativo, verificable en compilación.
#[cfg(any(not(feature = "ffi"), target_arch = "wasm32"))]
pub fn call(desc: &ExternDesc, _args: &[FfiVal]) -> Result<FfiRet, String> {
    if cfg!(target_arch = "wasm32") {
        Err(format!("FFI no disponible en el playground web (wasm): '{}'", desc.name))
    } else {
        Err(format!("este binary se compiló sin soporte de FFI (recompila con la feature 'ffi'): '{}'", desc.name))
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
    fn llama_a_pow_arity_2() {
        let d = desc("pow", vec![CKind::Float, CKind::Float], CKind::Float);
        match call(&d, &[FfiVal::Float(2.0), FfiVal::Float(10.0)]).unwrap() {
            FfiRet::Float(v) => assert!((v - 1024.0).abs() < 1e-9),
            other => panic!("se esperaba float, {other:?}"),
        }
    }

    #[test]
    fn simbolo_nonexistent_es_error() {
        let d = desc("no_existe_este_simbolo_xyz", vec![], CKind::Int);
        assert!(call(&d, &[]).is_err());
    }

    #[test]
    fn strlen_marshala_string_a_char_ptr() {
        let d = ExternDesc { name: "strlen".into(), lib: "c".into(), arg_kinds: vec![CKind::Str], ret_kind: CKind::Int };
        match call(&d, &[FfiVal::Str("hello mundo")]).unwrap() {
            FfiRet::Int(n) => assert_eq!(n, 11),
            other => panic!("se esperaba int, {other:?}"),
        }
    }

    #[test]
    fn strlen_marshala_bytes_nul_terminados() {
        let d = ExternDesc { name: "strlen".into(), lib: "c".into(), arg_kinds: vec![CKind::Bytes], ret_kind: CKind::Int };
        match call(&d, &[FfiVal::Bytes(b"abcde\x00")]).unwrap() {
            FfiRet::Int(n) => assert_eq!(n, 5),
            other => panic!("se esperaba int, {other:?}"),
        }
    }

    #[test]
    fn strstr_returns_char_ptr_como_optbytes() {
        let d = ExternDesc { name: "strstr".into(), lib: "c".into(), arg_kinds: vec![CKind::Str, CKind::Str], ret_kind: CKind::OptBytes };
        // Encontrado → Some(bytes desde la coincidencia).
        match call(&d, &[FfiVal::Str("hello world"), FfiVal::Str("world")]).unwrap() {
            FfiRet::OptBytes(Some(b)) => assert_eq!(b, b"world"),
            other => panic!("se esperaba Some, {other:?}"),
        }
        // No encontrado → NULL → None.
        match call(&d, &[FfiVal::Str("abc"), FfiVal::Str("z")]).unwrap() {
            FfiRet::OptBytes(None) => {}
            other => panic!("se esperaba None, {other:?}"),
        }
    }

    #[test]
    fn strstr_returns_option_ptr() {
        // M41.4b: el mismo char* como puntero OPACO. No se desreferencia: solo Some(dirección≠0)/None.
        let d = ExternDesc { name: "strstr".into(), lib: "c".into(), arg_kinds: vec![CKind::Str, CKind::Str], ret_kind: CKind::OptPtr };
        match call(&d, &[FfiVal::Str("hello"), FfiVal::Str("ll")]).unwrap() {
            FfiRet::OptPtr(Some(p)) => assert_ne!(p, 0),
            other => panic!("se esperaba Some, {other:?}"),
        }
        match call(&d, &[FfiVal::Str("hello"), FfiVal::Str("z")]).unwrap() {
            FfiRet::OptPtr(None) => {}
            other => panic!("se esperaba None, {other:?}"),
        }
    }
}
