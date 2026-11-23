use luck_allocator::Allocator;
use luck_ast::{
    Block, Call, Chunk, Expression, FunctionArgs, FunctionBody, GetSpan as _, Name, Statement,
    Suffix, SuffixedExpression, TreeCounts, Var,
};
use luck_diagnostics::Diagnostic;
use luck_semantic::{NodeSink, ScopeId, ScopeKind, ScopeTree, StdlibEntry, SymbolId};
use luck_token::{LuaVersion, Span};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dynamic_require::DynamicRequire;
use crate::exports::{Exports, has_value_exports};

/// A `require("literal")` call that reaches the global `require`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RequireSite<'ast> {
    /// The require string with escapes decoded, as real `require` would
    /// receive it.
    pub require_string: &'ast str,
    pub call_span: Span,
}

#[derive(Debug)]
pub(crate) struct ExtractedRequires<'ast> {
    /// In source order.
    pub sites: Vec<RequireSite<'ast>>,
    /// The `require` identifier of each call whose argument isn't a string
    /// literal, in source order. The emitter retargets just this token, so a
    /// static require nested in the argument still gets its own rewrite.
    pub dynamic_callees: Vec<Span>,
    pub diags: Vec<Diagnostic>,
    /// The module's own chunk reads `...`.
    pub has_vararg_read: bool,
    /// Lua 5.1: the module reads the global `arg`.
    pub has_global_arg_read: bool,
    /// Lua 5.2+: the module assigns its `_ENV` or calls `module`.
    pub has_env_rebinding: bool,
    /// Lua 5.1: the most locals any one function captures. Zero elsewhere.
    pub max_function_upvalues: usize,
    pub exports: Exports<'ast>,
}

/// What bundling needs from a module's tree, `block`, whose text is `source`.
///
/// A call that doesn't reach the global `require`, through a local binding
/// named `require`, a local `_ENV`, or a global the file replaced, keeps its
/// meaning and gets W005.
///
/// So does a reference to the global `require` that isn't a direct call,
/// since calls through it escape bundling, like `_G.require`. A standard
/// library's name is served from `package.loaded`, so it stays the host's. On
/// Luau, writes to `package.loaded` get E006.
///
/// `dynamic_require` decides whether a `require(expr)` aborts the bundle or
/// stays for the runtime. Each require string goes in `allocator`. `counts`
/// is the parse's.
pub(crate) fn extract_requires<'ast>(
    allocator: &'ast Allocator,
    chunk: &Chunk<'_>,
    counts: TreeCounts,
    source: &'ast [u8],
    dynamic_require: DynamicRequire,
) -> ExtractedRequires<'ast> {
    let (version, block) = (chunk.version, &chunk.block);
    let mut uses = Uses::new(version);
    let tree = if counts_upvalues(version) || has_exports(block, version) {
        ScopeTree::build_with(chunk, counts, &mut uses)
    } else {
        ScopeTree::build_for_names(chunk, &ASKED_NAMES, &mut uses)
    };
    RequireFinder::new(allocator, source, version, dynamic_require, &tree).read(block, uses)
}

/// Every name [`RequireFinder`] asks its tree about, unless it counts
/// captures or plans exports, which take every name. Lua 5.1's `arg` is
/// asked about only where captures are counted.
const ASKED_NAMES: [&str; 4] = ["require", "_G", "package", "module"];

/// Lua 5.1: the only target with an upvalue limit a loader can hit.
const fn counts_upvalues(version: LuaVersion) -> bool {
    !version.has_env_upvalue() && !version.is_luau()
}

fn has_exports(block: &Block<'_>, version: LuaVersion) -> bool {
    version.has_value_exports() && has_value_exports(block)
}

/// The most distinct outer locals any one function captures. A capture counts
/// on every function between the use and the declaring scope, as Lua's
/// compiler counts it.
fn max_function_upvalues(tree: &ScopeTree<'_>) -> usize {
    let mut captures: FxHashSet<(SymbolId, ScopeId)> = FxHashSet::default();
    let mut counts = vec![0usize; tree.scopes().len()];
    for reference in tree.references() {
        let Some(symbol) = reference.resolved else {
            continue;
        };
        let home = tree.scope(tree.symbol(symbol).scope).function;
        let mut function = tree.scope(reference.scope).function;
        while function != home {
            let scope = tree.scope(function);
            if scope.kind == ScopeKind::Function
                && captures.insert((symbol, function))
                && let Some(count) = counts.get_mut(function.index())
            {
                *count = count.saturating_add(1);
            }
            let Some(parent) = scope.parent else {
                break;
            };
            function = tree.scope(parent).function;
        }
    }
    counts.into_iter().max().unwrap_or(0)
}

/// What the scope tree's walk finds that bundling has to look at once the tree
/// can say what each name reaches.
struct Uses<'tree> {
    version: LuaVersion,
    /// The function bodies the walk is in as of the last [`Self::reach`],
    /// innermost last, and whether each declares `...`, binding its own `arg`
    /// on 5.1.
    functions: Vec<(Span, bool)>,
    requires: Vec<RequireUse<'tree>>,
    /// Lua 5.1: each name `arg` outside every vararg function.
    arg_reads: Vec<&'tree Name<'tree>>,
    /// Luau: each write to or into `package.loaded`, by its `package` name.
    package_loaded_writes: Vec<(&'tree Name<'tree>, Span)>,
    /// The module's own chunk reads `...`.
    has_vararg_read: bool,
}

#[derive(Clone, Copy)]
enum RequireUse<'tree> {
    /// A direct call of a name `require`.
    Call(Call<'tree>),
    /// A chain reading `require` from the global table named.
    GlobalTable(&'tree SuffixedExpression<'tree>, &'tree Name<'tree>),
}

impl RequireUse<'_> {
    fn start(&self) -> u32 {
        match self {
            Self::Call(call) => call.span().start,
            Self::GlobalTable(chain, _) => chain.span.start,
        }
    }
}

impl<'tree> Uses<'tree> {
    const fn new(version: LuaVersion) -> Self {
        Self {
            version,
            functions: Vec::new(),
            requires: Vec::new(),
            arg_reads: Vec::new(),
            package_loaded_writes: Vec::new(),
            has_vararg_read: false,
        }
    }

    /// Leaves each function body that doesn't hold `start`, since the walk
    /// is past it.
    fn reach(&mut self, start: u32) {
        while let Some(&(body, _)) = self.functions.last()
            && !(body.start <= start && start < body.end)
        {
            self.functions.pop();
        }
    }

    fn note_target(&mut self, target: &'tree Var<'tree>) {
        match target {
            Var::Name(name) => self.note_arg(name),
            // Lua targets use package.loaded itself as the bundle cache, so
            // writes to it behave as in real Lua. Luau has no package table,
            // and its bundle cache is private.
            Var::Suffixed(_) => {
                if self.version.is_luau()
                    && let Some(package) = package_loaded_root(target)
                {
                    self.package_loaded_writes.push((package, target.span()));
                }
            }
        }
    }

    /// Lua 5.1: a use of `arg`, outside every vararg function.
    fn note_arg(&mut self, name: &'tree Name<'tree>) {
        if self.version.has_implicit_vararg_table() && name.text == "arg" {
            self.reach(name.span.start);
            if !self.functions.iter().any(|&(_, is_vararg)| is_vararg) {
                self.arg_reads.push(name);
            }
        }
    }
}

impl<'tree> NodeSink<'tree> for Uses<'tree> {
    fn function_body(&mut self, body: &'tree FunctionBody<'tree>) {
        self.reach(body.span.start);
        self.functions.push((body.span, body.vararg.is_some()));
    }

    fn stmt(&mut self, stmt: &'tree Statement<'tree>) {
        if let Statement::Assignment(assignment) = stmt {
            for target in &assignment.targets {
                self.note_target(target);
            }
        }
        // Luau
        if let Statement::CompoundAssignment(compound) = stmt {
            self.note_target(&compound.var);
        }
    }

    fn expr(&mut self, expr: &'tree Expression<'tree>) {
        if let Expression::VarArg(span) = expr {
            self.reach(span.start);
            self.has_vararg_read |= self.functions.is_empty();
        }
        if let Expression::Name(name) = expr {
            self.note_arg(name);
        }
    }

    fn chain(&mut self, chain: &'tree SuffixedExpression<'tree>, _is_target: bool) {
        if let Some(table) = global_table_require(chain, self.version) {
            self.requires.push(RequireUse::GlobalTable(chain, table));
        }
        // Only the first call's callee can be a bare name.
        if let Some(call) = Call::new(chain.head(0), chain.suffixes.first())
            && require_callee(call).is_some()
        {
            self.requires.push(RequireUse::Call(call));
        }
    }
}

/// What the module's requires make of it, decided in source order.
struct RequireFinder<'tree, 'ast> {
    allocator: &'ast Allocator,
    source: &'ast [u8],
    version: LuaVersion,
    dynamic_require: DynamicRequire,
    tree: &'tree ScopeTree<'tree>,
    sites: Vec<RequireSite<'ast>>,
    dynamic_callees: Vec<Span>,
    diags: Vec<Diagnostic>,
    /// The call that first required each string.
    first_requires: FxHashMap<&'ast str, Span>,
    direct_callees: Vec<Span>,
}

impl<'tree, 'ast> RequireFinder<'tree, 'ast> {
    fn new(
        allocator: &'ast Allocator,
        source: &'ast [u8],
        version: LuaVersion,
        dynamic_require: DynamicRequire,
        tree: &'tree ScopeTree<'tree>,
    ) -> Self {
        Self {
            allocator,
            source,
            version,
            dynamic_require,
            tree,
            sites: Vec::new(),
            dynamic_callees: Vec::new(),
            diags: Vec::new(),
            first_requires: FxHashMap::default(),
            direct_callees: Vec::new(),
        }
    }

    /// What `uses`, found in `block` by the walk that built the tree, make of
    /// the module.
    fn read(mut self, block: &Block<'_>, mut uses: Uses<'_>) -> ExtractedRequires<'ast> {
        // The walk reaches an assignment's values before its targets, and the
        // diagnostics follow the source.
        uses.requires.sort_by_key(RequireUse::start);
        for require in &uses.requires {
            match *require {
                RequireUse::Call(call) => self.handle_call(call),
                RequireUse::GlobalTable(chain, table) => {
                    self.flag_global_table_require(chain, table);
                }
            }
        }
        let Self {
            source,
            version,
            tree,
            mut sites,
            dynamic_callees,
            mut diags,
            mut direct_callees,
            ..
        } = self;

        if let Some(require) = tree.name_id("require") {
            direct_callees.sort_unstable_by_key(|span| span.start);
            let is_direct_callee = |span: Span| {
                direct_callees
                    .binary_search_by_key(&span.start, |callee| callee.start)
                    .is_ok_and(|i| direct_callees.get(i) == Some(&span))
            };
            diags.extend(
                tree.global_references()
                    .filter(|reference| {
                        reference.name == require
                            && reference.kind.is_read()
                            && !is_direct_callee(reference.span)
                    })
                    .map(|reference| Diagnostic::w005_aliased(reference.span)),
            );
        }

        sites.sort_by_key(|site| site.call_span.start);
        // A local named `package` is no concern of the bundle's.
        diags.extend(
            uses.package_loaded_writes
                .iter()
                .filter(|(package, _)| tree.reaches_host_global(package))
                .map(|(_, target)| Diagnostic::e006(*target)),
        );
        let has_global_arg_read = uses.arg_reads.iter().any(|name| !tree.resolves_to_local(name));

        let has_env_rebinding = version.has_env_upvalue()
            && (tree.is_env_assigned()
                || tree.name_id("module").is_some_and(|module| {
                    tree.global_references().any(|reference| reference.name == module)
                }));
        let max_function_upvalues =
            if counts_upvalues(version) { max_function_upvalues(tree) } else { 0 };
        let exports = if has_exports(block, version) {
            Exports::new(block, tree, source)
        } else {
            Exports::default()
        };

        ExtractedRequires {
            sites,
            dynamic_callees,
            diags,
            has_vararg_read: uses.has_vararg_read,
            has_global_arg_read,
            has_env_rebinding,
            max_function_upvalues,
            exports,
        }
    }

    fn handle_call(&mut self, call: Call<'_>) {
        let Some(callee) = require_callee(call) else {
            return;
        };
        self.direct_callees.push(callee.span);
        if !self.tree.reaches_host_global(callee) {
            self.diags.push(Diagnostic::w005_shadowed(call.span()));
            return;
        }

        match self.literal_require_string(call) {
            // The real `require` finds a standard library in `package.loaded`
            // before any searcher, so no file can stand in for it.
            Some(require_string) if is_standard_library(require_string, self.version) => {}
            Some(require_string) => {
                match self.first_requires.get(require_string) {
                    Some(&first) => {
                        let diag = Diagnostic::w001(call.span(), require_string, first);
                        self.diags.push(diag);
                    }
                    None => {
                        self.first_requires.insert(require_string, call.span());
                    }
                }
                self.sites.push(RequireSite { require_string, call_span: call.span() });
            }
            // `require(expr)` can't be resolved statically. Aborting the whole
            // bundle over one such call is rarely what the author wants, so by
            // default it stays and resolves at runtime.
            None => self.handle_dynamic_call(call, callee.span),
        }
    }

    fn handle_dynamic_call(&mut self, call: Call<'_>, callee_span: Span) {
        match self.dynamic_require {
            DynamicRequire::Error => {
                self.diags.push(Diagnostic::e002(call.span()));
                return;
            }
            DynamicRequire::Warn if self.version.is_luau() => {
                self.diags.push(Diagnostic::w007_runtime(call.span()));
            }
            DynamicRequire::Warn => {
                self.diags.push(Diagnostic::w007_loader_fallback(call.span()));
            }
            DynamicRequire::Allow => {}
        }
        // Luau keys its cache by resolved file, and a Roblox require takes an
        // Instance, so a runtime argument has nothing in the bundle to look
        // up and the call stays as written.
        if !self.version.is_luau() {
            self.dynamic_callees.push(callee_span);
        }
    }

    /// The value of `call`'s argument when it's one string literal, copied
    /// into the arena. `None` if the value isn't UTF-8.
    fn literal_require_string(&self, call: Call<'_>) -> Option<&'ast str> {
        let literal = match call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                let [Expression::StringLiteral(literal)] = &**args else {
                    return None;
                };
                literal
            }
            FunctionArgs::StringLiteral(literal) => literal,
            FunctionArgs::TableConstructor(_) | FunctionArgs::Error(_) => return None,
        };
        Some(self.allocator.alloc_str(literal.as_str()?))
    }

    /// W005 for `_G.require` or `_ENV.require`, the global `require` reached
    /// through its table rather than a direct call.
    fn flag_global_table_require(&mut self, chain: &SuffixedExpression<'_>, table: &Name<'_>) {
        if self.tree.reaches_host_global(table) {
            self.diags.push(Diagnostic::w005_aliased(chain.head(1).span()));
        }
    }
}

/// Whether the real `require` serves `name` straight from `package.loaded`.
/// Luau has no `package` table, so its `require` always resolves a path.
fn is_standard_library(name: &str, version: LuaVersion) -> bool {
    !version.is_luau()
        && (name == "_G"
            || matches!(
                luck_semantic::library_for(version.into()).globals.get(name),
                Some(StdlibEntry::Namespace(_))
            ))
}

/// Whether `suffix` reads the field `key`, as `.key` or `["key"]`, however
/// the string is spelled.
fn is_key(suffix: &Suffix<'_>, key: &str) -> bool {
    match suffix {
        Suffix::Field(name) => name.text == key,
        Suffix::Index { index: Expression::StringLiteral(literal), .. } => {
            literal.value == key.as_bytes()
        }
        Suffix::Index { .. }
        | Suffix::Call(_)
        | Suffix::MethodCall(_)
        | Suffix::TypeInstantiation { .. } => false,
    }
}

/// When `chain` starts by reading the field `require` of the global table
/// (`_G`, or `_ENV` on Lua 5.2+), that table's name, which decides whether the
/// access reaches the host's global table.
fn global_table_require<'chain>(
    chain: &'chain SuffixedExpression<'_>,
    version: LuaVersion,
) -> Option<&'chain Name<'chain>> {
    let Expression::Name(table) = &chain.prefix else {
        return None;
    };
    ((table.text == "_G" || (table.text == "_ENV" && version.has_env_upvalue()))
        && is_key(chain.suffixes.first(), "require"))
    .then_some(table)
}

/// The callee when `call` is a direct, non-method call of a variable named
/// `require`, whatever that name resolves to.
fn require_callee(call: Call<'_>) -> Option<&Name<'_>> {
    if call.method.is_some() {
        return None;
    }
    call.callee.as_name().filter(|name| name.text == "require")
}

/// The `package` name when an assignment target is, or indexes into,
/// `package.loaded` or `package["loaded"]`.
fn package_loaded_root<'target>(target: &'target Var<'_>) -> Option<&'target Name<'target>> {
    let Var::Suffixed(chain) = target else {
        return None;
    };
    let Expression::Name(package) = &chain.prefix else {
        return None;
    };
    (package.text == "package" && is_key(chain.suffixes.first(), "loaded")).then_some(package)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::Path;

    use luck_testgen::{
        collect_fixture_files, generate_full, generate_runtime_safe, is_error_fixture,
    };

    use super::*;

    fn extract_in<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        version: LuaVersion,
        dynamic_require: DynamicRequire,
    ) -> ExtractedRequires<'ast> {
        let parsed = luck_parser::parse(allocator, source.as_bytes(), version);
        assert!(parsed.errors.is_empty(), "parse failed: {:?}", parsed.errors);
        extract_requires(
            allocator,
            &parsed.chunk,
            parsed.counts,
            source.as_bytes(),
            dynamic_require,
        )
    }

    fn extract<'ast>(allocator: &'ast Allocator, source: &'ast str) -> ExtractedRequires<'ast> {
        extract_with(allocator, source, DynamicRequire::default())
    }

    fn extract_with<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        dynamic_require: DynamicRequire,
    ) -> ExtractedRequires<'ast> {
        extract_in(allocator, source, LuaVersion::Lua54, dynamic_require)
    }

    fn extract_luau<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
    ) -> ExtractedRequires<'ast> {
        extract_in(allocator, source, LuaVersion::Luau, DynamicRequire::default())
    }

    fn require_strings<'ast>(result: &ExtractedRequires<'ast>) -> Vec<&'ast str> {
        result.sites.iter().map(|site| site.require_string).collect()
    }

    fn with_code<'result>(
        result: &'result ExtractedRequires<'_>,
        code: &str,
    ) -> Vec<&'result Diagnostic> {
        result.diags.iter().filter(|diag| diag.code == code).collect()
    }

    #[test]
    fn extracts_single_require() {
        let allocator = Allocator::new();
        let result = extract(
            &allocator,
            "\
local utils = require(\"utils\")
print(utils.foo())
",
        );
        assert_eq!(require_strings(&result), ["utils"]);
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn extracts_multiple_requires_in_order() {
        let allocator = Allocator::new();
        let result = extract(
            &allocator,
            "\
local a = require(\"mod_a\")
local b = require(\"mod_b\")
local c = require(\"mod_c\")
print(a, b, c)
",
        );
        assert_eq!(require_strings(&result), ["mod_a", "mod_b", "mod_c"]);
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn require_after_code_is_not_flagged() {
        let allocator = Allocator::new();
        // Requires are position-independent with the lazy loader.
        let result = extract(
            &allocator,
            "\
print(\"hello\")
local x = require(\"x\")
",
        );
        assert_eq!(result.sites.len(), 1);
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn non_literal_require_flags_e002_in_error_mode() {
        let allocator = Allocator::new();
        let result =
            extract_with(&allocator, "local x = require(varname)\n", DynamicRequire::Error);
        assert!(result.sites.is_empty());
        assert_eq!(result.dynamic_callees, Vec::<Span>::new());
        assert_eq!(with_code(&result, "E002").len(), 1, "{:?}", result.diags);
    }

    /// The default: one runtime-resolved require must not abort the bundle.
    #[test]
    fn non_literal_require_warns_and_is_kept_by_default() {
        let allocator = Allocator::new();
        let result = extract(&allocator, "local x = require(varname)\n");
        assert!(result.sites.is_empty());
        assert!(with_code(&result, "E002").is_empty(), "{:?}", result.diags);
        assert_eq!(with_code(&result, "W007").len(), 1, "{:?}", result.diags);
        // The callee token alone, so the argument expression is left intact.
        assert_eq!(result.dynamic_callees, [Span::new(10, 17)]);
    }

    #[test]
    fn allow_mode_keeps_the_call_without_warning() {
        let allocator = Allocator::new();
        let result =
            extract_with(&allocator, "local x = require(varname)\n", DynamicRequire::Allow);
        assert!(result.diags.is_empty(), "{:?}", result.diags);
        assert_eq!(result.dynamic_callees, [Span::new(10, 17)]);
    }

    /// Luau keys its cache by resolved file and Roblox requires take an
    /// Instance, so a runtime argument has nothing to look up. The call warns
    /// but is never retargeted.
    #[test]
    fn luau_dynamic_require_is_warned_but_not_retargeted() {
        let allocator = Allocator::new();
        let result = extract_luau(&allocator, "local x = require(varname)\n");
        assert_eq!(with_code(&result, "W007").len(), 1, "{:?}", result.diags);
        assert_eq!(result.dynamic_callees, Vec::<Span>::new());
    }

    /// A static require nested inside a dynamic one still bundles: the
    /// retargeted span is the outer callee only.
    #[test]
    fn static_require_nested_in_a_dynamic_one_is_still_extracted() {
        let allocator = Allocator::new();
        let result = extract(&allocator, "local x = require(require(\"names\").first)\n");
        assert_eq!(require_strings(&result), ["names"]);
        assert_eq!(result.dynamic_callees, [Span::new(10, 17)]);
    }

    #[test]
    fn bare_require_stmt_is_extracted() {
        let allocator = Allocator::new();
        let result = extract(&allocator, "require(\"something\")\n");
        assert_eq!(require_strings(&result), ["something"]);
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn escape_sequences_decode_like_real_lua() {
        let allocator = Allocator::new();
        // require("a\46b") is require("a.b") at runtime; the bundler must
        // resolve and deduplicate on the decoded value.
        let result = extract(
            &allocator,
            "\
local a = require(\"a\\46b\")
local b = require(\"a.b\")
",
        );
        assert_eq!(require_strings(&result), ["a.b", "a.b"]);
        // Same decoded module twice: the duplicate-require warning fires.
        assert_eq!(with_code(&result, "W001").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn long_string_leading_newline_is_stripped() {
        let allocator = Allocator::new();
        let result = extract(&allocator, "local m = require [[\nmymod]]\n");
        assert_eq!(require_strings(&result), ["mymod"]);
    }

    #[test]
    fn shadowed_require_is_skipped_with_w005() {
        let allocator = Allocator::new();
        let source = "\
local require = function(s) return s end
local x = require(\"dep\")
";
        let result = extract(&allocator, source);
        assert!(
            result.sites.is_empty(),
            "shadowed require must not be bundled: {:?}",
            result.sites
        );
        assert_eq!(with_code(&result, "W005").len(), 1, "{:?}", result.diags);
        assert!(with_code(&result, "E002").is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn shadowed_require_in_inner_scope_only_skips_there() {
        let allocator = Allocator::new();
        let source = "\
local a = require(\"real\")
local function f()
    local require = print
    require(\"fake\")
end
";
        let result = extract(&allocator, source);
        assert_eq!(require_strings(&result), ["real"]);
        assert_eq!(with_code(&result, "W005").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn aliased_require_flags_w005() {
        let allocator = Allocator::new();
        let result = extract(
            &allocator,
            "\
local r = require
local x = r(\"dep\")
",
        );
        assert!(result.sites.is_empty());
        assert_eq!(with_code(&result, "W005").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn direct_calls_do_not_flag_w005() {
        let allocator = Allocator::new();
        let result = extract(
            &allocator,
            "\
local a = require(\"x\")
require(\"y\")
",
        );
        assert_eq!(result.sites.len(), 2);
        assert!(with_code(&result, "W005").is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn package_loaded_write_is_allowed_on_lua_targets() {
        let allocator = Allocator::new();
        // On Lua targets the bundle cache is package.loaded itself, so
        // writes to it behave as in real Lua and need no E006.
        let result = extract(
            &allocator,
            "\
package.loaded[\"mymod\"] = {}
package.loaded.other = {}
",
        );
        assert!(with_code(&result, "E006").is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn package_loaded_index_write_flags_e006_on_luau() {
        let allocator = Allocator::new();
        let result = extract_luau(&allocator, "package.loaded[\"mymod\"] = {}\n");
        assert_eq!(with_code(&result, "E006").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn package_loaded_field_write_flags_e006_on_luau() {
        let allocator = Allocator::new();
        let result = extract_luau(&allocator, "package.loaded.mymod = {}\n");
        assert_eq!(with_code(&result, "E006").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn package_loaded_compound_assignment_flags_e006_on_luau() {
        let allocator = Allocator::new();
        let result = extract_luau(&allocator, "package.loaded.x ..= \"a\"\n");
        assert_eq!(with_code(&result, "E006").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn nested_package_loaded_compound_assignment_flags_e006_on_luau() {
        let allocator = Allocator::new();
        let result = extract_luau(&allocator, "package.loaded[\"x\"].y += 1\n");
        assert_eq!(with_code(&result, "E006").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn compound_assignment_to_another_table_is_not_flagged_on_luau() {
        let allocator = Allocator::new();
        let result = extract_luau(&allocator, "loaded.x ..= \"a\"\n");
        assert!(with_code(&result, "E006").is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn local_named_package_is_not_flagged_on_luau() {
        let allocator = Allocator::new();
        let result = extract_luau(
            &allocator,
            "\
local package = { loaded = {} }
package.loaded.x = 1
package.loaded.y ..= \"a\"
",
        );
        assert!(with_code(&result, "E006").is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn package_write_outside_a_local_named_package_is_flagged_on_luau() {
        let allocator = Allocator::new();
        let result = extract_luau(
            &allocator,
            "\
do local package = {} end
package.loaded.x = 1
",
        );
        assert_eq!(with_code(&result, "E006").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn duplicate_require_flags_w001_and_points_at_the_first() {
        let allocator = Allocator::new();
        let source = "\
local a = require(\"utils\")
local b = require(\"utils\")
";
        let result = extract(&allocator, source);
        assert_eq!(result.sites.len(), 2);
        let [warning] = with_code(&result, "W001")[..] else {
            panic!("{:?}", result.diags);
        };
        let labeled: Vec<&[u8]> =
            warning.labels.iter().map(|label| label.span.text(source.as_bytes())).collect();
        assert_eq!(labeled, [b"require(\"utils\")"], "{warning:?}");
    }

    #[test]
    fn top_level_vararg_is_not_flagged() {
        let allocator = Allocator::new();
        // The loader passes each module its real name, so the
        // `local modname = ...` idiom works and needs no warning.
        let result = extract(
            &allocator,
            "\
local modname = ...
return modname
",
        );
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn lua51_global_arg_read_ignores_a_vararg_functions_own_arg() {
        let extract_arg_read = |source: &'static str| {
            extract_in(&Allocator::new(), source, LuaVersion::Lua51, DynamicRequire::default())
                .has_global_arg_read
        };
        assert!(extract_arg_read("return arg\n"));
        assert!(extract_arg_read(
            "\
local function f() return arg end
return f
"
        ));
        assert!(!extract_arg_read(
            "\
local function f(...) return arg end
return f
"
        ));
        assert!(!extract_arg_read(
            "\
local function f(...) return function() return arg end end
return f
"
        ));
        assert!(!extract_arg_read(
            "\
local arg = {}
return arg
"
        ));
    }

    #[test]
    fn no_requires_yields_empty_result() {
        let allocator = Allocator::new();
        let result = extract(&allocator, "print(\"hello world\")\n");
        assert!(result.sites.is_empty());
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn require_string_call_syntax_is_extracted() {
        let allocator = Allocator::new();
        let result = extract(&allocator, "local m = require \"mymod\"\n");
        assert_eq!(require_strings(&result), ["mymod"]);
    }

    #[test]
    fn single_quoted_require_is_extracted() {
        let allocator = Allocator::new();
        let result = extract(&allocator, "local m = require('mymod')\n");
        assert_eq!(require_strings(&result), ["mymod"]);
    }

    #[test]
    fn long_bracket_require_is_extracted() {
        let allocator = Allocator::new();
        let result = extract(&allocator, "local m = require [[mymod]]\n");
        assert_eq!(require_strings(&result), ["mymod"]);
    }

    #[test]
    fn require_in_multi_name_local_is_extracted() {
        let allocator = Allocator::new();
        let result = extract(&allocator, "local a, b = require(\"x\"), 1\n");
        assert_eq!(require_strings(&result), ["x"]);
    }

    #[test]
    fn require_in_nested_function_is_extracted() {
        let allocator = Allocator::new();
        let result = extract(
            &allocator,
            "\
local function setup()
    local m = require(\"inner\")
end
",
        );
        assert_eq!(require_strings(&result), ["inner"]);
    }

    #[test]
    fn field_function_named_require_is_ignored() {
        let allocator = Allocator::new();
        let result = extract(
            &allocator,
            "\
local obj = {}
function obj.require(s) end
obj.require(\"x\")
",
        );
        assert!(result.sites.is_empty());
        assert!(with_code(&result, "W005").is_empty(), "{:?}", result.diags);
    }

    /// Extraction from a tree of every name, for `source` when it parses as
    /// `version`.
    fn extract_from_every_name<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        version: LuaVersion,
    ) -> Option<ExtractedRequires<'ast>> {
        let parsed = luck_parser::parse(allocator, source.as_bytes(), version);
        if !parsed.errors.is_empty() {
            return None;
        }
        let chunk = allocator.alloc(parsed.chunk);
        let mut uses = Uses::new(version);
        let tree = ScopeTree::build_with(chunk, parsed.counts, &mut uses);
        let finder =
            RequireFinder::new(allocator, source.as_bytes(), version, DynamicRequire::Warn, &tree);
        Some(finder.read(&chunk.block, uses))
    }

    fn assert_asked_names_answer_alone(source: &str, version: LuaVersion) {
        let allocator = Allocator::new();
        let Some(every_name) = extract_from_every_name(&allocator, source, version) else {
            return;
        };
        let asked_names = extract_in(&allocator, source, version, DynamicRequire::Warn);
        assert_eq!(format!("{asked_names:?}"), format!("{every_name:?}"), "{version:?}\n{source}");
    }

    #[test]
    fn asked_names_answer_as_every_name_does() {
        let sources = [
            "\
local r = require
require(\"a\")
_G.require(\"b\")
_ENV.require(\"c\")
_G[\"require\"](\"d\")
package.loaded.x = 1
package.loaded[\"y\"] = require
do
    local require = print
    require(\"e\")
end
local _ENV = { require = print }
require(\"f\")
module(\"m\")
return function(...) return ..., arg, r end
",
            "\
require(\"a\")
require = nil
",
            "\
_ENV = {}
require(\"a\")
",
            "\
module(\"m\", package.seeall)
return require(\"a\")
",
            "\
local _G = {}
_G.require(\"a\")
local package = {}
package.loaded.x = 1
local function module() end
module()
",
            "\
global require
global function module() end
require(\"a\")
do
    global *
    local _G <const> = {}
    _G.require(\"b\")
end
",
            "\
type require = number
type function f(t)
    return require(types)
end
require(\"./a\")
package.loaded.x ..= \"y\"
export type T = number
",
            "\
export local a = require(\"./a\")
export function f() a = 1 end
local a = 2
",
        ];
        for source in sources {
            for version in LuaVersion::ALL {
                assert_asked_names_answer_alone(source, version);
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        for path in collect_fixture_files(&root).expect("the fixture tree reads") {
            let relative = path.strip_prefix(&root).expect("a fixture is under the root");
            let is_source =
                matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau"));
            if !is_source || is_error_fixture(relative) {
                continue;
            }
            let source = fs::read_to_string(&path).expect("a fixture reads");
            for version in LuaVersion::ALL {
                assert_asked_names_answer_alone(&source, version);
            }
        }
        for version in LuaVersion::ALL {
            for seed in 0..40 {
                assert_asked_names_answer_alone(&generate_full(seed, version, 40), version);
                assert_asked_names_answer_alone(&generate_runtime_safe(seed, version, 40), version);
            }
        }
    }
}
