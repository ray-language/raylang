//! M445 (pepa #321): **avisos** del front-end — diagnósticos que no invalidan el programa.
//!
//! El primero: el **resultado descartado** de una función pura. `sort`, `sort_by`, `trim`,
//! `map`… devuelven un valor NUEVO y dejan su argumento intacto; escritas como sentencia
//! (`sort(xs);`, `xs.sort();`, `s.trim();`) no hacen nada, y pepa ordenó su agenda así sin que
//! nadie avisara. Es la clase de error que un `#[must_use]` caza en Rust. Solo se avisa de las
//! funciones del PRELUDE no redefinidas por el usuario (su `fn sort` propio puede tener otra
//! semántica), y en la forma de método solo si ningún struct tiene un campo con ese nombre (un
//! campo-closure `s.sort()` es una llamada corriente).
//!
//! Los avisos son del checker de Rust (no del espejo selfhost: no forman parte del contrato de
//! errores) y nunca cambian el código de salida; `ray run`/`ray check` los imprimen en stderr
//! con la forma `warning at L:C: msg` y el LSP los publica con severidad Warning.

use crate::ast::{Block, Expr, ExprKind, ForIter, Program, StmtKind};

/// Un aviso con posición (1-basada), como [`super::TypeError`] pero sin invalidar el programa.
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    pub msg: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "warning at {}:{}: {}", self.line, self.col, self.msg)
    }
}

/// Funciones del prelude (y métodos de sus traits) que devuelven un valor NUEVO sin tocar su
/// argumento: (nombre, qué devuelven). `reverse` también: `xs.reverse()` es una copia invertida.
const PURE: &[(&str, &str)] = &[
    ("sort", "a new array"),
    ("sort_by", "a new array"),
    ("sort_by_key", "a new array"),
    ("reverse", "a new array"),
    ("map", "a new array"),
    ("filter", "a new array"),
    ("slice", "a new array"),
    ("fold", "a value"),
    ("trim", "a new string"),
    ("replace", "a new string"),
    ("to_upper", "a new string"),
    ("to_lower", "a new string"),
    ("substring", "a new string"),
    ("repeat", "a new string"),
    ("split", "a new array"),
    ("chars", "a new array"),
    ("join", "a new string"),
    ("to_bytes", "new bytes"),
];

/// Los avisos de resultado descartado del programa, **antes** de `check` (con solo las funciones
/// del usuario en `program.functions`, sin el prelude inyectado ni las bajadas de UFCS).
pub fn discarded_results(program: &Program) -> Vec<Warning> {
    // Revisión 1.27.91: también un método de `impl` del usuario (`impl Sorter for Tree { fn sort }`) o
    // una función de un módulo (`pool::join`, que se llama `pool.join()`) con ese nombre.
    let overridden = |name: &str| {
        program.functions.iter().any(|f| f.name == name || f.name.rsplit("::").next() == Some(name))
            || program.impls.iter().any(|i| i.methods.iter().any(|m| m.name == name))
    };
    // Un campo de tipo función con ese nombre (`h.sort(1)` llama al closure del campo).
    let is_field = |name: &str| program.structs.iter().any(|s| s.fields.iter().any(|(f, ty)| f == name && matches!(ty, crate::ast::Type::Fn(..))));
    let mut out = Vec::new();
    {
        let mut visit = Visitor { out: &mut out, overridden: &overridden, is_field: &is_field };
        // Solo el módulo de entrada (los nombres sin `::`): un paquete que el usuario no puede editar
        // no debe avisar en cada ejecución.
        for f in program.functions.iter().filter(|f| !f.name.contains("::")) {
            visit.block(&f.body);
        }
        for imp in &program.impls {
            for m in &imp.methods {
                visit.block(&m.body);
            }
        }
        for t in &program.traits {
            for m in &t.methods {
                if let Some(body) = &m.default_body {
                    visit.block(body);
                }
            }
        }
    }
    out.sort_by_key(|w| (w.line, w.col));
    out
}

struct Visitor<'a> {
    out: &'a mut Vec<Warning>,
    overridden: &'a dyn Fn(&str) -> bool,
    is_field: &'a dyn Fn(&str) -> bool,
}

impl Visitor<'_> {
    fn block(&mut self, block: &Block) {
        for s in &block.statements {
            match &s.kind {
                StmtKind::Expr(e) => {
                    self.statement_call(e);
                    self.expr(e);
                }
                StmtKind::Let { value, .. } | StmtKind::LetTuple { value, .. } => self.expr(value),
                StmtKind::Assign { target, value } => {
                    self.expr(target);
                    self.expr(value);
                }
                StmtKind::Return { value } => {
                    if let Some(e) = value {
                        self.expr(e);
                    }
                }
                StmtKind::For { iter, body, .. } => {
                    match iter {
                        ForIter::Range { start, end } => {
                            self.expr(start);
                            self.expr(end);
                        }
                        ForIter::In(e) => self.expr(e),
                        ForIter::Iter { expr, .. } => self.expr(expr),
                    }
                    self.block(body);
                }
                StmtKind::Break { .. } | StmtKind::Continue { .. } => {}
            }
        }
        if let Some(t) = &block.tail {
            self.expr(t);
        }
    }

    /// Una sentencia-expresión que es una llamada a una función pura: aviso.
    fn statement_call(&mut self, e: &Expr) {
        let ExprKind::Call { callee, args } = &e.kind else { return };
        let (name, call) = match &callee.kind {
            ExprKind::Ident(name) => (name.as_str(), format!("{name}(…)")),
            ExprKind::Field { name, object } => {
                if (self.is_field)(name) {
                    return;
                }
                let recv = match &object.kind {
                    ExprKind::Ident(r) => r.as_str(),
                    _ => "x",
                };
                (name.as_str(), format!("{recv}.{name}({})", if args.is_empty() { "" } else { "…" }))
            }
            _ => return,
        };
        let Some((_, what)) = PURE.iter().find(|(n, _)| *n == name) else { return };
        if (self.overridden)(name) {
            return;
        }
        self.out.push(Warning {
            msg: format!("the result of `{call}` is discarded: `{name}` returns {what} and leaves its argument unchanged (assign it: `x = {call}`)"),
            line: e.line,
            col: e.col,
            len: 1,
        });
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Int(..)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Char(_)
            | ExprKind::Bytes(_)
            | ExprKind::Ident(_) => {}
            ExprKind::Unary { expr, .. } => self.expr(expr),
            ExprKind::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            ExprKind::Call { callee, args } => {
                self.expr(callee);
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
                for x in elems {
                    self.expr(x);
                }
            }
            ExprKind::MapLit(pairs) => {
                for (k, v) in pairs {
                    self.expr(k);
                    self.expr(v);
                }
            }
            ExprKind::Index { array, index } => {
                self.expr(array);
                self.expr(index);
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, x) in fields {
                    self.expr(x);
                }
            }
            ExprKind::Field { object, .. } => self.expr(object),
            ExprKind::Cast { expr, .. } => self.expr(expr),
            ExprKind::EnumLit { args, .. } => {
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Func(fe) => self.block(&fe.body),
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    self.expr(&arm.body);
                }
            }
            ExprKind::Try(inner) => self.expr(inner),
            ExprKind::If { cond, then_branch, else_branch } => {
                self.expr(cond);
                self.block(then_branch);
                if let Some(x) = else_branch {
                    self.expr(x);
                }
            }
            ExprKind::While { cond, body, .. } => {
                self.expr(cond);
                self.block(body);
            }
            ExprKind::Block(b) => self.block(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warnings_of(src: &str) -> Vec<String> {
        let toks = crate::lexer::lex(src).unwrap();
        let program = crate::parser::parse(toks).unwrap();
        discarded_results(&program).into_iter().map(|w| w.to_string()).collect()
    }

    /// pepa #321: `sort(xs);`, `xs.sort();` y `s.trim();` como sentencia avisan; asignadas, no.
    #[test]
    fn a_discarded_pure_result_warns_in_free_and_method_form() {
        let w = warnings_of("fn main() -> int {\n    var xs = [3, 1];\n    sort(xs);\n    xs.sort();\n    let s = \" a \";\n    s.trim();\n    let ys = sort(xs);\n    xs.push(1);\n    print(ys);\n    0\n}\n");
        assert_eq!(w.len(), 3, "{w:?}");
        assert_eq!(w[0], "warning at 3:5: the result of `sort(…)` is discarded: `sort` returns a new array and leaves its argument unchanged (assign it: `x = sort(…)`)");
        assert!(w[1].starts_with("warning at 4:5: the result of `xs.sort()` is discarded"), "{}", w[1]);
        assert!(w[2].starts_with("warning at 6:5: the result of `s.trim()` is discarded: `trim` returns a new string"), "{}", w[2]);
    }

    /// Dentro de closures, bloques de `if` y métodos de impl también; y un `fn sort` del usuario
    /// o un campo-closure `sort` callan el aviso.
    #[test]
    fn nested_bodies_warn_and_overrides_do_not() {
        let w = warnings_of("struct P { x: int }\ntrait M { fn m(self); }\nimpl M for P { fn m(self) { let xs = [1]; if (true) { xs.reverse(); } let f = fn() { xs.map(fn(n: int) -> int { n }); }; f(); } }\nfn main() -> int { 0 }\n");
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].contains("`xs.reverse()`") && w[1].contains("`xs.map(…)`"), "{w:?}");
        assert!(warnings_of("fn sort(xs: [int]) -> [int] { xs }\nfn main() -> int { let xs = [1]; sort(xs); xs.sort(); 0 }\n").is_empty());
        assert!(warnings_of("struct H { sort: fn(int) -> int }\nfn main() -> int { let h = H { sort: fn(n: int) -> int { n } }; h.sort(1); 0 }\n").is_empty());
        // Revisión 1.27.91: un método de impl del usuario y una función de módulo (`pool::join`) también callan.
        assert!(warnings_of("struct T { n: int }\ntrait Sorter { fn sort(self); }\nimpl Sorter for T { fn sort(self) { print(self.n); } }\nfn main() -> int { let t = T { n: 1 }; t.sort(); 0 }\n").is_empty());
        let toks = crate::lexer::lex("fn main() -> int { let p = 1; pool.join(); 0 }").unwrap();
        let mut program = crate::parser::parse(toks).unwrap();
        program.functions.push(crate::ast::Function { name: "pool::join".to_string(), ..program.functions[0].clone() });
        assert!(discarded_results(&program).is_empty());
    }
}
