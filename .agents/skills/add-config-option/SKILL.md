---
name: add-config-option
description: Adds or changes a luck.json configuration field end-to-end - typed field, schema regen, precedence layering, CLI and LSP plumbing. Use when asked to add a config option, make X configurable, add a luck.json field, or for any edit to config types in luck_core.
---

# Add a config option

Config is one typed source of truth in `luck_core`
(`src/config/*.rs`, `src/format_options.rs`, `src/transform_config.rs`),
deserialized with `deny_unknown_fields`, projected into a generated JSON
schema, and layered under a strict precedence. Skipping the schema regen
fails a test; skipping the precedence or CLI/LSP plumbing fails **silently** -
those are the steps this checklist exists for.

## 1. Add the typed field

On the right struct, mirroring its neighbors:

- `#[serde(default)]` with a sensible default - existing configs must keep
  working.
- Keep the struct's `#[serde(deny_unknown_fields)]` contract intact.
- Prefer enums over free-form strings/bools when there are >=2 modes.
- Derive `schemars::JsonSchema` with a `///` doc comment - it becomes the
  schema description users see in VS Code.

## 2. Regenerate the schema

`just schema`, then `just test -p luck_core` (the drift test must pass).
Never hand-edit `editors/vscode/schemas/luckrc.schema.json`.

## 3. Thread it to the consumer - in BOTH paths

Find where the option's struct is consumed (formatter, minifier, bundler,
linter driver) and use the field. Grep for a neighboring field to find every
consumption site: a field read in only one of the CLI (`luck_cli`) and LSP
(`luck_lsp/src/config.rs`) paths is a recurring bug.

## 4. Respect precedence (format options only)

defaults < `.editorconfig` < `luck.json` `format`. If the option has an
`.editorconfig` equivalent, map the `ec4rs::property` type onto the field in
`luck_core/src/editorconfig.rs` and add a precedence test.

## 5. CLI flag (only if warranted)

Most options are config-file-only. If a flag is justified, add it in
`luck_cli`; flag beats config file; flag name = JSON key, kebab-cased.

## 6. Tests

- Round-trip: a `luck.json` string with the new field deserializes and the
  value reaches the consumer.
- Rejection: a typo'd key next to the new field errors.
- If precedence applies: an `.editorconfig` + `luck.json` combination test.

## 7. Gate and bump

`just lint && just test -p luck_core -p luck_cli`. New config surface = minor
bump: follow `.agents/skills/bump-versions/SKILL.md`.
