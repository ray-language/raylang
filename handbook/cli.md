# Herramienta de terminal

Español · [English](cli.en.md)

Un programa de línea de comandos es el entregable más simple de raylang: un binario de menos de
un megabyte que arranca en milisegundos. Este capítulo construye **notes**, una herramienta de
notas para la terminal, y con ella las reglas de un buen ciudadano de la terminal: argumentos y
opciones, entrada por tubería, salida para personas y para programas, y códigos de salida.

El proyecto completo está en [`examples/apps/notes-cli`](../examples/apps/notes-cli/), con sus
tests. Los bloques de raylang están copiados de él y el CI comprueba que sigan siéndolo.

```sh
notes add "Compras" --tag casa < lista.txt
notes list --tag casa
notes search leche --json | jq '.[].title'
notes rm 3 --yes
```

## 1. El proyecto

```sh
ray new notes-cli && cd notes-cli
```

No hace falta ningún paquete: todo lo que usa está en la biblioteca estándar. El código se reparte
en cuatro módulos:

| Módulo | Qué hace |
|---|---|
| `flags.ray` | ordena los argumentos en comando, posicionales y opciones |
| `store.ray` | las notas, un archivo Markdown por nota |
| `output.ray` | lo que se imprime: tabla, colores y JSON |
| `main.ray` | los comandos y los códigos de salida |

Las notas se guardan como archivos de texto que cualquier otra herramienta puede leer:

```markdown
# Compras
tags: casa, finde

leche, pan
```

## 2. Argumentos y opciones

`args()` devuelve los argumentos como un arreglo. raylang no trae una librería de opciones, y una
herramienta con unos pocos comandos no la necesita: cincuenta líneas los ordenan.

<!-- check: project=examples/apps/notes-cli -->
```rust
/// A parsed command line: `notes add "Title" --tag work --json`.
pub struct Parsed {
    command: string,
    positional: [string],
    options: Map<string, [string]>,
}
```

<!-- check: project=examples/apps/notes-cli -->
```rust
/// Sorts `argv` into command, positionals and options. `value_options` are the options that
/// take a value (`--tag work` or `--tag=work`); any other `--name` is a switch. A bare `--`
/// ends the options: everything after it is positional.
pub fn parse(argv: [string], value_options: [string]) -> Result<Parsed, string> {
    var command = "";
    var positional: [string] = [];
    var options: Map<string, [string]> = Map.new();
    var only_positional = false;
    var i = 0;
    while (i < argv.len()) {
        let a = argv[i];
        i = i + 1;
        if (only_positional || !a.starts_with("--")) {
            if (command == "") {
                command = a;
            } else {
                positional.push(a);
            }
            continue;
        }
        if (a == "--") {
            only_positional = true;
            continue;
        }
        var name = a.substring(2, a.len());
        var values = options.get_or(name, []);
        match (name.index_of("=")) {
            // --tag=work
            Option.Some(eq) => {
                let value = name.substring(eq + 1, name.len());
                name = name.substring(0, eq);
                values = options.get_or(name, []);
                values.push(value);
            },
            Option.None => {
                if (value_options.contains(name)) {
                    // --tag work
                    if (i >= argv.len()) {
                        return Result.Err("option --" + name + " needs a value");
                    }
                    values.push(argv[i]);
                    i = i + 1;
                }
            },
        }
        options.insert(name, values);
    }
    Result.Ok(Parsed { command: command, positional: positional, options: options })
}
```

Tres detalles que los usuarios esperan: `--tag casa` y `--tag=casa` valen lo mismo, una opción
se puede repetir, y un `--` suelto termina las opciones, para poder buscar un texto que empieza
por `--`.

Una opción mal escrita debe ser un error, no un silencio:

<!-- check: project=examples/apps/notes-cli -->
```rust
/// Rejects options the command does not know, so a typo is an error and not a silent no-op.
pub fn only(p: Parsed, known: [string]) -> Result<int, string> {
    for (name, _) in p.options {
        if (!known.contains(name)) {
            return Result.Err("unknown option --" + name);
        }
    }
    Result.Ok(0)
}
```

## 3. Entrada por tubería

El cuerpo de una nota llega por la entrada estándar cuando viene de una tubería o un archivo.
`term.is_tty(0)` dice si la entrada es una terminal: si lo es, no hay nada que leer, y el programa
no debe quedarse esperando.

<!-- check: project=examples/apps/notes-cli -->
```rust
// Everything on stdin, when it is a pipe or a file. With a terminal on stdin there is nothing to
// read and the program must not sit waiting.
fn piped_input() -> string {
    if (term.is_tty(0)) {
        return "";
    }
    var lines: [string] = [];
    while (true) {
        match (input()) {
            Option.Some(line) => lines.push(line),
            Option.None => break,
        }
    }
    lines.join("\n")
}
```

`input()` devuelve una línea, o `None` al final de la entrada.

## 4. Salida para personas y para programas

Los colores solo tienen sentido cuando hay una persona mirando. La regla es: la salida es una
terminal, y el usuario no ha pedido lo contrario con `NO_COLOR`.

<!-- check: project=examples/apps/notes-cli -->
```rust
/// Whether to colour the output: a terminal on stdout, and the user has not opted out.
pub fn colours() -> bool {
    term.is_tty(1) && env("NO_COLOR").is_none()
}

fn paint(code: string, text: string, on: bool) -> string {
    if (on) { "\u{1B}[" + code + "m" + text + "\u{1B}[0m" } else { text }
}
```

Al redirigir a un archivo o a otra orden, `term.is_tty(1)` es falso y la salida queda limpia. La
tabla se ajusta al ancho de la terminal con `term.size()`, y usa `term.width` y `term.fit`, que
cuentan celdas y no caracteres: un emoji o un carácter chino ocupan dos.

Para que otro programa use la salida, `--json`. El texto se compone con una cadena de comilla
invertida y cada valor entra con `${…}`:

<!-- check: project=examples/apps/notes-cli -->
```rust
/// The notes as a JSON array, for scripts.
pub fn as_json(notes: [Note]) -> string {
    var items: [string] = [];
    for n in notes {
        let tags = json.render_arr(json.list(n.tags));
        items.push(
            `{"id": ${n.id}, "title": ${quote(n.title)}, "tags": ${tags}, "body": ${quote(n.body)}}`
        );
    }
    "[" + items.join(", ") + "]"
}
```

## 5. Errores y códigos de salida

Un script decide qué hacer según el código de salida, así que cada final tiene el suyo:

| Código | Significa |
|---|---|
| 0 | hecho |
| 1 | lo pedido no existe (una nota, o una búsqueda sin resultados) |
| 64 | la línea de comandos está mal |
| 74 | error de disco |

El código es el entero que devuelve `main`. Los errores viajan como valores de un tipo propio, que
distingue las dos clases de fallo:

<!-- check: project=examples/apps/notes-cli -->
```rust
// Why a command failed. The two kinds end differently: a wrong command line prints the help and
// exits 64; a disk error exits 74.
enum Failure {
    Usage(string),
    Io(string),
}

fn bad_usage(e: string) -> Failure {
    Failure.Usage(e)
}

fn io_error(e: string) -> Failure {
    Failure.Io(e)
}
```

Cada comando propaga con `?`, y `main` decide el final en un solo sitio:

<!-- check: project=examples/apps/notes-cli -->
```rust
fn main() -> int {
    let p = match (flags.parse(args(), ["tag"])) {
        Result.Ok(p) => p,
        Result.Err(e) => return usage(e),
    };
    if (flags.has(p, "help")) {
        print(HELP);
        return 0;
    }
    match (run(p, store.default_dir())) {
        Result.Ok(code) => code,
        Result.Err(Failure.Usage(e)) => usage(e),
        Result.Err(Failure.Io(e)) => {
            eprint("notes: " + e);
            IO_ERROR
        },
    }
}
```

Los mensajes de error van a la salida de errores con `eprint`, para que no se mezclen con los
datos en una tubería. La búsqueda sigue la convención de `grep`: sin resultados, sale con 1.

<!-- check: project=examples/apps/notes-cli -->
```rust
    if (p.command == "list" || p.command == "search") {
        flags.only(p, ["tag", "json"]).map_err(bad_usage)?;
        let text = if (p.command == "search") { p.positional.join(" ") } else { "" };
        if (p.command == "search" && text == "") {
            return Result.Err(Failure.Usage("search takes the text to look for"));
        }
        let tag = flags.values(p, "tag").join("");
        let found = store.filter(store.all(dir), tag, text);
        if (flags.has(p, "json")) {
            print(output.as_json(found));
        } else if (found.len() > 0) {
            print(output.table(found, output.colours()));
        }
        // Like grep: a search that finds nothing is exit code 1, so `notes search x && …` works.
        return Result.Ok(if (p.command == "search" && found.len() == 0) { NOT_FOUND } else { 0 });
    }
```

## 6. Preguntar antes de borrar

`notes rm` pregunta, salvo que se pase `--yes`. Si no hay una terminal no hay a quién preguntar:
la respuesta es no, y el mensaje sugiere `--yes`. Así un script nunca se queda colgado.

<!-- check: project=examples/apps/notes-cli -->
```rust
// Asks a yes/no question on the terminal. Without a terminal there is nobody to ask: the answer
// is no, and the caller tells the user about --yes.
fn confirm(question: string) -> bool {
    if (!term.is_tty(0)) {
        return false;
    }
    let _ = io.write(question + " [y/N] ");
    let _ = io.flush();
    match (input()) {
        Option.Some(answer) => answer.trim().to_lower() == "y",
        Option.None => false,
    }
}
```

`io.write` escribe sin salto de línea y `io.flush` hace que la pregunta aparezca antes de esperar
la respuesta. Para una contraseña, `term.read_hidden` lee sin mostrar lo que se teclea, y
`term.read_key` lee tecla a tecla para menús interactivos.

## 7. Tests

Los tests cubren las piezas sin lanzar el programa: el parser de argumentos, los archivos y la
salida. Cada uno usa su propia carpeta temporal.

```sh
ray test
```

## 8. El binario

```sh
ray build --native --release -o notes
./notes add "Primera nota" < /dev/null
```

El binario de notes ocupa unos 800 KB y no depende de nada instalado. Arranca en unos 3 ms, medido
en un MacBook Pro M3 Pro, así que se puede llamar en un bucle de shell sin que se note.
`--target` compila para otra plataforma, y el capítulo [Distribuir](shipping.md) cubre cómo
publicarlo.

## Siguiente paso

[**LLM y MCP**](llm-mcp.md): otra herramienta de terminal, esta vez un agente que habla con un
modelo y usa herramientas.
