//! # `luck_config`
//!
//! Loads a `luck.json`, through the files it `extends`, into one validated
//! [`Project`], which every luck command and the language server ask for its
//! definitions and standard library, each file's linter and format options,
//! and the build's entries and module resolution.
//!
//! Everything is checked at load: the JSON5 syntax, every key, the globs,
//! the lint rule names and options, the keys that can't go together, such as
//! `security` on a project that doesn't load `@roblox`, and the definition
//! files `definitions` names, which layer over Luau's library. A problem
//! comes back as one E014 [`Diagnostic`] placed where it is, or a definition
//! file's E008s, in the [`FileDiagnostics`] of the file at fault.
//!
//! Each tool derives the `luck.json` shape of its own option types behind its
//! `serde` feature, which this crate turns on, so `transforms`,
//! `dynamic_require`, `format`, and `lint` deserialize straight into them. The
//! `format` section leaves each formatter option unset unless the config sets
//! it, so it layers over `.editorconfig`. `overrides` layer `lint` and `format`
//! settings over the files their globs match.
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
//!
//! # Usage
//!
//! ```no_run
//! use std::path::Path;
//!
//! use luck_config::{Editorconfigs, Project, find_config};
//!
//! let directory = Path::new("src");
//! let project = match find_config(directory) {
//!     Some(config_path) => Project::load(&config_path).expect("a valid luck.json"),
//!     None => Project::defaults(directory.to_path_buf()),
//! };
//! let file = Path::new("src/main.luau");
//! let library = project.stdlib().library();
//! let linter = project.linter_for(Some(file));
//! let format = project.format_options(Some(file), Some(&Editorconfigs::default()));
//! ```

mod build;
mod config;
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
pub use editorconfig::{EDITORCONFIG_FILE_NAME, Editorconfigs};
pub use filter::{ProjectFilter, located_path};
pub use glob::glob_set;
pub use load::{CONFIG_FILE_NAME, find_config};
pub use project::{DefinitionSource, Project};
pub use walk::{WalkEntry, WalkError, WalkRoot, WalkVisitor, WalkedFile, walk_source_files};
