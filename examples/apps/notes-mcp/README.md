# notes-mcp

An MCP server written in raylang: it offers your notes to an assistant as tools. Claude Code,
Claude Desktop or any agent can search them, read them and add to them. It is the second project
of the handbook chapter [LLMs and MCP](../../../handbook/llm-mcp.en.md).

```sh
ray test                                   # the server, one protocol message at a time
ray build --native --release -o notes-mcp
claude mcp add notes -- "$PWD/notes-mcp"   # connect it to Claude Code
```

| Tool | What it does | Read-only |
|---|---|---|
| `search_notes` | lists the titles of the notes that contain a text | yes |
| `read_note` | returns the body of a note | yes |
| `add_note` | creates or replaces a note | no |

The notes are Markdown files in `NOTES_DIR` (`notes` in the current folder by default).
`notes-mcp --http 8765` serves over HTTP on this machine instead of standard input and output.
