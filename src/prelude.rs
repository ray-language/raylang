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
enum Option<T> { Some(T), None }
enum Result<T, E> { Ok(T), Err(E) }

// Igualdad estructural (M10.1). `@derive(Eq)` genera el `impl` para un struct/enum.
// Usa `Self` en posición de argumento, así que no es invocable sobre un `dyn Eq`
// (object safety): se compara entre valores concretos, `a.igual(b)`.
trait Eq {
    fn igual(self, otro: Self) -> bool;
}

// Representación textual (limpieza post-M11, L2). `@derive(Show)` genera el `impl` para un
// struct/enum. No usa `Self` fuera del receptor, así que sí es object-safe (`dyn Show`).
trait Show {
    fn mostrar(self) -> string;
}

// Orden total (M11.7d): `self < otro`. Lo usa `sort`. Los primitivos lo implementan vía el
// operador `<` (extendido a string/char en M11.7d); un tipo del usuario lo implementa a mano.
trait Ord {
    fn menor(self, otro: Self) -> bool;
}
impl Ord for int { fn menor(self, otro: int) -> bool { self < otro } }
impl Ord for float { fn menor(self, otro: float) -> bool { self < otro } }
impl Ord for string { fn menor(self, otro: string) -> bool { self < otro } }
impl Ord for char { fn menor(self, otro: char) -> bool { self < otro } }

// Hash (M40.3a): un valor hashable produce un `int`. `@derive(Hash)` lo genera para un struct/enum
// (combinando `.hash()` de sus campos); los primitivos lo implementan aquí, en raylang (el string
// itera sus caracteres con `char_code`). Lo consumen las tablas hash del prelude (Set, M40.3b).
// `float` NO es hashable (como en `Map`): un struct con campo float no puede derivar Hash.
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
trait Iterator<T> {
    fn next(self) -> Option<T>;
    // Transforma cada elemento con `f`, perezosamente. `xs.iter().map(f)` no calcula nada hasta el
    // `for`/`next`. Reusa el closure `paso` de `Iter`, que captura `self` (el iterador de origen,
    // por referencia → su estado avanza) y `f`.
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
struct Iter<T> { paso: fn() -> Option<T> }
impl<T> Iterator<T> for Iter<T> {
    fn next(self) -> Option<T> { (self.paso)() }
}

// Iterador sobre los elementos del arreglo, en orden. `xs.iter()` == `iter(xs)` (UFCS).
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
fn sum(it: Iter<int>) -> int {
    it.fold(0, fn(a: int, x: int) -> int { a + x })
}

// --- Conjuntos: Set<T> (M40.3b) ---
// Tabla hash **bucketed** escrita EN raylang sobre `@derive(Hash)` + `Eq`: `T` debe implementar ambos
// (los bounds se bajan a diccionarios, M9.2). Las operaciones llevan prefijo `set_` para no chocar con
// builtins ya tomados (`contains`/`insert`/`remove`). `set_new()` es un constructor vacío: su `T` lo fija
// el tipo esperado (inferencia bidireccional en la llamada, M40.3b). Nº de buckets fijo (sin resize aún).
struct Set<T> { buckets: [[T]], tam: int }

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
fn set_bucket<T: Hash>(x: T, n: int) -> int {
    let h = x.hash();
    ((h % n) + n) % n
}

// ¿Está `x` en el bucket `b`? Búsqueda lineal por igualdad (`Eq`).
fn set_en_bucket<T: Eq>(b: [T], x: T) -> bool {
    var i = 0;
    while (i < len(b)) {
        if (b[i].igual(x)) { return true; }
        i = i + 1;
    }
    false
}

// Añade `x` al conjunto (si no estaba ya). Muta `s`.
fn set_add<T: Hash + Eq>(s: Set<T>, x: T) {
    let idx = set_bucket(x, len(s.buckets));
    let b = s.buckets[idx];
    if (!set_en_bucket(b, x)) {
        push(b, x);
        s.tam = s.tam + 1;
    }
}

// ¿Pertenece `x` al conjunto?
fn set_has<T: Hash + Eq>(s: Set<T>, x: T) -> bool {
    let idx = set_bucket(x, len(s.buckets));
    set_en_bucket(s.buckets[idx], x)
}

// Quita `x` del conjunto (si estaba). Muta `s` reconstruyendo el bucket sin `x`.
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
struct StringBuilder { partes: [string] }

fn sb_new() -> StringBuilder { StringBuilder { partes: [] } }

// Añade un trozo al final (O(1) amortizado; no copia lo ya acumulado).
fn sb_push(sb: StringBuilder, s: string) { push(sb.partes, s); }

// Une todo lo acumulado en un solo string (O(total), una vez).
fn sb_build(sb: StringBuilder) -> string { join(sb.partes, "") }

// Número de trozos acumulados (no de caracteres).
fn sb_count(sb: StringBuilder) -> int { len(sb.partes) }

// --- Cola doble: Deque<T> (M40.3d) ---
// Respaldada por un arreglo + un índice `head` (los elementos vivos son `datos[head..]`). Así
// `push_back`/`pop_front` (uso de cola) son O(1); `pop_back` también; `push_front` es O(1) si hay
// hueco al frente (head>0), O(n) si toca reconstruir. `pop_*`/`peek_*` devuelven `Option<T>` (None si
// vacía). `deque_new()` es un constructor vacío (T lo fija el contexto, M40.3b). Prefijo `deque_`.
struct Deque<T> { datos: [T], head: int }

fn deque_new<T>() -> Deque<T> {
    var d: [T] = [];
    Deque { datos: d, head: 0 }
}

// Número de elementos vivos.
fn deque_len<T>(d: Deque<T>) -> int { len(d.datos) - d.head }
fn deque_is_empty<T>(d: Deque<T>) -> bool { deque_len(d) == 0 }

// Encola por detrás (O(1) amortizado).
fn deque_push_back<T>(d: Deque<T>, x: T) { push(d.datos, x); }

// Desencola por delante; None si vacía (O(1): solo avanza `head`).
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
fn deque_pop_back<T>(d: Deque<T>) -> Option<T> {
    if (len(d.datos) > d.head) { pop(d.datos) } else { Option.None }
}

// Encola por delante (O(1) si hay hueco; si no, reconstruye O(n)).
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
fn deque_peek_front<T>(d: Deque<T>) -> Option<T> {
    if (d.head < len(d.datos)) { Option.Some(d.datos[d.head]) } else { Option.None }
}

// Número de elementos del conjunto.
fn set_size<T>(s: Set<T>) -> int { s.tam }

// Los elementos del conjunto en un arreglo (orden no especificado — por bucket).
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
trait Add { fn add(self, otro: Self) -> Self; }
trait Sub { fn sub(self, otro: Self) -> Self; }
trait Mul { fn mul(self, otro: Self) -> Self; }
trait Div { fn div(self, otro: Self) -> Self; }
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
fn get<K, V>(m: Map<K, V>, k: K) -> Option<V> {
    let r = __map_get(m, k);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M13.1b: quita la clave `k` del mapa y devuelve su valor (None si no estaba).
fn remove<K, V>(m: Map<K, V>, k: K) -> Option<V> {
    let r = __map_remove(m, k);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// --- Aserciones (M13.2a) ---
// Sobre el primitivo `panic` (el único toque de runtime). No hay sobrecarga, así que en vez de
// `assert(cond)` y `assert(cond, msg)` se ofrece `assert(cond)` (mensaje genérico), `assert_eq`
// (mensaje detallado con los valores) y, para un mensaje a medida, `panic("...")` directo.

// Falla con un mensaje genérico si la condición no se cumple.
fn assert(cond: bool) {
    if (!cond) { panic("aserción falló"); }
}

// Falla mostrando ambos valores si no son iguales. `T` debe ser Eq (comparar) y Show (mostrar);
// los bounds se bajan a diccionarios (M9.2), así que esto es front-end puro sobre `panic`.
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
fn map<T, U>(xs: [T], f: fn(T) -> U) -> [U] {
    iter(xs).map(f).collect()
}

// Conserva los elementos para los que `pred` es verdadero, en un arreglo nuevo.
fn filter<T>(xs: [T], pred: fn(T) -> bool) -> [T] {
    iter(xs).filter(pred).collect()
}

// Reduce el arreglo a un único valor, acumulando de izquierda a derecha desde `init`.
fn fold<T, A>(xs: [T], init: A, f: fn(A, T) -> A) -> A {
    iter(xs).fold(init, f)
}

// --- I/O (M11.2): envoltorios sobre primitivos builtin que devuelven [T] (vacío/único) ---
// El runtime no sabe de Option: los primitivos devuelven un arreglo de 0 o 1 elementos y aquí,
// en raylang, se traducen a Option con Some/None corrientes (el patrón de la stdlib, M7.3).

// Parsea un entero; None si el texto no es un entero válido.
fn parse_int(s: string) -> Option<int> {
    let r = __parse_int(s);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Parsea un flotante; None si el texto no es un flotante válido (M14).
fn parse_float(s: string) -> Option<float> {
    let r = __parse_float(s);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Ed25519 (M43.3, cripto de producción vía ring). La semilla privada es de 32 octetos; None si no lo es.
// Clave pública (32 octetos) derivada de la semilla.
fn ed25519_public_key(seed: bytes) -> Option<bytes> {
    let r = __ed25519_public_key(seed);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Firma (64 octetos) de msg con la semilla. Determinista (RFC 8032). None si la semilla no mide 32.
fn ed25519_sign(seed: bytes, msg: bytes) -> Option<bytes> {
    let r = __ed25519_sign(seed, msg);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Lee una línea de stdin (sin el salto de línea); None en fin de entrada (EOF).
fn input() -> Option<string> {
    let r = __read_line();
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Lee una línea y la parsea como entero; None en EOF o si no es un entero.
fn read_int() -> Option<int> {
    let s = input()?;
    parse_int(s)
}

// M12.1 (concurrencia): recibe del canal. Some(v) si llega un valor; None si el canal está cerrado y
// vacío. Envuelve el primitivo __recv (que devuelve [T]) en un Option, como input/parse_int. Solo la VM.
fn recv<T>(ch: Channel<T>) -> Option<T> {
    let r = __recv(ch);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Valor de una variable de entorno; None si no está definida.
fn env(nombre: string) -> Option<string> {
    let r = __env(nombre);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7a: índice (de carácter) de la primera ocurrencia de `sub` en `s`; None si no aparece.
fn index_of(s: string, sub: string) -> Option<int> {
    let r = __index_of(s, sub);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7b: quita y devuelve el último elemento del arreglo (lo muta); None si está vacío.
fn pop<T>(a: [T]) -> Option<T> {
    let r = __pop(a);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// M11.7b: índice de la primera ocurrencia de `x` en el arreglo; None si no aparece.
fn position<T>(a: [T], x: T) -> Option<int> {
    let r = __position(a, x);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// --- Archivos (M11.2c): el primitivo devuelve un arreglo ETIQUETADO (primer elemento "ok"/"err");
// aquí se traduce a Result. Así el runtime tampoco sabe de Result (como con Option). ---

// Lee el archivo completo; Ok(contenido) u Err(mensaje del sistema).
fn read_file(ruta: string) -> Result<string, string> {
    let r = __read_file(ruta);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}

// M16.1b: decodifica bytes como UTF-8; Ok(string) u Err(mensaje) si no es válido.
fn from_utf8(b: bytes) -> Result<string, string> {
    let r = __from_utf8(b);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}

// M16.1c: I/O binaria. Las lecturas devuelven [bytes] etiquetado (tag en bytes para arreglo
// homogéneo); el mensaje de error viene como bytes y se decoda con from_utf8.
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

fn write_file_bytes(ruta: string, datos: bytes) -> Result<int, string> {
    let r = __write_file_bytes(ruta, datos);
    if (r[0] == "ok") { Result.Ok(len(datos)) } else { Result.Err(r[1]) }
}

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

fn socket_write_bytes(h: int, datos: bytes) -> Result<int, string> {
    let r = __socket_write_bytes(h, datos);
    if (r[0] == "ok") { Result.Ok(len(datos)) } else { Result.Err(r[1]) }
}

// Escribe el contenido en el archivo (lo crea/sobrescribe); Ok(nº de caracteres) u Err(mensaje).
fn write_file(ruta: string, contenido: string) -> Result<int, string> {
    let r = __write_file(ruta, contenido);
    if (r[0] == "ok") { Result.Ok(len(contenido)) } else { Result.Err(r[1]) }
}

// Añade el contenido al final del archivo (lo crea si no existe); Ok(nº de caracteres) u Err(mensaje).
fn append_file(ruta: string, contenido: string) -> Result<int, string> {
    let r = __append_file(ruta, contenido);
    if (r[0] == "ok") { Result.Ok(len(contenido)) } else { Result.Err(r[1]) }
}

// M11.7c: borra un archivo; Ok(0) u Err(mensaje del sistema).
fn remove_file(ruta: string) -> Result<int, string> {
    let r = __remove_file(ruta);
    if (r[0] == "ok") { Result.Ok(0) } else { Result.Err(r[1]) }
}

// M11.7c: nombres de las entradas de un directorio (ordenados); Ok([nombres]) u Err(mensaje).
// El primitivo devuelve ["ok", n0, n1, …] o ["err", msg]; aquí se reconstruye el [string].
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
fn read_line(h: int) -> Option<string> {
    let r = __read_line_handle(h);
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}

// Escribe en el handle; Ok(nº de caracteres) u Err(mensaje).
fn write(h: int, s: string) -> Result<int, string> {
    let r = __write_handle(h, s);
    if (r[0] == "ok") { Result.Ok(len(s)) } else { Result.Err(r[1]) }
}

// --- Cliente TCP (M15.2). Sobre los primitivos __tcp_connect/__socket_read/__socket_write. ---

// Conecta a host:port (resuelve el nombre); Ok(handle) u Err(mensaje).
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
fn socket_read(h: int) -> Result<string, string> {
    let r = __socket_read(h);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}

// Escribe en el socket; Ok(nº de bytes) u Err(mensaje).
fn socket_write(h: int, s: string) -> Result<int, string> {
    let r = __socket_write(h, s);
    if (r[0] == "ok") { Result.Ok(len(s)) } else { Result.Err(r[1]) }
}

// --- Servidor TCP (M15.3). Sobre __tcp_listen/__tcp_accept. ---

// Escucha en host:port (port=0 → puerto efímero); Ok(handle de escucha) u Err(mensaje).
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
