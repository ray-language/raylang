# agent-cli

A command-line agent written in raylang. You ask for a piece of raylang; the model writes it,
checks and runs it with the tools of `ray mcp`, and can save it to a file once you agree. It is
the project of the handbook chapter [LLMs and MCP](../../../handbook/llm-mcp.en.md).

It is built on three packages: `llm` (the model), `mcp` (the tool server) and `agent` (the loop).

```sh
ray test                                                 # offline: a scripted model, real tools
ANTHROPIC_API_KEY=… ray run -- "reverse the words of a string in raylang"
```

| Variable | What for | Default |
|---|---|---|
| `AGENT_PROVIDER` | `anthropic`, `openai`, `openrouter`, `groq`, `ollama`… | `anthropic` |
| `AGENT_MODEL` | the model | `claude-opus-5-5` for Anthropic |
| `AGENT_BASE_URL` | an endpoint of your own | the provider's |
| `RAY_BIN` | the `ray` binary that serves MCP | `ray` |

The key comes from the provider's variable (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`…) or from the
system keychain. `--autonomy ask|edits|auto` sets how much runs without asking; with no terminal
there is nobody to ask, so anything that needs approval does not run.
