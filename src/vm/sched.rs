//! El scheduler de fibras de la VM (M12/M38; movimiento puro, usar `git log --follow`).
//!
//! Estado compartido (`Shared`, `Fiber`, `Parked`/`IoParked`, `TaskSlot`/`ChanSlot`) y los
//! métodos de scheduling del `impl Vm` (arranque de workers M38, `poll_next`, aparcamiento/
//! despertar de fibras, cancelación M12.5). El bucle de despacho (`run_loop`) sigue en `mod.rs`.

use super::*;

/// Una **fibra** (green thread, M12.1): el estado suspendido de una tarea — su pila de marcos y su pila de
/// operandos. M38.3b: la fibra EN CURSO vive en `Vm.cur` (un `Fiber`); las demás esperan en `Shared`.
#[derive(Default)]
pub(super) struct Fiber {
    pub(super) frames: Vec<CallFrame>,
    pub(super) stack: Vec<HeapValue>,
    /// M38.1b-2: el **heap propio** de la fibra (aislamiento por actores, §46.2). `Vm.heap` es el de la
    /// fibra en curso; al conmutar se salva aquí y se restaura el de la siguiente. Un objeto de este heap
    /// solo lo alcanzan los marcos/pila de esta fibra (invariante: sin handles cruzados entre heaps).
    pub(super) heap: Heap,
    pub(super) is_main: bool,
    /// M12.3: la `Task` que esta fibra debe rellenar al terminar (`None` para `main`).
    pub(super) task: Option<Handle>,
    /// M12.3: pila de scopes activos en esta fibra (structured concurrency); las tareas que lance
    /// mientras un scope esté activo quedan adscritas al más interno.
    pub(super) scopes: Vec<ScopeFrame>,
    /// TA4 (bench treealloc, 22 jul 2026): handle CANÓNICO por variante de enum SIN payload
    /// (`Option.None`, etc.). Un enum es inmutable y sin identidad observable → todas las
    /// construcciones de la misma variante vacía comparten un solo objeto del heap (antes:
    /// una asignación POR construcción — en binary-trees, 2 `None` por hoja). Raíz del GC.
    pub(super) unit_enums: crate::gc::MapStore2,
    /// M97.2: pila de marcadores de `try_call` activos en esta fibra. Cada `TryCall` empuja uno con
    /// la altura de marcos y de pila de operandos ANTES de la llamada; al fallar, el error se
    /// desenrolla hasta el más interno en vez de tumbar la fibra. Anidan (un `try_call` dentro de
    /// otro) porque es una pila, y son POR FIBRA: un `spawn` dentro de un `try_call` no hereda el
    /// marcador (su fallo lo captura su `Task`, como siempre).
    pub(super) try_markers: Vec<TryMarker>,
}

/// Un `try_call` en vuelo (M97.2): a dónde volver si su cuerpo falla.
pub(super) struct TryMarker {
    /// `frames.len()` en el momento del `TryCall`. Al volver bien, el `Return` que devuelve los
    /// marcos a esta altura es el del cuerpo protegido; al fallar, se truncan hasta aquí.
    pub(super) frames_len: usize,
    /// Altura de la pila de operandos tras sacar la closure. El resultado (`[]` o `[msg]`) se
    /// empuja sobre esta altura, así el llamador ve exactamente un valor, falle o no.
    pub(super) stack_len: usize,
    /// Nº de scopes activos al entrar. Un `scope` abierto dentro del cuerpo que falla no puede
    /// quedar huérfano en la pila de la fibra: se cierran hasta esta altura al desenrollar.
    pub(super) scopes_len: usize,
}

/// Un scope activo (M12.3): la lista de tareas lanzadas mientras estuvo en la cima de la pila de la
/// fibra. Al cerrarse (`ScopeEnd`), el scope une a todas (las espera) y propaga el primer fallo.
pub(super) struct ScopeFrame {
    pub(super) children: Vec<Handle>,
}

/// Qué espera una fibra **bloqueada** (el handle por el que espera va en `Parked.on`):
/// - `Recv`: bloqueada en `recv` (canal vacío y abierto) → despierta cuando alguien envía o lo cierra.
/// - `Send(v)`: bloqueada en `send` (canal acotado y lleno) → despierta cuando un `recv` libera un hueco;
///   sostiene el valor `v` que aún no ha podido entregar (es una raíz del GC).
/// - `Join`: bloqueada en `join`/`ScopeEnd` esperando a una **tarea** (M12.3); al completarse la tarea se
///   la despierta y re-ejecuta el opcode (que rebobinó su `ip`).
/// - `Select`: bloqueada en `select` esperando a que CUALQUIERA de un conjunto de canales esté listo
///   (M12.4); `Parked.on` es el handle del **arreglo** de canales. Al despertar re-ejecuta el `select`.
pub(super) enum Waiting {
    Recv,
    Send(HeapValue),
    Join,
    Select,
}

/// Una fibra **bloqueada**, con el handle por el que espera (`on`: un canal para Recv/Send, una tarea para
/// Join) y qué espera.
pub(super) struct Parked {
    pub(super) on: Handle,
    pub(super) fiber: Fiber,
    pub(super) waiting: Waiting,
}

/// M15.5/M17: una fibra aparcada esperando **E/S de red**, junto al descriptor (`fd`) del socket por el
/// que espera. El `fd` permite que el scheduler lo registre en el poller del SO (`kqueue`/`epoll`, M17)
/// y despierte **solo** las fibras de los sockets que quedaron listos, en vez de re-encolarlas todas.
pub(super) struct IoParked {
    pub(super) fd: i32,
    pub(super) fiber: Fiber,
    /// `None` = aparcada esperando **lectura** (el caso de M15.5/M17). `Some` = esperando que el socket
    /// sea **escribible** para terminar una escritura parcial (cesión en `socket_write`, post-M19.4): el
    /// poller registra `fd` con interés de escritura y, al despertar, el scheduler drena lo que falta.
    pub(super) pending_write: Option<PendingWrite>,
    /// M56.4: el handle del socket por el que espera (para marcar su timeout al vencer el deadline).
    pub(super) handle: i64,
    /// M56.4: instante en que la espera de LECTURA vence (`net.set_read_timeout`): `io_wait` espera
    /// como mucho hasta el deadline más próximo y, al vencer, marca el handle y despierta la fibra
    /// (la lectura re-ejecutada devuelve el error de timeout). `None` = espera indefinida (default).
    pub(super) deadline: Option<std::time::Instant>,
}

/// Una escritura que bloqueó a medias: el handle del socket y los octetos que aún faltan por enviar.
pub(super) struct PendingWrite {
    pub(super) handle: i64,
    pub(super) remaining: Vec<u8>,
}

/// M98.1: un slot del almacén de tareas. `task: None` = libre (en `free_tasks`); `gen` se incrementa
/// al liberar, así un handle viejo (`gen<<32 | idx`) sobre un slot reusado NO colisiona (ABA).
#[derive(Default)]
pub(super) struct TaskSlot {
    pub(super) generation: u32,
    pub(super) task: Option<VmTask>,
}

/// M98.1: mensaje del doble-join (una tarea es de un solo consumidor: `join`/`try_join` la consumen,
/// y el `scope` consume a sus hijas al cerrar). Byte-idéntico en el runtime nativo.
pub(super) const TASK_CONSUMED: &str = "task already consumed (join/try_join takes the task)";

/// M98.3: un slot del almacén de canales. `chan: None` = libre (en `free_channels`); `generation`
/// se incrementa al liberar → un handle viejo sobre un slot reusado NO colisiona (ABA), y las
/// operaciones sobre él responden como sobre un canal cerrado y vacío (indistinguible).
#[derive(Default)]
pub(super) struct ChanSlot {
    pub(super) generation: u32,
    pub(super) chan: Option<VmChannel>,
}

/// M38.3a: el estado del scheduler que N hilos compartirían (M38.3b: tras `Arc<Mutex<Shared>>`). Con los
/// heaps aislados por fibra (M38.1), es lo ÚNICO compartido: las colas de fibras listas/aparcadas y los
/// almacenes del host de canales/tareas. La ejecución de cada fibra (frames/stack/heap/…) es thread-local.
#[derive(Default)]
pub(super) struct Shared {
    /// Fibras listas para ejecutar, en orden FIFO (scheduler determinista).
    pub(super) ready: VecDeque<Fiber>,
    /// Fibras bloqueadas en `recv`/`send`/`join`, con el handle (canal o tarea) que esperan.
    pub(super) parked: Vec<Parked>,
    /// M15.5/M17: fibras aparcadas esperando **E/S de red** (`accept`/`read` que dieron `WouldBlock`),
    /// cada una con el `fd` de su socket. El scheduler espera readiness real en el poller del SO (M17).
    pub(super) io_parked: Vec<IoParked>,
    /// M88.1: el canal de `signals()` (singleton) y el fd de LECTURA de su self-pipe.
    /// `signal_chan.is_some()` = fontanería instalada (es la FUENTE DE VERDAD; `signal_fd`
    /// solo es válido entonces — `Shared` deriva `Default`, y el default de `signal_fd`
    /// sería 0, que NO debe interpretarse como "fd válido"). Instalada, el fd entra al
    /// poller de `io_wait` y las fibras aparcadas en el canal NO son deadlock (esperan fuera).
    pub(super) signal_chan: Option<usize>,
    pub(super) signal_fd: i32,
    /// Canales `Channel<T>` (M12.1): sincronización COMPARTIDA entre actores, fuera del GC de las fibras
    /// (§46.2). Se referencian por id vía `HeapValue::Channel(id)`. El GC rootea sus valores en tránsito.
    /// M98.3: slots con generación + free-list, como las tareas (M98.1) — un canal se LIBERA al quedar
    /// **cerrado y drenado** (en el `close` si la cola está vacía; si no, en el `recv` que la vacía).
    /// Es seguro porque un canal liberado se comporta IDÉNTICO a uno cerrado y vacío: `recv` → None,
    /// `send` → "send on a closed channel", `close` → no-op (ya era idempotente), `select` → listo.
    pub(super) channels: Vec<ChanSlot>,
    /// M98.3: índices de slots libres de `channels`, para reuso.
    pub(super) free_channels: Vec<usize>,
    /// Tareas `Task<T>` (M12.3): compartidas entre la fibra hija y quien la une, fuera del GC. Se
    /// referencian por id vía `HeapValue::Task(id)`. El GC rootea el valor de `Done`.
    /// M98.1: almacén de **slots con generación** + free-list — las entradas se LIBERAN (antes solo
    /// crecía: el webserver fugaba ~1 KB/request). El handle codifica `gen << 32 | idx`; un handle
    /// stale (slot liberado/reusado) no colisiona: la generación no casa. Libera: `join`/`try_join`
    /// (consumen la tarea, semántica M98.1) y el `ScopeEnd` (consume a sus hijas al cerrar).
    pub(super) tasks: Vec<TaskSlot>,
    /// M98.1: índices de slots libres de `tasks`, para reuso.
    pub(super) free_tasks: Vec<usize>,
    /// M38.3b paso 3: nº de workers que **están ejecutando** una fibra ahora mismo (no ociosos). Invariante
    /// clave del scheduler M:N: un worker que toma una fibra de `ready` hace `running += 1`; cuando la aparca
    /// o termina, `running -= 1`. Un worker ocioso sólo puede declarar **deadlock** cuando `running == 0` (si
    /// alguien ejecuta, aún puede producir trabajo listo vía un canal). Con N=1 oscila 1↔0 trivialmente.
    pub(super) running: usize,
    /// M38.3b paso 3: el **resultado del programa**, fijado UNA vez (semántica Go: cuando `main` retorna, todo
    /// el programa termina; o un error fatal / deadlock). Su presencia es la **señal de apagado**: los demás
    /// workers, al verla, se detienen. El orquestador lo lee tras unir a los hilos.
    pub(super) outcome: Option<Result<HeapValue, RuntimeError>>,
}

impl Shared {
    /// M98.1: aloja una tarea nueva (reusa un slot libre si hay) y devuelve su handle
    /// (`gen << 32 | idx`). El `HeapValue::Task(usize)` transporta el handle tal cual.
    pub(super) fn alloc_task(&mut self) -> usize {
        let vt = VmTask { state: TaskState::Pending, heap: Heap::new() };
        if let Some(idx) = self.free_tasks.pop() {
            let slot = &mut self.tasks[idx];
            slot.task = Some(vt);
            (slot.generation as usize) << 32 | idx
        } else {
            self.tasks.push(TaskSlot { generation: 0, task: Some(vt) });
            self.tasks.len() - 1
        }
    }

    /// La tarea viva de un handle, o `None` si el handle es stale (slot liberado o reusado).
    pub(super) fn task(&self, h: usize) -> Option<&VmTask> {
        let slot = self.tasks.get(h & 0xFFFF_FFFF)?;
        if slot.generation as usize != h >> 32 {
            return None;
        }
        slot.task.as_ref()
    }

    /// Versión mutable de `task`.
    pub(super) fn task_mut(&mut self, h: usize) -> Option<&mut VmTask> {
        let slot = self.tasks.get_mut(h & 0xFFFF_FFFF)?;
        if slot.generation as usize != h >> 32 {
            return None;
        }
        slot.task.as_mut()
    }

    /// M98.1: **consume** la tarea — saca el `VmTask` (con su heap: al soltarlo se libera la memoria
    /// del resultado), incrementa la generación (mata handles viejos) y encola el slot como libre.
    pub(super) fn take_task(&mut self, h: usize) -> Option<VmTask> {
        let idx = h & 0xFFFF_FFFF;
        let slot = self.tasks.get_mut(idx)?;
        if slot.generation as usize != h >> 32 || slot.task.is_none() {
            return None;
        }
        slot.generation = slot.generation.wrapping_add(1);
        self.free_tasks.push(idx);
        slot.task.take()
    }

    // --- M98.3: el almacén de canales, misma anatomía que el de tareas ---

    /// Aloja un canal nuevo (reusa un slot libre si hay) y devuelve su handle (`gen << 32 | idx`).
    pub(super) fn alloc_channel(&mut self, cap: Option<usize>) -> usize {
        let vc = VmChannel { queue: VecDeque::new(), closed: false, cap, heap: Heap::new() };
        if let Some(idx) = self.free_channels.pop() {
            let slot = &mut self.channels[idx];
            slot.chan = Some(vc);
            (slot.generation as usize) << 32 | idx
        } else {
            self.channels.push(ChanSlot { generation: 0, chan: Some(vc) });
            self.channels.len() - 1
        }
    }

    /// El canal vivo de un handle, o `None` si es stale (liberado: se comporta como cerrado y vacío).
    pub(super) fn chan(&self, h: usize) -> Option<&VmChannel> {
        let slot = self.channels.get(h & 0xFFFF_FFFF)?;
        if slot.generation as usize != h >> 32 {
            return None;
        }
        slot.chan.as_ref()
    }

    /// Versión mutable de `chan`.
    pub(super) fn chan_mut(&mut self, h: usize) -> Option<&mut VmChannel> {
        let slot = self.channels.get_mut(h & 0xFFFF_FFFF)?;
        if slot.generation as usize != h >> 32 {
            return None;
        }
        slot.chan.as_mut()
    }

    /// M98.3: libera el canal (cerrado y drenado): suelta su heap, incrementa la generación y
    /// encola el slot como libre. Idempotente sobre handles stale.
    pub(super) fn free_channel(&mut self, h: usize) {
        let idx = h & 0xFFFF_FFFF;
        if let Some(slot) = self.channels.get_mut(idx) {
            if slot.generation as usize == h >> 32 && slot.chan.is_some() {
                slot.chan = None;
                slot.generation = slot.generation.wrapping_add(1);
                self.free_channels.push(idx);
            }
        }
    }
}


impl<'a> Vm<'a> {
    /// M38.3b paso 3: el bucle de un **worker**. Toma su primera fibra de `ready` (`poll_next`), la ejecuta
    /// (`run_loop`, que entre fibras vuelve a `poll_next`) hasta que `main` termina / hay un fatal / otro
    /// worker apagó el programa. Fija `Shared.outcome` con lo que ESTE worker determinó (si aún no lo fijó
    /// otro). No devuelve nada: el resultado viaja por `outcome`.
    pub(super) fn run_worker(&mut self) {
        match self.poll_next(0, 0) {
            Ok(true) => {}          // fibra cargada en `self.cur`
            Ok(false) => return,    // el programa ya terminó (outcome fijado por otro)
            Err(e) => {
                // Sin fibras ejecutables desde el arranque (no debería con main en cola): registra el fatal.
                let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                if sh.outcome.is_none() { sh.outcome = Some(Err(e)); }
                return;
            }
        }
        let res = self.run_loop();
        self.cur.heap.dump_probe(); // RAY_HEAP_STATS=1: picos exactos del heap de este worker
        // Si nos detuvimos porque otro worker ya fijó el outcome (`stop`), no lo pisamos.
        if !self.stop {
            let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
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
    pub(super) fn poll_next(&mut self, line: usize, col: usize) -> Result<bool, RuntimeError> {
        loop {
            let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
            if sh.outcome.is_some() {
                return Ok(false); // otro worker apagó el programa
            }
            // M88.1: entrega de señales pendientes en cada conmutación (bandera atómica barata).
            if crate::builtins::signals_pending() {
                Self::deliver_signals(&mut sh);
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
                if !sh.io_parked.is_empty() || sh.signal_chan.is_some() {
                    // M88.1: con la fontanería de señales instalada, "todo aparcado" no es
                    // deadlock — el exterior puede despertar el programa por el self-pipe.
                    Self::io_wait(&mut sh);
                    continue; // io_wait dejó fibras en `ready`; reintenta el pop
                }
                let msg = if !sh.parked.is_empty() {
                    "deadlock: all fibers are blocked waiting on a channel or a task"
                } else {
                    "no runnable fibers"
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

    // ----- Scheduler de fibras (M12.1 / M12.3) -----

    /// Empaqueta la fibra en ejecución (vaciando los campos de la VM) para aparcarla o descartarla. M12.3:
    /// además de `frames`/`stack`/`is_main`, lleva su `Task` y su pila de scopes.
    /// M38.3b: la fibra en curso ES `cur` → conmutar es un swap (deja una `Fiber` vacía).
    pub(super) fn take_current_fiber(cur: &mut Fiber) -> Fiber {
        std::mem::take(cur)
    }

    /// Cesión en `socket_write` (post-M19.4): la escritura llenó el buffer de envío y `remaining` no
    /// cupo. Aparca la fibra actual esperando que `handle` sea **escribible** (el `ip` ya apunta tras el
    /// opcode de escritura; el resultado se empuja al terminar, en `finish_parked_write`).
    /// M38.3b paso 3: método `&mut self`. Aparca la fibra bajo el guard (con `running -= 1`), suelta el
    /// lock y carga la siguiente con `poll_next` (que, en M:N, puede esperar a otro worker).
    pub(super) fn park_write(&mut self, handle: i64, remaining: Vec<u8>, line: usize, col: usize) -> Result<(), RuntimeError> {
        let fd = crate::builtins::raw_fd(handle).unwrap_or(-1);
        {
            let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
            let fiber = Self::take_current_fiber(&mut self.cur);
            sh.io_parked.push(IoParked { fd, fiber, pending_write: Some(PendingWrite { handle, remaining }), handle, deadline: None });
            sh.running -= 1; // esta fibra se aparcó → este worker queda ocioso
        }
        if !self.poll_next(line, col)? { self.stop = true; }
        Ok(())
    }

    /// Una fibra aparcada por escritura despertó (su socket es escribible): drena lo que falta. Si lo
    /// completa (o falla), empuja el resultado etiquetado (`["ok",""]`/`["err",msg]`) en su pila y la pone
    /// lista; si aún bloquea, la re-aparca con el resto. (`allocate` no colecta aquí → sin riesgo de GC.)
    pub(super) fn finish_parked_write(shared: &mut Shared, fd: i32, mut fiber: Fiber, mut pw: PendingWrite) {
        let result = match crate::builtins::socket_write_nb(pw.handle, &pw.remaining) {
            Ok(n) if n == pw.remaining.len() => Ok(()),
            Ok(n) => {
                pw.remaining.drain(..n); // descarta lo ya enviado y re-aparca el resto
                let handle = pw.handle;
                shared.io_parked.push(IoParked { fd, fiber, pending_write: Some(pw), handle, deadline: None });
                return;
            }
            Err(e) => Err(e),
        };
        let elems = match result {
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
    pub(super) fn on_fiber_done(&mut self, result: HeapValue) -> Result<Option<HeapValue>, RuntimeError> {
        if self.cur.is_main {
            return Ok(Some(result));
        }
        {
            let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
            if let Some(task) = self.cur.task.take() {
                // M38.1b-2: el resultado vive en el heap de ESTA fibra (que se descarta al terminar) → se
                // transfiere al heap de la tarea, donde `join` lo recogerá. M98.1: si el slot ya fue
                // consumido (el scope cerró en su camino de fallo con esta hija aún corriendo tras ser
                // cancelada), el handle es stale → el resultado se descarta (semántica de cancelación).
                if let Some(vt) = sh.task_mut(task) {
                    let mut t_heap = std::mem::take(&mut vt.heap);
                    let r2 = transfer_value(&self.cur.heap, &mut t_heap, &result, &mut HashMap::new());
                    let vt = sh.task_mut(task).expect("just seen live");
                    vt.heap = t_heap;
                    vt.state = TaskState::Done(r2);
                    Self::wake_task_waiters(&mut sh, task);
                }
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
    /// M97.2: desenrolla el fallo `e` hasta el `try_call` más interno y deja `[msg]` en la pila de
    /// operandos, en vez de tumbar la fibra. El llamador reanuda justo tras el `TryCall` (el `ip`
    /// del marco que lo ejecutó ya apunta después, porque el avance ocurre antes de ejecutar).
    ///
    /// Es el hermano de `fail_current_fiber` para un tramo acotado: misma disciplina con las tareas
    /// huérfanas (un `scope` abierto dentro del cuerpo que falló cancela sus hijas) y mismo trato
    /// del mensaje (viaja SIN traza ni posición; quien lo observe pone la suya).
    pub(super) fn unwind_to_try_marker(&mut self, e: RuntimeError) {
        let m = self
            .cur
            .try_markers
            .pop()
            .expect("the caller checks there is a marker");
        // Los marcos del tramo abortado se descartan reciclando sus locales, como hace `Return`:
        // sin esto el pool de locales (Opt.2) los perdería en cada recuperación.
        while self.cur.frames.len() > m.frames_len {
            if let Some(f) = self.cur.frames.pop() {
                self.recycle_locals(f.locals);
            }
        }
        // Scopes abiertos dentro del cuerpo que falló: sus hijas no pueden quedar huérfanas.
        if self.cur.scopes.len() > m.scopes_len {
            let orphans: Vec<usize> = self.cur.scopes[m.scopes_len..]
                .iter()
                .flat_map(|s| s.children.iter().copied())
                .collect();
            self.cur.scopes.truncate(m.scopes_len);
            if !orphans.is_empty() {
                let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
                for c in orphans {
                    Self::cancel_task(&mut sh, c);
                }
            }
        }
        self.cur.stack.truncate(m.stack_len);
        let arr = self
            .cur
            .heap
            .allocate(crate::gc::Obj::Array(vec![HeapValue::Str(e.msg)]));
        self.push(HeapValue::Obj(arr));
    }

    pub(super) fn fail_current_fiber(&mut self, e: RuntimeError) -> Result<(), RuntimeError> {
        {
            let mut sh = self.shared.lock().expect("the scheduler Mutex should not be poisoned");
            if let Some(task) = self.cur.task.take() {
                let msg = e.msg.clone(); // solo el mensaje; el join que lo observe le pone su propia posición
                // M98.1: slot ya consumido (scope cerrado con esta hija cancelada) → fallo descartado.
                if let Some(vt) = sh.task_mut(task) {
                    vt.state = TaskState::Failed(msg);
                }
                Self::wake_task_waiters(&mut sh, task);
                // M12.5: un `ScopeEnd` puede estar aparcado sobre OTRA hija pendiente (aparca sobre la
                // primera que encuentra); si no se le despierta, nunca re-escanea y no ve este fallo →
                // deadlock en vez de propagar. Despertar a TODOS los Join-waiters es seguro: re-ejecutan
                // su opcode (ip rebobinado) y se re-aparcan si su tarea sigue pendiente (despertar
                // espurio, como en select). Solo pasa al fallar una tarea (raro).
                Self::wake_all_join_waiters(&mut sh);
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
    ///
    /// M56.4 (timeouts): una fibra aparcada puede llevar un `deadline` (`net.set_read_timeout`). El
    /// poller espera como mucho hasta el más próximo; al vencer, se **marca el handle**
    /// (`mark_read_timeout`) y se despierta la fibra: su lectura re-ejecutada consume la marca y
    /// devuelve el error de timeout. Sin deadlines, la espera sigue siendo infinita (idéntico a M17).
    pub(super) fn io_wait(shared: &mut Shared) {
        loop {
            // 0) Expira los deadlines vencidos y despierta sus fibras. Una espera de LECTURA
            //    (handle >= 0) se marca (su lectura re-ejecutada devuelve el timeout); un SLEEP
            //    (M57.2: fd/handle = -1, sin re-ejecución) simplemente continúa tras dormir.
            //    Si expiró alguna, ya hay una fibra lista → volver.
            let now = std::time::Instant::now();
            let mut expired: Vec<IoParked> = Vec::new();
            let mut i = 0;
            while i < shared.io_parked.len() {
                if shared.io_parked[i].deadline.is_some_and(|d| d <= now) {
                    expired.push(shared.io_parked.remove(i));
                } else {
                    i += 1;
                }
            }
            if !expired.is_empty() {
                for p in expired {
                    if p.handle >= 0 {
                        crate::builtins::mark_read_timeout(p.handle);
                    }
                    shared.ready.push_back(p.fiber);
                }
                return;
            }

            // 1) Espera del poller, acotada por el deadline más próximo (o infinita si no hay).
            let timeout_ms: i32 = match shared.io_parked.iter().filter_map(|p| p.deadline).min() {
                // +1: redondeo hacia arriba para no despertar un pelo antes del deadline (y girar).
                Some(d) => d.saturating_duration_since(now).as_millis().min(i32::MAX as u128 - 1) as i32 + 1,
                None => -1,
            };
            // Cada fibra espera **lectura** (pending_write None) o **escritura** (Some) de su
            // socket. Las durmientes (fd < 0) no entran al poller: solo cuenta su deadline.
            let mut read_fds: Vec<i32> = shared.io_parked.iter().filter(|p| p.fd >= 0 && p.pending_write.is_none()).map(|p| p.fd).collect();
            // M88.1: el self-pipe de señales siempre en el conjunto de lectura.
            if shared.signal_chan.is_some() {
                read_fds.push(shared.signal_fd);
            }
            let write_fds: Vec<i32> = shared.io_parked.iter().filter(|p| p.fd >= 0 && p.pending_write.is_some()).map(|p| p.fd).collect();
            // Solo durmientes (sin fds): el poller con listas vacías retorna al instante (no honra
            // el timeout) → duerme el hilo hasta el deadline más próximo y expira en la vuelta.
            // (Un solo worker llega aquí — `running == 0` — así que dormir el hilo es correcto.)
            if read_fds.is_empty() && write_fds.is_empty() {
                crate::builtins::sleep_millis(timeout_ms.max(0) as i64);
                continue;
            }
            // (con solo el fd de señales y sin deadlines, el poller espera indefinido: correcto —
            // el programa está aparcado esperando al exterior.)
            if let crate::poll::PollResult::Ready(ready) = crate::poll::wait(&read_fds, &write_fds, timeout_ms) {
                if !ready.is_empty() {
                    // M88.1: ¿despertó el self-pipe de señales? Drénalo y entrega al canal
                    // (readya receptores); el pipe no está en io_parked, así que no entra
                    // al barrido de abajo.
                    if shared.signal_chan.is_some() && ready.contains(&shared.signal_fd) {
                        Self::deliver_signals(shared);
                    }
                    // Saca las fibras cuyo socket quedó listo; las demás siguen aparcadas.
                    let mut woken: Vec<IoParked> = Vec::new();
                    let mut i = 0;
                    while i < shared.io_parked.len() {
                        if shared.io_parked[i].fd >= 0 && ready.contains(&shared.io_parked[i].fd) {
                            woken.push(shared.io_parked.remove(i));
                        } else {
                            i += 1;
                        }
                    }
                    Self::wake_parked(shared, woken);
                    return;
                }
                // Despertar vacío CON deadlines pendientes: fue el timeout del poller (o EINTR) →
                // la próxima vuelta del bucle expira los vencidos. Sin deadlines: EINTR → respaldo.
                if timeout_ms >= 0 {
                    continue;
                }
            }
            // Respaldo (sin poller, o EINTR sin deadlines): busy-poll cooperativo de M15.5 —
            // despierta solo las fibras CON fd (retry); las durmientes esperan su deadline (las
            // expira el paso 0 en vueltas siguientes). Nota: sin poller los deadlines de LECTURA
            // no vencen (cada re-aparcado los renueva); macOS/Linux no caen aquí.
            crate::builtins::sleep_millis(1);
            let mut woken: Vec<IoParked> = Vec::new();
            let mut i = 0;
            while i < shared.io_parked.len() {
                if shared.io_parked[i].fd >= 0 {
                    woken.push(shared.io_parked.remove(i));
                } else {
                    i += 1;
                }
            }
            Self::wake_parked(shared, woken);
            return;
        }
    }

    /// Pone listas las fibras despertadas: las de lectura re-ejecutan su opcode (re-pushearon su handle);
    /// las de escritura terminan lo que faltaba (`finish_parked_write`).
    pub(super) fn wake_parked(shared: &mut Shared, woken: Vec<IoParked>) {
        for p in woken {
            match p.pending_write {
                None => shared.ready.push_back(p.fiber),
                Some(pw) => Self::finish_parked_write(shared, p.fd, p.fiber, pw),
            }
        }
    }

    /// Despierta a todas las fibras aparcadas en `join` sobre `task` (M12.3): al re-ejecutar su `Join`/
    /// `ScopeEnd` verán la tarea ya terminada (Done/Failed). No empuja nada (el opcode rebobinó su `ip`).
    pub(super) fn wake_task_waiters(shared: &mut Shared, task: Handle) {
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

    /// Despierta a TODAS las fibras aparcadas en `join`/`ScopeEnd`, sobre cualquier tarea (M12.5): se usa
    /// al FALLAR una tarea, porque un `ScopeEnd` aparcado sobre una hermana pendiente también debe
    /// observar el fallo (re-escanea al re-ejecutarse; las que no les toque se re-aparcan).
    pub(super) fn wake_all_join_waiters(shared: &mut Shared) {
        let mut i = 0;
        while i < shared.parked.len() {
            if matches!(shared.parked[i].waiting, Waiting::Join) {
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
    pub(super) fn wake_select_waiters(shared: &mut Shared, chan: usize) {
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
    pub(super) fn cancel_task(shared: &mut Shared, task: usize) {
        match shared.task_mut(task).map(|vt| &mut vt.state) {
            Some(estado @ TaskState::Pending) => {
                *estado = TaskState::Failed("task cancelled (a sibling failed)".to_string());
            }
            _ => return, // ya terminó (Done/Failed) o fue consumida (M98.1) → nada que cancelar
        }
        // Un joiner de la tarea cancelada (aparcado en `join` sobre ella) debe despertar y observar el
        // `Failed` — si es hija del mismo scope ya la retira el bucle de cancelación, pero un joiner
        // externo (handle cruzado por canal) quedaría aparcado para siempre.
        Self::wake_task_waiters(shared, task);
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
    /// M88.1: despierta un receptor con un valor PRIMITIVO (una señal, int inline) — no
    /// hay fibra emisora de la que transferir heap; el `[T]` se aloja directo en el
    /// heap del receptor.
    pub(super) fn wake_recv_primitive(shared: &mut Shared, mut fiber: Fiber, v: HeapValue) {
        let arr = fiber.heap.allocate(Obj::Array(vec![v]));
        fiber.stack.push(HeapValue::Obj(arr));
        shared.ready.push_back(fiber);
    }

    /// M88.1: drena el self-pipe y entrega cada señal al canal de `signals()`: receptor
    /// bloqueado → directo (FIFO); si no, a la cola (+ despierta a los `select` que lo
    /// esperan). Se llama en cada conmutación de fibra (bandera atómica barata) y desde
    /// `io_wait` cuando el fd del pipe despierta al poller.
    pub(super) fn deliver_signals(shared: &mut Shared) {
        let Some(chan) = shared.signal_chan else { return };
        let fd = shared.signal_fd;
        while let Some(sign) = crate::builtins::signals_read_one(fd) {
            let v = HeapValue::Int(sign as i64);
            if let Some(pos) = shared
                .parked
                .iter()
                .position(|p| p.on == chan && matches!(p.waiting, Waiting::Recv))
            {
                let parked = shared.parked.remove(pos);
                Self::wake_recv_primitive(shared, parked.fiber, v);
            } else if let Some(ch) = shared.chan_mut(chan) {
                // M98.3: acceso tolerante (el canal de señales nunca se cierra → siempre vivo).
                ch.queue.push_back(v);
                Self::wake_select_waiters(shared, chan);
            }
        }
    }

    pub(super) fn wake_recv(cur: &Fiber, shared: &mut Shared, mut fiber: Fiber, values: Vec<HeapValue>) {
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
    pub(super) fn wake_sender(shared: &mut Shared, mut fiber: Fiber) {
        fiber.stack.push(HeapValue::Unit);
        shared.ready.push_back(fiber);
    }

    /// Tras un `recv` que liberó un hueco: si hay un **emisor bloqueado** en `chan` (cola llena, M12.2),
    /// mete su valor pendiente en la cola (ahora hay sitio) y lo despierta. FIFO → el primero que se
    /// bloqueó despierta antes.
    pub(super) fn wake_blocked_sender(shared: &mut Shared, chan: usize) {
        if let Some(pos) = shared.parked.iter().position(
            |p| p.on == chan && matches!(p.waiting, Waiting::Send(_)))
        {
            let parked = shared.parked.remove(pos);
            let sv = match parked.waiting {
                Waiting::Send(sv) => sv,
                _ => unreachable!(),
            };
            // M38.1b-2: el valor del emisor (heap de su fibra) entra a la cola → al heap del canal.
            // M98.3: solo hay emisores bloqueados en canales VIVOS y abiertos (close con emisor
            // bloqueado es error; un canal liberado estaba cerrado) → el acceso no puede ser stale.
            let ch = shared.chan_mut(chan).expect("blocked senders imply a live open channel");
            let mut ch_heap = std::mem::take(&mut ch.heap);
            let sv2 = transfer_value(&parked.fiber.heap, &mut ch_heap, &sv, &mut HashMap::new());
            let ch = shared.chan_mut(chan).expect("blocked senders imply a live open channel");
            ch.heap = ch_heap;
            ch.queue.push_back(sv2);
            Self::wake_sender(shared, parked.fiber);
        }
    }
}
