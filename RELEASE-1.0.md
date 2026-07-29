# Camino a raylang 1.0

Checklist viva hacia el lanzamiento de la **1.0**. Los criterios los fija PRODUCTION.md (arco D → 1.0); aquí
se rastrea su estado honesto. Versión: **`1.0.0`** (bump hecho en `Cargo.toml`/`SPEC.md`; falta empujar el
tag `v1.0.0` para publicar la Release).

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
      completa verde (442 lib + integración).
- [ ] **SPEC publicada** — 🟡
      `SPEC.md` está **escrita y es normativa** (versiona con el lenguaje). Falta **publicarla** (hostearla
      como sitio/página). El parser auto-alojado (M14) valida la gramática descrita.
- [ ] **Benchmarks dentro del presupuesto** — 🟡
      Hay banco (`benchmarks/`) + **guarda de regresión** (`tests/perf_regression.rs`, opt-in) que falla si
      degrada >5%. M36.1 (superinstrucciones) dio un win real medido. El presupuesto **aspiracional** de M36
      (3–5× acumulado vía `HeapValue` 16 B / inline caches) **no se persiguió completo** — decisión de
      priorización. *Interpretación para 1.0*: "dentro del presupuesto" = sin regresión respecto al baseline;
      cumplido. (Optimización profunda de la VM queda como trabajo post-1.0, medido.)

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
      Versión subida a `1.0.0` en `Cargo.toml`/`SPEC.md`; notas en `CHANGELOG.md`; licencia MIT OR
      Apache-2.0 (`LICENSE-MIT`/`LICENSE-APACHE`). Falta **empujar el tag `v1.0.0`** (dispara la Release).

## Notas

- Buena parte de "Distribución" es **externa** (🌐): requiere cuentas del mantenedor (GitHub Releases, brew
  tap, marketplace de VSCode, hosting). Lo **en-repo** (instalador, workflow de release, `SECURITY.md`, subir
  la versión) se puede preparar aquí y ejecutar/publicar cuando se decida.
- Los criterios técnicos que faltan (SPEC/benchmarks) están **hechos en el repo**; su ⬜ es de publicación o
  de decisión de alcance, no de trabajo de código pendiente.
