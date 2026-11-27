# luck_cli

The `luck` command-line tool, for formatting, linting, type checking, bundling,
and minifying Luau.

## Install

```sh
cargo install luck_cli
```

Building it needs clang, for Luau's type checker. Install LLVM, or on macOS,
Xcode's command line tools.

## Commands

| Command | What it does |
| --- | --- |
| `luck init` | creates a `luck.json` and a `src/main` entry file |
| `luck build` | bundles the project in `luck.json`, then minifies it if enabled |
| `luck bundle` | bundles a multi-file project into a single file |
| `luck minify` | minifies a source file |
| `luck graph` | prints an entry file's dependency graph |
| `luck lint` | lints files, and fixes them with `--fix` |
| `luck fmt` | formats files |
| `luck check` | reports syntax, compile, and type errors |
| `luck lsp` | runs the language server over stdio, or TCP with `--socket` |

Run `luck <command> --help` for a command's flags.
