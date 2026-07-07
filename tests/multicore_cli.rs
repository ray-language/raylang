//! Pruebas del **scheduler M:N multicore** (M38.3b paso 3): la VM reparte fibras sobre N hilos worker
//! (`RAYLANG_THREADS=N`), con su heap thread-local por fibra (aislamiento por actores, M38.1) y el
//! scheduler compartido tras `Arc<Mutex<Shared>>`.
//!
//! Bajo M:N real el **orden** de ejecución no es determinista (a diferencia del default N=1, que sí lo es y
//! se prueba en `concurrency_cli.rs` contra salida exacta). Así que aquí se prueban programas cuyo
//! **resultado es independiente del scheduling** (sumas conmutativas, valores de retorno por tarea): validan
//! la **corrección** del multicore (sin races/cuelgues/pérdidas) y que el resultado coincide con la ejecución
//! en serie. Ver DESIGN §46.3/§46.5.

use std::io::Write;
use std::process::Command;

/// Escribe `src` en un `.ray` temporal y lo ejecuta en la VM con `RAYLANG_THREADS=n`. Devuelve
/// (stdout, stderr, código). Un `n > 1` habilita el pool multicore real.
fn run_threads(name: &str, src: &str, n: usize) -> (String, String, i32) {
    let mut path = std::env::temp_dir();
    path.push(format!("{name}.ray"));
    std::fs::File::create(&path).expect("crea").write_all(src.as_bytes()).expect("escribe");

    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .env("RAYLANG_THREADS", n.to_string())
        .arg("--vm")
        .arg(&path)
        .output()
        .expect("lanza raylang");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn suma_de_valores_de_tareas_en_paralelo() {
    // Ocho tareas independientes, cada una devuelve i*i; main las une y suma. El resultado
    // (0+1+4+9+16+25+36+49 = 140) NO depende del orden en que corran los 4 hilos.
    let src = r#"
fn cuadrado(x: int) -> int { x * x }
fn main() -> int {
    let t0 = spawn(fn() -> int { cuadrado(0) });
    let t1 = spawn(fn() -> int { cuadrado(1) });
    let t2 = spawn(fn() -> int { cuadrado(2) });
    let t3 = spawn(fn() -> int { cuadrado(3) });
    let t4 = spawn(fn() -> int { cuadrado(4) });
    let t5 = spawn(fn() -> int { cuadrado(5) });
    let t6 = spawn(fn() -> int { cuadrado(6) });
    let t7 = spawn(fn() -> int { cuadrado(7) });
    let s = join(t0) + join(t1) + join(t2) + join(t3)
          + join(t4) + join(t5) + join(t6) + join(t7);
    print(s);
    s
}
"#;
    let (out, err, code) = run_threads("mc_suma_tareas", src, 4);
    assert_eq!(out, "140\n", "stderr: {err}");
    assert_eq!(code, 140);
}

#[test]
fn muchos_productores_un_consumidor() {
    // Cuatro productores envían 100 valores cada uno a un canal compartido; main consume los 400 y suma.
    // La suma total (620200) es independiente del entrelazado; sólo el ORDEN de llegada varía (por eso no
    // se imprime cada valor, sólo el total). Valida que ningún envío se pierde bajo M:N.
    let src = r#"
fn productor(ch: Channel<int>, base: int) {
    var i = 1;
    while (i <= 100) { send(ch, base + i); i = i + 1; }
}
fn main() -> int {
    let ch: Channel<int> = Channel.new();
    spawn(fn() { productor(ch, 0); });
    spawn(fn() { productor(ch, 1000); });
    spawn(fn() { productor(ch, 2000); });
    spawn(fn() { productor(ch, 3000); });
    var total = 0;
    var got = 0;
    while (got < 400) {
        match (recv(ch)) {
            Option.Some(v) => { total = total + v; got = got + 1; },
            Option.None => { got = 400; },
        }
    }
    print(total);
    0
}
"#;
    let (out, err, code) = run_threads("mc_prod_cons", src, 4);
    assert_eq!(out, "620200\n", "stderr: {err}");
    assert_eq!(code, 0);
}

#[test]
fn scope_estructurado_en_paralelo() {
    // Structured concurrency (M12.3) bajo M:N: un scope lanza 6 tareas de cómputo y las une; el valor del
    // cuerpo del scope (la suma de i*100) es determinista pese al paralelismo.
    let src = r#"
fn main() -> int {
    let total = scope(fn() -> int {
        let a = spawn(fn() -> int { 100 });
        let b = spawn(fn() -> int { 200 });
        let c = spawn(fn() -> int { 300 });
        join(a) + join(b) + join(c)
    });
    print(total);
    0
}
"#;
    let (out, err, code) = run_threads("mc_scope", src, 4);
    assert_eq!(out, "600\n", "stderr: {err}");
    assert_eq!(code, 0);
}

#[test]
fn paralelo_coincide_con_serie() {
    // Meta-prueba: el MISMO programa da salida+código idénticos con 1 hilo (determinista) y con 4 (M:N).
    // Cubre que el multicore no cambia el resultado observable de un programa bien sincronizado.
    let src = r#"
fn trabajo(iters: int) -> int {
    var suma = 0;
    var i = 0;
    while (i < iters) { suma = suma + (i * i) % 7; i = i + 1; }
    suma
}
fn main() -> int {
    let t0 = spawn(fn() -> int { trabajo(200000) });
    let t1 = spawn(fn() -> int { trabajo(200000) });
    let t2 = spawn(fn() -> int { trabajo(200000) });
    let s = join(t0) + join(t1) + join(t2);
    print(s);
    0
}
"#;
    let (out1, _e1, code1) = run_threads("mc_meta", src, 1);
    let (out4, err4, code4) = run_threads("mc_meta", src, 4);
    assert_eq!(out1, out4, "N=1 vs N=4 difieren; stderr N=4: {err4}");
    assert_eq!(code1, code4);
    assert_eq!(out4, "1199997\n");
}

#[test]
fn default_es_multicore_sin_override() {
    // M38.4: sin `RAYLANG_THREADS` ni `--deterministic`, un programa CON `spawn` corre en multicore por
    // defecto (`available_parallelism()`). No podemos observar el paralelismo directamente, pero sí que el
    // camino por defecto produce el resultado correcto (independiente del scheduling). Un programa SIN
    // spawn caería a N=1 (probado indirectamente por todo el suite oráculo).
    let src = r#"
fn cuadrado(x: int) -> int { x * x }
fn main() -> int {
    let a = spawn(fn() -> int { cuadrado(6) });
    let b = spawn(fn() -> int { cuadrado(7) });
    print(join(a) + join(b));   // 36 + 49 = 85
    0
}
"#;
    let mut path = std::env::temp_dir();
    path.push("mc_default.ray");
    std::fs::File::create(&path).expect("crea").write_all(src.as_bytes()).expect("escribe");
    let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .env_remove("RAYLANG_THREADS") // asegura el DEFAULT (multicore para programas con spawn)
        .arg("--vm")
        .arg(&path)
        .output()
        .expect("lanza raylang");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "85\n");
    assert_eq!(out.status.code().unwrap_or(-1), 0);
}

#[test]
fn deterministic_reproducible() {
    // M38.4: `--deterministic` fuerza el scheduler M:1 (orden FIFO) → salida idéntica en cada corrida, aun
    // para un programa con varias fibras cuyo orden bajo multicore variaría.
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.new();
    spawn(fn() { var i = 0; while (i < 4) { send(ch, i); i = i + 1; } close(ch); });
    var seguir = true;
    while (seguir) {
        match (recv(ch)) {
            Option.Some(v) => print(v),
            Option.None => { seguir = false; },
        }
    }
    0
}
"#;
    let mut path = std::env::temp_dir();
    path.push("mc_det.ray");
    std::fs::File::create(&path).expect("crea").write_all(src.as_bytes()).expect("escribe");
    let corre = || {
        let out = Command::new(env!("CARGO_BIN_EXE_raylang"))
            .arg("--vm").arg("--deterministic").arg(&path)
            .output().expect("lanza raylang");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let a = corre();
    let b = corre();
    assert_eq!(a, "0\n1\n2\n3\n"); // orden FIFO reproducible
    assert_eq!(a, b);
}

#[test]
fn deadlock_detectado_en_paralelo() {
    // Un recv sobre un canal que nadie alimenta ni cierra: con todas las fibras bloqueadas y ningún worker
    // ejecutando (running == 0), el scheduler M:N debe detectar el deadlock (no colgarse indefinidamente).
    let src = r#"
fn main() -> int {
    let ch: Channel<int> = Channel.new();
    match (recv(ch)) {
        Option.Some(v) => print(v),
        Option.None => print(0),
    }
    0
}
"#;
    let (_out, err, code) = run_threads("mc_deadlock", src, 4);
    assert!(err.contains("deadlock"), "esperaba deadlock; stderr: {err}");
    assert_ne!(code, 0);
}
