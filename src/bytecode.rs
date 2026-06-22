//! Bytecode de raylang: el lenguaje intermedio que ejecuta la VM (M2).
//!
//! En vez de recorrer el AST en cada evaluación (como el intérprete de M1), lo
//! **compilamos una vez** a una secuencia de instrucciones simples y planas, y la
//! VM las ejecuta sobre una pila. Esa secuencia, junto a sus constantes, vive en un
//! `Chunk`.
//!
//! ## Nota de representación
//!
//! Una VM "de verdad" (como la de Lua o la de CPython) empaqueta las instrucciones
//! en **bytes** para densidad de caché. Aquí usamos un `enum` por instrucción
//! (`Vec<OpCode>`): es lo idiomático en Rust y muchísimo más claro para aprender,
//! a costa de algo de densidad. Empaquetar a bytes sería una optimización posterior.

use crate::interpreter::Value;

/// Una instrucción de la VM. Las que llevan operando (como `Constant`) lo guardan
/// inline.
#[derive(Debug, Clone, PartialEq)]
pub enum OpCode {
    /// Empuja `constants[idx]` a la pila.
    Constant(usize),
    /// Empuja un booleano literal.
    True,
    False,

    // Aritmética: sacan 2 operandos, empujan 1 resultado.
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// Niega el número en la cima (`-x`).
    Negate,
    /// Niega el booleano en la cima (`!b`).
    Not,

    // Comparación: sacan 2, empujan un bool.
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,

    // --- Pila y control de flujo ---
    /// Descarta el valor de la cima.
    Pop,
    /// Empuja el valor unit `()`.
    Unit,
    /// Salta incondicionalmente a la instrucción en el índice dado.
    Jump(usize),
    /// Si la cima es `false`, salta al índice dado. **No** saca la condición de la
    /// pila (la "ojea"); el compilador emite un `Pop` explícito donde corresponde.
    /// Esto es lo que permite el cortocircuito de `&&`/`||`.
    JumpIfFalse(usize),

    // --- Variables locales y llamadas (M2.3) ---
    /// Empuja a la pila el valor del slot local `slot` del marco actual.
    GetLocal(usize),
    /// Saca la cima y la guarda en el slot local `slot` del marco actual.
    SetLocal(usize),
    /// **Declara** un slot local (M4.2): saca la cima y la guarda inicializando el
    /// slot. Distinto de `SetLocal` porque si el slot está *boxeado* (capturado por
    /// una closure), crea una **celda nueva** — cada declaración estrena celda, lo
    /// que hace seguro el shadowing.
    InitLocal(usize),
    /// Llama a `functions[idx]` tomando `argc` argumentos de la pila.
    Call(usize, usize),
    /// Builtin `print`: saca un valor, lo imprime, y empuja unit.
    Print,

    // --- Funciones de primera clase (M4.1) ---
    /// Empuja un valor-función: `functions[idx]` como dato (sin llamarla). Solo para
    /// funciones **sin** captura.
    Function(usize),
    /// Llamada indirecta: en la pila están el valor-función y luego `argc`
    /// argumentos encima. Saca los argumentos y la función, y empuja un marco.
    CallValue(usize),

    // --- Closures (M4.2) ---
    /// Construye una closure de `functions[idx]`: arma su arreglo de upvalues
    /// tomando las celdas que indica `functions[idx].upvalues` del marco actual, y
    /// empuja el valor closure.
    Closure(usize),
    /// Empuja el valor del upvalue `i` de la closure en ejecución (lee su celda).
    GetUpvalue(usize),
    /// Saca la cima y la escribe en el upvalue `i` (muta su celda compartida).
    SetUpvalue(usize),

    // --- Arreglos (M3) ---
    /// Saca `n` valores de la pila y construye un arreglo con ellos (en orden);
    /// empuja el arreglo.
    MakeArray(usize),
    /// Saca el índice y el arreglo; empuja el elemento (chequea límites).
    Index,
    /// Saca valor, índice y arreglo; asigna `arreglo[índice] = valor`.
    SetIndex,
    /// Saca un arreglo; empuja su longitud (int). Builtin `len`.
    Len,
    /// Saca valor y arreglo; agrega el valor al final; empuja unit. Builtin `push`.
    Push,

    // --- Structs (M3.2) ---
    /// Construye el struct definido en `structs[idx]`: saca tantos valores como
    /// campos tenga (estaban en orden de declaración) y empuja el struct.
    MakeStruct(usize),
    /// Saca un struct; empuja el valor de su campo (buscado por nombre).
    GetField(String),
    /// Saca valor y struct; asigna `struct.campo = valor` (por nombre).
    SetField(String),

    // --- Enums (M5) ---
    /// Construye una variante de enum: `(enum_id, variant_id)` indexan
    /// `program.enums`. Saca de la pila tantos valores como aridad tenga la variante
    /// (el payload, en orden) y empuja el valor de enum.
    MakeEnum(usize, usize),

    // --- match (M5.3) ---
    /// Saca un enum de la pila y empuja `Bool(tag == arg)`: ¿es esta la variante?
    /// La cadena de brazos compara con esto y salta con `JumpIfFalse`.
    EnumTagEq(usize),
    /// Saca un enum y empuja el valor en la posición `i` de su payload (para ligar
    /// los sub-patrones de un brazo).
    GetEnumField(usize),
    /// El `match` no casó ningún brazo: error de ejecución. Es un **trap defensivo**:
    /// el checker garantiza exhaustividad, así que es inalcanzable en programas
    /// válidos.
    MatchFail,

    /// Termina la ejecución del chunk; el valor de retorno es la cima de la pila.
    Return,
}

/// Un bloque de bytecode compilado: las instrucciones, la tabla de constantes, y
/// la posición fuente de cada instrucción (para errores con ubicación).
#[derive(Debug, Default, Clone)]
pub struct Chunk {
    pub code: Vec<OpCode>,
    pub constants: Vec<Value>,
    /// `lines[i]` es la `(línea, columna)` de la instrucción `code[i]`. Paralela a
    /// `code`. Es el equivalente a la "line table" de un compilador real: el
    /// bytecode pierde el texto fuente, pero conservamos de dónde vino cada
    /// instrucción para poder reportar errores de ejecución con su ubicación.
    pub lines: Vec<(usize, usize)>,
}

impl Chunk {
    pub fn new() -> Self {
        Chunk::default()
    }

    /// Emite una instrucción con su posición fuente. Devuelve su índice (útil para
    /// el parcheo de saltos en M2.2).
    pub fn emit(&mut self, op: OpCode, line: usize, col: usize) -> usize {
        self.code.push(op);
        self.lines.push((line, col));
        self.code.len() - 1
    }

    /// Registra una constante y devuelve su índice.
    pub fn add_constant(&mut self, value: Value) -> usize {
        self.constants.push(value);
        self.constants.len() - 1
    }

    /// Desensambla el chunk a texto legible. Herramienta para *ver* el bytecode
    /// mientras aprendemos y depuramos.
    pub fn disassemble(&self, name: &str) -> String {
        let mut out = format!("== {} ==\n", name);
        for (i, op) in self.code.iter().enumerate() {
            let (line, col) = self.lines[i];
            // Para Constant mostramos también el valor al que apunta el índice.
            let detail = match op {
                OpCode::Constant(idx) => format!("Constant   {} -> {}", idx, self.constants[*idx]),
                other => format!("{:?}", other),
            };
            out.push_str(&format!("{:04}  {:>3}:{:<3}  {}\n", i, line, col, detail));
        }
        out
    }
}

/// De dónde sale la celda de un upvalue al construir una closure (M4.2). La
/// resolución la hace el compilador al estilo clox.
#[derive(Debug, Clone, PartialEq)]
pub enum UpvalueSource {
    /// Una variable **local** del marco que crea la closure, en este slot.
    Local(usize),
    /// Un **upvalue** del marco que crea la closure (captura transitiva), en este
    /// índice de su propio arreglo de upvalues.
    Upvalue(usize),
}

/// Un upvalue de una función: su nombre (para el intérprete/depuración) y de dónde
/// tomar su celda en el marco que la cierra.
#[derive(Debug, Clone, PartialEq)]
pub struct UpvalueRef {
    pub name: String,
    pub source: UpvalueSource,
}

/// Una función compilada a bytecode.
#[derive(Debug)]
pub struct CompiledFn {
    pub name: String,
    pub arity: usize,
    /// Tamaño del arreglo de slots locales que necesita un marco de esta función.
    pub num_locals: usize,
    /// `captured[s] == true` si el slot local `s` es capturado por alguna closure
    /// anidada y, por tanto, debe **boxearse** (vivir en una celda) (M4.2).
    pub captured: Vec<bool>,
    /// Los upvalues de esta función: cómo construir su entorno al crearla (M4.2).
    pub upvalues: Vec<UpvalueRef>,
    pub chunk: Chunk,
}

/// La definición de un struct, compilada: su nombre y sus campos en orden.
#[derive(Debug)]
pub struct CompiledStruct {
    pub name: String,
    pub fields: Vec<String>,
}

/// La definición de un enum, compilada: su nombre y sus variantes **en orden**. El
/// índice de una variante en `variants` es su *tag* (lo usará el `match` de M5.3).
#[derive(Debug)]
pub struct CompiledEnum {
    pub name: String,
    pub variants: Vec<CompiledVariant>,
}

/// Una variante compilada: su nombre y su aridad (cuántos valores de payload lleva).
#[derive(Debug)]
pub struct CompiledVariant {
    pub name: String,
    pub arity: usize,
}

/// Un programa compilado: sus structs, enums, funciones (indexadas) y el índice de
/// `main`.
#[derive(Debug)]
pub struct CompiledProgram {
    pub functions: Vec<CompiledFn>,
    pub structs: Vec<CompiledStruct>,
    pub enums: Vec<CompiledEnum>,
    pub main: usize,
}
