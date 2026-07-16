# Compilar a binario nativo: transpilar a Rust

raylang tiene dos motores para *correr* un programa —el intérprete (oráculo) y la VM (el
producto)—, pero ambos **interpretan**: leen bytecode y despachan en un bucle. Por rápido que
sea ese bucle, el hardware siempre cobra el despacho. La pregunta de este arco es distinta:
¿y si un programa raylang, en vez de correr *sobre* un motor, **se convirtiera en código
máquina** como cualquier binario compilado?

La respuesta es `ray build --native`: un **tercer backend** que traduce el programa a **Rust**
y lo compila con `rustc` a un binario nativo. El modelo es el de Rust mismo —**dev = VM /
deploy = nativo**—: durante el desarrollo corres con la VM (arranque instantáneo, REPL, LSP);
para producción, transpilas a un ejecutable que no lleva ni VM ni intérprete dentro, solo tu
programa como código máquina.

## En dos comandos

```sh
ray run fib.ray                 # desarrollo: sobre la VM
ray build --native fib.ray      # deploy: un binario nativo './fib'
./fib                           # código máquina real
```

`--native` acepta `-o <ruta>` (nombre de salida; por defecto el *stem* del archivo) y
`--release`, un tier de optimización más agresivo (`opt-level=3 + lto=fat + codegen-units=1 +
target-cpu=native`) que da ~10 % extra en cargas de asignación a cambio de ~9× de tiempo de
compilación y un binario no portable (usa las instrucciones de la CPU del host). El tier por
defecto (`-O`) compila un programa pequeño en ~0,2 s y es portable — el mejor equilibrio para
el día a día.

## El salto de rendimiento

El nativo no es "un poco" más rápido que la VM; borra el bucle de despacho por completo.
Medido en un M3 Pro:

| Benchmark | Nativo | VM | node (V8) |
|---|---:|---:|---:|
| fib(35) | **~0,3 s** | ~8 s (24×) | ~1,6 s |
| loopsum | **33 ms** | 808 ms (24×) | 93 ms |

El binario nativo va **24–61× más rápido que la propia VM**, y en cómputo puro (fib) **le gana
a node (V8 con JIT) por 5,4×**. raylang pasa de peor-de-la-clase en el arranque del proyecto a
mejor-de-la-clase, sin cambiar el lenguaje: el mismo programa, otro backend.

## Qué se transpila

El transpilador cubre el **lenguaje completo**: escalares, strings, arreglos, `Map`, structs,
`enum` + `match`, `Option`/`Result` + `?`, closures, genéricos, traits (con *bounds*, impls
genéricos, `dyn`), tuplas, `@derive`, enteros con tamaño… y también **toda la I/O** de
`std/fs` (texto, binaria, handles, directorios, stdin, env, args), `std/time`, `std/random`,
`std/math`, sockets TCP/UDP, la **concurrencia CSP** (`spawn`/canales/`Task`/`join`/`scope`/
`select`/`signals`) —con **hilos de SO reales**, no fibras cooperativas— y **FFI** a C.

El contrato de corrección es el mismo que gobierna los dos motores: **la salida del binario
nativo es byte-idéntica a la de la VM**, verificada con oráculos, nunca asumida. Todo el corpus
de ejemplos deterministas corre idéntico por los dos caminos.

Cómo lo hace, en una frase: cada valor de raylang mapea a una representación Rust natural
(int→`i64`, string→`Rc<str>`, arreglo→`Rc<RefCell<Vec<_>>>`, struct→`Rc<RefCell<…>>`,
closure→`Rc<dyn Fn>`…), la semántica de valor se resuelve clonando al leer los tipos de heap
(para un `Rc` es un *bump* de contador, O(1)), y los traits/genéricos se **borran** (erasure) —
`rustc` monomorfiza y produce código nativo.

## Crates de producción, solo cuando hacen falta

Un `rustc` suelto no puede enlazar dependencias de crates.io. Durante mucho tiempo eso puso un
techo: un programa que usara **TLS**, **criptografía de producción** o **SQLite** no podía
transpilar (esas funciones quedaban como *stubs* que panicaban).

Ese techo está quitado. La idea es **bajo demanda**: si tu programa no toca ningún subsistema
con-crate —el caso común—, se compila con `rustc` pelado, rápido y sin red, como siempre. Pero
si **detecta** que usas `crypto.sha256`, `net.tls_connect` o `db/sqlite`, el transpilador
genera un pequeño **proyecto Cargo** y lo compila con `cargo`, enlazando **solo** el crate que
necesitas:

```sh
ray build --native servicio.ray     # usa std/crypto → enlaza `ring` (vía cargo)
                                     # ok: binario nativo 'servicio' [ray-runtime: crypto]
ray build --native hola.ray         # no toca crates → rustc pelado, ~0,2 s
                                     # ok: binario nativo 'hola'
```

El código de esos subsistemas vive en un crate compartido, **`ray-runtime`**, del que dependen
tanto el binario `ray` (la VM) como el binario transpilado. Consecuencia clave: ambos llaman
**exactamente al mismo código** (`ring`, `rustls`, `rusqlite`) → la salida del nativo es
idéntica a la de la VM **por construcción**, no por una segunda implementación que hay que
mantener en sincronía. Los crates se compilan una vez por máquina (caché compartida) y los
builds siguientes solo recompilan tu programa.

## Excluir un subsistema a mano

A veces quieres el binario **sin** un crate aunque tu programa lo referencie: un build
hermético o para *cross-compile*, un contenedor endurecido, una política de "este servicio
nunca enlaza TLS", o simplemente un build rápido cuando el camino con-crate es inalcanzable en
la práctica. Para eso está `--without`:

```sh
ray build --native app.ray --without tls,sqlite
```

El subsistema excluido vuelve al *stub* que panica si se alcanza, y —si no queda ningún otro
crate— el binario compila por la **vía rápida `rustc`** otra vez. Un nombre inválido se
rechaza al instante (no un typo que pasa desapercibido).

Cuando la exclusión es una **política estable** del proyecto, decláralala en el `ray.toml` en
vez de repetir el flag en cada build:

```toml
[package]
name = "svc"
version = "0.1.0"

[native]
without = ["tls", "sqlite"]   # este servicio nunca enlaza TLS ni SQLite
```

El `--without` de la línea de comandos se **une** a esta lista (la CLI añade, no reemplaza). El
control es siempre del `ray` que compila: nada de esto queda incrustado en el binario final.

## Los límites, con honestidad

El transpilador es explícito sobre su alcance: un nodo fuera del subconjunto da un error claro.
Una función *no-main* que no transpila se emite como un *stub* que panica con su firma, así el
binario compila y —si el flujo real no la alcanza— corre idéntico a la VM; si la alcanza,
panica con un mensaje claro en vez de fallar en `rustc`. Quedan diferidos con criterio algunos
casos con *tradeoffs* reales (canales de tipos mutables cruzando hilos, `spawn` de función con
captura no-`Send`), no huecos silenciosos.

El resto del ecosistema —el REPL, el LSP, `ray fmt`, `ray doc`, los tests— sigue corriendo
sobre la VM, que es donde el arranque instantáneo importa. El nativo es para el último paso:
llevar el programa a producción a velocidad de código máquina.
