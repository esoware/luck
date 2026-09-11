//! # luck_core
//!
//! Shared types, configuration parsing, and diagnostics for the luck toolchain.
//!
//! ## Key types
//!
//! - [`LuaTarget`]: build target (Lua51-Lua55, Luau), maps to parser version + bundler behavior
//! - [`TransformConfig`]: flags controlling which minification passes are enabled
//! - [`Diagnostic`](diagnostics::Diagnostic): rich error/warning type with source spans
//!
//! # Usage
//!
//! ```
//! use luck_core::LuaTarget;
//!
//! assert!(LuaTarget::LuauRoblox.lua_version().is_luau());
//! ```

pub mod config;
pub mod diagnostics;
pub mod editorconfig;
pub mod format_options;
pub mod source_io;
pub mod transform_config;
pub mod types;

pub use config::{LintConfig, RuleSetting};
pub use diagnostics::{Category, DiagnosticSeverity};
pub use format_options::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, HexCase, IndentStyle, LineEndings,
    QuoteStyle, SpaceAfterFunction,
};
pub use transform_config::TransformConfig;
pub use types::{DynamicRequire, LuaTarget};
