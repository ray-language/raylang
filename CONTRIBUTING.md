# Contributing to raylang

A short, contractual guide: what a PR needs to be considered. The detail lives in the
contract documents (`SPEC.md`, `DESIGN.md`, `PRODUCTION.md`, `REFERENCE.md`, `SECURITY.md`).

## Quick start

```sh
# Rust is installed via rustup; the Makefile exports the cargo PATH for you.
make help          # every project command (build/run/test/clippy/ci, release, book, …)
cargo test         # full suite: all batteries + the guards (~15 min)
make ci            # local replica of CI: clippy + tests + release + VM-only build
```

Run a program: `cargo run --quiet -- examples/basics/fib.ray` (the VM is the product engine;
`--interp` forces the tree-walking interpreter, the development oracle).

### Directed tests (your inner loop)

The full suite is slow; while iterating, run only the suites for the files you touched:

```sh
cargo test --test term_cli                # e.g. after touching std/term.ray
cargo test -p ray-runtime --features ui   # runtime crate tests need their feature flags
```

CI runs the complete battery on every PR — directed tests are your inner loop, not a
substitute.

## The flow

1. **Branch + PR against `main`**, always. Never stack PRs on other PRs (twice, a stacked
   merge ended up orphaned); if your work depends on another PR, wait for its merge.
2. **Conventional Commits in Spanish** (`feat(parser): …`, `fix(vm): …`); every commit
   compiles. No signatures or generation footers in commits or PRs.
3. One PR = one reviewable step. The description says **what** it delivers and **why** (the
   finding or need that motivates it), not a diary of what you did.
4. CI must be green: the full `cargo test` runs the guards (`fmt_policy`, `naming_policy`,
   `module_policy`) on top of the suites.

## The non-negotiable principles

- **The SPEC rules**: changing the language's behavior = update `SPEC.md` FIRST. On a
  SPEC↔implementation conflict, one of the two is a bug and it gets resolved explicitly.
- **Engine byte-identity**: the interpreter (oracle), the VM (product) and the native binary
  produce the SAME output. Every behavior change carries its verification on the engines it
  touches (grep `assert_on_all_engines` for the pattern); checker messages go in tandem with
  their selfhost mirror (`selfhost/checker.ray`).
- **Language policy**: identifiers in English; `//` comments in Spanish; `///` docs
  (LSP/raydoc) in English; EVERYTHING the language hands the user (errors, LSP, CLI) in
  English.
- **Errors as values** (`Result`/`Option`/`?`); `panic` only for broken invariants.
- **Cargo dependencies**: accepted when they are better engineering than hand-written code
  (crypto, TLS, Unicode tables…), and each one is recorded in `SECURITY.md` with its
  justification.
- **Position in everything**: every token/node carries (line, column); errors report their
  location.

## New modules (`std/` or `packages/`)

Open the PR with the dedicated template (`?template=new_module.md` in the URL). The
**admission battery** (`tests/module_policy.rs`, runs in CI) requires:

1. The whole public surface (`pub fn/struct/enum/const`) with `///` docs in English.
2. A row in `REFERENCE.md` (the catalog is a contract); a recipe in `MANUAL.md` when it
   brings a workflow.
3. Dedicated tests in `tests/` exercising the module on both engines (deterministic golden
   output); packages carry `README.md` + `ray.toml`.
4. Plus the general guards: canonical form (`ray fmt`), naming, messages in English.

Beyond the battery (which is the verifiable part), admission weighs: **demonstrated real
use** (dogfood: an app, an e2e example — "it compiles" is not dogfood), an API aligned with
the language's design (§0 of `DESIGN.md`), pure raylang unless justified, and what is
deliberately left OUT.

## Publishing packages (ecosystem)

Ecosystem packages live in the **github.com/ray-language** organization; the official index
is **github.com/ray-language/ray-index**. The publication flow (immutable tag +
`ray registry publish` + content hash) is in `PUBLISH.md`. For anonymous consumption, publish
with an **https** URL (`--repo git+https://…@vX.Y.Z`); an ssh URL limits the package to
keyholders.

## Running what CI runs

```sh
source "$HOME/.cargo/env"
cargo test                                    # everything: suites + guards (fmt/naming/module)
cargo test --test module_policy               # just the admission battery
cargo test --test deps_live_cli -- --ignored  # package manager against real GitHub (network)
sh tools/pgo.sh                               # optimized release (optional)
```

## Reporting bugs

Use the bug report issue template. The single most useful thing you can include is a
**minimal `.ray` program** plus expected vs. observed output — and if the engines disagree
(`--vm` vs `--interp` vs native), say so: engine divergence is always a bug, and the
highest-priority kind.
