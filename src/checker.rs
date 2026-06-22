//! Type checker (análisis semántico) de raylang.
//!
//! Tercera fase del pipeline (DESIGN.md §2, reglas en §8). El parser garantiza que
//! el programa es sintácticamente válido; el checker garantiza que *tiene
//! sentido*: que no sumas un `bool` con un `string`, que no usas variables sin
//! declarar, que `fib` realmente devuelve `int`, etc. Un programa que pasa el
//! checker no puede fallar por un error de tipos en tiempo de ejecución.
//!
//! ## Dos pasadas
//!
//! 1. **Pre-pasada**: registra la firma de cada función (parámetros y retorno).
//!    Así una función puede llamar a otra declarada más abajo, y a sí misma
//!    (recursión), sin que el orden importe.
//! 2. **Verificación**: recorre el cuerpo de cada función comprobando las reglas.
//!
//! ## Ámbitos (scopes)
//!
//! Las variables viven en una **pila de ámbitos**. Cada bloque empuja un ámbito y
//! lo retira al salir. Buscar un nombre recorre la pila de dentro hacia afuera, lo
//! que da *shadowing* (una variable interior tapa una exterior) de forma natural.
//!
//! ## Una nota sobre el flujo
//!
//! Como raylang es orientado a expresiones, el cuerpo de una función `-> int` debe
//! *producir* un `int` (retorno implícito). Pero también vale salir antes con
//! `return`. Para aceptar `fn f() -> int { return 5; }` (sin expresión final)
//! hacemos un pequeño análisis de **divergencia**: si todos los caminos del bloque
//! terminan en `return`, el bloque "diverge" y no necesita valor final.

use std::collections::{HashMap, HashSet};

use crate::ast::*;

/// Error de tipos con ubicación.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error de tipos en {}:{}: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for TypeError {}

/// Firma de una función: parámetros de tipo (genéricos), tipos de parámetros y tipo
/// de retorno. `type_params` vacío = no genérica.
struct FnSig {
    type_params: Vec<String>,
    params: Vec<Type>,
    ret: Type,
}

/// Información de una variable en un ámbito.
struct VarInfo {
    ty: Type,
    mutable: bool,
}

/// Punto de entrada de la fase: verifica un programa completo.
///
/// Recibe el programa por **referencia mutable** porque, antes de verificar,
/// **reescribe** los `Field`/`Call` que en realidad son construcción de variantes de
/// enum (`Enum.Variante(args)`) en nodos `EnumLit` explícitos (M5). Esa resolución
/// es parte del front-end compartido: el intérprete y la VM reciben el AST ya
/// resuelto, sin duplicar la regla.
pub fn check(program: &mut Program) -> Result<(), TypeError> {
    // Paso 0: resolver la construcción de enums sobre el AST.
    let enum_names: HashSet<String> = program.enums.iter().map(|e| e.name.clone()).collect();
    if !enum_names.is_empty() {
        for f in &mut program.functions {
            resolve_block(&mut f.body, &enum_names);
        }
    }
    // Pasos 1–2: pre-pasada y verificación.
    Checker::new().check_program(program)
}

struct Checker {
    /// Firmas de todas las funciones (llenada en la pre-pasada).
    functions: HashMap<String, FnSig>,
    /// Definiciones de struct: nombre → campos (en orden). Pre-pasada.
    structs: HashMap<String, Vec<(String, Type)>>,
    /// Definiciones de enum: nombre → variantes (nombre, payload), en orden.
    /// Pre-pasada (M5).
    enums: HashMap<String, Vec<(String, Vec<Type>)>>,
    /// Solo los nombres de enum, para `resolve_type` (reclasificar `Struct`→`Enum`)
    /// y para validar tipos. Se llena antes que cualquier resolución de tipos.
    enum_names: HashSet<String>,
    /// Pila de ámbitos de variables. El último es el más interno.
    scopes: Vec<HashMap<String, VarInfo>>,
    /// Tipo de retorno de la función que estamos verificando ahora mismo, para
    /// validar las sentencias `return`.
    current_return: Type,
    /// Parámetros de tipo en ámbito ahora mismo: los `<T, U>` de la función que se
    /// registra o verifica (M6). `resolve_type` los reclasifica de `Struct(name)` a
    /// `Var(name)`, y `ensure_type` los acepta como tipos válidos.
    type_params: HashSet<String>,
}

impl Checker {
    fn new() -> Self {
        Checker {
            functions: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            enum_names: HashSet::new(),
            scopes: Vec::new(),
            current_return: Type::Unit,
            type_params: HashSet::new(),
        }
    }

    fn check_program(&mut self, program: &Program) -> Result<(), TypeError> {
        // --- Pre-pasada: nombres de los tipos nominales (enum y struct) ---
        // Los nombres de enum se necesitan antes de normalizar cualquier tipo, para
        // reclasificar `Struct(nombre)`→`Enum(nombre)` (`resolve_type`).
        for e in &program.enums {
            if !self.enum_names.insert(e.name.clone()) {
                return Err(self.err(e.line, e.col, format!("enum '{}' declarado dos veces", e.name)));
            }
        }
        for s in &program.structs {
            if self.enum_names.contains(&s.name) {
                return Err(self.err(s.line, s.col, format!("'{}' ya es un enum; no puede ser también un struct", s.name)));
            }
        }

        // --- Pre-pasada: registrar enums (variantes con payload normalizado) ---
        for e in &program.enums {
            let mut seen = HashSet::new();
            let mut variants = Vec::new();
            for v in &e.variants {
                if !seen.insert(v.name.clone()) {
                    return Err(self.err(v.line, v.col, format!("variante '{}' repetida en el enum '{}'", v.name, e.name)));
                }
                let payload: Vec<Type> = v.payload.iter().map(|t| self.resolve_type(t)).collect();
                for t in &payload {
                    self.ensure_type(t, v.line, v.col)?;
                }
                variants.push((v.name.clone(), payload));
            }
            self.enums.insert(e.name.clone(), variants);
        }

        // --- Pre-pasada: registrar structs (antes que las funciones, que pueden
        // usarlos en sus tipos) ---
        for s in &program.structs {
            if self.structs.contains_key(&s.name) {
                return Err(self.err(s.line, s.col, format!("struct '{}' declarado dos veces", s.name)));
            }
            let fields: Vec<(String, Type)> =
                s.fields.iter().map(|(n, t)| (n.clone(), self.resolve_type(t))).collect();
            self.structs.insert(s.name.clone(), fields);
        }
        // Validar que los tipos de los campos existen (p. ej. structs/enums referenciados).
        for s in &program.structs {
            let fields = self.structs.get(&s.name).expect("recién registrado").clone();
            for (_, ty) in &fields {
                self.ensure_type(ty, s.line, s.col)?;
            }
        }

        // --- Pre-pasada: registrar firmas (con tipos normalizados) ---
        for f in &program.functions {
            if self.functions.contains_key(&f.name) {
                return Err(self.err(f.line, f.col, format!("función '{}' declarada dos veces", f.name)));
            }
            // Los parámetros de tipo de ESTA función están en ámbito al resolver su
            // firma: así `x: T` se normaliza a `Var("T")` en vez de `Struct("T")`.
            self.type_params = f.type_params.iter().cloned().collect();
            let sig = FnSig {
                type_params: f.type_params.clone(),
                params: f.params.iter().map(|p| self.resolve_type(&p.ty)).collect(),
                ret: self.resolve_type(&f.return_type),
            };
            self.functions.insert(f.name.clone(), sig);
        }
        self.type_params.clear();

        // 'main' es obligatoria (DESIGN.md §11): sin parámetros y con retorno int o unit.
        match self.functions.get("main") {
            None => return Err(self.err(1, 1, "falta la función de entrada 'main'".into())),
            Some(sig) => {
                if !sig.params.is_empty() {
                    return Err(self.err(1, 1, "'main' no debe recibir parámetros".into()));
                }
                if sig.ret != Type::Int && sig.ret != Type::Unit {
                    return Err(self.err(1, 1, format!("'main' debe devolver int o unit, no {}", sig.ret)));
                }
            }
        }

        // --- Verificación de cada función ---
        for f in &program.functions {
            self.check_function(f)?;
        }
        Ok(())
    }

    fn check_function(&mut self, f: &Function) -> Result<(), TypeError> {
        // Los parámetros de tipo de la función entran en ámbito mientras se verifica
        // su firma y su cuerpo (M6): `Var("T")` es un tipo válido y opaco aquí.
        let mut seen = HashSet::new();
        for tp in &f.type_params {
            if !seen.insert(tp.clone()) {
                return Err(self.err(f.line, f.col, format!("parámetro de tipo '{}' repetido en '{}'", tp, f.name)));
            }
        }
        self.type_params = seen;
        for p in &f.params {
            self.ensure_type(&p.ty, p.line, p.col)?;
        }
        self.ensure_type(&f.return_type, f.line, f.col)?;
        let r = self.check_fn_body(&f.params, &f.return_type, &f.body, f.line, f.col, &format!("'{}'", f.name));
        self.type_params.clear();
        r
    }

    /// Verifica el cuerpo de una función (nombrada o anónima): declara los
    /// parámetros en un ámbito nuevo, comprueba el bloque y exige que su tipo-valor
    /// (el retorno implícito) coincida con el declarado, salvo que el cuerpo
    /// diverja (retorne por todos los caminos). `label` se usa en los mensajes.
    fn check_fn_body(
        &mut self,
        params: &[Param],
        return_type: &Type,
        body: &Block,
        line: usize,
        col: usize,
        label: &str,
    ) -> Result<(), TypeError> {
        // Normaliza el tipo de retorno (`: Figura` llega como `Struct`, puede ser
        // `Enum`) y úsalo en TODA esta función: tanto para validar los `return` como
        // para comparar con el tipo del cuerpo. Comparar contra el tipo crudo daría
        // un falso negativo `Enum` vs `Struct` con el mismo nombre.
        let return_type = self.resolve_type(return_type);
        self.current_return = return_type.clone();
        self.push_scope();
        // Los parámetros son inmutables (no hay 'var' para ellos).
        for p in params {
            let ty = self.resolve_type(&p.ty);
            self.declare(&p.name, ty, false);
        }

        let body_ty = self.check_block(body)?;
        let diverges = block_diverges(body);

        // Posición para el posible error: la expresión final si existe, si no la fn.
        let (eline, ecol) = match &body.tail {
            Some(t) => (t.line, t.col),
            None => (line, col),
        };

        let result = if return_type == Type::Unit {
            // Una función unit no debe terminar produciendo un valor.
            if body_ty != Type::Unit && !diverges {
                Err(self.err(eline, ecol, format!(
                    "{} no declara retorno (unit), pero su cuerpo produce {}",
                    label, body_ty
                )))
            } else {
                Ok(())
            }
        } else if body_ty == return_type || diverges {
            Ok(())
        } else {
            Err(self.err(eline, ecol, format!(
                "{} declara devolver {}, pero su cuerpo produce {}",
                label, return_type, body_ty
            )))
        };

        self.pop_scope();
        result
    }

    // ----- Sentencias -----

    fn check_stmt(&mut self, stmt: &Stmt) -> Result<(), TypeError> {
        match &stmt.kind {
            StmtKind::Let { name, ty, value, mutable } => {
                self.ensure_type(ty, stmt.line, stmt.col)?;
                // La anotación puede nombrar un enum (llega como `Struct`): normaliza.
                let ty = self.resolve_type(ty);
                // Caso especial: `[]` adopta el tipo de arreglo declarado (no hay
                // de dónde inferir el tipo de elemento de un arreglo vacío).
                let vt = if matches!(&value.kind, ExprKind::ArrayLit(e) if e.is_empty()) {
                    if matches!(ty, Type::Array(_)) {
                        ty.clone()
                    } else {
                        return Err(self.err(value.line, value.col, format!(
                            "'{}' se declara como {} pero se inicializa con un arreglo vacío",
                            name, ty
                        )));
                    }
                } else {
                    self.check_expr(value)?
                };
                if vt != ty {
                    return Err(self.err(value.line, value.col, format!(
                        "'{}' se declara como {} pero se inicializa con {}",
                        name, ty, vt
                    )));
                }
                self.declare(name, ty, *mutable);
                Ok(())
            }
            StmtKind::Assign { target, value } => self.check_assign(target, value, stmt.line, stmt.col),
            StmtKind::Return { value } => {
                let vt = match value {
                    Some(e) => self.check_expr(e)?,
                    None => Type::Unit,
                };
                if vt != self.current_return {
                    return Err(self.err(stmt.line, stmt.col, format!(
                        "se devuelve {} pero la función declara retorno {}",
                        vt, self.current_return
                    )));
                }
                Ok(())
            }
            StmtKind::Expr(e) => {
                // Una expresión-sentencia solo debe estar bien tipada; su valor se
                // descarta.
                self.check_expr(e)?;
                Ok(())
            }
        }
    }

    /// Verifica una asignación a un lvalue.
    fn check_assign(&mut self, target: &Expr, value: &Expr, line: usize, col: usize) -> Result<(), TypeError> {
        match &target.kind {
            // x = e  — requiere que la variable exista y sea mutable ('var').
            ExprKind::Ident(name) => {
                let (var_ty, mutable) = match self.lookup(name) {
                    Some(v) => (v.ty.clone(), v.mutable),
                    None => return Err(self.err(target.line, target.col, format!("variable '{}' no declarada", name))),
                };
                if !mutable {
                    return Err(self.err(line, col, format!(
                        "no se puede asignar a '{}': es inmutable (declarada con 'let'; usa 'var')",
                        name
                    )));
                }
                let vt = self.check_expr(value)?;
                if vt != var_ty {
                    return Err(self.err(value.line, value.col, format!("'{}' es {} pero se le asigna {}", name, var_ty, vt)));
                }
                Ok(())
            }
            // a[i] = e  — mutar el contenido NO requiere 'var' (DESIGN §12.3): la
            // inmutabilidad de `let` ata la variable, no congela el objeto.
            ExprKind::Index { array, index } => {
                let elem = self.check_index(array, index)?;
                let vt = self.check_expr(value)?;
                if vt != elem {
                    return Err(self.err(value.line, value.col, format!("el elemento es {} pero se le asigna {}", elem, vt)));
                }
                Ok(())
            }
            // p.x = e  — mutar un campo (no requiere 'var', como el índice).
            ExprKind::Field { object, name } => {
                let fty = self.check_field(object, name)?;
                let vt = self.check_expr(value)?;
                if vt != fty {
                    return Err(self.err(value.line, value.col, format!("el campo '{}' es {} pero se le asigna {}", name, fty, vt)));
                }
                Ok(())
            }
            _ => Err(self.err(target.line, target.col, "el lado izquierdo no es asignable".into())),
        }
    }

    /// Verifica `a[i]` y devuelve el tipo de elemento. Reusado por la indexación
    /// como expresión y como destino de asignación.
    fn check_index(&mut self, array: &Expr, index: &Expr) -> Result<Type, TypeError> {
        let at = self.check_expr(array)?;
        let it = self.check_expr(index)?;
        if it != Type::Int {
            return Err(self.err(index.line, index.col, format!("el índice debe ser int, no {}", it)));
        }
        match at {
            Type::Array(elem) => Ok(*elem),
            other => Err(self.err(array.line, array.col, format!("no se puede indexar un {} (no es un arreglo)", other))),
        }
    }

    /// Verifica que un tipo es válido: los structs referenciados deben existir.
    fn ensure_type(&self, ty: &Type, line: usize, col: usize) -> Result<(), TypeError> {
        match ty {
            Type::Array(elem) => self.ensure_type(elem, line, col),
            // Un identificador en posición de tipo llega como `Struct(name)` desde el
            // parser; aquí ya puede referirse a un struct, a un enum o a un parámetro
            // de tipo en ámbito (M6).
            Type::Struct(name) => {
                if self.structs.contains_key(name)
                    || self.enum_names.contains(name)
                    || self.type_params.contains(name)
                {
                    Ok(())
                } else {
                    Err(self.err(line, col, format!("tipo desconocido: '{}' no declarado", name)))
                }
            }
            Type::Enum(name) if !self.enum_names.contains(name) => {
                Err(self.err(line, col, format!("tipo desconocido: enum '{}' no declarado", name)))
            }
            // Un parámetro de tipo (M6) es válido si está en ámbito.
            Type::Var(name) if !self.type_params.contains(name) => {
                Err(self.err(line, col, format!("parámetro de tipo '{}' fuera de ámbito", name)))
            }
            Type::Fn(params, ret) => {
                for p in params {
                    self.ensure_type(p, line, col)?;
                }
                self.ensure_type(ret, line, col)
            }
            _ => Ok(()),
        }
    }

    /// Normaliza un tipo proveniente de una anotación. El parser produce
    /// `Struct(name)` para cualquier identificador; aquí se reclasifica según qué sea
    /// el nombre, recursivamente bajo `[T]` y `fn`:
    ///   - un **parámetro de tipo** en ámbito → `Var` (M6; tapa a los nombres de tipo);
    ///   - un **enum** → `Enum` (M5);
    ///   - en otro caso, se queda como `Struct`.
    /// Así las comparaciones de tipos cuadran con el tipo que produce cada expresión.
    fn resolve_type(&self, ty: &Type) -> Type {
        match ty {
            Type::Struct(name) => {
                if self.type_params.contains(name) {
                    Type::Var(name.clone())
                } else if self.enum_names.contains(name) {
                    Type::Enum(name.clone())
                } else {
                    Type::Struct(name.clone())
                }
            }
            Type::Array(elem) => Type::Array(Box::new(self.resolve_type(elem))),
            Type::Fn(params, ret) => Type::Fn(
                params.iter().map(|p| self.resolve_type(p)).collect(),
                Box::new(self.resolve_type(ret)),
            ),
            other => other.clone(),
        }
    }

    /// Verifica un literal de struct `Nombre { campo: valor, ... }`: el struct debe
    /// existir y los campos deben coincidir exactamente (mismos nombres y tipos).
    fn check_struct_lit(&mut self, name: &str, fields: &[(String, Expr)], line: usize, col: usize) -> Result<Type, TypeError> {
        let declared = match self.structs.get(name) {
            Some(d) => d.clone(), // clonamos para soltar el préstamo de self
            None => return Err(self.err(line, col, format!("struct '{}' no declarado", name))),
        };
        // No debe haber campos desconocidos.
        for (fname, fexpr) in fields {
            if !declared.iter().any(|(dname, _)| dname == fname) {
                return Err(self.err(fexpr.line, fexpr.col, format!("'{}' no tiene un campo '{}'", name, fname)));
            }
        }
        // Cada campo declarado debe estar presente exactamente una vez y con su tipo.
        for (dname, dty) in &declared {
            let matches: Vec<&(String, Expr)> = fields.iter().filter(|(fname, _)| fname == dname).collect();
            match matches.as_slice() {
                [] => return Err(self.err(line, col, format!("falta el campo '{}' en el literal de '{}'", dname, name))),
                [(_, value)] => {
                    let vt = self.check_expr(value)?;
                    if vt != *dty {
                        return Err(self.err(value.line, value.col, format!(
                            "campo '{}' de '{}': se esperaba {}, se dio {}", dname, name, dty, vt
                        )));
                    }
                }
                _ => return Err(self.err(line, col, format!("campo '{}' de '{}' repetido", dname, name))),
            }
        }
        Ok(Type::Struct(name.to_string()))
    }

    /// Verifica la construcción de una variante de enum `Enum.Variante(args)`: el
    /// enum y la variante deben existir, y los argumentos deben coincidir con el
    /// payload (aridad y tipos). Devuelve `Enum(enum_name)`.
    fn check_enum_lit(&mut self, enum_name: &str, variant: &str, args: &[Expr], line: usize, col: usize) -> Result<Type, TypeError> {
        let payload = match self.enums.get(enum_name) {
            Some(variants) => match variants.iter().find(|(vname, _)| vname == variant) {
                Some((_, payload)) => payload.clone(), // clonar para soltar el préstamo de self
                None => return Err(self.err(line, col, format!("el enum '{}' no tiene la variante '{}'", enum_name, variant))),
            },
            None => return Err(self.err(line, col, format!("enum '{}' no declarado", enum_name))),
        };
        if args.len() != payload.len() {
            return Err(self.err(line, col, format!(
                "la variante '{}.{}' espera {} argumento(s), se dieron {}",
                enum_name, variant, payload.len(), args.len()
            )));
        }
        for (arg, expected) in args.iter().zip(&payload) {
            let at = self.check_expr(arg)?;
            if at != *expected {
                return Err(self.err(arg.line, arg.col, format!(
                    "'{}.{}': se esperaba {}, se dio {}", enum_name, variant, expected, at
                )));
            }
        }
        Ok(Type::Enum(enum_name.to_string()))
    }

    /// Verifica un `match (escrutinio) { patrón => cuerpo, ... }` (M5.2):
    ///   - el escrutinio debe ser un enum;
    ///   - cada patrón debe pertenecer a ese enum y ligar el payload con la aridad
    ///     correcta; los brazos producen un tipo común (como las ramas de un `if`);
    ///   - debe ser **exhaustivo**: cubrir todas las variantes o tener un catch-all.
    fn check_match(&mut self, scrutinee: &Expr, arms: &[MatchArm], line: usize, col: usize) -> Result<Type, TypeError> {
        let scrut_ty = self.check_expr(scrutinee)?;
        let enum_name = match &scrut_ty {
            Type::Enum(n) => n.clone(),
            other => return Err(self.err(scrutinee.line, scrutinee.col, format!(
                "match requiere un enum, pero el escrutinio es {}", other
            ))),
        };
        if arms.is_empty() {
            return Err(self.err(line, col, "un match no puede estar vacío".into()));
        }
        // Variantes del enum (clonadas para soltar el préstamo de self).
        let variants = self.enums.get(&enum_name).expect("el checker registró el enum").clone();

        let mut covered: HashSet<String> = HashSet::new();
        let mut catchall = false;
        let mut result_ty: Option<Type> = None;

        for arm in arms {
            // Un brazo tras un catch-all nunca se alcanza.
            if catchall {
                return Err(self.err(arm.line, arm.col,
                    "brazo inalcanzable: un brazo anterior ya cubre todos los casos".into()));
            }
            // Comprueba el patrón y obtiene las variables a ligar en el cuerpo.
            let binds = self.check_pattern(&arm.pattern, &scrut_ty, &enum_name, &variants, &mut covered, &mut catchall)?;
            // Verifica el cuerpo con esas variables en un ámbito propio.
            self.push_scope();
            for (name, ty) in binds {
                self.declare(&name, ty, false);
            }
            let body_ty = self.check_expr(&arm.body);
            self.pop_scope();
            let body_ty = body_ty?;
            // Todos los brazos convergen a un mismo tipo (el tipo del match).
            match &result_ty {
                None => result_ty = Some(body_ty),
                Some(prev) if *prev != body_ty => {
                    return Err(self.err(arm.body.line, arm.body.col, format!(
                        "los brazos del match producen tipos distintos: {} y {}", prev, body_ty
                    )));
                }
                _ => {}
            }
        }

        // Exhaustividad: sin catch-all, deben estar TODAS las variantes.
        if !catchall {
            let missing: Vec<&str> = variants
                .iter()
                .map(|(v, _)| v.as_str())
                .filter(|v| !covered.contains(*v))
                .collect();
            if !missing.is_empty() {
                return Err(self.err(line, col, format!(
                    "match no exhaustivo en '{}': faltan las variantes: {}",
                    enum_name, missing.join(", ")
                )));
            }
        }

        Ok(result_ty.expect("hay al menos un brazo"))
    }

    /// Comprueba un patrón contra el enum del escrutinio. Devuelve las variables que
    /// liga (nombre, tipo) para declararlas en el cuerpo del brazo. Actualiza el
    /// conjunto de variantes cubiertas y marca si el patrón es catch-all.
    fn check_pattern(
        &self,
        pat: &Pattern,
        scrut_ty: &Type,
        enum_name: &str,
        variants: &[(String, Vec<Type>)],
        covered: &mut HashSet<String>,
        catchall: &mut bool,
    ) -> Result<Vec<(String, Type)>, TypeError> {
        match &pat.kind {
            PatternKind::Wildcard => {
                *catchall = true;
                Ok(Vec::new())
            }
            PatternKind::Binding(name) => {
                // Liga el escrutinio completo; cubre todo lo restante.
                *catchall = true;
                Ok(vec![(name.clone(), scrut_ty.clone())])
            }
            PatternKind::Variant { enum_name: pat_enum, variant, bindings } => {
                if pat_enum != enum_name {
                    return Err(self.err(pat.line, pat.col, format!(
                        "el patrón es del enum '{}', pero el escrutinio es '{}'", pat_enum, enum_name
                    )));
                }
                let payload = match variants.iter().find(|(v, _)| v == variant) {
                    Some((_, p)) => p,
                    None => return Err(self.err(pat.line, pat.col, format!(
                        "el enum '{}' no tiene la variante '{}'", enum_name, variant
                    ))),
                };
                if bindings.len() != payload.len() {
                    return Err(self.err(pat.line, pat.col, format!(
                        "el patrón '{}.{}' liga {} valor(es), pero la variante tiene {}",
                        enum_name, variant, bindings.len(), payload.len()
                    )));
                }
                if !covered.insert(variant.clone()) {
                    return Err(self.err(pat.line, pat.col, format!(
                        "la variante '{}' ya está cubierta por un brazo anterior", variant
                    )));
                }
                // Solo los sub-bindings nombrados (no `_`) ligan una variable.
                let mut binds = Vec::new();
                for (b, ty) in bindings.iter().zip(payload) {
                    if let Some(name) = b {
                        binds.push((name.clone(), ty.clone()));
                    }
                }
                Ok(binds)
            }
        }
    }

    /// Verifica `obj.name` y devuelve el tipo del campo. Reusado por el acceso como
    /// expresión y como destino de asignación.
    fn check_field(&mut self, object: &Expr, name: &str) -> Result<Type, TypeError> {
        let ot = self.check_expr(object)?;
        match ot {
            Type::Struct(sname) => {
                let fields = self.structs.get(&sname).expect("el checker registró el struct");
                match fields.iter().find(|(fname, _)| fname == name) {
                    Some((_, fty)) => Ok(fty.clone()),
                    None => Err(self.err(object.line, object.col, format!("el struct '{}' no tiene un campo '{}'", sname, name))),
                }
            }
            other => Err(self.err(object.line, object.col, format!("no se puede acceder a '.{}' en un {} (no es un struct)", name, other))),
        }
    }

    // ----- Expresiones (devuelven su tipo) -----

    fn check_expr(&mut self, expr: &Expr) -> Result<Type, TypeError> {
        match &expr.kind {
            ExprKind::Int(_) => Ok(Type::Int),
            ExprKind::Float(_) => Ok(Type::Float),
            ExprKind::Bool(_) => Ok(Type::Bool),
            ExprKind::Str(_) => Ok(Type::String),

            ExprKind::Ident(name) => {
                // Una variable tapa a una función con el mismo nombre.
                if let Some(v) = self.lookup(name) {
                    return Ok(v.ty.clone());
                }
                // Un nombre de función de nivel superior es un valor de primera
                // clase: su tipo es el tipo función correspondiente (M4.1). Una
                // función **genérica** no puede tomarse como valor (su tipo no es un
                // `fn(...)` concreto): hay que llamarla directamente (M6.1).
                if let Some(sig) = self.functions.get(name) {
                    if !sig.type_params.is_empty() {
                        return Err(self.err(expr.line, expr.col, format!(
                            "no se puede usar la función genérica '{}' como valor; llámala directamente", name
                        )));
                    }
                    return Ok(Type::Fn(sig.params.clone(), Box::new(sig.ret.clone())));
                }
                Err(self.err(expr.line, expr.col, format!("nombre '{}' no declarado", name)))
            }

            ExprKind::Unary { op, expr: inner } => {
                let t = self.check_expr(inner)?;
                match op {
                    UnaryOp::Neg if t == Type::Int || t == Type::Float => Ok(t),
                    UnaryOp::Neg => Err(self.err(expr.line, expr.col, format!("no se puede negar (-) un {}", t))),
                    UnaryOp::Not if t == Type::Bool => Ok(Type::Bool),
                    UnaryOp::Not => Err(self.err(expr.line, expr.col, format!("el '!' requiere bool, no {}", t))),
                }
            }

            ExprKind::Binary { op, left, right } => self.check_binary(*op, left, right, expr.line, expr.col),

            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.line, expr.col),

            ExprKind::ArrayLit(elems) => {
                if elems.is_empty() {
                    return Err(self.err(expr.line, expr.col,
                        "no se puede inferir el tipo de [] aquí; anótalo (p. ej. let xs: [int] = [];)".into()));
                }
                let first = self.check_expr(&elems[0])?;
                for e in &elems[1..] {
                    let t = self.check_expr(e)?;
                    if t != first {
                        return Err(self.err(e.line, e.col, format!(
                            "los elementos del arreglo deben ser del mismo tipo: {} y {}", first, t
                        )));
                    }
                }
                Ok(Type::Array(Box::new(first)))
            }

            ExprKind::Index { array, index } => self.check_index(array, index),

            ExprKind::StructLit { name, fields } => self.check_struct_lit(name, fields, expr.line, expr.col),

            ExprKind::Field { object, name } => self.check_field(object, name),

            ExprKind::EnumLit { enum_name, variant, args } => {
                self.check_enum_lit(enum_name, variant, args, expr.line, expr.col)
            }

            ExprKind::Match { scrutinee, arms } => self.check_match(scrutinee, arms, expr.line, expr.col),

            ExprKind::Func(fe) => {
                for p in &fe.params {
                    self.ensure_type(&p.ty, p.line, p.col)?;
                }
                self.ensure_type(&fe.return_type, fe.line, fe.col)?;

                // M4.2: con captura. El cuerpo se verifica con los ámbitos
                // envolventes VISIBLES (los parámetros se apilan encima), así que
                // puede referenciar variables externas — una closure. La
                // mutabilidad se respeta (capturar no reata: asignar a un `let`
                // capturado sigue siendo error). Solo guardamos/restauramos el tipo
                // de retorno, que cambia al de esta función.
                let saved_ret = self.current_return.clone();
                let r = self.check_fn_body(&fe.params, &fe.return_type, &fe.body, fe.line, fe.col, "la función anónima");
                self.current_return = saved_ret;
                r?;

                Ok(Type::Fn(
                    fe.params.iter().map(|p| self.resolve_type(&p.ty)).collect(),
                    Box::new(self.resolve_type(&fe.return_type)),
                ))
            }

            ExprKind::If { cond, then_branch, else_branch } => {
                let ct = self.check_expr(cond)?;
                if ct != Type::Bool {
                    return Err(self.err(cond.line, cond.col, format!("la condición del if debe ser bool, no {}", ct)));
                }
                let then_ty = self.check_block(then_branch)?;
                match else_branch {
                    None => {
                        // Un if sin else tiene tipo unit; entonces la rama 'then'
                        // tampoco puede producir un valor útil.
                        if then_ty != Type::Unit {
                            return Err(self.err(expr.line, expr.col, format!(
                                "un if sin else tiene tipo unit, pero su rama produce {} (añade un else)",
                                then_ty
                            )));
                        }
                        Ok(Type::Unit)
                    }
                    Some(else_e) => {
                        let else_ty = self.check_expr(else_e)?;
                        if then_ty != else_ty {
                            return Err(self.err(expr.line, expr.col, format!(
                                "las ramas del if tienen tipos distintos: {} y {}",
                                then_ty, else_ty
                            )));
                        }
                        Ok(then_ty)
                    }
                }
            }

            ExprKind::While { cond, body } => {
                let ct = self.check_expr(cond)?;
                if ct != Type::Bool {
                    return Err(self.err(cond.line, cond.col, format!("la condición del while debe ser bool, no {}", ct)));
                }
                // El valor del cuerpo se descarta en cada iteración; el while es unit.
                self.check_block(body)?;
                Ok(Type::Unit)
            }

            ExprKind::Block(b) => self.check_block(b),
        }
    }

    /// Verifica un bloque en su propio ámbito y devuelve su tipo-valor (el de la
    /// expresión final, o unit si no hay).
    fn check_block(&mut self, block: &Block) -> Result<Type, TypeError> {
        self.push_scope();
        for stmt in &block.statements {
            self.check_stmt(stmt)?;
        }
        let ty = match &block.tail {
            Some(e) => self.check_expr(e)?,
            None => Type::Unit,
        };
        self.pop_scope();
        Ok(ty)
    }

    fn check_binary(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        line: usize,
        col: usize,
    ) -> Result<Type, TypeError> {
        let lt = self.check_expr(left)?;
        let rt = self.check_expr(right)?;
        use BinaryOp::*;
        match op {
            // Aritméticos: ambos int → int, ambos float → float. Sin mezclas.
            Add | Sub | Mul | Div | Rem => match (&lt, &rt) {
                (Type::Int, Type::Int) => Ok(Type::Int),
                (Type::Float, Type::Float) => Ok(Type::Float),
                _ => Err(self.err(line, col, format!(
                    "el operador '{}' requiere ambos operandos int o ambos float, no {} y {}",
                    bin_op_str(op), lt, rt
                ))),
            },
            // Orden: solo números, del mismo tipo → bool.
            Lt | Le | Gt | Ge => match (&lt, &rt) {
                (Type::Int, Type::Int) | (Type::Float, Type::Float) => Ok(Type::Bool),
                _ => Err(self.err(line, col, format!(
                    "el operador '{}' compara números del mismo tipo, no {} y {}",
                    bin_op_str(op), lt, rt
                ))),
            },
            // Igualdad: mismo tipo y comparable → bool.
            Eq | Ne => {
                if lt == rt && is_comparable(&lt) {
                    Ok(Type::Bool)
                } else {
                    Err(self.err(line, col, format!(
                        "el operador '{}' requiere ambos operandos del mismo tipo comparable, no {} y {}",
                        bin_op_str(op), lt, rt
                    )))
                }
            }
            // Lógicos: ambos bool → bool.
            And | Or => {
                if lt == Type::Bool && rt == Type::Bool {
                    Ok(Type::Bool)
                } else {
                    Err(self.err(line, col, format!(
                        "el operador '{}' requiere operandos bool, no {} y {}",
                        bin_op_str(op), lt, rt
                    )))
                }
            }
        }
    }

    fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        line: usize,
        col: usize,
    ) -> Result<Type, TypeError> {
        // Si el callee no es un nombre, debe ser una expresión de tipo función
        // (p. ej. `(fn(x: int) -> int { x })(3)` o `dame_fn()(3)`). (M4.1)
        let name = match &callee.kind {
            ExprKind::Ident(n) => n.clone(),
            _ => {
                let ty = self.check_expr(callee)?;
                return self.call_type(ty, args, line, col);
            }
        };

        // 'print' es un builtin que el checker conoce (DESIGN.md §7): acepta un
        // único argumento de un tipo imprimible y devuelve unit.
        if name == "print" {
            if args.len() != 1 {
                return Err(self.err(line, col, format!("print espera 1 argumento, se le pasaron {}", args.len())));
            }
            let at = self.check_expr(&args[0])?;
            if !is_printable(&at) {
                return Err(self.err(args[0].line, args[0].col, format!("print no puede imprimir un {}", at)));
            }
            return Ok(Type::Unit);
        }

        // 'len(a) -> int': longitud de un arreglo.
        if name == "len" {
            if args.len() != 1 {
                return Err(self.err(line, col, format!("len espera 1 argumento, se le pasaron {}", args.len())));
            }
            let at = self.check_expr(&args[0])?;
            if !matches!(at, Type::Array(_)) {
                return Err(self.err(args[0].line, args[0].col, format!("len espera un arreglo, no {}", at)));
            }
            return Ok(Type::Int);
        }

        // 'push(a, x) -> unit': agrega x al final del arreglo a (lo muta).
        if name == "push" {
            if args.len() != 2 {
                return Err(self.err(line, col, format!("push espera 2 argumentos (arreglo, valor), se le pasaron {}", args.len())));
            }
            let elem = match self.check_expr(&args[0])? {
                Type::Array(e) => *e,
                other => return Err(self.err(args[0].line, args[0].col, format!("push espera un arreglo como primer argumento, no {}", other))),
            };
            let vt = self.check_expr(&args[1])?;
            if vt != elem {
                return Err(self.err(args[1].line, args[1].col, format!("push: el arreglo es de {} pero se empuja {}", elem, vt)));
            }
            return Ok(Type::Unit);
        }

        // Una variable local que guarda una función: llamada indirecta (M4.1).
        // (Tapa a una función global con el mismo nombre.)
        if let Some(v) = self.lookup(&name) {
            let ty = v.ty.clone();
            return self.call_type(ty, args, line, col);
        }

        // Función de nivel superior: llamada directa.
        if let Some(sig) = self.functions.get(&name) {
            let (type_params, params, ret) = (sig.type_params.clone(), sig.params.clone(), sig.ret.clone());
            let label = format!("'{}'", name);
            if type_params.is_empty() {
                // No genérica: aridad y tipos exactos.
                return self.check_args(&params, ret, args, &label, line, col);
            }
            // Genérica: inferir los argumentos de tipo unificando con los argumentos.
            return self.check_generic_call(&type_params, &params, &ret, args, &label, line, col);
        }

        Err(self.err(line, col, format!("función '{}' no declarada", name)))
    }

    /// Verifica una llamada cuyo *callee* es un valor (no un nombre directo): su
    /// tipo debe ser una función, y los argumentos deben encajar con su firma.
    fn call_type(&mut self, ty: Type, args: &[Expr], line: usize, col: usize) -> Result<Type, TypeError> {
        match ty {
            Type::Fn(params, ret) => self.check_args(&params, *ret, args, "la función", line, col),
            other => Err(self.err(line, col, format!(
                "no se puede llamar un valor de tipo {} (no es una función)",
                other
            ))),
        }
    }

    /// Comprueba aridad y tipos de los argumentos contra una firma `(params -> ret)`
    /// y devuelve `ret`. Compartido por las llamadas directas y las indirectas.
    fn check_args(&mut self, params: &[Type], ret: Type, args: &[Expr], label: &str, line: usize, col: usize) -> Result<Type, TypeError> {
        if args.len() != params.len() {
            return Err(self.err(line, col, format!(
                "{} espera {} argumento(s), se le pasaron {}",
                label, params.len(), args.len()
            )));
        }
        for (i, (arg, expected)) in args.iter().zip(params.iter()).enumerate() {
            let at = self.check_expr(arg)?;
            if at != *expected {
                return Err(self.err(arg.line, arg.col, format!(
                    "argumento {} de {}: se esperaba {}, se pasó {}",
                    i + 1, label, expected, at
                )));
            }
        }
        Ok(ret)
    }

    /// Verifica una llamada a una función **genérica** (M6.1): infiere sus argumentos
    /// de tipo unificando los tipos de los parámetros con los de los argumentos, y
    /// devuelve el tipo de retorno ya sustituido. Si algún parámetro de tipo no queda
    /// determinado por los argumentos, es error (M6.1 no usa el tipo esperado).
    fn check_generic_call(
        &mut self,
        type_params: &[String],
        params: &[Type],
        ret: &Type,
        args: &[Expr],
        label: &str,
        line: usize,
        col: usize,
    ) -> Result<Type, TypeError> {
        if args.len() != params.len() {
            return Err(self.err(line, col, format!(
                "{} espera {} argumento(s), se le pasaron {}",
                label, params.len(), args.len()
            )));
        }
        // σ: parámetro de tipo → tipo concreto inferido.
        let mut sigma: HashMap<String, Type> = HashMap::new();
        for (i, (arg, param)) in args.iter().zip(params.iter()).enumerate() {
            let at = self.check_expr(arg)?;
            unify(param, &at, &mut sigma).map_err(|reason| self.err(arg.line, arg.col, format!(
                "argumento {} de {}: {}", i + 1, label, reason
            )))?;
        }
        // Todos los parámetros de tipo deben haber quedado determinados.
        for tp in type_params {
            if !sigma.contains_key(tp) {
                return Err(self.err(line, col, format!(
                    "no se pudo inferir el parámetro de tipo '{}' de {} (no aparece en los argumentos)",
                    tp, label
                )));
            }
        }
        Ok(subst(ret, &sigma))
    }

    // ----- Manejo de ámbitos -----

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Declara una variable en el ámbito más interno (permite shadowing del exterior).
    fn declare(&mut self, name: &str, ty: Type, mutable: bool) {
        self.scopes
            .last_mut()
            .expect("siempre hay un ámbito activo al declarar")
            .insert(name.to_string(), VarInfo { ty, mutable });
    }

    /// Busca una variable de dentro hacia afuera.
    fn lookup(&self, name: &str) -> Option<&VarInfo> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn err(&self, line: usize, col: usize, msg: String) -> TypeError {
        TypeError { msg, line, col }
    }
}

// ----- Auxiliares libres -----

/// ¿Pueden compararse con == / != valores de este tipo? (Compuestos: estructural.)
/// Las funciones **no** son comparables (no tienen identidad estructural); un
/// arreglo lo es solo si su elemento lo es.
fn is_comparable(t: &Type) -> bool {
    match t {
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Struct(_) => true,
        Type::Array(elem) => is_comparable(elem),
        // Los enums (M5) no se comparan con ==: pueden ser recursivos y portar
        // funciones; se consumen por `match`. (Un `@derive(Eq)` futuro lo abriría.)
        // Un parámetro de tipo (M6) es opaco: podría ser una función o un enum, así
        // que no se puede comparar dentro de código genérico.
        Type::Unit | Type::Fn(_, _) | Type::Enum(_) | Type::Var(_) => false,
    }
}

/// ¿Puede `print` imprimir este tipo? Las funciones se imprimen como `<fn>`. Todo
/// valor concreto es imprimible, así que un parámetro de tipo (M6) también lo es.
fn is_printable(t: &Type) -> bool {
    matches!(
        t,
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Array(_)
            | Type::Struct(_) | Type::Fn(_, _) | Type::Enum(_) | Type::Var(_)
    )
}

/// **Sustitución** (M6): reemplaza cada `Var(n)` por `σ[n]`, recursivamente. Es cómo
/// se instancia un tipo genérico una vez inferidos sus parámetros: `subst([U], {U↦int})
/// = [int]`.
fn subst(ty: &Type, sigma: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Var(n) => sigma.get(n).cloned().unwrap_or_else(|| ty.clone()),
        Type::Array(e) => Type::Array(Box::new(subst(e, sigma))),
        Type::Fn(ps, r) => Type::Fn(
            ps.iter().map(|p| subst(p, sigma)).collect(),
            Box::new(subst(r, sigma)),
        ),
        // Primitivos, Struct y Enum: sin argumentos de tipo en M6.1, nada que sustituir.
        other => other.clone(),
    }
}

/// **Unificación** (M6), asimétrica: `param` viene de la firma de la función llamada
/// (sus `Var` son las **incógnitas** a inferir); `arg` viene del contexto del llamador
/// (sus `Var`, si los hay, son **rígidos**/opacos). Liga las incógnitas en `σ` y exige
/// consistencia; cualquier desacuerdo es un error con su razón.
fn unify(param: &Type, arg: &Type, sigma: &mut HashMap<String, Type>) -> Result<(), String> {
    // Incógnita del lado de la firma: ligarla (o exigir que coincida con lo ya ligado).
    if let Type::Var(n) = param {
        if let Some(prev) = sigma.get(n) {
            if prev != arg {
                return Err(format!("'{}' no puede ser {} y {} a la vez", n, prev, arg));
            }
        } else {
            sigma.insert(n.clone(), arg.clone());
        }
        return Ok(());
    }
    match (param, arg) {
        (Type::Array(a), Type::Array(b)) => unify(a, b, sigma),
        (Type::Fn(p1, r1), Type::Fn(p2, r2)) => {
            if p1.len() != p2.len() {
                return Err(format!("se esperaba {}, se pasó {}", param, arg));
            }
            for (a, b) in p1.iter().zip(p2) {
                unify(a, b, sigma)?;
            }
            unify(r1, r2, sigma)
        }
        // Resto (primitivos, Struct, Enum, Var rígido del llamador): igualdad exacta.
        _ if param == arg => Ok(()),
        _ => Err(format!("se esperaba {}, se pasó {}", param, arg)),
    }
}

fn bin_op_str(op: BinaryOp) -> &'static str {
    use BinaryOp::*;
    match op {
        Add => "+", Sub => "-", Mul => "*", Div => "/", Rem => "%",
        Eq => "==", Ne => "!=", Lt => "<", Le => "<=", Gt => ">", Ge => ">=",
        And => "&&", Or => "||",
    }
}

/// Análisis de divergencia: ¿todos los caminos de este bloque terminan en `return`?
/// Es una aproximación *conservadora* (sólida): si dice `true`, es seguro que el
/// bloque siempre retorna; si dice `false`, puede que sí o que no. Eso basta para
/// permitir omitir la expresión final cuando el cuerpo ya retorna por todas partes.
fn block_diverges(block: &Block) -> bool {
    block.statements.iter().any(stmt_diverges)
        || block.tail.as_ref().is_some_and(|t| expr_diverges(t))
}

fn stmt_diverges(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Return { .. } => true,
        StmtKind::Expr(e) => expr_diverges(e),
        _ => false,
    }
}

fn expr_diverges(expr: &Expr) -> bool {
    match &expr.kind {
        // Un if diverge solo si AMBAS ramas divergen (si falta el else, puede caer).
        ExprKind::If { then_branch, else_branch: Some(els), .. } => {
            block_diverges(then_branch) && expr_diverges(els)
        }
        ExprKind::Block(b) => block_diverges(b),
        // Un match diverge si TODOS sus brazos divergen (el checker garantiza que es
        // exhaustivo, así que siempre se toma alguno).
        ExprKind::Match { arms, .. } => !arms.is_empty() && arms.iter().all(|a| expr_diverges(&a.body)),
        _ => false,
    }
}

// =====================================================================
// Resolución de la construcción de enums (M5)
// =====================================================================
//
// `Enum.Variante(args)` y `obj.campo` comparten forma sintáctica, así que el parser
// no puede distinguirlos. Conocidos los nombres de enum, estas funciones recorren el
// AST y **reescriben** los `Field`/`Call` cuya cabeza es un enum en nodos `EnumLit`.
// Se ejecuta una vez, antes de verificar; los dos motores reciben el AST resuelto.

fn resolve_block(block: &mut Block, enums: &HashSet<String>) {
    for stmt in &mut block.statements {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } => resolve_expr(value, enums),
            StmtKind::Assign { target, value } => {
                resolve_expr(target, enums);
                resolve_expr(value, enums);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    resolve_expr(v, enums);
                }
            }
            StmtKind::Expr(e) => resolve_expr(e, enums),
        }
    }
    if let Some(t) = &mut block.tail {
        resolve_expr(t, enums);
    }
}

fn resolve_expr(expr: &mut Expr, enums: &HashSet<String>) {
    // Detectar la construcción de enum ANTES de recorrer los hijos. Si no, el `Field`
    // de la cabeza (`Enum.Variante`) se reescribiría como variante *nullary* antes de
    // que el `Call` que lo envuelve lo viera, perdiendo el payload.

    // Caso 1: `Enum.Variante(args)` — un Call cuyo callee es un Field con cabeza enum.
    if let ExprKind::Call { callee, args } = &mut expr.kind {
        if let ExprKind::Field { object, name } = &callee.kind {
            if is_enum_head(object, enums) {
                let enum_name = ident_name(object);
                let variant = name.clone();
                let mut args = std::mem::take(args);
                for a in &mut args {
                    resolve_expr(a, enums); // el payload sí se resuelve
                }
                expr.kind = ExprKind::EnumLit { enum_name, variant, args };
                return;
            }
        }
    }
    // Caso 2: `Enum.Variante` sin payload — un Field con cabeza enum.
    if let ExprKind::Field { object, name } = &expr.kind {
        if is_enum_head(object, enums) {
            expr.kind = ExprKind::EnumLit {
                enum_name: ident_name(object),
                variant: name.clone(),
                args: Vec::new(),
            };
            return;
        }
    }

    // Caso general: recorrer los sub-nodos.
    match &mut expr.kind {
        ExprKind::Unary { expr: inner, .. } => resolve_expr(inner, enums),
        ExprKind::Binary { left, right, .. } => {
            resolve_expr(left, enums);
            resolve_expr(right, enums);
        }
        ExprKind::Call { callee, args } => {
            resolve_expr(callee, enums);
            for a in args {
                resolve_expr(a, enums);
            }
        }
        ExprKind::ArrayLit(elems) => {
            for e in elems {
                resolve_expr(e, enums);
            }
        }
        ExprKind::Index { array, index } => {
            resolve_expr(array, enums);
            resolve_expr(index, enums);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                resolve_expr(e, enums);
            }
        }
        ExprKind::Field { object, .. } => resolve_expr(object, enums),
        ExprKind::Func(fe) => resolve_block(&mut fe.body, enums),
        ExprKind::Match { scrutinee, arms } => {
            resolve_expr(scrutinee, enums);
            for arm in arms {
                resolve_expr(&mut arm.body, enums);
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            resolve_expr(cond, enums);
            resolve_block(then_branch, enums);
            if let Some(e) = else_branch {
                resolve_expr(e, enums);
            }
        }
        ExprKind::While { cond, body } => {
            resolve_expr(cond, enums);
            resolve_block(body, enums);
        }
        ExprKind::Block(b) => resolve_block(b, enums),
        // Literales, Ident, EnumLit: nada que recorrer.
        _ => {}
    }
}

/// ¿Es `expr` un identificador que nombra un enum?
fn is_enum_head(expr: &Expr, enums: &HashSet<String>) -> bool {
    matches!(&expr.kind, ExprKind::Ident(n) if enums.contains(n))
}

/// Extrae el nombre de un `ExprKind::Ident` (precondición: `is_enum_head` fue cierto).
fn ident_name(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Ident(n) => n.clone(),
        _ => unreachable!("ident_name exige un Ident"),
    }
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// Lexea, parsea y verifica un fuente completo.
    fn check_src(src: &str) -> Result<(), TypeError> {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        check(&mut prog)
    }

    /// Atajo: ¿el mensaje de error contiene esta subcadena?
    fn err_contains(src: &str, needle: &str) {
        let e = check_src(src).expect_err("debería fallar la verificación");
        assert!(
            e.msg.contains(needle),
            "mensaje '{}' no contiene '{}'",
            e.msg,
            needle
        );
    }

    #[test]
    fn fib_es_valido() {
        let src = r#"
fn fib(n: int) -> int {
    if (n < 2) { n } else { fib(n - 1) + fib(n - 2) }
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
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn aritmetica_mezclada_falla() {
        err_contains("fn main() -> int { 1 + true }", "requiere ambos operandos");
        err_contains("fn main() { let x: float = 1 + 2.0; }", "requiere ambos operandos");
    }

    #[test]
    fn condicion_debe_ser_bool() {
        err_contains("fn main() { if (1) { } }", "condición del if debe ser bool");
        err_contains("fn main() { while (1) { } }", "condición del while debe ser bool");
    }

    #[test]
    fn ramas_del_if_mismo_tipo() {
        err_contains(
            "fn main() -> int { if (true) { 1 } else { true } }",
            "ramas del if tienen tipos distintos",
        );
    }

    #[test]
    fn if_sin_else_debe_ser_unit() {
        err_contains("fn main() { if (true) { 5 } }", "sin else tiene tipo unit");
    }

    #[test]
    fn asignar_a_let_falla_pero_a_var_ok() {
        err_contains(
            "fn main() { let x: int = 0; x = 1; }",
            "es inmutable",
        );
        assert!(check_src("fn main() { var x: int = 0; x = 1; }").is_ok());
    }

    #[test]
    fn variable_no_declarada() {
        err_contains("fn main() -> int { x }", "no declarado");
        err_contains("fn main() { y = 1; }", "no declarada");
    }

    #[test]
    fn tipo_de_declaracion_debe_coincidir() {
        err_contains("fn main() { let x: int = true; }", "se inicializa con bool");
    }

    #[test]
    fn retorno_incorrecto() {
        err_contains("fn f() -> int { true } fn main() {}", "produce bool");
        err_contains("fn g() -> int { return true; } fn main() {}", "se devuelve bool");
    }

    #[test]
    fn retorno_temprano_sin_valor_final_es_valido() {
        // Gracias al análisis de divergencia, esto es válido aunque no tenga
        // expresión final: todos los caminos retornan.
        let src = r#"
fn signo(x: int) -> int {
    if (x < 0) { return -1; } else { return 1; }
}
fn main() -> int { signo(3) }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn llamadas_validan_aridad_y_tipos() {
        err_contains(
            "fn add(a: int, b: int) -> int { a + b } fn main() -> int { add(1) }",
            "espera 2 argumento",
        );
        err_contains(
            "fn add(a: int, b: int) -> int { a + b } fn main() -> int { add(1, true) }",
            "se esperaba int, se pasó bool",
        );
        err_contains("fn main() -> int { desconocida() }", "no declarada");
    }

    #[test]
    fn print_builtin() {
        assert!(check_src("fn main() { print(42); print(\"hola\"); print(true); }").is_ok());
        err_contains("fn main() { print(); }", "espera 1 argumento");
        err_contains("fn main() { print(1, 2); }", "espera 1 argumento");
    }

    #[test]
    fn main_obligatoria_y_bien_formada() {
        err_contains("fn otra() -> int { 0 }", "falta la función de entrada 'main'");
        err_contains("fn main(x: int) -> int { x }", "no debe recibir parámetros");
        err_contains("fn main() -> bool { true }", "debe devolver int o unit");
    }

    #[test]
    fn shadowing_en_bloque_interno() {
        // Una variable interior puede tapar a una exterior con otro tipo.
        let src = r#"
fn main() -> int {
    let x: int = 1;
    { let x: bool = true; print(x); }
    x
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn funcion_no_declarada_dos_veces() {
        err_contains("fn f() {} fn f() {} fn main() {}", "declarada dos veces");
    }

    // ----- M3.1: arreglos -----

    #[test]
    fn arreglos_validos() {
        assert!(check_src("fn main() -> int { let a: [int] = [1, 2, 3]; a[0] }").is_ok());
        assert!(check_src("fn main() -> int { let a: [int] = []; push(a, 1); len(a) }").is_ok());
        assert!(check_src("fn main() { var a: [int] = [1]; a[0] = 9; }").is_ok());
        // Arreglos anidados.
        assert!(check_src("fn main() -> int { let m: [[int]] = [[1, 2], [3, 4]]; m[1][0] }").is_ok());
    }

    #[test]
    fn arreglos_errores_de_tipo() {
        err_contains("fn main() -> int { let a: [int] = [1, true]; a[0] }", "mismo tipo");
        err_contains("fn main() -> int { let a: [int] = [1]; a[true] }", "índice debe ser int");
        err_contains("fn main() -> int { let x: int = 5; x[0] }", "no es un arreglo");
        err_contains("fn main() { let x: int = []; }", "arreglo vacío");
        err_contains("fn main() -> int { let a: [int] = [1]; a[0] = true; a[0] }", "se le asigna bool");
        err_contains("fn main() -> int { len(5) }", "len espera un arreglo");
        err_contains("fn main() { let a: [int] = [1]; push(a, true); }", "se empuja bool");
    }

    // ----- M3.2: structs -----

    #[test]
    fn structs_validos() {
        assert!(check_src("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 1, y: 2 }; p.x + p.y }").is_ok());
        assert!(check_src("struct P { x: int } fn main() { var p: P = P { x: 1 }; p.x = 9; }").is_ok());
        // Campos en otro orden: válido.
        assert!(check_src("struct P { x: int, y: int } fn main() -> int { let p: P = P { y: 2, x: 1 }; p.x }").is_ok());
        // Structs anidados y como parámetro.
        assert!(check_src(
            "struct P { x: int } struct L { a: P, b: P }
             fn f(l: L) -> int { l.a.x } fn main() -> int { f(L { a: P { x: 1 }, b: P { x: 2 } }) }"
        ).is_ok());
    }

    #[test]
    fn structs_errores() {
        err_contains("fn main() { let p: Foo = Foo { x: 1 }; }", "no declarado");
        err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: true }; p.x }", "se esperaba int");
        err_contains("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 1 }; p.x }", "falta el campo");
        err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: 1, z: 2 }; p.x }", "no tiene un campo");
        err_contains("struct P { x: int } fn main() -> int { let p: P = P { x: 1 }; p.y }", "no tiene un campo");
        err_contains("struct P { x: int } fn main() -> int { let n: int = 5; n.x }", "no es un struct");
        err_contains("struct P {} struct P {} fn main() {}", "declarado dos veces");
    }

    // ----- M4.1: funciones de primera clase -----

    #[test]
    fn funciones_primera_clase_validas() {
        // Anónima en variable, con su tipo función.
        assert!(check_src("fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x + 1 }; f(2) }").is_ok());
        // De orden superior: recibe y aplica una función.
        assert!(check_src(
            "fn aplicar(f: fn(int) -> int, x: int) -> int { f(x) }
             fn main() -> int { aplicar(fn(n: int) -> int { n * n }, 3) }"
        ).is_ok());
        // Un nombre de función es un valor de tipo función.
        assert!(check_src(
            "fn inc(n: int) -> int { n + 1 }
             fn main() -> int { let g: fn(int) -> int = inc; g(4) }"
        ).is_ok());
        // Devolver una función.
        assert!(check_src(
            "fn dame() -> fn(int) -> int { fn(n: int) -> int { n } }
             fn main() -> int { let f: fn(int) -> int = dame(); f(5) }"
        ).is_ok());
        // Sin argumentos y retorno unit.
        assert!(check_src("fn main() { let f: fn() = fn() { print(1); }; f() }").is_ok());
    }

    #[test]
    fn funciones_primera_clase_errores() {
        // Tipo de la anónima no coincide con la anotación.
        err_contains(
            "fn main() { let f: fn(int) -> int = fn(x: bool) -> int { 0 }; }",
            "se inicializa con",
        );
        // Aridad incorrecta en una llamada indirecta.
        err_contains(
            "fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x }; f(1, 2) }",
            "espera 1 argumento",
        );
        // Tipo de argumento incorrecto en una llamada indirecta.
        err_contains(
            "fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x }; f(true) }",
            "se esperaba int, se pasó bool",
        );
        // Llamar a algo que no es función.
        err_contains("fn main() -> int { let x: int = 3; x(1) }", "no es una función");
        // El cuerpo de la anónima no respeta su tipo de retorno.
        err_contains(
            "fn main() { let f: fn() -> int = fn() -> int { true }; }",
            "produce bool",
        );
    }

    // ----- M4.2: closures (captura de entorno) -----

    #[test]
    fn closures_capturan_el_entorno() {
        // Captura de un `let` externo (lectura).
        assert!(check_src(
            "fn main() -> int { let b: int = 10; let f: fn(int) -> int = fn(x: int) -> int { x + b }; f(1) }"
        ).is_ok());
        // Captura de un `var` externo y su mutación.
        assert!(check_src(
            "fn contador() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
             fn main() -> int { let c: fn() -> int = contador(); c() }"
        ).is_ok());
        // Captura transitiva (dos niveles).
        assert!(check_src(
            "fn sumador(x: int) -> fn(int) -> int { fn(y: int) -> int { x + y } }
             fn main() -> int { let add5: fn(int) -> int = sumador(5); add5(10) }"
        ).is_ok());
    }

    #[test]
    fn closure_no_puede_reasignar_un_let_capturado() {
        // Capturar no reata: asignar a un `let` externo sigue siendo error.
        err_contains(
            "fn main() { let b: int = 1; let f: fn() = fn() { b = 2; }; f() }",
            "es inmutable",
        );
    }

    #[test]
    fn funciones_no_son_comparables() {
        err_contains(
            "fn inc(n: int) -> int { n } fn main() -> int { if (inc == inc) { 1 } else { 0 } }",
            "mismo tipo comparable",
        );
    }

    // ----- M5.1: enums (tipos suma) y construcción -----

    #[test]
    fn enum_construccion_valida() {
        let src = r#"
enum Figura { Circulo(float), Rect(float, float), Punto }
fn area(f: Figura) -> Figura { f }
fn main() {
    let a: Figura = Figura.Circulo(2.0);
    let b: Figura = Figura.Rect(3.0, 4.0);
    let c: Figura = Figura.Punto;
    print(a); print(b); print(c); print(area(a));
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn enum_recursivo_es_valido() {
        // Un enum puede portar su propio tipo: el norte de M5 (listas, árboles).
        let src = r#"
enum Lista { Cons(int, Lista), Nil }
fn main() { let xs: Lista = Lista.Cons(1, Lista.Cons(2, Lista.Nil)); print(xs); }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn enum_variante_inexistente() {
        err_contains("enum E { A, B } fn main() { let x: E = E.C; print(x); }", "no tiene la variante 'C'");
    }

    #[test]
    fn enum_aridad_incorrecta() {
        err_contains("enum E { A(int) } fn main() { let x: E = E.A(1, 2); print(x); }", "espera 1 argumento");
    }

    #[test]
    fn enum_tipo_de_payload_incorrecto() {
        err_contains("enum E { A(int) } fn main() { let x: E = E.A(true); print(x); }", "se esperaba int, se dio bool");
    }

    #[test]
    fn enum_no_es_comparable() {
        err_contains(
            "enum E { A, B } fn main() -> int { let x: E = E.A; if (x == E.B) { 1 } else { 0 } }",
            "mismo tipo comparable",
        );
    }

    #[test]
    fn enum_y_struct_no_comparten_nombre() {
        err_contains("enum E { A } struct E { x: int } fn main() {}", "no puede ser también un struct");
    }

    #[test]
    fn enum_variante_repetida() {
        err_contains("enum E { A, A } fn main() {}", "variante 'A' repetida");
    }

    #[test]
    fn enum_declarado_dos_veces() {
        err_contains("enum E { A } enum E { B } fn main() {}", "declarado dos veces");
    }

    #[test]
    fn enum_como_tipo_desconocido() {
        // Anotar con un nombre que no es ni struct ni enum.
        err_contains("fn main() { let x: NoExiste = 1; print(x); }", "no declarado");
    }

    // ----- M5.2: match y exhaustividad -----

    #[test]
    fn match_exhaustivo_es_valido() {
        let src = r#"
enum Lista { Cons(int, Lista), Nil }
fn suma(xs: Lista) -> int {
    match (xs) {
        Lista.Cons(h, t) => h + suma(t),
        Lista.Nil => 0,
    }
}
fn main() -> int { suma(Lista.Cons(1, Lista.Nil)) }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn match_con_comodin_es_exhaustivo() {
        let src = "enum E { A, B, C } fn f(e: E) -> int { match (e) { E.A => 1, _ => 0 } } fn main() {}";
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn match_no_exhaustivo() {
        err_contains(
            "enum E { A, B, C } fn f(e: E) -> int { match (e) { E.A => 1, E.B => 2 } } fn main() {}",
            "no exhaustivo",
        );
    }

    #[test]
    fn match_brazos_de_tipos_distintos() {
        err_contains(
            "enum E { A, B } fn f(e: E) -> int { match (e) { E.A => 1, E.B => true } } fn main() {}",
            "tipos distintos",
        );
    }

    #[test]
    fn match_variante_repetida() {
        err_contains(
            "enum E { A, B } fn f(e: E) -> int { match (e) { E.A => 1, E.A => 2, E.B => 3 } } fn main() {}",
            "ya está cubierta",
        );
    }

    #[test]
    fn match_brazo_inalcanzable_tras_catchall() {
        err_contains(
            "enum E { A, B } fn f(e: E) -> int { match (e) { otra => 0, E.A => 1 } } fn main() {}",
            "inalcanzable",
        );
    }

    #[test]
    fn match_aridad_de_binding_incorrecta() {
        err_contains(
            "enum E { A(int) } fn f(e: E) -> int { match (e) { E.A => 1 } } fn main() {}",
            "liga 0 valor(es), pero la variante tiene 1",
        );
    }

    #[test]
    fn match_sobre_no_enum() {
        err_contains(
            "fn f(n: int) -> int { match (n) { _ => 0 } } fn main() {}",
            "match requiere un enum",
        );
    }

    #[test]
    fn match_patron_de_otro_enum() {
        err_contains(
            "enum E { A } enum F { B } fn f(e: E) -> int { match (e) { F.B => 1, _ => 0 } } fn main() {}",
            "es del enum 'F'",
        );
    }

    #[test]
    fn match_liga_payload_para_el_cuerpo() {
        // El binding del payload debe estar disponible (y bien tipado) en el cuerpo.
        let src = "enum Caja { Con(int), Vacia } fn val(c: Caja) -> int { match (c) { Caja.Con(n) => n + 1, Caja.Vacia => 0 } } fn main() {}";
        assert!(check_src(src).is_ok());
    }

    // ----- M6.1: funciones genéricas e inferencia -----

    #[test]
    fn generica_identidad_y_uso() {
        let src = r#"
fn identidad<T>(x: T) -> T { x }
fn main() -> int {
    let a: int = identidad(5);
    let b: bool = identidad(true);
    if (b) { a } else { 0 }
}
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn generica_infiere_de_varios_argumentos() {
        // [T] y fn(T)->U determinan T y U a la vez.
        let src = r#"
fn aplicar<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }
fn doble(n: int) -> int { n * 2 }
fn main() -> int { aplicar(doble, 21) }
"#;
        assert!(check_src(src).is_ok());
    }

    #[test]
    fn generica_T_inconsistente() {
        err_contains(
            "fn par<T>(a: T, b: T) -> T { a } fn main() -> int { par(1, true) }",
            "no puede ser int y bool",
        );
    }

    #[test]
    fn generica_T_no_inferible() {
        err_contains(
            "fn vacio<T>() -> int { 0 } fn main() -> int { vacio() }",
            "no se pudo inferir el parámetro de tipo 'T'",
        );
    }

    #[test]
    fn generica_como_valor_es_error() {
        err_contains(
            "fn id<T>(x: T) -> T { x } fn main() -> int { let f: fn(int) -> int = id; f(3) }",
            "función genérica 'id' como valor",
        );
    }

    #[test]
    fn generica_no_se_puede_comparar_un_parametro_de_tipo() {
        err_contains(
            "fn ig<T>(a: T, b: T) -> bool { a == b } fn main() {}",
            "mismo tipo comparable",
        );
    }

    #[test]
    fn parametro_de_tipo_repetido() {
        err_contains("fn f<T, T>(x: T) -> T { x } fn main() {}", "parámetro de tipo 'T' repetido");
    }

    #[test]
    fn tipo_desconocido_no_es_parametro() {
        err_contains("fn f(x: Desconocido) -> int { 0 } fn main() {}", "'Desconocido' no declarado");
    }
}
