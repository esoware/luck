//! Module identity for the dependency graph.

use luck_ast::shared::Block;
use std::ops::Range;

/// Opaque identifier for a module in the dependency graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub usize);

/// A resolved `require()` edge out of a module.
#[derive(Debug, Clone)]
pub struct Dependency {
    /// The literal require string as written in source (`require("foo")` -> `foo`).
    pub require_string: String,
    /// The normalized path the require resolved to; the graph's canonical module key.
    pub resolved_path: String,
    /// Byte range of the `require(...)` call expression in source, for
    /// bundler-side diagnostics (cycle reporting) that render against source.
    pub call_span: Range<usize>,
}

/// Source file metadata: path, content, and discovered dependencies.
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub path: String,
    pub source: String,
    pub dependencies: Vec<Dependency>,
    /// Project-relative path: every path the bundle carries - provenance
    /// comments and the loader data real 5.2+ chunks receive - comes from
    /// here, so an absolute one would leak the build host's layout into
    /// shipped output. Built by `graph::make_relative`.
    pub relative_path: String,
    /// Byte ranges of the `require` identifier in this module's
    /// `require(expr)` calls, which the emitter retargets at the loader's
    /// dynamic entry point. Always empty on Luau targets.
    pub dynamic_callees: Vec<Range<usize>>,
    /// Parsed AST block, cached during graph construction to avoid re-parsing in the emitter.
    pub parsed_block: Option<Block>,
}
