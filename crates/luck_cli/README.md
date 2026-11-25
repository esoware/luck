# luck_cli

The `luck` command-line tool: bundler, minifier, formatter, linter, and language server for Luau.

```sh
cargo install luck_cli
```

## Commands

| Command | Does |
| --- | --- |
| `luck init` | Writes a `luck.json` and a `src/main.luau` entry file |
| `luck build` | Builds the project `luck.json` describes: bundles each entry, then minifies if enabled |
| `luck bundle <entry>` | Bundles one entry into a single file |
| `luck minify <file>` | Minifies one file, or stdin |
| `luck graph <entry>` | Prints an entry's dependency graph, as JSON or DOT |
| `luck lint [paths]` | Lints files, and with `--fix` applies the fixes until none is left, or with `--fix-suggestions` the suggestions too |
| `luck fmt [paths]` | Formats files, or with `--check` lists the ones that would change |
| `luck check [paths]` | Reports syntax and compile errors |
| `luck lsp` | Runs the language server over stdio, or over TCP with `--socket` |

`luck <command> --help` lists each command's flags.

## How it works

Every command runs on a thread with `luck_ast::MIN_STACK_BYTES` of stack, and reads each `luck.json` once, as a `luck_config::Project`. `lint`, `fmt`, and `check` give each file the nearest `luck.json` above it, or the one `-c` names, and work through the files in parallel on rayon's pool, each file in an arena from a pool, printing each file's results in input order. `build` takes the `luck.json` above the working directory. `bundle`, `minify`, and `graph` take the one above their input, and `bundle` and `graph` name each module by its path from its root, as `build` does. `build`, `bundle`, and `graph` read modules on rayon's pool too, each into its thread's arena, and `build --watch` resets those arenas before each rebuild, keeping their memory. The pool is built once, before any of those commands starts, with the stack every stage needs and as many threads as `--threads` gives, one per CPU core by default, and `lsp` starts as many workers of its own. A command-line flag wins over the config, its `overrides` included.

`lint --timings` prints how long each rule took over the run to stderr, slowest first, with its share of every rule's time and how many times the linter called it, as oxlint's `--debug timings` does.

Each output format is a reporter that renders one file's diagnostics into a buffer, from the file's bytes: the default, ariadne's report with a snippet at each label, where a byte that isn't UTF-8 shows as `?`, which people read on stderr, and for `lint`, `--format json`, an object a line with every field and label, and `--format github`, an annotation a line, which tools read on stdout.

The exit code is 0 on success, 1 when the command found problems or an operation failed, and 2 for bad arguments, a missing or invalid config, or no file to work on.
