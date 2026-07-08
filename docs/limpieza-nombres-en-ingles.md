# Limpieza: identificadores en inglés (deuda técnica, DIFERIDA)

**Regla** (ya en `CLAUDE.md` § Convenciones): los **identificadores** —nombres de funciones/métodos,
variables, parámetros, tipos y campos— van en **inglés**; los **comentarios** (`//`) en **español**, y la
**documentación** `///` (visible en el LSP/raydoc) en **inglés**. El código antiguo mezcla los dos idiomas
en los nombres (`cargar`, `analizar`, `nombre_fachada`, `receptor`, `otro`, …); esta limpieza lo unifica a inglés.

**Cuándo**: DIFERIDA — se hace **después de cerrar los puntos pendientes** en curso (M49.2 `std/time`+
`std/random`, M49.3 `std/crypto`, y lo que el usuario tenga en cola). Código **nuevo** ya se escribe en
inglés desde ahora.

## Alcance (dos superficies, tres tiers por riesgo)

### A. Rust `src/` — identificadores internos (NO rompe nada)
~66+ funciones con nombre en español (`cargar`, `analizar`, `nombre_fachada`, `fuente_de_modulo_cargado`,
`corto`, `primero`, `ruta_de_path_dep`, …) + muchas **variables locales** y **parámetros** (`receptor`,
`sintetico`, `linea`, `archivo`, `ruta`, `otro`, `siguiente`, …). Todo interno → rename mecánico, sin
efecto observable. Es el grueso pero el más seguro.

### B. Core en raylang — identificadores **internos** (NO rompe nada)
`selfhost/*.ray`, `src/prelude.rs` (SOURCE) y `std/*.ray`: variables locales y helpers privados en español.
`std/` ya está casi todo en inglés (`gcd`, `is_prime`, `binary_search`, `pad_left`, …). Rename interno.

### C. Core en raylang — superficie **USER-FACING** (⚠️ INCOMPATIBLE)
Los **métodos de los traits del prelude** están en español y son parte de la **cara del lenguaje**:
- `Eq { fn igual(self, otro: Self) -> bool }`  → p. ej. `equals`/`eq`
- `Show { fn mostrar(self) -> string }`        → p. ej. `show`/`display`
- `Ord { fn menor(self, otro: Self) -> bool }` → p. ej. `less`/`lt`

Renombrarlos toca **cada `impl`** (incl. los primitivos del prelude, los generados por `@derive(Eq,Show)`,
y los de usuario) y **cada sitio de llamada** (`x.igual(y)`, `x.mostrar()`, `a.menor(b)`) del corpus + de
cualquier código de usuario. **Cambio de lenguaje incompatible** → requiere: (1) actualizar DESIGN.md,
(2) el codegen de `@derive` (genera `fn igual`/`fn mostrar`), (3) el **reescritor AST** (como en M48.4e) para
migrar los sitios del corpus + las fixtures de test, (4) el compilador auto-alojado (`selfhost/*.ray`
también implementa/llama estos métodos), (5) DESIGN/MANUAL/libro/playground. Es el tier de mayor riesgo y
debe ir en su propia fase, verificado con el oráculo.

> Nota: los **nombres de parámetro** de esos métodos (`otro`) son internos (raylang no tiene args con
> nombre) → tier B, no C.

## Plan por fases (sugerido)
- **L1** — Rust `src/` (tier A): rename mecánico, por archivo, `cargo test` verde tras cada tanda. Sin
  cambio observable → la garantía es la regresión.
- **L2** — core raylang interno (tier B): `selfhost`/`prelude`/`std` variables/helpers privados. Oráculo +
  self-hosting byte-idéntico como red de seguridad.
- **L3** — traits del prelude (tier C, incompatible): `igual`/`mostrar`/`menor` → inglés, con reescritor
  AST + `@derive` + self-hosted + docs. Fase aparte, la última.

## Verificación
Tiers A/B: la **suite completa** verde (identificadores internos → sin cambio de comportamiento). Tier C:
además el **oráculo** VM↔intérprete y el **self-hosting** (byte-idéntico) sobre el corpus migrado.
