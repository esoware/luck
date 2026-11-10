//! # `luck_core`
//!
//! Shared types, configuration parsing, and diagnostics for the luck toolchain.
//!
//! ## Key types
//!
//! - [`LuaTarget`]: the user-facing dialect (Lua 5.1-5.5, Luau, or Roblox),
//!   which projects onto a syntax version and a stdlib environment.
//! - [`LuckConfig`]: a parsed `luck.json`, loaded with the directory it
//!   roots as a [`ProjectConfig`] by [`load_or_discover_config`].
//! - [`TransformConfig`]: which minifier passes run.
//! - [`Diagnostic`]: an error or warning with a code, span, and help. Its
//!   numbered constructors, such as [`Diagnostic::e008`], build every coded
//!   one.
//!
//! # Usage
//!
//! ```
//! use luck_core::LuaTarget;
//!
//! assert!(LuaTarget::LuauRoblox.lua_version().is_luau());
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod config;
mod diagnostic;
mod lua_target;
mod path;
mod source_io;

pub use config::{
    BlockNewlineGaps, BuildConfig, CONFIG_FILE_NAME, CallParentheses, Category, CategoryLevels,
    CollapseSimpleStatement, CyclomaticComplexityOptions, DEFAULT_SEARCH_PATHS, DialectKey,
    DynamicRequire, EntryConfig, FormatConfig, HexCase, IndentStyle, LineEndings, LintConfig,
    LuckConfig, ProfileOverrides, ProjectConfig, ProjectFilter, QuoteStyle,
    RestrictedModulePathsOptions, RuleLevel, RuleSetting, RuleSettings, SpaceAfterFunction,
    TransformConfig, check_output_paths, config_directory, discover_config,
    load_or_discover_config, load_with_extends, parse_luck_config, resolve_build_config,
    resolved_format_config,
};
pub use diagnostic::{Diagnostic, DiagnosticSeverity};
pub use lua_target::LuaTarget;
pub use path::{native_path, normalize_path};
pub use source_io::read_source_file;
