//! `ray mcp` — servidor MCP (Model Context Protocol) por stdio (IDEAS §51, pieza B).
//!
//! "El LSP para agentes": el bucle escribir→verificar→corregir que convierte la alucinación
//! de un LLM en iteración. **Cliente 100% externo** (como `lsp.rs`/`repl.rs`/`test_runner.rs`):
//! cero cambios en el core y cero dependencias de Cargo — MCP es JSON-RPC 2.0 con mensajes
//! delimitados por línea sobre stdin/stdout, y el JSON reusa el del LSP (`lsp::json`).
//!
//! Las tools que EJECUTAN código del invitado (`ray_run`/`ray_test`/`ray_check`/`ray_fmt`) van
//! por **subproceso del propio binario** (`current_exe`): aislamiento por proceso (la única
//! parada fiable del proyecto), stdout del invitado separado del canal MCP, y los límites de
//! embebido de M42 (`--fuel`, `--heap`) + un plazo de pared con kill. `ray_doc` es en-proceso
//! (consulta el registro de builtins). Los recursos `raylang://llms.txt` (contexto destilado,
//! pieza A) y `raylang://reference.md` (catálogo completo de firmas) van embebidos con
//! `include_str!`, como la stdlib; las **instructions** del `initialize` dirigen al cliente a
//! leerlos ANTES de asumir que una feature falta (el antídoto del patrón "propuse lo que ya
//! existía").

use crate::lsp::json::{self, Json};
use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Límite de instrucciones de la VM para `ray_run` (M42.1): ~1 s de CPU en release.
const FUEL: u64 = 100_000_000;

/// El fuel efectivo: `RAYLANG_MCP_FUEL` manda sobre el default. Existe por los tests: en un
/// build DEBUG sobre un runner lento, quemar los 100M puede tardar más que el plazo de pared
/// y la bomba de bucle moriría por timeout en vez de por fuel (carrera observada en CI);
/// bajando el fuel, el corte por fuel queda MUY por debajo del plazo en cualquier máquina.
fn fuel_limit() -> u64 {
    std::env::var("RAYLANG_MCP_FUEL").ok().and_then(|v| v.parse().ok()).unwrap_or(FUEL)
}
/// Tope de objetos vivos del heap para `ray_run` (M42.2).
const HEAP: u64 = 1_000_000;
/// Plazo de pared por tool que ejecuta un subproceso (un invitado bloqueado en red/stdin no
/// consume fuel): pasado el plazo, kill al hijo y se reporta el timeout.
const WALL_MS: u64 = 10_000;
/// Tope de salida reportada por flujo (stdout/stderr): el resto se trunca con aviso.
const MAX_OUT: usize = 64 * 1024;

/// El contexto destilado de la pieza A, embebido: el *resource* que sirve este servidor.
const LLMS_TXT: &str = include_str!("../llms.txt");
// El catálogo completo de firmas por módulo (pieza B): el mapa que evita "proponer" superficies
// que ya existen. Embebido como llms.txt — la stdlib no vive en disco del lado del cliente.
const REFERENCE_MD: &str = include_str!("../REFERENCE.md");

/// Arranca el servidor sobre stdin/stdout reales (lo llama `ray mcp`).
pub fn run() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve(stdin.lock(), stdout.lock());
}

/// El bucle del servidor: un mensaje JSON-RPC por línea; responde solo a peticiones (con `id`).
/// Genérico sobre los flujos para poder probarlo en memoria (como `lsp::serve`).
pub fn serve<R: BufRead, W: Write>(reader: R, mut writer: W) {
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = json::parse(line) else { continue };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
        let Some(id) = id else { continue }; // notificación (initialized, cancelled…): sin respuesta
        let reply = match method.as_str() {
            "initialize" => result(id, initialize_result()),
            "ping" => result(id, Json::Obj(vec![])),
            "tools/list" => result(id, Json::Obj(vec![("tools".into(), tools_list())])),
            "tools/call" => {
                let name = msg.get("params").and_then(|p| p.get("name")).and_then(|n| n.as_str()).unwrap_or("");
                let args = msg.get("params").and_then(|p| p.get("arguments")).cloned().unwrap_or(Json::Obj(vec![]));
                match call_tool(name, &args) {
                    Ok(text) => result(id, tool_result(&text, false)),
                    Err(text) => result(id, tool_result(&text, true)),
                }
            }
            "resources/list" => result(id, Json::Obj(vec![("resources".into(), resources_list())])),
            "resources/read" => {
                let uri = msg.get("params").and_then(|p| p.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                let body = match uri {
                    "raylang://llms.txt" => Some(("text/plain", LLMS_TXT)),
                    "raylang://reference.md" => Some(("text/markdown", REFERENCE_MD)),
                    _ => None,
                };
                match body {
                    Some((mime, text)) => {
                        result(id, Json::Obj(vec![("contents".into(), Json::Arr(vec![Json::Obj(vec![
                            ("uri".into(), Json::Str(uri.into())),
                            ("mimeType".into(), Json::Str(mime.into())),
                            ("text".into(), Json::Str(text.into())),
                        ])]))]))
                    }
                    None => error(id, -32602, &format!("unknown resource: {uri}")),
                }
            }
            _ => error(id, -32601, &format!("method not found: {method}")),
        };
        let _ = writeln!(writer, "{}", reply.serialize());
        let _ = writer.flush();
    }
}

/// La respuesta a `initialize`: versión de protocolo, capacidades (tools + resources), info e
/// **instructions** — el "system prompt" del servidor que los clientes incorporan. Nació de un
/// patrón real: tres reportes seguidos de un proyecto proponían features QUE YA EXISTÍAN
/// (inflate/M64, stdin_pipe/M100v3, FFI/M41) porque el modelo exploraba a ciegas; esto le dice
/// desde el primer mensaje dónde está el mapa.
fn initialize_result() -> Json {
    Json::Obj(vec![
        ("protocolVersion".into(), Json::Str("2024-11-05".into())),
        ("capabilities".into(), Json::Obj(vec![
            ("tools".into(), Json::Obj(vec![])),
            ("resources".into(), Json::Obj(vec![])),
        ])),
        ("serverInfo".into(), Json::Obj(vec![
            ("name".into(), Json::Str("raylang".into())),
            ("version".into(), Json::Str(env!("CARGO_PKG_VERSION").into())),
        ])),
        ("instructions".into(), Json::Str(
            "Before writing raylang or concluding that a feature is missing, read the \
             raylang://llms.txt resource (the distilled language context and stdlib map). \
             For exact signatures use the ray_doc tool; for the full per-module catalog read \
             raylang://reference.md. The stdlib is embedded in the toolchain (no std/ \
             directory on disk) — file searches will NOT find it. WORK LIKE A DEVELOPER, not \
             a snippet machine: for anything beyond a one-file experiment, create a real \
             project (ray.toml + src/), split modules, add Tier-2 packages with \
             [dependencies] (net = production webserver, web = the Express-style framework — \
             the recommended base for servers and desktop/mobile backends; rpc, db; discover \
             more in the public index, github.com/ray-language/ray-index, or with \
             'ray search'), and validate by passing 'path' to ray_check/ray_run/ray_test — \
             they run with the project as context, so multi-file imports and dependencies \
             resolve exactly like 'ray run'. The 'code' form of those tools is only for \
             quick self-contained experiments (isolated temp dir: project files and packages \
             do not resolve there). If you also have shell access, the ray binary itself \
             (ray run / ray test / ray build) is the same loop.".into(),
        )),
    ])
}

/// Un `result` JSON-RPC.
fn result(id: Json, value: Json) -> Json {
    Json::Obj(vec![
        ("jsonrpc".into(), Json::Str("2.0".into())),
        ("id".into(), id),
        ("result".into(), value),
    ])
}

/// Un `error` JSON-RPC.
fn error(id: Json, code: i64, message: &str) -> Json {
    Json::Obj(vec![
        ("jsonrpc".into(), Json::Str("2.0".into())),
        ("id".into(), id),
        ("error".into(), Json::Obj(vec![
            ("code".into(), Json::Num(code as f64)),
            ("message".into(), Json::Str(message.into())),
        ])),
    ])
}

/// El resultado de una tool: un bloque de texto (+ `isError` para fallos del ENVOLTORIO —
/// un diagnóstico del compilador es un resultado normal: es el feedback que el modelo necesita).
fn tool_result(text: &str, is_error: bool) -> Json {
    Json::Obj(vec![
        ("content".into(), Json::Arr(vec![Json::Obj(vec![
            ("type".into(), Json::Str("text".into())),
            ("text".into(), Json::Str(text.into())),
        ])])),
        ("isError".into(), Json::Bool(is_error)),
    ])
}

/// El esquema `{code: string}` que comparten las tools de código.
fn code_schema(desc: &str) -> Json {
    Json::Obj(vec![
        ("type".into(), Json::Str("object".into())),
        ("properties".into(), Json::Obj(vec![(
            "code".into(),
            Json::Obj(vec![
                ("type".into(), Json::Str("string".into())),
                ("description".into(), Json::Str(desc.into())),
            ]),
        ), (
            "path".into(),
            Json::Obj(vec![
                ("type".into(), Json::Str("string".into())),
                ("description".into(), Json::Str(
                    "Instead of 'code': a .ray file (or project directory) ON DISK. Runs with \
                     the file's project as context (nearest ray.toml upward): multi-file \
                     imports and [dependencies] packages resolve — use this for real projects."
                        .into(),
                )),
            ]),
        )])),
        ("required".into(), Json::Arr(vec![])),
    ])
}

/// Una definición de tool para `tools/list`.
fn tool(name: &str, desc: &str, schema: Json) -> Json {
    Json::Obj(vec![
        ("name".into(), Json::Str(name.into())),
        ("description".into(), Json::Str(desc.into())),
        ("inputSchema".into(), schema),
    ])
}

/// Las cinco tools (IDEAS §51): check / run / test / fmt / doc.
fn tools_list() -> Json {
    let run_schema = Json::Obj(vec![
        ("type".into(), Json::Str("object".into())),
        ("properties".into(), Json::Obj(vec![
            ("code".into(), Json::Obj(vec![
                ("type".into(), Json::Str("string".into())),
                ("description".into(), Json::Str("A complete raylang program (must define fn main).".into())),
            ])),
            ("stdin".into(), Json::Obj(vec![
                ("type".into(), Json::Str("string".into())),
                ("description".into(), Json::Str("Text piped to the program's stdin (optional).".into())),
            ])),
            ("path".into(), Json::Obj(vec![
                ("type".into(), Json::Str("string".into())),
                ("description".into(), Json::Str(
                    "Instead of 'code': a .ray file (or project directory) ON DISK, run with \
                     its project as context (multi-file imports and [dependencies] resolve)."
                        .into(),
                )),
            ])),
        ])),
        ("required".into(), Json::Arr(vec![])),
    ]);
    let doc_schema = Json::Obj(vec![
        ("type".into(), Json::Str("object".into())),
        ("properties".into(), Json::Obj(vec![(
            "symbol".into(),
            Json::Obj(vec![
                ("type".into(), Json::Str("string".into())),
                ("description".into(), Json::Str("A builtin, prelude or module function: 'len', 'parse_int', 'json.parse', 'kv.set' — and with 'path', a project/package symbol like 'framework.listen' or 'web.listen'.".into())),
            ]),
        ), (
            "path".into(),
            Json::Obj(vec![
                ("type".into(), Json::Str("string".into())),
                ("description".into(), Json::Str("Optional: a project file/directory — also resolves symbols from that project's own modules and its .ray-deps packages.".into())),
            ]),
        )])),
        ("required".into(), Json::Arr(vec![Json::Str("symbol".into())])),
    ]);
    Json::Arr(vec![
        tool(
            "ray_check",
            "Type-check raylang without running it: pass 'code' (a self-contained snippet) or 'path' (a real file/project on disk — imports and [dependencies] resolve). Returns 'ok' or the exact compiler diagnostics (up to 20, with positions). Fix what it reports.",
            code_schema("A complete raylang source file."),
        ),
        tool(
            "ray_run",
            "Run raylang on the VM (sandboxed: instruction fuel, heap cap and a 10 s wall clock): pass 'code' (snippet) or 'path' (a real file/project — the way to validate multi-file work). Returns exit code, stdout and stderr; the exit code is main's int return.",
            run_schema,
        ),
        tool(
            "ray_test",
            "Run @test functions: pass 'code' (snippet) or 'path' (a project — runs its whole suite like 'ray test'). Reports each test and a summary; the exit code is the number of failures.",
            code_schema("A raylang source file with @test functions."),
        ),
        tool(
            "ray_fmt",
            "Format a raylang program canonically. Returns the formatted source.",
            code_schema("A raylang source file to format."),
        ),
        tool(
            "ray_doc",
            "Signature and documentation of a raylang symbol: builtins, prelude, std/* module functions and trait methods — and, given 'path', the project's own modules and its .ray-deps packages (e.g. 'web.listen'). Kills API hallucination: check before calling anything you are not sure exists.",
            doc_schema,
        ),
    ])
}

/// Los recursos publicados: el contexto destilado "raylang for LLMs" (pieza A) y el catálogo
/// completo de firmas REFERENCE.md (pieza B).
fn resources_list() -> Json {
    Json::Arr(vec![
        Json::Obj(vec![
            ("uri".into(), Json::Str("raylang://llms.txt".into())),
            ("name".into(), Json::Str("raylang for LLMs".into())),
            ("description".into(), Json::Str("Distilled context for writing correct raylang: the delta vs Rust, canonical forms, exact error messages. Read this FIRST.".into())),
            ("mimeType".into(), Json::Str("text/plain".into())),
        ]),
        Json::Obj(vec![
            ("uri".into(), Json::Str("raylang://reference.md".into())),
            ("name".into(), Json::Str("raylang reference".into())),
            ("description".into(), Json::Str("The full signature catalog, module by module (stdlib, builtins, prelude). Check here before assuming a feature is missing — the stdlib is embedded, file searches will not find it.".into())),
            ("mimeType".into(), Json::Str("text/markdown".into())),
        ]),
    ])
}

/// Despacha una tool. `Ok(texto)` es un resultado (incluidos diagnósticos del compilador);
/// `Err(texto)` es un fallo del envoltorio (argumento ausente, timeout, E/S).
fn call_tool(name: &str, args: &Json) -> Result<String, String> {
    // check/run/test aceptan `code` (snippet autocontenido en un tmp) O `path` (dogfood
    // raydesk: un archivo/proyecto REAL en disco — imports multi-archivo y paquetes de
    // [dependencies] resuelven contra su ray.toml, como haría `ray run`).
    let code = || args.get("code").and_then(|c| c.as_str()).ok_or("missing argument: pass 'code' (a self-contained program) or 'path' (a .ray file or project dir on disk)".to_string());
    let path = args.get("path").and_then(|c| c.as_str());
    match name {
        "ray_check" => match path {
            Some(p) => run_self_at(&["build"], p, None),
            None => run_self(&["build"], code()?, None),
        },
        "ray_run" => {
            let stdin = args.get("stdin").and_then(|s| s.as_str()).map(str::to_string);
            let fuel = fuel_limit().to_string();
            let heap = HEAP.to_string();
            let run_args = ["run", "--deterministic", "--fuel", &fuel, "--heap", &heap];
            match path {
                Some(p) => run_self_at(&run_args, p, stdin),
                None => run_self(&run_args, code()?, stdin),
            }
        }
        "ray_test" => match path {
            Some(p) => run_self_at(&["test"], p, None),
            None => run_self(&["test"], code()?, None),
        },
        "ray_fmt" => run_self(&["fmt"], code()?, None),
        "ray_doc" => {
            let symbol = args.get("symbol").and_then(|s| s.as_str()).ok_or("missing required argument 'symbol'")?;
            Ok(doc_text_at(symbol, args.get("path").and_then(|p| p.as_str())))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// Firma + doc de un builtin (registro único, `src/builtins.rs`) o — *fallback* — de un
/// envoltorio del **prelude** (`parse_int`, `read_int`, `assert_eq`, `sort`…: funciones
/// raylang ordinarias, no filas de la tabla; cazado probando el MCP con Claude Code real).
#[cfg(test)] // producción entra por doc_text_at (call_tool); los tests usan la forma corta
fn doc_text(symbol: &str) -> String {
    doc_text_at(symbol, None)
}

fn doc_text_at(symbol: &str, path: Option<&str>) -> String {
    let sig = crate::builtins::signature(symbol)
        .map(|(params, ret)| format!("{}({}) -> {}", symbol, params.join(", "), ret));
    let doc = crate::builtins::doc(symbol);
    match (sig, doc) {
        (Some(s), Some(d)) => format!("{s}\n{d}"),
        (Some(s), None) => s,
        (None, Some(d)) => format!("{symbol}: {d}"),
        (None, None) => prelude_doc_text(symbol)
            .or_else(|| std_doc_text(symbol))
            .or_else(|| path.and_then(|p| project_doc_text(symbol, p)))
            .unwrap_or_else(|| format!(
                "'{symbol}' is not a builtin, a prelude function, nor a public std/* function. \
                 For module functions use 'module.function' (e.g. 'json.parse', 'regex.find_all'); \
                 see the stdlib map in the raylang://llms.txt resource and the full \
                 catalog in raylang://reference.md."
            )),
    }
}


/// Firma + doc de una función PÚBLICA de un módulo `std/*` embebido (cazado probando el MCP:
/// `ray_doc` no cubría `json.parse`/`regex.find_all`…). Acepta `modulo.funcion` (p. ej.
/// `json.parse`) y, sin punto, busca el nombre en TODOS los módulos embebidos (primer match,
/// prefijado con su módulo). La firma sale del AST parseado del fuente embebido; la doc, de las
/// `///` contiguas encima del `pub fn` en el texto.
fn std_doc_text(symbol: &str) -> Option<String> {
    let (module, func) = match symbol.split_once('.') {
        Some((m, f)) => (Some(m.to_string()), f.to_string()),
        None => (None, symbol.to_string()),
    };
    let candidates: Vec<(String, &'static str)> = match &module {
        Some(m) => ["std/", "std/collections/"]
            .iter()
            .filter_map(|p| {
                let name = format!("{p}{m}");
                crate::stdlib::embedded(&name).map(|src| (name, src))
            })
            .collect(),
        None => crate::stdlib::names().iter().filter_map(|n| {
            crate::stdlib::embedded(n).map(|src| (n.to_string(), src))
        }).collect(),
    };
    for (mod_name, src) in candidates {
        if let Some(text) = source_symbol_doc(&mod_name, src, &func) {
            return Some(text);
        }
    }
    None
}

/// El núcleo del doc por-fuente: firma (fn pública o método de trait público) + las `///`
/// contiguas. Lo comparten la stdlib embebida y los fuentes de un PROYECTO (modo `path`).
fn source_symbol_doc(mod_name: &str, src: &str, func: &str) -> Option<String> {
    {
        let Ok(tokens) = crate::lexer::lex(src) else { return None };
        let Ok(prog) = crate::parser::parse(tokens) else { return None };
        // Función pública top-level o, si no, un MÉTODO DE TRAIT público (dogfood raydesk:
        // la superficie de std/kv son los métodos de StoreOps — `s.get(k)`, `s.set(k, v)` —
        // y ray_doc los negaba; la firma sale del MethodSig, la doc del mismo escaneo de ///).
        let sig = match prog.functions.iter().find(|f| f.is_pub && f.name == func) {
            Some(f) => fn_signature(f),
            None => {
                let (t, m) = prog.traits.iter().filter(|t| t.is_pub).find_map(|t| {
                    t.methods.iter().find(|m| m.name == func).map(|m| (t, m))
                })?;
                let params: Vec<String> = m
                    .params
                    .iter()
                    .map(|p| match &p.ty {
                        crate::ast::Type::SelfType => p.name.clone(),
                        ty => format!("{}: {ty}", p.name),
                    })
                    .collect();
                let ret = match &m.return_type {
                    crate::ast::Type::Unit => String::new(),
                    ty => format!(" -> {ty}"),
                };
                format!(
                    "{}({}){ret}  [trait {} — method-call style: receiver.{}(…)]",
                    m.name,
                    params.join(", "),
                    t.name,
                    m.name
                )
            }
        };
        // Las `///` contiguas encima del `pub fn <func>(` en el fuente del módulo.
        let lines: Vec<&str> = src.lines().collect();
        let mut doc_lines: Vec<&str> = Vec::new();
        if let Some(i) = lines.iter().position(|l| {
            let t = l.trim_start();
            t.starts_with(&format!("pub fn {func}(")) || t.starts_with(&format!("fn {func}("))
        }) {
            let mut j = i;
            while j > 0 && (lines[j - 1].trim_start().starts_with("///") || lines[j - 1].trim_start().starts_with("//")) {
                j -= 1;
                let t = lines[j].trim_start();
                if t.starts_with("///") {
                    doc_lines.insert(0, t.trim_start_matches("///").trim());
                }
            }
        }
        let head = format!("{mod_name}: {sig}");
        Some(if doc_lines.is_empty() { head } else { format!("{head}\n{}", doc_lines.join(" ")) })
    }
}


/// La firma legible de una función del AST (`nombre<T: A + B>(params) -> ret`). La comparten el
/// fallback del prelude y el de los módulos `std/*`.
fn fn_signature(f: &crate::ast::Function) -> String {
    let tparams = if f.type_params.is_empty() {
        String::new()
    } else {
        let parts: Vec<String> = f.type_params.iter().map(|tp| {
            let traits: Vec<&str> =
                f.bounds.iter().filter(|(p, _)| p == tp).map(|(_, t)| t.as_str()).collect();
            if traits.is_empty() { tp.clone() } else { format!("{}: {}", tp, traits.join(" + ")) }
        }).collect();
        format!("<{}>", parts.join(", "))
    };
    let params: Vec<String> = f.params.iter().map(|p| format!("{}: {}", p.name, p.ty)).collect();
    let ret = match &f.return_type {
        crate::ast::Type::Unit => String::new(),
        t => format!(" -> {t}"),
    };
    format!("{}{tparams}({}){ret}", f.name, params.join(", "))
}

/// M150 (dogfood raydesk): doc de símbolos de un PROYECTO — sus módulos y los paquetes de
/// `.ray-deps` (p. ej. `framework.listen` o `web.listen` con `path` apuntando al proyecto):
/// el agente aprendía la API del framework leyendo el fuente a mano. Busca archivos cuyo STEM
/// sea el módulo pedido, o cualquier `.ray` dentro de un directorio con ese nombre (el nombre
/// de PAQUETE: `web.listen` encuentra `.ray-deps/web/framework.ray`). Cap de archivos para no
/// escanear un monorepo entero.
fn project_doc_text(symbol: &str, path: &str) -> Option<String> {
    let (module, func) = match symbol.split_once('.') {
        Some((m, f)) => (Some(m.to_string()), f.to_string()),
        None => (None, symbol.to_string()),
    };
    let anchor = std::path::Path::new(path);
    let anchor = anchor.canonicalize().ok()?;
    let dir = if anchor.is_dir() { anchor.clone() } else { anchor.parent()?.to_path_buf() };
    let root = match crate::manifest::Manifest::find(&dir) {
        Some(toml) => toml.parent()?.to_path_buf(),
        None => dir,
    };
    // Los fuentes del proyecto + los de sus dependencias descargadas.
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(d) = pending.pop() {
        if files.len() > 400 {
            break; // cap: esto es doc, no un indexador
        }
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !name.starts_with('.') || name == ".ray-deps" {
                    pending.push(p);
                }
            } else if name.ends_with(".ray") && !name.starts_with('.') {
                files.push(p);
            }
        }
    }
    files.sort();
    for f in &files {
        let stem = f.file_stem()?.to_string_lossy().into_owned();
        let matches_module = match &module {
            None => true,
            Some(m) => {
                // stem del archivo == módulo, o el archivo vive bajo un directorio llamado
                // como el módulo (nombre de paquete).
                stem == *m || f.parent().is_some_and(|d| d.file_name().is_some_and(|n| n.to_string_lossy() == **m))
            }
        };
        if !matches_module {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(f) else { continue };
        let label = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().into_owned();
        if let Some(text) = source_symbol_doc(&label, &src, &func) {
            return Some(text);
        }
    }
    None
}

/// Firma (del AST del prelude) + doc (las `///` del fuente) de una función del prelude.
fn prelude_doc_text(symbol: &str) -> Option<String> {
    if symbol.starts_with("__") || symbol.contains('#') {
        return None; // primitivos internos / métodos manglados: no son superficie
    }
    let funcs = crate::prelude::functions();
    let f = funcs.iter().find(|f| f.name == symbol)?;
    let sig = fn_signature(f);
    // Las líneas `///` contiguas encima del `fn <symbol>(` en el fuente del prelude.
    let mut doc_lines: Vec<&str> = Vec::new();
    let lines: Vec<&str> = crate::prelude::SOURCE.lines().collect();
    if let Some(i) = lines.iter().position(|l| l.trim_start().starts_with(&format!("fn {symbol}("))) {
        let mut j = i;
        while j > 0 && lines[j - 1].trim_start().starts_with("///") {
            j -= 1;
            doc_lines.insert(0, lines[j].trim_start().trim_start_matches("///").trim());
        }
    }
    if doc_lines.is_empty() {
        Some(sig)
    } else {
        Some(format!("{sig}\n{}", doc_lines.join(" ")))
    }
}

/// Modo PROYECTO (dogfood raydesk): corre `current_exe() <args> <ruta>` con el cwd en la RAÍZ
/// del proyecto de `path` (el ray.toml más cercano hacia arriba; sin él, el directorio del
/// archivo) — imports multi-archivo y paquetes de [dependencies] resuelven como en un
/// `ray run` real. Mismo plazo de pared y mismos límites que el modo snippet.
fn run_self_at(args: &[&str], path: &str, stdin: Option<String>) -> Result<String, String> {
    let target = std::path::Path::new(path);
    if !target.exists() {
        return Err(format!("path not found: {path}"));
    }
    let target = target.canonicalize().map_err(|e| format!("could not resolve '{path}': {e}"))?;
    let anchor = if target.is_dir() { target.clone() } else { target.parent().unwrap_or(&target).to_path_buf() };
    let cwd = match crate::manifest::Manifest::find(&anchor) {
        Some(toml) => toml.parent().unwrap_or(&anchor).to_path_buf(),
        None => anchor,
    };
    // Un DIRECTORIO no viaja como argumento (en `ray test` sería un filtro; en run/build, un
    // error): el subcomando resuelve la entrada por defecto desde el cwd del proyecto.
    let arg_target = if target.is_dir() { None } else { Some(target.as_path()) };
    run_child(args, arg_target, &cwd, stdin, false)
}

/// Escribe `code` a un temporal, corre `current_exe() <args> <archivo>` con plazo de pared,
/// y reporta `exit` + stdout + stderr (truncados). El invitado jamás toca el stdout del MCP.
fn run_self(args: &[&str], code: &str, stdin: Option<String>) -> Result<String, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    // Subdirectorio PROPIO por proceso, no el temp_dir compartido a pelo: aísla los snippets de
    // los residuos de otros procesos/tests en el /tmp compartido (histórico: la regeneración de
    // templates de M55 —hoy compilación en memoria, M102— escaneaba el directorio del archivo y un
    // `.ray.html` roto ajeno abortaba el check del snippet).
    let dir = std::env::temp_dir().join(format!("ray_mcp_{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create temp dir: {e}"))?;
    let path = dir.join(format!("snippet_{n}.ray"));
    std::fs::write(&path, code).map_err(|e| format!("could not write temp file: {e}"))?;
    run_child(args, Some(&path), &dir, stdin, true)
}

/// El corredor común de ambos modos: lanza `current_exe() <args> <target>` con `cwd`, plazo de
/// pared, drenado de pipes en hilos y truncado de salida. `cleanup` borra el target (snippet).
fn run_child(
    args: &[&str],
    path: Option<&std::path::Path>,
    cwd: &std::path::Path,
    stdin: Option<String>,
    cleanup: bool,
) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.args(args);
    if let Some(p) = path {
        cmd.arg(p);
    }
    cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|e| format!("could not spawn: {e}"));
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            if cleanup && let Some(p) = path {
                let _ = std::fs::remove_file(p);
            }
            return Err(e);
        }
    };
    // El stdin del invitado: el texto dado, o cerrado de inmediato (input() → None).
    if let Some(mut si) = child.stdin.take() {
        if let Some(text) = &stdin {
            let _ = si.write_all(text.as_bytes());
        }
        drop(si);
    }
    // Drenar stdout/stderr en hilos (evita el deadlock del pipe lleno) mientras corre el plazo.
    let mut out_pipe = child.stdout.take().unwrap();
    let mut err_pipe = child.stderr.take().unwrap();
    let out_h = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let err_h = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + Duration::from_millis(WALL_MS);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break None,
        }
    };
    let stdout = out_h.join().unwrap_or_default();
    let stderr = err_h.join().unwrap_or_default();
    if cleanup && let Some(p) = path {
        let _ = std::fs::remove_file(p);
    }
    if timed_out {
        return Err(format!(
            "timeout: the program did not finish within {} s (killed). stdout so far:\n{}",
            WALL_MS / 1000,
            clip(&stdout)
        ));
    }
    let code = status.and_then(|s| s.code()).unwrap_or(-1);
    let mut text = format!("exit: {code}");
    if !stdout.is_empty() {
        text.push_str("\n--- stdout ---\n");
        text.push_str(&clip(&stdout));
    }
    if !stderr.is_empty() {
        text.push_str("\n--- stderr ---\n");
        text.push_str(&clip(&stderr));
    }
    if stdout.is_empty() && stderr.is_empty() && code == 0 {
        text.push_str("\nok");
    }
    Ok(text)
}

/// UTF-8 con pérdida y truncado a `MAX_OUT` (con aviso), sin salto final redundante.
fn clip(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let s = s.trim_end_matches('\n');
    if s.len() > MAX_OUT {
        let mut cut = MAX_OUT;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}\n... [truncated at {} KiB]", &s[..cut], MAX_OUT / 1024)
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Manda las líneas dadas al servidor en memoria y devuelve las respuestas parseadas.
    fn roundtrip(lines: &[&str]) -> Vec<Json> {
        let input = lines.join("\n");
        let mut out = Vec::new();
        serve(Cursor::new(input), &mut out);
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| json::parse(l).expect("respuesta JSON válida"))
            .collect()
    }

    #[test]
    fn initialize_y_tools_list() {
        let replies = roundtrip(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        ]);
        assert_eq!(replies.len(), 2, "la notificación no se responde");
        let init = replies[0].get("result").expect("result de initialize");
        assert_eq!(
            init.get("serverInfo").and_then(|s| s.get("name")).and_then(|n| n.as_str()),
            Some("raylang")
        );
        let tools = replies[1]
            .get("result").and_then(|r| r.get("tools")).and_then(|t| t.as_array())
            .expect("lista de tools");
        let names: Vec<_> = tools.iter().filter_map(|t| t.get("name").and_then(|n| n.as_str())).collect();
        assert_eq!(names, vec!["ray_check", "ray_run", "ray_test", "ray_fmt", "ray_doc"]);
    }

    #[test]
    fn initialize_carries_the_reading_instructions() {
        // El antídoto del patrón "propuse lo que ya existía": las instructions del initialize
        // dirigen al cliente a llms.txt/reference.md ANTES de explorar a ciegas.
        let replies = roundtrip(&[r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#]);
        let text = replies[0]
            .get("result").and_then(|r| r.get("instructions")).and_then(|i| i.as_str())
            .expect("instructions presentes");
        assert!(text.contains("raylang://llms.txt"), "apunta al contexto destilado");
        assert!(text.contains("raylang://reference.md"), "apunta al catálogo");
        assert!(text.contains("embedded"), "advierte que la stdlib no está en disco");
    }

    #[test]
    fn resource_reference_md() {
        let replies = roundtrip(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"raylang://reference.md"}}"#,
        ]);
        let uris: Vec<_> = replies[0]
            .get("result").and_then(|r| r.get("resources")).and_then(|a| a.as_array())
            .expect("lista de resources")
            .iter()
            .filter_map(|r| r.get("uri").and_then(|u| u.as_str()))
            .collect();
        assert_eq!(uris, vec!["raylang://llms.txt", "raylang://reference.md"]);
        let text = replies[1]
            .get("result").and_then(|r| r.get("contents")).and_then(|a| a.as_array())
            .and_then(|a| a.first()).and_then(|c| c.get("text")).and_then(|t| t.as_str())
            .expect("contenido del recurso");
        assert!(text.contains("std/inflate"), "sirve el REFERENCE.md embebido");
    }

    #[test]
    fn resource_llms_txt() {
        let replies = roundtrip(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"raylang://llms.txt"}}"#,
        ]);
        let uri = replies[0]
            .get("result").and_then(|r| r.get("resources")).and_then(|a| a.as_array())
            .and_then(|a| a.first()).and_then(|r| r.get("uri")).and_then(|u| u.as_str());
        assert_eq!(uri, Some("raylang://llms.txt"));
        let text = replies[1]
            .get("result").and_then(|r| r.get("contents")).and_then(|a| a.as_array())
            .and_then(|a| a.first()).and_then(|c| c.get("text")).and_then(|t| t.as_str())
            .expect("contenido del recurso");
        assert!(text.contains("# raylang for LLMs"), "sirve el llms.txt embebido");
    }

    /// Métodos de TRAIT de un módulo std (dogfood raydesk: la superficie de std/kv son los
    /// métodos de StoreOps y ray_doc los negaba con el mensaje genérico).
    #[test]
    fn ray_doc_covers_std_trait_methods() {
        let d = doc_text("kv.set");
        assert!(d.contains("std/kv") && d.contains("trait StoreOps"), "{d}");
        assert!(d.contains("method-call style"), "{d}");
        let d = doc_text("kv.get");
        assert!(d.contains("Option<bytes>"), "{d}");
    }

    /// El fallback del prelude (cazado con Claude Code real: `parse_int` no es fila de la
    /// tabla de builtins — es un envoltorio raylang — y `ray_doc` lo negaba).
    #[test]
    fn ray_doc_covers_std_modules() {
        // Cazado probando el MCP con Claude Code real (jsondeserialize con std/json): `ray_doc` no
        // cubría las funciones públicas de los módulos std/* embebidos. Formas: `modulo.funcion` y
        // el nombre a secas (búsqueda en todos los módulos, primer match con su módulo).
        let d = doc_text("json.parse");
        assert!(d.contains("std/json") && d.contains("Result<Json, string>"), "{d}");
        let d = doc_text("regex.find_all");
        assert!(d.contains("find_all(pattern: string, text: string) -> [string]"), "{d}");
        let d = doc_text("find_all"); // sin módulo → lo encuentra igual
        assert!(d.contains("std/regex"), "{d}");
        let d = doc_text("crypto.x25519_public_key");
        assert!(d.contains("std/crypto") && d.contains("Option<bytes>"), "{d}");
        let d = doc_text("hkdf_sha256");
        assert!(d.contains("std/crypto") && d.contains("HKDF"), "{d}");
        // M115.1: escritura binaria sobre handle + fsync.
        let d = doc_text("fs.write_bytes");
        assert!(d.contains("std/fs") && d.contains("Binary twin of `write`"), "{d}");
        let d = doc_text("fs.sync");
        assert!(d.contains("std/fs") && d.contains("fsync"), "{d}");
        // M115.2: candados consultivos.
        let d = doc_text("fs.try_lock");
        assert!(d.contains("std/fs") && d.contains("advisory lock"), "{d}");
        // M115.3: metadatos.
        let d = doc_text("fs.stat");
        assert!(d.contains("std/fs") && d.contains("WITHOUT following symlinks"), "{d}");
        // M115.4: watch por eventos de kernel.
        let d = doc_text("fs.watch");
        assert!(d.contains("std/fs") && d.contains("kernel events"), "{d}");
        let d = doc_text("no_existe_tal_cosa");
        assert!(d.contains("is not a builtin"), "{d}");
    }

    #[test]
    fn ray_doc_covers_the_prelude() {
        let t = doc_text("parse_int");
        assert!(t.contains("parse_int(s: string) -> Option<int>"), "firma del prelude: {t}");
        assert!(t.contains("Parses a string as an integer"), "doc /// del prelude: {t}");
        let g = doc_text("assert_eq");
        assert!(g.contains("assert_eq<T: Eq + Show>"), "genéricos con bounds: {g}");
    }

    #[test]
    fn ray_doc_en_proceso_y_method_not_found() {
        let replies = roundtrip(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ray_doc","arguments":{"symbol":"len"}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ray_doc","arguments":{"symbol":"no_such_fn"}}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"nope/nope"}"#,
        ]);
        let text = |i: usize| {
            replies[i]
                .get("result").and_then(|r| r.get("content")).and_then(|c| c.as_array())
                .and_then(|a| a.first()).and_then(|b| b.get("text")).and_then(|t| t.as_str())
                .unwrap_or("").to_string()
        };
        assert!(text(0).contains("len("), "firma de len: {}", text(0));
        assert!(text(0).contains("length of a collection"), "doc de len");
        assert!(text(1).contains("is not a builtin"), "símbolo desconocido honesto");
        assert_eq!(
            replies[2].get("error").and_then(|e| e.get("code")).map(|c| c.serialize()),
            Some("-32601".to_string())
        );
    }
}
