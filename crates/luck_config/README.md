# luck_config

`luck.json` and `.editorconfig` loading for the luck toolchain.

## Overview

The tools never read a config file, but each one owns the `luck.json` shape of its options: `TransformConfig` in `luck_minifier`, `DynamicRequire` in `luck_bundler`, the option enums in `luck_formatter`, and `LintConfig` in `luck_linter` derive `Deserialize` behind an optional `serde` feature, and `JsonSchema` behind `schemars`. This crate sits above them, turns `serde` on, and deserializes `transforms`, `dynamic_require`, and `lint` straight into those types. The `format` section is the one shape of its own, `FormatConfig`, which leaves each option unset unless the config sets it, so it can layer over `.editorconfig`. `luck_cli` and `luck_lsp` depend on this crate, the `luck` facade re-exports it behind its `config` feature, and no tool depends on it.

## Architecture

Every struct uses `#[serde(deny_unknown_fields)]`, and unknown keys and invalid enum values are hard errors. The one open map is the linter's `RuleSettings`, whose other keys are rule names for the linter to check.

- **`config`** holds `LuckConfig`, its build sub-objects `EntryConfig` and `ProfileOverrides`, and the `extends` merge, which merges `format` with `FormatConfig::merge_onto` and `lint` with the linter's own `LintConfig::merge_onto`. `DialectKey` picks the `lua` or `luau` key for a file by its extension and holds each key's default, and `LuckConfig::target_for_path` resolves through it, so the CLI and the LSP pick a file's dialect the same way.
- **`format_config`** holds the `format` section's `FormatConfig` and its conversion into `FormatOptions`, which the CLI and the LSP both go through.
- **`load`** owns `parse_luck_config` (JSON5), the recursive `extends` chain with its cycle, depth, and transitive `root`-boundary checks, upward `discover_config`, and `load_or_discover_config`, which loads an explicit `-c` path or else discovers. Both return a `ProjectConfig`: the config and the directory holding it, which roots the project's relative paths and globs.
- **`resolve`** turns a loaded `LuckConfig` into `BuildConfig`s, applying the selected profile and resolving entry/output paths for the single-entry or multi-entry (`entries`) form. `check_output_paths` rejects an output that would replace a source file or another output, and `luck bundle` goes through it too.
- **`filter`** holds `ProjectFilter`, which decides from `include`/`exclude` globs whether a file belongs to the project (exclude wins). `*` stops at `/` and `**/` crosses directories. `is_excluded` asks about `exclude` alone, which is all a file named on the command line is held to. The CLI and the LSP both build it through `LuckConfig::project_filter`. `glob_set` compiles globs with the same rules, and the CLI uses it for its path arguments.
- **`editorconfig`** holds `resolved_format_config`, which layers `FormatConfig` precedence: built-in defaults < `.editorconfig` < the luck.json `format` section. Without a file path, for an untitled document or `fmt --no-editorconfig`, the `format` section stands alone. It reads the spec itself, and matches sections with a compiled glob whose memoized matching stays polynomial on any section.

`.luaurc` files belong to `luck_resolver`, which parses and caches them for Luau alias resolution.

The VS Code schema at `editors/vscode/schemas/luckrc.schema.json` is **generated** from `LuckConfig`, and a test checks it for drift. The type names are its `$defs` keys and the doc comments its descriptions, so renaming a config type or editing one of its doc comments changes the schema. Only that test derives `JsonSchema`, for this crate's types and, through each tool's `schemars` feature, for the tools' own, so `schemars` is a dev-dependency here and optional in each tool. Regenerate the schema with `just schema`.
