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
    /// Contador para asignar un id único a cada función anónima (M4.1).
    next_fn_id: usize,
}

/// Parámetros de tipo y sus bounds (M9.2): `(nombres, pares (parámetro, trait))`.
type TypeParamsAndBounds = (Vec<String>, Vec<(String, String)>);

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0, next_fn_id: 0 }
    }

    // =================================================================
    // Reglas de la gramática
    // =================================================================

    /// program = { import | [anns] [pub] (struct_def | enum_def | trait_def | impl_block | function) }
    pub fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut functions = Vec::new();
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        let mut traits = Vec::new();
        let mut impls = Vec::new();
        let mut imports = Vec::new();
        let mut from_imports = Vec::new();
        while !self.is_at_end() {
            // M11.3: `import M;` precede a todo (sin anotaciones ni `pub`).
            if self.check(&TokenKind::Import) {
                imports.push(self.import_decl()?);
                continue;
            }
            // M11.3b: `from M import a [as b]{, …};` trae nombres al ámbito.
            if self.check(&TokenKind::From) {
                from_imports.push(self.import_from_decl()?);
                continue;
            }
            // M10.1: las anotaciones (`@nombre[(args)]`) preceden a la declaración.
            let anns = self.annotations()?;
            // M11.3: `pub` exporta el ítem. En M11.3a solo se admite ante una función
            // (tipos/enums/traits son globales por ahora).
            let pub_tok = if self.check(&TokenKind::Pub) { Some(self.advance()) } else { None };
            if self.check(&TokenKind::Struct) {
                let mut s = self.struct_def()?;
                s.annotations = anns;
                s.is_pub = pub_tok.is_some();
                structs.push(s);
            } else if self.check(&TokenKind::Enum) {
                let mut e = self.enum_def()?;
                e.annotations = anns;
                e.is_pub = pub_tok.is_some();
                enums.push(e);
            } else if self.check(&TokenKind::Trait) {
                self.no_annotations(&anns, "un trait")?;
                let mut t = self.trait_def()?;
                t.is_pub = pub_tok.is_some();
                traits.push(t);
            } else if self.check(&TokenKind::Impl) {
                self.no_pub(&pub_tok, "un impl")?;
                self.no_annotations(&anns, "un impl")?;
                impls.push(self.impl_block()?);
            } else {
                let mut f = self.function()?;
                f.annotations = anns;
                f.is_pub = pub_tok.is_some();
                functions.push(f);
            }
        }
        Ok(Program { functions, structs, enums, traits, impls, imports, from_imports })
    }

    /// import_decl = 'import' IDENT ';'  (M11.3)
    fn import_decl(&mut self) -> Result<ImportDecl, ParseError> {
        let kw = self.expect(&TokenKind::Import, "'import'")?;
        let (module, _, _) = self.expect_ident("el nombre del módulo a importar")?;
        self.expect(&TokenKind::Semicolon, "';' tras 'import M'")?;
        Ok(ImportDecl { module, line: kw.line, col: kw.col })
    }

    /// from_import_decl = 'from' IDENT 'import' name { ',' name } ';'   (M11.3b)
    /// name            = IDENT [ 'as' IDENT ]
    fn import_from_decl(&mut self) -> Result<FromImport, ParseError> {
        let kw = self.expect(&TokenKind::From, "'from'")?;
        let (module, _, _) = self.expect_ident("el nombre del módulo en 'from M import …'")?;
        self.expect(&TokenKind::Import, "'import' tras 'from M'")?;
        let mut names = Vec::new();
        loop {
            let (name, line, col) = self.expect_ident("un nombre a importar")?;
            let alias = if self.eat(&TokenKind::As) {
                Some(self.expect_ident("el alias tras 'as'")?.0)
            } else {
                None
            };
            names.push(ImportName { name, alias, line, col });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::Semicolon, "';' para cerrar el 'from M import …'")?;
        Ok(FromImport { module, names, line: kw.line, col: kw.col })
    }

    /// Error si hay `pub` donde no se admite (hoy: un `impl`, que no se exporta por sí mismo;
    /// se exporta el trait y el tipo).
    fn no_pub(&self, pub_tok: &Option<crate::token::Token>, donde: &str) -> Result<(), ParseError> {
        match pub_tok {
            None => Ok(()),
            Some(t) => Err(ParseError {
                msg: format!("'pub' no se admite en {} (exporta el trait/tipo, no el impl)", donde),
                line: t.line,
                col: t.col,
            }),
        }
    }

    /// Recoge las anotaciones que preceden a una declaración (M10.1):
    /// `{ '@' IDENT [ '(' IDENT { ',' IDENT } ')' ] }`. Devuelve `[]` si no hay ninguna.
    fn annotations(&mut self) -> Result<Vec<Annotation>, ParseError> {
        let mut anns = Vec::new();
        while self.check(&TokenKind::At) {
            let at = self.advance();
            let (name, _, _) = self.expect_ident("el nombre de la anotación tras '@'")?;
            let mut args = Vec::new();
            if self.eat(&TokenKind::LParen) {
                if !self.check(&TokenKind::RParen) {
                    loop {
                        let (a, _, _) = self.expect_ident("un argumento de la anotación")?;
                        args.push(a);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RParen, "')' para cerrar los argumentos de la anotación")?;
            }
            anns.push(Annotation { name, args, line: at.line, col: at.col });
        }
        Ok(anns)
    }

    /// Error si hay anotaciones donde M10.1 no las admite (trait/impl).
    fn no_annotations(&self, anns: &[Annotation], donde: &str) -> Result<(), ParseError> {
        match anns.first() {
            None => Ok(()),
            Some(a) => Err(ParseError {
                msg: format!("no se permiten anotaciones sobre {}", donde),
                line: a.line,
                col: a.col,
            }),
        }
    }

    /// enum_def = 'enum' IDENT '{' [ variant { ',' variant } [ ',' ] ] '}'
    /// variant  = IDENT [ '(' type { ',' type } ')' ]
    ///
    /// El payload entre paréntesis es **posicional** (M5): cero o más tipos. Sin
    /// paréntesis, la variante es *unit*.
    fn enum_def(&mut self) -> Result<EnumDef, ParseError> {
        let kw = self.expect(&TokenKind::Enum, "'enum'")?;
        let (name, _, _) = self.expect_ident("el nombre del enum")?;
        let type_params = self.type_params()?;
        self.expect(&TokenKind::LBrace, "'{' tras el nombre del enum")?;
        let mut variants = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let (vname, vline, vcol) = self.expect_ident("el nombre de una variante")?;
            let mut payload = Vec::new();
            if self.eat(&TokenKind::LParen) {
                // Lista de tipos del payload; al menos uno (un '()' vacío no aporta).
                loop {
                    payload.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "')' para cerrar el payload de la variante")?;
            }
            variants.push(VariantDef { name: vname, payload, line: vline, col: vcol });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace, "'}' para cerrar el enum")?;
        Ok(EnumDef { annotations: Vec::new(), is_pub: false, name, type_params, variants, line: kw.line, col: kw.col })
    }

    /// struct_def = 'struct' IDENT '{' [ field { ',' field } [ ',' ] ] '}'
    /// field      = IDENT ':' type
    fn struct_def(&mut self) -> Result<StructDef, ParseError> {
        let kw = self.expect(&TokenKind::Struct, "'struct'")?;
        let (name, _, _) = self.expect_ident("el nombre del struct")?;
        let type_params = self.type_params()?;
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
        Ok(StructDef { annotations: Vec::new(), is_pub: false, name, type_params, fields, line: kw.line, col: kw.col })
    }

    /// function = 'fn' IDENT [ '<' tparam { ',' tparam } '>' ] '(' [ params ] ')'
    ///            [ '->' type ] block
    /// tparam   = IDENT [ ':' IDENT { '+' IDENT } ]    (el bound, M9.2)
    fn function(&mut self) -> Result<Function, ParseError> {
        let kw = self.expect(&TokenKind::Fn, "'fn'")?;
        let (name, _, _) = self.expect_ident("el nombre de la función")?;
        let (type_params, bounds) = self.type_params_with_bounds()?;
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
            annotations: Vec::new(),
            is_pub: false, // lo fija `parse_program` si vino precedido de `pub`
            name,
            type_params,
            bounds,
            params,
            return_type,
            body,
            line: kw.line,
            col: kw.col,
        })
    }

    /// trait_def = 'trait' IDENT '{' { method_sig } '}'  (M9)
    ///
    /// Un trait lista **firmas** de métodos (sin cuerpo, terminadas en ';'). Los
    /// métodos por defecto (con cuerpo) se difieren a M9.3.
    fn trait_def(&mut self) -> Result<TraitDef, ParseError> {
        let kw = self.expect(&TokenKind::Trait, "'trait'")?;
        let (name, _, _) = self.expect_ident("el nombre del trait")?;
        self.expect(&TokenKind::LBrace, "'{' tras el nombre del trait")?;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            methods.push(self.method_sig()?);
        }
        self.expect(&TokenKind::RBrace, "'}' para cerrar el trait")?;
        Ok(TraitDef { is_pub: false, name, methods, line: kw.line, col: kw.col })
    }

    /// method_sig = 'fn' IDENT '(' [ method_params ] ')' [ '->' type ] ( ';' | block )  (M9)
    ///
    /// Termina en `;` (método **requerido**) o en un bloque (método **por defecto**,
    /// M9.3a): el cuerpo que un impl hereda si no lo redefine.
    fn method_sig(&mut self) -> Result<MethodSig, ParseError> {
        let kw = self.expect(&TokenKind::Fn, "'fn' en una firma de método")?;
        let (name, _, _) = self.expect_ident("el nombre del método")?;
        self.expect(&TokenKind::LParen, "'(' tras el nombre del método")?;
        let params = self.method_params()?;
        self.expect(&TokenKind::RParen, "')'")?;
        let return_type = if self.eat(&TokenKind::Arrow) {
            self.parse_type()?
        } else {
            Type::Unit
        };
        // ';' → requerido;  '{ ... }' → cuerpo por defecto (M9.3a).
        let default_body = if self.check(&TokenKind::LBrace) {
            Some(self.block()?)
        } else {
            self.expect(&TokenKind::Semicolon, "';' o un cuerpo '{ ... }' para el método")?;
            None
        };
        Ok(MethodSig { name, params, return_type, default_body, line: kw.line, col: kw.col })
    }

    /// impl_block = 'impl' IDENT 'for' type '{' { impl_method } '}'  (M9)
    ///
    /// `for` no es palabra clave reservada del lenguaje: se reconoce **contextualmente**
    /// aquí (como un identificador con ese nombre), para no quitarle el nombre al usuario.
    fn impl_block(&mut self) -> Result<ImplBlock, ParseError> {
        let kw = self.expect(&TokenKind::Impl, "'impl'")?;
        // Parámetros de tipo del impl (M9.2b): `impl<T: A + B> Trait for Caja<T>`. Sin `<`,
        // ambos quedan vacíos (impl concreto de M9.1).
        let (type_params, bounds) = self.type_params_with_bounds()?;
        let (trait_name, _, _) = self.expect_ident("el nombre del trait")?;
        let (kw_for, fline, fcol) = self.expect_ident("'for' tras el nombre del trait")?;
        if kw_for != "for" {
            return Err(ParseError {
                msg: format!("se esperaba 'for', no '{}'", kw_for),
                line: fline,
                col: fcol,
            });
        }
        let target = self.parse_type()?;
        self.expect(&TokenKind::LBrace, "'{' para abrir el cuerpo del impl")?;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            methods.push(self.impl_method()?);
        }
        self.expect(&TokenKind::RBrace, "'}' para cerrar el impl")?;
        Ok(ImplBlock { trait_name, type_params, bounds, target, methods, line: kw.line, col: kw.col })
    }

    /// impl_method = 'fn' IDENT '(' [ method_params ] ')' [ '->' type ] block  (M9)
    ///
    /// Como `function()` pero con `method_params` (admite `self`) y sin parámetros de
    /// tipo propios (los métodos genéricos se difieren a M9.2).
    fn impl_method(&mut self) -> Result<Function, ParseError> {
        let kw = self.expect(&TokenKind::Fn, "'fn' en un método de impl")?;
        let (name, _, _) = self.expect_ident("el nombre del método")?;
        self.expect(&TokenKind::LParen, "'(' tras el nombre del método")?;
        let params = self.method_params()?;
        self.expect(&TokenKind::RParen, "')'")?;
        let return_type = if self.eat(&TokenKind::Arrow) {
            self.parse_type()?
        } else {
            Type::Unit
        };
        let body = self.block()?;
        Ok(Function {
            annotations: Vec::new(),
            is_pub: false, // los métodos de impl no llevan `pub` propio
            name,
            type_params: Vec::new(),
            bounds: Vec::new(),
            params,
            return_type,
            body,
            line: kw.line,
            col: kw.col,
        })
    }

    /// Parámetros de un método (M9): como `params()`, pero admite un **primer**
    /// parámetro `self` sin anotación (su tipo es `Type::SelfType`, que el checker
    /// sustituye por el tipo implementador). El resto son parámetros normales.
    fn method_params(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if self.check(&TokenKind::RParen) {
            return Ok(params);
        }
        // ¿El primer parámetro es 'self'? (un IDENT con ese nombre, sin anotación).
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "self") {
            let tok = self.advance();
            params.push(Param {
                name: "self".into(),
                ty: Type::SelfType,
                line: tok.line,
                col: tok.col,
            });
            if !self.eat(&TokenKind::Comma) {
                return Ok(params);
            }
        }
        // Resto: param { ',' param }, igual que `params()`.
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

    /// Como `type_params`, pero cada parámetro puede llevar **bounds** (M9.2):
    /// `<T: Mostrable, U: A + B>`. Devuelve los nombres y los pares `(parámetro, trait)`.
    /// Solo lo usan las funciones; struct/enum siguen con `type_params` (sin bounds).
    fn type_params_with_bounds(&mut self) -> Result<TypeParamsAndBounds, ParseError> {
        let mut params = Vec::new();
        let mut bounds = Vec::new();
        if self.eat(&TokenKind::Lt) {
            loop {
                let (name, _, _) = self.expect_ident("el nombre de un parámetro de tipo")?;
                // Bound opcional: ': Trait { + Trait }'.
                if self.eat(&TokenKind::Colon) {
                    loop {
                        let (tr, _, _) = self.expect_ident("el nombre de un trait en el bound")?;
                        bounds.push((name.clone(), tr));
                        if !self.eat(&TokenKind::Plus) {
                            break;
                        }
                    }
                }
                params.push(name);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::Gt, "'>' para cerrar los parámetros de tipo")?;
        }
        Ok((params, bounds))
    }

    /// Lista opcional de parámetros de tipo: `< IDENT { ',' IDENT } >` (M6). Devuelve
    /// un `Vec` vacío si no hay `<`. Reusa los tokens `Lt`/`Gt` (en posición de tipo
    /// no hay ambigüedad con la comparación).
    fn type_params(&mut self) -> Result<Vec<String>, ParseError> {
        let mut params = Vec::new();
        if self.eat(&TokenKind::Lt) {
            loop {
                let (name, _, _) = self.expect_ident("el nombre de un parámetro de tipo")?;
                params.push(name);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::Gt, "'>' para cerrar los parámetros de tipo")?;
        }
        Ok(params)
    }

    /// Lista opcional de argumentos de tipo: `< type { ',' type } >` (M6). Como
    /// `type_params`, pero los elementos son **tipos** (`Caja<int>`, `Par<A, [int]>`),
    /// no nombres. Vacío si no hay `<`.
    fn type_args(&mut self) -> Result<Vec<Type>, ParseError> {
        let mut args = Vec::new();
        if self.eat(&TokenKind::Lt) {
            loop {
                args.push(self.parse_type()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::Gt, "'>' para cerrar los argumentos de tipo")?;
        }
        Ok(args)
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
    ///      | 'fn' '(' [ type { ',' type } ] ')' [ '->' type ]
    fn parse_type(&mut self) -> Result<Type, ParseError> {
        // Trait object: dyn Trait (M9.3b).
        if self.eat(&TokenKind::Dyn) {
            let (name, _, _) = self.expect_ident("el nombre del trait tras 'dyn'")?;
            return Ok(Type::Dyn(name));
        }
        // Arreglo: [T]
        if self.check(&TokenKind::LBracket) {
            self.advance();
            let elem = self.parse_type()?;
            self.expect(&TokenKind::RBracket, "']' para cerrar el tipo de arreglo")?;
            return Ok(Type::Array(Box::new(elem)));
        }
        // Tipo función: fn(T1, T2) -> R  (el '-> R' es opcional; ausente = unit).
        if self.check(&TokenKind::Fn) {
            self.advance();
            self.expect(&TokenKind::LParen, "'(' tras 'fn' en un tipo función")?;
            let mut params = Vec::new();
            if !self.check(&TokenKind::RParen) {
                loop {
                    params.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RParen, "')' para cerrar el tipo función")?;
            let ret = if self.eat(&TokenKind::Arrow) {
                self.parse_type()?
            } else {
                Type::Unit
            };
            return Ok(Type::Fn(params, Box::new(ret)));
        }
        // Nombre de struct/enum, con argumentos de tipo opcionales: `Caja<int>`.
        // (Un identificador suelto es `Struct(name, [])`; el checker lo reclasifica.)
        if let TokenKind::Ident(name) = self.peek_kind() {
            let name = name.clone();
            self.advance();
            let args = self.type_args()?;
            return Ok(Type::Struct(name, args));
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

    /// letDecl = 'let' IDENT [ ':' type ] '=' expression ';'
    /// varDecl = 'var' ... (igual, pero mutable)
    ///
    /// La anotación de tipo es **opcional** (M8.1): si se omite, el checker infiere el
    /// tipo del inicializador.
    fn let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.advance(); // 'let' o 'var'
        let mutable = kw.kind == TokenKind::Var;
        let (name, _, _) = self.expect_ident("el nombre de la variable")?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
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

    /// expression = pipeline  (las expresiones-con-bloque se reconocen en `primary`)
    fn expression(&mut self) -> Result<Expr, ParseError> {
        self.pipeline()
    }

    /// pipeline = logic_or { '|>' call }
    ///
    /// Azúcar de M7.2: `x |> f(a)` ≡ `f(x, a)` —el receptor se inserta como **primer
    /// argumento**—; `x |> f` (sin paréntesis) ≡ `f(x)`. Es la precedencia **más baja**
    /// y **asociativo a la izquierda**: `x |> f |> g` ≡ `g(f(x))`. Se desazucara aquí
    /// mismo a un `Call` ordinario, así que el checker y los dos motores no ven `|>`.
    ///
    /// El operando derecho se parsea a nivel de `call` (un objetivo de llamada: `f`,
    /// `f(args)`, `m.f(args)`), no una expresión completa: para operar sobre el
    /// resultado de un pipeline hay que parentizar (`(x |> f) + 1`).
    fn pipeline(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.logic_or()?;
        while self.check(&TokenKind::PipeArrow) {
            self.advance(); // '|>'
            let rhs = self.call()?;
            left = make_pipeline(left, rhs);
        }
        Ok(left)
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
            } else if self.check(&TokenKind::Question) {
                self.advance(); // '?' postfijo (propagación de errores, M6.3)
                expr = Expr {
                    kind: ExprKind::Try(Box::new(expr)),
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
            // `fn(...) { ... }` en posición de expresión es una función anónima.
            // No hay ambigüedad: la `fn` de nivel superior lleva nombre.
            TokenKind::Fn => return self.fn_expr(),
            TokenKind::Match => return self.match_expr(),
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

    /// fnExpr = 'fn' '(' [ params ] ')' [ '->' type ] block   (M4.1)
    ///
    /// Una función anónima. Reutiliza `params()` y `block()` de la función nombrada.
    fn fn_expr(&mut self) -> Result<Expr, ParseError> {
        let kw = self.expect(&TokenKind::Fn, "'fn'")?;
        // El id se asigna en pre-orden: una fn-expr exterior recibe un id menor que
        // las anidadas en su cuerpo. Eso da ids densos 0..n.
        let id = self.next_fn_id;
        self.next_fn_id += 1;

        self.expect(&TokenKind::LParen, "'(' tras 'fn'")?;
        let params = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.params()?
        };
        self.expect(&TokenKind::RParen, "')'")?;
        let return_type = if self.eat(&TokenKind::Arrow) {
            self.parse_type()?
        } else {
            Type::Unit
        };
        let body = self.block()?;
        Ok(Expr {
            kind: ExprKind::Func(Box::new(FnExpr {
                id,
                params,
                return_type,
                body,
                line: kw.line,
                col: kw.col,
            })),
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

    /// matchExpr = 'match' '(' expression ')' '{' [ arm { ',' arm } [ ',' ] ] '}'
    /// arm       = pattern '=>' expression
    ///
    /// El escrutinio va entre paréntesis (como las condiciones de if/while): evita la
    /// ambigüedad con el literal de struct `Nombre { ... }` y es consistente.
    fn match_expr(&mut self) -> Result<Expr, ParseError> {
        let kw = self.expect(&TokenKind::Match, "'match'")?;
        self.expect(&TokenKind::LParen, "'(' tras 'match'")?;
        let scrutinee = self.expression()?;
        self.expect(&TokenKind::RParen, "')' tras la expresión de match")?;
        self.expect(&TokenKind::LBrace, "'{' para abrir los brazos del match")?;
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            arms.push(self.match_arm()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace, "'}' para cerrar el match")?;
        Ok(Expr {
            kind: ExprKind::Match { scrutinee: Box::new(scrutinee), arms },
            line: kw.line,
            col: kw.col,
        })
    }

    /// Un brazo: `patrón => expresión`.
    fn match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let pattern = self.pattern()?;
        let (line, col) = (pattern.line, pattern.col);
        self.expect(&TokenKind::FatArrow, "'=>' tras el patrón")?;
        let body = self.expression()?;
        Ok(MatchArm { pattern, body, line, col })
    }

    /// pattern = '_' | IDENT | IDENT '.' IDENT [ '(' subpat { ',' subpat } ')' ]
    /// subpat  = '_' | IDENT
    ///
    /// Como las variantes van **cualificadas** (`Enum.Variante`), no hay ambigüedad:
    /// un identificador seguido de `.` es una variante; uno suelto, un binding; `_`,
    /// el comodín.
    fn pattern(&mut self) -> Result<Pattern, ParseError> {
        let (name, line, col) = self.expect_ident("un patrón (variante, nombre o '_')")?;
        // Comodín.
        if name == "_" {
            return Ok(Pattern { kind: PatternKind::Wildcard, line, col });
        }
        // Variante cualificada: `Enum.Variante[(sub-bindings)]`.
        if self.eat(&TokenKind::Dot) {
            let (variant, _, _) = self.expect_ident("el nombre de la variante")?;
            let mut bindings = Vec::new();
            if self.eat(&TokenKind::LParen) {
                loop {
                    let (b, _, _) = self.expect_ident("un sub-patrón (nombre o '_')")?;
                    bindings.push(if b == "_" { None } else { Some(b) });
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "')' para cerrar el patrón de variante")?;
            }
            return Ok(Pattern {
                kind: PatternKind::Variant { enum_name: name, variant, bindings },
                line,
                col,
            });
        }
        // Identificador suelto: binding catch-all.
        Ok(Pattern { kind: PatternKind::Binding(name), line, col })
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
                // Coma final permitida (`[1, 2, 3,]`), como en los campos de struct.
                if self.check(&TokenKind::RBracket) {
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

/// Desazucara `recv |> rhs` (M7.2): inserta `recv` como **primer argumento** de la
/// llamada `rhs`. Si `rhs` ya es una llamada `f(args)`, el resultado es `f(recv, args)`;
/// si es cualquier otra expresión llamable `f`, es `f(recv)`. El nodo resultante es un
/// `Call` ordinario, posicionado en el `rhs` (donde está la función llamada).
fn make_pipeline(recv: Expr, rhs: Expr) -> Expr {
    let (line, col) = (rhs.line, rhs.col);
    let kind = match rhs.kind {
        ExprKind::Call { callee, mut args } => {
            let mut new_args = Vec::with_capacity(args.len() + 1);
            new_args.push(recv);
            new_args.append(&mut args);
            ExprKind::Call { callee, args: new_args }
        }
        other => ExprKind::Call {
            callee: Box::new(Expr { kind: other, line, col }),
            args: vec![recv],
        },
    };
    Expr { kind, line, col }
}

/// ¿Es una expresión "con bloque" (if/while/{})? Determina si puede usarse como
/// sentencia sin `;` (DESIGN.md §6).
fn expr_has_block(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::If { .. } | ExprKind::While { .. } | ExprKind::Block(_) | ExprKind::Match { .. }
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

    #[test]
    fn from_import_se_parsea_con_alias_y_varios_nombres() {
        let prog = parse_prog("from mates import doble, triple as tri;\nfn main() -> int { 0 }\n");
        assert_eq!(prog.from_imports.len(), 1);
        let fi = &prog.from_imports[0];
        assert_eq!(fi.module, "mates");
        assert_eq!(fi.names.len(), 2);
        assert_eq!(fi.names[0].name, "doble");
        assert_eq!(fi.names[0].alias, None);
        assert_eq!(fi.names[0].local(), "doble");
        assert_eq!(fi.names[1].name, "triple");
        assert_eq!(fi.names[1].alias.as_deref(), Some("tri"));
        assert_eq!(fi.names[1].local(), "tri");
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
            ExprKind::EnumLit { enum_name, variant, args } => {
                let a: Vec<String> = args.iter().map(sx).collect();
                format!("(enum {}.{} [{}])", enum_name, variant, a.join(" "))
            }
            ExprKind::Func(fe) => {
                let ps: Vec<String> = fe.params.iter().map(|p| p.name.clone()).collect();
                format!("(fn [{}] {})", ps.join(" "), sblock(&fe.body))
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                let els = else_branch
                    .as_ref()
                    .map(|b| sx(b))
                    .unwrap_or_else(|| "_".to_string());
                format!("(if {} {} {})", sx(cond), sblock(then_branch), els)
            }
            ExprKind::While { cond, body } => format!("(while {} {})", sx(cond), sblock(body)),
            ExprKind::Block(b) => sblock(b),
            ExprKind::Match { scrutinee, arms } => {
                let a: Vec<String> = arms.iter().map(|arm| format!("{} => {}", spat(&arm.pattern), sx(&arm.body))).collect();
                format!("(match {} [{}])", sx(scrutinee), a.join(", "))
            }
            ExprKind::Try(inner) => format!("(try {})", sx(inner)),
        }
    }

    /// Renderiza un patrón de forma compacta para los tests.
    fn spat(p: &Pattern) -> String {
        match &p.kind {
            PatternKind::Wildcard => "_".to_string(),
            PatternKind::Binding(n) => n.clone(),
            PatternKind::Variant { enum_name, variant, bindings } => {
                let bs: Vec<String> = bindings.iter().map(|b| b.clone().unwrap_or_else(|| "_".to_string())).collect();
                if bs.is_empty() {
                    format!("{}.{}", enum_name, variant)
                } else {
                    format!("{}.{}({})", enum_name, variant, bs.join(", "))
                }
            }
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
    fn operador_try_se_parsea() {
        // `?` es postfijo y se encadena con llamadas/campos.
        assert_eq!(sx(&parse_expr("f(x)?")), "(try (call f [x]))");
        assert_eq!(sx(&parse_expr("a?.b")), "(field (try a) b)");
    }

    #[test]
    fn funcion_generica_se_parsea() {
        let prog = parse_prog("fn mapear<T, U>(xs: [T], f: fn(T) -> U) -> [U] { xs } fn main() {}");
        let f = &prog.functions[0];
        assert_eq!(f.type_params, vec!["T".to_string(), "U".to_string()]);
        // El parser deja los parámetros de tipo como Struct(name); el checker los
        // reclasifica a Var. Aquí solo comprobamos que se parsean en su lugar.
        assert_eq!(f.params[0].ty, Type::Array(Box::new(Type::Struct("T".into(), vec![]))));
        assert_eq!(f.return_type, Type::Array(Box::new(Type::Struct("U".into(), vec![]))));
        // Una función sin <...> no tiene parámetros de tipo.
        assert!(prog.functions[1].type_params.is_empty());
    }

    #[test]
    fn enum_se_parsea() {
        // Variantes con payload posicional, con un solo tipo y unit; coma final ok.
        let prog = parse_prog("enum Figura { Circulo(float), Rect(float, float), Punto, } fn main() {}");
        assert_eq!(prog.enums.len(), 1);
        let e = &prog.enums[0];
        assert_eq!(e.name, "Figura");
        assert_eq!(e.variants.len(), 3);
        assert_eq!(e.variants[0].name, "Circulo");
        assert_eq!(e.variants[0].payload, vec![Type::Float]);
        assert_eq!(e.variants[1].payload, vec![Type::Float, Type::Float]);
        assert_eq!(e.variants[2].payload, Vec::<Type>::new()); // unit
    }

    #[test]
    fn match_se_parsea() {
        // Escrutinio entre paréntesis; patrones de variante con bindings, comodín y
        // binding suelto; coma final permitida.
        assert_eq!(
            sx(&parse_expr("match (xs) { Lista.Cons(h, t) => h, Lista.Nil => 0, }")),
            "(match xs [Lista.Cons(h, t) => h, Lista.Nil => 0])"
        );
        assert_eq!(
            sx(&parse_expr("match (f) { Figura.Punto => 0, _ => 1 }")),
            "(match f [Figura.Punto => 0, _ => 1])"
        );
        assert_eq!(
            sx(&parse_expr("match (e) { E.A(_, x) => x, otra => 9 }")),
            "(match e [E.A(_, x) => x, otra => 9])"
        );
    }

    #[test]
    fn funcion_anonima_se_parsea() {
        assert_eq!(sx(&parse_expr("fn(x: int) -> int { x + 1 }")), "(fn [x] {(+ x 1)})");
        assert_eq!(sx(&parse_expr("fn() { print(1); }")), "(fn [] {(call print [1])})");
        // Llamarla directamente: Call sobre una expresión Func.
        assert_eq!(sx(&parse_expr("(fn(x: int) -> int { x })(3)")), "(call (fn [x] {x}) [3])");
    }

    #[test]
    fn tipo_funcion_se_parsea() {
        let prog = parse_prog("fn aplica(f: fn(int) -> int, x: int) -> int { f(x) } fn main() {}");
        let f = &prog.functions[0];
        assert_eq!(f.params[0].ty, Type::Fn(vec![Type::Int], Box::new(Type::Int)));
        // fn sin '->' es retorno unit.
        let prog2 = parse_prog("fn t(c: fn()) { } fn main() {}");
        assert_eq!(prog2.functions[0].params[0].ty, Type::Fn(vec![], Box::new(Type::Unit)));
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
        // 'let x = 1;' (sin anotación) es VÁLIDO desde M8.1; pero ':' sin tipo no
        assert!(bad("fn main() { let x: = 1; }").is_err());
        // falta el '=' / inicializador
        assert!(bad("fn main() { let x; }").is_err());
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

    #[test]
    fn coma_final_en_arreglo() {
        // Limpieza: `[1, 2, 3,]` ahora se acepta (como la coma final en structs).
        assert_eq!(sx(&parse_expr("[1, 2, 3,]")), "[1, 2, 3]");
        assert_eq!(sx(&parse_expr("[1,]")), "[1]");
        // El arreglo vacío sigue válido y la doble coma sigue siendo error.
        assert_eq!(sx(&parse_expr("[]")), "[]");
    }

    // ----- M8.1: anotación de tipo opcional en let/var -----

    #[test]
    fn let_con_y_sin_anotacion() {
        let prog = parse_prog("fn main() { let a: int = 1; let b = 2; var c = 3; }");
        let stmts = &prog.functions[0].body.statements;
        let ty_of = |s: &Stmt| match &s.kind {
            StmtKind::Let { ty, .. } => ty.clone(),
            _ => panic!("se esperaba un let"),
        };
        assert!(ty_of(&stmts[0]).is_some(), "'let a: int' lleva anotación");
        assert!(ty_of(&stmts[1]).is_none(), "'let b' no lleva anotación");
        assert!(ty_of(&stmts[2]).is_none(), "'var c' no lleva anotación");
    }

    // ----- M7.2: pipelines (desugaring a Call) -----

    #[test]
    fn pipeline_inserta_receptor_como_primer_arg() {
        // `x |> f` ≡ f(x); `x |> f(a)` ≡ f(x, a).
        assert_eq!(sx(&parse_expr("x |> f")), "(call f [x])");
        assert_eq!(sx(&parse_expr("x |> f(a)")), "(call f [x a])");
        assert_eq!(sx(&parse_expr("x |> f(a, b)")), "(call f [x a b])");
    }

    #[test]
    fn pipeline_es_asociativo_a_la_izquierda() {
        // `x |> f |> g` ≡ g(f(x)).
        assert_eq!(sx(&parse_expr("x |> f |> g")), "(call g [(call f [x])])");
        assert_eq!(
            sx(&parse_expr("x |> f(a) |> g(b)")),
            "(call g [(call f [x a]) b])"
        );
    }

    #[test]
    fn pipeline_tiene_precedencia_minima() {
        // El operando izquierdo es una expresión completa: `2 + 3 |> f` ≡ f(2 + 3).
        assert_eq!(sx(&parse_expr("2 + 3 |> f")), "(call f [(+ 2 3)])");
        // Para operar sobre el resultado hay que parentizar.
        assert_eq!(sx(&parse_expr("(x |> f) + 1")), "(+ (call f [x]) 1)");
    }

    #[test]
    fn pipeline_compone_con_ufcs() {
        // `.f()` (UFCS, se resuelve en el checker) y `|> f` (pipeline, aquí) conviven.
        assert_eq!(
            sx(&parse_expr("x.doble() |> inc")),
            "(call inc [(call (field x doble) [])])"
        );
    }

    // ----- M9: traits -----

    #[test]
    fn parse_trait_y_impl() {
        let prog = parse_prog(r#"
            trait Mostrable { fn mostrar(self) -> string; fn n(self, k: int) -> int; }
            struct Punto { x: int }
            impl Mostrable for Punto {
                fn mostrar(self) -> string { "p" }
                fn n(self, k: int) -> int { k }
            }
            fn main() -> int { 0 }
        "#);
        // El trait quedó con dos firmas; la primera tiene un solo parámetro: self.
        assert_eq!(prog.traits.len(), 1);
        let t = &prog.traits[0];
        assert_eq!(t.name, "Mostrable");
        assert_eq!(t.methods.len(), 2);
        assert_eq!(t.methods[0].params.len(), 1);
        assert_eq!(t.methods[0].params[0].name, "self");
        assert_eq!(t.methods[0].params[0].ty, Type::SelfType);
        // El impl apunta a Punto y replica los métodos como funciones con cuerpo.
        assert_eq!(prog.impls.len(), 1);
        let im = &prog.impls[0];
        assert_eq!(im.trait_name, "Mostrable");
        assert_eq!(im.target, Type::Struct("Punto".into(), vec![]));
        assert_eq!(im.methods.len(), 2);
        assert_eq!(im.methods[1].params[0].ty, Type::SelfType);
        assert_eq!(im.methods[1].params[1].name, "k");
    }

    #[test]
    fn impl_sin_for_es_error() {
        let tokens = crate::lexer::lex("impl T S { } fn main() -> int { 0 }").expect("lex ok");
        let err = parse(tokens).expect_err("falta 'for'");
        assert!(err.msg.contains("se esperaba 'for'"), "mensaje: {}", err.msg);
    }

    #[test]
    fn parse_anotaciones() {
        let prog = parse_prog(r#"
            @test
            fn t() -> bool { true }

            @derive(Eq)
            struct P { x: int }

            fn main() -> int { 0 }
        "#);
        let t = prog.functions.iter().find(|f| f.name == "t").unwrap();
        assert_eq!(t.annotations.len(), 1);
        assert_eq!(t.annotations[0].name, "test");
        assert!(t.annotations[0].args.is_empty());
        let p = &prog.structs[0];
        assert_eq!(p.annotations[0].name, "derive");
        assert_eq!(p.annotations[0].args, vec!["Eq".to_string()]);
    }

    #[test]
    fn parse_anotacion_sobre_impl_es_error() {
        let tokens = crate::lexer::lex("@test\ntrait T { fn f(self) -> int; }").expect("lex ok");
        let err = parse(tokens).expect_err("anotación sobre trait");
        assert!(err.msg.contains("no se permiten anotaciones"), "mensaje: {}", err.msg);
    }

    #[test]
    fn parse_dyn_trait_object() {
        let prog = parse_prog(r#"
            trait Figura { fn area(self) -> int; }
            fn f(x: dyn Figura) -> int { 0 }
            fn main() -> int { 0 }
        "#);
        // El parámetro x tiene tipo Type::Dyn("Figura").
        let f = &prog.functions[0];
        assert_eq!(f.params[0].ty, Type::Dyn("Figura".to_string()));
    }

    #[test]
    fn parse_metodo_por_defecto() {
        let prog = parse_prog(r#"
            trait T {
                fn req(self) -> int;
                fn opt(self) -> int { 42 }
            }
            fn main() -> int { 0 }
        "#);
        let t = &prog.traits[0];
        assert_eq!(t.methods.len(), 2);
        assert!(t.methods[0].default_body.is_none(), "req es requerido (sin cuerpo)");
        assert!(t.methods[1].default_body.is_some(), "opt tiene cuerpo por defecto");
    }

    #[test]
    fn parse_bounds_de_genericos() {
        let prog = parse_prog(r#"
            fn f<T: Mostrable, U: A + B>(x: T, y: U) -> int { 0 }
            fn main() -> int { 0 }
        "#);
        let f = &prog.functions[0];
        assert_eq!(f.type_params, vec!["T", "U"]);
        // Bounds en orden: (T, Mostrable), (U, A), (U, B).
        assert_eq!(
            f.bounds,
            vec![
                ("T".to_string(), "Mostrable".to_string()),
                ("U".to_string(), "A".to_string()),
                ("U".to_string(), "B".to_string()),
            ]
        );
    }
}
