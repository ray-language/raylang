# raylang's MCP server (`ray mcp`)

[Español](mcp.md) · English

"The LSP for agents" (IDEAS §51, piece B): an [MCP](https://modelcontextprotocol.io) server
embedded in the `ray` binary that gives an LLM the **write → verify → fix** loop. Hallucination
turns into iteration: the model writes raylang, `ray_check` hands back the exact diagnostics
(positioned, up to 20), and `ray_run` verifies the behavior.

## Connecting it

Claude Code:

```sh
claude mcp add raylang -- ray mcp
```

Any MCP client (Claude Desktop, etc.), in its server config:

```json
{ "mcpServers": { "raylang": { "command": "ray", "args": ["mcp"] } } }
```

There is nothing else to configure: the server speaks JSON-RPC 2.0 over stdio (line-delimited
messages), with no dependencies and no state.

## The tools

| Tool | Arguments | What it returns |
|---|---|---|
| `ray_check` | `code` | `ok` or the compiler's exact diagnostics (position + line + `^`) |
| `ray_run` | `code`, `stdin?` | `exit` (the `int` of `main`) + stdout + stderr |
| `ray_test` | `code` | the `@test` runner's report; `exit` 0 = green, 1 = failures |
| `ray_fmt` | `code` | the canonical source (`ray fmt`) |
| `ray_doc` | `symbol` | signature + doc of a builtin, a `std/*` function or a **type** (`len`, `json.parse`, `ui.MenuItem` with its fields, `process.Exit` with its variants…) |

And two *resources*: **`raylang://llms.txt`** — the distilled context of piece A (delta against
Rust, canonical forms, exact error messages) — and **`raylang://reference.md`** — the full
module-by-module catalog of signatures (the English `REFERENCE.en.md`). In addition, the
`initialize` **instructions** (the server's "system prompt", which clients incorporate) direct the
model to read them BEFORE assuming a feature is missing: the stdlib is embedded in the toolchain and
a file search will not find it — the antidote to the "I proposed what already existed" pattern
(seen three times in a row from a real project: inflate/M64, stdin_pipe/M100 v3, FFI/M41).

## Sandboxing

`ray_run`/`ray_test` execute arbitrary code from the model → they run **in a subprocess** of the
binary itself (process isolation; the guest's stdout never touches the MCP channel) with the
embedding limits of M42:

- **fuel**: 100 M VM instructions (an infinite loop dies by fuel, with a clear error;
  `RAYLANG_MCP_FUEL` adjusts it — the tests use it so the fuel cut always wins over the wall-clock
  deadline, also on slow debug builds);
- **heap**: 1 M live objects;
- **wall-clock deadline**: 10 s with `kill` (for what does not consume fuel: network, blocked stdin);
- output truncated to 64 KiB per stream;
- `--deterministic` (M:1): the same input produces the same output — the agent can compare.

A compiler diagnostic is **not** a tool error (`isError: false`): it is the feedback the model needs.
`isError: true` is reserved for failures of the wrapper (timeout, I/O).

## Implementation

`src/mcp.rs` (~300 lines), a 100% external client like the LSP/REPL/runner: zero changes to the
core, zero dependencies (the JSON is the LSP's, `lsp::json`). Tests: in-memory unit tests (`serve`
is generic over the streams) + `tests/mcp_cli.rs` (the real server over stdio, the five tools end to
end, including the loop bomb cut by fuel).

## After updating `ray`

The MCP server is a long-lived process: **Claude Code starts it when the session opens and does not
replace it even if you reinstall the binary**. After a `make install` (or any upgrade), restart the
Claude Code session (or reconnect the server with `/mcp`) so that MCP-side fixes apply — an old
server may exhibit bugs that are already fixed (e.g. the template scan in the shared /tmp, fixed in
Jul 2026).

`ray_doc` accepts, besides builtins and prelude functions, the public functions of the embedded
`std/*` modules: `ray_doc("json.parse")`, `ray_doc("regex.find_all")` — or the bare name, which is
searched in every module.

## Agent workflow: format at the end

`ray fmt` is canonical and **rewrites** what is not: it collapses a multi-line call that fits in 100
columns, splits a long signature, moves a comment to its operand. An agent that applies changes by
text replacement and formats **between** patches leaves the next patch without its anchor — and the
failure is silent: it compiles, the tests pass and the change is not there (ray-sublime, entry 31).
Rule: format at the end of the task, never between patches, and verify with `grep` that every patch
landed.

<!-- sync: sha256:e8fb8f57c968 -->
