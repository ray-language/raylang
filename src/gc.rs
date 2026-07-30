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

use crate::runtime::MapKey;
use std::collections::{HashMap, VecDeque};

/// Almacén interno de un `Map` en la VM (P0.1, perf): `HashMap` con el hasher **aHash** en vez del
/// SipHash de std — 2–5× más rápido sobre claves cortas (int/string), con resistencia a hash-flooding.
/// En wasm (playground) no hay aHash (su runtime-rng exige getrandom) → SipHash de std, como pre-P0.1.
#[cfg(not(target_arch = "wasm32"))]
pub type MapStore = HashMap<MapKey, HeapValue, ahash::RandomState>;
#[cfg(target_arch = "wasm32")]
pub type MapStore = HashMap<MapKey, HeapValue>;
/// TA4: mapa (enum_id, tag) → handle canónico de la variante SIN payload (ver `Fiber.unit_enums`).
#[cfg(not(target_arch = "wasm32"))]
pub type MapStore2 = HashMap<(u32, u32), Handle, ahash::RandomState>;
#[cfg(target_arch = "wasm32")]
pub type MapStore2 = HashMap<(u32, u32), Handle>;

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
    /// Un **puntero opaco** foráneo (`ptr`, M41.4b): la dirección de un objeto de C, escalar inline. No
    /// es objeto del heap ni lo traza el GC (no contiene handles). Se compara por identidad.
    Ptr(i64),
    /// M38.1b: un **canal** (`Channel<T>`) por su **id** en el almacén del host de la VM (`Vm.channels`),
    /// NO un objeto del heap. Los canales son sincronización COMPARTIDA entre actores (§46.2): viven fuera
    /// del GC de cualquier fibra. Los valores en tránsito en su cola sí son raíces (los rootea la VM).
    Channel(usize),
    /// M38.1b: una **tarea** (`Task<T>`) por su **id** en el almacén del host (`Vm.tasks`), NO un objeto del
    /// heap. Compartida entre la fibra hija y quien la une. El valor de `Done` es raíz (lo rootea la VM).
    Task(usize),
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
    /// TA1 (bench treealloc, 22 jul 2026): la instancia NO lleva metadatos — solo el índice de su
    /// definición (`CompiledProgram.structs`) y los VALORES en orden de declaración. Antes cada
    /// instancia clonaba el nombre del struct + el nombre de CADA campo (Strings): en un árbol de
    /// binary-trees eso era ~150 B y 4 mallocs de metadatos POR NODO. Los nombres se consultan en
    /// la tabla del programa (acceso a campo, Show, borde a intérprete).
    pub struct_idx: usize,
    pub fields: Vec<HeapValue>,
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
    /// TA2: u32 (ningún programa real se acerca a 4G enums/variantes) → VmEnum 40→32 B, y con él
    /// `Obj` y el `Slot` del heap.
    pub enum_id: u32,
    pub tag: u32,
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
    /// M38.1b-2: heap propio del canal para los valores **en tránsito** (los de `queue`). Un valor no
    /// pertenece a ninguna fibra entre `send` y `recv`; se **transfiere** al heap del canal en `send` y de
    /// ahí al heap del receptor en `recv`. Se **limpia cuando la cola se vacía** (nadie referencia sus
    /// objetos entonces) → acota su tamaño sin un GC propio.
    pub heap: Heap,
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
    /// M97.1/M98.1: la semántica "un fallo observado es un fallo manejado" la implementa ahora la
    /// LIBERACIÓN del slot (M98.1): `try_join` consume la entrada de una tarea fallida y el
    /// `ScopeEnd` la salta por handle stale — ya no hace falta un flag `observed`.
    /// M38.1b-2: heap propio de la tarea para su valor de `Done` (producido por la fibra hija, cuyo heap
    /// se descarta al terminar; se **transfiere** aquí en `on_fiber_done` y de aquí al heap del que la
    /// une en `join`).
    pub heap: Heap,
}

/// Un objeto del heap. Las formas compuestas que el GC gestiona.
pub enum Obj {
    Array(Vec<HeapValue>),
    /// M98.5: arreglo **homogéneo de ints** (*storage strategy*, estilo V8/PyPy): 8 B/elemento en
    /// vez de los 32 B del `HeapValue` (docs/investigacion-uso-de-memoria.md §4) y trazado O(1)
    /// (sin handles). Nace en `MakeArray` con todos los elementos `Int`, o al hacer `push` de un
    /// `Int` sobre un arreglo genérico VACÍO (el patrón `var xs = []; while … push`). Cualquier
    /// operación no especializada lo **degrada** in place a `Array` (`Heap::degrade_int_array`,
    /// invisible para el programa); las calientes (push/index/set/len/pop) lo manejan nativo.
    IntArray(Vec<i64>),
    /// MM3 (bench matrixmul, 22 jul 2026 — la P1.2 pendiente): arreglo homogéneo de FLOATS,
    /// gemelo de `IntArray` (8 B/elemento en vez del `HeapValue` de 32; trazado O(1), sin handles).
    /// Nace en `specialize_array` (literal con todos Float) o al hacer `push` de un Float sobre un
    /// arreglo genérico vacío; cualquier operación no especializada lo degrada (mismo embudo que
    /// IntArray). Crítico en cómputo numérico: 4× de densidad de caché en `a[i][k]*b[k][j]`.
    FloatArray(Vec<f64>),
    Struct(VmStruct),
    Closure(VmClosure),
    /// Un enum: variante + payload (M5). El GC traza su payload.
    Enum(VmEnum),
    /// Una **celda**: una variable *boxeada* (un local capturado o un upvalue). Es
    /// lo que comparten una closure y el dueño de la variable (M4.2).
    Cell(HeapValue),
    /// Un mapa `Map<K, V>` (M13.1): clave hashable → valor. El GC traza los **valores**
    /// (las claves son primitivos *inline*, sin handles). Usa el hasher aHash (P0.1) vía [`MapStore`].
    /// TA2 (bench treealloc, 22 jul 2026): el Map va BOXEADO — `MapStore` (HashMap, 64 B) era la
    /// variante más grande y dimensionaba TODO `Obj` (72 B) y con él cada `Slot` del heap (88 B ×
    /// objeto vivo o libre). Boxeándolo, `Obj` lo dimensiona `VmEnum` y el Slot baja a ~48 B: el
    /// vector de slots es el mayor componente del pico en cargas de muchos objetos pequeños.
    Map(Box<MapStore>),
    // M38.1b: `Channel`/`Task` YA NO son objetos del heap. Son sincronización compartida entre actores,
    // viven en almacenes del host de la VM (`Vm.channels`/`Vm.tasks`) referenciados por
    // `HeapValue::Channel(id)`/`Task(id)`. Sus structs (`VmChannel`/`VmTask`/`TaskState`) siguen definidos
    // aquí (los usa la VM); el GC solo rootea los valores en tránsito / de `Done` que la VM le aporta.
}

/// Una ranura del heap: un objeto, su bit de marca y su tamaño estimado en bytes (V6).
struct Slot {
    obj: Obj,
    marked: bool,
    /// V6 (bench políglota): estimación del payload en BYTES (buffers de String/Bytes + los Vec del
    /// contenedor), calculada al asignar y REFRESCADA en cada sweep (las mutaciones entre GCs —
    /// push/insert— derivan la cuenta; el refresco la corrige). Gobierna `live_bytes`.
    /// TA2: u32 SATURADO (un objeto de >4 GiB satura la cuenta, no la corrompe) → Slot 8 B menos.
    bytes: u32,
}

/// Umbral inicial de objetos vivos antes de la primera recolección. Pequeño a
/// propósito: así los programas de prueba ejercitan el GC pronto.
const INITIAL_GC: usize = 64;

/// V6: umbral mínimo de BYTES estimados vivos+basura antes de un GC por bytes (16 MiB). Lo bastante
/// alto para que los programas pequeños nunca disparen por bytes (cero coste extra), y lo bastante
/// bajo para acotar el pico cuando pocos objetos acumulan mucha basura de buffers.
const INITIAL_GC_BYTES: usize = 16 << 20;

/// V6: bytes del *payload* inline de un valor (los buffers que el conteo por objetos no ve).
fn value_bytes(v: &HeapValue) -> usize {
    match v {
        HeapValue::Str(s) => s.capacity(),
        HeapValue::Bytes(b) => b.capacity(),
        _ => 0,
    }
}

/// V6: estimación en bytes del payload de un objeto (el Vec del contenedor + los buffers inline de
/// sus elementos). O(elementos) — el mismo orden que costó construir el objeto; se calcula al
/// asignar y se refresca en cada sweep.
fn obj_bytes(obj: &Obj) -> usize {
    match obj {
        Obj::Array(v) => {
            v.capacity() * std::mem::size_of::<HeapValue>() + v.iter().map(value_bytes).sum::<usize>()
        }
        Obj::IntArray(v) => v.capacity() * 8,
        Obj::FloatArray(v) => v.capacity() * 8,
        Obj::Struct(s) => {
            s.fields.capacity() * std::mem::size_of::<HeapValue>()
                + s.fields.iter().map(value_bytes).sum::<usize>()
        }
        Obj::Closure(c) => c.upvalues.len() * 8,
        Obj::Enum(e) => {
            e.payload.len() * std::mem::size_of::<HeapValue>()
                + e.payload.iter().map(value_bytes).sum::<usize>()
        }
        Obj::Cell(v) => std::mem::size_of::<HeapValue>() + value_bytes(v),
        Obj::Map(m) => {
            m.capacity() * (std::mem::size_of::<MapKey>() + std::mem::size_of::<HeapValue>())
                + m.iter()
                    .map(|(k, v)| {
                        let kb = match k {
                            MapKey::Str(s) => s.capacity(),
                            MapKey::Bytes(b) => b.capacity(),
                            _ => 0,
                        };
                        kb + value_bytes(v)
                    })
                    .sum::<usize>()
        }
    }
}

/// El heap: las ranuras, la lista de ranuras libres (para reusar handles), el conteo
/// de vivos y el umbral de disparo.
pub struct Heap {
    slots: Vec<Option<Slot>>,
    free: Vec<Handle>,
    /// Lista gris del marcado: handles ya marcados pero cuyos hijos faltan por trazar.
    gray: Vec<Handle>,
    live: usize,
    next_gc: usize,
    /// Opt.13: elementos escaneados por el ÚLTIMO trazado (lo llena `trace`, lo
    /// consume `sweep` para amortizar el umbral por trabajo, no solo por conteo).
    traced_work: usize,
    /// M42.2: **tope de heap** — máximo de objetos vivos permitidos. Junto al fuel (cuenta de
    /// instrucciones), es el otro recurso a acotar para embeber raylang confinado. `usize::MAX` =
    /// **sin límite** (el default): nunca dispara, coste nulo. Al acercarse al tope se fuerza un GC
    /// (ver `should_collect`); si tras recolectar sigue por encima, la VM aborta (`over_cap`).
    max_live: usize,
    /// Modo de estrés: si está activo, la VM recolecta en **cada** punto seguro. Sirve
    /// para destapar raíces faltantes en los tests.
    pub stress: bool,
    /// V6 (bench políglota): bytes estimados VIVOS (suma de `Slot.bytes`). El umbral por Nº de
    /// objetos es CIEGO a los bytes: pocos objetos grandes (arrays de strings de un `split`
    /// gigante) acumulaban cientos de MB de basura sin disparar el GC (medido: 536 MB de pico con
    /// ~3 MB vivos). Este contador añade un disparo por BYTES (`should_collect`).
    live_bytes: usize,
    /// V6: umbral de bytes para el próximo GC (doblado por bytes vivos, mínimo `INITIAL_GC_BYTES`).
    next_gc_bytes: usize,
    /// TA (bench treealloc, 22 jul 2026): sonda de picos EXACTOS del heap (`RAY_HEAP_STATS=1`),
    /// inmune al ruido del allocador (RSS/commit de mimalloc resultaron ADAPTATIVOS entre tandas —
    /// la lección de la Fase 64: medir memoria de la VM con esta sonda, no con el pico del SO).
    probe_peak_live: usize,
    probe_peak_bytes: usize,
    probe_total_allocs: usize,
    probe_total_bytes: usize,
    /// V11: ¿la sonda está activa (RAY_HEAP_STATS=1)? Leído UNA vez al crear el heap —
    /// el camino caliente de `allocate` paga una rama predecible en vez de 4 contadores.
    probes_on: bool,
}

impl Default for Heap {
    fn default() -> Self {
        Heap {
            slots: Vec::new(),
            free: Vec::new(),
            gray: Vec::new(),
            live: 0,
            next_gc: INITIAL_GC,
            traced_work: 0,
            max_live: usize::MAX,
            stress: false,
            live_bytes: 0,
            next_gc_bytes: INITIAL_GC_BYTES,
            probe_peak_live: 0,
            probe_peak_bytes: 0,
            probe_total_allocs: 0,
            probe_total_bytes: 0,
            probes_on: std::env::var_os("RAY_HEAP_STATS").is_some(),
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
        let bytes = obj_bytes(&obj).min(u32::MAX as usize) as u32;
        self.live_bytes += bytes as usize;
        if self.probes_on {
            self.probe_total_allocs += 1;
            self.probe_total_bytes += bytes as usize;
            if self.live > self.probe_peak_live { self.probe_peak_live = self.live; }
            if self.live_bytes > self.probe_peak_bytes { self.probe_peak_bytes = self.live_bytes; }
        }
        let slot = Slot { obj, marked: false, bytes };
        if let Some(h) = self.free.pop() {
            self.slots[h] = Some(slot);
            h
        } else {
            self.slots.push(Some(slot));
            self.slots.len() - 1
        }
    }

    pub fn get(&self, h: Handle) -> &Obj {
        &self.slots[h].as_ref().expect("valid handle (live object)").obj
    }

    pub fn get_mut(&mut self, h: Handle) -> &mut Obj {
        &mut self.slots[h].as_mut().expect("valid handle (live object)").obj
    }

    /// M98.5: **degrada** un `IntArray` a `Array` genérico in place (mismo handle → el aliasing se
    /// preserva). Lo llama toda operación no especializada antes de tratar el objeto como `Array`;
    /// es un no-op sobre cualquier otra forma. Invisible para el programa (mismos valores).
    pub fn degrade_int_array(&mut self, h: Handle) {
        let obj = self.get_mut(h);
        if let Obj::IntArray(v) = obj {
            let elems: Vec<HeapValue> = v.iter().map(|&i| HeapValue::Int(i)).collect();
            *obj = Obj::Array(elems);
        } else if let Obj::FloatArray(v) = obj {
            // MM3: mismo embudo para el gemelo de floats.
            let elems: Vec<HeapValue> = v.iter().map(|&f| HeapValue::Float(f)).collect();
            *obj = Obj::Array(elems);
        }
    }

    /// ¿Conviene recolectar? (En modo estrés, siempre.) M42.2: también al alcanzar el tope de
    /// heap, para forzar un GC antes de rebasarlo (si tras recolectar sigue por encima, `over_cap`).
    pub fn should_collect(&self) -> bool {
        self.stress
            || self.live >= self.next_gc
            || self.live >= self.max_live
            // V6: disparo por BYTES estimados — cubre el caso "pocos objetos, buffers enormes"
            // (arrays/maps de strings grandes) que el conteo por objetos no ve.
            || self.live_bytes >= self.next_gc_bytes
    }

    /// M42.2: ¿se rebasó el tope de heap tras recolectar? La VM lo consulta después del GC: si es
    /// cierto, el programa necesita más objetos vivos de los permitidos → aborta.
    pub fn over_cap(&self) -> bool {
        self.live > self.max_live
    }

    /// M42.2: fija el tope de objetos vivos (para embeber raylang confinado).
    pub fn set_max_live(&mut self, n: usize) {
        self.max_live = n;
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
    /// Opt.13: además CONTABILIZA el trabajo de trazado (elementos escaneados), que
    /// `sweep` usa para amortizar el umbral — un heap con un contenedor grande vivo
    /// paga O(sus elementos) por recolección aunque haya POCOS objetos, y con el
    /// umbral por conteo (`live*2`, mínimo 64) el GC corría cada ~50 asignaciones
    /// re-escaneando el contenedor entero (medido: `for x in xs.iter()` sobre 1M
    /// costaba 6.8 µs/elemento; el mismo bucle sobre 1k, 0.31 µs → 22×).
    pub fn trace(&mut self) {
        let mut work = 0usize;
        while let Some(h) = self.gray.pop() {
            work += self.trace_cost(h);
            for child in self.children(h) {
                self.mark(child);
            }
        }
        self.traced_work = work;
    }

    /// Opt.13: el coste de trazar un objeto = 1 + los elementos que `children`
    /// escanea (aunque sean primitivos sin handle: el escaneo se paga igual).
    fn trace_cost(&self, h: Handle) -> usize {
        1 + match self.get(h) {
            Obj::Array(v) => v.len(),
            Obj::IntArray(_) | Obj::FloatArray(_) => 0, // M98.5/MM3: sin handles → coste constante
            Obj::Struct(s) => s.fields.len(),
            Obj::Closure(c) => c.upvalues.len(),
            Obj::Enum(e) => e.payload.len(),
            Obj::Cell(_) => 1,
            Obj::Map(m) => m.len(),
        }
    }

    /// Los handles a los que apunta un objeto (sus hijos en el grafo de objetos).
    fn children(&self, h: Handle) -> Vec<Handle> {
        match self.get(h) {
            Obj::Array(v) => v.iter().filter_map(HeapValue::handle).collect(),
            Obj::IntArray(_) | Obj::FloatArray(_) => Vec::new(), // M98.5/MM3: inline, sin hijos
            Obj::Struct(s) => s.fields.iter().filter_map(|v| v.handle()).collect(),
            Obj::Closure(c) => c.upvalues.clone(),
            Obj::Enum(e) => e.payload.iter().filter_map(HeapValue::handle).collect(),
            Obj::Cell(v) => v.handle().into_iter().collect(),
            // M13.1: las claves son primitivos (sin handles); solo se trazan los valores.
            Obj::Map(m) => m.values().filter_map(HeapValue::handle).collect(),
            // M38.1b: Channel/Task ya no son objetos del heap (viven en el host); sus valores en
            // tránsito / de Done los rootea la VM directamente en `collect`.
        }
    }

    /// Barrido: libera lo no marcado y limpia las marcas de los sobrevivientes.
    /// Ajusta el umbral para la próxima recolección.
    pub fn sweep(&mut self) {
        // V6: la cuenta de bytes se RECOMPUTA de los supervivientes (corrige la deriva por
        // mutación —push/insert— entre GCs; O(elementos vivos), mismo orden que el marcado).
        let mut live_bytes = 0usize;
        for (h, opt) in self.slots.iter_mut().enumerate() {
            if let Some(slot) = opt {
                if slot.marked {
                    slot.marked = false; // limpiar para la próxima vuelta
                    slot.bytes = obj_bytes(&slot.obj).min(u32::MAX as usize) as u32;
                    live_bytes += slot.bytes as usize;
                } else {
                    *opt = None; // liberar
                    self.free.push(h);
                    self.live -= 1;
                }
            }
        }
        self.live_bytes = live_bytes;
        // V6: umbral de bytes doblado por vivos, con suelo (los programas pequeños no disparan
        // nunca por bytes → coste cero para ellos).
        self.next_gc_bytes = (self.live_bytes * 2).max(INITIAL_GC_BYTES);
        // El umbral crece con la población viva Y con el TRABAJO de la recolección
        // recién hecha (Opt.13): tras un trazado que escaneó W elementos se permiten
        // al menos W/4 asignaciones antes del próximo GC → el coste se amortiza a
        // O(1) por asignación aunque haya contenedores grandes vivos con pocos
        // objetos. Contrapartida consciente: más basura transitoria entre GCs
        // (espacio por tiempo); el tope de heap (`max_live`, M42.2) sigue mandando.
        self.next_gc = (self.live * 2).max(self.live + self.traced_work / 4).max(INITIAL_GC);
    }
}

impl Heap {
    /// Volcado de la sonda de picos exactos (`RAY_HEAP_STATS=1`); ver los campos `probe_*`.
    pub fn dump_probe(&self) {
        if std::env::var_os("RAY_HEAP_STATS").is_some() {
            eprintln!(
                "HEAP_STATS peak_live_objs={} peak_live_bytes={} slots_cap={} slot_size={} total_allocs={} total_bytes={}",
                self.probe_peak_live, self.probe_peak_bytes, self.slots.capacity(),
                std::mem::size_of::<Option<Slot>>(), self.probe_total_allocs, self.probe_total_bytes
            );
        }
    }
}

#[cfg(test)]
mod size_guard {
    use super::*;
    /// TA2: el `Slot` dimensiona el vector de ranuras del heap — el MAYOR componente del pico en
    /// cargas de muchos objetos (48 B × ranura). Esta guardia impide que una variante nueva de
    /// `Obj` lo re-infle en silencio (la variante grande se BOXEA, como `Map`).
    #[test]
    fn slot_stays_small() {
        assert!(std::mem::size_of::<Option<Slot>>() <= 48,
            "Slot creció a {} B (>48): boxea la variante grande de Obj (como Map)",
            std::mem::size_of::<Option<Slot>>());
        assert!(std::mem::size_of::<HeapValue>() <= 32, "HeapValue creció: {}", std::mem::size_of::<HeapValue>());
    }
}
