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

/// Un valor en tiempo de ejecución.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Unit,
    /// Arreglo (M3). `Rc` da la **semántica de referencia** (clonar el `Value`
    /// comparte el mismo arreglo); `RefCell` permite mutarlo. La GC de M4
    /// reemplazará el `Rc` para manejar ciclos. La igualdad (`==`) derivada es
    /// **estructural** (compara los elementos).
    Array(Rc<RefCell<Vec<Value>>>),
    /// Un struct (M3.2). Mismas propiedades que `Array`: referencia + mutación.
    Struct(Rc<RefCell<StructInstance>>),
    /// Una función como valor (M4.1). El `usize` es un índice en la **tabla de
    /// funciones** del motor: `0..N` son las nombradas (por orden de declaración),
    /// y `N + id` son las anónimas (por su `id`). Sin captura todavía (M4.2).
    Function(usize),
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
            Value::Function(_) => write!(f, "<fn>"),
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
    scopes: Vec<HashMap<String, Value>>,
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

    /// Ejecuta una función nombrada con sus argumentos ya evaluados.
    fn call_function(&mut self, func: &'a Function, args: Vec<Value>) -> EvalResult {
        self.call_body(&func.params, &func.body, args)
    }

    /// Despacha una llamada a través de un índice de la tabla de funciones: `idx`
    /// menor que el número de funciones nombradas es una nombrada; el resto, una
    /// anónima (`idx - N`). (M4.1)
    fn call_index(&mut self, idx: usize, args: Vec<Value>) -> EvalResult {
        let n = self.named.len();
        if idx < n {
            self.call_body(&self.named[idx].params, &self.named[idx].body, args)
        } else {
            let fe = self.anon[idx - n];
            self.call_body(&fe.params, &fe.body, args)
        }
    }

    /// Ejecuta el cuerpo de una función (nombrada o anónima) con sus argumentos.
    fn call_body(&mut self, params: &'a [Param], body: &'a Block, args: Vec<Value>) -> EvalResult {
        // Scoping léxico: la función arranca con una pila de ámbitos NUEVA, no la
        // de quien llama (M4.1: sin captura). Guardamos la actual y la restauramos.
        let saved = mem::take(&mut self.scopes);
        self.scopes.push(HashMap::new()); // ámbito base: los parámetros

        for (param, arg) in params.iter().zip(args.into_iter()) {
            self.define(&param.name, arg);
        }

        let result = self.exec_block(body);
        self.scopes = saved; // restaurar el entorno de quien llama

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
                let rc = self.eval_array(array)?;
                let i = self.eval_int(index)?;
                let len = rc.borrow().len();
                let idx = check_bounds(i, len, index.line, index.col)?;
                let v = rc.borrow()[idx].clone();
                Ok(v)
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
                // El valor de un literal de función es su índice en la tabla: las
                // anónimas viven en `N + id` (M4.1).
                Ok(Value::Function(self.named.len() + fe.id))
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
                // Builtins: evalúan sus argumentos y operan directamente.
                if name == "print" || name == "len" || name == "push" {
                    let mut values = Vec::with_capacity(args.len());
                    for arg in args {
                        values.push(self.eval_expr(arg)?);
                    }
                    return Ok(self.eval_builtin(name, values));
                }
                // Función de nivel superior: llamada directa.
                if let Some(&idx) = self.named_index.get(name.as_str()) {
                    let mut values = Vec::with_capacity(args.len());
                    for arg in args {
                        values.push(self.eval_expr(arg)?);
                    }
                    return self.call_index(idx, values);
                }
            }
        }

        // Camino indirecto: el callee es una expresión que produce un valor-función
        // (una variable, un literal `fn`, el resultado de otra llamada...). (M4.1)
        let idx = match self.eval_expr(callee)? {
            Value::Function(i) => i,
            _ => unreachable!("el checker garantiza una función"),
        };
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval_expr(arg)?);
        }
        self.call_index(idx, values)
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
                _ => unreachable!("el checker garantiza un arreglo"),
            },
            "push" => {
                match &values[0] {
                    Value::Array(rc) => rc.borrow_mut().push(values[1].clone()),
                    _ => unreachable!("el checker garantiza un arreglo"),
                }
                Value::Unit
            }
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

    fn define(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("siempre hay un ámbito activo")
            .insert(name.to_string(), value);
    }

    /// Busca una variable de dentro hacia afuera; `None` si no es una variable (en
    /// ese caso, en `eval` el nombre se interpreta como una función).
    fn lookup_opt(&self, name: &str) -> Option<Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(v.clone());
            }
        }
        None
    }

    fn assign(&mut self, name: &str, value: Value) {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
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

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn run_ok(src: &str) -> Value {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&prog).expect("check ok");
        run(&prog).expect("ejecución sin error")
    }

    fn run_err(src: &str) -> RuntimeError {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&prog).expect("check ok");
        run(&prog).expect_err("debería fallar en ejecución")
    }

    /// Ejecuta una función concreta por nombre (no solo `main`). Sirve para probar
    /// expresiones cuyo tipo no sería válido como retorno de `main` (bool, float).
    /// El fuente debe incluir esa función y un `main` (que el checker exige).
    fn run_named(src: &str, name: &str) -> Value {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&prog).expect("check ok");
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
}
