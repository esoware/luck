//! # `luck_config`
//!
//! Reads `luck.json` and `.editorconfig` into the options each luck tool
//! takes. Each tool derives the `luck.json` shape of its own option types
//! behind its `serde` feature, which this crate turns on, so `transforms`,
//! `dynamic_require`, and `lint` deserialize straight into them. The
//! `format` section is [`FormatConfig`], which leaves each formatter option
//! unset unless the config sets it, so it layers over `.editorconfig`, and
//! converts into `luck_formatter::FormatOptions` with `From`.
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

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

mod config;
mod editorconfig;
mod filter;
mod format_config;
mod load;
mod path;
mod resolve;

pub use config::{DialectKey, EntryConfig, LuckConfig, ProfileOverrides};
pub use editorconfig::resolved_format_config;
pub use filter::{ProjectFilter, glob_set};
pub use format_config::FormatConfig;
pub use load::{
    CONFIG_FILE_NAME, ProjectConfig, config_directory, discover_config, load_or_discover_config,
    load_with_extends, parse_luck_config,
};
pub use resolve::{BuildConfig, check_output_paths, resolve_build_config};
