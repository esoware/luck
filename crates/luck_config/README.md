# luck_config

Loads a `luck.json`, or a config an embedder gives, into one validated project, which every luck command and the language server ask for each file's settings.

## Overview

`luck.json` is JSON5, with arrays and objects nested at most 128 levels deep. Discovery walks up from a directory to the nearest `luck.json`, and only that file applies: a config takes nothing from one above it unless it `extends` it, and `root: true` keeps an `extends` chain inside its directory. `Project::load` reads the file and what it extends, merges them, and checks everything at once: the syntax, every key, the globs, the lint rule names and options, and the keys that can't go together, such as `entry` and `entries`, or `security` on a project that doesn't load `@roblox`. A problem comes back as one E014 `Diagnostic` placed where it is in the file at fault, or E010 when the file can't be read.

A config is a value as much as a file. `Project::from_config` takes the text of one no file holds, such as the one an editor gives the language server, with a root its paths and globs are relative to, and validates it as `Project::load` does a `luck.json` with that text, so a mistake in it gets the same E014, placed in that text, as a `ConfigError::Given`, and a definition file that can't be read comes back as an E010 in that file, as a `ConfigError::File`. Such a config has no file to extend another from, so `extends` in it is an E014.

A project reads everything, its config files, its definition files, and the `.editorconfig` files above a file, and canonicalizes every path it matches, through the `luck_resolver::FileSystem` it's given, the disk's `FileSystemOs` or files only its caller holds, as `find_config` and `located_path` do through the one they're given.

Each tool defines the type of its own section, so `transforms`, `dynamic_require`, `format`, and `lint` deserialize straight into `TransformConfig`, `DynamicRequire`, `FormatConfig`, and `LintConfig`, through the `serde` features this crate turns on, and `variant` and `security` into `Variant` and `SecurityLevel`. The linter checks its rule names as its section deserializes.

`definitions` lists what the project's files run with over Luau's standard library, in order, the entries of the files it extends first: `@roblox` for Roblox's API at the `security` level, another `@name` being an E014, and any other entry a definition file, relative to the config that names it, as `extends` does, whose name ends in `.d.luau`, or it's an E014. `Project::load` layers each over Luau's library, leaving out a definition file that doesn't parse rather than failing, since `lint`, `check`, and the language server report its errors on it, and keeps what each entry loads, a file's text among it, which `definitions` gives `luck check` to load in the same order. A project's files include its definition files, which the language server watches. Only what `definitions` lists is loaded, but a file is a definition file by its name, as `luck_resolver::is_definition_file` says, so a `.d.luau` file is one of a project's files like any other, which `lint`, `fmt`, and `check` read as a definition file, and the linter runs no rule on.

A project gives every file one standard library, and each file the rest of its settings:

- `stdlib` gives Luau's standard library with what `definitions` loads layered over it, which the linter and the language server read, and `definitions` each entry's `DefinitionSource`, Roblox's API or a definition file's path and text.
- `security` gives the `SecurityLevel` the `security` key names, `None` unless it names one, which decides the part of Roblox's API the files reach, and which a project that doesn't load `@roblox` can't set.
- `linter_for` gives the `luck_linter::Linter` of the `lint` section with the `overrides` the file's path matches layered over it, in order, as oxlint's `overrides` do. The section compiles once at load, and each set of overrides with `lint` settings compiles once, for the first file that matches it, so every file it covers shares that linter. `overlay_lint` layers command-line flags over every file's, overrides included.
- `format_options` layers the defaults, then `.editorconfig`, then the `format` section, then the matching `overrides`.
- `filter` holds `include` and `exclude`, and `bundle_options` resolves modules from the project's root.
- `builds` turns the build keys and a profile into the bundles `luck build` writes.

Every glob matches relative to the `luck.json` directory, whichever file in an `extends` chain sets it, and `*` stops at `/` while `**/` crosses directories. A directory with no `luck.json` above it gets `Project::defaults`, rooted where discovery started.

`walk_source_files` finds the `.luau` and `.lua` files under a directory on disk, past what `.gitignore` and `.luckignore` hide, so `lint`, `fmt`, and `check` take the same files the language server's workspace holds. The caller's `WalkVisitor` says which project each directory and file is in, whose `exclude` leaves a whole directory out and whose `include` and `exclude` take each file or leave it. A file's globs, as its `overrides`, match it under its directory's canonical path, as `located_path` gives it, so a symlink goes by where it is, the same place its project comes from, and is read where it leads. A document an editor opened as Luau needs no name of Luau's, so with no `include`, `includes_document` takes it whatever its name, as an editor's `inmemory://model/1` has none.

The VS Code extension's schema is generated from the config types, with the doc comments as its descriptions.

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
