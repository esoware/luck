use luck_allocator::Allocator;
use luck_ast::{Chunk, GetSpan as _, TreeCounts};
use luck_diagnostics::Diagnostic;
use luck_semantic::{ScopeId, ScopeKind, Scoping, SemanticBuilder, SymbolId};
use luck_token::{LuaVersion, Span};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dynamic_require::DynamicRequire;
use crate::exports::{Exports, has_value_exports};

/// A call of the global `require` with one string literal, whose module the
/// bundle holds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StaticRequire<'ast> {
    /// The module path with escapes decoded, as real `require` receives it.
    pub require_string: &'ast str,
    pub call_span: Span,
    /// The string literal that names the module, where a failure to resolve
    /// it is reported.
    pub path_span: Span,
}

/// What bundling needs from a module.
#[derive(Debug)]
pub(crate) struct Analysis<'ast> {
    /// In source order.
    pub requires: Vec<StaticRequire<'ast>>,
    /// The callee of each call of the global `require` whose argument isn't
    /// one string literal, in source order. The emitter retargets just the
    /// callee, so a static require nested in the argument still gets its own
    /// rewrite.
    pub dynamic_callees: Vec<Span>,
    /// In source order.
    pub diags: Vec<Diagnostic>,
    /// The module's own chunk reads `...`.
    pub has_vararg_read: bool,
    /// Lua 5.1: the module reads the global `arg`.
    pub has_global_arg_read: bool,
    /// Lua 5.2+: the module assigns its `_ENV` or reads the global `module`.
    pub has_env_rebinding: bool,
    /// Lua 5.1: the most locals any one function captures. Zero elsewhere.
    pub max_function_upvalues: usize,
    pub exports: Exports<'ast>,
}

/// What bundling needs from `chunk`, whose text is `source`, read off the one
/// semantic pass that finds the calls of the global `require`, with tables
/// sized from the parse's `counts`.
///
/// A read of a name `require` that bundling leaves alone gets W005: one of
/// the global `require` that doesn't call it, since a call through what it
/// reads escapes bundling, and one of any other `require`, since a call of it
/// keeps its meaning. A standard library's name is served from
/// `package.loaded`, so it stays the host's.
///
/// `dynamic_require` decides whether a `require(expr)` aborts the bundle or
/// stays for the runtime. Each require string goes in `allocator`, since it
/// outlives the borrow of the tree that the analysis reads, which the
/// emitters move and rewrite.
pub(crate) fn analyze<'ast>(
    allocator: &'ast Allocator,
    chunk: &Chunk<'_>,
    counts: TreeCounts,
    source: &'ast [u8],
    dynamic_require: DynamicRequire,
) -> Analysis<'ast> {
    let version = chunk.version;
    let semantic =
        SemanticBuilder::new().with_counts(counts).with_requires(true).build(chunk).semantic;
    let tree = semantic.scoping();
    let mut requires = Vec::new();
    let mut dynamic_callees = Vec::new();
    let mut diags = Vec::new();
    // The call that first required each string.
    let mut first_requires: FxHashMap<&str, Span> = FxHashMap::default();
    for site in semantic.requires() {
        let call_span = site.call.span();
        match site.path.and_then(|literal| Some((literal.as_str()?, literal.span))) {
            Some((path, _)) if luck_stdlib::is_preloaded_module(path, version) => {}
            Some((path, path_span)) => {
                let first = *first_requires.entry(path).or_insert(call_span);
                if first != call_span {
                    diags.push(Diagnostic::w001(call_span, path, first));
                }
                let require_string = allocator.alloc_str(path);
                requires.push(StaticRequire { require_string, call_span, path_span });
            }
            // `require(expr)` can't be resolved statically. Aborting the whole
            // bundle over one such call is rarely what the author wants, so by
            // default it stays and resolves at runtime.
            None => {
                match dynamic_require {
                    DynamicRequire::Error => {
                        diags.push(Diagnostic::e002(call_span));
                        continue;
                    }
                    DynamicRequire::Warn if version.is_luau() => {
                        diags.push(Diagnostic::w007_runtime(call_span));
                    }
                    DynamicRequire::Warn => {
                        diags.push(Diagnostic::w007_loader_fallback(call_span));
                    }
                    DynamicRequire::Allow => {}
                }
                // Luau keys its cache by resolved file, and a Roblox require
                // takes an Instance, so a runtime argument has nothing in the
                // bundle to look up and the call stays as written.
                if !version.is_luau() {
                    dynamic_callees.push(site.call.callee.span());
                }
            }
        }
    }

    // Where each call of the global `require` by its name starts, which is
    // where the name it calls does.
    let called: Vec<u32> = semantic
        .requires()
        .iter()
        .filter_map(|site| site.call.callee.as_name())
        .map(|callee| callee.span.start)
        .collect();
    let [require, arg, module] = ["require", "arg", "module"].map(|name| tree.name_id(name));
    let (mut has_global_arg_read, mut has_global_module_read) = (false, false);
    for reference in tree.references() {
        let name = Some(reference.name);
        if name == require {
            if reference.kind.is_read() && called.binary_search(&reference.span.start).is_err() {
                diags.push(if tree.is_host_global(reference) {
                    Diagnostic::w005_aliased(reference.span)
                } else {
                    Diagnostic::w005_shadowed(reference.span)
                });
            }
            continue;
        }
        if tree.is_global(reference) {
            has_global_arg_read |= name == arg;
            has_global_module_read |= name == module;
        }
    }
    diags.sort_by_key(|diag| diag.span.map(|span| span.start));

    Analysis {
        requires,
        dynamic_callees,
        diags,
        has_vararg_read: tree.scopes().first().is_some_and(|scope| scope.has_vararg_read),
        // Lua 5.1 binds `arg` in every vararg function, so a free one is the
        // chunk's own.
        has_global_arg_read: version.has_implicit_vararg_table() && has_global_arg_read,
        has_env_rebinding: version.has_env_upvalue()
            && (tree.is_env_assigned() || has_global_module_read),
        max_function_upvalues: if counts_upvalues(version) {
            max_function_upvalues(tree)
        } else {
            0
        },
        exports: if version.has_value_exports() && has_value_exports(&chunk.block) {
            Exports::new(&chunk.block, tree, source)
        } else {
            Exports::default()
        },
    }
}

/// Lua 5.1: the only target with an upvalue limit a loader can hit.
const fn counts_upvalues(version: LuaVersion) -> bool {
    !version.has_env_upvalue() && !version.is_luau()
}

/// The most distinct outer locals any one function captures. A capture counts
/// on every function between the use and the declaring scope, as Lua's
/// compiler counts it.
fn max_function_upvalues(tree: &Scoping) -> usize {
    let mut captures: FxHashSet<(SymbolId, ScopeId)> = FxHashSet::default();
    let mut counts = vec![0; tree.scopes().len()];
    for reference in tree.references() {
        let Some(symbol) = reference.resolved else {
            continue;
        };
        let home = tree.scope(tree.symbol(symbol).scope).function;
        let mut function = tree.scope(reference.scope).function;
        while function != home {
            let scope = tree.scope(function);
            if scope.kind == ScopeKind::Function && captures.insert((symbol, function)) {
                counts[function.index()] += 1;
            }
            let Some(parent) = scope.parent else {
                break;
            };
            function = tree.scope(parent).function;
        }
    }
    counts.into_iter().max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze_in<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        version: LuaVersion,
        dynamic_require: DynamicRequire,
    ) -> Analysis<'ast> {
        let parsed = luck_parser::parse(allocator, source.as_bytes(), version);
        assert!(parsed.errors.is_empty(), "parse failed: {:?}", parsed.errors);
        analyze(allocator, &parsed.chunk, parsed.counts, source.as_bytes(), dynamic_require)
    }

    fn analyze_with<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        dynamic_require: DynamicRequire,
    ) -> Analysis<'ast> {
        analyze_in(allocator, source, LuaVersion::Lua54, dynamic_require)
    }

    fn analyze_lua54<'ast>(allocator: &'ast Allocator, source: &'ast str) -> Analysis<'ast> {
        analyze_with(allocator, source, DynamicRequire::default())
    }

    fn analyze_luau<'ast>(allocator: &'ast Allocator, source: &'ast str) -> Analysis<'ast> {
        analyze_in(allocator, source, LuaVersion::Luau, DynamicRequire::default())
    }

    fn require_strings<'ast>(analysis: &Analysis<'ast>) -> Vec<&'ast str> {
        analysis.requires.iter().map(|require| require.require_string).collect()
    }

    fn with_code<'analysis>(
        analysis: &'analysis Analysis<'_>,
        code: &str,
    ) -> Vec<&'analysis Diagnostic> {
        analysis.diags.iter().filter(|diag| diag.code == code).collect()
    }

    /// The text each diagnostic points at, in order.
    fn flagged<'source>(analysis: &Analysis<'_>, source: &'source str) -> Vec<&'source str> {
        analysis
            .diags
            .iter()
            .filter_map(|diag| diag.span)
            .map(|span| str::from_utf8(span.text(source.as_bytes())).unwrap())
            .collect()
    }

    #[test]
    fn extracts_single_require() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(
            &allocator,
            "\
local utils = require(\"utils\")
print(utils.foo())
",
        );
        assert_eq!(require_strings(&analysis), ["utils"]);
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn extracts_multiple_requires_in_order() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(
            &allocator,
            "\
local a = require(\"mod_a\")
local b = require(\"mod_b\")
local c = require(\"mod_c\")
print(a, b, c)
",
        );
        assert_eq!(require_strings(&analysis), ["mod_a", "mod_b", "mod_c"]);
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn require_after_code_is_not_flagged() {
        let allocator = Allocator::new();
        // Requires are position-independent with the lazy loader.
        let analysis = analyze_lua54(
            &allocator,
            "\
print(\"hello\")
local x = require(\"x\")
",
        );
        assert_eq!(analysis.requires.len(), 1);
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn non_literal_require_flags_e002_in_error_mode() {
        let allocator = Allocator::new();
        let analysis =
            analyze_with(&allocator, "local x = require(varname)\n", DynamicRequire::Error);
        assert!(analysis.requires.is_empty());
        assert_eq!(analysis.dynamic_callees, Vec::<Span>::new());
        assert_eq!(with_code(&analysis, "E002").len(), 1, "{:?}", analysis.diags);
    }

    /// The default: one runtime-resolved require must not abort the bundle.
    #[test]
    fn non_literal_require_warns_and_is_kept_by_default() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(&allocator, "local x = require(varname)\n");
        assert!(analysis.requires.is_empty());
        assert!(with_code(&analysis, "E002").is_empty(), "{:?}", analysis.diags);
        assert_eq!(with_code(&analysis, "W007").len(), 1, "{:?}", analysis.diags);
        // The callee token alone, so the argument expression is left intact.
        assert_eq!(analysis.dynamic_callees, [Span::new(10, 17)]);
    }

    #[test]
    fn allow_mode_keeps_the_call_without_warning() {
        let allocator = Allocator::new();
        let analysis =
            analyze_with(&allocator, "local x = require(varname)\n", DynamicRequire::Allow);
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
        assert_eq!(analysis.dynamic_callees, [Span::new(10, 17)]);
    }

    /// Luau keys its cache by resolved file and Roblox requires take an
    /// Instance, so a runtime argument has nothing to look up. The call warns
    /// but is never retargeted.
    #[test]
    fn luau_dynamic_require_is_warned_but_not_retargeted() {
        let allocator = Allocator::new();
        let analysis = analyze_luau(&allocator, "local x = require(varname)\n");
        assert_eq!(with_code(&analysis, "W007").len(), 1, "{:?}", analysis.diags);
        assert_eq!(analysis.dynamic_callees, Vec::<Span>::new());
    }

    /// A static require nested inside a dynamic one still bundles: the
    /// retargeted span is the outer callee only.
    #[test]
    fn static_require_nested_in_a_dynamic_one_is_still_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(&allocator, "local x = require(require(\"names\").first)\n");
        assert_eq!(require_strings(&analysis), ["names"]);
        assert_eq!(analysis.dynamic_callees, [Span::new(10, 17)]);
    }

    #[test]
    fn bare_require_stmt_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(&allocator, "require(\"something\")\n");
        assert_eq!(require_strings(&analysis), ["something"]);
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn escape_sequences_decode_like_real_lua() {
        let allocator = Allocator::new();
        // require("a\46b") is require("a.b") at runtime; the bundler must
        // resolve and deduplicate on the decoded value.
        let analysis = analyze_lua54(
            &allocator,
            "\
local a = require(\"a\\46b\")
local b = require(\"a.b\")
",
        );
        assert_eq!(require_strings(&analysis), ["a.b", "a.b"]);
        // Same decoded module twice: the duplicate-require warning fires.
        assert_eq!(with_code(&analysis, "W001").len(), 1, "{:?}", analysis.diags);
    }

    #[test]
    fn long_string_leading_newline_is_stripped() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(&allocator, "local m = require [[\nmymod]]\n");
        assert_eq!(require_strings(&analysis), ["mymod"]);
    }

    #[test]
    fn shadowed_require_is_skipped_with_w005() {
        let allocator = Allocator::new();
        let source = "\
local require = function(s) return s end
local x = require(\"dep\")
";
        let analysis = analyze_lua54(&allocator, source);
        assert!(
            analysis.requires.is_empty(),
            "shadowed require must not be bundled: {:?}",
            analysis.requires
        );
        assert_eq!(flagged(&analysis, source), ["require"], "{:?}", analysis.diags);
        assert_eq!(with_code(&analysis, "W005").len(), 1, "{:?}", analysis.diags);
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
        let analysis = analyze_lua54(&allocator, source);
        assert_eq!(require_strings(&analysis), ["real"]);
        assert_eq!(with_code(&analysis, "W005").len(), 1, "{:?}", analysis.diags);
    }

    /// The common `local require = require` hides the global from every
    /// call after it, which W005 says at the alias and at each call.
    #[test]
    fn a_local_alias_of_require_flags_w005_where_it_reads_and_is_read() {
        let allocator = Allocator::new();
        let source = "\
local require = require
local a, b = require(\"a\"), require
";
        let analysis = analyze_lua54(&allocator, source);
        assert!(analysis.requires.is_empty(), "{:?}", analysis.requires);
        let messages: Vec<&str> = analysis.diags.iter().map(|diag| &*diag.message).collect();
        assert_eq!(
            messages,
            [
                "global `require` is referenced without being called, so aliased call sites are \
                 not bundled",
                "this `require` is not the global `require`, so calls through it are not bundled",
                "this `require` is not the global `require`, so calls through it are not bundled",
            ]
        );
    }

    #[test]
    fn aliased_require_flags_w005() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(
            &allocator,
            "\
local r = require
local x = r(\"dep\")
",
        );
        assert!(analysis.requires.is_empty());
        assert_eq!(with_code(&analysis, "W005").len(), 1, "{:?}", analysis.diags);
    }

    #[test]
    fn direct_calls_do_not_flag_w005() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(
            &allocator,
            "\
local a = require(\"x\")
require(\"y\")
",
        );
        assert_eq!(analysis.requires.len(), 2);
        assert!(with_code(&analysis, "W005").is_empty(), "{:?}", analysis.diags);
    }

    /// `_G.require(path)` calls the global `require` as surely as
    /// `require(path)` does, so it bundles too.
    #[test]
    fn a_call_of_require_through_the_global_table_is_extracted() {
        let allocator = Allocator::new();
        let source = "\
local a = _G.require(\"a\")
local b = _ENV[\"require\"](name)
local c = _G.require
";
        let analysis = analyze_lua54(&allocator, source);
        assert_eq!(require_strings(&analysis), ["a"]);
        assert_eq!(analysis.dynamic_callees, [Span::new(36, 51)]);
        assert_eq!(flagged(&analysis, source), ["_ENV[\"require\"](name)"]);
    }

    /// Lua targets use `package.loaded` itself as the bundle cache, and
    /// Luau's `require` never reads it, so a write to it means in a bundle
    /// what it means without one.
    #[test]
    fn package_loaded_writes_are_no_concern_of_the_bundle() {
        let source = "\
package.loaded[\"mymod\"] = {}
package.loaded.other = {}
";
        for version in [LuaVersion::Lua54, LuaVersion::Luau] {
            let allocator = Allocator::new();
            let analysis = analyze_in(&allocator, source, version, DynamicRequire::default());
            assert!(analysis.diags.is_empty(), "{version:?}: {:?}", analysis.diags);
        }
    }

    #[test]
    fn duplicate_require_flags_w001_and_points_at_the_first() {
        let allocator = Allocator::new();
        let source = "\
local a = require(\"utils\")
local b = require(\"utils\")
";
        let analysis = analyze_lua54(&allocator, source);
        assert_eq!(analysis.requires.len(), 2);
        let [warning] = with_code(&analysis, "W001")[..] else {
            panic!("{:?}", analysis.diags);
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
        let analysis = analyze_lua54(
            &allocator,
            "\
local modname = ...
return modname
",
        );
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
        assert!(analysis.has_vararg_read);
    }

    #[test]
    fn a_vararg_read_inside_a_function_is_not_the_chunks() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(&allocator, "return function(...) return ... end\n");
        assert!(!analysis.has_vararg_read);
    }

    #[test]
    fn lua51_global_arg_read_ignores_a_vararg_functions_own_arg() {
        let analyze_arg_read = |source: &'static str| {
            analyze_in(&Allocator::new(), source, LuaVersion::Lua51, DynamicRequire::default())
                .has_global_arg_read
        };
        assert!(analyze_arg_read("return arg\n"));
        assert!(analyze_arg_read(
            "\
local function f() return arg end
return f
"
        ));
        assert!(!analyze_arg_read(
            "\
local function f(...) return arg end
return f
"
        ));
        assert!(!analyze_arg_read(
            "\
local function f(...) return function() return arg end end
return f
"
        ));
        assert!(!analyze_arg_read(
            "\
local arg = {}
return arg
"
        ));
    }

    #[test]
    fn no_requires_yields_empty_result() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(&allocator, "print(\"hello world\")\n");
        assert!(analysis.requires.is_empty());
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn require_string_call_syntax_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(&allocator, "local m = require \"mymod\"\n");
        assert_eq!(require_strings(&analysis), ["mymod"]);
    }

    #[test]
    fn single_quoted_require_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(&allocator, "local m = require('mymod')\n");
        assert_eq!(require_strings(&analysis), ["mymod"]);
    }

    #[test]
    fn long_bracket_require_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(&allocator, "local m = require [[mymod]]\n");
        assert_eq!(require_strings(&analysis), ["mymod"]);
    }

    #[test]
    fn require_in_multi_name_local_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(&allocator, "local a, b = require(\"x\"), 1\n");
        assert_eq!(require_strings(&analysis), ["x"]);
    }

    #[test]
    fn require_in_nested_function_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(
            &allocator,
            "\
local function setup()
    local m = require(\"inner\")
end
",
        );
        assert_eq!(require_strings(&analysis), ["inner"]);
    }

    #[test]
    fn field_function_named_require_is_ignored() {
        let allocator = Allocator::new();
        let analysis = analyze_lua54(
            &allocator,
            "\
local obj = {}
function obj.require(s) end
obj.require(\"x\")
",
        );
        assert!(analysis.requires.is_empty());
        assert!(with_code(&analysis, "W005").is_empty(), "{:?}", analysis.diags);
    }
}
