//! M44a-3: el **punto de entrada WebAssembly** del playground (lenguaje núcleo).
//!
//! ABI **cruda, sin `wasm-bindgen`** (cero deps de Cargo, como `dlopen`/`poll.rs`/el JSON a mano): el
//! host JS reserva un buffer con [`alloc`], escribe ahí los bytes UTF-8 del fuente, y llama a [`run`],
//! que devuelve un `u64` empaquetado `(ptr_salida << 32) | len_salida` apuntando al texto de salida en la
//! memoria lineal del módulo. JS lee esos bytes y luego los libera con [`dealloc`].
//!
//! La salida es el **stdout capturado** (`print`/`eprint` van a un buffer, no hay stdout en el navegador;
//! ver [`crate::host_print`]) más, si hubo un error de léxico/sintaxis/tipos/ejecución, el diagnóstico
//! renderizado con contexto de fuente (`diagnostic::render`). El multicore está desactivado (N=1) y la
//! red/TLS/cripto/FFI **no están disponibles** (un programa que las use da un error claro).
//!
//! Alcance: **un solo archivo** (sin `import`/módulos, que requieren el loader host-side). Cubre todo el
//! lenguaje núcleo: aritmética, strings, arreglos, `Map`, structs, enums, `match`, closures, genéricos,
//! traits, y el prelude (`Option`/`Result`/`map`/`filter`/`fold`/`assert`/`sort`).

use std::cell::RefCell;

thread_local! {
    /// Buffer de salida del programa en curso (`print`/`eprint`). Se limpia al empezar cada `run` y se
    /// vuelca al final. En el navegador no hay stdout, así que ES la salida.
    static SALIDA: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Acumula una línea de `print` (la llama [`crate::host_print`] en wasm).
pub fn push_stdout(s: &str) {
    SALIDA.with(|o| {
        let mut b = o.borrow_mut();
        b.push_str(s);
        b.push('\n');
    });
}

/// Acumula una línea de `eprint`. En el playground stderr y stdout se muestran juntos.
pub fn push_stderr(s: &str) {
    push_stdout(s);
}

/// Vacía y devuelve el buffer de salida acumulado.
fn tomar_salida() -> String {
    SALIDA.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

/// Reserva `len` bytes en la memoria lineal del módulo y devuelve el puntero. JS escribe ahí el fuente
/// antes de llamar a [`run`]. `Vec::<u8>::with_capacity(len)` da capacidad EXACTA `len` (u8), así que
/// [`run`]/[`dealloc`] pueden reconstruir el `Vec` con `(len, len)` sin UB.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf); // JS es dueño ahora; se libera al reconstruirlo en run/dealloc
    ptr
}

/// Libera un buffer `(ptr, len)` devuelto por [`alloc`] o por [`run`] (la salida). JS lo llama tras leer.
///
/// # Safety
/// `ptr`/`len` deben provenir de un [`alloc`] previo (o de la salida de [`run`]) y no haberse liberado ya.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(unsafe { Vec::from_raw_parts(ptr, len, len) });
    }
}

/// Compila y ejecuta el fuente (bytes UTF-8 en `[src_ptr, src_ptr+src_len)`) y devuelve la salida como
/// `(ptr << 32) | len` en la memoria del módulo. Libera el buffer de entrada.
///
/// # Safety
/// `src_ptr`/`src_len` deben provenir de un [`alloc`] previo con esa longitud.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn run(src_ptr: *mut u8, src_len: usize) -> u64 {
    // Reconstruye el fuente (y libera el buffer de entrada al final del bloque).
    let src = {
        let v = unsafe { Vec::from_raw_parts(src_ptr, src_len, src_len) };
        String::from_utf8_lossy(&v).into_owned()
    };
    SALIDA.with(|o| o.borrow_mut().clear());

    let salida = ejecutar_fuente(&src);

    // Deja la salida en la memoria lineal y devuelve (ptr, len) empaquetados. `shrink_to_fit` fuerza
    // capacidad == longitud para que `dealloc` reconstruya el `Vec` con `(len, len)` sin UB.
    let mut bytes = salida.into_bytes();
    bytes.shrink_to_fit();
    let len = bytes.len();
    let ptr = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    ((ptr as u64) << 32) | (len as u64)
}

/// El pipeline del playground: lex → parse → check → VM, capturando la salida y renderizando cualquier
/// error con contexto de fuente. Sin loader (un solo archivo; `import` daría un error de tipos claro).
fn ejecutar_fuente(src: &str) -> String {
    use crate::{checker, diagnostic, lexer, parser};

    let tokens = match lexer::lex(src) {
        Ok(t) => t,
        Err(e) => return diagnostic::render(src, e.line, e.col, e.len, &e.to_string()),
    };
    let mut program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => return diagnostic::render(src, e.line, e.col, e.len, &e.to_string()),
    };
    if let Err(e) = checker::check(&mut program) {
        return tomar_salida() + &diagnostic::render(src, e.line, e.col, e.len, &e.to_string());
    }
    match crate::run_on_vm(&program) {
        Ok(_) => tomar_salida(),
        // Un error de ejecución no lleva `len`; subrayamos 1 columna.
        Err(e) => tomar_salida() + &diagnostic::render(src, e.line, e.col, 1, &e.to_string()),
    }
}
