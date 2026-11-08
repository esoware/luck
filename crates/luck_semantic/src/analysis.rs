use luck_ast::{Block, Expression, FunctionCall, Var};
use luck_token::{CompactString, LuaVersion, Span, StdlibEnvironment};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::builder;
use crate::resolve::{ShapeResolver, symbol_shapes};
use crate::scope::{ScopeTree, SymbolId};
use crate::stdlib_load::library_for;
use crate::stdlib_model::{StdlibEntry, StdlibLibrary};

/// Semantic analysis result for one Lua chunk.
#[derive(Debug)]
pub struct SemanticAnalysis {
    pub scope_tree: ScopeTree,
    /// Globals beyond the stdlib, such as `vim`, filled by the linter from
    /// the `extra_globals` config. Stdlib names aren't copied in, so use
    /// `is_known_global` to check both.
    pub extra_globals: FxHashSet<String>,
    pub version: LuaVersion,
    /// Which stdlib answers queries. Only matters for Luau, where `Roblox`
    /// selects the Roblox runtime and `Standalone` open-source Luau.
    pub environment: StdlibEnvironment,
    /// Shapes of never-reassigned locals initialized from a value of known
    /// shape, such as `local f = io.open(...)`.
    pub symbol_shapes: FxHashMap<SymbolId, CompactString>,
}

/// Analyze a parsed Lua block, building the scope tree.
///
/// Uses the [`StdlibEnvironment::Standalone`] stdlib, which is right for
/// plain Lua and standalone Luau. Roblox callers need
/// [`analyze_with_environment`] with [`StdlibEnvironment::Roblox`].
#[must_use]
pub fn analyze(block: &Block, version: LuaVersion) -> SemanticAnalysis {
    analyze_with_environment(block, version, StdlibEnvironment::Standalone)
}

/// Like [`analyze`], with an explicit stdlib environment. The scope tree is
/// the same either way.
#[must_use]
pub fn analyze_with_environment(
    block: &Block,
    version: LuaVersion,
    environment: StdlibEnvironment,
) -> SemanticAnalysis {
    let scope_tree = builder::build(block);
    let symbol_shapes = symbol_shapes(&scope_tree, library_for(version, environment), block);
    SemanticAnalysis {
        scope_tree,
        extra_globals: FxHashSet::default(),
        version,
        environment,
        symbol_shapes,
    }
}

/// A stdlib entry resolved from a call's callee.
#[derive(Debug, Clone)]
pub struct ResolvedCallee<'library> {
    /// Display path, such as `table.insert` or `game:GetService`.
    pub name: String,
    pub entry: &'library StdlibEntry,
}

impl SemanticAnalysis {
    /// The typed library for this analysis's version and environment.
    #[must_use]
    pub fn stdlib(&self) -> &'static StdlibLibrary {
        library_for(self.version, self.environment)
    }

    /// True when the identifier `name` at `span` resolves to a local rather
    /// than a global.
    ///
    /// Code that matches stdlib paths (`table.insert`, `require(...)`,
    /// `error(...)`) must bail when the base name is shadowed
    /// (`local table = {}`), or it reports stdlib diagnostics on user values.
    #[must_use]
    pub fn resolves_to_local(&self, name: &str, span: Span) -> bool {
        self.scope_tree.resolved_symbol(name, span).is_some()
    }

    /// Whether `name` is recognized as a global, either a stdlib entry for
    /// this environment or an entry in `extra_globals`.
    #[must_use]
    pub fn is_known_global(&self, name: &str) -> bool {
        self.stdlib().globals.contains_key(name) || self.extra_globals.contains(name)
    }

    /// Whether the stdlib marks the global `name` read-only, so code
    /// shouldn't reassign it. Namespaces always are.
    #[must_use]
    pub fn is_read_only_global(&self, name: &str) -> bool {
        self.stdlib().globals.get(name).is_some_and(StdlibEntry::is_read_only)
    }

    /// Resolve the callee of `call` to a stdlib entry and its display path.
    ///
    /// Returns `None` for shadowed bases, unknown names, values of unknown
    /// shape, and calls whose `.` or `:` doesn't match the function's
    /// `is_method`.
    #[must_use]
    pub fn resolve_callee(&self, call: &FunctionCall) -> Option<ResolvedCallee<'_>> {
        let resolver = ShapeResolver::new(&self.scope_tree, self.stdlib(), &self.symbol_shapes);
        if let Some(method_token) = &call.method {
            let method_name = method_token.identifier()?;
            let (shape, entry) = resolver.method_callee(&call.callee, method_name)?;
            let base = if let Expression::Var(Var::Name(token)) = &call.callee {
                token.identifier().unwrap_or(shape)
            } else {
                shape.as_str()
            };
            return Some(ResolvedCallee { name: format!("{base}:{method_name}"), entry });
        }
        let segments = resolver.global_path(&call.callee)?;
        let entry = self.stdlib().lookup(&segments)?;
        // A method called with `.` passes self explicitly, which its declared
        // arity doesn't count.
        if let StdlibEntry::Function(function) = entry
            && function.is_method
        {
            return None;
        }
        Some(ResolvedCallee { name: segments.join("."), entry })
    }

    /// Resolve a dotted or colon path from its segments, where the root may
    /// be a shaped local.
    ///
    /// With `local f = io.open(...)`, `["f", "read"]` resolves through the
    /// `file` shape. A non-local root is a plain library lookup, and a local
    /// root without a known shape resolves to nothing, which keeps shadowed
    /// names out of hover and semantic tokens.
    #[must_use]
    pub fn resolve_stdlib_path(
        &self,
        segments: &[&str],
        root_span: Span,
    ) -> Option<&'static StdlibEntry> {
        let library = self.stdlib();
        let first = segments.first()?;
        match self.scope_tree.resolved_symbol(first, root_span) {
            Some(symbol_id) => {
                let shape = self.symbol_shapes.get(&symbol_id)?;
                let mut current = library.shape_member(shape, segments.get(1)?)?;
                for segment in segments.iter().skip(2) {
                    current = library.child(current, segment)?;
                }
                Some(current)
            }
            None => library.lookup(segments),
        }
    }

    /// For completion: the shape of the last local named `name` declared
    /// before `offset`.
    ///
    /// Half-typed text like `f:` has no parsed reference to resolve by span.
    /// Scope isn't checked, so a local from a block that already closed can
    /// still match.
    #[must_use]
    pub fn shape_of_nearest_local(&self, name: &str, offset: u32) -> Option<&CompactString> {
        let symbol = self
            .scope_tree
            .symbols
            .iter()
            .filter(|symbol| symbol.name == name && symbol.definition_span.end <= offset)
            .max_by_key(|symbol| symbol.definition_span.start)?;
        self.symbol_shapes.get(&symbol.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze_lua54(source: &str) -> SemanticAnalysis {
        let parsed = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
        analyze(&parsed.block, LuaVersion::Lua54)
    }

    #[test]
    fn is_known_global_hits_stdlib() {
        let analysis = analyze_lua54("local x = 1");
        assert!(analysis.is_known_global("print"));
        assert!(analysis.is_known_global("table"));
        assert!(!analysis.is_known_global("not_a_real_name"));
    }

    #[test]
    fn is_known_global_honors_extra_globals() {
        let mut analysis = analyze_lua54("local x = 1");
        assert!(!analysis.is_known_global("vim"));
        analysis.extra_globals.insert("vim".to_owned());
        assert!(analysis.is_known_global("vim"));
    }

    #[test]
    fn roblox_globals_known_only_in_roblox_environment() {
        let parsed = luck_parser::parse("print(game)", LuaVersion::Luau);
        let standalone = analyze_with_environment(
            &parsed.block,
            LuaVersion::Luau,
            StdlibEnvironment::Standalone,
        );
        let roblox =
            analyze_with_environment(&parsed.block, LuaVersion::Luau, StdlibEnvironment::Roblox);
        assert!(!standalone.is_known_global("game"));
        assert!(roblox.is_known_global("game"));
        assert!(standalone.is_known_global("bit32"));
        assert!(roblox.is_known_global("bit32"));
    }
}
