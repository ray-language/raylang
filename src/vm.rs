//! La máquina virtual (VM) de raylang (M2, con GC en M4.3).
//!
//! Ejecuta bytecode sobre una **pila de operandos** y una **pila de marcos de
//! llamada** explícita (no la pila de Rust). Reificar los marcos así es lo que
//! mantiene abierta la puerta a la concurrencia (ver IDEAS.md §1) y, ahora, lo que
//! hace **enumerables las raíces** del recolector de basura.
//!
//! ## Memoria (M4.3)
//!
//! Los datos compuestos (arreglos, structs, closures y celdas) viven en un **heap
//! propio** (`gc::Heap`) y se referencian por *handle*. Un recolector
//! **mark-and-sweep** los libera —incluidos los ciclos, que el `Rc` del intérprete
//! no puede—. El intérprete se queda con `Rc` y hace de oráculo.
//!
//! El GC se dispara solo en **puntos seguros**: al inicio del bucle de
//! instrucciones, cuando todos los valores vivos están en la pila o los marcos (no
//! hay temporales a medio ensamblar en variables de Rust). Así marcar desde la pila
//! y los marcos es correcto sin más cuidado.

use std::cell::RefCell;
use std::rc::Rc;

use crate::bytecode::{CastTarget, Chunk, CompiledEnum, CompiledFn, CompiledProgram, OpCode, UpvalueSource};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::gc::{Handle, Heap, HeapValue, Obj, TaskState, VmChannel, VmClosure, VmEnum, VmStruct, VmTask};
use crate::runtime::{EnumInstance, MapKey, RuntimeError, StructInstance, Value};

/// Límite de marcos para detectar recursión infinita en vez de colgarse. Es el
/// **mismo** que el del intérprete (`runtime::MAX_CALL_DEPTH`, M13.3a) para que
/// ambos motores coincidan en la frontera: un programa que recurre justo al límite
/// da el mismo veredicto en los dos.
const MAX_FRAMES: usize = crate::runtime::MAX_CALL_DEPTH;

/// M38.3b paso 3: cuando un worker no encuentra fibra lista pero otro sigue ejecutando (`running > 0`),
/// espera con un *busy-poll* de baja frecuencia (en vez de una `Condvar`, que exigiría pasar el guard del
/// `Mutex` por valor y es más propensa a *lost-wakeups*). Este es el intervalo entre reintentos: corto para
/// baja latencia al despertar, pero no cero para no quemar un núcleo. Sólo se alcanza cuando un worker está
/// ocioso con trabajo pendiente en otro hilo (raro en cargas bien paralelizadas). Con N=1 nunca se usa.
const SPIN_SLEEP_US: u64 = 50;

/// M38.4: bandera global de **modo determinista** (`--deterministic`). La fija la CLI (proceso-local, como
/// `set_program_args`) antes de ejecutar; fuerza el scheduler **M:1 reproducible** (un hilo, orden FIFO) para
/// tests y para el oráculo, aunque el default sea multicore. `Relaxed` basta: se escribe una vez al arranque,
/// antes de lanzar hilos worker.
static FORCE_DETERMINISTIC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// M38.4: activa/desactiva el modo determinista (`--deterministic`). La CLI la llama al parsear el flag.
pub fn set_deterministic(v: bool) {
    FORCE_DETERMINISTIC.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// M38.4: nº de hilos worker del scheduler M:N. **El multicore es el default** (§46.4); lo determinista es
/// opt-in. Reglas, en orden:
/// 1. `--deterministic` (o el default de los tests) → **1** (M:1 reproducible; el oráculo/M12 lo exige).
/// 2. `RAYLANG_THREADS=N` explícito → **N** (override; `=1` = forzar determinista).
/// 3. El programa **no usa `spawn`** → **1**: sólo existe la fibra `main`, el multicore no aporta nada y
///    evitamos el coste de lanzar hilos (la inmensa mayoría de programas, incluidos todos los del oráculo).
/// 4. Concurrente y sin override → **`available_parallelism()`** (multicore por defecto).
///
/// El resultado se clampa al rango 1..=256.
fn num_workers(program: &CompiledProgram) -> usize {
    // M44a: en `wasm32` (el playground web) no hay hilos del SO → siempre 1 (nunca se invoca `thread::
    // scope`). `available_parallelism()` ya daría `Err`→1 allí, pero lo forzamos por robustez.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = program;
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if FORCE_DETERMINISTIC.load(std::sync::atomic::Ordering::Relaxed) {
            return 1;
        }
        if let Ok(s) = std::env::var("RAYLANG_THREADS") {
            return s.trim().parse::<usize>().unwrap_or(1).clamp(1, 256);
        }
        if !program_uses_spawn(program) {
            return 1;
        }
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 256)
    }
}

/// M38.4: ¿el programa contiene el opcode `Spawn`? Si no, sólo hay la fibra `main` → el scheduler M:N no
/// aporta nada y `num_workers` devuelve 1 (sin lanzar hilos). Un escaneo único y barato del bytecode.
/// M44a: solo lo usa la rama no-wasm de `num_workers` (en wasm siempre es 1).
#[cfg(not(target_arch = "wasm32"))]
fn program_uses_spawn(program: &CompiledProgram) -> bool {
    program.functions.iter().any(|f| f.chunk.code.iter().any(|op| matches!(op, OpCode::Spawn)))
}

/// M38.3b paso 3: una referencia al programa compilado **compartible entre hilos worker**. `CompiledProgram`
/// contiene `Value` (las constantes del chunk) que usa `Rc` → es `!Send`/`!Sync` **por tipo**. Pero durante
/// la ejecución el programa es **inmutable** y el único acceso a sus constantes (`const_to_heap`) sólo LEE:
/// copia los escalares y clona el `Vec` interno de `Bytes` **por deref** (nunca clona ni dropea un `Rc`, así
/// que ningún refcount se toca) → compartir esta referencia inmutable entre hilos es sano. El wrapper lo
/// afirma con `unsafe impl`; sólo cruza el borde del `thread::spawn` (dentro del worker se usa como `&`).
#[derive(Clone, Copy)]
struct ProgRef<'a>(&'a CompiledProgram);
// SAFETY: el programa es inmutable durante la ejecución y sus constantes sólo se leen (sin tocar refcounts
// de `Rc`), luego el `!Sync` de `Value` no puede provocar carreras. Ver la doc de `ProgRef`.
unsafe impl Send for ProgRef<'_> {}
unsafe impl Sync for ProgRef<'_> {}

/// Ejecuta un programa compilado (empezando por `main`) y devuelve su resultado.
pub fn run_program(program: &CompiledProgram) -> Result<Value, RuntimeError> {
    let mut vm = Vm::new(program);
    let result = vm.run()?;
    vm.print_gc_stats_if_requested(); // M37.1
    Ok(to_value(&vm.cur.heap, &program.enums, &result))
}

/// Como [`run_program`], pero con **límites de recursos** para embeber raylang confinado (un bucle
/// infinito o una entrada maliciosa no cuelgan ni agotan la memoria del anfitrión):
/// - `fuel` (M42.1): presupuesto de **instrucciones**; al superarlo, aborta en vez de correr sin fin.
/// - `heap_cap` (M42.2): tope de **objetos vivos**; al rebasarlo (tras un GC), aborta.
///
/// `None` en ambos = sin límite (idéntico a `run_program`).
pub fn run_program_con_limite(
    program: &CompiledProgram,
    fuel: Option<u64>,
    heap_cap: Option<usize>,
) -> Result<Value, RuntimeError> {
    let mut vm = Vm::new(program);
    if let Some(f) = fuel {
        vm.fuel = f;
    }
    if let Some(n) = heap_cap {
        vm.cur.heap.set_max_live(n);
    }
    let result = vm.run()?;
    vm.print_gc_stats_if_requested(); // M37.1
    Ok(to_value(&vm.cur.heap, &program.enums, &result))
}

/// Ejecuta un `Chunk` suelto (una expresión compilada). Lo envuelve como una
/// función sin parámetros ni locales. Se usa en los tests de expresiones.
pub fn run(chunk: &Chunk) -> Result<Value, RuntimeError> {
    let program = CompiledProgram {
        functions: vec![CompiledFn {
            name: "<expr>".to_string(),
            arity: 0,
            num_locals: 0,
            captured: Vec::new(),
            upvalues: Vec::new(),
            chunk: chunk.clone(),
        }],
        structs: Vec::new(),
        enums: Vec::new(),
        main: 0,
        externs: Vec::new(),
    };
    run_program(&program)
}

/// Un slot local. Normalmente guarda el valor directamente (`Plain`); si la variable
/// es capturada por una closure, vive **boxeada** en una celda del heap (`Boxed`),
/// referenciada por handle, para que la closure y el dueño la compartan (M4.2/M4.3).
enum Local {
    Plain(HeapValue),
    Boxed(Handle),
}

struct CallFrame {
    function: usize,
    ip: usize,
    locals: Vec<Local>,
    /// Upvalues de la closure en ejecución (handles a celdas); vacío si no lo es.
    upvalues: Vec<Handle>,
}

/// Una **fibra** (green thread, M12.1): el estado suspendido de una tarea — su pila de marcos y su pila de
/// operandos. M38.3b: la fibra EN CURSO vive en `Vm.cur` (un `Fiber`); las demás esperan en `Shared`.
#[derive(Default)]
struct Fiber {
    frames: Vec<CallFrame>,
    stack: Vec<HeapValue>,
    /// M38.1b-2: el **heap propio** de la fibra (aislamiento por actores, §46.2). `Vm.heap` es el de la
    /// fibra en curso; al conmutar se salva aquí y se restaura el de la siguiente. Un objeto de este heap
    /// solo lo alcanzan los marcos/pila de esta fibra (invariante: sin handles cruzados entre heaps).
    heap: Heap,
    is_main: bool,
    /// M12.3: la `Task` que esta fibra debe rellenar al terminar (`None` para `main`).
    task: Option<Handle>,
    /// M12.3: pila de scopes activos en esta fibra (structured concurrency); las tareas que lance
    /// mientras un scope esté activo quedan adscritas al más interno.
    scopes: Vec<ScopeFrame>,
}

/// Un scope activo (M12.3): la lista de tareas lanzadas mientras estuvo en la cima de la pila de la
/// fibra. Al cerrarse (`ScopeEnd`), el scope une a todas (las espera) y propaga el primer fallo.
struct ScopeFrame {
    children: Vec<Handle>,
}

/// Qué espera una fibra **bloqueada** (el handle por el que espera va en `Parked.on`):
/// - `Recv`: bloqueada en `recv` (canal vacío y abierto) → despierta cuando alguien envía o lo cierra.
/// - `Send(v)`: bloqueada en `send` (canal acotado y lleno) → despierta cuando un `recv` libera un hueco;
///   sostiene el valor `v` que aún no ha podido entregar (es una raíz del GC).
/// - `Join`: bloqueada en `join`/`ScopeEnd` esperando a una **tarea** (M12.3); al completarse la tarea se
///   la despierta y re-ejecuta el opcode (que rebobinó su `ip`).
/// - `Select`: bloqueada en `select` esperando a que CUALQUIERA de un conjunto de canales esté listo
///   (M12.4); `Parked.on` es el handle del **arreglo** de canales. Al despertar re-ejecuta el `select`.
enum Waiting {
    Recv,
    Send(HeapValue),
    Join,
    Select,
}

/// Una fibra **bloqueada**, con el handle por el que espera (`on`: un canal para Recv/Send, una tarea para
/// Join) y qué espera.
struct Parked {
    on: Handle,
    fiber: Fiber,
    waiting: Waiting,
}

/// M15.5/M17: una fibra aparcada esperando **E/S de red**, junto al descriptor (`fd`) del socket por el
/// que espera. El `fd` permite que el scheduler lo registre en el poller del SO (`kqueue`/`epoll`, M17)
/// y despierte **solo** las fibras de los sockets que quedaron listos, en vez de re-encolarlas todas.
struct IoParked {
    fd: i32,
    fiber: Fiber,
    /// `None` = aparcada esperando **lectura** (el caso de M15.5/M17). `Some` = esperando que el socket
    /// sea **escribible** para terminar una escritura parcial (cesión en `socket_write`, post-M19.4): el
    /// poller registra `fd` con interés de escritura y, al despertar, el scheduler drena lo que falta.
    pending_write: Option<PendingWrite>,
}

/// Una escritura que bloqueó a medias: el handle del socket y los octetos que aún faltan por enviar.
struct PendingWrite {
    handle: i64,
    remaining: Vec<u8>,
}

/// M38.3a: el estado del scheduler que N hilos compartirían (M38.3b: tras `Arc<Mutex<Shared>>`). Con los
/// heaps aislados por fibra (M38.1), es lo ÚNICO compartido: las colas de fibras listas/aparcadas y los
/// almacenes del host de canales/tareas. La ejecución de cada fibra (frames/stack/heap/…) es thread-local.
#[derive(Default)]
struct Shared {
    /// Fibras listas para ejecutar, en orden FIFO (scheduler determinista).
    ready: VecDeque<Fiber>,
    /// Fibras bloqueadas en `recv`/`send`/`join`, con el handle (canal o tarea) que esperan.
    parked: Vec<Parked>,
    /// M15.5/M17: fibras aparcadas esperando **E/S de red** (`accept`/`read` que dieron `WouldBlock`),
    /// cada una con el `fd` de su socket. El scheduler espera readiness real en el poller del SO (M17).
    io_parked: Vec<IoParked>,
    /// Canales `Channel<T>` (M12.1): sincronización COMPARTIDA entre actores, fuera del GC de las fibras
    /// (§46.2). Se referencian por id vía `HeapValue::Channel(id)`. El GC rootea sus valores en tránsito.
    channels: Vec<VmChannel>,
    /// Tareas `Task<T>` (M12.3): compartidas entre la fibra hija y quien la une, fuera del GC. Se
    /// referencian por id vía `HeapValue::Task(id)`. El GC rootea el valor de `Done`.
    tasks: Vec<VmTask>,
    /// M38.3b paso 3: nº de workers que **están ejecutando** una fibra ahora mismo (no ociosos). Invariante
    /// clave del scheduler M:N: un worker que toma una fibra de `ready` hace `running += 1`; cuando la aparca
    /// o termina, `running -= 1`. Un worker ocioso sólo puede declarar **deadlock** cuando `running == 0` (si
    /// alguien ejecuta, aún puede producir trabajo listo vía un canal). Con N=1 oscila 1↔0 trivialmente.
    running: usize,
    /// M38.3b paso 3: el **resultado del programa**, fijado UNA vez (semántica Go: cuando `main` retorna, todo
    /// el programa termina; o un error fatal / deadlock). Su presencia es la **señal de apagado**: los demás
    /// workers, al verla, se detienen. El orquestador lo lee tras unir a los hilos.
    outcome: Option<Result<HeapValue, RuntimeError>>,
}

struct Vm<'a> {
    program: &'a CompiledProgram,
    /// M38.3b: la **fibra en curso** (su ejecución: marcos, pila, heap, scopes, task, is_main). Es
    /// thread-local (en M:N cada worker tiene la suya); al conmutar se salva en `Shared` y se carga la
    /// siguiente. Antes eran campos sueltos del `Vm`; unificarlos en un `Fiber` hace la conmutación un
    /// simple swap y prepara el split exec/shared que exige el lock de M38.3.
    cur: Fiber,
    /// M38.3a: el **estado compartido del scheduler** (colas de fibras + almacenes de canales/tareas),
    /// agrupado para el paso a M:N (M38.3b lo envolverá en `Arc<Mutex<Shared>>` — con los heaps ya aislados
    /// en M38.1, es lo ÚNICO que N hilos compartirían; el GC no, cada heap es thread-local). Aquí sigue
    /// siendo propiedad directa del `Vm` (single-thread, sin lock) → comportamiento idéntico.
    /// M38.3b paso 2: el estado compartido del scheduler tras un `Mutex` (envuelto en `Arc` para que en el
    /// paso 3 lo compartan N hilos). Aquí (single-thread) el lock nunca tiene contención. Cada operación del
    /// scheduler bloquea UNA vez (`self.sched()`) y pasa el guard a la cadena de helpers (que ya toman
    /// `&mut Shared`), evitando la reentrancia (el `Mutex` no es reentrante).
    shared: Arc<Mutex<Shared>>,
    /// Opt.2: pool de `Vec<Local>` reutilizables. Cada llamada necesita un arreglo de locales; en vez de
    /// asignar/liberar uno por llamada (millones en recursión), reciclamos los de los marcos que retornan.
    /// NO es raíz del GC (sus contenidos son basura entre reciclar y reusar; `new_locals` los reconstruye).
    locals_pool: Vec<Vec<Local>>,
    /// M42.1: **fuel** — presupuesto de instrucciones restante (límite de recursos para embeber raylang
    /// como lenguaje de scripts confinado). Decrece una por instrucción; al llegar a 0 se aborta con un
    /// error limpio. `u64::MAX` = **sin límite** (el default): nunca se agota en la práctica, así que el
    /// coste es un decremento + comparación por instrucción, sin ramas.
    fuel: u64,
    /// M37.1: **instrumentación de pausas del GC**. Cuenta de recolecciones y la pausa máxima (ns) de una
    /// sola recolección stop-the-world. Sirve para MEDIR el objetivo de M37 (pausas acotadas, <1 ms) antes
    /// de decidir si el barrido/marcado incremental compensan. Coste: un `Instant` por recolección (raro).
    /// Se imprimen a stderr al terminar si `RAYLANG_GC_STATS` está en el entorno.
    gc_count: u64,
    gc_max_pause_ns: u128,
    gc_total_pause_ns: u128,
    /// M38.3b paso 3: señal **local al worker** de que debe detenerse porque el programa ya terminó (otro
    /// worker fijó `Shared.outcome`). El lazo la comprueba en su cima y retorna. Con N=1 nunca se activa.
    stop: bool,
}

impl<'a> Vm<'a> {
    fn new(program: &'a CompiledProgram) -> Self {
        Vm {
            program,
            cur: Fiber {
                frames: Vec::new(),
                stack: Vec::new(),
                heap: Heap::new(),
                is_main: true,
                task: None,
                scopes: Vec::new(),
            },
            shared: Arc::new(Mutex::new(Shared::default())),
            locals_pool: Vec::new(),
            fuel: u64::MAX, // sin límite por defecto
            gc_count: 0,
            gc_max_pause_ns: 0,
            gc_total_pause_ns: 0,
            stop: false,
        }
    }

    /// M38.3b paso 3: un **contexto de ejecución worker** para el scheduler M:N. Comparte `program`
    /// (inmutable) y `shared` (`Arc<Mutex<Shared>>`, el estado del scheduler) con los demás workers, pero su
    /// estado de ejecución (`cur`/`locals_pool`/`fuel`/stats-GC) es **thread-local**: cada worker recolecta
    /// sólo su heap y no comparte marcos/pila. Arranca sin fibra en curso (`cur` vacío); la toma de `ready`.
    fn worker(program: &'a CompiledProgram, shared: Arc<Mutex<Shared>>) -> Self {
        Vm {
            program,
            cur: Fiber::default(), // sin fibra aún; `poll_next` cargará la primera de `ready`
            shared,
            locals_pool: Vec::new(),
            fuel: u64::MAX,
            gc_count: 0,
            gc_max_pause_ns: 0,
            gc_total_pause_ns: 0,
            stop: false,
        }
    }

    /// M38.3b paso 2: bloquea el estado compartido del scheduler. Una operación del scheduler llama esto
    /// UNA vez y usa el guard (`&mut *sh`) para toda su cadena de helpers → sin reentrancia ni deadlock.
    fn sched(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado") // ice-ok: invariante
    }

    /// M38.3b paso 3: **orquestador** del scheduler M:N. Arma la fibra de `main`, la encola en `ready` y
    /// lanza `num_workers(program)` hilos worker (o corre single-thread si es 1 → scheduler determinista).
    /// Cada worker ejecuta fibras de la cola compartida hasta que `main` retorna (semántica Go) o hay un
    /// error fatal; el primero en terminar fija `Shared.outcome`, que los demás ven y se detienen. El
    /// resultado del programa es ese `outcome`.
    fn run(&mut self) -> Result<HeapValue, RuntimeError> {
        // Marco inicial: main, con su arreglo de locales (sin argumentos). Se encola como una fibra más en
        // `ready`; un worker la tomará (con N=1, este mismo Vm). La fibra de main **reutiliza el heap de
        // `self.cur`** (no un `Heap::new()`): `run_program_con_limite` fija ahí el tope de heap (M42.2) antes
        // de `run()`, y hay que conservarlo.
        let main = self.program.main;
        let locals = self.new_locals(main);
        let mut main_fiber = std::mem::take(&mut self.cur); // is_main: true, heap con el tope preconfigurado
        main_fiber.frames.push(CallFrame { function: main, ip: 0, locals, upvalues: Vec::new() });
        self.sched().ready.push_back(main_fiber);

        let n = num_workers(self.program);
        if n == 1 {
            // Single-thread determinista (default): este Vm ES el único worker. `poll_next` toma main de
            // `ready`; comportamiento idéntico a antes de M38.3b (con N=1 el `running` oscila 1↔0 y nunca se
            // espera en el busy-poll).
            self.run_worker();
        } else {
            // M:N: N hilos worker comparten `program` (inmutable, vía `ProgRef`) y `shared`. Cada uno con su
            // estado de ejecución (heap/marcos) thread-local. `thread::scope` garantiza que se unen antes de
            // salir → los préstamos de `self`/`program` viven lo suficiente (sin `'static`).
            let prog = ProgRef(self.program);
            let shared = Arc::clone(&self.shared);
            std::thread::scope(|s| {
                for _ in 0..n {
                    let shared = Arc::clone(&shared);
                    // Pila grande por worker (paridad con `with_big_stack`): la VM es iterativa, pero
                    // `format_value`/`collect`/`transfer_value` recurren sobre la pila de Rust en estructuras
                    // profundas. 256 MiB como el hilo principal del binario.
                    std::thread::Builder::new()
                        .stack_size(256 * 1024 * 1024)
                        .spawn_scoped(s, move || {
                            // Rebind del `ProgRef` ENTERO (no `prog.0`): fuerza la captura de la struct
                            // `Send`, no del `&CompiledProgram` disjunto (captura disjunta de la ed. 2021).
                            let prog = prog;
                            let mut w = Vm::worker(prog.0, shared);
                            w.run_worker();
                        })
                        .expect("no se pudo lanzar el hilo worker"); // ice-ok: fallo del SO al crear hilo
                }
            });
        }
        // Todos los workers unidos: `outcome` debe estar fijado (main terminó o hubo un fatal). Si por algún
        // camino quedó vacío, el programa no produjo nada → unit.
        self.sched().outcome.take().unwrap_or(Ok(HeapValue::Unit))
    }

    /// M38.3b paso 3: el bucle de un **worker**. Toma su primera fibra de `ready` (`poll_next`), la ejecuta
    /// (`run_loop`, que entre fibras vuelve a `poll_next`) hasta que `main` termina / hay un fatal / otro
    /// worker apagó el programa. Fija `Shared.outcome` con lo que ESTE worker determinó (si aún no lo fijó
    /// otro). No devuelve nada: el resultado viaja por `outcome`.
    fn run_worker(&mut self) {
        match self.poll_next(0, 0) {
            Ok(true) => {}          // fibra cargada en `self.cur`
            Ok(false) => return,    // el programa ya terminó (outcome fijado por otro)
            Err(e) => {
                // Sin fibras ejecutables desde el arranque (no debería con main en cola): registra el fatal.
                let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                if sh.outcome.is_none() { sh.outcome = Some(Err(e)); }
                return;
            }
        }
        let res = self.run_loop();
        // Si nos detuvimos porque otro worker ya fijó el outcome (`stop`), no lo pisamos.
        if !self.stop {
            let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
            if sh.outcome.is_none() { sh.outcome = Some(res); }
        }
    }

    /// M38.3b paso 3: carga la siguiente fibra lista en `self.cur`. Devuelve `Ok(true)` si cargó una,
    /// `Ok(false)` si el programa ya terminó (apagado), `Err` si es un deadlock/no-ejecutable fatal. El
    /// llamador YA aparcó/descartó su fibra y decrementó `running` (este worker está ocioso al entrar).
    ///
    /// Multicore (N>1): si no hay fibra lista pero **otro worker ejecuta** (`running > 0`), puede aún
    /// producir trabajo (un `send`) → espera con un *busy-poll* (`SPIN_SLEEP_US`) y reintenta. Sólo declara
    /// deadlock cuando `running == 0` (nadie ejecuta) y hay fibras aparcadas. Con N=1, `running` es 0 al
    /// entrar → nunca se espera; el camino es idéntico al viejo `schedule_next`.
    fn poll_next(&mut self, line: usize, col: usize) -> Result<bool, RuntimeError> {
        loop {
            let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
            if sh.outcome.is_some() {
                return Ok(false); // otro worker apagó el programa
            }
            if let Some(next) = sh.ready.pop_front() {
                sh.running += 1;
                drop(sh);
                self.cur = next;
                return Ok(true);
            }
            // Nadie listo.
            if sh.running == 0 {
                // Nadie ejecuta → nadie puede producir trabajo listo. Si hay E/S pendiente, espera readiness
                // (un solo worker llega aquí, por `running == 0`); si no, es deadlock o fin.
                if !sh.io_parked.is_empty() {
                    Self::io_wait(&mut sh);
                    continue; // io_wait dejó fibras en `ready`; reintenta el pop
                }
                let msg = if !sh.parked.is_empty() {
                    "deadlock: todas las fibras están bloqueadas esperando un canal o una tarea"
                } else {
                    "no hay fibras ejecutables"
                };
                let e = runtime_error(line, col, msg);
                sh.outcome = Some(Err(e.clone())); // apaga a los demás workers
                return Err(e);
            }
            // Otro worker ejecuta y podría desbloquearnos trabajo: espera un poco y reintenta.
            drop(sh);
            std::thread::sleep(std::time::Duration::from_micros(SPIN_SLEEP_US));
        }
    }

    /// M38.3b paso 3: el bucle de ejecución de fibras (antes el cuerpo de `run`). Ejecuta la fibra en
    /// `self.cur`; al bloquear/terminar una fibra `poll_next` carga la siguiente y el bucle sigue. Retorna
    /// cuando `main` termina (su valor), hay un fatal (Err) o `self.stop` (otro worker apagó → Unit ignorado).
    fn run_loop(&mut self) -> Result<HeapValue, RuntimeError> {
        // El programa es inmutable y vive tanto como la VM; copiamos su referencia a un local (Opt.1). Así
        // el `match` de cada instrucción la toma **prestada** del programa (no de `self`), y el cuerpo puede
        // mutar `self` sin que el préstamo choque — eliminando el clon de la instrucción por iteración.
        let program = self.program;

        loop {
            // M38.3b: si otro worker apagó el programa, deténte (el valor Unit se ignora: `outcome` ya está).
            if self.stop {
                return Ok(HeapValue::Unit);
            }
            // --- Punto seguro del GC ---
            if self.cur.heap.should_collect() {
                self.collect();
                // M42.2: tope de heap. Si tras recolectar siguen vivos más objetos de los permitidos,
                // el programa realmente necesita más memoria de la presupuestada → aborta limpio.
                if self.cur.heap.over_cap() {
                    let fi = self.cur.frames.len() - 1;
                    let func = self.cur.frames[fi].function;
                    let ip = self.cur.frames[fi].ip;
                    let (l, c) = program.functions[func].chunk.lines.get(ip).copied().unwrap_or((0, 0));
                    return Err(runtime_error(l, c, "límite de memoria agotado (tope de heap)"));
                }
            }

            let fi = self.cur.frames.len() - 1;
            let func = self.cur.frames[fi].function;
            let ip = self.cur.frames[fi].ip;

            // M42.1: fuel. Sin límite (`u64::MAX`) nunca dispara; con límite, aborta al agotarse. La
            // posición es la de la instrucción en curso (para el diagnóstico).
            if self.fuel == 0 {
                let (l, c) = program.functions[func].chunk.lines.get(ip).copied().unwrap_or((0, 0));
                return Err(runtime_error(l, c, "límite de instrucciones agotado (fuel)"));
            }
            self.fuel -= 1;

            // Robustez: si se acabó el chunk sin Return (no debería), retorna unit.
            if ip >= program.functions[func].chunk.code.len() {
                if let Some(frame) = self.cur.frames.pop() {
                    self.recycle_locals(frame.locals); // Opt.2
                }
                if self.cur.frames.is_empty() {
                    match self.on_fiber_done(HeapValue::Unit)? {
                        Some(v) => return Ok(v),
                        None => continue, // era una fibra spawn: el scheduler ya cargó la siguiente (o `stop`)
                    }
                }
                self.cur.stack.push(HeapValue::Unit);
                continue;
            }

            // La instrucción se toma PRESTADA del programa (Opt.1: sin clonar). `instr` vive lo que
            // `program` (toda la VM), así que no estorba a las mutaciones de `self` del cuerpo.
            let instr = &program.functions[func].chunk.code[ip];
            // Opt.7: la posición `(línea, col)` NO se lee por instrucción —el camino caliente
            // (locales/constantes/aritmética/saltos) nunca la usa, solo los sitios de error o de cesión—.
            // Se resuelve **bajo demanda** con `pos!()`, leyendo `lines[ip]` solo donde hace falta.
            macro_rules! pos { () => {{ let p = program.functions[func].chunk.lines[ip]; (p.0, p.1) }} }
            self.cur.frames[fi].ip = ip + 1; // avance por defecto; los saltos lo cambian

            // M12.3: ejecutamos la instrucción dentro de un cierre que devuelve `Ok(Some(v))` (fin del
            // programa), `Ok(None)` (seguir) o `Err` (fallo). Así el bucle puede CAPTURAR el error de una
            // fibra hija (propagación structured concurrency) en vez de abortar siempre.
            let outcome: Result<Option<HeapValue>, RuntimeError> = (|| {
            match instr {
                OpCode::Constant(idx) => {
                    let v = const_to_heap(&self.program.functions[func].chunk.constants[*idx]);
                    self.push(v);
                }
                OpCode::True => self.push(HeapValue::Bool(true)),
                OpCode::False => self.push(HeapValue::Bool(false)),
                OpCode::Unit => self.push(HeapValue::Unit),
                OpCode::Pop => {
                    self.pop();
                }

                OpCode::Negate => {
                    let v = self.pop();
                    self.push(match v {
                        // -i64::MIN desborda (M34, SPEC §8): error, como la aritmética binaria.
                        HeapValue::Int(n) => HeapValue::Int(n.checked_neg().ok_or_else(|| {
                            let (l, c) = pos!();
                            runtime_error(l, c, "desbordamiento aritmético en int")
                        })?),
                        HeapValue::Float(x) => HeapValue::Float(-x),
                        _ => unreachable!("el checker garantiza un número"),
                    });
                }
                OpCode::Not => {
                    let v = self.pop();
                    self.push(match v {
                        HeapValue::Bool(b) => HeapValue::Bool(!b),
                        _ => unreachable!("el checker garantiza un bool"),
                    });
                }
                OpCode::BitNot => {
                    let v = self.pop();
                    self.push(match v {
                        HeapValue::Int(n) => HeapValue::Int(!n), // M19.3a: complemento a uno
                        HeapValue::UInt(n, w) => uint_heap(!n, w), // M28.3: NOT sobre uint (enmascarado)
                        _ => unreachable!("el checker garantiza un int"),
                    });
                }

                bin @ (OpCode::Add
                | OpCode::Sub
                | OpCode::Mul
                | OpCode::Div
                | OpCode::Rem
                | OpCode::Equal
                | OpCode::NotEqual
                | OpCode::Less
                | OpCode::LessEqual
                | OpCode::Greater
                | OpCode::GreaterEqual
                | OpCode::BitAnd
                | OpCode::BitOr
                | OpCode::BitXor
                | OpCode::Shl
                | OpCode::Shr) => {
                    let right = self.pop();
                    let left = self.pop();
                    // Opt.4: fast-path entero. En la inmensa mayoría de programas (bucles, recursión
                    // aritmética) ambos operandos son `Int`; resolverlo aquí evita el doble match y la
                    // llamada a `apply_binary` (que rematchea opcode + ~30 combinaciones de tipos).
                    // Medido (mejor de 5, release): fib(35) -5%, bucle aritmético 10M -6%. Semántica
                    // idéntica al camino general — incluido el desbordamiento como error (M34, SPEC §8).
                    // (Opt.1/Opt.2 ya aplicadas; Opt.3 = `Rc<str>`, evaluada y descartada → por eso esto es Opt.4.)
                    if let (HeapValue::Int(a), HeapValue::Int(b)) = (&left, &right) {
                        let (a, b) = (*a, *b);
                        let ovf = || {
                            let (l, c) = pos!();
                            runtime_error(l, c, "desbordamiento aritmético en int")
                        };
                        let r = match bin {
                            OpCode::Add => HeapValue::Int(a.checked_add(b).ok_or_else(ovf)?),
                            OpCode::Sub => HeapValue::Int(a.checked_sub(b).ok_or_else(ovf)?),
                            OpCode::Mul => HeapValue::Int(a.checked_mul(b).ok_or_else(ovf)?),
                            OpCode::Div => {
                                if b == 0 {
                                    return Err(runtime_error(pos!().0, pos!().1, "división entera por cero"));
                                }
                                HeapValue::Int(a.checked_div(b).ok_or_else(ovf)?)
                            }
                            OpCode::Rem => {
                                if b == 0 {
                                    return Err(runtime_error(pos!().0, pos!().1, "módulo por cero"));
                                }
                                HeapValue::Int(a.checked_rem(b).ok_or_else(ovf)?)
                            }
                            OpCode::Less => HeapValue::Bool(a < b),
                            OpCode::LessEqual => HeapValue::Bool(a <= b),
                            OpCode::Greater => HeapValue::Bool(a > b),
                            OpCode::GreaterEqual => HeapValue::Bool(a >= b),
                            OpCode::Equal => HeapValue::Bool(a == b),
                            OpCode::NotEqual => HeapValue::Bool(a != b),
                            // Bit a bit (M19.3a): mismos `wrapping_*` que el intérprete.
                            OpCode::BitAnd => HeapValue::Int(a & b),
                            OpCode::BitOr => HeapValue::Int(a | b),
                            OpCode::BitXor => HeapValue::Int(a ^ b),
                            OpCode::Shl => HeapValue::Int(a.wrapping_shl(b as u32)),
                            OpCode::Shr => HeapValue::Int(a.wrapping_shr(b as u32)),
                            _ => unreachable!("el grupo `bin` solo trae operadores binarios"),
                        };
                        self.push(r);
                    }
                    // M11.7b: `+` sobre dos arreglos (objetos del heap) los concatena en uno nuevo.
                    // El checker garantiza que dos `Obj` con `Add` son arreglos (strings son inline).
                    else if let (OpCode::Add, HeapValue::Obj(l), HeapValue::Obj(r)) = (bin, &left, &right) {
                        let (l, r) = (*l, *r);
                        let mut elems = self.as_array(l).clone();
                        elems.extend(self.as_array(r).iter().cloned());
                        let h = self.cur.heap.allocate(Obj::Array(elems));
                        self.push(HeapValue::Obj(h));
                    } else {
                        let result = self.apply_binary(bin, left, right, pos!().0, pos!().1)?;
                        self.push(result);
                    }
                }

                OpCode::Jump(target) => {
                    self.cur.frames[fi].ip = *target;
                }
                OpCode::JumpIfFalse(target) => {
                    if matches!(self.peek(), HeapValue::Bool(false)) {
                        self.cur.frames[fi].ip = *target;
                    }
                }

                OpCode::GetLocal(slot) => {
                    let v = self.get_local(fi, *slot);
                    self.push(v);
                }
                // M36.1: superinstrucciones — dos empujes en una iteración del lazo.
                OpCode::GetLocalLocal(s, t) => {
                    let a = self.get_local(fi, *s);
                    let b = self.get_local(fi, *t);
                    self.push(a);
                    self.push(b);
                }
                OpCode::GetLocalConst(s, c) => {
                    let a = self.get_local(fi, *s);
                    let b = const_to_heap(&self.program.functions[func].chunk.constants[*c]);
                    self.push(a);
                    self.push(b);
                }
                OpCode::SetLocal(slot) => {
                    let v = self.pop();
                    self.set_local(fi, *slot, v);
                }
                OpCode::InitLocal(slot) => {
                    // Declaración: si el slot está boxeado, estrena celda (shadowing
                    // seguro); si no, guarda el valor directamente.
                    let v = self.pop();
                    let boxed = self.program.functions[func].captured.get(*slot).copied().unwrap_or(false);
                    self.cur.frames[fi].locals[*slot] = if boxed {
                        Local::Boxed(self.cur.heap.allocate(Obj::Cell(v)))
                    } else {
                        Local::Plain(v)
                    };
                }
                OpCode::GetUpvalue(i) => {
                    let h = self.cur.frames[fi].upvalues[*i];
                    let v = self.cell_get(h);
                    self.push(v);
                }
                OpCode::SetUpvalue(i) => {
                    let v = self.pop();
                    let h = self.cur.frames[fi].upvalues[*i];
                    self.cell_set(h, v);
                }

                OpCode::Print => {
                    let v = self.pop();
                    crate::host_print(&format_value(&self.cur.heap, &self.program.enums, &v));
                    self.push(HeapValue::Unit);
                }

                // --- FFI (M41): llamada a una función C ---
                OpCode::CallExtern(idx, argc) => {
                    let desc = &program.externs[*idx];
                    let mut fargs = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        fargs.push(self.pop());
                    }
                    fargs.reverse(); // se sacaron en orden inverso
                    // Los `HeapValue` retenidos en `fargs` viven durante la llamada; los `FfiVal` de
                    // string/bytes toman prestado su buffer inline (M41.2).
                    let mut cargs = Vec::with_capacity(*argc);
                    for v in &fargs {
                        cargs.push(match v {
                            HeapValue::Int(n) => crate::ffi::FfiVal::Int(*n),
                            HeapValue::UInt(v, _) => crate::ffi::FfiVal::Int(*v as i64), // M41.4: u64 → 64 bits
                            HeapValue::Float(f) => crate::ffi::FfiVal::Float(*f),
                            HeapValue::Bool(b) => crate::ffi::FfiVal::Int(*b as i64),
                            HeapValue::Str(s) => crate::ffi::FfiVal::Str(s.as_str()),
                            HeapValue::Bytes(b) => crate::ffi::FfiVal::Bytes(b.as_slice()),
                            HeapValue::Ptr(p) => crate::ffi::FfiVal::Int(*p), // M41.4b
                            _ => return Err(runtime_error(pos!().0, pos!().1,
                                "argumento no marshalable en la frontera FFI")),
                        });
                    }
                    let r = crate::ffi::call(desc, &cargs)
                        .map_err(|m| runtime_error(pos!().0, pos!().1, &m))?;
                    let val = match r {
                        crate::ffi::FfiRet::Int(n) if desc.ret_kind == crate::ffi::CKind::Bool => HeapValue::Bool(n != 0),
                        crate::ffi::FfiRet::Int(n) if desc.ret_kind == crate::ffi::CKind::U64 => HeapValue::UInt(n as u64, 64),
                        crate::ffi::FfiRet::Int(n) => HeapValue::Int(n),
                        crate::ffi::FfiRet::Float(f) => HeapValue::Float(f),
                        crate::ffi::FfiRet::Unit => HeapValue::Unit,
                        // M41.3: char* → Option<bytes>/Option<string> (construido como el enum del prelude).
                        crate::ffi::FfiRet::OptBytes(opt) => {
                            let (variant, payload): (&str, Vec<HeapValue>) = match opt {
                                None => ("None", vec![]),
                                Some(bytes) => {
                                    let inner = if desc.ret_kind == crate::ffi::CKind::OptStr {
                                        match String::from_utf8(bytes) {
                                            Ok(s) => HeapValue::Str(s),
                                            Err(_) => return Err(runtime_error(pos!().0, pos!().1,
                                                "la función C devolvió bytes que no son UTF-8 válido (declara Option<bytes> para recibirlos crudos)")),
                                        }
                                    } else {
                                        HeapValue::Bytes(bytes)
                                    };
                                    ("Some", vec![inner])
                                }
                            };
                            let (eid, tag) = option_variant(&program.enums, variant).ok_or_else(||
                                runtime_error(pos!().0, pos!().1, "el enum Option del prelude no está disponible para el retorno FFI"))?;
                            let h = self.cur.heap.allocate(Obj::Enum(VmEnum { enum_id: eid, tag, payload }));
                            HeapValue::Obj(h)
                        }
                        // M41.4b: puntero opaco.
                        crate::ffi::FfiRet::Ptr(p) => HeapValue::Ptr(p),
                        crate::ffi::FfiRet::OptPtr(opt) => {
                            let (variant, payload): (&str, Vec<HeapValue>) = match opt {
                                None => ("None", vec![]),
                                Some(p) => ("Some", vec![HeapValue::Ptr(p)]),
                            };
                            let (eid, tag) = option_variant(&program.enums, variant).ok_or_else(||
                                runtime_error(pos!().0, pos!().1, "el enum Option del prelude no está disponible para el retorno FFI"))?;
                            let h = self.cur.heap.allocate(Obj::Enum(VmEnum { enum_id: eid, tag, payload }));
                            HeapValue::Obj(h)
                        }
                    };
                    self.push(val);
                }

                // --- Arreglos (M3) ---
                OpCode::MakeArray(n) => {
                    let mut elems = Vec::with_capacity(*n);
                    for _ in 0..*n {
                        elems.push(self.pop());
                    }
                    elems.reverse(); // se sacaron en orden inverso
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Index => {
                    let i = self.pop_int();
                    match self.pop() {
                        HeapValue::Obj(h) => {
                            let idx = {
                                let arr = self.as_array(h);
                                bounds_check(i, arr.len(), pos!().0, pos!().1)?
                            };
                            let v = self.as_array(h)[idx].clone();
                            self.push(v);
                        }
                        // M11.4c-2: indexar un string → el carácter en esa posición.
                        HeapValue::Str(s) => {
                            let chars: Vec<char> = s.chars().collect();
                            let idx = bounds_check(i, chars.len(), pos!().0, pos!().1)?;
                            self.push(HeapValue::Char(chars[idx]));
                        }
                        // M16.1a: indexar bytes → el octeto como int.
                        HeapValue::Bytes(b) => {
                            let idx = bounds_check(i, b.len(), pos!().0, pos!().1)?;
                            self.push(HeapValue::Int(b[idx] as i64));
                        }
                        _ => unreachable!("el checker garantiza un arreglo, string o bytes"),
                    }
                }
                OpCode::SetIndex => {
                    let v = self.pop();
                    let i = self.pop_int();
                    let h = self.pop_obj();
                    let idx = bounds_check(i, self.as_array(h).len(), pos!().0, pos!().1)?;
                    self.as_array_mut(h)[idx] = v;
                }
                OpCode::Len => {
                    // M11.1a: len de arreglo o string; M13.1: len de Map (nº de entradas).
                    let len = match self.pop() {
                        HeapValue::Str(s) => s.chars().count() as i64,
                        // M16.1a: len de bytes = nº de octetos.
                        HeapValue::Bytes(b) => b.len() as i64,
                        HeapValue::Obj(h) => match self.cur.heap.get(h) {
                            Obj::Array(v) => v.len() as i64,
                            Obj::Map(m) => m.len() as i64,
                            _ => unreachable!("el checker garantiza un arreglo o Map"),
                        },
                        _ => unreachable!("el checker garantiza un arreglo, string, Map o bytes"),
                    };
                    self.push(HeapValue::Int(len));
                }
                // M27.4: conversión numérica `as` (según el valor en runtime + destino).
                OpCode::Cast(target) => {
                    let v = self.pop();
                    let out = match (&v, target) {
                        (HeapValue::Int(n), CastTarget::Float) => HeapValue::Float(*n as f64),
                        (HeapValue::Float(f), CastTarget::Int) => HeapValue::Int(*f as i64),
                        (HeapValue::Char(c), CastTarget::Int) => HeapValue::Int(*c as i64),
                        (HeapValue::Int(n), CastTarget::Char) => {
                            match u32::try_from(*n).ok().and_then(char::from_u32) {
                                Some(c) => HeapValue::Char(c),
                                None => {
                                    return Err(runtime_error(pos!().0, pos!().1,
                                        &format!("{} no es un carácter Unicode válido para 'as char'", n)));
                                }
                            }
                        }
                        // M28.3: conversiones de/hacia enteros sin signo con tamaño (enmascaran al ancho).
                        (HeapValue::Int(n), CastTarget::UInt(w)) => uint_heap(*n as u64, *w),
                        (HeapValue::UInt(n, _), CastTarget::Int) => HeapValue::Int(*n as i64),
                        (HeapValue::UInt(n, _), CastTarget::UInt(w)) => uint_heap(*n, *w),
                        (HeapValue::UInt(n, _), CastTarget::Float) => HeapValue::Float(*n as f64),
                        (HeapValue::Float(f), CastTarget::UInt(w)) => uint_heap(*f as i64 as u64, *w),
                        (HeapValue::Char(c), CastTarget::UInt(w)) => uint_heap(*c as u64, *w),
                        _ => v, // identidad
                    };
                    self.push(out);
                }
                // --- Mapas Map<K,V> (M13.1) ---
                OpCode::MapNew => {
                    let h = self.cur.heap.allocate(Obj::Map(HashMap::new()));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::MapInsert => {
                    let v = self.pop();
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    match self.cur.heap.get_mut(h) {
                        Obj::Map(m) => { m.insert(k, v); }
                        _ => unreachable!("el checker garantiza un Map"),
                    }
                    self.push(HeapValue::Unit);
                }
                OpCode::MapContainsKey => {
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let presente = match self.cur.heap.get(h) {
                        Obj::Map(m) => m.contains_key(&k),
                        _ => unreachable!("el checker garantiza un Map"),
                    };
                    self.push(HeapValue::Bool(presente));
                }
                OpCode::MapGet => {
                    // Primitivo: [] o [v]; el prelude lo envuelve en Option<V>.
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let elems = match self.cur.heap.get(h) {
                        Obj::Map(m) => match m.get(&k) {
                            Some(v) => vec![v.clone()],
                            None => vec![],
                        },
                        _ => unreachable!("el checker garantiza un Map"),
                    };
                    let arr = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::MapRemove => {
                    // M13.1b: quita la clave; [] o [v]. El prelude → Option<V>.
                    let k = heap_to_key(&self.pop());
                    let h = self.pop_obj();
                    let elems = match self.cur.heap.get_mut(h) {
                        Obj::Map(m) => match m.remove(&k) {
                            Some(v) => vec![v],
                            None => vec![],
                        },
                        _ => unreachable!("el checker garantiza un Map"),
                    };
                    let arr = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::MapKeys => {
                    // M13.1b: claves ordenadas (determinista).
                    let h = self.pop_obj();
                    let mut ks: Vec<MapKey> = match self.cur.heap.get(h) {
                        Obj::Map(m) => m.keys().cloned().collect(),
                        _ => unreachable!("el checker garantiza un Map"),
                    };
                    ks.sort();
                    let elems: Vec<HeapValue> = ks.iter().map(key_to_heap).collect();
                    let arr = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::MapValues => {
                    // M13.1b: valores en orden de clave ordenada (casa con keys).
                    let h = self.pop_obj();
                    let elems: Vec<HeapValue> = match self.cur.heap.get(h) {
                        Obj::Map(m) => {
                            let mut pares: Vec<(&MapKey, &HeapValue)> = m.iter().collect();
                            pares.sort_by(|a, b| a.0.cmp(b.0));
                            pares.iter().map(|(_, v)| (*v).clone()).collect()
                        }
                        _ => unreachable!("el checker garantiza un Map"),
                    };
                    let arr = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(arr));
                }
                OpCode::Push => {
                    let v = self.pop();
                    let h = self.pop_obj();
                    self.as_array_mut(h).push(v);
                    self.push(HeapValue::Unit);
                }

                // --- Concurrencia: CSP sobre la VM (M12.1) ---
                OpCode::Spawn => {
                    // Saca el valor-función; crea una fibra nueva que lo ejecuta (0 args), le asigna una
                    // Task<T> (M12.3) y la encola. Si hay un scope activo, adscribe la tarea a él.
                    let (fn_idx, upvalues) = match self.pop() {
                        HeapValue::Function(i) => (i, Vec::new()),
                        HeapValue::Obj(h) => match self.cur.heap.get(h) {
                            Obj::Closure(c) => (c.index, c.upvalues.clone()),
                            _ => unreachable!("el checker garantiza una función"),
                        },
                        _ => unreachable!("el checker garantiza una función"),
                    };
                    // M38.1b-2: la fibra hija tiene su PROPIO heap; las capturas (upvalues) del closure
                    // viven en el heap del spawner → se transfieren al heap nuevo (aislamiento por actores).
                    // Esto es thread-local (usa `self.cur.heap`/`self.locals_pool`) → sin lock.
                    let mut new_heap = Heap::new();
                    let mut remap = HashMap::new();
                    let upvalues: Vec<Handle> = upvalues.iter()
                        .map(|&up| transfer_obj(&self.cur.heap, &mut new_heap, up, &mut remap))
                        .collect();
                    let locals = self.new_locals(fn_idx);
                    let frame = CallFrame { function: fn_idx, ip: 0, locals, upvalues };
                    // M38.3b paso 3: alojar la Task y encolar la fibra hija en UN solo lock (bajo M:N real,
                    // dos `self.sched()` —len y push— tendrían un TOCTOU en el id de la tarea).
                    let task = {
                        let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                        let task = sh.tasks.len();
                        sh.tasks.push(VmTask { state: TaskState::Pending, heap: Heap::new() });
                        sh.ready.push_back(Fiber {
                            frames: vec![frame], stack: Vec::new(), heap: new_heap, is_main: false,
                            task: Some(task), scopes: Vec::new(),
                        });
                        task
                    };
                    if let Some(scope) = self.cur.scopes.last_mut() {
                        scope.children.push(task); // M12.3: adscribe la tarea al scope activo
                    }
                    self.push(HeapValue::Task(task)); // el Task<T> es el resultado de spawn
                }
                OpCode::ChannelNew => {
                    // channel() sin argumentos → canal NO acotado (cap = None). M38.1b: en el host.
                    // M38.3b paso 3: id + push en UN solo lock (TOCTOU bajo M:N real).
                    let id = {
                        let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                        let id = sh.channels.len();
                        sh.channels.push(VmChannel { queue: VecDeque::new(), closed: false, cap: None, heap: Heap::new() });
                        id
                    };
                    self.push(HeapValue::Channel(id));
                }
                OpCode::ChannelNewBounded => {
                    // channel(n) → canal acotado a la capacidad n ≥ 0 (n = 0 rendezvous), M12.2.
                    let n = match self.pop() {
                        HeapValue::Int(n) => n,
                        _ => unreachable!("el checker garantiza un int"),
                    };
                    if n < 0 {
                        return Err(runtime_error(pos!().0, pos!().1, "la capacidad de un canal no puede ser negativa"));
                    }
                    let id = {
                        let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                        let id = sh.channels.len();
                        sh.channels.push(VmChannel { queue: VecDeque::new(), closed: false, cap: Some(n as usize), heap: Heap::new() });
                        id
                    };
                    self.push(HeapValue::Channel(id));
                }
                OpCode::ChanSend => {
                    let v = self.pop();
                    let h = self.pop_channel();
                    // M38.3b paso 2: un ÚNICO lock por handler (lock-once). Se bloquea `self.shared`
                    // DIRECTAMENTE (no vía `self.sched()`, que tomaría `&self` entero) para que el guard
                    // preste solo el campo `self.shared`; así `self.cur.*` (campo disjunto) sigue accesible
                    // bajo el guard sostenido.
                    let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                    let c = &sh.channels[h];
                    let (closed, len, cap) = (c.closed, c.queue.len(), c.cap);
                    if closed {
                        return Err(runtime_error(pos!().0, pos!().1, "send sobre un canal cerrado"));
                    }
                    // (1) ¿Hay un receptor bloqueado en este canal? Entrégaselo directo (rendezvous) y
                    // despiértalo (el primero, FIFO → determinista).
                    if let Some(pos) = sh.parked.iter().position(
                        |p| p.on == h && matches!(p.waiting, Waiting::Recv))
                    {
                        let parked = sh.parked.remove(pos);
                        Self::wake_recv(&self.cur, &mut sh, parked.fiber, vec![v]);
                        self.cur.stack.push(HeapValue::Unit);
                    } else if cap.is_none() || len < cap.unwrap() {
                        // (2) Hay hueco (no acotado, o len < cap) → encola y sigue. M38.1b-2: el valor se
                        // transfiere del heap de la fibra al heap del canal (en tránsito).
                        let mut ch_heap = std::mem::take(&mut sh.channels[h].heap);
                        let v2 = transfer_value(&self.cur.heap, &mut ch_heap, &v, &mut HashMap::new());
                        sh.channels[h].heap = ch_heap;
                        sh.channels[h].queue.push_back(v2);
                        Self::wake_select_waiters(&mut sh, h); // M12.4: el canal ya tiene valor → listo para un select
                        self.cur.stack.push(HeapValue::Unit);
                    } else {
                        // (3) Cola llena (acotado) → BLOQUEAR al emisor (backpressure, M12.2). Guarda la
                        // fibra con el valor pendiente; al despertarla, `wake_sender` le deja unit (el
                        // resultado de `send`) en la pila y continúa tras el ChanSend.
                        let fiber = Self::take_current_fiber(&mut self.cur);
                        sh.parked.push(Parked { on: h, fiber, waiting: Waiting::Send(v) });
                        // M12.4: un emisor bloqueado vuelve al canal "listo" para un select (un recv lo
                        // tomaría); despierta a los selectores que lo esperan.
                        Self::wake_select_waiters(&mut sh, h);
                        sh.running -= 1; // este emisor se bloqueó → worker ocioso
                        drop(sh);
                        let (l, c2) = pos!();
                        if !self.poll_next(l, c2)? { self.stop = true; }
                    }
                }
                OpCode::ChanRecv => {
                    let h = self.pop_channel();
                    // M38.3b paso 2: lock-once (ver ChanSend).
                    let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                    // (1) ¿Valor en la cola? Sácalo; al liberar un hueco, si hay un emisor bloqueado en este
                    // canal, su valor entra a la cola (ya hay sitio) y se le despierta.
                    let from_queue = sh.channels[h].queue.pop_front();
                    if let Some(v) = from_queue {
                        // M38.1b-2: el valor viene del heap del canal → se transfiere al heap del receptor.
                        // Si la cola queda vacía, el heap del canal se limpia (nadie referencia sus objetos).
                        let ch_heap = std::mem::take(&mut sh.channels[h].heap);
                        let v2 = transfer_value(&ch_heap, &mut self.cur.heap, &v, &mut HashMap::new());
                        if !sh.channels[h].queue.is_empty() {
                            sh.channels[h].heap = ch_heap; // aún hay valores en tránsito → conserva el heap
                        } // si no, `ch_heap` se descarta (limpieza)
                        Self::wake_blocked_sender(&mut sh, h);
                        let arr = self.cur.heap.allocate(Obj::Array(vec![v2]));
                        self.cur.stack.push(HeapValue::Obj(arr));
                        return Ok(None);
                    }
                    // (2) Cola vacía: ¿hay un emisor bloqueado? (canal lleno con cap > 0, o rendezvous
                    // cap = 0). Toma su valor directo y despiértalo.
                    if let Some(pos) = sh.parked.iter().position(
                        |p| p.on == h && matches!(p.waiting, Waiting::Send(_)))
                    {
                        let parked = sh.parked.remove(pos);
                        let sv = match parked.waiting {
                            Waiting::Send(sv) => sv,
                            _ => unreachable!(),
                        };
                        // M38.1b-2: el valor del emisor bloqueado vive en el heap de SU fibra (aparcada) →
                        // se transfiere al heap del receptor antes de despertar al emisor.
                        let sv2 = transfer_value(&parked.fiber.heap, &mut self.cur.heap, &sv, &mut HashMap::new());
                        Self::wake_sender(&mut sh, parked.fiber);
                        let arr = self.cur.heap.allocate(Obj::Array(vec![sv2]));
                        self.cur.stack.push(HeapValue::Obj(arr));
                        return Ok(None);
                    }
                    // (3) Cola vacía y sin emisores: cerrado → None ([]); abierto → bloquear (Recv).
                    let closed = sh.channels[h].closed;
                    if closed {
                        let arr = self.cur.heap.allocate(Obj::Array(Vec::new()));
                        self.cur.stack.push(HeapValue::Obj(arr));
                    } else {
                        // Bloquear: guardar la fibra actual (ip ya apunta tras el ChanRecv → al
                        // despertarla, el `wake_recv` le deja el `[T]` en la pila y continúa) y conmutar.
                        let fiber = Self::take_current_fiber(&mut self.cur);
                        sh.parked.push(Parked { on: h, fiber, waiting: Waiting::Recv });
                        sh.running -= 1; // este receptor se bloqueó → worker ocioso
                        drop(sh);
                        let (l, c2) = pos!();
                        if !self.poll_next(l, c2)? { self.stop = true; }
                    }
                }
                OpCode::TaskJoin => {
                    // Une una tarea (M12.3): si terminó, su valor; si falló, re-lanza; si pendiente, bloquea.
                    // M38.3b paso 3: UN solo guard a través de leer-estado + aparcar (bajo M:N real, leer
                    // Pending y luego aparcar en dos locks separados perdería el wake si la tarea completa en
                    // medio → cuelgue).
                    let t = self.pop_task();
                    let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                    let outcome = match &sh.tasks[t].state {
                        TaskState::Done(v) => Some(Ok(v.clone())),
                        TaskState::Failed(msg) => Some(Err(msg.clone())),
                        TaskState::Pending => None,
                    };
                    match outcome {
                        Some(Ok(v)) => {
                            // M38.1b-2: el valor de Done vive en el heap de la tarea → al heap del que la une.
                            let t_heap = std::mem::take(&mut sh.tasks[t].heap);
                            let v2 = transfer_value(&t_heap, &mut self.cur.heap, &v, &mut HashMap::new());
                            sh.tasks[t].heap = t_heap;
                            drop(sh);
                            self.push(v2);
                        }
                        Some(Err(msg)) => return Err(runtime_error(pos!().0, pos!().1, &msg)),
                        None => {
                            // Bloquear: re-empuja el id (lo sacamos arriba) y rebobina el ip al
                            // TaskJoin, para que al despertar (con la tarea ya Done/Failed) lo re-ejecute.
                            self.cur.stack.push(HeapValue::Task(t)); // no `self.push` (guard sostenido)
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            sh.parked.push(Parked { on: t, fiber, waiting: Waiting::Join });
                            sh.running -= 1;
                            drop(sh);
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                OpCode::ScopeBegin => {
                    // Abre un scope (M12.3): las tareas spawneadas mientras esté activo se le adscriben.
                    self.cur.scopes.push(ScopeFrame { children: Vec::new() });
                }
                OpCode::ScopeEnd => {
                    // Cierra el scope: el valor del cuerpo (R) ya está en la pila.
                    // M38.3b paso 3: UN solo guard a través de comprobar-fallo/pendiente + aparcar (como
                    // TaskJoin: evita perder el wake si una hija completa entre el chequeo y el park).
                    let children: Vec<usize> =
                        self.cur.scopes.last().expect("ScopeEnd sin ScopeBegin").children.clone();
                    let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                    // (1) ¿Alguna hija FALLÓ? Cancela a las hermanas que sigan pendientes y propaga el fallo
                    // ORIGINAL de inmediato, sin esperar a las demás (M12.5: cancelación de hermanas).
                    let failure = children.iter().find_map(|&c| match &sh.tasks[c].state {
                        TaskState::Failed(msg) => Some(msg.clone()),
                        _ => None,
                    });
                    if let Some(msg) = failure {
                        for &c in &children {
                            Self::cancel_task(&mut sh, c); // ignora las no-pendientes (la que falló, las Done)
                        }
                        drop(sh);
                        self.cur.scopes.pop();
                        return Err(runtime_error(pos!().0, pos!().1, &msg));
                    }
                    // (2) ¿Alguna pendiente? Rebobina a ScopeEnd y bloquéate (al despertar re-escanea).
                    let pending = children.iter().copied().find(|&c|
                        matches!(sh.tasks[c].state, TaskState::Pending));
                    if let Some(c) = pending {
                        self.cur.frames.last_mut().unwrap().ip -= 1;
                        let fiber = Self::take_current_fiber(&mut self.cur);
                        sh.parked.push(Parked { on: c, fiber, waiting: Waiting::Join });
                        sh.running -= 1;
                        drop(sh);
                        let (l, c2) = pos!();
                        if !self.poll_next(l, c2)? { self.stop = true; }
                    } else {
                        // (3) Todas terminaron con éxito: desapila el scope.
                        drop(sh);
                        self.cur.scopes.pop();
                    }
                }
                OpCode::Select => {
                    // Espera a que algún canal de la lista esté listo para recibir; devuelve su índice
                    // (el menor, determinista). Si ninguno lo está, bloquea esperando al conjunto (M12.4).
                    let arr = self.pop_obj();
                    let chans: Vec<usize> = match self.cur.heap.get(arr) {
                        Obj::Array(elems) => elems.iter().filter_map(|v| match v {
                            HeapValue::Channel(id) => Some(*id),
                            _ => None,
                        }).collect(),
                        _ => unreachable!("el checker garantiza un arreglo de canales"),
                    };
                    // M38.3b paso 3: un ÚNICO guard sostenido a través del escaneo Y el park. Es reentrante-
                    // seguro (un solo lock, sin re-entrar `self.sched()`) y —clave bajo M:N real— atómico:
                    // escanear "ninguno listo" y aparcar deben ser indivisibles, o un canal que se vuelve
                    // listo entre medias dispararía `wake_select_waiters` antes de que estemos aparcados →
                    // wake perdido → cuelgue.
                    let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                    let mut ready_idx = None;
                    for (i, &c) in chans.iter().enumerate() {
                        let ch = &sh.channels[c];
                        let buffered_or_closed = !ch.queue.is_empty() || ch.closed;
                        let has_sender = sh.parked.iter()
                            .any(|p| p.on == c && matches!(p.waiting, Waiting::Send(_)));
                        if buffered_or_closed || has_sender {
                            ready_idx = Some(i);
                            break;
                        }
                    }
                    match ready_idx {
                        Some(i) => {
                            drop(sh);
                            self.push(HeapValue::Int(i as i64));
                        }
                        None => {
                            // Ninguno listo: re-empuja el arreglo (lo sacamos arriba), rebobina el ip al
                            // Select y aparca esperando al conjunto (el handle del arreglo va en `on`).
                            self.cur.stack.push(HeapValue::Obj(arr)); // no `self.push` (guard sostenido)
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            sh.parked.push(Parked { on: arr, fiber, waiting: Waiting::Select });
                            sh.running -= 1;
                            drop(sh);
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }

                // --- Stdlib de string (M11.1) ---
                OpCode::ToString => {
                    // Representación textual (la misma que `print`): coincide con el `Display`
                    // que usa el intérprete en `to_string`.
                    let v = self.pop();
                    let s = format_value(&self.cur.heap, &self.program.enums, &v);
                    self.push(HeapValue::Str(s));
                }
                OpCode::Trim => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Str(s.trim().to_string())),
                    _ => unreachable!("el checker garantiza un string"),
                },
                OpCode::Split => {
                    // El separador está encima del string (orden de los argumentos).
                    let sep = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(sep)) = (s, sep) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    let parts: Vec<HeapValue> =
                        s.split(sep.as_str()).map(|p| HeapValue::Str(p.to_string())).collect();
                    // El arreglo es un objeto del heap; los Str son inline, sin handles que rootear.
                    let h = self.cur.heap.allocate(Obj::Array(parts));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Chars => {
                    let s = match self.pop() {
                        HeapValue::Str(s) => s,
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let cs: Vec<HeapValue> = s.chars().map(HeapValue::Char).collect();
                    // El arreglo es un objeto del heap; los Char son inline, sin handles que rootear.
                    let h = self.cur.heap.allocate(Obj::Array(cs));
                    self.push(HeapValue::Obj(h));
                }
                // M40.3a: el code point Unicode de un char → int.
                OpCode::CharCode => {
                    let c = match self.pop() {
                        HeapValue::Char(c) => c,
                        _ => unreachable!("el checker garantiza un char"),
                    };
                    self.push(HeapValue::Int(c as i64));
                }
                // M16.1b: los octetos UTF-8 del string → bytes (inline, no objeto del heap).
                OpCode::ToBytes => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Bytes(s.into_bytes())),
                    _ => unreachable!("el checker garantiza un string"),
                },
                // M43: hashes de producción vía `ring` (helpers compartidos con el intérprete).
                OpCode::Sha256 => match self.pop() {
                    HeapValue::Bytes(b) => self.push(HeapValue::Bytes(crate::builtins::sha256(&b))),
                    _ => unreachable!("el checker garantiza bytes"),
                },
                OpCode::Sha512 => match self.pop() {
                    HeapValue::Bytes(b) => self.push(HeapValue::Bytes(crate::builtins::sha512(&b))),
                    _ => unreachable!("el checker garantiza bytes"),
                },
                OpCode::Sha1 => match self.pop() {
                    HeapValue::Bytes(b) => self.push(HeapValue::Bytes(crate::builtins::sha1(&b))),
                    _ => unreachable!("el checker garantiza bytes"),
                },
                OpCode::HmacSha256 => {
                    let m = self.pop();
                    let k = self.pop();
                    let (HeapValue::Bytes(k), HeapValue::Bytes(m)) = (k, m) else {
                        unreachable!("el checker garantiza bytes, bytes");
                    };
                    self.push(HeapValue::Bytes(crate::builtins::hmac_sha256(&k, &m)));
                }
                // M43.3: Ed25519. Los fallibles empujan `[bytes]` etiquetado; `verify` empuja un bool.
                OpCode::Ed25519PublicKey => {
                    let seed = match self.pop() {
                        HeapValue::Bytes(b) => b,
                        _ => unreachable!("el checker garantiza bytes"),
                    };
                    let elems = match crate::builtins::ed25519_public_key(&seed) {
                        Some(pk) => vec![HeapValue::Bytes(pk)],
                        None => vec![],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Ed25519Sign => {
                    let msg = self.pop();
                    let seed = self.pop();
                    let (HeapValue::Bytes(seed), HeapValue::Bytes(msg)) = (seed, msg) else {
                        unreachable!("el checker garantiza bytes, bytes");
                    };
                    let elems = match crate::builtins::ed25519_sign(&seed, &msg) {
                        Some(sig) => vec![HeapValue::Bytes(sig)],
                        None => vec![],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Ed25519Verify => {
                    let sig = self.pop();
                    let msg = self.pop();
                    let pk = self.pop();
                    let (HeapValue::Bytes(pk), HeapValue::Bytes(msg), HeapValue::Bytes(sig)) = (pk, msg, sig) else {
                        unreachable!("el checker garantiza bytes, bytes, bytes");
                    };
                    self.push(HeapValue::Bool(crate::builtins::ed25519_verify(&pk, &msg, &sig)));
                }
                // M43.4: ChaCha20-Poly1305 AEAD. Pop en orden inverso (dato, aad, nonce, clave).
                op @ (OpCode::ChaChaPolySeal | OpCode::ChaChaPolyOpen) => {
                    let data = self.pop();
                    let aad = self.pop();
                    let nonce = self.pop();
                    let key = self.pop();
                    let (HeapValue::Bytes(key), HeapValue::Bytes(nonce), HeapValue::Bytes(aad), HeapValue::Bytes(data)) =
                        (key, nonce, aad, data)
                    else {
                        unreachable!("el checker garantiza cuatro bytes");
                    };
                    let res = if matches!(op, OpCode::ChaChaPolySeal) {
                        crate::builtins::chacha20poly1305_seal(&key, &nonce, &aad, &data)
                    } else {
                        crate::builtins::chacha20poly1305_open(&key, &nonce, &aad, &data)
                    };
                    let elems = match res {
                        Some(out) => vec![HeapValue::Bytes(out)],
                        None => vec![],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M16.1b: decodifica bytes como UTF-8 → arreglo etiquetado; el prelude → Result.
                OpCode::FromUtf8 => {
                    let b = match self.pop() {
                        HeapValue::Bytes(b) => b,
                        _ => unreachable!("el checker garantiza bytes"),
                    };
                    let elems = match String::from_utf8(b) {
                        Ok(s) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(s)],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }

                // --- I/O binaria (M16.1c) ---
                // Lecturas → [bytes] etiquetado (tag en bytes para que el arreglo sea homogéneo).
                OpCode::ReadFileBytes => {
                    let path = match self.pop() {
                        HeapValue::Str(s) => s,
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let elems = match crate::builtins::read_file_bytes(&path) {
                        Ok(data) => vec![HeapValue::Bytes(b"ok".to_vec()), HeapValue::Bytes(data)],
                        Err(e) => vec![HeapValue::Bytes(b"err".to_vec()), HeapValue::Bytes(e.to_string().into_bytes())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::WriteFileBytes => {
                    let data = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Bytes(data)) = (path, data) else {
                        unreachable!("el checker garantiza string, bytes");
                    };
                    let elems = match crate::builtins::write_file_bytes(&path, &data) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M16.1c: lectura binaria del socket; cede al scheduler en WouldBlock (como SocketRead, M15.5).
                OpCode::SocketReadBytes => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("el checker garantiza un int"),
                    };
                    // M19.4b: una conexión TLS se lee con su bomba no bloqueante (conduce el handshake/
                    // descifrado); si bloquearía leyendo del peer, se aparca la fibra en el fd subyacente,
                    // igual que un socket normal. El intérprete usa el camino bloqueante (no llega aquí).
                    if crate::builtins::is_tls_handle(handle) {
                        match crate::builtins::tls_read_nb(handle) {
                            Ok(Some(data)) => {
                                let elems = vec![HeapValue::Bytes(b"ok".to_vec()), HeapValue::Bytes(data)];
                                let h = self.cur.heap.allocate(Obj::Array(elems));
                                self.push(HeapValue::Obj(h));
                            }
                            Err(e) => {
                                let elems = vec![HeapValue::Bytes(b"err".to_vec()), HeapValue::Bytes(e.into_bytes())];
                                let h = self.cur.heap.allocate(Obj::Array(elems));
                                self.push(HeapValue::Obj(h));
                            }
                            Ok(None) => {
                                self.push(HeapValue::Int(handle));
                                self.cur.frames.last_mut().unwrap().ip -= 1;
                                let fiber = Self::take_current_fiber(&mut self.cur);
                                let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
                                {
                                    let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                                    sh.io_parked.push(IoParked { fd, fiber, pending_write: None });
                                    sh.running -= 1; // aparcada por E/S → worker ocioso
                                }
                                let (l, c2) = pos!();
                                if !self.poll_next(l, c2)? { self.stop = true; }
                            }
                        }
                        return Ok(None);
                    }
                    match crate::builtins::socket_read_bytes_nb(handle) {
                        Ok(Some(data)) => {
                            let elems = vec![HeapValue::Bytes(b"ok".to_vec()), HeapValue::Bytes(data)];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Bytes(b"err".to_vec()), HeapValue::Bytes(e.into_bytes())];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(None) => {
                            self.push(HeapValue::Int(handle));
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            // M17: guarda el fd del socket para que el scheduler lo registre en el poller.
                            let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
                            {
                                let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                                sh.io_parked.push(IoParked { fd, fiber, pending_write: None });
                                sh.running -= 1; // aparcada por E/S → worker ocioso
                            }
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                OpCode::SocketWriteBytes => {
                    let data = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Bytes(data)) = (handle, data) else {
                        unreachable!("el checker garantiza int, bytes");
                    };
                    if crate::builtins::is_tls_handle(handle) {
                        // M19.4b: las escrituras TLS cifran por su propia bomba (busy-spin en el raro bloqueo).
                        let elems = match crate::builtins::tls_write_nb(handle, &data) {
                            Ok(_) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(String::new())],
                            Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                        };
                        let h = self.cur.heap.allocate(Obj::Array(elems));
                        self.push(HeapValue::Obj(h));
                    } else {
                        // TCP plano: escritura parcial; si el buffer se llena, CEDE la fibra hasta que el
                        // socket sea escribible (en vez de girar) → no acapara el hilo del scheduler.
                        match crate::builtins::socket_write_nb(handle, &data) {
                            Ok(n) if n == data.len() => {
                                let elems = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(String::new())];
                                let h = self.cur.heap.allocate(Obj::Array(elems));
                                self.push(HeapValue::Obj(h));
                            }
                            Ok(n) => {
                                let (line, col) = pos!();
                                self.park_write(handle, data[n..].to_vec(), line, col)?;
                                return Ok(None);
                            }
                            Err(e) => {
                                let elems = vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)];
                                let h = self.cur.heap.allocate(Obj::Array(elems));
                                self.push(HeapValue::Obj(h));
                            }
                        }
                    }
                }
                OpCode::Contains => {
                    // El valor buscado está encima del contenedor (orden de los argumentos).
                    let x = self.pop();
                    let cont = self.pop();
                    let res = match (&cont, &x) {
                        (HeapValue::Str(s), HeapValue::Str(sub)) => s.contains(sub.as_str()),
                        // M11.7b: arreglo → pertenencia por igualdad estructural.
                        (HeapValue::Obj(h), _) => {
                            self.as_array(*h).iter().any(|e| values_equal(&self.cur.heap, e, &x))
                        }
                        _ => unreachable!("el checker garantiza string+string o arreglo+elemento"),
                    };
                    self.push(HeapValue::Bool(res));
                }
                OpCode::Replace => {
                    // Orden de los argumentos en la pila: s, de, a → se sacan en orden inverso.
                    let a = self.pop();
                    let de = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(de), HeapValue::Str(a)) = (s, de, a) else {
                        unreachable!("el checker garantiza tres strings");
                    };
                    self.push(HeapValue::Str(s.replace(de.as_str(), a.as_str())));
                }

                // --- Más string (M11.7a) ---
                OpCode::StartsWith => {
                    let p = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(p)) = (s, p) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    self.push(HeapValue::Bool(s.starts_with(p.as_str())));
                }
                OpCode::EndsWith => {
                    let p = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(p)) = (s, p) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    self.push(HeapValue::Bool(s.ends_with(p.as_str())));
                }
                OpCode::ToUpper => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Str(s.to_uppercase())),
                    _ => unreachable!("el checker garantiza un string"),
                },
                OpCode::ToLower => match self.pop() {
                    HeapValue::Str(s) => self.push(HeapValue::Str(s.to_lowercase())),
                    _ => unreachable!("el checker garantiza un string"),
                },
                OpCode::Substring => {
                    // Orden en la pila: s, i, j → se sacan en inverso.
                    let j = self.pop();
                    let i = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Int(i), HeapValue::Int(j)) = (s, i, j) else {
                        unreachable!("el checker garantiza string, int, int");
                    };
                    self.push(HeapValue::Str(crate::builtins::substring_chars(&s, i, j)));
                }
                // M19.2: sub-secuencia de bytes por octeto (con clamp). Orden en la pila: b, i, j.
                OpCode::SubBytes => {
                    let j = self.pop();
                    let i = self.pop();
                    let b = self.pop();
                    let (HeapValue::Bytes(b), HeapValue::Int(i), HeapValue::Int(j)) = (b, i, j) else {
                        unreachable!("el checker garantiza bytes, int, int");
                    };
                    self.push(HeapValue::Bytes(crate::builtins::sub_bytes_octets(&b, i, j)));
                }
                // M19.3c: construye bytes a partir de un [int] (objeto del heap), truncando a octeto.
                OpCode::BytesOf => {
                    let HeapValue::Obj(h) = self.pop() else {
                        unreachable!("el checker garantiza un arreglo");
                    };
                    let octets: Vec<u8> = self.as_array(h).iter().map(|v| match v {
                        HeapValue::Int(n) => (*n & 0xff) as u8,
                        _ => unreachable!("el checker garantiza [int]"),
                    }).collect();
                    self.push(HeapValue::Bytes(octets));
                }
                OpCode::Repeat => {
                    let n = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Int(n)) = (s, n) else {
                        unreachable!("el checker garantiza string, int");
                    };
                    self.push(HeapValue::Str(crate::builtins::repeat_str(&s, n)));
                }
                OpCode::IndexOf => {
                    // Primitivo: [] o [i] (índice de carácter). El prelude → Option<int>.
                    let sub = self.pop();
                    let s = self.pop();
                    let (HeapValue::Str(s), HeapValue::Str(sub)) = (s, sub) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    let elems = match crate::builtins::char_index_of(&s, &sub) {
                        Some(i) => vec![HeapValue::Int(i as i64)],
                        None => vec![],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Join => {
                    // Orden en la pila: arr, sep → se saca sep primero.
                    let sep = self.pop();
                    let arr = self.pop();
                    let (HeapValue::Obj(h), HeapValue::Str(sep)) = (arr, sep) else {
                        unreachable!("el checker garantiza [string], string");
                    };
                    let parts: Vec<String> = self.as_array(h).iter().map(|v| match v {
                        HeapValue::Str(s) => s.clone(),
                        _ => unreachable!("el checker garantiza [string]"),
                    }).collect();
                    self.push(HeapValue::Str(parts.join(sep.as_str())));
                }

                // --- Más arreglos (M11.7b) ---
                OpCode::Reverse => {
                    let h = self.pop_obj();
                    let mut elems = self.as_array(h).clone();
                    elems.reverse();
                    let nh = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(nh));
                }
                OpCode::ArrayPop => {
                    // Muta el arreglo quitando el último; devuelve [] o [x]. Prelude → Option<T>.
                    let h = self.pop_obj();
                    let popped = self.as_array_mut(h).pop();
                    let elems = popped.map(|v| vec![v]).unwrap_or_default();
                    let nh = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(nh));
                }
                OpCode::Position => {
                    let x = self.pop();
                    let h = self.pop_obj();
                    let idx = self.as_array(h).iter().position(|e| values_equal(&self.cur.heap, e, &x));
                    let elems = idx.map(|i| vec![HeapValue::Int(i as i64)]).unwrap_or_default();
                    let nh = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(nh));
                }

                // --- I/O y API de runtime (M11.2) ---
                // M13.2a: aborta con el mensaje en la posición de la llamada (igual que el
                // intérprete, que lo intercepta en `eval_call`). El mensaje es el string en la cima.
                OpCode::Panic => {
                    let msg = match self.pop() {
                        HeapValue::Str(s) => s,
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    return Err(runtime_error(pos!().0, pos!().1, &msg));
                }
                OpCode::EPrint => {
                    let v = self.pop();
                    crate::host_eprint(&format_value(&self.cur.heap, &self.program.enums, &v));
                    self.push(HeapValue::Unit);
                }
                OpCode::ParseInt => {
                    // Primitivo: [] o [n]; el prelude lo envuelve en Option<int>.
                    let elems = match self.pop() {
                        HeapValue::Str(s) => match s.trim().parse::<i64>() {
                            Ok(n) => vec![HeapValue::Int(n)],
                            Err(_) => vec![],
                        },
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ParseFloat => {
                    // M14: [] o [f]; el prelude lo envuelve en Option<float>.
                    let elems = match self.pop() {
                        HeapValue::Str(s) => match s.trim().parse::<f64>() {
                            Ok(f) => vec![HeapValue::Float(f)],
                            Err(_) => vec![],
                        },
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ReadLine => {
                    // Primitivo: [] en EOF, [linea] si no (sin el '\n'). El prelude → Option<string>.
                    let mut line = String::new();
                    let elems = match std::io::stdin().read_line(&mut line) {
                        Ok(0) | Err(_) => vec![],
                        Ok(_) => vec![HeapValue::Str(line.trim_end_matches(['\n', '\r']).to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Env => {
                    // Primitivo: [] si no existe, [valor] si sí. El prelude → Option<string>.
                    let elems = match self.pop() {
                        HeapValue::Str(name) => match std::env::var(name.as_str()) {
                            Ok(v) => vec![HeapValue::Str(v)],
                            Err(_) => vec![],
                        },
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Args => {
                    // Argumentos del programa (del almacén de proceso); arreglo de strings.
                    let items: Vec<HeapValue> = crate::runtime::program_args()
                        .iter()
                        .map(|a| HeapValue::Str(a.clone()))
                        .collect();
                    let h = self.cur.heap.allocate(Obj::Array(items));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ReadFile => {
                    // Arreglo etiquetado ["ok", contenido] o ["err", msg]. El prelude → Result.
                    let elems = match self.pop() {
                        HeapValue::Str(path) => match std::fs::read_to_string(path.as_str()) {
                            Ok(c) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(c)],
                            Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                        },
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::WriteFile => {
                    // El contenido está encima de la ruta (orden de los argumentos).
                    let contents = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Str(contents)) = (path, contents) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    let elems = match std::fs::write(path.as_str(), contents.as_str()) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::Exists => match self.pop() {
                    HeapValue::Str(path) => self.push(HeapValue::Bool(std::path::Path::new(path.as_str()).exists())),
                    _ => unreachable!("el checker garantiza un string"),
                },
                OpCode::AppendFile => {
                    // El contenido está encima de la ruta (orden de los argumentos).
                    let contents = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Str(contents)) = (path, contents) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    let elems = match crate::builtins::append_to_file(path.as_str(), contents.as_str()) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::RemoveFile => {
                    let path = match self.pop() {
                        HeapValue::Str(p) => p,
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let elems = match std::fs::remove_file(&path) {
                        Ok(()) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ListDir => {
                    let path = match self.pop() {
                        HeapValue::Str(p) => p,
                        _ => unreachable!("el checker garantiza un string"),
                    };
                    let elems = match crate::builtins::list_dir(&path) {
                        Ok(nombres) => {
                            let mut v = vec![HeapValue::Str("ok".to_string())];
                            v.extend(nombres.into_iter().map(HeapValue::Str));
                            v
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e.to_string())],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }

                // --- I/O con buffering: handles de archivo (M11.8) ---
                OpCode::Open => {
                    let mode = self.pop();
                    let path = self.pop();
                    let (HeapValue::Str(path), HeapValue::Str(mode)) = (path, mode) else {
                        unreachable!("el checker garantiza dos strings");
                    };
                    let elems = match crate::builtins::open_file(&path, &mode) {
                        Ok(h) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::ReadLineHandle => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("el checker garantiza un int"),
                    };
                    let elems = crate::builtins::read_line_handle(handle).map(|l| vec![HeapValue::Str(l)]).unwrap_or_default();
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::WriteHandle => {
                    let s = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(s)) = (handle, s) else {
                        unreachable!("el checker garantiza int, string");
                    };
                    let elems = match crate::builtins::write_handle(handle, &s) {
                        Ok(_) => vec![HeapValue::Str("ok".to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // --- Cliente TCP (M15.2): arreglo etiquetado en el heap; el prelude → Result. ---
                OpCode::TcpConnect => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("el checker garantiza string, int");
                    };
                    let elems = match crate::builtins::tcp_connect(&host, port) {
                        Ok(h) => {
                            // M15.5: la VM usa sockets NO bloqueantes → socket_read cede al scheduler.
                            let _ = crate::builtins::set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M20.8: UDP. Bloqueante en ambos motores por ahora (la cesión cooperativa queda diferida).
                OpCode::UdpBind => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("el checker garantiza string, int");
                    };
                    let elems = match crate::builtins::udp_bind(&host, port) {
                        Ok(h) => {
                            // M20.11: la VM usa el socket NO bloqueante → udp_recv_from cede al scheduler.
                            let _ = crate::builtins::set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::UdpSendTo => {
                    let data = self.pop();
                    let port = self.pop();
                    let host = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(host), HeapValue::Int(port), HeapValue::Bytes(data)) =
                        (handle, host, port, data)
                    else {
                        unreachable!("el checker garantiza int, string, int, bytes");
                    };
                    let elems = match crate::builtins::udp_send_to(handle, &host, port, &data) {
                        Ok(n) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(n.to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M20.11: recv UDP no bloqueante; cede la fibra al scheduler en WouldBlock (como SocketReadBytes).
                OpCode::UdpRecvFrom => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("el checker garantiza un int"),
                    };
                    match crate::builtins::udp_recv_from_nb(handle) {
                        Ok(Some((host, port, data))) => {
                            let elems = vec![
                                HeapValue::Bytes(b"ok".to_vec()),
                                HeapValue::Bytes(host.into_bytes()),
                                HeapValue::Bytes(port.to_string().into_bytes()),
                                HeapValue::Bytes(data),
                            ];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Bytes(b"err".to_vec()), HeapValue::Bytes(e.into_bytes())];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(None) => {
                            // No hay datagrama: re-ejecuta el opcode al despertar (re-empuja el handle).
                            self.push(HeapValue::Int(handle));
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
                            {
                                let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                                sh.io_parked.push(IoParked { fd, fiber, pending_write: None });
                                sh.running -= 1; // aparcada por E/S → worker ocioso
                            }
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                // M19.4b: conexión TLS de cliente. La VM pone el socket en no bloqueante para que el I/O
                // TLS (SocketReadBytes) pueda ceder la fibra, como con un socket plano.
                OpCode::TlsConnect => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("el checker garantiza string, int");
                    };
                    let elems = match crate::builtins::tls_connect(&host, port) {
                        Ok(h) => {
                            let _ = crate::builtins::tls_set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M31.2a: conexión TLS con ALPN h2 (el handshake ya se completó de forma bloqueante en el
                // builtin; tras él se pone no bloqueante para el framing con cesión de fibras).
                OpCode::TlsConnectH2 => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("el checker garantiza string, int");
                    };
                    let elems = match crate::builtins::tls_connect_h2(&host, port) {
                        Ok(h) => {
                            let _ = crate::builtins::tls_set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                // M19.4b: envuelve un socket aceptado (ya no bloqueante) en una sesión TLS de servidor.
                // El handshake lo conduce el primer SocketReadBytes, cediendo la fibra si bloquea.
                OpCode::TlsAccept => {
                    let key = self.pop();
                    let cert = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(cert), HeapValue::Str(key)) = (handle, cert, key) else {
                        unreachable!("el checker garantiza int, string, string");
                    };
                    let elems = match crate::builtins::tls_accept(handle, &cert, &key) {
                        Ok(h) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())],
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::SocketRead => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("el checker garantiza un int"),
                    };
                    // M15.5: lectura no bloqueante. WouldBlock (Ok(None)) → aparcar la fibra y reintentar.
                    match crate::builtins::socket_read_nb(handle) {
                        Ok(Some(s)) => {
                            let elems = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(s)];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(None) => {
                            // Re-empuja el handle y rebobina al SocketRead: al despertar lo re-ejecuta.
                            self.push(HeapValue::Int(handle));
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            // M17: guarda el fd del socket para que el scheduler lo registre en el poller.
                            let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
                            {
                                let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                                sh.io_parked.push(IoParked { fd, fiber, pending_write: None });
                                sh.running -= 1; // aparcada por E/S → worker ocioso
                            }
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                OpCode::SocketWrite => {
                    let s = self.pop();
                    let handle = self.pop();
                    let (HeapValue::Int(handle), HeapValue::Str(s)) = (handle, s) else {
                        unreachable!("el checker garantiza int, string");
                    };
                    // Cesión en `socket_write` (como SocketWriteBytes): escritura parcial de los octetos
                    // UTF-8; si el buffer se llena, cede la fibra (el resto pendiente vive en bytes, lo que
                    // evita reconstruir un string roto a mitad de carácter multibyte).
                    let bytes = s.into_bytes();
                    match crate::builtins::socket_write_nb(handle, &bytes) {
                        Ok(n) if n == bytes.len() => {
                            let elems = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(String::new())];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(n) => {
                            let (line, col) = pos!();
                            self.park_write(handle, bytes[n..].to_vec(), line, col)?;
                            return Ok(None);
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                    }
                }
                // --- Servidor TCP (M15.3) ---
                OpCode::TcpListen => {
                    let port = self.pop();
                    let host = self.pop();
                    let (HeapValue::Str(host), HeapValue::Int(port)) = (host, port) else {
                        unreachable!("el checker garantiza string, int");
                    };
                    let elems = match crate::builtins::tcp_listen(&host, port) {
                        Ok(h) => {
                            // M15.5: escucha NO bloqueante → tcp_accept cede al scheduler.
                            let _ = crate::builtins::set_nonblocking(h);
                            vec![HeapValue::Str("ok".to_string()), HeapValue::Str(h.to_string())]
                        }
                        Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
                    };
                    let h = self.cur.heap.allocate(Obj::Array(elems));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::TcpAccept => {
                    let handle = match self.pop() {
                        HeapValue::Int(h) => h,
                        _ => unreachable!("el checker garantiza un int"),
                    };
                    // M15.5: accept no bloqueante. WouldBlock (Ok(None)) → aparcar y reintentar.
                    match crate::builtins::tcp_accept_nb(handle) {
                        Ok(Some(c)) => {
                            let elems = vec![HeapValue::Str("ok".to_string()), HeapValue::Str(c.to_string())];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Err(e) => {
                            let elems = vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)];
                            let h = self.cur.heap.allocate(Obj::Array(elems));
                            self.push(HeapValue::Obj(h));
                        }
                        Ok(None) => {
                            self.push(HeapValue::Int(handle));
                            self.cur.frames.last_mut().unwrap().ip -= 1;
                            let fiber = Self::take_current_fiber(&mut self.cur);
                            // M17: guarda el fd del socket para que el scheduler lo registre en el poller.
                            let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
                            {
                                let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                                sh.io_parked.push(IoParked { fd, fiber, pending_write: None });
                                sh.running -= 1; // aparcada por E/S → worker ocioso
                            }
                            let (l, c2) = pos!();
                            if !self.poll_next(l, c2)? { self.stop = true; }
                        }
                    }
                }
                OpCode::LocalPort => match self.pop() {
                    HeapValue::Int(h) => self.push(HeapValue::Int(crate::builtins::local_port(h))),
                    _ => unreachable!("el checker garantiza un int"),
                },
                OpCode::Close => {
                    // Ad-hoc polimórfico: un handle de archivo (int, M11.8) o un canal (M12.1).
                    match self.pop() {
                        HeapValue::Int(h) => {
                            crate::builtins::close_handle(h);
                            self.push(HeapValue::Int(0));
                        }
                        // Cerrar un canal: marcarlo cerrado y despertar a TODOS sus receptores bloqueados
                        // (recibirán [] → None). Devuelve unit. M12.2: cerrar un canal con un EMISOR
                        // bloqueado es un error de programa (alguien todavía esperaba enviar) → error de
                        // ejecución en el sitio del `close` (determinista, a diferencia de "panic en otra
                        // fibra").
                        HeapValue::Channel(ch) => {
                            // M38.3b paso 2: un ÚNICO lock para todo el cierre. El `if ... && matches!(...)`
                            // hacía dos `self.sched()` en la misma condición con guards solapados → doble-lock
                            // del Mutex no reentrante = DEADLOCK cuando el canal tenía un receptor aparcado.
                            let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
                            if sh.parked.iter().any(
                                |p| p.on == ch && matches!(p.waiting, Waiting::Send(_)))
                            {
                                return Err(runtime_error(pos!().0, pos!().1,
                                    "close sobre un canal con un emisor bloqueado"));
                            }
                            sh.channels[ch].closed = true;
                            let mut i = 0;
                            while i < sh.parked.len() {
                                if sh.parked[i].on == ch && matches!(sh.parked[i].waiting, Waiting::Recv) {
                                    let parked = sh.parked.remove(i);
                                    Self::wake_recv(&self.cur, &mut sh, parked.fiber, Vec::new());
                                } else {
                                    i += 1;
                                }
                            }
                            Self::wake_select_waiters(&mut sh, ch); // M12.4: un canal cerrado está "listo" para un select
                            self.cur.stack.push(HeapValue::Unit);
                        }
                        _ => unreachable!("el checker garantiza un handle (int) o un Channel"),
                    }
                }

                // --- Matemáticas (M15.1a) ---
                // Una sola rama para las 10 funciones float -> float; delega en el helper compartido
                // con el intérprete (mismo cálculo → oráculo cuadra, incl. NaN/inf).
                OpCode::MathF(f) => match self.pop() {
                    HeapValue::Float(x) => self.push(HeapValue::Float(crate::builtins::apply_mathf(*f, x))),
                    _ => unreachable!("el checker garantiza un float"),
                },
                OpCode::Pow => {
                    let exp = self.pop();
                    let base = self.pop();
                    let (HeapValue::Float(base), HeapValue::Float(exp)) = (base, exp) else {
                        unreachable!("el checker garantiza dos floats");
                    };
                    self.push(HeapValue::Float(base.powf(exp)));
                }
                OpCode::Abs => match self.pop() {
                    HeapValue::Int(x) => self.push(HeapValue::Int(x.abs())),
                    HeapValue::Float(x) => self.push(HeapValue::Float(x.abs())),
                    _ => unreachable!("el checker garantiza int o float"),
                },
                OpCode::Min => {
                    let b = self.pop();
                    let a = self.pop();
                    match (a, b) {
                        (HeapValue::Int(a), HeapValue::Int(b)) => self.push(HeapValue::Int(a.min(b))),
                        (HeapValue::Float(a), HeapValue::Float(b)) => self.push(HeapValue::Float(a.min(b))),
                        _ => unreachable!("el checker garantiza dos números del mismo tipo"),
                    }
                }
                OpCode::Max => {
                    let b = self.pop();
                    let a = self.pop();
                    match (a, b) {
                        (HeapValue::Int(a), HeapValue::Int(b)) => self.push(HeapValue::Int(a.max(b))),
                        (HeapValue::Float(a), HeapValue::Float(b)) => self.push(HeapValue::Float(a.max(b))),
                        _ => unreachable!("el checker garantiza dos números del mismo tipo"),
                    }
                }
                OpCode::Pi => self.push(HeapValue::Float(std::f64::consts::PI)),
                OpCode::E => self.push(HeapValue::Float(std::f64::consts::E)),

                // --- Reloj y aleatoriedad (M15.1b): delegan en los helpers compartidos. ---
                OpCode::Now => self.push(HeapValue::Int(crate::builtins::now_millis())),
                OpCode::Monotonic => self.push(HeapValue::Int(crate::builtins::monotonic_millis())),
                OpCode::Sleep => match self.pop() {
                    HeapValue::Int(ms) => {
                        crate::builtins::sleep_millis(ms);
                        self.push(HeapValue::Unit);
                    }
                    _ => unreachable!("el checker garantiza un int"),
                },
                OpCode::Random => self.push(HeapValue::Float(crate::builtins::random_f64())),
                OpCode::RandomInt => match self.pop() {
                    HeapValue::Int(n) => self.push(HeapValue::Int(crate::builtins::random_int(n))),
                    _ => unreachable!("el checker garantiza un int"),
                },

                // --- Structs (M3.2) ---
                OpCode::MakeStruct(idx) => {
                    let sname = self.program.structs[*idx].name.clone();
                    let field_names: Vec<String> = self.program.structs[*idx].fields.clone();
                    let mut values = Vec::with_capacity(field_names.len());
                    for _ in 0..field_names.len() {
                        values.push(self.pop());
                    }
                    values.reverse(); // orden de declaración
                    let fields: Vec<(String, HeapValue)> = field_names.into_iter().zip(values).collect();
                    let h = self.cur.heap.allocate(Obj::Struct(VmStruct { name: sname, fields }));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::MakeEnum(enum_id, tag) => {
                    // La aridad la da la tabla; sacamos ese tanto de payload.
                    let arity = self.program.enums[*enum_id].variants[*tag].arity;
                    let mut payload = Vec::with_capacity(arity);
                    for _ in 0..arity {
                        payload.push(self.pop());
                    }
                    payload.reverse(); // orden de declaración
                    let h = self.cur.heap.allocate(Obj::Enum(VmEnum { enum_id: *enum_id, tag: *tag, payload }));
                    self.push(HeapValue::Obj(h));
                }
                OpCode::EnumTagEq(tag) => {
                    let h = self.pop_obj();
                    let matches = self.as_enum(h).tag == *tag;
                    self.push(HeapValue::Bool(matches));
                }
                OpCode::GetEnumField(i) => {
                    let h = self.pop_obj();
                    let v = self.as_enum(h).payload[*i].clone();
                    self.push(v);
                }
                OpCode::MatchFail => {
                    return Err(runtime_error(pos!().0, pos!().1, "ningún brazo del match casó (no debería ocurrir)"));
                }
                OpCode::GetField(name) => {
                    let h = self.pop_obj();
                    let v = self.as_struct(h).fields.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
                        .expect("el checker garantiza el campo");
                    self.push(v);
                }
                OpCode::SetField(name) => {
                    let v = self.pop();
                    let h = self.pop_obj();
                    let s = self.as_struct_mut(h);
                    let slot = s.fields.iter_mut().find(|(n, _)| n == name).expect("el checker garantiza el campo");
                    slot.1 = v;
                }

                OpCode::Call(idx, argc) => {
                    if self.cur.frames.len() >= MAX_FRAMES {
                        return Err(runtime_error(pos!().0, pos!().1, "desbordamiento de pila (recursión demasiado profunda)"));
                    }
                    let mut locals = self.new_locals(*idx);
                    for i in (0..*argc).rev() {
                        let v = self.pop();
                        self.put_arg(&mut locals, i, v);
                    }
                    self.cur.frames.push(CallFrame { function: *idx, ip: 0, locals, upvalues: Vec::new() });
                }
                // M13.3b: llamada en cola — REUTILIZA el marco actual (no crece la pila de marcos).
                // En posición de cola, el valor de esta llamada es el de la función actual, así que
                // el resultado caerá en la misma posición de la pila. No hay límite que comprobar:
                // ese es justo el punto (recursión de cola en O(1) marcos).
                OpCode::TailCall(idx, argc) => {
                    let mut locals = self.new_locals(*idx);
                    for i in (0..*argc).rev() {
                        let v = self.pop();
                        self.put_arg(&mut locals, i, v);
                    }
                    self.cur.frames[fi].function = *idx;
                    self.cur.frames[fi].ip = 0;
                    let old = std::mem::replace(&mut self.cur.frames[fi].locals, locals);
                    self.recycle_locals(old); // Opt.2: la llamada en cola reemplaza las locales → recicla
                    self.cur.frames[fi].upvalues = Vec::new();
                }

                // --- Funciones de primera clase (M4.1) ---
                OpCode::Function(idx) => self.push(HeapValue::Function(*idx)),
                OpCode::CallValue(argc) => {
                    if self.cur.frames.len() >= MAX_FRAMES {
                        return Err(runtime_error(pos!().0, pos!().1, "desbordamiento de pila (recursión demasiado profunda)"));
                    }
                    let mut args_rev = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        args_rev.push(self.pop());
                    }
                    let (fn_idx, upvalues) = match self.pop() {
                        HeapValue::Function(i) => (i, Vec::new()),
                        HeapValue::Obj(h) => match self.cur.heap.get(h) {
                            Obj::Closure(c) => (c.index, c.upvalues.clone()),
                            _ => unreachable!("el checker garantiza una función"),
                        },
                        _ => unreachable!("el checker garantiza una función"),
                    };
                    let mut locals = self.new_locals(fn_idx);
                    for (j, val) in args_rev.into_iter().enumerate() {
                        self.put_arg(&mut locals, *argc - 1 - j, val);
                    }
                    self.cur.frames.push(CallFrame { function: fn_idx, ip: 0, locals, upvalues });
                }
                // M13.3b: llamada indirecta en cola — reutiliza el marco actual.
                OpCode::TailCallValue(argc) => {
                    let mut args_rev = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        args_rev.push(self.pop());
                    }
                    let (fn_idx, upvalues) = match self.pop() {
                        HeapValue::Function(i) => (i, Vec::new()),
                        HeapValue::Obj(h) => match self.cur.heap.get(h) {
                            Obj::Closure(c) => (c.index, c.upvalues.clone()),
                            _ => unreachable!("el checker garantiza una función"),
                        },
                        _ => unreachable!("el checker garantiza una función"),
                    };
                    let mut locals = self.new_locals(fn_idx);
                    for (j, val) in args_rev.into_iter().enumerate() {
                        self.put_arg(&mut locals, *argc - 1 - j, val);
                    }
                    self.cur.frames[fi].function = fn_idx;
                    self.cur.frames[fi].ip = 0;
                    let old = std::mem::replace(&mut self.cur.frames[fi].locals, locals);
                    self.recycle_locals(old); // Opt.2
                    self.cur.frames[fi].upvalues = upvalues;
                }

                // --- Closures (M4.2) ---
                OpCode::Closure(idx) => {
                    // Armamos el arreglo de upvalues tomando las celdas que indica la
                    // función, del marco actual (un local boxeado, o un upvalue propio
                    // para la captura transitiva).
                    let descs = self.program.functions[*idx].upvalues.clone();
                    let mut upvalues = Vec::with_capacity(descs.len());
                    for d in &descs {
                        let cell = match d.source {
                            UpvalueSource::Local(slot) => match &self.cur.frames[fi].locals[slot] {
                                Local::Boxed(h) => *h,
                                Local::Plain(_) => unreachable!("un local capturado debe estar boxeado"),
                            },
                            UpvalueSource::Upvalue(u) => self.cur.frames[fi].upvalues[u],
                        };
                        upvalues.push(cell);
                    }
                    let h = self.cur.heap.allocate(Obj::Closure(VmClosure { index: *idx, upvalues }));
                    self.push(HeapValue::Obj(h));
                }

                OpCode::Return => {
                    let result = self.pop();
                    if let Some(frame) = self.cur.frames.pop() {
                        self.recycle_locals(frame.locals); // Opt.2: el marco se descarta → recicla sus locales
                    }
                    if self.cur.frames.is_empty() {
                        // La fibra terminó: si es main → fin del programa; si es spawn → siguiente fibra.
                        match self.on_fiber_done(result)? {
                            Some(v) => return Ok(Some(v)),
                            None => return Ok(None),
                        }
                    }
                    self.push(result); // entregamos el valor al llamador
                }
            }
            Ok(None)
            })();

            match outcome {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => {}
                Err(e) => {
                    // Propagación de fallos (M12.3): el error de la fibra HIJA en curso no aborta el
                    // programa; se captura en su `Task` (`Failed`) y se planifica la siguiente. Abortan los
                    // de `main` y los del scheduler (frames vacíos = la fibra ya se aparcó/terminó → el
                    // error es un deadlock, no un fallo de la fibra actual).
                    if self.cur.frames.is_empty() || self.cur.is_main {
                        return Err(e);
                    }
                    self.fail_current_fiber(e)?;
                }
            }
        }
    }

    // ----- Scheduler de fibras (M12.1 / M12.3) -----

    /// Empaqueta la fibra en ejecución (vaciando los campos de la VM) para aparcarla o descartarla. M12.3:
    /// además de `frames`/`stack`/`is_main`, lleva su `Task` y su pila de scopes.
    /// M38.3b: la fibra en curso ES `cur` → conmutar es un swap (deja una `Fiber` vacía).
    fn take_current_fiber(cur: &mut Fiber) -> Fiber {
        std::mem::take(cur)
    }

    /// Cesión en `socket_write` (post-M19.4): la escritura llenó el buffer de envío y `remaining` no
    /// cupo. Aparca la fibra actual esperando que `handle` sea **escribible** (el `ip` ya apunta tras el
    /// opcode de escritura; el resultado se empuja al terminar, en `finish_parked_write`).
    /// M38.3b paso 3: método `&mut self`. Aparca la fibra bajo el guard (con `running -= 1`), suelta el
    /// lock y carga la siguiente con `poll_next` (que, en M:N, puede esperar a otro worker).
    fn park_write(&mut self, handle: i64, remaining: Vec<u8>, line: usize, col: usize) -> Result<(), RuntimeError> {
        let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
        {
            let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
            let fiber = Self::take_current_fiber(&mut self.cur);
            sh.io_parked.push(IoParked { fd, fiber, pending_write: Some(PendingWrite { handle, remaining }) });
            sh.running -= 1; // esta fibra se aparcó → este worker queda ocioso
        }
        if !self.poll_next(line, col)? { self.stop = true; }
        Ok(())
    }

    /// Una fibra aparcada por escritura despertó (su socket es escribible): drena lo que falta. Si lo
    /// completa (o falla), empuja el resultado etiquetado (`["ok",""]`/`["err",msg]`) en su pila y la pone
    /// lista; si aún bloquea, la re-aparca con el resto. (`allocate` no colecta aquí → sin riesgo de GC.)
    fn finish_parked_write(shared: &mut Shared, fd: i32, mut fiber: Fiber, mut pw: PendingWrite) {
        let resultado = match crate::builtins::socket_write_nb(pw.handle, &pw.remaining) {
            Ok(n) if n == pw.remaining.len() => Ok(()),
            Ok(n) => {
                pw.remaining.drain(..n); // descarta lo ya enviado y re-aparca el resto
                shared.io_parked.push(IoParked { fd, fiber, pending_write: Some(pw) });
                return;
            }
            Err(e) => Err(e),
        };
        let elems = match resultado {
            Ok(()) => vec![HeapValue::Str("ok".to_string()), HeapValue::Str(String::new())],
            Err(e) => vec![HeapValue::Str("err".to_string()), HeapValue::Str(e)],
        };
        // M38.1b-2: el resultado se aloja en el heap de la fibra que se despierta (no el de la actual).
        let h = fiber.heap.allocate(Obj::Array(elems));
        fiber.stack.push(HeapValue::Obj(h));
        shared.ready.push_back(fiber);
    }

    /// Una fibra terminó **con éxito**. Si es `main` → fin del programa (su valor; semántica Go). Si es una
    /// fibra `spawn` → escribe el resultado en su `Task` (M12.3), despierta a los que la unen y planifica la
    /// siguiente. Devuelve `Some(v)` si el programa termina, `None` si ya cargó otra fibra.
    /// M38.3b paso 3: método `&mut self`. `main` → `Some(v)` (fin del programa). Fibra hija → escribe su
    /// `Task` (Done), despierta a los que la unen (bajo el guard, ANTES de decrementar `running` → cuando
    /// otro worker vea `running`, la fibra despertada ya está en `ready`), suelta el lock y carga la siguiente.
    fn on_fiber_done(&mut self, result: HeapValue) -> Result<Option<HeapValue>, RuntimeError> {
        if self.cur.is_main {
            return Ok(Some(result));
        }
        {
            let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
            if let Some(task) = self.cur.task.take() {
                // M38.1b-2: el resultado vive en el heap de ESTA fibra (que se descarta al terminar) → se
                // transfiere al heap de la tarea, donde `join` lo recogerá.
                let mut t_heap = std::mem::take(&mut sh.tasks[task].heap);
                let r2 = transfer_value(&self.cur.heap, &mut t_heap, &result, &mut HashMap::new());
                sh.tasks[task].heap = t_heap;
                sh.tasks[task].state = TaskState::Done(r2);
                Self::wake_task_waiters(&mut sh, task);
            }
            sh.running -= 1; // esta fibra terminó → este worker queda ocioso
        }
        self.cur.scopes.clear();
        if !self.poll_next(0, 0)? { self.stop = true; }
        Ok(None)
    }

    /// Una fibra hija **falló** (M12.3): guarda el fallo en su `Task` como `Failed`, despierta a los que la
    /// unen (que lo re-lanzarán) y planifica la siguiente. El error solo se pierde si la tarea no la une
    /// nadie ni la posee un `scope`. M38.3b paso 3: método `&mut self` (park bajo guard + `poll_next`).
    fn fail_current_fiber(&mut self, e: RuntimeError) -> Result<(), RuntimeError> {
        {
            let mut sh = self.shared.lock().expect("el Mutex del scheduler no debería estar envenenado");
            if let Some(task) = self.cur.task.take() {
                let msg = e.msg.clone(); // solo el mensaje; el join que lo observe le pone su propia posición
                sh.tasks[task].state = TaskState::Failed(msg);
                Self::wake_task_waiters(&mut sh, task);
            }
            // M12.5: si esta fibra poseía tareas (scopes activos cuyo cuerpo hizo panic), cancélalas en vez de
            // dejarlas huérfanas. (En `main` el programa aborta, así que esto importa para fibras hijas.)
            let orphans: Vec<usize> = self.cur.scopes.iter().flat_map(|s| s.children.iter().copied()).collect();
            for c in orphans {
                Self::cancel_task(&mut sh, c);
            }
            sh.running -= 1; // esta fibra terminó (con fallo) → este worker queda ocioso
        }
        self.cur.frames.clear();
        self.cur.stack.clear();
        self.cur.scopes.clear();
        if !self.poll_next(e.line, e.col)? { self.stop = true; }
        Ok(())
    }

    /// M17: cuando nadie está listo pero hay fibras esperando E/S de red, espera **readiness real** del SO
    /// (`kqueue`/`epoll`): se bloquea hasta que algún socket esté listo para leer y despierta **solo** las
    /// fibras de esos descriptores. Si la plataforma no tiene poller (`Unsupported`) o la espera se
    /// interrumpe (`Ready` vacío por EINTR), cae al **busy-poll cooperativo** de M15.5 (duerme ~1 ms y
    /// re-encola todas) → siempre hay progreso. Garantiza dejar al menos una fibra en `ready`.
    fn io_wait(shared: &mut Shared) {
        // Cada fibra espera **lectura** (pending_write None) o **escritura** (Some) de su socket.
        let read_fds: Vec<i32> = shared.io_parked.iter().filter(|p| p.pending_write.is_none()).map(|p| p.fd).collect();
        let write_fds: Vec<i32> = shared.io_parked.iter().filter(|p| p.pending_write.is_some()).map(|p| p.fd).collect();
        if let crate::poll::PollResult::Ready(listos) = crate::poll::wait(&read_fds, &write_fds, -1)
            && !listos.is_empty()
        {
            // Saca las fibras cuyo socket quedó listo; las demás siguen aparcadas.
            let mut woken: Vec<IoParked> = Vec::new();
            let mut i = 0;
            while i < shared.io_parked.len() {
                if listos.contains(&shared.io_parked[i].fd) {
                    woken.push(shared.io_parked.remove(i));
                } else {
                    i += 1;
                }
            }
            Self::wake_parked(shared, woken);
            return;
        }
        // Respaldo (sin poller, o despertar vacío): busy-poll cooperativo de M15.5.
        crate::builtins::sleep_millis(1);
        let woken: Vec<IoParked> = shared.io_parked.drain(..).collect();
        Self::wake_parked(shared, woken);
    }

    /// Pone listas las fibras despertadas: las de lectura re-ejecutan su opcode (re-pushearon su handle);
    /// las de escritura terminan lo que faltaba (`finish_parked_write`).
    fn wake_parked(shared: &mut Shared, woken: Vec<IoParked>) {
        for p in woken {
            match p.pending_write {
                None => shared.ready.push_back(p.fiber),
                Some(pw) => Self::finish_parked_write(shared, p.fd, p.fiber, pw),
            }
        }
    }

    /// Despierta a todas las fibras aparcadas en `join` sobre `task` (M12.3): al re-ejecutar su `Join`/
    /// `ScopeEnd` verán la tarea ya terminada (Done/Failed). No empuja nada (el opcode rebobinó su `ip`).
    fn wake_task_waiters(shared: &mut Shared, task: Handle) {
        let mut i = 0;
        while i < shared.parked.len() {
            if shared.parked[i].on == task && matches!(shared.parked[i].waiting, Waiting::Join) {
                let parked = shared.parked.remove(i);
                shared.ready.push_back(parked.fiber);
            } else {
                i += 1;
            }
        }
    }

    /// Despierta a los `select` aparcados cuyo arreglo de canales contiene `chan`, porque acaba de pasar a
    /// estar listo para recibir (M12.4). Re-ejecutarán el `select` y verán el canal listo (o, si otro lo
    /// consumió antes, se volverán a bloquear). No empuja nada (el opcode rebobinó su `ip`).
    fn wake_select_waiters(shared: &mut Shared, chan: usize) {
        let mut i = 0;
        while i < shared.parked.len() {
            let on = shared.parked[i].on;
            let is_select = matches!(shared.parked[i].waiting, Waiting::Select);
            // M38.1b-2: el `on` de un Select es el handle del arreglo de canales, que vive en el heap de LA
            // FIBRA APARCADA (no en el de la fibra actual que dispara el wake). Sus elementos son
            // `HeapValue::Channel(id)`.
            let contains = is_select && match shared.parked[i].fiber.heap.get(on) {
                Obj::Array(elems) => elems.iter().any(|v| matches!(v, HeapValue::Channel(id) if *id == chan)),
                _ => false,
            };
            if contains {
                let parked = shared.parked.remove(i);
                shared.ready.push_back(parked.fiber);
            } else {
                i += 1;
            }
        }
    }

    /// Cancela una tarea **pendiente** (M12.5, structured concurrency): la marca `Failed`, **saca** su fibra
    /// de `ready`/`parked` (no se reanudará nunca; sus marcos/locales los reclama el GC) y cancela
    /// **recursivamente** los hijos de los scopes de esa fibra (cancelación transitiva: una hermana
    /// cancelada que era dueña de un scope no deja nietos huérfanos). Si la tarea ya terminó, no hace nada.
    /// Es trivial porque el scheduler es cooperativo M:1: una fibra solo corre en los puntos de yield, así
    /// que "cancelar" = "retirar de las colas". No es preemptiva: no interrumpe código que corra sin ceder.
    fn cancel_task(shared: &mut Shared, task: usize) {
        match &mut shared.tasks[task].state {
            estado @ TaskState::Pending => {
                *estado = TaskState::Failed("tarea cancelada (una hermana falló)".to_string());
            }
            _ => return, // ya terminó (Done/Failed) → nada que cancelar
        }
        let mut grandchildren: Vec<usize> = Vec::new();
        if let Some(pos) = shared.ready.iter().position(|f| f.task == Some(task)) {
            let f = shared.ready.remove(pos).unwrap();
            for s in &f.scopes {
                grandchildren.extend(s.children.iter().copied());
            }
        } else if let Some(pos) = shared.parked.iter().position(|p| p.fiber.task == Some(task)) {
            let p = shared.parked.remove(pos);
            for s in &p.fiber.scopes {
                grandchildren.extend(s.children.iter().copied());
            }
        } else if let Some(pos) = shared.io_parked.iter().position(|p| p.fiber.task == Some(task)) {
            // M15.5: la fibra cancelada podría estar esperando E/S de red.
            let p = shared.io_parked.remove(pos);
            for s in &p.fiber.scopes {
                grandchildren.extend(s.children.iter().copied());
            }
        }
        for g in grandchildren {
            Self::cancel_task(shared, g);
        }
    }

    /// Despierta una fibra bloqueada en `recv`: le deja `values` (envuelto en el `[T]` que devuelve el
    /// primitivo `__recv`) en su pila de operandos y la encola como lista. `[v]` la entrega un `send`; `[]`
    /// (vacío → `None`) la entrega un `close`.
    fn wake_recv(cur: &Fiber, shared: &mut Shared, mut fiber: Fiber, values: Vec<HeapValue>) {
        // M38.1b-2: los `values` vienen del heap de la fibra ACTUAL (el emisor en un rendezvous); se
        // transfieren al heap de la fibra que se despierta (el receptor) antes de alojar el `[T]` ahí.
        let mut remap = HashMap::new();
        let mut vals2 = Vec::with_capacity(values.len());
        for v in &values {
            vals2.push(transfer_value(&cur.heap, &mut fiber.heap, v, &mut remap));
        }
        let arr = fiber.heap.allocate(Obj::Array(vals2));
        fiber.stack.push(HeapValue::Obj(arr));
        shared.ready.push_back(fiber);
    }

    /// Despierta una fibra bloqueada en `send` (M12.2): su `send` ya quedó atrás (el `ip` apunta tras el
    /// ChanSend), así que solo le deja **unit** (el resultado de `send`) en la pila y la encola.
    fn wake_sender(shared: &mut Shared, mut fiber: Fiber) {
        fiber.stack.push(HeapValue::Unit);
        shared.ready.push_back(fiber);
    }

    /// Tras un `recv` que liberó un hueco: si hay un **emisor bloqueado** en `chan` (cola llena, M12.2),
    /// mete su valor pendiente en la cola (ahora hay sitio) y lo despierta. FIFO → el primero que se
    /// bloqueó despierta antes.
    fn wake_blocked_sender(shared: &mut Shared, chan: usize) {
        if let Some(pos) = shared.parked.iter().position(
            |p| p.on == chan && matches!(p.waiting, Waiting::Send(_)))
        {
            let parked = shared.parked.remove(pos);
            let sv = match parked.waiting {
                Waiting::Send(sv) => sv,
                _ => unreachable!(),
            };
            // M38.1b-2: el valor del emisor (heap de su fibra) entra a la cola → al heap del canal.
            let mut ch_heap = std::mem::take(&mut shared.channels[chan].heap);
            let sv2 = transfer_value(&parked.fiber.heap, &mut ch_heap, &sv, &mut HashMap::new());
            shared.channels[chan].heap = ch_heap;
            shared.channels[chan].queue.push_back(sv2);
            Self::wake_sender(shared, parked.fiber);
        }
    }

    // ----- Recolección de basura (mark-and-sweep) -----

    /// Recolecta: marca desde las raíces (pila + locales + upvalues de los marcos),
    /// propaga y barre. Solo se llama en puntos seguros del bucle.
    fn collect(&mut self) {
        // M37.1: cronometramos la pausa stop-the-world para medir el objetivo de M37 (pausas acotadas).
        // M44a: `Instant::now()` PANIQUEA en `wasm32-unknown-unknown` (sin reloj) → en wasm no se cronometra
        // (las stats del GC solo se imprimen con `RAYLANG_GC_STATS`, que no aplica en el playground).
        #[cfg(not(target_arch = "wasm32"))]
        let inicio = std::time::Instant::now();
        // Reunimos las raíces (handles) primero, para no tomar prestado `self.cur.stack`
        // y `self.cur.heap` a la vez. M12.1: además de la fibra en ejecución, rooteamos TODAS las fibras
        // (listas y bloqueadas) y los canales que esperan.
        // M38.1b-2: **heap-por-fibra**. Este `collect` recolecta SOLO el heap de la fibra en curso, cuyas
        // únicas raíces son sus propios marcos y pila (invariante de aislamiento: ningún objeto de este
        // heap lo alcanza otra fibra ni un canal/tarea — los valores que cruzan se TRANSFIEREN). Las demás
        // fibras tienen su propio heap (se recolecta cuando cada una corre); los valores en tránsito viven
        // en el heap del canal/tarea. Así la pausa la acota el tamaño del heap de una sola fibra.
        let mut roots: Vec<Handle> = Vec::new();
        gather_roots(&self.cur.frames, &self.cur.stack, &mut roots);

        for h in roots {
            self.cur.heap.mark(h);
        }
        self.cur.heap.trace();
        self.cur.heap.sweep();
        self.gc_count += 1;
        // M37.1: registra la pausa (una sola recolección stop-the-world). Solo fuera de wasm (ver arriba).
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dt = inicio.elapsed().as_nanos();
            self.gc_total_pause_ns += dt;
            if dt > self.gc_max_pause_ns {
                self.gc_max_pause_ns = dt;
            }
        }
    }

    /// M37.1: imprime las estadísticas de pausas del GC a stderr si `RAYLANG_GC_STATS` está en el entorno.
    /// Instrumentación de medición (arco B / M37): cuántas recolecciones y la pausa máxima/media.
    fn print_gc_stats_if_requested(&self) {
        if std::env::var_os("RAYLANG_GC_STATS").is_none() {
            return;
        }
        let media = if self.gc_count > 0 { self.gc_total_pause_ns / self.gc_count as u128 } else { 0 };
        eprintln!(
            "[gc] recolecciones={} pausa_max={:.3}ms pausa_media={:.3}ms total={:.3}ms",
            self.gc_count,
            self.gc_max_pause_ns as f64 / 1e6,
            media as f64 / 1e6,
            self.gc_total_pause_ns as f64 / 1e6,
        );
    }

    // ----- Locales (con boxing) -----

    /// Crea el arreglo de locales de un marco nuevo: cada slot capturado nace
    /// **boxeado** (su celda en el heap), los demás como `Plain(Unit)`.
    fn new_locals(&mut self, fn_idx: usize) -> Vec<Local> {
        let n = self.program.functions[fn_idx].num_locals;
        // Opt.2: reusa un `Vec` del pool (conserva su capacidad) en vez de asignar uno nuevo. Lo vaciamos
        // y lo reconstruimos entero, así no se lee ninguna basura que arrastrara del uso anterior.
        let mut locals = self.locals_pool.pop().unwrap_or_default();
        locals.clear();
        for s in 0..n {
            if self.program.functions[fn_idx].captured.get(s).copied().unwrap_or(false) {
                let cell = self.cur.heap.allocate(Obj::Cell(HeapValue::Unit));
                locals.push(Local::Boxed(cell));
            } else {
                locals.push(Local::Plain(HeapValue::Unit));
            }
        }
        locals
    }

    /// Opt.2: devuelve al pool el arreglo de locales de un marco que se descarta (Return, llamada en cola,
    /// fin de chunk). Acotado para no crecer sin límite; el GC no lo traza (contenido basura hasta reusar).
    fn recycle_locals(&mut self, locals: Vec<Local>) {
        if self.locals_pool.len() < 256 {
            self.locals_pool.push(locals);
        }
    }

    /// Coloca un argumento en un slot recién creado (respeta el boxing).
    fn put_arg(&mut self, locals: &mut [Local], slot: usize, v: HeapValue) {
        match &locals[slot] {
            Local::Boxed(h) => self.cell_set(*h, v),
            Local::Plain(_) => locals[slot] = Local::Plain(v),
        }
    }

    fn get_local(&self, fi: usize, slot: usize) -> HeapValue {
        match &self.cur.frames[fi].locals[slot] {
            Local::Plain(v) => v.clone(),
            Local::Boxed(h) => self.cell_get(*h),
        }
    }

    fn set_local(&mut self, fi: usize, slot: usize, v: HeapValue) {
        match &self.cur.frames[fi].locals[slot] {
            Local::Boxed(h) => {
                let h = *h;
                self.cell_set(h, v);
            }
            Local::Plain(_) => self.cur.frames[fi].locals[slot] = Local::Plain(v),
        }
    }

    fn cell_get(&self, h: Handle) -> HeapValue {
        match self.cur.heap.get(h) {
            Obj::Cell(v) => v.clone(),
            _ => unreachable!("se esperaba una celda"),
        }
    }

    fn cell_set(&mut self, h: Handle, v: HeapValue) {
        match self.cur.heap.get_mut(h) {
            Obj::Cell(slot) => *slot = v,
            _ => unreachable!("se esperaba una celda"),
        }
    }

    // ----- Acceso a objetos del heap -----

    fn as_array(&self, h: Handle) -> &Vec<HeapValue> {
        match self.cur.heap.get(h) {
            Obj::Array(v) => v,
            _ => unreachable!("el checker garantiza un arreglo"),
        }
    }

    fn as_array_mut(&mut self, h: Handle) -> &mut Vec<HeapValue> {
        match self.cur.heap.get_mut(h) {
            Obj::Array(v) => v,
            _ => unreachable!("el checker garantiza un arreglo"),
        }
    }

    fn as_struct(&self, h: Handle) -> &VmStruct {
        match self.cur.heap.get(h) {
            Obj::Struct(s) => s,
            _ => unreachable!("el checker garantiza un struct"),
        }
    }

    fn as_struct_mut(&mut self, h: Handle) -> &mut VmStruct {
        match self.cur.heap.get_mut(h) {
            Obj::Struct(s) => s,
            _ => unreachable!("el checker garantiza un struct"),
        }
    }

    fn as_enum(&self, h: Handle) -> &VmEnum {
        match self.cur.heap.get(h) {
            Obj::Enum(e) => e,
            _ => unreachable!("el checker garantiza un enum"),
        }
    }

    // ----- Pila de operandos -----

    fn push(&mut self, v: HeapValue) {
        self.cur.stack.push(v);
    }

    fn pop(&mut self) -> HeapValue {
        self.cur.stack.pop().expect("pila vacía: bytecode mal formado")
    }

    fn peek(&self) -> &HeapValue {
        self.cur.stack.last().expect("pila vacía: bytecode mal formado")
    }

    fn pop_int(&mut self) -> i64 {
        match self.pop() {
            HeapValue::Int(n) => n,
            _ => unreachable!("el checker garantiza un int"),
        }
    }

    fn pop_obj(&mut self) -> Handle {
        match self.pop() {
            HeapValue::Obj(h) => h,
            _ => unreachable!("el checker garantiza un objeto"),
        }
    }

    /// M38.1b: saca un id de canal (el checker garantiza un `Channel<T>`).
    fn pop_channel(&mut self) -> usize {
        match self.pop() {
            HeapValue::Channel(id) => id,
            _ => unreachable!("el checker garantiza un canal"),
        }
    }

    /// M38.1b: saca un id de tarea (el checker garantiza un `Task<T>`).
    fn pop_task(&mut self) -> usize {
        match self.pop() {
            HeapValue::Task(id) => id,
            _ => unreachable!("el checker garantiza una tarea"),
        }
    }

    /// Aplica un operador binario. Misma semántica que el intérprete de M1 (esa es la
    /// idea del oráculo: deben coincidir). La igualdad es **estructural** para los
    /// compuestos, por lo que necesita el heap.
    fn apply_binary(&self, op: &OpCode, left: HeapValue, right: HeapValue, line: usize, col: usize) -> Result<HeapValue, RuntimeError> {
        use HeapValue::*;
        use OpCode::*;
        // Igualdad: estructural, mirando el heap.
        match op {
            Equal => return Ok(Bool(values_equal(&self.cur.heap, &left, &right))),
            NotEqual => return Ok(Bool(!values_equal(&self.cur.heap, &left, &right))),
            _ => {}
        }
        Ok(match (op, left, right) {
            // M11.1a: `+` concatena dos strings.
            (Add, Str(a), Str(b)) => Str(a + &b),
            // M16.1b: `+` concatena dos bytes (inline, no son objetos del heap → van por aquí).
            (Add, Bytes(a), Bytes(b)) => {
                let mut v = a;
                v.extend_from_slice(&b);
                Bytes(v)
            }
            // Desbordamiento de int = error de ejecución (M34, SPEC §8), como en el intérprete.
            (Add, Int(a), Int(b)) => Int(a.checked_add(b).ok_or_else(|| runtime_error(line, col, "desbordamiento aritmético en int"))?),
            (Sub, Int(a), Int(b)) => Int(a.checked_sub(b).ok_or_else(|| runtime_error(line, col, "desbordamiento aritmético en int"))?),
            (Mul, Int(a), Int(b)) => Int(a.checked_mul(b).ok_or_else(|| runtime_error(line, col, "desbordamiento aritmético en int"))?),
            (Div, Int(a), Int(b)) => {
                if b == 0 {
                    return Err(runtime_error(line, col, "división entera por cero"));
                }
                Int(a.checked_div(b).ok_or_else(|| runtime_error(line, col, "desbordamiento aritmético en int"))?)
            }
            (Rem, Int(a), Int(b)) => {
                if b == 0 {
                    return Err(runtime_error(line, col, "módulo por cero"));
                }
                Int(a.checked_rem(b).ok_or_else(|| runtime_error(line, col, "desbordamiento aritmético en int"))?)
            }
            (Add, Float(a), Float(b)) => Float(a + b),
            (Sub, Float(a), Float(b)) => Float(a - b),
            (Mul, Float(a), Float(b)) => Float(a * b),
            (Div, Float(a), Float(b)) => Float(a / b),
            (Rem, Float(a), Float(b)) => Float(a % b),
            (Less, Int(a), Int(b)) => Bool(a < b),
            (LessEqual, Int(a), Int(b)) => Bool(a <= b),
            (Greater, Int(a), Int(b)) => Bool(a > b),
            (GreaterEqual, Int(a), Int(b)) => Bool(a >= b),
            (Less, Float(a), Float(b)) => Bool(a < b),
            (LessEqual, Float(a), Float(b)) => Bool(a <= b),
            (Greater, Float(a), Float(b)) => Bool(a > b),
            (GreaterEqual, Float(a), Float(b)) => Bool(a >= b),
            // M11.7d: orden de strings (lexicográfico) y char (por code point).
            (Less, Str(a), Str(b)) => Bool(a < b),
            (LessEqual, Str(a), Str(b)) => Bool(a <= b),
            (Greater, Str(a), Str(b)) => Bool(a > b),
            (GreaterEqual, Str(a), Str(b)) => Bool(a >= b),
            (Less, Char(a), Char(b)) => Bool(a < b),
            (LessEqual, Char(a), Char(b)) => Bool(a <= b),
            (Greater, Char(a), Char(b)) => Bool(a > b),
            (GreaterEqual, Char(a), Char(b)) => Bool(a >= b),
            // M28.3: enteros sin signo con tamaño. Mismo ancho garantizado por el checker; wrapping
            // dentro del ancho (`crate::runtime::uint_mask`), idéntico al intérprete.
            (Add, UInt(a, w), UInt(b, _)) => uint_heap(a.wrapping_add(b), w),
            (Sub, UInt(a, w), UInt(b, _)) => uint_heap(a.wrapping_sub(b), w),
            (Mul, UInt(a, w), UInt(b, _)) => uint_heap(a.wrapping_mul(b), w),
            (Div, UInt(a, w), UInt(b, _)) => {
                if b == 0 { return Err(runtime_error(line, col, "división entera por cero")); }
                uint_heap(a / b, w)
            }
            (Rem, UInt(a, w), UInt(b, _)) => {
                if b == 0 { return Err(runtime_error(line, col, "módulo por cero")); }
                uint_heap(a % b, w)
            }
            (Less, UInt(a, _), UInt(b, _)) => Bool(a < b),
            (LessEqual, UInt(a, _), UInt(b, _)) => Bool(a <= b),
            (Greater, UInt(a, _), UInt(b, _)) => Bool(a > b),
            (GreaterEqual, UInt(a, _), UInt(b, _)) => Bool(a >= b),
            (BitAnd, UInt(a, w), UInt(b, _)) => uint_heap(a & b, w),
            (BitOr, UInt(a, w), UInt(b, _)) => uint_heap(a | b, w),
            (BitXor, UInt(a, w), UInt(b, _)) => uint_heap(a ^ b, w),
            (Shl, UInt(a, w), UInt(b, _)) => uint_heap(a.wrapping_shl(b as u32), w),
            (Shr, UInt(a, w), UInt(b, _)) => uint_heap(a.wrapping_shr(b as u32), w),
            _ => unreachable!("combinación operador/operandos que el checker debió rechazar"),
        })
    }
}

/// M28.3: construye un `HeapValue::UInt` enmascarando al ancho (aplica el wrapping), como
/// `make_uint` del intérprete.
fn uint_heap(val: u64, width: u8) -> HeapValue {
    HeapValue::UInt(val & crate::runtime::uint_mask(width), width)
}

/// Convierte una constante del chunk (un `Value` del intérprete, siempre primitivo)
/// al valor de la VM.
fn const_to_heap(v: &Value) -> HeapValue {
    match v {
        Value::Int(n) => HeapValue::Int(*n),
        Value::Float(x) => HeapValue::Float(*x),
        Value::Bool(b) => HeapValue::Bool(*b),
        Value::Str(s) => HeapValue::Str(s.clone()),
        Value::Char(c) => HeapValue::Char(*c),
        Value::UInt(n, w) => HeapValue::UInt(*n, *w), // M28.3
        Value::Bytes(b) => HeapValue::Bytes((**b).clone()),
        Value::Unit => HeapValue::Unit,
        _ => unreachable!("las constantes del chunk son primitivas"),
    }
}

/// Igualdad estructural entre valores de la VM (mira el heap). Las funciones y
/// closures se comparan por identidad (el checker prohíbe `==` sobre ellas).
fn values_equal(heap: &Heap, a: &HeapValue, b: &HeapValue) -> bool {
    use HeapValue as H;
    match (a, b) {
        (H::Int(x), H::Int(y)) => x == y,
        (H::Float(x), H::Float(y)) => x == y,
        (H::Bool(x), H::Bool(y)) => x == y,
        (H::Str(x), H::Str(y)) => x == y,
        (H::Char(x), H::Char(y)) => x == y,
        (H::UInt(x, _), H::UInt(y, _)) => x == y, // M28.3 (mismo ancho garantizado por el checker)
        (H::Bytes(x), H::Bytes(y)) => x == y,
        (H::Ptr(x), H::Ptr(y)) => x == y, // M41.4b: identidad de puntero
        (H::Unit, H::Unit) => true,
        (H::Function(x), H::Function(y)) => x == y,
        (H::Obj(x), H::Obj(y)) => match (heap.get(*x), heap.get(*y)) {
            (Obj::Array(va), Obj::Array(vb)) => {
                va.len() == vb.len() && va.iter().zip(vb).all(|(p, q)| values_equal(heap, p, q))
            }
            (Obj::Struct(sa), Obj::Struct(sb)) => {
                sa.name == sb.name
                    && sa.fields.len() == sb.fields.len()
                    && sa.fields.iter().zip(&sb.fields).all(|((n1, v1), (n2, v2))| n1 == n2 && values_equal(heap, v1, v2))
            }
            // Closures: identidad (mismo handle).
            (Obj::Closure(_), Obj::Closure(_)) => x == y,
            _ => false,
        },
        _ => false,
    }
}

/// Resuelve `(enum_id, tag)` de un enum a `(nombre_enum, nombre_variante)` usando la
/// tabla de enums del programa.
fn enum_names<'a>(enums: &'a [CompiledEnum], enum_id: usize, tag: usize) -> (&'a str, &'a str) {
    let e = &enums[enum_id];
    (e.name.as_str(), e.variants[tag].name.as_str())
}

/// Formatea un valor de la VM como texto (siguiendo handles en el heap). Debe
/// coincidir con el `Display` del `Value` del intérprete, para que `print` sea igual.
fn format_value(heap: &Heap, enums: &[CompiledEnum], v: &HeapValue) -> String {
    match v {
        HeapValue::Int(n) => n.to_string(),
        HeapValue::Float(x) => x.to_string(),
        HeapValue::Bool(b) => b.to_string(),
        HeapValue::Str(s) => s.clone(),
        HeapValue::Char(c) => c.to_string(),
        HeapValue::UInt(n, _) => n.to_string(),
        HeapValue::Bytes(b) => crate::builtins::bytes_to_hex(b),
        HeapValue::Ptr(_) => "<ptr>".to_string(), // M41.4b: dirección no determinista → repr opaca
        HeapValue::Unit => "()".to_string(),
        HeapValue::Function(_) => "<fn>".to_string(),
        HeapValue::Obj(h) => match heap.get(*h) {
            Obj::Array(elems) => {
                let parts: Vec<String> = elems.iter().map(|e| format_value(heap, enums, e)).collect();
                format!("[{}]", parts.join(", "))
            }
            Obj::Struct(s) => {
                let parts: Vec<String> = s.fields.iter().map(|(n, v)| format!("{}: {}", n, format_value(heap, enums, v))).collect();
                format!("{} {{ {} }}", s.name, parts.join(", "))
            }
            Obj::Enum(e) => {
                let (ename, vname) = enum_names(enums, e.enum_id, e.tag);
                if e.payload.is_empty() {
                    format!("{}.{}", ename, vname)
                } else {
                    let parts: Vec<String> = e.payload.iter().map(|v| format_value(heap, enums, v)).collect();
                    format!("{}.{}({})", ename, vname, parts.join(", "))
                }
            }
            Obj::Closure(_) => "<fn>".to_string(),
            Obj::Cell(_) => "<cell>".to_string(), // no debería imprimirse directamente
            // M13.1: el print de un Map está diferido; se ordena por clave (determinista).
            Obj::Map(m) => {
                let mut parts: Vec<String> = m.iter()
                    .map(|(k, v)| format!("{}: {}", k.to_value(), format_value(heap, enums, v)))
                    .collect();
                parts.sort();
                format!("Map{{{}}}", parts.join(", "))
            }
        },
        // M38.1b: canal/tarea (host) no se inspeccionan textualmente.
        HeapValue::Channel(_) => "<channel>".to_string(),
        HeapValue::Task(_) => "<task>".to_string(),
    }
}

/// Convierte un valor de la VM al `Value` del intérprete (para el resultado final y
/// el oráculo). Los compuestos se reconstruyen siguiendo el heap.
fn to_value(heap: &Heap, enums: &[CompiledEnum], v: &HeapValue) -> Value {
    match v {
        HeapValue::Int(n) => Value::Int(*n),
        HeapValue::Float(x) => Value::Float(*x),
        HeapValue::Bool(b) => Value::Bool(*b),
        HeapValue::Str(s) => Value::Str(s.clone()),
        HeapValue::Char(c) => Value::Char(*c),
        HeapValue::UInt(n, w) => Value::UInt(*n, *w),
        HeapValue::Bytes(b) => Value::Bytes(Rc::new(b.clone())),
        HeapValue::Ptr(p) => Value::Ptr(*p), // M41.4b
        HeapValue::Unit => Value::Unit,
        HeapValue::Function(i) => Value::Function(*i),
        HeapValue::Obj(h) => match heap.get(*h) {
            Obj::Array(elems) => {
                let v: Vec<Value> = elems.iter().map(|e| to_value(heap, enums, e)).collect();
                Value::Array(Rc::new(RefCell::new(v)))
            }
            Obj::Struct(s) => {
                let fields: Vec<(String, Value)> = s.fields.iter().map(|(n, v)| (n.clone(), to_value(heap, enums, v))).collect();
                Value::Struct(Rc::new(RefCell::new(StructInstance { name: s.name.clone(), fields })))
            }
            Obj::Enum(e) => {
                let (ename, vname) = enum_names(enums, e.enum_id, e.tag);
                let payload: Vec<Value> = e.payload.iter().map(|v| to_value(heap, enums, v)).collect();
                Value::Enum(Rc::new(EnumInstance {
                    enum_name: ename.to_string(),
                    variant: vname.to_string(),
                    payload,
                }))
            }
            // Una closure como resultado: la representamos como función (su identidad
            // no se observa; se imprime <fn>).
            Obj::Closure(c) => Value::Function(c.index),
            Obj::Cell(inner) => to_value(heap, enums, inner),
            // M13.1: reconstruye el Map del intérprete (igual igualdad estructural → oráculo).
            Obj::Map(m) => {
                let mut hm: HashMap<MapKey, Value> = HashMap::with_capacity(m.len());
                for (k, val) in m {
                    hm.insert(k.clone(), to_value(heap, enums, val));
                }
                Value::Map(Rc::new(RefCell::new(hm)))
            }
        },
        // M38.1b: un canal/tarea (host) nunca es el resultado del programa ni cruza al intérprete
        // (main devuelve int/unit; no hay oráculo concurrente).
        HeapValue::Channel(_) => unreachable!("un canal nunca es el resultado del programa"),
        HeapValue::Task(_) => unreachable!("una tarea nunca es el resultado del programa"),
    }
}

// ===================== M38.1a — transferencia de subgrafo entre heaps =====================
//
// El ladrillo del aislamiento por actores (§46): copiar el subgrafo de un valor de un heap a otro,
// remapeando los handles. Lo usará `send`/`recv`/`spawn` cuando cada fibra tenga su heap (M38.1b): un
// valor que cruza de un actor a otro se **re-aloja** en el heap destino. Aquí se construye y prueba en
// AISLAMIENTO (dos heaps sueltos), sin tocar el VM en marcha.
//
// Correcto ante **sharing y ciclos**: un `remap: old_handle → new_handle` memoiza los objetos ya
// copiados, así (a) el sharing interno del subgrafo se preserva (un objeto alcanzado por dos caminos se
// copia una vez) y (b) los ciclos (las closures capturan celdas → grafos cíclicos) no ciclan al copiar.
// Para cerrar el ciclo se **reserva primero** un placeholder en el destino y se registra el mapeo ANTES
// de copiar los hijos, de modo que una referencia de vuelta encuentre el handle nuevo.
//
// `Channel`/`Task` NO se transfieren: son la sincronización **compartida** entre actores (viven fuera del
// heap de cualquier fibra; §46.2). Aquí es un caso inalcanzable (los valores que cruzan son datos).

/// Transfiere `v` del heap `src` al heap `dst`, devolviendo el valor equivalente con handles del destino.
/// Los primitivos se copian tal cual; los objetos se re-alojan (ver `transfer_obj`).
pub fn transfer_value(
    src: &Heap,
    dst: &mut Heap,
    v: &HeapValue,
    remap: &mut HashMap<Handle, Handle>,
) -> HeapValue {
    match v {
        HeapValue::Obj(h) => HeapValue::Obj(transfer_obj(src, dst, *h, remap)),
        // Escalares inline (Int/Float/Bool/Str/Char/UInt/Bytes/Ptr/Unit/Function): copia directa.
        otro => otro.clone(),
    }
}

/// Re-aloja el objeto `h` de `src` en `dst`, recursivamente. Reserva un placeholder + registra el mapeo
/// antes de copiar los hijos (para ciclos), y memoiza (para sharing).
fn transfer_obj(src: &Heap, dst: &mut Heap, h: Handle, remap: &mut HashMap<Handle, Handle>) -> Handle {
    if let Some(&nh) = remap.get(&h) {
        return nh; // ya copiado (sharing o ciclo) → reusa el handle destino
    }
    // Reserva un placeholder y registra el mapeo ANTES de recursar (cierra los ciclos).
    let nh = dst.allocate(Obj::Array(Vec::new()));
    remap.insert(h, nh);
    // Se **clona** la estructura del objeto origen para soltar el préstamo de `src` antes de transferir
    // los hijos (que mutan `dst`). El clon copia los `HeapValue` hijos (baratos salvo Str/Bytes); sus
    // handles se remapean al transferirlos.
    let nuevo: Obj = match src.get(h) {
        Obj::Array(elems) => {
            let elems = elems.clone();
            Obj::Array(elems.iter().map(|e| transfer_value(src, dst, e, remap)).collect())
        }
        Obj::Struct(s) => {
            let name = s.name.clone();
            let fields = s.fields.clone();
            Obj::Struct(VmStruct {
                name,
                fields: fields.iter().map(|(n, e)| (n.clone(), transfer_value(src, dst, e, remap))).collect(),
            })
        }
        Obj::Enum(e) => {
            let (enum_id, tag) = (e.enum_id, e.tag);
            let payload = e.payload.clone();
            Obj::Enum(VmEnum {
                enum_id,
                tag,
                payload: payload.iter().map(|e| transfer_value(src, dst, e, remap)).collect(),
            })
        }
        Obj::Closure(c) => {
            let index = c.index;
            let upvalues = c.upvalues.clone(); // handles a celdas
            Obj::Closure(VmClosure {
                index,
                upvalues: upvalues.iter().map(|&up| transfer_obj(src, dst, up, remap)).collect(),
            })
        }
        Obj::Cell(inner) => {
            let inner = inner.clone();
            Obj::Cell(transfer_value(src, dst, &inner, remap))
        }
        Obj::Map(m) => {
            let pares: Vec<(MapKey, HeapValue)> = m.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let mut nm: HashMap<MapKey, HeapValue> = HashMap::with_capacity(pares.len());
            for (k, val) in pares {
                let nv = transfer_value(src, dst, &val, remap);
                nm.insert(k, nv); // las claves son primitivos (sin handles)
            }
            Obj::Map(nm)
        }
    };
    *dst.get_mut(nh) = nuevo;
    nh
}

fn runtime_error(line: usize, col: usize, msg: &str) -> RuntimeError {
    RuntimeError { msg: msg.to_string(), line, col }
}

/// Localiza la variante `variant` del enum `Option` del prelude en la tabla compilada, devolviendo
/// `(enum_id, tag)` para armar un `VmEnum`. Lo usa el retorno FFI `char*` → `Option` (M41.3).
fn option_variant(enums: &[crate::bytecode::CompiledEnum], variant: &str) -> Option<(usize, usize)> {
    let ei = enums.iter().position(|e| e.name == "Option")?;
    let tag = enums[ei].variants.iter().position(|v| v.name == variant)?;
    Some((ei, tag))
}

/// Reúne las raíces del GC (handles) de una fibra: los valores en su pila de operandos y, por cada marco,
/// sus locales (los `Boxed` son celdas del heap) y sus upvalues. Compartida por la fibra en ejecución y
/// las suspendidas (M12.1). Función libre para no tomar prestado `self` entero durante la recolección.
fn gather_roots(frames: &[CallFrame], stack: &[HeapValue], roots: &mut Vec<Handle>) {
    for v in stack {
        if let Some(h) = v.handle() {
            roots.push(h);
        }
    }
    for frame in frames {
        for slot in &frame.locals {
            match slot {
                Local::Plain(v) => {
                    if let Some(h) = v.handle() {
                        roots.push(h);
                    }
                }
                Local::Boxed(h) => roots.push(*h),
            }
        }
        roots.extend(frame.upvalues.iter().copied());
    }
}

/// Reúne las raíces de una fibra suspendida o lista (M12.3): su pila/marcos, su `Task` y los hijos de sus
/// scopes activos (las tareas que aún no ha unido).
// M38.1b-2: `gather_fiber_roots` se eliminó — con heap-por-fibra, `collect` solo rootea el heap de la
// fibra en curso (sus marcos/pila); las demás fibras recolectan su propio heap cuando corren.

/// Convierte un valor de la VM en una clave de Map (M13.1). El checker garantiza el tipo.
fn heap_to_key(v: &HeapValue) -> MapKey {
    match v {
        HeapValue::Int(n) => MapKey::Int(*n),
        HeapValue::Str(s) => MapKey::Str(s.clone()),
        HeapValue::Char(c) => MapKey::Char(*c),
        HeapValue::Bool(b) => MapKey::Bool(*b),
        HeapValue::Bytes(b) => MapKey::Bytes(b.clone()),
        _ => unreachable!("el checker garantiza una clave hashable (int/string/char/bool/bytes)"),
    }
}

/// Reconstruye el valor de la VM a partir de una clave de Map (para `keys`, M13.1b).
fn key_to_heap(k: &MapKey) -> HeapValue {
    match k {
        MapKey::Int(n) => HeapValue::Int(*n),
        MapKey::Str(s) => HeapValue::Str(s.clone()),
        MapKey::Char(c) => HeapValue::Char(*c),
        MapKey::Bool(b) => HeapValue::Bool(*b),
        MapKey::Bytes(b) => HeapValue::Bytes(b.clone()),
    }
}

/// Comprueba que `i` es un índice válido en `0..len`; si no, error de ejecución.
fn bounds_check(i: i64, len: usize, line: usize, col: usize) -> Result<usize, RuntimeError> {
    if i < 0 || (i as usize) >= len {
        return Err(runtime_error(line, col, &format!("índice {} fuera de rango (longitud {})", i, len)));
    }
    Ok(i as usize)
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Expr;
    use crate::compiler::{compile_expr, compile_program};

    fn expr_of(src: &str) -> Expr {
        let prog_src = format!("fn v() {{ {} }}", src);
        let tokens = crate::lexer::lex(&prog_src).expect("lex ok");
        let prog = crate::parser::parse(tokens).expect("parse ok");
        *prog.functions[0].body.tail.clone().expect("expresión en posición tail")
    }

    fn run_vm(src: &str) -> Value {
        let chunk = compile_expr(&expr_of(src)).expect("compila");
        run(&chunk).expect("ejecuta sin error")
    }

    /// Oráculo a nivel de expresión (int): VM vs intérprete.
    fn oracle_int(src: &str) {
        let prog_src = format!("fn main() -> int {{ {} }}", src);
        let tokens = crate::lexer::lex(&prog_src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("intérprete ok");
        // La VM ejecuta el programa **ya chequeado** (no la expresión cruda): así se
        // aplican las bajadas del checker —UFCS/métodos— que la forma de método de los
        // builtins de contenedor (`s.len()`, `b.sub_bytes(...)`) necesita para compilar.
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect("vm ok");
        assert_eq!(interp, vm, "VM y intérprete difieren en `{}`", src);
    }

    /// **El oráculo a nivel de programa completo**: compila y ejecuta el programa
    /// en la VM y en el intérprete, y exige que el resultado coincida.
    fn oracle_program(src: &str) {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("intérprete ok");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect("vm ok");
        assert_eq!(interp, vm, "VM y intérprete difieren");
    }

    /// M41: **FFI**. Llamar a funciones C nativas (libm/libc) por `dlopen`/`dlsym`. Determinista
    /// (sqrt/pow/abs dan lo mismo siempre) → el oráculo VM↔intérprete vale: ambos motores llaman a la
    /// MISMA función C y deben coincidir. Cubre float→float, aridad 2, e int→int (libc `abs`).
    #[test]
    fn ffi_libm_oraculo() {
        oracle_program(
            "extern \"m\" {\n\
             \x20 fn sqrt(x: float) -> float;\n\
             \x20 fn pow(base: float, exp: float) -> float;\n\
             }\n\
             extern \"c\" {\n\
             \x20 fn abs(n: int) -> int;\n\
             }\n\
             fn main() -> int {\n\
             \x20 if (sqrt(16.0) == 4.0 && pow(2.0, 10.0) == 1024.0 && abs(0 - 5) == 5) { 42 } else { 0 }\n\
             }",
        );
    }

    /// M41.2: **FFI con string/bytes** → `char*`. Un `string` se marshala a una `CString`
    /// NUL-terminada; un `bytes` se pasa por el puntero de su buffer. Determinista (strlen/atoi) →
    /// oráculo. Programas separados porque el nombre extern ES el símbolo (un `strlen` por programa).
    #[test]
    fn ffi_strings_oraculo() {
        // string → char*: strlen y atoi.
        oracle_program(
            "extern \"c\" {\n\
             \x20 fn strlen(s: string) -> int;\n\
             \x20 fn atoi(s: string) -> int;\n\
             }\n\
             fn main() -> int {\n\
             \x20 if (strlen(\"hola mundo\") == 10 && atoi(\"42\") == 42 && atoi(\"  -7x\") == 0 - 7) { 1 } else { 0 }\n\
             }",
        );
        // bytes → puntero al buffer (NUL-terminado a mano con un literal de bytes).
        oracle_program(
            "extern \"c\" {\n\
             \x20 fn strlen(s: bytes) -> int;\n\
             }\n\
             fn main() -> int {\n\
             \x20 if (strlen(b\"abcde\\x00\") == 5) { 1 } else { 0 }\n\
             }",
        );
    }

    /// M41.3: **FFI con retorno `char*`** → `Option<bytes>`/`Option<string>`. `strstr` es determinista
    /// (devuelve un puntero DENTRO del argumento, o NULL si no encuentra) → oráculo. Some/None + el
    /// azúcar de string, en ambos motores.
    #[test]
    fn ffi_char_ptr_return_oraculo() {
        // Option<string>: encontrado → Some("world"); no encontrado → None.
        oracle_program(
            "extern \"c\" { fn strstr(h: string, n: string) -> Option<string>; }\n\
             fn d(o: Option<string>) -> int {\n\
             \x20 match (o) { Option.Some(s) => s.len(), Option.None => 0 - 1 }\n\
             }\n\
             fn main() -> int {\n\
             \x20 d(strstr(\"hello world\", \"world\")) * 10 + (d(strstr(\"abc\", \"z\")) + 1)\n\
             }",
        ); // "world"→len 5 ⇒ 50; no encontrado→-1 ⇒ +0 ⇒ 50
        // Option<bytes>: primitiva cruda.
        oracle_program(
            "extern \"c\" { fn strstr(h: string, n: string) -> Option<bytes>; }\n\
             fn main() -> int {\n\
             \x20 match (strstr(\"raylang\", \"lang\")) { Option.Some(b) => b.len(), Option.None => 0 }\n\
             }",
        ); // "lang" ⇒ len 4
    }

    /// M40.1a: **guardas** en los brazos del match (`patrón if <cond> => …`). El brazo casa solo si
    /// el patrón liga Y la guarda es true; si no, se sigue al siguiente. Oráculo VM↔intérprete.
    #[test]
    fn guardas_oraculo() {
        // clasificar por rango: 3=grande, 2=positivo, 1=neg/cero, 0=nada. Un dígito por caso.
        let prog = "\
            fn c(o: Option<int>) -> int {\n\
            \x20 match (o) {\n\
            \x20   Option.Some(n) if n > 100 => 3,\n\
            \x20   Option.Some(n) if n > 0 => 2,\n\
            \x20   Option.Some(n) => 1,\n\
            \x20   Option.None => 0,\n\
            \x20 }\n\
            }\n\
            fn main() -> int {\n\
            \x20 c(Option.Some(500)) * 1000 + c(Option.Some(7)) * 100 + c(Option.Some(0 - 5)) * 10 + c(Option.None)\n\
            }";
        oracle_program(prog); // ambos motores → 3210
        // Guarda sobre un binding catch-all (no sobre una variante), y fallback tras ella.
        oracle_program("\
            fn f(o: Option<int>) -> int { match (o) { x if false => 9, _ => 1 } }\n\
            fn main() -> int { f(Option.Some(5)) + f(Option.None) }"); // 2
        // Guarda que usa **UFCS** (`xs.len()`): debe pasar por el lowering (M40.1a: los pases bajan
        // también la guarda, no solo el cuerpo).
        oracle_program("\
            fn g(o: Option<[int]>) -> int { match (o) { Option.Some(xs) if xs.len() > 2 => 1, Option.Some(xs) => 0, Option.None => 0 - 1 } }\n\
            fn main() -> int { g(Option.Some([1, 2, 3])) * 100 + (g(Option.Some([9])) + 5) * 10 }"); // 150
    }

    /// M40.1b: `if let <patrón> = <expr> { … } else { … }` — azúcar del parser a un match de dos
    /// brazos. Oráculo VM↔intérprete: expresión (con else) y statement (sin else).
    #[test]
    fn if_let_oraculo() {
        // Expresión: `if let Some(v) = o { v } else { def }`.
        oracle_program("\
            fn vo(o: Option<int>, def: int) -> int { if let Option.Some(v) = o { v } else { def } }\n\
            fn main() -> int { vo(Option.Some(42), 0) * 100 + vo(Option.None, 7) }"); // 4207
        // Statement (sin else): solo actúa si el patrón casa.
        oracle_program("\
            fn main() -> int {\n\
            \x20 var s = 0;\n\
            \x20 if let Option.Some(n) = Option.Some(10) { s = s + n; }\n\
            \x20 let nada: Option<int> = Option.None;\n\
            \x20 if let Option.Some(n) = nada { s = s + 1000; }\n\
            \x20 s\n\
            }"); // 10
    }

    /// M40.1c: **patrones de variante anidados** (`Result.Ok(Option.Some(v))`). Exhaustividad
    /// conservadora → hace falta un fallback (`Ok(_)`). Oráculo VM↔intérprete (test + codegen).
    #[test]
    fn patrones_anidados_oraculo() {
        // Result<Option<int>, string>: cada caso a un dígito.
        oracle_program("\
            fn d(r: Result<Option<int>, string>) -> int {\n\
            \x20 match (r) {\n\
            \x20   Result.Ok(Option.Some(v)) => v,\n\
            \x20   Result.Ok(_) => 100,\n\
            \x20   Result.Err(e) => 200,\n\
            \x20 }\n\
            }\n\
            fn main() -> int {\n\
            \x20 let a: Result<Option<int>, string> = Result.Ok(Option.Some(42));\n\
            \x20 let b: Result<Option<int>, string> = Result.Ok(Option.None);\n\
            \x20 let c: Result<Option<int>, string> = Result.Err(\"x\");\n\
            \x20 d(a) + d(b) + d(c)\n\
            }"); // 42 + 100 + 200 = 342
        // Option<Option<int>> con un segundo nivel de anidamiento.
        oracle_program("\
            fn f(o: Option<Option<int>>) -> int {\n\
            \x20 match (o) { Option.Some(Option.Some(n)) => n, Option.Some(_) => 100, Option.None => 200 }\n\
            }\n\
            fn main() -> int {\n\
            \x20 let x: Option<Option<int>> = Option.Some(Option.Some(7));\n\
            \x20 let z: Option<Option<int>> = Option.None;\n\
            \x20 f(x) + f(z)\n\
            }"); // 7 + 200 = 207
    }

    /// M40.1d: **patrón de struct** (`Some(Punto { x, y })`). El struct irrefutable cubre la variante
    /// sin fallback. Oráculo VM↔intérprete (destructuración + campo con sub-patrón/`_`).
    #[test]
    fn patrones_struct_oraculo() {
        oracle_program("\
            struct Punto { x: int, y: int }\n\
            fn f(o: Option<Punto>) -> int {\n\
            \x20 match (o) {\n\
            \x20   Option.Some(Punto { x, y }) if x > 0 => x + y,\n\
            \x20   Option.Some(Punto { x: n, y: _ }) => n,\n\
            \x20   Option.None => 0 - 1,\n\
            \x20 }\n\
            }\n\
            fn main() -> int {\n\
            \x20 let a = Option.Some(Punto { x: 3, y: 4 });\n\
            \x20 let b = Option.Some(Punto { x: 0 - 9, y: 0 });\n\
            \x20 let c: Option<Punto> = Option.None;\n\
            \x20 f(a) * 1000 + (f(b) + 100) * 10 + (f(c) + 10)\n\
            }"); // 7*1000 + (-9+100)*10 + (-1+10) = 7000 + 910 + 9 = 7919
    }

    /// M40.2: `for x in it` sobre un tipo que implementa `Iterator<T>`. El `for` llama a `next`
    /// hasta `None`, ligando el elemento. Oráculo VM↔intérprete (el estado del iterador muta por
    /// referencia entre iteraciones).
    #[test]
    fn iterator_for_oraculo() {
        oracle_program("\
            struct Rango { actual: int, fin: int }\n\
            impl Iterator<int> for Rango {\n\
            \x20 fn next(self) -> Option<int> {\n\
            \x20   if (self.actual < self.fin) {\n\
            \x20     let v = self.actual;\n\
            \x20     self.actual = self.actual + 1;\n\
            \x20     Option.Some(v)\n\
            \x20   } else { Option.None }\n\
            \x20 }\n\
            }\n\
            fn main() -> int {\n\
            \x20 let r = Rango { actual: 1, fin: 6 };\n\
            \x20 var suma = 0;\n\
            \x20 for n in r {\n\
            \x20   suma = suma + n * n;\n\
            \x20 }\n\
            \x20 suma\n\
            }"); // 1+4+9+16+25 = 55
    }

    /// M40.2b: `.iter()` sobre arreglos (iterador genérico `ArrayIter<T>` del prelude) y `range`
    /// (iterador `RangeIter`), como iteradores de primera clase. Oráculo VM↔intérprete: el impl
    /// genérico de `Iterator` y la sustitución del elemento (`[int].iter()` liga `int`, no `T`).
    #[test]
    fn iter_range_oraculo() {
        oracle_program("\
            fn main() -> int {\n\
            \x20 let xs = [10, 20, 30, 40];\n\
            \x20 var s = 0;\n\
            \x20 for x in xs.iter() { s = s + x; }\n\
            \x20 var p = 1;\n\
            \x20 for i in range(1, 6) { p = p * i; }\n\
            \x20 let it = range(0, 4);\n\
            \x20 var q = 0;\n\
            \x20 for n in it { q = q + n; }\n\
            \x20 s * 10000 + p * 10 + q\n\
            }"); // 100*10000 + 120*10 + 6 = 1001206
    }

    /// M40.2c: adaptadores PEREZOSOS `.map()`/`.filter()` — métodos genéricos por defecto de
    /// `Iterator`, encadenables, respaldados por un closure (`Iter<T>`). Oráculo VM↔intérprete:
    /// map cambia de tipo de elemento, filter avanza el origen, y el encadenamiento se evalúa al
    /// recorrer. Ejercita métodos genéricos + captura mutable en closures + despacho por receptor.
    #[test]
    fn adaptadores_perezosos_oraculo() {
        oracle_program("\
            fn main() -> int {\n\
            \x20 var a = 0;\n\
            \x20 for x in range(1, 6).map(fn(n: int) -> int { n * n }) { a = a + x; }\n\
            \x20 var b = 0;\n\
            \x20 for x in range(0, 10).filter(fn(n: int) -> bool { n % 2 == 0 }) { b = b + x; }\n\
            \x20 var c = 0;\n\
            \x20 let it = range(1, 11)\n\
            \x20   .map(fn(n: int) -> int { n * 3 })\n\
            \x20   .filter(fn(n: int) -> bool { n > 15 });\n\
            \x20 for x in it { c = c + x; }\n\
            \x20 let xs = [7, 8, 9];\n\
            \x20 var d = 0;\n\
            \x20 for x in xs.iter().filter(fn(n: int) -> bool { n > 7 }) { d = d + x; }\n\
            \x20 a * 1000000 + b * 10000 + c * 100 + d\n\
            }"); // a=55, b=20, c=120, d=17 → 55*1000000 + 20*10000 + 120*100 + 17 = 55200012017... comprobado por el oráculo
    }

    /// M40.2d: operaciones TERMINALES `.fold()` (reduce a un valor, método genérico sobre el
    /// acumulador) y `.collect()` (materializa a `[T]`, puente de vuelta desde la cadena perezosa).
    /// Oráculo VM↔intérprete: fold cambia de tipo, collect tras map/filter, y coexistencia con el
    /// `fold` EAGER de arreglos (el `[T].fold` cae en la función libre).
    #[test]
    fn fold_collect_oraculo() {
        oracle_program("\
            fn main() -> int {\n\
            \x20 let a = range(1, 6).fold(0, fn(ac: int, x: int) -> int { ac + x });\n\
            \x20 let ys = range(1, 11)\n\
            \x20   .map(fn(n: int) -> int { n * n })\n\
            \x20   .filter(fn(n: int) -> bool { n % 2 == 1 })\n\
            \x20   .collect();\n\
            \x20 let b = ys.fold(0, fn(ac: int, x: int) -> int { ac + x });\n\
            \x20 let zs = [3, 1, 2].iter().map(fn(n: int) -> int { n + 10 }).collect();\n\
            \x20 a * 100000 + b * 100 + ys.len() * 10 + zs[0]\n\
            }"); // a=15, b=165 (1+9+25+49+81), len=5, zs[0]=13
    }

    /// M40.2e: adaptadores `.take(n)` (perezoso, corta) y `.enumerate()` (pares `(int, T)`), este
    /// último consumido con **patrón de tupla en el `for`** (`for (i, x) in it.enumerate()`). Oráculo
    /// VM↔intérprete. Ejercita también la inferencia genérica sobre tuplas (`Iter<(int, T)>`).
    #[test]
    fn take_enumerate_oraculo() {
        oracle_program("\
            fn main() -> int {\n\
            \x20 let ys = range(1, 1000).map(fn(n: int) -> int { n * n }).take(4).collect();\n\
            \x20 var a = 0;\n\
            \x20 for x in ys.iter() { a = a + x; }\n\
            \x20 var b = 0;\n\
            \x20 for par in [10, 20, 30].iter().enumerate() { let (i, v) = par; b = b + i * v; }\n\
            \x20 var c = 0;\n\
            \x20 for (i, v) in range(5, 100).enumerate().take(3) { c = c + i * 100 + v; }\n\
            \x20 a * 10000 + b * 100 + c\n\
            }"); // a=1+4+9+16=30, b=0*10+1*20+2*30=80, c=(0*100+5)+(1*100+6)+(2*100+7)=5+106+207=318
    }

    /// M40.2f: `.skip(n)` (descarta los primeros n), `.zip(otra)` (pares `(T, U)`, se agota con el
    /// más corto; método genérico) y `.sum()` (terminal, función libre sobre `Iter<int>` vía UFCS).
    /// Oráculo VM↔intérprete: zip con tipos distintos + patrón de tupla, y sum encadenado.
    #[test]
    fn skip_zip_sum_oraculo() {
        oracle_program("\
            fn main() -> int {\n\
            \x20 let a = sum(range(0, 100).skip(5).take(3));\n\
            \x20 var b = 0;\n\
            \x20 for (n, c) in range(1, 50).zip([\"a\", \"bb\", \"ccc\"].iter()) { b = b + n * c.len(); }\n\
            \x20 let d = range(1, 6).map(fn(n: int) -> int { n * n }).sum();\n\
            \x20 a * 100000 + b * 100 + d\n\
            }"); // a=5+6+7=18, b=1*1+2*2+3*3=14, d=55 → 18*100000+14*100+55
    }

    /// M40.3a: `@derive(Hash)` sobre struct y enum, más `char_code` y las impls de Hash de
    /// primitivos (int/bool/char/string) del prelude. El hash se calcula EN raylang (recursión por
    /// `.hash()` de campos), así que el oráculo VM↔intérprete verifica que ambos motores producen el
    /// MISMO entero. Cubre el fix de colisión de posiciones (dos derivados con campos de tipos
    /// distintos que van a `int#hash` vs `string#hash`).
    #[test]
    fn hash_derive_oraculo() {
        oracle_program("\
            @derive(Hash, Eq)\n\
            struct Punto { x: int, y: int }\n\
            @derive(Hash)\n\
            struct Persona { nombre: string, edad: int }\n\
            @derive(Hash)\n\
            enum Color { Rojo, Verde, RGB(int, int, int) }\n\
            fn main() -> int {\n\
            \x20 let p = Punto { x: 3, y: 4 };\n\
            \x20 let a = Persona { nombre: \"Ada\", edad: 36 };\n\
            \x20 let mismo = if (p.hash() == (Punto { x: 3, y: 4 }).hash()) { 1 } else { 0 };\n\
            \x20 p.hash() + a.hash() * 7 + Color.RGB(1, 2, 3).hash() * 13 + char_code('Z') + mismo * 100000\n\
            }");
    }

    /// M40.3b: `Set<T>` (tabla hash bucketed del prelude, sobre Hash + Eq). Cubre `set_new()` (con
    /// inferencia bidireccional del tipo elemento), add/has/remove/size con primitivos Y un tipo de
    /// usuario (`@derive(Hash, Eq)`), incl. deduplicación. Oráculo VM↔intérprete (tamaño/pertenencia
    /// son deterministas; el orden de `set_items` no).
    #[test]
    fn set_oraculo() {
        oracle_program("\
            @derive(Hash, Eq)\n\
            struct P { x: int, y: int }\n\
            fn main() -> int {\n\
            \x20 let s: Set<int> = set_new();\n\
            \x20 set_add(s, 3); set_add(s, 7); set_add(s, 3); set_add(s, 100);\n\
            \x20 set_remove(s, 7);\n\
            \x20 let ps: Set<P> = set_new();\n\
            \x20 set_add(ps, P { x: 1, y: 2 });\n\
            \x20 set_add(ps, P { x: 1, y: 2 });\n\
            \x20 set_add(ps, P { x: 5, y: 6 });\n\
            \x20 let a = if (set_has(s, 3)) { 1 } else { 0 };\n\
            \x20 let b = if (set_has(s, 7)) { 1 } else { 0 };\n\
            \x20 let c = if (ps.set_has(P { x: 1, y: 2 })) { 1 } else { 0 };\n\
            \x20 set_size(s) * 1000 + set_size(ps) * 100 + a * 10 + b + c\n\
            }"); // size(s)=2, size(ps)=2, a=1, b=0, c=1 → 2000+200+10+0+1 = 2211
    }

    /// M40.3c: `StringBuilder` (acumula trozos, une una vez con `join`) y `Deque<T>` (cola doble
    /// sobre arreglo + índice head). Oráculo VM↔interp: sb_build determinista; deque con push/pop por
    /// ambos extremos, incl. el vaciado (None) y la reconstrucción de push_front.
    #[test]
    fn sb_deque_oraculo() {
        oracle_program("\
            fn dv(o: Option<int>) -> int { match (o) { Option.Some(v) => v, Option.None => 0 - 1, } }\n\
            fn main() -> int {\n\
            \x20 let sb = sb_new();\n\
            \x20 var i = 1;\n\
            \x20 while (i <= 4) { sb.sb_push(to_string(i)); sb.sb_push(\",\"); i = i + 1; }\n\
            \x20 let texto = sb.sb_build();\n\
            \x20 let d: Deque<int> = deque_new();\n\
            \x20 deque_push_back(d, 1); deque_push_back(d, 2); deque_push_front(d, 0);\n\
            \x20 let a = dv(deque_pop_front(d));\n\
            \x20 let b = dv(deque_pop_back(d));\n\
            \x20 deque_push_front(d, 9);\n\
            \x20 let c = dv(deque_pop_front(d));\n\
            \x20 texto.len() * 1000 + a * 100 + b * 10 + c\n\
            }"); // texto=\"1,2,3,4,\" (len 8), a=0, b=2, c=9 → 8000+0+20+9 = 8029
    }

    /// Ejecuta un programa en la VM con el GC en **modo estrés** (recolecta en cada
    /// punto seguro) y exige que el resultado coincida con el intérprete. Es la
    /// prueba clave del GC: si una raíz faltara, un valor vivo se liberaría y el
    /// resultado cambiaría o reventaría.
    fn oracle_stress(src: &str) {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("intérprete ok");
        let compiled = compile_program(&prog).expect("compila");
        let mut vm = Vm::new(&compiled);
        vm.cur.heap.stress = true;
        let result = vm.run().expect("vm ok");
        let vm_result = to_value(&vm.cur.heap, &compiled.enums, &result);
        assert_eq!(interp, vm_result, "VM (estrés) y intérprete difieren en:\n{}", src);
    }

    // ----- M2.1 / M2.2: expresiones -----

    #[test]
    fn aritmetica_coincide_con_el_interprete() {
        oracle_int("1 + 2 * 3");
        oracle_int("(1 + 2) * 3");
        oracle_int("10 - 2 - 3");
        oracle_int("17 % 5");
        oracle_int("-5 + 3");
        oracle_int("2 * 3 * 4 - 10 / 2");
    }

    #[test]
    fn comparaciones_y_bools() {
        assert_eq!(run_vm("3 < 5"), Value::Bool(true));
        assert_eq!(run_vm("3 == 5"), Value::Bool(false));
        assert_eq!(run_vm("!(2 > 1)"), Value::Bool(false));
        assert_eq!(run_vm("true"), Value::Bool(true));
    }

    #[test]
    fn flotantes() {
        assert_eq!(run_vm("1.0 / 2.0"), Value::Float(0.5));
        assert_eq!(run_vm("1.5 + 1.5"), Value::Float(3.0));
    }

    #[test]
    fn division_por_cero_es_error() {
        let chunk = compile_expr(&expr_of("10 / 0")).unwrap();
        assert!(run(&chunk).unwrap_err().msg.contains("división"));
    }

    #[test]
    fn if_como_expresion_coincide_con_el_interprete() {
        oracle_int("if (3 < 5) { 10 } else { 20 }");
        oracle_int("if (3 > 5) { 10 } else { 20 }");
        oracle_int("if (1 < 2) { if (2 < 3) { 1 } else { 2 } } else { 3 }");
        oracle_int("if (1 < 2 && 3 < 4) { 7 } else { 8 }");
    }

    #[test]
    fn if_sin_else_es_unit() {
        assert_eq!(run_vm("if (true) { }"), Value::Unit);
        assert_eq!(run_vm("if (false) { }"), Value::Unit);
    }

    #[test]
    fn logicos_y_su_cortocircuito() {
        assert_eq!(run_vm("true && true"), Value::Bool(true));
        assert_eq!(run_vm("true && false"), Value::Bool(false));
        assert_eq!(run_vm("false || true"), Value::Bool(true));
        assert_eq!(run_vm("false && (1 / 0 == 0)"), Value::Bool(false));
        assert_eq!(run_vm("true || (1 / 0 == 0)"), Value::Bool(true));
    }

    #[test]
    fn bloque_con_sentencias_y_valor_final() {
        assert_eq!(run_vm("{ 1; 2; 3 }"), Value::Int(3));
        assert_eq!(run_vm("{ 1; }"), Value::Unit);
    }

    // ----- M2.3: programas completos (variables, while, llamadas) -----

    #[test]
    fn recursion_fibonacci() {
        oracle_program(
            "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }
             fn main() -> int { fib(10) }",
        );
    }

    #[test]
    fn factorial_con_while_y_mutacion() {
        oracle_program(
            "fn main() -> int {
                var n: int = 5; var f: int = 1;
                while (n > 1) { f = f * n; n = n - 1; }
                f
             }",
        );
    }

    #[test]
    fn retorno_temprano() {
        oracle_program(
            "fn signo(x: int) -> int { if (x < 0) { return -1; } if (x > 0) { return 1; } 0 }
             fn main() -> int { signo(-7) + signo(0) + signo(42) }",
        );
    }

    #[test]
    fn gcd_recursivo() {
        oracle_program(
            "fn gcd(a: int, b: int) -> int { if (b == 0) { a } else { gcd(b, a % b) } }
             fn main() -> int { gcd(1071, 462) }",
        );
    }

    /// M13.3a: recursión infinita → ambos motores cortan con el MISMO error de
    /// desbordamiento, en vez de colgarse o reventar la pila. Es el oráculo del
    /// límite compartido (`MAX_CALL_DEPTH` == `MAX_FRAMES`). Corre dentro del hilo de
    /// pila grande para que el intérprete alcance el tope sin desbordar la pila del
    /// hilo de test (que es pequeña por defecto). **La recursión es NO de cola**
    /// (`1 + bucle(...)`): la de cola, con el TCO de M13.3b, sería un bucle infinito
    /// legítimo (O(1) marcos) y nunca desbordaría —ese es justo el punto del TCO—.
    #[test]
    fn overflow_aritmetico_oraculo() {
        // M34 (SPEC §8): el desbordamiento de int es ERROR de ejecución idéntico en ambos
        // motores (antes: panic en debug / wrap silencioso en release — dependía del build).
        let casos = [
            "fn main() -> int { let m = 9223372036854775807; m + 1 }",       // Add
            "fn main() -> int { let m = -9223372036854775807 - 1; m - 1 }",  // Sub
            "fn main() -> int { let m = 9223372036854775807; m * 2 }",       // Mul
            "fn main() -> int { let m = -9223372036854775807 - 1; m / -1 }", // Div (MIN/-1)
            "fn main() -> int { let m = -9223372036854775807 - 1; m % -1 }", // Rem (MIN%-1)
            "fn main() -> int { let m = -9223372036854775807 - 1; -m }",     // Neg (-MIN)
        ];
        for src in casos {
            let tokens = crate::lexer::lex(src).expect("lex ok");
            let mut prog = crate::parser::parse(tokens).expect("parse ok");
            crate::checker::check(&mut prog).expect("check ok");
            let interp = crate::interpreter::run(&prog).expect_err("el intérprete debe errar");
            let compiled = compile_program(&prog).expect("compila");
            let vm = run_program(&compiled).expect_err("la VM debe errar");
            assert!(interp.msg.contains("desbordamiento aritmético en int"), "interp: {} ({src})", interp.msg);
            assert_eq!(interp.msg, vm.msg, "ambos motores idénticos ({src})");
        }
        // Y la aritmética al borde SIN desbordar sigue funcionando igual en ambos.
        let src = "fn main() -> int { let m = 9223372036854775806; print(m + 1); 0 }";
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        crate::interpreter::run(&prog).expect("interp ok");
        let compiled = compile_program(&prog).expect("compila");
        run_program(&compiled).expect("vm ok");
    }

    /// M42.1: **fuel** — límite de instrucciones de la VM. Un bucle infinito aborta con fuel finito
    /// (no cuelga); un programa que termina dentro del presupuesto da su resultado normal.
    #[test]
    fn fuel_limita_la_ejecucion() {
        fn compilar(src: &str) -> CompiledProgram {
            let tokens = crate::lexer::lex(src).expect("lex ok");
            let mut prog = crate::parser::parse(tokens).expect("parse ok");
            crate::checker::check(&mut prog).expect("check ok");
            compile_program(&prog).expect("compila")
        }
        // Bucle infinito: con fuel finito, aborta (sin fuel colgaría, así que no se prueba sin límite).
        let inf = compilar("fn main() -> int { var i = 0; while (true) { i = i + 1; } 0 }");
        let err = run_program_con_limite(&inf, Some(50_000), None).expect_err("debe agotar el fuel");
        assert!(err.msg.contains("fuel"), "mensaje de fuel: {}", err.msg);
        // Un programa que termina dentro del presupuesto da el mismo resultado que sin límite.
        let ok = compilar("fn main() -> int { var s = 0; var i = 0; while (i < 100) { s = s + i; i = i + 1; } s }");
        assert_eq!(run_program_con_limite(&ok, Some(1_000_000), None).unwrap(), Value::Int(4950));
        assert_eq!(run_program_con_limite(&ok, None, None).unwrap(), Value::Int(4950)); // None = sin límite
    }

    /// M38.1a: `transfer_value` re-aloja un subgrafo de un heap a otro con handles del destino.
    /// Cubre lo estructural (arreglo con struct + string), el **sharing interno** (un objeto alcanzado
    /// por dos caminos se copia UNA vez) y los **ciclos** (que un deep-copy ingenuo colgaría).
    #[test]
    fn transfer_value_entre_heaps() {
        use std::collections::HashMap;
        // (1) Estructural: [1, P{x:2}, "hi"] → estructuralmente igual, con handles del destino.
        {
            let mut a = Heap::new();
            let p = a.allocate(Obj::Struct(VmStruct { name: "P".into(), fields: vec![("x".into(), HeapValue::Int(2))] }));
            let top = a.allocate(Obj::Array(vec![HeapValue::Int(1), HeapValue::Obj(p), HeapValue::Str("hi".into())]));
            let mut b = Heap::new();
            let mut remap = HashMap::new();
            let tv = transfer_value(&a, &mut b, &HeapValue::Obj(top), &mut remap);
            assert_eq!(to_value(&b, &[], &tv), to_value(&a, &[], &HeapValue::Obj(top)), "estructuralmente iguales");
            assert_eq!(b.live(), 2, "se copiaron 2 objetos (arreglo + struct)");
        }
        // (2) Sharing: [sub, sub] con el MISMO handle → tras transferir, ambos apuntan al mismo destino.
        {
            let mut a = Heap::new();
            let sub = a.allocate(Obj::Array(vec![HeapValue::Int(7)]));
            let top = a.allocate(Obj::Array(vec![HeapValue::Obj(sub), HeapValue::Obj(sub)]));
            let mut b = Heap::new();
            let mut remap = HashMap::new();
            let tv = transfer_value(&a, &mut b, &HeapValue::Obj(top), &mut remap);
            let nt = tv.handle().unwrap();
            let (h0, h1) = match b.get(nt) {
                Obj::Array(e) => (e[0].handle().unwrap(), e[1].handle().unwrap()),
                _ => panic!("esperaba arreglo"),
            };
            assert_eq!(h0, h1, "el sharing interno se preserva (un solo objeto copiado)");
            assert_eq!(b.live(), 2, "sharing → 2 objetos (top + sub), no 3");
        }
        // (3) Ciclo: arr -> cell -> arr. Debe terminar y preservar el ciclo.
        {
            let mut a = Heap::new();
            let arr = a.allocate(Obj::Array(Vec::new())); // placeholder
            let cell = a.allocate(Obj::Cell(HeapValue::Obj(arr)));
            *a.get_mut(arr) = Obj::Array(vec![HeapValue::Obj(cell)]); // cierra el ciclo
            let mut b = Heap::new();
            let mut remap = HashMap::new();
            let tv = transfer_value(&a, &mut b, &HeapValue::Obj(arr), &mut remap);
            let narr = tv.handle().unwrap();
            let ncell = match b.get(narr) { Obj::Array(e) => e[0].handle().unwrap(), _ => panic!() };
            let back = match b.get(ncell) { Obj::Cell(HeapValue::Obj(h)) => *h, _ => panic!() };
            assert_eq!(back, narr, "el ciclo se preserva (la celda apunta de vuelta al arreglo)");
            assert_eq!(b.live(), 2, "ciclo → 2 objetos (arreglo + celda)");
        }
    }

    /// M42.2: **tope de heap** — límite de objetos vivos de la VM. Un programa que retiene un montón
    /// de objetos (aquí, un arreglo que crece sin cesar) aborta al rebasar el tope; uno frugal, no.
    #[test]
    fn tope_de_heap_limita_los_objetos_vivos() {
        fn compilar(src: &str) -> CompiledProgram {
            let tokens = crate::lexer::lex(src).expect("lex ok");
            let mut prog = crate::parser::parse(tokens).expect("parse ok");
            crate::checker::check(&mut prog).expect("check ok");
            compile_program(&prog).expect("compila")
        }
        // Retiene objetos vivos sin parar (cada iteración empuja un arreglo nuevo a `xs`, que sigue
        // alcanzable). Con un tope bajo, el GC no puede liberarlos → aborta.
        let crece = compilar(
            "fn main() -> int { var xs: [[int]] = []; var i = 0; while (i < 100000) { xs.push([i]); i = i + 1; } 0 }",
        );
        let err = run_program_con_limite(&crece, None, Some(1_000)).expect_err("debe rebasar el tope");
        assert!(err.msg.contains("tope de heap"), "mensaje de tope: {}", err.msg);
        // Un programa frugal (no retiene) termina normal aun con tope bajo: el GC recicla la basura.
        let frugal = compilar("fn main() -> int { var s = 0; var i = 0; while (i < 10000) { s = s + i; i = i + 1; } s }");
        assert_eq!(run_program_con_limite(&frugal, None, Some(1_000)).unwrap(), Value::Int(49995000));
    }

    #[test]
        fn overflow_recursion_oraculo() {
        let (interp_msg, vm_msg) = crate::with_big_stack(|| {
            let src = "fn bucle(n: int) -> int { 1 + bucle(n + 1) }
                       fn main() -> int { bucle(0) }";
            let tokens = crate::lexer::lex(src).expect("lex ok");
            let mut prog = crate::parser::parse(tokens).expect("parse ok");
            crate::checker::check(&mut prog).expect("check ok");
            let interp = crate::interpreter::run(&prog).expect_err("el intérprete debe errar");
            let compiled = compile_program(&prog).expect("compila");
            let vm = run_program(&compiled).expect_err("la VM debe errar");
            (interp.msg, vm.msg)
        });
        assert!(interp_msg.contains("desbordamiento de pila"), "intérprete: {interp_msg}");
        assert!(vm_msg.contains("desbordamiento de pila"), "vm: {vm_msg}");
        // Ambos motores reportan exactamente el mismo mensaje.
        assert_eq!(interp_msg, vm_msg, "los dos motores difieren en el mensaje");
    }

    /// M13.2a: aserciones que pasan no alteran el resultado (oráculo normal).
    #[test]
    fn assert_pasa_oraculo() {
        oracle_program(
            "fn main() -> int {
                assert(1 + 1 == 2);
                assert_eq(2 * 3, 6);
                assert_eq(\"ab\", \"a\" + \"b\");
                42
             }",
        );
    }

    /// M13.2a: `panic` / `assert_eq` que falla → ambos motores cortan con el MISMO mensaje.
    #[test]
    fn panic_y_assert_falla_oraculo() {
        for (src, esperado) in [
            ("fn main() -> int { panic(\"boom\"); 0 }", "boom"),
            ("fn main() -> int { assert_eq(2 + 2, 5); 0 }", "assert_eq falló: 4 != 5"),
            ("fn main() -> int { assert(false); 0 }", "aserción falló"),
        ] {
            let tokens = crate::lexer::lex(src).expect("lex ok");
            let mut prog = crate::parser::parse(tokens).expect("parse ok");
            crate::checker::check(&mut prog).expect("check ok");
            let interp = crate::interpreter::run(&prog).expect_err("el intérprete debe errar");
            let compiled = compile_program(&prog).expect("compila");
            let vm = run_program(&compiled).expect_err("la VM debe errar");
            assert_eq!(interp.msg, esperado, "intérprete: {}", src);
            assert_eq!(vm.msg, esperado, "vm: {}", src);
        }
    }

    /// M15.1a: la stdlib de matemáticas en el oráculo. Las funciones float se enrutan a `int` por la
    /// comparación de floats del propio lenguaje (NaN != NaN impediría comparar `Value::Float`
    /// directamente); abs/min/max sobre `int` devuelven `int`. El último caso fija la semántica de
    /// **borde** de `f64`: `sqrt(-1.0)` da `NaN` en ambos motores → `NaN == NaN` es `false` → `0`.
    #[test]
    fn matematicas_oraculo() {
        // Polimórficas sobre int → resultado int directo.
        oracle_int("abs(-7)");
        oracle_int("abs(7)");
        oracle_int("min(3, 8)");
        oracle_int("max(3, 8)");
        // Funciones float, verificadas por igualdad (ambos motores calculan idéntico).
        oracle_int("if (sqrt(16.0) == 4.0) { 1 } else { 0 }");
        oracle_int("if (pow(2.0, 10.0) == 1024.0) { 1 } else { 0 }");
        oracle_int("if (floor(3.7) == 3.0) { 1 } else { 0 }");
        oracle_int("if (ceil(3.2) == 4.0) { 1 } else { 0 }");
        oracle_int("if (round(2.5) == 3.0) { 1 } else { 0 }");
        oracle_int("if (abs(-2.5) == 2.5) { 1 } else { 0 }");
        oracle_int("if (min(1.5, 9.0) == 1.5) { 1 } else { 0 }");
        oracle_int("if (max(1.5, 9.0) == 9.0) { 1 } else { 0 }");
        oracle_int("if (sin(0.0) == 0.0) { 1 } else { 0 }");
        oracle_int("if (cos(0.0) == 1.0) { 1 } else { 0 }");
        oracle_int("if (ln(e()) == 1.0) { 1 } else { 0 }");
        oracle_int("if (log10(1000.0) == 3.0) { 1 } else { 0 }");
        oracle_int("if (exp(0.0) == 1.0) { 1 } else { 0 }");
        oracle_int("if (pi() > 3.14) { 1 } else { 0 }");
        // Borde: NaN se comporta igual en ambos motores (NaN != NaN → la rama else).
        oracle_int("if (sqrt(0.0 - 1.0) == sqrt(0.0 - 1.0)) { 1 } else { 0 }");
    }

    /// M27.1: tuplas — retorno múltiple, acceso `.N`, desestructuración (`_`), heterogéneas. Erasure a
    /// arreglos → ambos motores coinciden.
    #[test]
    fn tuplas_oraculo() {
        oracle_program("fn dm(a: int, b: int) -> (int, int) { (a / b, a % b) } fn main() -> int { let t = dm(17, 5); t.0 + t.1 * 10 }"); // 3 + 20 = 23
        oracle_program("fn main() -> int { let (q, r) = (7, 3); q * r }"); // 21
        oracle_program("fn main() -> int { let (_, b, _) = (1, 42, 9); b }"); // 42 (descarta con _)
        oracle_program("fn main() -> int { let t = (\"x\", 5, true); if (t.2) { t.1 } else { 0 } }"); // 5 (heterogénea)
        oracle_program("fn swap(a: int, b: int) -> (int, int) { (b, a) } fn main() -> int { let (x, y) = swap(1, 2); x * 10 + y }"); // 21
        // Tupla anidada (el acceso encadenado `t.0.1` choca con el float `0.1` en el lexer → binding
        // intermedio; limitación documentada de M27.1).
        oracle_program("fn main() -> int { let t = ((1, 2), 3); let inner = t.0; inner.1 + t.1 }"); // 2 + 3 = 5
    }

    /// M27.2: bucle `for` — rango, arreglo, string, Map `(k, v)`, `_`. Ambos motores coinciden.
    /// M27.5: constantes de nivel superior. Resueltas como `Ident` globales → ambos motores coinciden.
    #[test]
    fn const_oraculo() {
        oracle_program("const MAX: int = 100; fn main() -> int { MAX - 42 }"); // 58
        oracle_program("const A: int = 7; const B: int = 3; fn main() -> int { A * B }"); // 21
        oracle_program("const NEG: int = -5; fn main() -> int { NEG + 10 }"); // 5
        oracle_program("const PI: float = 3.0; fn f(r: float) -> float { PI * r } fn main() -> int { f(4.0) as int }"); // 12
        oracle_program("const ON: bool = true; fn main() -> int { if (ON) { 1 } else { 0 } }"); // 1
        oracle_int("if (\"x\" == \"x\") { 1 } else { 0 }"); // control
    }

    /// M27.4: casts `as` — int↔float, char↔int. Cambian la representación → ambos motores coinciden.
    #[test]
    fn cast_oraculo() {
        oracle_int("(3.99 as int) + (2.1 as int)");        // 3 + 2 = 5
        oracle_int("('A' as int) + ('a' as int)");         // 65 + 97 = 162
        oracle_int("if ((7 as float) == 7.0) { 1 } else { 0 }"); // 1
        oracle_int("if ((66 as char) == 'B') { 1 } else { 0 }"); // 1
        oracle_int("(0.0 - 4.7) as int");                  // -4 (trunca hacia cero)
        oracle_program("fn main() -> int { let s = 10; let n = 4; let avg = (s as float) / (n as float); avg as int }"); // 2
    }

    /// M27.3: interpolación de strings `"...${expr}..."`. Desazucara a `+ to_string(...)` → ambos
    /// motores coinciden. Se enruta a `int` comparando la longitud (print diferido en `oracle_int`).
    #[test]
    fn interpolacion_oraculo() {
        oracle_int("\"x=${1}\".len()");                     // "x=1" → 3
        oracle_int("\"${1}+${2}=${3}\".len()");             // "1+2=3" → 5
        oracle_int("if (\"a${1}b\" == \"a1b\") { 1 } else { 0 }");   // 1
        oracle_int("if (\"${2 + 3}\" == \"5\") { 1 } else { 0 }");   // 1
        oracle_int("if (\"${true}/${'z'}\" == \"true/z\") { 1 } else { 0 }"); // 1
        // Las llaves son SIEMPRE literales (sin `{{`/`}}`); solo `${` es especial.
        oracle_int("\"llave {lit}\".len()"); // "llave {lit}" = 11
        // Un `$` que no precede a `{` es literal (sin escape): "$5" → 2 caracteres.
        oracle_int("\"$5\".len()");                         // 2
        // `\$` escapa un `${` literal: "\${x}" → "${x}" = 4 caracteres, sin interpolar.
        oracle_int("\"\\${x}\".len()");                     // 4 (literal "${x}")
        // Interpolación con una variable local.
        oracle_program("fn main() -> int { let n = 42; if (\"n=${n}\" == \"n=42\") { 1 } else { 0 } }");
    }

    /// M28.3: enteros sin signo con tamaño (u8/u32/u64). Aritmética con wrapping dentro del ancho,
    /// bitops, comparación sin signo, conversión con `as`. Ambos motores comparten la máscara → iguales.
    #[test]
    fn uint_oraculo() {
        oracle_int("((200 as u8) + (100 as u8)) as int");   // 300 mod 256 = 44
        oracle_int("(511 as u8) as int");                    // 255 (enmascarado)
        oracle_int("((4294967295 as u32) + (1 as u32)) as int"); // wrap a 0
        oracle_int("((1 as u32) << (8 as u32)) as int");     // 256
        oracle_int("(~(0 as u8)) as int");                   // 255
        oracle_int("((250 as u8) - (5 as u8)) as int");      // 245
        oracle_int("((0 as u8) - (1 as u8)) as int");        // wrap a 255
        oracle_int("if ((255 as u8) > (1 as u8)) { 1 } else { 0 }"); // 1 (sin signo)
        oracle_int("(((240 as u8) & (15 as u8)) | (1 as u8)) as int"); // (0) | 1 = 1
        oracle_int("(((1000000 as u64) * (1000000 as u64))) as int"); // 10^12 (cabe en u64, no en u32)
        // Round-trip de anchos.
        oracle_int("(((300 as u8) as u32) as int)");         // 300&0xFF=44
        oracle_program("fn dobla(x: u32) -> u32 { x + x } fn main() -> int { dobla(10 as u32) as int }"); // 20
    }

    /// M28.3b: coerción de literal entero polimórfico — un literal adopta el ancho uint del contexto
    /// (tipo esperado u operando). Baja a un `as` → ambos motores coinciden.
    #[test]
    fn uint_literal_oraculo() {
        oracle_program("fn main() -> int { let x: u8 = 5; x as int }");            // 5
        oracle_program("fn main() -> int { let x: u8 = 200; let y: u8 = x + 100; y as int }"); // 44
        oracle_program("fn main() -> int { let z: u8 = 200 + 100; z as int }");    // 44 (ambos literales)
        oracle_program("fn main() -> int { let b: u32 = 4000000000; b as int }");  // 4000000000
        oracle_program("fn main() -> int { let a: [u8] = [1, 2, 3]; a[2] as int }"); // 3
        oracle_program("fn f(x: u8) -> u8 { x } fn main() -> int { f(42) as int }"); // 42 (arg literal)
        oracle_program("fn main() -> int { let m: u32 = (1 << 8) + 1; m as int }"); // 257 (bitop literales)
        // M28.3b: la asignación coerciona el literal al ancho del destino (var, campo, elemento).
        oracle_program("fn main() -> int { var x: u8 = 0; x = 200; x as int }");    // 200
        oracle_program("fn main() -> int { var a: [u32] = [0]; a[0] = 7; a[0] as int }"); // 7
    }

    /// M28.2: `?` con conversión de error vía `From<S>`. `expr?` (con `impl From<E1> for E2`) baja a
    /// un `match` que convierte en la rama de error → runtime intacto, ambos motores coinciden.
    #[test]
    fn conversion_error_oraculo() {
        let base = "enum MiErr { Io(string) } \
            impl From<string> for MiErr { fn desde(o: string) -> MiErr { MiErr.Io(o) } } \
            fn leer(f: bool) -> Result<int, string> { if (f) { Result.Err(\"x\") } else { Result.Ok(7) } } \
            fn proc(f: bool) -> Result<int, MiErr> { let x = leer(f)?; Result.Ok(x + 1) } ";
        // Camino Ok: proc(false) = Ok(8); code = 8.
        oracle_program(&format!("{base} fn main() -> int {{ match (proc(false)) {{ Result.Ok(v) => v, Result.Err(e) => 0 - 1 }} }}"));
        // Camino Err convertido: proc(true) = Err(MiErr.Io(\"x\")); se detecta la conversión → 99.
        oracle_program(&format!("{base} fn main() -> int {{ match (proc(true)) {{ Result.Ok(v) => v, Result.Err(e) => match (e) {{ MiErr.Io(s) => 99 }} }} }}"));
        // El `?` SIN conversión (mismo tipo de error) sigue intacto.
        oracle_program("fn leer() -> Result<int, string> { Result.Ok(5) } fn proc() -> Result<int, string> { let x = leer()?; Result.Ok(x * 2) } fn main() -> int { match (proc()) { Result.Ok(v) => v, Result.Err(e) => 0 } }");
    }

    /// Un `match` con TODOS los brazos divergentes (`return`) type-checkea (antes hacía panic el checker,
    /// "hay al menos un brazo"): el match diverge y vale unit; la función retorna por los `return`.
    #[test]
    fn match_todos_divergentes_oraculo() {
        oracle_program("fn f(o: Option<int>) -> int { match (o) { Option.Some(n) => { return n; }, Option.None => { return 0; } } } fn main() -> int { f(Option.Some(5)) + f(Option.None) }"); // 5
    }

    /// M28.1: sobrecarga de operadores vía traits (`Add`/`Sub`/`Mul`/`Div`/`Neg`). `a op b` sobre
    /// un tipo de usuario baja a `a.metodo(b)` (función manglada de M9) → ambos motores coinciden.
    #[test]
    fn operadores_oraculo() {
        let vec2 = "struct Vec2 { x: int, y: int } \
            impl Add for Vec2 { fn add(self, o: Vec2) -> Vec2 { Vec2 { x: self.x + o.x, y: self.y + o.y } } } \
            impl Sub for Vec2 { fn sub(self, o: Vec2) -> Vec2 { Vec2 { x: self.x - o.x, y: self.y - o.y } } } \
            impl Neg for Vec2 { fn neg(self) -> Vec2 { Vec2 { x: 0 - self.x, y: 0 - self.y } } } ";
        // Suma de vectores: (1,2) + (3,4) = (4,6) → 4+6 = 10.
        oracle_program(&format!("{vec2} fn main() -> int {{ let a = Vec2 {{ x: 1, y: 2 }}; let b = Vec2 {{ x: 3, y: 4 }}; let c = a + b; c.x + c.y }}"));
        // Resta y negación encadenadas: -((5,5) - (1,2)) = -(4,3) = (-4,-3) → -7.
        oracle_program(&format!("{vec2} fn main() -> int {{ let a = Vec2 {{ x: 5, y: 5 }}; let b = Vec2 {{ x: 1, y: 2 }}; let c = -(a - b); c.x + c.y }}"));
        // Suma triple encadenada (mismo operador, posición compartida en el AST): (1,0)+(1,0)+(1,0).
        oracle_program(&format!("{vec2} fn main() -> int {{ let u = Vec2 {{ x: 1, y: 0 }}; let s = u + u + u; s.x }}"));
        // Los operadores built-in sobre int/float siguen intactos (no se enrutan a traits).
        oracle_int("2 * 3 + 4");
    }

    #[test]
    fn for_oraculo() {
        oracle_program("fn main() -> int { var s = 0; for i in 0..5 { s = s + i; } s }"); // 10
        oracle_program("fn main() -> int { var t = 0; for x in [10, 20, 30] { t = t + x; } t }"); // 60
        oracle_program("fn main() -> int { var n = 0; for c in \"hola\" { n = n + 1; } n }"); // 4
        oracle_program("fn main() -> int { var m: Map<string, int> = Map.new(); m.insert(\"a\", 1); m.insert(\"b\", 5); var s = 0; for (k, v) in m { s = s + v; } s }"); // 6
        oracle_program("fn main() -> int { var m: Map<int, int> = Map.new(); m.insert(1, 10); m.insert(2, 20); var c = 0; for (k, _) in m { c = c + k; } c }"); // 3
        // for anidado.
        oracle_program("fn main() -> int { var s = 0; for i in 0..3 { for j in 0..3 { s = s + 1; } } s }"); // 9
        // `for` sobre un valor con return dentro (propaga).
        oracle_program("fn buscar(xs: [int], t: int) -> int { for x in xs { if (x == t) { return 1; } } 0 } fn main() -> int { buscar([3, 7, 9], 7) }"); // 1
    }

    #[test]
    fn bitops_oraculo() {
        // M19.3a: operadores bit a bit. Ambos motores comparten `wrapping_*` → idénticos.
        oracle_int("6 & 3");   // 0b110 & 0b011 = 0b010 = 2
        oracle_int("6 | 3");   // 0b111 = 7
        oracle_int("6 ^ 3");   // 0b101 = 5
        oracle_int("1 << 4");  // 16
        oracle_int("255 >> 4");// 15
        oracle_int("~0");      // -1 (complemento a uno)
        oracle_int("~5");      // -6
        // Precedencia: shift por debajo de aditivo; bit a bit por debajo de comparación.
        oracle_int("1 + 1 << 4");           // (1+1) << 4 = 32
        oracle_int("(0 | 1) & (2 | 1)");    // 1
        // Patrón típico de framing: combinar dos bytes en un entero de 16 bits.
        oracle_int("200 << 8 | 57");        // 200*256+57 = 51257
        // Máscara y desplazamiento encadenados (estilo extracción de campos).
        oracle_int("(51257 >> 8) & 255");   // 200
        oracle_int("51257 & 255");          // 57
    }

    /// M16.1a: el tipo `bytes` en el oráculo. Literal `b"..."` (con `\xNN`), `len`, indexar (→int) e
    /// igualdad. Se enruta a `int` (el booleano de `==` vía `if`) porque `print(bytes)` está diferido.
    #[test]
    fn bytes_oraculo() {
        oracle_int("b\"AB\".len()");                    // 2
        oracle_int("b\"hola\".len()");                  // 4
        oracle_int("b\"\\x00\\xff\"[1]");              // 255
        oracle_int("b\"AB\\x00\"[0]");                 // 65
        oracle_int("b\"AB\\x00\"[2]");                 // 0
        oracle_int("b\"\".len()");                      // 0 (vacío)
        // Igualdad estructural (misma secuencia / distinta) → 1/0.
        oracle_int("if (b\"AB\\xff\" == b\"AB\\xff\") { 1 } else { 0 }");
        oracle_int("if (b\"AB\" == b\"AC\") { 1 } else { 0 }");
        oracle_int("if (b\"AB\" == b\"ABC\") { 1 } else { 0 }");
        // Los caracteres no-ASCII se codifican como UTF-8 (á = 2 octetos).
        oracle_int("b\"á\".len()");                     // 2
        // M16.1b: to_bytes (builtin) + concatenación (opcode Add).
        oracle_int("\"hola, mundo\".to_bytes().len()");                   // 11
        oracle_int("\"á\".to_bytes().len()");                            // 2 (UTF-8)
        oracle_int("(\"AB\".to_bytes() + \"CD\".to_bytes()).len()");        // 4
        oracle_int("if (\"AB\".to_bytes() == b\"AB\") { 1 } else { 0 }");
        oracle_int("if (\"A\".to_bytes() + \"B\".to_bytes() == b\"AB\") { 1 } else { 0 }");
    }

    #[test]
    fn bytes_a_hex_oraculo() {
        // M16 (diferido): to_string(bytes) → hex en minúscula; idéntico en ambos motores.
        oracle_int("if (to_string(b\"Hi\\xff\") == \"4869ff\") { 1 } else { 0 }");   // H=48 i=69 ff
        oracle_int("if (to_string(b\"\\x00\\x01\\x02\") == \"000102\") { 1 } else { 0 }");
        oracle_int("if (to_string(b\"\") == \"\") { 1 } else { 0 }");                  // vacío
        oracle_int("if (to_string(\"raylang\".to_bytes()) == \"7261796c616e67\") { 1 } else { 0 }");
        oracle_int("to_string(b\"AB\\xff\").len()");                                    // 6 (2 hex por octeto)
    }

    /// M16.1b: `from_utf8` es un envoltorio del **prelude** (no un opcode), así que se prueba con el
    /// oráculo a nivel de programa completo (que inyecta el prelude), no con expresiones sueltas.
    #[test]
    fn bytes_from_utf8_oraculo() {
        // Round-trip válido: decodifica y mide la longitud del string.
        oracle_program("fn main() -> int { match (from_utf8(b\"hola\")) { Result.Ok(s) => s.len(), Result.Err(e) => -1, } }");
        // UTF-8 inválido → Err → 0.
        oracle_program("fn main() -> int { match (from_utf8(b\"\\xff\\xfe\")) { Result.Ok(s) => 1, Result.Err(e) => 0, } }");
        // to_bytes ∘ from_utf8 es identidad sobre texto válido.
        oracle_program("fn main() -> int { match (from_utf8(\"raylang\".to_bytes())) { Result.Ok(s) => s.len(), Result.Err(e) => -1, } }");
    }

    /// M19.2: `sub_bytes` (sub-secuencia por octeto, con clamp). Enrutado a int/bool (len/index/==),
    /// como el resto de oráculos de bytes (print de bytes diferido).
    #[test]
    fn sub_bytes_oraculo() {
        oracle_int("b\"hello\".sub_bytes(1, 4).len()");                       // 3 ("ell")
        oracle_int("b\"hello\".sub_bytes(1, 4)[0]");                          // 101 ('e')
        oracle_int("if (b\"ABCD\".sub_bytes(0, 2) == b\"AB\") { 1 } else { 0 }"); // 1
        oracle_int("if (b\"ABCD\".sub_bytes(2, 4) == b\"CD\") { 1 } else { 0 }"); // 1
        // Clamp: fin fuera de rango → recorta; inicio > n → vacío; i > j → vacío.
        oracle_int("b\"AB\".sub_bytes(0, 100).len()");                         // 2
        oracle_int("b\"AB\".sub_bytes(5, 10).len()");                          // 0
        oracle_int("b\"AB\".sub_bytes(1, 0).len()");                           // 0
        // Octetos crudos (incl. \x00/\xff) intactos.
        oracle_int("b\"\\x00\\xff\\x10\".sub_bytes(1, 2)[0]");               // 255
        oracle_int("b\"\\x00\\xff\\x10\".sub_bytes(0, 3).len()");             // 3
    }

    #[test]
    fn bytes_of_oraculo() {
        // M19.3c: construir bytes desde [int]. Indexar de vuelta da el mismo octeto.
        oracle_int("bytes_of([72, 105]).len()");                               // 2
        oracle_int("bytes_of([72, 105, 33])[1]");                             // 105
        // Truncado a octeto (`& 255`): 256 → 0, 511 → 255, negativos envuelven.
        oracle_int("bytes_of([256])[0]");                                     // 0
        oracle_int("bytes_of([511])[0]");                                     // 255
        // Round-trip con sub_bytes / igualdad de bytes.
        oracle_int("if (bytes_of([65, 66]) == b\"AB\") { 1 } else { 0 }");    // 1
        // Compone con concatenación de bytes (M16.1b): cabecera + carga.
        oracle_int("(bytes_of([129, 5]) + b\"hello\").len()");                   // 7
    }

    /// M13.1: Map en el oráculo. Las operaciones básicas dan el mismo resultado en ambos motores.
    #[test]
    fn map_basico_oraculo() {
        oracle_program(
            "fn main() -> int {
                let m: Map<string, int> = Map.new();
                m.insert(\"a\", 1);
                m.insert(\"b\", 2);
                m.insert(\"a\", 10);
                let total = match (m.get(\"a\")) { Option.Some(v) => v, Option.None => 0 };
                total + m.len()
             }",
        );
    }

    /// M48.4d: los métodos de `StrOps`/`BytesOps` (trim/split/replace/…/sub_bytes) despachan por trait
    /// y bajan a los builtins de string/bytes. Varios asignan heap → estrés del GC.
    #[test]
    fn trait_strops_bytesops_oraculo() {
        oracle_stress(
            "fn main() -> int {
                let s = \"  Hola Mundo  \";
                let t = s.trim();
                let up = t.to_upper();
                let partes = t.split(\" \");
                let r = t.replace(\"Mundo\", \"Ray\");
                let cs = t.chars();
                let rep = \"xy\".repeat(4);
                let b = t.to_bytes();
                let sb = b.sub_bytes(0, 4);
                t.len() + up.len() + partes.len() + r.len() + cs.len() + rep.len()
                    + b.len() + sb.len() + t.substring(0, 4).len()
                    + (if (t.starts_with(\"Hola\")) { 1 } else { 0 })
                    + (if (t.ends_with(\"Mundo\")) { 1 } else { 0 })
             }",
        );
    }

    /// M48.4c: `insert`/`contains_key`/`keys`/`values` como métodos del trait `MapOps` bajan a sus
    /// primitivos `__x`. `keys`/`values` asignan heap y son deterministas (orden de clave) → oráculo.
    #[test]
    fn trait_mapops_oraculo() {
        oracle_stress(
            "fn main() -> int {
                let m: Map<int, [int]> = [:];
                var i = 0;
                while (i < 20) { m.insert(i, [i, i * 3]); i = i + 1; }
                var suma = 0;
                let ks = m.keys();      // ordenadas 0..19
                let vs = m.values();    // en el mismo orden
                var j = 0;
                while (j < m.len()) { suma = suma + ks[j] + vs[j][1]; j = j + 1; }
                if (m.contains_key(7)) { suma = suma + 10000; }
                if (!m.contains_key(99)) { suma = suma + 100; }
                suma
             }",
        );
    }

    /// M48.4b: `push`/`reverse`/`contains` como métodos de trait (`Push`/`Reverse`/`Contains`) bajan a
    /// sus primitivos `__x`. `push`/`reverse` asignan heap → estrés del GC.
    #[test]
    fn trait_push_reverse_contains_oraculo() {
        oracle_stress(
            "struct Cola { items: [int] }
             impl Push<int> for Cola { fn push(self, x: int) { self.items.push(x) } }
             fn main() -> int {
                var a: [int] = [];
                var i = 0;
                while (i < 30) { a.push(i * 2); i = i + 1; }   // Push
                let r = a.reverse();                            // Reverse: [58, 56, …]
                let c = Cola { items: [7] };
                c.push(9);                                      // Push sobre tipo de usuario
                var suma = 0;
                if (a.contains(58)) { suma = suma + 1000; }     // Contains en arreglo
                if (\"abcdef\".contains(\"cde\")) { suma = suma + 100; } // Contains en string
                if (!a.contains(999)) { suma = suma + 10; }
                suma + a.len() + r[0] + c.items.len()           // 1110 + 30 + 58 + 2 = 1200
             }",
        );
    }

    /// M48.4a: `.len()` como método del trait `Len` (string/[T]/Map/bytes + tipo de usuario) baja al
    /// primitivo `__len` (mismo opcode `Len`) → ambos motores coinciden.
    #[test]
    fn trait_len_oraculo() {
        oracle_program(
            "struct Pila { d: [int] }
             impl Len for Pila { fn len(self) -> int { self.d.len() } }
             fn describir<T: Len>(x: T) -> int { x.len() }
             fn main() -> int {
                let m: Map<int, int> = [1: 10, 2: 20, 3: 30];
                let p = Pila { d: [7, 8, 9] };
                \"hola\".len() + [1,2,3,4,5].len() + m.len() + \"ab\".to_bytes().len()
                    + p.len() + describir([1,2]) + describir(p)
             }",
        );
    }

    /// M48.2: el literal de Map `[k: v, …]` baja a `Map.new()` + `insert` por par → ambos motores
    /// coinciden. Cubre poblado, `[:]` vacío, clave repetida (gana la última) y un valor con UFCS.
    #[test]
    fn map_literal_oraculo() {
        oracle_program(
            "fn dup(x: int) -> int { x * 2 }
             fn main() -> int {
                let m = [1: 10, 2: 20, 1: 30];
                let vacio: Map<int, int> = [:];
                vacio.insert(9, dup(5));
                let a = match (m.get(1)) { Option.Some(v) => v, Option.None => 0 };
                let b = match (vacio.get(9)) { Option.Some(v) => v, Option.None => 0 };
                a + b + m.len() + vacio.len()
             }",
        );
    }

    /// M48.2: el literal de Map asigna en el heap (Map + valores) → estrés del GC. Un literal con
    /// varios pares dentro de un bucle debe mantener sus valores vivos en cada recolección.
    #[test]
    fn map_literal_estres_gc_oraculo() {
        oracle_stress(
            "fn main() -> int {
                var suma = 0;
                var i = 0;
                while (i < 20) {
                    let m = [i: [i, i + 1], i + 100: [i + 2, i + 3]];
                    match (m.get(i)) {
                        Option.Some(par) => { suma = suma + par[0] + par[1]; },
                        Option.None => { suma = suma - 1; },
                    }
                    i = i + 1;
                }
                suma
             }",
        );
    }

    /// M48.1: `Map.new()` (función asociada) baja al mismo opcode `MapNew` que el antiguo `map_new()`
    /// → ambos motores coinciden. Mismo programa que `map_basico_oraculo` con la sintaxis nueva.
    #[test]
    fn map_new_asociada_oraculo() {
        oracle_program(
            "fn main() -> int {
                let m: Map<string, int> = Map.new();
                m.insert(\"a\", 1);
                m.insert(\"b\", 2);
                m.insert(\"a\", 10);
                let total = match (m.get(\"a\")) { Option.Some(v) => v, Option.None => 0 };
                total + m.len()
             }",
        );
    }

    /// M13.1: el Map asigna en el heap y guarda valores → estrés del GC (recolecta en cada paso).
    /// Si una raíz faltara, los valores guardados se liberarían y el resultado cambiaría.
    #[test]
    fn map_estres_gc_oraculo() {
        oracle_stress(
            "fn celda(n: int) -> [int] { [n, n * 2] }
             fn main() -> int {
                let m: Map<int, [int]> = Map.new();
                var i = 0;
                while (i < 30) { m.insert(i, celda(i)); i = i + 1; }
                var suma = 0;
                var j = 0;
                while (j < 30) {
                    match (m.get(j)) {
                        Option.Some(par) => { suma = suma + par[0] + par[1]; },
                        Option.None => { suma = suma - 1; },
                    }
                    j = j + 1;
                }
                suma + m.len()
             }",
        );
    }

    /// M13.1: claves de distintos tipos primitivos hashables.
    #[test]
    fn map_claves_variadas_oraculo() {
        oracle_program(
            "fn main() -> int {
                let porInt: Map<int, int> = Map.new();
                porInt.insert(7, 70);
                let porChar: Map<char, int> = Map.new();
                porChar.insert('z', 100);
                let porBool: Map<bool, int> = Map.new();
                porBool.insert(true, 1);
                porBool.insert(false, 2);
                let a = match (porInt.get(7)) { Option.Some(v) => v, Option.None => 0 };
                let b = match (porChar.get('z')) { Option.Some(v) => v, Option.None => 0 };
                let c = match (porBool.get(true)) { Option.Some(v) => v, Option.None => 0 };
                a + b + c + porBool.len()
             }",
        );
    }

    #[test]
    fn map_clave_bytes_oraculo() {
        // M16 (diferido): `bytes` como clave de Map. Incluye octetos crudos (\x00/\xff).
        oracle_program(
            "fn main() -> int {
                let m: Map<bytes, int> = Map.new();
                m.insert(b\"uno\", 10);
                m.insert(b\"\\x00\\xff\", 99);
                m.insert(b\"dos\", 20);
                let a = match (m.get(b\"uno\")) { Option.Some(v) => v, Option.None => 0 };
                let b = match (m.get(b\"\\x00\\xff\")) { Option.Some(v) => v, Option.None => 0 };
                let c = if (m.contains_key(b\"dos\")) { 1 } else { 0 };
                a + b + c + m.len()
             }",
        );
    }

    #[test]
    fn map_clave_bytes_keys_oraculo() {
        // keys/values con clave bytes: orden determinista (MapKey::Bytes es Ord lexicográfico).
        oracle_program(
            "fn main() -> int {
                let m: Map<bytes, int> = Map.new();
                m.insert(b\"c\", 3);
                m.insert(b\"a\", 1);
                m.insert(b\"b\", 2);
                let ks = m.keys();   // ordenadas: a, b, c
                let vs = m.values(); // 1, 2, 3
                var total = 0;
                var i = 0;
                while (i < vs.len()) { total = total + vs[i] * (i + 1); i = i + 1; }
                total + ks.len()
             }",
        );
    }

    /// M13.1b: keys (ordenadas) + values (en orden de clave) + remove, en el oráculo.
    #[test]
    fn map_keys_values_remove_oraculo() {
        oracle_program(
            "fn suma(a: [int]) -> int { var s = 0; var i = 0; while (i < a.len()) { s = s + a[i]; i = i + 1; } s }
             fn main() -> int {
                let m: Map<int, int> = Map.new();
                m.insert(3, 30);
                m.insert(1, 10);
                m.insert(2, 20);
                let ks = m.keys();              // [1, 2, 3]
                let vs = m.values();            // [10, 20, 30]
                let quitado = match (remove(m, 2)) { Option.Some(v) => v, Option.None => 0 };
                ks[0] * 100 + ks[2] + suma(vs) + quitado + m.len()
             }",
        );
    }

    /// M13.1b: keys/values asignan arreglos en el heap → estrés del GC.
    #[test]
    fn map_keys_values_estres_gc_oraculo() {
        oracle_stress(
            "fn suma(a: [int]) -> int { var s = 0; var i = 0; while (i < a.len()) { s = s + a[i]; i = i + 1; } s }
             fn main() -> int {
                let m: Map<int, int> = Map.new();
                var i = 0;
                while (i < 25) { m.insert(i, i * i); i = i + 1; }
                let total = suma(m.values()) + suma(m.keys());
                var quitados = 0;
                var j = 0;
                while (j < 25) {
                    match (remove(m, j)) {
                        Option.Some(v) => { quitados = quitados + 1; },
                        Option.None => {},
                    }
                    j = j + 2;
                }
                total + quitados + m.len()
             }",
        );
    }

    /// M13.3b: recursión de cola PROFUNDA (más allá de MAX_FRAMES) funciona en ambos motores
    /// gracias al TCO, y coinciden. Sin TCO, ambos cortarían en 1024 con desbordamiento.
    #[test]
    fn tco_recursion_de_cola_profunda_oraculo() {
        // 5000 > MAX_FRAMES (1024): solo pasa si la llamada en cola reutiliza el marco.
        oracle_program(
            "fn cuenta(n: int, acc: int) -> int {
                if (n == 0) { acc } else { cuenta(n - 1, acc + 1) }
             }
             fn main() -> int { cuenta(5000, 0) }",
        );
    }

    /// M13.3b: recursión mutua en cola + `return` en cola, también profunda.
    #[test]
    fn tco_mutua_y_return_en_cola_oraculo() {
        oracle_program(
            "fn par(n: int) -> bool { if (n == 0) { true } else { return impar(n - 1); } }
             fn impar(n: int) -> bool { if (n == 0) { false } else { par(n - 1) } }
             fn main() -> int { if (par(4000)) { 1 } else { 0 } }",
        );
    }

    /// M13.3b: una llamada que NO está en cola (su valor se usa en `n + ...`) sigue recurriendo de
    /// verdad —el TCO no debe convertirla— y da el mismo resultado en ambos motores. La profundidad
    /// es modesta porque el intérprete recurre sobre la pila de Rust (el hilo de test es pequeño; el
    /// binario real corre con pila grande, M13.3a). Que la recursión de cola SÍ se optimiza lo
    /// prueban `tco_recursion_de_cola_profunda_oraculo` (5000) y `tco_mutua_*` (4000).
    #[test]
    fn tco_no_aplica_a_llamada_no_en_cola_oraculo() {
        oracle_program(
            "fn suma_hasta(n: int) -> int { if (n == 0) { 0 } else { n + suma_hasta(n - 1) } }
             fn main() -> int { suma_hasta(30) }",
        );
    }

    #[test]
    fn variables_locales_y_shadowing() {
        oracle_program("fn main() -> int { let x: int = 1; { let x: int = 99; } x }");
        oracle_program(
            "fn main() -> int { var s: int = 0; var i: int = 0; while (i < 5) { s = s + i; i = i + 1; } s }",
        );
    }

    #[test]
    fn programa_con_print() {
        oracle_program("fn main() -> int { print(42); print(true); 0 }");
    }

    // ----- M3.1: arreglos -----

    #[test]
    fn arreglos_indexar_len_y_suma() {
        oracle_program("fn main() -> int { let a: [int] = [10, 20, 30]; a[0] + a[2] }");
        oracle_program("fn main() -> int { let a: [int] = [1, 2, 3, 4]; a.len() }");
    }

    #[test]
    fn arreglos_mutacion_y_push() {
        oracle_program("fn main() -> int { var a: [int] = [1, 2, 3]; a[1] = 99; a[1] }");
        oracle_program(
            "fn main() -> int { let a: [int] = []; a.push(5); a.push(7); a[0] + a[1] }",
        );
    }

    #[test]
    fn arreglos_son_por_referencia() {
        oracle_program("fn main() -> int { let a: [int] = [1, 2, 3]; let b: [int] = a; b[0] = 9; a[0] }");
    }

    #[test]
    fn suma_de_un_arreglo_con_while() {
        oracle_program(
            "fn suma(a: [int]) -> int {
                var s: int = 0; var i: int = 0;
                while (i < a.len()) { s = s + a[i]; i = i + 1; }
                s
             }
             fn main() -> int { suma([5, 10, 15, 20]) }",
        );
    }

    #[test]
    fn indice_fuera_de_rango_es_error() {
        let prog_src = "fn main() -> int { let a: [int] = [1, 2]; a[5] }";
        let tokens = crate::lexer::lex(prog_src).unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        let compiled = compile_program(&prog).unwrap();
        assert!(run_program(&compiled).unwrap_err().msg.contains("fuera de rango"));
    }

    // ----- M3.2: structs -----

    #[test]
    fn structs_acceso_y_orden_de_campos() {
        oracle_program("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 3, y: 4 }; p.x + p.y }");
        oracle_program("struct P { x: int, y: int } fn main() -> int { let p: P = P { y: 4, x: 3 }; p.x - p.y }");
    }

    #[test]
    fn structs_mutacion_de_campo() {
        oracle_program("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 1, y: 2 }; p.x = 9; p.x + p.y }");
    }

    #[test]
    fn structs_son_por_referencia() {
        oracle_program("struct C { v: int } fn main() -> int { let a: C = C { v: 1 }; let b: C = a; b.v = 9; a.v }");
    }

    #[test]
    fn structs_anidados_y_con_arreglos() {
        oracle_program(
            "struct P { x: int, y: int }
             struct L { a: P, b: P }
             fn dx(l: L) -> int { l.b.x - l.a.x }
             fn main() -> int { dx(L { a: P { x: 1, y: 0 }, b: P { x: 5, y: 0 } }) }",
        );
        oracle_program(
            "struct Pila { datos: [int] }
             fn main() -> int { let s: Pila = Pila { datos: [10, 20] }; s.datos.push(30); s.datos[2] }",
        );
    }

    // ----- M4.1: funciones de primera clase -----

    #[test]
    fn funcion_anonima_en_variable() {
        oracle_program("fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x * x }; f(9) }");
    }

    #[test]
    fn de_orden_superior_recibe_funcion() {
        oracle_program(
            "fn aplicar(f: fn(int) -> int, x: int) -> int { f(x) }
             fn main() -> int { aplicar(fn(n: int) -> int { n + 1 }, 41) }",
        );
    }

    #[test]
    fn nombre_de_funcion_como_valor() {
        oracle_program(
            "fn inc(n: int) -> int { n + 1 }
             fn aplicar(f: fn(int) -> int, x: int) -> int { f(x) }
             fn main() -> int { aplicar(inc, 10) }",
        );
    }

    #[test]
    fn devolver_una_funcion() {
        oracle_program(
            "fn elegir(b: bool) -> fn(int) -> int {
                 if (b) { fn(n: int) -> int { n + n } } else { fn(n: int) -> int { n * n } }
             }
             fn main() -> int { let f: fn(int) -> int = elegir(true); f(21) }",
        );
    }

    #[test]
    fn llamar_un_literal_de_funcion_directo() {
        oracle_program("fn main() -> int { (fn(x: int) -> int { x + x })(21) }");
    }

    #[test]
    fn variable_tapa_a_funcion_global() {
        oracle_program(
            "fn f(x: int) -> int { x * 100 }
             fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x + 1 }; f(41) }",
        );
    }

    #[test]
    fn mapear_sobre_arreglo_con_funcion() {
        oracle_program(
            "fn mapear(a: [int], f: fn(int) -> int) {
                 var i: int = 0;
                 while (i < a.len()) { a[i] = f(a[i]); i = i + 1; }
             }
             fn main() -> int {
                 var xs: [int] = [1, 2, 3, 4];
                 mapear(xs, fn(n: int) -> int { n * n });
                 xs[0] + xs[1] + xs[2] + xs[3]
             }",
        );
    }

    // ----- M4.2: closures (captura de entorno) -----

    #[test]
    fn closure_captura_un_let() {
        oracle_program(
            "fn main() -> int {
                 let base: int = 1000;
                 let f: fn(int) -> int = fn(d: int) -> int { base + d };
                 f(7)
             }",
        );
    }

    #[test]
    fn contador_con_estado_mutable() {
        oracle_program(
            "fn contador() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
             fn main() -> int { let c: fn() -> int = contador(); c(); c(); c() }",
        );
    }

    #[test]
    fn instancias_de_closure_son_independientes() {
        oracle_program(
            "fn contador() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
             fn main() -> int {
                 let a: fn() -> int = contador();
                 let b: fn() -> int = contador();
                 a(); a(); a();   // n de a -> 3
                 b();             // n de b -> 1 (su propia celda, independiente)
                 a() + b()        // a()->4, b()->2 => 6
             }",
        );
    }

    #[test]
    fn captura_transitiva_dos_niveles() {
        oracle_program(
            "fn sumador(x: int) -> fn(int) -> int { fn(y: int) -> int { x + y } }
             fn main() -> int { let add5: fn(int) -> int = sumador(5); add5(10) + add5(100) }",
        );
    }

    #[test]
    fn closures_hermanas_comparten_celda() {
        oracle_program(
            "struct Par { inc: fn(), get: fn() -> int }
             fn hacer() -> Par {
                 var n: int = 0;
                 Par { inc: fn() { n = n + 1; }, get: fn() -> int { n } }
             }
             fn main() -> int { let p: Par = hacer(); p.inc(); p.inc(); p.inc(); p.get() }",
        );
    }

    #[test]
    fn closure_en_arreglo_y_orden_superior() {
        oracle_program(
            "fn aplica_dos(f: fn(int) -> int, x: int) -> int { f(f(x)) }
             fn main() -> int {
                 let k: int = 3;
                 aplica_dos(fn(n: int) -> int { n + k }, 10)
             }",
        );
    }

    // ----- M5.1: enums (tipos suma) y construcción -----

    #[test]
    fn enum_construccion_oraculo() {
        // Ambos motores construyen variantes (con y sin payload) y coinciden en el
        // resultado. El payload se evalúa en orden antes de MakeEnum.
        oracle_program(
            "enum E { A(int, int), B }
             fn main() -> int { let x: E = E.A(2, 3); let y: E = E.B; print(x); print(y); 0 }",
        );
    }

    #[test]
    fn enum_recursivo_oraculo() {
        oracle_program(
            "enum Lista { Cons(int, Lista), Nil }
             fn main() -> int { let xs: Lista = Lista.Cons(1, Lista.Cons(2, Lista.Nil)); print(xs); 0 }",
        );
    }

    #[test]
    fn derive_show_oraculo() {
        // `@derive(Show)` genera `mostrar` (front-end → impls normales): el intérprete y la VM
        // deben producir la **misma** cadena. Se compara vía `len` (el oráculo mira el retorno).
        oracle_program(
            "@derive(Show)
             enum Color { Rojo, RGB(int, int, int) }
             @derive(Show)
             struct Punto { x: int, y: int }
             fn main() -> int {
                 let p = Punto { x: 3, y: 40 };
                 print(p.mostrar());
                 print(Color.RGB(1, 2, 3).mostrar());
                 p.mostrar().len() + Color.RGB(1, 2, 3).mostrar().len()
             }",
        );
    }

    #[test]
    fn enums_en_modo_estres() {
        // Construir enums (incl. recursivos) con el GC recolectando en cada punto
        // seguro: si el trazado del payload faltara, un valor vivo se liberaría.
        oracle_stress(
            "enum Lista { Cons(int, Lista), Nil }
             fn construir(n: int) -> Lista {
                 if (n == 0) { Lista.Nil } else { Lista.Cons(n, construir(n - 1)) }
             }
             fn main() -> int { let xs: Lista = construir(20); print(xs); 0 }",
        );
    }

    #[test]
    fn el_gc_libera_enums_inalcanzables() {
        // Cada llamada construye una lista enlazada que queda inalcanzable al
        // retornar. El mark-and-sweep debe barrer esos objetos de enum: el heap
        // queda acotado en vez de crecer sin parar.
        let src = r#"
            enum Lista { Cons(int, Lista), Nil }
            fn construir(n: int) -> Lista {
                if (n == 0) { Lista.Nil } else { Lista.Cons(n, construir(n - 1)) }
            }
            fn main() -> int {
                var i: int = 0;
                while (i < 50) { let xs: Lista = construir(10); i = i + 1; }
                0
            }
        "#;
        let tokens = crate::lexer::lex(src).unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        let compiled = compile_program(&prog).unwrap();
        let mut vm = Vm::new(&compiled);
        vm.run().expect("vm ok");
        // Sin GC habría ~550 objetos vivos; con barrido, muy pocos.
        assert!(vm.cur.heap.live() < 80, "el heap no se acotó: {} objetos vivos", vm.cur.heap.live());
    }

    // ----- M5.3: match en la VM (oráculo VM<->intérprete) -----

    #[test]
    fn match_recorrido_oraculo() {
        // Recorrer un enum recursivo con match: longitud y suma, en ambos motores.
        oracle_program(
            "enum Lista { Cons(int, Lista), Nil }
             fn longitud(xs: Lista) -> int { match (xs) { Lista.Cons(_, t) => 1 + longitud(t), Lista.Nil => 0 } }
             fn suma(xs: Lista) -> int { match (xs) { Lista.Cons(h, t) => h + suma(t), Lista.Nil => 0 } }
             fn main() -> int {
                 let xs: Lista = Lista.Cons(10, Lista.Cons(20, Lista.Cons(30, Lista.Nil)));
                 longitud(xs) * 100 + suma(xs)
             }",
        );
    }

    #[test]
    fn match_selecciona_brazo_oraculo() {
        // Variantes con distinta aridad de payload; cada brazo liga lo suyo.
        oracle_program(
            "enum Figura { Circulo(int), Rect(int, int), Punto }
             fn area(f: Figura) -> int {
                 match (f) { Figura.Circulo(r) => 3 * r * r, Figura.Rect(w, h) => w * h, Figura.Punto => 0 }
             }
             fn main() -> int { area(Figura.Rect(4, 5)) + area(Figura.Circulo(2)) + area(Figura.Punto) }",
        );
    }

    #[test]
    fn match_comodin_y_binding_oraculo() {
        // Comodín `_` (dentro de variante y suelto) y binding catch-all.
        oracle_program(
            "enum E { Uno, Dos, Otro }
             fn n(e: E) -> int { match (e) { E.Uno => 1, otro => 99 } }
             fn main() -> int { n(E.Uno) * 100 + n(E.Dos) }",
        );
    }

    #[test]
    fn match_en_modo_estres() {
        // La prueba clave de M5.3: con el GC recolectando en CADA punto seguro, el
        // escrutinio guardado en el local temporal y el payload extraído deben seguir
        // rooteados. Si faltara una raíz, recorrer la lista reventaría o cambiaría.
        oracle_stress(
            "enum Lista { Cons(int, Lista), Nil }
             fn construir(n: int) -> Lista { if (n == 0) { Lista.Nil } else { Lista.Cons(n, construir(n - 1)) } }
             fn suma(xs: Lista) -> int { match (xs) { Lista.Cons(h, t) => h + suma(t), Lista.Nil => 0 } }
             fn main() -> int { suma(construir(15)) }",
        );
    }

    #[test]
    fn match_binding_capturado_por_closure_oraculo() {
        // Interacción fina: un binding de match capturado por una closure debe
        // BOXEARSE (vivir en una celda). InitLocal sobre el slot del binding lo
        // maneja, igual que con un `let`. Ambos motores deben coincidir.
        oracle_program(
            "enum E { A(int), B(int), C }
             fn sumador(e: E) -> fn(int) -> int {
                 match (e) {
                     E.A(n) => fn(x: int) -> int { x + n },
                     E.B(n) => fn(x: int) -> int { x * n },
                     E.C    => fn(x: int) -> int { x },
                 }
             }
             fn main() -> int {
                 let f: fn(int) -> int = sumador(E.A(10));
                 let g: fn(int) -> int = sumador(E.B(3));
                 f(5) + g(5)
             }",
        );
    }

    #[test]
    fn match_anidado_en_expresiones_oraculo() {
        // match como expresión: su valor alimenta otra operación, y el cuerpo de un
        // brazo construye otra variante (resolución dentro del brazo).
        oracle_program(
            "enum Sem { Rojo, Verde }
             fn opuesto(s: Sem) -> Sem { match (s) { Sem.Rojo => Sem.Verde, Sem.Verde => Sem.Rojo } }
             fn a_int(s: Sem) -> int { match (s) { Sem.Rojo => 0, Sem.Verde => 1 } }
             fn main() -> int { a_int(opuesto(Sem.Rojo)) + a_int(opuesto(Sem.Verde)) * 10 }",
        );
    }

    // ----- M6.1: funciones genéricas (erasure: ambos motores coinciden) -----

    #[test]
    fn generica_identidad_oraculo() {
        // Con borrado de tipos, una función genérica solo mueve valores: el resultado
        // debe coincidir en intérprete y VM sin que el runtime sepa nada de T.
        oracle_program(
            "fn identidad<T>(x: T) -> T { x }
             fn main() -> int { let b: bool = identidad(true); let n: int = identidad(7); if (b) { n } else { 0 } }",
        );
    }

    #[test]
    fn generica_de_orden_superior_oraculo() {
        oracle_program(
            "fn aplicar<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }
             fn doble(n: int) -> int { n * 2 }
             fn main() -> int { aplicar(doble, 21) }",
        );
    }

    #[test]
    fn generica_sobre_arreglos_oraculo() {
        oracle_program(
            "fn par<T>(a: T, b: T) -> [T] { [a, b] }
             fn main() -> int { let xs: [int] = par(10, 32); xs[0] + xs[1] }",
        );
    }

    // ----- M6.2: tipos genéricos del usuario (erasure: ambos motores coinciden) -----

    #[test]
    fn enum_generico_oraculo() {
        oracle_program(
            "enum Caja<T> { Llena(T), Vacia }
             fn val(c: Caja<int>, def: int) -> int { match (c) { Caja.Llena(v) => v, Caja.Vacia => def } }
             fn main() -> int {
                 let a: Caja<int> = Caja.Llena(7);
                 let b: Caja<int> = Caja.Vacia;
                 val(a, 0) + val(b, 35)
             }",
        );
    }

    #[test]
    fn struct_generico_oraculo() {
        oracle_program(
            "struct Par<A, B> { primero: A, segundo: B }
             fn main() -> int {
                 let p: Par<int, bool> = Par { primero: 10, segundo: true };
                 if (p.segundo) { p.primero } else { 0 }
             }",
        );
    }

    // ----- M6.3: Option/Result y el operador ? (oráculo) -----

    #[test]
    fn try_result_oraculo() {
        oracle_program(
            "fn d(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } }
             fn calc(x: int, y: int, z: int) -> Result<int, string> { let q1: int = d(x, y)?; let q2: int = d(q1, z)?; Result.Ok(q1 + q2) }
             fn desemp(r: Result<int, string>) -> int { match (r) { Result.Ok(v) => v, Result.Err(_) => -1 } }
             fn main() -> int { desemp(calc(100, 5, 2)) * 100 + desemp(calc(100, 0, 2)) }",
        );
    }

    #[test]
    fn try_option_oraculo() {
        oracle_program(
            "fn primero(xs: [int]) -> Option<int> { if (xs.len() == 0) { Option.None } else { Option.Some(xs[0]) } }
             fn mas_uno(xs: [int]) -> Option<int> { let v: int = primero(xs)?; Option.Some(v + 1) }
             fn desemp(o: Option<int>) -> int { match (o) { Option.Some(v) => v, Option.None => -99 } }
             fn main() -> int { desemp(mas_uno([41])) * 100 + desemp(mas_uno([])) }",
        );
    }

    #[test]
    fn try_en_modo_estres() {
        // El ? construye/propaga valores de enum (Result) bajo el GC en cada punto
        // seguro: el escrutinio del ? vive en su local temporal y queda rooteado.
        oracle_stress(
            "fn d(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } }
             fn cadena(n: int) -> Result<int, string> { let a: int = d(n, 2)?; let b: int = d(a, 1)?; Result.Ok(a + b) }
             fn desemp(r: Result<int, string>) -> int { match (r) { Result.Ok(v) => v, Result.Err(_) => -1 } }
             fn main() -> int { desemp(cadena(40)) }",
        );
    }

    #[test]
    fn enum_generico_recursivo_en_estres() {
        // Lista genérica construida con un tipo concreto, recorrida con match, bajo el
        // GC en modo estrés: los valores de enum genérico se trazan como cualquier enum.
        oracle_stress(
            "enum Lista<T> { Cons(T, Lista<T>), Nil }
             fn suma(xs: Lista<int>) -> int { match (xs) { Lista.Cons(h, t) => h + suma(t), Lista.Nil => 0 } }
             fn construir(n: int) -> Lista<int> { if (n == 0) { Lista.Nil } else { Lista.Cons(n, construir(n - 1)) } }
             fn main() -> int { suma(construir(15)) }",
        );
    }

    // ----- M4.3: recolección de basura -----

    #[test]
    fn el_gc_no_rompe_programas_en_modo_estres() {
        // Si el GC liberara algo vivo (raíz faltante), estos resultados cambiarían.
        oracle_stress("fn fib(n: int) -> int { if (n < 2) { n } else { fib(n-1) + fib(n-2) } } fn main() -> int { fib(12) }");
        oracle_stress(
            "fn main() -> int {
                 var xs: [int] = [];
                 var i: int = 0;
                 while (i < 30) { xs.push(i * i); i = i + 1; }
                 var s: int = 0; var j: int = 0;
                 while (j < xs.len()) { s = s + xs[j]; j = j + 1; }
                 s
             }",
        );
        oracle_stress(
            "struct P { x: int, y: int }
             fn main() -> int { var p: P = P { x: 1, y: 2 }; p.x = 10; p.x + p.y }",
        );
        oracle_stress(
            "fn contador() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
             fn main() -> int { let c: fn() -> int = contador(); c(); c(); c(); c() }",
        );
    }

    #[test]
    fn el_gc_libera_ciclos() {
        // Cada 'make_cycle' crea un ciclo (celda <-> closure) que queda inalcanzable
        // al retornar. Con conteo de referencias se filtrarían (~200 objetos); el
        // mark-and-sweep los libera, así que el heap queda acotado.
        let src = r#"
            fn make_cycle() {
                var f: fn() = fn() {};
                f = fn() { f(); };
            }
            fn main() -> int {
                var i: int = 0;
                while (i < 100) { make_cycle(); i = i + 1; }
                0
            }
        "#;
        let tokens = crate::lexer::lex(src).unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        let compiled = compile_program(&prog).unwrap();
        let mut vm = Vm::new(&compiled);
        vm.run().expect("vm ok");
        // Sin GC habría ~200 objetos vivos; con mark-and-sweep, muy pocos.
        assert!(vm.cur.heap.live() < 80, "el heap no se acotó: {} objetos vivos", vm.cur.heap.live());
    }

    // ----- M7.1: UFCS (azúcar de front-end; ambos motores ven la llamada ya bajada) -----

    #[test]
    fn ufcs_oraculo() {
        // Función del usuario y builtin (len) usados como métodos.
        oracle_program(r#"
            fn suma(a: int, b: int) -> int { a + b }
            fn main() -> int {
                let xs: [int] = [1, 2, 3, 4];
                let n: int = xs.len();      // len(xs) = 4
                let v: int = 10;
                v.suma(n)                    // suma(10, 4) = 14
            }
        "#);
    }

    #[test]
    fn ufcs_encadenado_oraculo() {
        oracle_program(r#"
            fn doble(x: int) -> int { x * 2 }
            fn inc(x: int) -> int { x + 1 }
            fn main() -> int {
                let v: int = 5;
                v.doble().inc().doble()      // doble(inc(doble(5))) = 22
            }
        "#);
    }

    #[test]
    fn ufcs_sobre_struct_oraculo() {
        // 'norma1' no es campo de Punto -> UFCS; 'p.x' sigue siendo acceso a campo.
        oracle_program(r#"
            struct Punto { x: int, y: int }
            fn norma1(p: Punto) -> int { p.x + p.y }
            fn main() -> int {
                let p: Punto = Punto { x: 7, y: 6 };
                p.norma1() + p.x             // 13 + 7 = 20
            }
        "#);
    }

    #[test]
    fn ufcs_campo_funcion_oraculo() {
        // 'op' ES un campo de tipo función: c.op(x) llama al campo, no es UFCS.
        oracle_program(r#"
            struct Caja { op: fn(int) -> int }
            fn main() -> int {
                let c: Caja = Caja { op: fn(x: int) -> int { x + 100 } };
                c.op(41)                     // (c.op)(41) = 141
            }
        "#);
    }

    #[test]
    fn ufcs_en_modo_estres() {
        // El receptor y los argumentos viven en el heap: el GC en estrés no debe
        // romper la llamada UFCS bajada.
        oracle_stress(r#"
            fn cabeza(xs: [int]) -> int { xs[0] }
            fn cola_suma(xs: [int]) -> int {
                var s: int = 0;
                var i: int = 1;
                while (i < xs.len()) { s = s + xs[i]; i = i + 1; }
                s
            }
            fn main() -> int {
                let xs: [int] = [10, 20, 30, 40];
                xs.cabeza() + xs.cola_suma()   // 10 + 90 = 100
            }
        "#);
    }

    // ----- M7.2: pipelines (azúcar de parser; ambos motores ven la llamada bajada) -----

    #[test]
    fn pipeline_oraculo() {
        oracle_program(r#"
            fn doble(x: int) -> int { x * 2 }
            fn inc(x: int) -> int { x + 1 }
            fn suma(a: int, b: int) -> int { a + b }
            fn main() -> int {
                let v: int = 5;
                let a: int = v |> doble |> inc;   // inc(doble(5)) = 11
                let b: int = v |> suma(100);       // suma(5, 100) = 105
                a + b                               // 116
            }
        "#);
    }

    #[test]
    fn pipeline_y_ufcs_oraculo() {
        // `.f()` (UFCS) y `|> f` (pipeline) componen sobre el mismo valor.
        oracle_program(r#"
            fn doble(x: int) -> int { x * 2 }
            fn inc(x: int) -> int { x + 1 }
            fn main() -> int {
                let v: int = 5;
                v.doble() |> inc |> doble           // doble(inc(doble(5))) = 22
            }
        "#);
    }

    #[test]
    fn pipeline_en_modo_estres() {
        // El valor que fluye por el pipeline es un arreglo en el heap.
        oracle_stress(r#"
            fn suma_todo(xs: [int]) -> int {
                var s: int = 0;
                var i: int = 0;
                while (i < xs.len()) { s = s + xs[i]; i = i + 1; }
                s
            }
            fn con_extra(xs: [int], x: int) -> [int] { xs.push(x); xs }
            fn main() -> int {
                let xs: [int] = [1, 2, 3];
                xs |> con_extra(4) |> suma_todo     // suma_todo(con_extra(xs, 4)) = 10
            }
        "#);
    }

    // ----- M7.3: stdlib (prelude map/filter/fold escrito en raylang) -----

    #[test]
    fn prelude_map_filter_fold_oraculo() {
        oracle_program(r#"
            fn doble(x: int) -> int { x * 2 }
            fn par(x: int) -> bool { x % 2 == 0 }
            fn suma(a: int, b: int) -> int { a + b }
            fn main() -> int {
                let xs: [int] = [1, 2, 3, 4, 5];
                let ys: [int] = xs.map(doble).filter(par);  // [2,4,6,8,10]
                ys.fold(0, suma)                             // 30
            }
        "#);
    }

    #[test]
    fn prelude_pipeline_oraculo() {
        // El mismo cálculo, en estilo pipeline.
        oracle_program(r#"
            fn doble(x: int) -> int { x * 2 }
            fn par(x: int) -> bool { x % 2 == 0 }
            fn suma(a: int, b: int) -> int { a + b }
            fn main() -> int {
                let xs: [int] = [1, 2, 3, 4, 5];
                xs |> filter(par) |> map(doble) |> fold(0, suma)  // [2,4]->[4,8]->12
            }
        "#);
    }

    #[test]
    fn prelude_con_closures_oraculo() {
        // map/fold con funciones anónimas inline.
        oracle_program(r#"
            fn main() -> int {
                let xs: [int] = [1, 2, 3, 4];
                let cuadrados: [int] = xs |> map(fn(x: int) -> int { x * x });  // [1,4,9,16]
                cuadrados.fold(0, fn(a: int, x: int) -> int { a + x })           // 30
            }
        "#);
    }

    #[test]
    fn prelude_en_modo_estres() {
        // map y filter alojan arreglos nuevos en el heap: el GC en estrés debe
        // mantenerlos vivos durante toda la cadena.
        oracle_stress(r#"
            fn inc(x: int) -> int { x + 1 }
            fn pos(x: int) -> bool { x > 3 }
            fn suma(a: int, b: int) -> int { a + b }
            fn main() -> int {
                let xs: [int] = [1, 2, 3, 4, 5, 6];
                xs.map(inc).filter(pos).fold(0, suma)   // [2..7]->[4,5,6,7]->22
            }
        "#);
    }

    // ----- M8.1: inferencia local (solo checker; el runtime no cambia) -----

    #[test]
    fn inferencia_local_oraculo() {
        // Variables inferidas (int, [int], struct, enum genérico) deben dar el mismo
        // resultado en ambos motores: la inferencia se borra antes de ejecutar.
        oracle_program(r#"
            struct Punto { x: int, y: int }
            enum Caja<T> { Llena(T), Vacia }
            fn doble(x: int) -> int { x * 2 }
            fn main() -> int {
                let x = 3;
                let xs = [10, 20, 30];
                let p = Punto { x: 7, y: 6 };
                let c = Caja.Llena(5);
                var total = 0;
                total = total + x.doble();
                let dentro = match (c) { Caja.Llena(v) => v, Caja.Vacia => 0 };
                total + xs[0] + p.x + p.y + dentro   // 6 + 10 + 7 + 6 + 5 = 34
            }
        "#);
    }

    // ----- M9.1: traits (erasure; ambos motores ven funciones y llamadas ordinarias) -----

    #[test]
    fn traits_despacho_estatico_oraculo() {
        // Un trait implementado para un struct, un enum y un primitivo: los métodos se
        // bajan a funciones mangladas y las llamadas por punto a llamadas ordinarias,
        // así que la VM y el intérprete deben coincidir sin tocar el runtime.
        oracle_program(r#"
            trait Valor { fn valor(self) -> int; }
            struct Punto { x: int, y: int }
            enum Moneda { Cara, Cruz }
            impl Valor for Punto { fn valor(self) -> int { self.x + self.y } }
            impl Valor for Moneda {
                fn valor(self) -> int { match (self) { Moneda.Cara => 1, Moneda.Cruz => 0 } }
            }
            impl Valor for int { fn valor(self) -> int { self } }
            fn main() -> int {
                let p = Punto { x: 3, y: 4 };
                p.valor() + Moneda.Cara.valor() + 10.valor()   // 7 + 1 + 10 = 18
            }
        "#);
    }

    #[test]
    fn traits_self_y_metodos_internos_oraculo() {
        // `Self` en el retorno, parámetros extra, y un método que llama a otro del mismo
        // impl (`self.sumar(self)`): bajo estrés del GC para validar las raíces.
        oracle_stress(r#"
            trait Punteable {
                fn sumar(self, otro: Punto) -> Punto;
                fn doble(self) -> Self;
                fn norma(self) -> int;
            }
            struct Punto { x: int, y: int }
            impl Punteable for Punto {
                fn sumar(self, otro: Punto) -> Punto { Punto { x: self.x + otro.x, y: self.y + otro.y } }
                fn doble(self) -> Self { self.sumar(self) }
                fn norma(self) -> int { self.x * self.x + self.y * self.y }
            }
            fn main() -> int {
                let p = Punto { x: 3, y: 4 };
                p.doble().norma()   // (6,8) -> 36 + 64 = 100
            }
        "#);
    }

    // ----- M9.2: bounds vía paso de diccionarios -----

    #[test]
    fn bounds_diccionarios_oraculo() {
        // Genérico acotado sobre struct y primitivo + reenvío entre genéricos. Los
        // diccionarios son valores función; ambos motores deben coincidir.
        oracle_program(r#"
            trait Valor { fn valor(self) -> int; }
            struct Punto { x: int, y: int }
            impl Valor for Punto { fn valor(self) -> int { self.x + self.y } }
            impl Valor for int { fn valor(self) -> int { self } }
            fn doble_valor<T: Valor>(x: T) -> int { x.valor() + x.valor() }
            fn suma_tres<T: Valor>(a: T, b: T, c: T) -> int {
                doble_valor(a) + b.valor() + c.valor()   // reenvío del diccionario
            }
            fn main() -> int {
                let p = Punto { x: 3, y: 4 };
                doble_valor(p) + doble_valor(10) + suma_tres(p, p, p)   // 14 + 20 + 28 = 62
            }
        "#);
    }

    #[test]
    fn bounds_multiples_oraculo() {
        // T: A + B — dos diccionarios. Bajo estrés del GC.
        oracle_stress(r#"
            trait Nombre { fn largo(self) -> int; }
            trait Doble { fn doble(self) -> int; }
            struct Cosa { n: int }
            impl Nombre for Cosa { fn largo(self) -> int { self.n } }
            impl Doble for Cosa { fn doble(self) -> int { self.n + self.n } }
            fn usar<T: Nombre + Doble>(x: T) -> int { x.largo() + x.doble() }
            fn main() -> int {
                let c = Cosa { n: 5 };
                usar(c)   // 5 + 10 = 15
            }
        "#);
    }

    // ----- M9.2b: impls genéricos -----

    #[test]
    fn impl_generico_sin_bounds_oraculo() {
        // `impl<T> Trait for Caja<T>` cuyo método no usa T: el método manglado es genérico
        // pero sin diccionarios. Despacha igual para Caja<int> y Caja<string>.
        oracle_program(r#"
            struct Caja<T> { contenido: T }
            trait Contar { fn contar(self) -> int; }
            impl<T> Contar for Caja<T> { fn contar(self) -> int { 1 } }
            fn main() -> int {
                let c = Caja { contenido: 42 };
                let s = Caja { contenido: "hola" };
                c.contar() + s.contar()   // 1 + 1 = 2
            }
        "#);
    }

    #[test]
    fn impl_generico_acotado_llamada_directa_oraculo() {
        // `impl<T: Mostrable> Mostrable for Caja<T>`: el cuerpo usa T.mostrar() (vía el
        // diccionario interno). Llamada directa sobre Caja<int> → el dict interno es el de
        // int (plano). Es M9.2b-1: el caso anidado (pasar Caja a otro genérico) es -2.
        oracle_stress(r#"
            struct Caja<T> { contenido: T }
            trait Medir { fn medir(self) -> int; }
            impl Medir for int { fn medir(self) -> int { self } }
            impl<T: Medir> Medir for Caja<T> { fn medir(self) -> int { self.contenido.medir() + 1 } }
            fn main() -> int {
                let c = Caja { contenido: 41 };
                c.medir()   // 41 + 1 = 42
            }
        "#);
    }

    #[test]
    fn impl_generico_diccionario_anidado_oraculo() {
        // M9.2b-2: pasar un Caja<int> a otro genérico acotado. El diccionario de Caja<int> es
        // un **closure** que captura el de int. Ambos motores deben coincidir.
        oracle_program(r#"
            struct Caja<T> { contenido: T }
            trait Medir { fn medir(self) -> int; }
            impl Medir for int { fn medir(self) -> int { self } }
            impl<T: Medir> Medir for Caja<T> { fn medir(self) -> int { self.contenido.medir() + 1 } }
            fn medir_dos<X: Medir>(a: X, b: X) -> int { a.medir() + b.medir() }
            fn main() -> int {
                let c = Caja { contenido: 10 };
                medir_dos(c, c)   // (10+1) * 2 = 22
            }
        "#);
    }

    #[test]
    fn impl_generico_anidado_profundo_estres() {
        // Caja<Caja<int>>: un diccionario anidado que contiene otro. Bajo estrés del GC,
        // porque los closures-diccionario son objetos del heap (sus raíces deben trazarse).
        oracle_stress(r#"
            struct Caja<T> { contenido: T }
            trait Medir { fn medir(self) -> int; }
            impl Medir for int { fn medir(self) -> int { self } }
            impl<T: Medir> Medir for Caja<T> { fn medir(self) -> int { self.contenido.medir() + 1 } }
            fn medir_uno<X: Medir>(x: X) -> int { x.medir() }
            fn main() -> int {
                let c2 = Caja { contenido: Caja { contenido: 100 } };
                c2.medir() + medir_uno(c2)   // 102 + 102 = 204
            }
        "#);
    }

    // ----- M11.1: stdlib de string -----

    #[test]
    fn string_concat_len_oraculo() {
        // Concatenación con `+`, len de string y to_string; el resultado es un int.
        oracle_program(r#"
            fn main() -> int {
                let s = "hola, " + "mundo";       // concat
                let etiqueta = "n=" + to_string(s.len());
                print(etiqueta);                   // n=11
                s.len() + "123".len()               // 11 + 3 = 14
            }
        "#);
    }

    #[test]
    fn string_to_string_de_varios_tipos_oraculo() {
        oracle_program(r#"
            fn main() -> int {
                print(to_string(42));      // 42
                print(to_string(true));    // true
                print(to_string("ya"));    // ya (identidad)
                to_string(true).len() + to_string(false).len()   // 4 + 5 = 9
            }
        "#);
    }

    #[test]
    fn string_ufcs_oraculo() {
        // UFCS sobre los builtins de string (s.len(), n.to_string()).
        oracle_program(r#"
            fn main() -> int {
                let s = "raylang";
                print(s.len().to_string());   // 7
                s.len()
            }
        "#);
    }

    #[test]
    fn string_trim_split_oraculo() {
        oracle_program(r#"
            fn main() -> int {
                let limpio = "  hola  ".trim();
                print("[" + limpio + "]");        // [hola]
                let campos = "a,bb,ccc".split(",");
                print(campos[1]);                  // bb
                campos.len() + limpio.len()          // 3 + 4 = 7
            }
        "#);
    }

    #[test]
    fn char_tipo_oraculo() {
        // M11.4c-1: literal de char, anotación, ==, to_string, y @derive(Eq, Show) con campo char.
        oracle_program(r#"
            @derive(Eq, Show)
            struct Tecla { c: char, repetida: bool }
            fn clase(c: char) -> int {
                if (c == 'a') { 1 } else { if (c == '\n') { 2 } else { 0 } }
            }
            fn main() -> int {
                let c: char = 'z';
                print(c);                              // z
                print(to_string('x') + "!");           // x!
                print('a' == 'a');                     // true
                let t = Tecla { c: 'q', repetida: false };
                print(t.mostrar());                    // Tecla { c: q, repetida: false }
                print(t.igual(Tecla { c: 'q', repetida: false }));  // true
                clase('a') + clase('\n') + clase('z')  // 1 + 2 + 0 = 3
            }
        "#);
    }

    #[test]
    fn char_indexar_y_chars_oraculo() {
        // M11.4c-2: s[i] -> char, chars(s) -> [char] (asigna heap → estrés del GC).
        oracle_stress(r#"
            fn cuenta(s: string, c: char) -> int {
                var n = 0;
                var i = 0;
                while (i < s.len()) {
                    if (s[i] == c) { n = n + 1; }
                    i = i + 1;
                }
                n
            }
            fn main() -> int {
                let s = "racecar";
                print(s[0]);                       // r
                print(s[3]);                       // e
                let cs = s.chars();
                print(cs[1]);                      // a
                print(cs.len());                    // 7
                cuenta(s, 'r') + cuenta(s, 'c') + "hola".chars().len()  // 2 + 2 + 4 = 8
            }
        "#);
    }

    #[test]
    fn string_contains_replace_oraculo() {
        // contains -> bool; replace asigna un string nuevo (heap en la VM). Oráculo + estrés del GC.
        oracle_stress(r#"
            fn main() -> int {
                let s = "hola mundo, hola raylang";
                print(s.contains("mundo"));            // true
                print(s.contains("python"));           // false
                let r = s.replace("hola", "HOLA");
                print(r);                              // HOLA mundo, HOLA raylang
                print("a.b.c".replace(".", "/"));      // a/b/c
                if (s.contains("raylang")) { r.len() } else { 0 }  // 24
            }
        "#);
    }

    /// M43.1: **hashes de producción vía `ring`** (`sha256`/`sha512`/`sha1`). Doble red: el **oráculo**
    /// (interp==vm) verifica CONSISTENCIA —ambos motores llaman al mismo `ring`—, y los **vectores conocidos**
    /// (NIST/RFC) verifican CORRECCIÓN: el programa devuelve 1 solo si el hex calculado casa con el esperado,
    /// así un error de corrección da 0 (que el oráculo por sí solo no detectaría si ambos motores fallaran
    /// igual). Cubre entrada vacía y las tres funciones.
    #[test]
    fn sha_digests_oraculo() {
        let casos = [
            ("sha256", "abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            ("sha256", "", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            (
                "sha512",
                "abc",
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
            ("sha1", "abc", "a9993e364706816aba3e25717850c26c9cd0d89d"),
        ];
        for (f, input, hex) in casos {
            let src = format!(
                "fn main() -> int {{ if (to_string({f}(\"{input}\".to_bytes())) == \"{hex}\") {{ 1 }} else {{ 0 }} }}"
            );
            let tokens = crate::lexer::lex(&src).expect("lex ok");
            let mut prog = crate::parser::parse(tokens).expect("parse ok");
            crate::checker::check(&mut prog).expect("check ok");
            let interp = crate::interpreter::run(&prog).expect("interp ok");
            let compiled = compile_program(&prog).expect("compila");
            let vm = run_program(&compiled).expect("vm ok");
            assert_eq!(interp, vm, "VM≠intérprete en {f}(\"{input}\")");
            assert_eq!(vm, Value::Int(1), "{f}(\"{input}\") no casó con el vector conocido");
        }
        // Estrés del GC: cada hash asigna un `bytes` nuevo en el heap; encadenar hashes debe sobrevivir a
        // una recolección en cada paso seguro (destapa raíces faltantes).
        oracle_stress(
            r#"
            fn main() -> int {
                var acc = "semilla".to_bytes();
                var i = 0;
                while (i < 50) {
                    acc = sha256(acc);       // 32 octetos, heap nuevo cada vuelta
                    acc = sha512(acc);       // 64 octetos
                    acc = sha1(acc);         // 20 octetos
                    i = i + 1;
                }
                acc.len()                     // 20 (último es sha1)
            }
        "#,
        );
    }

    /// M43.2: **HMAC-SHA256** vía `ring`. Misma doble red: oráculo (interp==vm) + vector conocido
    /// (RFC 4231, Test Case 2: clave `"Jefe"`, mensaje `"what do ya want for nothing?"`).
    #[test]
    fn hmac_sha256_oraculo() {
        let src = format!(
            "fn main() -> int {{ if (to_string(hmac_sha256(\"Jefe\".to_bytes(), \"{}\".to_bytes())) == \"{}\") {{ 1 }} else {{ 0 }} }}",
            "what do ya want for nothing?",
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let tokens = crate::lexer::lex(&src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("interp ok");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect("vm ok");
        assert_eq!(interp, vm, "VM≠intérprete en hmac_sha256");
        assert_eq!(vm, Value::Int(1), "hmac_sha256 no casó con el vector RFC 4231");
        // Estrés de GC: HMAC en cadena (clave y mensaje del paso previo).
        oracle_stress(
            r#"
            fn main() -> int {
                var k = "clave".to_bytes();
                var m = "mensaje".to_bytes();
                var i = 0;
                while (i < 50) {
                    let t = hmac_sha256(k, m);
                    k = t;
                    m = sha256(t);
                    i = i + 1;
                }
                k.len()                       // 32
            }
        "#,
        );
    }

    /// M43.3: **Ed25519** vía `ring` (`sign`/`verify`/`public_key`). Oráculo (interp==vm) + validación
    /// RELACIONAL de corrección con `ring` como impl de confianza: la firma **verifica**, un mensaje
    /// alterado **no**, la semilla corta da `None`, y firmar dos veces da lo mismo (determinismo, RFC 8032).
    /// El programa devuelve 1 solo si TODO cuadra → un fallo de cableado da 0. La semilla son 32 octetos
    /// ASCII (`to_bytes` de 32 chars) para no depender de literales de byte largos.
    #[test]
    fn ed25519_oraculo() {
        let src = r#"
            fn main() -> int {
                let seed = "0123456789abcdef0123456789abcdef".to_bytes();   // 32 octetos
                let msg = "mensaje firmado".to_bytes();
                match (ed25519_public_key(seed)) {
                    Option.Some(pk) => {
                        match (ed25519_sign(seed, msg)) {
                            Option.Some(sig) => {
                                let ok = ed25519_verify(pk, msg, sig);                       // true
                                let alterado = ed25519_verify(pk, "mensaje alterad".to_bytes(), sig); // false
                                let otra = ed25519_sign(seed, msg);                           // determinista
                                let det = match (otra) {
                                    Option.Some(s2) => to_string(s2) == to_string(sig),       // true
                                    Option.None => false,
                                };
                                let corta = match (ed25519_public_key("corta".to_bytes())) {   // None (no 32)
                                    Option.Some(x) => false,
                                    Option.None => true,
                                };
                                if (ok && !alterado && det && corta) { 1 } else { 0 }
                            },
                            Option.None => 0,
                        }
                    },
                    Option.None => 0,
                }
            }
        "#;
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("interp ok");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect("vm ok");
        assert_eq!(interp, vm, "VM≠intérprete en ed25519");
        assert_eq!(vm, Value::Int(1), "Ed25519: falló roundtrip/manipulación/None/determinismo");
    }

    /// M43.4: **ChaCha20-Poly1305 AEAD** vía `ring`. Oráculo (interp==vm) + validación relacional:
    /// `seal` luego `open` recupera el texto, alterar el `aad` hace fallar la autenticación (`None`), y una
    /// clave de mal tamaño da `None` en `seal`. Devuelve 1 solo si todo cuadra.
    #[test]
    fn chacha20poly1305_oraculo() {
        let src = r#"
            fn main() -> int {
                let key = "0123456789abcdef0123456789abcdef".to_bytes();   // 32 octetos
                let nonce = "nonce-de-12b".to_bytes();                     // 12 octetos
                let aad = "cabecera".to_bytes();
                let pt = "texto secreto".to_bytes();
                match (chacha20poly1305_seal(key, nonce, aad, pt)) {
                    Option.Some(ct) => {
                        let recuperado = match (chacha20poly1305_open(key, nonce, aad, ct)) {
                            Option.Some(p) => to_string(p) == to_string(pt),
                            Option.None => false,
                        };
                        let manipulado = match (chacha20poly1305_open(key, nonce, "otra cab".to_bytes(), ct)) {
                            Option.Some(p) => false,
                            Option.None => true,
                        };
                        let corta = match (chacha20poly1305_seal("corta".to_bytes(), nonce, aad, pt)) {
                            Option.Some(x) => false,
                            Option.None => true,
                        };
                        if (recuperado && manipulado && corta) { 1 } else { 0 }
                    },
                    Option.None => 0,
                }
            }
        "#;
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("interp ok");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect("vm ok");
        assert_eq!(interp, vm, "VM≠intérprete en chacha20poly1305");
        assert_eq!(vm, Value::Int(1), "AEAD: falló roundtrip/autenticación/tamaño");
    }

    #[test]
    fn string_stdlib_m117_oraculo() {
        // M11.7a: starts_with/ends_with (bool); to_upper/to_lower/substring/repeat/join asignan
        // string nuevo (heap en la VM); index_of construye Option en el prelude. Oráculo + estrés GC.
        oracle_stress(r#"
            fn pos(o: Option<int>, def: int) -> int {
                match (o) { Option.Some(i) => i, Option.None => def, }
            }
            fn main() -> int {
                let s = "Hola, Mundo";
                print(s.starts_with("Hola"));      // true
                print(s.ends_with("xyz"));         // false
                print(s.to_upper());               // HOLA, MUNDO
                print(s.to_lower());               // hola, mundo
                print(s.substring(0, 4));          // Hola
                print(s.substring(6, 100));        // Mundo (clamp)
                print("ab".repeat(3));             // ababab
                print("".repeat(5));               // (vacío)
                let partes = ["a", "b", "c"];
                print(join(partes, "-"));          // a-b-c
                print(pos(index_of(s, "Mundo"), 0 - 1));   // 6
                print(pos(index_of(s, "zzz"), 0 - 1));      // -1
                s.substring(6, 11).len() + pos(index_of(s, "Mundo"), 0)  // 5 + 6 = 11
            }
        "#);
    }

    #[test]
    fn array_stdlib_m117b_oraculo() {
        // M11.7b: concat (a+b), reverse, pop (muta + Option), contains, position. reverse/pop/concat
        // asignan en el heap → estrés del GC; pop construye Option en el prelude.
        oracle_stress(r#"
            fn idx(o: Option<int>, def: int) -> int {
                match (o) { Option.Some(i) => i, Option.None => def, }
            }
            fn ult(o: Option<int>, def: int) -> int {
                match (o) { Option.Some(x) => x, Option.None => def, }
            }
            fn main() -> int {
                let a = [1, 2, 3];
                let b = [4, 5];
                let c = a + b;                      // [1,2,3,4,5]
                print(c.len());                      // 5
                let r = c.reverse();                 // [5,4,3,2,1]
                print(r[0]);                        // 5
                print(c.contains(4));               // true
                print(c.contains(99));              // false
                print(idx(position(c, 3), 0 - 1));  // 2
                print(idx(position(c, 99), 0 - 1)); // -1
                let v = [10, 20, 30];
                let x = ult(pop(v), 0);             // 30, y v queda [10,20]
                print(v.len());                      // 2
                x + c.len() + r[1]                   // 30 + 5 + 4 = 39
            }
        "#);
    }

    #[test]
    fn sort_ord_oraculo() {
        // M11.7d: sort<T: Ord> (bound → diccionarios M9.2) sobre primitivos y un tipo de usuario
        // que implementa Ord. Asigna arreglos en el heap → estrés del GC.
        oracle_stress(r#"
            struct Caja { peso: int }
            impl Ord for Caja {
                fn menor(self, otro: Caja) -> bool { self.peso < otro.peso }
            }
            fn main() -> int {
                let xs = sort([3, 1, 4, 1, 5, 9, 2, 6]);
                print(xs[0]); print(xs[7]);             // 1 ... 9
                let cs = sort(['c', 'a', 'b']);
                print(cs[0]);                            // a
                let cajas = sort([Caja { peso: 30 }, Caja { peso: 10 }, Caja { peso: 20 }]);
                print(cajas[0].peso);                    // 10
                print(cajas[2].peso);                    // 30
                xs[0] + xs[7] + cajas[0].peso            // 1 + 9 + 10 = 20
            }
        "#);
    }

    #[test]
    fn string_split_estres_gc() {
        // split asigna un arreglo (objeto del heap). Bajo estrés del GC: si una raíz faltara,
        // el arreglo recién creado se liberaría y el resultado cambiaría.
        oracle_stress(r#"
            fn main() -> int {
                let partes = "uno:dos:tres:cuatro".trim().split(":");
                let total = partes.len() + partes[0].len() + partes[3].len();
                print(partes[2]);                  // tres
                total                              // 4 + 3 + 6 = 13
            }
        "#);
    }

    #[test]
    fn parse_int_oraculo() {
        // parse_int es determinista (no toca stdin) → oráculo VM↔intérprete. Construye Option
        // en el prelude (raylang); el resultado debe coincidir en ambos motores.
        oracle_program(r#"
            fn valor(o: Option<int>, def: int) -> int {
                match (o) {
                    Option.Some(n) => n,
                    Option.None => def,
                }
            }
            fn main() -> int {
                let a = valor(parse_int("42"), 0);        // 42
                let b = valor(parse_int("  -7 "), 0);     // -7 (trim)
                let c = valor(parse_int("xyz"), 100);     // 100 (None)
                a + b + c                                 // 135
            }
        "#);
    }

    #[test]
    fn parse_float_oraculo() {
        // M14: parse_float, como parse_int, es determinista → oráculo. El formateo de float es
        // el mismo f64 de Rust en ambos motores, así que los valores coinciden.
        oracle_program(r#"
            fn main() -> int {
                let ok = match (parse_float("3.14")) { Option.Some(f) => f, Option.None => 0.0 };
                let no = match (parse_float("hola")) { Option.Some(_) => 1, Option.None => 0 };
                let ent = match (parse_float("42")) { Option.Some(f) => f, Option.None => 0.0 };
                // 3.14*100 = 314, 42.0 → 42; no=0. Resultado 314 + 42 + 0 = 356.
                let a: int = if (ok * 100.0 == 314.0) { 314 } else { -1 };
                let b: int = if (ent == 42.0) { 42 } else { -1 };
                a + b + no
            }
        "#);
    }

    #[test]
    fn args_y_env_oraculo() {
        // En el proceso de test no se fijan args (→ []) y la variable no existe (→ None): ambos
        // motores deben coincidir. (El comportamiento "real" se prueba por subproceso en io_cli.)
        oracle_program(r#"
            fn main() -> int {
                let n = args().len();                       // 0
                let e = match (env("RAYLANG_NO_EXISTE_XYZ")) {
                    Option.Some(_) => 1,
                    Option.None => 0,
                };
                n + e                                      // 0
            }
        "#);
    }

    #[test]
    fn read_file_inexistente_es_err_oraculo() {
        // Leer un archivo inexistente es determinista (misma llamada a std::fs en ambos motores) →
        // oráculo. Construye Result en el prelude vía el arreglo etiquetado; debe coincidir.
        oracle_program(r#"
            fn main() -> int {
                match (read_file("/raylang_no_existe_xyz_123.txt")) {
                    Result.Ok(_) => 0,
                    Result.Err(_) => 1,
                }
            }
        "#);
    }

    #[test]
    fn parse_int_option_construido_en_el_heap_estres_gc() {
        // El [int] del primitivo y el Option que arma el prelude son objetos del heap. Bajo
        // estrés del GC: si una raíz faltara, el valor vivo se liberaría.
        oracle_stress(r#"
            fn main() -> int {
                let xs = ["1", "2", "no", "4"];
                var suma = 0;
                var i = 0;
                while (i < xs.len()) {
                    match (parse_int(xs[i])) {
                        Option.Some(n) => { suma = suma + n; },
                        Option.None => {},
                    }
                    i = i + 1;
                }
                suma                               // 1 + 2 + 4 = 7
            }
        "#);
    }

    // ----- M9.3a: métodos por defecto -----

    #[test]
    fn metodos_por_defecto_oraculo() {
        // Defecto heredado, defecto que llama a otro método, y redefinición. El método
        // sintetizado es una función ordinaria: ambos motores deben coincidir.
        oracle_program(r#"
            trait Valor {
                fn base(self) -> int;
                fn doble(self) -> int { self.base() + self.base() }   // defecto usa otro
                fn diez(self) -> int { 10 }                            // defecto constante
            }
            struct A { n: int }
            impl Valor for A { fn base(self) -> int { self.n } }       // hereda doble y diez
            struct B { n: int }
            impl Valor for B {
                fn base(self) -> int { self.n }
                fn doble(self) -> int { self.n * 100 }                 // redefine doble
            }
            fn main() -> int {
                let a = A { n: 3 };
                let b = B { n: 4 };
                a.doble() + a.diez() + b.doble() + b.diez()   // 6 + 10 + 400 + 10 = 426
            }
        "#);
    }

    #[test]
    fn metodos_por_defecto_via_bound_oraculo() {
        // Un método por defecto invocado desde un genérico acotado (M9.2 + M9.3a).
        oracle_stress(r#"
            trait Saludo {
                fn nombre(self) -> int;
                fn doble_nombre(self) -> int { self.nombre() + self.nombre() }
            }
            struct P { v: int }
            impl Saludo for P { fn nombre(self) -> int { self.v } }
            fn usar<T: Saludo>(x: T) -> int { x.doble_nombre() }
            fn main() -> int { let p = P { v: 21 }; usar(p) }   // 42
        "#);
    }

    // ----- M9.3b: trait objects (despacho dinámico) -----

    #[test]
    fn trait_objects_despacho_dinamico_oraculo() {
        // Arreglo heterogéneo de trait objects + despacho por valor. El trait object se
        // realiza como un struct sintetizado (la vtable); ambos motores deben coincidir.
        oracle_program(r#"
            trait Figura { fn area(self) -> int; }
            struct Cuadrado { lado: int }
            impl Figura for Cuadrado { fn area(self) -> int { self.lado * self.lado } }
            struct Rect { ancho: int, alto: int }
            impl Figura for Rect { fn area(self) -> int { self.ancho * self.alto } }
            fn total(xs: [dyn Figura]) -> int {
                var s = 0; var i = 0;
                while (i < xs.len()) { s = s + xs[i].area(); i = i + 1; }
                s
            }
            fn main() -> int {
                let figuras: [dyn Figura] = [Cuadrado{lado:3}, Rect{ancho:4,alto:5}, Cuadrado{lado:2}];
                total(figuras)   // 9 + 20 + 4 = 33
            }
        "#);
    }

    #[test]
    fn dyn_multi_trait_oraculo() {
        // M9.5a: `dyn A + B` — un objeto que satisface dos traits; despacho a métodos de ambos.
        // El orden del conjunto es canónico (dyn Nombre + Area == dyn Area + Nombre).
        oracle_program(r#"
            trait Area { fn area(self) -> int; }
            trait Nombre { fn nombre(self) -> string; }
            struct Cuadrado { lado: int }
            impl Area for Cuadrado { fn area(self) -> int { self.lado * self.lado } }
            impl Nombre for Cuadrado { fn nombre(self) -> string { "cuad" } }
            struct Circ { r: int }
            impl Area for Circ { fn area(self) -> int { 3 * self.r * self.r } }
            impl Nombre for Circ { fn nombre(self) -> string { "circ" } }
            fn describe(x: dyn Nombre + Area) -> int { x.nombre().len() + x.area() }
            fn main() -> int {
                let xs: [dyn Area + Nombre] = [Cuadrado{lado:4}, Circ{r:2}];
                var s = 0; var i = 0;
                while (i < xs.len()) { s = s + describe(xs[i]); i = i + 1; }
                // (4 + 16) + (4 + 12) = 20 + 16 = 36
                s
            }
        "#);
    }

    #[test]
    fn dyn_upcasting_oraculo() {
        // M9.5b: upcasting `dyn A + B` -> `dyn A` (olvidar traits, S2 ⊆ S1). Se reconstruye el
        // struct menor proyectando los campos del mayor.
        oracle_program(r#"
            trait Area { fn area(self) -> int; }
            trait Nombre { fn nombre(self) -> string; }
            struct Cuadrado { lado: int }
            impl Area for Cuadrado { fn area(self) -> int { self.lado * self.lado } }
            impl Nombre for Cuadrado { fn nombre(self) -> string { "cuad" } }
            fn solo_area(a: dyn Area) -> int { a.area() }
            fn main() -> int {
                let ab: dyn Area + Nombre = Cuadrado { lado: 5 };
                let v1 = solo_area(ab);        // upcast en el argumento: 25
                let a: dyn Area = ab;          // upcast en el let
                v1 + a.area()                  // 25 + 25 = 50
            }
        "#);
    }

    #[test]
    fn dyn_sobre_impl_generico_oraculo() {
        // M9.4: coercionar a `dyn Trait` un tipo cuyo impl es genérico acotado (Caja<T>): la vtable
        // lleva un closure anidado (como un diccionario), no el método manglado plano. Incluye
        // anidamiento Caja<Caja<N>> y un impl concreto en el mismo arreglo heterogéneo.
        oracle_program(r#"
            trait Mostrar { fn mostrar(self) -> string; }
            struct N { x: int }
            impl Mostrar for N { fn mostrar(self) -> string { "N" } }
            struct Caja<T> { v: T }
            impl<T: Mostrar> Mostrar for Caja<T> {
                fn mostrar(self) -> string { "Caja(" + self.v.mostrar() + ")" }
            }
            fn describe(d: dyn Mostrar) -> string { d.mostrar() }
            fn main() -> int {
                let xs: [dyn Mostrar] = [N{x:1}, Caja{v:N{x:2}}, Caja{v:Caja{v:N{x:3}}}];
                var total = 0; var i = 0;
                while (i < xs.len()) { total = total + describe(xs[i]).len(); i = i + 1; }
                // len("N")=1, len("Caja(N)")=7, len("Caja(Caja(N))")=13 -> 21
                total
            }
        "#);
    }

    #[test]
    fn defecto_con_self_heredado_por_dos_impls() {
        // Regresión: un método por defecto que llama a `self.m()` y es heredado por DOS
        // impls. Cada cuerpo clonado debe resolver a SUS métodos (no compartir destino).
        oracle_program(r#"
            trait Animal {
                fn sonido(self) -> int;
                fn doble_sonido(self) -> int { self.sonido() + self.sonido() }   // defecto
            }
            struct Perro { v: int }
            impl Animal for Perro { fn sonido(self) -> int { self.v } }            // hereda
            struct Gato { v: int }
            impl Animal for Gato { fn sonido(self) -> int { self.v * 10 } }        // hereda
            fn main() -> int {
                let p = Perro { v: 3 };
                let g = Gato { v: 4 };
                p.doble_sonido() + g.doble_sonido()   // (3+3) + (40+40) = 6 + 80 = 86
            }
        "#);
    }

    #[test]
    fn trait_objects_estres_gc() {
        // El struct sintetizado (vtable) y el dato viven en el heap de la VM: el GC debe
        // trazar ambos. Bajo estrés (recolecta en cada punto seguro), un fallo de raíz
        // cambiaría el resultado o reventaría.
        oracle_stress(r#"
            trait Valor { fn valor(self) -> int; fn doble(self) -> int { self.valor() + self.valor() } }
            struct A { n: int }
            impl Valor for A { fn valor(self) -> int { self.n } }
            struct B { n: int }
            impl Valor for B { fn valor(self) -> int { self.n + 1 } fn doble(self) -> int { self.n } }
            fn usar(x: dyn Valor) -> int { x.valor() + x.doble() }
            fn main() -> int {
                let a: dyn Valor = A { n: 10 };
                let b: dyn Valor = B { n: 20 };
                usar(a) + usar(b)   // (10+20) + (21+20) = 30 + 41 = 71
            }
        "#);
    }

    // ----- M10.1: @derive(Eq) -----

    #[test]
    fn derive_eq_oraculo() {
        // El impl generado por @derive(Eq) baja a una función ordinaria (M9): ambos motores
        // deben coincidir, para struct, enum unit y enum con payload.
        oracle_program(r#"
            @derive(Eq)
            struct Punto { x: int, y: int }
            @derive(Eq)
            enum Color { Rojo, Verde, Azul }
            @derive(Eq)
            enum Forma { Circulo(int), Rect(int, int) }
            fn b2i(b: bool) -> int { if (b) { 1 } else { 0 } }
            fn main() -> int {
                let p = Punto { x: 1, y: 2 };
                let q = Punto { x: 1, y: 2 };
                let r = Punto { x: 9, y: 2 };
                let e1 = b2i(p.igual(q)) + b2i(p.igual(r));               // 1 + 0
                let e2 = b2i(Color.Verde.igual(Color.Verde)) + b2i(Color.Rojo.igual(Color.Azul)); // 1 + 0
                let f = Forma.Rect(3, 4);
                let e3 = b2i(f.igual(Forma.Rect(3, 4))) + b2i(f.igual(Forma.Circulo(3)));         // 1 + 0
                e1 + e2 + e3   // 3
            }
        "#);
    }
}
