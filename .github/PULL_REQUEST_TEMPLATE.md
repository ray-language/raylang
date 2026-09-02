## What changes

<!-- One sentence: what this PR delivers and why (the finding/need that motivates it). -->

## Kind

- [ ] Runtime/engines (VM, interpreter, native — remember byte-identity)
- [ ] Stdlib / packages (user-facing surface)
- [ ] Tooling (CLI, LSP, fmt, test runner)
- [ ] Docs only
- [ ] **New module** → use the dedicated template: append `?template=new_module.md` to this PR's URL

## Checklist

- [ ] The tests for the touched files pass (`cargo test --test <suites>`)
- [ ] Guards: `fmt_policy`, `naming_policy` (and `module_policy` if `std/`/`packages/` changed)
- [ ] If the language's BEHAVIOR changes: **SPEC.md updated first**, and the selfhost mirror in tandem
- [ ] Contract docs up to date: DESIGN.md (chronicle), REFERENCE.md, CHANGELOG.md ("Sin publicar"), IDEAS.md when it closes/opens an idea
- [ ] User-facing messages in ENGLISH; commits in Conventional Commits in Spanish; no signatures
- [ ] PR opened directly against `main` (no stacking on other PRs)
