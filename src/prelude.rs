//! El **prelude** de raylang (M6.3, M7.3).
//!
//! El prelude es la **biblioteca estándar** mínima, **escrita en el propio raylang** y
//! inyectada en cada programa antes de verificarlo. No hay nada incrustado en el
//! compilador: son definiciones normales que el checker, el intérprete y la VM tratan
//! como cualquier otra.
//!
//! - **Tipos (M6.3):** `Option<T>` y `Result<T, E>`, enums genéricos. El lenguaje no
//!   los trata especial salvo por el operador `?`. Que el modelo de errores sea "solo
//!   librería" es deliberado: el mismo mecanismo permite al usuario definir su propio
//!   `Either<A, B>` con el mismo poder.
//! - **Funciones de orden superior (M7.3):** `map`, `filter`, `fold`. Son la prueba de
//!   que con los genéricos (M6), los closures (M4) y los builtins `len`/`push` ya se
//!   puede escribir librería útil **dentro del lenguaje**, sin tocar el runtime. Lucen
//!   con UFCS (`xs.map(f)`) y pipelines (`xs |> map(f)`).

use crate::ast::{EnumDef, Function, StructDef, TraitDef};

/// El código fuente del prelude. Se parsea una vez; sus enums y funciones se anteponen
/// a los del programa del usuario.
pub const SOURCE: &str = r#"
/// An optional value: `Some(T)` when present, `None` when absent.
/// raylang has no null; use `Option` to model a possibly-missing value.
enum Option<T> {
    Some(T),
    None,
}

/// The result of a fallible operation: `Ok(T)` on success, `Err(E)` with the error value on failure.
/// Works with the `?` operator to propagate errors.
enum Result<T, E> {
    Ok(T),
    Err(E),
}

// Igualdad estructural (M10.1). `@derive(Eq)` genera el `impl` para un struct/enum.
// Usa `Self` en posición de argumento, así que no es invocable sobre un `dyn Eq`
// (object safety): se compara entre valores concretos, `a.eq(b)`.
/// Structural equality between two values of the same concrete type.
/// Derivable with `@derive(Eq)`; not object-safe (uses `Self` as an argument).
trait Eq {
    /// Returns true when `self` and `otro` are structurally equal.
    fn eq(self, other: Self) -> bool;
}

// Representación textual (limpieza post-M11, L2). `@derive(Show)` genera el `impl` para un
// struct/enum. No usa `Self` fuera del receptor, así que sí es object-safe (`dyn Show`).
/// Textual representation of a value. Derivable with `@derive(Show)`; object-safe (`dyn Show`).
trait Show {
    /// Returns a human-readable string representation of `self`.
    fn show(self) -> string;
}

// Orden total (M11.7d): `self < other`. Lo usa `sort`. Los primitivos lo implementan vía el
// operador `<` (extendido a string/char en M11.7d); un tipo del usuario lo implementa a mano.
/// Total ordering (`self < other`). Used by `sort`; primitives implement it via the `<` operator.
trait Ord {
    /// Returns true when `self` orders strictly before `otro`.
    fn less(self, other: Self) -> bool;
}

impl Ord for int {
    fn less(self, other: int) -> bool { self < other }
}

impl Ord for float {
    fn less(self, other: float) -> bool { self < other }
}

impl Ord for string {
    fn less(self, other: string) -> bool { self < other }
}

impl Ord for char {
    fn less(self, other: char) -> bool { self < other }
}

// Longitud (M48.4): número de elementos/caracteres/entradas/octetos de una colección. Los tipos
// incorporados lo implementan vía el primitivo `__len` (mismo opcode que el antiguo builtin `len`);
// un tipo del usuario puede implementarlo para su propia colección y usarse con `fn f<T: Len>(...)`.
/// The number of elements/characters/entries/octets in a collection.
trait Len {
    /// Returns the length of `self`.
    fn len(self) -> int;
}

impl Len for string {
    fn len(self) -> int { __len(self) }
}

impl<T> Len for [T] {
    fn len(self) -> int { __len(self) }
}

impl<K, V> Len for Map<K, V> {
    fn len(self) -> int { __len(self) }
}

impl Len for bytes {
    fn len(self) -> int { __len(self) }
}

// Agregar al final (M48.4b): añade un elemento a una colección, mutándola. Los arreglos lo
// implementan vía `__push`; un tipo del usuario (una pila, una cola…) puede implementarlo.
/// Appends a value to a growable collection, in place.
trait Push<T> {
    /// Appends `x` to the end of `self`.
    fn push(self, x: T);
}

impl<T> Push<T> for [T] {
    fn push(self, x: T) { __push(self, x) }
}

// Invertir (M48.4b): devuelve una nueva colección con los elementos en orden inverso.
/// Returns a copy of `self` with its elements in reverse order.
trait Reverse {
    /// Returns `self` reversed.
    fn reverse(self) -> Self;
}

impl<T> Reverse for [T] {
    fn reverse(self) -> [T] { __reverse(self) }
}

// Pertenencia/subcadena (M48.4b): ¿`self` contiene `x`? En un string/bytes es subcadena; en un
// arreglo, pertenencia por igualdad. Un solo trait genérico (se acepta la conflación).
/// Whether `self` contains `x` (substring for strings, membership for arrays).
trait Contains<T> {
    /// Returns true when `self` contains `x`.
    fn contains(self, x: T) -> bool;
}

impl Contains<string> for string {
    fn contains(self, x: string) -> bool { __contains(self, x) }
}

impl<T> Contains<T> for [T] {
    fn contains(self, x: T) -> bool { __contains(self, x) }
}

// Operaciones de Map (M48.4c): insertar, consultar la clave y listar claves/valores. Un solo impl
// (Map<K,V>); agrupadas en un trait porque son específicas de Map. `get`/`remove` (que devuelven
// Option) siguen siendo funciones del prelude, no métodos de este trait.
/// Core map operations: `insert`, `contains_key`, `keys`, `values`.
trait MapOps<K, V> {
    /// Inserts or updates the pair `(k, v)`, in place.
    fn insert(self, k: K, v: V);
    /// Whether the map contains `k`.
    fn contains_key(self, k: K) -> bool;
    /// The keys, as a sorted array (deterministic order).
    fn keys(self) -> [K];
    /// The values, in the same order as `keys()`.
    fn values(self) -> [V];
}

impl<K, V> MapOps<K, V> for Map<K, V> {
    fn insert(self, k: K, v: V) { __insert(self, k, v) }

    fn contains_key(self, k: K) -> bool { __contains_key(self, k) }

    fn keys(self) -> [K] { __keys(self) }

    fn values(self) -> [V] { __values(self) }
}

// Operaciones de string (M48.4d): un solo impl (`string`). Durante la coexistencia los cuerpos llaman
// a los builtins públicos (que siguen vivos); M48.4e los renombra a los primitivos `__x` al retirarlos.
// (`char_code` es de `char` y `join` es de `[string]` —no impl-able para un array concreto—: builtins.)
/// String operations: trim, split, replace, chars, case, substring, repeat, to_bytes.
trait StrOps {
    /// Removes leading/trailing whitespace.
    fn trim(self) -> string;
    /// Splits by a separator into parts.
    fn split(self, sep: string) -> [string];
    /// Replaces every occurrence of `old` with `new`.
    fn replace(self, old: string, new: string) -> string;
    /// The characters of the string.
    fn chars(self) -> [char];
    /// Whether the string starts with `prefix`.
    fn starts_with(self, prefix: string) -> bool;
    /// Whether the string ends with `suffix`.
    fn ends_with(self, suffix: string) -> bool;
    /// The string in uppercase.
    fn to_upper(self) -> string;
    /// The string in lowercase.
    fn to_lower(self) -> string;
    /// The substring `[start, end)` by character index (clamped).
    fn substring(self, start: int, end: int) -> string;
    /// The string repeated `times` times.
    fn repeat(self, times: int) -> string;
    /// The UTF-8 encoding of the string.
    fn to_bytes(self) -> bytes;
}

impl StrOps for string {
    fn trim(self) -> string { __trim(self) }

    fn split(self, sep: string) -> [string] { __split(self, sep) }

    fn replace(self, old: string, new: string) -> string { __replace(self, old, new) }

    fn chars(self) -> [char] { __chars(self) }

    fn starts_with(self, prefix: string) -> bool { __starts_with(self, prefix) }

    fn ends_with(self, suffix: string) -> bool { __ends_with(self, suffix) }

    fn to_upper(self) -> string { __to_upper(self) }

    fn to_lower(self) -> string { __to_lower(self) }

    fn substring(self, start: int, end: int) -> string { __substring(self, start, end) }

    fn repeat(self, times: int) -> string { __repeat(self, times) }

    fn to_bytes(self) -> bytes { __to_bytes(self) }
}

// Operaciones de bytes (M48.4d): un solo impl (`bytes`).
/// Bytes operations: slice by octet index.
trait BytesOps {
    /// The byte slice `[start, end)` by octet index (clamped).
    fn sub_bytes(self, start: int, end: int) -> bytes;
}

impl BytesOps for bytes {
    fn sub_bytes(self, start: int, end: int) -> bytes { __sub_bytes(self, start, end) }
}

// Hash (M40.3a): un valor hashable produce un `int`. `@derive(Hash)` lo genera para un struct/enum
// (combinando `.hash()` de sus campos); los primitivos lo implementan aquí, en raylang (el string
// itera sus caracteres con `char_code`). Lo consumen las tablas hash del prelude (Set, M40.3b).
// `float` NO es hashable (como en `Map`): un struct con campo float no puede derivar Hash.
/// Hashes a value to an `int`. Derivable with `@derive(Hash)`; consumed by the hash-based `Set`.
/// `float` is not hashable.
trait Hash {
    fn hash(self) -> int;
}

impl Hash for int {
    fn hash(self) -> int { self }
}

impl Hash for bool {
    fn hash(self) -> int {
        if (self) { 1 } else { 0 }
    }
}

impl Hash for char {
    fn hash(self) -> int { char_code(self) }
}

impl Hash for string {
    fn hash(self) -> int {
        var h = 17;
        let cs = self.chars();
        var i = 0;
        while (i < cs.len()) {
            h = h * 31 + char_code(cs[i]);
            i = i + 1;
        }
        h
    }
}

// Conversión de tipos (M28.2): `From<S>` construye un valor a partir de uno de tipo `S`. Su método
// `desde` NO tiene `self` (es asociado; el nombre es `desde` porque `from` es palabra clave del
// import). Lo consume el operador `?`: sobre un `Result<_, E1>` dentro de una función que devuelve
// `Result<_, E2>`, si hay `impl From<E1> for E2` el error se convierte automáticamente.
/// Conversion from a value of type `S` into `Self`, via the associated function `desde`.
/// The `?` operator uses it to auto-convert error types when an `impl From<E1> for E2` exists.
trait From<S> {
    fn desde(source: S) -> Self;
}

// Iteración (M40.2): un tipo que implemente `Iterator<T>` produce una secuencia de `T` — `next`
// devuelve `Some(elemento)` y avanza el cursor, o `None` cuando se agota. Habilita `for x in it`
// sobre iteradores de usuario (además de arreglos/strings/Map). Como los structs son valores de
// referencia con campos mutables, `next(self)` avanza el estado del propio iterador.
//
// M40.2c: `map`/`filter` son **adaptadores PEREZOSOS** — métodos por DEFECTO del trait, así que
// TODO iterador los tiene, y devuelven otro iterador (`Iter<U>`/`Iter<T>`) que solo calcula al
// recorrerse. Son **métodos genéricos** (`map<U>`): la primera feature de M40.2c. La desambiguación
// con el `map`/`filter` EAGER de arreglos es por el tipo del receptor: un arreglo (que no implementa
// `Iterator`) cae en la función libre; un iterador, en el método del trait.
/// A sequence of values of type `T`. Implementing `next` enables `for x in it` and provides
/// all the lazy adapters (`map`, `filter`, `take`, ...) and terminals (`fold`, `collect`) below.
trait Iterator<T> {
    /// Returns the next element as `Some(x)` and advances the iterator, or `None` when exhausted.
    fn next(self) -> Option<T>;
    // Transforma cada elemento con `f`, perezosamente. `xs.iter().map(f)` no calcula nada hasta el
    // `for`/`next`. Reusa el closure `paso` de `Iter`, que captura `self` (el iterador de origen,
    // por referencia → su estado avanza) y `f`.
    /// Lazily transforms each element with `f`, yielding an `Iter<U>`.
    /// Nothing is computed until the result is consumed.
    fn map<U>(self, f: fn(T) -> U) -> Iter<U> {
        Iter { step: fn() -> Option<U> {
    match (self.next()) {
        Option.Some(x) => Option.Some(f(x)),
        Option.None => Option.None,
    }
} }
    }
    // Conserva solo los elementos que cumplen `pred`, perezosamente. Avanza el iterador de origen
    // hasta el próximo que pasa el filtro (o `None` si se agota).
    /// Lazily keeps only the elements for which `pred` returns true.
    fn filter(self, pred: fn(T) -> bool) -> Iter<T> {
        Iter { step: fn() -> Option<T> {
    var res: Option<T> = Option.None;
    var keep_going = true;
    while (keep_going) {
        match (self.next()) {
            Option.Some(x) => {
                if (pred(x)) {
                    res = Option.Some(x);
                    keep_going = false;
                }
            },
            Option.None => {
                keep_going = false;
            },
        }
    }
    res
} }
    }
    // Perezoso: entrega a lo sumo los primeros `n` elementos, luego se agota. Corta la cadena sin
    // consumir el resto del origen (útil sobre iteradores infinitos/largos).
    /// Lazily yields at most the first `n` elements, then stops without consuming the rest of the source.
    fn take(self, n: int) -> Iter<T> {
        var remaining = n;
        Iter { step: fn() -> Option<T> {
    if (remaining <= 0) {
        Option.None
    } else {
        remaining = remaining - 1;
        self.next()
    }
} }
    }
    // Perezoso: descarta los primeros `n` elementos y entrega el resto. El descarte ocurre en la
    // primera llamada a `next` (el contador capturado se agota una vez).
    /// Lazily discards the first `n` elements and yields the rest.
    fn skip(self, n: int) -> Iter<T> {
        var to_skip = n;
        Iter { step: fn() -> Option<T> {
    while (to_skip > 0) {
        to_skip = to_skip - 1;
        self.next();
    }
    self.next()
} }
    }
    // Perezoso: empareja este iterador con `otra` posición a posición en tuplas `(T, U)`; se agota
    // cuando cualquiera de los dos lo hace. `otra` ha de ser un `Iter<U>` (los adaptadores devuelven
    // `Iter`; un iterador de usuario se convierte con `.map(...)` o similar). Método genérico sobre `U`.
    /// Lazily pairs this iterator with `other`, position by position, into `(T, U)` tuples;
    /// stops as soon as either side is exhausted.
    fn zip<U>(self, other: Iter<U>) -> Iter<(T, U)> {
        Iter { step: fn() -> Option<(T, U)> {
    match (self.next()) {
        Option.Some(a) => match (other.next()) {
            Option.Some(b) => Option.Some((a, b)),
            Option.None => Option.None,
        },
        Option.None => Option.None,
    }
} }
    }
    // Perezoso: empareja cada elemento con su índice (0, 1, 2, …) en una tupla `(int, T)`. Consúmelo
    // destructurando: `for par in it.enumerate() { let (i, x) = par; … }`.
    /// Lazily pairs each element with its index, yielding `(int, T)` tuples starting at 0.
    fn enumerate(self) -> Iter<(int, T)> {
        var i = 0;
        Iter { step: fn() -> Option<(int, T)> {
    match (self.next()) {
        Option.Some(x) => {
            let pair = (i, x);
            i = i + 1;
            Option.Some(pair)
        },
        Option.None => Option.None,
    }
} }
    }
    // TERMINAL: reduce el iterador a un único valor, acumulando de izquierda a derecha desde `init`.
    // A diferencia de map/filter, consume el iterador aquí mismo (no es perezoso). Método genérico
    // sobre el tipo del acumulador `A`.
    /// Terminal: consumes the iterator, accumulating left to right from `init` with `f`,
    /// and returns the final accumulator.
    fn fold<A>(self, init: A, f: fn(A, T) -> A) -> A {
        var acc: A = init;
        var keep_going = true;
        while (keep_going) {
            match (self.next()) {
                Option.Some(x) => {
                    acc = f(acc, x);
                },
                Option.None => {
                    keep_going = false;
                },
            }
        }
        acc
    }
    // TERMINAL: materializa el iterador (perezoso) en un arreglo `[T]`. El puente de vuelta desde la
    // cadena `iter().map().filter()` a un arreglo concreto.
    /// Terminal: consumes the iterator and materializes its elements into a new array `[T]`.
    fn collect(self) -> [T] {
        var out: [T] = [];
        var keep_going = true;
        while (keep_going) {
            match (self.next()) {
                Option.Some(x) => {
                    out.push(x);
                },
                Option.None => {
                    keep_going = false;
                },
            }
        }
        out
    }
}

// `.iter()` sobre arreglos y `range` (M40.2b/c): iteradores de PRIMERA CLASE. La representación es
// **un closure** `step: fn() -> Option<T>` (type-erasure): un iterador ES una función con estado que
// entrega el siguiente elemento o `None`. Así `iter`/`range`/`map`/`filter` producen todos el MISMO
// tipo `Iter<T>` y se encadenan sin bounds sobre traits parametrizados (que raylang no permite). El
// estado (posición, cursor) vive en variables capturadas por el closure (mutadas por referencia).
/// A first-class iterator: a closure `step` that returns the next element or `None`.
/// `iter`, `range` and all the adapters (`map`, `filter`, ...) produce this same type.
struct Iter<T> {
    step: fn() -> Option<T>,
}

impl<T> Iterator<T> for Iter<T> {
    fn next(self) -> Option<T> { self.step() }
}

// Iterador sobre los elementos del arreglo, en orden. `xs.iter()` == `iter(xs)` (UFCS).
/// Returns an iterator over the elements of the array, in order. UFCS: `xs.iter()`.
fn iter<T>(xs: [T]) -> Iter<T> {
    var i = 0;
    Iter { step: fn() -> Option<T> {
    if (i < xs.len()) {
        let v = xs[i];
        i = i + 1;
        Option.Some(v)
    } else {
        Option.None
    }
} }
}

// Iterador sobre los enteros de `desde` (inclusivo) a `hasta` (exclusivo) — el `a..b` del `for`,
// pero como valor de primera clase que se puede pasar, guardar, recorrer y encadenar con map/filter.
/// Returns an iterator over the integers from `start` (inclusive) to `end` (exclusive).
fn range(start: int, end: int) -> Iter<int> {
    var i = start;
    Iter { step: fn() -> Option<int> {
    if (i < end) {
        let v = i;
        i = i + 1;
        Option.Some(v)
    } else {
        Option.None
    }
} }
}

// TERMINAL: suma los elementos de un iterador de enteros (`it.sum()` vía UFCS). Es función libre —no
// método del trait— porque un `sum` genérico necesitaría un cero y un `+` del tipo del elemento, que
// raylang no expresa aún; se especializa a `Iter<int>` (lo más común).
/// Terminal: sums the elements of an integer iterator and returns the total.
fn sum(it: Iter<int>) -> int {
    it.fold(0, fn(a: int, x: int) -> int { a + x })
}

// Traits de sobrecarga de operadores (M28.1): un tipo que implemente estos traits puede usar los
// operadores aritméticos. El checker baja `a + b` (con `a`/`b` de un tipo de usuario) a `a.add(b)`.
/// Operator overloading: `a + b` on a user type dispatches to `a.add(b)`.
trait Add {
    fn add(self, other: Self) -> Self;
}

/// Operator overloading: `a - b` on a user type dispatches to `a.sub(b)`.
trait Sub {
    fn sub(self, other: Self) -> Self;
}

/// Operator overloading: `a * b` on a user type dispatches to `a.mul(b)`.
trait Mul {
    fn mul(self, other: Self) -> Self;
}

/// Operator overloading: `a / b` on a user type dispatches to `a.div(b)`.
trait Div {
    fn div(self, other: Self) -> Self;
}

/// Operator overloading: unary `-a` on a user type dispatches to `a.neg()`.
trait Neg {
    fn neg(self) -> Self;
}

// Eq/Show para los primitivos (M13.2a): los habilita `assert_eq` (que pide `T: Eq + Show`) y, en
// general, cualquier genérico acotado por Eq/Show sobre un primitivo. Vía `==` y `to_string`, que
// ya operan sobre int/float/bool/string/char. (Un tipo del usuario los obtiene con `@derive`.)
impl Eq for int {
    fn eq(self, other: int) -> bool { self == other }
}

impl Eq for float {
    fn eq(self, other: float) -> bool { self == other }
}

impl Eq for string {
    fn eq(self, other: string) -> bool { self == other }
}

impl Eq for bool {
    fn eq(self, other: bool) -> bool { self == other }
}

impl Eq for char {
    fn eq(self, other: char) -> bool { self == other }
}

impl Show for int {
    fn show(self) -> string { to_string(self) }
}

impl Show for float {
    fn show(self) -> string { to_string(self) }
}

impl Show for string {
    fn show(self) -> string { to_string(self) }
}

impl Show for bool {
    fn show(self) -> string { to_string(self) }
}

impl Show for char {
    fn show(self) -> string { to_string(self) }
}

// Ordena ascendente, devolviendo un arreglo NUEVO (insertion sort). `T` debe implementar `Ord`;
// el bound se baja a paso de diccionarios (M9.2), así que `sort` es front-end puro (cero opcodes).
/// Sorts ascending and returns a new array (insertion sort). `T` must implement `Ord`.
fn sort<T: Ord>(a: [T]) -> [T] {
    var out: [T] = [];
    var i: int = 0;
    while (i < a.len()) {
        let x: T = a[i];
        out.push(x);
        var j: int = out.len() - 1;
        while (j > 0 && x.less(out[j - 1])) {
            out[j] = out[j - 1];
            j = j - 1;
        }
        out[j] = x;
        i = i + 1;
    }
    out
}

// --- Mapas Map<K,V> (M13.1) ---
// `get` envuelve el primitivo `__map_get` (que devuelve [V]) en un Option, como los demás
// envoltorios. Las claves son hashables (int/string/char/bool); el checker lo garantiza al
// instanciar K. `map_new`/`insert`/`contains_key`/`len` son builtins (operan directo).
// Valor asociado a la clave `k`, o None si no está.
/// Returns the value associated with key `k` in the map, or `None` if the key is absent.
fn get<K, V>(m: Map<K, V>, k: K) -> Option<V> {
    let r = __map_get(m, k);
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// M13.1b: quita la clave `k` del mapa y devuelve su valor (None si no estaba).
/// Removes key `k` from the map and returns its value, or `None` if it was not present.
fn remove<K, V>(m: Map<K, V>, k: K) -> Option<V> {
    let r = __map_remove(m, k);
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// --- Aserciones (M13.2a) ---
// Sobre el primitivo `panic` (el único toque de runtime). No hay sobrecarga, así que en vez de
// `assert(cond)` y `assert(cond, msg)` se ofrece `assert(cond)` (mensaje genérico), `assert_eq`
// (mensaje detallado con los valores) y, para un mensaje a medida, `panic("...")` directo.
// Falla con un mensaje genérico si la condición no se cumple.
/// Panics with a generic message if the condition is false.
/// For a custom message, call `panic("...")` directly.
fn assert(cond: bool) {
    if (!cond) {
        panic("aserción falló");
    }
}

// Falla mostrando ambos valores si no son iguales. `T` debe ser Eq (comparar) y Show (mostrar);
// los bounds se bajan a diccionarios (M9.2), así que esto es front-end puro sobre `panic`.
/// Panics showing both values if they are not equal.
/// `T` must implement `Eq` (to compare) and `Show` (to display the values).
fn assert_eq<T: Eq + Show>(a: T, b: T) {
    if (!a.eq(b)) {
        panic("assert_eq falló: " + a.show() + " != " + b.show());
    }
}

// --- map/filter/fold EAGER sobre arreglos (M7.3), re-fundados sobre Iterator (M40.6) ---
// Estas funciones libres son la cara ERGONÓMICA de la operación: `xs.map(f)` (con `xs: [T]`) devuelve
// directamente un `[U]` indexable, sin `.iter()`/`.collect()`. Desde M40.6 NO reimplementan el bucle:
// delegan en la maquinaria PEREZOSA del trait `Iterator` (`iter(xs).map(f).collect()`), que es la única
// fuente de verdad de la lógica. El despacho por tipo de receptor evita recursión: dentro de estos
// cuerpos, `iter(xs)` es un `Iter<T>`, así que `.map`/`.filter`/`.fold` resuelven al MÉTODO del trait
// (campo→método→UFCS), nunca a estas funciones libres. Cara eager (materializa) vs. cara lazy
// (`xs.iter().map(f).filter(g).collect()`, fusiona sin arreglos intermedios): ver el libro, m40/iteradores.
// Aplica `f` a cada elemento, devolviendo un arreglo nuevo con los resultados.
/// Applies `f` to each element of the array and returns a new array with the results (eager).
fn map<T, U>(xs: [T], f: fn(T) -> U) -> [U] {
    iter(xs).map(f).collect()
}

// Conserva los elementos para los que `pred` es verdadero, en un arreglo nuevo.
/// Returns a new array with the elements for which `pred` returns true (eager).
fn filter<T>(xs: [T], pred: fn(T) -> bool) -> [T] {
    iter(xs).filter(pred).collect()
}

// Reduce el arreglo a un único valor, acumulando de izquierda a derecha desde `init`.
/// Reduces the array to a single value, accumulating left to right from `init` with `f`.
fn fold<T, A>(xs: [T], init: A, f: fn(A, T) -> A) -> A {
    iter(xs).fold(init, f)
}

// --- I/O (M11.2): envoltorios sobre primitivos builtin que devuelven [T] (vacío/único) ---
// El runtime no sabe de Option: los primitivos devuelven un arreglo de 0 o 1 elementos y aquí,
// en raylang, se traducen a Option con Some/None corrientes (el patrón de la stdlib, M7.3).
// Parsea un entero; None si el texto no es un entero válido.
/// Parses a string as an integer; `None` if the text is not a valid integer.
fn parse_int(s: string) -> Option<int> {
    let r = __parse_int(s);
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// Parsea un flotante; None si el texto no es un flotante válido (M14).
/// Parses a string as a float; `None` if the text is not a valid float.
fn parse_float(s: string) -> Option<float> {
    let r = __parse_float(s);
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// El inverso de char_code (diferido JSON-1); None si n no es un code point válido
// (surrogates D800-DFFF o fuera de [0, 10FFFF]). Habilita los escapes \uXXXX de std/json.
/// Builds a char from a Unicode code point; `None` if the value is not a valid code point
/// (surrogates or out of range). The inverse of `char_code`.
fn char_from_code(n: int) -> Option<char> {
    let r = __char_from_code(n);
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// M49.3: los envoltorios de cripto (ed25519_public_key/sign, chacha20poly1305 seal/open) se movieron al
// módulo `std/crypto` junto a sha*/hmac/ed25519_verify. Aquí quedan solo sus primitivos `__x` (builtins).

// Lee una línea de stdin (sin el salto de línea); None en fin de entrada (EOF).
/// Reads one line from stdin (without the trailing newline); `None` on end of input (EOF).
fn input() -> Option<string> {
    let r = __read_line();
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// Lee una línea y la parsea como entero; None en EOF o si no es un entero.
/// Reads one line from stdin and parses it as an integer; `None` on EOF or invalid integer.
fn read_int() -> Option<int> {
    let s = input()?;
    parse_int(s)
}

// M12.1 (concurrencia): recibe del canal. Some(v) si llega un valor; None si el canal está cerrado y
// vacío. Envuelve el primitivo __recv (que devuelve [T]) en un Option, como input/parse_int. Solo la VM.
/// Receives from the channel: `Some(v)` when a value arrives, `None` when the channel is
/// closed and empty. Blocks while the channel is empty and open. VM only.
fn recv<T>(ch: Channel<T>) -> Option<T> {
    let r = __recv(ch);
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// M56.5: une una tarea SIN re-lanzar su fallo (errores como valores; join sí re-lanza). Envuelve
// el primitivo __task_failed (bloquea hasta que termine; [] = bien, [msg] = falló); para el valor
// reusa join, que con la tarea ya terminada ni bloquea ni falla. Solo la VM.
/// Waits for the task and returns its outcome as a value: `Ok(its result)` if it finished, or
/// `Err(panic message)` if it failed — unlike `join`, which re-raises the failure. VM only.
fn try_join<T>(t: Task<T>) -> Result<T, string> {
    let f = __task_failed(t);
    if (f.len() > 0) {
        return Result.Err(f[0]);
    }
    Result.Ok(join(t))
}

// Valor de una variable de entorno; None si no está definida.
/// Returns the value of an environment variable, or `None` if it is not set.
fn env(name: string) -> Option<string> {
    let r = __env(name);
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7a: índice (de carácter) de la primera ocurrencia de `sub` en `s`; None si no aparece.
/// Returns the character index of the first occurrence of `sub` in `s`, or `None` if absent.
fn index_of(s: string, sub: string) -> Option<int> {
    let r = __index_of(s, sub);
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7b: quita y devuelve el último elemento del arreglo (lo muta); None si está vacío.
/// Removes and returns the last element of the array (mutating it), or `None` if it is empty.
fn pop<T>(a: [T]) -> Option<T> {
    let r = __pop(a);
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7b: índice de la primera ocurrencia de `x` en el arreglo; None si no aparece.
/// Returns the index of the first occurrence of `x` in the array, or `None` if absent.
fn position<T>(a: [T], x: T) -> Option<int> {
    let r = __position(a, x);
    if (r.len() == 0) { Option.None } else { Option.Some(r[0]) }
}

// --- Archivos (M11.2c): el primitivo devuelve un arreglo ETIQUETADO (primer elemento "ok"/"err");
// aquí se traduce a Result. Así el runtime tampoco sabe de Result (como con Option). ---
// M16.1b: decodifica bytes como UTF-8; Ok(string) u Err(mensaje) si no es válido.
/// Decodes bytes as UTF-8; `Ok(string)`, or `Err(message)` if the bytes are not valid UTF-8.
fn from_utf8(b: bytes) -> Result<string, string> {
    let r = __from_utf8(b);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}

"#;

/// Parsea el prelude UNA vez por proceso y cachea el AST (antes, cada accessor re-lexeaba y
/// re-parseaba el fuente entero → 5 pasadas completas por `check()`, en cada arranque del CLI y
/// en cada tecleo del LSP). Los accessors CLONAN del caché: clonar el AST es mucho más barato
/// que re-parsearlo, y los llamadores necesitan copias propias (las mutan al inyectar).
fn parsed() -> &'static crate::ast::Program {
    static P: std::sync::OnceLock<crate::ast::Program> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let tokens = crate::lexer::lex(SOURCE).unwrap_or_else(|e| crate::ice!("el prelude no lexea: {e}"));
        crate::parser::parse(tokens).unwrap_or_else(|e| crate::ice!("el prelude no parsea: {e}"))
    })
}

/// Los enums del prelude (`Option`/`Result`), ya parseados.
pub fn enums() -> Vec<EnumDef> {
    parsed().enums.clone()
}

/// Los structs del prelude (`ArrayIter`/`RangeIter` para `.iter()`/`range`, M40.2b), ya parseados.
pub fn structs() -> Vec<StructDef> {
    parsed().structs.clone()
}

/// Las funciones del prelude (`map`/`filter`/`fold`), ya parseadas.
pub fn functions() -> Vec<Function> {
    parsed().functions.clone()
}

/// Los traits del prelude (`Eq`/`Show`/`Ord`), ya parseados (M10.1).
pub fn traits() -> Vec<TraitDef> {
    parsed().traits.clone()
}

/// Los `impl` del prelude (M11.7d: `Ord` para int/float/string/char), ya parseados.
pub fn impls() -> Vec<crate::ast::ImplBlock> {
    parsed().impls.clone()
}
