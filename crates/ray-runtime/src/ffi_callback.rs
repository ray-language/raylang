//! Callbacks C→raylang de la FFI: la reserva de trampolines (IDEAS §131, ray-daw #425).
//!
//! Para pasar una función de raylang a C hace falta un puntero a una función C REAL. Sin libffi no
//! se puede fabricar código en ejecución, así que hay una reserva fija de trampolines
//! `extern "C"` generados por macro; cada uno conoce su índice y despacha al manejador que el
//! motor (VM, intérprete o binario nativo) instaló en esa casilla.
//!
//! **Un trampolín para cualquier firma.** En SysV x86_64 y en AAPCS64 los argumentos enteros y
//! los de coma flotante viajan por bancos de registros independientes: una función declarada con
//! 6 enteros y 4 flotantes lee cualquier combinación de hasta esos tantos, en el orden de cada
//! banco. Solo el retorno pide dos variantes (registro entero o de coma flotante). En Windows x64
//! los argumentos comparten posición entre bancos, así que ahí solo valen los enteros/punteros
//! (hasta 6: cuatro por registro y dos en la pila, donde este trampolín también los lee); el
//! motor rechaza un callback con `float` en Windows antes de llegar aquí.
//!
//! **Por hilo.** La tabla de manejadores es local al hilo: un manejador captura estado del motor
//! que no es `Send`. Por eso un trampolín llamado desde un hilo que no instaló nada en su casilla
//! (C guardó el puntero y lo llama desde un hilo suyo, o después de liberarlo) no tiene a quién
//! despachar: escribe el motivo y aborta. No hay a quién devolver un error, y seguir sería
//! ejecutar raylang sin heap. La detección NO es completa: si la casilla ya volvió a ocuparse, el
//! puntero caducado despacha al callback que la tiene ahora. Usar el puntero fuera de su llamada
//! está fuera de contrato; las casillas rotan para que lo habitual sea el aborto.
//!
//! Este archivo solo usa `std`: el transpilador lo incluye tal cual en el binario nativo que se
//! compila con `rustc` a secas (sin `ray-runtime`).

use std::cell::RefCell;
use std::rc::Rc;

/// Callbacks vivos a la vez, por hilo y por clase de retorno.
pub const SLOTS: usize = 64;
/// Argumentos enteros/puntero que lee un trampolín.
pub const MAX_INT_ARGS: usize = 6;
/// Argumentos de coma flotante que lee un trampolín.
pub const MAX_FLOAT_ARGS: usize = 4;

/// Lo que C pasó al callback: los registros de cada banco, en orden. Los enteros llegan como los
/// dejó el llamador: de un `int` de C solo valen los 32 bits bajos, de un `bool` los 8 bajos.
#[derive(Clone, Copy, Debug, Default)]
pub struct Regs {
    pub ints: [i64; MAX_INT_ARGS],
    pub floats: [f64; MAX_FLOAT_ARGS],
}

/// El manejador de un callback: recibe los registros y devuelve el valor de retorno en crudo
/// (los bits del registro entero, o `f64::to_bits` si el callback devuelve `float`).
pub type Handler = Rc<dyn Fn(&Regs) -> u64>;

thread_local! {
    static INT_TABLE: RefCell<Vec<Option<Handler>>> = RefCell::new(vec![None; SLOTS]);
    static FLOAT_TABLE: RefCell<Vec<Option<Handler>>> = RefCell::new(vec![None; SLOTS]);
    // Por dónde seguir buscando casilla: las casillas ROTAN en vez de reutilizar la primera libre.
    // Un puntero que C guardó más allá de su llamada apunta a una casilla ya liberada; cuanto más
    // tarde en volver a ocuparse, más probable es que ese uso indebido acabe en el aborto con
    // motivo y no despachando a otro callback.
    static NEXT_SLOT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Un callback instalado: el puntero que se le da a C y la casilla que ocupa. Soltarlo libera la
/// casilla; el puntero deja de valer.
pub struct Callback {
    slot: usize,
    float_ret: bool,
    ptr: usize,
}

impl Callback {
    /// La dirección de la función C.
    pub fn ptr(&self) -> i64 {
        self.ptr as i64
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        let table = if self.float_ret { &FLOAT_TABLE } else { &INT_TABLE };
        // Durante el desmontaje del hilo la tabla puede haberse ido ya: nada que liberar.
        let _ = table.try_with(|t| t.borrow_mut()[self.slot] = None);
    }
}

/// Instala `handler` en una casilla libre del hilo actual. `float_ret` elige la variante del
/// trampolín: la que devuelve por el registro de coma flotante o la del registro entero.
pub fn install(float_ret: bool, handler: Handler) -> Result<Callback, String> {
    let table = if float_ret { &FLOAT_TABLE } else { &INT_TABLE };
    table.with(|t| {
        let mut t = t.borrow_mut();
        let start = NEXT_SLOT.with(|n| n.get());
        let Some(slot) = (0..SLOTS).map(|i| (start + i) % SLOTS).find(|&i| t[i].is_none()) else {
            return Err(format!("too many live FFI callbacks (at most {SLOTS} at a time)"));
        };
        NEXT_SLOT.with(|n| n.set((slot + 1) % SLOTS));
        t[slot] = Some(handler);
        let ptr = if float_ret { FLOAT_TRAMPOLINES[slot] as usize } else { INT_TRAMPOLINES[slot] as usize };
        Ok(Callback { slot, float_ret, ptr })
    })
}

// El manejador de la casilla, clonado FUERA del préstamo de la tabla: el callback puede instalar
// o liberar otros (una llamada `extern` anidada con su propio callback) mientras corre.
fn handler_of(float_ret: bool, slot: usize) -> Handler {
    let table = if float_ret { &FLOAT_TABLE } else { &INT_TABLE };
    let found = table.try_with(|t| t.borrow()[slot].clone()).ok().flatten();
    match found {
        Some(h) => h,
        None => {
            eprintln!(
                "fatal: an FFI callback was called from a thread that did not create it, or after the call that passed it had returned"
            );
            std::process::abort();
        }
    }
}

type Trampoline = extern "C" fn(i64, i64, i64, i64, i64, i64, f64, f64, f64, f64) -> i64;
type FloatTrampoline = extern "C" fn(i64, i64, i64, i64, i64, i64, f64, f64, f64, f64) -> f64;

// Un par de trampolines por casilla. `$n` es el índice (un literal por invocación: macro_rules no
// hace aritmética).
macro_rules! trampolines {
    ($($n:literal)*) => {
        static INT_TRAMPOLINES: [Trampoline; SLOTS] = [$({
            extern "C" fn t(a0: i64, a1: i64, a2: i64, a3: i64, a4: i64, a5: i64, f0: f64, f1: f64, f2: f64, f3: f64) -> i64 {
                handler_of(false, $n)(&Regs { ints: [a0, a1, a2, a3, a4, a5], floats: [f0, f1, f2, f3] }) as i64
            }
            t
        }),*];
        static FLOAT_TRAMPOLINES: [FloatTrampoline; SLOTS] = [$({
            extern "C" fn t(a0: i64, a1: i64, a2: i64, a3: i64, a4: i64, a5: i64, f0: f64, f1: f64, f2: f64, f3: f64) -> f64 {
                f64::from_bits(handler_of(true, $n)(&Regs { ints: [a0, a1, a2, a3, a4, a5], floats: [f0, f1, f2, f3] }))
            }
            t
        }),*];
    };
}
trampolines!(
    0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
    32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63
);

#[cfg(test)]
mod tests {
    use super::*;

    // Llama al puntero como lo haría C, con la firma REAL del callback (no la del trampolín).
    #[test]
    fn a_trampoline_reads_integer_and_float_arguments_in_any_mix() {
        let cb = install(false, Rc::new(|r: &Regs| (r.ints[0] as i32 as i64 + r.ints[1] + r.floats[0] as i64 + r.floats[1] as i64) as u64)).unwrap();
        let f: extern "C" fn(i32, f64, i64, f64) -> i64 = unsafe { std::mem::transmute(cb.ptr() as usize) };
        assert_eq!(f(-1, 10.0, 100, 1000.0), 1109);
    }

    #[test]
    fn a_float_returning_callback_uses_the_float_register() {
        let cb = install(true, Rc::new(|r: &Regs| (r.floats[0] * 2.0 + r.ints[0] as f64).to_bits())).unwrap();
        let f: extern "C" fn(f64, i64) -> f64 = unsafe { std::mem::transmute(cb.ptr() as usize) };
        assert_eq!(f(1.25, 3), 5.5);
    }

    #[test]
    fn slots_are_distinct_released_on_drop_and_bounded() {
        let mut live = Vec::new();
        for i in 0..SLOTS {
            live.push(install(false, Rc::new(move |_: &Regs| i as u64)).unwrap());
        }
        assert!(install(false, Rc::new(|_: &Regs| 0)).is_err());
        for (i, cb) in live.iter().enumerate() {
            let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(cb.ptr() as usize) };
            assert_eq!(f(), i as i64);
        }
        live.truncate(10);
        let again = install(false, Rc::new(|_: &Regs| 77)).unwrap();
        let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(again.ptr() as usize) };
        assert_eq!(f(), 77);
    }

    // Una casilla recién liberada no es la siguiente en ocuparse.
    #[test]
    fn a_released_slot_is_not_reused_right_away() {
        let first = install(false, Rc::new(|_: &Regs| 1)).unwrap();
        let stale = first.ptr();
        drop(first);
        let second = install(false, Rc::new(|_: &Regs| 2)).unwrap();
        assert_ne!(second.ptr(), stale);
    }

    // Un callback puede instalar otro mientras corre (una `extern` anidada).
    #[test]
    fn a_callback_can_install_another_while_running() {
        let outer = install(false, Rc::new(|r: &Regs| {
            let inner = install(false, Rc::new(|r: &Regs| (r.ints[0] * 2) as u64)).unwrap();
            let g: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(inner.ptr() as usize) };
            g(r.ints[0]) as u64 + 1
        }))
        .unwrap();
        let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(outer.ptr() as usize) };
        assert_eq!(f(20), 41);
    }
}
