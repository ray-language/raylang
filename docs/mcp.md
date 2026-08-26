# El servidor MCP de raylang (`ray mcp`)

"El LSP para agentes" (IDEAS §51, pieza B): un servidor [MCP](https://modelcontextprotocol.io)
embebido en el binario `ray` que da a un LLM el bucle **escribir → verificar → corregir**. La
alucinación se convierte en iteración: el modelo escribe raylang, `ray_check` le devuelve los
diagnósticos exactos (posicionados, hasta 20), y `ray_run` verifica el comportamiento.

## Conectarlo

Claude Code:

```sh
claude mcp add raylang -- ray mcp
```

Cualquier cliente MCP (Claude Desktop, etc.), en su config de servidores:

```json
{ "mcpServers": { "raylang": { "command": "ray", "args": ["mcp"] } } }
```

No hay nada más que configurar: el servidor habla JSON-RPC 2.0 por stdio (mensajes delimitados
por línea), sin dependencias ni estado.

## Las tools

| Tool | Argumentos | Qué devuelve |
|---|---|---|
| `ray_check` | `code` | `ok` o los diagnósticos exactos del compilador (posición + línea + `^`) |
| `ray_run` | `code`, `stdin?` | `exit` (el `int` de `main`) + stdout + stderr |
| `ray_test` | `code` | el reporte del runner `@test`; `exit` 0 = verde, 1 = fallos |
| `ray_fmt` | `code` | el fuente canónico (`ray fmt`) |
| `ray_doc` | `symbol` | firma + doc de un builtin o `std/*` (`len`, `json.parse`, `crypto.x25519_public_key`…) |

Y un *resource*: **`raylang://llms.txt`** — el contexto destilado de la pieza A (delta contra
Rust, formas canónicas, mensajes de error exactos). Un cliente puede inyectarlo al contexto del
modelo antes de escribir la primera línea.

## Confinamiento

`ray_run`/`ray_test` ejecutan código arbitrario del modelo → corren **en un subproceso** del
propio binario (aislamiento por proceso; el stdout del invitado nunca toca el canal MCP) con
los límites de embebido de M42:

- **fuel**: 100 M de instrucciones de la VM (un bucle infinito muere por fuel, con error claro;
  `RAYLANG_MCP_FUEL` lo ajusta — lo usan los tests para que el corte por fuel gane siempre al
  plazo de pared, también en builds debug lentos);
- **heap**: 1 M de objetos vivos;
- **plazo de pared**: 10 s con `kill` (para lo que no consume fuel: red, stdin bloqueado);
- salida truncada a 64 KiB por flujo;
- `--deterministic` (M:1): la misma entrada produce la misma salida — el agente puede comparar.

Un diagnóstico del compilador **no** es un error de la tool (`isError: false`): es el feedback
que el modelo necesita. `isError: true` queda para fallos del envoltorio (timeout, E/S).

## Implementación

`src/mcp.rs` (~300 líneas), cliente 100% externo como el LSP/REPL/runner: cero cambios en el
core, cero dependencias (el JSON es el del LSP, `lsp::json`). Tests: unitarios en memoria
(`serve` es genérico sobre los flujos) + `tests/mcp_cli.rs` (el servidor real por stdio,
las cinco tools de punta a punta, incluida la bomba de bucle cortada por fuel).

## Tras actualizar `ray`

El servidor MCP es un proceso de larga vida: **Claude Code lo arranca al abrir la sesión y
no lo reemplaza aunque reinstales el binario**. Tras un `make install` (o cualquier upgrade),
reinicia la sesión de Claude Code (o reconecta el servidor con `/mcp`) para que las
correcciones del lado MCP apliquen — un servidor viejo puede exhibir bugs ya corregidos
(p. ej. el del escaneo de templates en el /tmp compartido, arreglado en jul 2026).

`ray_doc` acepta, además de builtins y funciones del prelude, las funciones públicas de los
módulos `std/*` embebidos: `ray_doc("json.parse")`, `ray_doc("regex.find_all")` — o el nombre
a secas, que se busca en todos los módulos.
