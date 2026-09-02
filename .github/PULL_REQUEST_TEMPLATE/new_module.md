## New module: `<std/name | package/module>`

<!-- What problem it solves and why it deserves to live in the language (a finding from real
     use beats a speculative idea; link the IDEAS.md § if there is one). -->

## Design decisions

<!-- Shape of the API (errors as values, modules vs methods, generics), what is deliberately
     OUT, and alternatives considered. If it adds a Cargo dependency: a SECURITY.md-style
     justification (what class of hand-written code it avoids). -->

## Admission battery (module_policy + guards — everything must be green)

- [ ] **Pure raylang unless justified** (no new runtime; if it needs a primitive/crate, the why above)
- [ ] **Public surface documented**: every `pub` with `///` docs in English (rule 1)
- [ ] **Row in REFERENCE.md** with signatures (rule 2) — and a MANUAL.md section if it has a usage recipe
- [ ] **Dedicated tests in `tests/`** exercising it on BOTH engines (rule 3/4); deterministic golden output
- [ ] **English identifiers, Spanish `//` comments** (`naming_policy`)
- [ ] **Canonical form** (`ray fmt --write` + `fmt_policy`)
- [ ] Errors **as values** (`Result`/`Option`), error messages in ENGLISH
- [ ] If it is a package (`packages/<p>`): `README.md` + `ray.toml` with name/version; if `std/`: registered in `src/stdlib.rs`
- [ ] **Byte-identity**: same output on interpreter, VM and (when its surface applies) the native binary
- [ ] DESIGN.md gains its chronicle section; CHANGELOG.md "Sin publicar"; IDEAS.md marked EXECUTED if it closes one

## Dogfood

<!-- How it was validated with real use: an app, an example, or an e2e test. "It compiles" is not dogfood. -->
