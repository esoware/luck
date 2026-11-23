use luck_ast::{Call, Chunk, SuffixedExpression, TreeCounts};
use luck_diagnostics::Diagnostic;
use luck_semantic::{NameId, Reference, Scoping, Semantic, SemanticBuilder, SymbolId};
use luck_token::{LuaTarget, LuaVersion};

use crate::resolve::{ChainCallees, ResolvedCallee, ShapeResolver, symbol_shapes};
use crate::stdlib_catalog::library_for;
use crate::stdlib_model::{StdlibEntry, StdlibLibrary};

/// A chunk's [`Semantic`], read against the stdlib of one target.
#[derive(Debug)]
pub struct SemanticAnalysis<'ast> {
    semantic: Semantic<'ast>,
    stdlib: &'static StdlibLibrary,
    /// Each symbol's shape, by symbol index: a never-reassigned local
    /// initialized from a value of known shape, such as
    /// `local f = io.open(...)`.
    symbol_shapes: Vec<Option<&'static str>>,
    /// Each name's standing as a global the host defines, by name index.
    host_globals: Vec<HostGlobal>,
}

#[derive(Debug, Clone, Copy)]
struct HostGlobal {
    is_stdlib: bool,
    is_extra: bool,
}

/// `chunk`'s semantic, read against the stdlib of `target`, the dialect
/// `chunk` was parsed as, and beside it the checker's errors.
///
/// The semantic holds the node table, which the shapes read, and its tables
/// are sized from `counts`, the parse's or the default. For Luau, it's the
/// same whether or not the target is Roblox, which only adds the Roblox
/// runtime's stdlib.
#[must_use]
pub fn analyze<'ast>(
    chunk: &'ast Chunk<'ast>,
    counts: TreeCounts,
    target: LuaTarget,
) -> (SemanticAnalysis<'ast>, Vec<Diagnostic>) {
    debug_assert_eq!(chunk.version, target.version(), "the target is the chunk's dialect");
    let built =
        SemanticBuilder::new().with_counts(counts).with_check(true).with_nodes(true).build(chunk);
    let semantic = built.semantic;
    let stdlib = library_for(target);
    let symbol_shapes = symbol_shapes(&semantic, stdlib);
    let host_globals = semantic
        .scoping()
        .names()
        .map(|(_, name)| HostGlobal {
            is_stdlib: stdlib.globals.contains_key(name),
            is_extra: false,
        })
        .collect();
    (SemanticAnalysis { semantic, stdlib, symbol_shapes, host_globals }, built.diagnostics)
}

impl<'ast> SemanticAnalysis<'ast> {
    #[must_use]
    pub const fn semantic(&self) -> &Semantic<'ast> {
        &self.semantic
    }

    #[must_use]
    pub const fn scoping(&self) -> &Scoping {
        self.semantic.scoping()
    }

    #[must_use]
    pub const fn stdlib(&self) -> &'static StdlibLibrary {
        self.stdlib
    }

    #[must_use]
    pub const fn target(&self) -> LuaTarget {
        self.stdlib.target
    }

    #[must_use]
    pub const fn version(&self) -> LuaVersion {
        self.stdlib.target.version()
    }

    /// Marks `names` as globals the host defines beyond the stdlib, such as
    /// `vim`, from the `extra_globals` config. Only the chunk's own names
    /// are kept, since no other name can reach a query.
    pub fn add_extra_globals<'name, Names>(&mut self, names: Names)
    where
        Names: IntoIterator<Item = &'name str>,
    {
        for name in names {
            if let Some(global) = self
                .semantic
                .scoping()
                .name_id(name)
                .and_then(|id| self.host_globals.get_mut(id.index()))
            {
                global.is_extra = true;
            }
        }
    }

    /// Whether `name` is recognized as a global: a stdlib entry for this
    /// target, an extra global, or a name a `global` statement in this
    /// chunk declares.
    #[must_use]
    pub fn is_known_global(&self, name: &str) -> bool {
        self.scoping().name_id(name).map_or_else(
            || self.stdlib.globals.contains_key(name),
            |id| self.is_known_global_name(id),
        )
    }

    /// [`Self::is_known_global`] for one of the chunk's names.
    #[must_use]
    pub fn is_known_global_name(&self, name: NameId) -> bool {
        self.host_globals
            .get(name.index())
            .is_some_and(|global| global.is_stdlib || global.is_extra)
            || self.scoping().is_declared_global(name)
    }

    /// Whether [`Self::add_extra_globals`] marked `name`.
    #[must_use]
    pub fn is_extra_global(&self, name: NameId) -> bool {
        self.host_globals.get(name.index()).is_some_and(|global| global.is_extra)
    }

    /// Whether a global's name is defined where `reference` uses it: known
    /// to [`Self::is_known_global`], or covered by a Lua 5.5 `global`
    /// statement in scope there, `global *` included.
    #[must_use]
    pub fn is_defined_global(&self, reference: &Reference) -> bool {
        reference.is_declared_global || self.is_known_global_name(reference.name)
    }

    /// Whether the stdlib marks the global `name` read-only, so code
    /// shouldn't reassign it. Namespaces always are.
    #[must_use]
    pub fn is_read_only_global(&self, name: &str) -> bool {
        self.stdlib.globals.get(name).is_some_and(StdlibEntry::is_read_only)
    }

    /// `symbol`'s shape: a never-reassigned local initialized from a value
    /// of known shape, such as `local f = io.open(...)`.
    #[must_use]
    pub fn symbol_shape(&self, symbol: SymbolId) -> Option<&'static str> {
        self.symbol_shapes.get(symbol.index()).copied().flatten()
    }

    /// Returns `None` for shadowed bases, unknown names, values of unknown
    /// shape, and calls whose `.` or `:` doesn't match the function's
    /// `is_method`.
    ///
    /// This reads the callee from its prefix, so for each call of a chain,
    /// [`Self::chain_callees`] costs less.
    #[must_use]
    pub fn resolve_callee<'call>(&'call self, call: Call<'call>) -> Option<ResolvedCallee<'call>> {
        self.shape_resolver().call_callee(call)
    }

    /// Resolves the calls of `chain`, asked for front to back, in one pass
    /// over it.
    #[must_use]
    pub fn chain_callees<'chain>(
        &'chain self,
        chain: &'chain SuffixedExpression<'chain>,
    ) -> ChainCallees<'chain> {
        ChainCallees::new(self.shape_resolver(), chain)
    }

    fn shape_resolver(&self) -> ShapeResolver<'_> {
        ShapeResolver::new(self.scoping(), self.stdlib, &self.symbol_shapes)
    }

    /// The entry `segments` names, where `root` is the reference their first
    /// makes.
    ///
    /// With `local f = io.open(...)`, `["f", "read"]` resolves through the
    /// `file` shape. A root that reaches the host's global is a plain library
    /// lookup. Any other root resolves to nothing unless it's a local of
    /// known shape, which keeps shadowed names out of hover and semantic
    /// tokens.
    #[must_use]
    pub fn resolve_stdlib_path(
        &self,
        segments: &[&str],
        root: &Reference,
    ) -> Option<&'static StdlibEntry> {
        let members = segments.get(1..)?;
        match root.resolved {
            Some(symbol) => {
                let (first, rest) = members.split_first()?;
                let mut current = self.stdlib.shape_member(self.symbol_shape(symbol)?, first)?;
                for segment in rest {
                    current = self.stdlib.child(current, segment)?;
                }
                Some(current)
            }
            None if self.scoping().is_host_global(root) => self.stdlib.lookup(segments),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_ast::Statement;

    use super::*;
    use crate::fixtures::parse;

    fn analyze_source<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        version: LuaVersion,
    ) -> SemanticAnalysis<'ast> {
        analyze(parse(allocator, source, version), TreeCounts::default(), version.into()).0
    }

    #[test]
    fn is_known_global_hits_stdlib() {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, "local x = print", LuaVersion::Lua54);
        assert!(analysis.is_known_global("print"));
        assert!(analysis.is_known_global("table"));
        assert!(!analysis.is_known_global("not_a_real_name"));
        assert!(!analysis.is_known_global("x"));
    }

    fn reaches_host_print(source: &str, version: LuaVersion) -> bool {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, source, version);
        let tree = analysis.scoping();
        let print = tree
            .references()
            .iter()
            .rfind(|reference| tree.name(reference.name) == "print")
            .expect("the source references print");
        tree.is_host_global(print)
    }

    #[test]
    fn reaches_host_global_requires_a_global_the_chunk_leaves_alone() {
        assert!(reaches_host_print("print(1)", LuaVersion::Lua54));
        assert!(!reaches_host_print(
            "\
function print() end
print(1)",
            LuaVersion::Lua54
        ));
    }

    #[test]
    fn reaches_host_global_is_false_for_a_local_or_a_field_of_env() {
        assert!(!reaches_host_print(
            "\
local print = 1
print(1)",
            LuaVersion::Lua54
        ));
        assert!(!reaches_host_print(
            "\
local _ENV = {}
print(1)",
            LuaVersion::Lua54
        ));
        assert!(reaches_host_print(
            "\
local _ENV = {}
print(1)",
            LuaVersion::Lua51
        ));
    }

    #[test]
    fn reaches_host_global_is_false_when_the_chunk_assigns_its_env() {
        assert!(!reaches_host_print(
            "\
_ENV = {}
print(1)",
            LuaVersion::Lua54
        ));
        assert!(reaches_host_print(
            "\
_ENV = {}
print(1)",
            LuaVersion::Lua51
        ));
    }

    #[test]
    fn reaches_host_global_is_true_for_a_declared_global_without_a_value() {
        assert!(reaches_host_print(
            "\
global print
print(1)",
            LuaVersion::Lua55
        ));
        assert!(!reaches_host_print(
            "\
global print = 1
print(1)",
            LuaVersion::Lua55
        ));
    }

    #[test]
    fn is_defined_global_counts_a_global_star_in_scope() {
        let allocator = Allocator::new();
        let analysis = analyze_source(
            &allocator,
            "\
global *
return y",
            LuaVersion::Lua55,
        );
        let y = &analysis.scoping().references()[0];
        assert!(analysis.is_defined_global(y));
        assert!(!analysis.is_known_global("y"), "rename still treats `y` as a free name");
    }

    #[test]
    fn extra_globals_are_known_globals() {
        let allocator = Allocator::new();
        let mut analysis = analyze_source(&allocator, "vim.cmd('x')", LuaVersion::Lua54);
        let vim = analysis.scoping().name_id("vim").expect("the chunk names vim");
        assert!(!analysis.is_known_global("vim"));
        analysis.add_extra_globals(["vim", "absent"]);
        assert!(analysis.is_known_global("vim"));
        assert!(analysis.is_extra_global(vim));
        assert!(analysis.is_defined_global(&analysis.scoping().references()[0]));
    }

    #[test]
    fn roblox_globals_known_only_on_roblox() {
        let allocator = Allocator::new();
        let chunk = parse(&allocator, "print(game, bit32)", LuaVersion::Luau);
        let (standalone, _) = analyze(chunk, TreeCounts::default(), LuaTarget::Luau);
        let (roblox, _) = analyze(chunk, TreeCounts::default(), LuaTarget::LuauRoblox);
        assert!(!standalone.is_known_global("game"));
        assert!(roblox.is_known_global("game"));
        assert!(standalone.is_known_global("bit32"));
        assert!(roblox.is_known_global("bit32"));
    }

    fn resolve_last_statement_call(source: &str, version: LuaVersion) -> Option<String> {
        let allocator = Allocator::new();
        let chunk = parse(&allocator, source, version);
        let (analysis, _) = analyze(chunk, TreeCounts::default(), version.into());
        let Some(Statement::FunctionCall(chain)) = chunk.block.stmts.last() else {
            panic!("the last statement must be a call");
        };
        analysis.resolve_callee(chain.last_call()?).map(|callee| callee.name())
    }

    fn unknown_globals(source: &str) -> Vec<String> {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, source, LuaVersion::Luau);
        let tree = analysis.scoping();
        tree.global_references()
            .filter(|reference| !analysis.is_defined_global(reference))
            .map(|reference| tree.name(reference.name).to_owned())
            .collect()
    }

    #[test]
    fn free_name_in_nested_function_captures_local_env() {
        let allocator = Allocator::new();
        let analysis = analyze_source(
            &allocator,
            "\
local _ENV = {}
local function f() return missing end
return f",
            LuaVersion::Lua54,
        );
        let tree = analysis.scoping();
        let env = tree
            .symbols()
            .iter()
            .find(|symbol| tree.name(symbol.name) == "_ENV")
            .expect("_ENV must be declared");
        assert!(
            env.is_upvalue,
            "a free name inside a nested function reads _ENV as an upvalue of that function"
        );
    }

    #[test]
    fn resolve_callee_bails_under_a_local_env() {
        let source = "\
local _ENV = { print = function() end }
print(1, 2)";
        assert_eq!(
            resolve_last_statement_call(source, LuaVersion::Lua54),
            None,
            "with a local _ENV, print is _ENV.print on the user's table, not the stdlib print"
        );
    }

    #[test]
    fn resolve_callee_bails_on_a_global_the_file_rebinds() {
        let source = "\
function tostring(a, b) return a end
tostring(1, 2)";
        assert_eq!(
            resolve_last_statement_call(source, LuaVersion::Lua54),
            None,
            "the chunk replaced the global tostring, so the call must not match the stdlib entry"
        );
    }

    #[test]
    fn resolve_stdlib_path_reads_a_shaped_local_or_the_host_global() {
        let allocator = Allocator::new();
        let analysis = analyze_source(
            &allocator,
            "\
local f = io.open('x')
f:read()
local table = {}
table.insert(f)",
            LuaVersion::Lua54,
        );
        let tree = analysis.scoping();
        let reference_to = |name: &str| {
            tree.references()
                .iter()
                .find(|reference| tree.name(reference.name) == name)
                .expect("the source references the name")
        };
        assert!(analysis.resolve_stdlib_path(&["f", "read"], reference_to("f")).is_some());
        assert!(analysis.resolve_stdlib_path(&["io", "open"], reference_to("io")).is_some());
        assert!(
            analysis.resolve_stdlib_path(&["table", "insert"], reference_to("table")).is_none()
        );
    }

    #[test]
    fn analysis_reports_the_checker_s_errors() {
        let allocator = Allocator::new();
        let chunk = parse(
            &allocator,
            "\
local x <const> = 1
x = 2",
            LuaVersion::Lua54,
        );
        let (_, diagnostics) = analyze(chunk, TreeCounts::default(), LuaTarget::Lua54);
        let messages: Vec<&str> = diagnostics.iter().map(|diag| &*diag.message).collect();
        assert_eq!(messages, ["attempt to assign to const variable `x`"]);
    }

    #[test]
    fn type_function_body_knows_the_types_library() {
        let unknown = unknown_globals(
            "\
type function f(t)
  return types.number
end",
        );
        assert!(
            unknown.is_empty(),
            "type functions run with the `types` library, so the body has no unknown global: \
             {unknown:?}"
        );
    }

    #[test]
    fn type_function_body_knows_sibling_type_functions() {
        let unknown = unknown_globals(
            "\
type function f(t)
  return t
end
type function g(t)
  return f(t)
end",
        );
        assert!(
            unknown.is_empty(),
            "a type function can call a type function declared before it, as luau-analyze \
             accepts, so the body has no unknown global: {unknown:?}"
        );
    }
}
