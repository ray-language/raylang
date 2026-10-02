# notes (command-line tool)

A command-line notes tool written in raylang: commands with options, input from a pipe, output
for people (aligned, coloured on a terminal) and for programs (`--json`), exit codes a script can
test, and the notes as plain Markdown files. It is the project of the handbook chapter
[Command-line tool](../../../handbook/cli.en.md).

```sh
ray test
ray build --native --release -o notes
./notes add "Groceries" --tag home < list.txt
./notes list --json
```
