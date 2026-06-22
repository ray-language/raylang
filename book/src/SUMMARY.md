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
