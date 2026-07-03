# Política de seguridad de raylang

raylang es un **proyecto de aprendizaje** para construir un lenguaje de programación de principio a fin.
No obstante, se toma la seguridad en serio: el runtime es memory-safe por construcción y el proyecto se
endureció deliberadamente (M42–M43). Este documento explica el modelo de seguridad, qué cuenta como
vulnerabilidad y cómo reportarla.

## Versiones soportadas

| Versión | Soporte |
|---------|---------|
| `1.0.0-beta.x` (línea actual) | ✅ correcciones de seguridad |
| `< 1.0.0-beta` | ❌ |

Tras la 1.0, se soportará la última línea estable publicada.

## Cómo reportar una vulnerabilidad

**No abras un issue público** para una vulnerabilidad. Repórtala en privado a:

- **dev@rayala.org** (asunto con el prefijo `[SECURITY]`)
- o, si el repositorio lo tiene habilitado, vía **GitHub Security Advisories** (pestaña *Security* →
  *Report a vulnerability*).

Incluye, en lo posible: una descripción del problema, un caso mínimo que lo reproduzca (un `.ray` y/o los
pasos), la versión/commit afectado, y el impacto que estimas.

**Compromiso de respuesta** (best-effort, al ser un proyecto de aprendizaje): acuse de recibo en **72 h**,
una primera evaluación en **7 días**, y divulgación **coordinada** (se acuerda un embargo razonable hasta
que haya corrección; se te dará crédito si lo deseas).

## Modelo de seguridad

Lo que raylang **garantiza por construcción**:

- **Memory safety.** El runtime está escrito en Rust *safe*; el lenguaje **no tiene `null`** (los errores
  son valores: `Option`/`Result`/`?`) y la memoria compuesta la gestiona un **GC mark-and-sweep** (sin
  use-after-free ni doble free en raylang puro).
- **Sin data races por construcción.** El modelo de concurrencia es de **actores con aislamiento de heap**
  (M38): cada fibra tiene su propio heap y la única comunicación entre ellas son **canales** que transfieren
  la propiedad del valor. No hay estado mutable compartido → *data-race freedom* sin necesidad de *ownership*
  en el sistema de tipos.
- **Confinamiento opcional.** Para embeber raylang como lenguaje de *scripts* no confiables, hay **límites de
  recursos** (`ray run --fuel N` acota las instrucciones; `--heap N` acota los objetos vivos): un bucle
  infinito o una entrada maliciosa **no cuelgan ni agotan la memoria** del anfitrión.
- **Compilador sin pánicos.** El front-end (lexer/parser/checker) convierte toda entrada del usuario en un
  **error con posición**, nunca en un *panic* de Rust. Los fallos de invariante interna se centralizan en un
  `ice!()` (Internal Compiler Error) que pide un reporte de bug. Esto se verifica con **fuzzing continuo**
  (`tests/fuzz_frontend.rs`, corre en cada `cargo test` + una campaña nocturna) y una política de ICE
  (`tests/ice_policy.rs`).
- **Superficie de supply-chain mínima.** El proyecto es **cero-dependencias de Cargo salvo una excepción
  consciente**: TLS/criptografía vía `rustls`/`ring` (que ya se auditan ampliamente). El gestor de paquetes
  usa un **lockfile con hashes SHA-256** por dependencia (verificados en cada resolución) para detectar
  manipulación de la cadena de suministro.

### La frontera insegura: FFI

La **única** vía por la que un programa raylang puede salirse de las garantías anteriores es el **FFI**
(`extern "lib" { fn … }`, M41): permite cargar y llamar a **código C arbitrario** (`dlopen`/`dlsym`).
**Declarar una función `extern` ES el acto que asume la responsabilidad de seguridad** (no hay un bloque
`unsafe {}` por llamada porque la declaración ya lo es). Todo lo que ocurra al otro lado de esa frontera
(corrupción de memoria, UB, etc.) **es responsabilidad de quien la declara**, exactamente como el `unsafe`
de Rust. El *playground* web y las builds `wasm32` **no** incluyen FFI (ni red/TLS/cripto).

### Bloques `unsafe` de Rust

El runtime contiene bloques `unsafe` acotados y auditados (M42), cada uno con su invariante `SAFETY`
documentada:

- **`src/ffi.rs`** — la frontera FFI (`dlopen`/`dlsym` declarados a mano, `transmute` del puntero al tipo
  de función según la firma declarada, `CStr::from_ptr` solo sobre punteros no-NULL con la `CString` viva).
- **`src/poll.rs`** — las llamadas al sistema del *poller* de E/S (`kqueue`/`epoll`), declaradas a mano para
  no traer `libc` (respeta cero-deps).
- **`src/wasm.rs`** — la ABI de memoria del *playground* (`alloc`/`run`/`dealloc` sobre la memoria lineal
  del módulo), solo en `wasm32`.
- **`src/vm.rs`** — la aserción `Send`/`Sync` sobre la referencia **inmutable** al programa compilado
  (`ProgRef`, M38.3), compartida entre hilos worker sin mutación.

## Qué cuenta como vulnerabilidad

**Sí** son vulnerabilidades a reportar:

- Corrupción de memoria, *use-after-free* o UB alcanzable **sin usar FFI** (desde raylang puro).
- Un *panic*/ICE de Rust (crash del proceso) provocado por **una entrada al compilador o un programa
  válido** (el compilador debe dar un error limpio, no morir).
- Un **escape del confinamiento** (`--fuel`/`--heap`): un programa que los burla y cuelga o agota la memoria
  del anfitrión.
- Verificación de supply-chain rota (un hash del lockfile que no detecta una manipulación).
- Un fallo en la verificación de certificados TLS del cliente HTTP/red.

**No** son vulnerabilidades (comportamiento por diseño, documentado):

- Que un programa que **declara y usa FFI** haga algo inseguro — es la frontera insegura por definición.
- Que la criptografía **pura en raylang** (`examples/`, material pedagógico) no sea de tiempo constante —
  por eso el código de producción usa `ring`; las puras son demostración del lenguaje, no para seguridad.
- Que un programa mal escrito produzca un resultado incorrecto sin violar las garantías del runtime.

## Alcance

Este proyecto es pedagógico y **no está pensado para producción crítica**. Aun así, las garantías de arriba
son reales y los reportes son bienvenidos. La corrección de una vulnerabilidad confirmada se prioriza sobre
el trabajo de features.
