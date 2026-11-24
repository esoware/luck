// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod config_transforms;
mod diagnostics;
mod exit_codes;
mod lint_output;
mod path_selection;
mod project_config;
mod regressions;
mod stack;
