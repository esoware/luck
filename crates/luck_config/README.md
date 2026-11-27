# luck_config

Loads and validates `luck.json` project configs.

`Project::load` reads a `luck.json`, following its `extends` chain, and checks
every key, glob, rule name, and option, along with keys that conflict. Each
problem comes back as an E014 diagnostic at its location in the file. The
resulting `Project` answers what each file needs: its linter, its format
options, and its definitions, along with the project's build entries.

`overrides` change settings for the files their globs match, and format
settings layer on top of `.editorconfig`. Every file is read through a
`FileSystem`, so an editor can load a project from unsaved buffers.
`walk_source_files` finds a project's `.luau` and `.lua` files, skipping
whatever `.gitignore` and `.luckignore` exclude.

The JSON schema in luck's VS Code extension is generated from the config types.

## Usage

This example finds the `luck.json` above `src`, or falls back to the defaults,
and asks for one file's settings:

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
let definitions = project.definitions_for(Some(file));
let library = definitions.stdlib().library();
let linter = project.linter_for(Some(file));
let format = project.format_options(Some(file), Some(&Editorconfigs::default()));
```

## API

* `Project`, from `load`, `from_config`, or `defaults`, with `definitions`,
  `definitions_for`, `reads_definition_files`, `takes_definition_at`,
  `linter_for`, `format_options`, `transforms`, `bundle_options`, and
  `builds`.
* `find_config`, which finds the nearest `luck.json` at or above a directory.
* `ConfigError`, why a config didn't load.
* `CONFIG_FILE_NAME` and `EDITORCONFIG_FILE_NAME`, the config files' names.
* `Definitions`, what a file loads, with `sources`, `security`, and `stdlib`,
  and `DefinitionSource`, one definition it loads.
* `BuildConfig`, one bundle `luck build` writes, and `check_output_paths`,
  which rejects outputs that would overwrite an input or each other.
* `Editorconfigs`, which caches the `.editorconfig` files a project reads.
* `ProjectFilter`, the project's `include` and `exclude` globs, and
  `located_path`, the path a file's globs match against.
* `glob_set`, which compiles globs the way a config does.
* `walk_source_files`, with `WalkVisitor`, `WalkRoot`, `WalkEntry`,
  `WalkedFile`, and `WalkError`.
