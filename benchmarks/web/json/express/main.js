// Carga `json` — escalón de FRAMEWORK: Node con express, la referencia de "impuesto de
// framework" en el ecosistema Node (el escalón pelado ya lo cubre `node:http`).
//
// Mismas 10 rutas y misma respuesta que las otras tres. `res.json` no se usa a propósito:
// añadiría el serializador de express sobre un objeto, y aquí las cuatro implementaciones
// interpolan el string directamente.
const express = require("express");

const host = process.argv[2];
const port = process.argv[3];
if (!host || !port) {
  console.error("uso: json-express <host> <puerto>");
  process.exit(2);
}

const app = express();
app.get("/users/:id", (req, res) => {
  res.type("application/json").send(`{"id":"${req.params.id}","name":"Ada"}`);
});
for (const p of ["/", "/health", "/version", "/items", "/items/:id",
                 "/orders", "/orders/:id", "/posts", "/posts/:id"]) {
  app.get(p, (req, res) => res.type("application/json").send("{}"));
}

app.listen(Number(port), host);
