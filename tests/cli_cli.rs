//! Pruebas del CLI de subcomandos (M39a) sobre el binario: `new`, `run`, `build`, `test`,
//! `help`, `version`, y la compatibilidad con la interfaz legada por flags.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raylang");

/// Ejecuta el binario con `args` y `cwd`, devuelve (stdout, stderr, código).
fn ray(cwd: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN).args(args).current_dir(cwd).output().expect("lanza el binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Un directorio temporal único por prueba (evita choques entre tests paralelos).
fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ray_cli_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("crea el dir temporal");
    d
}

#[test]
fn new_crea_el_esqueleto_y_run_lo_ejecuta() {
    let base = tmp("new");
    // `ray new proj` crea ray.toml + src/main.ray + .gitignore.
    let (out, _err, code) = ray(&base, &["new", "proj"]);
    assert_eq!(code, 0, "new must salir 0\n{out}");
    let proj = base.join("proj");
    assert!(proj.join("ray.toml").is_file(), "falta ray.toml");
    assert!(proj.join("src/main.ray").is_file(), "falta src/main.ray");
    assert!(proj.join(".gitignore").is_file(), "falta .gitignore");
    let manifest = std::fs::read_to_string(proj.join("ray.toml")).unwrap();
    assert!(manifest.contains("name = \"proj\""), "el manifest nombra el project\n{manifest}");

    // `ray run` sin archivo usa src/main.ray (convención de proyecto).
    let (out, _err, code) = ray(&proj, &["run"]);
    assert!(out.contains("hello from proj"), "run ejecuta el hello-mundo\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn new_fails_si_el_target_existe() {
    let base = tmp("new_dup");
    assert_eq!(ray(&base, &["new", "dup"]).2, 0);
    let (_o, err, code) = ray(&base, &["new", "dup"]);
    assert_ne!(code, 0, "no must sobrescribir un directory existente");
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn run_pasa_los_args_del_program() {
    let base = tmp("run_args");
    std::fs::write(
        base.join("prog.ray"),
        "fn main() -> int { print(args().len()); 0 }\n",
    )
    .unwrap();
    // Los argumentos tras el archivo llegan a `args()`.
    let (out, _err, _code) = ray(&base, &["run", "prog.ray", "one", "dos", "tres"]);
    assert!(out.contains("3"), "args() ve los 3 argumentos\n{out}");
}

#[test]
fn build_compila_ok_y_reports_errors() {
    let base = tmp("build");
    // Programa válido: build sale 0.
    std::fs::write(base.join("ok.ray"), "fn main() -> int { 1 + 2 }\n").unwrap();
    let (out, _err, code) = ray(&base, &["build", "ok.ray"]);
    assert_eq!(code, 0, "build de un program válido sale 0\n{out}");
    assert!(out.contains("compila"), "{out}");
    // build NO ejecuta: un programa que devolvería 42 igual sale 0 (solo compiló).
    std::fs::write(base.join("cuarenta.ray"), "fn main() -> int { 42 }\n").unwrap();
    assert_eq!(ray(&base, &["build", "cuarenta.ray"]).2, 0, "build no runs el program");
    // Programa con error de tipos: build sale 65 y no dice 'compila'.
    std::fs::write(base.join("mal.ray"), "fn main() -> int { 1 + true }\n").unwrap();
    let (_o, err, code) = ray(&base, &["build", "mal.ray"]);
    assert_eq!(code, 65, "build de un program con error sale 65\n{err}");
    assert!(err.contains("type error"), "{err}");
}

#[test]
fn build_native_produce_un_binario_que_corre_como_la_vm() {
    // `ray build --native` (P2.b): transpila a Rust, compila con `rustc -O` y produce un binario nativo
    // cuya salida coincide con la VM. Requiere `rustc` en el PATH; si no está, se salta (no es un fallo).
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native: rustc no disponible");
        return;
    }
    let base = tmp("build_native");
    std::fs::write(
        base.join("prog.ray"),
        "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }\n\
         fn main() -> int { print(fib(10)); 0 }\n",
    )
    .unwrap();
    let bin = base.join("prog_bin");
    let (out, err, code) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native sale 0\nstdout={out}\nstderr={err}");
    assert!(out.contains("binario nativo"), "reporta el binario\n{out}");
    assert!(bin.is_file(), "el binario nativo existe");
    // El binario nativo produce la MISMA salida que la VM.
    let native = Command::new(&bin).output().expect("corre el binario nativo");
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    let (vm_out, _e, _c) = ray(&base, &["run", "prog.ray"]);
    assert_eq!(native_out, vm_out, "nativo ≡ VM");
    assert_eq!(native_out.trim(), "55", "fib(10) = 55");
}

#[test]
fn build_native_de_un_proyecto_multi_modulo_es_un_solo_binario() {
    // `ray build --native` sobre un main que importa OTRO módulo con tipos propios: el loader aplana
    // todo en un Program, el transpilador mangla los tipos namespacados (`geo::Punto` → `geo_CC_Punto`)
    // y sale UN solo binario nativo cuya salida coincide con la VM.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native multi-módulo: rustc no disponible");
        return;
    }
    let base = tmp("build_native_multi");
    std::fs::write(
        base.join("geo.ray"),
        "pub struct Punto { x: int, y: int }\n\
         pub fn suma(p: Punto) -> int { p.x + p.y }\n",
    )
    .unwrap();
    std::fs::write(
        base.join("main.ray"),
        "import geo;\n\
         fn main() -> int { let p = geo.Punto { x: 3, y: 4 }; print(geo.suma(p)); print(p); 0 }\n",
    )
    .unwrap();
    let bin = base.join("app");
    let (out, err, code) = ray(&base, &["build", "main.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native multi-módulo sale 0\nstdout={out}\nstderr={err}");
    assert!(bin.is_file(), "un solo binario nativo");
    let native = Command::new(&bin).output().expect("corre el binario nativo");
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    let (vm_out, _e, _c) = ray(&base, &["run", "main.ray"]);
    assert_eq!(native_out, vm_out, "nativo ≡ VM (multi-módulo)");
    // suma(3,4)=7; el render default de `print` sobre un struct namespacado usa el nombre COMPLETO (geo::Punto).
    assert_eq!(native_out, "7\ngeo::Punto { x: 3, y: 4 }\n", "salida esperada");
}

#[test]
fn build_native_env_y_args_coinciden_con_la_vm() {
    // `ray build --native` de un programa que lee una variable de entorno (env) y los argumentos de
    // línea de comandos (args): el binario nativo, con el MISMO entorno/args, produce la salida de la VM.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native env/args: rustc no disponible");
        return;
    }
    let base = tmp("build_native_env");
    std::fs::write(
        base.join("prog.ray"),
        "fn main() -> int {\n\
           match (env(\"RAY_IT_VAR\")) { Option.Some(v) => print(\"env: \" + v), Option.None => print(\"env: none\") }\n\
           let a = args();\n\
           print(\"argc: \" + to_string(a.len()));\n\
           if (a.len() > 0) { print(\"arg0: \" + a[0]); } else { print(\"arg0: -\"); }\n\
           0\n\
         }\n",
    )
    .unwrap();
    let bin = base.join("prog_bin");
    let (_o, err, code) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native env/args ok\n{err}");

    // Nativo: mismo env + args posicionales.
    let native = Command::new(&bin)
        .env("RAY_IT_VAR", "hola")
        .args(["uno", "dos"])
        .output()
        .expect("corre el binario nativo");
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    // VM: `ray run prog.ray uno dos` con el mismo env (los args tras el archivo van a args()).
    let vm = Command::new(BIN)
        .args(["run", "prog.ray", "uno", "dos"])
        .env("RAY_IT_VAR", "hola")
        .current_dir(&base)
        .output()
        .expect("corre la VM");
    let vm_out = String::from_utf8_lossy(&vm.stdout).into_owned();
    assert_eq!(native_out, vm_out, "nativo ≡ VM (env + args)");
    assert_eq!(native_out, "env: hola\nargc: 2\narg0: uno\n", "salida esperada");
}

#[test]
fn build_native_concurrencia_csp_coincide_con_la_vm() {
    // `ray build --native` de un pipeline CSP (spawn + canales): el binario nativo usa hilos de SO reales
    // y, por el orden FIFO de los canales, produce la MISMA salida (determinista-por-diseño) que la VM.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native concurrencia: rustc no disponible");
        return;
    }
    let base = tmp("build_native_conc");
    // productor → cuadrador → main (pipeline con canal acotado, como examples/concurrency/concurrencia.ray).
    std::fs::write(
        base.join("prog.ray"),
        "fn gen(out: Channel<int>, n: int) { var i = 1; while (i <= n) { send(out, i); i = i + 1; } close(out); }\n\
         fn sq(inp: Channel<int>, out: Channel<int>) { var go = true; while (go) { match (recv(inp)) { Option.Some(v) => send(out, v * v), Option.None => { close(out); go = false; } } } }\n\
         fn main() -> int {\n\
           let a: Channel<int> = Channel.bounded(2);\n\
           let b: Channel<int> = Channel.new();\n\
           spawn(fn() { gen(a, 5); });\n\
           spawn(fn() { sq(a, b); });\n\
           var total = 0; var go = true;\n\
           while (go) { match (recv(b)) { Option.Some(v) => { print(v); total = total + v; }, Option.None => { go = false; } } }\n\
           print(total);\n\
           0\n\
         }\n",
    )
    .unwrap();
    let bin = base.join("prog_bin");
    let (_o, err, code) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native concurrencia ok\n{err}");

    let expected = "1\n4\n9\n16\n25\n55\n";
    let (vm_out, _e, _c) = ray(&base, &["run", "prog.ray"]);
    assert_eq!(vm_out, expected, "VM da el pipeline");
    // El binario nativo, 5 veces, siempre igual (determinista-por-diseño pese a los hilos reales).
    for _ in 0..5 {
        let native = Command::new(&bin).output().expect("corre el binario nativo");
        let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
        assert_eq!(native_out, expected, "nativo ≡ VM (estable)");
    }
}

#[test]
fn build_native_structured_concurrency_coincide_con_la_vm() {
    // `ray build --native` de structured concurrency (scope + spawn→Task + join): las tareas se lanzan en
    // hilos reales, join recoge sus resultados, scope las une al salir. Salida determinista-por-diseño.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native structured: rustc no disponible");
        return;
    }
    let base = tmp("build_native_struct");
    std::fs::write(
        base.join("prog.ray"),
        "fn sq(n: int) -> int { n * n }\n\
         fn main() -> int {\n\
           let total = scope(fn() -> int {\n\
             let a: Task<int> = spawn(fn() -> int { sq(3) });\n\
             let b: Task<int> = spawn(fn() -> int { sq(4) });\n\
             let c: Task<int> = spawn(fn() -> int { sq(5) });\n\
             join(a) + join(b) + join(c)\n\
           });\n\
           print(total);\n\
           0\n\
         }\n",
    )
    .unwrap();
    let bin = base.join("prog_bin");
    let (_o, err, code) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native structured ok\n{err}");

    let (vm_out, _e, _c) = ray(&base, &["run", "prog.ray"]);
    assert_eq!(vm_out, "50\n", "VM: 9+16+25 = 50");
    for _ in 0..5 {
        let native = Command::new(&bin).output().expect("corre el binario nativo");
        assert_eq!(String::from_utf8_lossy(&native.stdout), "50\n", "nativo ≡ VM (estable)");
    }
}

#[test]
fn build_native_select_invariante_coincide_con_la_vm() {
    // `select` sobre varios canales: bajo paralelismo real el ORDEN de impresión es no-determinista (la
    // VM multicore por default también varía), pero el INVARIANTE (multiset de valores + total + exit
    // code) casa. El binario nativo recoge los 4 valores {100,101,200,201}, total 602, exit 90 (602&0xFF).
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native select: rustc no disponible");
        return;
    }
    let base = tmp("build_native_select");
    std::fs::write(
        base.join("prog.ray"),
        "fn src(ch: Channel<int>, base: int, k: int) { var i = 0; while (i < k) { send(ch, base + i); i = i + 1; } }\n\
         fn main() -> int {\n\
           let a: Channel<int> = Channel.new();\n\
           let b: Channel<int> = Channel.new();\n\
           spawn(fn() { src(a, 100, 2); });\n\
           spawn(fn() { src(b, 200, 2); });\n\
           let chs: [Channel<int>] = [a, b];\n\
           var total = 0; var n = 0;\n\
           while (n < 4) { let i = select(chs); match (recv(chs[i])) { Option.Some(v) => { print(v); total = total + v; }, Option.None => { } } n = n + 1; }\n\
           print(total);\n\
           total\n\
         }\n",
    )
    .unwrap();
    let bin = base.join("prog_bin");
    let (_o, err, code) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native select ok\n{err}");

    // El binario, varias veces: el multiset ordenado y el exit code son invariantes (aunque el orden no).
    for _ in 0..5 {
        let out = Command::new(&bin).output().expect("corre el binario nativo");
        assert_eq!(out.status.code(), Some(90), "exit = 602 & 0xFF");
        let s = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut lines: Vec<&str> = s.lines().collect();
        lines.sort();
        assert_eq!(lines, vec!["100", "101", "200", "201", "602"], "multiset de valores + total");
    }
}

#[cfg(unix)]
#[test]
fn build_native_signals_apagado_ordenado() {
    // `ray build --native` de un programa que usa signals() (SIGTERM/SIGINT): el binario nativo instala
    // los handlers (self-pipe + FFI a libc) y, al recibir SIGTERM, drena el canal de señales y apaga.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native signals: rustc no disponible");
        return;
    }
    let base = tmp("build_native_signals");
    std::fs::write(
        base.join("prog.ray"),
        "fn main() -> int {\n\
           let work: Channel<int> = Channel.new();\n\
           let sig = signals();\n\
           spawn(fn() { var i = 0; while (i < 3) { send(work, i); i = i + 1; } });\n\
           let chs: [Channel<int>] = [work, sig];\n\
           var go = true;\n\
           while (go) {\n\
             let idx = select(chs);\n\
             if (idx == 0) { match (recv(work)) { Option.Some(x) => print(\"work \" + to_string(x)), Option.None => { } } }\n\
             else { match (recv(sig)) { Option.Some(n) => { print(\"signal \" + to_string(n) + \": shutdown\"); go = false; }, Option.None => { go = false; } } }\n\
           }\n\
           0\n\
         }\n",
    )
    .unwrap();
    let bin = base.join("prog_bin");
    let (_o, err, code) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native signals ok\n{err}");

    // Corre el binario y le da tiempo de sobra a arrancar e instalar los handlers de señal (bajo carga
    // paralela de tests el arranque puede tardar), luego envía SIGTERM (15) y comprueba el apagado ordenado.
    let mut child = Command::new(&bin).stdout(std::process::Stdio::piped()).spawn().expect("lanza el nativo");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let pid = child.id().to_string();
    let _ = Command::new("kill").args(["-TERM", &pid]).status();
    let out = child.wait_with_output().expect("espera al proceso");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("signal 15: shutdown"), "SIGTERM → apagado ordenado\nsalida:\n{s}");
    assert!(s.contains("work 0"), "procesó al menos un item de trabajo\n{s}");
}

#[test]
fn build_native_canal_de_string_coincide_con_la_vm() {
    // `ray build --native` de un programa que manda STRINGS por un canal y una Task<string>: el valor
    // viaja como repr Send (Arc<str>) al cruzar el hilo y se convierte de vuelta a Rc<str> al recibir.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native canal string: rustc no disponible");
        return;
    }
    let base = tmp("build_native_chstr");
    std::fs::write(
        base.join("prog.ray"),
        "fn saluda(n: string) -> string { \"hola \" + n }\n\
         fn main() -> int {\n\
           let ch: Channel<string> = Channel.new();\n\
           spawn(fn() { send(ch, \"mundo\"); send(ch, \"raylang\"); close(ch); });\n\
           var go = true;\n\
           while (go) { match (recv(ch)) { Option.Some(s) => print(saluda(s)), Option.None => { go = false; } } }\n\
           let t: Task<string> = spawn(fn() -> string { saluda(\"tarea\") });\n\
           print(join(t));\n\
           0\n\
         }\n",
    )
    .unwrap();
    let bin = base.join("prog_bin");
    let (_o, err, code) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native canal string ok\n{err}");

    let expected = "hola mundo\nhola raylang\nhola tarea\n";
    let (vm_out, _e, _c) = ray(&base, &["run", "prog.ray"]);
    assert_eq!(vm_out, expected, "VM");
    for _ in 0..5 {
        let native = Command::new(&bin).output().expect("corre el binario nativo");
        assert_eq!(String::from_utf8_lossy(&native.stdout), expected, "nativo ≡ VM (canal de string)");
    }
}

#[test]
fn build_native_release_produce_binario_correcto() {
    // `ray build --native --release` usa el tier agresivo (opt3+lto+cgu1+target-cpu=native): compila más
    // lento y no-portable, pero el binario da la MISMA salida que el default y la VM (solo cambia rustc).
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native --release: rustc no disponible");
        return;
    }
    let base = tmp("build_native_release");
    std::fs::write(base.join("prog.ray"), "fn main() -> int { print(2 + 3 * 4); 0 }\n").unwrap();
    let bin = base.join("prog_bin");
    let (out, err, code) =
        ray(&base, &["build", "prog.ray", "--native", "--release", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native --release sale 0\nstdout={out}\nstderr={err}");
    assert!(out.contains("release: opt3+lto+native"), "reporta el tier release\n{out}");
    assert!(bin.is_file(), "el binario existe");
    let native = Command::new(&bin).output().expect("corre el binario release");
    assert_eq!(String::from_utf8_lossy(&native.stdout).trim(), "14", "2 + 3*4 = 14");
}

#[test]
fn build_native_enteros_con_tamano_no_corrompen_el_prelude() {
    // Regresión: un módulo con literales `u64` (aquí `poly1305`, importado) se desplaza a una banda de
    // líneas por el loader; el prelude se inyecta con SUS líneas. Antes del fix, un literal u64 podía caer
    // en la misma (línea, col) que un literal `int` del prelude (p. ej. el `17` de `string#hash`), y la
    // lowering de uint por posición lo envolvía como u64 → `string#hash` no compilaba en Rust. Con el
    // prelude en su banda disjunta, esto no ocurre: poly1305 (crypto con u64) transpila y ≡ VM.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native poly1305: rustc no disponible");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/web/poly1305_demo.ray");
    let base = tmp("build_native_u64");
    let bin = base.join("poly_bin");
    let (out, err, code) =
        ray(&base, &["build", src.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native de poly1305 sale 0\nstdout={out}\nstderr={err}");
    assert!(bin.is_file(), "el binario nativo existe");
    let native = Command::new(&bin).output().expect("corre el binario nativo");
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    let (vm_out, _e, _c) = ray(&base, &["run", src.to_str().unwrap()]);
    assert_eq!(native_out, vm_out, "poly1305 nativo ≡ VM");
    assert!(!native_out.trim().is_empty(), "el MAC no es vacío\n{native_out}");
}

#[test]
fn build_native_json_con_map_coincide_con_la_vm() {
    // `ray build --native` de un serializador JSON: el enum `Json` tiene una variante `JObject(Map<
    // string, Json>)`. El transpiler emite el `RayShow` de TODOS los enums (aunque no se impriman); el
    // de `Json` recurre al del Map, así que necesita `impl RayShow for Map` (`Map{k: v}`, ordenado). Sin
    // él, rustc no compilaba. La salida (stringify canónico) debe coincidir byte a byte con la VM.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native json: rustc no disponible");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/web/json_demo.ray");
    let base = tmp("build_native_json");
    let bin = base.join("json_bin");
    let (out, err, code) =
        ray(&base, &["build", src.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native de json sale 0\nstdout={out}\nstderr={err}");
    let native = Command::new(&bin).output().expect("corre el binario nativo");
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    let (vm_out, _e, _c) = ray(&base, &["run", src.to_str().unwrap()]);
    assert_eq!(native_out, vm_out, "json nativo ≡ VM");
    assert!(native_out.contains("\"nombre\":\"raylang\""), "serializa el objeto\n{native_out}");
}

#[test]
fn build_native_protobuf_con_concat_de_bytes_coincide_con_la_vm() {
    // `ray build --native` de un codec protobuf: usa concatenación de bytes (`b1 + b2`) por doquier.
    // Antes rustc fallaba (`cannot add Rc<[u8]> to Rc<[u8]>`); ahora `a + b` baja a un concat de slices.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native protobuf: rustc no disponible");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/web/protobuf_demo.ray");
    let base = tmp("build_native_protobuf");
    let bin = base.join("pb_bin");
    let (out, err, code) =
        ray(&base, &["build", src.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native de protobuf sale 0\nstdout={out}\nstderr={err}");
    let native = Command::new(&bin).output().expect("corre el binario nativo");
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    let (vm_out, _e, _c) = ray(&base, &["run", src.to_str().unwrap()]);
    assert_eq!(native_out, vm_out, "protobuf nativo ≡ VM");
}

#[test]
fn build_native_url_con_override_y_utf8_coincide_con_la_vm() {
    // `ray build --native` de un codec URL: (1) redefine `get_or` (2 args) — el builtin del prelude no debe
    // taparlo; (2) recorre strings con `len`/`s[i]` sobre UTF-8 multibyte (`más`, `ñ`) — `len` debe contar
    // CARACTERES, no bytes. Ambos eran bugs; la salida (encode/decode) debe casar con la VM byte a byte.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native url: rustc no disponible");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/web/url_demo.ray");
    let base = tmp("build_native_url");
    let bin = base.join("url_bin");
    let (out, err, code) =
        ray(&base, &["build", src.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native de url sale 0\nstdout={out}\nstderr={err}");
    let native = Command::new(&bin).output().expect("corre el binario nativo");
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    let (vm_out, _e, _c) = ray(&base, &["run", src.to_str().unwrap()]);
    assert_eq!(native_out, vm_out, "url nativo ≡ VM");
    assert!(native_out.contains("hola mundo & más"), "decodifica UTF-8\n{native_out}");
}

#[test]
fn build_native_http_con_tls_no_alcanzado_coincide_con_la_vm() {
    // `ray build --native` de un cliente HTTP: importa el módulo `http`, que trae funciones TLS
    // (`std::net::tls_connect`) fuera del subconjunto. Antes rustc fallaba por la llamada colgante,
    // aunque el demo habla HTTP PLANO y nunca alcanza TLS. Ahora la función no soportada se emite como
    // stub que panica → compila, y como el flujo real no la llama, la salida ≡ VM.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native http: rustc no disponible");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/web/http_demo.ray");
    let base = tmp("build_native_http");
    let bin = base.join("http_bin");
    let (out, err, code) =
        ray(&base, &["build", src.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native de http sale 0\nstdout={out}\nstderr={err}");
    let native = Command::new(&bin).output().expect("corre el binario nativo");
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    let (vm_out, _e, _c) = ray(&base, &["run", src.to_str().unwrap()]);
    assert_eq!(native_out, vm_out, "http nativo ≡ VM (TLS no alcanzado)");
}

#[test]
fn build_native_closures_con_estado_mutable_coincide_con_la_vm() {
    // `ray build --native` de closures con ESTADO mutable (patrón contador: `var n` que la closure
    // incrementa entre llamadas, y contadores independientes). Antes rustc fallaba (`cannot assign to
    // captured variable in a Fn closure`); ahora la var capturada+mutada vive en Rc<RefCell> (B1) →
    // la salida (contadores, captura transitiva) coincide con la VM byte a byte.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native closures: rustc no disponible");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/stdlib/closures.ray");
    let base = tmp("build_native_closures");
    let bin = base.join("cl_bin");
    let (out, err, code) =
        ray(&base, &["build", src.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native de closures sale 0\nstdout={out}\nstderr={err}");
    let native = Command::new(&bin).output().expect("corre el binario nativo");
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    let (vm_out, _e, _c) = ray(&base, &["run", src.to_str().unwrap()]);
    assert_eq!(native_out, vm_out, "closures con estado nativo ≡ VM");
    // Contadores independientes: c1 llega a 4 mientras c2 arranca en 1.
    assert!(native_out.contains("4"), "el contador llega a 4\n{native_out}");
}

#[test]
fn build_native_iteradores_coinciden_con_la_vm() {
    // `ray build --native` de iteradores (B2): un `impl Iterator<T>` de usuario recorrido con `for`, más
    // los adaptadores escalares del prelude (`.iter()`, `range`, `.map()`, `.filter()`). Antes fallaba
    // (`for sobre iterador no soportado`); ahora baja a un `loop` que llama `next` hasta `None`. La salida
    // (contador con estado + map/filter encadenados) coincide con la VM byte a byte.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native iteradores: rustc no disponible");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/stdlib/iterador_escalar.ray");
    let base = tmp("build_native_iter");
    let bin = base.join("iter_bin");
    let (out, err, code) =
        ray(&base, &["build", src.to_str().unwrap(), "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native de iteradores sale 0\nstdout={out}\nstderr={err}");
    let native = Command::new(&bin).output().expect("corre el binario nativo");
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    let (vm_out, _e, _c) = ray(&base, &["run", src.to_str().unwrap()]);
    assert_eq!(native_out, vm_out, "iteradores nativo ≡ VM");
    assert!(native_out.starts_with("15"), "el iterador de usuario suma 15\n{native_out}");
}

#[test]
fn build_native_servidor_tcp_hace_eco() {
    // `ray build --native` de un servidor TCP: escucha en un puerto libre (lo imprime), acepta una
    // conexión, lee y hace ECO con un prefijo. El TEST hace de cliente (std::net) y verifica el round-trip.
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native servidor TCP: rustc no disponible");
        return;
    }
    let base = tmp("build_native_tcp");
    std::fs::write(
        base.join("prog.ray"),
        "import std/net;\n\
         fn main() -> int {\n\
           let l = match (net.tcp_listen(\"127.0.0.1\", 0)) { Result.Ok(h) => h, Result.Err(e) => 0 - 1 };\n\
           print(to_string(net.local_port(l)));\n\
           let c = match (net.tcp_accept(l)) { Result.Ok(h) => h, Result.Err(e) => 0 - 1 };\n\
           let m = match (net.socket_read(c)) { Result.Ok(s) => s, Result.Err(e) => \"ERR\" };\n\
           let _ = net.socket_write(c, \"eco: \" + m);\n\
           let _ = close(c); let _ = close(l);\n\
           0\n\
         }\n",
    )
    .unwrap();
    let bin = base.join("srv");
    let (_o, err, code) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native servidor TCP ok\n{err}");

    // Lanza el servidor y lee su puerto de stdout (primera línea).
    use std::io::{BufRead, BufReader, Read, Write};
    let mut srv = Command::new(&bin).stdout(std::process::Stdio::piped()).spawn().expect("lanza el servidor");
    let mut sout = BufReader::new(srv.stdout.take().unwrap());
    let mut port_line = String::new();
    sout.read_line(&mut port_line).expect("lee el puerto");
    let port: u16 = port_line.trim().parse().expect("puerto numérico");

    // El TEST es el cliente: conecta, envía, lee el eco.
    let mut cli = std::net::TcpStream::connect(("127.0.0.1", port)).expect("conecta");
    cli.write_all(b"hola").expect("envía");
    let mut resp = Vec::new();
    cli.read_to_end(&mut resp).expect("lee la respuesta");
    assert_eq!(String::from_utf8_lossy(&resp), "eco: hola", "el servidor hace eco con prefijo");
    let _ = srv.wait();
}

#[test]
fn build_native_udp_hace_eco() {
    // `ray build --native` de un servidor UDP: bind en un puerto libre (imprime el puerto vía
    // net.local_port), recibe un datagrama y responde al remitente. El TEST hace de cliente (UdpSocket).
    if Command::new("rustc").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("saltando build_native UDP: rustc no disponible");
        return;
    }
    let base = tmp("build_native_udp");
    std::fs::write(
        base.join("prog.ray"),
        "import std/net;\n\
         fn main() -> int {\n\
           let b = __udp_bind(\"127.0.0.1\", 0);\n\
           let h = match (parse_int(b[1])) { Option.Some(x) => x, Option.None => 0 - 1 };\n\
           print(to_string(net.local_port(h)));\n\
           let r = __udp_recv_from(h);\n\
           let host = match (from_utf8(r[1])) { Result.Ok(s) => s, Result.Err(e) => \"\" };\n\
           let port = match (parse_int(match (from_utf8(r[2])) { Result.Ok(s) => s, Result.Err(e) => \"0\" })) { Option.Some(n) => n, Option.None => 0 };\n\
           let _ = __udp_send_to(h, host, port, r[3]);\n\
           0\n\
         }\n",
    )
    .unwrap();
    let bin = base.join("srv");
    let (_o, err, code) = ray(&base, &["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native UDP ok\n{err}");

    use std::io::{BufRead, BufReader};
    let mut srv = Command::new(&bin).stdout(std::process::Stdio::piped()).spawn().expect("lanza el servidor UDP");
    let mut sout = BufReader::new(srv.stdout.take().unwrap());
    let mut port_line = String::new();
    sout.read_line(&mut port_line).expect("lee el puerto");
    let port: u16 = port_line.trim().parse().expect("puerto numérico");

    // El TEST es el cliente UDP: envía un datagrama y espera el eco.
    let cli = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind cliente");
    cli.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
    cli.send_to(b"hola udp", ("127.0.0.1", port)).expect("envía");
    let mut buf = [0u8; 64];
    let (n, _) = cli.recv_from(&mut buf).expect("recibe el eco");
    assert_eq!(&buf[..n], b"hola udp", "el servidor hace eco del datagrama");
    let _ = srv.wait();
}

#[test]
fn test_subcomando_runs_las_tests() {
    let base = tmp("test");
    std::fs::write(
        base.join("suite.ray"),
        "@test\nfn pasa() -> bool { true }\n@test\nfn fails() -> bool { false }\nfn main() -> int { 0 }\n",
    )
    .unwrap();
    let (out, _err, code) = ray(&base, &["test", "suite.ray"]);
    assert!(out.contains("pasa") && out.contains("fails"), "informa ambas tests\n{out}");
    assert_eq!(code, 1, "el código de output es el número de fallos (1)");
    // Filtro por subcadena: solo la que pasa.
    let (out, _err, code) = ray(&base, &["test", "suite.ray", "pasa"]);
    assert!(out.contains("pasa") && !out.contains("FALLO"), "el filtro deja solo 'pasa'\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn help_y_version() {
    let cwd = std::env::temp_dir();
    let (out, _err, code) = ray(&cwd, &["help"]);
    assert_eq!(code, 0);
    assert!(out.contains("Usage: ray") && out.contains("new") && out.contains("build"), "{out}");
    let (out, _err, code) = ray(&cwd, &["version"]);
    assert_eq!(code, 0);
    assert!(out.contains("raylang 1."), "la versión del lenguaje\n{out}");
}

#[test]
fn compat_flags_legadas() {
    let base = tmp("legacy");
    std::fs::write(base.join("p.ray"), "fn main() -> int { 7 }\n").unwrap();
    // La interfaz previa por flags sigue funcionando (un `<archivo>` directo, y --vm).
    assert_eq!(ray(&base, &["p.ray"]).2, 7, "raylang <file> direct");
    assert_eq!(ray(&base, &["--vm", "p.ray"]).2, 7, "raylang --vm <file>");
}

// ── M39b: el manifiesto ray.toml dirige build/run/test ───────────────────────────────

/// Crea un proyecto con un `ray.toml` a medida y un archivo de entrada.
fn project(name: &str, manifest: &str, entry_rel: &str, entry_src: &str) -> std::path::PathBuf {
    let root = tmp(name);
    std::fs::write(root.join("ray.toml"), manifest).unwrap();
    let entry = root.join(entry_rel);
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(entry, entry_src).unwrap();
    root
}

#[test]
fn build_y_run_usan_la_entry_del_manifest() {
    let root = project(
        "manifest_entry",
        "[package]\nname = \"miapp\"\nversion = \"2.0.0\"\nentry = \"src/arranque.ray\"\n",
        "src/arranque.ray",
        "fn main() -> int { print(\"arranque\"); 5 }\n",
    );
    // build: banner con nombre+versión (a stderr) y compila la entry del manifiesto.
    let (out, err, code) = ray(&root, &["build"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("compilando miapp v2.0.0"), "banner del project\n{err}");
    assert!(out.contains("arranque.ray") && out.contains("compila"), "{out}");
    // run: ejecuta la entry del manifiesto (sin pasar archivo).
    let (out, _err, code) = ray(&root, &["run"]);
    assert!(out.contains("arranque"), "{out}");
    assert_eq!(code, 5, "el exit es el int de main");
}

#[test]
fn run_sube_a_la_root_from_un_subdirectorio() {
    let root = project(
        "manifest_subdir",
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n",
        "src/main.ray",
        "fn main() -> int { print(\"root\"); 0 }\n",
    );
    // Ejecutar desde src/: el CLI sube buscando ray.toml (como cargo/git).
    let (out, _err, code) = ray(&root.join("src"), &["run"]);
    assert!(out.contains("root"), "encuentra el project subiendo\n{out}");
    assert_eq!(code, 0);
}

#[test]
fn dependency_inalcanzable_fails_al_descargar() {
    // M39c-2a: una dependencia declarada se descarga en `run`/`build`; si no se puede clonar
    // (aquí, una ruta local inexistente → fallo rápido y offline), es error de compilación.
    let root = project(
        "manifest_deps",
        "[package]\nname = \"condeps\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"git+file:///no/existe/geo@v1\"\n",
        "src/main.ray",
        "fn main() -> int { 0 }\n",
    );
    let (_out, err, code) = ray(&root, &["run"]);
    assert_eq!(code, 65, "one dependency inalcanzable abort\n{err}");
    assert!(err.contains("geo") && err.contains("clone"), "error claro de descarga\n{err}");
}

#[test]
fn manifest_mal_formado_fails_claro() {
    let root = tmp("manifest_bad");
    std::fs::write(root.join("ray.toml"), "[package]\nname = sinComillas\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.ray"), "fn main() -> int { 0 }\n").unwrap();
    let (_out, err, code) = ray(&root, &["build"]);
    assert_eq!(code, 65, "un ray.toml mal formado es error de compilación\n{err}");
    assert!(err.contains("ray.toml:2"), "el error trae la línea\n{err}");
}

// ── M39c-1: la caché `.ray-deps/` es raíz de módulos (un paquete = una cápsula) ──────

/// Escribe un paquete `nombre` en la caché `.ray-deps/` del proyecto `raiz`, con su `mod.ray`.
fn dep(root: &std::path::Path, name: &str, mod_ray: &str) {
    let d = root.join(".ray-deps").join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("mod.ray"), mod_ray).unwrap();
}

#[test]
fn dependency_de_ray_deps_es_importable() {
    let root = project(
        "dep_import",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ngeo = \"git+https://x/geo@v1\"\n",
        "src/main.ray",
        "from geo import duplicate;\nfn main() -> int { print(duplicate(21)); 0 }\n",
    );
    dep(&root, "geo", "pub fn duplicate(x: int) -> int { x * 2 }\n");
    // La dependencia está en la caché → el loader la encuentra y `from geo import` funciona.
    let (out, err, code) = ray(&root, &["run"]);
    assert!(out.contains("42"), "uses la función de la dependency\n{out}\n{err}");
    assert_eq!(code, 0);
    // Y como está presente, NO se avisa de dependencia sin descargar.
    assert!(!err.contains("sin descargar"), "no must avisar de one dep presente\n{err}");
}

#[test]
fn dependency_qualified_y_capsule_protege_internos() {
    let root = project(
        "dep_capsule",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        "src/main.ray",
        "import geo;\nfn main() -> int { print(geo.triplicar(10)); 0 }\n",
    );
    // El paquete geo es una cápsula que usa su propio submódulo interno.
    dep(
        &root,
        "geo",
        "from geo/internal import triple;\npub fn triplicar(x: int) -> int { triple(x) }\n",
    );
    std::fs::write(
        root.join(".ray-deps/geo/internal.ray"),
        "pub fn triple(x: int) -> int { x * 3 }\n",
    )
    .unwrap();
    // Acceso calificado a la cara pública del paquete.
    let (out, err, code) = ray(&root, &["run"]);
    assert!(out.contains("30"), "geo.triplicar via su internal\n{out}\n{err}");
    assert_eq!(code, 0);

    // La app NO puede alcanzar el submódulo interno del paquete (enforcement de cápsula, M11.6b).
    std::fs::write(
        root.join("src/main.ray"),
        "import geo/internal;\nfn main() -> int { print(geo.internal.triple(5)); 0 }\n",
    )
    .unwrap();
    let (_o, err, code) = ray(&root, &["run"]);
    assert_eq!(code, 65, "alcanzar el internal de one dependency es error\n{err}");
    assert!(err.contains("internal to capsule 'geo'"), "{err}");
}

#[test]
fn lo_local_tapa_a_la_dependency_del_same_name() {
    let root = project(
        "dep_shadow",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        "src/main.ray",
        "from util import greeting;\nfn main() -> int { print(greeting()); 0 }\n",
    );
    // Módulo local `util` en el proyecto...
    std::fs::write(root.join("src/util.ray"), "pub fn greeting() -> int { 1 }\n").unwrap();
    // ...y una dependencia `util` homónima en la caché.
    dep(&root, "util", "pub fn greeting() -> int { 999 }\n");
    // El proyecto se busca antes que la caché: gana el módulo local.
    let (out, err, code) = ray(&root, &["run"]);
    assert!(out.contains("1") && !out.contains("999"), "lo local tapa a la dependency\n{out}\n{err}");
    assert_eq!(code, 0);
}

#[test]
fn doc_genera_markdown_de_la_superficie_public() {
    let base = tmp("doc");
    let file = base.join("lib.ray");
    std::fs::write(
        &file,
        "/// Suma dos enteros.\npub fn sum(a: int, b: int) -> int { a + b }\n\nfn interna() -> int { 0 }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["doc", file.to_str().unwrap()]);
    assert_eq!(code, 0, "doc must salir 0\n{err}");
    assert!(out.contains("# lib.ray"), "encabezado con el name\n{out}");
    assert!(out.contains("### `fn sum(a: int, b: int) -> int`"), "signature\n{out}");
    assert!(out.contains("Suma dos enteros."), "el comment /// se documenta\n{out}");
    assert!(!out.contains("interna"), "los ítems privados no se documentan\n{out}");
}

#[test]
fn importa_la_stdlib_del_sistema() {
    // Un programa fuera del repo (dir temporal) puede `import std/math;` — la stdlib va EMBEBIDA en el
    // binario (M40.5), así que resuelve sin que `std/` exista en disco junto al programa.
    let base = tmp("std_import");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/math;\nfn main() -> int { print(math.gcd(48, 36)); print(math.is_prime(13)); 0 }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con import std/math must salir 0\n{err}");
    assert!(out.contains("12"), "gcd(48,36)=12\n{out}");
    assert!(out.contains("true"), "is_prime(13)\n{out}");
}

#[test]
fn stdlib_text_capitaliza_e_invierte() {
    let base = tmp("std_text");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/text;\nfn main() -> int { print(text.capitalize(\"hello\")); print(text.reverse(\"abc\")); print(text.count(\"aaaa\", \"aa\")); 0 }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con import std/text must salir 0\n{err}");
    assert!(out.contains("Hello"), "capitalize\n{out}");
    assert!(out.contains("cba"), "reverse\n{out}");
    assert!(out.contains("2"), "count no solapado\n{out}");
}

/// M66 — std/text de producción: `words` separa por whitespace (no solo espacio), `lines`
/// parte por `\n` tratando `\r\n` y el salto final, y `reverse`/`count` (reescritas a O(n))
/// conservan la semántica UTF-8/no-solapado.
#[test]
fn stdlib_text_words_y_lines() {
    let base = tmp("std_text_m66");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/text;\n\
         fn main() -> int {\n\
           let ws = text.words(\" a\\tb\\r\\nc \");\n\
           print(to_string(ws.len()) + \":\" + join(ws, \",\"));\n\
           let ls = text.lines(\"one\\r\\ndos\\ntres\\n\");\n\
           print(to_string(ls.len()) + \":\" + join(ls, \"|\"));\n\
           print(to_string(text.lines(\"\").len()));\n\
           print(to_string(text.lines(\"a\\n\\nb\").len()));\n\
           print(text.reverse(\"café\"));\n\
           print(to_string(text.count(\"ñoño\", \"ño\")));\n\
           0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con std/text M66 must salir 0\n{err}");
    assert!(out.contains("3:a,b,c"), "words por whitespace\n{out}");
    assert!(out.contains("3:one|dos|tres"), "lines con \\r\\n y salto final\n{out}");
    assert!(out.contains("éfac"), "reverse UTF-8 multibyte\n{out}");
    assert!(out.contains("\n2\n"), "count multibyte no solapado\n{out}");
}

#[test]
fn stdlib_sort_busca_y_deduplica() {
    let base = tmp("std_sort");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/sort;\n\
         fn main() -> int {\n\
             print(sort.dedup([5, 2, 8, 2, 1, 8]));\n\
             print(sort.binary_search([1, 3, 5, 7, 9], 7));\n\
             print(sort.merge([1, 4], [2, 3, 5]));\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con import std/sort must salir 0\n{err}");
    assert!(out.contains("[1, 2, 5, 8]"), "dedup ordena y quita repetidos\n{out}");
    assert!(out.contains("Option.Some(3)"), "binary_search halla el índice\n{out}");
    assert!(out.contains("[1, 2, 3, 4, 5]"), "merge fusiona ordenado\n{out}");
}

#[test]
fn stdlib_encoding_hex_base64_url_json() {
    // M40.7a: librerías de encoding promovidas de examples/web/ a std/ (embebidas, fuente única).
    let base = tmp("std_enc");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/hex;\n\
         import std/base64;\n\
         import std/url;\n\
         import std/json;\n\
         fn main() -> int {\n\
             print(hex.hex_encode(bytes_of([255, 0, 171])));\n\
             print(base64.base64(\"hi\".to_bytes()));\n\
             print(url.url_encode(\"a b&c\"));\n\
             match (json.parse(\"{\\\"n\\\": 42}\")) {\n\
                 Result.Ok(j) => { print(json.stringify(j)); },\n\
                 Result.Err(e) => { print(e); },\n\
             }\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con imports de std encoding must salir 0\n{err}");
    assert!(out.contains("ff00ab"), "hex_encode\n{out}");
    assert!(out.contains("aGk="), "base64 de \"hi\"\n{out}");
    assert!(out.contains("a%20b%26c"), "url_encode\n{out}");
    assert!(out.contains("{\"n\":42}"), "json parse+stringify\n{out}");
}

/// M68.2 — `crypto.random_bytes(n)`: aleatoriedad criptográfica (CSPRNG del SO vía ring).
/// No determinista → prueba de propiedades: longitud exacta, n<=0 vacío, y dos tiradas de
/// 32 octetos distintas (iguales por azar = 2^-256).
#[test]
fn crypto_random_bytes_propiedades() {
    let base = tmp("crypto_rand");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/crypto;\n\
         fn main() -> int {\n\
             let a = crypto.random_bytes(32);\n\
             let b = crypto.random_bytes(32);\n\
             print(a.len());\n\
             print(crypto.random_bytes(0).len());\n\
             print(crypto.random_bytes(-5).len());\n\
             print(to_string(a) != to_string(b));\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con crypto.random_bytes must salir 0\n{err}");
    assert_eq!(out, "32\n0\n0\ntrue\n", "longitudes y no-repetición\n{out}");
}

#[test]
fn crypto_builtins_hashing_vectors() {
    // M43.5b: la cripto de producción (builtins vía ring) a nivel CLI. sha256/sha512/sha1/hmac_sha256
    // (bytes -> bytes; `to_string` de un bytes da su hex). Vectores NIST/RFC. (Antes esto probaba la std
    // cripto pura embebida, ahora des-embebida → solo ejemplos; los builtins la sustituyen.)
    let base = tmp("crypto_hash");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/crypto;\n\
         fn main() -> int {\n\
             print(to_string(crypto.sha256(\"abc\".to_bytes())));\n\
             print(to_string(crypto.sha512(\"\".to_bytes())));\n\
             print(to_string(crypto.hmac_sha256(\"\".to_bytes(), \"\".to_bytes())));\n\
             print(to_string(crypto.sha1(\"abc\".to_bytes())));\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con los builtins cripto must salir 0\n{err}");
    assert!(out.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"), "sha256(abc)\n{out}");
    assert!(out.contains("cf83e1357eefb8bd"), "sha512(\"\")\n{out}");
    assert!(out.contains("b613679a0814d9ec772f95d778c35fc5ff1697c493715653c6c712144292c5ad"), "hmac_sha256(\"\",\"\")\n{out}");
    assert!(out.contains("a9993e364706816aba3e25717850c26c9cd0d89d"), "sha1(abc)\n{out}");
}

#[test]
fn stdlib_compresion_roundtrip() {
    // M40.7c: compresión promovida a std/. deflate → std/inflate (namespacado en el ejemplo).
    let base = tmp("std_comp");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/inflate;\n\
         import std/deflate;\n\
         import std/huffman;\n\
         fn main() -> int {\n\
             let comp = deflate.deflate_raw(\"raylang raylang raylang comprime\".to_bytes());\n\
             match (inflate.inflate_raw(comp)) {\n\
                 Result.Ok(back) => { match (from_utf8(back)) {\n\
                     Result.Ok(s) => { print(s); }, Result.Err(e) => { print(e); },\n\
                 } }, Result.Err(e) => { print(e); },\n\
             }\n\
             match (huffman.huffman_decode(huffman.huffman_encode([65, 65, 66, 67]))) {\n\
                 Result.Ok(d) => { print(d); }, Result.Err(e) => { print(e); },\n\
             }\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con imports de std compresión must salir 0\n{err}");
    assert!(out.contains("raylang raylang raylang comprime"), "deflate→inflate roundtrip\n{out}");
    assert!(out.contains("[65, 65, 66, 67]"), "huffman roundtrip\n{out}");
}

#[test]
fn stdlib_text_regex_csv_toml() {
    // M40.7d: procesamiento de texto/datos (librerías puras de examples/stdlib/, hojas).
    let base = tmp("std_txt");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/regex;\n\
         import std/csv;\n\
         import std/toml;\n\
         fn main() -> int {\n\
             print(regex.find_all(\"[0-9]+\", \"a12b345c\"));\n\
             match (csv.parse_csv(\"a,b\\n1,2\")) {\n\
                 Result.Ok(rows) => { print(rows); }, Result.Err(e) => { print(e); },\n\
             }\n\
             match (toml.parse_toml(\"port = 8080\")) {\n\
                 Result.Ok(es) => { match (toml.toml_get(es, \"port\")) {\n\
                     Option.Some(v) => { print(toml.toml_show(v)); }, Option.None => { print(\"?\"); },\n\
                 } }, Result.Err(e) => { print(e); },\n\
             }\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con imports de std text must salir 0\n{err}");
    assert!(out.contains("[12, 345]"), "regex find_all\n{out}");
    assert!(out.contains("[[a, b], [1, 2]]"), "csv parse\n{out}");
    assert!(out.contains("8080"), "toml get\n{out}");
}

#[test]
fn stdlib_cripto_aead_y_protobuf() {
    // AEAD (chacha20-poly1305) de PRODUCCIÓN vía el builtin `ring` (M43.4) + protobuf (std, M40.7e).
    // seal → ct||tag (Option<bytes>), open verifica y descifra. Antes esto usaba la std cripto pura
    // embebida (des-embebida en M43.5b); el builtin la sustituye. Protobuf sigue siendo std embebida.
    let base = tmp("std_crypto");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/protobuf;\n\
         import std/crypto;\n\
         fn main() -> int {\n\
             let key = \"0123456789abcdef0123456789abcdef\".to_bytes();\n\
             let nonce = \"noncenonce12\".to_bytes();\n\
             match (crypto.chacha20poly1305_seal(key, nonce, \"\".to_bytes(), \"Hi\".to_bytes())) {\n\
                 Option.Some(ct) => {\n\
                     match (crypto.chacha20poly1305_open(key, nonce, \"\".to_bytes(), ct)) {\n\
                         Option.Some(pt) => { print(to_string(pt)); },\n\
                         Option.None => { print(\"auth\"); },\n\
                     }\n\
                 },\n\
                 Option.None => { print(\"seal-none\"); },\n\
             }\n\
             let w = protobuf.writer();\n\
             protobuf.write_varint(w, 1, 150);\n\
             print(to_string(protobuf.finish(w)));\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con el builtin AEAD + std/protobuf must salir 0\n{err}");
    assert!(out.contains("4869"), "aead seal→open roundtrip (hex de \"Hi\")\n{out}");
    assert!(out.contains("089601"), "protobuf varint field1=150\n{out}");
}

#[test]
fn stdlib_uuid_genera_y_validates() {
    // M40.7f: uuid_v4 usa random_int (no determinista); se valida el ROUNDTRIP (is_uuid_v4 es determinista).
    let base = tmp("std_uuid");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "import std/uuid;\n\
         fn main() -> int {\n\
             print(uuid.is_uuid_v4(uuid.uuid_v4()));\n\
             print(uuid.is_uuid_v4(\"not-a-uuid\"));\n\
             print(uuid.uuid_v4().len());\n\
             0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con import std/uuid must salir 0\n{err}");
    assert!(out.contains("true"), "is_uuid_v4(uuid_v4()) roundtrip\n{out}");
    assert!(out.contains("false"), "is_uuid_v4 rejects basura\n{out}");
    assert!(out.contains("36"), "un uuid mide 36 chars\n{out}");
}

#[test]
fn ffi_llama_a_libm() {
    // M41.1: FFI. Un `extern "m" { … }` declara funciones de libm y se llaman como cualquier función.
    // Determinista (libm) → end-to-end por subproceso, motor de producto (VM).
    let base = tmp("ffi_libm");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "extern \"m\" {\n\
         \x20 fn sqrt(x: float) -> float;\n\
         \x20 fn pow(base: float, exp: float) -> float;\n\
         }\n\
         fn main() -> int {\n\
         \x20 if (sqrt(16.0) == 4.0 && pow(2.0, 10.0) == 1024.0) { print(\"ffi ok\"); } else { print(\"ffi mal\"); }\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con extern must salir 0\n{err}");
    assert!(out.contains("ffi ok"), "sqrt/pow de libm por FFI\n{out}");
}

#[test]
fn ffi_marshala_strings_a_char_ptr() {
    // M41.2: un `string` de raylang se pasa como `char*` (NUL-terminado) a una función C.
    let base = tmp("ffi_str");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "extern \"c\" { fn strlen(s: string) -> int; fn atoi(s: string) -> int; }\n\
         fn main() -> int {\n\
         \x20 print(strlen(\"hello mundo\"));\n\
         \x20 print(atoi(\"42\"));\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con string FFI must salir 0\n{err}");
    assert!(out.contains("11"), "strlen(\"hello mundo\")\n{out}");
    assert!(out.contains("42"), "atoi(\"42\")\n{out}");
}

#[test]
fn ffi_return_val_char_ptr_como_option() {
    // M41.3: un char* de retorno → Option<string> (None si NULL). strstr es determinista.
    let base = tmp("ffi_ret");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "extern \"c\" { fn strstr(h: string, n: string) -> Option<string>; }\n\
         fn main() -> int {\n\
         \x20 match (strstr(\"hello mundo\", \"mundo\")) {\n\
         \x20   Option.Some(s) => { print(s); }, Option.None => { print(\"none\"); },\n\
         \x20 }\n\
         \x20 match (strstr(\"hello\", \"zzz\")) {\n\
         \x20   Option.Some(s) => { print(s); }, Option.None => { print(\"none\"); },\n\
         \x20 }\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con return_val char* must salir 0\n{err}");
    assert!(out.contains("mundo"), "strstr encontró 'mundo'\n{out}");
    assert!(out.contains("none"), "strstr no encontrado → None\n{out}");
}

#[test]
fn ffi_anchura_int_y_puntero_opaco_como_u64() {
    // M41.4a: int → C int (32-bit, EOF=-1 corta el bucle); u64 → C long/size_t (64-bit); un FILE*
    // (puntero) se pasa como u64 (opaco). fopen/fgetc/fclose sobre un archivo con contenido conocido.
    let base = tmp("ffi_width");
    std::fs::write(base.join("data.txt"), "Hi!").unwrap();
    let data = base.join("data.txt");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        format!(
            "extern \"c\" {{\n\
             \x20 fn fopen(path: string, mode: string) -> u64;\n\
             \x20 fn fgetc(stream: u64) -> int;\n\
             \x20 fn fclose(stream: u64) -> int;\n\
             \x20 fn strlen(s: string) -> u64;\n\
             }}\n\
             fn main() -> int {{\n\
             \x20 print(strlen(\"hello mundo\") as int);\n\
             \x20 let h = fopen(\"{}\", \"r\");\n\
             \x20 if (h == 0) {{ print(\"no abrió\"); return 1; }}\n\
             \x20 var n = 0;\n\
             \x20 var c = fgetc(h);\n\
             \x20 while (c >= 0) {{ n = n + 1; c = fgetc(h); }}\n\
             \x20 fclose(h);\n\
             \x20 print(n);\n\
             \x20 0\n\
             }}\n",
            data.to_str().unwrap()
        ),
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con u64/int FFI must salir 0\n{err}");
    assert!(out.contains("11"), "strlen size_t (u64)\n{out}");
    assert!(out.contains("3"), "fgetc leyó 3 bytes y EOF (-1) cortó el loop\n{out}");
}

#[test]
fn ffi_ptr_opaco_y_option_ptr() {
    // M41.4b: tipo `ptr` opaco + Option<ptr> fallible. fopen(existe)→Some, fopen(no existe)→None.
    let base = tmp("ffi_ptr");
    std::fs::write(base.join("data.txt"), "Hi!").unwrap();
    let data = base.join("data.txt");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        format!(
            "extern \"c\" {{\n\
             \x20 fn fopen(path: string, mode: string) -> Option<ptr>;\n\
             \x20 fn fgetc(stream: ptr) -> int;\n\
             \x20 fn fclose(stream: ptr) -> int;\n\
             }}\n\
             fn leer(h: ptr) -> int {{\n\
             \x20 var n = 0; var c = fgetc(h);\n\
             \x20 while (c >= 0) {{ n = n + 1; c = fgetc(h); }}\n\
             \x20 fclose(h); n\n\
             }}\n\
             fn main() -> int {{\n\
             \x20 match (fopen(\"{}\", \"r\")) {{\n\
             \x20   Option.Some(h) => {{ print(leer(h)); }}, Option.None => {{ print(\"no\"); }},\n\
             \x20 }}\n\
             \x20 match (fopen(\"{}/no_existe\", \"r\")) {{\n\
             \x20   Option.Some(h) => {{ fclose(h); print(\"abrió?!\"); }}, Option.None => {{ print(\"None ok\"); }},\n\
             \x20 }}\n\
             \x20 0\n\
             }}\n",
            data.to_str().unwrap(),
            base.to_str().unwrap()
        ),
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run", file.to_str().unwrap()]);
    assert_eq!(code, 0, "run con ptr/Option<ptr> must salir 0\n{err}");
    assert!(out.contains("3"), "leyó 3 bytes por el handle ptr\n{out}");
    assert!(out.contains("None ok"), "fopen de file nonexistent → None\n{out}");
}

#[test]
fn dependency_por_path_local() {
    // M40.8a: `nombre = "path:<dir>"` consume un paquete-cápsula LOCAL sin git ni descarga (un paquete
    // adicional que no va en el binario). El paquete vive fuera del proyecto que lo importa.
    let base = tmp("pathdep");
    // El paquete-cápsula `saludo` (con mod.ray).
    std::fs::create_dir_all(base.join("pkgs/greeting")).unwrap();
    std::fs::write(
        base.join("pkgs/greeting/mod.ray"),
        "pub fn hello(n: string) -> string { \"hello, \" + n + \"!\" }\n",
    )
    .unwrap();
    // El proyecto que lo consume por ruta.
    std::fs::create_dir_all(base.join("app/src")).unwrap();
    std::fs::write(
        base.join("app/ray.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nsaludo = \"path:../pkgs/greeting\"\n",
    )
    .unwrap();
    std::fs::write(
        base.join("app/src/main.ray"),
        "import greeting;\nfn main() -> int { print(greeting.hello(\"mundo\")); 0 }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base.join("app"), &["run"]);
    assert_eq!(code, 0, "run con path-dep must salir 0\n{err}");
    assert!(out.contains("hello, mundo!"), "usó la función del package local\n{out}");
    // La path-dep NO se descarga: no debe crear `.ray-deps`.
    assert!(!base.join("app/.ray-deps").exists(), "one path-dep no se clona");
}

#[test]
fn package_net_jwt_via_path_dep() {
    // M40.8b: el paquete `net` (adicional, no embebido) consumido por path-dep. jwt es determinista
    // (firma+verifica) y se apoya en net/crypto (HMAC de producción vía ring, M43.5) + std/base64.
    let repo = env!("CARGO_MANIFEST_DIR");
    let base = tmp("net_jwt");
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"netapp\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(
        base.join("src/main.ray"),
        "import net/jwt;\n\
         fn main() -> int {\n\
         \x20 let tok = jwt.jwt_sign(\"secreto\".to_bytes(), \"{\\\"sub\\\":\\\"ada\\\"}\");\n\
         \x20 match (jwt.jwt_verify(\"secreto\".to_bytes(), tok)) {\n\
         \x20   Result.Ok(p) => { print(p); }, Result.Err(e) => { print(\"err\"); },\n\
         \x20 }\n\
         \x20 match (jwt.jwt_verify(\"mala\".to_bytes(), tok)) {\n\
         \x20   Result.Ok(p) => { print(\"¿?\"); }, Result.Err(e) => { print(\"rechazado\"); },\n\
         \x20 }\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run"]);
    assert_eq!(code, 0, "run con el package net must salir 0\n{err}");
    assert!(out.contains("{\"sub\":\"ada\"}"), "jwt_verify con la clave correcta → Ok(payload)\n{out}");
    assert!(out.contains("rechazado"), "jwt_verify con clave mala → Err\n{out}");
}

#[test]
fn package_net_hpack_roundtrip() {
    // M40.8c: el grupo HTTP del paquete net. hpack (HPACK) es determinista: codifica cabeceras y las
    // decodifica de vuelta. Con deps INTERNAS del paquete (http2_client → net/http2 + net/hpack).
    let repo = env!("CARGO_MANIFEST_DIR");
    let base = tmp("net_hpack");
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"h2\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(
        base.join("src/main.ray"),
        "import net/hpack;\n\
         fn main() -> int {\n\
         \x20 let enc = hpack.new_hpack();\n\
         \x20 let hs = [hpack.header(\":method\", \"GET\"), hpack.header(\":path\", \"/\")];\n\
         \x20 let blob = hpack.encode(enc, hs);\n\
         \x20 let dec = hpack.new_hpack();\n\
         \x20 match (hpack.decode(dec, blob)) {\n\
         \x20   Result.Ok(out) => { print(out.len()); }, Result.Err(e) => { print(\"err\"); },\n\
         \x20 }\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run"]);
    assert_eq!(code, 0, "run con net/hpack must salir 0\n{err}");
    assert!(out.contains("2"), "hpack encode+decode roundtrip de 2 headers\n{out}");
}

#[test]
fn package_net_hpack_decode_malformado() {
    // M78: HPACK decodifica bloques del PEER (no confiables). Un entero truncado, un string
    // sobredimensionado, un size-update > 4096 y una bomba de varint deben dar Err como VALOR,
    // no un trap que tumbe al cliente. El índice estático legítimo (0x82 = :method GET) sí decodifica.
    let repo = env!("CARGO_MANIFEST_DIR");
    let base = tmp("net_hpack_mal");
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"h2m\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(
        base.join("src/main.ray"),
        "import net/hpack;\n\
         fn probar(data: bytes) {\n\
         \x20 let h = hpack.new_hpack();\n\
         \x20 match (hpack.decode(h, data)) {\n\
         \x20   Result.Ok(_) => { print(\"ok\"); }, Result.Err(_) => { print(\"err\"); },\n\
         \x20 }\n\
         }\n\
         fn main() -> int {\n\
         \x20 probar(bytes_of([255, 255]));                    // entero truncado\n\
         \x20 probar(bytes_of([64, 10, 97, 98]));              // string sobredimensionado\n\
         \x20 probar(bytes_of([63, 233, 38]));                 // size-update a 5000 (> 4096)\n\
         \x20 probar(bytes_of([255, 255, 255, 255, 255, 255, 255, 255, 255, 255]));  // bomba de varint\n\
         \x20 probar(bytes_of([130]));                          // legítimo: :method GET\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run"]);
    assert_eq!(code, 0, "el client sobrevive a HPACK malformado (Err, sin crash)\n{err}");
    assert_eq!(out, "err\nerr\nerr\nerr\nok\n", "4 rechazos + 1 decodificación válida\n{out}");
}

#[test]
fn package_net_websocket_accept_key() {
    // M40.8d: transporte/servicios. websocket.accept_key es determinista (RFC 6455) y se apoya en
    // net/crypto (SHA-1 de producción vía ring, M43.5) + std/base64. Deps internas (dns → net/udp) validadas.
    let repo = env!("CARGO_MANIFEST_DIR");
    let base = tmp("net_ws");
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"ws\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(
        base.join("src/main.ray"),
        "import net/websocket;\n\
         fn main() -> int {\n\
         \x20 print(websocket.accept_key(\"dGhlIHNhbXBsZSBub25jZQ==\"));\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run"]);
    assert_eq!(code, 0, "run con net/websocket must salir 0\n{err}");
    assert!(out.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="), "handshake WebSocket RFC 6455\n{out}");
}

#[test]
fn package_net_observabilidad() {
    // M40.8e: time (formateo determinista) + metrics (Prometheus). log → net/time (dep interna).
    let repo = env!("CARGO_MANIFEST_DIR");
    let base = tmp("net_obs");
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(
        base.join("ray.toml"),
        format!("[package]\nname = \"obs\"\nversion = \"0.1.0\"\n\n[dependencies]\nnet = \"path:{repo}/packages/net\"\n"),
    )
    .unwrap();
    std::fs::write(
        base.join("src/main.ray"),
        "import net/time;\n\
         import net/metrics;\n\
         fn main() -> int {\n\
         \x20 print(time.to_iso8601(time.from_epoch_millis(1609459200000)));\n\
         \x20 let reg = metrics.registry();\n\
         \x20 metrics.register_counter(reg, \"hits\", \"total\");\n\
         \x20 metrics.inc(reg, \"hits\", metrics.no_labels());\n\
         \x20 metrics.inc(reg, \"hits\", metrics.no_labels());\n\
         \x20 print(metrics.render(reg));\n\
         \x20 0\n\
         }\n",
    )
    .unwrap();
    let (out, err, code) = ray(&base, &["run"]);
    assert_eq!(code, 0, "run con net/time+metrics must salir 0\n{err}");
    assert!(out.contains("2021-01-01T00:00:00Z"), "time formatea el epoch\n{out}");
    assert!(out.contains("hits 2"), "metrics account y renderiza Prometheus\n{out}");
}

#[test]
fn fuel_abort_un_loop_iter_infinito() {
    // M42.1: `ray run --fuel N` limita las instrucciones de la VM (para embeber raylang confinado).
    // Un bucle infinito aborta en vez de colgar; el error lo dice y el código de salida es 70.
    let base = tmp("fuel");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "fn main() -> int { var i = 0; while (true) { i = i + 1; } 0 }\n",
    )
    .unwrap();
    let (_out, err, code) = ray(&base, &["run", "--fuel", "200000", file.to_str().unwrap()]);
    assert_eq!(code, 70, "un loop infinito con fuel finito abort (EX_SOFTWARE)\n{err}");
    assert!(err.contains("fuel"), "el error menciona el límite de instrucciones\n{err}");
}

#[test]
fn tope_de_heap_abort_un_program_glotón() {
    // M42.2: `ray run --heap N` limita los objetos vivos de la VM (el otro recurso, junto al fuel).
    // Un programa que retiene objetos sin cesar aborta al rebasar el tope; el error lo dice, exit 70.
    let base = tmp("heap");
    let file = base.join("main.ray");
    std::fs::write(
        &file,
        "fn main() -> int { var xs: [[int]] = []; var i = 0; while (i < 1000000) { xs.push([i]); i = i + 1; } 0 }\n",
    )
    .unwrap();
    let (_out, err, code) = ray(&base, &["run", "--heap", "5000", file.to_str().unwrap()]);
    assert_eq!(code, 70, "un program glotón con tope de heap abort (EX_SOFTWARE)\n{err}");
    assert!(err.contains("heap cap"), "el error menciona el tope de heap\n{err}");
}
