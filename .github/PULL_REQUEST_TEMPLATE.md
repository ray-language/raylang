## Qué cambia

<!-- Una frase: qué entrega este PR y por qué (el hallazgo/necesidad que lo motiva). -->

## Tipo

- [ ] Runtime/motores (VM, intérprete, nativo — recordar byte-identidad)
- [ ] Stdlib / paquetes (superficie de usuario)
- [ ] Tooling (CLI, LSP, fmt, test runner)
- [ ] Solo docs
- [ ] **Módulo nuevo** → usa el template dedicado: añade `?template=new_module.md` a la URL de este PR

## Checklist

- [ ] Los tests de los archivos tocados pasan (`cargo test --test <suites>`)
- [ ] Guardas: `fmt_policy`, `naming_policy` (y `module_policy` si toca `std/`/`packages/`)
- [ ] Si cambia el COMPORTAMIENTO del lenguaje: **SPEC.md actualizada primero** y espejo selfhost en tándem
- [ ] Docs-contrato al día: DESIGN.md (crónica), REFERENCE.md, CHANGELOG.md ("Sin publicar"), IDEAS.md si cierra/abre ideas
- [ ] Mensajes de cara al usuario en INGLÉS; commits en Conventional Commits en español; sin firmas
- [ ] PR directo contra `main` (sin apilar sobre otros PRs)
