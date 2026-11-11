# luck_cli

Command-line interface for the luck bundler, minifier, formatter, linter, and language server.

## Overview

`luck_cli` is the binary crate that wires the individual `luck_*` crates (bundler, minifier, formatter, linter, LSP) into a `clap`-based CLI. It runs each command on a worker thread with a 16 MB stack, since deeply nested ASTs can overflow a default one.

## Key features

- **Flat subcommands.** Every operation is a top-level command (`init`, `build`, `bundle`, `minify`, `graph`, `lint`, `fmt`, `check`, `lsp`). `init`, `bundle`, `minify`, and `graph` take `-t/--target` to pick the dialect.
- **Project config.** `luck init` writes `luck.json`, and `luck build` runs the full pipeline from it. `lint`, `fmt`, and `check` give each file the nearest `luck.json` above it, so a nested config governs its own tree without merging into its parent, and `-c/--config` puts every file under one config instead. `--stdin-filepath` resolves the same way from the path it's given. `bundle`, `minify`, and `graph` walk up from the input's directory (the working directory for stdin), and `bundle` and `minify` also take `-c/--config`. The config supplies the target for the input's extension, `dynamic_require`, and the minifier transforms, and CLI flags override each of them. `-t` replaces only the target. Build-only fields such as `entry`, `output`, `profiles`, `minify`, and `search_paths` don't apply: `bundle` minifies only with `--minify`, and `bundle` and `graph` resolve `-s` search paths from the entry's directory.
- **Path selection.** `lint`, `fmt`, and `check` take files, directories, and globs, and default to the working directory. A directory walk honors `.gitignore`, `.luckignore`, and its project's `include`/`exclude`. A glob is relative to the working directory unless it's absolute and selects from the walk of its directory part, a file named outright skips `include` but not `exclude`, and a glob starting with `!` leaves out whatever it matches and everything under a directory it matches. A glob that matches no file, or a run that ends up with none, exits 2 unless `--no-error-on-unmatched-pattern` is given.
- **Lint levels.** `-A/--allow`, `-W/--warn`, and `-D/--deny` set a rule or a category to `off`, `warn`, or `error` over `luck.json`, in the order given, and a rule's own level wins over its category's. `--quiet` prints errors only, while warnings still count toward `--max-warnings` and `--deny-warnings`.
- **Profiles.** On `build`, `--release`, `--dev`, or `-P/--profile <name>` select an entry in the `luck.json` `profiles`, whose `minify` and `transforms` replace the top-level ones.
- **File watching.** `build --watch` rebuilds through `notify` when a `.lua`, `.luau`, `.json`, or `.luaurc` file changes.
- **Per-transform toggles.** `bundle` and `minify` take `--<pass>` and `--no-<pass>` for every minifier pass, so the CLI can turn a pass on or off whatever `luck.json` says. The flag given last wins. On `bundle` they only matter with `--minify`.
- **Language server.** `luck lsp` serves the LSP backend over stdio, or over TCP with `--socket <port>`.

## Commands

All commands are top-level. Without `-t/--target`, a file's dialect comes from the `luck.json` key for its extension: `luau` for `.luau` files (default Luau) and `lua` for everything else (default Lua 5.4).

```sh
luck init [-t <target>]                      # Scaffold luck.json and src/main.{lua,luau}
luck build                                   # Bundle (and minify) using luck.json config
luck bundle <entry> [-t <target>] [-o <out>] # Bundle a multi-file project into one file
luck minify <input> [-t <target>] [-o <out>] # Minify a source file
luck graph <entry> [-t <target>]             # Print the dependency graph (--format json|dot)
luck check [paths...]                        # Parse and report errors (config-driven)
luck lint [paths...]                         # Lint source files (config-driven)
luck fmt [paths...]                          # Format source files (config-driven)
luck lsp [--socket <port>]                   # Run the language server over stdio or TCP
```

### Flag groups

`--quiet` is global and hides progress, summaries, and warnings, the lint warnings included.

| Command | Flags |
|---------|-------|
| `bundle` | `-c/--config`, `--fold-constants`/`--no-fold-constants`, `--rename-locals`/`--no-rename-locals`, ... (per-transform), `--rename-globals`/`--no-rename-globals`, `--minify`, `--line-map`, `-s/--search-path`, `--dynamic-require` |
| `minify` | `-c/--config`, `--fold-constants`/`--no-fold-constants`, `--rename-locals`/`--no-rename-locals`, ... (per-transform), `--rename-globals`/`--no-rename-globals`, `--stats` |
| `graph` | `--format` (json / dot), `-s/--search-path`, `--dynamic-require` |
| `check` | `-c/--config`, `--no-error-on-unmatched-pattern` |
| `fmt` | `--write`, `--check`, `--list-different`, `--no-editorconfig`, `--stdin-filepath`, `--range-start`/`--range-end`, `--verify`, `-c/--config`, `--no-error-on-unmatched-pattern` (layout options live in `luck.json`/`.editorconfig`, not flags) |
| `lint` | `--fix`, `-f/--format` (default / json / github), `-A/--allow`, `-W/--warn`, `-D/--deny` per rule or category, `--global`, `--max-warnings`, `--deny-warnings`, `--silent`, `--rules`, `--print-config`, `--stdin-filepath`, `-c/--config`, `--no-error-on-unmatched-pattern` |
| `build` | `--release`, `--dev`, `-P/--profile <name>`, `-w/--watch`, `--dry-run`, `-c/--config` |

## Architecture

### Worker thread

`main.rs` runs the command on a thread with a 16 MB stack and joins it. The OS sizes the main thread's stack (usually 8 MB on Linux and macOS, 1 MB on Windows), and Rust gives other threads 2 MB by default, which isn't always enough for deeply nested ASTs. `luck lsp` gives its tokio worker threads the same 16 MB, but the rayon threads that `check`, `lint`, and `fmt` spread files across keep the 2 MB default.

### Module layout

| File | Purpose |
|------|---------|
| `main.rs` | Crate root: module wiring, the global allocator, and the entry point that spawns the 16 MB-stack worker thread |
| `exit.rs` | Exit codes (`Exit`) and the error reports that end a run with one |
| `verbosity.rs` | `Verbosity`, set by `--quiet` |
| `cli.rs` | Top-level `clap` model (`Cli`/`Command`) and dispatch |
| `commands/` | One module per subcommand, each owning its `clap` args struct, its `run` handler, and its tests |
| `project.rs` | Target/config resolution, file discovery, and the in-order parallel walk shared by the path commands |
| `output.rs` | Output and stdin plumbing, and size display |
| `minify_flags.rs` | The `--<pass>`/`--no-<pass>` toggles shared by `bundle` and `minify` |
| `render.rs` | `ariadne`-based diagnostic rendering and file cache |
