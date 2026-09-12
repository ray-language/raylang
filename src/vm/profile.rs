//! M240 — `ray profile`: perfilador por función de la VM, instrumentado en el camino de
//! llamada (`Call`/`TailCall`/`CallValue`/`Return`). Apagado no cuesta nada (un `AtomicBool`
//! relajado por llamada); encendido paga dos lecturas de reloj por llamada. Mide tiempo
//! INCLUSIVO (la función y todo lo que llama) y PROPIO (sin los hijos); en recursión, el
//! inclusivo se cuenta una vez por activación EXTERNA (no se acumula por nivel). Los builtins
//! no son funciones: su coste cae en el propio de quien los llama. Las fibras suman en una
//! tabla común (un mutex por retorno, solo con el perfil activo).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Configuración del informe (la CLI la fija con `ray profile`).
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub json: bool,
    pub out: Option<String>,
    pub top: usize,
}

struct State {
    names: Vec<String>,
    entries: Vec<Entry>,
    started: Option<Instant>,
    fibers: u64,
    config: Config,
    reported: bool,
}

#[derive(Clone, Copy, Default)]
struct Entry {
    calls: u64,
    self_ns: u64,
    inclusive_ns: u64,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { names: Vec::new(), entries: Vec::new(), started: None, fibers: 0, config: Config::default(), reported: false }))
}

/// Enciende el perfil (la CLI, antes de ejecutar). `config` decide el formato del informe.
pub fn enable(config: Config) {
    let mut s = state().lock().unwrap();
    s.config = config;
    s.started = Some(Instant::now());
    s.reported = false;
    ENABLED.store(true, Ordering::Relaxed);
}

#[inline(always)]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Registra los nombres del programa (una vez por proceso; los workers del scheduler comparten
/// el mismo programa).
pub fn register_program(names: impl Iterator<Item = String>) {
    let mut s = state().lock().unwrap();
    if s.names.is_empty() {
        s.names = names.collect();
        s.entries = vec![Entry::default(); s.names.len()];
    }
}

pub fn note_fiber() {
    state().lock().unwrap().fibers += 1;
}

/// Un marco perfilado, apilado en paralelo a `Fiber.frames`.
#[derive(Clone, Copy)]
pub struct ProfFrame {
    pub func: usize,
    pub start: Instant,
    /// Tiempo inclusivo de los hijos ya cerrados (se resta para el propio).
    pub child_ns: u64,
    /// ¿Es la activación externa de su función en esta fibra (la que cuenta el inclusivo)?
    pub outermost: bool,
}

/// La pila de marcos perfilados de una fibra.
#[derive(Default)]
pub struct FiberProfile {
    pub frames: Vec<ProfFrame>,
    /// Activaciones vivas por función (para el inclusivo no acumulativo en recursión).
    active: Vec<u32>,
}

impl FiberProfile {
    /// Entra en `func`; `depth` es `Fiber.frames.len()` tras empujar el marco real (mantiene
    /// las dos pilas alineadas aunque un desenrollado por error se haya llevado marcos).
    pub fn enter(&mut self, func: usize, depth: usize) {
        if self.frames.len() >= depth {
            self.frames.truncate(depth - 1);
        }
        if self.active.len() <= func {
            self.active.resize(func + 1, 0);
        }
        let outermost = self.active[func] == 0;
        self.active[func] += 1;
        self.frames.push(ProfFrame { func, start: Instant::now(), child_ns: 0, outermost });
    }

    /// Sale del marco superior; `depth` es `Fiber.frames.len()` ANTES de sacar el marco real.
    pub fn leave(&mut self, depth: usize) {
        if self.frames.len() > depth {
            self.frames.truncate(depth);
        }
        let Some(f) = self.frames.pop() else { return };
        let elapsed = f.start.elapsed().as_nanos() as u64;
        if let Some(a) = self.active.get_mut(f.func) {
            *a = a.saturating_sub(1);
        }
        if let Some(parent) = self.frames.last_mut() {
            parent.child_ns += elapsed;
        }
        let mut s = state().lock().unwrap();
        if f.func >= s.entries.len() {
            s.entries.resize(f.func + 1, Entry::default());
        }
        let e = &mut s.entries[f.func];
        e.calls += 1;
        e.self_ns += elapsed.saturating_sub(f.child_ns);
        if f.outermost {
            e.inclusive_ns += elapsed;
        }
    }
}

/// Emite el informe (una vez) si el perfil está activo: a `out` o a stderr.
pub fn report_if_enabled() {
    if !enabled() {
        return;
    }
    let mut s = state().lock().unwrap();
    if s.reported {
        return;
    }
    s.reported = true;
    let wall_ns = s.started.map(|t| t.elapsed().as_nanos() as u64).unwrap_or(0);
    let mut rows: Vec<(usize, Entry)> = s.entries.iter().copied().enumerate().filter(|(_, e)| e.calls > 0).collect();
    rows.sort_by(|a, b| b.1.self_ns.cmp(&a.1.self_ns).then(b.1.inclusive_ns.cmp(&a.1.inclusive_ns)));
    let name = |i: usize| s.names.get(i).cloned().unwrap_or_else(|| format!("fn#{i}"));
    let text = if s.config.json {
        let mut out = String::from("{");
        out.push_str(&format!("\"wall_ns\":{wall_ns},\"fibers\":{},\"functions\":[", s.fibers));
        for (k, (i, e)) in rows.iter().enumerate() {
            if k > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"name\":{},\"calls\":{},\"self_ns\":{},\"inclusive_ns\":{}}}",
                json_str(&name(*i)),
                e.calls,
                e.self_ns,
                e.inclusive_ns
            ));
        }
        out.push_str("]}\n");
        out
    } else {
        let top = if s.config.top == 0 { 30 } else { s.config.top };
        let total_self: u64 = rows.iter().map(|(_, e)| e.self_ns).sum();
        let mut out = format!(
            "[profile] {} function(s), {} fiber(s), {:.3} s wall, {:.3} s in raylang functions\n{:>10} {:>6} {:>10} {:>9} {:>9}  function\n",
            rows.len(),
            s.fibers,
            wall_ns as f64 / 1e9,
            total_self as f64 / 1e9,
            "self ms",
            "self%",
            "incl ms",
            "calls",
            "avg µs"
        );
        for (i, e) in rows.iter().take(top) {
            let pct = if total_self > 0 { e.self_ns as f64 * 100.0 / total_self as f64 } else { 0.0 };
            out.push_str(&format!(
                "{:>10.3} {:>5.1}% {:>10.3} {:>9} {:>9.2}  {}\n",
                e.self_ns as f64 / 1e6,
                pct,
                e.inclusive_ns as f64 / 1e6,
                e.calls,
                e.inclusive_ns as f64 / 1e3 / e.calls.max(1) as f64,
                name(*i)
            ));
        }
        if rows.len() > top {
            out.push_str(&format!("  … {} more (--top N)\n", rows.len() - top));
        }
        out
    };
    match &s.config.out {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &text) {
                eprintln!("[profile] could not write {path}: {e}");
            }
        }
        None => eprint!("{text}"),
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursion_counts_inclusive_once_per_outer_activation() {
        let mut p = FiberProfile::default();
        p.enter(0, 1);
        p.enter(0, 2);
        p.enter(0, 3);
        p.leave(3);
        p.leave(2);
        p.leave(1);
        assert_eq!(p.frames.len(), 0);
        assert_eq!(p.active[0], 0);
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    }
}
