# Contribuir a raylang

Guía corta y contractual: lo que un PR necesita para ser considerado. El detalle vive en los
documentos-contrato (`SPEC.md`, `DESIGN.md`, `PRODUCTION.md`, `REFERENCE.md`, `SECURITY.md`).

## El flujo

1. **Rama + PR contra `main`**, siempre. Nunca PRs apilados sobre otros PRs (dos veces un merge
   apilado quedó huérfano); si tu trabajo depende de otro PR, espera su merge.
2. **Conventional Commits en español** (`feat(parser): …`, `fix(vm): …`); cada commit compila.
   Sin firmas ni pies de generación en commits ni PRs.
3. Un PR = un paso revisable. La descripción dice **qué** entrega y **por qué** (el hallazgo o
   necesidad que lo motiva), no un diario de lo que hiciste.
4. CI debe quedar en verde: `cargo test` completo corre las guardas (`fmt_policy`,
   `naming_policy`, `module_policy`) además de las suites.

## Los principios no negociables

- **La SPEC manda**: cambiar el comportamiento del lenguaje = actualizar `SPEC.md` PRIMERO. Ante
  conflicto SPEC↔implementación, uno de los dos es un bug y se resuelve explícito.
- **Byte-identidad de motores**: intérprete (oráculo), VM (producto) y binario nativo producen la
  MISMA salida. Todo cambio de comportamiento lleva su verificación en los motores que toque; los
  mensajes del checker van en tándem con su espejo selfhost (`selfhost/checker.ray`).
- **Idioma**: identificadores en inglés; comentarios `//` en español; doc `///` (LSP/raydoc) en
  inglés; TODO lo que el lenguaje entrega al usuario (errores, LSP, CLI) en inglés.
- **Errores como valores** (`Result`/`Option`/`?`); `panic` solo para invariantes rotas.
- **Dependencias de Cargo**: se aceptan cuando son mejor ingeniería que el código artesanal
  (cripto, TLS, tablas Unicode…), y cada una se anota en `SECURITY.md` con su justificación.
- **Posición en todo**: cada token/nodo lleva (línea, columna); los errores reportan ubicación.

## Módulos nuevos (`std/` o `packages/`)

Abre el PR con el template dedicado (`?template=new_module.md` en la URL). La **batería de
admisión** (`tests/module_policy.rs`, corre en CI) exige:

1. Toda la superficie pública (`pub fn/struct/enum/const`) con doc `///` en inglés.
2. Fila en `REFERENCE.md` (el catálogo es contrato); receta en `MANUAL.md` si aporta un flujo.
3. Tests dedicados en `tests/` que ejercitan el módulo por ambos motores (golden determinista);
   los paquetes llevan `README.md` + `ray.toml`.
4. Y las guardas generales: forma canónica (`ray fmt`), naming, mensajes en inglés.

Además de la batería (que es lo verificable), la admisión pondera: **uso real demostrado**
(dogfood: una app, un ejemplo e2e — "compila" no es dogfood), API alineada con el diseño del
lenguaje (§0 de DESIGN.md), raylang puro salvo justificación, y qué queda FUERA a propósito.

## Publicar paquetes (ecosistema)

Los paquetes del ecosistema viven en la organización **github.com/ray-language**; el índice
oficial es **github.com/ray-language/ray-index**. El flujo de publicación (tag inmutable +
`ray registry publish` + hash de contenido) está en `PUBLISH.md`. Para consumo anónimo, publica
con URL **https** (`--repo git+https://…@vX.Y.Z`); una URL ssh limita el paquete a quien tenga
clave.

## Cómo correr lo que CI corre

```sh
source "$HOME/.cargo/env"
cargo test                                    # todo: suites + guardas (fmt/naming/module)
cargo test --test module_policy               # solo la batería de admisión
cargo test --test deps_live_cli -- --ignored  # manejador de paquetes contra GitHub real (red)
sh tools/pgo.sh                               # release optimizada (opcional)
```
