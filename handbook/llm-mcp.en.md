# LLMs and MCP

[Español](llm-mcp.md) · English

This chapter has three parts. The first is **using an LLM assistant to write raylang**: what to
give it so that the code it writes compiles. The second is **writing an agent in raylang**: a
command-line program that talks to a model, uses tools and asks before it changes anything. The
third is **offering your own tools** to an assistant, with an MCP server.

The last two rest on three packages: `llm` talks to the model, `mcp` to the tool servers, and
`agent` is the loop that joins them. The projects are in
[`examples/apps/agent-cli`](../examples/apps/agent-cli/) and
[`examples/apps/notes-mcp`](../examples/apps/notes-mcp/), with their tests. The raylang blocks
are copied from them and CI checks that they still are.

```sh
agent-cli "write a function that reverses the words of a string"
```

```text
[tool] mcp__ray__ray_check
[tool] mcp__ray__ray_run
Here it is, checked and run: …

? run write_file (write)? [y]es / [a]lways / [N]o
```

## 1. An assistant that writes correct raylang

raylang is new, and a model does not know it as well as Python or Rust. Two pieces make up for it.

**`llms.txt`.** It is the distilled context of the language: how it differs from Rust, the
canonical forms and the exact error messages. It lives at the root of the repository, and
`ray mcp` offers it as the `raylang://llms.txt` resource. One line in the project's `CLAUDE.md` is
enough:

```markdown
# CLAUDE.md
This project is written in raylang. Before writing code, read the raylang://llms.txt resource of
the raylang server, and compile and run everything with ray_check and ray_run.
```

**`ray mcp`.** It is an [MCP](https://modelcontextprotocol.io) server that ships in the `ray`
binary. It gives the assistant five tools:

| Tool | What for |
|---|---|
| `ray_check` | compiles and returns the exact diagnostics |
| `ray_run` | runs and returns the output and the exit code |
| `ray_test` | runs the `@test` functions |
| `ray_fmt` | returns the canonical formatting |
| `ray_doc` | the signature and documentation of any function or type |

With Claude Code, one command connects it:

```sh
claude mcp add raylang -- ray mcp
```

From then on the assistant writes, compiles, reads the error and fixes it without you copying
anything by hand. In a project, the tools should receive **`path`** (the file or the project
folder) instead of loose code: that way imports across files and the `ray.toml` dependencies
resolve exactly as with `ray run`. The server's own instructions tell the model so. The details
are in [docs/mcp.en.md](../docs/mcp.en.md).

Other MCP clients, such as Claude Desktop or an editor, are configured with a JSON file that
launches the same command:

```json
{ "mcpServers": { "raylang": { "command": "ray", "args": ["mcp"] } } }
```

With the tools connected, three habits give better results:

- **Ask for a project, not a snippet.** With a `ray.toml` and a `src/`, the assistant checks the
  whole program, with its modules and dependencies, and not an isolated piece.
- **Have it look things up instead of assuming.** `ray_doc` returns the real signature of any
  function in the standard library or in one of the project's packages. An assistant that looks
  it up does not invent functions that do not exist.
- **Have it finish with the tests.** `ray_check` says it compiles; `ray_test` says it does what it
  should.

## 2. An agent written in raylang

An agent is a loop: the model asks for a tool, the program runs it, hands back the result and
repeats until the model answers. `agent-cli` takes a question on the command line; the model
writes raylang, compiles it and runs it with the tools of `ray mcp`, and can save it to a file
once you allow it.

```sh
ray new agent-cli && cd agent-cli
ray add agent
ray add llm
ray add mcp
```

| Package | What it brings |
|---|---|
| `llm` | the conversation with the model: Claude, OpenAI or any server that speaks their dialect |
| `mcp` | the tools of an MCP server, and the server side of section 9 |
| `agent` | the loop, the step budget, and what runs unasked and what asks |

With the three of them, the whole program fits in three short modules:

| Module | What it does |
|---|---|
| `model.ray` | which model to talk to, from the environment |
| `files.ray` | the program's own tools: reading and writing files |
| `main.ray` | builds the agent, asks the user and shows what happens |

## 3. The model

The provider, the model and the key come from the environment, so the same binary works against
Claude, against OpenAI or against a model on your machine:

<!-- check: project=examples/apps/agent-cli -->
```rust
/// The configuration from the environment: AGENT_PROVIDER (a known provider; "anthropic" by
/// default), AGENT_MODEL, and AGENT_BASE_URL for an endpoint of your own.
pub fn from_env() -> Result<Config, string> {
    let provider = env("AGENT_PROVIDER").unwrap_or("anthropic");
    let preset = match (config.find_preset(provider)) {
        Option.Some(p) => p,
        Option.None => {
            return Result.Err("unknown AGENT_PROVIDER '${provider}'");
        },
    };
    let model = env("AGENT_MODEL").unwrap_or(default_model(provider));
    if (model == "") {
        return Result.Err("set AGENT_MODEL: '${provider}' has no default model");
    }
    let base_url = env("AGENT_BASE_URL").unwrap_or(preset.base_url);
    var c = config.new_config(preset.dialect, base_url, model, api_key(preset)?);
    c.max_tokens = 8000;
    Result.Ok(c)
}
```

`config.find_preset` knows the providers by name, and each one speaks one of two dialects:
Anthropic's or OpenAI's.

| `AGENT_PROVIDER` | Dialect | Key in |
|---|---|---|
| `anthropic` | Anthropic | `ANTHROPIC_API_KEY` |
| `openai` | OpenAI | `OPENAI_API_KEY` |
| `openrouter`, `groq`, `deepseek`, `mistral`, `xai`, `gemini` | OpenAI | each one's variable |
| `ollama`, `lmstudio` | OpenAI | none: they are local |

```sh
ANTHROPIC_API_KEY=… agent-cli "…"                                     # Claude, the default
AGENT_PROVIDER=openai OPENAI_API_KEY=… agent-cli "…"
AGENT_PROVIDER=ollama AGENT_MODEL=llama3.2 agent-cli "…"               # a local model, no key
AGENT_PROVIDER=ollama AGENT_BASE_URL=http://127.0.0.1:8080/v1 AGENT_MODEL=… agent-cli "…"
```

The last form works for any OpenAI-compatible server, such as llama.cpp or LM Studio on another
port.

The package takes care of what is tedious in a model API: it retries a 429 or a 5xx waiting for
as long as the provider asks, fixes by itself the parameters the provider rejects, and with
Claude it replays the reasoning blocks untouched and uses the prompt cache. What it does not
model you add by hand: a body field in `c.extra` and a header in `c.headers`.

## 4. The tools

The agent has tools of two kinds, and the loop does not tell them apart.

**Its own** are functions of the program. Each declares what the model sees (name, description
and the JSON Schema of its arguments), its risk, and the function that runs it:

<!-- check: project=examples/apps/agent-cli -->
```rust
/// Gives the agent `read_file` and `write_file`, both confined to `root`.
pub fn add_tools(a: Agent, root: string) -> Result<int, string> {
    let read = fn(args: Json) -> Result<string, string> { read_file(root, args) };
    let write = fn(args: Json) -> Result<string, string> { write_file(root, args) };
    agent.tool(
        a,
        "read_file",
        "Reads a text file of the working folder",
        READ_SCHEMA,
        agent.READ,
        read
    )?;
    agent.tool(
        a,
        "write_file",
        "Creates or replaces a text file of the working folder",
        WRITE_SCHEMA,
        agent.WRITE,
        write
    )
}
```

The function receives the arguments as `Json` and returns `Result<string, string>`:

<!-- check: project=examples/apps/agent-cli -->
```rust
const WRITE_SCHEMA: string = `{
    "type": "object",
    "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
    "required": ["path", "content"]
}`;

// What the model reads back is the tool's answer: an `Err` reaches it as a failed call.
fn write_file(root: string, args: Json) -> Result<string, string> {
    let path = inside(root, json.get_string(args, "path").unwrap_or(""))?;
    let content = json.get_string(args, "content").unwrap_or("");
    fs.write_file(path, content)?;
    Result.Ok("wrote ${content.len()} characters")
}
```

The `Ok` is the text the model reads; the `Err` reaches it as a failed call, with its message,
and it decides what to do. A tool that aborts does not bring the turn down either. Tools run in
the same fiber that called `agent.run`, so they can keep state like any other function.

The arguments are written by a model, which may have read text from a third party. They are
checked like those of a form:

<!-- check: project=examples/apps/agent-cli -->
```rust
/// The path of `name` inside `root`, or why it is refused. The arguments come from a model, so
/// they are checked like any other input: nothing absolute, nothing that climbs out.
pub fn inside(root: string, name: string) -> Result<string, string> {
    if (name.trim() == "") {
        return Result.Err("a path is required");
    }
    if (name.starts_with("/") || name.contains("\\") || name.contains(":")) {
        return Result.Err("only paths relative to the working folder are allowed");
    }
    for part in name.split("/") {
        if (part == "..") {
            return Result.Err("the path may not leave the working folder");
        }
    }
    Result.Ok(root + "/" + name)
}
```

**Those of an MCP server** are discovered on connecting. Here, the toolchain's own:

<!-- check: project=examples/apps/agent-cli -->
```rust
    // The tools of raylang's own toolchain. `RAY_BIN` points at a specific `ray` binary.
    let ray = match (mcp.connect_stdio(env("RAY_BIN").unwrap_or("ray"), ["mcp"])) {
        Result.Ok(s) => s,
        Result.Err(e) => {
            eprint("could not start `ray mcp`: " + e);
            return 1;
        },
    };
    match (agent.connect(a, "ray", ray)) {
        Result.Ok(_) => { },
        Result.Err(e) => eprint("ray mcp: " + e),
    }
```

The model sees them as `mcp__ray__ray_check`, `mcp__ray__ray_run` and so on. The instructions the
server declares join the system prompt.

## 5. What runs unasked and what asks

Every tool has a risk and the agent has an autonomy level. Whether a call runs without asking
follows from the two:

| The tool's risk | `ask` | `edits` | `auto` |
|---|---|---|---|
| `READ`: only reads | runs | runs | runs |
| `WRITE`: changes something that can be taken back | asks | runs | runs |
| `EXEC`: does something that cannot be taken back | asks | asks | runs |

`read_file` is `READ` and `write_file` is `WRITE`. Of the `ray mcp` ones, those the server
declares read-only (`ray_fmt`, `ray_doc`) are `READ`; the rest are `EXEC`, because a server that
says nothing is treated as if it changed everything.

Whatever must be asked is asked of a function of yours:

<!-- check: project=examples/apps/agent-cli -->
```rust
// Asks the user about one call. Only wired when there is a terminal to ask on.
fn ask_user(call: ToolCall, risk: string) -> Decision {
    eprint("");
    let _ = io.ewrite("? run ${call.name} (${risk})? [y]es / [a]lways / [N]o ");
    match (input()) {
        Option.Some(answer) => {
            let said = answer.trim().to_lower();
            if (said == "y") {
                Decision.Yes
            } else if (said == "a") {
                Decision.Always
            } else {
                Decision.No
            }
        },
        Option.None => Decision.No,
    }
}
```

- `Decision.Yes` lets it run this once.
- `Decision.Always` lets it run and does not ask about that tool again.
- `Decision.No` does not run it, and the model is told the user did not allow it.

A "no" does not break the conversation: it goes back to the model as the result of the call, and
the model carries on from there.

The question is only wired when there is a terminal:

<!-- check: project=examples/apps/agent-cli -->
```rust
    // Without a terminal there is nobody to ask: what needs approval simply does not run.
    if (term.is_tty(0)) {
        agent.on_approve(a, ask_user);
    }
```

That is the safe default. In a script, in CI or on a server, `write_file` and `ray_run` do not
run unless the program is started with `--autonomy auto`.

## 6. The turn

`agent.run` takes the question to the end and returns why it ended:

<!-- check: project=examples/apps/agent-cli -->
```rust
    let code = match (agent.run(a, words.join(" "))) {
        Result.Ok(outcome) => {
            print("");
            if (outcome.stop != agent.DONE) {
                eprint("stopped: ${outcome.stop} after ${outcome.steps} steps");
            }
            0
        },
        Result.Err(e) => {
            eprint(e);
            1
        },
    };
```

| `stop` | What happened |
|---|---|
| `DONE` | the model answered without asking for more tools |
| `MAX_STEPS` | the step budget ran out (20 by default); another `run` can continue |
| `TRUNCATED` | the answer was cut at the token limit |
| `REFUSED` | the model declined to answer |
| `CANCELLED` | the wait was abandoned, with `run_cancellable` |

`run` only returns `Err` when talking to the model fails. A tool that fails, or that was not
allowed, is part of the conversation.

The package knows nothing about terminals. What happens in the turn arrives as events, and the
program decides how to show it:

<!-- check: project=examples/apps/agent-cli -->
```rust
// The answer goes to standard output as it arrives; what the agent does, to standard error.
fn show(e: Event) {
    match (e) {
        Event.Text(piece) => {
            let _ = io.write(piece);
            let _ = io.flush();
        },
        Event.Calling(call, risk) => eprint("[tool] ${call.name}"),
        Event.Returned(call, output, failed) => {
            if (failed) {
                eprint("[tool] ${call.name} failed");
            }
        },
        Event.Declined(call, why) => eprint("[tool] ${call.name} was not run"),
        _ => { },
    }
}
```

The answer goes to standard output as it arrives, because `a.stream` is on. What the agent does
goes to standard error. So `agent-cli "…" > answer.md` saves only the answer.

The conversation stays in `a.history`: a second call to `run` on the same agent continues where
the first one left off, and `a.usage` carries the tokens of every turn.

## 7. The API key

The key comes from the provider's environment variable or, failing that, from the system
keychain through `std/keychain`:

<!-- check: project=examples/apps/agent-cli -->
```rust
// The API key of a provider: its environment variable, or the system keychain (service
// "agent-cli", account = the provider), where `keychain.set` stored it once. A local server
// needs none.
fn api_key(p: Preset) -> Result<string, string> {
    if (p.key_env == "") {
        return Result.Ok("");
    }
    match (env(p.key_env)) {
        Option.Some(k) => return Result.Ok(k),
        Option.None => { },
    }
    match (keychain.get("agent-cli", p.id)?) {
        Option.Some(k) => Result.Ok(k),
        Option.None => Result.Err("set ${p.key_env}, or store the key in the keychain"),
    }
}
```

To store it once: a one-line program with `keychain.set("agent-cli", "anthropic", key)`. On
macOS it goes to Keychain, on Linux to Secret Service and on Windows to Credential Manager. Never
into a text file or the repository.

## 8. Testing without network

The tests call no API and need no key. The model is replaced by a tiny server in the same
process, which speaks the OpenAI dialect and follows a two-step script:

<!-- check: project=examples/apps/agent-cli -->
```rust
// The scripted model: the first request has no tool result yet, so it asks for `write_file`;
// once the result is there, it repeats it as its answer.
fn scripted(req: Request) -> Response {
    let body = webserver.request_text(req).unwrap_or("");
    let messages = json.get_array(json.parse(body).unwrap_or(Json.JNull), "messages").unwrap_or([]);
    let last = messages[messages.len() - 1];
    if (json.get_string(last, "role").unwrap_or("") == "tool") {
        let said = json.stringify(
            Json.JStr("Done: " + json.get_string(last, "content").unwrap_or(""))
        );
        return webserver.json_response(
            `{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":${said}}}]}`
        );
    }
    webserver.json_response(
        `{"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"write_file","arguments":"{\\"path\\": \\"hello.ray\\", \\"content\\": \\"fn main() { }\\"}"}}]}}]}`
    )
}
```

The agent, the tools and the approval are the real ones. This test checks that a write waits for
the user and that, with their permission, the file ends up written:

<!-- check: project=examples/apps/agent-cli -->
```rust
@test
fn a_write_waits_for_the_user_and_runs_when_allowed() {
    let (a, root) = fixture("yes");
    agent.on_approve(a, fn(call: ToolCall, risk: string) -> Decision {
        assert_eq(call.name, "write_file");
        assert_eq(risk, agent.WRITE);
        Decision.Yes
    });
    let outcome = agent.run(a, "save an empty program").unwrap();
    assert_eq(outcome.stop, agent.DONE);
    assert_eq(outcome.text, "Done: wrote 13 characters");
    assert_eq(fs.read_file(root + "/hello.ray").unwrap(), "fn main() { }");
}
```

Two more tests cover the opposite cases: with nobody to ask nothing is written, and at the
`edits` level the write runs by itself.

```sh
ray test
```

## 9. Your own MCP server

The other side of the protocol: your program offering its tools to an assistant. `notes-mcp`
exposes some notes to Claude Code, to Claude Desktop or to any agent, including the one from the
previous sections.

```sh
ray new notes-mcp && cd notes-mcp
ray add mcp
```

A server is a `Provider` with its tools and its resources. Those that only read are registered
with `read_only_tool`, which is what lets a client run them without asking; the one that writes,
with `tool`:

<!-- check: project=examples/apps/notes-mcp -->
```rust
/// What the server offers over the notes in `dir`.
pub fn provider_for(dir: string) -> Provider {
    var p = serve.provider("notes", "0.1.0");
    p.instructions = "The user's personal notes. Search before you add, so a note is not duplicated.";
    let search = fn(args: Json) -> Result<string, string> { search_notes(dir, args) };
    let read = fn(args: Json) -> Result<string, string> { read_note(dir, args) };
    let add = fn(args: Json) -> Result<string, string> { add_note(dir, args) };
    let index = fn() -> Result<string, string> { Result.Ok(store.titles(dir).join("\n")) };
    // The two that only read are announced as read-only: a client may run them without asking.
    let _ = serve.read_only_tool(
        p,
        "search_notes",
        "Lists the titles of the notes that contain a text; an empty text lists them all",
        SEARCH_SCHEMA,
        search
    );
    let _ = serve.read_only_tool(
        p,
        "read_note",
        "Returns the body of the note with this exact title",
        TITLE_SCHEMA,
        read
    );
    let _ = serve.tool(
        p,
        "add_note",
        "Creates a note, or replaces the one with the same title",
        NOTE_SCHEMA,
        add
    );
    let _ = serve.resource(
        p,
        "notes://index",
        "Index",
        "The title of every note, one per line",
        "text/plain",
        index
    );
    p
}
```

Each tool is a function that receives the arguments and returns the text the model reads:

<!-- check: project=examples/apps/notes-mcp -->
```rust
// What the model reads back is the tool's answer: an `Err` reaches it as a failed call.
fn add_note(dir: string, args: Json) -> Result<string, string> {
    let title = text_arg(args, "title")?;
    store.write(dir, title, text_arg(args, "body")?)?;
    Result.Ok("saved '${title}'")
}
```

A note's title ends up as part of a path, and a model writes it. It is validated before touching
the disk:

<!-- check: project=examples/apps/notes-mcp -->
```rust
/// Whether `title` can be a file name: letters, digits, spaces, dashes and underscores. A
/// title comes from a model, and it becomes part of a path.
pub fn valid(title: string) -> bool {
    if (title.trim() == "" || title.len() > 80) {
        return false;
    }
    for ch in title.chars() {
        let code = char_code(ch);
        let letter = (code >= 65 && code <= 90) || (code >= 97 && code <= 122);
        let digit = code >= 48 && code <= 57;
        if (!(letter || digit || ch == ' ' || ch == '-' || ch == '_')) {
            return false;
        }
    }
    true
}
```

`main` serves the provider. With no arguments it speaks over standard input and output, which is
how an assistant launches a local server:

<!-- check: project=examples/apps/notes-mcp -->
```rust
fn main() -> int {
    let argv = args();
    // `notes-mcp --http 8765` serves over HTTP on this machine; with no arguments it speaks
    // over standard input and output, which is how an assistant launches a local server.
    if (argv.len() == 2 && argv[0] == "--http") {
        return match (serve.http(build, "127.0.0.1", argv[1].parse_int().unwrap_or(0))) {
            Result.Ok(_) => 0,
            Result.Err(e) => {
                eprint(e);
                1
            },
        };
    }
    serve.stdio(build())
}
```

To connect it to Claude Code, the command is your binary:

```sh
ray build --native --release -o notes-mcp
claude mcp add notes -- /path/to/notes-mcp
```

| | Over stdio | Over HTTP |
|---|---|---|
| Started with | `serve.stdio(p)` | `serve.http(build, "127.0.0.1", port)` |
| Who launches it | the assistant, as a child process | you; the assistant connects to the URL |
| For | tools of this machine | a shared service |
| State between requests | the program's variables | a database or a channel: every connection runs in its own fiber |
| Protection | the process's own | refuses another origin; `p.token` requires `Authorization: Bearer` |

Over stdio, standard output is the protocol's channel: whatever the program wants to say about
itself goes through `eprint`, never `print`.

The server is tested one message at a time. `serve.handle` takes the request line and returns
the reply line, with no process and no port:

<!-- check: project=examples/apps/notes-mcp -->
```rust
// The `result` of the reply to one request, as JSON.
fn ask(dir: string, request: string) -> Json {
    let reply = serve.handle(main.provider_for(dir), request).unwrap();
    json.member(json.parse(reply).unwrap(), "result").unwrap_or(Json.JNull)
}
```

<!-- check: project=examples/apps/notes-mcp -->
```rust
@test
fn a_bad_call_is_an_error_the_model_can_read() {
    let dir = fs.make_temp_dir("notes-mcp-bad").unwrap();
    assert_eq(
        call(dir, "read_note", `{"title": "holidays"}`),
        ("there is no note titled 'holidays'", true)
    );
    assert_eq(call(dir, "read_note", "{}"), ("'title' is required", true));
    let escape = call(dir, "add_note", `{"title": "../../etc/passwd", "body": "x"}`);
    assert(escape.1);
    assert(escape.0.contains("not a valid title"));
}
```

The three cases in the test are errors the model reads and can recover from: a note that does not
exist, a missing argument and a title that tries to leave the folder.

## 10. What the agent does not do yet

`agent-cli` is short on purpose, and the packages are at their first version. Before putting it
in front of users, it helps to know what is missing:

| What | Where it stands |
|---|---|
| Multi-turn conversation | the package supports it (`a.history`); the example asks a single question |
| Cancelling | `agent.run_cancellable` abandons the wait for the model; the example does not wire it to a key |
| Parallel tools | they run one after another, in order |
| Long history | it is not compacted: a very long conversation ends up filling the model's context |
| Token or cost budget | there is only a step budget; `a.usage` gives the tokens so you can measure |
| Images and other content | text only |

## Next step

[**Performance**](performance.en.md): how to measure a program, find where the time goes and make
it fast.

<!-- sync: sha256:9942d9c7794d -->
