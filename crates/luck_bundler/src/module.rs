use std::path::PathBuf;

use luck_ast::Block;
use luck_token::Span;

use crate::exports::Exports;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub usize);

#[derive(Debug, Clone, Copy)]
pub struct Dependency<'ast> {
    /// The require string with escapes decoded, as real `require` would
    /// receive it.
    pub require_string: &'ast str,
    pub target: ModuleId,
    /// The `require(...)` call in source. The emitter splices the loader call
    /// over it, and cycle warnings point at it.
    pub call_span: Span,
}

/// One module as discovery found it. Its source and tree live in one of the
/// arenas the graph was built in.
#[derive(Debug)]
pub struct ModuleInfo<'ast> {
    /// The canonical path, from
    /// [`FileSystem::canonicalize`](luck_resolver::FileSystem::canonicalize),
    /// which keys the graph.
    pub path: PathBuf,
    /// Project-relative path, built against the graph's project root.
    ///
    /// It's the only path the bundle may carry, in `-- module` comments and
    /// in the loader data 5.2+ chunks receive.
    pub relative_path: String,
    /// Bytes, as Lua reads them, which the bundle copies as written.
    pub source: &'ast [u8],
    pub block: Block<'ast>,
    /// `block`'s depth as its parse counted it, never below what
    /// [`luck_ast::block_nesting_depth`] finds.
    pub nesting_depth: usize,
    /// In source order.
    pub dependencies: Vec<Dependency<'ast>>,
    /// The callee, `require` or `_G.require`, of each of this module's calls
    /// of the global `require` whose argument isn't a string literal, which
    /// the emitter retargets at the loader's dynamic entry point, in source
    /// order. Always empty on Luau targets.
    pub dynamic_callees: Vec<Span>,
    /// This module's leading `--!` hot comments. Empty except on Luau targets.
    pub hot_comments: Vec<Span>,
    /// The `#` first line, which Lua and Luau skip only at the start of a
    /// chunk.
    pub shebang: Option<Span>,
    /// The module's own chunk reads `...`.
    pub has_vararg_read: bool,
    /// Lua 5.1: the module reads the global `arg`, which a vararg wrapper's
    /// hidden local shadows.
    pub has_global_arg_read: bool,
    /// Lua 5.2+: the module can rebind `_ENV`, by assigning it or calling
    /// `module`, so its wrapper keeps a private `_ENV`.
    pub has_env_rebinding: bool,
    /// Lua 5.1: the most locals any one of the module's functions captures,
    /// which decides whether the loader fits as an upvalue. Zero elsewhere.
    pub max_function_upvalues: usize,
    /// Whether the source holds `__luck`, the stem of every prefix the
    /// bundle's generated names can take.
    pub has_prefix_stem: bool,
    pub exports: Exports<'ast>,
}
