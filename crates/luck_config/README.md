# luck_config

Loads `luck.json` and `.editorconfig` into the options each luck tool takes.

## Overview

`luck.json` is JSON5, and `LuckConfig` is its top level. Each tool defines the type of its own section, so `transforms`, `dynamic_require`, and `lint` deserialize straight into `TransformConfig`, `DynamicRequire`, and `LintConfig`, through the `serde` features this crate turns on. The `format` section is a `FormatConfig`, which leaves each option unset unless the config sets it, so it layers over `.editorconfig`: built-in defaults, then `.editorconfig`, then `format`. It converts into `luck_formatter::FormatOptions` with `From`. An unknown key or an invalid value is an error.

Discovery walks up from a directory to the nearest `luck.json`, and only that file applies. A config takes nothing from one above it unless it `extends` it, and `root: true` keeps an `extends` chain inside its directory. The `lua` and `luau` keys name the dialect of `.lua` and `.luau` files, and either can name any dialect. `include` and `exclude` choose the project's files, and the build keys, `entry`, `entries`, `output`, and `profiles` among them, resolve into one `BuildConfig` per entry.

The VS Code extension's schema is generated from `LuckConfig`, with the doc comments as its descriptions.

## Usage

- `parse_luck_config` reads one file's text, `load_with_extends` loads a file and what it extends, and `discover_config` and `load_or_discover_config` find one.
- `LuckConfig::target_for_path` picks a file's dialect, through `DialectKey`.
- `resolved_format_config`, or `Editorconfigs` across many files, layers a file's `.editorconfig` under the `format` section.
- `resolve_build_config` turns the build settings and a profile into the builds `luck build` runs.

```rust
use luck_config::parse_luck_config;
use luck_formatter::FormatOptions;

let config = parse_luck_config(r#"{ "format": { "line_width": 80 } }"#).unwrap();
let options = FormatOptions::from(&config.format.unwrap_or_default());
assert_eq!(options.line_width, 80);
```
