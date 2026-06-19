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

    /// Termina la ejecución del chunk; el valor de retorno es la cima de la pila.
    Return,
}

/// Un bloque de bytecode compilado: las instrucciones, la tabla de constantes, y
/// la posición fuente de cada instrucción (para errores con ubicación).
#[derive(Debug, Default)]
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
