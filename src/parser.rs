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
use crate::token::{InterpPart, Token, TokenKind};

/// Error sintáctico con ubicación. `len` (M33a) es la extensión en caracteres del
/// token ofensor — el renderizador de diagnósticos subraya el lexema completo. No
/// entra en el `Display` (la cabecera no cambia).
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
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

/// Variante acumuladora (M33c): parsea recuperándose de los errores — ante uno, lo guarda,
/// **resincroniza** al próximo ítem top-level (`sync_item`) y sigue. Devuelve el programa
/// **parcial** (los ítems que sí parsearon) y todos los errores (hasta `MAX_ERRORES`). El
/// primer error es **idéntico** al de `parse` (mismo recorrido hasta ahí). Es la variante
/// de *diagnóstico* (LSP/CLI); el camino de ejecución sigue usando `parse` (fail-fast).
pub fn parse_all(tokens: Vec<Token>) -> (Program, Vec<ParseError>) {
    Parser::new(tokens).parse_program_all()
}

/// Tope de errores acumulados (M33c): la cascada tras una recuperación imperfecta no inunda.
const MAX_ERRORES: usize = 20;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Contador para asignar un id único a cada función anónima (M4.1).
    next_fn_id: usize,
    /// Si está activo, un `Nombre { … }` NO se parsea como literal de struct (M27.2): en la cabecera de
    /// un `for` (sin paréntesis) el `{` abre el cuerpo del bucle, no un struct. Igual que Rust.
    no_struct_lit: bool,
    /// Spans de expresiones (M33a-2): inicio → fin (exclusivo) del último token consumido,
    /// registrados en `expression()` con política max-end. Al terminar pasan al `Program`.
    expr_spans: std::collections::HashMap<(usize, usize), (usize, usize)>,
    /// Posición del nombre en un acceso `recv.name` (M10.2g): `(línea, col, nombre)` del acceso →
    /// `(línea, col)` del `name` tras el `.`. Al terminar pasa al `Program` (para el hover del LSP).
    field_name_pos: std::collections::HashMap<(usize, usize, String), Vec<(usize, usize)>>,
    /// Azúcar preservado para el formateador (M29.3): forma de superficie de interpolación y pipelines,
    /// indexada por la posición del nodo desazucarado raíz. Al terminar pasan al `Program`. Ver
    /// [`crate::ast::Program::interp_sites`].
    interp_sites: std::collections::HashMap<(usize, usize), Vec<InterpSeg>>,
    pipe_sites: std::collections::HashMap<(usize, usize), (Expr, Expr)>,
    /// Profundidad de recursión actual (M33d): la incrementan los tres puntos recursivos
    /// (`expression`/`parse_type`/`block`); al pasar `MAX_PARSE_DEPTH` se corta con un
    /// `ParseError` — sin esto, un `((((…` hostil desborda la pila y ABORTA el proceso
    /// (un stack overflow ni siquiera es capturable como ICE).
    depth: usize,
}

/// Límite de anidamiento del parser (M33d), espejo del `MAX_CALL_DEPTH` del runtime
/// (M13.3a): ~2.6 KB de pila por nivel → 1000 niveles caben holgados incluso en un hilo
/// estándar de 8 MB. Ningún programa legítimo anida 1000 niveles.
const MAX_PARSE_DEPTH: usize = 1000;

/// Parámetros de tipo y sus bounds (M9.2): `(nombres, pares (parámetro, trait))`.
type TypeParamsAndBounds = (Vec<String>, Vec<(String, String)>);

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0, next_fn_id: 0, no_struct_lit: false, expr_spans: std::collections::HashMap::new(), field_name_pos: std::collections::HashMap::new(), interp_sites: std::collections::HashMap::new(), pipe_sites: std::collections::HashMap::new(), depth: 0 }
    }

    // =================================================================
    // Reglas de la gramática
    // =================================================================

    /// program = { import | [anns] [pub] (struct_def | enum_def | trait_def | impl_block | function) }
    pub fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut acc = Program::default();
        while !self.is_at_end() {
            self.parse_item(&mut acc)?;
        }
        acc.expr_spans = std::mem::take(&mut self.expr_spans);
        acc.field_name_pos = std::mem::take(&mut self.field_name_pos);
        acc.interp_sites = std::mem::take(&mut self.interp_sites);
        acc.pipe_sites = std::mem::take(&mut self.pipe_sites);
        Ok(acc)
    }

    /// La variante acumuladora de `parse_program` (M33c): un error se guarda y se
    /// **resincroniza** al próximo ítem; el programa devuelto es parcial.
    pub fn parse_program_all(&mut self) -> (Program, Vec<ParseError>) {
        let mut acc = Program::default();
        let mut errores = Vec::new();
        while !self.is_at_end() {
            let antes = self.pos;
            if let Err(e) = self.parse_item(&mut acc) {
                errores.push(e);
                if errores.len() >= MAX_ERRORES {
                    break;
                }
                if self.pos == antes {
                    self.advance(); // progreso garantizado: nunca reintentar el mismo token
                }
                self.sync_item();
            }
        }
        acc.expr_spans = std::mem::take(&mut self.expr_spans);
        acc.field_name_pos = std::mem::take(&mut self.field_name_pos);
        acc.interp_sites = std::mem::take(&mut self.interp_sites);
        acc.pipe_sites = std::mem::take(&mut self.pipe_sites);
        (acc, errores)
    }

    /// Resincronización (M33c): avanza hasta el próximo arranque plausible de ítem
    /// top-level. El contador de llaves **relativo** exige ≤ 0: un error dentro de un
    /// cuerpo salta el cuerpo entero antes de anclar (un `fn` anónimo en medio de un
    /// cuerpo roto aún puede anclar de más — heurística honesta, acotada por el tope).
    fn sync_item(&mut self) {
        let mut depth: i64 = 0;
        while !self.is_at_end() {
            match &self.peek().kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => depth -= 1,
                k if depth <= 0
                    && matches!(
                        k,
                        TokenKind::Fn
                            | TokenKind::Struct
                            | TokenKind::Enum
                            | TokenKind::Trait
                            | TokenKind::Impl
                            | TokenKind::Pub
                            | TokenKind::Import
                            | TokenKind::From
                            | TokenKind::Const
                            | TokenKind::Extern
                            | TokenKind::At
                    ) =>
                {
                    return;
                }
                _ => {}
            }
            self.advance();
        }
    }

    /// Parsea UN ítem de nivel superior y lo agrega al programa. Extraído del bucle de
    /// `parse_program` (M33c) para que el fail-fast y el acumulador compartan el código.
    fn parse_item(&mut self, acc: &mut Program) -> Result<(), ParseError> {
        // M11.3: `import M;` precede a todo (sin anotaciones ni `pub`).
        if self.check(&TokenKind::Import) {
            acc.imports.push(self.import_decl()?);
            return Ok(());
        }
        // M11.3b: `from M import a [as b]{, …};` trae nombres al ámbito.
        if self.check(&TokenKind::From) {
            acc.from_imports.push(self.import_from_decl(false)?);
            return Ok(());
        }
        // M11.6a: `pub from M import …;` reexporta (construye la cara pública de un `mod.ray`).
        if self.check(&TokenKind::Pub) && self.check_next(&TokenKind::From) {
            self.advance(); // 'pub'
            acc.from_imports.push(self.import_from_decl(true)?);
            return Ok(());
        }
        // M41: `extern "lib" { fn … }` declara funciones C (FFI). Sin anotaciones ni `pub`.
        if self.check(&TokenKind::Extern) {
            self.extern_block(acc)?;
            return Ok(());
        }
        // M10.1: las anotaciones (`@nombre[(args)]`) preceden a la declaración.
        let anns = self.annotations()?;
        // M11.3: `pub` exporta el ítem. En M11.3a solo se admite ante una función
        // (tipos/enums/traits son globales por ahora).
        let pub_tok = if self.check(&TokenKind::Pub) { Some(self.advance()) } else { None };
        if self.check(&TokenKind::Const) {
            // M27.5: `const NAME: T = <literal>;`.
            self.no_annotations(&anns, "una constante")?;
            let kw = self.advance();
            let (name, _, _) = self.expect_ident("el nombre de la constante")?;
            self.expect(&TokenKind::Colon, "':' tras el nombre de la constante")?;
            let ty = self.parse_type()?;
            self.expect(&TokenKind::Eq, "'=' en la constante")?;
            let value = self.expression()?;
            self.expect(&TokenKind::Semicolon, "';' al final de la constante")?;
            acc.consts.push(ConstDef { name, ty, value, is_pub: pub_tok.is_some(), line: kw.line, col: kw.col });
        } else if self.check(&TokenKind::Struct) {
            let mut s = self.struct_def()?;
            s.annotations = anns;
            s.is_pub = pub_tok.is_some();
            acc.structs.push(s);
        } else if self.check(&TokenKind::Enum) {
            let mut e = self.enum_def()?;
            e.annotations = anns;
            e.is_pub = pub_tok.is_some();
            acc.enums.push(e);
        } else if self.check(&TokenKind::Trait) {
            self.no_annotations(&anns, "un trait")?;
            let mut t = self.trait_def()?;
            t.is_pub = pub_tok.is_some();
            acc.traits.push(t);
        } else if self.check(&TokenKind::Impl) {
            self.no_pub(&pub_tok, "un impl")?;
            self.no_annotations(&anns, "un impl")?;
            acc.impls.push(self.impl_block()?);
        } else {
            let mut f = self.function()?;
            f.annotations = anns;
            f.is_pub = pub_tok.is_some();
            acc.functions.push(f);
        }
        Ok(())
    }

    /// import_decl  = 'import' module_path [ 'as' IDENT ] ';'   (M11.3 / M11.5)
    /// module_path  = IDENT { '/' IDENT }                        (ruta de directorios; M11.5)
    fn import_decl(&mut self) -> Result<ImportDecl, ParseError> {
        let kw = self.expect(&TokenKind::Import, "'import'")?;
        let (module, _, _) = self.module_path()?;
        let alias = if self.eat(&TokenKind::As) {
            Some(self.expect_ident("el alias tras 'as'")?.0)
        } else {
            None
        };
        self.expect(&TokenKind::Semicolon, "';' tras 'import …'")?;
        Ok(ImportDecl { module, alias, line: kw.line, col: kw.col })
    }

    /// Lee una **ruta de módulo** `a/b/c` (M11.5): uno o más identificadores separados por `/`.
    /// Dentro de un `import`/`from` no hay división, así que `/` se reinterpreta sin ambigüedad como
    /// separador de directorios. Devuelve la ruta unida con `/` y la posición del primer segmento.
    fn module_path(&mut self) -> Result<(String, usize, usize), ParseError> {
        let (first, line, col) = self.expect_ident("el nombre del módulo a importar")?;
        let mut path = first;
        while self.eat(&TokenKind::Slash) {
            let (seg, _, _) = self.expect_ident("un segmento de la ruta tras '/'")?;
            path.push('/');
            path.push_str(&seg);
        }
        Ok((path, line, col))
    }

    /// from_import_decl = [ 'pub' ] 'from' module_path 'import' name { ',' name } ';'  (M11.3b/.5/.6a)
    /// name            = IDENT [ 'as' IDENT ]
    /// `is_pub` (M11.6a): el `pub` ya lo consumió el bucle de `parse_program` (reexport).
    fn import_from_decl(&mut self, is_pub: bool) -> Result<FromImport, ParseError> {
        let kw = self.expect(&TokenKind::From, "'from'")?;
        let (module, _, _) = self.module_path()?;
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
        Ok(FromImport { module, names, is_pub, line: kw.line, col: kw.col })
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
                len: t.len,
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
                len: 1, // la anotación no guarda su extensión; el '@' basta
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
        let (type_params, bounds) = self.type_params_with_bounds()?; // M9.4: bounds en `<T: Trait>`
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
        Ok(EnumDef { annotations: Vec::new(), is_pub: false, name, type_params, bounds, variants, line: kw.line, col: kw.col })
    }

    /// struct_def = 'struct' IDENT '{' [ field { ',' field } [ ',' ] ] '}'
    /// field      = IDENT ':' type
    fn struct_def(&mut self) -> Result<StructDef, ParseError> {
        let kw = self.expect(&TokenKind::Struct, "'struct'")?;
        let (name, _, _) = self.expect_ident("el nombre del struct")?;
        let (type_params, bounds) = self.type_params_with_bounds()?; // M9.4: bounds en `<T: Trait>`
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
        Ok(StructDef { annotations: Vec::new(), is_pub: false, name, type_params, bounds, fields, line: kw.line, col: kw.col })
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

    /// extern_block = 'extern' STRING '{' { extern_sig } '}'   (M41, FFI)
    /// extern_sig   = 'fn' IDENT '(' [ params ] ')' [ '->' type ] ';'
    ///
    /// Declara funciones C de una librería. Cada firma va sin cuerpo (termina en `;`); su nombre es a
    /// la vez el identificador en raylang y el símbolo a resolver con `dlsym`. El nombre de la librería
    /// es el string literal tras `extern` (`extern "m"` → `libm`).
    fn extern_block(&mut self, acc: &mut Program) -> Result<(), ParseError> {
        self.expect(&TokenKind::Extern, "'extern'")?;
        let lib_tok = self.advance();
        let lib = match &lib_tok.kind {
            TokenKind::Str(s) => s.clone(),
            _ => return Err(ParseError {
                msg: "se esperaba el nombre de la librería como string tras 'extern' (p. ej. extern \"m\")".into(),
                line: lib_tok.line, col: lib_tok.col, len: 1,
            }),
        };
        self.expect(&TokenKind::LBrace, "'{' tras extern \"lib\"")?;
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            let kw = self.expect(&TokenKind::Fn, "'fn' en el bloque extern")?;
            let (name, _, _) = self.expect_ident("el nombre de la función externa")?;
            self.expect(&TokenKind::LParen, "'(' tras el nombre de la función externa")?;
            let params = if self.check(&TokenKind::RParen) { Vec::new() } else { self.params()? };
            self.expect(&TokenKind::RParen, "')'")?;
            let return_type = if self.eat(&TokenKind::Arrow) { self.parse_type()? } else { Type::Unit };
            self.expect(&TokenKind::Semicolon, "';' al final de la firma externa (no lleva cuerpo)")?;
            acc.externs.push(ExternFn { name, lib: lib.clone(), params, return_type, line: kw.line, col: kw.col });
        }
        self.expect(&TokenKind::RBrace, "'}' para cerrar el bloque extern")?;
        Ok(())
    }

    /// trait_def = 'trait' IDENT '{' { method_sig } '}'  (M9)
    ///
    /// Un trait lista **firmas** de métodos (sin cuerpo, terminadas en ';'). Los
    /// métodos por defecto (con cuerpo) se difieren a M9.3.
    fn trait_def(&mut self) -> Result<TraitDef, ParseError> {
        let kw = self.expect(&TokenKind::Trait, "'trait'")?;
        let (name, _, _) = self.expect_ident("el nombre del trait")?;
        // M28.2: parámetros de tipo del trait, `trait From<S>`. Sin `<`, vacío (trait de M9).
        let (type_params, _) = self.type_params_with_bounds()?;
        self.expect(&TokenKind::LBrace, "'{' tras el nombre del trait")?;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            methods.push(self.method_sig()?);
        }
        self.expect(&TokenKind::RBrace, "'}' para cerrar el trait")?;
        Ok(TraitDef { is_pub: false, name, type_params, methods, line: kw.line, col: kw.col })
    }

    /// method_sig = 'fn' IDENT '(' [ method_params ] ')' [ '->' type ] ( ';' | block )  (M9)
    ///
    /// Termina en `;` (método **requerido**) o en un bloque (método **por defecto**,
    /// M9.3a): el cuerpo que un impl hereda si no lo redefine.
    fn method_sig(&mut self) -> Result<MethodSig, ParseError> {
        let kw = self.expect(&TokenKind::Fn, "'fn' en una firma de método")?;
        let (name, _, _) = self.expect_ident("el nombre del método")?;
        // M40.2c: parámetros de tipo propios del método, `fn map<U>(self, ...)`. Sin `<`, vacío.
        let (type_params, bounds) = self.type_params_with_bounds()?;
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
        Ok(MethodSig { name, type_params, bounds, params, return_type, default_body, line: kw.line, col: kw.col })
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
        // M28.2: argumentos de tipo del trait, `impl From<string> for E`. Sin `<`, vacío.
        let trait_args = self.type_args()?;
        // `for` es palabra clave desde M27.2 (antes era un identificador aquí). Se conserva el mensaje de
        // error original para que el oráculo del parser auto-alojado (que aún trata `for` como ident) cuadre.
        if self.check(&TokenKind::For) {
            self.advance();
        } else {
            let tok = self.peek();
            let (fline, fcol, flen) = (tok.line, tok.col, tok.len);
            let found = match &tok.kind {
                TokenKind::Ident(n) => n.clone(),
                other => format!("{:?}", other),
            };
            return Err(ParseError { msg: format!("se esperaba 'for', no '{}'", found), line: fline, col: fcol, len: flen });
        }
        let target = self.parse_type()?;
        self.expect(&TokenKind::LBrace, "'{' para abrir el cuerpo del impl")?;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            methods.push(self.impl_method()?);
        }
        self.expect(&TokenKind::RBrace, "'}' para cerrar el impl")?;
        Ok(ImplBlock { trait_name, trait_args, type_params, bounds, target, methods, line: kw.line, col: kw.col })
    }

    /// impl_method = 'fn' IDENT [ '<' type_params '>' ] '(' [ method_params ] ')' [ '->' type ] block
    ///
    /// Como `function()` pero con `method_params` (admite `self`). M40.2c: admite parámetros de
    /// tipo propios del método (`fn map<U>(self, ...)`), para casar con la firma del trait.
    fn impl_method(&mut self) -> Result<Function, ParseError> {
        let kw = self.expect(&TokenKind::Fn, "'fn' en un método de impl")?;
        let (name, _, _) = self.expect_ident("el nombre del método")?;
        let (type_params, bounds) = self.type_params_with_bounds()?;
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
            type_params,
            bounds,
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
            self.close_type_angle("'>' para cerrar los parámetros de tipo")?;
        }
        Ok((params, bounds))
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
            self.close_type_angle("'>' para cerrar los argumentos de tipo")?;
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
        self.enter()?;
        let t = self.parse_type_inner();
        self.depth -= 1;
        t
    }

    fn parse_type_inner(&mut self) -> Result<Type, ParseError> {
        // Trait object: dyn A [+ B + ...] (M9.3b / M9.5). El conjunto se guarda canónico
        // (ordenado y sin duplicados) para que `dyn A + B` y `dyn B + A` sean el mismo tipo.
        if self.eat(&TokenKind::Dyn) {
            let mut traits = Vec::new();
            loop {
                let (name, _, _) = self.expect_ident("el nombre del trait tras 'dyn'")?;
                traits.push(name);
                if !self.eat(&TokenKind::Plus) {
                    break;
                }
            }
            traits.sort();
            traits.dedup();
            return Ok(Type::Dyn(traits));
        }
        // Arreglo: [T]
        if self.check(&TokenKind::LBracket) {
            self.advance();
            let elem = self.parse_type()?;
            self.expect(&TokenKind::RBracket, "']' para cerrar el tipo de arreglo")?;
            return Ok(Type::Array(Box::new(elem)));
        }
        // Tupla: (T1, T2, …). Un `(T)` de un solo elemento es solo un paréntesis → el tipo interior.
        if self.check(&TokenKind::LParen) {
            self.advance();
            let mut elems = Vec::new();
            if !self.check(&TokenKind::RParen) {
                loop {
                    elems.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                    if self.check(&TokenKind::RParen) {
                        break; // coma final permitida
                    }
                }
            }
            self.expect(&TokenKind::RParen, "')' para cerrar el tipo tupla")?;
            if elems.len() == 1 {
                return Ok(elems.into_iter().next().unwrap_or_else(|| crate::ice!("tupla de 1 elemento sin elemento")));
            }
            return Ok(Type::Tuple(elems));
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
            let mut name = name.clone();
            self.advance();
            // Tipo calificado por módulo: `M.Tipo` (M11.3c-3). El `.` se guarda **en el nombre**;
            // el loader lo resuelve a `M::Tipo` (validando `import M;` + `pub`). Sin loader (REPL,
            // un solo archivo) un `M.Tipo` no resuelve y el checker lo rechaza.
            if self.eat(&TokenKind::Dot) {
                let (ty, _, _) = self.expect_ident("el nombre del tipo tras 'M.'")?;
                name = format!("{}.{}", name, ty);
            }
            let args = self.type_args()?;
            return Ok(Type::Struct(name, args));
        }
        let ty = match self.peek_kind() {
            TokenKind::IntType => Type::Int,
            TokenKind::FloatType => Type::Float,
            TokenKind::BoolType => Type::Bool,
            TokenKind::StringType => Type::String,
            TokenKind::CharType => Type::Char,
            TokenKind::BytesType => Type::Bytes,
            TokenKind::PtrType => Type::Ptr,
            TokenKind::UIntType(w) => Type::UInt(*w),
            // Nota: el mensaje conserva su texto original (sin u8/u32/u64) para no romper el oráculo del
            // parser auto-alojado (`selfhost/parser.ray`), que aún no conoce los enteros con tamaño.
            _ => return Err(self.error_here("se esperaba un tipo (int, float, bool, string, char, bytes, [T] o un struct)".into())),
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
        self.enter()?;
        let b = self.block_inner();
        self.depth -= 1;
        b
    }

    fn block_inner(&mut self) -> Result<Block, ParseError> {
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
            if self.check(&TokenKind::For) {
                statements.push(self.for_stmt()?);
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
        Ok(Block {
            statements,
            tail,
            line: open.line,
            col: open.col,
            end_line: close.line,
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
        // Desestructuración de tupla: `let (a, b) = e;` (M27.1). `_` descarta una posición.
        if self.check(&TokenKind::LParen) {
            self.advance();
            let mut names = Vec::new();
            loop {
                let (n, _, _) = self.expect_ident("un nombre en la desestructuración")?;
                names.push(if n == "_" { None } else { Some(n) }); // `_` descarta esa posición
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RParen) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "')' para cerrar la desestructuración")?;
            self.expect(&TokenKind::Eq, "'=' en la desestructuración")?;
            let value = self.expression()?;
            self.expect(&TokenKind::Semicolon, "';' al final de la declaración")?;
            return Ok(Stmt {
                kind: StmtKind::LetTuple { names, value, mutable },
                line: kw.line,
                col: kw.col,
            });
        }
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

    /// Baja una cadena interpolada (M27.3) a la concatenación `"lit" + to_string(expr) + …`. Cada
    /// fragmento de expresión se re-lexea (**en su posición real**, con `lex_at`) y re-parsea como una
    /// expresión completa, así el sub-AST lleva posiciones de la fuente → el LSP da hover dentro de
    /// `${…}`. Las partes literales usan la posición de la cadena.
    fn build_interp(&mut self, parts: Vec<InterpPart>, line: usize, col: usize) -> Result<Expr, ParseError> {
        let mut acc: Option<Expr> = None;
        // M29.3: en paralelo al AST desazucarado, se guardan los segmentos de superficie (texto + exprs
        // ya parseadas) para que el formateador reemita `"…${e}…"` sin perderla.
        let mut segs: Vec<InterpSeg> = Vec::with_capacity(parts.len());
        for part in parts {
            let piece = match part {
                InterpPart::Lit(s) => {
                    segs.push(InterpSeg::Lit(s.clone()));
                    Expr { kind: ExprKind::Str(s), line, col }
                }
                InterpPart::Expr(src, el, ec) => {
                    // Se re-lexa el fragmento ARRANCANDO en su posición real `(el, ec)`, así el
                    // sub-AST nace con posiciones de la fuente (no las de la cadena) → el LSP da
                    // hover sobre los identificadores dentro de `${…}`.
                    let toks = crate::lexer::lex_at(&src, el, ec)
                        .map_err(|e| ParseError { msg: format!("en la interpolación: {}", e.msg), line: el, col: ec, len: 1 })?;
                    let mut sub = Parser::new(toks);
                    let e = sub.expression()
                        .map_err(|e| ParseError { msg: format!("en la interpolación: {}", e.msg), line: el, col: ec, len: 1 })?;
                    if !sub.is_at_end() {
                        return Err(ParseError { msg: "la interpolación debe ser una sola expresión".into(), line: el, col: ec, len: 1 });
                    }
                    // El azúcar de una expresión anidada (interpolación/pipeline dentro de `${…}`) vive en
                    // el sub-parser → se fusiona para no perderlo al formatear.
                    self.interp_sites.extend(std::mem::take(&mut sub.interp_sites));
                    self.pipe_sites.extend(std::mem::take(&mut sub.pipe_sites));
                    segs.push(InterpSeg::Expr(e.clone()));
                    // to_string(e) — convierte primitivos/string a texto para concatenar.
                    let callee = Expr { kind: ExprKind::Ident("to_string".into()), line: el, col: ec };
                    Expr { kind: ExprKind::Call { callee: Box::new(callee), args: vec![e] }, line: el, col: ec }
                }
            };
            acc = Some(match acc {
                None => piece,
                Some(prev) => Expr {
                    kind: ExprKind::Binary { op: BinaryOp::Add, left: Box::new(prev), right: Box::new(piece) },
                    line,
                    col,
                },
            });
        }
        let root = acc.unwrap_or_else(|| crate::ice!("una cadena interpolada llegó sin partes del lexer"));
        // Indexado por la posición del nodo RAÍZ desazucarado (el propio `to_string` si es un solo `${e}`,
        // o el `+` externo si hay literales). El formateador lo consulta ahí para reemitir la cadena.
        self.interp_sites.insert((root.line, root.col), segs);
        Ok(root)
    }

    /// Parsea una expresión con el literal de struct desactivado (para la cabecera del `for`, M27.2).
    fn expr_no_struct(&mut self) -> Result<Expr, ParseError> {
        let saved = self.no_struct_lit;
        self.no_struct_lit = true;
        let r = self.expression();
        self.no_struct_lit = saved;
        r
    }

    /// forStmt = 'for' ( IDENT | '(' names ')' ) 'in' ( expr '..' expr | expr ) block   (M27.2)
    fn for_stmt(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.advance(); // 'for'
        // Patrón: un nombre, o una tupla `(k, v)` para iterar un Map.
        let pat = if self.check(&TokenKind::LParen) {
            self.advance();
            let mut names = Vec::new();
            loop {
                let (n, _, _) = self.expect_ident("un nombre en el patrón del for")?;
                names.push(if n == "_" { None } else { Some(n) });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RParen) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "')' para cerrar el patrón del for")?;
            ForPat::Tuple(names)
        } else {
            let (n, _, _) = self.expect_ident("el nombre de la variable del for")?;
            ForPat::Single(n)
        };
        self.expect(&TokenKind::In, "'in' en el for")?;
        // El iterable. La condición del bloque va sin paréntesis (como if/while), así que el literal de
        // struct está prohibido aquí (mismo compromiso): se usa `no_struct` en la expresión.
        let start = self.expr_no_struct()?;
        let iter = if self.eat(&TokenKind::DotDot) {
            let end = self.expr_no_struct()?;
            ForIter::Range { start, end }
        } else {
            ForIter::In(start)
        };
        let body = self.block()?;
        Ok(Stmt {
            kind: StmtKind::For { pat, iter, body },
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
    ///
    /// M33a-2: aquí —la puerta por la que pasa toda expresión de usuario— se registra su
    /// **span**: inicio `(line, col)` del nodo → fin (exclusivo) del último token consumido.
    /// Política *max-end*: si dos expresiones comparten inicio (un binario hereda la posición
    /// de su operando izquierdo), queda la más ancha. El checker lo consulta en `err()`.
    fn expression(&mut self) -> Result<Expr, ParseError> {
        self.enter()?;
        let e = self.pipeline()?;
        self.depth -= 1;
        let end = self.prev_end();
        self.expr_spans
            .entry((e.line, e.col))
            .and_modify(|old| { if end > *old { *old = end; } })
            .or_insert(end);
        Ok(e)
    }

    /// Entra un nivel de recursión (M33d); corta con error si se pasa del límite. En un
    /// `Err` propagado no se decrementa: el parser es fail-fast y el parseo termina ahí.
    fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            return Err(self.error_here(format!(
                "anidamiento demasiado profundo (límite: {MAX_PARSE_DEPTH} niveles)"
            )));
        }
        Ok(())
    }

    /// El fin (exclusivo) del último token consumido: `(línea, col + len)`.
    fn prev_end(&self) -> (usize, usize) {
        let t = &self.tokens[self.pos.saturating_sub(1)];
        (t.line, t.col + t.len)
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
            // M29.3: guardar la forma de superficie `(receptor, rhs)` para el formateador, indexada por
            // la posición del `Call` desazucarado que produce `make_pipeline` (la de `rhs`). El receptor
            // puede ser a su vez un pipeline (encadenado) → su clave está también en la tabla.
            let clave = (rhs.line, rhs.col);
            let (recv_sfc, rhs_sfc) = (left.clone(), rhs.clone());
            left = make_pipeline(left, rhs);
            self.pipe_sites.insert(clave, (recv_sfc, rhs_sfc));
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

    /// logic_and = bit_or { '&&' bit_or }
    fn logic_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.bit_or()?;
        while self.check(&TokenKind::AmpAmp) {
            self.advance();
            let right = self.bit_or()?;
            left = make_binary(BinaryOp::And, left, right);
        }
        Ok(left)
    }

    // Operadores bit a bit (M19.3a). Precedencia estándar (estilo C), de menor a mayor:
    // '|' (bit_or) < '^' (bit_xor) < '&' (bit_and), todos por debajo de la igualdad. Así
    // `a & b == c` agrupa como `a & (b == c)` (compatible con C), y `flags & MASK` corriente.
    /// bit_or = bit_xor { '|' bit_xor }
    fn bit_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.bit_xor()?;
        while self.check(&TokenKind::Pipe) {
            self.advance();
            let right = self.bit_xor()?;
            left = make_binary(BinaryOp::BitOr, left, right);
        }
        Ok(left)
    }

    /// bit_xor = bit_and { '^' bit_and }
    fn bit_xor(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.bit_and()?;
        while self.check(&TokenKind::Caret) {
            self.advance();
            let right = self.bit_and()?;
            left = make_binary(BinaryOp::BitXor, left, right);
        }
        Ok(left)
    }

    /// bit_and = equality { '&' equality }
    fn bit_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.equality()?;
        while self.check(&TokenKind::Amp) {
            self.advance();
            let right = self.equality()?;
            left = make_binary(BinaryOp::BitAnd, left, right);
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

    /// comparison = shift { ('<' | '<=' | '>' | '>=') shift }
    fn comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.shift()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Lt => BinaryOp::Lt,
                TokenKind::LtEq => BinaryOp::Le,
                TokenKind::Gt => BinaryOp::Gt,
                TokenKind::GtEq => BinaryOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.shift()?;
            left = make_binary(op, left, right);
        }
        Ok(left)
    }

    /// shift = term { ('<<' | '>>') term }  (M19.3a)
    ///
    /// `shift` está por debajo de `comparison` y por encima de `term` (aritmética), así que
    /// el shift agrupa MÁS FLOJO que `+`/`-`: `a + b << c` es `(a + b) << c` y `a << 2 + 1`
    /// es `a << (2 + 1)`. Es la precedencia de C (aditivo < shift), elegida para que el
    /// framing de WS (`hi << 8 | lo`) lea natural junto con `|` aún más flojo.
    fn shift(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.term()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Shl => BinaryOp::Shl,
                TokenKind::Shr => BinaryOp::Shr,
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
        let mut left = self.cast()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => BinaryOp::Mul,
                TokenKind::Slash => BinaryOp::Div,
                TokenKind::Percent => BinaryOp::Rem,
                _ => break,
            };
            self.advance();
            let right = self.cast()?;
            left = make_binary(op, left, right);
        }
        Ok(left)
    }

    /// cast = unary { 'as' type }   — conversión numérica (M27.4). Liga más fuerte que los binarios,
    /// más débil que los unarios (como Rust): `-x as float` = `(-x) as float`.
    fn cast(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.unary()?;
        while self.check(&TokenKind::As) {
            let tok = self.advance();
            let ty = self.parse_type()?;
            expr = Expr { kind: ExprKind::Cast { expr: Box::new(expr), ty }, line: tok.line, col: tok.col };
        }
        Ok(expr)
    }

    /// unary = ('!' | '-') unary | call
    fn unary(&mut self) -> Result<Expr, ParseError> {
        let op = match self.peek_kind() {
            TokenKind::Bang => Some(UnaryOp::Not),
            TokenKind::Minus => Some(UnaryOp::Neg),
            TokenKind::Tilde => Some(UnaryOp::BitNot), // ~ (NOT bit a bit, M19.3a)
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
                // Acceso a tupla `t.0`: tras el '.' viene un número, no un identificador (M27.1).
                if let TokenKind::Int(n) = self.peek().kind {
                    let (nl, nc) = (self.peek().line, self.peek().col);
                    self.advance();
                    self.field_name_pos.entry((line, col, n.to_string())).or_default().push((nl, nc));
                    expr = Expr {
                        kind: ExprKind::Field { object: Box::new(expr), name: n.to_string() },
                        line,
                        col,
                    };
                    continue;
                }
                let (name, nl, nc) = self.expect_ident("el nombre del campo tras '.'")?;
                // Posición del nombre del campo/método, para el hover del LSP (M10.2g).
                self.field_name_pos.entry((line, col, name.clone())).or_default().push((nl, nc));
                // `M.Tipo { ... }`: literal de struct calificado por módulo (M11.3c-3). Solo si el
                // receptor del `.` es un `Ident` (el módulo); el nombre calificado guarda el `.`,
                // que el loader resuelve a `M::Tipo`. (Mismo compromiso struct-literal-vs-bloque
                // que `Tipo { ... }`: las condiciones de if/while/match van entre paréntesis.)
                if let (true, false, ExprKind::Ident(m)) = (self.check(&TokenKind::LBrace), self.no_struct_lit, &expr.kind) {
                    let qualified = format!("{}.{}", m, name);
                    expr = self.struct_literal(qualified, line, col)?;
                    continue;
                }
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
            // M27.3: cadena interpolada `"...${expr}..."` → concatenación con `to_string` de cada expresión.
            TokenKind::InterpStr(parts) => return self.build_interp(parts, tok.line, tok.col),
            TokenKind::Char(c) => ExprKind::Char(c),
            TokenKind::Bytes(b) => ExprKind::Bytes(b),
            TokenKind::True => ExprKind::Bool(true),
            TokenKind::False => ExprKind::Bool(false),
            TokenKind::Ident(name) => {
                // `Nombre { ... }` es un literal de struct. (Las condiciones de
                // if/while van entre paréntesis, así que no hay ambigüedad con los
                // bloques.) En la cabecera de un `for` el flag lo desactiva (M27.2).
                if self.check(&TokenKind::LBrace) && !self.no_struct_lit {
                    return self.struct_literal(name, tok.line, tok.col);
                }
                ExprKind::Ident(name)
            }
            TokenKind::LParen => {
                // Agrupación `(e)` o literal de **tupla** `(e1, e2, …)` (M27.1). Un solo elemento sin coma
                // es agrupación (no deja rastro en el AST); con 2+ es una tupla.
                let first = self.expression()?;
                if self.eat(&TokenKind::Comma) {
                    let mut elems = vec![first];
                    if !self.check(&TokenKind::RParen) {
                        loop {
                            elems.push(self.expression()?);
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                            if self.check(&TokenKind::RParen) {
                                break; // coma final permitida
                            }
                        }
                    }
                    self.expect(&TokenKind::RParen, "')' para cerrar la tupla")?;
                    return Ok(Expr { kind: ExprKind::TupleLit(elems), line: tok.line, col: tok.col });
                }
                self.expect(&TokenKind::RParen, "')' para cerrar el paréntesis")?;
                return Ok(Expr { kind: first.kind, line: tok.line, col: tok.col });
            }
            other => {
                return Err(ParseError {
                    msg: format!("se esperaba una expresión, se encontró {:?}", other),
                    line: tok.line,
                    col: tok.col,
                    len: tok.len,
                })
            }
        };
        Ok(Expr { kind, line: tok.line, col: tok.col })
    }

    /// ifExpr = 'if' '(' expression ')' block [ 'else' ( block | ifExpr ) ]
    fn if_expr(&mut self) -> Result<Expr, ParseError> {
        let kw = self.expect(&TokenKind::If, "'if'")?;
        // M40.1b: `if let <patrón> = <expr> { … } [else …]`. Azúcar puro → un `match` de dos brazos
        // (`patrón => {then}`, `_ => {else}`); el checker/los motores/fmt lo tratan como match.
        if self.eat(&TokenKind::Let) {
            return self.if_let_expr(kw.line, kw.col);
        }
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

    /// `if let <patrón> = <expr> <bloque> [else (<bloque> | if…)]` (M40.1b), tras consumir `if let`.
    /// Se desazucara a `match (<expr>) { <patrón> => <bloque>, _ => <else | {}> }`. El patrón usa la
    /// misma gramática que el match (variantes **calificadas**: `if let Option.Some(v) = o { … }`).
    fn if_let_expr(&mut self, line: usize, col: usize) -> Result<Expr, ParseError> {
        let pattern = self.pattern()?;
        self.expect(&TokenKind::Eq, "'=' en 'if let <patrón> = <expr>'")?;
        // El escrutinio va sin paréntesis hasta el `{`; `no_struct_lit` evita tomar `Nombre {` como
        // literal de struct (el `{` abre el bloque del `then`), igual que en la cabecera de un `for`.
        let saved = self.no_struct_lit;
        self.no_struct_lit = true;
        let scrut = self.expression();
        self.no_struct_lit = saved;
        let scrut = scrut?;
        let then_block = self.block()?;
        // El cuerpo del brazo `_`: el `else` (bloque o `else if` encadenado), o unit (bloque vacío).
        let else_body = if self.eat(&TokenKind::Else) {
            if self.check(&TokenKind::If) {
                self.if_expr()? // `else if …` es una expresión `if`/`if let`; va tal cual al brazo `_`.
            } else {
                let b = self.block()?;
                Expr { line: b.line, col: b.col, kind: ExprKind::Block(b) }
            }
        } else {
            Expr { line, col, kind: ExprKind::Block(Block { statements: Vec::new(), tail: None, line, col, end_line: line }) }
        };
        let arm_then = MatchArm {
            pattern,
            guard: None,
            body: Expr { line: then_block.line, col: then_block.col, kind: ExprKind::Block(then_block) },
            line,
            col,
        };
        let arm_else = MatchArm {
            pattern: Pattern { kind: PatternKind::Wildcard, line, col },
            guard: None,
            body: else_body,
            line,
            col,
        };
        Ok(Expr {
            kind: ExprKind::Match { scrutinee: Box::new(scrut), arms: vec![arm_then, arm_else] },
            line,
            col,
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

    /// Un brazo: `patrón [if guarda] => expresión` (M40.1a).
    fn match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let pattern = self.pattern()?;
        let (line, col) = (pattern.line, pattern.col);
        // Guarda opcional: `if <cond>` entre el patrón y el `=>`.
        let guard = if self.eat(&TokenKind::If) {
            Some(self.expression()?)
        } else {
            None
        };
        self.expect(&TokenKind::FatArrow, "'=>' tras el patrón")?;
        let body = self.expression()?;
        Ok(MatchArm { pattern, guard, body, line, col })
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
            let (mut variant, _, _) = self.expect_ident("el nombre de la variante")?;
            // Variante calificada por módulo: `M.Enum.Variante` (M11.3c-3). El `enum_name` guarda
            // el `M.Enum` (con `.`), que el loader resuelve a `M::Enum`.
            let mut enum_name = name;
            if self.eat(&TokenKind::Dot) {
                let (real, _, _) = self.expect_ident("el nombre de la variante tras 'M.Enum.'")?;
                enum_name = format!("{}.{}", enum_name, variant);
                variant = real;
            }
            let mut subpatterns = Vec::new();
            if self.eat(&TokenKind::LParen) {
                loop {
                    // M40.1c: cada posición es un sub-patrón completo (recursivo), no solo un nombre.
                    subpatterns.push(self.pattern()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "')' para cerrar el patrón de variante")?;
            }
            return Ok(Pattern {
                kind: PatternKind::Variant { enum_name, variant, subpatterns },
                line,
                col,
            });
        }
        // Patrón de struct: `Nombre { campo [: sub-patrón], … }` (M40.1d). En posición de patrón el
        // `{` es inequívocamente una destructuración (no hay ambigüedad con un bloque, a diferencia
        // de las expresiones); no hace falta `no_struct_lit`.
        if self.eat(&TokenKind::LBrace) {
            let mut fields = Vec::new();
            while !self.check(&TokenKind::RBrace) {
                let (campo, fl, fc) = self.expect_ident("el nombre de un campo")?;
                // Forma larga `campo: <patrón>` o corta `campo` (≡ `campo: campo`, un binding).
                let sub = if self.eat(&TokenKind::Colon) {
                    self.pattern()?
                } else {
                    Pattern { kind: PatternKind::Binding(campo.clone()), line: fl, col: fc }
                };
                fields.push((campo, sub));
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace, "'}' para cerrar el patrón de struct")?;
            return Ok(Pattern { kind: PatternKind::Struct { name, fields }, line, col });
        }
        // Identificador suelto: binding catch-all.
        Ok(Pattern { kind: PatternKind::Binding(name), line, col })
    }

    /// arrayLiteral = '[' [ expression { ',' expression } ] ']'
    /// Literal entre corchetes: **arreglo** `[e, …]` o **Map** `[k: v, …]` (M48.2). Se decide por el `:`:
    /// `[:]` es el Map vacío; si tras el primer elemento viene `:`, es un Map; si no, un arreglo.
    fn array_literal(&mut self) -> Result<Expr, ParseError> {
        let open = self.expect(&TokenKind::LBracket, "'['")?;
        // `[:]` — el Map vacío (indeterminado). El `:` justo tras `[` solo puede ser esto.
        if self.check(&TokenKind::Colon) {
            self.advance(); // ':'
            self.expect(&TokenKind::RBracket, "']' para cerrar el Map vacío '[:]'")?;
            return Ok(Expr { kind: ExprKind::MapLit(Vec::new()), line: open.line, col: open.col });
        }
        // `[]` — el arreglo vacío.
        if self.check(&TokenKind::RBracket) {
            self.advance();
            return Ok(Expr { kind: ExprKind::ArrayLit(Vec::new()), line: open.line, col: open.col });
        }
        // Primer elemento; el `:` que le siga decide arreglo vs Map.
        let first = self.expression()?;
        if self.eat(&TokenKind::Colon) {
            // Map: `k: v { , k: v }`. Ya consumimos `first` (clave) y el `:`.
            let mut pares = vec![(first, self.expression()?)];
            while self.eat(&TokenKind::Comma) {
                if self.check(&TokenKind::RBracket) {
                    break; // coma final `[a: 1,]`
                }
                let k = self.expression()?;
                self.expect(&TokenKind::Colon, "':' entre la clave y el valor del Map")?;
                pares.push((k, self.expression()?));
            }
            self.expect(&TokenKind::RBracket, "']' para cerrar el Map")?;
            return Ok(Expr { kind: ExprKind::MapLit(pares), line: open.line, col: open.col });
        }
        // Arreglo: el resto de elementos separados por comas.
        let mut elems = vec![first];
        while self.eat(&TokenKind::Comma) {
            if self.check(&TokenKind::RBracket) {
                break; // coma final `[1, 2, 3,]`
            }
            elems.push(self.expression()?);
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

    /// `true` si el **siguiente** token (uno por delante del actual) es `kind`. Para el lookahead de
    /// `pub from …` (M11.6a), que hay que distinguir de `pub fn`/`pub struct`/etc.
    fn check_next(&self, kind: &TokenKind) -> bool {
        self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(kind)
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

    /// Cierra una lista de argumentos/parámetros de tipo con `>` (M19.3a).
    ///
    /// Problema clásico: el lexer junta `>>` en un solo token `Shr` (desplazamiento),
    /// pero en `Caja<Caja<int>>` esos `>>` cierran DOS niveles de genéricos. Solución
    /// (estilo Rust/Java): si al cerrar nos topamos con un `Shr`, lo **partimos** en dos
    /// `>` — consumimos uno conceptualmente reescribiendo el token actual a `Gt` (sin
    /// avanzar), dejando el otro `>` para que lo cierre el nivel exterior. Un `>=` o `>`
    /// suelto se trata normal.
    fn close_type_angle(&mut self, what: &str) -> Result<(), ParseError> {
        if self.check(&TokenKind::Gt) {
            self.advance();
            Ok(())
        } else if self.check(&TokenKind::Shr) {
            // Parte `>>` → consume un `>`, deja el token como `Gt` para el nivel de fuera.
            let t = &self.tokens[self.pos];
            self.tokens[self.pos] = Token::new(TokenKind::Gt, t.line, t.col + 1, 1);
            Ok(())
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

    /// Construye un error apuntando al token actual, subrayándolo entero (M33a).
    fn error_here(&self, msg: String) -> ParseError {
        let t = self.peek();
        ParseError { msg, line: t.line, col: t.col, len: t.len }
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
    fn parse_all_se_recupera_y_acumula() {
        // M33c: dos ítems rotos → dos errores; los ítems sanos entre medias sobreviven.
        let src = "fn f() -> int { let = 1; 0 }\nfn buena() -> int { 42 }\nstruct P { x int }\nfn main() -> int { buena() }";
        let toks = crate::lexer::lex(src).expect("lex ok");
        let (prog, errs) = parse_all(toks.clone());
        assert_eq!(errs.len(), 2, "{errs:?}");
        // El primer error es byte-idéntico al de la variante fail-fast (oráculos).
        let solo = parse(toks).unwrap_err();
        assert_eq!(errs[0], solo);
        // El programa parcial conserva los ítems que sí parsearon.
        let nombres: Vec<&str> = prog.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(nombres.contains(&"buena") && nombres.contains(&"main"), "{nombres:?}");
    }

    #[test]
    fn parse_all_acota_la_cascada() {
        // Un fuente patológico no produce errores sin fin: el tope corta.
        let src = "fn ( fn ( fn ( fn ( fn ( fn (".repeat(20);
        let toks = crate::lexer::lex(&src).expect("lex ok");
        let (_, errs) = parse_all(toks);
        assert!(!errs.is_empty() && errs.len() <= 20, "{}", errs.len());
    }

        #[test]
    fn el_anidamiento_hostil_se_corta_con_error() {
        // M33d: sin límite, 100k niveles desbordan la pila y ABORTAN el proceso.
        // Con pila grande para que el guard (y no la pila del hilo de test) sea el límite.
        crate::with_big_stack(|| {
            for (abre, cierra) in [("(", ")"), ("[", "]"), ("{ ", " }")] {
                let n = 2000;
                let src = format!("fn main() -> int {{ {}1{} }}", abre.repeat(n), cierra.repeat(n));
                let e = parse(crate::lexer::lex(&src).expect("lex ok")).unwrap_err();
                assert!(e.msg.contains("anidamiento demasiado profundo"), "{abre}: {}", e.msg);
            }
            // Y el anidamiento razonable sigue pasando.
            let src = format!("fn main() -> int {{ {}1{} }}", "(".repeat(500), ")".repeat(500));
            parse(crate::lexer::lex(&src).expect("lex ok")).expect("500 niveles pasan");
        });
    }

        #[test]
    fn el_error_sintactico_subraya_el_token_ofensor() {
        // M33a: el error lleva la extensión del token que lo provocó.
        let tokens = crate::lexer::lex("fn main() -> int { let x = enum; x }").expect("lex ok");
        let e = parse(tokens).unwrap_err();
        assert!(e.msg.contains("se esperaba una expresión"), "{}", e.msg);
        assert_eq!(e.len, 4, "'enum' mide 4 caracteres");
        // Un identificador ofensor mide su nombre entero.
        let tokens = crate::lexer::lex("fn main() -> int { 1 nombrelargo }").expect("lex ok");
        let e = parse(tokens).unwrap_err();
        assert_eq!(e.len, "nombrelargo".chars().count());
    }

    #[test]
    fn import_con_ruta_de_directorios_y_leaf() {
        // M11.5: una ruta `a/b/c` se parsea como un solo módulo; el leaf es el último segmento.
        let prog = parse_prog("import geo/formas/circulo;\nfn main() -> int { 0 }\n");
        assert_eq!(prog.imports.len(), 1);
        let i = &prog.imports[0];
        assert_eq!(i.module, "geo/formas/circulo");
        assert_eq!(i.alias, None);
        assert_eq!(i.leaf(), "circulo");
    }

    #[test]
    fn import_con_ruta_y_alias() {
        let prog = parse_prog("import geo/formas/circulo as c;\nfn main() -> int { 0 }\n");
        let i = &prog.imports[0];
        assert_eq!(i.module, "geo/formas/circulo");
        assert_eq!(i.alias.as_deref(), Some("c"));
        assert_eq!(i.leaf(), "c");
    }

    #[test]
    fn from_import_admite_ruta_de_directorios() {
        let prog = parse_prog("from geo/formas import area;\nfn main() -> int { 0 }\n");
        assert_eq!(prog.from_imports[0].module, "geo/formas");
        assert!(!prog.from_imports[0].is_pub, "un 'from' normal no es reexport");
    }

    #[test]
    fn pub_from_marca_reexport() {
        // M11.6a: `pub from M import …` es un reexport (cara pública de un mod.ray).
        let prog = parse_prog("pub from geo/formas/circulo import area, Circulo;\nfn main() -> int { 0 }\n");
        assert_eq!(prog.from_imports.len(), 1);
        assert!(prog.from_imports[0].is_pub, "el 'pub from' marca is_pub");
        assert_eq!(prog.from_imports[0].names.len(), 2);
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

    #[test]
    fn tipo_calificado_por_modulo_guarda_el_punto() {
        // M11.3c-3: `M.Tipo` en posición de tipo → `Struct("M.Tipo")` (el loader resuelve el `.`).
        let prog = parse_prog("fn f(p: geo.Punto) -> mods.Color { p }\nfn main() -> int { 0 }\n");
        assert_eq!(prog.functions[0].params[0].ty, Type::Struct("geo.Punto".into(), vec![]));
        assert_eq!(prog.functions[0].return_type, Type::Struct("mods.Color".into(), vec![]));
    }

    #[test]
    fn literal_de_struct_calificado_guarda_el_punto() {
        // `M.Tipo { ... }` → `StructLit { name: "M.Tipo", ... }`.
        let e = parse_expr("geo.Punto { x: 1, y: 2 }");
        match &e.kind {
            ExprKind::StructLit { name, fields } => {
                assert_eq!(name, "geo.Punto");
                assert_eq!(fields.len(), 2);
            }
            other => panic!("se esperaba un literal de struct calificado, fue {:?}", other),
        }
    }

    #[test]
    fn patron_de_variante_calificado_guarda_el_punto() {
        // `M.Enum.Variante` en un patrón → `enum_name: "M.Enum"`, `variant: "Variante"`.
        let e = parse_expr("match (c) { geo.Color.Verde(n) => n, _ => 0, }");
        let ExprKind::Match { arms, .. } = &e.kind else { panic!("se esperaba match") };
        match &arms[0].pattern.kind {
            PatternKind::Variant { enum_name, variant, subpatterns } => {
                assert_eq!(enum_name, "geo.Color");
                assert_eq!(variant, "Verde");
                assert_eq!(spat(&subpatterns[0]), "n"); // el sub-patrón es un binding `n`
            }
            other => panic!("se esperaba un patrón de variante calificado, fue {:?}", other),
        }
    }

    /// Renderiza una expresión como S-expression para asertar su forma de forma
    /// compacta: `1 + 2 * 3` → `(+ 1 (* 2 3))`.
    fn sx(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Int(v) => v.to_string(),
            ExprKind::Float(v) => v.to_string(),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Str(s) => format!("{:?}", s),
            ExprKind::Char(c) => format!("{:?}", c),
            ExprKind::Bytes(b) => format!("{:?}", b),
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
            ExprKind::MapLit(pares) => {
                let e: Vec<String> = pares.iter().map(|(k, v)| format!("{}: {}", sx(k), sx(v))).collect();
                format!("[{}]", e.join(", "))
            }
            ExprKind::TupleLit(elems) => {
                let e: Vec<String> = elems.iter().map(sx).collect();
                format!("({})", e.join(", "))
            }
            ExprKind::Index { array, index } => format!("(index {} {})", sx(array), sx(index)),
            ExprKind::Cast { expr, ty } => format!("(as {} {})", sx(expr), ty),
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
            PatternKind::Variant { enum_name, variant, subpatterns } => {
                if subpatterns.is_empty() {
                    format!("{}.{}", enum_name, variant)
                } else {
                    let bs: Vec<String> = subpatterns.iter().map(spat).collect();
                    format!("{}.{}({})", enum_name, variant, bs.join(", "))
                }
            }
            PatternKind::Struct { name, fields } => {
                let fs: Vec<String> = fields.iter().map(|(f, p)| format!("{}: {}", f, spat(p))).collect();
                format!("{} {{ {} }}", name, fs.join(", "))
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
            StmtKind::LetTuple { names, value, mutable } => {
                let kw = if *mutable { "var" } else { "let" };
                let ns: Vec<String> = names.iter().map(|n| n.clone().unwrap_or_else(|| "_".to_string())).collect();
                format!("{} ({}) = {}", kw, ns.join(", "), sx(value))
            }
            StmtKind::For { pat, iter, .. } => {
                let p = match pat {
                    ForPat::Single(n) => n.clone(),
                    ForPat::Tuple(ns) => format!("({})", ns.iter().map(|n| n.clone().unwrap_or_else(|| "_".to_string())).collect::<Vec<_>>().join(", ")),
                };
                let it = match iter {
                    ForIter::Range { start, end } => format!("{}..{}", sx(start), sx(end)),
                    ForIter::In(e) => sx(e),
                    ForIter::Iter { expr, .. } => sx(expr),
                };
                format!("for {} in {}", p, it)
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
            UnaryOp::BitNot => "~",
        }
    }

    fn bop(op: BinaryOp) -> &'static str {
        use BinaryOp::*;
        match op {
            Add => "+", Sub => "-", Mul => "*", Div => "/", Rem => "%",
            Eq => "==", Ne => "!=", Lt => "<", Le => "<=", Gt => ">", Ge => ">=",
            And => "&&", Or => "||",
            BitAnd => "&", BitOr => "|", BitXor => "^", Shl => "<<", Shr => ">>",
        }
    }

    #[test]
    fn precedencia_multiplicacion_sobre_suma() {
        assert_eq!(sx(&parse_expr("1 + 2 * 3")), "(+ 1 (* 2 3))");
        assert_eq!(sx(&parse_expr("1 * 2 + 3")), "(+ (* 1 2) 3)");
    }

    #[test]
    fn literal_map_vs_arreglo() {
        // M48.2: el `:` distingue Map de arreglo; `[:]` es el Map vacío, `[]` el arreglo vacío.
        assert!(matches!(parse_expr("[]").kind, ExprKind::ArrayLit(ref e) if e.is_empty()));
        assert!(matches!(parse_expr("[:]").kind, ExprKind::MapLit(ref p) if p.is_empty()));
        assert!(matches!(parse_expr("[1, 2, 3]").kind, ExprKind::ArrayLit(_)));
        assert_eq!(sx(&parse_expr("[1: \"a\", 2: \"b\"]")), "[1: \"a\", 2: \"b\"]");
        // Coma final permitida en el Map.
        assert!(matches!(parse_expr("[1: 2,]").kind, ExprKind::MapLit(ref p) if p.len() == 1));
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
        // El parámetro x tiene tipo Type::Dyn(["Figura"]).
        let f = &prog.functions[0];
        assert_eq!(f.params[0].ty, Type::Dyn(vec!["Figura".to_string()]));
    }

    #[test]
    fn parse_dyn_multi_trait_es_canonico() {
        // `dyn B + A` se guarda ordenado (canónico): igual a `dyn A + B`. (M9.5)
        let prog = parse_prog(r#"
            trait A { fn a(self) -> int; }
            trait B { fn b(self) -> int; }
            fn f(x: dyn B + A) -> int { 0 }
            fn main() -> int { 0 }
        "#);
        assert_eq!(prog.functions[0].params[0].ty, Type::Dyn(vec!["A".to_string(), "B".to_string()]));
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
