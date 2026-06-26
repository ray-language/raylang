# Construyendo raylang

[Prefacio](prefacio.md)

---

# Diseño

- [Decisiones fundacionales](diseno/decisiones-fundacionales.md)

# M1 — El front-end y el intérprete

- [El lexer](m1/lexer.md)
- [El parser](m1/parser.md)
- [El checker](m1/checker.md)
- [El intérprete](m1/interprete.md)

# M2 — La máquina virtual

- [La máquina virtual](m2/intro.md)
- [Bytecode y la pila](m2/bytecode-y-pila.md)
- [Control de flujo](m2/control-de-flujo.md)
- [Variables y llamadas](m2/variables-y-llamadas.md)

# M3 — Datos compuestos

- [Datos compuestos](m3/intro.md)
- [Arreglos](m3/arreglos.md)
- [Structs](m3/structs.md)

# M4 — Funciones, closures y memoria

- [Funciones, closures y memoria](m4/intro.md)
- [Funciones de primera clase](m4/primera-clase.md)
- [Closures: capturar el entorno](m4/closures.md)
- [El recolector de basura](m4/recoleccion-de-basura.md)

# M5 — Tipos suma y pattern matching

- [Tipos suma y pattern matching](m5/intro.md)
- [Enums: uniones etiquetadas](m5/enums.md)
- [match: consumir por casos](m5/match.md)
- [match en la máquina virtual](m5/match-en-la-vm.md)

# M6 — Genéricos, Option y Result

- [Genéricos, Option y Result](m6/intro.md)
- [Funciones genéricas e inferencia](m6/funciones-genericas.md)
- [Tipos genéricos y chequeo bidireccional](m6/tipos-genericos.md)
- [Option, Result y el operador `?`](m6/option-result.md)

# M7 — UFCS, pipelines y stdlib

- [UFCS, pipelines y stdlib](m7/intro.md)
- [UFCS: funciones como métodos](m7/ufcs.md)
- [Pipelines: el operador `|>`](m7/pipelines.md)
- [La stdlib en el propio lenguaje](m7/stdlib.md)

# M8 — Inferencia local, REPL y mejores errores

- [Inferencia local, REPL y mejores errores](m8/intro.md)
- [Inferencia local: `let x = 3`](m8/inferencia-local.md)
- [El REPL: un cliente externo](m8/repl.md)
- [Mejores errores: contexto de fuente](m8/errores.md)

# M9 — Traits

- [Traits: comportamiento polimórfico](m9/intro.md)
- [trait e impl: despacho estático](m9/traits.md)
- [Bounds: genéricos que exigen comportamiento](m9/bounds.md)
- [Impls genéricos: diccionarios anidados](m9/impls-genericos.md)
- [Métodos por defecto](m9/defaults.md)
- [Trait objects: despacho dinámico](m9/trait-objects.md)

# M10 — Tooling: anotaciones y LSP

- [Tooling: anotaciones y LSP](m10/intro.md)
- [@test y @derive (Eq, Show)](m10/anotaciones.md)
- [El LSP: diagnósticos en vivo](m10/lsp.md)
- [Hover e ir-a-definición](m10/hover-definicion.md)

# M11 — Módulos, I/O y stdlib

- [La stdlib de string](m11/strings.md)
- [Arreglos, `sort` y el registro de builtins](m11/arreglos-y-sort.md)
- [I/O y la API de runtime](m11/io.md)
- [Módulos y `pub`](m11/modulos.md)

# M12 — Concurrencia

- [Concurrencia: CSP sobre la VM](m12/intro.md)
- [Fibras y canales](m12/canales.md)
- [Structured concurrency](m12/structured.md)
- [select: multiplexar canales](m12/select.md)

# M13 — Habilitadores de self-hosting

- [Map&lt;K, V&gt;: diccionarios](m13/mapas.md)
- [panic, assert y el runner de pruebas](m13/aserciones.md)
- [Recursión profunda y llamadas en cola](m13/recursion.md)

# M14 — Self-hosting: raylang en raylang

- [El plan y el oráculo](m14/intro.md)
- [El lexer auto-alojado](m14/lexer.md)
- [El parser auto-alojado](m14/parser.md)
- [El checker auto-alojado](m14/checker.md)
- [El intérprete auto-alojado](m14/interprete.md)
- [El loader: juntar módulos](m14/loader.md)
- [Meta-circularidad](m14/meta-circularidad.md)

# M15 — Redes y la base moderna

- [Redes y la base moderna](m15/intro.md)
- [Sockets TCP: el transporte](m15/sockets.md)
- [Protocolos en raylang: JSON y HTTP](m15/protocolos.md)
- [El servidor concurrente](m15/concurrente.md)

# M16 — El tipo bytes

- [Datos binarios: el tipo bytes](m16/bytes.md)

# M17 — E/S asíncrona real

- [epoll y kqueue: readiness del SO](m17/readiness.md)

# M19 — La capa web

- [Un servidor web concurrente y SSE](m19/servidor-web.md)
- [HTTP en bytes](m19/http-bytes.md)

# Optimización de la VM

- [Medir, cambiar, a veces revertir](opt/intro.md)
