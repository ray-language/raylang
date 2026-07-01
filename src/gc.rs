//! El **heap con recolección de basura** de la VM (M4.3).
//!
//! Hasta M4.2 la VM compartía el `Value` del intérprete, que usa `Rc` para los
//! datos compuestos. El `Rc` libera por **conteo de referencias**: es simple, pero
//! **no libera ciclos** (y las closures, al capturar celdas, los crean fácilmente).
//! M4.3 sustituye ese `Rc` en la VM por un **recolector trazador** (mark-and-sweep)
//! que sí los libera.
//!
//! ## Por qué un heap propio (con *handles*, no punteros)
//!
//! Un GC trazador debe **poseer** los objetos para poder liberarlos. En Rust, los
//! punteros crudos exigirían `unsafe`; en su lugar, el heap es un arreglo de
//! ranuras y un objeto se referencia por su **índice** (`Handle`). Es la misma idea
//! que un GC con punteros —marcar desde las raíces, barrer lo no marcado—, pero
//! segura y clara, fiel a nuestro lema de priorizar el aprendizaje. El precio es una
//! indirección por acceso.
//!
//! ## El algoritmo
//!
//! 1. **Marca**: desde las raíces (las da la VM: pila, locales de los marcos,
//!    upvalues), se marca todo lo alcanzable. Se usa una *lista gris* (worklist) en
//!    vez de recursión, para no desbordar la pila de Rust ni pelear con el
//!    *borrow checker*.
//! 2. **Barrido**: se recorren las ranuras; lo **no** marcado se libera (vuelve a la
//!    lista de libres) y a lo marcado se le limpia la marca para la próxima vuelta.
//! 3. **Disparo**: se recolecta cuando el número de objetos vivos cruza un umbral
//!    que **crece** tras cada recolección (estilo clox `nextGC`).

use crate::interpreter::MapKey;
use std::collections::{HashMap, VecDeque};

/// Un *handle*: la referencia a un objeto del heap (su índice de ranura).
pub type Handle = usize;

/// Un valor en la VM (M4.3). Los primitivos viven *inline*; los compuestos
/// (arreglo, struct, closure, celda) viven en el heap y aquí solo va su `Handle`.
#[derive(Clone)]
pub enum HeapValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Char(char), // M11.4c
    /// Entero sin signo con tamaño (M28.3): `(valor_enmascarado, ancho_en_bits)`. Escalar inline
    /// como `Int`/`Char`; no es objeto del heap ni lo traza el GC.
    UInt(u64, u8),
    /// Bytes (M16.1a): secuencia inmutable de octetos, **inline** en el valor (como `Str`); no es un
    /// objeto del heap ni lo traza el GC (no contiene handles).
    Bytes(Vec<u8>),
    Unit,
    /// Una función **sin** captura: un índice en la tabla de funciones (no es un
    /// objeto del heap, no se recolecta).
    Function(usize),
    /// Un objeto gestionado por el GC (arreglo, struct, closure o celda). El tipo
    /// concreto lo dice el `Obj` al que apunta el handle.
    Obj(Handle),
}

impl HeapValue {
    /// El handle si este valor referencia un objeto del heap; si no, `None`.
    pub fn handle(&self) -> Option<Handle> {
        match self {
            HeapValue::Obj(h) => Some(*h),
            _ => None,
        }
    }
}

/// Un struct en el heap: nombre + campos en orden de declaración.
pub struct VmStruct {
    pub name: String,
    pub fields: Vec<(String, HeapValue)>,
}

/// Una closure en el heap: el índice de su función y sus upvalues (handles a celdas).
pub struct VmClosure {
    pub index: usize,
    pub upvalues: Vec<Handle>,
}

/// Un valor de enum en el heap (M5): el *tag* de la variante (su índice en el enum)
/// y el payload posicional. `enum_id` indexa la tabla de enums del programa; juntos
/// dan el nombre del enum y de la variante para imprimir y para el oráculo.
pub struct VmEnum {
    pub enum_id: usize,
    pub tag: usize,
    pub payload: Vec<HeapValue>,
}

/// Un canal `Channel<T>` (M12.1): una cola FIFO de valores en tránsito + si está cerrado. Los receptores
/// y emisores bloqueados NO viven aquí, sino en el scheduler de la VM (`parked`), para no acoplar el GC a
/// las fibras.
///
/// `cap` (M12.2) fija la capacidad: `None` = **no acotado** (la cola crece sin límite, `send` nunca
/// bloquea); `Some(n)` = **acotado** a `n` (`send` bloquea cuando `queue.len() == n` → backpressure), con
/// `n = 0` un canal **rendezvous** (síncrono: `send` y `recv` se encuentran, la cola siempre está vacía).
pub struct VmChannel {
    pub queue: VecDeque<HeapValue>,
    pub closed: bool,
    pub cap: Option<usize>,
}

/// El estado de una `Task<T>` (M12.3, structured concurrency):
/// - `Pending`: la fibra todavía corre (los que la unan se bloquean).
/// - `Done(valor)`: terminó normal; `join` devuelve `valor`.
/// - `Failed(mensaje)`: terminó con un panic; `join`/`scope` re-lanzan ese mensaje (propagación).
pub enum TaskState {
    Pending,
    Done(HeapValue),
    Failed(String),
}

/// Una tarea `Task<T>` (M12.3): el handle a una fibra `spawn`eada, con su estado de terminación. El GC
/// traza el valor de `Done` (las fibras que esperan a la tarea viven en el scheduler de la VM).
pub struct VmTask {
    pub state: TaskState,
}

/// Un objeto del heap. Las formas compuestas que el GC gestiona.
pub enum Obj {
    Array(Vec<HeapValue>),
    Struct(VmStruct),
    Closure(VmClosure),
    /// Un enum: variante + payload (M5). El GC traza su payload.
    Enum(VmEnum),
    /// Una **celda**: una variable *boxeada* (un local capturado o un upvalue). Es
    /// lo que comparten una closure y el dueño de la variable (M4.2).
    Cell(HeapValue),
    /// Un mapa `Map<K, V>` (M13.1): clave hashable → valor. El GC traza los **valores**
    /// (las claves son primitivos *inline*, sin handles).
    Map(HashMap<MapKey, HeapValue>),
    /// Un canal `Channel<T>` (M12.1): el GC traza los valores **en tránsito** (la cola).
    Channel(VmChannel),
    /// Una tarea `Task<T>` (M12.3): el GC traza el valor de `Done`.
    Task(VmTask),
}

/// Una ranura del heap: un objeto y su bit de marca.
struct Slot {
    obj: Obj,
    marked: bool,
}

/// Umbral inicial de objetos vivos antes de la primera recolección. Pequeño a
/// propósito: así los programas de prueba ejercitan el GC pronto.
const INITIAL_GC: usize = 64;

/// El heap: las ranuras, la lista de ranuras libres (para reusar handles), el conteo
/// de vivos y el umbral de disparo.
pub struct Heap {
    slots: Vec<Option<Slot>>,
    free: Vec<Handle>,
    /// Lista gris del marcado: handles ya marcados pero cuyos hijos faltan por trazar.
    gray: Vec<Handle>,
    live: usize,
    next_gc: usize,
    /// Modo de estrés: si está activo, la VM recolecta en **cada** punto seguro. Sirve
    /// para destapar raíces faltantes en los tests.
    pub stress: bool,
}

impl Default for Heap {
    fn default() -> Self {
        Heap {
            slots: Vec::new(),
            free: Vec::new(),
            gray: Vec::new(),
            live: 0,
            next_gc: INITIAL_GC,
            stress: false,
        }
    }
}

impl Heap {
    pub fn new() -> Self {
        Heap::default()
    }

    /// Reserva un objeto y devuelve su handle. Reusa una ranura libre si la hay.
    pub fn allocate(&mut self, obj: Obj) -> Handle {
        self.live += 1;
        let slot = Slot { obj, marked: false };
        if let Some(h) = self.free.pop() {
            self.slots[h] = Some(slot);
            h
        } else {
            self.slots.push(Some(slot));
            self.slots.len() - 1
        }
    }

    pub fn get(&self, h: Handle) -> &Obj {
        &self.slots[h].as_ref().expect("handle válido (objeto vivo)").obj
    }

    pub fn get_mut(&mut self, h: Handle) -> &mut Obj {
        &mut self.slots[h].as_mut().expect("handle válido (objeto vivo)").obj
    }

    /// ¿Conviene recolectar? (En modo estrés, siempre.)
    pub fn should_collect(&self) -> bool {
        self.stress || self.live >= self.next_gc
    }

    /// Número de objetos vivos (para tests/diagnóstico).
    pub fn live(&self) -> usize {
        self.live
    }

    // ----- Marcado (la VM aporta las raíces) -----

    /// Marca un objeto como alcanzable y lo encola para trazar sus hijos. La VM
    /// llama esto por cada raíz; luego `trace` propaga.
    pub fn mark(&mut self, h: Handle) {
        if let Some(slot) = self.slots[h].as_mut() {
            if !slot.marked {
                slot.marked = true;
                self.gray.push(h);
            }
        }
    }

    /// Propaga la marca: vacía la lista gris marcando los hijos de cada objeto.
    pub fn trace(&mut self) {
        while let Some(h) = self.gray.pop() {
            for child in self.children(h) {
                self.mark(child);
            }
        }
    }

    /// Los handles a los que apunta un objeto (sus hijos en el grafo de objetos).
    fn children(&self, h: Handle) -> Vec<Handle> {
        match self.get(h) {
            Obj::Array(v) => v.iter().filter_map(HeapValue::handle).collect(),
            Obj::Struct(s) => s.fields.iter().filter_map(|(_, v)| v.handle()).collect(),
            Obj::Closure(c) => c.upvalues.clone(),
            Obj::Enum(e) => e.payload.iter().filter_map(HeapValue::handle).collect(),
            Obj::Cell(v) => v.handle().into_iter().collect(),
            // M13.1: las claves son primitivos (sin handles); solo se trazan los valores.
            Obj::Map(m) => m.values().filter_map(HeapValue::handle).collect(),
            // M12.1: los valores en tránsito en el canal son raíces mientras estén en la cola.
            Obj::Channel(c) => c.queue.iter().filter_map(HeapValue::handle).collect(),
            // M12.3: el valor de una tarea terminada con éxito es raíz hasta que alguien la una.
            Obj::Task(t) => match &t.state {
                TaskState::Done(v) => v.handle().into_iter().collect(),
                _ => Vec::new(),
            },
        }
    }

    /// Barrido: libera lo no marcado y limpia las marcas de los sobrevivientes.
    /// Ajusta el umbral para la próxima recolección.
    pub fn sweep(&mut self) {
        for (h, opt) in self.slots.iter_mut().enumerate() {
            if let Some(slot) = opt {
                if slot.marked {
                    slot.marked = false; // limpiar para la próxima vuelta
                } else {
                    *opt = None; // liberar
                    self.free.push(h);
                    self.live -= 1;
                }
            }
        }
        // El umbral crece con la población viva (amortiza el costo del GC).
        self.next_gc = (self.live * 2).max(INITIAL_GC);
    }
}
