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

Cinco features, todas activas por defecto (el binario normal es idéntico a siempre).
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
| `regex` | el crate `regex` vía `ray-runtime`: la VM despacha las `run_*` de `std/regex` al motor nativo (R7; mismo borde y dialecto que el binario transpilado, R5) | `std/regex` funciona IGUAL (misma salida): la Pike VM raylang se interpreta tal cual — más lenta (~30×), sin pérdida de capacidad. `RAYLANG_REGEX_PIKE=1` fuerza este camino aun con la feature |

### Combos medidos (M3 Pro, jul 2026)

| Combo | Comando | Tamaño |
|---|---|---|
| Default | `cargo build --release` | 6,1 MB |
| Sin SQLite | `cargo build --release --no-default-features --features interp,net-tls,ffi,regex` | 4,4 MB (−28%; medido antes de la feature `regex`) |
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

## 6. Compilar un PROGRAMA a binario nativo (`ray build --native`, arco P2.b)

Todo lo anterior construye la **toolchain** (el binario `ray`/`raylang`: la VM + el
resto). Distinto es compilar un **programa de usuario** a un ejecutable nativo:
`ray build --native prog.ray` **transpila el programa a Rust** y lo compila con `rustc`
a un binario de código máquina (24–61× la VM, byte-idéntico; ver el capítulo del libro
*Compilación a binario nativo* y `docs/transpilador-nativo.md` para el diseño).

```sh
ray build --native prog.ray            # rustc -O (opt2), ~0,2 s, portable
ray build --native prog.ray --release  # opt3+lto+cu1+target-cpu=native (no portable)
ray build --native prog.ray --fast     # int wrapping (sin check de overflow; div/0 sí se chequea)
ray build --native prog.ray -o bin/app # nombre de salida
ray build --native prog.ray --target x86_64-unknown-linux-gnu   # cross-compile (H20)
```

**Cross-compilation** (`--target <triple>`). El triple se pasa tal cual a `rustc`/`cargo`;
el target debe estar instalado (`rustup target add <triple>`) y, para targets con
linker cruzado (p. ej. Linux desde macOS), el linker configurado (`~/.cargo/config.toml`).
Con `--target`, `--release` omite `target-cpu=native` (release **portable** al target).

**Crates de producción bajo demanda.** El código de TLS/cripto/SQLite vive en el crate
del workspace `crates/ray-runtime` (features `tls`/`crypto`/`sqlite`), del que dependen
**tanto la VM como el binario transpilado** → paridad por construcción. Si el programa
usa uno de esos subsistemas, `build --native` genera un proyecto Cargo temporal con
`ray-runtime` (fuentes incrustadas vía `include_str!`) + la feature detectada y compila
con `cargo`; si no toca ninguno, compila con `rustc` pelado (rápido, sin red). La caché
de target es compartida y **persistente** (`~/.ray/native-cache/`, H14): ring/rustls se
compilan una vez por máquina. Ahí también se persiste el **`Cargo.lock` resuelto**
(`ray-native.Cargo.lock`, H20): los builds siguientes de esta máquina fijan las mismas
versiones de deps (reproducibilidad por máquina; entre máquinas puede variar dentro de
los rangos declarados).

**mimalloc por defecto (N1, jul 2026).** El binario transpilado enlaza **mimalloc** como
`#[global_allocator]` (feature `mimalloc` de `ray-runtime`, mismo crate/versión que usa el
binario `ray` desde P0.4). Sin él, el binario caía al malloc del sistema — lento en churn
de strings pequeños (macOS). Medido (bench políglota, `docs/bench-poliglota-optimizacion.md`):
wordcount/logparse **−40 %**, jsonserialize **−18 %**. Consecuencia: el build nativo por
defecto va por el **camino Cargo** (con la caché compartida, mimalloc se compila una vez por
máquina); `--without mimalloc,ahash,fibers` recupera el `rustc` pelado (sin Cargo/red).

**Regex acelerado (R5, jul 2026).** Si el programa usa `std/regex`, el nativo enlaza el crate `regex` de Rust vía `ray-runtime` (feature detectada por uso): mismo comportamiento que la Pike VM de la librería (dialecto traducido, validación raylang) a velocidad de Rust — medido 570→71 ms en el bench regex, por delante de Go. `--without regex` recupera la Pike VM transpilada (raylang puro).

**aHash por defecto (N2, jul 2026).** Los `Map` del binario transpilado usan **aHash**
(feature `ahash` de `ray-runtime`) — el mismo hasher que el `MapStore` de la VM desde P0.1;
el `HashMap` std con SipHash es lento en claves string. Medido: wordcount **−8.5 %**
adicional sobre mimalloc (neutro donde el Map no domina). La resistencia a hash-flooding
se conserva (RandomState con RNG de runtime, como la VM).

**Fibras (jul 2026).** La concurrencia del binario nativo corre POR DEFECTO sobre el scheduler
M:N de fibras (`ray_runtime::fibers`: corrutinas corosensei + reactor kqueue/epoll) en vez de un
hilo de SO por tarea/conexión — medido en el banco web: techo +16 %, y 14 hilos / 8 MB donde el
modelo antiguo levantaba un hilo por conexión (docs/diseno-concurrencia-nativa.md §7-§8).
`--without fibers` recupera el hilo-por-tarea (y es necesario para la vía `rustc` pelada). En
targets Windows se apaga solo (el reactor es kqueue/epoll).

**Exclusión.** `--without crypto,tls,sqlite,mimalloc,ahash,regex,fibers,process,watch,audio,ui` — para los subsistemas de uso
(crypto/tls/sqlite) fuerza el *stub* que panica; `mimalloc`/`ahash` vuelven al malloc del
sistema / al HashMap std (→ con todo excluido, vía rápida `rustc`); `[native] without =
[...]` en el `ray.toml` fija la política estable del proyecto (la CLI se une a ella). Para
builds herméticos/cross-compile/policy.

**Empaquetado (M147c).** `ray bundle` compone este build (`--native --release` + embed) y lo
deja en el formato del SO: `.app` en macOS, directorio + `.desktop` en Linux. Ver REFERENCE §14.

**Assets embebidos (M147).** `[native] embed = ["assets"]` en el `ray.toml` (o `--embed dirs`
ad-hoc) hornea los directorios dados DENTRO del binario (`include_bytes!`): `std/embed` los lee
por clave ("assets/app.css") con el mismo espacio de nombres que en `ray run` (donde se leen en
vivo del disco). Un dir configurado que no existe aborta el build (exit 64, nombrando el
origen). No es un subsistema de ray-runtime: un programa solo-embed conserva la vía `rustc`
pelada.

> El workspace: el `Cargo.toml` raíz declara `[workspace] members = ["crates/ray-runtime"]`.
> `ray-runtime` es dep **opcional** (no-wasm) del binario `ray`, activada por `net-tls`
> (feature `crypto`); su `tls`/`sqlite` solo se enlazan en el proyecto GENERADO del
> binario transpilado, no en la VM (que compila rustls/rusqlite directo tras sus features).

### 6.b Android (M156): el programa como `.so` (modo lib)

`ray build --native --lib --target aarch64-linux-android prog.ray -o libray_app.so` compila el
programa como **cdylib** cargable con `System.loadLibrary` (los símbolos JNI `JNI_OnLoad` /
`Java_org_raylang_shell_RayBridge_*` van en el propio `.so`). Requisitos: el target rustup
(`rustup target add aarch64-linux-android`) y el **NDK** — se localiza por `ANDROID_NDK_HOME`,
`$ANDROID_HOME/ndk/<versión>` o `~/Library/Android/sdk/ndk/<versión>` (instalable con
`sdkmanager "ndk;27.2.12479018"`). El build inyecta solo el toolchain del NDK como
linker/CC/AR (sin `cargo-ndk`) y alinea el `.so` a 16KB (requisito de Android 15+). minSdk de
referencia: 24 (el clang `aarch64-linux-android24-clang`).
