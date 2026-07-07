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
enum Option<T> { Some(T), None }
/// The result of a fallible operation: `Ok(T)` on success, `Err(E)` with the error value on failure.
/// Works with the `?` operator to propagate errors.
enum Result<T, E> { Ok(T), Err(E) }

// Igualdad estructural (M10.1). `@derive(Eq)` genera el `impl` para un struct/enum.
// Usa `Self` en posición de argumento, así que no es invocable sobre un `dyn Eq`
// (object safety): se compara entre valores concretos, `a.igual(b)`.
/// Structural equality between two values of the same concrete type.
/// Derivable with `@derive(Eq)`; not object-safe (uses `Self` as an argument).
trait Eq {
    /// Returns true when `self` and `otro` are structurally equal.
    fn igual(self, otro: Self) -> bool;
}

// Representación textual (limpieza post-M11, L2). `@derive(Show)` genera el `impl` para un
// struct/enum. No usa `Self` fuera del receptor, así que sí es object-safe (`dyn Show`).
/// Textual representation of a value. Derivable with `@derive(Show)`; object-safe (`dyn Show`).
trait Show {
    /// Returns a human-readable string representation of `self`.
    fn mostrar(self) -> string;
}

// Orden total (M11.7d): `self < otro`. Lo usa `sort`. Los primitivos lo implementan vía el
// operador `<` (extendido a string/char en M11.7d); un tipo del usuario lo implementa a mano.
/// Total ordering (`self < otro`). Used by `sort`; primitives implement it via the `<` operator.
trait Ord {
    /// Returns true when `self` orders strictly before `otro`.
    fn menor(self, otro: Self) -> bool;
}
impl Ord for int { fn menor(self, otro: int) -> bool { self < otro } }
impl Ord for float { fn menor(self, otro: float) -> bool { self < otro } }
impl Ord for string { fn menor(self, otro: string) -> bool { self < otro } }
impl Ord for char { fn menor(self, otro: char) -> bool { self < otro } }

// Longitud (M48.4): número de elementos/caracteres/entradas/octetos de una colección. Los tipos
// incorporados lo implementan vía el primitivo `__len` (mismo opcode que el antiguo builtin `len`);
// un tipo del usuario puede implementarlo para su propia colección y usarse con `fn f<T: Len>(...)`.
/// The number of elements/characters/entries/octets in a collection.
trait Len {
    /// Returns the length of `self`.
    fn len(self) -> int;
}
impl Len for string { fn len(self) -> int { __len(self) } }
impl<T> Len for [T] { fn len(self) -> int { __len(self) } }
impl<K, V> Len for Map<K, V> { fn len(self) -> int { __len(self) } }
impl Len for bytes { fn len(self) -> int { __len(self) } }

// Agregar al final (M48.4b): añade un elemento a una colección, mutándola. Los arreglos lo
// implementan vía `__push`; un tipo del usuario (una pila, una cola…) puede implementarlo.
/// Appends a value to a growable collection, in place.
trait Push<T> {
    /// Appends `x` to the end of `self`.
    fn push(self, x: T);
}
impl<T> Push<T> for [T] { fn push(self, x: T) { __push(self, x) } }

// Invertir (M48.4b): devuelve una nueva colección con los elementos en orden inverso.
/// Returns a copy of `self` with its elements in reverse order.
trait Reverse {
    /// Returns `self` reversed.
    fn reverse(self) -> Self;
}
impl<T> Reverse for [T] { fn reverse(self) -> [T] { __reverse(self) } }

// Pertenencia/subcadena (M48.4b): ¿`self` contiene `x`? En un string/bytes es subcadena; en un
// arreglo, pertenencia por igualdad. Un solo trait genérico (se acepta la conflación).
/// Whether `self` contains `x` (substring for strings, membership for arrays).
trait Contains<T> {
    /// Returns true when `self` contains `x`.
    fn contains(self, x: T) -> bool;
}
impl Contains<string> for string { fn contains(self, x: string) -> bool { __contains(self, x) } }
impl<T> Contains<T> for [T] { fn contains(self, x: T) -> bool { __contains(self, x) } }

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
    /// Replaces every occurrence of `de` with `a`.
    fn replace(self, de: string, a: string) -> string;
    /// The characters of the string.
    fn chars(self) -> [char];
    /// Whether the string starts with `prefijo`.
    fn starts_with(self, prefijo: string) -> bool;
    /// Whether the string ends with `sufijo`.
    fn ends_with(self, sufijo: string) -> bool;
    /// The string in uppercase.
    fn to_upper(self) -> string;
    /// The string in lowercase.
    fn to_lower(self) -> string;
    /// The substring `[inicio, fin)` by character index (clamped).
    fn substring(self, inicio: int, fin: int) -> string;
    /// The string repeated `veces` times.
    fn repeat(self, veces: int) -> string;
    /// The UTF-8 encoding of the string.
    fn to_bytes(self) -> bytes;
}
impl StrOps for string {
    fn trim(self) -> string { __trim(self) }
    fn split(self, sep: string) -> [string] { __split(self, sep) }
    fn replace(self, de: string, a: string) -> string { __replace(self, de, a) }
    fn chars(self) -> [char] { __chars(self) }
    fn starts_with(self, prefijo: string) -> bool { __starts_with(self, prefijo) }
    fn ends_with(self, sufijo: string) -> bool { __ends_with(self, sufijo) }
    fn to_upper(self) -> string { __to_upper(self) }
    fn to_lower(self) -> string { __to_lower(self) }
    fn substring(self, inicio: int, fin: int) -> string { __substring(self, inicio, fin) }
    fn repeat(self, veces: int) -> string { __repeat(self, veces) }
    fn to_bytes(self) -> bytes { __to_bytes(self) }
}

// Operaciones de bytes (M48.4d): un solo impl (`bytes`).
/// Bytes operations: slice by octet index.
trait BytesOps {
    /// The byte slice `[inicio, fin)` by octet index (clamped).
    fn sub_bytes(self, inicio: int, fin: int) -> bytes;
}
impl BytesOps for bytes {
    fn sub_bytes(self, inicio: int, fin: int) -> bytes { __sub_bytes(self, inicio, fin) }
}

// Hash (M40.3a): un valor hashable produce un `int`. `@derive(Hash)` lo genera para un struct/enum
// (combinando `.hash()` de sus campos); los primitivos lo implementan aquí, en raylang (el string
// itera sus caracteres con `char_code`). Lo consumen las tablas hash del prelude (Set, M40.3b).
// `float` NO es hashable (como en `Map`): un struct con campo float no puede derivar Hash.
/// Hashes a value to an `int`. Derivable with `@derive(Hash)`; consumed by the hash-based `Set`.
/// `float` is not hashable.
trait Hash { fn hash(self) -> int; }
impl Hash for int { fn hash(self) -> int { self } }
impl Hash for bool { fn hash(self) -> int { if (self) { 1 } else { 0 } } }
impl Hash for char { fn hash(self) -> int { char_code(self) } }
impl Hash for string {
    fn hash(self) -> int {
        var h = 17;
        let cs = chars(self);
        var i = 0;
        while (i < len(cs)) {
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
trait From<S> { fn desde(origen: S) -> Self; }

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
        Iter { paso: fn() -> Option<U> {
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
        Iter { paso: fn() -> Option<T> {
            var res: Option<T> = Option.None;
            var seguir = true;
            while (seguir) {
                match (self.next()) {
                    Option.Some(x) => { if (pred(x)) { res = Option.Some(x); seguir = false; } },
                    Option.None => { seguir = false; },
                }
            }
            res
        } }
    }
    // Perezoso: entrega a lo sumo los primeros `n` elementos, luego se agota. Corta la cadena sin
    // consumir el resto del origen (útil sobre iteradores infinitos/largos).
    /// Lazily yields at most the first `n` elements, then stops without consuming the rest of the source.
    fn take(self, n: int) -> Iter<T> {
        var restantes = n;
        Iter { paso: fn() -> Option<T> {
            if (restantes <= 0) {
                Option.None
            } else {
                restantes = restantes - 1;
                self.next()
            }
        } }
    }
    // Perezoso: descarta los primeros `n` elementos y entrega el resto. El descarte ocurre en la
    // primera llamada a `next` (el contador capturado se agota una vez).
    /// Lazily discards the first `n` elements and yields the rest.
    fn skip(self, n: int) -> Iter<T> {
        var saltar = n;
        Iter { paso: fn() -> Option<T> {
            while (saltar > 0) {
                saltar = saltar - 1;
                self.next();
            }
            self.next()
        } }
    }
    // Perezoso: empareja este iterador con `otra` posición a posición en tuplas `(T, U)`; se agota
    // cuando cualquiera de los dos lo hace. `otra` ha de ser un `Iter<U>` (los adaptadores devuelven
    // `Iter`; un iterador de usuario se convierte con `.map(...)` o similar). Método genérico sobre `U`.
    /// Lazily pairs this iterator with `otra`, position by position, into `(T, U)` tuples;
    /// stops as soon as either side is exhausted.
    fn zip<U>(self, otra: Iter<U>) -> Iter<(T, U)> {
        Iter { paso: fn() -> Option<(T, U)> {
            match (self.next()) {
                Option.Some(a) => match (otra.next()) {
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
        Iter { paso: fn() -> Option<(int, T)> {
            match (self.next()) {
                Option.Some(x) => {
                    let par = (i, x);
                    i = i + 1;
                    Option.Some(par)
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
        var seguir = true;
        while (seguir) {
            match (self.next()) {
                Option.Some(x) => { acc = f(acc, x); },
                Option.None => { seguir = false; },
            }
        }
        acc
    }
    // TERMINAL: materializa el iterador (perezoso) en un arreglo `[T]`. El puente de vuelta desde la
    // cadena `iter().map().filter()` a un arreglo concreto.
    /// Terminal: consumes the iterator and materializes its elements into a new array `[T]`.
    fn collect(self) -> [T] {
        var out: [T] = [];
        var seguir = true;
        while (seguir) {
            match (self.next()) {
                Option.Some(x) => { push(out, x); },
                Option.None => { seguir = false; },
            }
        }
        out
    }
}

// `.iter()` sobre arreglos y `range` (M40.2b/c): iteradores de PRIMERA CLASE. La representación es
// **un closure** `paso: fn() -> Option<T>` (type-erasure): un iterador ES una función con estado que
// entrega el siguiente elemento o `None`. Así `iter`/`range`/`map`/`filter` producen todos el MISMO
// tipo `Iter<T>` y se encadenan sin bounds sobre traits parametrizados (que raylang no permite). El
// estado (posición, cursor) vive en variables capturadas por el closure (mutadas por referencia).
/// A first-class iterator: a closure `paso` that returns the next element or `None`.
/// `iter`, `range` and all the adapters (`map`, `filter`, ...) produce this same type.
struct Iter<T> { paso: fn() -> Option<T> }
impl<T> Iterator<T> for Iter<T> {
    fn next(self) -> Option<T> { (self.paso)() }
}

// Iterador sobre los elementos del arreglo, en orden. `xs.iter()` == `iter(xs)` (UFCS).
/// Returns an iterator over the elements of the array, in order. UFCS: `xs.iter()`.
fn iter<T>(xs: [T]) -> Iter<T> {
    var i = 0;
    Iter { paso: fn() -> Option<T> {
        if (i < len(xs)) {
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
/// Returns an iterator over the integers from `desde` (inclusive) to `hasta` (exclusive).
fn range(desde: int, hasta: int) -> Iter<int> {
    var i = desde;
    Iter { paso: fn() -> Option<int> {
        if (i < hasta) {
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

// --- Conjuntos: Set<T> (M40.3b) ---
// Tabla hash **bucketed** escrita EN raylang sobre `@derive(Hash)` + `Eq`: `T` debe implementar ambos
// (los bounds se bajan a diccionarios, M9.2). Las operaciones llevan prefijo `set_` para no chocar con
// builtins ya tomados (`contains`/`insert`/`remove`). `set_new()` es un constructor vacío: su `T` lo fija
// el tipo esperado (inferencia bidireccional en la llamada, M40.3b). Nº de buckets fijo (sin resize aún).
/// A hash set of `T`, backed by buckets. `T` must implement `Hash` and `Eq`.
/// Operate on it with the `set_*` functions (`set_add`, `set_has`, `set_remove`, ...).
struct Set<T> { buckets: [[T]], tam: int }

/// Creates an empty set. The element type is fixed by the expected type at the call site.
fn set_new<T>() -> Set<T> {
    var bs: [[T]] = [];
    var i = 0;
    while (i < 16) {
        var e: [T] = [];
        push(bs, e);
        i = i + 1;
    }
    Set { buckets: bs, tam: 0 }
}

// Índice de bucket de `x` (hash módulo nº de buckets, normalizado a 0..n aunque el hash sea negativo).
/// Internal helper: bucket index for `x` (hash modulo `n`, normalized to be non-negative).
fn set_bucket<T: Hash>(x: T, n: int) -> int {
    let h = x.hash();
    ((h % n) + n) % n
}

// ¿Está `x` en el bucket `b`? Búsqueda lineal por igualdad (`Eq`).
/// Internal helper: linear search for `x` in bucket `b` using `Eq`; true if present.
fn set_en_bucket<T: Eq>(b: [T], x: T) -> bool {
    var i = 0;
    while (i < len(b)) {
        if (b[i].igual(x)) { return true; }
        i = i + 1;
    }
    false
}

// Añade `x` al conjunto (si no estaba ya). Muta `s`.
/// Adds `x` to the set if it is not already present. Mutates `s`.
fn set_add<T: Hash + Eq>(s: Set<T>, x: T) {
    let idx = set_bucket(x, len(s.buckets));
    let b = s.buckets[idx];
    if (!set_en_bucket(b, x)) {
        push(b, x);
        s.tam = s.tam + 1;
    }
}

// ¿Pertenece `x` al conjunto?
/// Returns true if `x` is a member of the set.
fn set_has<T: Hash + Eq>(s: Set<T>, x: T) -> bool {
    let idx = set_bucket(x, len(s.buckets));
    set_en_bucket(s.buckets[idx], x)
}

// Quita `x` del conjunto (si estaba). Muta `s` reconstruyendo el bucket sin `x`.
/// Removes `x` from the set if present. Mutates `s`.
fn set_remove<T: Hash + Eq>(s: Set<T>, x: T) {
    let idx = set_bucket(x, len(s.buckets));
    let b = s.buckets[idx];
    var nuevo: [T] = [];
    var i = 0;
    var quitado = false;
    while (i < len(b)) {
        if (b[i].igual(x)) { quitado = true; } else { push(nuevo, b[i]); }
        i = i + 1;
    }
    if (quitado) {
        s.buckets[idx] = nuevo;
        s.tam = s.tam - 1;
    }
}

// --- Constructor de strings: StringBuilder (M40.3c) ---
// Acumula trozos y los une UNA vez al final (`join`), evitando el O(n²) de concatenar con `+` en un
// bucle (cada `+` copia todo lo acumulado). Prefijo `sb_` (para no chocar con `push`). UFCS: `sb.sb_push(s)`.
/// Accumulates string chunks and joins them once at the end, avoiding the O(n²) cost of
/// repeated `+` concatenation in a loop. Operate on it with the `sb_*` functions.
struct StringBuilder { partes: [string] }

/// Creates an empty StringBuilder.
fn sb_new() -> StringBuilder { StringBuilder { partes: [] } }

// Añade un trozo al final (O(1) amortizado; no copia lo ya acumulado).
/// Appends a chunk to the builder (amortized O(1); does not copy what is already accumulated).
fn sb_push(sb: StringBuilder, s: string) { push(sb.partes, s); }

// Une todo lo acumulado en un solo string (O(total), una vez).
/// Joins all accumulated chunks into a single string (O(total), done once).
fn sb_build(sb: StringBuilder) -> string { join(sb.partes, "") }

// Número de trozos acumulados (no de caracteres).
/// Returns the number of accumulated chunks (not characters).
fn sb_count(sb: StringBuilder) -> int { len(sb.partes) }

// --- Cola doble: Deque<T> (M40.3d) ---
// Respaldada por un arreglo + un índice `head` (los elementos vivos son `datos[head..]`). Así
// `push_back`/`pop_front` (uso de cola) son O(1); `pop_back` también; `push_front` es O(1) si hay
// hueco al frente (head>0), O(n) si toca reconstruir. `pop_*`/`peek_*` devuelven `Option<T>` (None si
// vacía). `deque_new()` es un constructor vacío (T lo fija el contexto, M40.3b). Prefijo `deque_`.
/// A double-ended queue backed by an array plus a `head` index; `push_back`, `pop_front` and
/// `pop_back` are O(1). Operate on it with the `deque_*` functions.
struct Deque<T> { datos: [T], head: int }

/// Creates an empty deque. The element type is fixed by the expected type at the call site.
fn deque_new<T>() -> Deque<T> {
    var d: [T] = [];
    Deque { datos: d, head: 0 }
}

// Número de elementos vivos.
/// Returns the number of live elements in the deque.
fn deque_len<T>(d: Deque<T>) -> int { len(d.datos) - d.head }
/// Returns true if the deque has no elements.
fn deque_is_empty<T>(d: Deque<T>) -> bool { deque_len(d) == 0 }

// Encola por detrás (O(1) amortizado).
/// Appends `x` at the back of the deque (amortized O(1)).
fn deque_push_back<T>(d: Deque<T>, x: T) { push(d.datos, x); }

// Desencola por delante; None si vacía (O(1): solo avanza `head`).
/// Removes and returns the front element, or `None` if the deque is empty (O(1)).
fn deque_pop_front<T>(d: Deque<T>) -> Option<T> {
    if (d.head < len(d.datos)) {
        let v = d.datos[d.head];
        d.head = d.head + 1;
        Option.Some(v)
    } else {
        Option.None
    }
}

// Desencola por detrás; None si vacía.
/// Removes and returns the back element, or `None` if the deque is empty.
fn deque_pop_back<T>(d: Deque<T>) -> Option<T> {
    if (len(d.datos) > d.head) { pop(d.datos) } else { Option.None }
}

// Encola por delante (O(1) si hay hueco; si no, reconstruye O(n)).
/// Inserts `x` at the front (O(1) if there is room at the front, otherwise an O(n) rebuild).
fn deque_push_front<T>(d: Deque<T>, x: T) {
    if (d.head > 0) {
        d.head = d.head - 1;
        d.datos[d.head] = x;
    } else {
        var nuevo: [T] = [];
        push(nuevo, x);
        var i = d.head;
        while (i < len(d.datos)) { push(nuevo, d.datos[i]); i = i + 1; }
        d.datos = nuevo;
        d.head = 0;
    }
}

// Mira el frente sin desencolar; None si vacía.
/// Returns the front element without removing it, or `None` if the deque is empty.
fn deque_peek_front<T>(d: Deque<T>) -> Option<T> {
    if (d.head < len(d.datos)) { Option.Some(d.datos[d.head]) } else { Option.None }
}

// Número de elementos del conjunto.
/// Returns the number of elements in the set.
fn set_size<T>(s: Set<T>) -> int { s.tam }

// Los elementos del conjunto en un arreglo (orden no especificado — por bucket).
/// Returns the elements of the set as an array (order unspecified — by bucket).
fn set_items<T>(s: Set<T>) -> [T] {
    var out: [T] = [];
    var i = 0;
    while (i < len(s.buckets)) {
        let b = s.buckets[i];
        var j = 0;
        while (j < len(b)) { push(out, b[j]); j = j + 1; }
        i = i + 1;
    }
    out
}

// Traits de sobrecarga de operadores (M28.1): un tipo que implemente estos traits puede usar los
// operadores aritméticos. El checker baja `a + b` (con `a`/`b` de un tipo de usuario) a `a.add(b)`.
/// Operator overloading: `a + b` on a user type dispatches to `a.add(b)`.
trait Add { fn add(self, otro: Self) -> Self; }
/// Operator overloading: `a - b` on a user type dispatches to `a.sub(b)`.
trait Sub { fn sub(self, otro: Self) -> Self; }
/// Operator overloading: `a * b` on a user type dispatches to `a.mul(b)`.
trait Mul { fn mul(self, otro: Self) -> Self; }
/// Operator overloading: `a / b` on a user type dispatches to `a.div(b)`.
trait Div { fn div(self, otro: Self) -> Self; }
/// Operator overloading: unary `-a` on a user type dispatches to `a.neg()`.
trait Neg { fn neg(self) -> Self; }

// Eq/Show para los primitivos (M13.2a): los habilita `assert_eq` (que pide `T: Eq + Show`) y, en
// general, cualquier genérico acotado por Eq/Show sobre un primitivo. Vía `==` y `to_string`, que
// ya operan sobre int/float/bool/string/char. (Un tipo del usuario los obtiene con `@derive`.)
impl Eq for int { fn igual(self, otro: int) -> bool { self == otro } }
impl Eq for float { fn igual(self, otro: float) -> bool { self == otro } }
impl Eq for string { fn igual(self, otro: string) -> bool { self == otro } }
impl Eq for bool { fn igual(self, otro: bool) -> bool { self == otro } }
impl Eq for char { fn igual(self, otro: char) -> bool { self == otro } }
impl Show for int { fn mostrar(self) -> string { to_string(self) } }
impl Show for float { fn mostrar(self) -> string { to_string(self) } }
impl Show for string { fn mostrar(self) -> string { to_string(self) } }
impl Show for bool { fn mostrar(self) -> string { to_string(self) } }
impl Show for char { fn mostrar(self) -> string { to_string(self) } }

// Ordena ascendente, devolviendo un arreglo NUEVO (insertion sort). `T` debe implementar `Ord`;
// el bound se baja a paso de diccionarios (M9.2), así que `sort` es front-end puro (cero opcodes).
/// Sorts ascending and returns a new array (insertion sort). `T` must implement `Ord`.
fn sort<T: Ord>(a: [T]) -> [T] {
    var out: [T] = [];
    var i: int = 0;
    while (i < len(a)) {
        let x: T = a[i];
        push(out, x);
        var j: int = len(out) - 1;
        while (j > 0 && x.menor(out[j - 1])) {
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
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M13.1b: quita la clave `k` del mapa y devuelve su valor (None si no estaba).
/// Removes key `k` from the map and returns its value, or `None` if it was not present.
fn remove<K, V>(m: Map<K, V>, k: K) -> Option<V> {
    let r = __map_remove(m, k);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// --- Aserciones (M13.2a) ---
// Sobre el primitivo `panic` (el único toque de runtime). No hay sobrecarga, así que en vez de
// `assert(cond)` y `assert(cond, msg)` se ofrece `assert(cond)` (mensaje genérico), `assert_eq`
// (mensaje detallado con los valores) y, para un mensaje a medida, `panic("...")` directo.

// Falla con un mensaje genérico si la condición no se cumple.
/// Panics with a generic message if the condition is false.
/// For a custom message, call `panic("...")` directly.
fn assert(cond: bool) {
    if (!cond) { panic("aserción falló"); }
}

// Falla mostrando ambos valores si no son iguales. `T` debe ser Eq (comparar) y Show (mostrar);
// los bounds se bajan a diccionarios (M9.2), así que esto es front-end puro sobre `panic`.
/// Panics showing both values if they are not equal.
/// `T` must implement `Eq` (to compare) and `Show` (to display the values).
fn assert_eq<T: Eq + Show>(a: T, b: T) {
    if (!a.igual(b)) {
        panic("assert_eq falló: " + a.mostrar() + " != " + b.mostrar());
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
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Parsea un flotante; None si el texto no es un flotante válido (M14).
/// Parses a string as a float; `None` if the text is not a valid float.
fn parse_float(s: string) -> Option<float> {
    let r = __parse_float(s);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Ed25519 (M43.3, cripto de producción vía ring). La semilla privada es de 32 octetos; None si no lo es.
// Clave pública (32 octetos) derivada de la semilla.
/// Derives the Ed25519 public key (32 bytes) from a 32-byte private seed;
/// `None` if the seed is not exactly 32 bytes.
fn ed25519_public_key(seed: bytes) -> Option<bytes> {
    let r = __ed25519_public_key(seed);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Firma (64 octetos) de msg con la semilla. Determinista (RFC 8032). None si la semilla no mide 32.
/// Signs `msg` with the 32-byte Ed25519 seed, returning a 64-byte deterministic
/// signature (RFC 8032); `None` if the seed is not exactly 32 bytes.
fn ed25519_sign(seed: bytes, msg: bytes) -> Option<bytes> {
    let r = __ed25519_sign(seed, msg);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// ChaCha20-Poly1305 AEAD (M43.4). Clave de 32 octetos, nonce de 12. seal → texto_cifrado||etiqueta;
// None si los tamaños no cuadran.
/// ChaCha20-Poly1305 AEAD encryption: returns ciphertext followed by the authentication tag.
/// The key must be 32 bytes and the nonce 12; `None` if the sizes are wrong.
fn chacha20poly1305_seal(key: bytes, nonce: bytes, aad: bytes, plaintext: bytes) -> Option<bytes> {
    let r = __chacha20poly1305_seal(key, nonce, aad, plaintext);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// open verifica y descifra; None si la autenticación falla (dato manipulado) o los tamaños no cuadran.
/// ChaCha20-Poly1305 AEAD decryption: verifies and decrypts;
/// `None` if authentication fails (tampered data) or the sizes are wrong.
fn chacha20poly1305_open(key: bytes, nonce: bytes, aad: bytes, ciphertext: bytes) -> Option<bytes> {
    let r = __chacha20poly1305_open(key, nonce, aad, ciphertext);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Lee una línea de stdin (sin el salto de línea); None en fin de entrada (EOF).
/// Reads one line from stdin (without the trailing newline); `None` on end of input (EOF).
fn input() -> Option<string> {
    let r = __read_line();
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
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
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Valor de una variable de entorno; None si no está definida.
/// Returns the value of an environment variable, or `None` if it is not set.
fn env(nombre: string) -> Option<string> {
    let r = __env(nombre);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7a: índice (de carácter) de la primera ocurrencia de `sub` en `s`; None si no aparece.
/// Returns the character index of the first occurrence of `sub` in `s`, or `None` if absent.
fn index_of(s: string, sub: string) -> Option<int> {
    let r = __index_of(s, sub);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7b: quita y devuelve el último elemento del arreglo (lo muta); None si está vacío.
/// Removes and returns the last element of the array (mutating it), or `None` if it is empty.
fn pop<T>(a: [T]) -> Option<T> {
    let r = __pop(a);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7b: índice de la primera ocurrencia de `x` en el arreglo; None si no aparece.
/// Returns the index of the first occurrence of `x` in the array, or `None` if absent.
fn position<T>(a: [T], x: T) -> Option<int> {
    let r = __position(a, x);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// --- Archivos (M11.2c): el primitivo devuelve un arreglo ETIQUETADO (primer elemento "ok"/"err");
// aquí se traduce a Result. Así el runtime tampoco sabe de Result (como con Option). ---

// Lee el archivo completo; Ok(contenido) u Err(mensaje del sistema).
/// Reads the whole file as a string; `Ok(contents)` or `Err(system error message)`.
fn read_file(ruta: string) -> Result<string, string> {
    let r = __read_file(ruta);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}

// M16.1b: decodifica bytes como UTF-8; Ok(string) u Err(mensaje) si no es válido.
/// Decodes bytes as UTF-8; `Ok(string)`, or `Err(message)` if the bytes are not valid UTF-8.
fn from_utf8(b: bytes) -> Result<string, string> {
    let r = __from_utf8(b);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}

// M16.1c: I/O binaria. Las lecturas devuelven [bytes] etiquetado (tag en bytes para arreglo
// homogéneo); el mensaje de error viene como bytes y se decoda con from_utf8.
/// Reads the whole file as raw bytes; `Ok(bytes)` or `Err(message)` on I/O failure.
fn read_file_bytes(ruta: string) -> Result<bytes, string> {
    let r = __read_file_bytes(ruta);
    if (r[0] == b"ok") {
        Result.Ok(r[1])
    } else {
        match (from_utf8(r[1])) {
            Result.Ok(m) => Result.Err(m),
            Result.Err(e) => Result.Err("error de E/S"),
        }
    }
}

/// Writes raw bytes to the file (creating or overwriting it); `Ok(byte count)` or `Err(message)`.
fn write_file_bytes(ruta: string, datos: bytes) -> Result<int, string> {
    let r = __write_file_bytes(ruta, datos);
    if (r[0] == "ok") { Result.Ok(len(datos)) } else { Result.Err(r[1]) }
}

/// Performs one raw read from the socket; `Ok(bytes)` (empty = EOF) or `Err(message)`.
fn socket_read_bytes(h: int) -> Result<bytes, string> {
    let r = __socket_read_bytes(h);
    if (r[0] == b"ok") {
        Result.Ok(r[1])
    } else {
        match (from_utf8(r[1])) {
            Result.Ok(m) => Result.Err(m),
            Result.Err(e) => Result.Err("error de socket"),
        }
    }
}

/// Writes raw bytes to the socket; `Ok(byte count)` or `Err(message)`.
fn socket_write_bytes(h: int, datos: bytes) -> Result<int, string> {
    let r = __socket_write_bytes(h, datos);
    if (r[0] == "ok") { Result.Ok(len(datos)) } else { Result.Err(r[1]) }
}

// Escribe el contenido en el archivo (lo crea/sobrescribe); Ok(nº de caracteres) u Err(mensaje).
/// Writes the string to the file (creating or overwriting it); `Ok(character count)` or `Err(message)`.
fn write_file(ruta: string, contenido: string) -> Result<int, string> {
    let r = __write_file(ruta, contenido);
    if (r[0] == "ok") { Result.Ok(len(contenido)) } else { Result.Err(r[1]) }
}

// Añade el contenido al final del archivo (lo crea si no existe); Ok(nº de caracteres) u Err(mensaje).
/// Appends the string to the end of the file (creating it if needed); `Ok(character count)` or `Err(message)`.
fn append_file(ruta: string, contenido: string) -> Result<int, string> {
    let r = __append_file(ruta, contenido);
    if (r[0] == "ok") { Result.Ok(len(contenido)) } else { Result.Err(r[1]) }
}

// M11.7c: borra un archivo; Ok(0) u Err(mensaje del sistema).
/// Deletes a file; `Ok(0)` or `Err(system error message)`.
fn remove_file(ruta: string) -> Result<int, string> {
    let r = __remove_file(ruta);
    if (r[0] == "ok") { Result.Ok(0) } else { Result.Err(r[1]) }
}

// M11.7c: nombres de las entradas de un directorio (ordenados); Ok([nombres]) u Err(mensaje).
// El primitivo devuelve ["ok", n0, n1, …] o ["err", msg]; aquí se reconstruye el [string].
/// Returns the names of a directory's entries, sorted; `Ok(names)` or `Err(message)`.
fn list_dir(ruta: string) -> Result<[string], string> {
    let r = __list_dir(ruta);
    if (r[0] == "ok") {
        var nombres: [string] = [];
        var i = 1;
        while (i < len(r)) { push(nombres, r[i]); i = i + 1; }
        Result.Ok(nombres)
    } else {
        Result.Err(r[1])
    }
}

// --- I/O con buffering: handles de archivo (M11.8). open/read_line/write/close. ---

// Abre un archivo (modo "r"/"w"/"a") y devuelve un handle (int); Err(mensaje) si falla.
/// Opens a file in mode "r" (read), "w" (write) or "a" (append) and returns a
/// buffered handle; `Ok(handle)` or `Err(message)`. Close it with `close(h)`.
fn open(ruta: string, modo: string) -> Result<int, string> {
    let r = __open(ruta, modo);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// Lee la siguiente línea del handle (sin el salto); None en EOF (o handle no-lector).
/// Reads the next line from the file handle (without the newline); `None` on EOF
/// (or on a handle that is not open for reading).
fn read_line(h: int) -> Option<string> {
    let r = __read_line_handle(h);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Escribe en el handle; Ok(nº de caracteres) u Err(mensaje).
/// Writes the string to the file handle; `Ok(character count)` or `Err(message)`.
fn write(h: int, s: string) -> Result<int, string> {
    let r = __write_handle(h, s);
    if (r[0] == "ok") { Result.Ok(len(s)) } else { Result.Err(r[1]) }
}

// --- Cliente TCP (M15.2). Sobre los primitivos __tcp_connect/__socket_read/__socket_write. ---

// Conecta a host:port (resuelve el nombre); Ok(handle) u Err(mensaje).
/// Opens a TCP connection to host:port (resolving the host name); `Ok(socket handle)` or `Err(message)`.
fn tcp_connect(host: string, port: int) -> Result<int, string> {
    let r = __tcp_connect(host, port);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// M19.4a: conecta por TLS a host:port (verifica el certificado del servidor); Ok(handle) u Err. El
// handle se lee/escribe con socket_read_bytes/socket_write_bytes (que desvían a TLS) y se cierra con close.
/// Opens a TLS connection to host:port, verifying the server certificate; `Ok(handle)` or
/// `Err(message)`. Read/write with `socket_read_bytes`/`socket_write_bytes`; close with `close`.
fn tls_connect(host: string, port: int) -> Result<int, string> {
    let r = __tls_connect(host, port);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// M31.2a: conexión TLS con ALPN 'h2' (HTTP/2); error si el servidor no lo negocia.
/// Opens a TLS connection negotiating ALPN "h2" (HTTP/2); `Ok(handle)`, or `Err(message)`
/// if the connection fails or the server does not negotiate h2.
fn tls_connect_h2(host: string, port: int) -> Result<int, string> {
    let r = __tls_connect_h2(host, port);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// M19.4b: envuelve un socket TCP ya aceptado (handle de tcp_accept) en una sesión TLS de servidor con
// el certificado y la clave en PEM; Ok(handle) u Err. Reusa el handle. Solo VM (servidor concurrente).
/// Wraps an already-accepted TCP socket in a server-side TLS session, using a PEM certificate
/// and private key; `Ok(handle)` or `Err(message)`. VM only.
fn tls_accept(handle: int, cert: string, key: string) -> Result<int, string> {
    let r = __tls_accept(handle, cert, key);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// Hace una lectura del socket; Ok(datos) ("" = EOF) u Err(mensaje).
/// Performs one read from the socket; `Ok(data)` (empty string = EOF) or `Err(message)`.
fn socket_read(h: int) -> Result<string, string> {
    let r = __socket_read(h);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}

// Escribe en el socket; Ok(nº de bytes) u Err(mensaje).
/// Writes the string to the socket; `Ok(byte count)` or `Err(message)`.
fn socket_write(h: int, s: string) -> Result<int, string> {
    let r = __socket_write(h, s);
    if (r[0] == "ok") { Result.Ok(len(s)) } else { Result.Err(r[1]) }
}

// --- Servidor TCP (M15.3). Sobre __tcp_listen/__tcp_accept. ---

// Escucha en host:port (port=0 → puerto efímero); Ok(handle de escucha) u Err(mensaje).
/// Listens for TCP connections on host:port (port 0 picks an ephemeral port);
/// `Ok(listener handle)` or `Err(message)`.
fn tcp_listen(host: string, port: int) -> Result<int, string> {
    let r = __tcp_listen(host, port);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}

// Bloquea hasta una conexión; Ok(handle de conexión) u Err(mensaje).
/// Blocks until a client connects to the listener; `Ok(connection handle)` or `Err(message)`.
fn tcp_accept(listener: int) -> Result<int, string> {
    let r = __tcp_accept(listener);
    if (r[0] == "ok") {
        match (parse_int(r[1])) {
            Option.Some(h) => Result.Ok(h),
            Option.None => Result.Err("handle inválido"),
        }
    } else {
        Result.Err(r[1])
    }
}
"#;

/// Parsea el prelude una vez. El `expect` no puede fallar: el fuente es una constante
/// conocida y válida.
fn parse() -> crate::ast::Program {
    let tokens = crate::lexer::lex(SOURCE).unwrap_or_else(|e| crate::ice!("el prelude no lexea: {e}"));
    crate::parser::parse(tokens).unwrap_or_else(|e| crate::ice!("el prelude no parsea: {e}"))
}

/// Los enums del prelude (`Option`/`Result`), ya parseados.
pub fn enums() -> Vec<EnumDef> {
    parse().enums
}

/// Los structs del prelude (`ArrayIter`/`RangeIter` para `.iter()`/`range`, M40.2b), ya parseados.
pub fn structs() -> Vec<StructDef> {
    parse().structs
}

/// Las funciones del prelude (`map`/`filter`/`fold`), ya parseadas.
pub fn functions() -> Vec<Function> {
    parse().functions
}

/// Los traits del prelude (`Eq`/`Show`/`Ord`), ya parseados (M10.1).
pub fn traits() -> Vec<TraitDef> {
    parse().traits
}

/// Los `impl` del prelude (M11.7d: `Ord` para int/float/string/char), ya parseados.
pub fn impls() -> Vec<crate::ast::ImplBlock> {
    parse().impls
}
