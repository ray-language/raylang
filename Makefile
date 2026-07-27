# Makefile de raylang — punto de entrada único a los comandos del proyecto.
# `make` (o `make help`) lista los targets. Guía completa de builds: docs/build.md.
#
# Los targets son envoltorios finos: cada herramienta sigue siendo invocable directa
# (cargo, tools/pgo.sh, benchmarks/measure.py, mdbook…); esto solo unifica la llamada.

# Rust se instala vía rustup y no está en el PATH por defecto (gotcha de CLAUDE.md).
export PATH := $(HOME)/.cargo/bin:$(PATH)

# Variables sobreescribibles: make run FILE=examples/basics/fib.ray ARGS="1 2"
FILE  ?= examples/basics/fib.ray
ARGS  ?=
LABEL ?= medición

.DEFAULT_GOAL := help
.PHONY: help build run run-interp repl test test-one test-slow clippy ci \
        release slim pgo pgo-slim playground playground-serve \
        bench bench-gate bench-record bench-vs-interp bench-poly bench-poly-build \
        bench-web bench-web-build \
        book book-serve vscode install clean clean-cache

help: ## Lista los targets disponibles
	@awk 'BEGIN {FS = ":.*## "; printf "\nUso: make <target> [VAR=valor]\n\n"} \
	     /^[a-zA-Z_-]+:.*## / {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2} \
	     /^##@/ {printf "\n\033[1m%s\033[0m\n", substr($$0, 5)}' $(MAKEFILE_LIST)
	@echo ""

##@ Desarrollo

build: ## Build de desarrollo (debug)
	cargo build

run: ## Ejecuta un programa en la VM: make run FILE=prog.ray [ARGS="…"]
	cargo run --quiet --bin ray -- $(FILE) $(ARGS)

run-interp: ## Ejecuta con el intérprete (oráculo): make run-interp FILE=prog.ray
	cargo run --quiet --bin raylang -- --interp $(FILE) $(ARGS)

repl: ## REPL interactivo (sobre la VM)
	cargo run --quiet --bin ray

test: ## Batería completa de tests (lib + integración)
	cargo test

test-one: ## Una suite de integración: make test-one T=selfhost_checker
	cargo test --test $(T)

test-slow: ## Tests #[ignore] lentos (metacircularidad, TCO de la VM auto-alojada)
	cargo test --test selfhost_metacircular -- --ignored
	cargo test --test selfhost_metacircular_vm -- --ignored
	cargo test --test selfhost_vm -- --ignored

clippy: ## Lints (como el CI)
	cargo clippy --all-targets

ci: ## Réplica local del CI: clippy + tests + release + build solo-VM
	cargo clippy --all-targets
	cargo test
	cargo build --release
	cargo build --release --no-default-features

##@ Release y binarios (detalle: docs/build.md)

release: ## Release normal, features default (~6,1 MB) → target/release/{ray,raylang}
	cargo build --release

slim: ## Release slim total: solo 'interp', sin sqlite/net-tls/ffi (~2,9 MB)
	cargo build --release --no-default-features --features interp

pgo: ## Release default optimizada por perfil (tools/pgo.sh; para cortar releases)
	sh tools/pgo.sh

pgo-slim: ## Release slim + PGO
	sh tools/pgo.sh --slim

playground: ## Compila el playground web a wasm (playground/raylang.wasm)
	sh playground/build.sh

playground-serve: ## Sirve el playground en http://localhost:8000
	cd playground && python3 -m http.server 8000

##@ Medición (banco sobre la VM de release; recompila antes si tocaste src/)

bench: ## Banco mejor-de-15: make bench LABEL="etiqueta"
	python3 benchmarks/measure.py "$(LABEL)"

bench-gate: ## Gate de regresión contra benchmarks/baseline.json (>5% = falla)
	python3 benchmarks/regress.py

bench-record: ## Graba el baseline en esta máquina (release PLANO, no PGO)
	python3 benchmarks/regress.py --record

bench-vs-interp: ## Compara VM vs intérprete con hyperfine: make bench-vs-interp FILE=prog.ray
	benchmarks/bench.sh $(FILE)

bench-poly-build: ## Compila los binarios del banco poliglota (requiere ray/go/rustc en PATH)
	cd benchmarks/poly && ./build-all.sh

bench-poly: ## Banco poliglota (tiempo+memoria): make bench-poly PROG=wordcount (o PROG=all)
	cd benchmarks/poly && ./bench.py $(or $(PROG),list)

bench-web-build: ## Compila el banco de carga web (requiere ray/go/cargo en PATH)
	benchmarks/web/build-all.sh

bench-web: ## Banco de carga web (requiere oha): make bench-web [ARGS="--only ray,hyper"]
	cd benchmarks/web && ./webbench.py $(ARGS)

##@ Documentación y tooling

book: ## Compila el libro (mdbook) → book/book/
	mdbook build book

book-serve: ## Sirve el libro con recarga en vivo
	mdbook serve book

vscode: ## Compila la extensión de VSCode (npm install + tsc)
	cd editors/vscode && npm install --no-fund --no-audit && npm run compile

install: ## Enlaza target/release/{ray,raylang} en ~/.local/bin (compila release si falta)
	@test -x target/release/ray || cargo build --release
	mkdir -p $(HOME)/.local/bin
	ln -sf $(CURDIR)/target/release/ray $(HOME)/.local/bin/ray
	ln -sf $(CURDIR)/target/release/raylang $(HOME)/.local/bin/raylang
	@echo "enlazados: ~/.local/bin/{ray,raylang} → target/release"

clean: ## Limpia TODO target/ (⚠ rompe el symlink de `make install` y el binario PGO; suele bastar clean-cache)
	cargo clean
	rm -rf target/pgo-gen

clean-cache: ## Libera los cachés de build (debug + builds especiales, ~25G) CONSERVANDO target/release (el ray instalado/PGO)
	rm -rf target/debug target/pgo-gen target/wasm32-unknown-unknown target/profiling \
	       target/pgo target/slim target/slim3 target/plain-check target/pgo-use
	@du -sh target 2>/dev/null | awk '{print "target/ ahora ocupa " $$1}'
