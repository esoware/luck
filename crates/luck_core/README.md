# luck_core

Shared types, configuration parsing, and diagnostics for the luck toolchain.

## Overview

`luck_core` sits one level above `luck_token` and provides everything else the higher-level tools share: build targets, transform and format configuration, diagnostics, and `luck.json` project configuration parsing.

## Key features

- **Build targets.** `LuaTarget` enumerates the supported flavors (Lua 5.1 through 5.5, standalone Luau, and Roblox Luau) and parses from common string forms (`"lua54"`, `"5.4"`, `"luau"`, `"roblox"`). It projects onto the two independent axes the rest of the workspace keys off: `lua_version()` (syntax) and `stdlib_environment()` (stdlib).
- **Transform toggles.** `TransformConfig` enables and disables individual minifier passes, with serde-driven partial overrides from JSON.
- **Format options.** The option enums (`IndentStyle`, `QuoteStyle`, `HexCase`, `CallParentheses`, `CollapseSimpleStatement`, `LineEndings`, `BlockNewlineGaps`, `SpaceAfterFunction`) live here so `FormatConfig` deserializes directly into them, and `luck_formatter` re-exports every type. Their defaults live in `luck_formatter`'s `FormatOptions`.
- **Diagnostics.** `Diagnostic` carries a code, message, severity, file path, span, and help text. Its numbered `E002`-`E012` / `W001`-`W007` constructors, such as `Diagnostic::e008`, live in `diagnostic/codes.rs` and are the only way to build one with an `E` or `W` code.
- **Project configuration.** `luck.json` (parsed as JSON5), profile overrides, `extends` chains, config discovery up the directory tree, and `.editorconfig` layering.
- **Source loading.** `read_source_file` reads a file with SIMD UTF-8 validation, matching `fs::read_to_string`'s error behavior.
- **Path form.** `normalize_path` gives the canonical, forward-slash form the resolver and bundler key modules on, and `native_path` turns such a path back into the platform's separators for filesystem calls.

Every item is exported once, from the crate root.

## Architecture

### Build targets

`LuaTarget` is the configuration-layer view of a dialect. It maps to `luck_token::LuaVersion` for the parser, lexer, codegen, and formatter, and to `luck_token::StdlibEnvironment` for the semantic layer, linter, and LSP. The split happens once at each entry boundary.

### Transform configuration

`TransformConfig` is a struct of bool flags, one per minifier pass: `fold_constants`, `rename_locals`, `inline_locals`, `merge_locals`, `remove_dead_code`, `simplify_statements`, `simplify_indexes`, `simplify_parens`, `shorten_strings`, `shorten_numbers`, `lift_locals`, and `rename_globals`. All default to on except `rename_globals`, which is opt-in because renamed globals live under different `_G` keys and break cross-chunk consumers.

### Project configuration

The `config` module handles project configuration, split into focused submodules. The deserialized `luck.json` types come first, one module per section. Every struct uses `#[serde(deny_unknown_fields)]` and every type derives `schemars::JsonSchema`, and unknown keys and invalid enum values are hard errors.

- **`luck_config`** holds `LuckConfig`, its build sub-objects `EntryConfig` and `ProfileOverrides`, and the `extends` merge. `DialectKey` picks the `lua` or `luau` key for a file by its extension and holds each key's default, and `LuckConfig::target_for_path` resolves through it, so the CLI and the LSP pick a file's dialect the same way.
- **`format_config`** holds the `format` section's `FormatConfig` and the formatter option enums.
- **`lint_config`** holds the `lint` section's `LintConfig`, `RuleSetting`, and `Category`, which `luck_linter` re-exports.
- **`transform_config`** and **`dynamic_require`** hold `TransformConfig` and `DynamicRequire`, the `transforms` and `dynamic_require` values.
- **`load`** owns `parse_luck_config` (JSON5), the recursive `extends` chain with cycle and `root`-boundary checks, upward `discover_config`, and `load_or_discover_config`, which loads an explicit `-c` path or else discovers. Both return a `ProjectConfig`: the config and the directory holding it, which roots the project's relative paths and globs.
- **`resolve`** turns a loaded `LuckConfig` into `BuildConfig`s, applying the selected profile and resolving entry/output paths for the single-entry or multi-entry (`entries`) form.
- **`filter`** holds `ProjectFilter`, which decides from `include`/`exclude` globs whether a file belongs to the project (exclude wins).
- **`editorconfig`** holds `resolved_format_config`, which layers `FormatConfig` precedence: built-in defaults < `.editorconfig` < the luck.json `format` section. Without a file path, for an untitled document or `fmt --no-editorconfig`, the `format` section stands alone.

`.luaurc` files belong to `luck_resolver`, which parses and caches them for Luau alias resolution.

The VS Code schema at `editors/vscode/schemas/luckrc.schema.json` is **generated** from `LuckConfig`, and a test checks it for drift. The type names are its `$defs` keys and the doc comments its descriptions, so renaming a config type or editing one of its doc comments changes the schema. Regenerate it with `cargo test -p luck_core regenerate_luckrc_schema -- --ignored`.

### Diagnostics

`Diagnostic` carries a code, message, severity, file path, source `Span`, and optional help text. Consumers never inline literal codes. They call the numbered constructors in `diagnostic/codes.rs`, such as `Diagnostic::e004`. Only a lint diagnostic, whose code is its rule's name, is built field by field. Codes marked "not emitted" have no constructor, and their numbers stay reserved:

| Code | Meaning |
|------|---------|
| E001 | require not at top of file (not emitted) |
| E002 | require argument not a string literal (only in `dynamic_require: "error"`) |
| E003 | require not assigned to local (not emitted) |
| E004 | module not found (lists searched paths) |
| E005 | circular dependency (not emitted, since cycles warn with W003) |
| E006 | write to `package.loaded` (Luau targets) |
| E007 | ambiguous module resolution |
| E008 | parse error |
| E009 | too many modules (bundle module-count limit exceeded) |
| E010 | cannot read file (IO error) |
| E011 | entry file not found |
| E012 | file too large (byte-length limit exceeded) |
| W001 | duplicate require |
| W002 | top-level vararg in bundled module (not emitted) |
| W003 | circular dependency between lazily-loaded modules |
| W004 | `.luaurc` alias `self` shadowed by built-in `@self` |
| W005 | require call not bundled (a local named `require`, or the global `require` used other than in a direct call) |
| W006 | Luau hot comment has no effect in a bundle |
| W007 | require argument not a string literal, so the call resolves at runtime |
