//! # `luck_resolver`
//!
//! Maps the string a `require()` call names to the file it loads.
//!
//! Lua 5.x searches path templates. `require("foo.bar")` substitutes `foo/bar`
//! into each template (`?.lua`, `?/init.lua`, and so on) and returns the
//! first candidate that exists, taking relative templates from the project
//! root.
//!
//! Luau resolves relative imports (`./module`, `../module`) from the requiring
//! file's directory, and `@alias` prefixes from the `.luaurc` files found up
//! the directory tree.
//!
//! A [`ResolverGeneric`] reads through a [`FileSystem`], the OS's own in a
//! [`Resolver`], so a caller can resolve against files only it holds. It
//! caches the directory listings it reads, what each Lua require string
//! resolved to, `.luaurc` aliases, and Luau's file probes, which its threads
//! share, until [`ResolverGeneric::clear_cache`]. A require that names no
//! file, or more than one, gets a [`ResolveError`] saying why, which
//! [`ResolveError::to_diagnostic`] turns into the diagnostic luck reports it
//! with, and [`Resolution::to_diagnostic`] gives the warning of a `@self`
//! that shadows a `.luaurc` alias.
//!
//! This is the lowest crate that touches the file system, so the crates above
//! it read Lua sources and canonicalize paths through its [`FileSystemOs`].
//! Paths stay paths inside luck, [`lexically_normalized`] resolves a path's
//! `.` and `..` without reading the disk, and [`to_slash_lossy`] gives the
//! form luck writes one out in.
//!
//! # Usage
//!
//! ```no_run
//! use std::path::{Path, PathBuf};
//!
//! use luck_resolver::{ResolveOptions, Resolver};
//! use luck_token::LuaVersion;
//!
//! let resolver = Resolver::new(ResolveOptions {
//!     version: LuaVersion::Lua54,
//!     search_paths: vec!["?.lua".to_owned()],
//!     root: PathBuf::from("."),
//! });
//! let resolution = resolver.resolve(Path::new("main.lua"), "foo").unwrap();
//! assert!(resolution.path.ends_with("foo.lua"));
//! ```

mod aliases;
mod cache;
mod error;
mod file_system;
#[cfg(test)]
mod fixtures;
mod lua;
mod luau;
mod luaurc;
mod path;
mod resolver;
mod shared_cache;

pub use error::ResolveError;
pub use file_system::{DirEntry, FileMetadata, FileSystem, FileSystemOs};
pub use path::{lexically_normalized, to_slash_lossy};
pub use resolver::{Resolution, ResolveOptions, Resolver, ResolverGeneric};
