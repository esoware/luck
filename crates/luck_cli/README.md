# luck_cli

Command-line interface for the luck bundler, minifier, formatter, and linter.

## Overview

`luck_cli` is the binary crate that wires the luck facade into a `clap`-based CLI. It spawns a 16 MB-stack worker thread to handle deeply nested ASTs without overflowing default stacks, then dispatches to the requested subcommand.

## Key Features

- **Flat subcommands** — every operation is a top-level command (`init`, `build`, `bundle`, `minify`, `graph`, `lint`, `fmt`, `check`, `lsp`). The Lua version is selected per command with a `-t/--target` flag, not via per-target subcommands.
- **Project config** — `luck init` and `luck build` read `luck.json` and drive the full pipeline. `lint`/`fmt`/`check` discover `luck.json` by walking up from cwd.
- **Profiles** — on `build`, `--release`, `--dev`, or `--profile <name>` override config-file settings.
- **File watching** — on `build`, `--watch` rebuilds on filesystem changes via `notify`.
- **Per-transform toggles** — `bundle` and `minify` expose a `--no-<pass>` flag per minifier pass for targeted comparisons.
- **Language server** — `luck lsp` serves the LSP backend over stdio (or TCP with `--socket <port>`).

## Commands

All commands are top-level; the Lua target is chosen with `-t/--target` (inferred from the file extension where an input is given).

```sh
luck init [-t <target>]                      # Scaffold luck.json and src/main.{lua,luau}
luck build                                   # Bundle (and minify) using luck.json config
luck bundle <entry> [-t <target>] -o <out>   # Bundle a multi-file project into one file
luck minify <input> [-t <target>] -o <out>   # Minify a source file
luck graph <entry> [-t <target>]             # Print the dependency graph (--format json|dot)
luck check [paths...]                        # Parse and report errors (config-driven)
luck lint [paths...]                         # Lint source files (config-driven)
luck fmt [paths...]                          # Format source files (config-driven)
luck lsp [--socket <port>]                   # Run the language server over stdio or TCP
```

### Flag Groups

| Command | Flags |
|---------|-------|
| `bundle` | `--no-fold-constants`, `--no-rename-locals`, … (per-transform), `--rename-globals`, `--minify`, `--line-map`, `-s/--search-path` |
| `minify` | `--no-fold-constants`, `--no-rename-locals`, … (per-transform), `--rename-globals`, `--stats` |
| `fmt` | `--write`, `--check`, `--list-different`, `--no-editorconfig`, `--stdin-filepath`, `-c/--config` (layout options live in `luck.json`/`.editorconfig`, not flags) |
| `lint` | `--fix`, `--format` (default / json), `-A/--allow`, `-W/--warn`, `-D/--deny` per rule or category, `--global`, `--max-warnings`, `--deny-warnings`, `--silent`, `--rules`, `--print-config`, `--stdin-filepath` |
| `build` | `--release`, `--dev`, `--profile <name>`, `--watch`, `--dry-run`, `-c/--config` |

## Architecture

### Worker Thread

`main.rs` spawns a thread with a 16 MB stack and joins it after the command finishes. Default thread stacks (1 MB on Linux, 8 MB on macOS, varies on Windows) are not enough for deeply nested AST processing on real-world Lua code, so the worker-thread pattern is permanent.

### Module Layout

| File | Purpose |
|------|---------|
| `main.rs` | Entry point; spawns the 16 MB-stack worker thread |
| `cli.rs` | Clap command definitions and subcommand handlers |
| `render.rs` | `ariadne`-based diagnostic rendering and file cache |
| `lib.rs` | Unit-testable CLI dispatch |
