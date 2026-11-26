// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod build_preamble;
mod config_transforms;
mod diagnostics;
mod exit_codes;
mod fmt_range;
mod init;
mod lint_fix;
mod lint_output;
mod output_paths;
mod path_selection;
mod project_config;
mod stack;
mod type_errors;
