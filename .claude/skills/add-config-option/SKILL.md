---
name: add-config-option
description: Adds or changes a luck.json configuration field end-to-end - typed field, schema regen, precedence layering, CLI and LSP plumbing - for format options, lint settings, minifier flags, bundler options, or project shape. Use when asked to add a config option, make X configurable, add a new luck.json field or setting, or for any edit to crates/luck_core/src/config.rs, format_options.rs, or transform_config.rs.
argument-hint: <option-name>
allowed-tools: Read, Edit, Write, Grep, Glob, Bash(cargo:*)
---

# Add a config option

Config is one typed source of truth in `luck_core`, deserialized with
`deny_unknown_fields`, projected into a generated JSON schema, and
layered under a strict precedence. A new option touches up to five
places; skipping the schema regen fails a test, but skipping the
precedence or CLI plumbing fails silently.

## Layout

| Where | What |
|---|---|
| `crates/luck_core/src/config.rs` | `LuckConfig` + project shape (`extends`/`include`/`exclude`/`root`), discovery |
| `crates/luck_core/src/format_options.rs` | `FormatOptions` + its enums |
| `crates/luck_core/src/transform_config.rs` | `TransformConfig` - minifier pass flags |
| `crates/luck_core/src/editorconfig.rs` | `.editorconfig` layer (format options only) |
| `editors/vscode/schemas/luckrc.schema.json` | **generated** - never hand-edit |
| `crates/luck_cli/src/cli.rs` | flag plumbing, `resolve_project_config` |
| `crates/luck_lsp/src/config.rs` | LSP-side config discovery/caching |

## Steps

### 1. Add the typed field

Put it on the right struct (see table). Rules:

- `#[serde(default)]` with a sensible default - existing configs must
  keep working.
- The struct must have (or keep) `#[serde(deny_unknown_fields)]` -
  unknown keys and invalid enum values are hard errors project-wide;
  don't introduce a struct that breaks that contract.
- Enums over free-form strings/bools wherever there are >=2 modes -
  they self-document in the schema.
- Derive `schemars::JsonSchema` like the neighboring fields, with a
  `///` doc comment - the doc comment becomes the schema description
  users see in VS Code.

### 2. Regenerate the schema

```sh
cargo test -p luck_core regenerate_luckrc_schema -- --ignored
cargo test -p luck_core          # drift test must pass now
```

### 3. Thread it to the consumer

Find where the option's struct is consumed (formatter, minifier,
bundler, linter driver) and use the field. Grep for a neighboring field
to find every consumption site - config fields read in only one of the
CLI and LSP paths are a recurring bug.

### 4. Respect precedence (format options only)

defaults < `.editorconfig` < `luck.json` `format`. If the new option has
an `.editorconfig` equivalent, map the corresponding `ec4rs::property`
type onto the `FormatConfig` field in `editorconfig.rs` (parsing/walking
is ec4rs's job - only the property->field mapping lives here) and add a
precedence test; if not, it simply layers via `luck.json`.

### 5. CLI flag (only if the option warrants a flag)

Most options are config-file-only. If a flag is justified, add it to the
relevant command in `cli.rs`; flag beats config file. Keep the flag name
identical to the JSON key (kebab-cased).

### 6. Tests

- Round-trip: a `luck.json` string with the new field deserializes and
  the value reaches the consumer.
- Rejection: a typo'd key next to the new field errors (guards the
  `deny_unknown_fields` contract).
- If precedence applies: an `.editorconfig`+`luck.json` combination test.

### 7. Gate & bump

```sh
cargo clippy -p luck_core -p luck_cli --all-targets -- -D warnings
cargo test -p luck_core && cargo test -p luck_cli
```

Minor bump `luck_core` (new config surface) plus any consumer crate that
gained behavior. Use `/bump-versions`.
