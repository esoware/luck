use luck_ast::Block;
use luck_token::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub usize);

#[derive(Debug, Clone)]
pub struct Dependency {
    /// The require string with escapes decoded, as real `require` would
    /// receive it.
    pub require_string: String,
    pub target: ModuleId,
    /// The `require(...)` call in source. The emitter splices the loader call
    /// over it, and cycle warnings point at it.
    pub call_span: Span,
}

#[derive(Debug, Clone)]
pub struct ModuleInfo {
    /// The canonical path, in the [`normalize_path`](luck_resolver::normalize_path)
    /// form, which keys the graph.
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
    /// The AST from graph construction, kept so the emitter doesn't reparse.
    pub block: Block,
}
