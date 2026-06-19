//! Parser (analizador sintáctico) de raylang.
//!
//! Segunda fase del pipeline (DESIGN.md §2): tokens → AST. Implementa la gramática
//! de DESIGN.md §7 con la técnica de *recursive descent* (descenso recursivo):
//! hay una función por cada regla de la gramática, y las reglas se llaman entre sí
//! reflejando la estructura del lenguaje.
//!
//! ## Precedencia por jerarquía
//!
//! La cadena `expression → logic_or → logic_and → equality → comparison → term →
//! factor → unary → call → primary` codifica la precedencia: cada nivel más
//! profundo amarra más fuerte. Así `1 + 2 * 3` se parsea como `1 + (2 * 3)` sin
//! ninguna tabla de precedencia explícita: el `*` vive en un nivel (`factor`) más
//! profundo que el `+` (`term`).
//!
//! ## Asociatividad
//!
//! Cada nivel usa un bucle `while` sobre sus operadores, construyendo el árbol
//! hacia la izquierda: `1 - 2 - 3` → `(1 - 2) - 3` (asociativo a la izquierda).

use crate::ast::*;
use crate::token::{Token, TokenKind};

/// Error sintáctico con ubicación.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error de sintaxis en {}:{}: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for ParseError {}

/// Conveniencia: parsea una lista de tokens (la salida del lexer) a un `Program`.
pub fn parse(tokens: Vec<Token>) -> Result<Program, ParseError> {
    Parser::new(tokens).parse_program()
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    // =================================================================
    // Reglas de la gramática
    // =================================================================

    /// program = { struct_def | function }
    pub fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut functions = Vec::new();
        let mut structs = Vec::new();
        while !self.is_at_end() {
            if self.check(&TokenKind::Struct) {
                structs.push(self.struct_def()?);
            } else {
                functions.push(self.function()?);
            }
        }
        Ok(Program { functions, structs })
    }

    /// struct_def = 'struct' IDENT '{' [ field { ',' field } [ ',' ] ] '}'
    /// field      = IDENT ':' type
    fn struct_def(&mut self) -> Result<StructDef, ParseError> {
        let kw = self.expect(&TokenKind::Struct, "'struct'")?;
        let (name, _, _) = self.expect_ident("el nombre del struct")?;
        self.expect(&TokenKind::LBrace, "'{' tras el nombre del struct")?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let (fname, _, _) = self.expect_ident("el nombre de un campo")?;
            self.expect(&TokenKind::Colon, "':' tras el nombre del campo")?;
            let ty = self.parse_type()?;
            fields.push((fname, ty));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace, "'}' para cerrar el struct")?;
        Ok(StructDef { name, fields, line: kw.line, col: kw.col })
    }

    /// function = 'fn' IDENT '(' [ params ] ')' [ '->' type ] block
    fn function(&mut self) -> Result<Function, ParseError> {
        let kw = self.expect(&TokenKind::Fn, "'fn'")?;
        let (name, _, _) = self.expect_ident("el nombre de la función")?;
        self.expect(&TokenKind::LParen, "'(' tras el nombre de la función")?;
        let params = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.params()?
        };
        self.expect(&TokenKind::RParen, "')'")?;

        // El tipo de retorno es opcional; ausente significa `unit`.
        let return_type = if self.eat(&TokenKind::Arrow) {
            self.parse_type()?
        } else {
            Type::Unit
        };

        let body = self.block()?;
        Ok(Function {
            name,
            params,
            return_type,
            body,
            line: kw.line,
            col: kw.col,
        })
    }

    /// params = param { ',' param } ;  param = IDENT ':' type
    fn params(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        loop {
            let (name, line, col) = self.expect_ident("el nombre de un parámetro")?;
            self.expect(&TokenKind::Colon, "':' tras el nombre del parámetro")?;
            let ty = self.parse_type()?;
            params.push(Param { name, ty, line, col });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(params)
    }

    /// type = 'int' | 'bool' | 'float' | 'string' | '[' type ']'
    fn parse_type(&mut self) -> Result<Type, ParseError> {
        // Arreglo: [T]
        if self.check(&TokenKind::LBracket) {
            self.advance();
            let elem = self.parse_type()?;
            self.expect(&TokenKind::RBracket, "']' para cerrar el tipo de arreglo")?;
            return Ok(Type::Array(Box::new(elem)));
        }
        // Nombre de struct (un identificador es un tipo).
        if let TokenKind::Ident(name) = self.peek_kind() {
            let name = name.clone();
            self.advance();
            return Ok(Type::Struct(name));
        }
        let ty = match self.peek_kind() {
            TokenKind::IntType => Type::Int,
            TokenKind::FloatType => Type::Float,
            TokenKind::BoolType => Type::Bool,
            TokenKind::StringType => Type::String,
            _ => return Err(self.error_here("se esperaba un tipo (int, float, bool, string, [T] o un struct)".into())),
        };
        self.advance();
        Ok(ty)
    }

    /// block = '{' { statement } [ expression ] '}'
    ///
    /// Aquí vive la regla de la orientación a expresiones (DESIGN.md §6): la
    /// expresión final **sin** `;` es el valor del bloque (`tail`); lo demás son
    /// sentencias.
    fn block(&mut self) -> Result<Block, ParseError> {
        let open = self.expect(&TokenKind::LBrace, "'{'")?;
        let mut statements = Vec::new();
        let mut tail: Option<Box<Expr>> = None;

        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            // Sentencias que empiezan con una palabra clave inequívoca.
            if self.check(&TokenKind::Let) || self.check(&TokenKind::Var) {
                statements.push(self.let_stmt()?);
                continue;
            }
            if self.check(&TokenKind::Return) {
                statements.push(self.return_stmt()?);
                continue;
            }
            // Parseamos una expresión. Puede ser el lado izquierdo de una
            // asignación, una sentencia-de-expresión, o el valor final del bloque.
            let expr = self.expression()?;
            let (line, col) = (expr.line, expr.col);

            // Asignación: `lvalue '=' value ';'`. Como `==` es otro token (EqEq),
            // `x == y` ya se consumió como expresión y no se confunde con esto.
            if self.eat(&TokenKind::Eq) {
                if !is_lvalue(&expr) {
                    return Err(self.error_here("el lado izquierdo de '=' no es asignable".into()));
                }
                let value = self.expression()?;
                self.expect(&TokenKind::Semicolon, "';' al final de la asignación")?;
                statements.push(Stmt { kind: StmtKind::Assign { target: expr, value }, line, col });
                continue;
            }

            let with_block = expr_has_block(&expr);
            if self.eat(&TokenKind::Semicolon) {
                // `expr ;` → sentencia de expresión (valor descartado).
                statements.push(Stmt { kind: StmtKind::Expr(expr), line, col });
            } else if self.check(&TokenKind::RBrace) {
                // `expr }` → es el valor del bloque.
                tail = Some(Box::new(expr));
                break;
            } else if with_block {
                // Expresión-con-bloque (if/while/{}) usada como sentencia: no
                // necesita `;` (DESIGN.md §6).
                statements.push(Stmt { kind: StmtKind::Expr(expr), line, col });
            } else {
                return Err(self.error_here("se esperaba ';' después de la expresión".into()));
            }
        }

        let close = self.expect(&TokenKind::RBrace, "'}' para cerrar el bloque")?;
        let _ = close;
        Ok(Block {
            statements,
            tail,
            line: open.line,
            col: open.col,
        })
    }

    /// letDecl = 'let' IDENT ':' type '=' expression ';'
    /// varDecl = 'var' ... (igual, pero mutable)
    fn let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.advance(); // 'let' o 'var'
        let mutable = kw.kind == TokenKind::Var;
        let (name, _, _) = self.expect_ident("el nombre de la variable")?;
        self.expect(&TokenKind::Colon, "':' (el tipo es obligatorio en M1)")?;
        let ty = self.parse_type()?;
        self.expect(&TokenKind::Eq, "'=' en la declaración")?;
        let value = self.expression()?;
        self.expect(&TokenKind::Semicolon, "';' al final de la declaración")?;
        Ok(Stmt {
            kind: StmtKind::Let { name, ty, value, mutable },
            line: kw.line,
            col: kw.col,
        })
    }

    /// returnStmt = 'return' [ expression ] ';'
    fn return_stmt(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.advance(); // 'return'
        let value = if self.check(&TokenKind::Semicolon) {
            None
        } else {
            Some(self.expression()?)
        };
        self.expect(&TokenKind::Semicolon, "';' al final del return")?;
        Ok(Stmt {
            kind: StmtKind::Return { value },
            line: kw.line,
            col: kw.col,
        })
    }

    // ----- Expresiones (por precedencia, de menor a mayor) -----

    /// expression = logic_or  (las expresiones-con-bloque se reconocen en `primary`)
    fn expression(&mut self) -> Result<Expr, ParseError> {
        self.logic_or()
    }

    /// logic_or = logic_and { '||' logic_and }
    fn logic_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.logic_and()?;
        while self.check(&TokenKind::PipePipe) {
            self.advance();
            let right = self.logic_and()?;
            left = make_binary(BinaryOp::Or, left, right);
        }
        Ok(left)
    }

    /// logic_and = equality { '&&' equality }
    fn logic_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.equality()?;
        while self.check(&TokenKind::AmpAmp) {
            self.advance();
            let right = self.equality()?;
            left = make_binary(BinaryOp::And, left, right);
        }
        Ok(left)
    }

    /// equality = comparison { ('==' | '!=') comparison }
    fn equality(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.comparison()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::EqEq => BinaryOp::Eq,
                TokenKind::BangEq => BinaryOp::Ne,
                _ => break,
            };
            self.advance();
            let right = self.comparison()?;
            left = make_binary(op, left, right);
        }
        Ok(left)
    }

    /// comparison = term { ('<' | '<=' | '>' | '>=') term }
    fn comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.term()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Lt => BinaryOp::Lt,
                TokenKind::LtEq => BinaryOp::Le,
                TokenKind::Gt => BinaryOp::Gt,
                TokenKind::GtEq => BinaryOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.term()?;
            left = make_binary(op, left, right);
        }
        Ok(left)
    }

    /// term = factor { ('+' | '-') factor }
    fn term(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.factor()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.factor()?;
            left = make_binary(op, left, right);
        }
        Ok(left)
    }

    /// factor = unary { ('*' | '/' | '%') unary }
    fn factor(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => BinaryOp::Mul,
                TokenKind::Slash => BinaryOp::Div,
                TokenKind::Percent => BinaryOp::Rem,
                _ => break,
            };
            self.advance();
            let right = self.unary()?;
            left = make_binary(op, left, right);
        }
        Ok(left)
    }

    /// unary = ('!' | '-') unary | call
    fn unary(&mut self) -> Result<Expr, ParseError> {
        let op = match self.peek_kind() {
            TokenKind::Bang => Some(UnaryOp::Not),
            TokenKind::Minus => Some(UnaryOp::Neg),
            _ => None,
        };
        if let Some(op) = op {
            let tok = self.advance();
            // Recursión a `unary` para permitir `--x`, `!!b`.
            let expr = self.unary()?;
            Ok(Expr {
                kind: ExprKind::Unary { op, expr: Box::new(expr) },
                line: tok.line,
                col: tok.col,
            })
        } else {
            self.call()
        }
    }

    /// call = primary { '(' [ args ] ')' | '[' expression ']' }
    ///
    /// Postfijos encadenables: llamadas `f(...)` e indexación `a[i]`. Así
    /// `f()[i]` o `a[i][j]` se parsean de izquierda a derecha.
    fn call(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.primary()?;
        loop {
            let (line, col) = (expr.line, expr.col);
            if self.check(&TokenKind::LParen) {
                self.advance(); // '('
                let args = if self.check(&TokenKind::RParen) {
                    Vec::new()
                } else {
                    self.args()?
                };
                self.expect(&TokenKind::RParen, "')' para cerrar la llamada")?;
                expr = Expr {
                    kind: ExprKind::Call { callee: Box::new(expr), args },
                    line,
                    col,
                };
            } else if self.check(&TokenKind::LBracket) {
                self.advance(); // '['
                let index = self.expression()?;
                self.expect(&TokenKind::RBracket, "']' para cerrar la indexación")?;
                expr = Expr {
                    kind: ExprKind::Index { array: Box::new(expr), index: Box::new(index) },
                    line,
                    col,
                };
            } else if self.check(&TokenKind::Dot) {
                self.advance(); // '.'
                let (name, _, _) = self.expect_ident("el nombre del campo tras '.'")?;
                expr = Expr {
                    kind: ExprKind::Field { object: Box::new(expr), name },
                    line,
                    col,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// args = expression { ',' expression }
    fn args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        loop {
            args.push(self.expression()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(args)
    }

    /// primary = literal | IDENT | '(' expression ')'
    ///         | ifExpr | whileExpr | block
    fn primary(&mut self) -> Result<Expr, ParseError> {
        // Expresiones-con-bloque: se pueden usar en cualquier posición de
        // expresión (p. ej. `let x: int = if (c) { 1 } else { 2 };`).
        match self.peek_kind() {
            TokenKind::If => return self.if_expr(),
            TokenKind::While => return self.while_expr(),
            TokenKind::LBrace => {
                let b = self.block()?;
                return Ok(Expr { line: b.line, col: b.col, kind: ExprKind::Block(b) });
            }
            TokenKind::LBracket => return self.array_literal(),
            _ => {}
        }

        let tok = self.advance();
        let kind = match tok.kind {
            TokenKind::Int(v) => ExprKind::Int(v),
            TokenKind::Float(v) => ExprKind::Float(v),
            TokenKind::Str(s) => ExprKind::Str(s),
            TokenKind::True => ExprKind::Bool(true),
            TokenKind::False => ExprKind::Bool(false),
            TokenKind::Ident(name) => {
                // `Nombre { ... }` es un literal de struct. (Las condiciones de
                // if/while van entre paréntesis, así que no hay ambigüedad con los
                // bloques.)
                if self.check(&TokenKind::LBrace) {
                    return self.struct_literal(name, tok.line, tok.col);
                }
                ExprKind::Ident(name)
            }
            TokenKind::LParen => {
                // Agrupación: el paréntesis no deja rastro en el AST, solo afecta
                // el orden de parseo. Conservamos la posición del '('.
                let inner = self.expression()?;
                self.expect(&TokenKind::RParen, "')' para cerrar el paréntesis")?;
                return Ok(Expr { kind: inner.kind, line: tok.line, col: tok.col });
            }
            other => {
                return Err(ParseError {
                    msg: format!("se esperaba una expresión, se encontró {:?}", other),
                    line: tok.line,
                    col: tok.col,
                })
            }
        };
        Ok(Expr { kind, line: tok.line, col: tok.col })
    }

    /// ifExpr = 'if' '(' expression ')' block [ 'else' ( block | ifExpr ) ]
    fn if_expr(&mut self) -> Result<Expr, ParseError> {
        let kw = self.expect(&TokenKind::If, "'if'")?;
        self.expect(&TokenKind::LParen, "'(' tras 'if'")?;
        let cond = self.expression()?;
        self.expect(&TokenKind::RParen, "')' tras la condición")?;
        let then_branch = self.block()?;

        let else_branch = if self.eat(&TokenKind::Else) {
            // El `else` puede ser un bloque o, encadenado, otro `if` (`else if`).
            let e = if self.check(&TokenKind::If) {
                self.if_expr()?
            } else {
                let b = self.block()?;
                Expr { line: b.line, col: b.col, kind: ExprKind::Block(b) }
            };
            Some(Box::new(e))
        } else {
            None
        };

        Ok(Expr {
            kind: ExprKind::If { cond: Box::new(cond), then_branch, else_branch },
            line: kw.line,
            col: kw.col,
        })
    }

    /// whileExpr = 'while' '(' expression ')' block
    fn while_expr(&mut self) -> Result<Expr, ParseError> {
        let kw = self.expect(&TokenKind::While, "'while'")?;
        self.expect(&TokenKind::LParen, "'(' tras 'while'")?;
        let cond = self.expression()?;
        self.expect(&TokenKind::RParen, "')' tras la condición")?;
        let body = self.block()?;
        Ok(Expr {
            kind: ExprKind::While { cond: Box::new(cond), body },
            line: kw.line,
            col: kw.col,
        })
    }

    /// arrayLiteral = '[' [ expression { ',' expression } ] ']'
    fn array_literal(&mut self) -> Result<Expr, ParseError> {
        let open = self.expect(&TokenKind::LBracket, "'['")?;
        let mut elems = Vec::new();
        if !self.check(&TokenKind::RBracket) {
            loop {
                elems.push(self.expression()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RBracket, "']' para cerrar el arreglo")?;
        Ok(Expr { kind: ExprKind::ArrayLit(elems), line: open.line, col: open.col })
    }

    /// structLiteral = IDENT '{' [ fieldInit { ',' fieldInit } [ ',' ] ] '}'
    /// fieldInit     = IDENT ':' expression
    fn struct_literal(&mut self, name: String, line: usize, col: usize) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let (fname, _, _) = self.expect_ident("el nombre de un campo")?;
            self.expect(&TokenKind::Colon, "':' tras el nombre del campo")?;
            let value = self.expression()?;
            fields.push((fname, value));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace, "'}' para cerrar el literal de struct")?;
        Ok(Expr { kind: ExprKind::StructLit { name, fields }, line, col })
    }

    // =================================================================
    // Primitivas del cursor de tokens
    // =================================================================

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    /// `true` si el token actual es exactamente `kind`. Solo se usa con variantes
    /// sin datos (palabras clave, signos), nunca con `Int`/`Ident`/etc.
    fn check(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    /// Consume y devuelve el token actual (clonado). No avanza más allá de `Eof`.
    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if !self.is_at_end() {
            self.pos += 1;
        }
        tok
    }

    /// Si el token actual es `kind`, lo consume y devuelve `true`.
    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume el token actual si es `kind`; si no, error describiendo qué se
    /// esperaba (`what`).
    fn expect(&mut self, kind: &TokenKind, what: &str) -> Result<Token, ParseError> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(self.error_here(format!("se esperaba {}", what)))
        }
    }

    /// Consume un identificador y devuelve `(nombre, línea, columna)`.
    fn expect_ident(&mut self, what: &str) -> Result<(String, usize, usize), ParseError> {
        let tok = self.peek().clone();
        if let TokenKind::Ident(name) = tok.kind {
            self.advance();
            Ok((name, tok.line, tok.col))
        } else {
            Err(self.error_here(format!("se esperaba {}", what)))
        }
    }

    /// Construye un error apuntando al token actual.
    fn error_here(&self, msg: String) -> ParseError {
        let t = self.peek();
        ParseError { msg, line: t.line, col: t.col }
    }
}

// ----- Auxiliares libres -----

/// Crea un nodo binario heredando la posición del operando izquierdo.
fn make_binary(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    let (line, col) = (left.line, left.col);
    Expr {
        kind: ExprKind::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        },
        line,
        col,
    }
}

/// ¿Es una expresión "con bloque" (if/while/{})? Determina si puede usarse como
/// sentencia sin `;` (DESIGN.md §6).
fn expr_has_block(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::If { .. } | ExprKind::While { .. } | ExprKind::Block(_)
    )
}

/// ¿Es una expresión a la que se puede asignar (un *lvalue*)? En M3.1, un nombre
/// o una indexación. El acceso a campo (`p.x`) se sumará en M3.2.
fn is_lvalue(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Ident(_) | ExprKind::Index { .. } | ExprKind::Field { .. })
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// Parsea una expresión suelta (para tests de precedencia/asociatividad).
    fn parse_expr(src: &str) -> Expr {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut p = Parser::new(tokens);
        p.expression().expect("parse de expresión ok")
    }

    /// Parsea un programa completo.
    fn parse_prog(src: &str) -> Program {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        parse(tokens).expect("parse ok")
    }

    /// Renderiza una expresión como S-expression para asertar su forma de forma
    /// compacta: `1 + 2 * 3` → `(+ 1 (* 2 3))`.
    fn sx(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Int(v) => v.to_string(),
            ExprKind::Float(v) => v.to_string(),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Str(s) => format!("{:?}", s),
            ExprKind::Ident(n) => n.clone(),
            ExprKind::Unary { op, expr } => format!("({} {})", uop(*op), sx(expr)),
            ExprKind::Binary { op, left, right } => {
                format!("({} {} {})", bop(*op), sx(left), sx(right))
            }
            ExprKind::Call { callee, args } => {
                let a: Vec<String> = args.iter().map(sx).collect();
                format!("(call {} [{}])", sx(callee), a.join(" "))
            }
            ExprKind::ArrayLit(elems) => {
                let e: Vec<String> = elems.iter().map(sx).collect();
                format!("[{}]", e.join(", "))
            }
            ExprKind::Index { array, index } => format!("(index {} {})", sx(array), sx(index)),
            ExprKind::StructLit { name, fields } => {
                let fs: Vec<String> = fields.iter().map(|(n, e)| format!("{}: {}", n, sx(e))).collect();
                format!("{} {{{}}}", name, fs.join(", "))
            }
            ExprKind::Field { object, name } => format!("(field {} {})", sx(object), name),
            ExprKind::If { cond, then_branch, else_branch } => {
                let els = else_branch
                    .as_ref()
                    .map(|b| sx(b))
                    .unwrap_or_else(|| "_".to_string());
                format!("(if {} {} {})", sx(cond), sblock(then_branch), els)
            }
            ExprKind::While { cond, body } => format!("(while {} {})", sx(cond), sblock(body)),
            ExprKind::Block(b) => sblock(b),
        }
    }

    fn sblock(b: &Block) -> String {
        let mut parts: Vec<String> = b.statements.iter().map(sstmt).collect();
        if let Some(t) = &b.tail {
            parts.push(sx(t));
        }
        format!("{{{}}}", parts.join("; "))
    }

    fn sstmt(s: &Stmt) -> String {
        match &s.kind {
            StmtKind::Let { name, value, mutable, .. } => {
                let kw = if *mutable { "var" } else { "let" };
                format!("{} {} = {}", kw, name, sx(value))
            }
            StmtKind::Assign { target, value } => format!("{} = {}", sx(target), sx(value)),
            StmtKind::Return { value } => match value {
                Some(v) => format!("return {}", sx(v)),
                None => "return".to_string(),
            },
            StmtKind::Expr(e) => sx(e),
        }
    }

    fn uop(op: UnaryOp) -> &'static str {
        match op {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
        }
    }

    fn bop(op: BinaryOp) -> &'static str {
        use BinaryOp::*;
        match op {
            Add => "+", Sub => "-", Mul => "*", Div => "/", Rem => "%",
            Eq => "==", Ne => "!=", Lt => "<", Le => "<=", Gt => ">", Ge => ">=",
            And => "&&", Or => "||",
        }
    }

    #[test]
    fn precedencia_multiplicacion_sobre_suma() {
        assert_eq!(sx(&parse_expr("1 + 2 * 3")), "(+ 1 (* 2 3))");
        assert_eq!(sx(&parse_expr("1 * 2 + 3")), "(+ (* 1 2) 3)");
    }

    #[test]
    fn asociatividad_izquierda() {
        assert_eq!(sx(&parse_expr("1 - 2 - 3")), "(- (- 1 2) 3)");
        assert_eq!(sx(&parse_expr("10 / 2 / 5")), "(/ (/ 10 2) 5)");
    }

    #[test]
    fn parentesis_cambian_el_orden() {
        assert_eq!(sx(&parse_expr("(1 + 2) * 3")), "(* (+ 1 2) 3)");
    }

    #[test]
    fn cadena_completa_de_precedencia() {
        // || más débil que &&, que < que ==, que comparación, que aritmética.
        assert_eq!(
            sx(&parse_expr("a || b && c == d < e + f")),
            "(|| a (&& b (== c (< d (+ e f)))))"
        );
    }

    #[test]
    fn unarios() {
        assert_eq!(sx(&parse_expr("-x + 1")), "(+ (- x) 1)");
        assert_eq!(sx(&parse_expr("!a && b")), "(&& (! a) b)");
        assert_eq!(sx(&parse_expr("--5")), "(- (- 5))");
    }

    #[test]
    fn llamadas_con_y_sin_argumentos() {
        assert_eq!(sx(&parse_expr("f()")), "(call f [])");
        assert_eq!(sx(&parse_expr("f(1, 2 + 3)")), "(call f [1 (+ 2 3)])");
        assert_eq!(sx(&parse_expr("g(h(x))")), "(call g [(call h [x])])");
    }

    #[test]
    fn arreglos_literal_e_indice() {
        assert_eq!(sx(&parse_expr("[1, 2, 3]")), "[1, 2, 3]");
        assert_eq!(sx(&parse_expr("[]")), "[]");
        assert_eq!(sx(&parse_expr("a[0]")), "(index a 0)");
        assert_eq!(sx(&parse_expr("a[i + 1]")), "(index a (+ i 1))");
        assert_eq!(sx(&parse_expr("m[0][1]")), "(index (index m 0) 1)");
    }

    #[test]
    fn asignacion_a_indice() {
        assert_eq!(
            sx(&parse_expr("{ a[0] = 9; a[0] }")),
            "{(index a 0) = 9; (index a 0)}"
        );
    }

    #[test]
    fn structs_literal_y_campo() {
        assert_eq!(sx(&parse_expr("Punto { x: 1, y: 2 }")), "Punto {x: 1, y: 2}");
        assert_eq!(sx(&parse_expr("p.x")), "(field p x)");
        assert_eq!(sx(&parse_expr("p.pos.x")), "(field (field p pos) x)");
        assert_eq!(sx(&parse_expr("a[0].x")), "(field (index a 0) x)");
    }

    #[test]
    fn if_como_expresion() {
        assert_eq!(
            sx(&parse_expr("if (x < 0) { -x } else { x }")),
            "(if (< x 0) {(- x)} {x})"
        );
        // if sin else: rama else es '_'
        assert_eq!(sx(&parse_expr("if (c) { 1 }")), "(if c {1} _)");
    }

    #[test]
    fn else_if_encadenado() {
        let s = sx(&parse_expr("if (a) { 1 } else if (b) { 2 } else { 3 }"));
        assert_eq!(s, "(if a {1} (if b {2} {3}))");
    }

    #[test]
    fn bloque_distingue_sentencias_de_valor_final() {
        // 'x = 1;' es sentencia; 'x + 1' (sin ';') es el valor del bloque.
        let s = sx(&parse_expr("{ var x: int = 0; x = 1; x + 1 }"));
        assert_eq!(s, "{var x = 0; x = 1; (+ x 1)}");
    }

    #[test]
    fn programa_fib_se_parsea() {
        let src = r#"
fn fib(n: int) -> int {
    if (n < 2) {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn main() -> int {
    var i: int = 0;
    while (i < 10) {
        print(fib(i));
        i = i + 1;
    }
    0
}
"#;
        let prog = parse_prog(src);
        assert_eq!(prog.functions.len(), 2);

        let fib = &prog.functions[0];
        assert_eq!(fib.name, "fib");
        assert_eq!(fib.params.len(), 1);
        assert_eq!(fib.params[0].name, "n");
        assert_eq!(fib.params[0].ty, Type::Int);
        assert_eq!(fib.return_type, Type::Int);
        // El cuerpo de fib es un único if-expresión como valor del bloque (sin return).
        assert!(fib.body.statements.is_empty());
        assert!(matches!(fib.body.tail.as_deref(), Some(Expr { kind: ExprKind::If { .. }, .. })));

        let main = &prog.functions[1];
        assert_eq!(main.name, "main");
        assert_eq!(main.return_type, Type::Int);
        // main: var, while, y el valor final 0.
        assert_eq!(main.body.statements.len(), 2);
        assert!(matches!(main.body.tail.as_deref(), Some(Expr { kind: ExprKind::Int(0), .. })));
    }

    #[test]
    fn errores_de_sintaxis() {
        let bad = |src: &str| {
            let tokens = crate::lexer::lex(src).expect("lex ok");
            parse(tokens)
        };
        // falta ';'
        assert!(bad("fn main() { let x: int = 1 }").is_err());
        // falta el tipo
        assert!(bad("fn main() { let x = 1; }").is_err());
        // paréntesis sin cerrar
        assert!(bad("fn main() { f(1 }").is_err());
        // falta el tipo de retorno tras '->'
        assert!(bad("fn main() -> { 0 }").is_err());
        // expresión incompleta
        assert!(bad("fn main() { 1 + }").is_err());
    }

    #[test]
    fn posiciones_se_propagan() {
        // El '+' hereda la posición de su operando izquierdo '1' (col 1).
        let e = parse_expr("1 + 2");
        assert_eq!((e.line, e.col), (1, 1));
    }
}
