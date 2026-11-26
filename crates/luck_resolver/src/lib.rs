//! # `luck_resolver`
//!
//! Maps the string a `require()` call names to the file it loads, as Luau's
//! require by string does.
//!
//! A relative require (`./module`, `../module`) resolves from the requiring
//! file's directory, and an `@alias` prefix from the `.luaurc` and
//! `.config.luau` files found up the directory tree. A `.config.luau` is Luau
//! code, which only Luau can run, so a resolver reads its aliases through
//! the [`LuauConfigAliases`] its caller gives it.
//!
//! A [`ResolverGeneric`] reads through a [`FileSystem`], the OS's own in a
//! [`Resolver`], so a caller can resolve against files only it holds. It
//! caches the directory listings it reads, the aliases of each config, and
//! its file probes, which its threads share, until
//! [`ResolverGeneric::clear_cache`]. A
//! require that names no file, or more than one, gets a [`ResolveError`]
//! saying why, which [`ResolveError::to_diagnostic`] turns into the
//! diagnostic luck reports it with, and [`Resolution::to_diagnostic`] gives
//! the warning of a `@self` that shadows a `.luaurc` alias.
//!
//! This is the lowest crate that touches the file system, so the crates above
//! it read sources and canonicalize paths through its [`FileSystemOs`]. Paths
//! stay paths inside luck, [`lexically_normalized`] resolves a path's `.` and
//! `..` without reading the disk, and [`to_slash_lossy`] gives the form luck
//! writes one out in.
//!
//! # Usage
//!
//! ```no_run
//! use std::path::Path;
//!
//! use luck_resolver::Resolver;
//!
//! let resolver = Resolver::new();
//! let resolution = resolver.resolve(Path::new("main.luau"), "./foo").unwrap();
//! assert!(resolution.path.ends_with("foo.luau"));
//! ```

mod aliases;
mod error;
mod file_system;
#[cfg(test)]
mod fixtures;
mod fs_cache;
mod luaurc;
mod path;
mod resolver;
mod shared_cache;

pub use error::ResolveError;
pub use file_system::{DirEntry, FileMetadata, FileSystem, FileSystemOs};
pub use path::{is_definition_file, lexically_normalized, to_slash_lossy};
pub use resolver::{LuauConfigAliases, Resolution, Resolver, ResolverGeneric};
