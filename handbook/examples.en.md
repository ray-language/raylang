# More examples

[Español](examples.md) · English

The apps in this handbook cover the most common cases, each explained step by step. The
[ray-language](https://github.com/ray-language) organization publishes more apps written in
raylang, with their full source, for other use cases. They are a good starting point when what you
want to build is not in the handbook: find the closest one, clone it and change it.

| Use case | Apps |
|---|---|
| Desktop and mobile | [ray808](https://github.com/ray-language/ray808) (drum machine with React for desktop, iOS and Android), [raydesk](https://github.com/ray-language/raydesk) (task manager), [raynote](https://github.com/ray-language/raynote) (notepad with native menus and dialogs), [rayplay](https://github.com/ray-language/rayplay) (background audio player), [raystage](https://github.com/ray-language/raystage) (window kinds) |
| Servers and networking | [raygate](https://github.com/ray-language/raygate) (API gateway), [raywatch](https://github.com/ray-language/raywatch) (service monitor with an SSE dashboard), [raystream](https://github.com/ray-language/raystream) (media server), [rayq](https://github.com/ray-language/rayq) (message queue with a WAL), [raykv](https://github.com/ray-language/raykv) (Redis-compatible server), [raybot](https://github.com/ray-language/raybot) (WebSocket bot), [raymail](https://github.com/ray-language/raymail) (SMTP client), [rayrelay](https://github.com/ray-language/rayrelay) (relay) |
| Distributed systems | [raymart](https://github.com/ray-language/raymart) (e-commerce with microservices, a gateway, a queue and four databases), [raycall](https://github.com/ray-language/raycall) (microservices with RPC and tracing) |
| Security and P2P | [msg](https://github.com/ray-language/msg) (encrypted P2P chat), [takeit](https://github.com/ray-language/takeit) (encrypted file transfer), [raypass](https://github.com/ray-language/raypass) (secrets vault), [raysync](https://github.com/ray-language/raysync) (encrypted sync) |
| Web | [store](https://github.com/ray-language/store) (store with an Astro frontend and a raylang backend), [raysite](https://github.com/ray-language/raysite) (static site generator) |
| Terminal | [raytop](https://github.com/ray-language/raytop) (process viewer), [raylogs](https://github.com/ray-language/raylogs) (log analyzer), [raytetris](https://github.com/ray-language/raytetris), [rallyx](https://github.com/ray-language/rallyx) and [1942](https://github.com/ray-language/1942) (games at 30 fps) |

**[raymart](https://github.com/ray-language/raymart)** is the most complete example: a distributed e-commerce system with four
hexagonal-architecture APIs (products, cart, orders and payments), each with its own database
(PostgreSQL, MySQL, MongoDB and raykv), behind raygate, talking over rpc and gRPC and coordinated
by the rayq queue. It runs as native binaries with `docker compose`, and includes end-to-end, chaos
and load tests. It is the reference for seeing how several of these apps fit into a real system.

The packages they use are in the [package index](https://github.com/ray-language/ray-index), and `ray search` finds them.

For any function, the [reference](../REFERENCE.en.md) has its signature, and the
[manual](../MANUAL.md) (Spanish) explains the language in depth. The [overview](index.en.md)
gathers every chapter.

<!-- sync: sha256:0ac4b996fc1e -->
