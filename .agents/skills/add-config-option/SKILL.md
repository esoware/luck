---
name: add-config-option
description: Adds or changes a luck.json configuration field end-to-end, covering the typed field, schema regen, precedence layering, and CLI and LSP plumbing. Use when asked to add a config option, make X configurable, add a luck.json field, or for any edit to config types in luck_config.
---

# Add a config option

Config is one typed source of truth in `luck_config` (`src/*.rs`, one module per `luck.json` section), deserialized with `deny_unknown_fields`, projected into a generated JSON schema, and layered under a strict precedence. The tools never read config, but each derives the `luck.json` shape of its own option types behind optional `serde` and `schemars` features, and `luck_config` deserializes straight into them. The `format` section is the exception: `luck_config`'s `FormatConfig` holds each formatter option unset unless the config sets it, and converts into `FormatOptions` with `From`. Skipping the schema regen fails a test. Skipping the precedence or CLI/LSP plumbing fails **silently**. Those are the steps this checklist exists for.

## 1. Add the typed field

On the right struct, mirroring its neighbors:

- A tool option goes on the tool's type (`luck_minifier::TransformConfig`, `luck_bundler::DynamicRequire`, `luck_linter::LintConfig`) with its default there and its serde attributes under `cfg_attr(feature = "serde", ...)`. A formatter option goes on `luck_formatter::FormatOptions`, as an `Option` on `luck_config`'s `FormatConfig`, and into `FormatConfig::merge_onto` and the `From` conversion, which destructures in full, so a missed field fails to compile.
- `#[serde(default)]` with a sensible default, because existing configs must keep working.
- Keep the struct's `#[serde(deny_unknown_fields)]` contract intact.
- Prefer enums over free-form strings/bools when there are >=2 modes.
- Give it a `///` doc comment. It becomes the schema description users see in VS Code. `luck_config`'s types derive `schemars::JsonSchema` under `cfg_attr(test)`, and each tool's types derive it under its `schemars` feature, which only `luck_config`'s dev-dependencies turn on.

## 2. Regenerate the schema

`just schema`, then `just test -p luck_config` (the drift test must pass). Never hand-edit `editors/vscode/schemas/luckrc.schema.json`.

## 3. Thread it to the consumer, in BOTH paths

Find where the option's struct is used (formatter, minifier, bundler, linter driver) and read the field there. Grep for a neighboring field to find every site: a field read in only one of the CLI (`luck_cli`) and LSP (`luck_lsp/src/settings.rs`) paths is a recurring bug. When both paths read a key the same way, give `luck_config` one function they both go through, as `LuckConfig::target`, `LuckConfig::project_filter`, and `resolved_format_config` do, instead of reading the raw field twice.

## 4. Respect precedence (format options only)

defaults < `.editorconfig` < `luck.json` `format`. If the option has an `.editorconfig` equivalent, map the property onto the field in `format_config_for` in `luck_config/src/editorconfig.rs` and add a precedence test.

## 5. CLI flag (only if warranted)

Most options are config-file-only. If a flag is justified, add it in `luck_cli`. The flag beats the config file, and its name is the JSON key, kebab-cased.

## 6. Tests

- Round-trip: a `luck.json` string with the new field deserializes and the value reaches the consumer.
- Rejection: a typo'd key next to the new field errors.
- If precedence applies: an `.editorconfig` + `luck.json` combination test.

## 7. Gate and bump

`just lint && just test -p luck_config -p luck_cli`. A new config option = minor bump: follow `.agents/skills/bump-versions/SKILL.md`.
