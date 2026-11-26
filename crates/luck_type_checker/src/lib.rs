//! # `luck_type_checker`
//!
//! Luau's own type checker: the new solver of the Analysis library in
//! luau-lang/luau, built from the release the `luau` submodule pins, with the
//! flags `luau-analyze` turns on by default.
//!
//! A [`Checker`] reads everything through a [`Host`] the caller implements:
//! each module's source by the name the host gives it, the module each
//! `require` string names, and the `.luaurc` files that make up a module's
//! config, which Luau's own parser reads. So it works with nothing on disk. It
//! starts with Luau's built-in definitions, which [`builtin_definitions`]
//! gives as text, loads more from text with
//! [`Checker::load_definitions`], types what a definition file can't when
//! asked, with [`Checker::return_named_type`] and
//! [`Checker::bind_type_namespace`], and keeps each module it checks until
//! [`Checker::invalidate`] marks it changed. [`Checker::check`] checks many
//! modules on worker threads it starts and joins, through Analysis's own
//! queue, and a [`CancellationToken`] stops it from another thread. Each
//! error is a `luck_diagnostics::Diagnostic` in Luau's own words, with a span
//! in the module's bytes.
//!
//! Luau's C++ includes its VM and compiler, which run type functions, so this
//! crate can't share a binary with another build of Luau, such as `mlua`'s.
//!
//! # Usage
//!
//! ```
//! use std::num::NonZero;
//!
//! use luck_type_checker::{CancellationToken, Checker, Config, Host};
//!
//! struct OneModule;
//!
//! impl Host for OneModule {
//!     fn source(&self, module: &str) -> Option<Vec<u8>> {
//!         (module == "main").then(|| b"--!strict\nlocal x: number = \"one\"\n".to_vec())
//!     }
//!
//!     fn resolve(&self, _: &str, _: &str) -> Option<String> {
//!         None
//!     }
//!
//!     fn config(&self, _: &str, _: &mut Config<'_>) {}
//! }
//!
//! let mut checker = Checker::new(OneModule);
//! let diags = checker.check(&["main"], NonZero::<usize>::MIN, &CancellationToken::new()).unwrap();
//! assert_eq!(diags[0][0].code, "E015");
//! ```

mod checker;
mod ffi;

pub use checker::{CheckError, Checker, Host};
pub use ffi::{CancellationToken, Config, builtin_definitions};
