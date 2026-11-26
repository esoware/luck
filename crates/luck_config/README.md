# luck_config

Loads and validates `luck.json` project configs, including `extends`, `overrides`, and `.editorconfig`.

## Overview

The result is a `Project`, which every luck command and the language server get their settings from. It gives each file its lint and format settings, and the project its definitions and build entries.

Every key is checked when the config loads. Each problem becomes an E014 diagnostic pointing at its spot in the file.

`overrides` change lint and format settings for files matching a glob, and format settings layer on top of `.editorconfig`.

Files are read through a `FileSystem`, so an editor can pass in unsaved files. `walk_source_files` finds a project's `.luau` and `.lua` files, skipping anything `.gitignore` or `.luckignore` excludes.

The JSON schema in the VS Code extension is generated from these config types.

## Usage

```rust,no_run
use std::path::Path;

use luck_config::{Editorconfigs, Project, find_config};
use luck_resolver::FileSystemOs;

let directory = Path::new("src");
let project = match find_config(&FileSystemOs, directory) {
    Some(config_path) => Project::load(&config_path, FileSystemOs).expect("a valid luck.json"),
    None => Project::defaults(directory.to_path_buf(), FileSystemOs),
};
let file = Path::new("src/main.luau");
let library = project.stdlib().library();
let linter = project.linter_for(Some(file));
let format = project.format_options(Some(file), Some(&Editorconfigs::default()));
```

## API

- `Project` is a loaded project, from `load`, `from_config`, or `defaults`.
- `Project` gives its settings through `stdlib`, `definitions`, `linter_for`, `format_options`, `transforms`, `bundle_options`, and `builds`.
- `find_config` finds the nearest `luck.json` at or above a directory.
- `ConfigError` says why a config didn't load.
- `CONFIG_FILE_NAME` is `luck.json`.
- `DefinitionSource` is what one `definitions` entry loads.
- `BuildConfig` is one bundle that `luck build` writes.
- `check_output_paths` fails if a build's outputs would overwrite its inputs or each other.
- `Editorconfigs` caches `.editorconfig` lookups, and `EDITORCONFIG_FILE_NAME` is `.editorconfig`.
- `ProjectFilter` applies the project's `include` and `exclude` globs.
- `located_path` gives the path a file's globs match against.
- `glob_set` compiles globs the way a config does.
- `walk_source_files` finds a directory's source files, with `WalkVisitor`, `WalkRoot`, `WalkEntry`, `WalkedFile`, and `WalkError`.
