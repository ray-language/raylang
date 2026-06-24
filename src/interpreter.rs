//! Intérprete (tree-walking) de raylang.
//!
//! Cuarta y última fase de M1 (DESIGN.md §2, §8): por fin **ejecuta**. Recorre el
//! AST ya verificado por el checker y lo evalúa nodo a nodo. Como el checker ya
//! garantizó que el programa está bien tipado, el intérprete **confía**: no
//! re-verifica tipos, y las combinaciones imposibles se marcan con `unreachable!`
//! (si alguna saltara, sería un bug del checker, no del programa del usuario).
//!
//! ## Tres ideas nuevas
//!
//! 1. **Valores en runtime** (`Value`): lo que las expresiones producen al
//!    ejecutarse (un `Int`, un `Bool`, …). Distinto de `Type`, que es lo que el
//!    checker razonaba en estático.
//!
//! 2. **Entorno con marcos**: las variables viven en una pila de ámbitos. Cada
//!    *llamada a función* arranca con una pila **nueva** (solo sus parámetros y
//!    locales), de modo que una función no ve las variables de quien la llamó:
//!    eso es *scoping léxico*. Guardamos y restauramos la pila al entrar/salir.
//!
//! 3. **`return` como señal**: en un intérprete de árbol, `return` es un salto que
//!    debe abandonar varios bloques de golpe. Lo modelamos como un `Err(Flow::
//!    Return(v))` que se propaga hacia arriba (con `?`) hasta que la llamada a la
//!    función lo "atrapa" y lo convierte en el valor de retorno.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem;
use std::rc::Rc;

use crate::ast::*;

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
    Map(Rc<RefCell<HashMap<MapKey, Value>>>),
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
}

impl MapKey {
    /// Convierte un valor del intérprete en una clave (el checker garantiza el tipo).
    pub fn from_value(v: &Value) -> MapKey {
        match v {
            Value::Int(n) => MapKey::Int(*n),
            Value::Str(s) => MapKey::Str(s.clone()),
            Value::Char(c) => MapKey::Char(*c),
            Value::Bool(b) => MapKey::Bool(*b),
            _ => unreachable!("el checker garantiza una clave hashable (int/string/char/bool)"),
        }
    }

    /// Reconstruye el valor del intérprete a partir de la clave (para `keys`, M13.1b).
    pub fn to_value(&self) -> Value {
        match self {
            MapKey::Int(n) => Value::Int(*n),
            MapKey::Str(s) => Value::Str(s.clone()),
            MapKey::Char(c) => Value::Char(*c),
            MapKey::Bool(b) => Value::Bool(*b),
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

/// Error en tiempo de ejecución (p. ej. división por cero). Lleva ubicación.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error en ejecución en {}:{}: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for RuntimeError {}

/// Lo que interrumpe la evaluación normal de una expresión/sentencia. Usamos el
/// canal de error de `Result` para DOS cosas:
///   - `Return(v)`: un `return` que se está propagando hacia el borde de la función.
///   - `Error(e)`: un error de ejecución real, que se propaga hasta el tope.
/// Es un truco clásico: ambos "desenrollan" la pila de llamadas de Rust con `?`,
/// pero se tratan distinto en el borde de la función.
enum Flow {
    Return(Value),
    Error(RuntimeError),
}

/// Resultado de evaluar una expresión: un valor, o una interrupción de flujo.
type EvalResult = Result<Value, Flow>;

/// Punto de entrada: ejecuta el programa llamando a `main` y devuelve su valor.
pub fn run(program: &Program) -> Result<Value, RuntimeError> {
    Interpreter::new(program).run_main()
}

/// Almacén de proceso para los **argumentos del programa** (M11.2b). El runner (`main.rs`) los
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

struct Interpreter<'a> {
    /// Todas las funciones del programa, por nombre (las referencias viven mientras
    /// viva el `program`, de ahí el lifetime `'a`).
    functions: HashMap<String, &'a Function>,
    /// Las funciones nombradas, en orden de declaración (para resolver un
    /// `Value::Function(idx)` con `idx < N`).
    named: &'a [Function],
    /// Nombre de función → su índice en `named` (para usar el nombre como valor).
    named_index: HashMap<String, usize>,
    /// Las funciones anónimas, indexadas por su `id` (M4.1).
    anon: Vec<&'a FnExpr>,
    /// Definiciones de struct, por nombre (para construir literales en orden).
    structs: HashMap<String, &'a StructDef>,
    /// Pila de ámbitos de la función en ejecución. El último es el más interno.
    /// Cada variable es una **celda** compartible (M4.2): así una closure puede
    /// capturarla por referencia.
    scopes: Vec<HashMap<String, Cell>>,
    /// Profundidad de llamadas anidadas actualmente activas (M13.3a). Se incrementa
    /// al entrar en `call_body` y se decrementa al salir; al alcanzar `MAX_CALL_DEPTH`
    /// se corta con un error en vez de desbordar la pila de Rust.
    depth: usize,
}

impl<'a> Interpreter<'a> {
    fn new(program: &'a Program) -> Self {
        let mut functions = HashMap::new();
        let mut named_index = HashMap::new();
        for (i, f) in program.functions.iter().enumerate() {
            functions.insert(f.name.clone(), f);
            named_index.insert(f.name.clone(), i);
        }
        let mut structs = HashMap::new();
        for s in &program.structs {
            structs.insert(s.name.clone(), s);
        }
        Interpreter {
            functions,
            named: &program.functions,
            named_index,
            anon: collect_fn_exprs(program),
            structs,
            scopes: Vec::new(),
            depth: 0,
        }
    }

    fn run_main(&mut self) -> Result<Value, RuntimeError> {
        // El checker ya garantizó que 'main' existe.
        let main = *self.functions.get("main").expect("el checker garantiza 'main'");
        match self.call_function(main, Vec::new()) {
            Ok(v) => Ok(v),
            Err(Flow::Error(e)) => Err(e),
            // Un 'return' nunca debería escapar de call_function, pero por si acaso.
            Err(Flow::Return(v)) => Ok(v),
        }
    }

    /// Ejecuta una función nombrada con sus argumentos ya evaluados (sin entorno
    /// capturado).
    fn call_function(&mut self, func: &'a Function, args: Vec<Value>) -> EvalResult {
        self.call_body(&func.params, &func.body, args, &[])
    }

    /// Despacha una llamada a través de un índice de la tabla de funciones: `idx`
    /// menor que el número de funciones nombradas es una nombrada; el resto, una
    /// anónima (`idx - N`). `captured` es el entorno de la closure (vacío si es una
    /// función sin captura). (M4.1/M4.2)
    fn call_index(&mut self, idx: usize, args: Vec<Value>, captured: &[(String, Cell)]) -> EvalResult {
        let n = self.named.len();
        if idx < n {
            self.call_body(&self.named[idx].params, &self.named[idx].body, args, captured)
        } else {
            let fe = self.anon[idx - n];
            self.call_body(&fe.params, &fe.body, args, captured)
        }
    }

    /// Ejecuta el cuerpo de una función (nombrada o anónima) con sus argumentos y su
    /// entorno capturado.
    fn call_body(&mut self, params: &'a [Param], body: &'a Block, args: Vec<Value>, captured: &[(String, Cell)]) -> EvalResult {
        // Guardia de recursión (M13.3a): si ya hay `MAX_CALL_DEPTH` llamadas activas,
        // cortamos con un error limpio en vez de seguir recurriendo sobre la pila de
        // Rust (que acabaría en segfault). La comprobación es ANTES de incrementar,
        // igual que la VM mira `frames.len()` antes de empujar el marco → ambos motores
        // coinciden en la frontera. La posición es la del cuerpo de la función.
        if self.depth >= MAX_CALL_DEPTH {
            return Err(runtime_error(
                body.line,
                body.col,
                "desbordamiento de pila (recursión demasiado profunda)",
            ));
        }
        self.depth += 1;

        // Scoping léxico: la función arranca con una pila de ámbitos NUEVA, no la de
        // quien llama. Guardamos la actual y la restauramos al volver.
        let saved = mem::take(&mut self.scopes);

        // Ámbito base: las celdas capturadas (compartidas con su origen). Una closure
        // lee y muta estas celdas; una función sin captura recibe un mapa vacío.
        let mut base: HashMap<String, Cell> = HashMap::new();
        for (name, cell) in captured {
            base.insert(name.clone(), cell.clone());
        }
        self.scopes.push(base);

        // Ámbito de los parámetros, encima (tapan capturas con el mismo nombre).
        self.scopes.push(HashMap::new());
        for (param, arg) in params.iter().zip(args.into_iter()) {
            self.define(&param.name, arg);
        }

        let result = self.exec_block(body);
        self.scopes = saved; // restaurar el entorno de quien llama
        self.depth -= 1; // salimos de esta llamada

        match result {
            Ok(v) => Ok(v),                         // el cuerpo cayó hasta su valor final
            Err(Flow::Return(v)) => Ok(v),          // un 'return' temprano: ese es el valor
            Err(e @ Flow::Error(_)) => Err(e),      // un error real sigue propagándose
        }
    }

    /// Ejecuta un bloque en su propio ámbito y devuelve su valor (el de la
    /// expresión final, o `Unit`). Propaga `return`/errores que ocurran dentro.
    fn exec_block(&mut self, block: &'a Block) -> EvalResult {
        self.scopes.push(HashMap::new());

        let mut interrupted: Option<Flow> = None;
        for stmt in &block.statements {
            if let Err(flow) = self.exec_stmt(stmt) {
                interrupted = Some(flow);
                break;
            }
        }

        let result = match interrupted {
            Some(flow) => Err(flow),
            None => match &block.tail {
                Some(tail) => self.eval_expr(tail),
                None => Ok(Value::Unit),
            },
        };

        self.scopes.pop();
        result
    }

    /// Ejecuta una sentencia. No produce valor; puede interrumpir el flujo.
    fn exec_stmt(&mut self, stmt: &'a Stmt) -> Result<(), Flow> {
        match &stmt.kind {
            StmtKind::Let { name, value, .. } => {
                let v = self.eval_expr(value)?;
                self.define(name, v);
                Ok(())
            }
            StmtKind::Assign { target, value } => {
                let v = self.eval_expr(value)?;
                match &target.kind {
                    ExprKind::Ident(name) => self.assign(name, v),
                    ExprKind::Index { array, index } => {
                        let rc = self.eval_array(array)?;
                        let i = self.eval_int(index)?;
                        let len = rc.borrow().len();
                        let idx = check_bounds(i, len, index.line, index.col)?;
                        rc.borrow_mut()[idx] = v;
                    }
                    ExprKind::Field { object, name } => {
                        let rc = self.eval_struct(object)?;
                        let mut s = rc.borrow_mut();
                        let slot = s.fields.iter_mut().find(|(n, _)| n == name)
                            .expect("el checker garantiza el campo");
                        slot.1 = v;
                    }
                    _ => unreachable!("el checker garantiza un lvalue"),
                }
                Ok(())
            }
            StmtKind::Return { value } => {
                let v = match value {
                    Some(e) => self.eval_expr(e)?,
                    None => Value::Unit,
                };
                // Lanzamos la señal de retorno: desenrolla hasta call_function.
                Err(Flow::Return(v))
            }
            StmtKind::Expr(e) => {
                self.eval_expr(e)?; // se evalúa por su efecto; el valor se descarta
                Ok(())
            }
        }
    }

    /// Evalúa una expresión a un valor.
    fn eval_expr(&mut self, expr: &'a Expr) -> EvalResult {
        match &expr.kind {
            ExprKind::Int(v) => Ok(Value::Int(*v)),
            ExprKind::Float(v) => Ok(Value::Float(*v)),
            ExprKind::Bool(v) => Ok(Value::Bool(*v)),
            ExprKind::Str(v) => Ok(Value::Str(v.clone())),
            ExprKind::Char(v) => Ok(Value::Char(*v)),

            ExprKind::Ident(name) => match self.lookup_opt(name) {
                Some(v) => Ok(v),
                // No es una variable: es un nombre de función usado como valor.
                None => {
                    let idx = *self.named_index.get(name).expect("el checker garantiza el nombre");
                    Ok(Value::Function(idx))
                }
            },

            ExprKind::Unary { op, expr: inner } => {
                let v = self.eval_expr(inner)?;
                Ok(match (op, v) {
                    (UnaryOp::Neg, Value::Int(n)) => Value::Int(-n),
                    (UnaryOp::Neg, Value::Float(x)) => Value::Float(-x),
                    (UnaryOp::Not, Value::Bool(b)) => Value::Bool(!b),
                    _ => unreachable!("el checker garantiza operandos válidos para el unario"),
                })
            }

            ExprKind::Binary { op, left, right } => {
                self.eval_binary(*op, left, right, expr.line, expr.col)
            }

            ExprKind::Call { callee, args } => self.eval_call(callee, args),

            ExprKind::ArrayLit(elems) => {
                let mut vec = Vec::with_capacity(elems.len());
                for e in elems {
                    vec.push(self.eval_expr(e)?);
                }
                Ok(Value::Array(Rc::new(RefCell::new(vec))))
            }

            ExprKind::Index { array, index } => {
                let target = self.eval_expr(array)?;
                let i = self.eval_int(index)?;
                match target {
                    Value::Array(rc) => {
                        let len = rc.borrow().len();
                        let idx = check_bounds(i, len, index.line, index.col)?;
                        Ok(rc.borrow()[idx].clone())
                    }
                    // M11.4c-2: indexar un string → el carácter en esa posición.
                    Value::Str(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        let idx = check_bounds(i, chars.len(), index.line, index.col)?;
                        Ok(Value::Char(chars[idx]))
                    }
                    _ => unreachable!("el checker garantiza un arreglo o un string"),
                }
            }

            ExprKind::StructLit { name, fields } => {
                // Construimos (y evaluamos) los campos en ORDEN DE DECLARACIÓN, para
                // que la igualdad y la impresión coincidan con la VM.
                let field_names: Vec<String> = self
                    .structs
                    .get(name.as_str())
                    .expect("el checker registró el struct")
                    .fields
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect();
                let mut built = Vec::with_capacity(field_names.len());
                for fname in &field_names {
                    let value_expr = fields
                        .iter()
                        .find(|(n, _)| n == fname)
                        .map(|(_, e)| e)
                        .expect("el checker garantiza que el campo está presente");
                    let v = self.eval_expr(value_expr)?;
                    built.push((fname.clone(), v));
                }
                let inst = StructInstance { name: name.clone(), fields: built };
                Ok(Value::Struct(Rc::new(RefCell::new(inst))))
            }

            ExprKind::EnumLit { enum_name, variant, args } => {
                // Evaluamos el payload en orden y armamos el valor de enum.
                let mut payload = Vec::with_capacity(args.len());
                for a in args {
                    payload.push(self.eval_expr(a)?);
                }
                let inst = EnumInstance {
                    enum_name: enum_name.clone(),
                    variant: variant.clone(),
                    payload,
                };
                Ok(Value::Enum(Rc::new(inst)))
            }

            ExprKind::Field { object, name } => {
                let rc = self.eval_struct(object)?;
                let v = rc
                    .borrow()
                    .fields
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
                    .expect("el checker garantiza el campo");
                Ok(v)
            }

            ExprKind::Func(fe) => {
                let index = self.named.len() + fe.id;
                // Capturamos por referencia las celdas visibles en este punto
                // (M4.2). Snapshot de todos los ámbitos, de fuera hacia dentro para
                // que una variable interior tape a una exterior del mismo nombre.
                let mut map: HashMap<String, Cell> = HashMap::new();
                for scope in &self.scopes {
                    for (name, cell) in scope {
                        map.insert(name.clone(), cell.clone());
                    }
                }
                if map.is_empty() {
                    // Sin nada que capturar: una función simple (más barata).
                    Ok(Value::Function(index))
                } else {
                    let upvalues: Vec<(String, Cell)> = map.into_iter().collect();
                    Ok(Value::Closure(Rc::new(Closure { index, upvalues })))
                }
            }

            ExprKind::If { cond, then_branch, else_branch } => {
                if self.eval_bool(cond)? {
                    self.exec_block(then_branch)
                } else if let Some(else_e) = else_branch {
                    self.eval_expr(else_e)
                } else {
                    Ok(Value::Unit)
                }
            }

            ExprKind::While { cond, body } => {
                while self.eval_bool(cond)? {
                    // Ejecutar el cuerpo; su valor se descarta, pero un 'return' o un
                    // error dentro del bucle se propaga (el '?' sale de la función).
                    self.exec_block(body)?;
                }
                Ok(Value::Unit)
            }

            ExprKind::Match { scrutinee, arms } => {
                let value = self.eval_expr(scrutinee)?;
                // Los brazos se prueban en orden; el checker garantiza que alguno
                // casa (exhaustividad), así que el error final es inalcanzable.
                for arm in arms {
                    if let Some(binds) = match_pattern(&arm.pattern, &value) {
                        self.scopes.push(HashMap::new());
                        for (name, v) in binds {
                            self.define(&name, v);
                        }
                        let result = self.eval_expr(&arm.body);
                        self.scopes.pop();
                        return result;
                    }
                }
                Err(Flow::Error(RuntimeError {
                    msg: "ningún brazo del match casó (no debería ocurrir)".into(),
                    line: scrutinee.line,
                    col: scrutinee.col,
                }))
            }

            ExprKind::Try(inner) => {
                // `?`: desempaqueta Ok/Some, o propaga Err/None como un `return` de la
                // función. El checker garantiza que el valor es un Result o un Option.
                let value = self.eval_expr(inner)?;
                match &value {
                    Value::Enum(e) if e.variant == "Ok" || e.variant == "Some" => {
                        Ok(e.payload[0].clone())
                    }
                    // Err(e) / None: retornar ese mismo valor desde la función actual.
                    Value::Enum(_) => Err(Flow::Return(value)),
                    _ => unreachable!("el checker garantiza un Result o un Option"),
                }
            }

            ExprKind::Block(b) => self.exec_block(b),
        }
    }

    fn eval_call(&mut self, callee: &'a Expr, args: &'a [Expr]) -> EvalResult {
        // Camino directo: el callee es un nombre que NO está tapado por una variable
        // local — un builtin o una función global. Es la vía eficiente (no se
        // construye un valor-función intermedio).
        if let ExprKind::Ident(name) = &callee.kind {
            let is_local = self.lookup_opt(name).is_some();
            if !is_local {
                // Builtins: evalúan sus argumentos y operan directamente. La membresía la da el
                // registro único (`src/builtins.rs`); la implementación vive en `eval_builtin`.
                if crate::builtins::is_builtin(name) {
                    let mut values = Vec::with_capacity(args.len());
                    for arg in args {
                        values.push(self.eval_expr(arg)?);
                    }
                    // `panic` (M13.2a) aborta con el mensaje en la posición de la llamada. Se
                    // intercepta aquí —no en `eval_builtin`— porque debe producir un `Flow::Error`
                    // (un valor de error, no un `Value`), y aquí tenemos la posición del callee.
                    if name == "panic" {
                        let msg = match &values[0] {
                            Value::Str(s) => s.clone(),
                            _ => unreachable!("el checker garantiza un string"),
                        };
                        return Err(runtime_error(callee.line, callee.col, &msg));
                    }
                    return Ok(self.eval_builtin(name, values));
                }
                // Función de nivel superior: llamada directa.
                if let Some(&idx) = self.named_index.get(name.as_str()) {
                    let mut values = Vec::with_capacity(args.len());
                    for arg in args {
                        values.push(self.eval_expr(arg)?);
                    }
                    return self.call_index(idx, values, &[]);
                }
            }
        }

        // Camino indirecto: el callee es una expresión que produce un valor-función
        // (una variable, un literal `fn`, el resultado de otra llamada...). Puede ser
        // una función simple o una closure con su entorno. (M4.1/M4.2)
        let callee_val = self.eval_expr(callee)?;
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval_expr(arg)?);
        }
        match callee_val {
            Value::Function(idx) => self.call_index(idx, values, &[]),
            Value::Closure(c) => {
                // Clonamos el entorno capturado (clona los `Rc` de las celdas, las
                // comparte) para soltar el préstamo del valor antes de la llamada.
                let captured = c.upvalues.clone();
                self.call_index(c.index, values, &captured)
            }
            _ => unreachable!("el checker garantiza una función"),
        }
    }

    /// Ejecuta un builtin (`print`/`len`/`push`) con sus argumentos ya evaluados.
    fn eval_builtin(&self, name: &str, values: Vec<Value>) -> Value {
        match name {
            "print" => {
                println!("{}", values[0]);
                Value::Unit
            }
            "len" => match &values[0] {
                Value::Array(rc) => Value::Int(rc.borrow().len() as i64),
                // M11.1a: len de string = nº de caracteres (Unicode scalar values).
                Value::Str(s) => Value::Int(s.chars().count() as i64),
                // M13.1: len de un Map = nº de entradas.
                Value::Map(rc) => Value::Int(rc.borrow().len() as i64),
                _ => unreachable!("el checker garantiza un arreglo, string o Map"),
            },
            // --- Mapas (M13.1) ---
            "map_new" => Value::Map(Rc::new(RefCell::new(HashMap::new()))),
            "insert" => {
                match &values[0] {
                    Value::Map(rc) => {
                        rc.borrow_mut().insert(MapKey::from_value(&values[1]), values[2].clone());
                    }
                    _ => unreachable!("el checker garantiza un Map"),
                }
                Value::Unit
            }
            "contains_key" => match &values[0] {
                Value::Map(rc) => Value::Bool(rc.borrow().contains_key(&MapKey::from_value(&values[1]))),
                _ => unreachable!("el checker garantiza un Map"),
            },
            // Primitivo: [] o [v]; el prelude lo envuelve en Option<V>.
            "__map_get" => match &values[0] {
                Value::Map(rc) => {
                    let elems = match rc.borrow().get(&MapKey::from_value(&values[1])) {
                        Some(v) => vec![v.clone()],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("el checker garantiza un Map"),
            },
            // M13.1b: quita y devuelve [] o [v]; el prelude → Option<V>.
            "__map_remove" => match &values[0] {
                Value::Map(rc) => {
                    let elems = match rc.borrow_mut().remove(&MapKey::from_value(&values[1])) {
                        Some(v) => vec![v],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("el checker garantiza un Map"),
            },
            // M13.1b: claves ordenadas (determinista).
            "keys" => match &values[0] {
                Value::Map(rc) => {
                    let mut ks: Vec<MapKey> = rc.borrow().keys().cloned().collect();
                    ks.sort();
                    let elems: Vec<Value> = ks.iter().map(|k| k.to_value()).collect();
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("el checker garantiza un Map"),
            },
            // M13.1b: valores en orden de clave ordenada (casa posición a posición con keys).
            "values" => match &values[0] {
                Value::Map(rc) => {
                    let m = rc.borrow();
                    let mut pares: Vec<(&MapKey, &Value)> = m.iter().collect();
                    pares.sort_by(|a, b| a.0.cmp(b.0));
                    let elems: Vec<Value> = pares.iter().map(|(_, v)| (*v).clone()).collect();
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("el checker garantiza un Map"),
            },
            "push" => {
                match &values[0] {
                    Value::Array(rc) => rc.borrow_mut().push(values[1].clone()),
                    _ => unreachable!("el checker garantiza un arreglo"),
                }
                Value::Unit
            }
            // M11.1a: representación textual de un primitivo (la misma que `print`/Display).
            "to_string" => Value::Str(format!("{}", values[0])),
            // M11.1b: recorta los extremos.
            "trim" => match &values[0] {
                Value::Str(s) => Value::Str(s.trim().to_string()),
                _ => unreachable!("el checker garantiza un string"),
            },
            // M11.1b: parte por el separador → arreglo de strings.
            "split" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Str(sep)) => {
                    let parts: Vec<Value> = s.split(sep.as_str()).map(|p| Value::Str(p.to_string())).collect();
                    Value::Array(Rc::new(RefCell::new(parts)))
                }
                _ => unreachable!("el checker garantiza dos strings"),
            },
            // M11.4c-2: los caracteres del string → arreglo de char.
            "chars" => match &values[0] {
                Value::Str(s) => {
                    let cs: Vec<Value> = s.chars().map(Value::Char).collect();
                    Value::Array(Rc::new(RefCell::new(cs)))
                }
                _ => unreachable!("el checker garantiza un string"),
            },
            // M11.4a/M11.7b: ¿el string contiene la subcadena? / ¿el arreglo contiene el elemento?
            "contains" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Str(sub)) => Value::Bool(s.contains(sub.as_str())),
                (Value::Array(rc), x) => Value::Bool(rc.borrow().iter().any(|e| e == x)),
                _ => unreachable!("el checker garantiza string+string o arreglo+elemento"),
            },
            // M11.4a: reemplaza todas las ocurrencias de `de` por `a`.
            "replace" => match (&values[0], &values[1], &values[2]) {
                (Value::Str(s), Value::Str(de), Value::Str(a)) => {
                    Value::Str(s.replace(de.as_str(), a.as_str()))
                }
                _ => unreachable!("el checker garantiza tres strings"),
            },
            // M11.7a: ¿empieza/termina con la subcadena?
            "starts_with" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Str(p)) => Value::Bool(s.starts_with(p.as_str())),
                _ => unreachable!("el checker garantiza dos strings"),
            },
            "ends_with" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Str(p)) => Value::Bool(s.ends_with(p.as_str())),
                _ => unreachable!("el checker garantiza dos strings"),
            },
            // M11.7a: mayúsculas/minúsculas.
            "to_upper" => match &values[0] {
                Value::Str(s) => Value::Str(s.to_uppercase()),
                _ => unreachable!("el checker garantiza un string"),
            },
            "to_lower" => match &values[0] {
                Value::Str(s) => Value::Str(s.to_lowercase()),
                _ => unreachable!("el checker garantiza un string"),
            },
            // M11.7a: subcadena por índice de carácter (con clamp); repetir.
            "substring" => match (&values[0], &values[1], &values[2]) {
                (Value::Str(s), Value::Int(i), Value::Int(j)) => {
                    Value::Str(crate::builtins::substring_chars(s, *i, *j))
                }
                _ => unreachable!("el checker garantiza string, int, int"),
            },
            "repeat" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Int(n)) => Value::Str(crate::builtins::repeat_str(s, *n)),
                _ => unreachable!("el checker garantiza string, int"),
            },
            // M11.7a: primitivo de búsqueda → [] o [i] (índice de carácter). El prelude → Option<int>.
            "__index_of" => match (&values[0], &values[1]) {
                (Value::Str(s), Value::Str(sub)) => {
                    let elems = match crate::builtins::char_index_of(s, sub) {
                        Some(i) => vec![Value::Int(i as i64)],
                        None => vec![],
                    };
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("el checker garantiza dos strings"),
            },
            // M11.7a: une un [string] con el separador.
            "join" => match (&values[0], &values[1]) {
                (Value::Array(rc), Value::Str(sep)) => {
                    let parts: Vec<String> = rc.borrow().iter().map(|v| match v {
                        Value::Str(s) => s.clone(),
                        _ => unreachable!("el checker garantiza [string]"),
                    }).collect();
                    Value::Str(parts.join(sep.as_str()))
                }
                _ => unreachable!("el checker garantiza [string], string"),
            },
            // M11.7b: arreglo nuevo en orden inverso.
            "reverse" => match &values[0] {
                Value::Array(rc) => {
                    let mut v = rc.borrow().clone();
                    v.reverse();
                    Value::Array(Rc::new(RefCell::new(v)))
                }
                _ => unreachable!("el checker garantiza un arreglo"),
            },
            // M11.7b: primitivo que muta el arreglo quitando el último → [] o [x]. Prelude → Option<T>.
            "__pop" => match &values[0] {
                Value::Array(rc) => {
                    let popped = rc.borrow_mut().pop();
                    let elems = popped.map(|v| vec![v]).unwrap_or_default();
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("el checker garantiza un arreglo"),
            },
            // M11.7b: primitivo de búsqueda en arreglo → [] o [i]. Prelude → Option<int>.
            "__position" => match (&values[0], &values[1]) {
                (Value::Array(rc), x) => {
                    let idx = rc.borrow().iter().position(|e| e == x);
                    let elems = idx.map(|i| vec![Value::Int(i as i64)]).unwrap_or_default();
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("el checker garantiza arreglo+elemento"),
            },
            // M11.2a: como print, pero a stderr.
            "eprint" => {
                eprintln!("{}", values[0]);
                Value::Unit
            }
            // M11.2a: primitivo de parseo → [] o [n]. El prelude lo envuelve en Option.
            "__parse_int" => match &values[0] {
                Value::Str(s) => match s.trim().parse::<i64>() {
                    Ok(n) => Value::Array(Rc::new(RefCell::new(vec![Value::Int(n)]))),
                    Err(_) => Value::Array(Rc::new(RefCell::new(vec![]))),
                },
                _ => unreachable!("el checker garantiza un string"),
            },
            // M11.2a: primitivo de lectura de línea → [] en EOF, [linea] si no (sin el '\n').
            "__read_line" => {
                let mut line = String::new();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) => Value::Array(Rc::new(RefCell::new(vec![]))), // EOF
                    Ok(_) => {
                        let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
                        Value::Array(Rc::new(RefCell::new(vec![Value::Str(trimmed)])))
                    }
                    Err(_) => Value::Array(Rc::new(RefCell::new(vec![]))),
                }
            }
            // M11.2b: primitivo de entorno → [] si no existe, [valor] si sí.
            "__env" => match &values[0] {
                Value::Str(name) => match std::env::var(name) {
                    Ok(v) => Value::Array(Rc::new(RefCell::new(vec![Value::Str(v)]))),
                    Err(_) => Value::Array(Rc::new(RefCell::new(vec![]))),
                },
                _ => unreachable!("el checker garantiza un string"),
            },
            // M11.2b: argumentos del programa (del almacén de proceso; [] si no se fijaron).
            "args" => {
                let items = program_args().iter().map(|a| Value::Str(a.clone())).collect();
                Value::Array(Rc::new(RefCell::new(items)))
            }
            // M11.2c: lee un archivo → arreglo etiquetado ["ok", contenido] o ["err", msg].
            "__read_file" => {
                let arr = match &values[0] {
                    Value::Str(path) => match std::fs::read_to_string(path) {
                        Ok(c) => vec![Value::Str("ok".to_string()), Value::Str(c)],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("el checker garantiza un string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.2c: escribe un archivo → ["ok"] o ["err", msg].
            "__write_file" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(path), Value::Str(contents)) => match std::fs::write(path, contents) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("el checker garantiza dos strings"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.4b: ¿existe la ruta? (total, no falla).
            "exists" => match &values[0] {
                Value::Str(path) => Value::Bool(std::path::Path::new(path).exists()),
                _ => unreachable!("el checker garantiza un string"),
            },
            // M11.4b: añade al final del archivo (lo crea si no existe) → ["ok"] o ["err", msg].
            "__append_file" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(path), Value::Str(contents)) => match crate::builtins::append_to_file(path, contents) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("el checker garantiza dos strings"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.7c: borra un archivo → ["ok"] o ["err", msg].
            "__remove_file" => {
                let arr = match &values[0] {
                    Value::Str(path) => match std::fs::remove_file(path) {
                        Ok(()) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("el checker garantiza un string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.7c: lista un directorio → ["ok", n0, …] o ["err", msg].
            "__list_dir" => {
                let arr = match &values[0] {
                    Value::Str(path) => match crate::builtins::list_dir(path) {
                        Ok(nombres) => {
                            let mut v = vec![Value::Str("ok".to_string())];
                            v.extend(nombres.into_iter().map(Value::Str));
                            v
                        }
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e.to_string())],
                    },
                    _ => unreachable!("el checker garantiza un string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.8: abre un archivo → ["ok", handle] o ["err", msg].
            "__open" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Str(path), Value::Str(mode)) => match crate::builtins::open_file(path, mode) {
                        Ok(h) => vec![Value::Str("ok".to_string()), Value::Str(h.to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("el checker garantiza dos strings"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.8: lee una línea del handle → [] (EOF) o [linea].
            "__read_line_handle" => match &values[0] {
                Value::Int(h) => {
                    let elems = crate::builtins::read_line_handle(*h).map(|l| vec![Value::Str(l)]).unwrap_or_default();
                    Value::Array(Rc::new(RefCell::new(elems)))
                }
                _ => unreachable!("el checker garantiza un int"),
            },
            // M11.8: escribe en el handle → ["ok"] o ["err", msg].
            "__write_handle" => {
                let arr = match (&values[0], &values[1]) {
                    (Value::Int(h), Value::Str(s)) => match crate::builtins::write_handle(*h, s) {
                        Ok(_) => vec![Value::Str("ok".to_string())],
                        Err(e) => vec![Value::Str("err".to_string()), Value::Str(e)],
                    },
                    _ => unreachable!("el checker garantiza int, string"),
                };
                Value::Array(Rc::new(RefCell::new(arr)))
            }
            // M11.8: cierra el handle (total).
            "close" => match &values[0] {
                Value::Int(h) => {
                    crate::builtins::close_handle(*h);
                    Value::Int(0)
                }
                _ => unreachable!("el checker garantiza un int"),
            },
            _ => unreachable!("builtin desconocido"),
        }
    }

    fn eval_binary(
        &mut self,
        op: BinaryOp,
        left: &'a Expr,
        right: &'a Expr,
        line: usize,
        col: usize,
    ) -> EvalResult {
        use BinaryOp::*;

        // Los operadores lógicos hacen *cortocircuito*: no evalúan la derecha si la
        // izquierda ya decide el resultado. Por eso se tratan aparte.
        match op {
            And => {
                if !self.eval_bool(left)? {
                    return Ok(Value::Bool(false));
                }
                return Ok(Value::Bool(self.eval_bool(right)?));
            }
            Or => {
                if self.eval_bool(left)? {
                    return Ok(Value::Bool(true));
                }
                return Ok(Value::Bool(self.eval_bool(right)?));
            }
            _ => {}
        }

        let l = self.eval_expr(left)?;
        let r = self.eval_expr(right)?;
        use Value::*;
        Ok(match (op, l, r) {
            // Concatenación de strings (M11.1a): `+` sobre dos strings.
            (Add, Str(a), Str(b)) => Str(a + &b),
            // Concatenación de arreglos (M11.7b): `+` sobre dos arreglos → arreglo nuevo.
            (Add, Array(a), Array(b)) => {
                let mut v = a.borrow().clone();
                v.extend(b.borrow().iter().cloned());
                Array(Rc::new(RefCell::new(v)))
            }
            // Aritmética entera.
            (Add, Int(a), Int(b)) => Int(a + b),
            (Sub, Int(a), Int(b)) => Int(a - b),
            (Mul, Int(a), Int(b)) => Int(a * b),
            (Div, Int(a), Int(b)) => {
                if b == 0 {
                    return Err(runtime_error(line, col, "división entera por cero"));
                }
                Int(a / b)
            }
            (Rem, Int(a), Int(b)) => {
                if b == 0 {
                    return Err(runtime_error(line, col, "módulo por cero"));
                }
                Int(a % b)
            }
            // Aritmética flotante (división por cero da inf/NaN, como IEEE-754).
            (Add, Float(a), Float(b)) => Float(a + b),
            (Sub, Float(a), Float(b)) => Float(a - b),
            (Mul, Float(a), Float(b)) => Float(a * b),
            (Div, Float(a), Float(b)) => Float(a / b),
            (Rem, Float(a), Float(b)) => Float(a % b),
            // Orden (números del mismo tipo).
            (Lt, Int(a), Int(b)) => Bool(a < b),
            (Le, Int(a), Int(b)) => Bool(a <= b),
            (Gt, Int(a), Int(b)) => Bool(a > b),
            (Ge, Int(a), Int(b)) => Bool(a >= b),
            (Lt, Float(a), Float(b)) => Bool(a < b),
            (Le, Float(a), Float(b)) => Bool(a <= b),
            (Gt, Float(a), Float(b)) => Bool(a > b),
            (Ge, Float(a), Float(b)) => Bool(a >= b),
            // M11.7d: orden de strings (lexicográfico) y char (por code point).
            (Lt, Str(a), Str(b)) => Bool(a < b),
            (Le, Str(a), Str(b)) => Bool(a <= b),
            (Gt, Str(a), Str(b)) => Bool(a > b),
            (Ge, Str(a), Str(b)) => Bool(a >= b),
            (Lt, Char(a), Char(b)) => Bool(a < b),
            (Le, Char(a), Char(b)) => Bool(a <= b),
            (Gt, Char(a), Char(b)) => Bool(a > b),
            (Ge, Char(a), Char(b)) => Bool(a >= b),
            // Igualdad (mismo tipo, garantizado por el checker).
            (Eq, a, b) => Bool(a == b),
            (Ne, a, b) => Bool(a != b),
            _ => unreachable!("combinación de operador/operandos que el checker debió rechazar"),
        })
    }

    // ----- Auxiliares -----

    /// Evalúa una expresión que el checker garantizó booleana, y extrae el `bool`.
    fn eval_bool(&mut self, expr: &'a Expr) -> Result<bool, Flow> {
        match self.eval_expr(expr)? {
            Value::Bool(b) => Ok(b),
            _ => unreachable!("el checker garantiza una condición booleana"),
        }
    }

    /// Evalúa una expresión que el checker garantizó `int`.
    fn eval_int(&mut self, expr: &'a Expr) -> Result<i64, Flow> {
        match self.eval_expr(expr)? {
            Value::Int(n) => Ok(n),
            _ => unreachable!("el checker garantiza un int"),
        }
    }

    /// Evalúa una expresión que el checker garantizó arreglo y devuelve su `Rc`
    /// (compartido: mutar a través de él afecta a todos los alias).
    fn eval_array(&mut self, expr: &'a Expr) -> Result<Rc<RefCell<Vec<Value>>>, Flow> {
        match self.eval_expr(expr)? {
            Value::Array(rc) => Ok(rc),
            _ => unreachable!("el checker garantiza un arreglo"),
        }
    }

    /// Evalúa una expresión que el checker garantizó struct y devuelve su `Rc`.
    fn eval_struct(&mut self, expr: &'a Expr) -> Result<Rc<RefCell<StructInstance>>, Flow> {
        match self.eval_expr(expr)? {
            Value::Struct(rc) => Ok(rc),
            _ => unreachable!("el checker garantiza un struct"),
        }
    }

    /// Declara una variable: una **celda nueva**. Como cada declaración crea su
    /// propia celda, el shadowing es seguro aunque una closure haya capturado la
    /// celda anterior (se queda con la vieja).
    fn define(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("siempre hay un ámbito activo")
            .insert(name.to_string(), Rc::new(RefCell::new(value)));
    }

    /// Busca una variable de dentro hacia afuera; `None` si no es una variable (en
    /// ese caso, en `eval` el nombre se interpreta como una función).
    fn lookup_opt(&self, name: &str) -> Option<Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(cell) = scope.get(name) {
                return Some(cell.borrow().clone());
            }
        }
        None
    }

    /// Asigna a una variable **mutando su celda** (no reemplazándola): así el cambio
    /// se ve a través de cualquier closure que la haya capturado.
    fn assign(&mut self, name: &str, value: Value) {
        for scope in self.scopes.iter().rev() {
            if let Some(cell) = scope.get(name) {
                *cell.borrow_mut() = value;
                return;
            }
        }
        unreachable!("el checker garantiza que '{}' está declarada", name)
    }
}

fn runtime_error(line: usize, col: usize, msg: &str) -> Flow {
    Flow::Error(RuntimeError { msg: msg.to_string(), line, col })
}

/// Comprueba que `i` es un índice válido en `0..len`; si no, error de ejecución.
fn check_bounds(i: i64, len: usize, line: usize, col: usize) -> Result<usize, Flow> {
    if i < 0 || (i as usize) >= len {
        return Err(runtime_error(line, col, &format!("índice {} fuera de rango (longitud {})", i, len)));
    }
    Ok(i as usize)
}

/// Intenta casar un patrón (M5.2) contra un valor. Si casa, devuelve las variables a
/// ligar `(nombre, valor)`; si no, `None`. El checker garantiza que la variante y la
/// aridad son consistentes, así que aquí solo se compara la etiqueta y se reparte el
/// payload.
fn match_pattern(pat: &Pattern, value: &Value) -> Option<Vec<(String, Value)>> {
    match &pat.kind {
        PatternKind::Wildcard => Some(Vec::new()),
        PatternKind::Binding(name) => Some(vec![(name.clone(), value.clone())]),
        PatternKind::Variant { variant, bindings, .. } => {
            let e = match value {
                Value::Enum(e) => e,
                _ => return None, // el checker lo impide; por robustez
            };
            if e.variant != *variant {
                return None;
            }
            let mut binds = Vec::new();
            for (b, v) in bindings.iter().zip(&e.payload) {
                if let Some(name) = b {
                    binds.push((name.clone(), v.clone()));
                }
            }
            Some(binds)
        }
    }
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn run_ok(src: &str) -> Value {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        run(&prog).expect("ejecución sin error")
    }

    fn run_err(src: &str) -> RuntimeError {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        run(&prog).expect_err("debería fallar en ejecución")
    }

    /// Ejecuta una función concreta por nombre (no solo `main`). Sirve para probar
    /// expresiones cuyo tipo no sería válido como retorno de `main` (bool, float).
    /// El fuente debe incluir esa función y un `main` (que el checker exige).
    fn run_named(src: &str, name: &str) -> Value {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        // Construimos el intérprete a mano para llamar a la función elegida.
        // Como los tests viven en el mismo módulo, accedemos a sus internos.
        let prog_ref: &'static Program = Box::leak(Box::new(prog));
        let mut interp = Interpreter::new(prog_ref);
        let func = *interp.functions.get(name).expect("la función del test existe");
        match interp.call_function(func, Vec::new()) {
            Ok(v) => v,
            Err(Flow::Return(v)) => v,
            Err(Flow::Error(e)) => panic!("error de ejecución inesperado: {}", e),
        }
    }

    #[test]
    fn aritmetica_y_precedencia() {
        assert_eq!(run_ok("fn main() -> int { 1 + 2 * 3 }"), Value::Int(7));
        assert_eq!(run_ok("fn main() -> int { (1 + 2) * 3 }"), Value::Int(9));
        assert_eq!(run_ok("fn main() -> int { 10 - 2 - 3 }"), Value::Int(5));
        assert_eq!(run_ok("fn main() -> int { 17 % 5 }"), Value::Int(2));
    }

    /// Envuelve un cuerpo de expresión en `fn v() -> RET { BODY } fn main() {}` y
    /// evalúa `v`. Cómodo para probar expresiones bool/float.
    fn eval_as(ret: &str, body: &str) -> Value {
        let src = format!("fn v() -> {} {{ {} }} fn main() {{}}", ret, body);
        run_named(&src, "v")
    }

    #[test]
    fn flotantes() {
        assert_eq!(eval_as("float", "1.0 / 2.0"), Value::Float(0.5));
    }

    #[test]
    fn booleanos_y_comparaciones() {
        assert_eq!(eval_as("bool", "3 < 5"), Value::Bool(true));
        assert_eq!(eval_as("bool", "3 == 5"), Value::Bool(false));
        assert_eq!(eval_as("bool", "!(2 > 1)"), Value::Bool(false));
        assert_eq!(eval_as("bool", "true && false"), Value::Bool(false));
        assert_eq!(eval_as("bool", "false || true"), Value::Bool(true));
    }

    #[test]
    fn cortocircuito_no_evalua_la_derecha() {
        // Si '&&' NO cortocircuitara, evaluaría '1 / 0' y reventaría. Como la
        // izquierda es false, ni lo toca → resultado false, sin error.
        assert_eq!(eval_as("bool", "false && (1 / 0 == 0)"), Value::Bool(false));
        // Lo mismo con '||' y la izquierda true.
        assert_eq!(eval_as("bool", "true || (1 / 0 == 0)"), Value::Bool(true));
    }

    #[test]
    fn if_como_expresion() {
        assert_eq!(run_ok("fn main() -> int { if (true) { 1 } else { 2 } }"), Value::Int(1));
        assert_eq!(
            run_ok("fn main() -> int { let x: int = -4; if (x < 0) { -x } else { x } }"),
            Value::Int(4)
        );
    }

    #[test]
    fn variables_mutacion_y_while() {
        // Suma 0+1+2+3+4 = 10.
        let src = r#"
fn main() -> int {
    var i: int = 0;
    var s: int = 0;
    while (i < 5) {
        s = s + i;
        i = i + 1;
    }
    s
}
"#;
        assert_eq!(run_ok(src), Value::Int(10));
    }

    #[test]
    fn factorial_iterativo() {
        let src = r#"
fn main() -> int {
    var n: int = 5;
    var f: int = 1;
    while (n > 1) {
        f = f * n;
        n = n - 1;
    }
    f
}
"#;
        assert_eq!(run_ok(src), Value::Int(120));
    }

    #[test]
    fn recursion_fibonacci() {
        let src = r#"
fn fib(n: int) -> int {
    if (n < 2) { n } else { fib(n - 1) + fib(n - 2) }
}
fn main() -> int { fib(10) }
"#;
        assert_eq!(run_ok(src), Value::Int(55));
    }

    #[test]
    fn retorno_temprano() {
        let src = r#"
fn signo(x: int) -> int {
    if (x < 0) { return -1; }
    if (x > 0) { return 1; }
    0
}
fn main() -> int { signo(-7) + signo(0) + signo(42) }
"#;
        // -1 + 0 + 1 = 0
        assert_eq!(run_ok(src), Value::Int(0));
    }

    #[test]
    fn scoping_lexico_la_funcion_no_ve_al_llamador() {
        // 'g' usa su propio parámetro 'x'; no ve el 'x' de 'main'. Si hubiera
        // scoping dinámico, esto daría otro resultado.
        let src = r#"
fn g(x: int) -> int { x + 1 }
fn main() -> int {
    let x: int = 100;
    g(5)
}
"#;
        assert_eq!(run_ok(src), Value::Int(6));
    }

    #[test]
    fn shadowing_restaura_el_valor_exterior() {
        let src = r#"
fn main() -> int {
    let x: int = 1;
    { let x: int = 99; }
    x
}
"#;
        assert_eq!(run_ok(src), Value::Int(1));
    }

    #[test]
    fn division_por_cero_es_error_de_ejecucion() {
        let e = run_err("fn main() -> int { 10 / 0 }");
        assert!(e.msg.contains("división"));
        let e = run_err("fn main() -> int { 10 % 0 }");
        assert!(e.msg.contains("módulo"));
    }

    // ----- M5.2: match (ejecución en el intérprete) -----

    #[test]
    fn match_recorre_lista_recursiva() {
        let src = r#"
enum Lista { Cons(int, Lista), Nil }
fn longitud(xs: Lista) -> int {
    match (xs) { Lista.Cons(_, t) => 1 + longitud(t), Lista.Nil => 0 }
}
fn suma(xs: Lista) -> int {
    match (xs) { Lista.Cons(h, t) => h + suma(t), Lista.Nil => 0 }
}
fn main() -> int {
    let xs: Lista = Lista.Cons(10, Lista.Cons(20, Lista.Cons(30, Lista.Nil)));
    longitud(xs) * 100 + suma(xs)
}
"#;
        assert_eq!(run_ok(src), Value::Int(360)); // 3*100 + 60
    }

    #[test]
    fn match_selecciona_el_brazo_correcto() {
        let src = r#"
enum Figura { Circulo(int), Rect(int, int), Punto }
fn area(f: Figura) -> int {
    match (f) {
        Figura.Circulo(r) => 3 * r * r,
        Figura.Rect(w, h) => w * h,
        Figura.Punto => 0,
    }
}
fn main() -> int { area(Figura.Rect(4, 5)) + area(Figura.Circulo(2)) + area(Figura.Punto) }
"#;
        assert_eq!(run_ok(src), Value::Int(32)); // 20 + 12 + 0
    }

    #[test]
    fn match_con_binding_catchall() {
        // El binding suelto liga el valor completo del escrutinio.
        let src = r#"
enum E { Uno, Dos, Otro }
fn n(e: E) -> int { match (e) { E.Uno => 1, E.Dos => 2, otro => 99 } }
fn main() -> int { n(E.Dos) * 100 + n(E.Otro) }
"#;
        assert_eq!(run_ok(src), Value::Int(299)); // 2*100 + 99
    }

    #[test]
    fn try_propaga_y_desempaqueta() {
        let src = r#"
fn d(a: int, b: int) -> Result<int, string> {
    if (b == 0) { Result.Err("cero") } else { Result.Ok(a / b) }
}
fn calc(x: int, y: int, z: int) -> Result<int, string> {
    let q1: int = d(x, y)?;
    let q2: int = d(q1, z)?;
    Result.Ok(q1 + q2)
}
fn desemp(r: Result<int, string>) -> int { match (r) { Result.Ok(v) => v, Result.Err(_) => -1 } }
fn main() -> int {
    desemp(calc(100, 5, 2)) * 100 + desemp(calc(100, 0, 2))   // 30*100 + (-1)
}
"#;
        assert_eq!(run_ok(src), Value::Int(2999));
    }

    #[test]
    fn try_option_none_propaga() {
        let src = r#"
fn primero(xs: [int]) -> Option<int> { if (len(xs) == 0) { Option.None } else { Option.Some(xs[0]) } }
fn mas_uno(xs: [int]) -> Option<int> { let v: int = primero(xs)?; Option.Some(v + 1) }
fn desemp(o: Option<int>) -> int { match (o) { Option.Some(v) => v, Option.None => -99 } }
fn main() -> int { desemp(mas_uno([41])) * 100 + desemp(mas_uno([])) }
"#;
        assert_eq!(run_ok(src), Value::Int(4101)); // 42*100 + (-99)
    }

    #[test]
    fn match_cuerpo_construye_enum() {
        // El cuerpo de un brazo puede construir otra variante (resolución dentro del
        // brazo): comprobamos la cadena completa devolviendo un int derivado.
        let src = r#"
enum Sem { Rojo, Verde }
fn opuesto(s: Sem) -> Sem { match (s) { Sem.Rojo => Sem.Verde, Sem.Verde => Sem.Rojo } }
fn a_int(s: Sem) -> int { match (s) { Sem.Rojo => 0, Sem.Verde => 1 } }
fn main() -> int { a_int(opuesto(Sem.Rojo)) }
"#;
        assert_eq!(run_ok(src), Value::Int(1));
    }
}
