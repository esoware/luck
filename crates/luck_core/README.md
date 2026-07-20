# luck_core

Shared types, configuration parsing, and diagnostics for the luck toolchain.

## Overview

`luck_core` sits one level above `luck_token` and provides everything else the higher-level tools share: build targets, transform and format configuration, diagnostics, and `luck.json` project configuration parsing.

## Key Features

- **Build targets** — `LuaTarget` enumerates the supported flavors (Lua 5.1 through 5.5, standalone Luau, and Roblox Luau) and parses from common string forms (`"lua54"`, `"5.4"`, `"luau"`, `"roblox"`). It projects onto the two independent axes the rest of the workspace keys off: `lua_version()` (syntax) and `stdlib_environment()` (stdlib).
- **Transform toggles** — `TransformConfig` enables and disables individual minifier passes, with serde-driven partial overrides from JSON.
- **Format options** — the option enums (`IndentStyle`, `QuoteStyle`, `HexCase`, `CallParentheses`, `CollapseSimpleStatement`, `LineEndings`, `BlockNewlineGaps`, `SpaceAfterFunction`) live here so `FormatConfig` deserializes directly into them; `luck_formatter` re-exports every type.
- **Rich diagnostics** — `Diagnostic` carries a code, message, severity, span, labels, and help text; the `error_at`/`warning_at` constructors are the single entry point, and the `diagnostics::errors` module owns the numbered `E001`–`E012` / `W001`–`W004` codes.
- **Project configuration** — `luck.json` (parsed as JSON5), `.luaurc`, profile overrides, `extends` chains, and config discovery up the directory tree.
- **Source loading** — `source_io::read_source_file` reads a file with SIMD UTF-8 validation, matching `fs::read_to_string`'s error behavior.

## Architecture

### Build Targets

`LuaTarget` is the configuration-layer view of a dialect. It maps to `luck_token::LuaVersion` for the parser, lexer, codegen, and formatter, and to `luck_token::StdlibEnvironment` for the semantic layer, linter, and LSP. The split happens once, here, at the config boundary; downstream never re-derives it.

### Transform Configuration

`TransformConfig` is a struct of bool flags, one per minifier pass: `fold_constants`, `rename_locals`, `inline_locals`, `merge_locals`, `remove_dead_code`, `simplify_statements`, `simplify_indexes`, `simplify_parens`, `shorten_strings`, `shorten_numbers`, `lift_locals`, and `rename_globals`. All default to on except `rename_globals` (opt-in — renamed globals live under different `_G` keys and break cross-chunk consumers); the config layer applies user overrides.

### Project Configuration

The `config` module handles project configuration, split into focused submodules re-exported under `config::`:

- **`schema`** — the deserialized `luck.json` surface (`LuckConfig`, `FormatConfig`, `LintConfig`, `RuleSetting`, `EntryConfig`, `ProfileOverrides`) and its `extends`/profile merge semantics. Every type uses `#[serde(deny_unknown_fields)]`, so unknown keys and invalid enum values are hard errors, and derives `schemars::JsonSchema`.
- **`load`** — `parse_luck_config` (JSON5), the recursive `extends` chain with cycle and `root`-boundary checks, upward `discover_config`, and `.luaurc` parsing (`LuauRc`, `parse_luaurc`).
- **`resolve`** — turns a loaded `LuckConfig` into executable `BuildConfig`s, applying the selected profile and resolving entry/output paths for the single-entry or multi-entry (`entries`) form.
- **`filter`** — `ProjectFilter` decides which files belong to a project from `include`/`exclude` globs (exclude wins).

`FormatConfig` precedence is layered separately in the `editorconfig` module: built-in defaults < `.editorconfig` < the luck.json `format` section, with luck.json always winning.

The VS Code schema at `editors/vscode/schemas/luckrc.schema.json` is **generated** from `LuckConfig` and drift-checked by a test — regenerate it with `cargo test -p luck_core regenerate_luckrc_schema -- --ignored`.

### Diagnostics

`Diagnostic` carries a code, message, severity, file path, source span, labels, and optional help text; `with_label`/`with_help` build it up. Consumers never inline literal codes — they call the `diagnostics::errors` constructors:

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
