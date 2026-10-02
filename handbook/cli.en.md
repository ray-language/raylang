# Command-line tool

[Español](cli.md) · English

A command-line program is the simplest thing raylang ships: a binary under a megabyte that starts
in milliseconds. This chapter builds **notes**, a notes tool for the terminal, and with it the
rules of a good terminal citizen: arguments and options, piped input, output for people and for
programs, and exit codes.

The complete project is in [`examples/apps/notes-cli`](../examples/apps/notes-cli/), with its
tests. The raylang blocks are copied from it and CI checks that they still are.

```sh
notes add "Shopping" --tag home < list.txt
notes list --tag home
notes search milk --json | jq '.[].title'
notes rm 3 --yes
```

## 1. The project

```sh
ray new notes-cli && cd notes-cli
```

No package is needed: everything it uses is in the standard library. The code is split into four
modules:

| Module | What it does |
|---|---|
| `flags.ray` | sorts the arguments into command, positionals and options |
| `store.ray` | the notes, one Markdown file per note |
| `output.ray` | what gets printed: table, colours and JSON |
| `main.ray` | the commands and the exit codes |

The notes are stored as text files any other tool can read:

```markdown
# Shopping
tags: home, weekend

milk, bread
```

## 2. Arguments and options

`args()` returns the arguments as an array. raylang ships no options library, and a tool with a
handful of commands does not need one: fifty lines sort them out.

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

Three details users expect: `--tag home` and `--tag=home` mean the same, an option can be
repeated, and a bare `--` ends the options, so you can search for a text that starts with `--`.

A mistyped option must be an error, not silence:

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

## 3. Piped input

The body of a note arrives on standard input when it comes from a pipe or a file.
`term.is_tty(0)` tells whether the input is a terminal: if it is, there is nothing to read, and
the program must not sit waiting.

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

`input()` returns one line, or `None` at the end of the input.

## 4. Output for people and for programs

Colours only make sense when a person is looking. The rule: the output is a terminal, and the user
has not asked otherwise with `NO_COLOR`.

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

When redirected to a file or another command, `term.is_tty(1)` is false and the output stays
clean. The table adapts to the terminal width with `term.size()`, and uses `term.width` and
`term.fit`, which count cells rather than characters: an emoji or a Chinese character takes two.

For another program to use the output, `--json`. The text is composed with a backtick string and
each value goes in with `${…}`:

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

## 5. Errors and exit codes

A script decides what to do from the exit code, so every ending has its own:

| Code | Meaning |
|---|---|
| 0 | done |
| 1 | what was asked for does not exist (a note, or a search with no results) |
| 64 | the command line is wrong |
| 74 | disk error |

The code is the integer `main` returns. Errors travel as values of a type of their own, which
tells the two kinds of failure apart:

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

Each command propagates with `?`, and `main` decides the ending in one place:

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

Error messages go to standard error with `eprint`, so they do not mix with the data in a pipe.
The search follows the `grep` convention: with no results, it exits with 1.

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

## 6. Asking before deleting

`notes rm` asks, unless `--yes` is given. Without a terminal there is nobody to ask: the answer is
no, and the message suggests `--yes`. That way a script never hangs.

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

`io.write` writes without a newline and `io.flush` makes the question appear before waiting for
the answer. For a password, `term.read_hidden` reads without showing what is typed, and
`term.read_key` reads key by key for interactive menus.

## 7. Tests

The tests cover the pieces without launching the program: the argument parser, the files and the
output. Each one uses its own temporary folder.

```sh
ray test
```

## 8. The binary

```sh
ray build --native --release -o notes
./notes add "First note" < /dev/null
```

The notes binary is about 800 KB and depends on nothing installed. It starts in about 3 ms,
measured on a MacBook Pro M3 Pro, so it can be called in a shell loop without anyone noticing.
`--target` builds for another platform, and the [Shipping](shipping.en.md) chapter covers how to
publish it.

## Next step

[**LLMs and MCP**](llm-mcp.en.md): another terminal tool, this time an agent that talks to a model
and uses tools.

<!-- sync: sha256:4e21b830496e -->
