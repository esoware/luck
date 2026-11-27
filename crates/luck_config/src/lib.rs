//! # `luck_config`
//!
//! Loads a `luck.json`, through the files it `extends`, or a config no file
//! holds, such as one an editor gives, into one validated [`Project`], which
//! every luck command and the language server ask for its definitions and
//! standard library, each file's linter and format options, and the build's
//! entries and module resolution. A project reads everything, its config
//! files, definition files, and `.editorconfig` files, through the
//! [`FileSystem`] it's given, the disk or files only its caller holds.
//!
//! Everything is checked at load: the JSON5 syntax, every key, the globs,
//! the lint rule names and options, the keys that can't go together, such as
//! `security` on a project that doesn't load `@roblox`, and the definition
//! files `definitions` takes, each a `.d.luau` file a path names or a
//! directory or glob finds, which layer over Luau's library, but for one that
//! doesn't parse, which adds nothing. A problem comes back as one E014
//! [`Diagnostic`] placed where it is, in the [`FileDiagnostics`] of the file
//! at fault, or for a config no file holds, as a [`ConfigError`].
//!
//! Each tool derives the `luck.json` shape of its own option types behind its
//! `serde` feature, which this crate turns on, so `transforms`,
//! `dynamic_require`, `format`, and `lint` deserialize straight into them. The
//! `format` section leaves each formatter option unset unless the config sets
//! it, so it layers over `.editorconfig`. `overrides` layer `lint` and `format`
//! settings, and `definitions` and `security`, over the files their globs
//! match.
//!
//! [`walk_source_files`] finds the `.luau` and `.lua` files a directory holds,
//! past what `.gitignore` and `.luckignore` hide, each taken or left out by
//! the `include` and `exclude` of the project a [`WalkVisitor`] says it's in,
//! so the path commands and the language server's workspace find the same
//! files.
//!
//! The VS Code schema is generated from the config types, with the type
//! names as its `$defs` keys and the doc comments as its descriptions.
//!
//! [`Diagnostic`]: luck_diagnostics::Diagnostic
//! [`FileDiagnostics`]: luck_diagnostics::FileDiagnostics
//! [`FileSystem`]: luck_resolver::FileSystem
//!
//! # Usage
//!
//! ```no_run
//! use std::path::Path;
//!
//! use luck_config::{Editorconfigs, Project, find_config};
//! use luck_resolver::FileSystemOs;
//!
//! let directory = Path::new("src");
//! let project = match find_config(&FileSystemOs, directory) {
//!     Some(config_path) => Project::load(&config_path, FileSystemOs).expect("a valid luck.json"),
//!     None => Project::defaults(directory.to_path_buf(), FileSystemOs),
//! };
//! let file = Path::new("src/main.luau");
//! let definitions = project.definitions_for(Some(file));
//! let library = definitions.stdlib().library();
//! let linter = project.linter_for(Some(file));
//! let format = project.format_options(Some(file), Some(&Editorconfigs::default()));
//! ```

mod build;
mod config;
mod definitions;
mod editorconfig;
mod filter;
#[cfg(test)]
mod fixtures;
mod glob;
mod load;
mod parse;
mod project;
mod walk;

pub use build::{BuildConfig, check_output_paths};
pub use definitions::{DefinitionSource, Definitions};
pub use editorconfig::{EDITORCONFIG_FILE_NAME, Editorconfigs};
pub use filter::{ProjectFilter, located_path};
pub use glob::glob_set;
pub use globset::GlobSet;
pub use load::{CONFIG_FILE_NAME, ConfigError, find_config};
pub use project::Project;
pub use walk::{WalkEntry, WalkError, WalkRoot, WalkVisitor, WalkedFile, walk_source_files};
