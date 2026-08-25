//! M134 — el manejador de paquetes en ESCENARIO REAL: paquetes e índice alojados en GitHub
//! (la organización `ray-language`), consumidos anónimamente por `git+https`. Hasta aquí todo el
//! flujo git/registry se probaba solo con repos `git+file://` locales; este harness cierra ese
//! hueco contra la infraestructura real:
//!
//!   - github.com/ray-language/greeting  — la cápsula de demo (mod.ray + submódulo interno),
//!     tags v1.0.0 (URL ssh en el índice) y v1.0.1 (URL https, la de consumo anónimo).
//!   - github.com/ray-language/ray-index — el índice (greeting.toml con versiones + hashes).
//!
//! Cubre: dep directa `git+https@tag` (clone real + checkout + lockfile con commit+hash),
//! resolución POR NOMBRE contra el índice remoto (`ray add` elige la última no-yanked y verifica
//! el hash publicado), y reproducibilidad (caché borrada → re-descarga y el hash del lock
//! verifica). Red real → `#[ignore]`:
//!   cargo test --test deps_live_cli -- --ignored

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ray");

fn net_available() -> bool {
    Command::new("git")
        .args(["ls-remote", "https://github.com/ray-language/greeting", "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn project(name: &str, ray_toml: &str, main: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("ray_live_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("ray.toml"), ray_toml).unwrap();
    std::fs::write(base.join("src/main.ray"), main).unwrap();
    base
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0") // sin prompts: el fallo de red/credenciales es un error
        .output()
        .expect("lanza ray");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

const MAIN: &str = "import greeting;\n\nfn main() -> int {\n    print(greeting.hello(\"live\"));\n    print(greeting.hello_loud(\"live\"));\n    0\n}\n";

/// Dep directa `git+https://…@tag` contra GitHub real: descarga, corre, y el lockfile fija
/// commit + hash. Segundo run: desde la caché, sin red.
#[test]
#[ignore]
fn direct_git_https_dependency() {
    if !net_available() {
        eprintln!("saltando: sin red o GitHub inaccesible");
        return;
    }
    let dir = project(
        "direct",
        "[package]\nname = \"live\"\nversion = \"0.1.0\"\n\n[dependencies]\ngreeting = \"git+https://github.com/ray-language/greeting@v1.0.1\"\n",
        MAIN,
    );
    let (out, err, code) = ray(&dir, &["run"]);
    assert_eq!(code, 0, "run con dep git+https debe salir 0\n{err}");
    assert!(out.contains("hello, live!"), "la cápsula real responde\n{out}");
    assert!(out.contains("HELLO, LIVE!!!"), "el submódulo interno de la cápsula funciona\n{out}");
    let lock = std::fs::read_to_string(dir.join("ray.lock")).expect("ray.lock");
    assert!(lock.contains("https://github.com/ray-language/greeting"), "url en el lock\n{lock}");
    assert!(lock.contains("commit = "), "commit resuelto en el lock\n{lock}");
    assert!(lock.contains("hash = \"sha256:"), "hash de contenido en el lock\n{lock}");
    // Reproducibilidad: caché fuera → re-descarga y el hash del lock VERIFICA (mismo contenido).
    let _ = std::fs::remove_dir_all(dir.join(".ray-deps"));
    let (out2, err2, code2) = ray(&dir, &["run"]);
    assert_eq!(code2, 0, "re-resolución contra el lock\n{err2}");
    assert!(out2.contains("hello, live!"), "{out2}");
}

/// M135 (piloto de espejos): el paquete REAL `rpc` — espejado del monorepo a
/// github.com/ray-language/rpc por `tools/publish-packages.sh` — consumido POR NOMBRE desde el
/// índice, con un e2e de VERDAD: servidor rpc con apagado por canal + cliente en el mismo
/// proceso (fibras), sobre la superficie descargada de GitHub.
#[test]
#[ignore]
fn real_rpc_package_by_name_e2e() {
    if !net_available() {
        eprintln!("saltando: sin red o GitHub inaccesible");
        return;
    }
    let dir = project(
        "rpc",
        "[package]\nname = \"live\"\nversion = \"0.1.0\"\n\n[registry]\nindex = \"git+https://github.com/ray-language/ray-index@main\"\n",
        RPC_MAIN,
    );
    let (out, err, code) = ray(&dir, &["add", "rpc"]);
    assert_eq!(code, 0, "ray add rpc contra el índice\n{out}{err}");
    let (out, err, code) = ray(&dir, &["run"]);
    assert_eq!(code, 0, "e2e rpc sobre el paquete espejado\n{out}{err}");
    assert!(out.contains("rpc says pong"), "la llamada RPC responde\n{out}");
}

const RPC_MAIN: &str = r#"import rpc/rpc;
import std/time;
from std/json import Json;

fn main() -> int {
    let port = 36917;
    let stop: Channel<int> = Channel.new();
    spawn(fn() {
        let _ = rpc.serve_shutdown("127.0.0.1", port, stop, 2000, fn(req: rpc.Req) -> Result<Json, string> {
            if (req.method == "ping") { Result.Ok(Json.JStr("pong")) } else { Result.Err("unknown") }
        });
    });
    time.sleep(150);
    let c = match (rpc.connect("127.0.0.1", port)) {
        Result.Ok(x) => x,
        Result.Err(e) => {
            print("connect err " + e);
            return 1;
        },
    };
    match (rpc.call(c, "ping", Json.JNull)) {
        Result.Ok(j) => {
            match (j) {
                Json.JStr(s) => print("rpc says " + s),
                _ => print("unexpected shape"),
            }
        },
        Result.Err(e) => print("call err " + e),
    }
    rpc.disconnect(c);
    send(stop, 0);
    time.sleep(100);
    0
}
"#;

/// M135b (todos los paquetes espejados): dependencia TRANSITIVA real — `ray add db` descarga
/// `db` del espejo y su dep reescrita (`net = git+https://…/net@v0.1.0`) arrastra `net` de la
/// org; el e2e usa `db/bson` (que importa `net/time` transitivamente): roundtrip encode/decode.
#[test]
#[ignore]
fn real_db_package_transitive_net() {
    if !net_available() {
        eprintln!("saltando: sin red o GitHub inaccesible");
        return;
    }
    let dir = project(
        "db",
        "[package]\nname = \"live\"\nversion = \"0.1.0\"\n\n[registry]\nindex = \"git+https://github.com/ray-language/ray-index@main\"\n",
        DB_MAIN,
    );
    let (out, err, code) = ray(&dir, &["add", "db"]);
    assert_eq!(code, 0, "ray add db contra el índice\n{out}{err}");
    let (out, err, code) = ray(&dir, &["run"]);
    assert_eq!(code, 0, "e2e bson sobre el paquete espejado (+ net transitivo)\n{out}{err}");
    assert!(out.contains("bson roundtrip mundo"), "el roundtrip responde\n{out}");
    let lock = std::fs::read_to_string(dir.join("ray.lock")).unwrap();
    assert!(
        lock.contains("https://github.com/ray-language/net"),
        "net llegó TRANSITIVAMENTE al lock\n{lock}"
    );
}

const DB_MAIN: &str = r#"import db/bson;
from db/bson import Bson;

fn main() -> int {
    let doc = [bson.field("saludo", Bson.Str("mundo"))];
    match (bson.decode(bson.encode(doc))) {
        Result.Ok(fields) => {
            match (bson.get(fields, "saludo")) {
                Option.Some(v) => {
                    match (v) {
                        Bson.Str(s) => print("bson roundtrip " + s),
                        _ => print("unexpected shape"),
                    }
                },
                Option.None => print("missing field"),
            }
        },
        Result.Err(e) => print("decode err " + e),
    }
    0
}
"#;

/// Resolución POR NOMBRE contra el índice remoto real: `ray add greeting` elige la última
/// versión publicada (1.0.1, la de URL https) y la descarga verificando el hash del índice.
#[test]
#[ignore]
fn by_name_via_remote_index() {
    if !net_available() {
        eprintln!("saltando: sin red o GitHub inaccesible");
        return;
    }
    let dir = project(
        "index",
        "[package]\nname = \"live\"\nversion = \"0.1.0\"\n\n[registry]\nindex = \"git+https://github.com/ray-language/ray-index@main\"\n",
        MAIN,
    );
    let (out, err, code) = ray(&dir, &["add", "greeting"]);
    assert_eq!(code, 0, "ray add contra el índice remoto\n{out}{err}");
    let manifest = std::fs::read_to_string(dir.join("ray.toml")).unwrap();
    assert!(manifest.contains("greeting = \"^1.0.1\""), "elige la última versión\n{manifest}");
    let (out, err, code) = ray(&dir, &["run"]);
    assert_eq!(code, 0, "run tras el add\n{err}");
    assert!(out.contains("hello, live!"), "{out}");
    let lock = std::fs::read_to_string(dir.join("ray.lock")).unwrap();
    assert!(lock.contains("ref = \"v1.0.1\""), "el índice resolvió el tag\n{lock}");
}
