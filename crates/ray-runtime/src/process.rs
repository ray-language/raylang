//! Ejecución de procesos del SO (M100, IDEAS §53.8) — el primitivo `run` compartido.
//!
//! Vive en `ray-runtime` (feature `process`, sin dependencias externas) porque lo consumen DOS
//! binarios: el `ray` de la VM/intérprete (vía el reexport de `builtins.rs`) y el binario
//! TRANSPILADO a Rust (`__ray_run` en el preámbulo generado). El mismo código en ambos = paridad
//! byte-idéntica por construcción, como crypto/tls/sqlite (docs/transpilador-nativo.md §4).

// Contrato completo en `IDEAS.md` §53.8. Lo que este módulo implementa, y POR QUÉ (cada punto
// esquiva un error documentado de otro lenguaje):
//
// - **Sin shell**: `argv` tipado. Una tubería se escribe `run("sh", ["-c", …])`, visible en el código.
// - **stdin = /dev/null** salvo que se pase `stdin`, que se escribe y se CIERRA. Heredar el stdin
//   del proceso (un servidor) cuelga al hijo o le da datos que no le tocan.
// - **Los dos pipes se drenan CONCURRENTEMENTE** con `poll(2)`: el deadlock clásico ("wait antes de
//   leer" con un pipe lleno — Go `cmd.Wait`, Python `Popen.wait`) es imposible por construcción.
// - **Tope de captura** con `truncated`: un `Vec` sin límite es vía de OOM en un servidor. Truncar y
//   DECIRLO, en vez de matar al hijo con un error confuso (el `maxBuffer` de Node).
// - **Timeout NO es error**: devuelve el `Output` PARCIAL con `timed_out`. La escalera de apagado
//   cierra stdin → `SIGTERM` al GRUPO → margen → `SIGKILL` al GRUPO: los nietos de un `sh -c "a | b"`
//   mueren también (Go, vía context, mata solo al hijo directo).
// - **El grupo se crea con `Command::process_group(0)` de std, SIN `pre_exec`**: entre `fork` y
//   `exec` solo es legal código async-signal-safe, y este proceso tiene ~14 hilos y mimalloc (un
//   lock del asignador tomado por otro hilo colgaría al hijo para siempre). Sin `pre_exec`, std usa
//   su camino `posix_spawn`.
// - **Siempre se cosecha** (raylang no tiene destructores): también tras el timeout.
// - `bytes` en todo el borde: decodificar es decisión del llamador.

/// Opciones de una ejecución (las pone `std/process` desde el builder; el primitivo no las inventa).
pub struct RunOpts {
    pub dir: Option<String>,
    /// Pares (clave, valor) a AÑADIR/pisar sobre el entorno heredado.
    pub env: Vec<(String, String)>,
    /// ¿Vaciar el entorno heredado antes de aplicar `env`?
    pub env_clear: bool,
    /// `Some(data)` = escribir eso en el stdin del hijo y cerrarlo; `None` = `/dev/null`.
    pub stdin: Option<Vec<u8>>,
    /// Presupuesto total en ms (`<= 0` = sin plazo).
    pub timeout_ms: i64,
    /// Tope de captura por flujo, en octetos.
    pub max_output: i64,
    /// `dup2` de stderr al MISMO pipe que stdout → orden REAL del kernel (fusionarlos en userspace
    /// da un orden inventado: los buffers de los dos pipes son independientes).
    pub merge_output: bool,
}

/// El resultado de una ejecución que SÍ llegó a lanzarse (salir con código ≠ 0 no es un error).
pub struct RunOutput {
    /// `Ok(code)` si terminó normalmente; `Err(sig)` si lo mató una señal (nunca `128+sig`).
    pub exit: Result<i32, i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub truncated: bool,
}

/// Lanza `program` con `args` y devuelve su salida. `Err` = **no se pudo lanzar** (ENOENT/EACCES/
/// dir inválido); todo lo demás es `Ok`, incluido un hijo que falló o murió por señal.
///
/// Esta es la versión BLOQUEANTE (intérprete, el oráculo secuencial). VM y nativo aparcan la fibra
/// (fases siguientes de M100); la semántica observable —y por tanto el golden— es la misma.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn run(program: &str, args: &[String], opts: &RunOpts) -> Result<RunOutput, String> {
    use std::io::{Read, Write};
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(d) = &opts.dir {
        cmd.current_dir(d);
    }
    if opts.env_clear {
        cmd.env_clear();
    }
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    // stdin: /dev/null por defecto (jamás heredado); con datos, un pipe que se escribe y se cierra.
    cmd.stdin(if opts.stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    // stdout/stderr: pipes propios. Con `merge_output`, los DOS fds del hijo van al MISMO pipe (un
    // `dup` del extremo de escritura) → el entrelazado es el REAL del kernel; fusionar en userspace
    // dos pipes independientes inventa un orden. `std::io::pipe` pone CLOEXEC de forma atómica.
    let mut merged = None;
    if opts.merge_output {
        let (r, w) = std::io::pipe().map_err(|e| format!("{program}: {e}"))?;
        let w2 = w.try_clone().map_err(|e| format!("{program}: {e}"))?;
        cmd.stdout(w);
        cmd.stderr(w2);
        merged = Some(std::fs::File::from(std::os::fd::OwnedFd::from(r)));
    } else {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }
    // Grupo propio: la escalera del timeout mata al GRUPO, no solo al hijo directo.
    cmd.process_group(0);

    let mut child = cmd.spawn().map_err(|e| format!("{program}: {e}"))?;
    // `Command` RETIENE sus `Stdio` (permite re-spawn): con merge, mantendría vivos los extremos de
    // escritura y el EOF del pipe fusionado no llegaría jamás. Soltarlo cierra nuestras copias.
    drop(cmd);
    let pid = child.id() as i32;
    let deadline = if opts.timeout_ms > 0 {
        Some(std::time::Instant::now() + std::time::Duration::from_millis(opts.timeout_ms as u64))
    } else {
        None
    };

    // El stdin se escribe ENTERO antes de drenar. Es correcto para el contrato de v1 (`stdin(bytes)`
    // ya está en memoria) mientras quepa en el buffer del pipe; si el hijo no lee y el pipe se
    // llena, el write bloquea — el caso de "alimentar megabytes a un hijo que no consume" es de la
    // v2 (streaming), donde el canal acotado es la contrapresión. Un `Err` aquí NO aborta: el hijo
    // ya corre y hay que cosecharlo, así que se ignora (el hijo verá EOF).
    if let (Some(data), Some(mut si)) = (&opts.stdin, child.stdin.take()) {
        let _ = si.write_all(data);
    } // el drop de `si` cierra el pipe → EOF para el hijo

    // Ambos flujos como `File`: la misma lógica de drenaje sirve para stdout, stderr y el pipe
    // fusionado (con merge, `child.stderr` es None y todo llega por `merged`).
    let mut out_pipe = match merged {
        Some(f) => Some(f),
        None => child.stdout.take().map(|p| std::fs::File::from(std::os::fd::OwnedFd::from(p))),
    };
    let mut err_pipe = child.stderr.take().map(|p| std::fs::File::from(std::os::fd::OwnedFd::from(p)));
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let mut truncated = false;
    let cap = opts.max_output.max(0) as usize;

    // Drenaje CONCURRENTE de ambos pipes con poll(2) (nunca uno y luego el otro: el hijo podría
    // llenar el que no leemos y bloquearse para siempre). Los pipes van no-bloqueantes para que un
    // POLLIN espurio no nos clave.
    let (ofd, efd) = (
        out_pipe.as_ref().map_or(-1, std::os::fd::AsRawFd::as_raw_fd),
        err_pipe.as_ref().map_or(-1, std::os::fd::AsRawFd::as_raw_fd),
    );
    for fd in [ofd, efd] {
        if fd >= 0 {
            // SAFETY: fcntl variádica (ver el self-pipe de M88.1: con aridad fija es UB en arm64);
            // el fd es de un pipe propio recién creado por std.
            unsafe {
                let fl = fcntl_get(fd);
                let _ = fcntl(fd, F_SETFL_P, fl | O_NONBLOCK_P);
            }
        }
    }
    let mut timed_out = false;
    let mut buf = [0u8; 65536];
    let (mut o_open, mut e_open) = (ofd >= 0, efd >= 0);
    while o_open || e_open {
        // Plazo restante para el poll: al vencer, se rompe el drenaje y actúa la escalera.
        let wait_ms: i32 = match deadline {
            None => -1,
            Some(d) => {
                let rem = d.saturating_duration_since(std::time::Instant::now()).as_millis();
                if rem == 0 {
                    timed_out = true;
                    break;
                }
                rem.min(i32::MAX as u128) as i32
            }
        };
        let mut fds = [
            PollFd { fd: if o_open { ofd } else { -1 }, events: POLLIN, revents: 0 },
            PollFd { fd: if e_open { efd } else { -1 }, events: POLLIN, revents: 0 },
        ];
        // SAFETY: `poll` sobre un array local de 2 entradas, vivo durante la llamada.
        let n = unsafe { poll(fds.as_mut_ptr(), 2, wait_ms) };
        if n < 0 {
            // SAFETY: errno del hilo actual.
            if unsafe { *errno_ptr() } == EINTR {
                continue;
            }
            break; // error no transitorio: se corta el drenaje y se cosecha igual
        }
        if n == 0 {
            timed_out = true;
            break;
        }
        for (i, slot) in [(0usize, true), (1usize, false)] {
            if fds[i].revents == 0 {
                continue;
            }
            // Una lectura del flujo elegido. Devuelve (sigue_abierto, se_truncó) para no mantener
            // dos préstamos mutables vivos a la vez (out_pipe/err_pipe son campos distintos).
            let mut drain = |pipe: &mut Option<std::fs::File>, sink: &mut Vec<u8>| -> (bool, bool) {
                let Some(p) = pipe.as_mut() else { return (false, false) };
                match p.read(&mut buf) {
                    Ok(0) => (false, false), // EOF: ese extremo cerró
                    Ok(k) => {
                        // Tope por flujo: se trunca y se DICE (nunca se mata al hijo por pasarse).
                        let room = cap.saturating_sub(sink.len());
                        if room == 0 {
                            return (true, true);
                        }
                        let take = k.min(room);
                        sink.extend_from_slice(&buf[..take]);
                        (true, take < k)
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (true, false),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => (true, false),
                    Err(_) => (false, false),
                }
            };
            let (open, trunc) = if slot {
                drain(&mut out_pipe, &mut stdout)
            } else {
                drain(&mut err_pipe, &mut stderr)
            };
            truncated |= trunc;
            if slot { o_open = open; } else { e_open = open; }
        }
    }
    // Cierra nuestros extremos: si el hijo sigue vivo escribiendo, verá EPIPE.
    drop(out_pipe);
    drop(err_pipe);

    // Escalera de apagado (solo si venció el plazo): SIGTERM al GRUPO → margen → SIGKILL al GRUPO.
    // El grupo es -pid porque `process_group(0)` hizo al hijo líder de su propio grupo.
    if timed_out {
        // SAFETY: `kill` a un grupo propio (creado por nosotros con process_group(0)).
        unsafe { kill(-pid, SIGTERM) };
        let grace = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => {
                    if std::time::Instant::now() >= grace {
                        // SAFETY: idem; SIGKILL no se puede ignorar.
                        unsafe { kill(-pid, SIGKILL) };
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }

    // SIEMPRE se cosecha (sin destructores, un `wait` omitido deja un zombi para toda la vida del
    // proceso). Tras la escalera, este wait retorna de inmediato.
    let status = child.wait().map_err(|e| format!("{program}: {e}"))?;
    let exit = match status.code() {
        Some(c) => Ok(c),
        None => Err(status.signal().unwrap_or(0)),
    };
    Ok(RunOutput { exit, stdout, stderr, timed_out, truncated })
}

/// Sin unix (Windows, wasm): un `Err` honesto, como `packages/tz` en plataformas sin TZif.
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
pub fn run(program: &str, _args: &[String], _opts: &RunOpts) -> Result<RunOutput, String> {
    Err(format!("{program}: running OS processes is not supported on this platform"))
}

/// Ejecuta y APLANA el resultado al arreglo etiquetado del builtin `__run`. Los dos motores envuelven
/// estos octetos en su tipo de valor propio — una sola codificación aquí = cero divergencia:
/// `[b"ok", b"code"|b"signal", valor decimal, b"1"|b"0" (timed_out), b"1"|b"0" (truncated), stdout,
/// stderr]` o `[b"err", msg]`.
pub fn run_encoded(program: &str, args: &[String], opts: &RunOpts) -> Vec<Vec<u8>> {
    match run(program, args, opts) {
        Ok(o) => {
            let (kind, val) = match o.exit {
                Ok(code) => ("code", code),
                Err(sig) => ("signal", sig),
            };
            vec![
                b"ok".to_vec(),
                kind.as_bytes().to_vec(),
                val.to_string().into_bytes(),
                vec![if o.timed_out { b'1' } else { b'0' }],
                vec![if o.truncated { b'1' } else { b'0' }],
                o.stdout,
                o.stderr,
            ]
        }
        Err(e) => vec![b"err".to_vec(), e.into_bytes()],
    }
}

/// Reconstruye `RunOpts` desde los argumentos APLANADOS del builtin `__run` (el decodificador único
/// para los dos motores; el orden es el de la firma). `dir == ""` = heredado; `env` llega como pares
/// clave/valor consecutivos; `stdin` solo cuenta si `has_stdin`.
#[allow(clippy::too_many_arguments)] // es el borde plano del builtin, no una API para humanos
pub fn run_opts_from_flat(
    dir: &str,
    env_flat: Vec<String>,
    env_clear: bool,
    stdin: &[u8],
    has_stdin: bool,
    timeout_ms: i64,
    max_output: i64,
    merge_output: bool,
) -> RunOpts {
    RunOpts {
        dir: if dir.is_empty() { None } else { Some(dir.to_string()) },
        env: env_flat.chunks_exact(2).map(|p| (p[0].clone(), p[1].clone())).collect(),
        env_clear,
        stdin: if has_stdin { Some(stdin.to_vec()) } else { None },
        timeout_ms,
        max_output,
        merge_output,
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
const POLLIN: i16 = 0x0001;
#[cfg(all(unix, not(target_arch = "wasm32")))]
const EINTR: i32 = 4;
#[cfg(all(unix, not(target_arch = "wasm32")))]
const SIGTERM: i32 = 15;
#[cfg(all(unix, not(target_arch = "wasm32")))]
const SIGKILL: i32 = 9;
#[cfg(all(unix, not(target_arch = "wasm32")))]
const F_GETFL: i32 = 3;
#[cfg(all(unix, not(target_arch = "wasm32")))]
const F_SETFL_P: i32 = 4;
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
const O_NONBLOCK_P: i32 = 0x0004;
#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
const O_NONBLOCK_P: i32 = 0o4000;

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
type Nfds = u64;
#[cfg(all(unix, not(target_os = "linux"), not(target_arch = "wasm32")))]
type Nfds = u32;

#[cfg(all(unix, not(target_arch = "wasm32")))]
unsafe extern "C" {
    fn poll(fds: *mut PollFd, n: Nfds, timeout: i32) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    // Variádica a propósito (lección de arm64: con aridad fija los varargs van mal por la pila).
    #[link_name = "fcntl"]
    fn fcntl_raw(fd: i32, cmd: i32, ...) -> i32;
}

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
unsafe extern "C" {
    #[link_name = "__errno_location"]
    fn errno_ptr() -> *mut i32;
}
#[cfg(all(unix, not(target_os = "linux"), not(target_arch = "wasm32")))]
unsafe extern "C" {
    #[link_name = "__error"]
    fn errno_ptr() -> *mut i32;
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
unsafe fn fcntl_get(fd: i32) -> i32 {
    unsafe { fcntl_raw(fd, F_GETFL) }
}
#[cfg(all(unix, not(target_arch = "wasm32")))]
unsafe fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32 {
    unsafe { fcntl_raw(fd, cmd, arg) }
}

/// M100 fase 1a: los tests del primitivo `run`. Cada uno asevera un invariante del contrato de
/// IDEAS §53.8; los comandos son deterministas (sh/cat/env) para que sirvan de base al golden.
#[cfg(all(test, unix, not(target_arch = "wasm32")))]
mod process_tests {
    use super::{run, RunOpts};

    fn opts() -> RunOpts {
        RunOpts {
            dir: None,
            env: vec![],
            env_clear: false,
            stdin: None,
            timeout_ms: 0,
            max_output: 16 * 1024 * 1024,
            merge_output: false,
        }
    }

    fn sh(script: &str) -> Vec<String> {
        vec!["-c".to_string(), script.to_string()]
    }

    // Salir con código ≠ 0 NO es Err: el Err queda reservado a "no se pudo lanzar".
    #[test]
    fn nonzero_exit_code_is_ok_not_err() {
        let o = run("sh", &sh("echo hi; exit 3"), &opts()).unwrap();
        assert_eq!(o.exit, Ok(3));
        assert_eq!(o.stdout, b"hi\n");
        assert!(o.stderr.is_empty());
        assert!(!o.timed_out && !o.truncated);
    }

    // Muerte por señal = Err(sig) en el enum, jamás el 128+sig aplanado del shell.
    #[test]
    fn death_by_signal_reports_the_signal() {
        let o = run("sh", &sh("kill -TERM $$"), &opts()).unwrap();
        assert_eq!(o.exit, Err(15));
    }

    // ENOENT sí es Err: el hijo nunca llegó a existir.
    #[test]
    fn spawn_failure_is_err() {
        assert!(run("raylang-no-such-binary-m100", &[], &opts()).is_err());
    }

    // stdin se escribe entero y SE CIERRA: `cat` termina por EOF, no se queda colgado.
    #[test]
    fn stdin_is_written_and_closed() {
        let mut o = opts();
        o.stdin = Some(b"a\nb\n".to_vec());
        let out = run("cat", &[], &o).unwrap();
        assert_eq!(out.exit, Ok(0));
        assert_eq!(out.stdout, b"a\nb\n");
    }

    // Sin `.stdin(…)`, el hijo ve /dev/null: EOF inmediato, nunca el stdin del padre.
    #[test]
    fn default_stdin_is_dev_null() {
        let o = run("cat", &[], &opts()).unwrap();
        assert_eq!(o.exit, Ok(0));
        assert!(o.stdout.is_empty());
    }

    // Ambos flujos por ENCIMA del buffer del pipe (64 KB): sin drenaje concurrente esto es el
    // deadlock clásico de Go `cmd.Wait`/Python `Popen.wait`. Aquí es imposible por construcción.
    #[test]
    fn both_streams_drain_concurrently_past_pipe_capacity() {
        let o = run("sh", &sh("yes | head -c 200000; yes | head -c 200000 >&2"), &opts()).unwrap();
        assert_eq!(o.stdout.len(), 200_000);
        assert_eq!(o.stderr.len(), 200_000);
        assert!(!o.truncated);
    }

    // Tope de captura: se trunca al PREFIJO y se DICE; el hijo termina normal (nada de matarlo).
    #[test]
    fn max_output_truncates_and_says_so() {
        let mut o = opts();
        o.max_output = 10;
        let out = run("sh", &sh("yes | head -c 1000"), &o).unwrap();
        assert_eq!(out.exit, Ok(0));
        assert_eq!(out.stdout.len(), 10);
        assert!(out.truncated);
    }

    // Timeout NO es Err: Output PARCIAL con timed_out, y la escalera mata al GRUPO (el `sleep`
    // en segundo plano es el nieto: si solo muriera el sh directo, este test dejaría huérfanos).
    #[test]
    fn timeout_returns_partial_output_and_kills_the_group() {
        let mut o = opts();
        o.timeout_ms = 200;
        let out = run("sh", &sh("echo partial; sleep 30 & wait"), &o).unwrap();
        assert!(out.timed_out);
        assert_eq!(out.stdout, b"partial\n");
        assert!(out.exit.is_err(), "the ladder ends in a signal, got {:?}", out.exit);
    }

    // dir + env: el hijo ve el directorio pedido y la variable añadida sobre el entorno heredado.
    #[test]
    fn dir_and_env_are_applied() {
        let mut o = opts();
        o.dir = Some("/".to_string());
        o.env = vec![("RAY_TEST_VAR".to_string(), "v1".to_string())];
        let out = run("sh", &sh("printf '%s %s' \"$RAY_TEST_VAR\" \"$PWD\""), &o).unwrap();
        assert_eq!(out.stdout, b"v1 /");
    }

    // env_clear: el entorno heredado se vacía; `env` imprime EXACTAMENTE lo que pusimos.
    #[test]
    fn env_clear_empties_the_inherited_environment() {
        let mut o = opts();
        o.env_clear = true;
        o.env = vec![("ONLY_VAR".to_string(), "x".to_string())];
        let out = run("env", &[], &o).unwrap();
        assert_eq!(out.stdout, b"ONLY_VAR=x\n");
    }

    // merge_output: un solo pipe (dup) → el entrelazado stdout/stderr es el REAL del kernel.
    #[test]
    fn merge_output_preserves_kernel_order() {
        let mut o = opts();
        o.merge_output = true;
        let out = run("sh", &sh("echo one; echo two >&2; echo three"), &o).unwrap();
        assert_eq!(out.exit, Ok(0));
        assert_eq!(out.stdout, b"one\ntwo\nthree\n");
        assert!(out.stderr.is_empty());
    }
}
