//! **SPIKE / arco P2.b — transpile a Rust** (codegen nativo; jul 2026).
//!
//! Emite código Rust para un subconjunto creciente de raylang. El checker garantiza el tipado; aquí
//! solo se baja a Rust. Fases: **escalares** (`int`/`float`/`bool`, aritmética, `if`/`while`/`for`-rango,
//! recursión, `print`) → **strings** (`Rc<str>`, concat, `to_string`, `len`) → datos → control → …
//! Todo nodo fuera del subconjunto → `Err` claro (el transpilador es honesto sobre su alcance).
//!
//! **Modelo de valores** (como el intérprete): escalares *unboxed* (`i64`/`f64`/`bool`), tipos de heap
//! envueltos en `Rc` (strings inmutables → `Rc<str>`; arreglos/structs → `Rc<RefCell<…>>`, futuro). La
//! semántica de VALOR de raylang sobre la de MOVIMIENTO de Rust se resuelve **clonando al leer** los
//! valores de heap (para `Rc` es un bump de refcount, O(1)); los escalares son `Copy`. Un entorno de
//! tipos propio (params explícitos + inferencia mínima de los `let`) decide qué clonar.
//!
//! Semántica del spike: aritmética `int` con **wrapping** (operadores nativos; release sin
//! overflow-checks), NO checked como la VM. Fiel para programas sin desbordamiento.

use crate::ast::{
    BinaryOp, Block, Expr, ExprKind, ForIter, ForPat, Function, MatchArm, Pattern, PatternKind, Program,
    Stmt, StmtKind, Type, UnaryOp,
};
use std::collections::HashMap;
use std::fmt::Write;

mod names;
mod analysis;
mod runtime;
mod types;
mod emit;
mod calls;
use names::*;
use analysis::*;
use runtime::*;
use types::*;
use emit::*;

/// Firma de una función del usuario: params, retorno y sus parámetros de tipo (para inferir las
/// llamadas genéricas por unificación).
struct FnSig {
    params: Vec<Type>,
    ret: Type,
    tparams: Vec<String>,
}

struct Transpiler {
    funcs: HashMap<String, FnSig>,
    /// Pila de ámbitos: nombre de variable → su tipo (para decidir clonado y para la inferencia de `let`).
    scopes: Vec<HashMap<String, Type>>,
    /// Nombres de enum del usuario (para clasificar un `Type::Struct(n)` como struct vs enum).
    enums: std::collections::HashSet<String>,
    /// Campos de cada struct (nombre → tipo), en orden, para inferir el tipo de `p.campo`.
    struct_fields: HashMap<String, Vec<(String, Type)>>,
    /// Parámetros de tipo de cada struct/enum (para sustituir en `Caja<int>`).
    struct_tparams: HashMap<String, Vec<String>>,
    enum_tparams: HashMap<String, Vec<String>>,
    /// Payload de cada variante de enum (`Enum` → `Variante` → tipos), para los bindings de `match`.
    enum_variants: HashMap<String, HashMap<String, Vec<Type>>>,
    /// Contador para nombres de temporales de escrutinio de `match` (evita colisiones al anidar).
    match_temp: usize,
    /// Marcas H21-N5c: función → índices de sus params de tipo fn que cruzan un spawn (se emiten
    /// como genéricos de Rust con bound Send+Sync).
    fn_marks: HashMap<String, std::collections::HashSet<usize>>,
    /// Nombres de los params marcados de la FUNCIÓN EN CURSO (para las capturas de spawn: se clonan,
    /// no se convierten — su bound ya garantiza Send).
    send_fn_params: std::collections::HashSet<String>,
    /// Conversores Send generados bajo demanda (H21-N5a): (tipo concreto, su rust_ty como clave única).
    /// El índice en el Vec es el id de las fns `__to_send_N`/`__from_send_N`; se emiten al final
    /// (worklist: generar el cuerpo de uno puede registrar otros — tipos anidados).
    send_convs: Vec<(Type, String)>,
    /// Constantes de nivel superior (nombre → tipo). Se bajan a funciones `NAME()` (uniforme para
    /// escalares y strings, que no pueden ser `const` en Rust por el `Rc`); una referencia `NAME` → `NAME()`.
    consts: HashMap<String, Type>,
    /// Firma de cada método de trait (nombre → (tipos de args SIN `self`, tipo de retorno)). Para bajar
    /// `dyn Trait`: el struct sintetizado `__dyn_T` lleva un campo-closure por método (`Rc<dyn Fn(..)->R>`).
    trait_method_sigs: HashMap<String, (Vec<Type>, Type)>,
    /// Parámetros de tipo de la función genérica en curso (p. ej. `{T, U}`): un `Struct(n)` con `n` aquí
    /// es un tipo VARIABLE → se emite como el genérico `n` de Rust (no como un struct de usuario).
    tparams: std::collections::HashSet<String>,
    /// ¿El programa usa handles de archivo (`open`/`read_line`/`write`/`close`)? Se activa al emitirlos;
    /// si es cierto, se anexa al final el registro global de handles (espejo del `FileRegistry` de la VM).
    needs_handles: bool,
    /// ¿Usa concurrencia (`spawn`/canales)? Si es cierto, se anexa el runtime de canales MPMC.
    needs_concurrency: bool,
    /// ¿Usa `signals()`? Si es cierto, se anexa el runtime de señales del SO (self-pipe + FFI a libc).
    needs_signals: bool,
    /// ¿Usa `std::time::monotonic`/`std::random::*`? Si es cierto, se anexa el PRNG (SplitMix64) + el
    /// reloj monotónico (necesitan estado global; `now`/`sleep` son inline y no lo activan).
    needs_time_rng: bool,
    /// ¿Usa sockets TCP (`std::net::*`)? Comparte el registro de handles con los archivos y añade los ops
    /// de socket (`std::net::TcpStream`/`TcpListener`).
    needs_net: bool,
    /// ¿Usa cripto de producción (`__sha256`/`__hmac_sha256`/`__ed25519_*`/`__chacha20poly1305_*`/…)? Se
    /// interceptan a `ray_runtime::crypto::*` → el binario nativo llama al MISMO código que la VM (ring).
    /// Activa la feature `crypto` de `ray-runtime` → `build_native` genera un proyecto Cargo (no rustc pelado).
    needs_rt_crypto: bool,
    /// ¿Usa TLS (`__tls_connect`/`__tls_connect_h2`/`__tls_accept`/`__tls_upgrade`)? El binario transpilado
    /// hace I/O TLS **bloqueante** (hilos reales; `ray_runtime::tls::TlsStream` sobre `StreamOwned`). Añade
    /// la variante `Tls` al registro de handles inline y el despacho en `socket_read/write`. Implica
    /// `needs_net` (registro + TcpStream). Activa la feature `tls` de `ray-runtime`.
    needs_rt_tls: bool,
    /// ¿Usa SQLite (`__sqlite_open`/`__sqlite_exec`/`__sqlite_query`)? El binario transpilado guarda cada
    /// conexión (`ray_runtime::sqlite::Conn`) en el registro de handles inline (variante `Sqlite`). I/O
    /// local → se retiene el lock global (como la VM). Activa la feature `sqlite` de `ray-runtime`.
    needs_rt_sqlite: bool,
    /// R5: el programa ejecuta std/regex → feature `regex` de ray-runtime (motor acelerado).
    pub(super) needs_rt_regex: bool,
    /// Subsistemas con-crate EXCLUIDOS por `--without` (crypto/tls/sqlite): sus builtins no se interceptan
    /// (caen en stub que panica) → el binario puede usar la vía rápida `rustc`. Ver `transpile_with`.
    exclude: std::collections::HashSet<String>,
    /// F2 (concurrencia nativa): ¿emitir la concurrencia sobre el scheduler de FIBRAS de
    /// `ray_runtime::fibers` (`--fibers`)? Cambia la forma de spawn/sockets/contexto por-tarea;
    /// con `false` (default) se emite el modelo de hilo-por-tarea de siempre, byte-idéntico.
    pub(super) fibers: bool,
    /// Nombres de `var` locales que van en una **celda** `Rc<RefCell<T>>` (B1): capturadas y mutadas por
    /// una closure. Se leen con `.borrow().clone()` y se escriben con `.borrow_mut()`; la closure captura
    /// un clon del `Rc`. Se pueblan al entrar en cada función/closure (con su `cell_vars`) y se quitan al
    /// salir (set plano; el shadowing de una var-celda es una limitación conocida, raro en la práctica).
    cells: std::collections::HashSet<String>,
}

/// Transpila un programa (ya chequeado) a Rust autocontenido, o un error si usa algo fuera del subconjunto.
/// El resultado de transpilar: el fuente Rust + las **features de `ray-runtime`** que el programa necesita
/// (activadas bajo demanda al interceptar un builtin que envuelve un crate). Vacío → cero deps externas →
/// `build_native` compila con `rustc` pelado (camino rápido); no vacío → genera un proyecto Cargo con
/// `ray-runtime` (esas features) y compila con `cargo`. Ver docs/transpilador-nativo.md §4.5.
pub struct Transpiled {
    pub source: String,
    pub rt_features: Vec<&'static str>,
    /// Funciones cuyo CUERPO cayó fuera del subconjunto → se emitieron como STUB que panica (u omitieron):
    /// `(nombre, motivo)`. El binario COMPILA, pero llamarlas panica. `build_native` lo AVISA al usuario
    /// (antes solo con `RAYLANG_TRANSPILE_DEBUG`) para que el "ok" no oculte una divergencia en runtime (H7).
    pub stubbed: Vec<(String, String)>,
}

/// Transpila sin excluir ningún subsistema (el caso común; lo usan `ray emit-rust` y los tests).
pub fn transpile(prog: &Program) -> Result<Transpiled, String> {
    transpile_with(prog, &[])
}

/// Transpila EXCLUYENDO los subsistemas con-crate dados (`--without crypto,tls,sqlite`): un uso de un
/// subsistema excluido NO se intercepta a `ray_runtime::*` → su función cae en un stub que panica, y el
/// binario compila por la vía rápida (`rustc` pelada) si no queda otro subsistema con-crate. Escape hatch
/// para builds herméticos/cross-compile/policy (docs/transpilador-nativo.md §3.3).
pub fn transpile_with(prog: &Program, exclude: &[String]) -> Result<Transpiled, String> {
    transpile_with_opts(prog, exclude, false)
}

/// Como [`transpile_with`], con opciones. `fast = true` (flag `--fast` de `ray build --native`) emite
/// la aritmética de `int` ENVOLVENTE (wrapping) en vez de checked: renuncia a la paridad de overflow
/// con la VM a cambio del último tramo de rendimiento (medido: ~2× en un bucle de puro int, ~20 % en
/// código de llamadas calientes tipo fib, ~0 en código idiomático con arrays/strings). Div/mod por
/// cero SIGUEN siendo error (Rust los chequea igual; no cuestan nada). Solo cambia el PREÁMBULO
/// (los cuerpos de `__ray_add`/…); los sitios de llamada son idénticos.
pub fn transpile_with_opts(prog: &Program, exclude: &[String], fast: bool) -> Result<Transpiled, String> {
    transpile_full(prog, exclude, fast, false)
}

/// Como [`transpile_with_opts`], con el modo **fibras** (F2 del arco de concurrencia nativa,
/// `ray build --native --fibers`, EXPERIMENTAL): la concurrencia del binario corre sobre el
/// scheduler M:N de `ray_runtime::fibers` (corrutinas corosensei + reactor kqueue/epoll) en vez de
/// hilo-de-SO-por-tarea — los sockets se emiten no-bloqueantes y aparcan la FIBRA, `__ray_spawn`
/// crea fibras, y el contexto por-tarea (cancelación/scopes/try) viaja con la fibra. Ver
/// docs/diseno-concurrencia-nativa.md §5.
pub fn transpile_full(prog: &Program, exclude: &[String], fast: bool, fibers: bool) -> Result<Transpiled, String> {
    // Índice de firmas de funciones NO genéricas y NO sintéticas (para inferir tipos de llamada).
    let mut funcs = HashMap::new();
    for f in &prog.functions {
        if skip_fn_def(f) {
            continue;
        }
        // Se NORMALIZAN los tipos de la firma (Struct("Map"/"Channel"/"Task"/"Option"/"Result") → su
        // variante propia): el parser deja `Map<K,V>` como `Struct("Map", …)`, y `type_of` de una llamada
        // devuelve el retorno guardado → sin normalizar, `get(mkmap(), k)` veía `Struct("Map")` y fallaba.
        funcs.insert(f.name.clone(), FnSig {
            params: f.params.iter().map(|p| normalize_type(&p.ty)).collect(),
            ret: normalize_type(&f.return_type),
            tparams: f.type_params.clone(),
        });
    }
    // Funciones externas (FFI, M41): se registran como funciones ordinarias → una llamada `sqrt(2.0)`
    // resuelve al WRAPPER emitido (que marshala y llama al símbolo C por `extern "C"`).
    for e in &prog.externs {
        funcs.insert(e.name.clone(), FnSig {
            params: e.params.iter().map(|p| normalize_type(&p.ty)).collect(),
            ret: normalize_type(&e.return_type),
            tparams: Vec::new(),
        });
    }
    // Enums de USUARIO (incl. genéricos). Option/Result se excluyen: son los nativos de Rust, no se emiten.
    let enums: std::collections::HashSet<String> =
        prog.enums.iter().filter(|e| e.name != "Option" && e.name != "Result").map(|e| e.name.clone()).collect();
    let struct_fields = prog.structs.iter().map(|s| (s.name.clone(), s.fields.clone())).collect();
    let struct_tparams = prog.structs.iter().map(|s| (s.name.clone(), s.type_params.clone())).collect();
    let enum_variants = prog
        .enums
        .iter()
        .map(|e| {
            (e.name.clone(), e.variants.iter().map(|v| (v.name.clone(), v.payload.clone())).collect())
        })
        .collect();
    let enum_tparams = prog.enums.iter().map(|e| (e.name.clone(), e.type_params.clone())).collect();
    let consts = prog.consts.iter().map(|c| (c.name.clone(), c.ty.clone())).collect();
    // Firmas de los métodos de trait (self excluido) para bajar `dyn Trait`.
    let mut trait_method_sigs = HashMap::new();
    for tr in &prog.traits {
        for m in &tr.methods {
            let args: Vec<Type> = m.params.iter().skip(1).map(|p| p.ty.clone()).collect(); // skip self
            trait_method_sigs.insert(m.name.clone(), (args, m.return_type.clone()));
        }
    }
    let marks = spawn_fn_param_marks(prog);
    let mut t = Transpiler {
        funcs,
        scopes: Vec::new(),
        enums,
        struct_fields,
        struct_tparams,
        enum_variants,
        enum_tparams,
        match_temp: 0,
        fn_marks: marks,
        send_fn_params: std::collections::HashSet::new(),
        send_convs: Vec::new(),
        consts,
        trait_method_sigs,
        tparams: std::collections::HashSet::new(),
        needs_handles: false,
        needs_concurrency: false,
        needs_signals: false,
        needs_time_rng: false,
        needs_net: false,
        needs_rt_crypto: false,
        needs_rt_tls: false,
        needs_rt_sqlite: false,
        needs_rt_regex: false,
        exclude: exclude.iter().cloned().collect(),
        cells: std::collections::HashSet::new(),
        fibers,
    };

    let mut out = String::new();
    // N2: el hasher de los Map (aHash por defecto, std con `--without ahash`) se decide aquí porque el
    // alias `__RayMap` vive en el preámbulo; la feature se añade a rt_features más abajo, junto a mimalloc.
    let use_ahash = !t.exclude.contains("ahash");
    emit_core_runtime(&mut out, fast, use_ahash, fibers);

    // Definiciones de tipos de usuario (no genéricos). struct → Rust struct; enum → Rust enum. `Clone`
    // para el clon-al-leer y para los payloads. El orden no importa (Rust permite referencias adelantadas).
    for s in &prog.structs {
        // (El struct `Iter` del protocolo de iterador del prelude SÍ se emite desde B2: es
        // `{ step: Rc<dyn Fn() -> Option<T>> }`, y `iter`/`range`/`map`/`filter` lo construyen con
        // closures que mutan su cursor capturado — transpilable desde B1.)
        t.tparams = s.type_params.iter().cloned().collect();
        // `dyn Trait` (M9.3b): struct sintetizado `__dyn_T { data, métodos… }`. Aquí, un juego de closures
        // que CAPTURAN el valor concreto (sin `data`, sin Box<dyn Any>): cada campo `Rc<dyn Fn(args)->ret>`.
        if s.name.starts_with("__dyn_") {
            writeln!(out, "#[derive(Clone)]\nstruct {} {{", mangle(&s.name)).unwrap();
            for (fname, _) in &s.fields {
                if fname == "data" {
                    continue; // el valor concreto lo capturan las closures, no se guarda aparte
                }
                let (args, ret) = t
                    .trait_method_sigs
                    .get(fname)
                    .ok_or_else(|| format!("unknown dyn method '{}'", fname))?;
                let atys: Vec<String> =
                    args.iter().map(|a| rust_ty(a, &t.enums, &t.tparams)).collect::<Result<_, _>>()?;
                writeln!(out, "    {}: Rc<dyn Fn({}) -> {}>,", mangle(fname), atys.join(", "), rust_ty(ret, &t.enums, &t.tparams)?).unwrap();
            }
            out.push_str("}\n");
            continue;
        }
        writeln!(out, "#[derive(Clone)]\nstruct {}{} {{", mangle(&s.name), generic_decl(&s.type_params)).unwrap();
        for (fname, fty) in &s.fields {
            // El nombre de campo puede ser palabra reservada de Rust (`type`, `ref`, …): mismo mangle
            // que en literal/acceso/asignación → consistente.
            writeln!(out, "    {}: {},", mangle(fname), rust_ty(fty, &t.enums, &t.tparams)?).unwrap();
        }
        out.push_str("}\n");
    }
    for e in &prog.enums {
        if e.name == "Option" || e.name == "Result" {
            continue; // nativos de Rust
        }
        t.tparams = e.type_params.iter().cloned().collect();
        writeln!(out, "#[derive(Clone)]\nenum {}{} {{", mangle(&e.name), generic_decl(&e.type_params)).unwrap();
        for v in &e.variants {
            if v.payload.is_empty() {
                writeln!(out, "    {},", mangle(&v.name)).unwrap(); // la variante puede ser keyword de Rust
            } else {
                let tys: Vec<String> =
                    v.payload.iter().map(|t2| rust_ty(t2, &t.enums, &t.tparams)).collect::<Result<_, _>>()?;
                writeln!(out, "    {}({}),", mangle(&v.name), tys.join(", ")).unwrap();
            }
        }
        out.push_str("}\n");
    }
    t.tparams.clear();
    // impls de Display (= el Show de raylang): struct `Name { f: v, … }`, enum `Name.Variant(payload)`.
    t.emit_rayshow_impls(&mut out, prog)?;
    // Constantes de nivel superior → funciones `fn NAME() -> T { <literal> }`.
    for c in &prog.consts {
        // `std::math::PI`/`E` se emiten como constantes de `std::f64::consts` en el sitio de uso.
        if c.name.starts_with("std::math::") {
            continue;
        }
        // La DEFINICIÓN debe usar el mismo `mangle` que el uso (línea ~1605): una constante namespacada
        // de un módulo importado llega como `geo::PI` → `fn geo::PI()` sería Rust inválido.
        write!(out, "fn {}() -> {} {{ ", mangle(&c.name), rust_ty(&c.ty, &t.enums, &t.tparams)?).unwrap();
        t.emit_expr(&mut out, &c.value)?;
        out.push_str(" }\n");
    }
    // Funciones externas (FFI, M41): declaraciones `extern "C"` + wrappers que marshalan.
    t.emit_externs(&mut out, prog)?;
    out.push('\n');

    let mut main_ret_int = false;
    let mut main_seen = false;
    let mut stubbed: Vec<(String, String)> = Vec::new();
    for f in &prog.functions {
        if skip_fn_def(f) {
            continue;
        }
        let rust_name = if f.name == "main" { "ray_main".to_string() } else { mangle(&f.name) };
        let mut fbuf = String::new();
        match t.emit_function(&mut fbuf, &rust_name, f) {
            Ok(()) => {
                out.push_str(&fbuf);
                out.push('\n');
                if f.name == "main" {
                    main_seen = true;
                    main_ret_int = matches!(f.return_type, Type::Int);
                }
            }
            Err(e) => {
                if f.name == "main" {
                    return Err(format!("main is outside the supported subset: {}", e));
                }
                // Una función no-main cuyo CUERPO no transpila se emite como STUB que panica (con su firma):
                // el programa COMPILA y, si el flujo real no la llama, corre igual que la VM. Si ni la firma
                // es representable, se OMITE (última salida; una llamada colgante haría fallar rustc).
                // `RAYLANG_TRANSPILE_DEBUG` reporta qué se convirtió en stub (u omitió) y por qué.
                let mut sbuf = String::new();
                match t.emit_stub(&mut sbuf, &rust_name, f) {
                    Ok(()) => {
                        out.push_str(&sbuf);
                        out.push('\n');
                        stubbed.push((f.name.clone(), e.clone()));
                        if std::env::var_os("RAYLANG_TRANSPILE_DEBUG").is_some() {
                            eprintln!("[transpile stub] {} — {}", f.name, e);
                        }
                    }
                    Err(se) => {
                        stubbed.push((f.name.clone(), format!("{e} (signature: {se})")));
                        if std::env::var_os("RAYLANG_TRANSPILE_DEBUG").is_some() {
                            eprintln!("[transpile skip] {} — cuerpo: {} — firma: {}", f.name, e, se);
                        }
                    }
                }
            }
        }
    }
    if !main_seen {
        return Err("`main` is not in the supported subset".into());
    }

    // H6 + H21-N1: `main` instala el hook (calla los `__RayErr`: su mensaje limpio se imprime aquí,
    // al observarlos) y captura todo unwind → los errores de ejecución propios dan `runtime error:
    // <msg>` + exit 70; los panics RESTANTES de Rust (índice fuera de rango, expects de FFI…) dan
    // exit 70 con el texto de Rust (paridad de código, no de texto, para esa cola).
    out.push_str("fn main() {\n");
    // Sube el límite blando de fds al duro (acotado) — espejo de `lib::raise_fd_limit` del host:
    // el default de macOS (256) tumbaba un webserver nativo bajo `wrk -c500` sin culpa del programa.
    out.push_str("    #[cfg(unix)] unsafe {\n");
    out.push_str("        #[repr(C)] struct RL { cur: u64, max: u64 }\n");
    out.push_str("        #[cfg(target_os = \"linux\")] const NOFILE: i32 = 7;\n");
    out.push_str("        #[cfg(not(target_os = \"linux\"))] const NOFILE: i32 = 8;\n");
    out.push_str("        unsafe extern \"C\" { fn getrlimit(r: i32, l: *mut RL) -> i32; fn setrlimit(r: i32, l: *const RL) -> i32; }\n");
    out.push_str("        let cap: u64 = if cfg!(target_os = \"macos\") { 10240 } else { 65536 };\n");
    out.push_str("        let mut r = RL { cur: 0, max: 0 };\n");
    out.push_str("        if getrlimit(NOFILE, &mut r) == 0 { let o = r.max.min(cap); if o > r.cur { let n = RL { cur: o, max: r.max }; let _ = setrlimit(NOFILE, &n); } }\n");
    out.push_str("    }\n");
    out.push_str("    let __rt_hook = std::panic::take_hook();\n");
    // M97.2: el hook también calla dentro de un `try_call` — el fallo se va a convertir en valor,
    // así que imprimir "thread panicked at …" sería ruido que la VM no emite.
    out.push_str("    std::panic::set_hook(std::boxed::Box::new(move |i| { if i.payload().downcast_ref::<__RayErr>().is_none() && !__ray_in_try() { __rt_hook(i); } }));\n");
    out.push_str("    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(ray_main)) {\n");
    if main_ret_int {
        out.push_str("        Ok(code) => { __ray_flush_prints(); std::process::exit(code as i32) },\n");
    } else {
        out.push_str("        Ok(_) => { __ray_flush_prints(); std::process::exit(0) },\n");
    }
    out.push_str("        Err(e) => {\n");
    out.push_str("            if let Some(r) = e.downcast_ref::<__RayErr>() { eprintln!(\"runtime error: {}\", r.0); }\n");
    out.push_str("            __ray_flush_prints();\n");
    out.push_str("            std::process::exit(70)\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("}\n");
    // M96f: `print` deja de tomar el lock GLOBAL de `Stdout` en cada llamada — bajo impresión
    // concurrente intensiva (p. ej. `log_requests()`) era el mayor cuello de contención medido
    // (docs/investigacion-p99-framework-web.md §12). Diseño: un ÚNICO hilo escritor consume un
    // canal mpsc (std, sin dependencias); cada `print` solo hace `send`, nunca toca stdout
    // directo. **Por qué no un buffer por hilo** (primer intento, revertido): rompía el orden
    // CAUSAL entre hilos sincronizados por `join`/canales — un test lo cazó
    // (`build_native_valores_de_heap_y_funciones_cruzan_los_hilos`: una tarea que imprime y
    // luego el padre que la `join`-ea e imprime después deben verse en ESE orden; con buffers
    // independientes por hilo y flush por tiempo, podían intercalarse distinto). Un solo canal
    // preserva la MISMA garantía que el lock de hoy dan: si el envío A pasa-antes-que el envío B
    // (por una sincronización externa real, como `join`), la cola FIFO los entrega en ese orden;
    // entre hilos sin relación causal, el orden ya era no-determinista antes también (competían
    // por el lock de Stdout en el orden que tocara el scheduler) — nada cambia ahí.
    // `std::process::exit` no corre destructores de ningún hilo → `__ray_flush_prints()` espera
    // (con un tope de 500 ms de salvavidas) a que el escritor haya vaciado TODO lo enviado hasta
    // ese instante, antes de cada uno de los 3 sitios que llaman `std::process::exit`.
    out.push_str(concat!(
        "static __RAY_PRINT_SENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);\n",
        "static __RAY_PRINT_WRITTEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);\n",
        "static __RAY_PRINT_TX: std::sync::OnceLock<std::sync::mpsc::Sender<String>> = std::sync::OnceLock::new();\n",
        "fn __ray_print_tx() -> &'static std::sync::mpsc::Sender<String> {\n",
        "    __RAY_PRINT_TX.get_or_init(|| {\n",
        "        let (tx, rx) = std::sync::mpsc::channel::<String>();\n",
        "        std::thread::spawn(move || {\n",
        "            use std::io::Write;\n",
        "            let mut out = std::io::stdout();\n",
        "            loop {\n",
        "                match rx.recv_timeout(std::time::Duration::from_millis(5)) {\n",
        "                    Ok(line) => {\n",
        "                        let mut buf = line; buf.push('\\n'); let mut n: u64 = 1;\n",
        "                        while let Ok(more) = rx.try_recv() { buf.push_str(&more); buf.push('\\n'); n += 1; }\n",
        "                        let _ = out.write_all(buf.as_bytes());\n",
        "                        let _ = out.flush();\n",
        "                        __RAY_PRINT_WRITTEN.fetch_add(n, std::sync::atomic::Ordering::Release);\n",
        "                    }\n",
        "                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}\n",
        "                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,\n",
        "                }\n            }\n        });\n",
        "        tx\n    })\n}\n",
        "fn __ray_buffered_print(line: String) {\n",
        "    __RAY_PRINT_SENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);\n",
        "    let _ = __ray_print_tx().send(line);\n}\n",
        "fn __ray_flush_prints() {\n",
        "    if __RAY_PRINT_TX.get().is_none() { return; }\n",
        "    let target = __RAY_PRINT_SENT.load(std::sync::atomic::Ordering::Relaxed);\n",
        "    let start = std::time::Instant::now();\n",
        "    while __RAY_PRINT_WRITTEN.load(std::sync::atomic::Ordering::Acquire) < target {\n",
        "        if start.elapsed() > std::time::Duration::from_millis(500) { break; }\n",
        "        std::thread::sleep(std::time::Duration::from_micros(100));\n",
        "    }\n}\n",
    ));
    // H21-N5a: los conversores Send registrados durante la emisión (tipos que cruzan hilos). Worklist:
    // generar uno puede registrar tipos anidados. Va antes de los bloques de runtime (orden top-level
    // libre en Rust; solo importa que TODA la emisión de cuerpos ya pasó).
    t.emit_send_convs(&mut out)?;
    emit_runtime_features(&mut out, &mut t);
    // Features de `ray-runtime` a activar (bajo demanda). Vacío → `build_native` usa `rustc` pelado.
    let mut rt_features = Vec::new();
    if t.needs_rt_crypto {
        rt_features.push("crypto");
    }
    if t.needs_rt_tls {
        rt_features.push("tls");
    }
    if t.needs_rt_sqlite {
        rt_features.push("sqlite");
    }
    // R5: motor de regex acelerado (detectado por USO de std/regex, como crypto/tls/sqlite;
    // `--without regex` ya evitó marcar el flag y la Pike VM raylang se transpila tal cual).
    if t.needs_rt_regex {
        rt_features.push("regex");
    }
    // N1 (bench políglota, jul 2026): mimalloc como allocador del binario transpilado, POR DEFECTO. El
    // malloc del sistema (macOS) es lento en churn de strings pequeños: medido wordcount/logparse −40%,
    // jsonserialize −18% (docs/bench-poliglota-optimizacion.md §3). A diferencia de crypto/tls/sqlite no
    // se detecta por uso: siempre on salvo `--without mimalloc` (escape para builds herméticos/cross sin
    // toolchain C/rustc-pelado). El `#[global_allocator]` va en el main GENERADO — una dep no referenciada
    // no se enlaza y el allocador no aplicaría (por eso ray-runtime solo REEXPORTA `MiMalloc`).
    if !t.exclude.contains("mimalloc") {
        out.push_str("#[global_allocator]\nstatic __RAY_ALLOC: ray_runtime::MiMalloc = ray_runtime::MiMalloc;\n");
        rt_features.push("mimalloc");
    }
    // N2: aHash en los Map, por defecto (como mimalloc: siempre-on salvo `--without ahash`). El alias
    // `__RayMap` ya se emitió en el preámbulo; aquí solo se activa la feature que trae el crate.
    if use_ahash {
        rt_features.push("ahash");
    }
    // F2: el scheduler de fibras vive en ray-runtime tras su feature → --fibers fuerza la vía Cargo
    // (una feature no vacía nunca compila por rustc pelado, que no puede traer corosensei).
    if fibers {
        rt_features.push("fibers");
    }
    Ok(Transpiled { source: out, rt_features, stubbed })
}

#[cfg(test)]
mod tests;
