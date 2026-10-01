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

## Coming next

These chapters are published one by one, each with its complete example project in
`examples/apps/`:

- **Mobile app for iOS and Android** with a React + TypeScript frontend and data in `std/kv`.
- **Cross-platform app**: the same app on macOS, Linux and Windows, with SQLite.
- **Web API** with the `web` framework, Postgres and connection pools.
- **Server-rendered site** with templates and the data in files.
- **Site with an embedded React frontend** in the binary, with Redis.
- **LLMs and MCP**: an assistant that writes verified raylang, and an agent written in raylang.
- **Shipping**: packaging, signing, notarizing and auto-updating.

Meanwhile, the desktop and mobile details are in the manual (Spanish):
[windows with `std/ui`](../MANUAL.md#ventanas-stdui),
[packaging with `ray bundle`](../MANUAL.md#empaquetar-la-app-ray-bundle) and
[hot reload on the phone](../MANUAL.md#hot-reload-del-programa-en-el-teléfono-ray-dev---device).

<!-- sync: sha256:996b4ae28c10 -->
