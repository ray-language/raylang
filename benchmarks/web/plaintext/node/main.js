// Carga `plaintext` — Node con el módulo `node:http` de la stdlib, sin express ni fastify.
//
// Es el escalón PELADO: comparar `web/framework` contra esto mezclaría el coste del azúcar
// con el del I/O. El escalón de framework (express/fastify vs web/framework) es un banco
// aparte; ver README.md §Escalones.
//
// Un solo proceso, sin `cluster`: raylang sirve desde un proceso y Go también, así que
// multiplicar procesos aquí mediría otra cosa.
const http = require("node:http");

const host = process.argv[2];
const port = process.argv[3];
if (!host || !port) {
  console.error("uso: plaintext-node <host> <puerto>");
  process.exit(2);
}

const body = Buffer.from("Hello, World!");

http
  .createServer((req, res) => {
    res.writeHead(200, { "Content-Type": "text/plain", "Content-Length": body.length });
    res.end(body);
  })
  .listen(Number(port), host);
