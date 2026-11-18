# luck_cli

The `luck` command-line tool: bundler, minifier, formatter, linter, and language server for Lua 5.1 to 5.5 and Luau.

```sh
cargo install luck_cli
```

## Commands

| Command | Does |
| --- | --- |
| `luck init` | Writes a `luck.json` and a `src/main` entry file |
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

Every command runs on a thread with `luck_ast::MIN_STACK_BYTES` of stack, and reads each `luck.json` once, as a `luck_config::Project`. `lint`, `fmt`, and `check` give each file the nearest `luck.json` above it, or the one `-c` names, and work through the files in parallel on rayon's pool, each file in an arena from a pool, printing each file's results in input order. `build` takes the `luck.json` above the working directory. `bundle`, `minify`, and `graph` take the one above their input, and `bundle` and `graph` resolve modules from its root and `search_paths` as `build` does. `build`, `bundle`, and `graph` read modules on rayon's pool too, each into its thread's arena, and `build --watch` resets those arenas before each rebuild, keeping their memory. A command-line flag wins over the config, its `overrides` included.

The exit code is 0 on success, 1 when the command found problems or an operation failed, and 2 for bad arguments, a missing or invalid config, or no file to work on.
