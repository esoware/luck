# luck_cli

The `luck` command-line tool: bundler, minifier, formatter, linter, and language server for Luau.

```sh
cargo install luck_cli
```

Building it needs clang, which compiles Luau's type checker: install [LLVM](https://github.com/llvm/llvm-project/releases), or on macOS Xcode's command line tools.

## Commands

| Command | Does |
| --- | --- |
| `luck init` | Writes a `luck.json`, which loads Roblox's API with `--roblox`, and a `src/main.luau` entry file |
| `luck build` | Builds the project `luck.json` describes: bundles each entry, then minifies if enabled |
| `luck bundle <entry>` | Bundles one entry into a single file |
| `luck minify <file>` | Minifies one file, or stdin |
| `luck graph <entry>` | Prints an entry's dependency graph, as JSON or DOT |
| `luck lint [paths]` | Lints files, and with `--fix` applies the fixes until none is left, or with `--fix-suggestions` the suggestions too |
| `luck fmt [paths]` | Formats files, or with `--check` lists the ones that would change |
| `luck check [paths]` | Reports syntax and compile errors, and the type errors Luau's own type checker finds |
| `luck lsp` | Runs the language server over stdio, or over TCP with `--socket` |

`luck <command> --help` lists each command's flags.

## How it works

Every command runs on a thread with `luck_ast::MIN_STACK_BYTES` of stack, and reads each `luck.json` once, as a `luck_config::Project`. `lint`, `fmt`, and `check` give each file the nearest `luck.json` above it, or the one `-c` names, and work through the files in parallel on rayon's pool, each file in an arena from a pool, printing each file's results in input order. `build` takes the `luck.json` above the working directory. `bundle`, `minify`, and `graph` take the one above their input, and `bundle` and `graph` name each module by its path from its root, as `build` does. `build`, `bundle`, and `graph` read modules on rayon's pool too, each into its thread's arena, and `build --watch` resets those arenas before each rebuild, keeping their memory. The pool is built once, before any of those commands starts, with the stack every stage needs and as many threads as `--threads` gives, one per CPU core by default, and `lsp` starts as many workers of its own. A command-line flag wins over the config, its `overrides` included.

`check` first type-checks together the files of the projects with one security level and one list of definitions, with `luck_type_checker` on as many threads as `--threads` gives, over Luau's built-in definitions and what the project's `definitions` loads, in order, reporting Luau's errors in a definition file at it: for `@roblox`, Roblox's at its level, with the class `Instance.new`, `GetService`, and `FindService` name and the `Enum.Material` types Studio gives, and for any other entry, the definition file, each module named by its canonical path from the working directory, each `require` resolved as `build` resolves it, and each `.luaurc` or `.config.luau` up the tree read once a run, for the mode of every module below it, as `luau-analyze` reads them, the latter run by Luau's own Config library and stopped once it has run for a second, reporting one Luau can't read or stops, or a directory with both, once, as an E014 at that file, then reports a file's type errors where it has no syntax or compile error. A definition file, which a name ending in `.d.luau` makes one, is no module, so it's checked only as `definitions` loads it.

`lint`, `fmt`, and `check` read a `.d.luau` file as a definition file, with `luck_parser::parse_definitions`, wherever they meet it, and since it's no code, `lint` runs no rule on it and reports its syntax errors alone. One that a project lists and luck's parser or Luau can't load fails no project and stops no run: the project's files run without it, `lint` reports its syntax errors when it lints it, and `check` reports Luau's errors in it, at it, or luck's syntax errors in their place when it checks that file too. `build`, `bundle`, `graph`, and `check` read each `.config.luau`'s aliases through `luck_type_checker`, Luau's own Config library, and a `require` of a definition file is an E004.

`lint --timings` prints how long each rule took over the run to stderr, slowest first, with its share of every rule's time and how many times the linter called it, as oxlint's `--debug timings` does.

Each output format is a reporter that renders one file's diagnostics into a buffer, from the file's bytes: the default, ariadne's report with a snippet at each label, where a byte that isn't UTF-8 shows as `?`, which people read on stderr, and for `lint`, `--format json`, an object a line with every field and label, and `--format github`, an annotation a line, which tools read on stdout.

The exit code is 0 on success, 1 when the command found problems or an operation failed, and 2 for bad arguments, a missing or invalid config, or no file to work on.
