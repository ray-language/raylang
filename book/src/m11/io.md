# I/O y la API de runtime

Hasta aquí raylang tenía **un solo cable hacia afuera**: `print` (stdout) y el código de salida de
`main`. No podía leer entrada, ni parsearla, ni ver sus argumentos, ni el entorno. M11.2 abre ese
resto: lo justo para escribir apps de CLI e interactivas —y un paso más hacia el self-hosting—.

## `main` no cambia de forma

Una tentación clásica es meter los argumentos en la firma de la entrada, estilo C
(`main(argc, argv)`). raylang **no** lo hace (decisión de §0): `main` sigue siendo `main() -> int`
—punto de entrada y código de salida, nada más—. El acceso al exterior se hace por **builtins**,
estilo Go/Python (`args()`, `env(...)`). Así la capacidad está en *cualquier* función, no solo en la
entrada, y encaja con cómo ya funcionaba `print`.

## La decisión que define M11.2: la I/O falible devuelve `Option`

Leer una línea puede toparse con el **fin de la entrada**. Parsear un entero puede **fallar**. Una
variable de entorno puede **no existir**. ¿Qué devuelve `input()` en EOF? Un centinela (`""`)
confundiría "línea vacía" con "no hay más". raylang, fiel a su norte —**errores como valores, sin
`null`**—, devuelve `Option`:

```rust
input()      -> Option<string>      // None en EOF
parse_int(s) -> Option<int>         // None si no es un entero
read_int()   -> Option<int>         // None en EOF o si no parsea
env(nombre)  -> Option<string>      // None si la variable no existe
```

Aquí es donde el prelude de M6.3 por fin **paga**: hasta ahora `Option`/`Result` eran tipos que el
usuario producía; M11.2 trae los **primeros productores naturales** de la stdlib. Y `read_int`
muestra el operador `?` en su salsa:

```rust
fn read_int() -> Option<int> {
    let s = input()?;      // si es None, retorna None aquí mismo
    parse_int(s)
}
```

## El truco: el runtime no sabe qué es `Option`

¿Cómo construye un builtin un valor de `Option`? En la VM eso exigiría que el opcode conociera el
`enum_id` y los *tags* de `Option` —un acoplamiento feo entre el runtime y un tipo del prelude—. Se
evita con el mismo patrón de la stdlib (M7.3): **primitivos mínimos + envoltorios en raylang**.

Cada operación falible se parte en dos:

1. un **primitivo builtin** (un opcode) que devuelve un **`[T]`** —vacío = "nada", un elemento = "el
   valor"—, una forma que el runtime ya sabe construir (como `split`);
2. un **envoltorio en el prelude, escrito en raylang**, que lo traduce a `Option`:

```rust
// en el prelude (raylang):
fn parse_int(s: string) -> Option<int> {
    let r = __parse_int(s);                                   // [] o [n]  (opcode ParseInt)
    if (len(r) == 0) { Option.None } else { Option.Some(r[0]) }
}
```

Así el intérprete y la VM **siguen sin saber qué es `Option`**: solo devuelven arreglos. La
ergonomía la pone el prelude. Es el mismo reparto de responsabilidades de `map`/`filter`/`fold`.

## El catálogo

- **M11.2a — salida de error + entrada/parseo:**
  - `eprint(x)` — como `print`, pero a **stderr**. Opcode `EPrint`.
  - `parse_int(s) -> Option<int>` — primitivo `__parse_int` (`ParseInt`) + envoltorio.
  - `input() -> Option<string>` — lee una línea de stdin (sin el `\n`). Primitivo `__read_line`
    (`ReadLine`) + envoltorio.
  - `read_int() -> Option<int>` — pura composición en el prelude (`input()?` y `parse_int`).
- **M11.2b — entorno + argumentos:**
  - `env(nombre) -> Option<string>` — primitivo `__env` (`Env`) + envoltorio.
  - `args() -> [string]` — los argumentos del programa. Opcode `Args`. El runner deja los args (lo
    que sigue a la ruta en `raylang archivo.ray a b c`) en un **almacén de proceso** (`OnceLock`)
    que ambos motores leen; los clientes sin args (REPL, `--test`, tests) ven `[]`.

## Dos motores, otra vez el oráculo

Como M11.1, esto **toca el runtime** (los primitivos son opcodes), así que vuelve la disciplina del
**oráculo** VM↔intérprete. Pero hay un matiz: la I/O **no es determinista** (depende de stdin, del
entorno, de los argumentos). Por eso se prueba en dos capas:

- lo **determinista** (`parse_int`, y `args()`/`env()` "vacíos" en el proceso de test) con el
  oráculo, exigiendo que ambos motores coincidan, incluido el estrés del GC (el `[T]` y el `Option`
  son objetos del heap);
- lo **interactivo** (stdin, stderr, argv, entorno) con **tests de integración por subproceso**, que
  alimentan stdin, capturan stderr y pasan args/env reales.

## Archivos: el truco del `[T]` crece a `Result` (M11.2c)

> **Nota (M50.1):** desde M50.1 las funciones de archivos ya **no** son globales; viven en el módulo
> **`std/fs`**. Impórtalo con `import std/fs;` y llámalas calificadas: `fs.read_file(ruta)`,
> `fs.write_file(...)`, `fs.exists(...)`, etc. Importar `std/fs` funciona como un *capability hint*
> suave (señala que el archivo toca el disco). Los ejemplos de abajo usan los nombres sin calificar por
> claridad histórica; en código real, antepón `fs.`. Los primitivos `__read_file`/`__write_file`/
> `__exists` siguen siendo builtins globales (el envoltorio de `std/fs` los envuelve en `Result`/`bool`).

Leer y escribir archivos puede fallar, así que devuelven **`Result`** —el otro productor natural de
errores-como-valores—. Pero `Result` carga **dos** payloads (el valor en `Ok`, el mensaje en `Err`),
y el truco del `[T]` (vacío/único) solo distinguía "hay valor / no hay". La solución: un **arreglo
etiquetado**, cuyo **primer elemento es la etiqueta**:

```rust
read_file(ruta)            -> Result<string, string>   // Ok(contenido) | Err(mensaje)
write_file(ruta, contenido) -> Result<int, string>     // Ok(nº de caracteres) | Err(mensaje)
```

El primitivo `__read_file` devuelve `["ok", contenido]` o `["err", mensaje]`; el envoltorio del
prelude mira `r[0]`:

```rust
fn read_file(ruta: string) -> Result<string, string> {
    let r = __read_file(ruta);
    if (r[0] == "ok") { Result.Ok(r[1]) } else { Result.Err(r[1]) }
}
```

`write_file` usa `["ok"]` / `["err", msg]` y el prelude pone `Result.Ok(len(contenido))` —los
caracteres escritos— en el caso bueno (devolver `int` evita necesitar un literal *unit*). El runtime,
una vez más, **no sabe qué es `Result`**: solo devuelve arreglos de strings; el prelude pone el tipo.

Con archivos, raylang ya puede **leer sus propias fuentes** — el último cimiento que faltaba para
encarar el self-hosting.

**Aditivos (M11.4b):** `exists(ruta) -> bool` (total, no falla) y `append_file(ruta, cont) ->
Result<int, string>` (añade al final, crea si no existe). `append_file` reusa exactamente el patrón:
primitivo `__append_file` con arreglo etiquetado + envoltorio en el prelude que arma el `Result`.

```rust
if (exists(ruta)) { … }                     // bool, directo
append_file("log.txt", "una línea\n");      // acumula; no sobrescribe
```

## Dos motores, otra vez el oráculo

Como M11.1, esto **toca el runtime** (los primitivos son opcodes), así que vuelve la disciplina del
**oráculo** VM↔intérprete. Pero hay un matiz: la I/O **no es determinista** (depende de stdin, del
entorno, de los argumentos, del disco). Por eso se prueba en dos capas:

- lo **determinista** (`parse_int`, `args()`/`env()` "vacíos" en el test, leer un archivo
  **inexistente** → `Err`) con el oráculo, exigiendo que ambos motores coincidan, incluido el estrés
  del GC (el `[T]` y el `Option`/`Result` son objetos del heap);
- lo **interactivo/real** (stdin, stderr, argv, entorno, ida y vuelta de archivos) con **tests de
  integración por subproceso**, que alimentan stdin, capturan stderr y usan archivos temporales.

## Más I/O de archivos (M11.7c)

`remove_file(ruta) -> Result<int,string>` borra; `list_dir(ruta) -> Result<[string],string>` lista
los nombres de un directorio (**ordenados**, para que el resultado sea determinista). Mismo patrón de
arreglo etiquetado + envoltorio en el prelude; `list_dir` reconstruye el `[string]` con un `while`+
`push` (raylang aún no tiene slice de arreglos).

## Streaming: handles de archivo (M11.8)

Hasta aquí la I/O de archivos era *de una vez* (leer/escribir el archivo entero). M11.8 añade
**streaming**: abrir una vez y leer/escribir por partes.

```rust
match (open("salida.txt", "w")) {       // "r" lectura, "w" escritura, "a" añade
    Result.Ok(h) => {
        write(h, "una línea\n");            // se escribe sin recargar nada
        write(h, "otra\n");
        close(h);
    },
    Result.Err(e) => print("no se pudo abrir: " + e),
}

match (open("salida.txt", "r")) {
    Result.Ok(h) => {
        match (read_line(h)) {              // lee la siguiente línea; None en EOF
            Option.Some(l) => print(l),
            Option.None => print("vacío"),
        }
        close(h);
    },
    Result.Err(e) => print(e),
}
```

La pieza interesante es **cómo se representa el handle sin tocar el GC ni inventar un tipo de valor**:
el handle es un **`int`**, y los archivos abiertos viven en un **almacén de proceso** del host (un
`Mutex<HashMap<i64, …>>` en `builtins.rs`, igual que el almacén de `args`). La lectura es
**bufferizada** (`BufReader`). El runtime de raylang no sabe de archivos: solo pasa enteros.

## Lo que queda fuera

Lo más estructural: *buffering* de escritura configurable, lectura binaria (no por líneas), y un slice
de arreglos. Aditivos; llegarán cuando hagan falta.

> M11.2 cierra un círculo abierto en M6.3: los tipos `Option`/`Result` existían desde entonces, pero
> hasta ahora casi todo el que los producía era el propio usuario. La I/O es su hábitat natural —el
> mundo exterior es, por definición, donde las cosas fallan— y el lenguaje las recibe sin un solo
> `null`, sin una sola excepción: solo valores.
