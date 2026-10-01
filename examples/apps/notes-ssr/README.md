# Notes (server-rendered site)

Notes as a website rendered on the server: compiled raylang templates, forms with
POST → redirect → GET, Markdown bodies, and the notes stored as one JSON file each. No JavaScript
in the browser. It is the project of the handbook chapter
[Server-rendered site](../../../handbook/ssr.en.md), which explains it step by step.

```sh
ray test                         # the store and the pages, without a server
ray run                          # http://127.0.0.1:8080 (PORT and NOTES_DIR change it)
ray build --native --release     # one self-contained binary (templates and CSS inside)
```
