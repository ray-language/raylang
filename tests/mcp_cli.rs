//! IDEAS §51 pieza B — el servidor MCP (`ray mcp`): el bucle escribir→verificar→corregir para
//! agentes LLM. Habla JSON-RPC 2.0 delimitado por línea sobre stdio; las tools que ejecutan
//! código van por subproceso del propio binario con fuel/heap/plazo. Aquí se pilota el servidor
//! REAL por stdin/stdout (como `lsp_cli.rs`) y se ejercitan las cinco tools de punta a punta.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Mcp {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("lanza ray mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Mcp { child, stdin, stdout }
    }

    /// Manda una petición (una línea) y devuelve la respuesta (una línea).
    fn ask(&mut self, req: &str) -> String {
        writeln!(self.stdin, "{req}").expect("escribe la petición");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("lee la respuesta");
        line.trim().to_string()
    }

    /// Llama una tool con `arguments` ya serializados y devuelve el TEXTO del resultado
    /// (des-escapando el JSON de una forma suficiente para los asserts).
    fn call(&mut self, id: u32, tool: &str, arguments: &str) -> String {
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}","arguments":{arguments}}}}}"#
        );
        let reply = self.ask(&req);
        // El texto está en result.content[0].text; para los asserts basta des-escapar \n, \" y \\.
        let marker = r#""text":""#;
        let start = reply.find(marker).map(|i| i + marker.len()).unwrap_or_else(|| panic!("sin text en: {reply}"));
        let rest = &reply[start..];
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => break,
                },
                other => out.push(other),
            }
        }
        out
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// El handshake y las cinco tools, de punta a punta sobre el binario real.
#[test]
fn handshake_y_las_cinco_tools() {
    let mut mcp = Mcp::start();

    // initialize + tools/list
    let init = mcp.ask(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#);
    assert!(init.contains(r#""name":"raylang""#), "initialize: {init}");
    let tools = mcp.ask(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
    for t in ["ray_check", "ray_run", "ray_test", "ray_fmt", "ray_doc"] {
        assert!(tools.contains(t), "tools/list trae {t}");
    }

    // ray_check: código válido → ok; inválido → el diagnóstico EXACTO con posición.
    let ok = mcp.call(2, "ray_check", r#"{"code":"fn main() { print(1); }"}"#);
    assert!(ok.contains("exit: 0"), "check ok: {ok}");
    let bad = mcp.call(3, "ray_check", r#"{"code":"fn main() { let x: int = \"s\"; }"}"#);
    assert!(bad.contains("type error at"), "check reporta el diagnóstico: {bad}");

    // ray_run: stdout + código de salida (= el int de main) + stdin pipeado.
    let run = mcp.call(4, "ray_run", r#"{"code":"fn main() -> int { print(\"hola\"); 7 }"}"#);
    assert!(run.contains("exit: 7") && run.contains("hola"), "run: {run}");
    let with_stdin = mcp.call(
        5,
        "ray_run",
        r#"{"code":"fn main() { match (input()) { Option.Some(l) => print(\"leido: \" + l), Option.None => print(\"nada\"), } }","stdin":"mundo\n"}"#,
    );
    assert!(with_stdin.contains("leido: mundo"), "run con stdin: {with_stdin}");

    // ray_run confinado: un bucle infinito lo corta el FUEL (no el plazo de pared).
    let bomb = mcp.call(6, "ray_run", r#"{"code":"fn main() { var i = 0; while (true) { i = i + 1; } }"}"#);
    assert!(!bomb.contains("exit: 0"), "el bucle infinito no sale 0: {bomb}");
    assert!(bomb.contains("fuel"), "el corte es por fuel: {bomb}");

    // ray_test: un test que pasa y uno que falla → exit 1 (hubo fallos, M101) + reporte.
    let tests = mcp.call(
        7,
        "ray_test",
        r#"{"code":"@test\nfn pasa() { assert_eq(2 + 2, 4); }\n@test\nfn falla() { assert_eq(1, 2); }\nfn main() { }"}"#,
    );
    assert!(tests.contains("exit: 1"), "exit 1 = hubo fallos: {tests}");

    // ray_fmt: devuelve el fuente canónico.
    let fmt = mcp.call(8, "ray_fmt", r#"{"code":"fn main(){print(1);}"}"#);
    assert!(fmt.contains("fn main() {"), "fmt canónico: {fmt}");

    // ray_doc: firma + doc — de un builtin de tabla Y de un envoltorio del prelude
    // (parse_int: el assert laxo de antes dejaba pasar el "is not a builtin").
    let doc = mcp.call(9, "ray_doc", r#"{"symbol":"parse_int"}"#);
    assert!(doc.contains("-> Option<int>"), "firma del prelude en ray_doc: {doc}");
    let builtin = mcp.call(12, "ray_doc", r#"{"symbol":"len"}"#);
    assert!(builtin.contains("length of a collection"), "doc de builtin: {builtin}");
    let unknown = mcp.call(10, "ray_doc", r#"{"symbol":"no_existe"}"#);
    assert!(unknown.contains("is not a builtin"), "símbolo desconocido: {unknown}");

    // resources/read: sirve el llms.txt embebido.
    let res = mcp.ask(r#"{"jsonrpc":"2.0","id":11,"method":"resources/read","params":{"uri":"raylang://llms.txt"}}"#);
    assert!(res.contains("raylang for LLMs"), "recurso llms.txt: {res}");
}
