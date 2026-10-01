# agent-cli

A command-line agent written in raylang: it sends your question to Claude through the Messages
API (plain HTTP, no SDK), and the model checks and runs the raylang it writes with the tools of
`ray mcp`, through an MCP client over stdio. It is the project of the handbook chapter
[LLMs and MCP](../../../handbook/llm-mcp.en.md).

```sh
ray test                                              # offline: scripted API + real `ray mcp`
ANTHROPIC_API_KEY=… ray run -- "reverse the words of a string in raylang"
```

`AGENT_MODEL` changes the model (default `claude-opus-5-5`), `ANTHROPIC_BASE_URL` the endpoint,
and `RAY_BIN` the `ray` binary that serves MCP.
