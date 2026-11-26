//! rayhotel #495: en un binario nativo, un `scope` que falla cancela a sus hijas como la VM. Antes
//! el cuerpo que fallaba dejaba el marco del scope en la pila (nadie cancelaba a las hijas), y
//! `time.sleep` y las esperas de red no notaban la cancelación: la hija seguía escribiendo después
//! de que quien llamó la diera por terminada. Un `try_call` de la hija tampoco recupera la
//! cancelación. Los tiempos dejan 150 ms de margen entre el fallo y el paso siguiente de la hija.
use std::process::Command;

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// La VM y el binario nativo (con fibras y, si `threads`, también con un hilo por tarea) dan `want`.
fn vm_and_native(name: &str, source: &str, threads: bool, want: &str) {
    let d = std::env::temp_dir().join(format!("raylang_test_scope_cancel_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let src = d.join("main.ray");
    std::fs::write(&src, source).unwrap();
    let src = src.to_str().unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", src]).output().unwrap();
    assert!(o.status.success(), "VM: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(String::from_utf8_lossy(&o.stdout), want, "VM");
    if has_rustc() {
        let mut builds: Vec<(&str, Vec<&str>)> = vec![("fibras", vec![])];
        if threads {
            builds.push(("hilos", vec!["--without", "fibers,mimalloc,ahash"]));
        }
        for (label, extra) in builds {
            let bin = d.join(format!("prog_{label}"));
            let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", src, "--native", "-o", bin.to_str().unwrap()]).args(&extra).output().unwrap();
            assert!(b.status.success(), "build --native ({label}): {}", String::from_utf8_lossy(&b.stderr));
            let o = Command::new(&bin).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&o.stdout), want, "nativo ({label}): {}", String::from_utf8_lossy(&o.stderr));
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_failing_scope_cancels_its_sleeping_children() {
    vm_and_native(
        "sleep",
        r#"import std/time;

fn work(tag: string) -> int {
    var i = 0;
    while (i < 3) {
        time.sleep(300);
        print("${tag} step ${i}");
        i = i + 1;
    }
    0
}

// La hija espera dentro de su propio try_call: la cancelación no se recupera.
fn guarded(tag: string) -> int {
    let r = try_call(fn() -> int { work(tag) });
    print("${tag} survived try_call");
    0
}

fn body_fails() -> int {
    scope(fn() -> int {
        let _a = spawn(fn() -> int { work("body") });
        time.sleep(450);
        panic("body failed")
    })
}

fn child_fails() -> int {
    scope(fn() -> int {
        let _a = spawn(fn() -> int { work("sibling") });
        let _b = spawn(fn() -> int { time.sleep(450); panic("child failed") });
        0
    })
}

fn guarded_child() -> int {
    scope(fn() -> int {
        let _a = spawn(fn() -> int { guarded("guarded") });
        time.sleep(450);
        panic("guarded failed")
    })
}

fn main() {
    print(try_call(fn() -> int { body_fails() }));
    time.sleep(700);
    print(try_call(fn() -> int { child_fails() }));
    time.sleep(700);
    print(try_call(fn() -> int { guarded_child() }));
    time.sleep(700);
    // Tras un scope fallido, el marco no queda en la pila: una tarea suelta se une sin más.
    let t = spawn(fn() -> int { 7 });
    print(join(t));
}
"#,
        true,
        "body step 0\nResult.Err(body failed)\nsibling step 0\nResult.Err(child failed)\nguarded step 0\nResult.Err(guarded failed)\n7\n",
    );
}

/// Una hija aparcada en `tcp_accept` cuando el scope falla: la conexión que llega después la
/// despierta, y sale por la cancelación sin aceptarla. (Con un hilo por tarea la espera es una
/// llamada bloqueante del sistema y no hay punto de cancelación: solo se comprueba con fibras.)
#[test]
fn a_child_cancelled_while_waiting_for_a_connection_does_not_accept_it() {
    vm_and_native(
        "accept",
        r#"import std/net;
import std/time;

fn acceptor(l: int) -> int {
    let r = net.tcp_accept(l);
    print("accept returned");
    0
}

fn accept_case(l: int) -> int {
    scope(fn() -> int {
        let _a = spawn(fn() -> int { acceptor(l) });
        time.sleep(200);
        panic("accept case failed")
    })
}

fn main() {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    print(try_call(fn() -> int { accept_case(l) }));
    let c = net.tcp_connect("127.0.0.1", net.local_port(l));
    time.sleep(400);
    print("end");
}
"#,
        false,
        "Result.Err(accept case failed)\nend\n",
    );
}
