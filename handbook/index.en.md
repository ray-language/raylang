# The raylang handbook

[Español](index.md) · English

Guides for **building real things** with raylang, from `ray new` to the installed app or the
deployed service. Each guide is a complete path for one kind of project: it does not walk the
language module by module (the [manual](../MANUAL.md) and the [reference](../REFERENCE.en.md) do
that), but shows which pieces to use, in what order and why.

The guides build on each other: the notes app that starts as a mobile app comes back as a desktop
app, as an API, as a server-rendered site and as a site with a React frontend. Each one stores its
data differently, so by the end you have used the whole range: the `std/kv` store, SQLite,
Postgres, files and Redis.

All the code on these pages compiles: CI runs `ray check` on every block.

## Chapters

1. [Getting started](empezar.en.md): install, the editor, a project, the language in fifteen
   minutes, concurrency, working with an LLM assistant and the tools.
2. [Mobile app for iOS and Android](movil.en.md): a notes app with a React + TypeScript frontend
   and data in `std/kv`, from the project to the simulator, the emulator and hot reload on the
   phone.
3. [Cross-platform app](multiplataforma.en.md): the same app on macOS, Linux and Windows, with
   SQLite, native menus and dialogs, and an interface that adapts to every screen.
4. [Server-rendered site](ssr.en.md): the notes as a server-rendered website, with compiled
   templates, forms, Markdown and the data in files.
5. [Web API](api.en.md): the notes as a JSON API with the `web` framework, Postgres with a
   connection pool, token authentication and graceful shutdown.
6. [Site with a React frontend](web-react.en.md): a React frontend embedded in the binary, a JSON
   API and the notes in Redis.
7. [LLMs and MCP](llm-mcp.en.md): an assistant that writes raylang verified with `ray mcp`, and an
   agent written in raylang that talks to Claude and uses tools through MCP.
8. [Shipping](distribuir.en.md): packaging and signing for each system, publishing to the mobile
   stores and signed automatic updates on the desktop.

The details of every window API are in the manual (Spanish):
[windows with `std/ui`](../MANUAL.md#ventanas-stdui) and
[packaging with `ray bundle`](../MANUAL.md#empaquetar-la-app-ray-bundle).

<!-- sync: sha256:56e75c733a4e -->
