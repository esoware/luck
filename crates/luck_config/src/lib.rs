//! # `luck_config`
//!
//! Loads a `luck.json`, through the files it `extends`, into one validated
//! [`Project`], which every luck command and the language server ask for
//! each file's dialect and standard library, linter, and format options, and
//! for the build's entries and module resolution.
//!
//! Everything is checked at load: the JSON5 syntax, every key, the dialects,
//! the globs, the lint rule names and options, the build keys that can't go
//! together, and the std definitions `std` names, which layer over each
//! dialect's library. A problem comes back as one E014 [`Diagnostic`] placed
//! where it is, in the [`FileDiagnostics`] of the file at fault.
//!
//! Each tool derives the `luck.json` shape of its own option types behind its
//! `serde` feature, which this crate turns on, so `transforms`,
//! `dynamic_require`, and `lint` deserialize straight into them. The `format`
//! section leaves each formatter option unset unless the config sets it, so
//! it layers over `.editorconfig`. `overrides` layer `lint` and `format`
//! settings over the files their globs match.
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
//! let file = Path::new("src/main.lua");
//! let target = project.target_for(Some(file));
//! let linter = project.linter_for(Some(file));
//! let format = project.format_options(Some(file), Some(&Editorconfigs::default()));
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod build;
mod config;
mod editorconfig;
mod filter;
#[cfg(test)]
mod fixtures;
mod format_config;
mod glob;
mod load;
mod path;
mod project;

pub use build::{BuildConfig, check_output_paths};
pub use editorconfig::Editorconfigs;
pub use filter::ProjectFilter;
pub use glob::glob_set;
pub use load::{CONFIG_FILE_NAME, find_config};
pub use project::Project;
