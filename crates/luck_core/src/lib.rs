//! # `luck_core`
//!
//! Types the luck toolchain shares across crates that don't depend on each
//! other.
//!
//! ## Key types
//!
//! - [`LuaTarget`]: the user-facing dialect (Lua 5.1-5.5, Luau, or Roblox),
//!   which projects onto a syntax version and a stdlib environment.
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

mod diagnostic;
mod lua_target;

pub use diagnostic::{Diagnostic, DiagnosticSeverity};
pub use lua_target::LuaTarget;
