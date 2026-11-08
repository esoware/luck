//! Module identity for the dependency graph.

use luck_ast::Block;
use luck_token::Span;

/// Index into the dependency graph's `modules` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub usize);

/// A resolved `require()` edge out of a module.
#[derive(Debug, Clone)]
pub struct Dependency {
    /// The require string with escapes decoded, as real `require` would
    /// receive it.
    pub require_string: String,
    /// The canonical path the require resolved to, which keys the graph.
    pub resolved_path: String,
    /// The `require(...)` call in source. The emitter splices the loader call
    /// over it, and cycle warnings point at it.
    pub call_span: Span,
}

#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub path: String,
    pub source: String,
    pub dependencies: Vec<Dependency>,
    /// Project-relative path, built against the graph's project root.
    ///
    /// It's the only path the bundle may carry, in `-- module` comments and
    /// in the loader data 5.2+ chunks receive.
    pub relative_path: String,
    /// The `require` identifier in each of this module's `require(expr)`
    /// calls, which the emitter retargets at the loader's dynamic entry
    /// point. Always empty on Luau targets.
    pub dynamic_callees: Vec<Span>,
    /// The AST from graph construction, kept so the emitter doesn't reparse.
    pub block: Block,
}
