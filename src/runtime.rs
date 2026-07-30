//! Modelo de valores de runtime, compartido por ambos motores (M35b).
//!
//! Este módulo se **compila siempre** (a diferencia del intérprete de árbol, que vive
//! tras la feature `interp`). Contiene el `Value` que ambos motores producen en la
//! frontera —el intérprete lo usa nativamente; la VM convierte su `HeapValue` a `Value`
//! al salir— más los tipos y helpers que el checker, el compilador, la VM y el binario
//! necesitan sin depender del tree-walker: `MapKey`, `RuntimeError`, la máscara de los
//! enteros con tamaño, el evaluador de constantes literales, el almacén de argumentos de
//! proceso y el límite de profundidad de llamadas.
//!
//! Antes de M35b todo esto vivía en `interpreter.rs`; extraerlo permite que una *release*
//! sin el intérprete (`--no-default-features`) siga teniendo un modelo de valores completo.

use std::cell::RefCell;
use std::collections::HashMap;

/// Almacén interno de un `Map` en el intérprete (P0.1, perf): `HashMap` con el hasher **aHash** en
/// vez del SipHash de std — 2–5× más rápido sobre claves cortas. Espeja [`crate::gc::MapStore`] (VM).
/// En wasm (playground) no hay aHash (su runtime-rng exige getrandom) → SipHash de std, como pre-P0.1.
#[cfg(not(target_arch = "wasm32"))]
pub type MapStore = HashMap<MapKey, Value, ahash::RandomState>;
#[cfg(target_arch = "wasm32")]
pub type MapStore = HashMap<MapKey, Value>;
use std::rc::Rc;

use crate::ast::{Expr, ExprKind, UnaryOp};

/// Una **celda**: una variable en el heap, compartible. Es lo que hace posible la
/// captura por referencia (M4.2): una closure que captura una variable comparte su
/// celda, y mutarla por un lado se ve por el otro. `Rc` da el compartir; `RefCell`,
/// la mutación interior.
pub type Cell = Rc<RefCell<Value>>;

/// Una closure: una función más su **entorno capturado** (M4.2). `index` es el
/// índice en la tabla de funciones del motor (como `Value::Function`); `captured`
/// son las celdas que tomó del entorno donde se creó, por nombre.
#[derive(Debug, Clone)]
pub struct Closure {
    pub index: usize,
    pub upvalues: Vec<(String, Cell)>,
}

/// Un valor en tiempo de ejecución.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Char(char), // M11.4c
    /// Entero sin signo con tamaño (M28.3): `u8`/`u32`/`u64`. El valor va enmascarado a su ancho
    /// (`(val, ancho_en_bits)`). La aritmética envuelve dentro del ancho; el tipo estático distingue
    /// los anchos (el runtime lleva el ancho para poder envolver bien).
    UInt(u64, u8),
    /// Bytes (M16.1a): secuencia inmutable de octetos. `Rc` da clon barato (inmutable → sin `RefCell`,
    /// como el `String` de `Str`). Igualdad estructural.
    Bytes(Rc<Vec<u8>>),
    /// Un **puntero opaco** foráneo (`ptr`, M41.4b): la dirección de un objeto de C, como escalar
    /// inline. Opaco: se compara por identidad (misma dirección) y se imprime `<ptr>` (la dirección
    /// real es no determinista → no se muestra). El GC no lo traza (no apunta al heap de raylang).
    Ptr(i64),
    Unit,
    /// Arreglo (M3). `Rc` da la **semántica de referencia** (clonar el `Value`
    /// comparte el mismo arreglo); `RefCell` permite mutarlo. La GC de M4
    /// reemplazará el `Rc` para manejar ciclos. La igualdad (`==`) derivada es
    /// **estructural** (compara los elementos).
    Array(Rc<RefCell<Vec<Value>>>),
    /// Un struct (M3.2). Mismas propiedades que `Array`: referencia + mutación.
    Struct(Rc<RefCell<StructInstance>>),
    /// Una función como valor **sin entorno capturado** (M4.1): una función de
    /// nivel superior usada como valor, o una anónima que no captura nada. El
    /// `usize` es un índice en la **tabla de funciones** del motor: `0..N` son las
    /// nombradas (por orden de declaración), y `N + id` son las anónimas.
    Function(usize),
    /// Una **closure**: una función con su entorno capturado (M4.2).
    Closure(Rc<Closure>),
    /// Un valor de enum (tipo suma, M5): la variante y su payload. `Rc` da
    /// semántica de referencia (como struct/array) y permite enums recursivos sin
    /// tamaño infinito. Es **inmutable** (no hay asignación a su payload), de ahí que
    /// no lleve `RefCell`.
    Enum(Rc<EnumInstance>),
    /// Un mapa `Map<K, V>` (M13.1). Como el arreglo: `Rc<RefCell<...>>` da semántica de
    /// referencia + mutación. La clave es un `MapKey` (primitivo hashable).
    Map(Rc<RefCell<MapStore>>),
}

/// La **clave** de un `Map` en runtime (M13.1): un primitivo hashable. No incluye `float`
/// (no implementa `Hash`/`Eq` de forma fiable) ni tipos compuestos. El checker garantiza
/// que solo lleguen estos tipos como clave.
/// `Ord` (M13.1b) ordena las claves para que `keys`/`values` sean **deterministas** pese al
/// `HashMap` (clave del oráculo). En un mapa dado todas las claves son del mismo tipo (el checker
/// fija un único `K`), así que el orden entre variantes nunca se observa.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MapKey {
    Int(i64),
    Str(String),
    Char(char),
    Bool(bool),
    Bytes(Vec<u8>), // M16 (diferido): secuencia de octetos como clave (Hash/Eq/Ord fiables)
}

impl MapKey {
    /// Convierte un valor del intérprete en una clave (el checker garantiza el tipo).
    pub fn from_value(v: &Value) -> MapKey {
        match v {
            Value::Int(n) => MapKey::Int(*n),
            Value::Str(s) => MapKey::Str(s.clone()),
            Value::Char(c) => MapKey::Char(*c),
            Value::Bool(b) => MapKey::Bool(*b),
            Value::Bytes(b) => MapKey::Bytes((**b).clone()),
            _ => unreachable!("the checker guarantees a hashable key (int/string/char/bool/bytes)"),
        }
    }

    /// Reconstruye el valor del intérprete a partir de la clave (para `keys`, M13.1b).
    pub fn to_value(&self) -> Value {
        match self {
            MapKey::Int(n) => Value::Int(*n),
            MapKey::Str(s) => Value::Str(s.clone()),
            MapKey::Char(c) => Value::Char(*c),
            MapKey::Bool(b) => Value::Bool(*b),
            MapKey::Bytes(b) => Value::Bytes(Rc::new(b.clone())),
        }
    }
}

/// `PartialEq` de `Value` escrito a mano (no derivado) por dos razones:
///   - las funciones/closures **no** tienen igualdad estructural — se comparan por
///     identidad (las closures, por puntero), lo que además evita una recursión
///     infinita si una closure se captura a sí misma (ciclo);
///   - el resto es estructural, como antes.
/// El checker prohíbe `==` sobre funciones, así que estas reglas casi nunca se
/// ejercitan; están por robustez.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        use Value::*;
        match (self, other) {
            (Int(a), Int(b)) => a == b,
            (Float(a), Float(b)) => a == b,
            (Bool(a), Bool(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Char(a), Char(b)) => a == b,
            (UInt(a, _), UInt(b, _)) => a == b, // M28.3 (mismo ancho garantizado por el checker)
            (Bytes(a), Bytes(b)) => a == b,
            (Ptr(a), Ptr(b)) => a == b, // M41.4b: identidad de puntero (misma dirección)
            (Unit, Unit) => true,
            (Array(a), Array(b)) => *a.borrow() == *b.borrow(),
            (Struct(a), Struct(b)) => *a.borrow() == *b.borrow(),
            (Function(a), Function(b)) => a == b,
            (Closure(a), Closure(b)) => Rc::ptr_eq(a, b),
            // Estructural (variante + payload). El checker prohíbe `==` sobre enums,
            // así que esto está por robustez (y no se ejercita en programas válidos).
            (Enum(a), Enum(b)) => {
                a.enum_name == b.enum_name && a.variant == b.variant && a.payload == b.payload
            }
            // Mapas (M13.1): igualdad estructural, independiente del orden (HashMap). El checker
            // no permite `==` sobre Map en programas; esto es para el oráculo y por robustez.
            (Map(a), Map(b)) => *a.borrow() == *b.borrow(),
            _ => false,
        }
    }
}

/// Instancia de un enum en tiempo de ejecución (M5): qué variante es y su payload
/// posicional. El nombre del enum se guarda para imprimir y para el oráculo.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumInstance {
    pub enum_name: String,
    pub variant: String,
    pub payload: Vec<Value>,
}

/// Instancia de un struct en tiempo de ejecución. Los campos se guardan en **orden
/// de declaración**, para que la igualdad estructural y la impresión sean
/// consistentes entre el intérprete y la VM.
#[derive(Debug, Clone, PartialEq)]
pub struct StructInstance {
    pub name: String,
    pub fields: Vec<(String, Value)>,
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{}", v),
            Value::Float(v) => write!(f, "{}", v),
            Value::Bool(v) => write!(f, "{}", v),
            Value::Str(v) => write!(f, "{}", v),
            Value::Char(v) => write!(f, "{}", v),
            Value::UInt(v, _) => write!(f, "{}", v),
            // M16.1a: print(bytes) está diferido (el checker lo rechaza); esta repr solo existe para
            // completar el Display y se ve, p. ej., en mensajes de depuración.
            Value::Bytes(v) => write!(f, "{}", crate::builtins::bytes_to_hex(v)),
            // M41.4b: la dirección real es no determinista (ASLR) → repr opaca y estable.
            Value::Ptr(_) => write!(f, "<ptr>"),
            Value::Unit => write!(f, "()"),
            Value::Array(rc) => {
                let elems = rc.borrow();
                let parts: Vec<String> = elems.iter().map(|v| v.to_string()).collect();
                write!(f, "[{}]", parts.join(", "))
            }
            Value::Struct(rc) => {
                let s = rc.borrow();
                let parts: Vec<String> = s.fields.iter().map(|(n, v)| format!("{}: {}", n, v)).collect();
                write!(f, "{} {{ {} }}", s.name, parts.join(", "))
            }
            // Las funciones no tienen una representación textual útil: marcador opaco.
            Value::Function(_) | Value::Closure(_) => write!(f, "<fn>"),
            Value::Enum(rc) => {
                if rc.payload.is_empty() {
                    write!(f, "{}.{}", rc.enum_name, rc.variant)
                } else {
                    let parts: Vec<String> = rc.payload.iter().map(|v| v.to_string()).collect();
                    write!(f, "{}.{}({})", rc.enum_name, rc.variant, parts.join(", "))
                }
            }
            // M13.1: el `print` de un Map está diferido (no es printable en el checker), pero
            // Display debe ser total; se ordena por clave para que sea determinista.
            Value::Map(rc) => {
                let m = rc.borrow();
                let mut parts: Vec<String> = m.iter().map(|(k, v)| format!("{}: {}", k.to_value(), v)).collect();
                parts.sort();
                write!(f, "Map{{{}}}", parts.join(", "))
            }
        }
    }
}

/// M79: un marco de la traza de llamadas de un error de runtime. La entrada 0 es el
/// marco más interno (nombre de la función + posición del error); cada entrada
/// siguiente es un llamador, con la posición de su llamada en vuelo.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceFrame {
    pub name: String,
    pub line: usize,
    pub col: usize,
}

/// Error en tiempo de ejecución (p. ej. división por cero). Lleva ubicación y, desde
/// M79, la traza de llamadas (la rellenan los motores al capturar el error; el
/// `Display` NO la incluye — la presenta solo el cliente CLI).
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
    pub trace: Vec<TraceFrame>,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "runtime error at {}:{}: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for RuntimeError {}

/// M27.5: evalúa el valor literal de una constante (el checker garantiza que es un literal).
/// Compartido por ambos motores (lo usa el compilador al construir la tabla de constantes).
pub fn eval_const_literal(e: &Expr) -> Value {
    match &e.kind {
        ExprKind::Int(n) => Value::Int(*n),
        ExprKind::Float(f) => Value::Float(*f),
        ExprKind::Bool(b) => Value::Bool(*b),
        ExprKind::Str(s) => Value::Str(s.clone()),
        ExprKind::Char(c) => Value::Char(*c),
        ExprKind::Bytes(b) => Value::Bytes(Rc::new(b.clone())),
        ExprKind::Unary { op: UnaryOp::Neg, expr } => match &expr.kind {
            ExprKind::Int(n) => Value::Int(-n),
            ExprKind::Float(f) => Value::Float(-f),
            _ => unreachable!("the checker guarantees a negated numeric literal"),
        },
        _ => unreachable!("the checker guarantees a literal"),
    }
}

/// M28.3: máscara de bits de un entero sin signo de `width` bits (`0xFF` para u8, etc.).
pub fn uint_mask(width: u8) -> u64 {
    if width >= 64 { u64::MAX } else { (1u64 << width) - 1 }
}

/// M28.3: construye un `UInt` enmascarando el valor a su ancho (aplica el wrapping).
pub fn make_uint(val: u64, width: u8) -> Value {
    Value::UInt(val & uint_mask(width), width)
}

/// Los argumentos del programa (M11.2b): un almacén de proceso que el runner del binario
/// fija antes de ejecutar; el builtin `args()` los lee en ambos motores. Los clientes que no los
/// fijan (REPL, runner de `@test`, tests) ven `[]`. Es estado de proceso, como `std::env::args`.
static PROGRAM_ARGS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// Fija los argumentos del programa (idempotente: solo la primera vez tiene efecto).
pub fn set_program_args(args: Vec<String>) {
    let _ = PROGRAM_ARGS.set(args);
}

/// Los argumentos del programa fijados por el runner; `&[]` si no se fijaron.
pub fn program_args() -> &'static [String] {
    PROGRAM_ARGS.get().map(Vec::as_slice).unwrap_or(&[])
}

/// Profundidad máxima de llamadas anidadas antes de cortar con un error limpio
/// (M13.3a). La **comparten ambos motores** —el intérprete cuenta llamadas a
/// `call_body`; la VM cuenta marcos (`CallFrame`)— para que coincidan en la
/// frontera: un programa que recurre justo a este límite o lo pasa da el mismo
/// veredicto en los dos. Sin esto, el intérprete desbordaría la pila de Rust
/// (segfault) en vez de errar; con la pila grande del hilo worker (`lib::with_big_stack`)
/// este límite se alcanza holgadamente sin reventar.
pub const MAX_CALL_DEPTH: usize = 1024;
