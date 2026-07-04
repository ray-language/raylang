//! AST (árbol de sintaxis abstracta) de raylang.
//!
//! El parser (`parser.rs`) transforma la secuencia plana de tokens en este árbol,
//! que *sí* tiene estructura: una llamada contiene sus argumentos, un `if`
//! contiene su condición y sus ramas, etc. El AST es la moneda de cambio entre el
//! front-end y las fases siguientes (checker, intérprete).
//!
//! Es "abstracto" porque descarta detalles sintácticos irrelevantes para la
//! semántica: paréntesis de agrupación, espacios, el `;` final. Solo guarda la
//! forma esencial.
//!
//! Cada nodo de expresión y sentencia lleva su posición `(line, col)` para poder
//! reportar errores con ubicación en las fases posteriores.

/// Tipos que el programador puede escribir en M1 (DESIGN.md §4).
///
/// `Unit` no se escribe nunca de forma explícita: es el tipo de retorno implícito
/// de una función sin `-> ...`. Lo modelamos aquí para uniformar.
///
/// Nota de diseño: este enum está pensado para **crecer** (futuros structs, enums,
/// `Option<T>`, `Result<T,E>` añadirán una variante tipo `Named(String, Vec<Type>)`),
/// como dice la nota de arquitectura de DESIGN.md §4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Bool,
    String,
    Char, // M11.4c
    Bytes, // M16.1a: secuencia inmutable de octetos (datos binarios)
    /// Entero **sin signo con tamaño** (M28.3): `u8`/`u32`/`u64`. El `u8` interno es el ancho en
    /// bits (8/32/64). Aritmética con **wrapping** dentro del ancho; conversión solo con `as`
    /// (sin promoción implícita). `int` sigue siendo el i64 por defecto.
    UInt(u8),
    Unit,
    /// Arreglo dinámico de un tipo de elemento: `[T]`. Tipado estructural (M3).
    Array(Box<Type>),
    /// Una **tupla** `(T1, T2, …)` de 2+ elementos (M27.1): tipos heterogéneos, aridad fija. Tipado
    /// estructural (dos tuplas son iguales si coinciden aridad y tipos posición a posición). En runtime
    /// se **borra a un arreglo** (los arreglos del runtime son heterogéneos): `TupleLit`→`ArrayLit`,
    /// acceso `t.0`→`Index`. Habilita el retorno múltiple y la iteración `(clave, valor)` de `Map`.
    Tuple(Vec<Type>),
    /// Un mapa/diccionario `Map<K, V>` (M13.1): claves `K` (primitivo hashable:
    /// int/string/char/bool) → valores `V`. Semántica de referencia (objeto del heap, como
    /// los arreglos). El parser lo produce como `Struct("Map", [K, V])`; el checker lo
    /// reclasifica aquí (igual que `Enum`/`Var`).
    Map(Box<Type>, Box<Type>),
    /// Un canal tipado `Channel<T>` (M12.1): el medio de comunicación entre green threads (CSP).
    /// Semántica de referencia (objeto del heap). El parser lo produce como `Struct("Channel", [T])`;
    /// el checker lo reclasifica aquí (igual que `Map`). La concurrencia vive solo en la VM.
    Channel(Box<Type>),
    /// Una tarea `Task<T>` (M12.3, structured concurrency): el handle al resultado futuro de un `spawn`.
    /// Semántica de referencia (objeto del heap). El parser la produce como `Struct("Task", [T])`; el
    /// checker la reclasifica aquí (igual que `Channel`/`Map`). Solo la VM.
    Task(Box<Type>),
    /// Un struct nominal, con sus **argumentos de tipo**: `Punto` es
    /// `Struct("Punto", [])`; `Par<int, bool>` es `Struct("Par", [Int, Bool])` (M6).
    /// Tipado **nominal** (M3.2): la igualdad compara nombre y argumentos.
    Struct(String, Vec<Type>),
    /// Un enum (tipo suma) nominal, con sus argumentos de tipo: `Figura` es
    /// `Enum("Figura", [])`; `Option<int>` es `Enum("Option", [Int])` (M5/M6). Como
    /// un identificador en posición de tipo puede ser un struct **o** un enum, el
    /// parser produce siempre `Struct(name, args)` y el checker reclasifica a `Enum`
    /// al resolver el nombre contra la tabla de tipos.
    Enum(String, Vec<Type>),
    /// Un tipo función: `fn(T1, T2) -> R` (M4.1). Las funciones son valores de
    /// primera clase: se pueden pasar, devolver y guardar. Tipado **estructural**
    /// (dos `fn(int) -> int` son el mismo tipo).
    Fn(Vec<Type>, Box<Type>),
    /// Un **parámetro de tipo** genérico: la `T` dentro de `fn id<T>(x: T) -> T`
    /// (M6). Es opaco: dos `Var` solo son iguales si tienen el mismo nombre. El
    /// parser produce `Struct(name)` para cualquier identificador en posición de
    /// tipo; el checker lo reclasifica a `Var` si el nombre es un parámetro de tipo
    /// en ámbito (igual que reclasifica a `Enum`).
    Var(String),
    /// El tipo `Self` dentro de un `trait`/`impl` (M9): denota el tipo implementador.
    /// Como `Var`/`Enum`, el parser produce `Struct("Self")` y el checker lo
    /// reclasifica a `SelfType` cuando hay un tipo implementador en ámbito; al
    /// verificar un `impl`, `Self` se sustituye por el tipo destino concreto.
    SelfType,
    /// Un **trait object** `dyn A + B` (M9.3b/M9.5): un valor cuyo tipo concreto se borra,
    /// pero que carga su vtable para despachar los métodos en runtime. El `Vec` es el conjunto
    /// de traits, **canónico** (ordenado y sin duplicados); `dyn A` es `Dyn(["A"])`. En runtime
    /// se realiza como un struct sintetizado `__dyn_A+B { data, métodos de la unión... }`.
    Dyn(Vec<String>),
    /// Un **puntero opaco** foráneo (`ptr`, M41.4b): la dirección de un objeto de C (un `FILE*`, un
    /// handle de sqlite, …). Es un escalar **opaco**: se puede recibir de C, pasar a C, guardar y
    /// comparar por identidad, pero NO desreferenciar ni operar aritméticamente. El GC no lo traza.
    Ptr,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => f.write_str("int"),
            Type::Float => f.write_str("float"),
            Type::Bool => f.write_str("bool"),
            Type::String => f.write_str("string"),
            Type::Char => f.write_str("char"),
            Type::Bytes => f.write_str("bytes"),
            Type::UInt(w) => write!(f, "u{}", w),
            Type::Ptr => f.write_str("ptr"),
            Type::Unit => f.write_str("unit"),
            Type::Array(elem) => write!(f, "[{}]", elem),
            Type::Tuple(ts) => {
                let inner: Vec<String> = ts.iter().map(|t| t.to_string()).collect();
                write!(f, "({})", inner.join(", "))
            }
            Type::Map(k, v) => write!(f, "Map<{}, {}>", k, v),
            Type::Channel(t) => write!(f, "Channel<{}>", t),
            Type::Task(t) => write!(f, "Task<{}>", t),
            Type::Struct(name, args) | Type::Enum(name, args) => {
                f.write_str(name)?;
                if !args.is_empty() {
                    let ps: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                    write!(f, "<{}>", ps.join(", "))?;
                }
                Ok(())
            }
            Type::Fn(params, ret) => {
                let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
                write!(f, "fn({}) -> {}", ps.join(", "), ret)
            }
            Type::Var(name) => f.write_str(name),
            Type::SelfType => f.write_str("Self"),
            Type::Dyn(traits) => write!(f, "dyn {}", traits.join(" + ")),
        }
    }
}

/// Una **anotación** (M10.1): un metadato `@nombre` o `@nombre(arg, …)` adherido a una
/// declaración. Los argumentos son identificadores (p. ej. `@derive(Eq)`). El conjunto de
/// nombres válidos es **cerrado** (lo conoce el compilador); el checker rechaza los demás.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub name: String,
    pub args: Vec<String>,
    pub line: usize,
    pub col: usize,
}

/// Un programa completo: definiciones de tipos (struct/enum) y funciones de nivel
/// superior.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub functions: Vec<Function>,
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
    /// Constantes de nivel superior (M27.5): `const NAME: T = <literal>;`. Su valor es un literal
    /// (compile-time); los motores resuelven una referencia `NAME` contra esta tabla.
    pub consts: Vec<ConstDef>,
    /// Declaraciones de trait: contratos de comportamiento (M9). Solo firmas.
    pub traits: Vec<TraitDef>,
    /// Bloques `impl Trait for Tipo` que implementan un trait para un tipo (M9).
    pub impls: Vec<ImplBlock>,
    /// Declaraciones `import M;` del archivo (M11.3). Las consume el *loader*, que carga y
    /// fusiona los módulos; tras esa fase el `Program` fusionado las deja vacías.
    pub imports: Vec<ImportDecl>,
    /// Declaraciones `from M import a [as b];` del archivo (M11.3b). También las consume el
    /// loader: traen funciones `pub` de `M` al ámbito del módulo (sin calificar), con alias.
    pub from_imports: Vec<FromImport>,
    /// Metadata que deja el **loader** para el checker: nombre local de una función `from`-importada
    /// → su nombre global (`modulo::f`). Permite que **UFCS** (`recv.f(...)`) resuelva funciones
    /// importadas, no solo las del módulo de entrada (el azúcar `recv.f` no lo reescribe el loader,
    /// que no tiene tipos; el checker lo usa como *fallback* tras campo/método). Vacío sin imports.
    pub ufcs_aliases: std::collections::HashMap<String, String>,
    /// Spans de expresiones (M33a-2): inicio `(línea, col)` de una expresión → su **fin**
    /// `(línea_fin, col_fin)` (exclusivo), registrado por el parser en `expression()` con
    /// política *max-end* (si dos expresiones comparten inicio, queda la más ancha). Lo
    /// consulta el `err()` del checker para subrayar la expresión completa; una posición
    /// ausente (expresiones sintetizadas por el lowering, prelude) degrada a extensión 1.
    /// El loader lo desplaza y fusiona con las bandas de líneas de cada módulo (L3).
    pub expr_spans: std::collections::HashMap<(usize, usize), (usize, usize)>,
    /// Posición del **nombre del campo/método** en un acceso `recv.name` (M10.2g): la clave es la
    /// posición del acceso `(línea, col)` —que es la del receptor— más el nombre, y el valor es la
    /// `(línea, col)` del propio `name` tras el `.`. El AST `Field` no la guarda (comparte posición
    /// con el receptor); esta tabla la registra el parser para que el LSP dé **hover del campo/
    /// método** en su posición. Mismo esquema de clave que `ufcs_sites`. El loader la desplaza.
    pub field_name_pos: std::collections::HashMap<(usize, usize, String), (usize, usize)>,
    /// Funciones externas (M41, FFI): `extern "lib" { fn nombre(params) -> ret; … }`. Cada una es
    /// una firma **sin cuerpo** cuya implementación vive en una librería C nativa, cargada por
    /// `dlopen`/`dlsym` en tiempo de ejecución (`src/ffi.rs`). El checker valida que los tipos sean
    /// marshalables (primitivos en M41.1) y las registra como llamables; los motores despachan la
    /// llamada a `ffi::call` en vez de ejecutar un cuerpo. Es **la** frontera insegura del lenguaje.
    pub externs: Vec<ExternFn>,
    /// **Azúcar preservado para el formateador** (M29.3). El parser desazucara la interpolación
    /// `"…${e}…"` a una concatenación `+ to_string(e)` y los pipelines `x |> f(a)` a `f(x, a)` —así el
    /// checker y los motores nunca los ven—, PERO guarda aquí la forma de superficie, indexada por la
    /// posición del **nodo desazucarado raíz**, para que `ray fmt` la reemita sin perderla (antes
    /// convertía los ejemplos idiomáticos en su forma verbosa). Solo lo consulta el formateador;
    /// están vacíos en el programa fusionado del loader (que trabaja sobre el AST desazucarado).
    pub interp_sites: std::collections::HashMap<(usize, usize), Vec<InterpSeg>>,
    /// Ver [`Program::interp_sites`]. `(receptor, rhs)` de un pipeline: `x |> f(a)` → `(x, f(a))`.
    pub pipe_sites: std::collections::HashMap<(usize, usize), (Expr, Expr)>,
}

/// Un segmento de una cadena interpolada preservada para el formateador (M29.3): texto literal o una
/// expresión `${…}` (ya parseada). Reconstruye `"a${x}b"` a partir de `[Lit("a"), Expr(x), Lit("b")]`.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpSeg {
    Lit(String),
    Expr(Expr),
}

/// Una función externa (M41, FFI): una firma sin cuerpo asociada a una librería C (`lib`). El nombre
/// es a la vez el identificador en raylang y el símbolo a resolver con `dlsym`. Sin genéricos ni
/// `pub`/anotaciones en M41.1. Declarar una `extern fn` es el acto consciente de abrir la frontera
/// insegura; llamarla luego se ve como cualquier función.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternFn {
    pub name: String,
    /// Nombre corto de la librería (`"m"` → `libm`); lo resuelve `ffi` al archivo de plataforma o al
    /// handle global del proceso (donde viven libc/libm ya enlazadas).
    pub lib: String,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub line: usize,
    pub col: usize,
}

/// Una declaración `import M;` / `import a/b/c [as x];` (M11.3 / M11.5): importa el módulo dado por
/// su **ruta** (`module`, con `/` como separador de directorios, archivo `a/b/c.ray`) como espacio
/// de nombres. El nombre local es el **leaf** (último segmento) o el `alias`; sus ítems `pub` se
/// acceden calificados (`c.f(...)`). La ruta solo aparece aquí; las referencias usan el leaf.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub module: String,
    pub alias: Option<String>,
    pub line: usize,
    pub col: usize,
}

impl ImportDecl {
    /// El nombre local con el que el módulo queda en ámbito: el alias si lo hay, si no el último
    /// segmento de la ruta (`import geo/formas/circulo;` → `circulo`).
    pub fn leaf(&self) -> &str {
        self.alias.as_deref()
            .unwrap_or_else(|| self.module.rsplit('/').next().unwrap_or(&self.module))
    }
}

/// Una declaración `[pub] from M import a [as b]{, c [as d]};` (M11.3b / M11.6a): trae nombres `pub`
/// del módulo `M` al ámbito del módulo actual, sin calificar, con renombrado opcional (`as`). Si
/// lleva `pub` (**reexport**, M11.6a), los nombres traídos pasan también a la **superficie pública**
/// de este módulo —se usa en un `mod.ray` para construir la cara pública de su directorio/cápsula—.
#[derive(Debug, Clone, PartialEq)]
pub struct FromImport {
    pub module: String,
    pub names: Vec<ImportName>,
    pub is_pub: bool,
    pub line: usize,
    pub col: usize,
}

/// Un nombre traído por un `from`-import: el nombre original en el módulo origen y, opcionalmente,
/// el alias local con el que se usará (`as b`). `local()` devuelve el nombre efectivo en ámbito.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportName {
    pub name: String,
    pub alias: Option<String>,
    pub line: usize,
    pub col: usize,
}

impl ImportName {
    /// El nombre con el que el ítem queda en ámbito: el alias si lo hay, si no el original.
    pub fn local(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

/// Una constante de nivel superior (M27.5): `const NAME: T = value;`. `value` es un literal.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstDef {
    pub name: String,
    pub ty: Type,
    pub value: Expr,
    pub is_pub: bool,
    pub line: usize,
    pub col: usize,
}

/// Definición de un struct: `struct Nombre { campo: Tipo, ... }` (M3.2). Los campos
/// se guardan **en orden de declaración**.
#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    /// Anotaciones de la declaración (M10.1), p. ej. `@derive(Eq)`. Vacío si no hay.
    pub annotations: Vec<Annotation>,
    /// `pub` (M11.3c): el tipo se exporta de su módulo. Sin `pub`, privado al módulo.
    pub is_pub: bool,
    pub name: String,
    /// Parámetros de tipo: los `A, B` de `struct Par<A, B> { ... }` (M6). Vacío = no
    /// genérico. En los tipos de los campos, esos nombres aparecen como `Type::Var`.
    pub type_params: Vec<String>,
    /// Bounds de los parámetros de tipo: los `T: Show` de `struct Caja<T: Show>` (M9.4). Vacío =
    /// sin bounds. Se verifican en la **construcción** del struct (no hay runtime).
    pub bounds: Vec<(String, String)>,
    pub fields: Vec<(String, Type)>,
    pub line: usize,
    pub col: usize,
}

/// Definición de un enum (tipo suma): `enum Nombre { Variante(tipos...), ... }`
/// (M5). Las variantes se guardan **en orden de declaración**. Una variante lleva
/// un *payload* posicional (cero o más tipos); sin tipos es una variante *unit*.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    /// Anotaciones de la declaración (M10.1), p. ej. `@derive(Eq)`. Vacío si no hay.
    pub annotations: Vec<Annotation>,
    /// `pub` (M11.3c): el tipo se exporta de su módulo. Sin `pub`, privado al módulo.
    pub is_pub: bool,
    pub name: String,
    /// Parámetros de tipo: los `T` de `enum Option<T> { ... }` (M6). Vacío = no
    /// genérico. En las variantes, esos nombres aparecen como `Type::Var`.
    pub type_params: Vec<String>,
    /// Bounds de los parámetros de tipo: los `T: Eq` de `enum Lista<T: Eq>` (M9.4). Vacío = sin
    /// bounds. Se verifican en la **construcción** de una variante (no hay runtime).
    pub bounds: Vec<(String, String)>,
    pub variants: Vec<VariantDef>,
    pub line: usize,
    pub col: usize,
}

/// Una variante de enum: su nombre y su payload posicional (`Vec` vacío = unit).
#[derive(Debug, Clone, PartialEq)]
pub struct VariantDef {
    pub name: String,
    pub payload: Vec<Type>,
    pub line: usize,
    pub col: usize,
}

/// Declaración de un **trait** (M9): un contrato de comportamiento. Lista las
/// **firmas** de los métodos que un tipo debe proveer para "ser" un `Trait`. No
/// tiene cuerpos (los métodos por defecto se difieren a M9.3).
#[derive(Debug, Clone, PartialEq)]
pub struct TraitDef {
    /// `pub` (M11.3c): el trait se exporta de su módulo. Sin `pub`, privado al módulo.
    pub is_pub: bool,
    pub name: String,
    /// Parámetros de tipo del trait (M28.2): los `S` de `trait From<S>`. Vacío = trait
    /// ordinario (M9). Habilitan el patrón de conversión de `From<S>` que consume `?`.
    pub type_params: Vec<String>,
    pub methods: Vec<MethodSig>,
    pub line: usize,
    pub col: usize,
}

/// La **firma** de un método de trait (M9): `fn nombre(self, p: T, ...) -> R;`.
/// `params` incluye `self` como **primer** parámetro (su `ty` es `Type::SelfType`).
#[derive(Debug, Clone, PartialEq)]
pub struct MethodSig {
    pub name: String,
    /// Parámetros de tipo **propios del método** (M40.2c): los `U` de `fn map<U>(self, ...)`.
    /// Vacío = método no genérico (M9). Se suman a los del impl al bajar el método a función.
    pub type_params: Vec<String>,
    /// Bounds de esos parámetros de tipo (M40.2c): pares `(parámetro, trait)`. Vacío = sin bounds.
    pub bounds: Vec<(String, String)>,
    pub params: Vec<Param>,
    pub return_type: Type, // Unit si se omitió el `-> ...`
    /// Cuerpo **por defecto** (M9.3a): `Some` si la firma trae un bloque en vez de `;`.
    /// Un impl que no redefina el método hereda este cuerpo (con `Self` = el tipo destino).
    pub default_body: Option<Block>,
    pub line: usize,
    pub col: usize,
}

/// Un bloque **`impl Trait for Tipo { ... }`** (M9): da los cuerpos de los métodos
/// del trait para un tipo. `target` es el tipo implementador. Cada método es una
/// `Function` ordinaria cuyo primer parámetro es `self` (de tipo `Self`, que el checker
/// sustituye por `target`).
///
/// `type_params`/`bounds` (M9.2b) habilitan **impls genéricos**: `impl<T: Mostrable>
/// Trait for Caja<T>`. Vacíos = impl concreto (M9.1). Cuando hay parámetros de tipo,
/// `target` los menciona (`Caja<T>`) y cada método se baja a una función genérica acotada.
#[derive(Debug, Clone, PartialEq)]
pub struct ImplBlock {
    pub trait_name: String,
    /// Argumentos de tipo del trait (M28.2): los `string` de `impl From<string> for MiError`.
    /// Vacío para un trait sin parámetros de tipo (M9). Distinguen `impl From<string> for E` de
    /// `impl From<IoError> for E` (mismo target y trait, distinta conversión).
    pub trait_args: Vec<Type>,
    /// Parámetros de tipo del impl (M9.2b): los `T` de `impl<T> Trait for Caja<T>`.
    pub type_params: Vec<String>,
    /// Bounds de esos parámetros (M9.2b): `impl<T: A + B> ...`. Pares `(parámetro, trait)`.
    pub bounds: Vec<(String, String)>,
    pub target: Type,
    pub methods: Vec<Function>,
    pub line: usize,
    pub col: usize,
}

/// Una función: `fn nombre<params de tipo>(params) -> retorno { cuerpo }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// Anotaciones de la declaración (M10.1), p. ej. `@test`. Vacío si no hay.
    pub annotations: Vec<Annotation>,
    /// `pub` (M11.3): la función se exporta de su módulo. Sin `pub`, privada al módulo.
    pub is_pub: bool,
    pub name: String,
    /// Parámetros de tipo: los `T, U` de `fn mapear<T, U>(...)` (M6). Vacío = no
    /// genérica. Dentro del cuerpo, cada nombre está en ámbito como `Type::Var`.
    pub type_params: Vec<String>,
    /// Bounds de los parámetros de tipo (M9.2): pares `(parámetro, trait)` de
    /// `fn f<T: A + B>(...)`. Vacío = sin bounds. Cada bound exige que `T` implemente el
    /// trait, y habilita llamar sus métodos sobre un valor de tipo `T` dentro del cuerpo.
    pub bounds: Vec<(String, String)>,
    pub params: Vec<Param>,
    pub return_type: Type, // Unit si se omitió el `-> ...`
    pub body: Block,
    pub line: usize,
    pub col: usize,
}

/// Una función **anónima** usada como expresión: `fn(x: int) -> int { x + 1 }`
/// (M4.1). Comparte la forma de una función nombrada salvo el nombre.
///
/// `id` es un identificador único asignado por el parser (un contador global de
/// fn-exprs). Sirve para que el intérprete y el compilador asocien cada literal de
/// función con su entrada en la tabla de funciones, sin depender de punteros.
#[derive(Debug, Clone, PartialEq)]
pub struct FnExpr {
    pub id: usize,
    pub params: Vec<Param>,
    pub return_type: Type, // Unit si se omitió el `-> ...`
    pub body: Block,
    pub line: usize,
    pub col: usize,
}

/// Un parámetro formal: `nombre: tipo`.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub line: usize,
    pub col: usize,
}

/// Un bloque `{ sentencias... [expresión-final] }`.
///
/// `tail` es la expresión final **sin** `;`: el valor del bloque (DESIGN.md §6).
/// Si es `None`, el bloque vale `unit`.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub line: usize,
    pub col: usize,
    /// Línea (1-basada) del `}` de cierre. La usa el **formateador** para colocar un comentario que
    /// va justo antes del `}` dentro del bloque (sin ella se reubicaba tras el `}`). En bloques
    /// **sintéticos** (checker/desugar) vale lo mismo que `line` (nunca se formatean).
    pub end_line: usize,
}

/// Una sentencia: se ejecuta por su efecto, no produce un valor de bloque.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// `let x: T = e;` (mutable=false) o `var x: T = e;` (mutable=true).
    ///
    /// `ty` es **opcional** (M8.1): si la anotación se omite (`let x = e;`), el tipo de
    /// la variable se infiere del inicializador en el checker.
    Let {
        name: String,
        ty: Option<Type>,
        value: Expr,
        mutable: bool,
    },
    /// Desestructuración de **tupla** en un `let`/`var` (M27.1): `let (a, b) = e;`. Cada nombre liga una
    /// posición de la tupla (`_` = descartar, `None`). El checker exige que `value` sea una tupla de la
    /// misma aridad y liga cada nombre con el tipo de su posición; en runtime se evalúa `value` a un
    /// arreglo y se liga por índice.
    LetTuple {
        names: Vec<Option<String>>,
        value: Expr,
        mutable: bool,
    },
    /// Bucle `for` (M27.2): `for x in iterable { … }`. El iterable puede ser un **rango** `a..b`
    /// (`ForIter::Range`), o una colección (`ForIter::In`): arreglo (`x` = elemento), string (`x` =
    /// `char`), o `Map` (el patrón debe ser una tupla `(k, v)`). Azúcar de front-end sobre `while`;
    /// cada motor lo ejecuta directamente (bind de la(s) variable(s) por iteración).
    For {
        pat: ForPat,
        iter: ForIter,
        body: Block,
    },
    /// Asignación a un *lvalue*: `x = e;`, `a[i] = e;`, `p.x = e;` (M3.2).
    /// `target` es una expresión asignable (`Ident`, `Index`, o `Field`).
    Assign { target: Expr, value: Expr },
    /// `return e;` o `return;`.
    Return { value: Option<Expr> },
    /// Una expresión usada como sentencia: su valor se descarta. P. ej.
    /// `print(x);` o `if (c) { ... }`.
    Expr(Expr),
}

/// El patrón de variable de un `for` (M27.2): un solo nombre, o una tupla `(k, v)` (para iterar `Map`).
#[derive(Debug, Clone, PartialEq)]
pub enum ForPat {
    Single(String),
    Tuple(Vec<Option<String>>),
}

/// El iterable de un `for` (M27.2): un rango `a..b` o una colección.
#[derive(Debug, Clone, PartialEq)]
pub enum ForIter {
    Range { start: Expr, end: Expr },
    In(Expr),
    /// `for x in it` sobre un tipo que implementa `Iterator<T>` (M40.2). El parser produce `In`; el
    /// checker lo reescribe a esto cuando el iterable es un iterador, guardando el nombre manglado de
    /// su método `next` (`Contador#next`). Los motores iteran llamando a `next` hasta `None`.
    Iter { expr: Expr, next_fn: String },
}

/// Una expresión: produce un valor.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    // --- Literales ---
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Char(char), // M11.4c
    Bytes(Vec<u8>), // M16.1a: literal de bytes `b"..."`

    /// Referencia a una variable o función por nombre.
    Ident(String),

    /// Operador unario: `-x`, `!b`.
    Unary { op: UnaryOp, expr: Box<Expr> },

    /// Operador binario: `a + b`, `x < y`, `p && q`.
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },

    /// Llamada: `f(a, b)`. `callee` es lo que se llama (en M1, casi siempre un
    /// `Ident`, pero lo dejamos general para el futuro UFCS/closures).
    Call { callee: Box<Expr>, args: Vec<Expr> },

    /// Literal de arreglo: `[1, 2, 3]` (o `[]` vacío). (M3)
    ArrayLit(Vec<Expr>),

    /// Literal de **tupla**: `(a, b, …)` de 2+ elementos (M27.1). El checker le da un `Type::Tuple` y lo
    /// **baja a `ArrayLit`** para el runtime (erasure). Un `(e)` de un solo elemento es un paréntesis, no
    /// una tupla.
    TupleLit(Vec<Expr>),

    /// Indexación: `a[i]`. (M3)
    Index { array: Box<Expr>, index: Box<Expr> },

    /// Conversión numérica `expr as T` (M27.4): int↔float, char↔int. Cambia la representación en runtime
    /// (a diferencia de las tuplas/genéricos que son erasure) → cada motor la ejecuta según el tipo del
    /// valor y el destino.
    Cast { expr: Box<Expr>, ty: Type },

    /// Literal de struct: `Punto { x: 1, y: 2 }`. (M3.2)
    StructLit { name: String, fields: Vec<(String, Expr)> },

    /// Acceso a campo: `p.x`. (M3.2)
    Field { object: Box<Expr>, name: String },

    /// Construcción de una variante de enum: `Figura.Circulo(2.0)` o, sin payload,
    /// `Figura.Punto` (`args` vacío). (M5)
    ///
    /// El parser **no** produce este nodo: `Enum.Variante` es sintácticamente igual
    /// a un acceso a campo `obj.campo`. Lo genera la **resolución** del checker, que
    /// reescribe los `Field`/`Call` cuya cabeza es un nombre de enum (ver
    /// `checker::resolve_enum_construction`). Así la ambigüedad se decide una sola
    /// vez y los dos motores reciben un AST explícito.
    EnumLit { enum_name: String, variant: String, args: Vec<Expr> },

    /// Función anónima como valor: `fn(x: int) -> int { x + 1 }`. (M4.1)
    Func(Box<FnExpr>),

    /// `match (escrutinio) { patrón => cuerpo, ... }` (M5.2). Es una **expresión**:
    /// produce el valor del brazo que casa. Los brazos se prueban en orden.
    Match { scrutinee: Box<Expr>, arms: Vec<MatchArm> },

    /// Operador de propagación `expr?` (M6.3). Sobre un `Result<T, E>` o un
    /// `Option<T>`: si es `Ok(v)`/`Some(v)`, vale `v`; si es `Err(e)`/`None`, hace
    /// que la función **retorne** ese valor. La función envolvente debe declarar un
    /// retorno compatible (lo valida el checker).
    Try(Box<Expr>),

    // --- Expresiones con bloque (producen valor, DESIGN.md §6) ---
    /// `if (cond) { then } else { ... }`. `else_branch`, si existe, es otro `Expr`
    /// que será un `Block` o (en cadenas `else if`) otro `If`.
    If {
        cond: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Box<Expr>>,
    },
    /// `while (cond) { body }`. Su valor siempre es `unit`.
    While { cond: Box<Expr>, body: Block },
    /// Un bloque usado como expresión: `{ ...; valor }`.
    Block(Block),
}

/// Un brazo de `match`: un patrón y el cuerpo (una expresión) que se evalúa si casa.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    /// Guarda opcional (M40.1a): `patrón if <cond> => …`. El brazo solo casa si el patrón liga Y la
    /// guarda (una expresión `bool`, con los bindings del patrón en ámbito) evalúa a `true`; si no,
    /// se sigue al brazo siguiente. Un brazo con guarda **no** cuenta para la exhaustividad.
    pub guard: Option<Expr>,
    pub body: Expr,
    pub line: usize,
    pub col: usize,
}

/// Un patrón de `match` (M5.2). Patrones **planos** (un nivel): variante, comodín o
/// binding. Lleva ubicación para los errores de exhaustividad/aridad.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    /// `_`: descarta, no liga. Cubre **todo** lo restante (catch-all).
    Wildcard,
    /// Un identificador suelto: liga el escrutinio completo a ese nombre. También
    /// catch-all (cubre todo lo restante).
    Binding(String),
    /// `Enum.Variante(sub-patrones)`: casa con esa variante y aplica un **sub-patrón** a cada
    /// posición del payload (M40.1c: recursivo). Un sub-patrón puede ser `_` (descarta), un binding
    /// (liga), u **otra variante anidada** (`Result.Ok(Option.Some(v))`). Sin `(…)`, sin payload.
    Variant {
        enum_name: String,
        variant: String,
        subpatterns: Vec<Pattern>,
    },
    /// `Nombre { campo [: sub-patrón], … }` (M40.1d): destructura un struct. La forma corta
    /// `Nombre { x, y }` liga cada campo a un binding de su nombre (`x: x`); la larga `x: <patrón>`
    /// aplica un sub-patrón. Solo aparece **anidado** (el escrutinio de un match es un enum).
    Struct {
        name: String,
        fields: Vec<(String, Pattern)>,
    },
}

/// Recolecta todas las funciones anónimas (`FnExpr`) del programa, **indexadas por
/// su `id`**: el resultado en la posición `i` es la fn-expr con `id == i`. (M4.1)
///
/// Tanto el intérprete como el compilador necesitan una tabla de funciones que
/// incluya las anónimas; este recorrido del AST la construye una sola vez. Como el
/// parser asigna ids densos (`0..n`), el vector queda completo y sin huecos.
pub fn collect_fn_exprs(program: &Program) -> Vec<&FnExpr> {
    let mut acc: Vec<&FnExpr> = Vec::new();
    for f in &program.functions {
        walk_block(&f.body, &mut acc);
    }
    // Colocar cada fn-expr en la posición de su id.
    let mut by_id: Vec<Option<&FnExpr>> = (0..acc.len()).map(|_| None).collect();
    for fe in acc {
        by_id[fe.id] = Some(fe);
    }
    by_id.into_iter().map(|o| o.unwrap_or_else(|| crate::ice!("hueco en los ids densos de fn-expr"))).collect()
}

fn walk_block<'a>(block: &'a Block, acc: &mut Vec<&'a FnExpr>) {
    for s in &block.statements {
        match &s.kind {
            StmtKind::Let { value, .. } => walk_expr(value, acc),
            StmtKind::LetTuple { value, .. } => walk_expr(value, acc),
            StmtKind::For { iter, body, .. } => {
                match iter {
                    ForIter::Range { start, end } => { walk_expr(start, acc); walk_expr(end, acc); }
                    ForIter::In(e) => walk_expr(e, acc),
                    ForIter::Iter { expr, .. } => walk_expr(expr, acc),
                }
                walk_block(body, acc);
            }
            StmtKind::Assign { target, value } => {
                walk_expr(target, acc);
                walk_expr(value, acc);
            }
            StmtKind::Return { value } => {
                if let Some(e) = value {
                    walk_expr(e, acc);
                }
            }
            StmtKind::Expr(e) => walk_expr(e, acc),
        }
    }
    if let Some(t) = &block.tail {
        walk_expr(t, acc);
    }
}

fn walk_expr<'a>(expr: &'a Expr, acc: &mut Vec<&'a FnExpr>) {
    match &expr.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) | ExprKind::Char(_) | ExprKind::Bytes(_) | ExprKind::Ident(_) => {}
        ExprKind::Unary { expr, .. } => walk_expr(expr, acc),
        ExprKind::Binary { left, right, .. } => {
            walk_expr(left, acc);
            walk_expr(right, acc);
        }
        ExprKind::Call { callee, args } => {
            walk_expr(callee, acc);
            for a in args {
                walk_expr(a, acc);
            }
        }
        ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
            for e in elems {
                walk_expr(e, acc);
            }
        }
        ExprKind::Index { array, index } => {
            walk_expr(array, acc);
            walk_expr(index, acc);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, e) in fields {
                walk_expr(e, acc);
            }
        }
        ExprKind::Field { object, .. } => walk_expr(object, acc),
        ExprKind::Cast { expr, .. } => walk_expr(expr, acc),
        ExprKind::EnumLit { args, .. } => {
            for a in args {
                walk_expr(a, acc);
            }
        }
        ExprKind::Func(fe) => {
            acc.push(fe);
            walk_block(&fe.body, acc); // fn-exprs anidadas
        }
        ExprKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, acc);
            for arm in arms {
                walk_expr(&arm.body, acc);
            }
        }
        ExprKind::Try(inner) => walk_expr(inner, acc),
        ExprKind::If { cond, then_branch, else_branch } => {
            walk_expr(cond, acc);
            walk_block(then_branch, acc);
            if let Some(e) = else_branch {
                walk_expr(e, acc);
            }
        }
        ExprKind::While { cond, body } => {
            walk_expr(cond, acc);
            walk_block(body, acc);
        }
        ExprKind::Block(b) => walk_block(b, acc),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg, // -
    Not, // !
    BitNot, // ~  (NOT bit a bit, M19.3a): operando int → int.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add, // +
    Sub, // -
    Mul, // *
    Div, // /
    Rem, // %
    Eq,  // ==
    Ne,  // !=
    Lt,  // <
    Le,  // <=
    Gt,  // >
    Ge,  // >=
    And, // &&
    Or,  // ||
    // Bit a bit (M19.3a): ambos operandos int → int. Habilitan SHA-1 / base64 / framing WS.
    BitAnd, // &
    BitOr,  // |
    BitXor, // ^
    Shl,    // <<
    Shr,    // >>
}
