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
    /// It's the only path the bundle may carry, in its `-- module` comments.
    pub relative_path: String,
    /// Bytes, as Luau reads them, which the bundle copies as written.
    pub source: &'ast [u8],
    pub block: Block<'ast>,
    /// In source order.
    pub dependencies: Vec<Dependency<'ast>>,
    /// `block`'s depth as its parse counted it, never below what
    /// [`luck_ast::block_nesting_depth`] finds.
    pub(crate) nesting_depth: usize,
    /// This module's leading `--!` hot comments.
    pub(crate) hot_comments: Vec<Span>,
    /// The `#!` first line, which Luau skips only at the start of a chunk.
    pub(crate) shebang: Option<Span>,
    /// Whether the source holds `__luck`, the stem of every prefix the
    /// bundle's generated names can take.
    pub(crate) has_prefix_stem: bool,
    pub(crate) exports: Exports<'ast>,
}

/// Whether `spans`, sorted by start and none starting where another does,
/// holds `span`.
pub(crate) fn contains_span(spans: &[Span], span: Span) -> bool {
    spans
        .binary_search_by_key(&span.start, |candidate| candidate.start)
        .is_ok_and(|i| spans[i] == span)
}
