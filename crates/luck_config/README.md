# luck_config

Loads a `luck.json` into one validated project, which every luck command and the language server ask for each file's settings.

## Overview

`luck.json` is JSON5. Discovery walks up from a directory to the nearest `luck.json`, and only that file applies: a config takes nothing from one above it unless it `extends` it, and `root: true` keeps an `extends` chain inside its directory. `Project::load` reads the file and what it extends, merges them, and checks everything at once: the syntax, every key, the dialects, the globs, the lint rule names and options, and the build keys that can't go together. A problem comes back as one E014 `Diagnostic` placed where it is in the file at fault, or E010 when the file can't be read.

Each tool defines the type of its own section, so `transforms`, `dynamic_require`, `format`, and `lint` deserialize straight into `TransformConfig`, `DynamicRequire`, `FormatConfig`, and `LintConfig`, through the `serde` features this crate turns on, and the dialect keys into `LuaTarget`. The linter checks its rule names as its section deserializes.

`std` names std definition files, each relative to the config that names it, as `extends` does, and listed after the ones of the files it extends. `Project::load` reads each as a `luck_stdlib::StdlibDefinition` and layers them, in order, over the library of each dialect, so a std file with a problem is reported in it, and a project's files include its std files, which the language server watches.

A project gives each file its settings:

- `target_for` picks its dialect: the `luau` key for a `.luau` file and the `lua` key for any other, and either can name any dialect. `stdlib_for` gives that dialect's standard library with the `std` definitions layered over it, which the linter and the language server read.
- `linter_for` gives the `luck_linter::Linter` of the `lint` section with the `overrides` the file's path matches layered over it, in order, as oxlint's `overrides` do. The section compiles once at load, and each set of overrides with `lint` settings compiles once, for the first file that matches it, so every file it covers shares that linter. `overlay_lint` layers command-line flags over every file's, overrides included.
- `format_options` layers the defaults, then `.editorconfig`, then the `format` section, then the matching `overrides`.
- `filter` holds `include` and `exclude`, and `bundle_options` and `search_paths` resolve modules from the project's root.
- `builds` turns the build keys and a profile into the bundles `luck build` writes.

Every glob matches relative to the `luck.json` directory, whichever file in an `extends` chain sets it, and `*` stops at `/` while `**/` crosses directories. A directory with no `luck.json` above it gets `Project::defaults`, rooted where discovery started.

`walk_lua_files` finds the Lua files under a directory, past what `.gitignore` and `.luckignore` hide, so `lint`, `fmt`, and `check` take the same files the language server's workspace holds. The caller's `WalkVisitor` says which project each directory and file is in, whose `exclude` leaves a whole directory out and whose `include` and `exclude` take each file or leave it, by its canonical path, a symlink's target's for a symlink.

The VS Code extension's schema is generated from the config types, with the doc comments as its descriptions.

## Usage

```rust,no_run
use std::path::Path;

use luck_config::{Editorconfigs, Project, find_config};

let directory = Path::new("src");
let project = match find_config(directory) {
    Some(config_path) => Project::load(&config_path).expect("a valid luck.json"),
    None => Project::defaults(directory.to_path_buf()),
};
let file = Path::new("src/main.lua");
let target = project.target_for(Some(file));
let linter = project.linter_for(Some(file));
let format = project.format_options(Some(file), Some(&Editorconfigs::default()));
```
