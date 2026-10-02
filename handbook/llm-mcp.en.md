# LLMs and MCP

[Español](llm-mcp.md) · English

This chapter has two halves. The first is **using an LLM assistant to write raylang**: what to
give it so the code it writes compiles. The second is **writing an agent in raylang**: a terminal
program that talks to Claude through the Messages API and uses tools through MCP.

The project for the second half is in [`examples/apps/agent-cli`](../examples/apps/agent-cli/),
with its tests. The raylang blocks are copied from it and CI checks that they still are.

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

`agent-cli` takes a question in the terminal. Claude answers it and, along the way, compiles and
runs the raylang it writes with the tools of `ray mcp`. There are three modules:

| Module | What it does |
|---|---|
| `claude.ray` | the Messages API client, over HTTP |
| `mcp.ray` | the MCP client over stdio |
| `agent.ray` | the loop: ask, run tools, return results |

There is no official Anthropic SDK for raylang, but the API is JSON over HTTPS and `net/http` is
enough.

## 3. Talking to the Messages API

A request is a `POST /v1/messages` with three headers: the key, the API version and the content
type.

<!-- check: project=examples/apps/agent-cli -->
```rust
/// Sends one request. A long answer can take minutes, hence the 10-minute timeout.
pub fn send(c: Config, body: string) -> Result<string, string> {
    var headers: Map<string, string> = Map.new();
    headers.insert("content-type", "application/json");
    headers.insert("x-api-key", c.api_key);
    headers.insert("anthropic-version", "2023-06-01");
    // `fallbacks: "default"`: if a safety classifier declines, the API retries on a fallback model.
    headers.insert("anthropic-beta", "server-side-fallback-2026-07-01");
    let r = http.request_bytes(
        "POST",
        c.base_url + "/v1/messages",
        body.to_bytes(),
        headers,
        600000
    )?;
    let text = http.body_text(r)?;
    if (r.status >= 400) {
        return Result.Err("HTTP " + to_string(r.status) + ": " + text);
    }
    Result.Ok(text)
}
```

The timeout is 10 minutes because a long answer can take several. The body carries the model, the
tools and the conversation:

<!-- check: project=examples/apps/agent-cli -->
```rust
/// The request body. `tools` and `messages` are JSON arrays already rendered as text.
pub fn request_body(model: string, system: string, tools: string, messages: [string]) -> string {
    `{
  "model": ${quote(model)},
  "max_tokens": 16000,
  "output_config": {"effort": "medium"},
  "fallbacks": "default",
  "system": ${quote(system)},
  "tools": ${tools},
  "messages": [${messages.join(",")}]
}`
}
```

- **`claude-opus-5-5`** is the default model. `AGENT_MODEL` changes it.
- **`effort: "medium"`** controls how much the model reasons before answering. It is this model's
  default; it is written out so you can see where to tune it.
- **`fallbacks: "default"`**, together with the `anthropic-beta` header above, makes the API retry
  the request on another model in the same call if a safety classifier declines it. Without it,
  the response simply stops with `stop_reason: "refusal"`.

Messages travel as JSON text. The assistant turn goes back to the API **exactly as it arrived**:
it may carry reasoning blocks, and the API requires them unchanged.

A response that asks for a tool looks like this. The content is a list of blocks, and each
`tool_use` carries an identifier:

```json
{
  "role": "assistant",
  "stop_reason": "tool_use",
  "content": [
    {"type": "text", "text": "Let me run it."},
    {"type": "tool_use", "id": "toolu_1", "name": "ray_run",
     "input": {"code": "fn main() { print(6 * 7); }"}}
  ]
}
```

The agent runs the tool and answers with a user message that carries the result, with the same
identifier:

```json
{
  "role": "user",
  "content": [
    {"type": "tool_result", "tool_use_id": "toolu_1",
     "content": "exit: 0\n--- stdout ---\n42", "is_error": false}
  ]
}
```

The `stop_reason` field says why the model stopped, and the next step depends on it:

| `stop_reason` | What happened | What the agent does |
|---|---|---|
| `end_turn` | the model finished its answer | returns the text |
| `tool_use` | it wants to use tools | runs them and asks again |
| `max_tokens` | the answer hit the limit and was cut | returns what there is |
| `refusal` | the model declined the request | ends with an error |

## 4. The agent loop

The model answers with a stop reason. If it is `tool_use`, it wants tools; the agent runs them and
sends back the results. Any other reason means the answer is ready.

<!-- check: project=examples/apps/agent-cli -->
```rust
    while (step < max_steps) {
        step = step + 1;
        let reply = claude.parse_reply(ask(claude.request_body(model, SYSTEM, tools, messages))?)?;
        messages.push(claude.assistant_turn(reply));
        if (reply.stop_reason == "refusal") {
            return Result.Err("the model declined this request");
        }
        if (reply.stop_reason != "tool_use") {
            // end_turn (done), or max_tokens (the answer was cut: return what there is)
            return Result.Ok(reply.text);
        }
        var results: [string] = [];
        for call in reply.calls {
            eprint("[tool] " + call.name);
            results.push(match (mcp.call_tool(server, call.name, call.input)) {
                Result.Ok(out) => claude.tool_result(call.id, out, false),
                Result.Err(e) => claude.tool_result(call.id, e, true),
            });
        }
        messages.push(claude.results_turn(results));
    }
```

Three details matter:

- **All the results of a turn go in a single message.** Splitting them across several teaches the
  model to stop asking for tools in parallel.
- **A failing tool is not hidden:** its output goes back with `is_error: true`, and the model
  decides what to do.
- **`refusal` and `max_tokens` are checked** before reading the text. A step limit keeps a loop
  from running forever.

## 5. An MCP client over stdio

An MCP server is a process that reads JSON-RPC requests on its standard input, one per line, and
answers on its standard output. The client starts it once and talks to it with `std/process`:

<!-- check: project=examples/apps/agent-cli -->
```rust
/// Starts `program args…` as an MCP server and performs the handshake.
pub fn connect(program: string, args: [string]) -> Result<Client, string> {
    let proc = process.cmd(program, args).stdin_pipe().stream()?;
    let c = Client { proc: proc, pending: "", next_id: 0 };
    let _ = call(
        c,
        "initialize",
        `{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"agent-cli","version":"0.1.0"}}`
    )?;
    let _ = c.proc.write(
        (`{"jsonrpc": "2.0", "method": "notifications/initialized"}` + "\n").to_bytes()
    )?;
    Result.Ok(c)
}
```

`stdin_pipe()` keeps the process input open to write one request after another, and `stream()`
delivers its output as a channel. That channel carries chunks of bytes, not lines, so the client
keeps what is left after the last newline:

<!-- check: project=examples/apps/agent-cli -->
```rust
// The next complete line of output. The process delivers chunks of bytes, not lines, so the
// client keeps what is left after the last newline for the next call.
fn read_line(c: Client) -> Result<string, string> {
    while (true) {
        match (c.pending.index_of("\n")) {
            Option.Some(i) => {
                let line = c.pending.substring(0, i);
                c.pending = c.pending.substring(i + 1, c.pending.len());
                return Result.Ok(line);
            },
            Option.None => { },
        }
        match (recv(c.proc.out)) {
            Option.Some(chunk) => {
                c.pending = c.pending + from_utf8(chunk).unwrap_or("");
            },
            Option.None => return Result.Err("the MCP server closed its output"),
        }
    }
    Result.Err("unreachable")
}
```

With that, `tools(c)` asks for the list of tools (`tools/list`) and `call_tool(c, name, arguments)`
calls one (`tools/call`). MCP tools go to the API with the same name and their JSON schema as
`input_schema`.

<!-- check: project=examples/apps/agent-cli -->
```rust
/// The MCP tools in the shape the Messages API expects (`input_schema`).
pub fn tools_json(tools: [mcp.Tool]) -> string {
    var parts: [string] = [];
    for t in tools {
        parts.push(
            `{"name": ${claude.quote(t.name)}, "description": ${claude.quote(t.description)}, "input_schema": ${t.schema}}`
        );
    }
    "[" + parts.join(",") + "]"
}
```

This is what travels between the agent (→) and `ray mcp` (←), one line per message, trimmed:

```text
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05",…}}
← {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"raylang",…},…}}
→ {"jsonrpc":"2.0","method":"notifications/initialized"}
→ {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
← {"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"ray_check","description":"…","inputSchema":{…}},…]}}
→ {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ray_run","arguments":{"code":"fn main() { print(6 * 7); }"}}}
← {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"exit: 0\n--- stdout ---\n42"}],"isError":false}}
```

Every request carries an `id` and the response repeats it; the `initialized` notification has no
`id` because it expects no answer. The client knows nothing about raylang:
`mcp.connect(program, args)` works for any MCP server that talks over standard input and output.

## 6. The API key

The key comes from `ANTHROPIC_API_KEY` or, if it is not set, from the system keychain with
`std/keychain`:

<!-- check: project=examples/apps/agent-cli -->
```rust
// The API key: ANTHROPIC_API_KEY, or the system keychain (service "agent-cli", account
// "anthropic"), where `keychain.set("agent-cli", "anthropic", key)` stored it once.
fn api_key() -> Result<string, string> {
    match (env("ANTHROPIC_API_KEY")) {
        Option.Some(k) => return Result.Ok(k),
        Option.None => { },
    }
    match (keychain.get("agent-cli", "anthropic")?) {
        Option.Some(k) => Result.Ok(k),
        Option.None => Result.Err("set ANTHROPIC_API_KEY or store the key in the keychain"),
    }
}
```

To store it once: a one-line program with `keychain.set("agent-cli", "anthropic", key)`. On macOS
it goes to Keychain, on Linux to Secret Service and on Windows to Credential Manager. Never into a
text file.

The rest of the configuration comes from the environment too:

| Variable | What for | Default |
|---|---|---|
| `ANTHROPIC_API_KEY` | the API key | the one in the keychain |
| `AGENT_MODEL` | the model | `claude-opus-5-5` |
| `ANTHROPIC_BASE_URL` | the API address, to go through your own proxy | `https://api.anthropic.com` |
| `RAY_BIN` | the `ray` binary that acts as the MCP server | `ray` |

The program exits with 64 if it gets no question and with 1 on any other failure. The answer goes
to standard output and the tool trace (`[tool] ray_run`) to standard error, so
`agent-cli "…" > answer.txt` saves only the answer.

## 7. Testing without network

The tests do not call the API and need no key. The loop receives the function that sends requests
as a parameter, so a test replaces it with a script: the first time it asks for `ray_run`, and when
the result comes back, it answers. The MCP server is the real `ray mcp`.

<!-- check: project=examples/apps/agent-cli -->
```rust
@test
fn the_loop_runs_a_tool_and_returns_the_answer() {
    let server = mcp.connect(ray_bin(), ["mcp"]).unwrap();
    // The scripted API: the first request has no tool result yet, so it asks for `ray_run`;
    // once the result (with 42 in it) comes back, it answers.
    let seen: Channel<string> = Channel.bounded(8);
    let ask = fn(body: string) -> Result<string, string> {
        send(seen, body);
        if (body.contains("tool_result")) { Result.Ok(END_TURN) } else { Result.Ok(TOOL_USE) }
    };
    let answer = agent.run(ask, server, "claude-opus-5-5", "what does 6 * 7 print?", 5).unwrap();
    assert_eq(answer, "It prints 42.");
    let _first = recv(seen).unwrap();
    let second = recv(seen).unwrap();
    assert(second.contains("tool_use_id"));
    assert(second.contains("42"));
    mcp.close(server);
}
```

```sh
ray test                                                   # no network, no key
ANTHROPIC_API_KEY=… ray run -- "reverse the words of a string in raylang"
```

This agent implements both protocols by hand, in about 400 lines including the loop. Turning them
into standard packages, `llm` and `mcp`, so any app can use them without copying is under
evaluation. Until then, this example is the reference.

## 8. What is missing for a production agent

The example is short on purpose. Before putting it in front of users, add:

| What | Why |
|---|---|
| Streaming | the agent waits for the whole response, which is why the timeout is 10 minutes; with streaming the text shows up as it is generated |
| Retries | a 429 (rate limit) or a 529 (API overloaded) ends the program; the right thing is to retry with growing waits |
| Prompt caching | the instructions and the tools are sent again at every step; the API's cache avoids paying for them each time |
| History | every run is a single question; a conversation needs the messages stored |
| Confirmation | the agent runs whatever the model asks for; a tool with side effects, such as writing files, should ask for permission first |

## Next step

[**Performance**](performance.en.md): how to measure a program, find where the time goes and make
it fast.

<!-- sync: sha256:d94ff739cceb -->
