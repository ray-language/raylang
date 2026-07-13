# Guía de construcción del binario

Todas las formas de construir `ray`/`raylang`, de la release normal al binario slim
para contenedores, y cómo combinar cada una con PGO (`tools/pgo.sh`). Contexto de
las decisiones: IDEAS §45 (PGO), §47 (features slim, arco M89).

> Recuerda: `source "$HOME/.cargo/env"` antes de cualquier `cargo` (PATH).

## 1. Builds base

| Build | Comando | Resultado |
|---|---|---|
| Desarrollo | `cargo build` | `target/debug/{ray,raylang}` |
| Release normal | `cargo build --release` | `target/release/{ray,raylang}` (~6,1 MB) |
| Release PGO | `sh tools/pgo.sh` | el mismo `target/release`, optimizado por perfil |

El perfil release ya lleva `strip = "symbols"` (Cargo.toml): los símbolos se quitan
siempre, no hace falta `strip` manual. El symlink `~/.local/bin/ray` apunta a
`target/release/ray` → **lo que dejes ahí es lo que corre el día a día**.

## 2. Features de compilación (arco M89)

Cuatro features, todas activas por defecto (el binario normal es idéntico a siempre).
Cada una se puede excluir para adelgazar el binario o reducir superficie de ataque;
un programa que use la capacidad excluida recibe un **error claro como valor** (nunca
un fallo silencioso) y el checker acepta el programa igual (la tabla BUILTINS se
registra siempre).

| Feature | Qué trae | Sin ella |
|---|---|---|
| `interp` | el intérprete de árbol (`--interp`, oráculo de desarrollo) | solo la VM (el motor de producto); `--interp` no disponible |
| `sqlite` | rusqlite `bundled` (el C de SQLite embebido) | `db/sqlite` → `Err` claro en `connect` |
| `net-tls` | rustls+ring+webpki: https/wss + sha/hmac/ed25519/chacha | TLS falible → `Err`-valor; cripto infalible → aborta con error claro; `ray publish --sign`/verificación de firmas no disponibles (fail-closed, explícito). El hash del lockfile NO depende de esto (`src/sha256.rs` es Rust puro) |
| `ffi` | `libloading`: carga de librerías nativas (`extern fn`) | propiedad "no puede cargar código nativo" (contenedores endurecidos); llamar un `extern fn` → error de ejecución claro |

### Combos medidos (M3 Pro, jul 2026)

| Combo | Comando | Tamaño |
|---|---|---|
| Default | `cargo build --release` | 6,1 MB |
| Sin SQLite | `cargo build --release --no-default-features --features interp,net-tls,ffi` | 4,4 MB (−28%) |
| **Slim total** | `cargo build --release --no-default-features --features interp` | **2,9 MB (−53%)** |
| Slim sin oráculo | `cargo build --release --no-default-features` | el mínimo; solo VM |

El CI ya ejercita `--no-default-features` (guardia anti-bitrot del build slim).

## 3. PGO (`tools/pgo.sh`)

Tres pasos: build instrumentado → entrenamiento (banco + strings/iter + parser
auto-alojado + concurrencia) → build final con el perfil, en `target/release`.
Requiere `rustup component add llvm-tools`.

```sh
sh tools/pgo.sh                      # release default + PGO
sh tools/pgo.sh --slim               # slim total (interp solo) + PGO
sh tools/pgo.sh --features "interp,net-tls"   # combo a medida + PGO
```

El set de features se aplica a los DOS builds (instrumentado y final), así el perfil
casa función a función. Las cargas de entrenamiento no usan TLS/SQLite/FFI → valen
para cualquier combo.

**Qué esperar (medido, jul 2026)**: el binario PGO da tiempos estables
(fib(35) ~1,59 s en el M3 Pro). El delta contra el build plano **depende del layout
que le tocó al plano ese día**: se midió −5 a −10% en el cierre de la ronda 2 y ~0-4%
tras el renombrado ES→EN (un cambio masivo de símbolos rebaraja las codegen units y el
layout por defecto cayó casi óptimo por azar). La métrica estable es el **tiempo
absoluto del binario PGO**, no el % relativo; PGO es la *garantía* del layout bueno
para cortar releases. Medición: `python3 benchmarks/measure.py "etiqueta"` (mejor de 15).

**Gotcha de caché**: cambiar `RUSTFLAGS` (lo que hace el paso 3) invalida la caché de
`target/release` → el siguiente `cargo build --release` a secas recompila entero, y
viceversa. El script es para cortar releases; el ciclo de desarrollo sigue con cargo
a secas.

## 4. Flags extra de adelgazamiento (opcionales)

No están en el perfil por defecto (alargan la compilación o tienen trade-offs). Se
activan por variable de entorno, sin tocar Cargo.toml, y **componen con `pgo.sh`**
(el script hereda el entorno):

```sh
# LTO fat + 1 codegen unit: mejor inlining cruzado y algo menos de binario.
# Compilación notablemente más lenta; candidato natural para el corte de release.
CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 sh tools/pgo.sh --slim

# panic=abort: quita el código de unwinding (~100-300 KB).
# Seguro hoy: el único catch_unwind vive en un test (diagnostic.rs) y el hook del
# ICE banner (M33b) corre antes del abort. Revalidar si algún día un worker
# captura panics.
CARGO_PROFILE_RELEASE_PANIC=abort cargo build --release --no-default-features --features interp
```

`opt-level = "z"` (optimizar por tamaño) **no se recomienda**: el producto es una VM
y el lazo de despacho paga la des-optimización; el ahorro de tamaño es menor que el
de las features slim.

## 5. wasm (playground, M44a)

`cargo build --release --target wasm32-unknown-unknown` — el `cdylib` exporta
`alloc`/`run`/`dealloc`. ring/rustls/rusqlite quedan fuera solos (dependencias
condicionadas por target); el playground embarca solo el lenguaje núcleo.
