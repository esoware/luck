use luck_allocator::Allocator;
use luck_ast::{
    Block, Call, Expression, FunctionArgs, FunctionBody, Name, Statement, Suffix,
    SuffixedExpression, TreeCounts, Var,
};
use luck_core::Diagnostic;
use luck_semantic::{NodeSink, ScopeId, ScopeKind, ScopeTree, StdlibEntry, SymbolId};
use luck_token::{LuaVersion, Span, StdlibEnvironment, decode_string};
use rustc_hash::FxHashSet;

use crate::dynamic_require::DynamicRequire;
use crate::exports::Exports;

/// A `require("literal")` call that reaches the global `require`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RequireSite<'ast> {
    /// The require string with escapes decoded, as real `require` would
    /// receive it.
    pub require_string: &'ast str,
    pub call_span: Span,
}

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
/// stays for the runtime. Each require string goes in `allocator`.
pub(crate) fn extract_requires<'ast>(
    allocator: &'ast Allocator,
    block: &Block<'_>,
    source: &'ast str,
    file_path: &str,
    version: LuaVersion,
    dynamic_require: DynamicRequire,
) -> ExtractedRequires<'ast> {
    let mut uses = Uses {
        version,
        functions: Vec::new(),
        requires: Vec::new(),
        arg_reads: Vec::new(),
        package_loaded_writes: Vec::new(),
        has_vararg_read: false,
    };
    let tree = ScopeTree::build_with(block, TreeCounts::default(), version, &mut uses);
    // The walk reaches an assignment's values before its targets, and the
    // diagnostics follow the source.
    uses.requires.sort_by_key(RequireUse::start);
    let mut finder = RequireFinder {
        allocator,
        source,
        file_path,
        version,
        dynamic_require,
        tree: &tree,
        sites: Vec::new(),
        dynamic_callees: Vec::new(),
        diags: Vec::new(),
        seen_require_strings: FxHashSet::default(),
        direct_callees: Vec::new(),
    };
    for require in &uses.requires {
        match *require {
            RequireUse::Call(call) => finder.handle_call(call),
            RequireUse::GlobalTable(chain, table) => finder.flag_global_table_require(chain, table),
        }
    }
    let RequireFinder { mut sites, dynamic_callees, mut diags, mut direct_callees, .. } = finder;

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
                .map(|reference| Diagnostic::w005_aliased(file_path, reference.span)),
        );
    }

    sites.sort_by_key(|site| site.call_span.start);
    // A local named `package` is no concern of the bundle's.
    diags.extend(
        uses.package_loaded_writes
            .iter()
            .filter(|(package, _)| tree.reaches_host_global(package))
            .map(|(_, target)| Diagnostic::e006(file_path, *target)),
    );
    let has_global_arg_read = uses.arg_reads.iter().any(|name| !tree.resolves_to_local(name));

    let has_env_rebinding = version.has_env_upvalue()
        && (tree.is_env_assigned()
            || tree.name_id("module").is_some_and(|module| {
                tree.global_references().any(|reference| reference.name == module)
            }));
    // Lua 5.1: the only target with an upvalue limit a loader can hit.
    let max_function_upvalues = if version.has_env_upvalue() || version.is_luau() {
        0
    } else {
        max_function_upvalues(&tree)
    };
    let exports = if version.has_value_exports() {
        Exports::new(block, &tree, source)
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
    /// The function bodies the walk is in, innermost last, and whether each
    /// declares `...`, binding its own `arg` on 5.1.
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
            Var::Name(name) => self.note_arg_read(name),
            // Lua targets use package.loaded itself as the bundle cache, so
            // writes to it behave as in real Lua. Luau has no package table,
            // and its bundle cache is private.
            Var::Suffixed(_) => {
                if self.version.is_luau()
                    && let Some(package) = package_loaded_root(target, self.version)
                {
                    self.package_loaded_writes.push((package, target.span()));
                }
            }
        }
    }

    /// Lua 5.1: a read of `arg`, outside every vararg function.
    fn note_arg_read(&mut self, name: &'tree Name<'tree>) {
        if self.version.has_implicit_vararg_table()
            && name.text == "arg"
            && !self.functions.iter().any(|&(_, is_vararg)| is_vararg)
        {
            self.arg_reads.push(name);
        }
    }
}

impl<'tree> NodeSink<'tree> for Uses<'tree> {
    fn function_body(&mut self, body: &'tree FunctionBody<'tree>) {
        self.reach(body.span.start);
        self.functions.push((body.span, body.vararg.is_some()));
    }

    fn stmt(&mut self, stmt: &'tree Statement<'tree>) {
        self.reach(stmt.span().start);
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
        self.reach(expr.span().start);
        if matches!(expr, Expression::VarArg(_)) && self.functions.is_empty() {
            self.has_vararg_read = true;
        }
        if let Expression::Name(name) = expr {
            self.note_arg_read(name);
        }
    }

    fn chain(&mut self, chain: &'tree SuffixedExpression<'tree>, _is_target: bool) {
        if let Some(table) = global_table_require(chain, self.version) {
            self.requires.push(RequireUse::GlobalTable(chain, table));
        }
        for call in chain.calls() {
            if require_callee(call).is_some() {
                self.requires.push(RequireUse::Call(call));
            }
        }
    }
}

/// What the module's requires make of it, decided in source order.
struct RequireFinder<'tree, 'ast> {
    allocator: &'ast Allocator,
    source: &'ast str,
    file_path: &'tree str,
    version: LuaVersion,
    dynamic_require: DynamicRequire,
    tree: &'tree ScopeTree<'tree>,
    sites: Vec<RequireSite<'ast>>,
    dynamic_callees: Vec<Span>,
    diags: Vec<Diagnostic>,
    seen_require_strings: FxHashSet<&'ast str>,
    direct_callees: Vec<Span>,
}

impl<'ast> RequireFinder<'_, 'ast> {
    fn handle_call(&mut self, call: Call<'_>) {
        let Some(callee) = require_callee(call) else {
            return;
        };
        self.direct_callees.push(callee.span);
        if !self.tree.reaches_host_global(callee) {
            self.diags.push(Diagnostic::w005_shadowed(self.file_path, call.span()));
            return;
        }

        match self.literal_require_string(call) {
            // The real `require` finds a standard library in `package.loaded`
            // before any searcher, so no file can stand in for it.
            Some(require_string) if is_standard_library(require_string, self.version) => {}
            Some(require_string) => {
                if !self.seen_require_strings.insert(require_string) {
                    self.diags.push(Diagnostic::w001(self.file_path, call.span(), require_string));
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
                self.diags.push(Diagnostic::e002(self.file_path, call.span()));
                return;
            }
            DynamicRequire::Warn if self.version.is_luau() => {
                self.diags.push(Diagnostic::w007_runtime(self.file_path, call.span()));
            }
            DynamicRequire::Warn => {
                self.diags.push(Diagnostic::w007_loader_fallback(self.file_path, call.span()));
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

    /// The argument of `call` when it's one string literal, with escapes
    /// resolved and a long string's leading newline stripped. `None` if it
    /// doesn't decode or the result isn't UTF-8.
    fn literal_require_string(&self, call: Call<'_>) -> Option<&'ast str> {
        let literal_span = match call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                let [Expression::StringLiteral(literal)] = &**args else {
                    return None;
                };
                literal.span
            }
            FunctionArgs::StringLiteral(literal) => literal.span,
            FunctionArgs::TableConstructor(_) | FunctionArgs::Error(_) => return None,
        };
        let decoded = decode_string(literal_span.text(self.source), self.version)?;
        Some(self.allocator.alloc_str(str::from_utf8(&decoded).ok()?))
    }

    /// W005 for `_G.require` or `_ENV.require`, the global `require` reached
    /// through its table rather than a direct call.
    fn flag_global_table_require(&mut self, chain: &SuffixedExpression<'_>, table: &Name<'_>) {
        if self.tree.reaches_host_global(table) {
            self.diags.push(Diagnostic::w005_aliased(self.file_path, chain.head(1).span()));
        }
    }
}

/// Whether the real `require` serves `name` straight from `package.loaded`.
/// Luau has no `package` table, so its `require` always resolves a path.
fn is_standard_library(name: &str, version: LuaVersion) -> bool {
    !version.is_luau()
        && (name == "_G"
            || matches!(
                luck_semantic::library_for(version, StdlibEnvironment::Standalone)
                    .globals
                    .get(name),
                Some(StdlibEntry::Namespace(_))
            ))
}

/// Whether `suffix` reads the field `key`, as `.key` or `["key"]`.
fn is_key(suffix: &Suffix<'_>, key: &str, version: LuaVersion) -> bool {
    match suffix {
        Suffix::Field(name) => name.text == key,
        Suffix::Index { index: Expression::StringLiteral(literal), .. } => {
            decode_string(literal.text, version).is_some_and(|value| *value == *key.as_bytes())
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
    if !is_key(chain.suffixes.first(), "require", version) {
        return None;
    }
    let Expression::Name(table) = &chain.prefix else {
        return None;
    };
    (table.text == "_G" || (table.text == "_ENV" && version.has_env_upvalue())).then_some(table)
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
fn package_loaded_root<'target>(
    target: &'target Var<'_>,
    version: LuaVersion,
) -> Option<&'target Name<'target>> {
    let Var::Suffixed(chain) = target else {
        return None;
    };
    let Expression::Name(package) = &chain.prefix else {
        return None;
    };
    (package.text == "package" && is_key(chain.suffixes.first(), "loaded", version))
        .then_some(package)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_in<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        version: LuaVersion,
        dynamic_require: DynamicRequire,
    ) -> ExtractedRequires<'ast> {
        let parsed = luck_parser::parse(allocator, source, version);
        assert!(parsed.errors.is_empty(), "parse failed: {:?}", parsed.errors);
        extract_requires(allocator, &parsed.block, source, "test.lua", version, dynamic_require)
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
    fn duplicate_require_flags_w001() {
        let allocator = Allocator::new();
        let result = extract(
            &allocator,
            "\
local a = require(\"utils\")
local b = require(\"utils\")
",
        );
        assert_eq!(result.sites.len(), 2);
        assert_eq!(with_code(&result, "W001").len(), 1, "{:?}", result.diags);
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
}
