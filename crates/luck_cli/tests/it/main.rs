// Lets clippy.toml's allow-*-in-tests settings cover the shared helpers, not
// just the `#[test]` functions.
#![cfg(test)]
#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

mod common;

mod config_transforms;
mod diagnostics;
mod exit_codes;
mod lint_output;
mod path_selection;
mod project_config;
mod regressions;
