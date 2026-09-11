# luck_cli

Command-line interface for the luck bundler, minifier, formatter, and linter.

## Overview

`luck_cli` is the binary crate that wires the individual `luck_*` crates (bundler, minifier, formatter, linter, LSP) into a `clap`-based CLI. It spawns a 16 MB-stack worker thread to handle deeply nested ASTs without overflowing default stacks, then dispatches to the requested subcommand.

## Key features

- **Flat subcommands.** Every operation is a top-level command (`init`, `build`, `bundle`, `minify`, `graph`, `lint`, `fmt`, `check`, `lsp`). A `-t/--target` flag picks the Lua version per command, instead of per-target subcommands.
- **Project config.** `luck init` and `luck build` read `luck.json` and drive the full pipeline. `lint`/`fmt`/`check` discover `luck.json` by walking up from cwd. `bundle`/`minify` discover it from the input directory (cwd for stdin), or accept `-c/--config`; both use its per-extension target and transforms. Explicit CLI flags override those settings, including with `-t` present. Build-only fields such as profiles, output paths, and entry points do not change these one-shot commands; `bundle` still requires `--minify` and roots its `-s` search paths at the entry directory.
- **Profiles.** On `build`, `--release`, `--dev`, or `--profile <name>` override config-file settings.
- **File watching.** On `build`, `--watch` rebuilds on filesystem changes via `notify`.
- **Per-transform toggles.** `bundle` and `minify` expose `--<pass>` and `--no-<pass>` flags per minifier pass for targeted comparisons. Both polarities exist so either can override the configured value; the flag given last wins.
- **Language server.** `luck lsp` serves the LSP backend over stdio, or over TCP with `--socket <port>`.

## Commands

All commands are top-level. Pick the Lua target with `-t/--target`; where an input is given, its extension infers one.

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

### Flag groups

| Command | Flags |
|---------|-------|
| `bundle` | `-c/--config`, `--fold-constants`/`--no-fold-constants`, `--rename-locals`/`--no-rename-locals`, ... (per-transform), `--rename-globals`/`--no-rename-globals`, `--minify`, `--line-map`, `-s/--search-path` |
| `minify` | `-c/--config`, `--fold-constants`/`--no-fold-constants`, `--rename-locals`/`--no-rename-locals`, ... (per-transform), `--rename-globals`/`--no-rename-globals`, `--stats` |
| `fmt` | `--write`, `--check`, `--list-different`, `--no-editorconfig`, `--stdin-filepath`, `--range-start`/`--range-end`, `--verify`, `-c/--config` (layout options live in `luck.json`/`.editorconfig`, not flags) |
| `lint` | `--fix`, `--format` (default / json), `-A/--allow`, `-W/--warn`, `-D/--deny` per rule or category, `--global`, `--max-warnings`, `--deny-warnings`, `--silent`, `--rules`, `--print-config`, `--stdin-filepath` |
| `build` | `--release`, `--dev`, `--profile <name>`, `--watch`, `--dry-run`, `-c/--config` |

## Architecture

### Worker thread

`main.rs` spawns a thread with a 16 MB stack and joins it after the command finishes. Default thread stacks (1 MB on Linux, 8 MB on macOS, varies on Windows) are not enough for deeply nested AST processing on real-world Lua code, so the worker-thread pattern is permanent.

### Module layout

| File | Purpose |
|------|---------|
| `main.rs` | Entry point; spawns the 16 MB-stack worker thread |
| `lib.rs` | Crate root: exit codes, `Verbosity`, module wiring, `run` re-export |
| `args.rs` | Top-level `clap` model (`Cli`/`Command`) and dispatch |
| `commands/` | One module per subcommand, each owning its `clap` args struct, its `run` handler, and its tests |
| `project.rs` | Target/config resolution and file discovery shared by the path commands |
| `output.rs` | Output, stdin, and diagnostic-cache plumbing |
| `minify_flags.rs` | The `--no-<pass>` toggles shared by `bundle` and `minify` |
| `render.rs` | `ariadne`-based diagnostic rendering and file cache |
