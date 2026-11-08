//! # `luck_resolver`
//!
//! Maps the string argument of a `require()` call to a filesystem path.
//!
//! Lua 5.x searches path templates. `require("foo.bar")` substitutes `foo/bar`
//! into each template (`./?.lua`, `./?/init.lua`, and so on) and returns the
//! first candidate that exists, taking relative templates from the project
//! root.
//!
//! Luau resolves relative imports (`./module`, `../module`) from the requiring
//! file's directory, and `@alias` prefixes from the `.luaurc` files found up
//! the directory tree.
//!
//! A [`Resolver`] caches `.luaurc` aliases and never invalidates them, so
//! create one per build to see alias edits.
//!
//! # Usage
//!
//! ```no_run
//! use std::path::Path;
//!
//! use luck_resolver::{ResolveRequest, Resolver};
//! use luck_token::{LuaVersion, Span};
//!
//! let search_paths = ["./?.lua".to_owned()];
//! let mut resolver = Resolver::new(LuaVersion::Lua54, &search_paths, Path::new("."));
//! let resolved = resolver
//!     .resolve(&ResolveRequest {
//!         module: "foo",
//!         from_file: "main.lua",
//!         span: Span::new(0, 3),
//!     })
//!     .unwrap();
//! assert!(resolved.path.ends_with(".lua"));
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

#[cfg(test)]
mod fixtures;
mod lua;
mod luau;
mod luaurc;
mod resolver;

pub use resolver::{ResolveRequest, ResolvedModule, Resolver};
