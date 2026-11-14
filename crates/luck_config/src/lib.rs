//! # `luck_config`
//!
//! Reads `luck.json` and `.editorconfig` into the options each luck tool
//! takes. The tools own plain option types and know nothing of config files.
//! This crate holds the `luck.json` shape of each one, with the same name, and
//! converts it into the tool's type with `From`. The `lint` section is the
//! linter's own [`LintConfig`], since rule names are its keys.
//!
//! Every config struct denies unknown fields. The one open map is
//! `RuleSettings`, whose other keys are rule names for the linter to check.
//!
//! The VS Code schema is generated from [`LuckConfig`], with the type names as
//! its `$defs` keys and the doc comments as its descriptions.
//!
//! # Usage
//!
//! ```
//! use luck_config::parse_luck_config;
//! use luck_formatter::FormatOptions;
//!
//! let config = parse_luck_config(r#"{ "format": { "line_width": 80 } }"#).unwrap();
//! let options = FormatOptions::from(&config.format.unwrap_or_default());
//! assert_eq!(options.line_width, 80);
//! ```
//!
//! [`LintConfig`]: luck_linter::LintConfig

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

mod config;
mod dynamic_require;
mod editorconfig;
mod filter;
mod format_config;
mod load;
mod path;
mod resolve;
mod transform_config;

pub use config::{DialectKey, EntryConfig, LuckConfig, ProfileOverrides};
pub use dynamic_require::DynamicRequire;
pub use editorconfig::resolved_format_config;
pub use filter::{ProjectFilter, glob_set};
pub use format_config::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatConfig, HexCase, IndentStyle,
    LineEndings, QuoteStyle, SpaceAfterFunction,
};
pub use load::{
    CONFIG_FILE_NAME, ProjectConfig, config_directory, discover_config, load_or_discover_config,
    load_with_extends, parse_luck_config,
};
pub use resolve::{BuildConfig, check_output_paths, resolve_build_config};
pub use transform_config::TransformConfig;
