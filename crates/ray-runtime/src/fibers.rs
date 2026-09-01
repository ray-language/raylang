//! F1 del arco de concurrencia nativa (`docs/diseno-concurrencia-nativa.md` §5): scheduler M:N de
//! **fibras** para el binario transpilado, sustituto del hilo-por-conexión (medido: 1002 hilos y
//! ~265 KB/conexión a `-c 1000`, contra los 13 hilos y 23 KB del mismo modelo en la VM).
//!
//! Piezas:
//! - **Corrutinas con pila propia** (`corosensei`): el código generado sigue pareciendo bloqueante
//!   (cero coloreado); la pila es `mmap` con página de guarda, así que la RESERVA (128 KiB por
//!   defecto) es virtual y solo cuestan las páginas tocadas (medido: 4-12 KiB en `net/webserver`).
//! - **Workers**: N hilos (= cores, o `RAYLANG_THREADS`), cada uno con SU cola. Una fibra queda
//!   FIJADA al worker que le toca al nacer (round-robin) y siempre reanuda en ÉL. No es una
//!   preferencia: es CORRECCIÓN. Con opt3+LTO, LLVM cachea la dirección de un thread-local a
//!   través del cambio de contexto (el asm de corosensei no le dice que el hilo puede cambiar);
//!   una fibra que migrara escribiría en los TLS del hilo ANTIGUO — UB real, cazado por el test
//!   de estrés del reactor SOLO en release (yielder nulo, RefCell double-borrow, SIGBUS). Fijar
//!   la fibra hace válida para siempre cualquier dirección TLS que su código cachee. (Es el
//!   hazard clásico de las corrutinas stackful compiladas; Go lo evita porque SU compilador
//!   conoce los puntos de cesión — nosotros no controlamos LLVM.) Robar trabajo entre workers
//!   queda PROHIBIDO por lo mismo, no "pendiente".
//! - **Reactor**: un hilo con kqueue/epoll **persistente** (a diferencia de `src/poll.rs` de la VM,
//!   que crea y destruye el poller en cada llamada) + una tubería de despertar (CLOEXEC, como la
//!   auditoría de IDEAS §53.4) + temporizadores para `sleep`.
//!
//! La cesión "profunda" (aparcar desde dentro de `__ray_socket_read`, a N marcos del arranque de la
//! fibra) usa un TLS con el yielder de la fibra en ejecución, repuesto tras cada reanudación
//! (ver `suspend`).
//!
//! **Seguridad del movimiento entre workers**: el binario transpilado usa `Rc` en sus valores; mover
//! una fibra suspendida a otro hilo es correcto por el MISMO argumento que en la VM (M38): los
//! valores de una fibra no se comparten con otras (aislamiento del modelo de actores), así que sus
//! contadores `Rc` solo los toca la fibra dueña, esté en el hilo que esté.

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};
use std::cell::Cell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Motivo por el que una fibra devuelve el control al scheduler. Los parks de E/S llevan un
/// deadline OPCIONAL (M56.4 del lado nativo: el timeout de lectura de un socket no-bloqueante no
/// puede ser SO_RCVTIMEO — vive en el park, como el `read_deadline` de la VM); al reanudar, la
/// fibra recibe un `bool`: ¿despertó por VENCIMIENTO (true) o por readiness (false)?
enum Park {
    /// Aparca hasta que el fd esté listo para leer (o venza el deadline).
    Read(i32, Option<Instant>),
    /// Aparca hasta que el fd esté listo para escribir (o venza el deadline).
    Write(i32, Option<Instant>),
    /// Aparca hasta el instante dado (`sleep`).
    SleepUntil(Instant),
    /// F3: aparca en una LISTA DE ESPERAS (canales/tareas/actividad del binario transpilado). El
    /// u64 es la generación vista al decidir esperar: si al registrar ya cambió, hubo un despertar
    /// entre soltar el lock de la condición y suspender → re-encolar (anti despertar-perdido).
    WaitOn(WaitList, u64),
    /// Cede el turno y vuelve al final de la cola de listas.
    Yield,
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Read,
    Write,
}

type FiberCo = Coroutine<bool, Park, ()>;

/// Una fibra en vuelo: la corrutina + la celda donde se publica su resultado.
struct Task {
    co: FiberCo,
    done: Arc<DoneCell>,
    /// Almacén FIBER-LOCAL genérico (viaja con la fibra entre workers): el binario transpilado
    /// guarda aquí su contexto (token de cancelación, pila de scopes, profundidad de try_call),
    /// que en el modelo de hilos eran thread-locals. Ver `with_local`.
    local: Option<Box<dyn std::any::Any>>,
    /// Con qué valor reanudar: ¿el último park despertó por VENCIMIENTO del deadline?
    timed_out: bool,
    /// Worker al que esta fibra está FIJADA (ver el doc del módulo: no migra jamás).
    home: usize,
}

// SAFETY: corosensei NO marca `Coroutine` como Send a propósito — una pila suspendida puede contener
// valores no-Send (aquí: los `Rc` del binario transpilado) y el sistema de tipos ya no los ve. Este
// impl es la decisión consciente que habilita el modelo M:N, y su corrección se apoya en dos cosas:
// (1) EXCLUSIÓN: una Task es propiedad de exactamente un dueño a la vez (un worker mientras corre,
//     la cola/el reactor mientras espera); el movimiento entre hilos pasa por Mutex → hay
//     happens-before entre el último toque en un hilo y el primero en el siguiente.
// (2) AISLAMIENTO (el argumento de M38 en la VM): los valores de una fibra no se comparten con otras
//     fibras — los contadores `Rc` de su pila solo los toca la fibra dueña, esté en el worker que
//     esté. Lo que cruza fibras cruza por canales (F3), que imponen su propia sincronización.
// El slot `local` (Box<dyn Any>, sin cota Send) viaja bajo el MISMO argumento: es estado privado
// de la fibra, solo accesible mientras ELLA corre (puntero TLS publicado alrededor de resume).
// (3) FIJACIÓN: la Task cruza hilos solo como DATO (worker→reactor→worker); su código EJECUTA
//     siempre en su worker de origen (`home`), lo que además hace válidas las direcciones TLS
//     que LLVM cachee a través de una suspensión (ver el doc del módulo).
unsafe impl Send for Task {}

/// Celda de terminación de una fibra: `None` mientras corre; `Ok`/`Err(mensaje del panic)` al
/// acabar. `wl` (F3): los joins desde OTRA fibra esperan aquí aparcados de verdad.
struct DoneCell {
    state: Mutex<Option<Result<(), String>>>,
    cv: Condvar,
    wl: WaitList,
}

/// Asa de espera de una fibra. Desde un hilo plano (`main`, tests) bloquea en la condvar; desde
/// una FIBRA cede el turno entre comprobaciones — con fibras FIJADAS a su worker, bloquear el
/// hilo dentro de una fibra interbloquearía a las hermanas del mismo worker (la esperada podría
/// no correr nunca). El despertar por lista de esperas (sin ceder en bucle) es de F3.
pub struct JoinHandle {
    done: Arc<DoneCell>,
}

impl JoinHandle {
    pub fn join(self) -> Result<(), String> {
        if in_fiber() {
            // F3: espera de lista (aparcada de verdad), no ceder-en-bucle. prepare ANTES de
            // soltar el lock del estado: el protocolo anti despertar-perdido de WaitList.
            loop {
                let seen = {
                    let mut st = self.done.state.lock().unwrap();
                    if let Some(r) = st.take() {
                        return r;
                    }
                    self.done.wl.prepare()
                };
                block_on(&self.done.wl, seen);
            }
        }
        let mut st = self.done.state.lock().unwrap();
        while st.is_none() {
            st = self.done.cv.wait(st).unwrap();
        }
        st.take().unwrap()
    }
}

/// F3 — LISTA DE ESPERAS: la primitiva de bloqueo de condición para fibras (canales, tareas,
/// actividad). Sustituye al interino de F2 (ceder-en-bucle, que quemaba CPU con esperas ociosas).
///
/// Protocolo anti despertar-perdido (el clásico de las condvars, en versión fibras):
/// el esperador lee la GENERACIÓN con el lock de su condición aún tomado (`prepare`), lo suelta y
/// suspende con `block_on`; el worker, al registrar la fibra, re-lee la generación — si cambió
/// entre el prepare y el registro, hubo un `wake_all` en la ventana y la fibra se re-encola en vez
/// de dormirse (el llamador rechequea su condición en bucle, como con una condvar).
///
/// CANCELACIÓN (H21-N3): cada espera lleva un PULSO de 10 ms (temporizador del reactor): la fibra
/// despierta, el llamador rechequea condición y cancelación, y re-espera. Es la MISMA cadencia que
/// el `wait_timeout(10ms)` del modelo de hilos — una tarea cancelada nota su cancelación en ≤10 ms
/// — pero con la fibra APARCADA de verdad entre pulsos (cero CPU), no cediendo en bucle.
#[derive(Clone)]
pub struct WaitList(Arc<WlInner>);

struct WlInner {
    generation: std::sync::atomic::AtomicU64,
    waiters: Mutex<Vec<(u64, Task)>>,
}

impl Default for WaitList {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitList {
    pub fn new() -> WaitList {
        WaitList(Arc::new(WlInner { generation: std::sync::atomic::AtomicU64::new(0), waiters: Mutex::new(Vec::new()) }))
    }

    /// Lee la generación actual. Llamar CON el lock de la condición tomado, antes de soltarlo.
    pub fn prepare(&self) -> u64 {
        self.0.generation.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Despierta a TODOS los esperadores (y avanza la generación, cerrando la ventana del
    /// despertar perdido). Los pulsos de cancelación pendientes de los despertados quedan
    /// huérfanos y se descartan solos al vencer (no encuentran su id).
    pub fn wake_all(&self) {
        self.0.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let woken: Vec<(u64, Task)> = std::mem::take(&mut *self.0.waiters.lock().unwrap());
        if woken.is_empty() {
            return;
        }
        let s = sched();
        for (_, t) in woken {
            s.enqueue(t);
        }
    }

    /// Saca un esperador por id (lo usa el pulso de cancelación del reactor). `None` = ya despertó.
    fn remove(&self, id: u64) -> Option<Task> {
        let mut w = self.0.waiters.lock().unwrap();
        w.iter().position(|(wid, _)| *wid == id).map(|pos| w.swap_remove(pos).1)
    }
}

/// Suspende la fibra actual hasta el próximo `wake_all` de `wl` (o el pulso de 10 ms). `seen` es
/// la generación devuelta por `prepare` ANTES de soltar el lock de la condición. Solo en fibra
/// (el hilo `main` espera por la condvar de siempre; el runtime emitido elige la vía).
pub fn block_on(wl: &WaitList, seen: u64) {
    suspend(Park::WaitOn(wl.clone(), seen));
}

/// Operaciones que los workers encargan al reactor (via buzón + tubería de despertar).
enum Op {
    Wait(i32, Dir, Option<Instant>, Task),
    Timer(Instant, Task),
    /// Pulso de cancelación de una espera de lista (F3): al vencer, si el id sigue en la lista,
    /// se despierta esa fibra (rechequeará condición y cancelación y re-esperará).
    WaitPoll(Instant, WaitList, u64),
}

/// La cola de UN worker: solo su dueño saca; cualquiera (spawn, reactor) mete.
struct WorkerQueue {
    q: Mutex<VecDeque<Task>>,
    cv: Condvar,
}

struct Scheduler {
    /// Una cola POR WORKER (fijación de fibras, ver el doc del módulo). El sharding además evita
    /// la contención de una cola global.
    queues: Vec<WorkerQueue>,
    /// Reparto round-robin de fibras nuevas entre workers.
    next_home: std::sync::atomic::AtomicUsize,
    /// Buzón del reactor: los workers dejan aquí los aparcados y tocan la tubería.
    inbox: Mutex<Vec<Op>>,
    /// Extremo de escritura de la tubería de despertar del reactor.
    wake_wr: i32,
}

impl Scheduler {
    /// Encola una fibra LISTA en la cola de su worker de origen (nunca en otra).
    fn enqueue(&self, t: Task) {
        let wq = &self.queues[t.home];
        wq.q.lock().unwrap().push_back(t);
        wq.cv.notify_one();
    }

    fn to_reactor(&self, op: Op) {
        let was_empty = {
            let mut q = self.inbox.lock().unwrap();
            let e = q.is_empty();
            q.push(op);
            e
        };
        // F5: un byte por LOTE, no por op — si el buzón no estaba vacío, ya hay un byte pendiente
        // en la tubería (persiste hasta el drain del reactor): escribir otro solo cuesta syscall.
        if was_empty {
            sys::wake(self.wake_wr, 1);
        }
    }
}

/// Default PROGRAMÁTICO de la pila de fibra en KiB (0 = no fijado). Lo fija el binario emitido
/// ANTES de la primera fibra — p. ej. 1 MiB cuando el programa declara externs (FFI): el código C
/// asume pilas de hilo grandes y los 128 KiB de una fibra pueden quedarse cortos (la página de
/// guarda convierte el desborde en SIGSEGV limpio, pero mudo). Reserva virtual: solo cuestan las
/// páginas tocadas.
static DEFAULT_STACK_KIB: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Fija el default de pila de fibra (KiB). Llamar ANTES de la primera fibra (el tamaño se decide
/// una sola vez); después es inerte. `RAY_FIBER_STACK_KIB` (el mando del usuario) siempre gana.
pub fn set_default_fiber_stack_kib(kib: usize) {
    DEFAULT_STACK_KIB.store(kib, std::sync::atomic::Ordering::Relaxed);
}

/// Tamaño de RESERVA de la pila de cada fibra. Reserva virtual: con página de guarda, solo cuestan
/// las páginas tocadas. Precedencia: `RAY_FIBER_STACK_KIB` (usuario) > default programático
/// (`set_default_fiber_stack_kib`, p. ej. FFI) > 128 KiB. Mínimo 32 KiB, por seguridad.
fn fiber_stack_size() -> usize {
    static SIZE: OnceLock<usize> = OnceLock::new();
    *SIZE.get_or_init(|| {
        std::env::var("RAY_FIBER_STACK_KIB")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|k| k.max(32) * 1024)
            .unwrap_or_else(|| {
                match DEFAULT_STACK_KIB.load(std::sync::atomic::Ordering::Relaxed) {
                    0 => 128 * 1024,
                    kib => kib.max(32) * 1024,
                }
            })
    })
}

fn sched() -> &'static Scheduler {
    static S: OnceLock<&'static Scheduler> = OnceLock::new();
    S.get_or_init(|| {
        let (wake_rd, wake_wr) = sys::wake_pipe();
        // Mismo mando que la VM: RAYLANG_THREADS acota los workers (1 = ejecución M:1).
        let workers = std::env::var("RAYLANG_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));
        let s: &'static Scheduler = Box::leak(Box::new(Scheduler {
            queues: (0..workers).map(|_| WorkerQueue { q: Mutex::new(VecDeque::new()), cv: Condvar::new() }).collect(),
            next_home: std::sync::atomic::AtomicUsize::new(0),
            inbox: Mutex::new(Vec::new()),
            wake_wr,
        }));
        for i in 0..workers {
            std::thread::Builder::new()
                .name(format!("ray-fiber-worker-{i}"))
                .spawn(move || worker_loop(s, i))
                .expect("could not start a fiber worker");
        }
        std::thread::Builder::new()
            .name("ray-fiber-reactor".into())
            .spawn(move || reactor_loop(s, wake_rd))
            .expect("could not start the fiber reactor");
        s
    })
}

/// Ids de espera de lista (F3), globales y monótonos: casan el pulso de cancelación con su fibra.
static NEXT_WAIT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

thread_local! {
    /// Yielder de la fibra actualmente en ejecución en ESTE worker (nulo fuera de fibra). Puntero
    /// crudo porque el tipo lleva lifetime; ver los SAFETY de `suspend`.
    static CURRENT: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
    /// Puntero al slot `local` de la Task en resume en ESTE worker (nulo fuera de fibra). Lo
    /// publica el worker alrededor de cada resume; lo consume `with_local`.
    static CURRENT_LOCAL: Cell<*mut Option<Box<dyn std::any::Any>>> = const { Cell::new(std::ptr::null_mut()) };
}

/// Lanza `f` como fibra. Llamable desde cualquier hilo (incluida otra fibra).
pub fn spawn(f: impl FnOnce() + Send + 'static) -> JoinHandle {
    let done = Arc::new(DoneCell { state: Mutex::new(None), cv: Condvar::new(), wl: WaitList::new() });
    let stack = DefaultStack::new(fiber_stack_size()).expect("could not map a fiber stack");
    let co = Coroutine::with_stack(stack, move |y: &Yielder<bool, Park>, _timed_out: bool| {
        // Prólogo: deja el yielder a mano para la cesión profunda (park desde N marcos más abajo).
        CURRENT.with(|c| c.set(y as *const Yielder<bool, Park> as *const ()));
        f();
    });
    let s = sched();
    let home = s.next_home.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % s.queues.len();
    let task = Task { co, done: done.clone(), local: None, timed_out: false, home };
    s.enqueue(task);
    JoinHandle { done }
}

/// ¿Está este hilo ejecutando una fibra ahora mismo? (El runtime lo usa para decidir aparcar-fibra
/// contra bloquear-hilo, p. ej. en el hilo `main`, que no es una fibra.)
pub fn in_fiber() -> bool {
    CURRENT.with(|c| !c.get().is_null())
}

/// Cede el control del worker con el motivo dado. Solo dentro de una fibra. Devuelve `true` si el
/// despertar fue por VENCIMIENTO del deadline del park (solo posible en parks de E/S con plazo).
fn suspend(park: Park) -> bool {
    let y = CURRENT.with(|c| c.get());
    assert!(!y.is_null(), "fiber park outside of a fiber");
    // SAFETY: `y` apunta al `Yielder` de la corrutina EN EJECUCIÓN en este hilo: vive en la pila de
    // la corrutina (viva hasta que retorna) y solo lo usa la propia fibra. `suspend` devuelve el
    // control al `resume` del worker; cuando la fibra despierte, la ejecución sigue aquí.
    let timed_out = unsafe { (*(y as *const Yielder<bool, Park>)).suspend(park) };
    // Reponer el TLS al reanudar (el prólogo solo corre una vez). La fibra reanuda SIEMPRE en su
    // worker de origen (fijación) → esta escritura toca el TLS del hilo correcto aunque LLVM
    // hubiera cacheado la dirección a través de la suspensión.
    CURRENT.with(|c| c.set(y));
    timed_out
}

/// Aparca la fibra hasta que `fd` esté listo para **leer**.
pub fn park_readable(fd: i32) {
    suspend(Park::Read(fd, None));
}

/// Aparca la fibra hasta que `fd` esté listo para **escribir**.
pub fn park_writable(fd: i32) {
    suspend(Park::Write(fd, None));
}

/// Espera a que `fd` esté listo para **leer**, desde CUALQUIER contexto: en fibra aparca (park);
/// fuera de fibra (p. ej. el hilo `main` del binario transpilado) hace un `poll(2)` bloqueante.
/// Es el helper que usa el runtime emitido con `--fibers` (los sockets son no-bloqueantes SIEMPRE,
/// también cuando los toca `main`).
pub fn wait_readable(fd: i32) {
    if in_fiber() { park_readable(fd) } else { sys_common::poll_block(fd, false, -1); }
}

/// Como [`wait_readable`], para **escritura**.
pub fn wait_writable(fd: i32) {
    if in_fiber() { park_writable(fd) } else { sys_common::poll_block(fd, true, -1); }
}

/// Como [`wait_readable`], con PLAZO: devuelve `true` si venció sin readiness (`timeout_ms <= 0`
/// = sin plazo). Es el timeout de lectura de sockets (M56.4) del lado fibras: en un socket
/// no-bloqueante SO_RCVTIMEO es inerte, así que el plazo vive en el park (como el `read_deadline`
/// de la VM).
pub fn wait_readable_timeout(fd: i32, timeout_ms: i64) -> bool {
    if timeout_ms <= 0 {
        wait_readable(fd);
        return false;
    }
    if in_fiber() {
        suspend(Park::Read(fd, Some(Instant::now() + Duration::from_millis(timeout_ms as u64))))
    } else {
        let clamped = timeout_ms.min(i32::MAX as i64) as i32;
        sys_common::poll_block(fd, false, clamped)
    }
}

/// Despierta el reactor para que RE-REGISTRE sus intereses. Lo llama el `close` del runtime
/// emitido: el fd cerrado se re-registra en el siguiente ciclo y kqueue/epoll lo devuelven como
/// error/listo → la fibra aparcada en él despierta y su syscall reporta el error real (el mismo
/// papel que cumple el re-poll por ronda del scheduler de la VM).
pub fn poke() {
    sys::wake(sched().wake_wr, 2);
}

/// Acceso al almacén FIBER-LOCAL de la fibra en ejecución: `None` si este hilo no está ejecutando
/// una fibra ahora mismo (el llamador cae a su thread-local de siempre). El slot es un
/// `Box<dyn Any>` que el binario transpilado puebla con su contexto (cancelación/scopes/try).
/// CONTRATO: `f` no debe aparcar la fibra ni llamar a `with_local` anidado (el préstamo exclusivo
/// del slot vive durante `f`).
pub fn with_local<R>(f: impl FnOnce(&mut Option<Box<dyn std::any::Any>>) -> R) -> Option<R> {
    let p = CURRENT_LOCAL.with(|c| c.get());
    if p.is_null() {
        return None;
    }
    // SAFETY: el puntero lo publicó ESTE worker alrededor del resume de la fibra actual (ver
    // worker_loop) y apunta al slot de esa Task, viva en el marco del worker durante todo el
    // resume. El contrato de arriba (sin parks ni anidación dentro de `f`) garantiza exclusividad.
    Some(f(unsafe { &mut *p }))
}

/// Duerme la fibra (no el worker) `ms` milisegundos.
pub fn fiber_sleep(ms: i64) {
    suspend(Park::SleepUntil(Instant::now() + Duration::from_millis(ms.max(0) as u64)));
}

/// Duerme `ms` desde CUALQUIER contexto: la FIBRA si este hilo ejecuta una (timer del reactor,
/// el worker queda libre), o el hilo si no (el `main` del binario transpilado).
pub fn sleep_ms(ms: i64) {
    if in_fiber() {
        fiber_sleep(ms);
    } else if ms > 0 {
        // M119: fuera de fibra (el `main` del binario transpilado sin `spawn`) dormíamos con
        // `thread::sleep`, que en macOS se pasa varios ms y descuadra el pacing (§72). `poll(2)` con
        // cero descriptores es la misma espera precisa que ya usa el reactor.
        sys_common::poll_sleep(ms as u64);
    }
}

/// Cede el turno: la fibra vuelve al final de la cola de listas.
pub fn yield_now() {
    suspend(Park::Yield);
}

// ============================================================================
// Pool bloqueante (extern fn blocking, FFI): descarga llamadas C bloqueantes
// ============================================================================
//
// Con las fibras FIJADAS a su worker (sin work-stealing), una llamada C bloqueante dentro de una
// fibra bloquearía el worker entero y VARARÍA a todas sus fibras hermanas aunque haya otros workers
// ociosos. `run_blocking` es la válvula: ejecuta el closure en un hilo de un pool aparte y APARCA la
// fibra (WaitList) hasta el resultado — el worker queda libre. Fuera de fibra (hilo `main`) llama
// directo: bloquear un hilo plano es el statu quo y no vara a nadie.
//
// El pool cachea hilos ociosos (un despacho caliente es un lock+condvar, sin `thread::spawn`) y
// crece bajo demanda sin tope: cada trabajo pendiente representa una fibra ya aparcada, así que el
// número de hilos queda acotado por la concurrencia real de llamadas bloqueantes en vuelo (el mismo
// compromiso que el hilo-por-tarea que las fibras sustituyen). Un hilo ocioso muere tras 10 s.

/// Un trabajo encargado al pool bloqueante.
type BlockingJob = Box<dyn FnOnce() + Send>;

struct BlockingPool {
    state: Mutex<BlockingState>,
    cv: Condvar,
}

struct BlockingState {
    jobs: VecDeque<BlockingJob>,
    /// Hilos esperando trabajo en la condvar (para decidir spawn vs notify).
    idle: usize,
}

fn blocking_pool() -> &'static BlockingPool {
    static P: OnceLock<BlockingPool> = OnceLock::new();
    P.get_or_init(|| BlockingPool { state: Mutex::new(BlockingState { jobs: VecDeque::new(), idle: 0 }), cv: Condvar::new() })
}

/// Encola un trabajo: lo toma un hilo ocioso, o se levanta uno nuevo si no lo hay.
fn blocking_submit(job: BlockingJob) {
    let p = blocking_pool();
    let needs_thread = {
        let mut st = p.state.lock().unwrap();
        st.jobs.push_back(job);
        if st.idle > 0 {
            p.cv.notify_one();
            false
        } else {
            true
        }
    };
    if needs_thread {
        std::thread::Builder::new()
            .name("ray-blocking".into())
            .spawn(blocking_worker_loop)
            .expect("could not start a blocking pool thread");
    }
}

fn blocking_worker_loop() {
    let p = blocking_pool();
    loop {
        let job = {
            let mut st = p.state.lock().unwrap();
            loop {
                if let Some(j) = st.jobs.pop_front() {
                    break j;
                }
                st.idle += 1;
                let (next, timeout) = p.cv.wait_timeout(st, Duration::from_secs(10)).unwrap();
                st = next;
                st.idle -= 1;
                // Venció ocioso y sin trabajo pendiente → el hilo muere (el pool se encoge solo).
                if timeout.timed_out() && st.jobs.is_empty() {
                    return;
                }
            }
        };
        job();
    }
}

// El puntero al `errno` del hilo actual (para transportarlo a través del pool bloqueante). Mismo
// trío de plataformas que process.rs; viven en la libc, siempre enlazada. M156: Android es
// unix pero NO "linux" — bionic usa __errno_location; el brazo not(linux) con __error (Darwin)
// era un error de link latente.
#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe extern "C" {
    #[link_name = "__errno_location"]
    fn blocking_errno_ptr() -> *mut i32;
}
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
unsafe extern "C" {
    #[link_name = "__error"]
    fn blocking_errno_ptr() -> *mut i32;
}
#[cfg(windows)]
unsafe extern "C" {
    #[link_name = "_errno"]
    fn blocking_errno_ptr() -> *mut i32;
}

/// Ejecuta `f` (presumiblemente bloqueante: una extern fn C marcada `blocking`) sin bloquear el
/// worker de fibras: en fibra, `f` corre en un hilo del pool bloqueante y la fibra queda APARCADA
/// (cero CPU) hasta el resultado; fuera de fibra, llama directo. Un panic dentro de `f` se
/// re-propaga en el llamador (mismo comportamiento que la llamada directa).
///
/// El `errno` VIAJA con el resultado: una extern C estilo POSIX deja su motivo en el errno del
/// hilo del POOL — se captura allí tras `f` y se repone en el hilo del llamador al despertar, de
/// modo que `std/ffi.errno()` tras una extern `blocking` lee lo que dejó ESA llamada (misma
/// semántica que la llamada directa).
pub fn run_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    if !in_fiber() {
        return f();
    }
    // El slot del resultado (+ el errno del hilo del pool) + su lista de esperas (el protocolo
    // anti despertar-perdido de F3).
    struct Slot<T> {
        state: Mutex<Option<(std::thread::Result<T>, i32)>>,
        wl: WaitList,
    }
    let slot = Arc::new(Slot::<T> { state: Mutex::new(None), wl: WaitList::new() });
    let s2 = slot.clone();
    blocking_submit(Box::new(move || {
        // AssertUnwindSafe: el resultado (o el panic) se transporta entero al llamador; nadie
        // observa estado a medias.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        // SAFETY: puntero al errno de ESTE hilo del pool, siempre válido.
        let e = unsafe { *blocking_errno_ptr() };
        *s2.state.lock().unwrap() = Some((r, e));
        s2.wl.wake_all();
    }));
    loop {
        let seen = {
            let mut st = slot.state.lock().unwrap();
            if let Some((r, e)) = st.take() {
                // SAFETY: puntero al errno del hilo del llamador, siempre válido. Reponerlo ANTES
                // de devolver: el llamador puede leerlo justo después (std/ffi.errno()).
                unsafe { *blocking_errno_ptr() = e };
                match r {
                    Ok(v) => return v,
                    Err(p) => std::panic::resume_unwind(p),
                }
            }
            slot.wl.prepare()
        };
        block_on(&slot.wl, seen);
    }
}

fn finish(done: &DoneCell, result: Result<(), String>) {
    *done.state.lock().unwrap() = Some(result);
    done.cv.notify_all();
    done.wl.wake_all();
}

fn panic_msg(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

fn worker_loop(s: &'static Scheduler, me: usize) {
    let wq = &s.queues[me];
    loop {
        let mut task = {
            let mut q = wq.q.lock().unwrap();
            loop {
                if let Some(t) = q.pop_front() {
                    break t;
                }
                q = wq.cv.wait(q).unwrap();
            }
        };
        debug_assert_eq!(task.home, me, "una fibra solo reanuda en su worker de origen");
        // Publica el slot fiber-local de ESTA task mientras corre (puntero crudo: la Task vive en
        // este marco durante todo el resume; se retira ANTES de ceder la Task a nadie).
        CURRENT_LOCAL.with(|c| c.set(&mut task.local as *mut _));
        let timed_out = std::mem::replace(&mut task.timed_out, false);
        // El catch_unwind delimita el panic de LA FIBRA (corosensei lo propaga a través de resume):
        // se publica como Err en su celda y el worker sigue con la siguiente.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task.co.resume(timed_out)));
        CURRENT.with(|c| c.set(std::ptr::null()));
        CURRENT_LOCAL.with(|c| c.set(std::ptr::null_mut()));
        match r {
            Ok(CoroutineResult::Yield(park)) => match park {
                Park::Read(fd, dl) => s.to_reactor(Op::Wait(fd, Dir::Read, dl, task)),
                Park::Write(fd, dl) => s.to_reactor(Op::Wait(fd, Dir::Write, dl, task)),
                Park::SleepUntil(at) => s.to_reactor(Op::Timer(at, task)),
                // F3: registrar en la lista de esperas — releyendo la generación BAJO el lock de
                // la lista: si cambió desde el prepare del esperador, un wake_all ganó la carrera
                // → re-encolar ya (el llamador rechequea su condición). Si no, queda registrado y
                // se arma su pulso de cancelación (10 ms, cadencia del modelo de hilos).
                Park::WaitOn(wl, seen) => {
                    let mut waiters = wl.0.waiters.lock().unwrap();
                    if wl.0.generation.load(std::sync::atomic::Ordering::SeqCst) != seen {
                        drop(waiters);
                        s.enqueue(task);
                    } else {
                        let id = NEXT_WAIT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        waiters.push((id, task));
                        drop(waiters);
                        s.to_reactor(Op::WaitPoll(Instant::now() + Duration::from_millis(10), wl.clone(), id));
                    }
                }
                Park::Yield => s.enqueue(task),
            },
            Ok(CoroutineResult::Return(())) => finish(&task.done, Ok(())),
            Err(p) => finish(&task.done, Err(panic_msg(&*p))),
        }
    }
}

/// Esperas registradas sobre un fd: fibras aparcadas por lectura y por escritura, cada una con
/// `(id, tiene_deadline, fibra)` — el bool evita apuntar cancelaciones de ids sin temporizador.
#[derive(Default)]
struct FdWaiters {
    read: Vec<(u64, bool, Task)>,
    write: Vec<(u64, bool, Task)>,
}

/// Entrada del heap de deadlines de E/S (orden por instante; `Reverse` para min-heap).
#[derive(PartialEq, Eq)]
struct IoDeadline {
    at: Instant,
    fd: i32,
    dir_write: bool,
    id: u64,
}
impl Ord for IoDeadline {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Min-heap vía Reverse en el llamador: aquí orden natural por instante (id desempata).
        (self.at, self.id).cmp(&(other.at, other.id))
    }
}
impl PartialOrd for IoDeadline {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn reactor_loop(s: &'static Scheduler, wake_rd: i32) {
    use std::cmp::Reverse;
    let mut poller = sys::Poller::new(wake_rd);
    // Esperas por fd. F5: las entradas NO se borran al vaciarse — conservan la capacidad de sus
    // Vec (cero asignaciones por park en régimen). El mapa queda acotado por el pico de fds
    // concurrentes (los números de fd se REUTILIZAN), no por el total histórico.
    let mut fds: HashMap<i32, FdWaiters> = HashMap::new();
    // Sleeps: pocos y de vida legítima → Vec con barrido lineal basta.
    let mut sleeps: Vec<(Instant, Task)> = Vec::new();
    // Pulsos de cancelación de las esperas de lista (F3): barrido lineal, acotado por el número
    // de fibras esperando condiciones (no por el caudal).
    let mut wait_polls: Vec<(Instant, WaitList, u64)> = Vec::new();
    // Deadlines de E/S: min-heap + cancelación explícita + compactación (ver F2: los huérfanos
    // del read-timeout llegaban a ~1M de entradas a 100k rps con barrido O(n)).
    let mut io_deadlines: std::collections::BinaryHeap<Reverse<IoDeadline>> = std::collections::BinaryHeap::new();
    let mut cancelled: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut next_id: u64 = 0;
    // Buffers reutilizados (F5): el buzón se intercambia por swap (el buffer hace ping-pong y no
    // se libera nunca → sin churn cruzado de hilos) y los despertares se agrupan POR WORKER para
    // tomar cada cola una sola vez por ciclo.
    let mut ops: Vec<Op> = Vec::new();
    let mut batches: Vec<Vec<Task>> = (0..s.queues.len()).map(|_| Vec::new()).collect();
    loop {
        // 1) Drena el buzón de los workers (swap: sin asignar ni liberar buffers).
        std::mem::swap(&mut *s.inbox.lock().unwrap(), &mut ops);
        for op in ops.drain(..) {
            match op {
                Op::Wait(fd, dir, deadline, t) => {
                    next_id += 1;
                    let w = fds.entry(fd).or_default();
                    let has_dl = deadline.is_some();
                    match dir {
                        Dir::Read => w.read.push((next_id, has_dl, t)),
                        Dir::Write => w.write.push((next_id, has_dl, t)),
                    }
                    if let Some(at) = deadline {
                        io_deadlines.push(Reverse(IoDeadline { at, fd, dir_write: dir == Dir::Write, id: next_id }));
                    }
                    // F5: armado INCREMENTAL — solo el interés nuevo (kqueue lo presenta con el
                    // siguiente kevent; epoll hace el ctl aquí). Si no se puede armar (fd ya
                    // cerrado en carrera), despierta YA: su syscall verá el error real.
                    if !poller.arm(fd, dir) {
                        let w = fds.get_mut(&fd).unwrap();
                        let v = match dir {
                            Dir::Read => &mut w.read,
                            Dir::Write => &mut w.write,
                        };
                        if let Some((id, had_dl, t)) = v.pop() {
                            if had_dl {
                                cancelled.insert(id);
                            }
                            batches[t.home].push(t);
                        }
                    }
                }
                Op::Timer(at, t) => sleeps.push((at, t)),
                Op::WaitPoll(at, wl, id) => wait_polls.push((at, wl, id)),
            }
        }
        // 2) Despierta los vencidos y calcula el timeout hasta el siguiente plazo vivo.
        let now = Instant::now();
        let mut i = 0;
        while i < sleeps.len() {
            if sleeps[i].0 <= now {
                let (_, t) = sleeps.swap_remove(i);
                batches[t.home].push(t);
            } else {
                i += 1;
            }
        }
        let mut j = 0;
        while j < wait_polls.len() {
            if wait_polls[j].0 <= now {
                let (_, wl, id) = wait_polls.swap_remove(j);
                if let Some(t) = wl.remove(id) {
                    batches[t.home].push(t); // pulso: rechequea condición/cancelación y re-espera
                }
            } else {
                j += 1;
            }
        }
        while let Some(Reverse(dl)) = io_deadlines.peek() {
            if dl.at > now {
                break;
            }
            let Reverse(dl) = io_deadlines.pop().unwrap();
            if cancelled.remove(&dl.id) {
                continue; // el readiness ganó la carrera: temporizador ya cancelado
            }
            if let Some(w) = fds.get_mut(&dl.fd) {
                let v = if dl.dir_write { &mut w.write } else { &mut w.read };
                if let Some(pos) = v.iter().position(|(wid, _, _)| *wid == dl.id) {
                    let (_, _, mut t) = v.swap_remove(pos);
                    t.timed_out = true;
                    batches[t.home].push(t);
                }
            }
        }
        if cancelled.len() > 8192 {
            io_deadlines.retain(|Reverse(dl)| !cancelled.contains(&dl.id));
            cancelled.clear();
        }
        // 3) Entrega los despertares acumulados hasta aquí, agrupados por worker (una toma de
        //    lock por cola y por ciclo), y espera readiness.
        flush_batches(s, &mut batches);
        let next_sleep = sleeps.iter().map(|(at, _)| *at).min();
        let next_io = io_deadlines.peek().map(|Reverse(dl)| dl.at);
        let next_poll = wait_polls.iter().map(|(at, _, _)| *at).min();
        let timeout_ms: i32 = match [next_sleep, next_io, next_poll].into_iter().flatten().min() {
            None => -1, // sin plazos: espera infinita; la tubería interrumpe con trabajo
            Some(at) => at.saturating_duration_since(now).as_millis().min(i32::MAX as u128) as i32,
        };
        let mut rearm_all = false;
        for &(fd, dir) in poller.wait(timeout_ms) {
            if fd == wake_rd {
                if sys::drain(wake_rd) {
                    rearm_all = true;
                }
                continue;
            }
            if let Some(w) = fds.get_mut(&fd) {
                let v = match dir {
                    Dir::Read => &mut w.read,
                    Dir::Write => &mut w.write,
                };
                // Drena los waiters SIN liberar la capacidad del Vec (entrada persistente).
                for (id, had_dl, t) in v.drain(..) {
                    if had_dl {
                        cancelled.insert(id); // su temporizador ya no debe despertar a nadie
                    }
                    batches[t.home].push(t);
                }
            }
        }
        // 4) Tras un POKE (un close del programa): el knote/registro de un fd cerrado muere en
        //    silencio → re-arma TODOS los intereses vivos; el fd muerto vuelve como error/listo
        //    en el siguiente wait y su fibra despierta al error real. O(fds aparcados), solo en
        //    closes (coalescidos por ciclo), que es lo que el modelo de re-registro-total pagaba
        //    en TODOS los ciclos.
        if rearm_all {
            for (&fd, w) in fds.iter() {
                if !w.read.is_empty() {
                    let _ = poller.arm(fd, Dir::Read);
                }
                if !w.write.is_empty() {
                    let _ = poller.arm(fd, Dir::Write);
                }
            }
        }
        flush_batches(s, &mut batches);
    }
}

/// Encola cada lote en la cola de su worker con UNA toma de lock por cola (los buffers de lote se
/// reutilizan entre ciclos).
fn flush_batches(s: &'static Scheduler, batches: &mut [Vec<Task>]) {
    for (home, batch) in batches.iter_mut().enumerate() {
        if batch.is_empty() {
            continue;
        }
        let wq = &s.queues[home];
        {
            let mut q = wq.q.lock().unwrap();
            for t in batch.drain(..) {
                q.push_back(t);
            }
        }
        wq.cv.notify_one();
    }
}

// ─── poll(2) bloqueante para hilos NO-fibra (el `main` del binario transpilado) ─────────────────
//
// Con `--fibers` los sockets son no-bloqueantes SIEMPRE; cuando los toca un hilo que no es fibra
// (main), la espera es un poll(2) clásico de un solo fd. Común a macOS y Linux (mismos valores de
// POLLIN/POLLOUT; solo difiere el tipo de `nfds`).
mod sys_common {
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    const POLLIN: i16 = 0x0001;
    const POLLOUT: i16 = 0x0004;
    const EINTR: i32 = 4;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    type Nfds = u64;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    type Nfds = u32;

    unsafe extern "C" {
        fn poll(fds: *mut PollFd, n: Nfds, timeout: i32) -> i32;
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe extern "C" {
        #[link_name = "__errno_location"]
        fn errno_ptr() -> *mut i32;
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    unsafe extern "C" {
        #[link_name = "__error"]
        fn errno_ptr() -> *mut i32;
    }

    /// Espera bloqueante a que `fd` esté listo (lectura o escritura). Devuelve `true` si VENCIÓ el
    /// timeout sin readiness (`timeout_ms < 0` = sin plazo, siempre `false`). EINTR reintenta; otro
    /// error devuelve `false` de inmediato: el llamador hará su syscall y verá el error real.
    pub(super) fn poll_block(fd: i32, want_write: bool, timeout_ms: i32) -> bool {
        let mut p = PollFd { fd, events: if want_write { POLLOUT } else { POLLIN }, revents: 0 };
        loop {
            // SAFETY: `poll` de libc sobre un array local de 1 entrada, vivo durante la llamada;
            // errno_ptr es el errno del hilo (__error/__errno_location, siempre válido).
            let n = unsafe { poll(&mut p as *mut PollFd, 1, timeout_ms) };
            if n > 0 {
                return false; // listo (o error del fd: la syscall del llamador lo reporta)
            }
            if n == 0 {
                return true; // venció el plazo
            }
            if unsafe { *errno_ptr() } != EINTR {
                return false; // error no transitorio: que la syscall del llamador lo vea
            }
        }
    }

    /// Duerme el HILO `ms` milisegundos con precisión (M119): `poll(NULL, 0, ms)` en vez de
    /// `thread::sleep`. En macOS el `nanosleep` de éste se pasa varios ms por *timer coalescing*
    /// (medido: `sleep(33)` → ~37 ms) y descuadra el pacing (§72); `poll(2)` con cero descriptores
    /// honra el plazo por la vía de eventos del kernel (~34 ms). Reintenta ante EINTR hasta cubrir el
    /// plazo → duerme al menos lo pedido. Lo usa el camino FUERA de fibra (el `main` del binario);
    /// dentro de una fibra el reactor ya duerme preciso vía kqueue/epoll.
    pub(super) fn poll_sleep(ms: u64) {
        use std::time::{Duration, Instant};
        let deadline = Instant::now() + Duration::from_millis(ms);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            // +1: redondeo hacia arriba para no despertar un pelo antes del plazo.
            let timeout_ms = (remaining.as_millis().min(i32::MAX as u128 - 1) as i32) + 1;
            // SAFETY: `poll` de libc con lista nula y n=0 → espera pura por timeout; errno del hilo.
            let n = unsafe { poll(core::ptr::null_mut(), 0, timeout_ms) };
            if n >= 0 {
                return; // n == 0: venció el plazo (con 0 fds no hay otro retorno posible)
            }
            if unsafe { *errno_ptr() } != EINTR {
                return; // error inesperado: no insistir
            }
        }
    }
}

// ─── Plataforma: poller persistente + tubería de despertar ─────────────────────────────────────
//
// Mismo principio que `src/poll.rs` de la VM (FFI directo a libSystem/libc, sin crate `libc`),
// con dos diferencias: la instancia es PERSISTENTE (un kqueue/epoll para toda la vida del reactor,
// no uno por llamada) y los intereses de fd son ONESHOT (se auto-limpian al dispararse, que es
// exactamente la semántica de "aparcar hasta que esté listo una vez").

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
mod sys {
    use super::Dir;

    const EVFILT_READ: i16 = -1;
    const EVFILT_WRITE: i16 = -2;
    const EV_ADD: u16 = 0x0001;
    const EV_ONESHOT: u16 = 0x0010;

    /// `struct kevent` de Darwin/BSD (64-bit): 32 bytes, alineación 8 (ver `src/poll.rs`).
    #[repr(C)]
    #[allow(dead_code)]
    struct Kevent {
        ident: usize,
        filter: i16,
        flags: u16,
        fflags: u32,
        data: isize,
        udata: *mut core::ffi::c_void,
    }

    #[repr(C)]
    struct Timespec {
        tv_sec: isize,
        tv_nsec: isize,
    }

    unsafe extern "C" {
        fn kqueue() -> i32;
        fn kevent(
            kq: i32,
            changelist: *const Kevent,
            nchanges: i32,
            eventlist: *mut Kevent,
            nevents: i32,
            timeout: *const Timespec,
        ) -> i32;
        fn pipe(fds: *mut i32) -> i32;
        // OJO: fcntl es VARIÁDICA — declararla con aridad fija es UB en arm64 (los varargs van
        // por la pila en la convención de Apple; la lección ya está aprendida en builtins.rs).
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    }

    pub struct Poller {
        kq: i32,
        /// Cambios PENDIENTES de registrar (se acumulan en `arm` y los presenta el siguiente
        /// `wait` en su mismo syscall). Buffer persistente: capacidad reutilizada entre ciclos.
        changes: Vec<Kevent>,
        /// Buffer de eventos de salida, persistente. Si un ciclo devuelve el buffer LLENO, el
        /// resto sigue pendiente en el kqueue y sale en el ciclo siguiente (kevent no los pierde).
        events: Vec<Kevent>,
        /// Pares (fd, dir) listos del último `wait` (buffer reutilizado).
        ready: Vec<(i32, Dir)>,
    }

    impl Poller {
        pub fn new(wake_rd: i32) -> Poller {
            // SAFETY: syscall sin argumentos; kqueue(2) NO se hereda por fork (no necesita CLOEXEC).
            let kq = unsafe { kqueue() };
            assert!(kq >= 0, "could not create the fiber reactor (kqueue)");
            let mut p = Poller {
                kq,
                changes: Vec::with_capacity(64),
                events: (0..1024).map(|_| Poller::ev(0, 0, 0)).collect(),
                ready: Vec::with_capacity(64),
            };
            // La tubería de despertar, registrada UNA vez y sin oneshot (permanente).
            p.changes.push(Poller::ev(wake_rd, EVFILT_READ, EV_ADD));
            p
        }

        fn ev(fd: i32, filter: i16, flags: u16) -> Kevent {
            Kevent { ident: fd as usize, filter, flags, fflags: 0, data: 0, udata: core::ptr::null_mut() }
        }

        /// Registra (o re-registra) el interés ONESHOT de un fd. F5: INCREMENTAL — antes se
        /// re-registraban TODOS los fds aparcados en cada ciclo (a -c 1000, un changelist de ~1000
        /// entradas por evento); ahora solo entran los intereses NUEVOS, y los no disparados
        /// siguen armados en el kqueue. Un fd CERRADO pierde su knote en silencio: de eso se
        /// encarga el re-armado global que dispara `poke` (ver el reactor).
        pub fn arm(&mut self, fd: i32, dir: Dir) -> bool {
            let filter = if dir == Dir::Write { EVFILT_WRITE } else { EVFILT_READ };
            self.changes.push(Poller::ev(fd, filter, EV_ADD | EV_ONESHOT));
            true // el fallo real (EBADF…) llega como evento EV_ERROR en banda → despierta igual
        }

        /// Presenta los cambios acumulados y espera. Devuelve los (fd, dir) listos en un buffer
        /// interno reutilizado (cero asignaciones en régimen).
        pub fn wait(&mut self, timeout_ms: i32) -> &[(i32, Dir)] {
            let ts = Timespec {
                tv_sec: (timeout_ms as isize) / 1000,
                tv_nsec: ((timeout_ms as isize) % 1000) * 1_000_000,
            };
            let tsp = if timeout_ms < 0 { core::ptr::null() } else { &ts as *const Timespec };
            // SAFETY: buffers propios vivos durante la llamada, del tamaño declarado; fds de
            // sockets del programa (uno cerrado en carrera produce EV_ERROR en banda, no UB).
            let n = unsafe {
                kevent(self.kq, self.changes.as_ptr(), self.changes.len() as i32, self.events.as_mut_ptr(), self.events.len() as i32, tsp)
            };
            self.changes.clear();
            self.ready.clear();
            if n < 0 {
                return &self.ready; // EINTR u otro transitorio: el bucle del reactor reintenta
            }
            for e in &self.events[..n as usize] {
                self.ready.push((e.ident as i32, if e.filter == EVFILT_WRITE { Dir::Write } else { Dir::Read }));
            }
            &self.ready
        }
    }

    /// Crea la tubería de despertar: no-bloqueante y CLOEXEC en ambos extremos (IDEAS §53.4:
    /// `pipe(2)` nace sin CLOEXEC y `F_SETFL` toca los flags de estado, no los del descriptor).
    pub fn wake_pipe() -> (i32, i32) {
        const F_SETFD: i32 = 2;
        const FD_CLOEXEC: i32 = 1;
        const F_SETFL: i32 = 4;
        const O_NONBLOCK: i32 = 0x0004;
        let mut fds = [0i32; 2];
        // SAFETY: `pipe`/`fcntl` sobre un array local; fds recién creados, propiedad de este módulo.
        unsafe {
            assert!(pipe(fds.as_mut_ptr()) == 0, "could not create the reactor wake pipe");
            for fd in fds {
                let _ = fcntl(fd, F_SETFL, O_NONBLOCK);
                let _ = fcntl(fd, F_SETFD, FD_CLOEXEC);
            }
        }
        (fds[0], fds[1])
    }

    /// Toca la tubería (si está llena, el reactor ya tiene un despertar pendiente). El byte
    /// distingue el motivo: 1 = trabajo en el buzón; 2 = POKE de un close (re-armar intereses).
    pub fn wake(wake_wr: i32, tag: u8) {
        // SAFETY: escribe 1 byte de un buffer local a un fd propio no-bloqueante.
        unsafe {
            let _ = write(wake_wr, &tag as *const u8, 1);
        }
    }

    /// Vacía la tubería tras un despertar. Devuelve `true` si algún byte era un POKE (un `close`
    /// del programa, byte 2): el reactor debe RE-ARMAR todos los intereses, porque el knote de un
    /// fd cerrado muere en silencio y su fibra quedaría aparcada para siempre.
    pub fn drain(wake_rd: i32) -> bool {
        let mut poked = false;
        // SAFETY: lee a un buffer local desde un fd propio no-bloqueante.
        unsafe {
            let mut buf = [0u8; 256];
            loop {
                let n = read(wake_rd, buf.as_mut_ptr(), buf.len());
                if n <= 0 {
                    break;
                }
                if buf[..n as usize].contains(&2u8) {
                    poked = true;
                }
            }
        }
        poked
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod sys {
    use super::Dir;
    use std::collections::HashSet;

    const EPOLL_CTL_ADD: i32 = 1;
    const EPOLL_CTL_MOD: i32 = 3;
    const EPOLL_CLOEXEC: i32 = 0o2000000;
    const EPOLLIN: u32 = 0x001;
    const EPOLLOUT: u32 = 0x004;
    const EPOLLERR: u32 = 0x008;
    const EPOLLHUP: u32 = 0x010;
    const EPOLLONESHOT: u32 = 1 << 30;

    // Empaquetado solo en x86_64, como en `src/poll.rs`.
    #[cfg(target_arch = "x86_64")]
    #[repr(C, packed)]
    #[allow(dead_code)]
    struct EpollEvent {
        events: u32,
        data: u64,
    }
    #[cfg(not(target_arch = "x86_64"))]
    #[repr(C)]
    #[allow(dead_code)]
    struct EpollEvent {
        events: u32,
        data: u64,
    }

    unsafe extern "C" {
        fn epoll_create1(flags: i32) -> i32;
        fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: *mut EpollEvent) -> i32;
        fn epoll_wait(epfd: i32, events: *mut EpollEvent, maxevents: i32, timeout: i32) -> i32;
        fn pipe(fds: *mut i32) -> i32;
        // OJO: fcntl es VARIÁDICA — declararla con aridad fija es UB en arm64 (los varargs van
        // por la pila en la convención de Apple; la lección ya está aprendida en builtins.rs).
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    }

    pub struct Poller {
        ep: i32,
        /// fds que este epoll ya conoce (re-ADD da EEXIST: hay que alternar con MOD). Un ONESHOT
        /// disparado sigue "conocido" pero desarmado; se re-arma con MOD.
        known: HashSet<i32>,
        /// Interés pendiente combinado por fd (un fd puede tener esperas de lectura Y escritura);
        /// se arma en `arm` directamente (epoll_ctl es por-fd, no hay changelist que amortizar).
        interest: std::collections::HashMap<i32, u32>,
        events: Vec<EpollEvent>,
        ready: Vec<(i32, Dir)>,
    }

    impl Poller {
        pub fn new(wake_rd: i32) -> Poller {
            // SAFETY: syscall directa; CLOEXEC como en el epoll de la VM (auditoría IDEAS §53.4).
            let ep = unsafe { epoll_create1(EPOLL_CLOEXEC) };
            assert!(ep >= 0, "could not create the fiber reactor (epoll)");
            let mut p = Poller {
                ep,
                known: HashSet::new(),
                interest: std::collections::HashMap::new(),
                events: (0..1024).map(|_| EpollEvent { events: 0, data: 0 }).collect(),
                ready: Vec::with_capacity(64),
            };
            p.ctl(wake_rd, EPOLLIN); // la tubería, permanente (sin ONESHOT)
            p
        }

        /// Arma (o re-arma) el interés de un fd vía epoll_ctl. Ante cualquier fallo devuelve
        /// `false`: el llamador despierta la fibra YA (nunca se pierde; como mucho, de más).
        fn ctl(&mut self, fd: i32, events: u32) -> bool {
            let mut ev = EpollEvent { events, data: fd as u64 };
            let (first, second) = if self.known.contains(&fd) {
                (EPOLL_CTL_MOD, EPOLL_CTL_ADD)
            } else {
                (EPOLL_CTL_ADD, EPOLL_CTL_MOD)
            };
            // SAFETY: `ev` vive durante la llamada; el fd viene de un socket del programa.
            unsafe {
                if epoll_ctl(self.ep, first, fd, &mut ev as *mut EpollEvent) == 0
                    || epoll_ctl(self.ep, second, fd, &mut ev as *mut EpollEvent) == 0
                {
                    self.known.insert(fd);
                    return true;
                }
            }
            self.known.remove(&fd);
            false
        }

        /// F5: INCREMENTAL, como en kqueue. Combina lectura+escritura del mismo fd re-armando con
        /// la unión (el mapa `interest` recuerda la máscara viva de cada fd armado).
        pub fn arm(&mut self, fd: i32, dir: Dir) -> bool {
            let bit = if dir == Dir::Write { EPOLLOUT } else { EPOLLIN };
            let evs = { let e = self.interest.entry(fd).or_insert(0); *e |= bit; *e };
            self.ctl(fd, evs | EPOLLONESHOT)
        }

        pub fn wait(&mut self, timeout_ms: i32) -> &[(i32, Dir)] {
            let cap = self.events.len() as i32;
            // SAFETY: buffer propio del tamaño declarado; contrato normal de epoll_wait.
            let n = unsafe { epoll_wait(self.ep, self.events.as_mut_ptr(), cap, timeout_ms) };
            self.ready.clear();
            if n < 0 {
                return &self.ready; // EINTR: el bucle del reactor reintenta
            }
            for e in &self.events[..n as usize] {
                let evs = e.events;
                let fd = { e.data } as i32; // copia (struct empaquetado en x86_64)
                self.interest.remove(&fd); // el oneshot quedó desarmado: la máscara viva expira
                // ERR/HUP despiertan AMBAS direcciones: la fibra hará la syscall y verá el error.
                if evs & (EPOLLIN | EPOLLERR | EPOLLHUP) != 0 {
                    self.ready.push((fd, Dir::Read));
                }
                if evs & (EPOLLOUT | EPOLLERR | EPOLLHUP) != 0 {
                    self.ready.push((fd, Dir::Write));
                }
            }
            &self.ready
        }
    }

    pub fn wake_pipe() -> (i32, i32) {
        const F_SETFD: i32 = 2;
        const FD_CLOEXEC: i32 = 1;
        const F_SETFL: i32 = 4;
        const O_NONBLOCK: i32 = 0o4000;
        let mut fds = [0i32; 2];
        // SAFETY: `pipe`/`fcntl` sobre un array local; fds recién creados, propiedad de este módulo.
        unsafe {
            assert!(pipe(fds.as_mut_ptr()) == 0, "could not create the reactor wake pipe");
            for fd in fds {
                let _ = fcntl(fd, F_SETFL, O_NONBLOCK);
                let _ = fcntl(fd, F_SETFD, FD_CLOEXEC);
            }
        }
        (fds[0], fds[1])
    }

    /// Ver la variante kqueue: 1 = trabajo en el buzón; 2 = POKE de un close.
    pub fn wake(wake_wr: i32, tag: u8) {
        // SAFETY: escribe 1 byte de un buffer local a un fd propio no-bloqueante.
        unsafe {
            let _ = write(wake_wr, &tag as *const u8, 1);
        }
    }

    /// Ver la variante kqueue: devuelve `true` si hubo POKE (re-armar todos los intereses).
    pub fn drain(wake_rd: i32) -> bool {
        let mut poked = false;
        // SAFETY: lee a un buffer local desde un fd propio no-bloqueante.
        unsafe {
            let mut buf = [0u8; 256];
            loop {
                let n = read(wake_rd, buf.as_mut_ptr(), buf.len());
                if n <= 0 {
                    break;
                }
                if buf[..n as usize].contains(&2u8) {
                    poked = true;
                }
            }
        }
        poked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Lee aparcando la fibra en WouldBlock (el patrón que usará el runtime transpilado en F2).
    fn read_parked(s: &mut TcpStream, buf: &mut [u8]) -> usize {
        loop {
            match s.read(buf) {
                Ok(n) => return n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => park_readable(s.as_raw_fd()),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => panic!("read: {e}"),
            }
        }
    }

    fn write_parked(s: &mut TcpStream, mut buf: &[u8]) {
        while !buf.is_empty() {
            match s.write(buf) {
                Ok(0) => panic!("write: connection closed"),
                Ok(n) => buf = &buf[n..],
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => park_writable(s.as_raw_fd()),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => panic!("write: {e}"),
            }
        }
    }

    #[test]
    fn run_blocking_returns_the_value_from_a_fiber() {
        let h = spawn(|| {
            let v = run_blocking(|| 40 + 2);
            assert_eq!(v, 42);
        });
        h.join().expect("la fibra termina sin panic");
    }

    #[test]
    fn run_blocking_outside_a_fiber_calls_directly() {
        assert_eq!(run_blocking(|| 7), 7);
    }

    #[test]
    fn run_blocking_carries_the_pool_thread_errno_back_to_the_caller() {
        // Una extern C estilo POSIX deja su motivo en el errno del hilo del POOL; run_blocking lo
        // repone en el hilo del llamador al despertar. Aquí se escribe un valor centinela directo.
        let h = spawn(|| {
            // SAFETY: errno del hilo actual, siempre válido (escritura previa para distinguir).
            unsafe { *blocking_errno_ptr() = 0 };
            run_blocking(|| unsafe { *blocking_errno_ptr() = 4242 });
            assert_eq!(unsafe { *blocking_errno_ptr() }, 4242, "el errno del pool viaja al llamador");
        });
        h.join().expect("la fibra termina sin panic");
    }

    #[test]
    fn run_blocking_propagates_the_panic_to_the_caller() {
        let h = spawn(|| {
            run_blocking(|| panic!("boom del pool"));
        });
        let err = h.join().expect_err("el panic del closure llega a la fibra");
        assert!(err.contains("boom del pool"), "mensaje: {err}");
    }

    #[test]
    fn run_blocking_does_not_stall_sibling_fibers_on_the_same_worker() {
        // El closure bloqueante (en un hilo del pool) espera a que TODAS las hermanas hayan
        // corrido. Si `run_blocking` bloqueara el worker en vez de aparcar la fibra, las hermanas
        // fijadas a ese mismo worker no correrían jamás y el test COLGARÍA (64 fibras > nº de
        // workers garantiza que varias cohabitan con la bloqueante). Determinista, sin tiempos.
        static RAN: AtomicUsize = AtomicUsize::new(0);
        let blocker = spawn(|| {
            run_blocking(|| {
                while RAN.load(Ordering::SeqCst) < 64 {
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
        });
        let handles: Vec<_> = (0..64)
            .map(|_| {
                spawn(|| {
                    RAN.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("la fibra hermana termina");
        }
        blocker.join().expect("la fibra bloqueante termina");
    }

    #[test]
    fn thousands_of_fibers_run_to_completion_on_few_workers() {
        static DONE: AtomicUsize = AtomicUsize::new(0);
        let handles: Vec<_> = (0..2000)
            .map(|_| {
                spawn(|| {
                    yield_now(); // fuerza al menos una vuelta por el scheduler
                    DONE.fetch_add(1, Ordering::Relaxed);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("la fibra termina sin panic");
        }
        assert_eq!(DONE.load(Ordering::Relaxed), 2000);
    }

    #[test]
    fn parked_io_echoes_across_two_hundred_concurrent_connections() {
        const N: usize = 200;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        listener.set_nonblocking(true).expect("nonblocking listener");

        // Servidor: una fibra acepta (aparcada en el listener) y lanza una fibra de eco por conexión.
        let server = spawn(move || {
            let mut echoes = Vec::new();
            for _ in 0..N {
                let mut conn = loop {
                    match listener.accept() {
                        Ok((c, _)) => break c,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            park_readable(listener.as_raw_fd())
                        }
                        Err(e) => panic!("accept: {e}"),
                    }
                };
                echoes.push(spawn(move || {
                    conn.set_nonblocking(true).expect("nonblocking conn");
                    let mut buf = [0u8; 32];
                    let n = read_parked(&mut conn, &mut buf);
                    write_parked(&mut conn, &buf[..n]);
                }));
            }
            for e in echoes {
                e.join().expect("la fibra de eco termina");
            }
        });

        // Clientes: N fibras concurrentes; cada una escribe su marca y verifica el eco.
        //
        // ⚠️ Lección de este mismo test: el `connect` BLOQUEANTE aquí construye un livelock real.
        // Con N > backlog del listener (128), los SYN sobrantes se retransmiten DENTRO del worker;
        // si los N workers acaban clavados en `connect`, el servidor — listo en la cola — no tiene
        // dónde correr, nadie acepta, y todo muere por timeout. Es la propiedad documentada del
        // modelo ("una syscall bloqueante retiene al worker"): las syscalls bloqueantes dentro de
        // una fibra deben ser ACOTADAS. De ahí el connect con timeout corto + reintento con cesión
        // (el connect real del runtime irá no-bloqueante con `park_writable` en F2).
        let clients: Vec<_> = (0..N)
            .map(|i| {
                spawn(move || {
                    let mut s = loop {
                        match TcpStream::connect_timeout(&addr, Duration::from_millis(50)) {
                            Ok(s) => break s,
                            Err(_) => yield_now(), // backlog lleno: cede y deja correr al servidor
                        }
                    };
                    s.set_nonblocking(true).expect("nonblocking client");
                    let msg = format!("fiber-{i}");
                    write_parked(&mut s, msg.as_bytes());
                    let mut buf = [0u8; 32];
                    let mut got = Vec::new();
                    while got.len() < msg.len() {
                        let n = read_parked(&mut s, &mut buf);
                        assert!(n > 0, "el servidor cerró antes del eco completo");
                        got.extend_from_slice(&buf[..n]);
                    }
                    assert_eq!(got, msg.as_bytes());
                })
            })
            .collect();
        for c in clients {
            c.join().expect("la fibra cliente termina");
        }
        server.join().expect("la fibra servidora termina");
    }

    #[test]
    fn sleeping_fibers_wake_after_their_deadline() {
        // El contrato de sleep es "duerme AL MENOS ms". La latencia despertar→correr es mejor-
        // esfuerzo y NO se asevera: con fibras FIJADAS, una vecina que bloquee su worker retrasa
        // la reanudación sin límite teórico (los tests de eco de esta MISMA suite, corriendo en
        // paralelo, encadenan connect_timeout de 50 ms; con RAYLANG_THREADS=2 se midieron
        // retrasos > 400 ms). Cualquier aserción de orden entre fibras dormidas es flaky por
        // construcción — se aprendió dos veces antes de rendirse.
        let handles: Vec<_> = [400u64, 10, 120]
            .into_iter()
            .map(|ms| {
                spawn(move || {
                    let t0 = Instant::now();
                    fiber_sleep(ms as i64);
                    assert!(t0.elapsed() >= Duration::from_millis(ms), "duerme al menos {ms} ms");
                })
            })
            .collect();
        for h in handles {
            h.join().expect("la fibra durmiente termina");
        }
    }

    #[test]
    fn the_fiber_local_slot_survives_reactor_parks_under_load() {
        // Estrés de la VÍA DEL REACTOR (inbox → fds → runq), que el test de slots con `yield` no
        // recorre: N clientes de eco concurrentes que marcan su slot, aparcan en E/S real muchas
        // veces, y verifican su identidad tras CADA park. Caza corrupción del puntero fiber-local
        // en migraciones entre workers.
        const N: usize = 100;
        const ROUNDS: usize = 20;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        listener.set_nonblocking(true).expect("nonblocking listener");
        let server = spawn(move || {
            let mut echoes = Vec::new();
            for _ in 0..N {
                let mut conn = loop {
                    match listener.accept() {
                        Ok((c, _)) => break c,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            park_readable(listener.as_raw_fd())
                        }
                        Err(e) => panic!("accept: {e}"),
                    }
                };
                echoes.push(spawn(move || {
                    conn.set_nonblocking(true).expect("nonblocking conn");
                    let mut buf = [0u8; 8];
                    for _ in 0..ROUNDS {
                        let n = read_parked(&mut conn, &mut buf);
                        if n == 0 {
                            break;
                        }
                        write_parked(&mut conn, &buf[..n]);
                    }
                }));
            }
            for e in echoes {
                e.join().expect("la fibra de eco termina");
            }
        });
        let clients: Vec<_> = (0..N as u64)
            .map(|i| {
                spawn(move || {
                    with_local(|slot| *slot = Some(Box::new(i))).expect("en fibra");
                    let mut s = loop {
                        match TcpStream::connect_timeout(&addr, Duration::from_millis(50)) {
                            Ok(s) => break s,
                            Err(_) => yield_now(),
                        }
                    };
                    s.set_nonblocking(true).expect("nonblocking client");
                    let msg = i.to_be_bytes();
                    let mut buf = [0u8; 8];
                    for _ in 0..ROUNDS {
                        write_parked(&mut s, &msg);
                        let mut got = 0;
                        while got < 8 {
                            let n = read_parked(&mut s, &mut buf[got..]);
                            assert!(n > 0, "eco cortado");
                            got += n;
                        }
                        assert_eq!(buf, msg, "el eco es el mío");
                        // La identidad del slot tras varios parks (pudo migrar de worker N veces).
                        let mine = with_local(|slot| {
                            *slot.as_ref().expect("poblado").downcast_ref::<u64>().expect("u64")
                        })
                        .expect("en fibra");
                        assert_eq!(mine, i, "el slot fiber-local es de ESTA fibra");
                    }
                })
            })
            .collect();
        for c in clients {
            c.join().expect("la fibra cliente termina");
        }
        server.join().expect("la fibra servidora termina");
    }

    #[test]
    fn a_read_park_with_deadline_times_out_on_a_silent_socket() {
        // Un socket sin datos + plazo de 30 ms → despierta con timed_out=true (el "read timeout"
        // de M56.4 del lado fibras). Después llegan datos con plazo holgado → false y lee.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let h = spawn(move || {
            let (mut server_side, _) = listener.accept().expect("accept"); // bloqueante: test corto
            let client_started = Instant::now();
            server_side.set_nonblocking(true).expect("nonblocking");
            let fd = server_side.as_raw_fd();
            assert!(wait_readable_timeout(fd, 30), "sin datos: vence el plazo");
            assert!(client_started.elapsed() >= Duration::from_millis(25), "no antes de tiempo");
            // El cliente escribe a los ~60 ms (ver abajo): ahora el plazo holgado NO vence.
            assert!(!wait_readable_timeout(fd, 5_000), "con datos en camino: readiness gana");
            let mut buf = [0u8; 4];
            let n = server_side.read(&mut buf).expect("lee tras readiness");
            assert_eq!(&buf[..n], b"ping");
        });
        let mut client = TcpStream::connect(addr).expect("connect");
        std::thread::sleep(Duration::from_millis(60));
        client.write_all(b"ping").expect("write");
        h.join().expect("la fibra del test termina");
    }

    #[test]
    fn the_fiber_local_slot_travels_with_the_fiber_across_parks() {
        // El slot fiber-local sobrevive a parks (que pueden reanudar en OTRO worker) y es privado
        // por fibra: N fibras escriben su marca, ceden varias veces, y cada una relee LA SUYA.
        let handles: Vec<_> = (0..50)
            .map(|i: usize| {
                spawn(move || {
                    with_local(|slot| *slot = Some(Box::new(i))).expect("en fibra hay slot");
                    for _ in 0..5 {
                        yield_now();
                        let got = with_local(|slot| {
                            *slot.as_ref().expect("sigue poblado").downcast_ref::<usize>().expect("mi tipo")
                        })
                        .expect("en fibra hay slot");
                        assert_eq!(got, i, "el slot es de ESTA fibra");
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("la fibra termina");
        }
        // Fuera de fibra no hay slot: el llamador cae a su thread-local.
        assert!(with_local(|_| ()).is_none());
    }

    #[test]
    fn wait_readable_blocks_a_plain_thread_until_data_arrives() {
        // La vía NO-fibra de wait_readable (poll(2) bloqueante): el hilo del test espera datos de
        // una fibra sin estar él mismo en el scheduler.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let _writer = spawn(move || {
            let mut s = TcpStream::connect(addr).expect("connect");
            fiber_sleep(40);
            s.write_all(b"late").expect("write");
            // mantiene el socket vivo hasta que el lector termine
            fiber_sleep(200);
        });
        let (mut conn, _) = listener.accept().expect("accept");
        conn.set_nonblocking(true).expect("nonblocking");
        let t0 = Instant::now();
        wait_readable(conn.as_raw_fd());
        assert!(t0.elapsed() >= Duration::from_millis(20), "esperó de verdad");
        let mut buf = [0u8; 4];
        assert_eq!(conn.read(&mut buf).expect("lee"), 4);
        // timeout vencido en la vía no-fibra:
        assert!(wait_readable_timeout(conn.as_raw_fd(), 30), "sin más datos: vence");
    }

    #[test]
    fn a_panicking_fiber_reports_the_message_and_the_scheduler_survives() {
        let bad = spawn(|| panic!("boom in fiber"));
        assert_eq!(bad.join(), Err("boom in fiber".to_string()));
        // El scheduler sigue vivo: una fibra posterior corre con normalidad.
        let ok = spawn(|| {});
        assert_eq!(ok.join(), Ok(()));
    }

    #[test]
    fn a_fiber_can_spawn_other_fibers() {
        static DONE: AtomicUsize = AtomicUsize::new(0);
        let h = spawn(|| {
            let inner: Vec<_> = (0..50)
                .map(|_| {
                    spawn(|| {
                        DONE.fetch_add(1, Ordering::Relaxed);
                    })
                })
                .collect();
            // join desde una fibra CEDE entre comprobaciones (ver JoinHandle::join): correcto
            // incluso con RAYLANG_THREADS=1 y con las hijas fijadas a este mismo worker.
            for i in inner {
                i.join().expect("la fibra interior termina");
            }
        });
        h.join().expect("la fibra exterior termina");
        assert_eq!(DONE.load(Ordering::Relaxed), 50);
    }
}
