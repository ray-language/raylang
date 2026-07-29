# Estado del lanzamiento de raylang 1.0

Checklist viva. **Los criterios técnicos de la 1.0 están cumplidos en el repo** (los fijó el arco D
del plan de producción, hoy resumido en [PRODUCTION.md](PRODUCTION.md)); lo que queda es
**publicación**: empujar el tag `v1.0.0` —que dispara el workflow de release— y las piezas que
dependen de cuentas del mantenedor. La versión ya es `1.0.0` en `Cargo.toml` y en `SPEC.md`.

> Desde entonces el trabajo ha seguido sobre esa línea (binario nativo, fibras, framework web,
> `std/process`…): lo entregado y aún sin publicar se lee en [CHANGELOG.md](CHANGELOG.md). Cuando
> se decida publicar, este documento es la lista de comprobación previa.

Leyenda: ✅ hecho · 🟡 hecho en el repo, falta publicar/ejecutar fuera · ⬜ pendiente · 🌐 requiere cuentas/
servicios externos (del mantenedor).

## Criterios técnicos (los que deciden la 1.0)

- [x] **Compilador sin ICEs / robusto ante entrada arbitraria** — ✅
      Front-end sin pánicos (M33): toda entrada → error con posición o `ice!()`. **Fuzzing continuo**
      (`tests/fuzz_frontend.rs`, en cada `cargo test` + campaña nocturna) con **0 crashes**; política de ICE
      (`tests/ice_policy.rs`). *Antes del tag*: correr una campaña sostenida (varios millones de iteraciones,
      `RAYLANG_FUZZ_ITERS`) y confirmar 0 crashes.
- [x] **`ray` + gestor de paquetes funcionando** — ✅
      M39: `ray new/run/build/test/fmt/doc/lsp/repl`, manifiesto `ray.toml`, lockfile `ray.lock` con hashes
      SHA-256, dependencias git / ruta local / transitivas (MVS ligero).
- [x] **Multicore estable** — ✅
      M38: scheduler M:N por actores (heap aislado por fibra), speedup real medido (3,84× en 4 tareas),
      *stress-tested*; `--deterministic` / `RAYLANG_THREADS=1` para salida reproducible.
- [x] **Política de seguridad vigente** — ✅
      `SECURITY.md` (modelo de seguridad, alcance, proceso de reporte).
- [x] **Motor único de producto + oráculo en desarrollo** — ✅
      La VM es el motor de producto (M35); el intérprete es el oráculo de validación cruzada (verde). Suite
      completa verde: **626 tests unitarios + 101 archivos de integración**. (Después de la 1.0 se sumó un
      tercer motor, el **binario nativo**, con su propio corpus de paridad byte-idéntica.)
- [ ] **SPEC publicada** — 🟡
      `SPEC.md` está **escrita y es normativa** (versiona con el lenguaje). Falta **publicarla** (hostearla
      como sitio/página). El parser auto-alojado (M14) valida la gramática descrita.
- [x] **Benchmarks dentro del presupuesto** — ✅
      Hay banco (`benchmarks/`, con el poliglota y el de carga web) + **guardas de regresión** de tiempo
      (`tests/perf_regression.rs`, falla si degrada >5%) y de **memoria** (pico de RSS). El criterio de la
      1.0 —"sin regresión respecto al baseline"— se cumplió entonces, y el trabajo posterior lo superó con
      creces: el rendimiento pasó a ser el objetivo nº 1 el 14 jul (ver [PERFORMANCE.md](PERFORMANCE.md)).

## Distribución y lanzamiento (M44)

- [x] **Playground web (WASM)** — ✅ M44a
      raylang en el navegador (VM en `wasm32`, cero `wasm-bindgen`). `playground/` + `playground/build.sh`.
- [ ] **Binarios por plataforma + instalador** — 🟡
      Listos en-repo: **`install.sh`** (`curl -sSfL …/install.sh | sh`; detecta OS/arch → target, descarga
      de la Release, instala en `~/.local/bin`) y el workflow de release (abajo) que produce los binarios.
      Falta **ejecutarlos** (empujar un tag → publicar la primera Release) y, opcionalmente, un `brew` tap.
      Verificado localmente: detección de target + round-trip de empaquetado (tar → extrae `ray`/`raylang`
      → corre).
- [ ] **CI de releases** — 🟡
      **`.github/workflows/release.yml`** listo: en un tag `v*`, construye NATIVAMENTE por plataforma
      (macOS arm/intel, Linux x86_64/arm64, Windows) — así `ring`/`rustls` compilan sin cross — empaqueta
      `ray`+`raylang` y los sube a la Release con `gh` (sin acciones de terceros). Falta **dispararlo** con
      un tag. (El CI de **test** —`ci.yml`— ya existía.)
- [ ] **Extensión VSCode publicada** — 🌐
      La extensión existe (`editors/vscode/`, con cliente LSP); falta **publicarla** en el marketplace.
- [ ] **Libro y sitio publicados** — 🌐
      El libro (`book/`, mdBook) existe; falta hostearlo + un sitio de aterrizaje (que puede alojar el
      playground y la SPEC).
- [ ] **Declarar `1.0.0`** — 🟡
      Versión subida a `1.0.0` en `Cargo.toml`/`SPEC.md`; licencia MIT OR Apache-2.0
      (`LICENSE-MIT`/`LICENSE-APACHE`). Falta **empujar el tag `v1.0.0`** (dispara la Release). Antes de
      hacerlo: cerrar el bloque "Sin publicar" del `CHANGELOG.md` con el número de versión que toque —
      lo acumulado desde la 1.0.0 es material de una **1.1** larga, no de un parche.

## Notas

- Buena parte de "Distribución" es **externa** (🌐): requiere cuentas del mantenedor (GitHub Releases, brew
  tap, marketplace de VSCode, hosting). Lo **en-repo** (instalador, workflow de release, `SECURITY.md`, subir
  la versión) está preparado y se ejecuta cuando se decida.
- **Ningún criterio técnico está pendiente de código**: los ⬜/🟡 que quedan son de publicación (hostear la
  SPEC y el libro, subir la extensión, empujar el tag).
- El repositorio **no tiene todavía ningún tag `v*`**: la primera Release está por hacer, y con ella la
  comprobación de que el workflow funciona de punta a punta.
