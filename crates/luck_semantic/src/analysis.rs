use luck_ast::{Block, Call, SuffixedExpression};
use luck_token::{LuaVersion, Span, StdlibEnvironment};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::builder;
use crate::resolve::{ChainCallees, ResolvedCallee, ShapeResolver, symbol_shapes};
use crate::scope::{Reference, ScopeTree, Symbol, SymbolId};
use crate::stdlib_catalog::library_for;
use crate::stdlib_model::{StdlibEntry, StdlibLibrary};

#[derive(Debug)]
pub struct SemanticAnalysis {
    pub scope_tree: ScopeTree,
    /// Globals beyond the stdlib, such as `vim`, filled by the linter and the
    /// language server from the `extra_globals` config. Stdlib names aren't
    /// copied in, so use `is_known_global` to check both.
    pub extra_globals: FxHashSet<String>,
    pub version: LuaVersion,
    /// Which stdlib answers queries. Only matters for Luau, where `Roblox`
    /// selects the Roblox runtime and `Standalone` open-source Luau.
    pub environment: StdlibEnvironment,
    /// Shapes of never-reassigned locals initialized from a value of known
    /// shape, such as `local f = io.open(...)`.
    pub symbol_shapes: FxHashMap<SymbolId, &'static str>,
}

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
    let (scope_tree, initialized_locals) = builder::build_with_initializers(block, version);
    let symbol_shapes =
        symbol_shapes(&scope_tree, library_for(version, environment), &initialized_locals);
    SemanticAnalysis {
        scope_tree,
        extra_globals: FxHashSet::default(),
        version,
        environment,
        symbol_shapes,
    }
}

impl SemanticAnalysis {
    #[must_use]
    pub fn stdlib(&self) -> &'static StdlibLibrary {
        library_for(self.version, self.environment)
    }

    /// Code that matches stdlib paths (`table.insert`, `require(...)`,
    /// `error(...)`) must bail when the base name is shadowed
    /// (`local table = {}`), or it reports stdlib diagnostics on user values.
    #[must_use]
    pub fn resolves_to_local(&self, name: &str, span: Span) -> bool {
        self.scope_tree.resolves_to_local(name, span)
    }

    /// See [`ScopeTree::reaches_host_global`].
    #[must_use]
    pub fn reaches_host_global(&self, name: &str, span: Span) -> bool {
        self.scope_tree.reaches_host_global(name, span)
    }

    /// Whether `name` is recognized as a global: a stdlib entry for this
    /// environment, an entry in `extra_globals`, or a name a `global` statement
    /// in this chunk declares.
    #[must_use]
    pub fn is_known_global(&self, name: &str) -> bool {
        self.stdlib().globals.contains_key(name)
            || self.extra_globals.contains(name)
            || self.scope_tree.declared_globals.contains(name)
    }

    /// Whether a global's name is defined where `reference` uses it: known
    /// to [`Self::is_known_global`], or covered by a Lua 5.5 `global`
    /// statement in scope there, `global *` included.
    #[must_use]
    pub fn is_defined_global(&self, reference: &Reference) -> bool {
        reference.is_declared_global || self.is_known_global(&reference.name)
    }

    /// Whether the stdlib marks the global `name` read-only, so code
    /// shouldn't reassign it. Namespaces always are.
    #[must_use]
    pub fn is_read_only_global(&self, name: &str) -> bool {
        self.stdlib().globals.get(name).is_some_and(StdlibEntry::is_read_only)
    }

    /// Returns `None` for shadowed bases, unknown names, values of unknown
    /// shape, and calls whose `.` or `:` doesn't match the function's
    /// `is_method`.
    ///
    /// This reads the callee from its prefix, so for each call of a chain,
    /// [`Self::chain_callees`] costs less.
    #[must_use]
    pub fn resolve_callee<'call>(&'call self, call: Call<'call>) -> Option<ResolvedCallee<'call>> {
        self.shape_resolver().head_callee(call.callee, call.method)
    }

    /// Resolves the calls of `chain` in one pass over it.
    #[must_use]
    pub fn chain_callees<'chain>(
        &'chain self,
        chain: &'chain SuffixedExpression,
    ) -> ChainCallees<'chain> {
        ChainCallees::new(self.shape_resolver(), chain)
    }

    fn shape_resolver(&self) -> ShapeResolver<'_> {
        ShapeResolver::new(&self.scope_tree, self.stdlib(), &self.symbol_shapes)
    }

    /// With `local f = io.open(...)`, `["f", "read"]` resolves through the
    /// `file` shape. A root that reaches the host's global is a plain library
    /// lookup. Any other root resolves to nothing unless it's a local of
    /// known shape, which keeps shadowed names out of hover and semantic
    /// tokens.
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
            None if self.scope_tree.reaches_host_global(first, root_span) => {
                library.lookup(segments)
            }
            None => None,
        }
    }

    /// For completion: the last local named `name` declared before `offset`.
    ///
    /// Half-typed text like `f:` has no parsed reference to resolve by span.
    /// Scope isn't checked, so a local from a block that already closed can
    /// still match.
    #[must_use]
    pub fn nearest_local(&self, name: &str, offset: u32) -> Option<&Symbol> {
        self.scope_tree
            .symbols
            .iter()
            .filter(|symbol| symbol.name == name && symbol.definition_span.end <= offset)
            .max_by_key(|symbol| symbol.definition_span.start)
    }

    #[must_use]
    pub fn shape_of_nearest_local(&self, name: &str, offset: u32) -> Option<&'static str> {
        self.symbol_shapes.get(&self.nearest_local(name, offset)?.id).copied()
    }
}

#[cfg(test)]
mod tests {
    use luck_ast::Statement;

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

    fn reaches_host_print(source: &str, version: LuaVersion) -> bool {
        let parsed = luck_parser::parse(source, version);
        assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
        let analysis = analyze(&parsed.block, version);
        let print = analysis
            .scope_tree
            .references
            .iter()
            .rfind(|reference| reference.name == "print")
            .expect("the source references print");
        analysis.reaches_host_global("print", print.span)
    }

    #[test]
    fn reaches_host_global_requires_a_global_the_chunk_leaves_alone() {
        assert!(reaches_host_print("print(1)", LuaVersion::Lua54));
        assert!(!reaches_host_print("function print() end\nprint(1)", LuaVersion::Lua54));
    }

    #[test]
    fn reaches_host_global_is_false_for_a_local_or_a_field_of_env() {
        assert!(!reaches_host_print("local print = 1\nprint(1)", LuaVersion::Lua54));
        assert!(!reaches_host_print("local _ENV = {}\nprint(1)", LuaVersion::Lua54));
        assert!(reaches_host_print("local _ENV = {}\nprint(1)", LuaVersion::Lua51));
    }

    #[test]
    fn reaches_host_global_is_false_when_the_chunk_assigns_its_env() {
        assert!(!reaches_host_print("_ENV = {}\nprint(1)", LuaVersion::Lua54));
        assert!(reaches_host_print("_ENV = {}\nprint(1)", LuaVersion::Lua51));
    }

    #[test]
    fn reaches_host_global_is_true_for_a_declared_global_without_a_value() {
        assert!(reaches_host_print("global print\nprint(1)", LuaVersion::Lua55));
        assert!(!reaches_host_print("global print = 1\nprint(1)", LuaVersion::Lua55));
    }

    #[test]
    fn is_defined_global_counts_a_global_star_in_scope() {
        let parsed = luck_parser::parse("global *\nreturn y", LuaVersion::Lua55);
        assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
        let analysis = analyze(&parsed.block, LuaVersion::Lua55);
        let y = &analysis.scope_tree.references[0];
        assert!(analysis.is_defined_global(y));
        assert!(!analysis.is_known_global("y"), "rename still treats `y` as a free name");
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

    fn env_symbol(analysis: &SemanticAnalysis) -> &Symbol {
        analysis
            .scope_tree
            .symbols
            .iter()
            .find(|symbol| symbol.name == "_ENV")
            .expect("_ENV must be declared")
    }

    fn resolve_last_statement_call(source: &str, version: LuaVersion) -> Option<String> {
        let parsed = luck_parser::parse(source, version);
        assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
        let analysis = analyze(&parsed.block, version);
        let Some(Statement::FunctionCall(chain)) = parsed.block.stmts.last() else {
            panic!("the last statement must be a call");
        };
        analysis.resolve_callee(chain.last_call()?).map(|callee| callee.name())
    }

    fn unknown_globals(source: &str) -> Vec<String> {
        let parsed = luck_parser::parse(source, LuaVersion::Luau);
        assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
        let analysis = analyze(&parsed.block, LuaVersion::Luau);
        analysis
            .scope_tree
            .unresolved_references()
            .filter(|reference| !analysis.is_known_global(&reference.name))
            .map(|reference| reference.name.to_string())
            .collect()
    }

    #[test]
    fn free_name_in_nested_function_captures_local_env() {
        let analysis =
            analyze_lua54("local _ENV = {}\nlocal function f() return missing end\nreturn f");
        assert!(
            env_symbol(&analysis).is_upvalue,
            "a free name inside a nested function reads _ENV as an upvalue of that function"
        );
    }

    #[test]
    fn resolve_callee_bails_under_a_local_env() {
        let source = "local _ENV = { print = function() end }\nprint(1, 2)";
        assert_eq!(
            resolve_last_statement_call(source, LuaVersion::Lua54),
            None,
            "with a local _ENV, print is _ENV.print on the user's table, not the stdlib print"
        );
    }

    #[test]
    fn resolve_callee_bails_on_a_global_the_file_rebinds() {
        let source = "function tostring(a, b) return a end\ntostring(1, 2)";
        assert_eq!(
            resolve_last_statement_call(source, LuaVersion::Lua54),
            None,
            "the chunk replaced the global tostring, so the call must not match the stdlib entry"
        );
    }

    #[test]
    fn type_function_body_knows_the_types_library() {
        let unknown = unknown_globals("type function f(t)\n  return types.number\nend");
        assert!(
            unknown.is_empty(),
            "type functions run with the `types` library, so the body has no unknown global: \
             {unknown:?}"
        );
    }

    #[test]
    fn type_function_body_knows_sibling_type_functions() {
        let unknown = unknown_globals(
            "type function f(t)\n  return t\nend\ntype function g(t)\n  return f(t)\nend",
        );
        assert!(
            unknown.is_empty(),
            "a type function can call a type function declared before it, as luau-analyze \
             accepts, so the body has no unknown global: {unknown:?}"
        );
    }
}
