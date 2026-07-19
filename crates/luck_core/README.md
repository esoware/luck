# luck_core

Shared types, configuration parsing, and diagnostics for the luck toolchain.

## Overview

`luck_core` sits one level above `luck_token` and provides everything else the higher-level tools share: build targets, transform configuration, module identity, diagnostics, and project configuration parsing.

## Key Features

- **Build targets** — `LuaTarget` enumerates the supported flavors (Lua 5.1 through 5.5 and Luau) and parses from common string forms (`"lua54"`, `"5.4"`, `"luau"`).
- **Transform toggles** — `TransformConfig` enables and disables individual minifier passes, with serde-driven partial overrides from JSON.
- **Module identity** — `ModuleId` and `ModuleInfo` carry path, source text, AST, and byte ranges through the bundler's dependency graph.
- **Rich diagnostics** — `Diagnostic` carries codes, severities, spans, labels, and help text, rendered to the terminal via `ariadne`.
- **Project configuration** — `luck.json` (parsed as JSON5), `.luaurc`, profile overrides, and config discovery up the directory tree.

## Architecture

### Build Targets

`LuaTarget` is the configuration-layer view of a Lua version. It maps to `luck_token::LuaVersion` for the parser and lexer, and gates bundler and minifier behavior that depends on the target language flavor.

### Transform Configuration

`TransformConfig` is a struct of bool flags, one per minifier pass: `fold_constants`, `rename_locals`, `inline_locals`, `merge_locals`, `remove_dead_code`, `simplify_statements`, `simplify_indexes`, `simplify_parens`, `shorten_strings`, `shorten_numbers`, `lift_locals`, and `rename_globals`. All default to on except `rename_globals` (opt-in — renamed globals live under different `_G` keys and break cross-chunk consumers); the config layer applies user overrides.

### Project Configuration

The `config` module handles project configuration files:

- **`luck.json`** — project config in JSON5. Supports `target`, `entry` / `entries`, `output` / `output_dir`, `minify`, `search_paths`, `transforms`, `preamble`, and `format`.
- **`.luaurc`** — Luau-specific config with `aliases` for module resolution and `languageMode`.
- **Discovery** — walks up the directory tree from the working directory to find `luck.json`.
- **Profile overrides** — named profiles (e.g. `release`, `dev`) override `minify` and `transforms`.
- **`BuildConfig`** — the fully resolved configuration for a single build target, ready for execution.
- **`FormatConfig`** — formatter settings: `line_width`, `indent_style`, `indent_width`, `quote_style`, `hexadecimal_case` (case of the hex digits `A`–`F` in numeric literals — `preserve`/`lower`/`upper`; the `0x` prefix is always lowercased), `call_parentheses`, `collapse_simple_statement`, `line_endings`, `block_newline_gaps` (blank-line preservation between block statements), `sort_requires` (alphabetize `require` blocks), `space_after_function_names` (space between a call target and its arguments), and `magic_trailing_comma` (force multiline when a trailing comma is present).

### Diagnostics

`Diagnostic` carries a code, message, severity, file path, source span, labels, and optional help text. The builder pattern (`with_label`, `with_help`) keeps construction readable.

Pre-defined error codes used across the toolchain:

| Code | Meaning |
|------|---------|
| E001 | require not at top of file |
| E002 | require argument not a string literal |
| E003 | require not assigned to local |
| E004 | module not found (lists searched paths) |
| E005 | circular dependency |
| E006 | `package.loaded` manipulation |
| E007 | ambiguous module resolution |
| E008 | parse error |
| E009 | too many modules (bundle module-count limit exceeded) |
| E010 | cannot read file (IO error) |
| E011 | entry file not found |
| E012 | file too large (byte-length limit exceeded) |
| W001 | duplicate require |
| W002 | top-level vararg in bundled module |
| W003 | circular dependency between lazily-loaded modules |
| W004 | `.luaurc` alias `self` shadowed by built-in `@self` |
